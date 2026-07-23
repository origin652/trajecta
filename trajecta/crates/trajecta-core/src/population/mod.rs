//! # Contract: full particle-population lifecycle
//!
//! A population strategy owns initialization, pre-step work, emission,
//! post-advection accounting, boundary maintenance, and finalization. Domain
//! filling preserves residual boundary mass across steps and chooses inflow
//! according to integration direction.

mod vertical_resolve;
pub use vertical_resolve::MetReleaseVerticalResolver;

use std::collections::{BTreeMap, BTreeSet};

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, ReleaseDrivenSpec, ReleaseVerticalSpec,
};
use trajecta_case::model::time::{Direction, Timestamp};
use trajecta_met::field::{CanonicalField, FieldQuality};
use trajecta_met::query::engine::{ExecutionContext, MetEngine};

use crate::clock::{SignedDuration, SimulationClock, StepBoundary, add_timestamp};
use crate::particle::ParticleBatch;
use crate::reference::{ReferenceError, flexpart_pv60_ozone_mass_kg};
use crate::release::{
    GeometrySampler, ReleaseAllocationRequest, ReleaseAllocator, ReleaseError, ReleaseEvent,
    ReleaseSamplingRequest, ReleaseSchedule, VerticalSampler, release_birth_time,
};

/// Mutable services and physical time supplied to a population strategy.
pub struct PopulationContext<'a> {
    /// Current physical time.
    pub time: Timestamp,
    /// Configured integration direction.
    pub direction: Direction,
    /// Signed planned step for per-step callbacks; absent during init/finalize.
    pub step: Option<SignedDuration>,
    /// Stable zero-based numerical-step index for per-step lifecycle events.
    pub step_index: Option<u64>,
    /// Declared or running-manifest generated seed.
    pub random_seed: u64,
    /// Meteorology engine accessed through explicit prepare/query phases.
    pub meteorology: &'a mut MetEngine,
    /// Caller-owned parallel execution context.
    pub execution: &'a dyn ExecutionContext,
    /// Selected domain when the lifecycle operation is domain-specific.
    pub domain: Option<DomainId>,
}

/// Persistent private state associated with a built-in population strategy.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PopulationState {
    /// No persistent state has been initialized.
    #[default]
    Uninitialized,
    /// Release schedule cursor.
    ReleaseDriven {
        /// Number of exact particle births emitted so far.
        emitted_birth_count: usize,
    },
    /// Domain-fill residual mass and counters.
    DomainFill {
        /// Boundary-face residual mass accumulator.
        residual_mass: BoundaryMassAccumulator,
        /// Number of particles seeded so far.
        seeded_particles: u64,
    },
}

/// Residual dry-air mass by deterministic boundary-face identifier.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundaryMassAccumulator {
    /// Residual kilograms below one particle threshold.
    pub residual_mass_kg: BTreeMap<u64, f64>,
}

/// Converts a declared mass budget into deterministic particle seeds.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParticleSeeder;

impl ParticleSeeder {
    /// Seeds a particle batch without depending on worker or iteration order.
    pub fn seed(_target_count: usize) -> Result<ParticleBatch, PopulationError> {
        Err(PopulationError::NotImplemented)
    }
}

/// Full lifecycle interface for one particle-population strategy.
pub trait PopulationStrategy: Send {
    /// Returns the stable lifecycle implementation identifier.
    fn model_id(&self) -> &'static str;

    /// Returns an auditable clone of persistent strategy state.
    fn snapshot_state(&self) -> PopulationState;

    /// Returns exact lifecycle events inside one requested signed step.
    ///
    /// The runner merges these boundaries with meteorology, output, and end
    /// events before calling [`crate::clock::StepPlanner`].
    fn step_boundaries(
        &self,
        _clock: SimulationClock,
        _requested: SignedDuration,
    ) -> Result<Vec<StepBoundary>, PopulationError> {
        Ok(Vec::new())
    }

