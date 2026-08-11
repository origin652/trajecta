//! # Contract: release scheduling and deterministic geometry sampling
//!
//! Release events carry exact time, geometry, vertical-coordinate mode, mass,
//! and particle count. Sampling is deterministic and independent of worker
//! order. GeoJSON parsing and scientific allocation algorithms follow later.

pub mod geometry;
pub mod vertical;

use std::collections::{BTreeMap, BTreeSet};

use trajecta_case::model::population::{
    GeoJsonGeometry, PopulationId, ReleaseEventId, ReleaseVerticalSpec,
};
use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::Timestamp;

use crate::particle::{
    ParticleBatch, ParticleError, ParticleId, ParticleOrigin, ParticleStatus, SubstanceMassStore,
};
use crate::rng::{CounterRng, RELEASE_BIRTH_TIME_DIMENSION, RandomKey, StableRandomId};

pub use geometry::{CanonicalReleaseGeometry, SphericalGeometrySampler, canonicalize_geometry};
pub use vertical::SpecVerticalSampler;

/// Ordered collection of release events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReleaseSchedule {
    /// Events in deterministic schedule order.
    pub events: Vec<ReleaseEvent>,
}

impl ReleaseSchedule {
    /// Validates stable IDs, event intervals, mass, and deterministic ordering.
    pub fn validate(&self) -> Result<(), ReleaseError> {
        let mut ids = BTreeSet::new();
        let mut previous = None;
        for event in &self.events {
            event.validate()?;
            if !ids.insert(event.id.clone()) {
                return Err(ReleaseError::DuplicateEventId(event.id.clone()));
            }
            let key = (event.start, event.end, event.id.0.as_str());
            if previous.is_some_and(|value| value > key) {
                return Err(ReleaseError::UnorderedSchedule);
            }
            previous = Some(key);
        }
        Ok(())
    }
}

/// One exact release event.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseEvent {
    /// Stable event identifier.
    pub id: ReleaseEventId,
    /// Inclusive release start time.
    pub start: Timestamp,
    /// Inclusive release end time.
    pub end: Timestamp,
    /// Canonical typed GeoJSON after local-file resolution and normalization.
    pub geometry: GeoJsonGeometry,
    /// Vertical-coordinate interpretation and fixed/range values.
    pub vertical: ReleaseVerticalSpec,
    /// Requested particle count.
    pub particle_count: u64,
    /// Total released mass in kilograms by substance.
    pub mass_kg: BTreeMap<SubstanceId, f64>,
}

impl ReleaseEvent {
    /// Validates one canonical runtime release event.
    pub fn validate(&self) -> Result<(), ReleaseError> {
        if self.id.0.trim().is_empty() || self.end < self.start || self.particle_count == 0 {
            return Err(ReleaseError::InvalidEvent(
                "release ID, time interval, or particle count is invalid".into(),
            ));
        }
        if self.mass_kg.is_empty()
            || self.mass_kg.values().all(|mass| *mass == 0.0)
            || self.mass_kg.iter().any(|(substance, mass)| {
                substance.0.trim().is_empty() || !mass.is_finite() || *mass < 0.0
            })
        {
            return Err(ReleaseError::InvalidMass);
        }
        Ok(())
    }
}

/// Complete order-independent request for one release sampling chunk.
#[derive(Clone, Copy, Debug)]
pub struct ReleaseSamplingRequest<'a> {
    /// Stable owning population identity.
    pub population_id: &'a PopulationId,
    /// Exact release event being sampled.
    pub event: &'a ReleaseEvent,
    /// Declared or manifest-generated run seed.
    pub seed: u64,
    /// First stable event-local particle ordinal in this chunk.
    pub first_ordinal: u64,
    /// Number of consecutive ordinals in this chunk.
    pub count: usize,
}

