//! # Contract: particle boundary policies
//!
//! Policies receive both the step start and mutable proposal plus a sampler
//! capable of querying the physical path. This makes continuous surface and
//! domain intersection possible; endpoint clamping is not part of the API.

pub mod met_path;
pub use met_path::MetBoundaryPathSamplerFactory;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;
use trajecta_met::query::output::SampleStatus;

use crate::particle::{ParticleState, TerminationReason};

/// One physical sample along a proposed trajectory.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundarySample {
    /// Fraction of the full step in the closed interval `[0, 1]`.
    pub fraction: f64,
    /// Interpolated particle position at this fraction.
    pub position: ParticleState,
    /// Meteorology status at this point.
    pub meteorology_status: SampleStatus,
    /// Local physical surface height in metres ASL.
    pub surface_height_asl_m: Option<f64>,
    /// Native physical model-top height in metres ASL.
    pub model_top_height_asl_m: Option<f64>,
}

/// Exact path sampler used by continuous boundary policies.
pub trait BoundaryPathSampler {
    /// Returns ordered interpolation-cell path segments covering `[0, 1]`.
    ///
    /// Policies search segments from left to right so multiple terrain
    /// crossings cannot hide the earliest physical intersection. Each segment
    /// must be certified by the sampler to contain at most one down-crossing
    /// for every supplied scalar boundary and at most one inside-to-outside
    /// domain transition; interpolation-cell extrema therefore require an
    /// additional split rather than fixed-point probing by the policy.
    fn ordered_segments(&self) -> Result<Vec<BoundaryPathSegment>, BoundaryError>;

    /// Samples meteorology and path geometry at one deterministic fraction.
    fn sample(&mut self, fraction: f64) -> Result<BoundarySample, BoundaryError>;

    /// Rebinds the sampler to the post-collision remainder of the same step.
    ///
    /// `start_fraction` is expressed in the original full-step coordinate.
    /// Subsequent segments and samples again use a local `[0, 1]` coordinate
    /// over the remaining path. This is required for multiple continuous
    /// surface reflections; silently reusing the pre-collision path is invalid.
    fn retarget(
        &mut self,
        start_fraction: f64,
        start: &ParticleState,
        proposed: &ParticleState,
    ) -> Result<(), BoundaryError>;
}

/// One interpolation-cell interval crossed by a proposed trajectory.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundaryPathSegment {
    /// Inclusive fraction at which the segment begins.
    pub start_fraction: f64,
    /// Inclusive fraction at which the segment ends.
    pub end_fraction: f64,
}

impl BoundaryPathSegment {
    /// Validates a finite, non-empty segment contained in `[0, 1]`.
    pub fn validate(self) -> Result<(), BoundaryError> {
        if !self.start_fraction.is_finite()
            || !self.end_fraction.is_finite()
            || self.start_fraction < 0.0
            || self.end_fraction > 1.0
            || self.end_fraction <= self.start_fraction
        {
            return Err(BoundaryError::InvalidPathSegments);
        }
        Ok(())
    }
}

/// Validates a contiguous ordered path partition covering the complete step.
pub fn validate_ordered_path_segments(
    segments: &[BoundaryPathSegment],
) -> Result<(), BoundaryError> {
    if segments.is_empty() {
        return Err(BoundaryError::InvalidPathSegments);
    }
    for segment in segments {
        segment.validate()?;
    }
    if segments[0].start_fraction != 0.0
        || segments
            .last()
            .is_none_or(|segment| segment.end_fraction != 1.0)
        || segments
            .windows(2)
            .any(|pair| pair[0].end_fraction != pair[1].start_fraction)
    {
        return Err(BoundaryError::InvalidPathSegments);
    }
    Ok(())
}

/// Context required to apply one boundary policy.
pub struct BoundaryContext<'a> {
    /// Physical step start.
    pub start_time: Timestamp,
    /// Physical step end.
    pub end_time: Timestamp,
    /// Selected meteorology domain, when available.
    pub domain: Option<DomainId>,
    /// Deterministic path sampler.
    pub path: &'a mut dyn BoundaryPathSampler,
}

/// Typed result of one boundary-policy application.
#[derive(Clone, Debug, PartialEq)]
pub enum BoundaryDecision {
    /// Proposal remains active and may be passed to the next policy.
    Continue,
    /// Proposal was reflected and remains active.
    Reflected {
        /// Number of collisions resolved by this policy in the current step.
        collision_count: u32,
    },
    /// Proposal was terminated at a located path intersection.
    Terminated {
        /// Frozen normal or abnormal reason.
        reason: TerminationReason,
        /// Located fraction of the proposed full step.
        intersection_fraction: f64,
    },
}

