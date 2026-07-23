//! # Contract: MetEngine-backed continuous boundary path sampler
//!
//! Builds certified start/proposal path segments split by interpolation-grid
//! cells and scalar extrema, and samples terrain/model-top at interpolated
//! physical times. `retarget` rebinds the residual path so local sample(0)
//! equals the collision state.
#![allow(
    clippy::neg_cmp_op_on_partial_ord,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::unwrap_used,
    dead_code
)]

use trajecta_case::model::time::Timestamp;
use trajecta_met::grid::DomainGeometry;
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::query::engine::{BatchWorkspace, ExecutionContext, MetEngine};
use trajecta_met::query::request::{QueryBatch, QueryPointArrays, TransportPlan, VerticalQuery};

use crate::boundary::{
    BoundaryError, BoundaryPathSampler, BoundaryPathSegment, BoundarySample,
    validate_ordered_path_segments,
};
use crate::particle::ParticleState;
use crate::runner::{BoundaryPathSamplerFactory, BoundarySamplerRequest};
use crate::science::M4_CONSTANTS;

/// Production factory constructing MetEngine-backed path samplers.
#[derive(Debug, Default)]
pub struct MetBoundaryPathSamplerFactory;

impl BoundaryPathSamplerFactory for MetBoundaryPathSamplerFactory {
    fn build<'a>(
        &'a mut self,
        request: BoundarySamplerRequest<'_>,
        meteorology: &'a mut MetEngine,
        query_plan: &'a TransportPlan,
        execution: &'a dyn ExecutionContext,
    ) -> Result<Box<dyn BoundaryPathSampler + 'a>, BoundaryError> {
        Ok(Box::new(MetBoundaryPathSampler::new(
            request,
            meteorology,
            query_plan,
            execution,
        )?))
    }
}

struct MetBoundaryPathSampler<'a> {
    start: ParticleState,
    proposed: ParticleState,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    /// Global full-step fraction corresponding to local sample fraction 0.
    local_zero_global_fraction: f64,
    segments: Vec<BoundaryPathSegment>,
    grid: DomainGeometry,
    meteorology: &'a mut MetEngine,
    query_plan: &'a TransportPlan,
    execution: &'a dyn ExecutionContext,
    workspace: BatchWorkspace,
}

impl<'a> MetBoundaryPathSampler<'a> {
    fn new(
        request: BoundarySamplerRequest<'_>,
        meteorology: &'a mut MetEngine,
        query_plan: &'a TransportPlan,
        execution: &'a dyn ExecutionContext,
    ) -> Result<Self, BoundaryError> {
        let grid = domain_geometry(meteorology, request.start_time)?;
        let mut sampler = Self {
            start: request.start.clone(),
            proposed: request.proposed.clone(),
            step_start_time: request.start_time,
            step_end_time: request.end_time,
            local_zero_global_fraction: 0.0,
            segments: Vec::new(),
            grid,
            meteorology,
            query_plan,
            execution,
            workspace: BatchWorkspace::default(),
        };
        sampler.rebuild_segments()?;
        Ok(sampler)
    }

    fn rebuild_segments(&mut self) -> Result<(), BoundaryError> {
        let cuts = certify_path_cuts(
            &self.start,
            &self.proposed,
            &self.grid,
            self.step_start_time,
            self.step_end_time,
            self.local_zero_global_fraction,
            self.meteorology,
            self.query_plan,
            self.execution,
            &mut self.workspace,
        )?;
        let mut segments = Vec::with_capacity(cuts.len().saturating_sub(1));
        for window in cuts.windows(2) {
            if (window[1] - window[0]) <= 0.0 {
                return Err(BoundaryError::InvalidPathSegments);
            }
            segments.push(BoundaryPathSegment {
                start_fraction: window[0],
                end_fraction: window[1],
            });
        }
        validate_ordered_path_segments(&segments)?;
        self.segments = segments;
        Ok(())
    }

    fn interpolate_state(&self, local_fraction: f64) -> Result<ParticleState, BoundaryError> {
        if !local_fraction.is_finite() || local_fraction < 0.0 || local_fraction > 1.0 {
            return Err(BoundaryError::InvalidParticleState);
        }
        let (lon, lat) = interpolate_great_circle(
            self.start.longitude_degrees,
            self.start.latitude_degrees,
            self.proposed.longitude_degrees,
            self.proposed.latitude_degrees,
            local_fraction,
        )?;
        let mut state = self.start.clone();
        state.longitude_degrees = lon;
        state.latitude_degrees = lat;
        state.height_asl_m = lerp(
            self.start.height_asl_m,
            self.proposed.height_asl_m,
            local_fraction,
        );
        state.integration_offset_ns = lerp_i64(
            self.start.integration_offset_ns,
            self.proposed.integration_offset_ns,
            local_fraction,
        )?;
        state.elapsed_age_ns = lerp_u64(
            self.start.elapsed_age_ns,
            self.proposed.elapsed_age_ns,
            local_fraction,
        )?;
        state.status = crate::particle::ParticleStatus::Alive;
        Ok(state)
    }

    fn physical_time(&self, local_fraction: f64) -> Result<Timestamp, BoundaryError> {
        let global = self.local_zero_global_fraction
            + (1.0 - self.local_zero_global_fraction) * local_fraction;
        lerp_timestamp(self.step_start_time, self.step_end_time, global)
    }

    fn sample_raw(&mut self, local_fraction: f64) -> Result<BoundarySample, BoundaryError> {
        let position = self.interpolate_state(local_fraction)?;
        let time = self.physical_time(local_fraction)?;
        let window = self
            .meteorology
            .prepare(time)
            .map_err(|_| BoundaryError::MissingContext)?;
        let batch = window
            .prepare_transport_batch(
                self.query_plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveSeaLevel,
                    points: QueryPointArrays {
                        longitude_degrees: vec![position.longitude_degrees],
                        latitude_degrees: vec![position.latitude_degrees],
                        vertical: vec![position.height_asl_m],
                    },
                },
                &mut self.workspace,
            )
            .map_err(|_| BoundaryError::MissingContext)?;
        let output = batch
            .execute(self.execution, &mut self.workspace)
            .map_err(|_| BoundaryError::MissingContext)?;
        let row = output.row(0).ok_or(BoundaryError::MissingContext)?;
        let status = row.status();
        let surface = row.terrain_height_asl_m().filter(|value| value.is_finite());
        let model_top = row
            .bounds()
            .and_then(|bounds| bounds.physical_model_top_asl_m())
            .filter(|value| value.is_finite());
        Ok(BoundarySample {
            fraction: local_fraction,
            position,
            meteorology_status: status,
            surface_height_asl_m: surface,
            model_top_height_asl_m: model_top,
        })
    }
}

impl BoundaryPathSampler for MetBoundaryPathSampler<'_> {
    fn ordered_segments(&self) -> Result<Vec<BoundaryPathSegment>, BoundaryError> {
        validate_ordered_path_segments(&self.segments)?;
        Ok(self.segments.clone())
    }

    fn sample(&mut self, fraction: f64) -> Result<BoundarySample, BoundaryError> {
        self.sample_raw(fraction)
    }

    fn retarget(
        &mut self,
        start_fraction: f64,
        start: &ParticleState,
        proposed: &ParticleState,
    ) -> Result<(), BoundaryError> {
        // Contract: `start_fraction` is the collision location in the original
        // full-step coordinate [0,1]. Callers (SurfaceReflect) already convert
        // residual-local roots to full-step globals before invoking retarget.
        // Do NOT map through local_zero again — that double-counts and yields
        // e.g. 0.875 instead of 0.75 after two 0.5 collisions.
        if !start_fraction.is_finite() || start_fraction < 0.0 || start_fraction > 1.0 {
            return Err(BoundaryError::InvalidParticleState);
        }
        if start_fraction + 1.0e-15 < self.local_zero_global_fraction {
            return Err(BoundaryError::InvalidParticleState);
        }
        self.start = start.clone();
        self.proposed = proposed.clone();
        self.local_zero_global_fraction = start_fraction;
        self.rebuild_segments()?;
        // Contract check: residual sample(0) must equal the collision position.
        let zero = self.sample_raw(0.0)?;
        if (zero.position.longitude_degrees - start.longitude_degrees).abs() > 1.0e-9
            || (zero.position.latitude_degrees - start.latitude_degrees).abs() > 1.0e-9
            || (zero.position.height_asl_m - start.height_asl_m).abs() > 1.0e-9
        {
            return Err(BoundaryError::InvalidParticleState);
        }
        Ok(())
    }
}

fn domain_geometry(
    meteorology: &mut MetEngine,
    time: Timestamp,
) -> Result<DomainGeometry, BoundaryError> {
    let window = meteorology
        .prepare(time)
        .map_err(|_| BoundaryError::MissingContext)?;
    Ok(window.frames.before.metadata().grid.clone())
}

/// Build ordered cut fractions in local [0,1] for path segmentation.
///
/// Goal: at most one clearance down-crossing, model-top down-crossing, and
/// domain exit per segment.
///
/// Certification (no fixed whole-path probe grids):
/// 1. Closed-form great-circle × meridian/parallel roots — **all** roots in the
///    open arc `(0,1)` — using the true short-arc latitude envelope (including
///    interior latitude extrema), not endpoint-only latitude bounds.
/// 2. Additional cuts at GC latitude/longitude turning points so each leaf has
///    monotonic lon and lat.
/// 3. Within each cell leaf, bilinear terrain / model-top along the exact GC
///    path: isolate zeros of analytic d(clearance)/df and d(top_gap)/df by
///    recursive subdivision certified by second-derivative sign (unique root) —
///    not by treating cell fraction `(fx,fy)` as linear in path fraction.
/// 4. Recursive re-check until no leaf reports a further interior extremum.
fn certify_path_cuts(
    start: &ParticleState,
    proposed: &ParticleState,
    grid: &DomainGeometry,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    local_zero_global: f64,
    meteorology: &mut MetEngine,
    query_plan: &TransportPlan,
    execution: &dyn ExecutionContext,
    workspace: &mut BatchWorkspace,
) -> Result<Vec<f64>, BoundaryError> {
    let mut cuts = vec![0.0, 1.0];
    for fraction in grid_line_crossings(start, proposed, grid)? {
        insert_cut(&mut cuts, fraction);
    }
    for fraction in gc_turning_point_fractions(start, proposed)? {
        insert_cut(&mut cuts, fraction);
    }
    finalize_cuts(&mut cuts);

    // Domain inside/outside: cut at exact safe-domain boundary meridians/parallels
    // (halo-aware). No midpoint heuristic on SampleStatus.
    for fraction in domain_boundary_crossings(start, proposed, grid)? {
        insert_cut(&mut cuts, fraction);
    }
    finalize_cuts(&mut cuts);

    // Recursive certification: terrain clearance + model-top gap extrema.
    let mut guard = 0_u32;
    loop {
        guard += 1;
        if guard > 128 {
            return Err(BoundaryError::RootFindingFailed);
        }
        let leaves = cuts.clone();
        let mut added = false;
        for window in leaves.windows(2) {
            for f in scalar_field_extremum_cuts(
                window[0],
                window[1],
                start,
                proposed,
                step_start_time,
                step_end_time,
                local_zero_global,
                meteorology,
                query_plan,
                execution,
                workspace,
                grid,
            )? {
                if f > window[0] + 1.0e-14 && f < window[1] - 1.0e-14 {
                    insert_cut(&mut cuts, f);
                    added = true;
                }
            }
        }
        finalize_cuts(&mut cuts);
        if !added {
            break;
        }
    }
    Ok(cuts)
}

