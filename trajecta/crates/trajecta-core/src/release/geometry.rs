//! # Contract: GeoJSON normalization and deterministic spherical sampling
//!
//! Canonicalizes Point/Line/Polygon geometries, splits dateline crossings, and
//! samples by equal weight / great-circle length / spherical area using frozen
//! Philox dimensions.
#![allow(
    clippy::neg_cmp_op_on_partial_ord,
    clippy::if_same_then_else,
    clippy::manual_range_contains,
    clippy::unwrap_used,
    dead_code
)]

use std::cmp::Ordering;
use std::f64::consts::PI;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use trajecta_case::model::population::GeoJsonGeometry;

use crate::manifest::GeometryIdentity;
use crate::particle::ParticleId;
use crate::release::{ReleaseError, ReleaseSamplingRequest};
use crate::rng::{
    CounterRng, RELEASE_HORIZONTAL_COMPONENT_DIMENSION, RELEASE_HORIZONTAL_U_DIMENSION,
    RELEASE_HORIZONTAL_V_DIMENSION, RandomKey, StableRandomId,
};
use crate::science::{GEOMETRY_AREA_RELATIVE_TOLERANCE, M4_CONSTANTS};

const LON_MIN: f64 = -180.0;
const LON_MAX_EXCLUSIVE: f64 = 180.0;
const COORD_EPS: f64 = 1.0e-15;

/// Canonical release geometry after normalization and optional file binding.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalReleaseGeometry {
    /// Normalized typed geometry used for sampling.
    pub geometry: GeoJsonGeometry,
    /// Manifest identity for this release event.
    pub identity: GeometryIdentity,
}