/// One composable post-integration particle boundary policy.
pub trait BoundaryPolicy: Send + Sync {
    /// Returns the stable policy identifier.
    fn policy_id(&self) -> &'static str;

    /// Applies a deterministic policy to one complete proposed motion.
    fn apply(
        &self,
        start: &ParticleState,
        proposed: &mut ParticleState,
        context: &mut BoundaryContext<'_>,
    ) -> Result<BoundaryDecision, BoundaryError>;
}

/// Reflects proposed states below the local physical surface.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceReflect;

impl BoundaryPolicy for SurfaceReflect {
    fn policy_id(&self) -> &'static str {
        crate::science::SURFACE_REFLECT_ID
    }

    fn apply(
        &self,
        start: &ParticleState,
        proposed: &mut ParticleState,
        context: &mut BoundaryContext<'_>,
    ) -> Result<BoundaryDecision, BoundaryError> {
        if validate_boundary_state(start, proposed).is_err() {
            return Ok(terminate_proposal(
                proposed,
                TerminationReason::NonFiniteState,
                0.0,
            ));
        }
        let mut collision_count = 0_u32;
        let mut remaining_start_fraction = 0.0;
        loop {
            let crossing = match find_first_scalar_downcrossing(context.path, |sample| {
                let surface = sample
                    .surface_height_asl_m
                    .ok_or(BoundaryError::MissingContext)?;
                Ok(sample.position.height_asl_m - surface)
            }) {
                Ok(crossing) => crossing,
                Err(BoundaryError::RootFindingFailed | BoundaryError::ReflectionLimit) => {
                    return Ok(terminate_proposal(
                        proposed,
                        TerminationReason::ReflectionLimit,
                        remaining_start_fraction,
                    ));
                }
                Err(BoundaryError::MissingContext) => {
                    return Ok(terminate_proposal(
                        proposed,
                        TerminationReason::InvalidMeteorology,
                        remaining_start_fraction,
                    ));
                }
                Err(BoundaryError::InvalidParticleState) => {
                    return Ok(terminate_proposal(
                        proposed,
                        TerminationReason::NonFiniteState,
                        remaining_start_fraction,
                    ));
                }
                Err(error) => return Err(error),
            };
            let Some((local_fraction, mut collision)) = crossing else {
                return Ok(if collision_count == 0 {
                    BoundaryDecision::Continue
                } else {
                    BoundaryDecision::Reflected { collision_count }
                });
            };
            let global_fraction =
                remaining_start_fraction + (1.0 - remaining_start_fraction) * local_fraction;
            if collision_count == M4_MAXIMUM_REFLECTIONS {
                collision.position.status = crate::particle::ParticleStatus::Terminated {
                    reason: TerminationReason::ReflectionLimit,
                };
                *proposed = collision.position;
                return Ok(BoundaryDecision::Terminated {
                    reason: TerminationReason::ReflectionLimit,
                    intersection_fraction: global_fraction,
                });
            }

            let surface = collision
                .surface_height_asl_m
                .ok_or(BoundaryError::MissingContext)?;
            let remaining_vertical_displacement = proposed.height_asl_m - surface;
            let clearance = surface_clearance(surface);
            collision.position.height_asl_m = surface + clearance;
            collision.position.status = crate::particle::ParticleStatus::Alive;
            proposed.height_asl_m = surface + remaining_vertical_displacement.abs() + clearance;
            proposed.status = crate::particle::ParticleStatus::Alive;
            collision_count += 1;
            remaining_start_fraction = global_fraction;
            if let Err(error) =
                context
                    .path
                    .retarget(global_fraction, &collision.position, proposed)
            {
                if let Some(reason) = particle_level_reason(&error) {
                    return Ok(terminate_proposal(proposed, reason, global_fraction));
                }
                return Err(error);
            }
        }
    }
}

/// Terminates particles that cross the native model top.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModelTopTerminate;