impl ReleaseSamplingRequest<'_> {
    /// Validates that the requested ordinal chunk lies inside the event.
    pub fn validate(&self) -> Result<(), ReleaseError> {
        if self.first_ordinal > self.event.particle_count {
            return Err(ReleaseError::InvalidOrdinal {
                ordinal: self.first_ordinal,
                particle_count: self.event.particle_count,
            });
        }
        let count = u64::try_from(self.count).map_err(|_| ReleaseError::CountOverflow)?;
        let end = self
            .first_ordinal
            .checked_add(count)
            .ok_or(ReleaseError::CountOverflow)?;
        if end > self.event.particle_count {
            return Err(ReleaseError::InvalidOrdinal {
                ordinal: end.saturating_sub(1),
                particle_count: self.event.particle_count,
            });
        }
        Ok(())
    }
}

/// Deterministic horizontal geometry sampler.
pub trait GeometrySampler: Send + Sync {
    /// Samples longitude/latitude pairs for one exact ordinal chunk.
    fn sample_horizontal(
        &self,
        request: ReleaseSamplingRequest<'_>,
    ) -> Result<Vec<(f64, f64)>, ReleaseError>;
}

/// Deterministic vertical-coordinate sampler.
pub trait VerticalSampler: Send + Sync {
    /// Samples vertical coordinates for one exact ordinal chunk.
    fn sample_vertical(
        &self,
        request: ReleaseSamplingRequest<'_>,
    ) -> Result<Vec<f64>, ReleaseError>;
}

/// Fully resolved inputs for a contiguous release allocation chunk.
#[derive(Clone, Copy, Debug)]
pub struct ReleaseAllocationRequest<'a> {
    /// Stable owning population.
    pub population_id: &'a PopulationId,
    /// Canonical release event.
    pub event: &'a ReleaseEvent,
    /// Declared or manifest-generated run seed.
    pub seed: u64,
    /// First event-local ordinal represented by the supplied coordinates.
    pub first_ordinal: u64,
    /// Deterministically sampled longitude/latitude pairs.
    pub horizontal: &'a [(f64, f64)],
    /// Fully resolved geometric heights above mean sea level.
    pub height_asl_m: &'a [f64],
}

/// Allocates event mass and stable particle identities.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReleaseAllocator;

impl ReleaseAllocator {
    /// Builds one deterministic contiguous particle chunk.
    ///
    /// Horizontal and vertical sampling is deliberately external. This method
    /// owns the high-risk identity, birth-time, and exact mass-allocation
    /// rules, including final-particle floating-point compensation.
    pub fn allocate(request: ReleaseAllocationRequest<'_>) -> Result<ParticleBatch, ReleaseError> {
        request.event.validate()?;
        if request.population_id.0.trim().is_empty()
            || request.horizontal.len() != request.height_asl_m.len()
        {
            return Err(ReleaseError::InvalidEvent(
                "population ID or sampled coordinate lengths are invalid".into(),
            ));
        }
        let count = request.horizontal.len();
        ReleaseSamplingRequest {
            population_id: request.population_id,
            event: request.event,
            seed: request.seed,
            first_ordinal: request.first_ordinal,
            count,
        }
        .validate()?;
        if request.horizontal.iter().zip(request.height_asl_m).any(
            |((longitude, latitude), height)| {
                !longitude.is_finite()
                    || !latitude.is_finite()
                    || !(-90.0..=90.0).contains(latitude)
                    || !height.is_finite()
            },
        ) {
            return Err(ReleaseError::InvalidGeometry);
        }

        let duration_ns = event_duration_ns(request.event)?;
        let population_random_id = StableRandomId::from_text(&request.population_id.0);
        let event_random_id = StableRandomId::from_text(&request.event.id.0);
        let mut batch = ParticleBatch {
            id: Vec::with_capacity(count),
            population_id: Vec::with_capacity(count),
            origin: Vec::with_capacity(count),
            birth_time: Vec::with_capacity(count),
            longitude_degrees: Vec::with_capacity(count),
            latitude_degrees: Vec::with_capacity(count),
            height_asl_m: Vec::with_capacity(count),
            integration_offset_ns: vec![0; count],
            elapsed_age_ns: vec![0; count],
            dry_air_mass_kg: vec![0.0; count],
            status: vec![ParticleStatus::Alive; count],
            termination: vec![None; count],
            mass: SubstanceMassStore {
                mass_kg: request
                    .event
                    .mass_kg
                    .keys()
                    .cloned()
                    .map(|substance| (substance, Vec::with_capacity(count)))
                    .collect(),
            },
            adjoint: Default::default(),
            motion: Default::default(),
        };

        for (local_index, ((longitude, latitude), height)) in request
            .horizontal
            .iter()
            .zip(request.height_asl_m)
            .enumerate()
        {
            let local_ordinal =
                u64::try_from(local_index).map_err(|_| ReleaseError::CountOverflow)?;
            let ordinal = request
                .first_ordinal
                .checked_add(local_ordinal)
                .ok_or(ReleaseError::CountOverflow)?;
            let id = ParticleId::for_release(request.population_id, &request.event.id, ordinal);
            batch.id.push(id);
            batch.population_id.push(request.population_id.clone());
            batch.origin.push(ParticleOrigin::Release {
                event_id: request.event.id.clone(),
            });
            batch.birth_time.push(release_birth_time_with_ids(
                request.event,
                request.seed,
                population_random_id,
                event_random_id,
                id,
                ordinal,
                duration_ns,
            )?);
            batch.longitude_degrees.push(*longitude);
            batch.latitude_degrees.push(*latitude);
            batch.height_asl_m.push(*height);

            for (substance, total_mass) in &request.event.mass_kg {
                let share =
                    compensated_mass_share(*total_mass, request.event.particle_count, ordinal);
                batch
                    .mass
                    .mass_kg
                    .get_mut(substance)
                    .ok_or(ReleaseError::InvalidMass)?
                    .push(share);
            }
        }
        batch.validate().map_err(ReleaseError::Particle)?;
        Ok(batch)
    }
}