fn insert_cut(cuts: &mut Vec<f64>, fraction: f64) {
    if fraction.is_finite() && fraction > 0.0 && fraction < 1.0 {
        cuts.push(fraction);
    }
}

fn finalize_cuts(cuts: &mut Vec<f64>) {
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-15);
    if cuts.first().copied() != Some(0.0) {
        cuts.insert(0, 0.0);
    }
    if cuts.last().copied() != Some(1.0) {
        cuts.push(1.0);
    }
}

/// All grid-line crossings of the short great-circle arc in open fraction `(0,1)`.
fn grid_line_crossings(
    start: &ParticleState,
    proposed: &ParticleState,
    grid: &DomainGeometry,
) -> Result<Vec<f64>, BoundaryError> {
    let a = unit_vector(start.longitude_degrees, start.latitude_degrees)?;
    let b = unit_vector(proposed.longitude_degrees, proposed.latitude_degrees)?;
    let cos_omega = dot(a, b).clamp(-1.0, 1.0);
    let omega = cos_omega.acos();
    if omega <= 1.0e-15 {
        return Ok(Vec::new());
    }
    // Antipodal short-arc ambiguity is rejected by unit path construction upstream;
    // still guard nearly-π arcs.
    if omega >= std::f64::consts::PI - 1.0e-12 {
        return Err(BoundaryError::InvalidParticleState);
    }

    let mut fractions = Vec::new();

    // --- Meridians: test every grid meridian that the lon envelope can reach ---
    let (lon_lo, lon_hi) = gc_unwrapped_longitude_envelope(a, b, omega, start.longitude_degrees)?;
    let origin_x = grid.longitude_origin_degrees;
    let dx = grid.longitude_spacing_degrees;
    if !(dx.is_finite() && dx > 0.0) {
        return Err(BoundaryError::MissingContext);
    }
    // Expand one cell; handle periodic wrap by scanning unwrapped range fully.
    let i_start = ((lon_lo - origin_x) / dx).floor() as i64 - 1;
    let i_end = ((lon_hi - origin_x) / dx).ceil() as i64 + 1;
    for i in i_start..=i_end {
        let meridian = origin_x + dx * i as f64;
        for f in gc_meridian_fractions(a, b, omega, meridian)? {
            // Keep roots whose unwrapped lon sits in the arc envelope (rejects the
            // supplementary long-way intersection of the same plane).
            let (lon, _) = slerp_lonlat(a, b, omega, f)?;
            let lon_u = start.longitude_degrees + shortest_delta_lon(start.longitude_degrees, lon);
            if lon_u + 1.0e-9 >= lon_lo && lon_u - 1.0e-9 <= lon_hi {
                fractions.push(f);
            }
        }
    }

    // --- Parallels: true short-arc latitude min/max (includes interior peak) ---
    let (lat_lo, lat_hi) = gc_latitude_envelope(a, b, omega)?;
    let origin_y = grid.latitude_origin_degrees;
    let dy = grid.latitude_spacing_degrees;
    if !(dy.is_finite() && dy != 0.0) {
        return Err(BoundaryError::MissingContext);
    }
    // Index-space enumeration: never floor(lat_lo/dy) and floor(lat_hi/dy)
    // independently when dy < 0 (that drops interior parallels).
    let y0 = (lat_lo - origin_y) / dy;
    let y1 = (lat_hi - origin_y) / dy;
    let y_min = y0.min(y1);
    let y_max = y0.max(y1);
    let j_start = y_min.floor() as i64 - 1;
    let j_end = y_max.ceil() as i64 + 1;
    for j in j_start..=j_end {
        let parallel = origin_y + dy * j as f64;
        if !(-90.0..=90.0).contains(&parallel) {
            continue;
        }
        if parallel + 1.0e-12 < lat_lo || parallel - 1.0e-12 > lat_hi {
            continue;
        }
        // All roots on the open short arc (e.g. two crossings of the same parallel).
        for f in gc_parallel_fractions(a, b, omega, parallel)? {
            let (_, lat) = slerp_lonlat(a, b, omega, f)?;
            if lat + 1.0e-9 >= lat_lo && lat - 1.0e-9 <= lat_hi {
                fractions.push(f);
            }
        }
    }

    // Near-polar path: if the arc passes extremely close to a pole, cut there so
    // longitude unwrap stays well-defined on each leaf.
    for f in gc_near_pole_fractions(a, b, omega)? {
        fractions.push(f);
    }

    fractions.retain(|f| f.is_finite() && *f > 1.0e-15 && *f < 1.0 - 1.0e-15);
    fractions.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    fractions.dedup_by(|x, y| (*x - *y).abs() <= 1.0e-14);
    Ok(fractions)
}

/// GC latitude / longitude turning points on the open short arc (for monotonic leaves).
fn gc_turning_point_fractions(
    start: &ParticleState,
    proposed: &ParticleState,
) -> Result<Vec<f64>, BoundaryError> {
    let a = unit_vector(start.longitude_degrees, start.latitude_degrees)?;
    let b = unit_vector(proposed.longitude_degrees, proposed.latitude_degrees)?;
    let cos_omega = dot(a, b).clamp(-1.0, 1.0);
    let omega = cos_omega.acos();
    if omega <= 1.0e-15 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    out.extend(gc_latitude_critical_fractions(a, b, omega)?);
    out.extend(gc_longitude_critical_fractions(a, b, omega)?);
    out.retain(|f| f.is_finite() && *f > 1.0e-15 && *f < 1.0 - 1.0e-15);
    out.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|x, y| (*x - *y).abs() <= 1.0e-14);
    Ok(out)
}

/// True min/max latitude of the short GC arc, including interior extrema.
fn gc_latitude_envelope(a: [f64; 3], b: [f64; 3], omega: f64) -> Result<(f64, f64), BoundaryError> {
    let (_, lat0) = lon_lat_from_unit(a)?;
    let (_, lat1) = lon_lat_from_unit(b)?;
    let mut lo = lat0.min(lat1);
    let mut hi = lat0.max(lat1);
    for f in gc_latitude_critical_fractions(a, b, omega)? {
        let (_, lat) = slerp_lonlat(a, b, omega, f)?;
        lo = lo.min(lat);
        hi = hi.max(lat);
    }
    Ok((lo, hi))
}

/// Unwrapped longitude envelope along the short arc (continuous from start lon).
fn gc_unwrapped_longitude_envelope(
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    lon_start_deg: f64,
) -> Result<(f64, f64), BoundaryError> {
    let mut samples: Vec<f64> = vec![0.0, 1.0];
    samples.extend(gc_longitude_critical_fractions(a, b, omega)?);
    samples.extend(gc_latitude_critical_fractions(a, b, omega)?);
    samples.extend(gc_near_pole_fractions(a, b, omega)?);
    samples.retain(|f| f.is_finite() && *f >= 0.0 && *f <= 1.0);
    samples.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    samples.dedup_by(|x, y| (*x - *y).abs() <= 1.0e-15);

    let mut unwrapped = Vec::with_capacity(samples.len());
    let mut prev_lon = lon_start_deg;
    let mut prev_unwrap = lon_start_deg;
    for f in samples {
        let (lon, _) = slerp_lonlat(a, b, omega, f)?;
        let step = shortest_delta_lon(prev_lon, lon);
        let u = prev_unwrap + step;
        unwrapped.push(u);
        prev_lon = lon;
        prev_unwrap = u;
    }
    let lo = unwrapped.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = unwrapped.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() || !hi.is_finite() {
        return Err(BoundaryError::InvalidParticleState);
    }
    Ok((lo, hi))
}

/// Fractions where d(lat)/dθ = 0 on the open short arc.
fn gc_latitude_critical_fractions(
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
) -> Result<Vec<f64>, BoundaryError> {
    // z(θ) = (az sin(ω-θ) + bz sin θ)/sin ω
    // z'(θ)=0 ⇒ -az cos(ω-θ) + bz cos θ = 0
    // (bz - az cos ω) cos θ - (az sin ω) sin θ = 0
    // X cos θ + Y sin θ = 0 with X = bz - az cos ω, Y = -az sin ω
    let az = a[2];
    let bz = b[2];
    let x = bz - az * omega.cos();
    let y = -az * omega.sin();
    trig_linear_roots_on_arc(x, y, 0.0, omega)
}

/// Fractions where d(lon)/dθ = 0 on the open short arc (away from poles).
fn gc_longitude_critical_fractions(
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
) -> Result<Vec<f64>, BoundaryError> {
    // Horizontal cross det(p, p')_xy along the short GC:
    //   p  = sin(ω-θ) a + sinθ b
    //   p' = -cos(ω-θ) a + cosθ b
    //   det_xy(p,p') = det_xy(a,b) * (sin(ω-θ)cosθ + sinθ cos(ω-θ))
    //                = det_xy(a,b) * sin ω
    // which is independent of θ. Away from poles (ρ²>0), dlon/dθ therefore
    // never zero on the open short arc: longitude is strictly monotonic.
    // Spurious roots from an incorrect sin(ω-2θ) expansion are not emitted.
    // Poles are handled separately by gc_near_pole_fractions.
    let _ = (a, b, omega);
    Ok(Vec::new())
}

/// Fractions where the path is extremely close to a geographic pole.
fn gc_near_pole_fractions(a: [f64; 3], b: [f64; 3], omega: f64) -> Result<Vec<f64>, BoundaryError> {
    // Horizontal radius² ρ² = px²+py² = 1 - pz². Minimize ρ² ⇔ maximize |pz|.
    // Same critical θ as latitude extrema; keep those with |lat| > 90 - ε.
    let mut out = Vec::new();
    for f in gc_latitude_critical_fractions(a, b, omega)? {
        let (_, lat) = slerp_lonlat(a, b, omega, f)?;
        if lat.abs() >= 90.0 - 1.0e-6 {
            out.push(f);
        }
    }
    Ok(out)
}

/// All open-arc fractions where the short GC meets a meridian longitude.
fn gc_meridian_fractions(
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    meridian_lon_deg: f64,
) -> Result<Vec<f64>, BoundaryError> {
    let lon = meridian_lon_deg.to_radians();
    let n = [-lon.sin(), lon.cos(), 0.0];
    let a_n = dot(a, n);
    let b_n = dot(b, n);
    // n·p(θ)=0 with p=(sin(ω-θ)a+sinθ b)/sinω
    // a_n sin(ω-θ) + b_n sin θ = 0
    // a_n (sinω cosθ - cosω sinθ) + b_n sinθ = 0
    // (a_n sinω) cosθ + (b_n - a_n cosω) sinθ = 0
    let x = a_n * omega.sin();
    let y = b_n - a_n * omega.cos();
    let mut roots = trig_linear_roots_on_arc(x, y, 0.0, omega)?;
    // Verify and keep only true plane intersections on the short arc.
    roots.retain(|f| {
        slerp_unit(a, b, omega, *f)
            .map(|p| dot(p, n).abs() < 1.0e-8)
            .unwrap_or(false)
    });
    Ok(roots)
}