impl BoundaryPolicy for ModelTopTerminate {
    fn policy_id(&self) -> &'static str {
        crate::science::MODEL_TOP_TERMINATE_ID
    }

    fn apply(
        &self,
        start: &ParticleState,
        proposed: &mut ParticleState,
        context: &mut BoundaryContext<'_>,
    ) -> Result<BoundaryDecision, BoundaryError> {
        if validate_boundary_state(start, proposed).is_err() {
            return Ok(terminate_proposal(
                proposed,
                TerminationReason::NonFiniteState,
                0.0,
            ));
        }
        let crossing = match find_first_optional_scalar_downcrossing(context.path, |sample| {
            sample
                .model_top_height_asl_m
                .map(|top| top - sample.position.height_asl_m)
        }) {
            Ok(crossing) => crossing,
            Err(error) => {
                if let Some(reason) = particle_level_reason(&error) {
                    return Ok(terminate_proposal(proposed, reason, 0.0));
                }
                return Err(error);
            }
        };
        let Some((fraction, mut sample)) = crossing else {
            return Ok(BoundaryDecision::Continue);
        };
        if let Some(top) = sample.model_top_height_asl_m {
            sample.position.height_asl_m = top;
        }
        terminate_at_sample(proposed, sample, TerminationReason::ModelTop);
        Ok(BoundaryDecision::Terminated {
            reason: TerminationReason::ModelTop,
            intersection_fraction: fraction,
        })
    }
}

/// Terminates particles outside a limited domain.
#[derive(Clone, Copy, Debug, Default)]
pub struct LimitedDomainTerminate;

impl BoundaryPolicy for LimitedDomainTerminate {
    fn policy_id(&self) -> &'static str {
        crate::science::LIMITED_DOMAIN_TERMINATE_ID
    }

    fn apply(
        &self,
        start: &ParticleState,
        proposed: &mut ParticleState,
        context: &mut BoundaryContext<'_>,
    ) -> Result<BoundaryDecision, BoundaryError> {
        if validate_boundary_state(start, proposed).is_err() {
            return Ok(terminate_proposal(
                proposed,
                TerminationReason::NonFiniteState,
                0.0,
            ));
        }
        let crossing = match find_first_domain_exit(context.path) {
            Ok(crossing) => crossing,
            Err(error) => {
                if let Some(reason) = particle_level_reason(&error) {
                    return Ok(terminate_proposal(proposed, reason, 0.0));
                }
                return Err(error);
            }
        };
        let Some((fraction, sample)) = crossing else {
            return Ok(BoundaryDecision::Continue);
        };
        terminate_at_sample(proposed, sample, TerminationReason::OutsideDomain);
        Ok(BoundaryDecision::Terminated {
            reason: TerminationReason::OutsideDomain,
            intersection_fraction: fraction,
        })
    }
}

/// Normalizes longitude for a globally periodic domain.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlobalPeriodicBoundary;

impl BoundaryPolicy for GlobalPeriodicBoundary {
    fn policy_id(&self) -> &'static str {
        crate::science::GLOBAL_PERIODIC_ID
    }

    fn apply(
        &self,
        start: &ParticleState,
        proposed: &mut ParticleState,
        _context: &mut BoundaryContext<'_>,
    ) -> Result<BoundaryDecision, BoundaryError> {
        if validate_boundary_state(start, proposed).is_err() {
            return Ok(terminate_proposal(
                proposed,
                TerminationReason::NonFiniteState,
                0.0,
            ));
        }
        proposed.longitude_degrees = normalize_longitude(proposed.longitude_degrees);
        proposed
            .validate()
            .map_err(|_| BoundaryError::InvalidParticleState)?;
        Ok(BoundaryDecision::Continue)
    }
}

const ROOT_BISECTION_ITERATIONS: usize =
    crate::science::M4_CONSTANTS.boundary_root_bisection_iterations as usize;
const M4_MAXIMUM_REFLECTIONS: u32 =
    crate::science::M4_CONSTANTS.maximum_surface_reflections_per_step;

fn validate_boundary_state(
    start: &ParticleState,
    proposed: &ParticleState,
) -> Result<(), BoundaryError> {
    start
        .validate()
        .and_then(|_| proposed.validate())
        .map_err(|_| BoundaryError::InvalidParticleState)
}

fn sample_at(
    path: &mut dyn BoundaryPathSampler,
    fraction: f64,
) -> Result<BoundarySample, BoundaryError> {
    if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
        return Err(BoundaryError::InvalidFraction);
    }
    let sample = path.sample(fraction)?;
    if !sample.fraction.is_finite()
        || !(0.0..=1.0).contains(&sample.fraction)
        || (sample.fraction - fraction).abs() > 8.0 * f64::EPSILON
        || sample.position.validate().is_err()
        || sample
            .surface_height_asl_m
            .is_some_and(|value| !value.is_finite())
        || sample
            .model_top_height_asl_m
            .is_some_and(|value| !value.is_finite())
    {
        return Err(BoundaryError::InvalidParticleState);
    }
    Ok(sample)
}