    /// Initializes persistent strategy state and optional initial particles.
    fn initialize(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError>;

    /// Performs deterministic accounting immediately before a numerical step.
    fn before_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError>;

    /// Emits scheduled or boundary-generated particles for the current event.
    fn emit_particles(
        &mut self,
        context: &mut PopulationContext<'_>,
    ) -> Result<ParticleBatch, PopulationError>;

    /// Updates lifecycle accounting after particle advection.
    fn after_advection(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError>;

    /// Removes outflow particles and replenishes eligible inflow boundaries.
    fn apply_boundary_maintenance(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError>;

    /// Finalizes auditable mass and lifecycle summaries.
    fn finalize(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError>;
}

/// Ordinary scheduled-release population.
pub struct ReleaseDrivenPopulation {
    /// Portable Case configuration.
    pub specification: ReleaseDrivenSpec,
    /// Persistent runtime state.
    pub state: PopulationState,
    schedule: ReleaseSchedule,
    geometry_sampler: Box<dyn GeometrySampler>,
    vertical_sampler: Box<dyn VerticalSampler>,
    vertical_resolver: Box<dyn ReleaseVerticalResolver>,
    births: Vec<ScheduledBirth>,
    emitted: Vec<bool>,
}

/// Resolves sampled event-native vertical values to geometric ASL height.
///
/// AGL and pressure implementations must perform official meteorological
/// queries at each particle's exact birth time. They may not clamp, resample,
/// or silently drop invalid release points.
pub trait ReleaseVerticalResolver: Send + Sync {
    /// Returns one ASL height for every sampled particle.
    fn resolve_asl(
        &self,
        event: &ReleaseEvent,
        birth_times: &[Timestamp],
        horizontal: &[(f64, f64)],
        sampled_vertical: &[f64],
        context: &mut PopulationContext<'_>,
    ) -> Result<Vec<f64>, PopulationError>;
}

/// Direct resolver for already-ASL release events.
#[derive(Clone, Copy, Debug, Default)]
pub struct DirectAslReleaseResolver;

impl ReleaseVerticalResolver for DirectAslReleaseResolver {
    fn resolve_asl(
        &self,
        event: &ReleaseEvent,
        birth_times: &[Timestamp],
        horizontal: &[(f64, f64)],
        sampled_vertical: &[f64],
        _context: &mut PopulationContext<'_>,
    ) -> Result<Vec<f64>, PopulationError> {
        if !matches!(event.vertical, ReleaseVerticalSpec::AboveSeaLevel { .. })
            || birth_times.len() != horizontal.len()
            || horizontal.len() != sampled_vertical.len()
            || sampled_vertical.iter().any(|value| !value.is_finite())
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        Ok(sampled_vertical.to_vec())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ScheduledBirth {
    time: Timestamp,
    event_index: usize,
    ordinal: u64,
}

impl ReleaseDrivenPopulation {
    /// Constructs the ordinary-release lifecycle from a resolved schedule and
    /// deterministic sampling implementations.
    #[must_use]
    pub fn new(
        specification: ReleaseDrivenSpec,
        schedule: ReleaseSchedule,
        geometry_sampler: Box<dyn GeometrySampler>,
        vertical_sampler: Box<dyn VerticalSampler>,
        vertical_resolver: Box<dyn ReleaseVerticalResolver>,
    ) -> Self {
        Self {
            specification,
            state: PopulationState::Uninitialized,
            schedule,
            geometry_sampler,
            vertical_sampler,
            vertical_resolver,
            births: Vec::new(),
            emitted: Vec::new(),
        }
    }
}

/// Dry-air-mass domain-filling population.
#[derive(Clone, Debug)]
pub struct DomainFillAirMass {
    /// Portable Case configuration.
    pub specification: DomainFillAirMassSpec,
    /// Persistent runtime state.
    pub state: PopulationState,
}

/// Stratospheric-ozone domain-filling population.
#[derive(Clone, Debug)]
pub struct DomainFillStratosphericOzone {
    /// Portable Case configuration.
    pub specification: DomainFillStratosphericOzoneSpec,
    /// Persistent runtime state.
    pub state: PopulationState,
}

macro_rules! unimplemented_population {
    ($type_name:ty, $model_id:expr) => {
        impl PopulationStrategy for $type_name {
            fn model_id(&self) -> &'static str {
                $model_id
            }

            fn snapshot_state(&self) -> PopulationState {
                self.state.clone()
            }

            fn initialize(
                &mut self,
                _context: &mut PopulationContext<'_>,
                _particles: &mut ParticleBatch,
            ) -> Result<(), PopulationError> {
                Err(PopulationError::NotImplemented)
            }

            fn before_step(
                &mut self,
                _context: &mut PopulationContext<'_>,
                _particles: &ParticleBatch,
            ) -> Result<(), PopulationError> {
                Err(PopulationError::NotImplemented)
            }

            fn emit_particles(
                &mut self,
                _context: &mut PopulationContext<'_>,
            ) -> Result<ParticleBatch, PopulationError> {
                Err(PopulationError::NotImplemented)
            }

            fn after_advection(
                &mut self,
                _context: &mut PopulationContext<'_>,
                _particles: &ParticleBatch,
            ) -> Result<(), PopulationError> {
                Err(PopulationError::NotImplemented)
            }

            fn apply_boundary_maintenance(
                &mut self,
                _context: &mut PopulationContext<'_>,
                _particles: &mut ParticleBatch,
            ) -> Result<(), PopulationError> {
                Err(PopulationError::NotImplemented)
            }

            fn finalize(
                &mut self,
                _context: &mut PopulationContext<'_>,
                _particles: &ParticleBatch,
            ) -> Result<(), PopulationError> {
                Err(PopulationError::NotImplemented)
            }
        }
    };
}

unimplemented_population!(DomainFillAirMass, crate::science::DRY_AIR_DOMAIN_FILL_ID);
unimplemented_population!(
    DomainFillStratosphericOzone,
    crate::science::OZONE_DOMAIN_FILL_ID
);

impl PopulationStrategy for ReleaseDrivenPopulation {
    fn model_id(&self) -> &'static str {
        crate::science::RELEASE_DRIVEN_POPULATION_ID
    }

    fn snapshot_state(&self) -> PopulationState {
        self.state.clone()
    }

    fn step_boundaries(
        &self,
        clock: SimulationClock,
        requested: SignedDuration,
    ) -> Result<Vec<StepBoundary>, PopulationError> {
        let target =
            add_timestamp(clock.current, requested).map_err(|_| PopulationError::TimeOverflow)?;
        let mut by_time = BTreeMap::<Timestamp, String>::new();
        for (index, birth) in self.births.iter().enumerate() {
            if self.emitted.get(index).copied().unwrap_or(false) {
                continue;
            }
            let inside = match clock.direction {
                Direction::Forward => birth.time > clock.current && birth.time <= target,
                Direction::Backward => birth.time < clock.current && birth.time >= target,
            };
            if inside {
                by_time
                    .entry(birth.time)
                    .or_insert_with(|| self.schedule.events[birth.event_index].id.0.clone());
            }
        }
        let mut boundaries = by_time
            .into_iter()
            .map(|(time, event_id)| StepBoundary::Release { time, event_id })
            .collect::<Vec<_>>();
        if clock.direction == Direction::Backward {
            boundaries.reverse();
        }
        Ok(boundaries)
    }

    fn initialize(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        if self.state != PopulationState::Uninitialized || self.specification.id.0.trim().is_empty()
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        self.schedule.validate().map_err(PopulationError::Release)?;
        validate_release_schedule_identity(&self.specification, &self.schedule)?;
        let birth_count = self.schedule.events.iter().try_fold(0_u64, |total, event| {
            total.checked_add(event.particle_count)
        });
        let birth_count = birth_count
            .and_then(|count| usize::try_from(count).ok())
            .ok_or(PopulationError::ResourceLimit)?;
        self.births.clear();
        self.births
            .try_reserve_exact(birth_count)
            .map_err(|_| PopulationError::ResourceLimit)?;
        for (event_index, event) in self.schedule.events.iter().enumerate() {
            for ordinal in 0..event.particle_count {
                self.births.push(ScheduledBirth {
                    time: release_birth_time(
                        &self.specification.id,
                        event,
                        context.random_seed,
                        ordinal,
                    )
                    .map_err(PopulationError::Release)?,
                    event_index,
                    ordinal,
                });
            }
        }
        self.births.sort_by(|left, right| {
            (
                left.time,
                &self.schedule.events[left.event_index].id,
                left.ordinal,
            )
                .cmp(&(
                    right.time,
                    &self.schedule.events[right.event_index].id,
                    right.ordinal,
                ))
        });
        self.emitted.clear();
        self.emitted
            .try_reserve_exact(self.births.len())
            .map_err(|_| PopulationError::ResourceLimit)?;
        self.emitted.resize(self.births.len(), false);
        self.state = PopulationState::ReleaseDriven {
            emitted_birth_count: 0,
        };
        Ok(())
    }

    fn before_step(
        &mut self,
        _context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        particles
            .validate()
            .map(|_| ())
            .map_err(|_| PopulationError::InvalidParticleBatch)
    }

    fn emit_particles(
        &mut self,
        context: &mut PopulationContext<'_>,
    ) -> Result<ParticleBatch, PopulationError> {
        if !matches!(self.state, PopulationState::ReleaseDriven { .. }) {
            return Err(PopulationError::InvalidConfiguration);
        }
        let pending = self
            .births
            .iter()
            .enumerate()
            .filter_map(|(index, birth)| {
                (!self.emitted[index] && birth.time == context.time).then_some(index)
            })
            .collect::<Vec<_>>();
        let mut emitted_batch = ParticleBatch::default();
        let mut cursor = 0;
        while cursor < pending.len() {
            let first_index = pending[cursor];
            let first = self.births[first_index];
            let mut end = cursor + 1;
            while end < pending.len() {
                let next = self.births[pending[end]];
                if next.event_index != first.event_index
                    || next.ordinal != first.ordinal + (end - cursor) as u64
                {
                    break;
                }
                end += 1;
            }
            let event = &self.schedule.events[first.event_index];
            let request = ReleaseSamplingRequest {
                population_id: &self.specification.id,
                event,
                seed: context.random_seed,
                first_ordinal: first.ordinal,
                count: end - cursor,
            };
            let horizontal = self
                .geometry_sampler
                .sample_horizontal(request)
                .map_err(PopulationError::Release)?;
            let sampled_vertical = self
                .vertical_sampler
                .sample_vertical(request)
                .map_err(PopulationError::Release)?;
            if horizontal.len() != request.count || sampled_vertical.len() != request.count {
                return Err(PopulationError::InvalidConfiguration);
            }
            let birth_times = (0..request.count)
                .map(|offset| {
                    release_birth_time(
                        &self.specification.id,
                        event,
                        context.random_seed,
                        first.ordinal + offset as u64,
                    )
                    .map_err(PopulationError::Release)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if birth_times.iter().any(|time| *time != context.time) {
                return Err(PopulationError::InvalidConfiguration);
            }
            let height_asl_m = self.vertical_resolver.resolve_asl(
                event,
                &birth_times,
                &horizontal,
                &sampled_vertical,
                context,
            )?;
            emitted_batch
                .append(
                    ReleaseAllocator::allocate(ReleaseAllocationRequest {
                        population_id: &self.specification.id,
                        event,
                        seed: context.random_seed,
                        first_ordinal: first.ordinal,
                        horizontal: &horizontal,
                        height_asl_m: &height_asl_m,
                    })
                    .map_err(PopulationError::Release)?,
                )
                .map_err(|_| PopulationError::InvalidParticleBatch)?;
            cursor = end;
        }
        for index in pending {
            self.emitted[index] = true;
        }
        self.state = PopulationState::ReleaseDriven {
            emitted_birth_count: self.emitted.iter().filter(|value| **value).count(),
        };
        Ok(emitted_batch)
    }

    fn after_advection(
        &mut self,
        _context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        particles
            .validate()
            .map(|_| ())
            .map_err(|_| PopulationError::InvalidParticleBatch)
    }

    fn apply_boundary_maintenance(
        &mut self,
        _context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        particles
            .validate()
            .map(|_| ())
            .map_err(|_| PopulationError::InvalidParticleBatch)
    }

    fn finalize(
        &mut self,
        _context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        if self.emitted.iter().any(|emitted| !emitted) {
            return Err(PopulationError::InvalidConfiguration);
        }
        particles
            .validate()
            .map(|_| ())
            .map_err(|_| PopulationError::InvalidParticleBatch)
    }
}

fn validate_release_schedule_identity(
    specification: &ReleaseDrivenSpec,
    schedule: &ReleaseSchedule,
) -> Result<(), PopulationError> {
    if specification.events.len() != schedule.events.len() {
        return Err(PopulationError::InvalidConfiguration);
    }
    let mut seen = BTreeSet::new();
    for event in &schedule.events {
        let spec = specification
            .events
            .iter()
            .find(|candidate| candidate.id == event.id)
            .ok_or(PopulationError::InvalidConfiguration)?;
        if !seen.insert(&event.id)
            || spec.start != event.start
            || spec.end != event.end
            || spec.particle_count != event.particle_count
            || spec.vertical != event.vertical
            || spec.mass.len() != event.mass_kg.len()
            || spec.mass.iter().any(|(substance, mass)| {
                event.mass_kg.get(substance).copied() != Some(mass.value_si())
            })
        {
            return Err(PopulationError::InvalidConfiguration);
        }
    }
    Ok(())
}

/// Scalar inputs presented to an ozone assignment rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OzoneAssignmentInput {
    /// Dry-air carrier mass in kilograms.
    pub carrier_dry_air_mass_kg: f64,
    /// Particle height above mean sea level in metres.
    pub height_asl_m: f64,
    /// Particle latitude in degrees north.
    pub latitude_degrees: f64,
    /// Potential vorticity in PVU at the assigned location.
    pub potential_vorticity_pvu: f64,
}

/// Ozone mass plus audit metadata returned by a named rule.
#[derive(Clone, Debug, PartialEq)]
pub struct OzoneAssignment {
    /// Assigned ozone mass in kilograms; zero is a valid outside-mask result.
    pub ozone_mass_kg: f64,
    /// Source/derived/estimated scientific quality.
    pub quality: FieldQuality,
    /// Stable compact provenance description.
    pub provenance: String,
}

/// Extensible named ozone-mass assignment rule.
pub trait OzoneAssignmentRule: Send + Sync {
    /// Returns the stable rule identifier.
    fn model_id(&self) -> &'static str;

    /// Returns exact canonical meteorological field dependencies.
    fn required_fields(&self) -> &'static [CanonicalField];

    /// Assigns ozone mass without mutating carrier mass.
    fn assign(&self, input: OzoneAssignmentInput) -> Result<OzoneAssignment, PopulationError>;
}

/// Frozen empirical FLEXPART PV60 ozone proxy.
#[derive(Clone, Copy, Debug, Default)]
pub struct FlexpartPv60OzoneRule;

impl OzoneAssignmentRule for FlexpartPv60OzoneRule {
    fn model_id(&self) -> &'static str {
        crate::science::FLEXPART_PV60_OZONE_ID
    }

    fn required_fields(&self) -> &'static [CanonicalField] {
        const FIELDS: [CanonicalField; 1] = [CanonicalField::PotentialVorticity];
        &FIELDS
    }

    fn assign(&self, input: OzoneAssignmentInput) -> Result<OzoneAssignment, PopulationError> {
        let ozone_mass_kg = flexpart_pv60_ozone_mass_kg(
            input.carrier_dry_air_mass_kg,
            input.height_asl_m,
            input.latitude_degrees,
            input.potential_vorticity_pvu,
        )
        .map_err(PopulationError::OzoneReference)?
        .unwrap_or(0.0);
        Ok(OzoneAssignment {
            ozone_mass_kg,
            quality: FieldQuality::Derived,
            provenance: crate::science::FLEXPART_PV60_OZONE_ID.into(),
        })
    }
}

/// Population lifecycle or mass-accounting failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PopulationError {
    /// Population algorithm has not been implemented yet.
    NotImplemented,
    /// Configuration does not define a usable target mass or count.
    InvalidConfiguration,
    /// Required domain-fill meteorology is unavailable.
    MissingMeteorology,
    /// Mass conservation exceeds the declared tolerance.
    MassImbalance,
    /// Particle batch is inconsistent.
    InvalidParticleBatch,
    /// A named ozone rule received invalid physical input.
    OzoneReference(ReferenceError),
    /// Deterministic release scheduling, sampling, or allocation failed.
    Release(ReleaseError),
    /// Physical timestamp arithmetic overflowed.
    TimeOverflow,
    /// Particle birth metadata cannot fit the configured runtime resources.
    ResourceLimit,
}

impl PopulationError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented => "population.not_implemented",
            Self::InvalidConfiguration => "population.invalid_configuration",
            Self::MissingMeteorology => "population.missing_meteorology",
            Self::MassImbalance => "population.mass_imbalance",
            Self::InvalidParticleBatch => "population.invalid_particle_batch",
            Self::OzoneReference(_) => "population.ozone_reference",
            Self::Release(_) => "population.release",
            Self::TimeOverflow => "population.time_overflow",
            Self::ResourceLimit => "population.resource_limit",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use trajecta_case::model::population::{
        GeoJsonGeometry, GeoJsonSource, PopulationId, ReleaseEventId, ReleaseEventSpec,
    };
    use trajecta_case::model::substance::SubstanceId;
    use trajecta_case::quantity::{Dimension, Length, Mass, Quantity, Unit};
    use trajecta_met::field::FieldRegistry;
    use trajecta_met::io::inventory::MetCatalog;
    use trajecta_met::profile::document::ProfileCatalog;
    use trajecta_met::query::cache::MemoryBudget;
    use trajecta_met::query::engine::{MetEngineConfig, RayonExecutionContext};
    use trajecta_met::surface_layer::SurfaceLayerRegistry;

    use super::*;

    struct OrdinalGeometry;

    impl GeometrySampler for OrdinalGeometry {
        fn sample_horizontal(
            &self,
            request: ReleaseSamplingRequest<'_>,
        ) -> Result<Vec<(f64, f64)>, ReleaseError> {
            request.validate()?;
            Ok((0..request.count)
                .map(|offset| {
                    let ordinal = request.first_ordinal + offset as u64;
                    (ordinal as f64, -(ordinal as f64))
                })
                .collect())
        }
    }

    struct OrdinalVertical;

    impl VerticalSampler for OrdinalVertical {
        fn sample_vertical(
            &self,
            request: ReleaseSamplingRequest<'_>,
        ) -> Result<Vec<f64>, ReleaseError> {
            request.validate()?;
            Ok((0..request.count)
                .map(|offset| 100.0 + (request.first_ordinal + offset as u64) as f64)
                .collect())
        }
    }

    fn unit(symbol: &str, dimension: Dimension) -> Unit {
        Unit::new(symbol, dimension, 1.0, 0.0).unwrap()
    }

    fn release_fixture(
        start: Timestamp,
        end: Timestamp,
        particle_count: u64,
    ) -> (ReleaseDrivenSpec, ReleaseSchedule, ReleaseEvent) {
        let id = ReleaseEventId("event".into());
        let vertical = ReleaseVerticalSpec::AboveSeaLevel {
            lower: Quantity::<Length>::from_si(100.0, unit("m", Dimension::Length)).unwrap(),
            upper: None,
        };
        let specification = ReleaseDrivenSpec {
            id: PopulationId("population".into()),
            events: vec![ReleaseEventSpec {
                id: id.clone(),
                start,
                end,
                particle_count,
                mass: BTreeMap::from([(
                    SubstanceId("tracer".into()),
                    Quantity::<Mass>::from_si(1.0, unit("kg", Dimension::Mass)).unwrap(),
                )]),
                geometry: GeoJsonSource::Inline {
                    geometry: GeoJsonGeometry::Point([0.0, 0.0]),
                },
                vertical: vertical.clone(),
            }],
        };
        let event = ReleaseEvent {
            id,
            start,
            end,
            geometry: GeoJsonGeometry::Point([0.0, 0.0]),
            vertical,
            particle_count,
            mass_kg: BTreeMap::from([(SubstanceId("tracer".into()), 1.0)]),
        };
        (
            specification,
            ReleaseSchedule {
                events: vec![event.clone()],
            },
            event,
        )
    }

    fn population(
        start: Timestamp,
        end: Timestamp,
        particle_count: u64,
    ) -> (ReleaseDrivenPopulation, ReleaseEvent) {
        let (specification, schedule, event) = release_fixture(start, end, particle_count);
        (
            ReleaseDrivenPopulation::new(
                specification,
                schedule,
                Box::new(OrdinalGeometry),
                Box::new(OrdinalVertical),
                Box::new(DirectAslReleaseResolver),
            ),
            event,
        )
    }

    fn engine() -> MetEngine {
        MetEngine::new(MetEngineConfig {
            catalog: MetCatalog::default(),
            profiles: ProfileCatalog::default(),
            fields: FieldRegistry::canonical().unwrap(),
            surface_layers: SurfaceLayerRegistry::default(),
            memory_budget: MemoryBudget::new(1024, 0).unwrap(),
        })
    }

    #[test]
    fn instantaneous_release_emits_once_with_exact_count_mass_and_ids() {
        let (mut population, _) = population(Timestamp::UNIX_EPOCH, Timestamp::UNIX_EPOCH, 3);
        let mut meteorology = engine();
        let execution = RayonExecutionContext { worker_threads: 1 };
        let mut particles = ParticleBatch::default();
        let mut context = PopulationContext {
            time: Timestamp::UNIX_EPOCH,
            direction: Direction::Forward,
            step: None,
            step_index: None,
            random_seed: 42,
            meteorology: &mut meteorology,
            execution: &execution,
            domain: None,
        };
        population.initialize(&mut context, &mut particles).unwrap();
        let emitted = population.emit_particles(&mut context).unwrap();
        assert_eq!(emitted.len(), Ok(3));
        assert_eq!(
            emitted.mass.mass_kg[&SubstanceId("tracer".into())]
                .iter()
                .sum::<f64>(),
            1.0
        );
        assert_eq!(
            population.emit_particles(&mut context).unwrap().len(),
            Ok(0)
        );
        assert_eq!(
            population.snapshot_state(),
            PopulationState::ReleaseDriven {
                emitted_birth_count: 3
            }
        );
    }

    #[test]
    fn continuous_birth_boundaries_are_exact_and_direction_ordered() {
        let (mut forward, event) =
            population(Timestamp::UNIX_EPOCH, Timestamp::new(9, 0).unwrap(), 3);
        let mut meteorology = engine();
        let execution = RayonExecutionContext { worker_threads: 1 };
        let mut particles = ParticleBatch::default();
        let mut context = PopulationContext {
            time: Timestamp::new(-1, 0).unwrap(),
            direction: Direction::Forward,
            step: None,
            step_index: None,
            random_seed: 7,
            meteorology: &mut meteorology,
            execution: &execution,
            domain: None,
        };
        forward.initialize(&mut context, &mut particles).unwrap();
        let expected = (0..3)
            .map(|ordinal| {
                release_birth_time(&forward.specification.id, &event, 7, ordinal).unwrap()
            })
            .collect::<Vec<_>>();
        let boundaries = forward
            .step_boundaries(
                SimulationClock {
                    current: Timestamp::new(-1, 0).unwrap(),
                    direction: Direction::Forward,
                },
                SignedDuration(11_000_000_000),
            )
            .unwrap();
        assert_eq!(
            boundaries
                .iter()
                .map(StepBoundary::time)
                .collect::<Vec<_>>(),
            expected
        );

        let (mut backward, _) = population(Timestamp::UNIX_EPOCH, Timestamp::new(9, 0).unwrap(), 3);
        let mut backward_meteorology = engine();
        let mut backward_context = PopulationContext {
            time: Timestamp::new(10, 0).unwrap(),
            direction: Direction::Backward,
            step: None,
            step_index: None,
            random_seed: 7,
            meteorology: &mut backward_meteorology,
            execution: &execution,
            domain: None,
        };
        backward
            .initialize(&mut backward_context, &mut ParticleBatch::default())
            .unwrap();
        let backward_boundaries = backward
            .step_boundaries(
                SimulationClock {
                    current: Timestamp::new(10, 0).unwrap(),
                    direction: Direction::Backward,
                },
                SignedDuration(-11_000_000_000),
            )
            .unwrap();
        assert_eq!(
            backward_boundaries
                .iter()
                .map(StepBoundary::time)
                .collect::<Vec<_>>(),
            expected.into_iter().rev().collect::<Vec<_>>()
        );
    }
}