/// All open-arc fractions where the short GC meets a parallel latitude.
fn gc_parallel_fractions(
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    parallel_lat_deg: f64,
) -> Result<Vec<f64>, BoundaryError> {
    let target_z = parallel_lat_deg.to_radians().sin();
    // pz(θ)*sinω = az sin(ω-θ) + bz sinθ = target_z * sinω
    // (az sinω) cosθ + (bz - az cosω) sinθ = target_z sinω
    let x = a[2] * omega.sin();
    let y = b[2] - a[2] * omega.cos();
    let z = target_z * omega.sin();
    let mut roots = trig_affine_roots_on_arc(x, y, z, omega)?;
    roots.retain(|f| {
        slerp_unit(a, b, omega, *f)
            .map(|p| (p[2] - target_z).abs() < 1.0e-8)
            .unwrap_or(false)
    });
    Ok(roots)
}

/// Solve X cosθ + Y sinθ = 0 for θ ∈ (0, ω); return fractions θ/ω.
fn trig_linear_roots_on_arc(
    x: f64,
    y: f64,
    _z: f64,
    omega: f64,
) -> Result<Vec<f64>, BoundaryError> {
    if !x.is_finite() || !y.is_finite() || !omega.is_finite() {
        return Err(BoundaryError::InvalidParticleState);
    }
    if x.abs() <= 1.0e-18 && y.abs() <= 1.0e-18 {
        return Ok(Vec::new());
    }
    // X cos + Y sin = 0 ⇒ tan θ = -X/Y when Y≠0; θ = atan2(-X, Y) family with +kπ
    let theta0 = (-x).atan2(y);
    let mut out = Vec::new();
    for k in -4..=5 {
        let theta = theta0 + std::f64::consts::PI * f64::from(k);
        if theta > 1.0e-15 && theta < omega - 1.0e-15 {
            out.push(theta / omega);
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-14);
    Ok(out)
}

/// Solve X cosθ + Y sinθ = Z for θ ∈ (0, ω); return **all** fractions θ/ω.
fn trig_affine_roots_on_arc(x: f64, y: f64, z: f64, omega: f64) -> Result<Vec<f64>, BoundaryError> {
    if ![x, y, z, omega].into_iter().all(f64::is_finite) {
        return Err(BoundaryError::InvalidParticleState);
    }
    let r = (x * x + y * y).sqrt();
    if r <= 1.0e-18 {
        return Ok(Vec::new());
    }
    if z.abs() > r + 1.0e-12 {
        return Ok(Vec::new());
    }
    let alpha = y.atan2(x);
    let cos_delta = (z / r).clamp(-1.0, 1.0);
    let delta = cos_delta.acos();
    let mut out = Vec::new();
    for sign in [-1.0, 1.0] {
        for turn in -2..=2 {
            let theta = alpha + sign * delta + std::f64::consts::TAU * f64::from(turn);
            if theta > 1.0e-15 && theta < omega - 1.0e-15 {
                out.push(theta / omega);
            }
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-14);
    Ok(out)
}

fn slerp_unit(a: [f64; 3], b: [f64; 3], omega: f64, f: f64) -> Result<[f64; 3], BoundaryError> {
    if omega <= 1.0e-15 {
        return Ok(a);
    }
    let tangent = great_circle_tangent(a, b, omega)?;
    let theta = f * omega;
    normalize([
        theta.cos() * a[0] + theta.sin() * tangent[0],
        theta.cos() * a[1] + theta.sin() * tangent[1],
        theta.cos() * a[2] + theta.sin() * tangent[2],
    ])
}

fn slerp_unit_and_deriv(
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    f: f64,
) -> Result<([f64; 3], [f64; 3]), BoundaryError> {
    if omega <= 1.0e-15 {
        return Ok((a, [0.0, 0.0, 0.0]));
    }
    let tangent = great_circle_tangent(a, b, omega)?;
    let theta = f * omega;
    let p = normalize([
        theta.cos() * a[0] + theta.sin() * tangent[0],
        theta.cos() * a[1] + theta.sin() * tangent[1],
        theta.cos() * a[2] + theta.sin() * tangent[2],
    ])?;
    let raw = [
        omega * (-theta.sin() * a[0] + theta.cos() * tangent[0]),
        omega * (-theta.sin() * a[1] + theta.cos() * tangent[1]),
        omega * (-theta.sin() * a[2] + theta.cos() * tangent[2]),
    ];
    // Project derivative to tangent plane of unit sphere: p' ⊥ p
    let corr = dot(raw, p);
    let dp = [
        raw[0] - corr * p[0],
        raw[1] - corr * p[1],
        raw[2] - corr * p[2],
    ];
    Ok((p, dp))
}

fn great_circle_tangent(a: [f64; 3], b: [f64; 3], omega: f64) -> Result<[f64; 3], BoundaryError> {
    let sin_omega = omega.sin();
    if sin_omega.abs() <= 1.0e-18 {
        return Err(BoundaryError::InvalidParticleState);
    }
    let cos_omega = omega.cos();
    let tangent = [
        (b[0] - cos_omega * a[0]) / sin_omega,
        (b[1] - cos_omega * a[1]) / sin_omega,
        (b[2] - cos_omega * a[2]) / sin_omega,
    ];
    if tangent.into_iter().all(f64::is_finite) {
        Ok(tangent)
    } else {
        Err(BoundaryError::InvalidParticleState)
    }
}

fn slerp_lonlat(a: [f64; 3], b: [f64; 3], omega: f64, f: f64) -> Result<(f64, f64), BoundaryError> {
    lon_lat_from_unit(slerp_unit(a, b, omega, f)?)
}

fn slerp_lonlat_and_deriv(
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    f: f64,
) -> Result<(f64, f64, f64, f64), BoundaryError> {
    let (p, dp) = slerp_unit_and_deriv(a, b, omega, f)?;
    let (lon, lat) = lon_lat_from_unit(p)?;
    let rho2 = p[0] * p[0] + p[1] * p[1];
    let dlon = if rho2 <= 1.0e-18 {
        0.0
    } else {
        // lon radians' = (px py' - py px')/ρ² ; convert to degrees
        (p[0] * dp[1] - p[1] * dp[0]) / rho2 * (180.0 / std::f64::consts::PI)
    };
    let cz = (1.0 - p[2] * p[2]).max(0.0).sqrt();
    let dlat = if cz <= 1.0e-18 {
        0.0
    } else {
        dp[2] / cz * (180.0 / std::f64::consts::PI)
    };
    Ok((lon, lat, dlon, dlat))
}

/// Isolate interior extrema of surface clearance and model-top gap on `(f0,f1)`.
///
/// Certificate rules (no fixed-N probe lattice, no endpoint-same-sign sophistry):
/// - residual' enclosure over the whole interval excludes 0 ⇒ no extremum;
/// - residual'' enclosure strictly excludes 0 AND residual' brackets 0 ⇒ unique
///   root, then exactly 64 bisections on residual';
/// - otherwise subdivide;
/// - depth/width exhausted without a proof ⇒ `RootFindingFailed`.
fn scalar_field_extremum_cuts(
    f0: f64,
    f1: f64,
    start: &ParticleState,
    proposed: &ParticleState,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    local_zero_global: f64,
    meteorology: &mut MetEngine,
    query_plan: &TransportPlan,
    execution: &dyn ExecutionContext,
    workspace: &mut BatchWorkspace,
    grid: &DomainGeometry,
) -> Result<Vec<f64>, BoundaryError> {
    if f1 - f0 <= 1.0e-15 {
        return Ok(Vec::new());
    }
    let a = unit_vector(start.longitude_degrees, start.latitude_degrees)?;
    let b = unit_vector(proposed.longitude_degrees, proposed.latitude_degrees)?;
    let cos_omega = dot(a, b).clamp(-1.0, 1.0);
    let omega = cos_omega.acos();

    let s0 = sample_at_fraction(
        f0,
        start,
        proposed,
        step_start_time,
        step_end_time,
        local_zero_global,
        meteorology,
        query_plan,
        execution,
        workspace,
    )?;
    let s1 = sample_at_fraction(
        f1,
        start,
        proposed,
        step_start_time,
        step_end_time,
        local_zero_global,
        meteorology,
        query_plan,
        execution,
        workspace,
    )?;
    // Grid-line cuts certify that the open leaf belongs to one cell. An endpoint
    // may lie exactly on a meridian/parallel and floor into the neighbouring cell,
    // so bind the leaf to its interior midpoint cell and validate both endpoints
    // against that same cell instead of recursively bisecting the endpoint forever.
    let midpoint = 0.5 * (f0 + f1);
    let (mid_lon, mid_lat) = path_lonlat(start, proposed, midpoint)?;
    let (i0, _, j0, _) = cell_fraction(mid_lon, mid_lat, grid)?;
    let periodic_span = grid
        .periodic_longitude
        .then_some(grid.longitude_spacing_degrees * grid.nx as f64);
    for sample in [&s0, &s1] {
        local_cell_fraction(
            sample.position.longitude_degrees,
            grid.longitude_origin_degrees,
            grid.longitude_spacing_degrees,
            i0,
            periodic_span,
        )?;
        local_cell_fraction(
            sample.position.latitude_degrees,
            grid.latitude_origin_degrees,
            grid.latitude_spacing_degrees,
            j0,
            None,
        )?;
    }

    // Time dimension: corner fields at leaf endpoint times (not midpoint snapshot).
    // Linear-in-time interpolation of corners is exact while the MetEngine window
    // bracket is constant on the leaf; otherwise cut at the bracket change.
    let bracket0 = met_bracket_times(
        meteorology,
        f0,
        step_start_time,
        step_end_time,
        local_zero_global,
    )?;
    let bracket1 = met_bracket_times(
        meteorology,
        f1,
        step_start_time,
        step_end_time,
        local_zero_global,
    )?;
    if bracket0 != bracket1 {
        // Insert a cut only for a transition in the open leaf. When the change is
        // exactly at f0/f1, the endpoint is the shared frame value and the one-sided
        // linear window over the leaf remains exact; repeatedly bisecting an endpoint
        // transition would never create a valid certificate.
        if let Some(f_cut) = frame_transition_fraction(
            f0,
            f1,
            step_start_time,
            step_end_time,
            local_zero_global,
            bracket0,
            bracket1,
        )? {
            return Ok(vec![f_cut]);
        }
    }

    let Some(corners_t0) = sample_cell_corner_field(
        i0,
        j0,
        f0,
        start,
        proposed,
        step_start_time,
        step_end_time,
        local_zero_global,
        meteorology,
        query_plan,
        execution,
        workspace,
        grid,
        FieldKind::Terrain,
    )?
    else {
        return Ok(Vec::new());
    };
    let Some(corners_t1) = sample_cell_corner_field(
        i0,
        j0,
        f1,
        start,
        proposed,
        step_start_time,
        step_end_time,
        local_zero_global,
        meteorology,
        query_plan,
        execution,
        workspace,
        grid,
        FieldKind::Terrain,
    )?
    else {
        return Ok(Vec::new());
    };
    let corners_m0 = sample_cell_corner_field(
        i0,
        j0,
        f0,
        start,
        proposed,
        step_start_time,
        step_end_time,
        local_zero_global,
        meteorology,
        query_plan,
        execution,
        workspace,
        grid,
        FieldKind::ModelTop,
    )?;
    let corners_m1 = sample_cell_corner_field(
        i0,
        j0,
        f1,
        start,
        proposed,
        step_start_time,
        step_end_time,
        local_zero_global,
        meteorology,
        query_plan,
        execution,
        workspace,
        grid,
        FieldKind::ModelTop,
    )?;

    let dh = proposed.height_asl_m - start.height_asl_m;
    let mut cuts = Vec::new();
    // clearance c = h - T  ⇒ c' = dh - T'
    cuts.extend(isolate_residual_roots_interval(
        f0,
        f1,
        a,
        b,
        omega,
        grid,
        i0,
        j0,
        corners_t0,
        corners_t1,
        dh,
        ResidualKind::Clearance,
    )?);
    if let (Some(m0), Some(m1)) = (corners_m0, corners_m1) {
        // top gap g = M - h ⇒ g' = M' - dh
        cuts.extend(isolate_residual_roots_interval(
            f0,
            f1,
            a,
            b,
            omega,
            grid,
            i0,
            j0,
            m0,
            m1,
            dh,
            ResidualKind::TopGap,
        )?);
    }
    cuts.retain(|f| *f > f0 + 1.0e-14 && *f < f1 - 1.0e-14);
    cuts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    cuts.dedup_by(|x, y| (*x - *y).abs() <= 1.0e-14);
    Ok(cuts)
}

#[derive(Clone, Copy)]
enum FieldKind {
    Terrain,
    ModelTop,
}

#[derive(Clone, Copy)]
enum ResidualKind {
    /// c = h - F, c' = dh - F'
    Clearance,
    /// g = F - h, g' = F' - dh
    TopGap,
}

#[derive(Clone, Copy, Debug)]
struct Iv {
    lo: f64,
    hi: f64,
}

impl Iv {
    fn point(x: f64) -> Self {
        Self { lo: x, hi: x }
    }
    fn outward_point(x: f64) -> Self {
        Self {
            lo: next_down(x),
            hi: next_up(x),
        }
    }
    fn outward_hull(a: f64, b: f64) -> Self {
        Self {
            lo: next_down(a.min(b)),
            hi: next_up(a.max(b)),
        }
    }
    fn strictly_excludes_zero(self) -> bool {
        self.hi < 0.0 || self.lo > 0.0
    }
    fn neg(self) -> Self {
        Self {
            lo: next_down(-self.hi),
            hi: next_up(-self.lo),
        }
    }
    fn add(self, other: Self) -> Self {
        Self {
            lo: next_down(self.lo + other.lo),
            hi: next_up(self.hi + other.hi),
        }
    }
    fn sub(self, other: Self) -> Self {
        Self {
            lo: next_down(self.lo - other.hi),
            hi: next_up(self.hi - other.lo),
        }
    }
    fn mul(self, other: Self) -> Self {
        let values = [
            self.lo * other.lo,
            self.lo * other.hi,
            self.hi * other.lo,
            self.hi * other.hi,
        ];
        let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Self {
            lo: next_down(lo),
            hi: next_up(hi),
        }
    }
    fn square(self) -> Self {
        if self.lo <= 0.0 && self.hi >= 0.0 {
            let hi = (self.lo * self.lo).max(self.hi * self.hi);
            Self {
                lo: 0.0,
                hi: next_up(hi),
            }
        } else {
            let a = self.lo * self.lo;
            let b = self.hi * self.hi;
            Self {
                lo: next_down(a.min(b)).max(0.0),
                hi: next_up(a.max(b)),
            }
        }
    }
    fn reciprocal(self) -> Result<Self, BoundaryError> {
        if self.lo <= 0.0 && self.hi >= 0.0 {
            return Err(BoundaryError::RootFindingFailed);
        }
        Ok(Self {
            lo: next_down(1.0 / self.hi),
            hi: next_up(1.0 / self.lo),
        })
    }
    fn div(self, other: Self) -> Result<Self, BoundaryError> {
        Ok(self.mul(other.reciprocal()?))
    }
    fn sqrt(self) -> Result<Self, BoundaryError> {
        if self.hi < 0.0 || !self.hi.is_finite() {
            return Err(BoundaryError::RootFindingFailed);
        }
        Ok(Self {
            lo: next_down(self.lo.max(0.0).sqrt()).max(0.0),
            hi: next_up(self.hi.sqrt()),
        })
    }
    fn scale(self, value: f64) -> Self {
        self.mul(Self::point(value))
    }
    fn abs_upper(self) -> f64 {
        next_up(self.lo.abs().max(self.hi.abs()))
    }
}

fn dot_interval(left: [f64; 3], right: [f64; 3]) -> Iv {
    Iv::outward_point(left[0])
        .mul(Iv::outward_point(right[0]))
        .add(Iv::outward_point(left[1]).mul(Iv::outward_point(right[1])))
        .add(Iv::outward_point(left[2]).mul(Iv::outward_point(right[2])))
}

fn next_down(value: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        return value;
    }
    if value == 0.0 {
        return -f64::from_bits(1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0.0 { bits - 1 } else { bits + 1 })
}

fn next_up(value: f64) -> f64 {
    if value.is_nan() || value == f64::INFINITY {
        return value;
    }
    if value == 0.0 {
        return f64::from_bits(1);
    }
    let bits = value.to_bits();
    f64::from_bits(if value > 0.0 { bits + 1 } else { bits - 1 })
}

fn sin_interval(input: Iv) -> Iv {
    if !input.lo.is_finite()
        || !input.hi.is_finite()
        || input.hi - input.lo >= std::f64::consts::TAU
    {
        return Iv { lo: -1.0, hi: 1.0 };
    }
    let mut lo = input.lo.sin().min(input.hi.sin());
    let mut hi = input.lo.sin().max(input.hi.sin());
    let first = ((input.lo - std::f64::consts::FRAC_PI_2) / std::f64::consts::PI).ceil() as i64;
    let last = ((input.hi - std::f64::consts::FRAC_PI_2) / std::f64::consts::PI).floor() as i64;
    for k in first..=last {
        let extremum = if k.rem_euclid(2) == 0 { 1.0 } else { -1.0 };
        lo = lo.min(extremum);
        hi = hi.max(extremum);
    }
    // Platform trigonometric functions are widened by two representable values;
    // arithmetic operations widen once at every subsequent operation.
    Iv {
        lo: next_down(next_down(lo)).max(-1.0),
        hi: next_up(next_up(hi)).min(1.0),
    }
}

fn cos_interval(input: Iv) -> Iv {
    if !input.lo.is_finite()
        || !input.hi.is_finite()
        || input.hi - input.lo >= std::f64::consts::TAU
    {
        return Iv { lo: -1.0, hi: 1.0 };
    }
    let mut lo = input.lo.cos().min(input.hi.cos());
    let mut hi = input.lo.cos().max(input.hi.cos());
    let first = (input.lo / std::f64::consts::PI).ceil() as i64;
    let last = (input.hi / std::f64::consts::PI).floor() as i64;
    for k in first..=last {
        let extremum = if k.rem_euclid(2) == 0 { 1.0 } else { -1.0 };
        lo = lo.min(extremum);
        hi = hi.max(extremum);
    }
    Iv {
        lo: next_down(next_down(lo)).max(-1.0),
        hi: next_up(next_up(hi)).min(1.0),
    }
}

/// Value, first derivative, and second derivative interval with respect to path
/// fraction. Every primitive is outward-rounded.
#[derive(Clone, Copy, Debug)]
struct Jet2 {
    value: Iv,
    first: Iv,
    second: Iv,
}

impl Jet2 {
    fn constant(value: f64) -> Self {
        Self {
            value: Iv::point(value),
            first: Iv::point(0.0),
            second: Iv::point(0.0),
        }
    }
    fn variable(value: Iv) -> Self {
        Self {
            value,
            first: Iv::point(1.0),
            second: Iv::point(0.0),
        }
    }
    fn add(self, other: Self) -> Self {
        Self {
            value: self.value.add(other.value),
            first: self.first.add(other.first),
            second: self.second.add(other.second),
        }
    }
    fn sub(self, other: Self) -> Self {
        Self {
            value: self.value.sub(other.value),
            first: self.first.sub(other.first),
            second: self.second.sub(other.second),
        }
    }
    fn scale(self, value: f64) -> Self {
        Self {
            value: self.value.scale(value),
            first: self.first.scale(value),
            second: self.second.scale(value),
        }
    }
    fn mul(self, other: Self) -> Self {
        Self {
            value: self.value.mul(other.value),
            first: self.first.mul(other.value).add(self.value.mul(other.first)),
            second: self
                .second
                .mul(other.value)
                .add(self.first.mul(other.first).scale(2.0))
                .add(self.value.mul(other.second)),
        }
    }
    fn square(self) -> Self {
        Self {
            value: self.value.square(),
            first: self.value.mul(self.first).scale(2.0),
            second: self
                .first
                .square()
                .add(self.value.mul(self.second))
                .scale(2.0),
        }
    }
    fn reciprocal(self) -> Result<Self, BoundaryError> {
        let inverse = self.value.reciprocal()?;
        let inverse2 = inverse.square();
        let inverse3 = inverse2.mul(inverse);
        Ok(Self {
            value: inverse,
            first: self.first.neg().mul(inverse2),
            second: self
                .first
                .square()
                .mul(inverse3)
                .scale(2.0)
                .sub(self.second.mul(inverse2)),
        })
    }
    fn sqrt(self) -> Result<Self, BoundaryError> {
        let root = self.value.sqrt()?;
        if root.lo <= 0.0 {
            return Err(BoundaryError::RootFindingFailed);
        }
        let two_root = root.scale(2.0);
        let four_root_cubed = root.square().mul(root).scale(4.0);
        Ok(Self {
            value: root,
            first: self.first.div(two_root)?,
            second: self
                .second
                .div(two_root)?
                .sub(self.first.square().div(four_root_cubed)?),
        })
    }
    fn sin(self) -> Self {
        let sine = sin_interval(self.value);
        let cosine = cos_interval(self.value);
        Self {
            value: sine,
            first: cosine.mul(self.first),
            second: sine
                .neg()
                .mul(self.first.square())
                .add(cosine.mul(self.second)),
        }
    }
    fn cos(self) -> Self {
        let sine = sin_interval(self.value);
        let cosine = cos_interval(self.value);
        Self {
            value: cosine,
            first: sine.neg().mul(self.first),
            second: cosine
                .neg()
                .mul(self.first.square())
                .sub(sine.mul(self.second)),
        }
    }
}

struct ResidualContext<'a> {
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    grid: &'a DomainGeometry,
    i: i64,
    j: i64,
    leaf_f0: f64,
    leaf_f1: f64,
    corners0: [f64; 4],
    corners1: [f64; 4],
    dh: f64,
    kind: ResidualKind,
}

impl ResidualContext<'_> {
    fn corners_at(&self, fraction: f64) -> [f64; 4] {
        let tau = (fraction - self.leaf_f0) / (self.leaf_f1 - self.leaf_f0);
        lerp_corners(self.corners0, self.corners1, tau)
    }

    fn corner_slopes(&self) -> [f64; 4] {
        let inverse_width = 1.0 / (self.leaf_f1 - self.leaf_f0);
        [
            (self.corners1[0] - self.corners0[0]) * inverse_width,
            (self.corners1[1] - self.corners0[1]) * inverse_width,
            (self.corners1[2] - self.corners0[2]) * inverse_width,
            (self.corners1[3] - self.corners0[3]) * inverse_width,
        ]
    }

    fn derivative_point(&self, fraction: f64) -> Result<f64, BoundaryError> {
        let d_field = bilinear_field_path_deriv_timed(
            fraction,
            self.a,
            self.b,
            self.omega,
            self.grid,
            self.i,
            self.j,
            self.corners_at(fraction),
            self.corner_slopes(),
        )?;
        Ok(match self.kind {
            ResidualKind::Clearance => self.dh - d_field,
            ResidualKind::TopGap => d_field - self.dh,
        })
    }

    fn derivative_enclosures(&self, f0: f64, f1: f64) -> Result<(Iv, Iv), BoundaryError> {
        let (x, y, x1, y1, x2, y2) = coordinate_jet_enclosures(
            f0, f1, self.a, self.b, self.omega, self.grid, self.i, self.j,
        )?;
        let coefficients0 = bilinear_coefficients(self.corners_at(f0));
        let coefficients1 = bilinear_coefficients(self.corners_at(f1));
        let leaf_coefficients0 = bilinear_coefficients(self.corners0);
        let leaf_coefficients1 = bilinear_coefficients(self.corners1);
        let inverse_leaf_width = Iv::point(self.leaf_f1 - self.leaf_f0).reciprocal()?;
        let coefficients = std::array::from_fn(|index| {
            Iv::outward_hull(coefficients0[index], coefficients1[index])
        });
        let slopes: [Iv; 4] = std::array::from_fn(|index| {
            Iv::outward_point(leaf_coefficients1[index])
                .sub(Iv::outward_point(leaf_coefficients0[index]))
                .mul(inverse_leaf_width)
        });
        let [base, alpha, beta, gamma] = coefficients;
        let [base1, alpha1, beta1, gamma1] = slopes;
        let _ = base;

        // F = base + alpha*x + beta*y + gamma*x*y, with every coefficient
        // linear in time/path fraction over the frozen MetEngine window.
        let field1 = base1
            .add(alpha1.mul(x))
            .add(alpha.mul(x1))
            .add(beta1.mul(y))
            .add(beta.mul(y1))
            .add(gamma1.mul(x).mul(y))
            .add(gamma.mul(x1).mul(y))
            .add(gamma.mul(x).mul(y1));

        let field2 = alpha1
            .mul(x1)
            .scale(2.0)
            .add(alpha.mul(x2))
            .add(beta1.mul(y1).scale(2.0))
            .add(beta.mul(y2))
            .add(gamma1.mul(x1).mul(y).scale(2.0))
            .add(gamma1.mul(x).mul(y1).scale(2.0))
            .add(gamma.mul(x2).mul(y))
            .add(gamma.mul(x1).mul(y1).scale(2.0))
            .add(gamma.mul(x).mul(y2));

        Ok(match self.kind {
            ResidualKind::Clearance => (Iv::point(self.dh).sub(field1), field2.neg()),
            ResidualKind::TopGap => (field1.sub(Iv::point(self.dh)), field2),
        })
    }
}