/// Returns one exact deterministic particle birth time.
pub fn release_birth_time(
    population_id: &PopulationId,
    event: &ReleaseEvent,
    seed: u64,
    ordinal: u64,
) -> Result<Timestamp, ReleaseError> {
    event.validate()?;
    if ordinal >= event.particle_count {
        return Err(ReleaseError::InvalidOrdinal {
            ordinal,
            particle_count: event.particle_count,
        });
    }
    let duration_ns = event_duration_ns(event)?;
    if duration_ns == 0 {
        return Ok(event.start);
    }
    let particle_id = ParticleId::for_release(population_id, &event.id, ordinal);
    release_birth_time_with_ids(
        event,
        seed,
        StableRandomId::from_text(&population_id.0),
        StableRandomId::from_text(&event.id.0),
        particle_id,
        ordinal,
        duration_ns,
    )
}

#[allow(clippy::too_many_arguments)]
fn release_birth_time_with_ids(
    event: &ReleaseEvent,
    seed: u64,
    population_random_id: StableRandomId,
    event_random_id: StableRandomId,
    particle_id: ParticleId,
    ordinal: u64,
    duration_ns: u64,
) -> Result<Timestamp, ReleaseError> {
    if duration_ns == 0 {
        return Ok(event.start);
    }
    let random = CounterRng::sample_u64(RandomKey {
        seed,
        population: population_random_id,
        lifecycle_event: event_random_id,
        particle: particle_id,
        sampling_dimension: RELEASE_BIRTH_TIME_DIMENSION,
        draw_index: 0,
    });
    let birth_offset =
        stratified_birth_offset_ns(duration_ns, event.particle_count, ordinal, random)?;
    add_unsigned_nanoseconds(event.start, birth_offset)
}

fn compensated_mass_share(total_mass: f64, particle_count: u64, ordinal: u64) -> f64 {
    let share = total_mass / particle_count as f64;
    if ordinal + 1 == particle_count {
        total_mass - share * (particle_count - 1) as f64
    } else {
        share
    }
}

fn event_duration_ns(event: &ReleaseEvent) -> Result<u64, ReleaseError> {
    let duration = timestamp_nanoseconds(event.end)
        .checked_sub(timestamp_nanoseconds(event.start))
        .ok_or(ReleaseError::TimeOverflow)?;
    u64::try_from(duration).map_err(|_| ReleaseError::TimeOverflow)
}