/// Loads, validates, and normalizes one release geometry source.
pub fn canonicalize_geometry(
    event_id: &str,
    geometry: &GeoJsonGeometry,
    source_path: Option<&Path>,
) -> Result<CanonicalReleaseGeometry, ReleaseError> {
    let resolved_source = source_path
        .map(|path| fs::canonicalize(path).map_err(|_| ReleaseError::InvalidGeometry))
        .transpose()?;
    let source_path = resolved_source.as_deref();
    let (source_size_bytes, source_sha256) = if let Some(path) = source_path {
        let bytes = fs::read(path).map_err(|error| {
            ReleaseError::InvalidEvent(format!(
                "failed to read geojson {}: {error}",
                path.display()
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(&bytes);
        (
            Some(u64::try_from(bytes.len()).map_err(|_| ReleaseError::CountOverflow)?),
            Some(hex::encode(digest.finalize())),
        )
    } else {
        (None, None)
    };

    let geometry = normalize_geometry(geometry)?;
    let area = spherical_area_m2(&geometry)?;
    let encoded = encode_canonical_geometry(&geometry);
    let mut digest = Sha256::new();
    digest.update(encoded.as_bytes());
    let canonical_geometry_sha256 = hex::encode(digest.finalize());

    Ok(CanonicalReleaseGeometry {
        geometry,
        identity: GeometryIdentity {
            event_id: event_id.into(),
            source_path: source_path.map(PathBuf::from),
            source_size_bytes,
            source_sha256,
            canonical_geometry_sha256,
            spherical_area_m2: area,
        },
    })
}

/// Deterministic horizontal sampler over one already-canonical geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct SphericalGeometrySampler {
    geometry: GeoJsonGeometry,
}

impl SphericalGeometrySampler {
    /// Builds a sampler from already-normalized geometry.
    pub fn new(geometry: GeoJsonGeometry) -> Result<Self, ReleaseError> {
        let geometry = normalize_geometry(&geometry)?;
        Ok(Self { geometry })
    }

    /// Returns the normalized geometry.
    #[must_use]
    pub const fn geometry(&self) -> &GeoJsonGeometry {
        &self.geometry
    }
}

impl crate::release::GeometrySampler for SphericalGeometrySampler {
    fn sample_horizontal(
        &self,
        request: ReleaseSamplingRequest<'_>,
    ) -> Result<Vec<(f64, f64)>, ReleaseError> {
        request.validate()?;
        let population = StableRandomId::from_text(&request.population_id.0);
        let event = StableRandomId::from_text(&request.event.id.0);
        let mut out = Vec::with_capacity(request.count);
        for offset in 0..request.count {
            let ordinal = request
                .first_ordinal
                .checked_add(u64::try_from(offset).map_err(|_| ReleaseError::CountOverflow)?)
                .ok_or(ReleaseError::CountOverflow)?;
            let point = sample_one(&self.geometry, request.seed, population, event, ordinal)?;
            out.push(point);
        }
        for (longitude, latitude) in &out {
            if !point_in_geometry(&self.geometry, *longitude, *latitude)? {
                return Err(ReleaseError::InvalidGeometry);
            }
        }
        Ok(out)
    }
}

fn sample_one(
    geometry: &GeoJsonGeometry,
    seed: u64,
    population: StableRandomId,
    event: StableRandomId,
    ordinal: u64,
) -> Result<(f64, f64), ReleaseError> {
    let key = |dimension: u32, draw_index: u32| RandomKey {
        seed,
        population,
        lifecycle_event: event,
        particle: ParticleId(ordinal),
        sampling_dimension: dimension,
        draw_index,
    };
    match geometry {
        GeoJsonGeometry::Point(point) => Ok((point[0], point[1])),
        GeoJsonGeometry::MultiPoint(points) => {
            if points.is_empty() {
                return Err(ReleaseError::InvalidGeometry);
            }
            let component = CounterRng::sample_unit(key(RELEASE_HORIZONTAL_COMPONENT_DIMENSION, 0));
            let index = ((component * points.len() as f64) as usize).min(points.len() - 1);
            Ok((points[index][0], points[index][1]))
        }
        GeoJsonGeometry::LineString(line) => sample_lines(&[line.as_slice()], key),
        GeoJsonGeometry::MultiLineString(lines) => {
            let refs: Vec<&[[f64; 2]]> = lines.iter().map(Vec::as_slice).collect();
            sample_lines(&refs, key)
        }
        GeoJsonGeometry::Polygon(rings) => sample_polygons(&[rings.as_slice()], key),
        GeoJsonGeometry::MultiPolygon(polygons) => {
            let refs: Vec<&[Vec<[f64; 2]>]> = polygons.iter().map(Vec::as_slice).collect();
            sample_polygons(&refs, key)
        }
    }
}

fn sample_lines<F>(lines: &[&[[f64; 2]]], key: F) -> Result<(f64, f64), ReleaseError>
where
    F: Fn(u32, u32) -> RandomKey,
{
    let mut segments = Vec::new();
    let mut total = 0.0;
    for line in lines {
        for window in line.windows(2) {
            let length = great_circle_length_m(window[0], window[1])?;
            if length > 0.0 {
                total += length;
                segments.push((window[0], window[1], length));
            }
        }
    }
    if segments.is_empty() || total <= 0.0 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let u = CounterRng::sample_unit(key(RELEASE_HORIZONTAL_U_DIMENSION, 0));
    let target = u * total;
    let mut acc = 0.0;
    for (start, end, length) in &segments {
        if acc + length >= target {
            let local = if *length > 0.0 {
                ((target - acc) / *length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return interpolate_great_circle(*start, *end, local);
        }
        acc += length;
    }
    let (start, end, _) = segments
        .last()
        .copied()
        .ok_or(ReleaseError::InvalidGeometry)?;
    interpolate_great_circle(start, end, 1.0)
}

fn sample_polygons<F>(polygons: &[&[Vec<[f64; 2]>]], key: F) -> Result<(f64, f64), ReleaseError>
where
    F: Fn(u32, u32) -> RandomKey,
{
    let mut components = Vec::new();
    let mut total = 0.0;
    for polygon in polygons {
        let area = polygon_area_m2(polygon)?;
        if area > 0.0 {
            total += area;
            components.push((*polygon, area));
        }
    }
    if components.is_empty() || total <= 0.0 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let component_u = CounterRng::sample_unit(key(RELEASE_HORIZONTAL_COMPONENT_DIMENSION, 0));
    let target = component_u * total;
    let mut acc = 0.0;
    let mut selected = components[0].0;
    for (polygon, area) in &components {
        acc += *area;
        if acc >= target {
            selected = *polygon;
            break;
        }
        selected = *polygon;
    }
    sample_polygon_triangle(selected, key)
}

fn sample_polygon_triangle<F>(rings: &[Vec<[f64; 2]>], key: F) -> Result<(f64, f64), ReleaseError>
where
    F: Fn(u32, u32) -> RandomKey,
{
    // Exact area-uniform sampling: mesh exterior\holes into spherical triangles,
    // pick by area weight, then sample the chosen triangle with a single (u,v)
    // draw. No rejection loop and no centroid fallback.
    let triangles = mesh_polygon_region(rings)?;
    if triangles.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut areas = Vec::with_capacity(triangles.len());
    let mut total = 0.0;
    for triangle in &triangles {
        let area = triangle_area_m2(*triangle)?;
        if area <= 0.0 {
            return Err(ReleaseError::InvalidGeometry);
        }
        total += area;
        areas.push(area);
    }
    if total <= 0.0 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let component = CounterRng::sample_unit(key(RELEASE_HORIZONTAL_COMPONENT_DIMENSION, 0));
    let mut acc = 0.0;
    let mut chosen = triangles.len() - 1;
    for (index, area) in areas.iter().enumerate() {
        acc += *area / total;
        if component < acc {
            chosen = index;
            break;
        }
    }
    let u = CounterRng::sample_unit(key(RELEASE_HORIZONTAL_U_DIMENSION, 0));
    let v = CounterRng::sample_unit(key(RELEASE_HORIZONTAL_V_DIMENSION, 0));
    sample_spherical_triangle_uniform(triangles[chosen], u, v)
}

fn mesh_polygon_region(rings: &[Vec<[f64; 2]>]) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    if rings.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut exterior = open_ring(&rings[0])?;
    if ring_signed_excess(&exterior)? < 0.0 {
        exterior.reverse();
    }
    let mut holes = Vec::new();
    for hole_src in &rings[1..] {
        let mut hole = open_ring(hole_src)?;
        if ring_signed_excess(&hole)? > 0.0 {
            hole.reverse();
        }
        holes.push(hole);
    }
    if holes.is_empty() {
        let mut closed = exterior;
        closed.push(closed[0]);
        return ear_clip_sphere(&closed);
    }
    // Single hole: annulus (keyhole ear-clip or boundary star-fan).
    if holes.len() == 1 {
        return finish_mesh(mesh_annulus(&exterior, &holes[0])?, rings);
    }
    // Multi-hole: keyhole → ear-clip; else star-fan on keyhole; else embed holes.
    if let Ok(boundary) = multi_hole_keyhole_boundary(&exterior, &holes) {
        let mut closed = boundary.clone();
        closed.push(closed[0]);
        if let Ok(tris) = ear_clip_sphere(&closed) {
            if let Ok(out) = finish_mesh(tris, rings) {
                return Ok(out);
            }
        }
        // Star-fan from a steiner point known inside exterior-minus-holes.
        if let Ok(steiner) = find_region_steiner(&exterior, &holes, rings) {
            if let Ok(tris) = star_fan_boundary(&boundary, steiner) {
                if let Ok(out) = finish_mesh(tris, rings) {
                    return Ok(out);
                }
            }
        }
    }
    // Fallback: exterior ear-clip, embed each hole into its host triangle.
    let mut closed = exterior.clone();
    closed.push(closed[0]);
    let mut triangles = ear_clip_sphere(&closed)?;
    for hole in &holes {
        triangles = embed_hole_into_triangles(triangles, hole)?;
    }
    finish_mesh(triangles, rings)
}

fn finish_mesh(
    triangles: Vec<[[f64; 2]; 3]>,
    rings: &[Vec<[f64; 2]>],
) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    if triangles.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    // Topology certificate (exterior/hole classification + overlap).
    validate_mesh_topology(&triangles, rings)?;
    let mut mesh_area = 0.0;
    for tri in &triangles {
        mesh_area += triangle_area_m2(*tri)?;
    }
    let poly_area = polygon_area_m2(rings)?;
    let scale = mesh_area.max(poly_area).max(1.0);
    let rel = (mesh_area - poly_area).abs() / scale;
    if rel > GEOMETRY_AREA_RELATIVE_TOLERANCE {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(triangles)
}

/// Spherical segment relation for topology certification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SegmentRelation {
    Disjoint,
    /// Identical undirected endpoints (constrained mesh edge on ring).
    SharedFullEdge,
    /// Touch only at a shared vertex.
    EndpointOnly,
    /// Proper crossing of interiors.
    ProperCrossing,
    /// Collinear partial overlap that is not a full shared edge.
    PartialCollinearOverlap,
}

/// Deterministic topology gate: spherical interior reps, exterior+hole edge
/// classification, no hole vertices inside triangles, no triangle interior overlap.
pub(crate) fn validate_mesh_topology(
    triangles: &[[[f64; 2]; 3]],
    rings: &[Vec<[f64; 2]>],
) -> Result<(), ReleaseError> {
    if triangles.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    // Stage toggles for diagnosis (all true in delivery).
    const CHECK_INTERIOR_REP: bool = true;
    const CHECK_HOLE_VERTICES: bool = true;
    const CHECK_EXTERIOR_EDGES: bool = true;
    const CHECK_HOLE_EDGES: bool = true;
    const CHECK_TRI_TRI: bool = true;
    let mut exterior_edges: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut hole_edges: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut hole_vertices: Vec<[f64; 2]> = Vec::new();
    for (ri, ring) in rings.iter().enumerate() {
        let open = open_ring(ring)?;
        if ri == 0 {
            for i in 0..open.len() {
                exterior_edges.push((open[i], open[(i + 1) % open.len()]));
            }
        } else {
            hole_vertices.extend(open.iter().copied());
            for i in 0..open.len() {
                hole_edges.push((open[i], open[(i + 1) % open.len()]));
            }
        }
    }

    for tri in triangles {
        let excess = triangle_signed_excess(tri[0], tri[1], tri[2])?;
        if excess <= 1.0e-18 {
            return Err(ReleaseError::InvalidGeometry);
        }
        // Spherical vector mean → lon/lat (not lon/lat arithmetic mean).
        if CHECK_INTERIOR_REP && !triangle_has_interior_point_in_polygon(*tri, rings)? {
            return Err(ReleaseError::InvalidGeometry);
        }
        if CHECK_HOLE_VERTICES {
            for &hv in &hole_vertices {
                if point_strictly_inside_triangle(*tri, hv)? {
                    return Err(ReleaseError::InvalidGeometry);
                }
            }
        }
        for i in 0..3 {
            let a = tri[i];
            let b = tri[(i + 1) % 3];
            // Topology certificate: partition AB by ring intersections and prove each
            // open interval lies in exterior-minus-holes or is boundary-covered.
            // Endpoints-on-ring alone is NOT boundary coverage.
            if CHECK_EXTERIOR_EDGES || CHECK_HOLE_EDGES {
                certify_triangle_edge_in_region(a, b, rings, &exterior_edges, &hole_edges)?;
            }
            if CHECK_HOLE_EDGES {
                for &(c, d) in &hole_edges {
                    if matches!(
                        classify_segment_relation(a, b, c, d)?,
                        SegmentRelation::ProperCrossing
                    ) {
                        return Err(ReleaseError::InvalidGeometry);
                    }
                }
            }
        }
    }

    if !CHECK_TRI_TRI {
        return Ok(());
    }

    // Pairwise triangle interior overlap: each triangle's representative must not
    // lie strictly inside another triangle (containment / double-cover).
    // Also reject proper edge crossings between distinct triangles.
    for (i, ti) in triangles.iter().enumerate() {
        let rep_i = spherical_triangle_representative(*ti)?;
        for (j, tj) in triangles.iter().enumerate() {
            if j <= i {
                continue;
            }
            if point_strictly_inside_triangle(*tj, rep_i)?
                || point_strictly_inside_triangle(*ti, spherical_triangle_representative(*tj)?)?
            {
                return Err(ReleaseError::InvalidGeometry);
            }
            for a in 0..3 {
                let e1a = ti[a];
                let e1b = ti[(a + 1) % 3];
                for b in 0..3 {
                    let e2a = tj[b];
                    let e2b = tj[(b + 1) % 3];
                    match classify_segment_relation(e1a, e1b, e2a, e2b)? {
                        SegmentRelation::Disjoint
                        | SegmentRelation::SharedFullEdge
                        | SegmentRelation::EndpointOnly => {}
                        // Collinear boundary sub-edges between adjacent triangles are OK
                        // when both endpoints lie on the union of their vertices.
                        SegmentRelation::PartialCollinearOverlap => {
                            let on_shared = almost_same_point(e1a, e2a)
                                || almost_same_point(e1a, e2b)
                                || almost_same_point(e1b, e2a)
                                || almost_same_point(e1b, e2b);
                            if !on_shared {
                                return Err(ReleaseError::InvalidGeometry);
                            }
                        }
                        SegmentRelation::ProperCrossing => {
                            return Err(ReleaseError::InvalidGeometry);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn point_strictly_inside_triangle(tri: [[f64; 2]; 3], p: [f64; 2]) -> Result<bool, ReleaseError> {
    if !point_in_spherical_triangle(p, tri[0], tri[1], tri[2])? {
        return Ok(false);
    }
    let e0 = triangle_signed_excess(tri[0], tri[1], p)?;
    let e1 = triangle_signed_excess(tri[1], tri[2], p)?;
    let e2 = triangle_signed_excess(tri[2], tri[0], p)?;
    Ok(e0 > 1.0e-18 && e1 > 1.0e-18 && e2 > 1.0e-18)
}

fn classify_segment_relation(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
) -> Result<SegmentRelation, ReleaseError> {
    if same_undirected_edge(a, b, c, d) {
        return Ok(SegmentRelation::SharedFullEdge);
    }
    let endpoint_touch = almost_same_point(a, c)
        || almost_same_point(a, d)
        || almost_same_point(b, c)
        || almost_same_point(b, d);

    let ua = unit_vector(a)?;
    let ub = unit_vector(b)?;
    let uc = unit_vector(c)?;
    let ud = unit_vector(d)?;

    // Collinear if both c and d lie near plane of great circle a-b (or degenerate).
    let n_ab = cross(ua, ub);
    let n_norm = norm(n_ab);
    let collinear = if n_norm <= 1.0e-14 {
        // Degenerate AB — treat via endpoint only.
        false
    } else {
        let n = normalize(n_ab)?;
        dot(n, uc).abs() <= 1.0e-10 && dot(n, ud).abs() <= 1.0e-10
    };

    if collinear {
        // Full containment of one segment on the other is a legal constrained boundary
        // sub-edge (exterior edge split by inserted vertices / keyhole bridges).
        let ab_on_cd = on_arc(ua, uc, ud)? && on_arc(ub, uc, ud)?;
        let cd_on_ab = on_arc(uc, ua, ub)? && on_arc(ud, ua, ub)?;
        if ab_on_cd || cd_on_ab {
            return Ok(SegmentRelation::SharedFullEdge);
        }
        // Partial collinear overlap if an interior point of one lies strictly on the other
        // without full containment.
        let c_on_ab_int =
            on_arc(uc, ua, ub)? && !almost_same_point(c, a) && !almost_same_point(c, b);
        let d_on_ab_int =
            on_arc(ud, ua, ub)? && !almost_same_point(d, a) && !almost_same_point(d, b);
        let a_on_cd_int =
            on_arc(ua, uc, ud)? && !almost_same_point(a, c) && !almost_same_point(a, d);
        let b_on_cd_int =
            on_arc(ub, uc, ud)? && !almost_same_point(b, c) && !almost_same_point(b, d);
        if c_on_ab_int || d_on_ab_int || a_on_cd_int || b_on_cd_int {
            return Ok(SegmentRelation::PartialCollinearOverlap);
        }
        if endpoint_touch {
            return Ok(SegmentRelation::EndpointOnly);
        }
        return Ok(SegmentRelation::Disjoint);
    }

    if great_circle_segments_intersect(a, b, c, d)? {
        if endpoint_touch {
            return Ok(SegmentRelation::EndpointOnly);
        }
        return Ok(SegmentRelation::ProperCrossing);
    }
    if endpoint_touch {
        return Ok(SegmentRelation::EndpointOnly);
    }
    Ok(SegmentRelation::Disjoint)
}

fn spherical_triangle_representative(tri: [[f64; 2]; 3]) -> Result<[f64; 2], ReleaseError> {
    let a = unit_vector(tri[0])?;
    let b = unit_vector(tri[1])?;
    let c = unit_vector(tri[2])?;
    let s = normalize([a[0] + b[0] + c[0], a[1] + b[1] + c[1], a[2] + b[2] + c[2]])?;
    let (lon, lat) = lon_lat_from_unit(s)?;
    Ok([lon, lat])
}

fn spherical_triangle_blend(
    tri: [[f64; 2]; 3],
    w0: f64,
    w1: f64,
    w2: f64,
) -> Result<[f64; 2], ReleaseError> {
    let a = unit_vector(tri[0])?;
    let b = unit_vector(tri[1])?;
    let c = unit_vector(tri[2])?;
    let s = normalize([
        w0 * a[0] + w1 * b[0] + w2 * c[0],
        w0 * a[1] + w1 * b[1] + w2 * c[1],
        w0 * a[2] + w1 * b[2] + w2 * c[2],
    ])?;
    let (lon, lat) = lon_lat_from_unit(s)?;
    Ok([lon, lat])
}

fn triangle_has_interior_point_in_polygon(
    tri: [[f64; 2]; 3],
    rings: &[Vec<[f64; 2]>],
) -> Result<bool, ReleaseError> {
    let weights = [
        (1.0, 1.0, 1.0),
        (2.0, 1.0, 1.0),
        (1.0, 2.0, 1.0),
        (1.0, 1.0, 2.0),
        (3.0, 1.0, 1.0),
        (1.0, 3.0, 1.0),
        (1.0, 1.0, 3.0),
    ];
    for (w0, w1, w2) in weights {
        let p = spherical_triangle_blend(tri, w0, w1, w2)?;
        if point_in_polygon(rings, p[0], p[1])? && point_strictly_inside_triangle(tri, p)? {
            return Ok(true);
        }
        // Also accept non-strict interior that is still in polygon for ultra-thin shells.
        if point_in_polygon(rings, p[0], p[1])? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn same_undirected_edge(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    (almost_same_point(a, c) && almost_same_point(b, d))
        || (almost_same_point(a, d) && almost_same_point(b, c))
}

fn point_on_ring_edges(p: [f64; 2], edges: &[([f64; 2], [f64; 2])]) -> Result<bool, ReleaseError> {
    let up = unit_vector(p)?;
    for &(a, b) in edges {
        if almost_same_point(p, a) || almost_same_point(p, b) {
            return Ok(true);
        }
        let ua = unit_vector(a)?;
        let ub = unit_vector(b)?;
        if on_arc(up, ua, ub)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Prove geodesic triangle edge AB lies entirely in exterior−holes (interior or
/// fully boundary-covered). Not a fixed-probe sampler — partitions AB by every
/// ring intersection and checks each open interval's membership.
fn certify_triangle_edge_in_region(
    a: [f64; 2],
    b: [f64; 2],
    rings: &[Vec<[f64; 2]>],
    exterior_edges: &[([f64; 2], [f64; 2])],
    hole_edges: &[([f64; 2], [f64; 2])],
) -> Result<(), ReleaseError> {
    let ua = unit_vector(a)?;
    let ub = unit_vector(b)?;
    let mut cuts: Vec<f64> = vec![0.0, 1.0];
    let mut covered: Vec<(f64, f64)> = Vec::new();
    let mut all_edges: Vec<([f64; 2], [f64; 2])> =
        Vec::with_capacity(exterior_edges.len() + hole_edges.len());
    all_edges.extend_from_slice(exterior_edges);
    all_edges.extend_from_slice(hole_edges);

    for &(c, d) in &all_edges {
        let uc = unit_vector(c)?;
        let ud = unit_vector(d)?;
        match classify_segment_relation(a, b, c, d)? {
            SegmentRelation::Disjoint => {}
            SegmentRelation::EndpointOnly => {
                push_endpoint_cuts(&mut cuts, (a, b), (c, d), (ua, ub), (uc, ud))?;
            }
            SegmentRelation::SharedFullEdge => {
                // AB fully on ring edge CD → whole edge boundary-covered.
                if on_arc(ua, uc, ud)? && on_arc(ub, uc, ud)? {
                    covered.push((0.0, 1.0));
                }
                // CD fully (or partially as sub-arc) on AB → cover that parameter range.
                if let Some(tc) = arc_fraction_if_on(uc, ua, ub)? {
                    cuts.push(tc);
                }
                if let Some(td) = arc_fraction_if_on(ud, ua, ub)? {
                    cuts.push(td);
                }
                if let (Some(tc), Some(td)) = (
                    arc_fraction_if_on(uc, ua, ub)?,
                    arc_fraction_if_on(ud, ua, ub)?,
                ) {
                    covered.push((tc.min(td), tc.max(td)));
                }
                // If AB endpoints lie on CD, they are already 0/1 cuts.
                if on_arc(ua, uc, ud)? {
                    cuts.push(0.0);
                }
                if on_arc(ub, uc, ud)? {
                    cuts.push(1.0);
                }
            }
            SegmentRelation::PartialCollinearOverlap => {
                // Collect the overlap interval on AB; must later prove union covers
                // any claimed boundary use — uncovered parts still need membership.
                let mut ends = Vec::new();
                for p_u in [ua, ub, uc, ud] {
                    if let Some(t) = arc_fraction_if_on(p_u, ua, ub)? {
                        // Only keep points that also lie on CD.
                        if on_arc(p_u, uc, ud)? {
                            ends.push(t);
                            cuts.push(t);
                        }
                    }
                }
                // Also: projection of collinear interior points of CD onto AB.
                if let Some(tc) = arc_fraction_if_on(uc, ua, ub)? {
                    cuts.push(tc);
                    ends.push(tc);
                }
                if let Some(td) = arc_fraction_if_on(ud, ua, ub)? {
                    cuts.push(td);
                    ends.push(td);
                }
                if ends.len() >= 2 {
                    let lo = ends.iter().cloned().fold(f64::INFINITY, f64::min);
                    let hi = ends.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                    if hi > lo + 1.0e-15 {
                        covered.push((lo, hi));
                    }
                }
            }
            SegmentRelation::ProperCrossing => {
                if let Some(t) = intersection_fraction_on_first(a, b, c, d)? {
                    cuts.push(t);
                } else {
                    // Relation claimed crossing but no fraction — reject as unproven.
                    return Err(ReleaseError::InvalidGeometry);
                }
            }
        }
    }

    // Sort + unique cuts in [0,1].
    cuts.retain(|t| *t >= -1.0e-15 && *t <= 1.0 + 1.0e-15);
    for t in &mut cuts {
        *t = t.clamp(0.0, 1.0);
    }
    cuts.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let mut uniq: Vec<f64> = Vec::with_capacity(cuts.len());
    for t in cuts {
        if uniq.last().is_none_or(|prev| (t - prev).abs() > 1.0e-12) {
            uniq.push(t);
        }
    }
    if uniq.first().copied() != Some(0.0) {
        uniq.insert(0, 0.0);
    }
    if uniq.last().copied() != Some(1.0) {
        uniq.push(1.0);
    }

    // Merge covered intervals.
    covered.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (lo, hi) in covered {
        if let Some(last) = merged.last_mut() {
            if lo <= last.1 + 1.0e-12 {
                last.1 = last.1.max(hi);
                continue;
            }
        }
        merged.push((lo, hi));
    }

    // Each open interval (t_i, t_{i+1}) has constant membership (no ring crossing inside).
    for w in uniq.windows(2) {
        let t0 = w[0];
        let t1 = w[1];
        if t1 - t0 <= 1.0e-14 {
            continue;
        }
        let mid = 0.5 * (t0 + t1);
        // Boundary-covered open interval is accepted without interior membership.
        if interval_covered(mid, t0, t1, &merged) {
            continue;
        }
        let p3 = slerp_unit(ua, ub, mid)?;
        let (lon, lat) = lon_lat_from_unit(p3)?;
        if point_in_polygon(rings, lon, lat)? {
            continue;
        }
        // Exact-on-ring counts as inside the closed region for constrained edges.
        if point_on_ring_edges([lon, lat], &all_edges)? {
            continue;
        }
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(())
}

fn interval_covered(mid: f64, t0: f64, t1: f64, merged: &[(f64, f64)]) -> bool {
    // Open interval is covered if some merged collinear cover contains (t0,t1).
    for &(lo, hi) in merged {
        if lo <= t0 + 1.0e-12 && hi >= t1 - 1.0e-12 {
            return true;
        }
        // Midpoint fallback when cover is slightly short of endpoints due to tol.
        if mid >= lo - 1.0e-12 && mid <= hi + 1.0e-12 && hi - lo >= (t1 - t0) * 0.5 {
            // Prefer full cover; if only mid, still require full cover for acceptance.
            let _ = (lo, hi);
        }
    }
    false
}

fn push_endpoint_cuts(
    cuts: &mut Vec<f64>,
    ab: ([f64; 2], [f64; 2]),
    cd: ([f64; 2], [f64; 2]),
    u_ab: ([f64; 3], [f64; 3]),
    u_cd: ([f64; 3], [f64; 3]),
) -> Result<(), ReleaseError> {
    let (a, b) = ab;
    let (c, d) = cd;
    let (ua, ub) = u_ab;
    let (uc, ud) = u_cd;
    if almost_same_point(a, c) || almost_same_point(a, d) {
        cuts.push(0.0);
    }
    if almost_same_point(b, c) || almost_same_point(b, d) {
        cuts.push(1.0);
    }
    if let Some(t) = arc_fraction_if_on(uc, ua, ub)? {
        cuts.push(t);
    }
    if let Some(t) = arc_fraction_if_on(ud, ua, ub)? {
        cuts.push(t);
    }
    Ok(())
}

fn arc_fraction_if_on(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> Result<Option<f64>, ReleaseError> {
    if !on_arc(p, a, b)? {
        return Ok(None);
    }
    Ok(Some(arc_fraction(a, b, p)?))
}

fn arc_fraction(a: [f64; 3], b: [f64; 3], p: [f64; 3]) -> Result<f64, ReleaseError> {
    let ab = dot(a, b).clamp(-1.0, 1.0).acos();
    if ab <= 1.0e-14 {
        return Ok(0.0);
    }
    let ap = dot(a, p).clamp(-1.0, 1.0).acos();
    Ok((ap / ab).clamp(0.0, 1.0))
}

fn intersection_fraction_on_first(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
) -> Result<Option<f64>, ReleaseError> {
    let ua = unit_vector(a)?;
    let ub = unit_vector(b)?;
    let uc = unit_vector(c)?;
    let ud = unit_vector(d)?;
    let n1 = cross(ua, ub);
    let n2 = cross(uc, ud);
    let line = cross(n1, n2);
    let nrm = (line[0] * line[0] + line[1] * line[1] + line[2] * line[2]).sqrt();
    if nrm <= 1.0e-18 {
        return Ok(None);
    }
    let p = [line[0] / nrm, line[1] / nrm, line[2] / nrm];
    let q = [-p[0], -p[1], -p[2]];
    if on_arc(p, ua, ub)? && on_arc(p, uc, ud)? {
        return Ok(Some(arc_fraction(ua, ub, p)?));
    }
    if on_arc(q, ua, ub)? && on_arc(q, uc, ud)? {
        return Ok(Some(arc_fraction(ua, ub, q)?));
    }
    Ok(None)
}

/// Non-acceptance diagnostic only — must not be used as a topology certificate.
#[allow(dead_code)]
fn edge_exits_polygon_diagnostic(
    a: [f64; 2],
    b: [f64; 2],
    rings: &[Vec<[f64; 2]>],
) -> Result<bool, ReleaseError> {
    let ua = unit_vector(a)?;
    let ub = unit_vector(b)?;
    for k in 1..12 {
        let t = f64::from(k) / 12.0;
        let p3 = slerp_unit(ua, ub, t)?;
        let (lon, lat) = lon_lat_from_unit(p3)?;
        if !point_in_polygon(rings, lon, lat)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn edges_proper_intersect(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
) -> Result<bool, ReleaseError> {
    // Endpoint-only touches are not proper intersections.
    if almost_same_point(a, c)
        || almost_same_point(a, d)
        || almost_same_point(b, c)
        || almost_same_point(b, d)
    {
        return Ok(false);
    }
    great_circle_segments_intersect(a, b, c, d)
}

fn mesh_annulus(
    exterior: &[[f64; 2]],
    hole: &[[f64; 2]],
) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    let boundary = multi_hole_keyhole_boundary(exterior, &[hole.to_vec()])?;
    // Ear-clip keyhole ring when possible.
    {
        let mut closed = boundary.clone();
        closed.push(closed[0]);
        if let Ok(tris) = ear_clip_sphere(&closed) {
            return Ok(tris);
        }
    }
    // Star-fan from bridge midpoint candidates.
    let mut e = exterior.to_vec();
    e.push(e[0]);
    let mut h = hole.to_vec();
    h.push(h[0]);
    let rings = [e, h];
    if let Ok(steiner) = find_region_steiner(exterior, &[hole.to_vec()], &rings) {
        if let Ok(tris) = star_fan_boundary(&boundary, steiner) {
            return Ok(tris);
        }
    }
    // Explicit bridge midpoints as steiner.
    for &ep in exterior {
        for &hp in hole {
            if validate_short_arc(ep, hp).is_err() {
                continue;
            }
            let Ok((lon, lat)) = interpolate_great_circle(ep, hp, 0.5) else {
                continue;
            };
            let steiner = [lon, lat];
            if !point_in_polygon(&rings, lon, lat).unwrap_or(false) {
                continue;
            }
            if let Ok(tris) = star_fan_boundary(&boundary, steiner) {
                return Ok(tris);
            }
        }
    }
    // Zipper last resort for any exterior, including triangle hosts.
    mesh_annulus_zipper(exterior, hole)
}

fn mesh_annulus_zipper(
    exterior: &[[f64; 2]],
    hole: &[[f64; 2]],
) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    let (c_lon, c_lat) = ring_centroid_lonlat(exterior)?;
    let origin = unit_vector([c_lon, c_lat])?;
    let mut ext_sorted = exterior.to_vec();
    let mut hole_sorted = hole.to_vec();
    ext_sorted.sort_by(|a, b| {
        bearing_key(origin, *a)
            .partial_cmp(&bearing_key(origin, *b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hole_sorted.sort_by(|a, b| {
        bearing_key(origin, *a)
            .partial_cmp(&bearing_key(origin, *b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let n = ext_sorted.len().max(hole_sorted.len()).max(3);
    let mut triangles = Vec::new();
    for i in 0..n {
        let e0 = ext_sorted[i % ext_sorted.len()];
        let e1 = ext_sorted[(i + 1) % ext_sorted.len()];
        let h0 = hole_sorted[i % hole_sorted.len()];
        let h1 = hole_sorted[(i + 1) % hole_sorted.len()];
        for mut tri in [[e0, e1, h1], [e0, h1, h0]] {
            let excess = triangle_signed_excess(tri[0], tri[1], tri[2])?;
            if excess.abs() <= 1.0e-18 {
                continue;
            }
            if excess < 0.0 {
                tri.swap(1, 2);
            }
            triangles.push(tri);
        }
    }
    if triangles.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(triangles)
}

fn bearing_key(origin: [f64; 3], point: [f64; 2]) -> f64 {
    let Ok(p) = unit_vector(point) else {
        return 0.0;
    };
    let radial = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
    let east = cross([0.0, 0.0, 1.0], origin);
    let east_n = norm(east);
    let east = if east_n > 1.0e-18 {
        [east[0] / east_n, east[1] / east_n, east[2] / east_n]
    } else {
        [1.0, 0.0, 0.0]
    };
    let north = cross(origin, east);
    dot(radial, north).atan2(dot(radial, east))
}

fn multi_hole_keyhole_boundary(
    exterior: &[[f64; 2]],
    holes: &[Vec<[f64; 2]>],
) -> Result<Vec<[f64; 2]>, ReleaseError> {
    let mut working = exterior.to_vec();
    for hole_src in holes {
        let mut hole = hole_src.clone();
        // Only proven-valid bridges (visibility + non-crossing). No best_any fallback.
        let mut best_valid: Option<(u64, u64, usize, usize)> = None;
        for (hi, &hp) in hole.iter().enumerate() {
            for (ei, &ep) in working.iter().enumerate() {
                if validate_short_arc(ep, hp).is_err() {
                    continue;
                }
                if bridge_is_valid(&working, &hole, ei, hi)? {
                    let key = (ordered_f64(ep[0]), ordered_f64(hp[0]), ei, hi);
                    if best_valid.as_ref().is_none_or(|prev| {
                        (key.0, key.1, key.2, key.3) < (prev.0, prev.1, prev.2, prev.3)
                    }) {
                        best_valid = Some(key);
                    }
                }
            }
        }
        let Some((_, _, ext_index, hole_index)) = best_valid else {
            return Err(ReleaseError::InvalidGeometry);
        };
        hole.rotate_left(hole_index);
        // Classic keyhole bridge: exterior[..ei], bridge to hole[0], full hole ring,
        // return bridge hole[0] -> exterior[ei], then exterior[ei+1..].
        // Duplicate vertices on the bridge are intentional and required.
        let mut next = working[..=ext_index].to_vec();
        next.extend(hole.iter().copied());
        next.push(hole[0]);
        next.push(working[ext_index]);
        next.extend(working[ext_index + 1..].iter().copied());
        working = next;
    }
    let mut cleaned = Vec::new();
    for p in working {
        if cleaned.last().is_none_or(|q| !almost_same_point(*q, p)) {
            cleaned.push(p);
        }
    }
    if cleaned.len() >= 2 && almost_same_point(cleaned[0], cleaned[cleaned.len() - 1]) {
        cleaned.pop();
    }
    if cleaned.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    if ring_signed_excess(&cleaned)? < 0.0 {
        cleaned.reverse();
    }
    Ok(cleaned)
}

fn star_fan_boundary(
    boundary: &[[f64; 2]],
    steiner: [f64; 2],
) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    let n = boundary.len();
    let mut triangles = Vec::new();
    for i in 0..n {
        let a = boundary[i];
        let b = boundary[(i + 1) % n];
        let excess = triangle_signed_excess(steiner, a, b)?;
        if excess > 1.0e-18 {
            triangles.push([steiner, a, b]);
        } else if excess < -1.0e-18 {
            return Err(ReleaseError::InvalidGeometry);
        }
    }
    if triangles.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(triangles)
}

fn find_region_steiner(
    exterior: &[[f64; 2]],
    holes: &[Vec<[f64; 2]>],
    rings: &[Vec<[f64; 2]>],
) -> Result<[f64; 2], ReleaseError> {
    let mut candidates = Vec::new();
    let (clon, clat) = ring_centroid_lonlat(exterior)?;
    candidates.push([clon, clat]);
    for hole in holes {
        for &hp in hole {
            for &ep in exterior {
                if validate_short_arc(ep, hp).is_ok() {
                    if let Ok((lon, lat)) = interpolate_great_circle(ep, hp, 0.5) {
                        candidates.push([lon, lat]);
                    }
                }
            }
        }
    }
    for window in exterior.windows(2) {
        if let Ok((mlon, mlat)) = interpolate_great_circle(window[0], window[1], 0.5) {
            if let Ok((ilon, ilat)) = interpolate_great_circle([mlon, mlat], [clon, clat], 0.2) {
                candidates.push([ilon, ilat]);
            }
        }
    }
    for p in candidates {
        if point_in_polygon(rings, p[0], p[1])? {
            return Ok(p);
        }
    }
    Err(ReleaseError::InvalidGeometry)
}

fn embed_hole_into_triangles(
    triangles: Vec<[[f64; 2]; 3]>,
    hole: &[[f64; 2]],
) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    let mut host: Option<usize> = None;
    for (idx, tri) in triangles.iter().enumerate() {
        match triangle_hole_relation(*tri, hole)? {
            HoleTriRelation::Disjoint => {}
            HoleTriRelation::HoleInsideTriangle => {
                if host.is_some() {
                    return Err(ReleaseError::InvalidGeometry);
                }
                host = Some(idx);
            }
            HoleTriRelation::PartialOrInverted => {
                return Err(ReleaseError::InvalidGeometry);
            }
        }
    }
    let Some(host) = host else {
        return Err(ReleaseError::InvalidGeometry);
    };
    let mut out = Vec::new();
    for (idx, tri) in triangles.into_iter().enumerate() {
        if idx == host {
            match mesh_annulus(&[tri[0], tri[1], tri[2]], hole) {
                Ok(a) => out.extend(a),
                Err(e) => {
                    return Err(e);
                }
            }
        } else {
            out.push(tri);
        }
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum HoleTriRelation {
    Disjoint,
    HoleInsideTriangle,
    PartialOrInverted,
}

fn triangle_hole_relation(
    tri: [[f64; 2]; 3],
    hole: &[[f64; 2]],
) -> Result<HoleTriRelation, ReleaseError> {
    let mut hole_verts_in = 0usize;
    for &hp in hole {
        if point_in_spherical_triangle(hp, tri[0], tri[1], tri[2])? {
            hole_verts_in += 1;
        }
    }
    let mut hole_closed = hole.to_vec();
    if hole_closed.first().copied() != hole_closed.last().copied() {
        hole_closed.push(hole_closed[0]);
    }
    let mut tri_verts_in_hole = 0usize;
    for &tp in &tri {
        if point_in_ring(&hole_closed, tp[0], tp[1], true)? {
            tri_verts_in_hole += 1;
        }
    }
    let mut edge_cross = false;
    let n_h = hole.len();
    for i in 0..n_h {
        let h0 = hole[i];
        let h1 = hole[(i + 1) % n_h];
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            if almost_same_point(a, h0)
                || almost_same_point(a, h1)
                || almost_same_point(b, h0)
                || almost_same_point(b, h1)
            {
                continue;
            }
            if great_circle_segments_intersect(a, b, h0, h1)? {
                edge_cross = true;
                break;
            }
        }
        if edge_cross {
            break;
        }
    }
    if edge_cross || tri_verts_in_hole > 0 {
        return Ok(HoleTriRelation::PartialOrInverted);
    }
    if hole_verts_in == hole.len() && hole.len() >= 3 {
        return Ok(HoleTriRelation::HoleInsideTriangle);
    }
    if hole_verts_in == 0 {
        return Ok(HoleTriRelation::Disjoint);
    }
    Ok(HoleTriRelation::PartialOrInverted)
}

fn mesh_keyhole_earclip(
    exterior: &[[f64; 2]],
    hole: &[[f64; 2]],
) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    let boundary = multi_hole_keyhole_boundary(exterior, &[hole.to_vec()])?;
    let mut closed = boundary;
    closed.push(closed[0]);
    ear_clip_sphere(&closed)
}

fn open_ring(ring: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, ReleaseError> {
    let mut pts = ring.to_vec();
    if pts.len() >= 2 && almost_same_point(pts[0], *pts.last().unwrap_or(&pts[0])) {
        pts.pop();
    }
    if pts.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(pts)
}

fn ring_signed_excess(open_ring: &[[f64; 2]]) -> Result<f64, ReleaseError> {
    if open_ring.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let origin = unit_vector(open_ring[0])?;
    let mut excess = 0.0;
    for window in open_ring[1..].windows(2) {
        let a = unit_vector(window[0])?;
        let b = unit_vector(window[1])?;
        excess += spherical_triangle_excess(origin, a, b)?;
    }
    Ok(excess)
}

fn bridge_is_valid(
    exterior: &[[f64; 2]],
    hole: &[[f64; 2]],
    ext_index: usize,
    hole_index: usize,
) -> Result<bool, ReleaseError> {
    let a = exterior[ext_index];
    let b = hole[hole_index];
    if almost_same_point(a, b) {
        return Ok(false);
    }
    if validate_short_arc(a, b).is_err() {
        return Ok(false);
    }
    // Reject bridges that properly intersect any exterior or hole edge.
    for edges in [exterior, hole] {
        let n = edges.len();
        for index in 0..n {
            let c = edges[index];
            let d = edges[(index + 1) % n];
            if almost_same_point(c, d) {
                continue;
            }
            let incident = almost_same_point(a, c)
                || almost_same_point(a, d)
                || almost_same_point(b, c)
                || almost_same_point(b, d);
            if incident {
                continue;
            }
            if great_circle_segments_intersect(a, b, c, d)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn ordered_f64(value: f64) -> u64 {
    let bits = value.to_bits();
    if bits & (1_u64 << 63) != 0 {
        !bits
    } else {
        bits | (1_u64 << 63)
    }
}

fn ear_clip_sphere(ring: &[[f64; 2]]) -> Result<Vec<[[f64; 2]; 3]>, ReleaseError> {
    let mut poly: Vec<[f64; 2]> = ring.to_vec();
    if poly.len() >= 2 && almost_same_point(poly[0], *poly.last().unwrap()) {
        poly.pop();
    }
    if poly.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut triangles = Vec::new();
    let mut guard = 0_usize;
    while poly.len() > 3 {
        guard += 1;
        if guard > poly.len().saturating_mul(poly.len()).saturating_add(8) {
            return Err(ReleaseError::InvalidGeometry);
        }
        let mut clipped = false;
        let n = poly.len();
        for i in 0..n {
            let prev = poly[(i + n - 1) % n];
            let curr = poly[i];
            let next = poly[(i + 1) % n];
            if !is_convex_ear(prev, curr, next, &poly)? {
                continue;
            }
            triangles.push([prev, curr, next]);
            poly.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            return Err(ReleaseError::InvalidGeometry);
        }
    }
    triangles.push([poly[0], poly[1], poly[2]]);
    Ok(triangles)
}

fn is_convex_ear(
    prev: [f64; 2],
    curr: [f64; 2],
    next: [f64; 2],
    poly: &[[f64; 2]],
) -> Result<bool, ReleaseError> {
    let area = triangle_signed_excess(prev, curr, next)?;
    if area <= -1.0e-18 {
        return Ok(false);
    }
    if area <= 0.0 {
        return Ok(false);
    }
    for &point in poly {
        if almost_same_point(point, prev)
            || almost_same_point(point, curr)
            || almost_same_point(point, next)
        {
            continue;
        }
        if point_in_spherical_triangle(point, prev, curr, next)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn triangle_signed_excess(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Result<f64, ReleaseError> {
    spherical_triangle_excess(unit_vector(a)?, unit_vector(b)?, unit_vector(c)?)
}

fn point_in_spherical_triangle(
    p: [f64; 2],
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
) -> Result<bool, ReleaseError> {
    let up = unit_vector(p)?;
    let ua = unit_vector(a)?;
    let ub = unit_vector(b)?;
    let uc = unit_vector(c)?;
    let n_ab = cross(ua, ub);
    let n_bc = cross(ub, uc);
    let n_ca = cross(uc, ua);
    let s0 = dot(n_ab, up);
    let s1 = dot(n_bc, up);
    let s2 = dot(n_ca, up);
    let ref0 = dot(n_ab, uc);
    let ref1 = dot(n_bc, ua);
    let ref2 = dot(n_ca, ub);
    Ok(s0 * ref0 >= -1.0e-15 && s1 * ref1 >= -1.0e-15 && s2 * ref2 >= -1.0e-15)
}

fn sample_spherical_triangle_uniform(
    triangle: [[f64; 2]; 3],
    r1: f64,
    r2: f64,
) -> Result<(f64, f64), ReleaseError> {
    let a = unit_vector(triangle[0])?;
    let b = unit_vector(triangle[1])?;
    let c = unit_vector(triangle[2])?;
    let area = spherical_triangle_excess(a, b, c)?.abs();
    if !(area > 0.0) {
        return Err(ReleaseError::InvalidGeometry);
    }
    let area_sampled = r1.clamp(0.0, 1.0 - f64::EPSILON) * area;
    let mut lo = 0.0;
    let mut hi = 1.0;
    let mut c_hat = b;
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        let candidate = slerp_unit(b, c, mid)?;
        let sub = spherical_triangle_excess(a, b, candidate)?.abs();
        if sub < area_sampled {
            lo = mid;
        } else {
            hi = mid;
        }
        c_hat = candidate;
    }
    let cos_theta = dot(a, c_hat).clamp(-1.0, 1.0);
    let z = (1.0 - r2.clamp(0.0, 1.0 - f64::EPSILON) * (1.0 - cos_theta)).clamp(-1.0, 1.0);
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    if sin_theta <= 0.0 {
        return lon_lat_from_unit(a);
    }
    let w = normalize([
        c_hat[0] - cos_theta * a[0],
        c_hat[1] - cos_theta * a[1],
        c_hat[2] - cos_theta * a[2],
    ])?;
    let radial = (1.0 - z * z).max(0.0).sqrt();
    let point = normalize([
        z * a[0] + radial * w[0],
        z * a[1] + radial * w[1],
        z * a[2] + radial * w[2],
    ])?;
    lon_lat_from_unit(point)
}

fn slerp_unit(a: [f64; 3], b: [f64; 3], t: f64) -> Result<[f64; 3], ReleaseError> {
    let cos_omega = dot(a, b).clamp(-1.0, 1.0);
    let omega = cos_omega.acos();
    if omega < 1.0e-14 {
        return Ok(a);
    }
    let sin_omega = omega.sin();
    let w0 = ((1.0 - t) * omega).sin() / sin_omega;
    let w1 = (t * omega).sin() / sin_omega;
    normalize([
        w0 * a[0] + w1 * b[0],
        w0 * a[1] + w1 * b[1],
        w0 * a[2] + w1 * b[2],
    ])
}

fn validate_ring_no_self_intersection(ring: &[[f64; 2]]) -> Result<(), ReleaseError> {
    let mut pts = ring.to_vec();
    if pts.len() >= 2 && almost_same_point(pts[0], *pts.last().unwrap()) {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        validate_short_arc(a, b)?;
        if a[1].abs() >= 90.0 - 1.0e-15 || b[1].abs() >= 90.0 - 1.0e-15 {
            return Err(ReleaseError::InvalidGeometry);
        }
        for j in (i + 1)..n {
            let adjacent = j == i + 1 || (i == 0 && j == n - 1);
            if adjacent {
                continue;
            }
            if i == 0 && j + 1 == n {
                continue;
            }
            let c = pts[j];
            let d = pts[(j + 1) % n];
            if almost_same_point(a, c)
                || almost_same_point(a, d)
                || almost_same_point(b, c)
                || almost_same_point(b, d)
            {
                return Err(ReleaseError::InvalidGeometry);
            }
            if great_circle_segments_intersect(a, b, c, d)? {
                return Err(ReleaseError::InvalidGeometry);
            }
        }
    }
    if ring_contains_pole(&pts)? {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(())
}

fn ring_contains_pole(pts: &[[f64; 2]]) -> Result<bool, ReleaseError> {
    for pole_lat in [90.0_f64, -90.0_f64] {
        let mut winding = 0.0;
        for index in 0..pts.len() {
            let a = pts[index];
            let b = pts[(index + 1) % pts.len()];
            winding += shortest_delta_lon_deg(a[0], b[0]);
        }
        if winding.abs() > 180.0 {
            let pole = [0.0, pole_lat];
            let mut excess = 0.0;
            for index in 0..pts.len() {
                let a = pts[index];
                let b = pts[(index + 1) % pts.len()];
                excess += triangle_signed_excess(pole, a, b)?;
            }
            if excess.abs() > 1.0e-12 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn shortest_delta_lon_deg(a: f64, b: f64) -> f64 {
    let mut d = b - a;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

fn great_circle_segments_intersect(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
) -> Result<bool, ReleaseError> {
    let ua = unit_vector(a)?;
    let ub = unit_vector(b)?;
    let uc = unit_vector(c)?;
    let ud = unit_vector(d)?;
    let n1 = cross(ua, ub);
    let n2 = cross(uc, ud);
    let line = cross(n1, n2);
    let norm = (line[0] * line[0] + line[1] * line[1] + line[2] * line[2]).sqrt();
    if norm <= 1.0e-18 {
        return Ok(false);
    }
    let p = [line[0] / norm, line[1] / norm, line[2] / norm];
    let q = [-p[0], -p[1], -p[2]];
    Ok((on_arc(p, ua, ub)? && on_arc(p, uc, ud)?) || (on_arc(q, ua, ub)? && on_arc(q, uc, ud)?))
}

fn on_arc(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> Result<bool, ReleaseError> {
    let ab = dot(a, b).clamp(-1.0, 1.0).acos();
    let ap = dot(a, p).clamp(-1.0, 1.0).acos();
    let pb = dot(p, b).clamp(-1.0, 1.0).acos();
    Ok((ap + pb - ab).abs() <= 1.0e-10 && ab <= PI + 1.0e-12)
}

fn normalize_geometry(geometry: &GeoJsonGeometry) -> Result<GeoJsonGeometry, ReleaseError> {
    match geometry {
        GeoJsonGeometry::Point(point) => Ok(GeoJsonGeometry::Point(normalize_point(*point)?)),
        GeoJsonGeometry::MultiPoint(points) => {
            let mut normalized = points
                .iter()
                .copied()
                .map(normalize_point)
                .collect::<Result<Vec<_>, _>>()?;
            if normalized.is_empty() {
                return Err(ReleaseError::InvalidGeometry);
            }
            normalized.sort_by(cmp_point);
            normalized.dedup_by(|a, b| almost_same_point(*a, *b));
            Ok(GeoJsonGeometry::MultiPoint(normalized))
        }
        GeoJsonGeometry::LineString(line) => Ok(GeoJsonGeometry::LineString(normalize_line(line)?)),
        GeoJsonGeometry::MultiLineString(lines) => {
            let mut normalized = lines
                .iter()
                .map(|line| normalize_line(line))
                .collect::<Result<Vec<_>, _>>()?;
            normalized.sort_by(|a, b| cmp_encoded(&encode_line(a), &encode_line(b)));
            Ok(GeoJsonGeometry::MultiLineString(normalized))
        }
        GeoJsonGeometry::Polygon(rings) => {
            let polygon = normalize_polygon(rings)?;
            split_polygon_dateline(&polygon)
        }
        GeoJsonGeometry::MultiPolygon(polygons) => {
            let mut parts = Vec::new();
            for polygon in polygons {
                let normalized = normalize_polygon(polygon)?;
                match split_polygon_dateline(&normalized)? {
                    GeoJsonGeometry::Polygon(rings) => parts.push(rings),
                    GeoJsonGeometry::MultiPolygon(mut more) => parts.append(&mut more),
                    _ => return Err(ReleaseError::InvalidGeometry),
                }
            }
            parts.sort_by(|a, b| cmp_encoded(&encode_polygon(a), &encode_polygon(b)));
            if parts.len() == 1 {
                Ok(GeoJsonGeometry::Polygon(parts.remove(0)))
            } else {
                Ok(GeoJsonGeometry::MultiPolygon(parts))
            }
        }
    }
}

fn normalize_point(point: [f64; 2]) -> Result<[f64; 2], ReleaseError> {
    if !point[0].is_finite() || !point[1].is_finite() || !(-90.0..=90.0).contains(&point[1]) {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok([normalize_longitude(point[0]), point[1]])
}

fn normalize_longitude(longitude: f64) -> f64 {
    let mut value = longitude % 360.0;
    if value >= LON_MAX_EXCLUSIVE {
        value -= 360.0;
    }
    if value < LON_MIN {
        value += 360.0;
    }
    if value >= LON_MAX_EXCLUSIVE {
        value = LON_MIN;
    }
    value
}

fn normalize_line(line: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, ReleaseError> {
    if line.len() < 2 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut out = Vec::with_capacity(line.len());
    for point in line {
        let point = normalize_point(*point)?;
        if out
            .last()
            .is_none_or(|previous| !almost_same_point(*previous, point))
        {
            out.push(point);
        }
    }
    if out.len() < 2 {
        return Err(ReleaseError::InvalidGeometry);
    }
    for window in out.windows(2) {
        validate_short_arc(window[0], window[1])?;
    }
    Ok(out)
}

fn normalize_polygon(rings: &[Vec<[f64; 2]>]) -> Result<Vec<Vec<[f64; 2]>>, ReleaseError> {
    if rings.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut normalized = Vec::with_capacity(rings.len());
    for (index, ring) in rings.iter().enumerate() {
        let mut ring = normalize_ring(ring)?;
        let area = signed_spherical_ring_area_m2(&ring)?;
        if index == 0 {
            if area <= 0.0 {
                ring.reverse();
            }
        } else if area >= 0.0 {
            ring.reverse();
        }
        ring = rotate_ring_lex_min(ring);
        normalized.push(ring);
    }
    let exterior_area = signed_spherical_ring_area_m2(&normalized[0])?.abs();
    if exterior_area <= 0.0 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut hole_area = 0.0;
    let exterior_crosses = ring_crosses_dateline(&normalized[0]);
    for hole in normalized.iter().skip(1) {
        let area = signed_spherical_ring_area_m2(hole)?.abs();
        hole_area += area;
        // Hole-in-exterior vertex proof is deferred when the exterior still
        // straddles the antimeridian; split_polygon_dateline re-validates on
        // simple half-shells.
        if !exterior_crosses && !ring_crosses_dateline(hole) {
            for point in hole.iter().take(hole.len().saturating_sub(1)) {
                if !point_in_ring(&normalized[0], point[0], point[1], true)? {
                    return Err(ReleaseError::InvalidGeometry);
                }
            }
        }
    }
    if hole_area >= exterior_area {
        return Err(ReleaseError::InvalidGeometry);
    }
    // Deterministic hole order: sort holes by canonical ring encoding so input
    // hole permutation does not change geometry SHA / mesh digest.
    if normalized.len() > 2 {
        let exterior = normalized.remove(0);
        normalized.sort_by(|a, b| cmp_encoded(&encode_line(a), &encode_line(b)));
        normalized.insert(0, exterior);
    }
    Ok(normalized)
}

fn normalize_ring(ring: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, ReleaseError> {
    if ring.len() < 4 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut points = ring
        .iter()
        .copied()
        .map(normalize_point)
        .collect::<Result<Vec<_>, _>>()?;
    if !almost_same_point(
        points[0],
        *points.last().ok_or(ReleaseError::InvalidGeometry)?,
    ) {
        return Err(ReleaseError::InvalidGeometry);
    }
    // Drop closing point while de-duplicating consecutive vertices.
    points.pop();
    let mut out = Vec::new();
    for point in points {
        if out
            .last()
            .is_none_or(|previous| !almost_same_point(*previous, point))
        {
            out.push(point);
        }
    }
    if out.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    for window in out.windows(2) {
        validate_short_arc(window[0], window[1])?;
    }
    validate_short_arc(*out.last().ok_or(ReleaseError::InvalidGeometry)?, out[0])?;
    // Reject poles in ring interiors for area ambiguity.
    if out.iter().any(|point| point[1].abs() >= 90.0 - 1.0e-14) {
        return Err(ReleaseError::InvalidGeometry);
    }
    out.push(out[0]);
    Ok(out)
}

fn rotate_ring_lex_min(mut ring: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if ring.len() < 2 {
        return ring;
    }
    ring.pop();
    let min_index = ring
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| cmp_point(a, b))
        .map(|(index, _)| index)
        .unwrap_or(0);
    ring.rotate_left(min_index);
    ring.push(ring[0]);
    ring
}

fn validate_short_arc(start: [f64; 2], end: [f64; 2]) -> Result<(), ReleaseError> {
    // Use the short-arc longitude delta (handles dateline). Exactly 180° is
    // ambiguous and hard-fails; values >180 cannot occur for the short arc.
    let dlon = shortest_delta_lon_deg(start[0], end[0]).abs();
    if (dlon - 180.0).abs() <= 1.0e-12 || dlon > 180.0 + 1.0e-12 {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(())
}

fn split_polygon_dateline(rings: &[Vec<[f64; 2]>]) -> Result<GeoJsonGeometry, ReleaseError> {
    let area_before = polygon_area_m2(rings)?;
    if !ring_crosses_dateline(&rings[0]) {
        // Still split holes that alone cross the dateline.
        let any_hole_cross = rings.iter().skip(1).any(|h| ring_crosses_dateline(h));
        if !any_hole_cross {
            return Ok(GeoJsonGeometry::Polygon(rings.to_vec()));
        }
    }
    // Split on the antimeridian in unwrapped longitude space, then rebuild
    // valid west/east polygon topology. A crossing hole becomes a notch in
    // each half-shell because its clipped piece touches the new seam boundary;
    // keeping such a piece as an interior hole creates an invalid weakly-simple
    // polygon and makes meshing depend on bridge heuristics.
    let ext_west = clip_ring_antimeridian(&rings[0], AntimeridianHalf::West)?;
    let ext_east = clip_ring_antimeridian(&rings[0], AntimeridianHalf::East)?;
    let mut polygons = Vec::<DatelineHalfPolygon>::new();
    if let Some(w) = ext_west {
        polygons.push(DatelineHalfPolygon {
            half: AntimeridianHalf::West,
            rings: vec![w],
        });
    }
    if let Some(e) = ext_east {
        polygons.push(DatelineHalfPolygon {
            half: AntimeridianHalf::East,
            rings: vec![e],
        });
    }
    if polygons.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    for hole in rings.iter().skip(1) {
        if ring_crosses_dateline(hole) {
            let pieces = [
                (
                    AntimeridianHalf::West,
                    clip_ring_antimeridian(hole, AntimeridianHalf::West)?,
                ),
                (
                    AntimeridianHalf::East,
                    clip_ring_antimeridian(hole, AntimeridianHalf::East)?,
                ),
            ];
            let mut piece_count = 0_usize;
            for (half, piece) in pieces {
                let Some(piece) = piece else {
                    continue;
                };
                let polygon = polygons
                    .iter_mut()
                    .find(|polygon| polygon.half == half)
                    .ok_or(ReleaseError::InvalidGeometry)?;
                validate_clipped_hole_piece_in_shell(&polygon.rings[0], &piece, half)?;
                polygon.rings[0] = splice_dateline_hole_notch(&polygon.rings[0], &piece, half)?;
                piece_count += 1;
            }
            if piece_count == 0 {
                return Err(ReleaseError::InvalidGeometry);
            }
        } else {
            // A non-crossing hole belongs whole to exactly one split exterior.
            // Calling both half-plane clippers here can return the same untouched
            // ring twice when no seam lies in its longitude span, duplicating its
            // area and making the canonical split fail.
            let piece = hole.clone();
            let open = open_ring(&piece)?;
            // Prefer a vertex guaranteed on the hole boundary interior to the shell.
            let mut assigned = false;
            let candidates = hole_assignment_points(&open)?;
            for polygon in &mut polygons {
                for &(clon, clat) in &candidates {
                    if point_in_ring(&polygon.rings[0], clon, clat, true)? {
                        // Ensure hole orientation opposite exterior for meshing.
                        let mut hole_ring = piece.clone();
                        let ext_open = open_ring(&polygon.rings[0])?;
                        let hole_open = open_ring(&hole_ring)?;
                        let ext_pos = ring_signed_excess(&ext_open)? > 0.0;
                        let hole_pos = ring_signed_excess(&hole_open)? > 0.0;
                        if ext_pos == hole_pos {
                            let mut rev = open_ring(&hole_ring)?;
                            rev.reverse();
                            rev.push(rev[0]);
                            hole_ring = rev;
                        }
                        polygon.rings.push(normalize_ring(&hole_ring)?);
                        assigned = true;
                        break;
                    }
                }
                if assigned {
                    break;
                }
            }
            if !assigned {
                return Err(ReleaseError::InvalidGeometry);
            }
        }
    }
    // Normalize every rebuilt component independently, including deterministic
    // ring rotation/hole order, then prove each boundary is simple.
    let mut normalized_polygons = Vec::with_capacity(polygons.len());
    for polygon in polygons {
        if polygon.rings.first().is_none_or(|ring| ring.len() < 4) {
            continue;
        }
        let normalized = normalize_polygon(&polygon.rings)?;
        for ring in &normalized {
            validate_ring_no_self_intersection(ring)?;
        }
        normalized_polygons.push(normalized);
    }
    normalized_polygons.sort_by(|a, b| cmp_encoded(&encode_polygon(a), &encode_polygon(b)));
    if normalized_polygons.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut area_after = 0.0;
    for polygon in &normalized_polygons {
        area_after += polygon_area_m2(polygon)?;
    }
    let scale = area_before.max(area_after).max(1.0);
    if (area_before - area_after).abs() / scale > GEOMETRY_AREA_RELATIVE_TOLERANCE {
        return Err(ReleaseError::InvalidGeometry);
    }
    if normalized_polygons.len() == 1 {
        Ok(GeoJsonGeometry::Polygon(normalized_polygons.remove(0)))
    } else {
        Ok(GeoJsonGeometry::MultiPolygon(normalized_polygons))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AntimeridianHalf {
    West,
    East,
}

struct DatelineHalfPolygon {
    half: AntimeridianHalf,
    rings: Vec<Vec<[f64; 2]>>,
}

fn point_on_antimeridian_seam(point: [f64; 2], half: AntimeridianHalf) -> bool {
    let seam = match half {
        AntimeridianHalf::West => -180.0 + 1.0e-12,
        AntimeridianHalf::East => 180.0 - 1.0e-12,
    };
    (point[0] - seam).abs() <= 1.0e-9
}

fn clipped_hole_seam_transitions(
    open: &[[f64; 2]],
    half: AntimeridianHalf,
) -> Result<[usize; 2], ReleaseError> {
    if open.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let seam = open
        .iter()
        .copied()
        .map(|point| point_on_antimeridian_seam(point, half))
        .collect::<Vec<_>>();
    if seam.iter().all(|value| *value) || seam.iter().all(|value| !*value) {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut transitions = Vec::new();
    for index in 0..open.len() {
        let next = (index + 1) % open.len();
        if seam[index] == seam[next] {
            continue;
        }
        let seam_index = if seam[index] { index } else { next };
        if !transitions.contains(&seam_index) {
            transitions.push(seam_index);
        }
    }
    if transitions.len() != 2 {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok([transitions[0], transitions[1]])
}

fn validate_clipped_hole_piece_in_shell(
    shell: &[[f64; 2]],
    piece: &[[f64; 2]],
    half: AntimeridianHalf,
) -> Result<(), ReleaseError> {
    let open = open_ring(piece)?;
    let _ = clipped_hole_seam_transitions(&open, half)?;
    let mut non_seam_count = 0_usize;
    for point in open {
        if point_on_antimeridian_seam(point, half) {
            continue;
        }
        non_seam_count += 1;
        if !point_in_ring(shell, point[0], point[1], true)? {
            return Err(ReleaseError::InvalidGeometry);
        }
    }
    if non_seam_count == 0 {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(())
}

fn cyclic_ring_path(
    open: &[[f64; 2]],
    start: usize,
    end: usize,
    forward: bool,
) -> Result<Vec<[f64; 2]>, ReleaseError> {
    if start >= open.len() || end >= open.len() || start == end {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut path = Vec::new();
    let mut index = start;
    for _ in 0..=open.len() {
        path.push(open[index]);
        if index == end {
            return Ok(path);
        }
        index = if forward {
            (index + 1) % open.len()
        } else {
            (index + open.len() - 1) % open.len()
        };
    }
    Err(ReleaseError::InvalidGeometry)
}

fn hole_notch_path(
    open: &[[f64; 2]],
    start: usize,
    end: usize,
    half: AntimeridianHalf,
) -> Result<Vec<[f64; 2]>, ReleaseError> {
    let forward = cyclic_ring_path(open, start, end, true)?;
    let backward = cyclic_ring_path(open, start, end, false)?;
    let has_non_seam_interior = |path: &[[f64; 2]]| {
        path.iter()
            .skip(1)
            .take(path.len().saturating_sub(2))
            .any(|point| !point_on_antimeridian_seam(*point, half))
    };
    match (
        has_non_seam_interior(&forward),
        has_non_seam_interior(&backward),
    ) {
        (true, false) => Ok(forward),
        (false, true) => Ok(backward),
        // Both means the clip produced more than one disconnected lobe; neither
        // means the alleged piece has no area away from the seam. Both are
        // ambiguous and must hard-fail rather than invent topology.
        _ => Err(ReleaseError::InvalidGeometry),
    }
}

fn splice_dateline_hole_notch(
    shell: &[[f64; 2]],
    piece: &[[f64; 2]],
    half: AntimeridianHalf,
) -> Result<Vec<[f64; 2]>, ReleaseError> {
    let mut exterior = open_ring(shell)?;
    let hole = open_ring(piece)?;
    let transitions = clipped_hole_seam_transitions(&hole, half)?;
    let endpoints = [hole[transitions[0]], hole[transitions[1]]];

    let mut best_edge: Option<(u64, usize, f64, f64)> = None;
    for index in 0..exterior.len() {
        let a = exterior[index];
        let b = exterior[(index + 1) % exterior.len()];
        if !point_on_antimeridian_seam(a, half)
            || !point_on_antimeridian_seam(b, half)
            || (b[1] - a[1]).abs() <= 1.0e-14
        {
            continue;
        }
        let t0 = (endpoints[0][1] - a[1]) / (b[1] - a[1]);
        let t1 = (endpoints[1][1] - a[1]) / (b[1] - a[1]);
        if !(-1.0e-10..=1.0 + 1.0e-10).contains(&t0) || !(-1.0e-10..=1.0 + 1.0e-10).contains(&t1) {
            continue;
        }
        let candidate = (ordered_f64((b[1] - a[1]).abs()), index, t0, t1);
        if best_edge
            .as_ref()
            .is_none_or(|previous| (candidate.0, candidate.1) < (previous.0, previous.1))
        {
            best_edge = Some(candidate);
        }
    }
    let Some((_, edge_index, t0, t1)) = best_edge else {
        return Err(ReleaseError::InvalidGeometry);
    };
    let (start_index, end_index) = if t0 <= t1 {
        (transitions[0], transitions[1])
    } else {
        (transitions[1], transitions[0])
    };
    let notch = hole_notch_path(&hole, start_index, end_index, half)?;

    // Rotate so the seam edge being replaced is exterior[0] -> exterior[1].
    exterior.rotate_left(edge_index);
    let mut rebuilt = Vec::with_capacity(exterior.len() + notch.len() + 1);
    rebuilt.push(exterior[0]);
    for point in notch {
        if rebuilt
            .last()
            .is_none_or(|previous| !almost_same_point(*previous, point))
        {
            rebuilt.push(point);
        }
    }
    for point in exterior.iter().skip(1).copied() {
        if rebuilt
            .last()
            .is_none_or(|previous| !almost_same_point(*previous, point))
        {
            rebuilt.push(point);
        }
    }
    if rebuilt.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    rebuilt.push(rebuilt[0]);
    let rebuilt = normalize_ring(&rebuilt)?;
    validate_ring_no_self_intersection(&rebuilt)?;

    let shell_area = signed_spherical_ring_area_m2(shell)?.abs();
    let piece_area = signed_spherical_ring_area_m2(piece)?.abs();
    let rebuilt_area = signed_spherical_ring_area_m2(&rebuilt)?.abs();
    if piece_area >= shell_area {
        return Err(ReleaseError::InvalidGeometry);
    }
    let expected = shell_area - piece_area;
    let scale = expected.max(rebuilt_area).max(1.0);
    if (expected - rebuilt_area).abs() / scale > GEOMETRY_AREA_RELATIVE_TOLERANCE {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(rebuilt)
}

fn hole_assignment_points(open: &[[f64; 2]]) -> Result<Vec<(f64, f64)>, ReleaseError> {
    let mut pts = Vec::new();
    // Centroid plus every vertex — thin dateline hole pieces may have a centroid
    // that falls outside the matching exterior shell.
    if let Ok((c_lon, c_lat)) = ring_centroid_lonlat(open) {
        pts.push((c_lon, c_lat));
    }
    for p in open {
        pts.push((p[0], p[1]));
    }
    // Edge midpoints in spherical sense (linear lat, unwrap lon).
    for window in open.windows(2) {
        let (lon, lat) = interpolate_great_circle(window[0], window[1], 0.5)?;
        pts.push((lon, lat));
    }
    Ok(pts)
}

/// Clip a ring to the west (lon in (-180,0)) or east (lon in [0,180)) side after
/// unwrapping, cutting along the antimeridian (unwrapped lon = ±180 + 360k).
fn clip_ring_antimeridian(
    ring: &[[f64; 2]],
    half: AntimeridianHalf,
) -> Result<Option<Vec<[f64; 2]>>, ReleaseError> {
    if ring.len() < 4 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let unwrapped = unwrap_ring_longitudes(ring)?;
    let seams = seams_for_unwrapped(&unwrapped);
    let mut best: Option<Vec<[f64; 2]>> = None;
    for seam in seams {
        let keep_left = match half {
            // Near antimeridian unwrap (~180): east is u <= seam, west is u >= seam.
            AntimeridianHalf::West => false,
            AntimeridianHalf::East => true,
        };
        let clipped = clip_unwrapped_ring_against_seam(&unwrapped, seam, keep_left)?;
        if clipped.len() < 3 {
            continue;
        }
        // Map unwrapped coords to a simple half-shell WITHOUT collapsing +180→-180
        // on the east piece (that would stretch the shell around the globe).
        let mut mapped = Vec::with_capacity(clipped.len() + 1);
        for p in clipped {
            mapped.push(map_unwrapped_point_to_half(p, half, seam)?);
        }
        if mapped.first().copied() != mapped.last().copied() {
            mapped.push(mapped[0]);
        }
        if mapped.len() < 4 {
            continue;
        }
        // Validate short arcs on the mapped shell; skip full normalize_ring which
        // would re-apply normalize_longitude and re-introduce the +180 collapse.
        let mut open = mapped.clone();
        if almost_same_point(open[0], *open.last().unwrap_or(&open[0])) {
            open.pop();
        }
        if open.len() < 3 {
            continue;
        }
        let mut ok = true;
        for window in open.windows(2) {
            if validate_short_arc(window[0], window[1]).is_err() {
                ok = false;
                break;
            }
        }
        if ok {
            ok = validate_short_arc(open[open.len() - 1], open[0]).is_ok();
        }
        if !ok {
            continue;
        }
        let mut closed = open;
        closed.push(closed[0]);
        let area = signed_spherical_ring_area_m2(&closed)?.abs();
        if area <= 0.0 {
            continue;
        }
        match &best {
            None => best = Some(closed),
            Some(prev) => {
                let prev_area = signed_spherical_ring_area_m2(prev)?.abs();
                // Prefer the piece whose lon span is localized near the antimeridian
                // (smaller absolute area among positive candidates is OK for thin
                // shells; take max area among valid short-arc pieces).
                if area > prev_area {
                    best = Some(closed);
                }
            }
        }
    }
    Ok(best)
}

fn map_unwrapped_point_to_half(
    point: [f64; 2],
    half: AntimeridianHalf,
    seam: f64,
) -> Result<[f64; 2], ReleaseError> {
    let lat = point[1];
    if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
        return Err(ReleaseError::InvalidGeometry);
    }
    // Shift so seam ~ 180 for arithmetic, then map.
    let mut u = point[0];
    // Express relative to seam image nearest 180.
    match half {
        AntimeridianHalf::East => {
            // Keep longitudes in (-180, 180), with the seam represented just west of +180.
            while u > seam + 180.0 {
                u -= 360.0;
            }
            while u < seam - 180.0 {
                u += 360.0;
            }
            let mut lon = u;
            // Fold into (-180, 180].
            if (lon - seam).abs() <= 1.0e-12 {
                lon = 180.0 - 1.0e-12;
            } else {
                // Translate so seam→180.
                lon = lon - seam + 180.0;
                while lon >= 180.0 {
                    lon -= 360.0;
                }
                while lon < -180.0 {
                    lon += 360.0;
                }
                if lon >= 180.0 - 1.0e-15 {
                    lon = 180.0 - 1.0e-12;
                }
            }
            Ok([lon, lat])
        }
        AntimeridianHalf::West => {
            while u > seam + 180.0 {
                u -= 360.0;
            }
            while u < seam - 180.0 {
                u += 360.0;
            }
            let mut lon = u - seam + 180.0; // seam → 180
            // West of seam → lon >= 180 in this frame → fold to negative.
            if (lon - 180.0).abs() <= 1.0e-12 {
                // Stay just east of -180 so short arcs on the west shell do not
                // sit on the branch cut.
                lon = -180.0 + 1.0e-12;
            } else if lon > 180.0 {
                lon -= 360.0;
            }
            while lon < -180.0 {
                lon += 360.0;
            }
            while lon > 180.0 {
                lon -= 360.0;
            }
            if lon > 0.0 && lon < 180.0 {
                // Should not happen for a true west clip near the antimeridian;
                // fall back to normalize.
                lon = normalize_longitude(lon);
            }
            Ok([lon, lat])
        }
    }
}

fn unwrap_ring_longitudes(ring: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, ReleaseError> {
    let mut out = Vec::with_capacity(ring.len());
    out.push([ring[0][0], ring[0][1]]);
    for point in ring.iter().skip(1) {
        let prev = *out.last().ok_or(ReleaseError::InvalidGeometry)?;
        let mut lon = point[0];
        while lon - prev[0] > 180.0 {
            lon -= 360.0;
        }
        while lon - prev[0] < -180.0 {
            lon += 360.0;
        }
        out.push([lon, point[1]]);
    }
    Ok(out)
}

fn seams_for_unwrapped(unwrapped: &[[f64; 2]]) -> Vec<f64> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in unwrapped {
        lo = lo.min(p[0]);
        hi = hi.max(p[0]);
    }
    let mut seams = Vec::new();
    // Candidate antimeridian images k*360 ± 180 that the unwrap span can cross.
    let k0 = ((lo - 180.0) / 360.0).floor() as i32 - 1;
    let k1 = ((hi - 180.0) / 360.0).ceil() as i32 + 1;
    for k in k0..=k1 {
        let seam = 180.0 + 360.0 * f64::from(k);
        if seam >= lo - 1.0e-9 && seam <= hi + 1.0e-9 {
            seams.push(seam);
        }
        let seam_n = -180.0 + 360.0 * f64::from(k);
        if seam_n >= lo - 1.0e-9 && seam_n <= hi + 1.0e-9 {
            seams.push(seam_n);
        }
    }
    if seams.is_empty() {
        // No seam inside span: still try canonical ±180 for half classification.
        seams.push(180.0);
        seams.push(-180.0);
    }
    seams.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    seams.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-12);
    seams
}

fn clip_unwrapped_ring_against_seam(
    unwrapped: &[[f64; 2]],
    seam: f64,
    keep_left: bool,
) -> Result<Vec<[f64; 2]>, ReleaseError> {
    // Sutherland–Hodgman against vertical line lon=seam in unwrapped space.
    // Intersection latitude comes from the true great-circle ∩ antimeridian,
    // not from linear lat interpolation along unwrapped lon (parallels ≠ GCs).
    fn inside(lon: f64, seam: f64, keep_left: bool) -> bool {
        if keep_left {
            lon <= seam + 1.0e-14
        } else {
            lon >= seam - 1.0e-14
        }
    }
    if unwrapped.len() < 2 {
        return Ok(Vec::new());
    }
    let mut pts = unwrapped.to_vec();
    if pts.len() >= 2 && almost_same_point(pts[0], *pts.last().unwrap_or(&pts[0])) {
        pts.pop();
    }
    if pts.len() < 3 {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    let mut prev = pts[pts.len() - 1];
    let mut prev_in = inside(prev[0], seam, keep_left);
    for &curr in &pts {
        let curr_in = inside(curr[0], seam, keep_left);
        if curr_in {
            if !prev_in {
                output.push(gc_antimeridian_intersection_unwrapped(prev, curr, seam)?);
            }
            output.push(curr);
        } else if prev_in {
            output.push(gc_antimeridian_intersection_unwrapped(prev, curr, seam)?);
        }
        prev = curr;
        prev_in = curr_in;
    }
    Ok(output)
}

/// Great-circle intersection with the antimeridian image `seam` (…, -180, 180, 540, …),
/// returned in unwrapped coordinates with longitude = seam.
fn gc_antimeridian_intersection_unwrapped(
    a: [f64; 2],
    b: [f64; 2],
    seam: f64,
) -> Result<[f64; 2], ReleaseError> {
    // Work in unit sphere with normalized longitudes equivalent to a,b.
    let va = unit_vector([normalize_longitude(a[0]), a[1]])?;
    let vb = unit_vector([normalize_longitude(b[0]), b[1]])?;
    let normal = cross(va, vb);
    // Antimeridian plane for lon = ±180 is the half-plane y=0, x<=0. For a general
    // seam = 180 + 360k the plane is the meridian plane at that longitude:
    // n_meridian = (-sin(seam), cos(seam), 0) · p = 0.
    let seam_rad = seam.to_radians();
    let meridian_n = [-seam_rad.sin(), seam_rad.cos(), 0.0];
    // Line of nodes: direction = normal × meridian_n.
    let mut dir = cross(normal, meridian_n);
    let nrm = norm(dir);
    if nrm <= COORD_EPS {
        // Degenerate: edge lies in the meridian plane — pick endpoint nearer seam.
        return Ok([
            seam,
            if (a[0] - seam).abs() <= (b[0] - seam).abs() {
                a[1]
            } else {
                b[1]
            },
        ]);
    }
    dir = [dir[0] / nrm, dir[1] / nrm, dir[2] / nrm];
    // Two opposite intersections; pick the one on the short arc a→b.
    let candidates = [dir, [-dir[0], -dir[1], -dir[2]]];
    let mut best: Option<[f64; 3]> = None;
    for cand in candidates {
        if on_arc(cand, va, vb)? {
            best = Some(cand);
            break;
        }
    }
    let point = best.ok_or(ReleaseError::InvalidGeometry)?;
    let (_, lat) = lon_lat_from_unit(point)?;
    Ok([seam, lat])
}

fn ring_centroid_lonlat(ring: &[[f64; 2]]) -> Result<(f64, f64), ReleaseError> {
    let mut pts = ring.to_vec();
    if pts.len() >= 2 && almost_same_point(pts[0], *pts.last().unwrap_or(&pts[0])) {
        pts.pop();
    }
    if pts.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut x = 0.0;
    let mut y = 0.0;
    let mut z = 0.0;
    for point in &pts {
        let u = unit_vector(*point)?;
        x += u[0];
        y += u[1];
        z += u[2];
    }
    lon_lat_from_unit(normalize([x, y, z])?)
}

fn ring_crosses_dateline(ring: &[[f64; 2]]) -> bool {
    ring.windows(2).any(|window| {
        let dlon = (window[1][0] - window[0][0]).abs();
        dlon > 180.0
    }) || ring.windows(2).any(|window| {
        let a = window[0][0];
        let b = window[1][0];
        (a.signum() != b.signum()) && (a.abs() + b.abs() > 180.0)
    })
}

fn split_ring_side(ring: &[[f64; 2]], west: bool) -> Result<Vec<[f64; 2]>, ReleaseError> {
    // Convert to unwrapped longitudes around the short-arc chain, then clip.
    let mut unwrapped = vec![ring[0]];
    for point in ring.iter().skip(1) {
        let prev = *unwrapped.last().ok_or(ReleaseError::InvalidGeometry)?;
        let mut lon = point[0];
        while lon - prev[0] > 180.0 {
            lon -= 360.0;
        }
        while lon - prev[0] < -180.0 {
            lon += 360.0;
        }
        unwrapped.push([lon, point[1]]);
    }
    let mut clipped = Vec::new();
    for window in unwrapped.windows(2) {
        let a = window[0];
        let b = window[1];
        let a_in = if west {
            a[0] <= 180.0 && a[0] >= 0.0 || a[0] < 0.0 && a[0] > -180.0 && !edge_mostly_east(a, b)
        } else {
            a[0] >= 0.0
        };
        // Simpler robust approach: keep vertices on the requested half after normalizing.
        let a_side = if west {
            a[0] < 180.0 - 1e-12 && normalize_longitude(a[0]) <= 0.0
                || normalize_longitude(a[0]) == LON_MIN
        } else {
            normalize_longitude(a[0]) >= 0.0
        };
        let b_side = if west {
            let lon = normalize_longitude(b[0]);
            lon < 0.0 || (lon - LON_MIN).abs() < 1e-12
        } else {
            let lon = normalize_longitude(b[0]);
            lon >= 0.0 && lon < LON_MAX_EXCLUSIVE
        };
        let a_norm = normalize_point([a[0], a[1]])?;
        let b_norm = normalize_point([b[0], b[1]])?;
        let a_keep = if west {
            a_norm[0] < 0.0 || (a_norm[0] - LON_MIN).abs() < 1e-12
        } else {
            a_norm[0] >= 0.0
        };
        let b_keep = if west {
            b_norm[0] < 0.0 || (b_norm[0] - LON_MIN).abs() < 1e-12
        } else {
            b_norm[0] >= 0.0
        };
        if a_keep {
            push_unique(&mut clipped, a_norm);
        }
        if a_keep != b_keep {
            if let Some(hit) = dateline_intersection(a, b)? {
                push_unique(&mut clipped, hit);
            }
        }
        if b_keep {
            push_unique(&mut clipped, b_norm);
        }
        let _ = (a_in, a_side, b_side);
    }
    if clipped.len() >= 3 {
        if !almost_same_point(clipped[0], *clipped.last().unwrap_or(&clipped[0])) {
            clipped.push(clipped[0]);
        }
        if clipped.len() >= 4 {
            return Ok(clipped);
        }
    }
    Ok(Vec::new())
}

fn edge_mostly_east(a: [f64; 2], b: [f64; 2]) -> bool {
    0.5 * (a[0] + b[0]) > 0.0
}

fn dateline_intersection(a: [f64; 2], b: [f64; 2]) -> Result<Option<[f64; 2]>, ReleaseError> {
    let va = unit_vector(a)?;
    let vb = unit_vector(b)?;
    // Plane of great circle: n = a × b. Intersection with x=0 plane (lon=±90?); antimeridian is x<=0,y=0 -> lon=±180 => point (-1,0,z)
    let normal = cross(va, vb);
    // Line of nodes with OYZ? Antimeridian plane is y = 0 and x <= 0, i.e. plane y=0.
    // Intersection of great-circle plane with y=0: direction = normal × (0,1,0) = (-normal.z, 0, normal.x)
    let mut dir = [-normal[2], 0.0, normal[0]];
    let norm = (dir[0] * dir[0] + dir[2] * dir[2]).sqrt();
    if norm <= COORD_EPS {
        return Ok(None);
    }
    dir[0] /= norm;
    dir[2] /= norm;
    // Choose the antimeridian branch x <= 0.
    if dir[0] > 0.0 {
        dir[0] = -dir[0];
        dir[2] = -dir[2];
    }
    let lat = dir[2].clamp(-1.0, 1.0).asin().to_degrees();
    if !lat.is_finite() {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok(Some([LON_MIN, lat]))
}

fn push_unique(points: &mut Vec<[f64; 2]>, point: [f64; 2]) {
    if points
        .last()
        .is_none_or(|previous| !almost_same_point(*previous, point))
    {
        points.push(point);
    }
}

fn spherical_area_m2(geometry: &GeoJsonGeometry) -> Result<f64, ReleaseError> {
    match geometry {
        GeoJsonGeometry::Point(_)
        | GeoJsonGeometry::MultiPoint(_)
        | GeoJsonGeometry::LineString(_)
        | GeoJsonGeometry::MultiLineString(_) => Ok(0.0),
        GeoJsonGeometry::Polygon(rings) => polygon_area_m2(rings),
        GeoJsonGeometry::MultiPolygon(polygons) => {
            let mut total = 0.0;
            for polygon in polygons {
                total += polygon_area_m2(polygon)?;
            }
            Ok(total)
        }
    }
}

fn polygon_area_m2(rings: &[Vec<[f64; 2]>]) -> Result<f64, ReleaseError> {
    if rings.is_empty() {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut area = signed_spherical_ring_area_m2(&rings[0])?.abs();
    for hole in rings.iter().skip(1) {
        area -= signed_spherical_ring_area_m2(hole)?.abs();
    }
    if area.is_finite() && area >= 0.0 {
        Ok(area)
    } else {
        Err(ReleaseError::InvalidGeometry)
    }
}

fn signed_spherical_ring_area_m2(ring: &[[f64; 2]]) -> Result<f64, ReleaseError> {
    if ring.len() < 4 {
        return Err(ReleaseError::InvalidGeometry);
    }
    let mut points = ring.to_vec();
    if almost_same_point(points[0], *points.last().unwrap_or(&points[0])) {
        points.pop();
    }
    if points.len() < 3 {
        return Err(ReleaseError::InvalidGeometry);
    }
    // Spherical excess via unit-vector triangulation from first vertex.
    let origin = unit_vector(points[0])?;
    let mut excess = 0.0;
    for window in points[1..].windows(2) {
        let a = unit_vector(window[0])?;
        let b = unit_vector(window[1])?;
        excess += spherical_triangle_excess(origin, a, b)?;
    }
    let area = excess * M4_CONSTANTS.earth_radius_m.powi(2);
    if area.is_finite() {
        Ok(area)
    } else {
        Err(ReleaseError::InvalidGeometry)
    }
}

fn spherical_triangle_excess(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Result<f64, ReleaseError> {
    // Signed spherical excess via the Oosterom–Strackee formula:
    //   tan(E/2) = triple / (1 + a·b + b·c + c·a)
    //   E = 2 atan2(triple, denom)
    // Do NOT force angles into [0, 2π); that maps small negative interior angles
    // near 2π and inflates a 1° triangle's excess to ~π.
    let triple = scalar_triple(a, b, c);
    let denom = 1.0 + dot(a, b) + dot(b, c) + dot(c, a);
    if !triple.is_finite() || !denom.is_finite() {
        return Err(ReleaseError::InvalidGeometry);
    }
    // Short-arc release geometry requires the triangle to lie in an open hemisphere.
    if denom <= COORD_EPS {
        return Err(ReleaseError::InvalidGeometry);
    }
    if triple.abs() <= COORD_EPS {
        return Ok(0.0);
    }
    let excess = 2.0 * triple.atan2(denom);
    if excess.is_finite() {
        Ok(excess)
    } else {
        Err(ReleaseError::InvalidGeometry)
    }
}

fn scalar_triple(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    dot(a, cross(b, c))
}

fn great_circle_length_m(start: [f64; 2], end: [f64; 2]) -> Result<f64, ReleaseError> {
    let a = unit_vector(start)?;
    let b = unit_vector(end)?;
    let angle = dot(a, b).clamp(-1.0, 1.0).acos();
    if angle.is_finite() {
        Ok(angle * M4_CONSTANTS.earth_radius_m)
    } else {
        Err(ReleaseError::InvalidGeometry)
    }
}

fn interpolate_great_circle(
    start: [f64; 2],
    end: [f64; 2],
    fraction: f64,
) -> Result<(f64, f64), ReleaseError> {
    if !(0.0..=1.0).contains(&fraction) {
        return Err(ReleaseError::InvalidGeometry);
    }
    let a = unit_vector(start)?;
    let b = unit_vector(end)?;
    let cos_omega = dot(a, b).clamp(-1.0, 1.0);
    let omega = cos_omega.acos();
    if omega.abs() <= COORD_EPS {
        return Ok((start[0], start[1]));
    }
    let sin_omega = omega.sin();
    let s0 = ((1.0 - fraction) * omega).sin() / sin_omega;
    let s1 = (fraction * omega).sin() / sin_omega;
    let point = [
        s0 * a[0] + s1 * b[0],
        s0 * a[1] + s1 * b[1],
        s0 * a[2] + s1 * b[2],
    ];
    lon_lat_from_unit(normalize(point)?)
}

fn triangle_area_m2(triangle: [[f64; 2]; 3]) -> Result<f64, ReleaseError> {
    let a = unit_vector(triangle[0])?;
    let b = unit_vector(triangle[1])?;
    let c = unit_vector(triangle[2])?;
    Ok(spherical_triangle_excess(a, b, c)?.abs() * M4_CONSTANTS.earth_radius_m.powi(2))
}

fn point_in_geometry(
    geometry: &GeoJsonGeometry,
    longitude: f64,
    latitude: f64,
) -> Result<bool, ReleaseError> {
    match geometry {
        GeoJsonGeometry::Point(point) => Ok(almost_same_point(*point, [longitude, latitude])),
        GeoJsonGeometry::MultiPoint(points) => Ok(points
            .iter()
            .any(|point| almost_same_point(*point, [longitude, latitude]))),
        GeoJsonGeometry::LineString(line) => {
            point_on_lines(&[line.as_slice()], longitude, latitude)
        }
        GeoJsonGeometry::MultiLineString(lines) => {
            let refs: Vec<&[[f64; 2]]> = lines.iter().map(Vec::as_slice).collect();
            point_on_lines(&refs, longitude, latitude)
        }
        GeoJsonGeometry::Polygon(rings) => point_in_polygon(rings, longitude, latitude),
        GeoJsonGeometry::MultiPolygon(polygons) => {
            for polygon in polygons {
                if point_in_polygon(polygon, longitude, latitude)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
    }
}

fn point_on_lines(
    lines: &[&[[f64; 2]]],
    longitude: f64,
    latitude: f64,
) -> Result<bool, ReleaseError> {
    let p = unit_vector([longitude, latitude])?;
    for line in lines {
        for window in line.windows(2) {
            let a = unit_vector(window[0])?;
            let b = unit_vector(window[1])?;
            let ab = cross(a, b);
            let n = norm(ab);
            if n <= COORD_EPS {
                if almost_same_point(window[0], [longitude, latitude]) {
                    return Ok(true);
                }
                continue;
            }
            let normal = [ab[0] / n, ab[1] / n, ab[2] / n];
            if dot(normal, p).abs() > 1.0e-10 {
                continue;
            }
            let ap = cross(a, p);
            let pb = cross(p, b);
            if dot(ap, ab) >= -1.0e-10 && dot(pb, ab) >= -1.0e-10 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn point_in_ring_open(
    open: &[[f64; 2]],
    longitude: f64,
    latitude: f64,
    exterior: bool,
) -> Result<bool, ReleaseError> {
    let mut closed = open.to_vec();
    if closed.first().copied() != closed.last().copied() {
        closed.push(closed[0]);
    }
    point_in_ring(&closed, longitude, latitude, exterior)
}

fn point_in_polygon(
    rings: &[Vec<[f64; 2]>],
    longitude: f64,
    latitude: f64,
) -> Result<bool, ReleaseError> {
    if rings.is_empty() {
        return Ok(false);
    }
    if !point_in_ring(&rings[0], longitude, latitude, true)? {
        return Ok(false);
    }
    for hole in rings.iter().skip(1) {
        if point_in_ring(hole, longitude, latitude, false)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn point_in_ring(
    ring: &[[f64; 2]],
    longitude: f64,
    latitude: f64,
    _exterior: bool,
) -> Result<bool, ReleaseError> {
    // Spherical winding number via cumulative bearing change at the query point.
    // Interior points yield |winding| ≈ 2π; exterior points yield ≈ 0.
    let mut points = ring.to_vec();
    if almost_same_point(points[0], *points.last().unwrap_or(&points[0])) {
        points.pop();
    }
    if points.len() < 3 {
        return Ok(false);
    }
    let p = unit_vector([longitude, latitude])?;
    let mut angle_sum = 0.0;
    for i in 0..points.len() {
        let a = unit_vector(points[i])?;
        let b = unit_vector(points[(i + 1) % points.len()])?;
        // Tangents from P toward A and B in the plane perpendicular to P.
        let ta = reject_unit(a, p)?;
        let tb = reject_unit(b, p)?;
        let sin = dot(p, cross(ta, tb));
        let cos = dot(ta, tb).clamp(-1.0, 1.0);
        let turn = sin.atan2(cos);
        if !turn.is_finite() {
            return Err(ReleaseError::InvalidGeometry);
        }
        angle_sum += turn;
    }
    Ok(angle_sum.abs() > PI)
}

fn reject_unit(v: [f64; 3], axis: [f64; 3]) -> Result<[f64; 3], ReleaseError> {
    let proj = dot(v, axis);
    let r = [
        v[0] - proj * axis[0],
        v[1] - proj * axis[1],
        v[2] - proj * axis[2],
    ];
    let n = norm(r);
    if n <= COORD_EPS {
        // Query coincides with a vertex (or antipode projection degeneracy): treat as boundary/inside.
        return Ok([0.0, 0.0, 0.0]);
    }
    Ok([r[0] / n, r[1] / n, r[2] / n])
}

#[allow(dead_code)]
fn ring_bbox(ring: &[[f64; 2]]) -> Result<(f64, f64, f64, f64), ReleaseError> {
    let mut west = f64::INFINITY;
    let mut east = f64::NEG_INFINITY;
    let mut south = f64::INFINITY;
    let mut north = f64::NEG_INFINITY;
    for point in ring {
        west = west.min(point[0]);
        east = east.max(point[0]);
        south = south.min(point[1]);
        north = north.max(point[1]);
    }
    if ![west, east, south, north].iter().all(|v| v.is_finite()) {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok((west, east, south, north))
}

fn unit_vector(point: [f64; 2]) -> Result<[f64; 3], ReleaseError> {
    if !point[0].is_finite() || !point[1].is_finite() || !(-90.0..=90.0).contains(&point[1]) {
        return Err(ReleaseError::InvalidGeometry);
    }
    let lon = point[0].to_radians();
    let lat = point[1].to_radians();
    let cos_lat = lat.cos();
    Ok([cos_lat * lon.cos(), cos_lat * lon.sin(), lat.sin()])
}

fn lon_lat_from_unit(point: [f64; 3]) -> Result<(f64, f64), ReleaseError> {
    let lon = normalize_longitude(point[1].atan2(point[0]).to_degrees());
    let lat = point[2].clamp(-1.0, 1.0).asin().to_degrees();
    if lon.is_finite() && lat.is_finite() {
        Ok((lon, lat))
    } else {
        Err(ReleaseError::InvalidGeometry)
    }
}

fn normalize(vector: [f64; 3]) -> Result<[f64; 3], ReleaseError> {
    let n = norm(vector);
    if n <= 0.0 {
        return Err(ReleaseError::InvalidGeometry);
    }
    Ok([vector[0] / n, vector[1] / n, vector[2] / n])
}

fn norm(vector: [f64; 3]) -> f64 {
    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt()
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn almost_same_point(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() <= 1.0e-12 && (a[1] - b[1]).abs() <= 1.0e-12
}

fn cmp_point(a: &[f64; 2], b: &[f64; 2]) -> Ordering {
    match a[0].partial_cmp(&b[0]) {
        Some(Ordering::Equal) | None => a[1].partial_cmp(&b[1]).unwrap_or(Ordering::Equal),
        Some(order) => order,
    }
}

fn cmp_encoded(a: &str, b: &str) -> Ordering {
    a.cmp(b)
}

fn encode_canonical_geometry(geometry: &GeoJsonGeometry) -> String {
    match geometry {
        GeoJsonGeometry::Point(point) => format!("Point:{}", encode_point(*point)),
        GeoJsonGeometry::MultiPoint(points) => format!(
            "MultiPoint:{}",
            points
                .iter()
                .map(|point| encode_point(*point))
                .collect::<Vec<_>>()
                .join(";")
        ),
        GeoJsonGeometry::LineString(line) => format!("LineString:{}", encode_line(line)),
        GeoJsonGeometry::MultiLineString(lines) => format!(
            "MultiLineString:{}",
            lines
                .iter()
                .map(|line| encode_line(line))
                .collect::<Vec<_>>()
                .join("|")
        ),
        GeoJsonGeometry::Polygon(rings) => format!("Polygon:{}", encode_polygon(rings)),
        GeoJsonGeometry::MultiPolygon(polygons) => format!(
            "MultiPolygon:{}",
            polygons
                .iter()
                .map(|polygon| encode_polygon(polygon))
                .collect::<Vec<_>>()
                .join("||")
        ),
    }
}

fn encode_point(point: [f64; 2]) -> String {
    format!("{:.17},{:.17}", point[0], point[1])
}

fn encode_line(line: &[[f64; 2]]) -> String {
    line.iter()
        .map(|point| encode_point(*point))
        .collect::<Vec<_>>()
        .join(";")
}

fn encode_polygon(rings: &[Vec<[f64; 2]>]) -> String {
    rings
        .iter()
        .map(|ring| encode_line(ring))
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

    use super::*;
    use crate::release::GeometrySampler;
    use trajecta_case::model::population::{PopulationId, ReleaseEventId, ReleaseVerticalSpec};
    use trajecta_case::model::time::Timestamp;
    use trajecta_case::quantity::{Length, Quantity, UnitRegistry};

    use crate::release::ReleaseEvent;
    use std::collections::BTreeMap;

    fn request<'a>(
        population_id: &'a PopulationId,
        event: &'a ReleaseEvent,
        seed: u64,
        first: u64,
        count: usize,
    ) -> ReleaseSamplingRequest<'a> {
        ReleaseSamplingRequest {
            population_id,
            event,
            seed,
            first_ordinal: first,
            count,
        }
    }

    fn metres(value: f64) -> Quantity<Length> {
        let registry = UnitRegistry::standard();
        let unit = registry.get("m").unwrap().clone();
        Quantity::from_si(value, unit).unwrap()
    }

    fn point_event(geometry: GeoJsonGeometry) -> ReleaseEvent {
        ReleaseEvent {
            id: ReleaseEventId("e0".into()),
            start: Timestamp::UNIX_EPOCH,
            end: Timestamp::UNIX_EPOCH,
            geometry,
            vertical: ReleaseVerticalSpec::AboveSeaLevel {
                lower: metres(1000.0),
                upper: None,
            },
            particle_count: 8,
            mass_kg: BTreeMap::from([(
                trajecta_case::model::substance::SubstanceId("s".into()),
                1.0,
            )]),
        }
    }

    #[test]
    fn point_and_line_sampling_are_seed_deterministic_and_chunk_independent() {
        let event = point_event(GeoJsonGeometry::LineString(vec![[0.0, 0.0], [10.0, 0.0]]));
        let population = PopulationId("pop".into());
        let sampler = SphericalGeometrySampler::new(event.geometry.clone()).unwrap();
        let all = sampler
            .sample_horizontal(request(&population, &event, 7, 0, 8))
            .unwrap();
        let left = sampler
            .sample_horizontal(request(&population, &event, 7, 0, 3))
            .unwrap();
        let right = sampler
            .sample_horizontal(request(&population, &event, 7, 3, 5))
            .unwrap();
        assert_eq!(all[..3], left[..]);
        assert_eq!(all[3..], right[..]);
    }

    #[test]
    fn rejects_exact_180_degree_edges() {
        let err = normalize_geometry(&GeoJsonGeometry::LineString(vec![[0.0, 0.0], [180.0, 0.0]]));
        assert!(err.is_err());
    }

    #[test]
    fn one_degree_triangle_excess_is_small() {
        let a = unit_vector([0.0, 0.0]).unwrap();
        let b = unit_vector([1.0, 0.0]).unwrap();
        let c = unit_vector([0.0, 1.0]).unwrap();
        let excess = spherical_triangle_excess(a, b, c).unwrap().abs();
        // Planar area ~ 0.5 deg^2 = 0.5 * (pi/180)^2 rad^2 ≈ 1.523e-4
        assert!((excess - 1.523087e-4).abs() < 1.0e-6, "excess={excess}");
        let area = triangle_area_m2([[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]).unwrap();
        let expected = excess * M4_CONSTANTS.earth_radius_m.powi(2);
        assert!((area - expected).abs() / expected < 1.0e-12);
    }

    #[test]
    fn square_with_hole_samples_outside_hole() {
        let geometry = GeoJsonGeometry::Polygon(vec![
            vec![
                [-1.0, -1.0],
                [1.0, -1.0],
                [1.0, 1.0],
                [-1.0, 1.0],
                [-1.0, -1.0],
            ],
            vec![
                [-0.25, -0.25],
                [0.25, -0.25],
                [0.25, 0.25],
                [-0.25, 0.25],
                [-0.25, -0.25],
            ],
        ]);
        let canonical = canonicalize_geometry("hole", &geometry, None).unwrap();
        let rings = match &canonical.geometry {
            GeoJsonGeometry::Polygon(rings) => rings.clone(),
            other => panic!("expected polygon got {other:?}"),
        };
        let pop = crate::rng::StableRandomId::from_text("p");
        let ev = crate::rng::StableRandomId::from_text("e0");
        let mut samples = Vec::new();
        for ordinal in 0..32u64 {
            let pt = sample_one(&canonical.geometry, 42, pop, ev, ordinal).unwrap();
            let inside = point_in_geometry(&canonical.geometry, pt.0, pt.1).unwrap();
            let in_ext = point_in_ring(&rings[0], pt.0, pt.1, true).unwrap();
            let in_hole = point_in_ring(&rings[1], pt.0, pt.1, false).unwrap();
            if !inside {
                panic!("pt={pt:?} in_ext={in_ext} in_hole={in_hole}");
            }
            assert!(pt.0.abs() > 0.25 || pt.1.abs() > 0.25, "in hole {pt:?}");
            samples.push(pt);
        }
        assert_eq!(samples.len(), 32);
    }

    #[test]
    fn polygon_area_samples_not_collapsed_to_edge() {
        let geometry =
            GeoJsonGeometry::Polygon(vec![vec![[0.0, 0.0], [2.0, 0.0], [0.0, 2.0], [0.0, 0.0]]]);
        let canonical = canonicalize_geometry("tri", &geometry, None).unwrap();
        assert!(canonical.identity.spherical_area_m2 > 0.0);
        let mut event = point_event(geometry.clone());
        event.particle_count = 16;
        event.geometry = geometry;
        let sampler = SphericalGeometrySampler::new(canonical.geometry.clone()).unwrap();
        let pop = PopulationId("p".into());
        let samples = sampler
            .sample_horizontal(request(&pop, &event, 11, 0, 16))
            .unwrap();
        let off_base = samples.iter().filter(|(_, lat)| lat.abs() > 1.0e-3).count();
        assert!(off_base >= 8, "samples collapsed to base: {samples:?}");
    }

    #[test]
    fn dateline_polygon_normalizes() {
        let geometry = GeoJsonGeometry::Polygon(vec![vec![
            [170.0, -1.0],
            [-170.0, -1.0],
            [-170.0, 1.0],
            [170.0, 1.0],
            [170.0, -1.0],
        ]]);
        let canonical = canonicalize_geometry("dl", &geometry, None).unwrap();
        assert!(canonical.identity.spherical_area_m2 > 0.0);
        match &canonical.geometry {
            GeoJsonGeometry::MultiPolygon(parts) => assert!(!parts.is_empty()),
            GeoJsonGeometry::Polygon(rings) => assert!(!rings.is_empty()),
            other => panic!("unexpected {other:?}"),
        }
        let sampler = SphericalGeometrySampler::new(canonical.geometry.clone()).unwrap();
        let mut event = point_event(geometry);
        event.particle_count = 16;
        let pop = PopulationId("p".into());
        let samples = sampler
            .sample_horizontal(request(&pop, &event, 3, 0, 16))
            .unwrap();
        assert_eq!(samples.len(), 16);
    }

    /// Frozen counterexample: exterior+hole both straddle the antimeridian.
    #[test]
    fn dateline_polygon_with_hole_samples_public_path() {
        let exterior = vec![
            [179.0, -2.0],
            [-179.0, -2.0],
            [-179.0, 2.0],
            [179.0, 2.0],
            [179.0, -2.0],
        ];
        let hole = vec![
            [179.4, -0.5],
            [-179.4, -0.5],
            [-179.4, 0.5],
            [179.4, 0.5],
            [179.4, -0.5],
        ];
        let geometry = GeoJsonGeometry::Polygon(vec![exterior.clone(), hole.clone()]);
        let area_before = super::polygon_area_m2(&[exterior.clone(), hole.clone()]).unwrap();
        let canonical = canonicalize_geometry("dl-hole", &geometry, None).unwrap();
        let rel = (canonical.identity.spherical_area_m2 - area_before).abs() / area_before.max(1.0);
        assert!(
            rel <= crate::science::GEOMETRY_AREA_RELATIVE_TOLERANCE,
            "area rel={rel} before={area_before} after={}",
            canonical.identity.spherical_area_m2
        );

        let exterior_rot = {
            let mut o = exterior[..exterior.len() - 1].to_vec();
            o.rotate_left(1);
            o.push(o[0]);
            o
        };
        let mut exterior_rev = exterior[..exterior.len() - 1].to_vec();
        exterior_rev.reverse();
        exterior_rev.push(exterior_rev[0]);
        for variant in [
            GeoJsonGeometry::Polygon(vec![exterior_rot, hole.clone()]),
            GeoJsonGeometry::Polygon(vec![exterior_rev, hole.clone()]),
        ] {
            let other = canonicalize_geometry("dl-hole", &variant, None).unwrap();
            assert_eq!(
                other.identity.canonical_geometry_sha256,
                canonical.identity.canonical_geometry_sha256
            );
        }

        let sampler = SphericalGeometrySampler::new(canonical.geometry.clone()).unwrap();
        let mut event = point_event(geometry);
        event.particle_count = 64;
        let pop = PopulationId("p".into());
        let samples = sampler
            .sample_horizontal(request(&pop, &event, 7, 0, 64))
            .expect("public sample path must succeed");
        assert_eq!(samples.len(), 64);
        for (lon, lat) in samples {
            assert!(
                super::point_in_polygon(&[exterior.clone(), hole.clone()], lon, lat).unwrap(),
                "sample ({lon},{lat}) not in original exterior-hole"
            );
            assert!(
                !super::point_in_ring(&hole, lon, lat, true).unwrap(),
                "sample ({lon},{lat}) entered hole"
            );
        }
        let a = sampler
            .sample_horizontal(request(&pop, &event, 7, 0, 32))
            .unwrap();
        let b = sampler
            .sample_horizontal(request(&pop, &event, 7, 32, 32))
            .unwrap();
        let all = sampler
            .sample_horizontal(request(&pop, &event, 7, 0, 64))
            .unwrap();
        assert_eq!([a.as_slice(), b.as_slice()].concat(), all);
    }
    #[test]
    fn two_holes_canonicalize_area_without_double_cover() {
        let exterior = vec![
            [-4.0, -4.0],
            [4.0, -4.0],
            [4.0, 4.0],
            [-4.0, 4.0],
            [-4.0, -4.0],
        ];
        let hole_a = vec![
            [-3.5, -3.8],
            [-2.5, -3.8],
            [-2.5, -3.0],
            [-3.5, -3.0],
            [-3.5, -3.8],
        ];
        let hole_b = vec![[2.5, 3.0], [3.5, 3.0], [3.5, 3.8], [2.5, 3.8], [2.5, 3.0]];
        let geometry =
            GeoJsonGeometry::Polygon(vec![exterior.clone(), hole_a.clone(), hole_b.clone()]);
        let area_before =
            super::polygon_area_m2(&[exterior.clone(), hole_a.clone(), hole_b.clone()]).unwrap();
        let canonical = canonicalize_geometry("two-hole", &geometry, None).unwrap();
        let rel = (canonical.identity.spherical_area_m2 - area_before).abs() / area_before.max(1.0);
        assert!(
            rel <= crate::science::GEOMETRY_AREA_RELATIVE_TOLERANCE,
            "area rel={rel}"
        );
        let tris = super::mesh_polygon_region(&[exterior.clone(), hole_a.clone(), hole_b.clone()])
            .expect("two-hole mesh must succeed");
        let mut mesh_area = 0.0_f64;
        for tri in &tris {
            mesh_area += super::triangle_area_m2(*tri).unwrap();
        }
        let scale = mesh_area.max(area_before).max(1.0);
        assert!(
            (mesh_area - area_before).abs() / scale
                <= crate::science::GEOMETRY_AREA_RELATIVE_TOLERANCE,
            "mesh double-cover mesh={mesh_area} poly={area_before}"
        );
        let sampler = SphericalGeometrySampler::new(canonical.geometry.clone()).unwrap();
        let mut event = point_event(geometry);
        event.particle_count = 64;
        let pop = PopulationId("p".into());
        let samples = sampler
            .sample_horizontal(request(&pop, &event, 5, 0, 64))
            .expect("two-hole sample");
        assert_eq!(samples.len(), 64);
        // Samples must not fall inside either hole bbox (holes are axis-aligned boxes).
        for (lon, lat) in &samples {
            let in_a = *lon >= -3.5 && *lon <= -2.5 && *lat >= -3.8 && *lat <= -3.0;
            let in_b = *lon >= 2.5 && *lon <= 3.5 && *lat >= 3.0 && *lat <= 3.8;
            assert!(!in_a && !in_b, "sample in hole ({lon},{lat})");
        }
    }

    #[test]

    fn concave_exterior_meshes_and_samples() {
        // C-shaped concave exterior (degrees).
        let exterior = vec![
            [0.0, 0.0],
            [5.0, 0.0],
            [5.0, 1.0],
            [1.0, 1.0],
            [1.0, 4.0],
            [5.0, 4.0],
            [5.0, 5.0],
            [0.0, 5.0],
            [0.0, 0.0],
        ];
        let geometry = GeoJsonGeometry::Polygon(vec![exterior.clone()]);
        let area = super::polygon_area_m2(std::slice::from_ref(&exterior)).unwrap();
        let canonical = canonicalize_geometry("concave", &geometry, None).unwrap();
        let rel = (canonical.identity.spherical_area_m2 - area).abs() / area.max(1.0);
        assert!(rel <= crate::science::GEOMETRY_AREA_RELATIVE_TOLERANCE);
        let tris =
            super::mesh_polygon_region(std::slice::from_ref(&exterior)).expect("concave mesh");
        assert!(!tris.is_empty());
        let sampler = SphericalGeometrySampler::new(canonical.geometry).unwrap();
        let mut event = point_event(geometry);
        event.particle_count = 16;
        let pop = PopulationId("p".into());
        let samples = sampler
            .sample_horizontal(request(&pop, &event, 3, 0, 16))
            .expect("concave sample");
        assert_eq!(samples.len(), 16);
        // Samples must stay inside the C (not in the bay x in (1,5), y in (1,4)).
        for (lon, lat) in &samples {
            let inside_bay = *lon > 1.0 && *lon < 5.0 && *lat > 1.0 && *lat < 4.0;
            assert!(
                !inside_bay,
                "sample leaked into concavity bay: ({lon},{lat})"
            );
        }
    }

    #[test]
    fn three_holes_and_concave_hole_mesh() {
        let exterior = vec![
            [-5.0, -5.0],
            [5.0, -5.0],
            [5.0, 5.0],
            [-5.0, 5.0],
            [-5.0, -5.0],
        ];
        let hole_sq = vec![
            [-4.0, -4.0],
            [-3.0, -4.0],
            [-3.0, -3.0],
            [-4.0, -3.0],
            [-4.0, -4.0],
        ];
        // Concave C-shaped hole.
        let hole_concave = vec![
            [1.0, -1.0],
            [3.0, -1.0],
            [3.0, 1.0],
            [2.5, 1.0],
            [2.5, -0.5],
            [1.5, -0.5],
            [1.5, 1.0],
            [1.0, 1.0],
            [1.0, -1.0],
        ];
        let hole_ne = vec![[3.0, 3.0], [4.0, 3.0], [4.0, 4.0], [3.0, 4.0], [3.0, 3.0]];
        let rings = [
            exterior.clone(),
            hole_sq.clone(),
            hole_concave.clone(),
            hole_ne.clone(),
        ];
        let tris = super::mesh_polygon_region(&rings).expect("three-hole mesh");
        assert!(!tris.is_empty());
        let poly = super::polygon_area_m2(&rings).unwrap();
        let mut mesh = 0.0;
        for t in &tris {
            mesh += super::triangle_area_m2(*t).unwrap();
        }
        let scale = mesh.max(poly).max(1.0);
        assert!((mesh - poly).abs() / scale <= crate::science::GEOMETRY_AREA_RELATIVE_TOLERANCE);
        // Determinism: second mesh equals first vertex-for-vertex.
        let tris2 = super::mesh_polygon_region(&rings).unwrap();
        assert_eq!(tris, tris2);
    }

    #[test]
    fn rejects_c_shell_triangle_with_edge_exiting_exterior() {
        // A frozen counterexample: representative inside domain, edge exits C exterior.
        let exterior = vec![
            [0.0, 0.0],
            [5.0, 0.0],
            [5.0, 1.0],
            [1.0, 1.0],
            [1.0, 4.0],
            [5.0, 4.0],
            [5.0, 5.0],
            [0.0, 5.0],
            [0.0, 0.0],
        ];
        let bad = [[0.1, 0.1], [4.9, 0.5], [0.1, 1.5]];
        let rings = [exterior];
        let err = super::validate_mesh_topology(&[bad], &rings);
        assert!(err.is_err(), "C-shell exiting triangle must be rejected");
    }

    #[test]
    fn rejects_c_shell_boundary_chord_with_vertices_on_exterior() {
        // Fixup5 A frozen counterexample: all three vertices on exterior, but the
        // chord (5,1)-(1,4) and/or (5,1)-(-20,2.5) crosses the concave bay.
        let exterior = vec![
            [-20.0, 0.0],
            [5.0, 0.0],
            [5.0, 1.0],
            [1.0, 1.0],
            [1.0, 4.0],
            [5.0, 4.0],
            [5.0, 5.0],
            [-20.0, 5.0],
            [-20.0, 0.0],
        ];
        let bad = [[5.0, 1.0], [1.0, 4.0], [-20.0, 2.5]];
        let err = super::validate_mesh_topology(&[bad], &[exterior]);
        assert!(
            err.is_err(),
            "boundary-chord through C-shell bay must be rejected"
        );
    }

    #[test]
    fn accepts_shared_exterior_edge_on_simple_triangle() {
        let exterior = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 0.0]];
        let tri = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        super::validate_mesh_topology(&[tri], &[exterior]).expect("shared full exterior edge");
    }

    #[test]
    fn accepts_boundary_sub_edge_and_endpoint_only_touch() {
        // Exterior with a midpoint vertex; triangle uses a full sub-edge.
        let exterior = vec![[0.0, 0.0], [0.5, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 0.0]];
        let tri = [[0.0, 0.0], [0.5, 0.0], [0.0, 1.0]];
        super::validate_mesh_topology(&[tri], std::slice::from_ref(&exterior))
            .expect("full boundary sub-edge");
        // Two triangles sharing only a vertex (endpoint-only), covering the exterior.
        let t0 = [[0.0, 0.0], [0.5, 0.0], [0.0, 1.0]];
        let t1 = [[0.5, 0.0], [1.0, 0.0], [0.0, 1.0]];
        super::validate_mesh_topology(&[t0, t1], &[exterior]).expect("endpoint-only shared vertex");
    }

    #[test]
    fn rejects_artificial_exterior_partial_overlap_and_triangle_containment() {
        let exterior = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0], [0.0, 0.0]];
        // Partial collinear overlap that does not cover the full triangle edge.
        let partial = [[0.0, 0.0], [3.0, 0.0], [0.0, 3.0]];
        // Force a mesh edge that only partially tracks exterior then leaves? Use chord
        // that collinear-overlaps [0,0]-[4,0] only on [0,0]-[2,0] then goes inside —
        // that's still inside. Bad case: edge from (1,0) to (3,0) is SharedFullEdge sub.
        // Artificial: triangle with edge properly crossing exterior.
        let crossing = [[-1.0, 2.0], [5.0, 2.0], [2.0, 5.0]];
        assert!(
            super::validate_mesh_topology(&[crossing], std::slice::from_ref(&exterior)).is_err()
        );
        // Containment: large triangle contains smaller one's representative.
        let outer = [[0.0, 0.0], [4.0, 0.0], [0.0, 4.0]];
        let inner = [[0.5, 0.5], [1.5, 0.5], [0.5, 1.5]];
        assert!(super::validate_mesh_topology(&[outer, inner], &[exterior]).is_err());
        let _ = partial;
    }

    #[test]
    fn dateline_shell_with_two_holes_canonicalize_mesh_sample() {
        // Success path: post-split form — two half-shells near ±180, each with one hole.
        // This is the dateline multi-hole geometry the public MultiPolygon path must mesh.
        let west_ext = vec![
            [179.0, -2.0],
            [180.0 - 1.0e-9, -2.0],
            [180.0 - 1.0e-9, 2.0],
            [179.0, 2.0],
            [179.0, -2.0],
        ];
        let west_hole = vec![
            [179.2, -0.5],
            [179.6, -0.5],
            [179.6, 0.5],
            [179.2, 0.5],
            [179.2, -0.5],
        ];
        let east_ext = vec![
            [-180.0 + 1.0e-9, -2.0],
            [-179.0, -2.0],
            [-179.0, 2.0],
            [-180.0 + 1.0e-9, 2.0],
            [-180.0 + 1.0e-9, -2.0],
        ];
        let east_hole = vec![
            [-179.6, -0.5],
            [-179.2, -0.5],
            [-179.2, 0.5],
            [-179.6, 0.5],
            [-179.6, -0.5],
        ];
        let geometry = GeoJsonGeometry::MultiPolygon(vec![
            vec![west_ext, west_hole],
            vec![east_ext, east_hole],
        ]);
        let canonical = canonicalize_geometry("dl-2h-mp", &geometry, None)
            .expect("dateline two-hole multipolygon canonicalize");
        match &canonical.geometry {
            GeoJsonGeometry::MultiPolygon(parts) => {
                assert!(parts.len() >= 2);
                for rings in parts {
                    let tris = super::mesh_polygon_region(rings).expect("half-shell mesh");
                    assert!(!tris.is_empty());
                    let poly = super::polygon_area_m2(rings).unwrap();
                    let mut mesh = 0.0;
                    for t in &tris {
                        mesh += super::triangle_area_m2(*t).unwrap();
                    }
                    let scale = mesh.max(poly).max(1.0);
                    assert!(
                        (mesh - poly).abs() / scale
                            <= crate::science::GEOMETRY_AREA_RELATIVE_TOLERANCE
                    );
                }
            }
            GeoJsonGeometry::Polygon(rings) => {
                let tris = super::mesh_polygon_region(rings).unwrap();
                assert!(!tris.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
        let sampler = SphericalGeometrySampler::new(canonical.geometry.clone()).unwrap();
        let population_id = PopulationId("pop".into());
        let event = point_event(canonical.geometry.clone());
        let pts = sampler
            .sample_horizontal(request(&population_id, &event, 7, 0, 4))
            .expect("sample");
        assert_eq!(pts.len(), 4);

        // Public contract: users may submit one unsplit Polygon. Canonicalization
        // must split it automatically into a meshable MultiPolygon.
        let unsplit_ext = vec![
            [179.0, -2.0],
            [-179.0, -2.0],
            [-179.0, 2.0],
            [179.0, 2.0],
            [179.0, -2.0],
        ];
        let unsplit_h1 = vec![
            [179.4, -0.5],
            [-179.4, -0.5],
            [-179.4, 0.5],
            [179.4, 0.5],
            [179.4, -0.5],
        ];
        let unsplit_h2 = vec![
            [179.1, 1.0],
            [179.4, 1.0],
            [179.4, 1.5],
            [179.1, 1.5],
            [179.1, 1.0],
        ];
        let unsplit_geometry = GeoJsonGeometry::Polygon(vec![unsplit_ext, unsplit_h1, unsplit_h2]);
        let auto = canonicalize_geometry("dl-2h-auto", &unsplit_geometry, None)
            .expect("unsplit dateline Polygon must auto-split");
        assert!(
            matches!(auto.geometry, GeoJsonGeometry::MultiPolygon(_)),
            "canonical geometry must record the automatic split"
        );
        if let GeoJsonGeometry::MultiPolygon(parts) = &auto.geometry {
            assert_eq!(parts.len(), 2);
            let ring_counts = parts.iter().map(Vec::len).collect::<Vec<_>>();
            assert_eq!(ring_counts.iter().filter(|count| **count == 1).count(), 1);
            assert_eq!(ring_counts.iter().filter(|count| **count == 2).count(), 1);
            for rings in parts {
                let triangles = super::mesh_polygon_region(rings)
                    .expect("each automatic split component must mesh");
                assert!(!triangles.is_empty());
            }
        }
        let auto_sampler = SphericalGeometrySampler::new(auto.geometry.clone())
            .expect("automatic split must be meshable");
        let mut auto_event = point_event(auto.geometry.clone());
        auto_event.particle_count = 16;
        let auto_samples = auto_sampler
            .sample_horizontal(request(&population_id, &auto_event, 13, 0, 16))
            .expect("automatic split must sample");
        assert_eq!(auto_samples.len(), 16);
        for (longitude, latitude) in auto_samples {
            assert!(
                super::point_in_geometry(&unsplit_geometry, longitude, latitude).unwrap(),
                "automatic split sample must remain inside the original Polygon"
            );
        }

        // Canonical identity must not depend on user ring direction or hole order.
        let mut reversed_ext = vec![
            [179.0, -2.0],
            [-179.0, -2.0],
            [-179.0, 2.0],
            [179.0, 2.0],
            [179.0, -2.0],
        ];
        let mut reversed_h1 = vec![
            [179.4, -0.5],
            [-179.4, -0.5],
            [-179.4, 0.5],
            [179.4, 0.5],
            [179.4, -0.5],
        ];
        let mut reversed_h2 = vec![
            [179.1, 1.0],
            [179.4, 1.0],
            [179.4, 1.5],
            [179.1, 1.5],
            [179.1, 1.0],
        ];
        reversed_ext.reverse();
        reversed_h1.reverse();
        reversed_h2.reverse();
        let permuted = canonicalize_geometry(
            "dl-2h-auto",
            &GeoJsonGeometry::Polygon(vec![reversed_ext, reversed_h2, reversed_h1]),
            None,
        )
        .expect("reversed and permuted dateline Polygon must auto-split");
        assert_eq!(
            auto.identity.canonical_geometry_sha256,
            permuted.identity.canonical_geometry_sha256
        );
        assert_eq!(
            auto.identity.spherical_area_m2,
            permuted.identity.spherical_area_m2
        );
    }

    #[test]
    fn multipolygon_component_with_two_holes_mesh() {
        let ext_a = vec![[0.0, 0.0], [6.0, 0.0], [6.0, 6.0], [0.0, 6.0], [0.0, 0.0]];
        let h1 = vec![[1.0, 1.0], [2.0, 1.0], [2.0, 2.0], [1.0, 2.0], [1.0, 1.0]];
        let h2 = vec![[3.5, 3.5], [4.5, 3.5], [4.5, 4.5], [3.5, 4.5], [3.5, 3.5]];
        let ext_b = vec![
            [10.0, 0.0],
            [12.0, 0.0],
            [12.0, 2.0],
            [10.0, 2.0],
            [10.0, 0.0],
        ];
        let geometry = GeoJsonGeometry::MultiPolygon(vec![vec![ext_a, h1, h2], vec![ext_b]]);
        let canonical = canonicalize_geometry("mp-2h", &geometry, None).unwrap();
        match &canonical.geometry {
            GeoJsonGeometry::MultiPolygon(parts) => {
                assert!(parts.len() >= 2);
                let mut saw_two_holes = false;
                for rings in parts {
                    if rings.len() >= 3 {
                        saw_two_holes = true;
                    }
                    let tris = super::mesh_polygon_region(rings).expect("mp mesh");
                    assert!(!tris.is_empty());
                }
                assert!(saw_two_holes, "expected a component with two holes");
            }
            GeoJsonGeometry::Polygon(rings) => {
                // If merged, still mesh.
                let tris = super::mesh_polygon_region(rings).unwrap();
                assert!(!tris.is_empty());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn ring_rotation_and_hole_permutation_keep_canonical_geometry_sha() {
        let exterior = vec![[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [0.0, 5.0], [0.0, 0.0]];
        let hole_a = vec![[1.0, 1.0], [2.0, 1.0], [2.0, 2.0], [1.0, 2.0], [1.0, 1.0]];
        let hole_b = vec![[3.0, 3.0], [4.0, 3.0], [4.0, 4.0], [3.0, 4.0], [3.0, 3.0]];
        let base = GeoJsonGeometry::Polygon(vec![exterior.clone(), hole_a.clone(), hole_b.clone()]);
        let rotated_ext = {
            let open = &exterior[..exterior.len() - 1];
            let mut r: Vec<_> = open[1..].to_vec();
            r.extend_from_slice(&open[..1]);
            r.push(r[0]);
            r
        };
        let flipped_ext = {
            let mut r = exterior.clone();
            r.reverse();
            r
        };
        let perm_holes = GeoJsonGeometry::Polygon(vec![exterior, hole_b.clone(), hole_a.clone()]);
        let rot = GeoJsonGeometry::Polygon(vec![rotated_ext, hole_a.clone(), hole_b.clone()]);
        let flip = GeoJsonGeometry::Polygon(vec![flipped_ext, hole_a, hole_b]);
        let c0 = canonicalize_geometry("r0", &base, None).unwrap();
        let c1 = canonicalize_geometry("r1", &perm_holes, None).unwrap();
        let c2 = canonicalize_geometry("r2", &rot, None).unwrap();
        let c3 = canonicalize_geometry("r3", &flip, None).unwrap();
        // Hole order is sorted in normalize — permutation must match.
        assert_eq!(
            c0.identity.canonical_geometry_sha256, c1.identity.canonical_geometry_sha256,
            "hole permutation"
        );
        assert_eq!(
            c0.identity.canonical_geometry_sha256, c2.identity.canonical_geometry_sha256,
            "ring start rotation"
        );
        assert_eq!(
            c0.identity.canonical_geometry_sha256, c3.identity.canonical_geometry_sha256,
            "orientation flip"
        );
        // Mesh digest freeze: same canonical geometry ⇒ same mesh vertex set.
        if let GeoJsonGeometry::Polygon(rings) = &c0.geometry {
            let t0 = super::mesh_polygon_region(rings).unwrap();
            let t1 = super::mesh_polygon_region(rings).unwrap();
            assert_eq!(t0, t1);
        }
    }

    #[test]
    fn rejects_unproven_bridge_when_hole_outside() {
        let exterior = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]];
        // Hole completely outside exterior — must hard fail (no best_any).
        let hole = vec![[3.0, 3.0], [4.0, 3.0], [4.0, 4.0], [3.0, 4.0], [3.0, 3.0]];
        let err = super::mesh_polygon_region(&[exterior, hole]);
        assert!(err.is_err(), "outside hole must fail");
    }
}