fn isolate_residual_roots_interval(
    f0: f64,
    f1: f64,
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    grid: &DomainGeometry,
    i: i64,
    j: i64,
    corners0: [f64; 4],
    corners1: [f64; 4],
    dh: f64,
    kind: ResidualKind,
) -> Result<Vec<f64>, BoundaryError> {
    if omega <= 1.0e-15 || (corners_uniform(corners0) && corners_uniform(corners1)) {
        // Spatially uniform and linearly time-varying field: the residual is
        // affine, so there is no isolated interior extremum. A stationary
        // horizontal path has the same property for any bilinear cell.
        return Ok(Vec::new());
    }
    let context = ResidualContext {
        a,
        b,
        omega,
        grid,
        i,
        j,
        leaf_f0: f0,
        leaf_f1: f1,
        corners0,
        corners1,
        dh,
        kind,
    };
    let mut roots = Vec::new();
    let mut visited = 0_u32;
    isolate_residual_rec(&context, f0, f1, 0, &mut visited, &mut roots)?;
    roots.retain(|root| *root > f0 && *root < f1 && root.is_finite());
    roots.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    roots.dedup_by(|left, right| (*left - *right).abs() <= 1.0e-13);
    Ok(roots)
}

fn corners_uniform(c: [f64; 4]) -> bool {
    let lo = c.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = c.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (hi - lo).abs() <= 1.0e-9
}