fn find_first_scalar_downcrossing<F>(
    path: &mut dyn BoundaryPathSampler,
    mut scalar: F,
) -> Result<Option<(f64, BoundarySample)>, BoundaryError>
where
    F: FnMut(&BoundarySample) -> Result<f64, BoundaryError>,
{
    let segments = path.ordered_segments()?;
    validate_ordered_path_segments(&segments)?;
    for segment in segments {
        let left_fraction = segment.start_fraction;
        let left = sample_at(path, left_fraction)?;
        if left.meteorology_status == SampleStatus::OutOfDomain {
            return Ok(None);
        }
        let left_value = scalar(&left)?;
        if !left_value.is_finite() {
            return Err(BoundaryError::InvalidParticleState);
        }
        let right_fraction = segment.end_fraction;
        let right = sample_at(path, right_fraction)?;
        if right.meteorology_status == SampleStatus::OutOfDomain {
            let (inside_fraction, inside, _, _) = bisect_domain_exit_bracket(
                path,
                left_fraction,
                left.clone(),
                right_fraction,
                right,
            )?;
            let inside_value = scalar(&inside)?;
            if !inside_value.is_finite() {
                return Err(BoundaryError::InvalidParticleState);
            }
            if left_value < 0.0 || (left_value == 0.0 && inside_value < 0.0) {
                return Ok(Some((left_fraction, left)));
            }
            if left_value > 0.0 && inside_value <= 0.0 {
                return bisect_scalar_downcrossing(
                    path,
                    left_fraction,
                    left_value,
                    inside_fraction,
                    inside_value,
                    scalar,
                )
                .map(Some);
            }
            return Ok(None);
        }
        let right_value = scalar(&right)?;
        if !right_value.is_finite() {
            return Err(BoundaryError::InvalidParticleState);
        }
        if left_value < 0.0 || (left_value == 0.0 && right_value < 0.0) {
            return Ok(Some((left_fraction, left)));
        }
        if left_value > 0.0 && right_value <= 0.0 {
            return bisect_scalar_downcrossing(
                path,
                left_fraction,
                left_value,
                right_fraction,
                right_value,
                scalar,
            )
            .map(Some);
        }
    }
    Ok(None)
}

fn bisect_scalar_downcrossing<F>(
    path: &mut dyn BoundaryPathSampler,
    mut left_fraction: f64,
    mut left_value: f64,
    mut right_fraction: f64,
    mut right_value: f64,
    mut scalar: F,
) -> Result<(f64, BoundarySample), BoundaryError>
where
    F: FnMut(&BoundarySample) -> Result<f64, BoundaryError>,
{
    if !(left_value > 0.0 && right_value <= 0.0) {
        return Err(BoundaryError::RootFindingFailed);
    }
    let mut right = sample_at(path, right_fraction)?;
    for _ in 0..ROOT_BISECTION_ITERATIONS {
        let midpoint_fraction = 0.5 * (left_fraction + right_fraction);
        if midpoint_fraction == left_fraction || midpoint_fraction == right_fraction {
            break;
        }
        let midpoint = sample_at(path, midpoint_fraction)?;
        let midpoint_value = scalar(&midpoint)?;
        if !midpoint_value.is_finite() {
            return Err(BoundaryError::InvalidParticleState);
        }
        if midpoint_value > 0.0 {
            left_fraction = midpoint_fraction;
            left_value = midpoint_value;
        } else {
            right_fraction = midpoint_fraction;
            right_value = midpoint_value;
            right = midpoint;
        }
    }
    if !(left_value > 0.0 && right_value <= 0.0) {
        return Err(BoundaryError::RootFindingFailed);
    }
    Ok((right_fraction, right))
}

fn find_first_optional_scalar_downcrossing<F>(
    path: &mut dyn BoundaryPathSampler,
    mut scalar: F,
) -> Result<Option<(f64, BoundarySample)>, BoundaryError>
where
    F: FnMut(&BoundarySample) -> Option<f64>,
{
    let first = sample_at(path, 0.0)?;
    if scalar(&first).is_none() {
        let last = sample_at(path, 1.0)?;
        return if scalar(&last).is_none() {
            Ok(None)
        } else {
            Err(BoundaryError::MissingContext)
        };
    }
    find_first_scalar_downcrossing(path, |sample| {
        scalar(sample).ok_or(BoundaryError::MissingContext)
    })
}