fn timestamp_nanoseconds(timestamp: Timestamp) -> i128 {
    i128::from(timestamp.seconds_since_unix_epoch()) * 1_000_000_000
        + i128::from(timestamp.nanosecond())
}

fn add_unsigned_nanoseconds(
    timestamp: Timestamp,
    nanoseconds: u64,
) -> Result<Timestamp, ReleaseError> {
    let value = timestamp_nanoseconds(timestamp)
        .checked_add(i128::from(nanoseconds))
        .ok_or(ReleaseError::TimeOverflow)?;
    let seconds =
        i64::try_from(value.div_euclid(1_000_000_000)).map_err(|_| ReleaseError::TimeOverflow)?;
    let nanos =
        u32::try_from(value.rem_euclid(1_000_000_000)).map_err(|_| ReleaseError::TimeOverflow)?;
    Timestamp::new(seconds, nanos).map_err(|_| ReleaseError::TimeOverflow)
}

/// Returns the deterministic nanosecond birth offset for one release stratum.
///
/// `random_u64` is mapped with exact multiply-high arithmetic, avoiding a
/// floating-point endpoint bias. A zero-duration event always returns zero.
pub fn stratified_birth_offset_ns(
    duration_ns: u64,
    particle_count: u64,
    ordinal: u64,
    random_u64: u64,
) -> Result<u64, ReleaseError> {
    if particle_count == 0 {
        return Err(ReleaseError::InvalidEvent(
            "particle_count must be greater than zero".into(),
        ));
    }
    if ordinal >= particle_count {
        return Err(ReleaseError::InvalidOrdinal {
            ordinal,
            particle_count,
        });
    }
    if duration_ns == 0 {
        return Ok(0);
    }
    let duration = u128::from(duration_ns);
    let count = u128::from(particle_count);
    let lower = u128::from(ordinal) * duration / count;
    let upper = (u128::from(ordinal) + 1) * duration / count;
    let width = upper - lower;
    if width == 0 {
        return u64::try_from(lower).map_err(|_| ReleaseError::TimeOverflow);
    }
    let within = (u128::from(random_u64) * width) >> 64;
    u64::try_from(lower + within).map_err(|_| ReleaseError::TimeOverflow)
}

/// Release schedule, sampling, or allocation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseError {
    /// Release algorithms have not been implemented yet.
    NotImplemented,
    /// Event interval or particle count is invalid.
    InvalidEvent(String),
    /// Geometry cannot be interpreted by the selected sampler.
    InvalidGeometry,
    /// Released mass is negative or non-finite.
    InvalidMass,
    /// Two canonical events share one stable identifier.
    DuplicateEventId(ReleaseEventId),
    /// Events are not sorted by `(start, end, id)`.
    UnorderedSchedule,
    /// Particle ordinal lies outside the exact event allocation.
    InvalidOrdinal {
        /// Requested ordinal.
        ordinal: u64,
        /// Event particle count.
        particle_count: u64,
    },
    /// Integer nanosecond arithmetic exceeded the portable representation.
    TimeOverflow,
    /// A platform-sized sampling chunk cannot be represented portably.
    CountOverflow,
    /// Allocated particle storage violated the frozen particle contract.
    Particle(ParticleError),
}