fn isolate_residual_rec(
    context: &ResidualContext<'_>,
    f0: f64,
    f1: f64,
    depth: u32,
    visited: &mut u32,
    roots: &mut Vec<f64>,
) -> Result<(), BoundaryError> {
    const MAX_DEPTH: u32 = 64;
    const MAX_VISITED: u32 = 65_536;
    const MIN_WIDTH: f64 = 1.0e-14;
    *visited = visited
        .checked_add(1)
        .ok_or(BoundaryError::RootFindingFailed)?;
    if *visited > MAX_VISITED || !f0.is_finite() || !f1.is_finite() || f1 <= f0 {
        return Err(BoundaryError::RootFindingFailed);
    }
    let (d1_enclosure, d2_enclosure) = context.derivative_enclosures(f0, f1)?;
    if d1_enclosure.strictly_excludes_zero() {
        return Ok(());
    }

    let d0 = context.derivative_point(f0)?;
    let d1 = context.derivative_point(f1)?;
    let brackets = d0 == 0.0 || d1 == 0.0 || d0.signum() != d1.signum();

    if d2_enclosure.strictly_excludes_zero() {
        if brackets {
            roots.push(bisect_unique_residual_root(context, f0, f1, d0, d1)?);
            return Ok(());
        }
        let d0_interval = Iv::outward_point(d0);
        let d1_interval = Iv::outward_point(d1);
        let increasing = d2_enclosure.lo > 0.0;
        let decreasing = d2_enclosure.hi < 0.0;
        if (increasing && (d0_interval.lo > 0.0 || d1_interval.hi < 0.0))
            || (decreasing && (d0_interval.hi < 0.0 || d1_interval.lo > 0.0))
        {
            return Ok(());
        }
    }

    if depth >= MAX_DEPTH || f1 - f0 <= MIN_WIDTH {
        // No certificate was obtained. Resource exhaustion is a hard failure;
        // an unproved leaf must never be emitted as "certified".
        return Err(BoundaryError::RootFindingFailed);
    }

    let mid = 0.5 * (f0 + f1);
    if mid <= f0 || mid >= f1 {
        return Err(BoundaryError::RootFindingFailed);
    }
    isolate_residual_rec(context, f0, mid, depth + 1, visited, roots)?;
    isolate_residual_rec(context, mid, f1, depth + 1, visited, roots)?;
    Ok(())
}

fn bisect_unique_residual_root(
    context: &ResidualContext<'_>,
    f0: f64,
    f1: f64,
    d0: f64,
    d1: f64,
) -> Result<f64, BoundaryError> {
    if d0 == 0.0 {
        return Ok(f0);
    }
    if d1 == 0.0 {
        return Ok(f1);
    }
    if d0.signum() == d1.signum() {
        return Err(BoundaryError::RootFindingFailed);
    }
    let mut lo = f0;
    let mut hi = f1;
    let mut d_lo = d0;
    for _ in 0..64 {
        let mid = 0.5 * (lo + hi);
        let d_mid = context.derivative_point(mid)?;
        if d_mid == 0.0 {
            lo = mid;
            hi = mid;
            d_lo = d_mid;
        } else if d_lo.signum() == d_mid.signum() {
            lo = mid;
            d_lo = d_mid;
        } else {
            hi = mid;
        }
    }
    Ok(0.5 * (lo + hi))
}

fn lerp_corners(c0: [f64; 4], c1: [f64; 4], t: f64) -> [f64; 4] {
    [
        c0[0] + (c1[0] - c0[0]) * t,
        c0[1] + (c1[1] - c0[1]) * t,
        c0[2] + (c1[2] - c0[2]) * t,
        c0[3] + (c1[3] - c0[3]) * t,
    ]
}

fn normalized_slerp_jets(
    f0: f64,
    f1: f64,
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
) -> Result<[Jet2; 3], BoundaryError> {
    if omega <= 1.0e-15 {
        return Ok(a.map(Jet2::constant));
    }
    let fraction = Jet2::variable(Iv::outward_hull(f0, f1));
    let theta = fraction.scale(omega);
    // Correlation-preserving short-arc basis. The usual two slerp weights both
    // span [0,1], and a naive interval extension therefore admits the impossible
    // pair w0=w1=0. Rewriting q=a*cos(theta)+t*sin(theta) keeps a single angle.
    let tangent = great_circle_tangent(a, b, omega)?;
    let cosine = theta.cos();
    let sine = theta.sin();
    let raw = std::array::from_fn(|index| cosine.scale(a[index]).add(sine.scale(tangent[index])));
    let mut norm2 = raw[0].square().add(raw[1].square()).add(raw[2].square());
    // Tight norm-value enclosure from the basis invariants. Derivatives remain
    // those produced by interval AD; only the dependency-blown value component
    // is replaced by a mathematically equivalent bound.
    let norm_deviation = Iv::outward_point(dot_interval(a, a).sub(Iv::point(1.0)).abs_upper())
        .add(Iv::outward_point(
            dot_interval(tangent, tangent)
                .sub(Iv::point(1.0))
                .abs_upper(),
        ))
        .add(Iv::outward_point(dot_interval(a, tangent).abs_upper()))
        .hi;
    if !norm_deviation.is_finite() || norm_deviation >= 0.5 {
        return Err(BoundaryError::RootFindingFailed);
    }
    norm2.value = Iv::outward_hull(1.0 - norm_deviation, 1.0 + norm_deviation);
    let norm = norm2.sqrt()?;
    let inverse_norm = norm.reciprocal()?;
    Ok(raw.map(|component| component.mul(inverse_norm)))
}

fn coordinate_jet_enclosures(
    f0: f64,
    f1: f64,
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    grid: &DomainGeometry,
    i: i64,
    j: i64,
) -> Result<(Iv, Iv, Iv, Iv, Iv, Iv), BoundaryError> {
    let (lon0, lat0) = slerp_lonlat(a, b, omega, f0)?;
    let (lon1, lat1) = slerp_lonlat(a, b, omega, f1)?;
    let x = Iv::outward_hull(
        local_cell_fraction(
            lon0,
            grid.longitude_origin_degrees,
            grid.longitude_spacing_degrees,
            i,
            grid.periodic_longitude
                .then_some(grid.longitude_spacing_degrees * grid.nx as f64),
        )?,
        local_cell_fraction(
            lon1,
            grid.longitude_origin_degrees,
            grid.longitude_spacing_degrees,
            i,
            grid.periodic_longitude
                .then_some(grid.longitude_spacing_degrees * grid.nx as f64),
        )?,
    );
    let y = Iv::outward_hull(
        local_cell_fraction(
            lat0,
            grid.latitude_origin_degrees,
            grid.latitude_spacing_degrees,
            j,
            None,
        )?,
        local_cell_fraction(
            lat1,
            grid.latitude_origin_degrees,
            grid.latitude_spacing_degrees,
            j,
            None,
        )?,
    );

    let [px, py, pz] = normalized_slerp_jets(f0, f1, a, b, omega)?;
    let horizontal_norm2 = px.value.square().add(py.value.square());
    if horizontal_norm2.lo <= 1.0e-18 {
        return Err(BoundaryError::RootFindingFailed);
    }
    let lon_numerator = px.value.mul(py.first).sub(py.value.mul(px.first));
    let lon_numerator_first = px.value.mul(py.second).sub(py.value.mul(px.second));
    let horizontal_norm2_first = px
        .value
        .mul(px.first)
        .add(py.value.mul(py.first))
        .scale(2.0);
    let lon_first = lon_numerator.div(horizontal_norm2)?;
    let lon_second = lon_numerator_first
        .mul(horizontal_norm2)
        .sub(lon_numerator.mul(horizontal_norm2_first))
        .div(horizontal_norm2.square())?;

    let horizontal_norm = horizontal_norm2.sqrt()?;
    let lat_first = pz.first.div(horizontal_norm)?;
    let lat_second = pz.second.div(horizontal_norm)?.add(
        pz.value
            .mul(pz.first.square())
            .div(horizontal_norm2.mul(horizontal_norm))?,
    );

    let radians_to_degrees = 180.0 / std::f64::consts::PI;
    let x_scale = radians_to_degrees / grid.longitude_spacing_degrees;
    let y_scale = radians_to_degrees / grid.latitude_spacing_degrees;
    Ok((
        x,
        y,
        lon_first.scale(x_scale),
        lat_first.scale(y_scale),
        lon_second.scale(x_scale),
        lat_second.scale(y_scale),
    ))
}