fn find_first_domain_exit(
    path: &mut dyn BoundaryPathSampler,
) -> Result<Option<(f64, BoundarySample)>, BoundaryError> {
    let segments = path.ordered_segments()?;
    validate_ordered_path_segments(&segments)?;
    for segment in segments {
        let left_fraction = segment.start_fraction;
        let left = sample_at(path, left_fraction)?;
        let left_inside = left.meteorology_status != SampleStatus::OutOfDomain;
        if !left_inside {
            return Ok(Some((left_fraction, left)));
        }
        let right_fraction = segment.end_fraction;
        let right = sample_at(path, right_fraction)?;
        if right.meteorology_status == SampleStatus::OutOfDomain {
            return bisect_domain_exit(path, left_fraction, left, right_fraction, right).map(Some);
        }
    }
    Ok(None)
}

fn bisect_domain_exit(
    path: &mut dyn BoundaryPathSampler,
    left_fraction: f64,
    left: BoundarySample,
    right_fraction: f64,
    right: BoundarySample,
) -> Result<(f64, BoundarySample), BoundaryError> {
    let (_, _, right_fraction, right) =
        bisect_domain_exit_bracket(path, left_fraction, left, right_fraction, right)?;
    Ok((right_fraction, right))
}

fn bisect_domain_exit_bracket(
    path: &mut dyn BoundaryPathSampler,
    mut left_fraction: f64,
    mut left: BoundarySample,
    mut right_fraction: f64,
    mut right: BoundarySample,
) -> Result<(f64, BoundarySample, f64, BoundarySample), BoundaryError> {
    if left.meteorology_status == SampleStatus::OutOfDomain
        || right.meteorology_status != SampleStatus::OutOfDomain
    {
        return Err(BoundaryError::RootFindingFailed);
    }
    for _ in 0..ROOT_BISECTION_ITERATIONS {
        let midpoint_fraction = 0.5 * (left_fraction + right_fraction);
        if midpoint_fraction == left_fraction || midpoint_fraction == right_fraction {
            break;
        }
        let midpoint = sample_at(path, midpoint_fraction)?;
        if midpoint.meteorology_status == SampleStatus::OutOfDomain {
            right_fraction = midpoint_fraction;
            right = midpoint;
        } else {
            left_fraction = midpoint_fraction;
            left = midpoint;
        }
    }
    Ok((left_fraction, left, right_fraction, right))
}

fn terminate_at_sample(
    proposed: &mut ParticleState,
    mut sample: BoundarySample,
    reason: TerminationReason,
) {
    sample.position.status = crate::particle::ParticleStatus::Terminated {
        reason: reason.clone(),
    };
    *proposed = sample.position;
}

fn terminate_proposal(
    proposed: &mut ParticleState,
    reason: TerminationReason,
    intersection_fraction: f64,
) -> BoundaryDecision {
    proposed.status = crate::particle::ParticleStatus::Terminated {
        reason: reason.clone(),
    };
    BoundaryDecision::Terminated {
        reason,
        intersection_fraction,
    }
}

fn particle_level_reason(error: &BoundaryError) -> Option<TerminationReason> {
    match error {
        BoundaryError::MissingContext => Some(TerminationReason::InvalidMeteorology),
        BoundaryError::InvalidParticleState => Some(TerminationReason::NonFiniteState),
        BoundaryError::RootFindingFailed | BoundaryError::ReflectionLimit => {
            Some(TerminationReason::NumericalFailure)
        }
        BoundaryError::NotImplemented
        | BoundaryError::InvalidFraction
        | BoundaryError::InvalidPathSegments => None,
    }
}

fn normalize_longitude(longitude_degrees: f64) -> f64 {
    (longitude_degrees + 180.0).rem_euclid(360.0) - 180.0
}

fn surface_clearance(surface_height_asl_m: f64) -> f64 {
    let ulp = ulp_size(surface_height_asl_m.abs().max(1.0));
    crate::science::M4_CONSTANTS
        .surface_clearance_min_m
        .max(ulp * f64::from(crate::science::M4_CONSTANTS.surface_clearance_ulps))
}

fn ulp_size(value: f64) -> f64 {
    if value == 0.0 {
        return f64::from_bits(1);
    }
    let next = f64::from_bits(value.to_bits() + 1);
    next - value
}