impl ReleaseError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented => "release.not_implemented",
            Self::InvalidEvent(_) => "release.invalid_event",
            Self::InvalidGeometry => "release.invalid_geometry",
            Self::InvalidMass => "release.invalid_mass",
            Self::DuplicateEventId(_) => "release.duplicate_event_id",
            Self::UnorderedSchedule => "release.unordered_schedule",
            Self::InvalidOrdinal { .. } => "release.invalid_ordinal",
            Self::TimeOverflow => "release.time_overflow",
            Self::CountOverflow => "release.count_overflow",
            Self::Particle(_) => "release.particle",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use trajecta_case::model::population::{GeoJsonGeometry, ReleaseVerticalSpec};
    use trajecta_case::model::substance::SubstanceId;
    use trajecta_case::quantity::{Dimension, Length, Quantity, Unit};

    use super::*;

    fn event(start: Timestamp, end: Timestamp, count: u64) -> ReleaseEvent {
        ReleaseEvent {
            id: ReleaseEventId("event".into()),
            start,
            end,
            geometry: GeoJsonGeometry::Point([0.0, 0.0]),
            vertical: ReleaseVerticalSpec::AboveSeaLevel {
                lower: Quantity::<Length>::from_si(
                    100.0,
                    Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap(),
                )
                .unwrap(),
                upper: None,
            },
            particle_count: count,
            mass_kg: BTreeMap::from([(SubstanceId("tracer".into()), 1.0)]),
        }
    }

    #[test]
    fn continuous_births_stay_inside_exact_integer_strata() {
        let offsets = (0..3)
            .map(|ordinal| stratified_birth_offset_ns(10, 3, ordinal, u64::MAX).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(offsets, vec![2, 5, 9]);
        assert_eq!(stratified_birth_offset_ns(0, 3, 2, 7), Ok(0));
        assert!(matches!(
            stratified_birth_offset_ns(10, 3, 3, 0),
            Err(ReleaseError::InvalidOrdinal { .. })
        ));
    }

    #[test]
    fn allocator_is_chunk_independent_and_compensates_final_mass_share() {
        let population = PopulationId("population".into());
        let event = event(Timestamp::UNIX_EPOCH, Timestamp::new(10, 0).unwrap(), 3);
        let horizontal = [(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)];
        let height = [100.0, 200.0, 300.0];
        let complete = ReleaseAllocator::allocate(ReleaseAllocationRequest {
            population_id: &population,
            event: &event,
            seed: 7,
            first_ordinal: 0,
            horizontal: &horizontal,
            height_asl_m: &height,
        })
        .unwrap();
        let mut chunked = ReleaseAllocator::allocate(ReleaseAllocationRequest {
            population_id: &population,
            event: &event,
            seed: 7,
            first_ordinal: 0,
            horizontal: &horizontal[..2],
            height_asl_m: &height[..2],
        })
        .unwrap();
        chunked
            .append(
                ReleaseAllocator::allocate(ReleaseAllocationRequest {
                    population_id: &population,
                    event: &event,
                    seed: 7,
                    first_ordinal: 2,
                    horizontal: &horizontal[2..],
                    height_asl_m: &height[2..],
                })
                .unwrap(),
            )
            .unwrap();
        assert_eq!(complete, chunked);
        let masses = &complete.mass.mass_kg[&SubstanceId("tracer".into())];
        assert_eq!(masses.iter().sum::<f64>(), 1.0);
        assert_eq!(complete.integration_offset_ns, vec![0; 3]);
        assert_eq!(complete.elapsed_age_ns, vec![0; 3]);
        assert!(
            complete
                .birth_time
                .windows(2)
                .all(|pair| pair[0] <= pair[1])
        );
    }

    #[test]
    fn schedule_rejects_duplicate_ids_and_noncanonical_order() {
        let first = event(Timestamp::UNIX_EPOCH, Timestamp::UNIX_EPOCH, 1);
        let mut duplicate = first.clone();
        duplicate.start = Timestamp::new(1, 0).unwrap();
        duplicate.end = duplicate.start;
        assert!(matches!(
            ReleaseSchedule {
                events: vec![first.clone(), duplicate]
            }
            .validate(),
            Err(ReleaseError::DuplicateEventId(_))
        ));

        let mut later = first.clone();
        later.id = ReleaseEventId("later".into());
        later.start = Timestamp::new(2, 0).unwrap();
        later.end = later.start;
        assert_eq!(
            ReleaseSchedule {
                events: vec![later, first]
            }
            .validate(),
            Err(ReleaseError::UnorderedSchedule)
        );
    }

    #[test]
    fn runtime_event_rejects_empty_or_all_zero_mass() {
        let mut value = event(Timestamp::UNIX_EPOCH, Timestamp::UNIX_EPOCH, 1);
        value.mass_kg.clear();
        assert_eq!(value.validate(), Err(ReleaseError::InvalidMass));
        value.mass_kg.insert(SubstanceId("tracer".into()), 0.0);
        assert_eq!(value.validate(), Err(ReleaseError::InvalidMass));
    }
}