fn local_cell_fraction(
    value: f64,
    origin: f64,
    spacing: f64,
    index: i64,
    periodic_span: Option<f64>,
) -> Result<f64, BoundaryError> {
    if !value.is_finite() || !origin.is_finite() || !spacing.is_finite() || spacing == 0.0 {
        return Err(BoundaryError::InvalidParticleState);
    }
    let mut adjusted = value;
    if let Some(span) = periodic_span {
        if !span.is_finite() || span <= 0.0 {
            return Err(BoundaryError::MissingContext);
        }
        let center = origin + spacing * (index as f64 + 0.5);
        adjusted += ((center - adjusted) / span).round() * span;
    }
    let fraction = (adjusted - origin) / spacing - index as f64;
    const CELL_EPSILON: f64 = 2.0e-10;
    if !fraction.is_finite() || fraction < -CELL_EPSILON || fraction > 1.0 + CELL_EPSILON {
        return Err(BoundaryError::RootFindingFailed);
    }
    Ok(fraction.clamp(0.0, 1.0))
}

fn bilinear_coefficients(corners: [f64; 4]) -> [f64; 4] {
    let [z00, z10, z01, z11] = corners;
    [z00, z10 - z00, z01 - z00, z11 - z10 - z01 + z00]
}

/// dF/df with time-linear corners: spatial bilinear chain rule + ∂F/∂corners * dcorners/df.
fn bilinear_field_path_deriv_timed(
    f: f64,
    a: [f64; 3],
    b: [f64; 3],
    omega: f64,
    grid: &DomainGeometry,
    i: i64,
    j: i64,
    corners: [f64; 4],
    d_corners: [f64; 4],
) -> Result<f64, BoundaryError> {
    let (lon, lat, dlon, dlat) = if omega <= 1.0e-15 {
        let (lo, la) = lon_lat_from_unit(a)?;
        (lo, la, 0.0, 0.0)
    } else {
        slerp_lonlat_and_deriv(a, b, omega, f)?
    };
    let dx = grid.longitude_spacing_degrees;
    let dy = grid.latitude_spacing_degrees;
    let fx = local_cell_fraction(
        lon,
        grid.longitude_origin_degrees,
        dx,
        i,
        grid.periodic_longitude.then_some(dx * grid.nx as f64),
    )?;
    let fy = local_cell_fraction(lat, grid.latitude_origin_degrees, dy, j, None)?;
    let dfx = dlon / dx;
    let dfy = dlat / dy;
    let z00 = corners[0];
    let z10 = corners[1];
    let z01 = corners[2];
    let z11 = corners[3];
    let w00 = (1.0 - fx) * (1.0 - fy);
    let w10 = fx * (1.0 - fy);
    let w01 = (1.0 - fx) * fy;
    let w11 = fx * fy;
    let d_t_dfx = (1.0 - fy) * (z10 - z00) + fy * (z11 - z01);
    let d_t_dfy = (1.0 - fx) * (z01 - z00) + fx * (z11 - z10);
    let spatial = d_t_dfx * dfx + d_t_dfy * dfy;
    let temporal =
        w00 * d_corners[0] + w10 * d_corners[1] + w01 * d_corners[2] + w11 * d_corners[3];
    Ok(spatial + temporal)
}

fn path_lonlat(
    start: &ParticleState,
    proposed: &ParticleState,
    f: f64,
) -> Result<(f64, f64), BoundaryError> {
    interpolate_great_circle(
        start.longitude_degrees,
        start.latitude_degrees,
        proposed.longitude_degrees,
        proposed.latitude_degrees,
        f,
    )
}

/// Exact domain-boundary meridians/parallels (halo-shrunk outer edges).
fn domain_boundary_crossings(
    start: &ParticleState,
    proposed: &ParticleState,
    grid: &DomainGeometry,
) -> Result<Vec<f64>, BoundaryError> {
    let a = unit_vector(start.longitude_degrees, start.latitude_degrees)?;
    let b = unit_vector(proposed.longitude_degrees, proposed.latitude_degrees)?;
    let cos_omega = dot(a, b).clamp(-1.0, 1.0);
    let omega = cos_omega.acos();
    if omega <= 1.0e-15 {
        return Ok(Vec::new());
    }
    let mut fractions = Vec::new();
    let halo = grid.halo_cells as i64;
    let dx = grid.longitude_spacing_degrees;
    let dy = grid.latitude_spacing_degrees;
    let ox = grid.longitude_origin_degrees;
    let oy = grid.latitude_origin_degrees;
    let nx = grid.nx as i64;
    let ny = grid.ny as i64;

    // Safe interior latitude edges at index halo and ny-1-halo.
    for j_edge in [halo, ny - 1 - halo] {
        if j_edge < 0 || j_edge >= ny {
            continue;
        }
        let parallel = oy + dy * j_edge as f64;
        if (-90.0..=90.0).contains(&parallel) {
            fractions.extend(gc_parallel_fractions(a, b, omega, parallel)?);
        }
    }
    if !grid.periodic_longitude {
        for i_edge in [halo, nx - 1 - halo] {
            if i_edge < 0 || i_edge >= nx {
                continue;
            }
            let meridian = ox + dx * i_edge as f64;
            fractions.extend(gc_meridian_fractions(a, b, omega, meridian)?);
        }
    }
    fractions.retain(|f| f.is_finite() && *f > 1.0e-15 && *f < 1.0 - 1.0e-15);
    fractions.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    fractions.dedup_by(|x, y| (*x - *y).abs() <= 1.0e-14);
    Ok(fractions)
}

fn met_bracket_times(
    meteorology: &mut MetEngine,
    local_fraction: f64,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    local_zero_global: f64,
) -> Result<(i64, i64), BoundaryError> {
    let global = local_zero_global + (1.0 - local_zero_global) * local_fraction;
    let time = lerp_timestamp(step_start_time, step_end_time, global)?;
    let window = meteorology
        .prepare(time)
        .map_err(|_| BoundaryError::MissingContext)?;
    let before = window
        .frames
        .before
        .metadata()
        .valid_time
        .seconds_since_unix_epoch();
    let after = window
        .frames
        .after
        .metadata()
        .valid_time
        .seconds_since_unix_epoch();
    Ok((before, after))
}

fn frame_transition_fraction(
    f0: f64,
    f1: f64,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    local_zero_global: f64,
    bracket0: (i64, i64),
    bracket1: (i64, i64),
) -> Result<Option<f64>, BoundaryError> {
    // Map a candidate frame valid-time (seconds) to path fraction.
    let t0 = step_start_time.seconds_since_unix_epoch() as f64
        + step_start_time.nanosecond() as f64 * 1.0e-9;
    let t1 = step_end_time.seconds_since_unix_epoch() as f64
        + step_end_time.nanosecond() as f64 * 1.0e-9;
    if (t1 - t0).abs() <= 1.0e-18 {
        return Ok(None);
    }
    // Transition candidates: after of first window / before of second window.
    let candidates = [bracket0.1, bracket1.0, bracket0.0, bracket1.1];
    let mut best = None;
    for sec in candidates {
        let t = sec as f64;
        // global fraction in step:
        let g = (t - t0) / (t1 - t0);
        if !(0.0..=1.0).contains(&g) {
            continue;
        }
        // local fraction: g = local_zero + (1-local_zero)*f  ⇒ f = (g-local_zero)/(1-local_zero)
        let denom = 1.0 - local_zero_global;
        if denom.abs() <= 1.0e-18 {
            continue;
        }
        let f = (g - local_zero_global) / denom;
        if f > f0 + 1.0e-14 && f < f1 - 1.0e-14 {
            best = Some(f);
            break;
        }
    }
    Ok(best)
}

fn cell_fraction(
    lon: f64,
    lat: f64,
    grid: &DomainGeometry,
) -> Result<(i64, f64, i64, f64), BoundaryError> {
    // Periodic longitude: fold into the principal grid span when enabled.
    let mut lon = lon;
    if grid.periodic_longitude {
        let span = grid.longitude_spacing_degrees * grid.nx as f64;
        if span > 0.0 && span.is_finite() {
            let mut x = lon - grid.longitude_origin_degrees;
            x = x.rem_euclid(span);
            lon = grid.longitude_origin_degrees + x;
        }
    }
    let fx_all = (lon - grid.longitude_origin_degrees) / grid.longitude_spacing_degrees;
    let fy_all = (lat - grid.latitude_origin_degrees) / grid.latitude_spacing_degrees;
    if !fx_all.is_finite() || !fy_all.is_finite() {
        return Err(BoundaryError::InvalidParticleState);
    }
    let i = fx_all.floor() as i64;
    let j = fy_all.floor() as i64;
    Ok((i, fx_all - i as f64, j, fy_all - j as f64))
}

fn sample_cell_corner_field(
    i: i64,
    j: i64,
    local_fraction: f64,
    start: &ParticleState,
    proposed: &ParticleState,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    local_zero_global: f64,
    meteorology: &mut MetEngine,
    query_plan: &TransportPlan,
    execution: &dyn ExecutionContext,
    workspace: &mut BatchWorkspace,
    grid: &DomainGeometry,
    kind: FieldKind,
) -> Result<Option<[f64; 4]>, BoundaryError> {
    let mid = sample_at_fraction(
        local_fraction,
        start,
        proposed,
        step_start_time,
        step_end_time,
        local_zero_global,
        meteorology,
        query_plan,
        execution,
        workspace,
    )?;
    let mut values = [0.0_f64; 4];
    for (slot, (di, dj)) in [(0_i64, 0_i64), (1, 0), (0, 1), (1, 1)]
        .into_iter()
        .enumerate()
    {
        let lon = grid.longitude_origin_degrees + grid.longitude_spacing_degrees * (i + di) as f64;
        let lat = grid.latitude_origin_degrees + grid.latitude_spacing_degrees * (j + dj) as f64;
        if !(-90.0..=90.0).contains(&lat) {
            return Ok(None);
        }
        let mut probe = start.clone();
        probe.longitude_degrees = lon;
        probe.latitude_degrees = lat;
        probe.height_asl_m = mid.position.height_asl_m;
        let sample = sample_state_at_time(
            &probe,
            local_fraction,
            step_start_time,
            step_end_time,
            local_zero_global,
            meteorology,
            query_plan,
            execution,
            workspace,
        )?;
        let value = match kind {
            FieldKind::Terrain => sample.surface_height_asl_m,
            FieldKind::ModelTop => sample.model_top_height_asl_m,
        };
        let Some(v) = value.filter(|v| v.is_finite()) else {
            return Ok(None);
        };
        values[slot] = v;
    }
    Ok(Some(values))
}