/// Boundary-policy failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundaryError {
    /// Boundary implementation has not been written yet.
    NotImplemented,
    /// Required path or meteorology context is absent.
    MissingContext,
    /// A requested path fraction lies outside `[0, 1]`.
    InvalidFraction,
    /// Ordered interpolation-cell path segments are incomplete or overlap.
    InvalidPathSegments,
    /// Proposed particle coordinates are non-finite.
    InvalidParticleState,
    /// Continuous intersection root finding did not converge.
    RootFindingFailed,
    /// More than the frozen per-step reflection limit was required.
    ReflectionLimit,
}

impl BoundaryError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented => "boundary.not_implemented",
            Self::MissingContext => "boundary.missing_context",
            Self::InvalidFraction => "boundary.invalid_fraction",
            Self::InvalidPathSegments => "boundary.invalid_path_segments",
            Self::InvalidParticleState => "boundary.invalid_particle_state",
            Self::RootFindingFailed => "boundary.root_finding_failed",
            Self::ReflectionLimit => "boundary.reflection_limit",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use trajecta_case::model::population::{PopulationId, ReleaseEventId};

    use super::*;
    use crate::particle::{ParticleId, ParticleOrigin, ParticleStatus};

    fn particle(longitude: f64, height: f64) -> ParticleState {
        ParticleState {
            id: ParticleId(1),
            population_id: PopulationId("p".into()),
            origin: ParticleOrigin::Release {
                event_id: ReleaseEventId("e".into()),
            },
            birth_time: Timestamp::UNIX_EPOCH,
            longitude_degrees: longitude,
            latitude_degrees: 0.0,
            height_asl_m: height,
            integration_offset_ns: 0,
            elapsed_age_ns: 0,
            dry_air_mass_kg: 0.0,
            mass_kg: BTreeMap::new(),
            sensitivity_weight: None,
            status: ParticleStatus::Alive,
        }
    }

    struct LinearPath {
        start: ParticleState,
        proposed: ParticleState,
        terrain: fn(f64) -> f64,
        model_top: Option<f64>,
        domain_max_longitude: Option<f64>,
        segments: Vec<BoundaryPathSegment>,
        retarget_count: u32,
    }

    impl BoundaryPathSampler for LinearPath {
        fn ordered_segments(&self) -> Result<Vec<BoundaryPathSegment>, BoundaryError> {
            Ok(self.segments.clone())
        }

        fn sample(&mut self, fraction: f64) -> Result<BoundarySample, BoundaryError> {
            let mut position = self.proposed.clone();
            position.longitude_degrees = self.start.longitude_degrees
                + fraction * (self.proposed.longitude_degrees - self.start.longitude_degrees);
            position.latitude_degrees = self.start.latitude_degrees
                + fraction * (self.proposed.latitude_degrees - self.start.latitude_degrees);
            position.height_asl_m = self.start.height_asl_m
                + fraction * (self.proposed.height_asl_m - self.start.height_asl_m);
            let status = if self
                .domain_max_longitude
                .is_some_and(|maximum| position.longitude_degrees > maximum)
            {
                SampleStatus::OutOfDomain
            } else {
                SampleStatus::Ok
            };
            Ok(BoundarySample {
                fraction,
                surface_height_asl_m: Some((self.terrain)(position.longitude_degrees)),
                model_top_height_asl_m: self.model_top,
                position,
                meteorology_status: status,
            })
        }

        fn retarget(
            &mut self,
            _start_fraction: f64,
            start: &ParticleState,
            proposed: &ParticleState,
        ) -> Result<(), BoundaryError> {
            self.start = start.clone();
            self.proposed = proposed.clone();
            self.segments = vec![BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }];
            self.retarget_count += 1;
            Ok(())
        }
    }

    struct AlwaysCollisionPath {
        template: ParticleState,
        retarget_count: u32,
    }

    struct MissingTerrainPath {
        template: ParticleState,
    }

    struct DomainExitMissingFieldsPath {
        start: ParticleState,
        proposed: ParticleState,
    }

    impl BoundaryPathSampler for DomainExitMissingFieldsPath {
        fn ordered_segments(&self) -> Result<Vec<BoundaryPathSegment>, BoundaryError> {
            Ok(vec![BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }])
        }

        fn sample(&mut self, fraction: f64) -> Result<BoundarySample, BoundaryError> {
            let mut position = self.start.clone();
            position.longitude_degrees = self.start.longitude_degrees
                + fraction * (self.proposed.longitude_degrees - self.start.longitude_degrees);
            let inside = position.longitude_degrees <= 1.0;
            Ok(BoundarySample {
                fraction,
                position,
                meteorology_status: if inside {
                    SampleStatus::Ok
                } else {
                    SampleStatus::OutOfDomain
                },
                surface_height_asl_m: inside.then_some(0.0),
                model_top_height_asl_m: inside.then_some(100.0),
            })
        }

        fn retarget(
            &mut self,
            _start_fraction: f64,
            start: &ParticleState,
            proposed: &ParticleState,
        ) -> Result<(), BoundaryError> {
            self.start = start.clone();
            self.proposed = proposed.clone();
            Ok(())
        }
    }

    impl BoundaryPathSampler for MissingTerrainPath {
        fn ordered_segments(&self) -> Result<Vec<BoundaryPathSegment>, BoundaryError> {
            Ok(vec![BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }])
        }

        fn sample(&mut self, fraction: f64) -> Result<BoundarySample, BoundaryError> {
            Ok(BoundarySample {
                fraction,
                position: self.template.clone(),
                meteorology_status: SampleStatus::InvalidVerticalColumn,
                surface_height_asl_m: None,
                model_top_height_asl_m: None,
            })
        }

        fn retarget(
            &mut self,
            _start_fraction: f64,
            _start: &ParticleState,
            _proposed: &ParticleState,
        ) -> Result<(), BoundaryError> {
            Ok(())
        }
    }

    impl BoundaryPathSampler for AlwaysCollisionPath {
        fn ordered_segments(&self) -> Result<Vec<BoundaryPathSegment>, BoundaryError> {
            Ok(vec![BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }])
        }

        fn sample(&mut self, fraction: f64) -> Result<BoundarySample, BoundaryError> {
            let mut position = self.template.clone();
            position.height_asl_m = 1.0 - 2.0 * fraction;
            Ok(BoundarySample {
                fraction,
                position,
                meteorology_status: SampleStatus::Ok,
                surface_height_asl_m: Some(0.0),
                model_top_height_asl_m: Some(100.0),
            })
        }

        fn retarget(
            &mut self,
            _start_fraction: f64,
            _start: &ParticleState,
            proposed: &ParticleState,
        ) -> Result<(), BoundaryError> {
            self.template = proposed.clone();
            self.retarget_count += 1;
            Ok(())
        }
    }

    fn context(path: &mut dyn BoundaryPathSampler) -> BoundaryContext<'_> {
        BoundaryContext {
            start_time: Timestamp::UNIX_EPOCH,
            end_time: Timestamp::new(60, 0).unwrap(),
            domain: None,
            path,
        }
    }

    #[test]
    fn path_segments_must_form_an_exact_ordered_partition() {
        assert_eq!(
            validate_ordered_path_segments(&[
                BoundaryPathSegment {
                    start_fraction: 0.0,
                    end_fraction: 0.25,
                },
                BoundaryPathSegment {
                    start_fraction: 0.25,
                    end_fraction: 1.0,
                },
            ]),
            Ok(())
        );
        assert_eq!(
            validate_ordered_path_segments(&[BoundaryPathSegment {
                start_fraction: 0.1,
                end_fraction: 1.0,
            }]),
            Err(BoundaryError::InvalidPathSegments)
        );
    }

    #[test]
    fn met_dependent_policies_stop_at_domain_exit_before_limited_termination() {
        let start = particle(0.0, 10.0);
        let mut proposed = particle(2.0, 10.0);
        let mut path = DomainExitMissingFieldsPath {
            start: start.clone(),
            proposed: proposed.clone(),
        };
        let mut boundary_context = context(&mut path);
        assert_eq!(
            SurfaceReflect
                .apply(&start, &mut proposed, &mut boundary_context)
                .unwrap(),
            BoundaryDecision::Continue
        );
        assert_eq!(
            ModelTopTerminate
                .apply(&start, &mut proposed, &mut boundary_context)
                .unwrap(),
            BoundaryDecision::Continue
        );
        let decision = LimitedDomainTerminate
            .apply(&start, &mut proposed, &mut boundary_context)
            .unwrap();
        assert!(matches!(
            decision,
            BoundaryDecision::Terminated {
                reason: TerminationReason::OutsideDomain,
                ..
            }
        ));
    }

    #[test]
    fn surface_reflection_locates_first_crossing_and_retargets_remainder() {
        let start = particle(0.0, 10.0);
        let mut proposed = particle(1.0, -10.0);
        let mut path = LinearPath {
            start: start.clone(),
            proposed: proposed.clone(),
            terrain: |_| 0.0,
            model_top: Some(100.0),
            domain_max_longitude: None,
            segments: vec![
                BoundaryPathSegment {
                    start_fraction: 0.0,
                    end_fraction: 0.4,
                },
                BoundaryPathSegment {
                    start_fraction: 0.4,
                    end_fraction: 1.0,
                },
            ],
            retarget_count: 0,
        };
        let decision = SurfaceReflect
            .apply(&start, &mut proposed, &mut context(&mut path))
            .unwrap();
        assert_eq!(decision, BoundaryDecision::Reflected { collision_count: 1 });
        assert!(proposed.height_asl_m > 10.0);
        assert_eq!(path.retarget_count, 1);
        assert_eq!(proposed.status, ParticleStatus::Alive);
    }

    #[test]
    fn reflection_limit_is_an_abnormal_particle_termination_not_a_run_error() {
        let start = particle(0.0, 1.0);
        let mut proposed = particle(1.0, -1.0);
        let mut path = AlwaysCollisionPath {
            template: start.clone(),
            retarget_count: 0,
        };
        let decision = SurfaceReflect
            .apply(&start, &mut proposed, &mut context(&mut path))
            .unwrap();
        assert!(matches!(
            decision,
            BoundaryDecision::Terminated {
                reason: TerminationReason::ReflectionLimit,
                ..
            }
        ));
        assert_eq!(path.retarget_count, M4_MAXIMUM_REFLECTIONS);
        assert!(matches!(
            proposed.status,
            ParticleStatus::Terminated {
                reason: TerminationReason::ReflectionLimit
            }
        ));
    }

    #[test]
    fn missing_surface_context_is_particle_local_invalid_meteorology() {
        let start = particle(0.0, 1.0);
        let mut proposed = particle(1.0, -1.0);
        let mut path = MissingTerrainPath {
            template: start.clone(),
        };
        let decision = SurfaceReflect
            .apply(&start, &mut proposed, &mut context(&mut path))
            .unwrap();
        assert!(matches!(
            decision,
            BoundaryDecision::Terminated {
                reason: TerminationReason::InvalidMeteorology,
                ..
            }
        ));
    }

    #[test]
    fn model_top_and_limited_domain_terminate_at_continuous_intersections() {
        let start = particle(0.0, 50.0);
        let mut proposed = particle(2.0, 150.0);
        let mut top_path = LinearPath {
            start: start.clone(),
            proposed: proposed.clone(),
            terrain: |_| 0.0,
            model_top: Some(100.0),
            domain_max_longitude: None,
            segments: vec![BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }],
            retarget_count: 0,
        };
        let top = ModelTopTerminate
            .apply(&start, &mut proposed, &mut context(&mut top_path))
            .unwrap();
        assert!(matches!(
            top,
            BoundaryDecision::Terminated {
                reason: TerminationReason::ModelTop,
                intersection_fraction
            } if (intersection_fraction - 0.5).abs() < 1.0e-12
        ));
        assert!((proposed.height_asl_m - 100.0).abs() < 1.0e-12);

        let start = particle(0.0, 10.0);
        let mut proposed = particle(2.0, 10.0);
        let mut domain_path = LinearPath {
            start: start.clone(),
            proposed: proposed.clone(),
            terrain: |_| 0.0,
            model_top: Some(100.0),
            domain_max_longitude: Some(1.0),
            segments: vec![BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }],
            retarget_count: 0,
        };
        let domain = LimitedDomainTerminate
            .apply(&start, &mut proposed, &mut context(&mut domain_path))
            .unwrap();
        assert!(matches!(
            domain,
            BoundaryDecision::Terminated {
                reason: TerminationReason::OutsideDomain,
                intersection_fraction
            } if (intersection_fraction - 0.5).abs() < 1.0e-12
        ));
    }

    #[test]
    fn global_periodic_boundary_normalizes_any_finite_wrap() {
        let start = particle(179.0, 10.0);
        let mut proposed = particle(541.0, 10.0);
        let mut path = LinearPath {
            start: start.clone(),
            proposed: proposed.clone(),
            terrain: |_| 0.0,
            model_top: None,
            domain_max_longitude: None,
            segments: vec![BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }],
            retarget_count: 0,
        };
        assert_eq!(
            GlobalPeriodicBoundary
                .apply(&start, &mut proposed, &mut context(&mut path))
                .unwrap(),
            BoundaryDecision::Continue
        );
        assert_eq!(proposed.longitude_degrees, -179.0);
    }
}