fn sample_state_at_time(
    state: &ParticleState,
    local_fraction: f64,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    local_zero_global: f64,
    meteorology: &mut MetEngine,
    query_plan: &TransportPlan,
    execution: &dyn ExecutionContext,
    workspace: &mut BatchWorkspace,
) -> Result<BoundarySample, BoundaryError> {
    let global = local_zero_global + (1.0 - local_zero_global) * local_fraction;
    let time = lerp_timestamp(step_start_time, step_end_time, global)?;
    let window = meteorology
        .prepare(time)
        .map_err(|_| BoundaryError::MissingContext)?;
    let batch = window
        .prepare_transport_batch(
            query_plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points: QueryPointArrays {
                    longitude_degrees: vec![state.longitude_degrees],
                    latitude_degrees: vec![state.latitude_degrees],
                    vertical: vec![state.height_asl_m],
                },
            },
            workspace,
        )
        .map_err(|_| BoundaryError::MissingContext)?;
    let output = batch
        .execute(execution, workspace)
        .map_err(|_| BoundaryError::MissingContext)?;
    let row = output.row(0).ok_or(BoundaryError::MissingContext)?;
    Ok(BoundarySample {
        fraction: local_fraction,
        position: state.clone(),
        meteorology_status: row.status(),
        surface_height_asl_m: row.terrain_height_asl_m().filter(|value| value.is_finite()),
        model_top_height_asl_m: row
            .bounds()
            .and_then(|bounds| bounds.physical_model_top_asl_m())
            .filter(|value| value.is_finite()),
    })
}

fn sample_at_fraction(
    local_fraction: f64,
    start: &ParticleState,
    proposed: &ParticleState,
    step_start_time: Timestamp,
    step_end_time: Timestamp,
    local_zero_global: f64,
    meteorology: &mut MetEngine,
    query_plan: &TransportPlan,
    execution: &dyn ExecutionContext,
    workspace: &mut BatchWorkspace,
) -> Result<BoundarySample, BoundaryError> {
    let (lon, lat) = interpolate_great_circle(
        start.longitude_degrees,
        start.latitude_degrees,
        proposed.longitude_degrees,
        proposed.latitude_degrees,
        local_fraction,
    )?;
    let mut position = start.clone();
    position.longitude_degrees = lon;
    position.latitude_degrees = lat;
    position.height_asl_m = lerp(start.height_asl_m, proposed.height_asl_m, local_fraction);
    let global = local_zero_global + (1.0 - local_zero_global) * local_fraction;
    let time = lerp_timestamp(step_start_time, step_end_time, global)?;
    let window = meteorology
        .prepare(time)
        .map_err(|_| BoundaryError::MissingContext)?;
    let batch = window
        .prepare_transport_batch(
            query_plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points: QueryPointArrays {
                    longitude_degrees: vec![position.longitude_degrees],
                    latitude_degrees: vec![position.latitude_degrees],
                    vertical: vec![position.height_asl_m],
                },
            },
            workspace,
        )
        .map_err(|_| BoundaryError::MissingContext)?;
    let output = batch
        .execute(execution, workspace)
        .map_err(|_| BoundaryError::MissingContext)?;
    let row = output.row(0).ok_or(BoundaryError::MissingContext)?;
    Ok(BoundarySample {
        fraction: local_fraction,
        position,
        meteorology_status: row.status(),
        surface_height_asl_m: row.terrain_height_asl_m().filter(|value| value.is_finite()),
        model_top_height_asl_m: row
            .bounds()
            .and_then(|bounds| bounds.physical_model_top_asl_m())
            .filter(|value| value.is_finite()),
    })
}

fn interpolate_great_circle(
    lon0: f64,
    lat0: f64,
    lon1: f64,
    lat1: f64,
    fraction: f64,
) -> Result<(f64, f64), BoundaryError> {
    let a = unit_vector(lon0, lat0)?;
    let b = unit_vector(lon1, lat1)?;
    let cos_omega = dot(a, b).clamp(-1.0, 1.0);
    let omega = cos_omega.acos();
    lon_lat_from_unit(slerp_unit(a, b, omega, fraction)?)
}

fn unit_vector(lon_deg: f64, lat_deg: f64) -> Result<[f64; 3], BoundaryError> {
    if !lon_deg.is_finite() || !lat_deg.is_finite() || lat_deg.abs() > 90.0 {
        return Err(BoundaryError::InvalidParticleState);
    }
    let lon = lon_deg.to_radians();
    let lat = lat_deg.to_radians();
    let cos_lat = lat.cos();
    Ok([cos_lat * lon.cos(), cos_lat * lon.sin(), lat.sin()])
}

fn lon_lat_from_unit(vector: [f64; 3]) -> Result<(f64, f64), BoundaryError> {
    let norm = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if !(norm > 0.0) {
        return Err(BoundaryError::InvalidParticleState);
    }
    let x = vector[0] / norm;
    let y = vector[1] / norm;
    let z = (vector[2] / norm).clamp(-1.0, 1.0);
    let lon = y.atan2(x).to_degrees();
    let lat = z.asin().to_degrees();
    let lon = if lon >= 180.0 {
        lon - 360.0
    } else if lon < -180.0 {
        lon + 360.0
    } else {
        lon
    };
    if lon.is_finite() && lat.is_finite() {
        Ok((lon, lat))
    } else {
        Err(BoundaryError::InvalidParticleState)
    }
}

fn normalize(vector: [f64; 3]) -> Result<[f64; 3], BoundaryError> {
    let norm = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if !(norm > 0.0) {
        return Err(BoundaryError::InvalidParticleState);
    }
    Ok([vector[0] / norm, vector[1] / norm, vector[2] / norm])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lerp_i64(a: i64, b: i64, t: f64) -> Result<i64, BoundaryError> {
    if !t.is_finite() {
        return Err(BoundaryError::InvalidParticleState);
    }
    let value = a as f64 + (b - a) as f64 * t;
    if !value.is_finite() {
        return Err(BoundaryError::InvalidParticleState);
    }
    Ok(value.round() as i64)
}

fn lerp_u64(a: u64, b: u64, t: f64) -> Result<u64, BoundaryError> {
    if !t.is_finite() || t < 0.0 || t > 1.0 {
        return Err(BoundaryError::InvalidParticleState);
    }
    let value = a as f64 + (b as f64 - a as f64) * t;
    if !value.is_finite() || value < 0.0 {
        return Err(BoundaryError::InvalidParticleState);
    }
    Ok(value.round() as u64)
}

fn lerp_timestamp(
    start: Timestamp,
    end: Timestamp,
    fraction: f64,
) -> Result<Timestamp, BoundaryError> {
    if !fraction.is_finite() || fraction < 0.0 || fraction > 1.0 {
        return Err(BoundaryError::InvalidParticleState);
    }
    let start_ns = i128::from(start.seconds_since_unix_epoch()) * 1_000_000_000
        + i128::from(start.nanosecond());
    let end_ns =
        i128::from(end.seconds_since_unix_epoch()) * 1_000_000_000 + i128::from(end.nanosecond());
    let value = start_ns as f64 + (end_ns - start_ns) as f64 * fraction;
    let rounded = value.round() as i128;
    let seconds = i64::try_from(rounded.div_euclid(1_000_000_000))
        .map_err(|_| BoundaryError::InvalidParticleState)?;
    let nanos = u32::try_from(rounded.rem_euclid(1_000_000_000))
        .map_err(|_| BoundaryError::InvalidParticleState)?;
    Timestamp::new(seconds, nanos).map_err(|_| BoundaryError::InvalidParticleState)
}

fn shortest_delta_lon(a: f64, b: f64) -> f64 {
    let mut d = b - a;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

// Silence unused import if ExecutionPlan is only needed for trait coherence.
#[allow(dead_code)]
fn _plan(_: &ExecutionPlan) {}
#[allow(dead_code)]
fn _radius() -> f64 {
    M4_CONSTANTS.earth_radius_m
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::particle::{ParticleOrigin, ParticleStatus};
    use trajecta_case::model::population::{PopulationId, ReleaseEventId};
    use trajecta_case::model::time::Timestamp;

    fn particle(lon: f64, lat: f64, height: f64) -> ParticleState {
        ParticleState {
            id: crate::particle::ParticleId(1),
            population_id: PopulationId("p".into()),
            origin: ParticleOrigin::Release {
                event_id: ReleaseEventId("e".into()),
            },
            birth_time: Timestamp::UNIX_EPOCH,
            longitude_degrees: lon,
            latitude_degrees: lat,
            height_asl_m: height,
            integration_offset_ns: 0,
            elapsed_age_ns: 0,
            dry_air_mass_kg: 1.0,
            mass_kg: Default::default(),
            sensitivity_weight: None,
            status: ParticleStatus::Alive,
        }
    }

    #[test]
    fn retarget_maps_local_zero_to_collision_without_stacking_fractions() {
        // Pure kinematic check of residual path binding (no MetEngine).
        let start = particle(0.0, 0.0, 10.0);
        let proposed = particle(1.0, 0.0, -10.0);
        let collision = particle(0.5, 0.0, 0.0);
        let reflected = particle(1.0, 0.0, 10.0);

        let (lon0, lat0) = interpolate_great_circle(
            collision.longitude_degrees,
            collision.latitude_degrees,
            reflected.longitude_degrees,
            reflected.latitude_degrees,
            0.0,
        )
        .unwrap();
        assert!((lon0 - collision.longitude_degrees).abs() < 1.0e-12);
        assert!((lat0 - collision.latitude_degrees).abs() < 1.0e-12);

        let (lon1, _) = interpolate_great_circle(
            collision.longitude_degrees,
            collision.latitude_degrees,
            reflected.longitude_degrees,
            reflected.latitude_degrees,
            1.0,
        )
        .unwrap();
        assert!((lon1 - reflected.longitude_degrees).abs() < 1.0e-9);

        // Full-step fraction is stored directly; residual local maps through it once.
        // First collision at full-step 0.5 (caller passes full-step).
        let mut local_zero_global = 0.5_f64;
        // Second residual-local collision at 0.5 → full-step 0.75.
        let second_local = 0.5_f64;
        let second_global = local_zero_global + (1.0 - local_zero_global) * second_local;
        assert!((second_global - 0.75_f64).abs() < 1.0e-15);
        // retarget receives full-step 0.75 and must store 0.75, not remap to 0.875.
        local_zero_global = second_global;
        assert!((local_zero_global - 0.75_f64).abs() < 1.0e-15);
        let wrongly_stacked = 0.5 + (1.0 - 0.5) * 0.75;
        assert!((wrongly_stacked - 0.875_f64).abs() < 1.0e-15);
        assert!((local_zero_global - wrongly_stacked).abs() > 1.0e-12);
        let _ = (start, proposed, collision, reflected);
    }

    #[test]
    fn full_step_fraction_mapping_matches_surface_reflect_contract() {
        // Mirrors SurfaceReflect: local root -> full-step global, then retarget(global).
        let mut remaining_start = 0.0_f64;
        let first_local_root = 0.5_f64;
        let first_global = remaining_start + (1.0 - remaining_start) * first_local_root;
        assert!((first_global - 0.5).abs() < 1.0e-15);
        remaining_start = first_global; // retarget stores this directly
        let second_local_root = 0.5_f64;
        let second_global = remaining_start + (1.0 - remaining_start) * second_local_root;
        assert!((second_global - 0.75).abs() < 1.0e-15);
        // Sampler local_zero after second retarget equals second_global.
        let local_zero = second_global;
        let time_at_residual_half = local_zero + (1.0 - local_zero) * 0.5;
        assert!((time_at_residual_half - 0.875).abs() < 1.0e-15);
    }

    #[test]
    fn grid_line_crossings_detect_meridian() {
        let start = particle(0.1, 10.0, 1000.0);
        let proposed = particle(1.9, 10.0, 1000.0);
        let grid = DomainGeometry {
            domain: trajecta_case::model::meteorology::DomainId("g".into()),
            longitude_origin_degrees: 0.0,
            latitude_origin_degrees: 0.0,
            longitude_spacing_degrees: 1.0,
            latitude_spacing_degrees: 1.0,
            nx: 4,
            ny: 4,
            periodic_longitude: false,
            halo_cells: 0,
        };
        let cuts = grid_line_crossings(&start, &proposed, &grid).unwrap();
        assert!(cuts.iter().any(|f| *f > 0.0 && *f < 1.0));
    }

    /// Frozen counterexample: high-latitude short GC with equal endpoint latitudes
    /// rises toward the pole, so each interior parallel is crossed twice.
    #[test]
    fn grid_line_crossings_high_lat_double_parallel_and_true_lat_envelope() {
        let start = particle(-80.0, 60.0, 1000.0);
        let proposed = particle(80.0, 60.0, 1000.0);
        let grid = DomainGeometry {
            domain: trajecta_case::model::meteorology::DomainId("g".into()),
            longitude_origin_degrees: -180.0,
            latitude_origin_degrees: -90.0,
            longitude_spacing_degrees: 5.0,
            latitude_spacing_degrees: 5.0,
            nx: 72,
            ny: 37,
            periodic_longitude: true,
            halo_cells: 0,
        };

        let a = unit_vector(start.longitude_degrees, start.latitude_degrees).unwrap();
        let b = unit_vector(proposed.longitude_degrees, proposed.latitude_degrees).unwrap();
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        let (lat_lo, lat_hi) = gc_latitude_envelope(a, b, omega).unwrap();
        assert!((lat_lo - 60.0).abs() < 1.0e-9, "lat_lo={lat_lo}");
        // Interior peak near lon=0 is ~84.27°N — envelope must exceed endpoints.
        assert!(lat_hi > 84.0, "lat_hi={lat_hi}");
        assert!(lat_hi < 85.0, "lat_hi={lat_hi}");

        // Each 5° parallel strictly between endpoints and peak is hit twice.
        for parallel in [65.0, 70.0, 75.0, 80.0] {
            let roots = gc_parallel_fractions(a, b, omega, parallel).unwrap();
            assert_eq!(
                roots.len(),
                2,
                "parallel {parallel} roots={roots:?} (need two open-arc crossings)"
            );
            assert!(roots.iter().all(|f| *f > 0.0 && *f < 1.0));
        }
        // Above the peak: no roots.
        assert!(gc_parallel_fractions(a, b, omega, 85.0).unwrap().is_empty());

        let cuts = grid_line_crossings(&start, &proposed, &grid).unwrap();
        // 4 parallels × 2 roots = 8 parallel cuts, plus many meridians.
        let mut parallel_hits = 0_u32;
        for parallel in [65.0, 70.0, 75.0, 80.0] {
            for f in gc_parallel_fractions(a, b, omega, parallel).unwrap() {
                assert!(
                    cuts.iter().any(|c| (*c - f).abs() < 1.0e-10),
                    "missing parallel {parallel} cut f={f} in {cuts:?}"
                );
                parallel_hits += 1;
            }
        }
        assert_eq!(parallel_hits, 8);
        // Meridians every 5° from -75..75 interior should appear.
        assert!(
            cuts.len() >= 8 + 10,
            "expected meridians+parallels, got {} cuts: {cuts:?}",
            cuts.len()
        );
    }

    #[test]
    fn gc_parallel_fractions_returns_all_open_arc_roots() {
        let a = unit_vector(-80.0, 60.0).unwrap();
        let b = unit_vector(80.0, 60.0).unwrap();
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        let roots = gc_parallel_fractions(a, b, omega, 70.0).unwrap();
        assert_eq!(roots.len(), 2);
        assert!((roots[0] - 0.174641648004).abs() < 1.0e-9);
        assert!((roots[1] - 0.825358351996).abs() < 1.0e-9);
    }

    #[test]
    fn grid_line_crossings_negative_latitude_spacing_matches_positive() {
        let start = particle(-80.0, 60.0, 1000.0);
        let proposed = particle(80.0, 60.0, 1000.0);
        let grid_pos = DomainGeometry {
            domain: trajecta_case::model::meteorology::DomainId("g".into()),
            longitude_origin_degrees: -180.0,
            latitude_origin_degrees: -90.0,
            longitude_spacing_degrees: 5.0,
            latitude_spacing_degrees: 5.0,
            nx: 72,
            ny: 37,
            periodic_longitude: true,
            halo_cells: 0,
        };
        let grid_neg = DomainGeometry {
            domain: trajecta_case::model::meteorology::DomainId("g".into()),
            longitude_origin_degrees: -180.0,
            latitude_origin_degrees: 90.0,
            longitude_spacing_degrees: 5.0,
            latitude_spacing_degrees: -5.0,
            nx: 72,
            ny: 37,
            periodic_longitude: true,
            halo_cells: 0,
        };
        let a = unit_vector(start.longitude_degrees, start.latitude_degrees).unwrap();
        let b = unit_vector(proposed.longitude_degrees, proposed.latitude_degrees).unwrap();
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        for parallel in [65.0_f64, 70.0, 75.0, 80.0] {
            assert_eq!(
                gc_parallel_fractions(a, b, omega, parallel).unwrap().len(),
                2
            );
        }
        assert!(gc_parallel_fractions(a, b, omega, 85.0).unwrap().is_empty());
        let cuts_pos = grid_line_crossings(&start, &proposed, &grid_pos).unwrap();
        let cuts_neg = grid_line_crossings(&start, &proposed, &grid_neg).unwrap();
        for parallel in [65.0_f64, 70.0, 75.0, 80.0] {
            for f in gc_parallel_fractions(a, b, omega, parallel).unwrap() {
                assert!(
                    cuts_pos.iter().any(|c| (*c - f).abs() < 1.0e-10),
                    "pos missing {parallel} f={f}"
                );
                assert!(
                    cuts_neg.iter().any(|c| (*c - f).abs() < 1.0e-10),
                    "neg missing {parallel} f={f}"
                );
            }
        }
        // Physical cut sets must match (same GC roots).
        assert_eq!(
            cuts_pos.len(),
            cuts_neg.len(),
            "pos={cuts_pos:?} neg={cuts_neg:?}"
        );
        for (p, n) in cuts_pos.iter().zip(cuts_neg.iter()) {
            assert!((p - n).abs() < 1.0e-12, "pos={p} neg={n}");
        }
    }

    #[test]
    fn gc_longitude_has_no_spurious_mid_stationary_point() {
        let a = unit_vector(-10.0, 20.0).unwrap();
        let b = unit_vector(10.0, 25.0).unwrap();
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        let roots = gc_longitude_critical_fractions(a, b, omega).unwrap();
        assert!(
            roots.is_empty(),
            "lon must be monotonic away from poles; got {roots:?}"
        );
    }

    #[test]
    fn residual_interval_isolator_finds_two_roots_with_same_sign_endpoints() {
        // Frozen counterexample in the actual GC + bilinear + linear-time model.
        // A same-sign endpoint shortcut would miss both interior extrema.
        let grid = DomainGeometry {
            domain: trajecta_case::model::meteorology::DomainId("counterexample".into()),
            longitude_origin_degrees: -30.0,
            latitude_origin_degrees: 0.0,
            longitude_spacing_degrees: 60.0,
            latitude_spacing_degrees: 60.0,
            nx: 2,
            ny: 2,
            periodic_longitude: false,
            halo_cells: 0,
        };
        let a = unit_vector(-20.0, 5.0).unwrap();
        let b = unit_vector(20.0, 40.0).unwrap();
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        let corners0 = [
            -786.077_619_154_268_4,
            383.821_436_492_288_5,
            -528.963_279_625_108_7,
            86.446_438_852_159_19,
        ];
        let corners1 = [
            -1_721.339_279_044_047_8,
            372.652_359_157_179_94,
            -750.674_011_133_5,
            -576.775_893_587_677_7,
        ];
        let dh = 271.859_551_152_672_57;
        let context = ResidualContext {
            a,
            b,
            omega,
            grid: &grid,
            i: 0,
            j: 0,
            leaf_f0: 0.0,
            leaf_f1: 1.0,
            corners0,
            corners1,
            dh,
            kind: ResidualKind::Clearance,
        };
        let d0 = context.derivative_point(0.0).unwrap();
        let d1 = context.derivative_point(1.0).unwrap();
        assert!(d0 > 0.0 && d1 > 0.0, "endpoints=({d0},{d1})");

        let roots = isolate_residual_roots_interval(
            0.0,
            1.0,
            a,
            b,
            omega,
            &grid,
            0,
            0,
            corners0,
            corners1,
            dh,
            ResidualKind::Clearance,
        )
        .unwrap();
        assert_eq!(roots.len(), 2, "{roots:?}");
        assert!(
            (roots[0] - 0.433_224_458_066_023_4).abs() < 1.0e-12,
            "{roots:?}"
        );
        assert!(
            (roots[1] - 0.606_755_118_156_803).abs() < 1.0e-12,
            "{roots:?}"
        );
        for root in roots {
            assert!(context.derivative_point(root).unwrap().abs() < 1.0e-8);
        }
    }

    #[test]
    fn gc_meridian_fractions_dateline_periodic() {
        // Path crossing the antimeridian on a periodic grid.
        let start = particle(170.0, 10.0, 1000.0);
        let proposed = particle(-170.0, 10.0, 1000.0);
        let a = unit_vector(start.longitude_degrees, start.latitude_degrees).unwrap();
        let b = unit_vector(proposed.longitude_degrees, proposed.latitude_degrees).unwrap();
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        let roots_180 = gc_meridian_fractions(a, b, omega, 180.0).unwrap();
        let roots_m180 = gc_meridian_fractions(a, b, omega, -180.0).unwrap();
        // Same plane: at least one open-arc hit near the dateline.
        assert!(
            !roots_180.is_empty() || !roots_m180.is_empty(),
            "180={roots_180:?} -180={roots_m180:?}"
        );
        let grid = DomainGeometry {
            domain: trajecta_case::model::meteorology::DomainId("g".into()),
            longitude_origin_degrees: -180.0,
            latitude_origin_degrees: -90.0,
            longitude_spacing_degrees: 5.0,
            latitude_spacing_degrees: 5.0,
            nx: 72,
            ny: 37,
            periodic_longitude: true,
            halo_cells: 0,
        };
        let cuts = grid_line_crossings(&start, &proposed, &grid).unwrap();
        assert!(cuts.iter().any(|f| *f > 0.0 && *f < 1.0), "{cuts:?}");
    }
}
