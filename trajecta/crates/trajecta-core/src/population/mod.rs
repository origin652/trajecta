//! # Contract: full particle-population lifecycle
//!
//! A population strategy owns initialization, pre-step work, emission,
//! post-advection accounting, boundary maintenance, and finalization. Domain
//! filling preserves residual boundary mass across steps and chooses inflow
//! according to integration direction.

mod air_mass;
mod vertical_resolve;
pub use air_mass::{
    AirMassPopulationError, DomainFillMassLedger, InitialAirMassSeeding, InitialOzoneSeeding,
    MassLedgerInput, active_carrier_mass_kg, residual_mass_kg, seed_initial_air_mass,
    seed_initial_stratospheric_ozone,
};
pub use vertical_resolve::MetReleaseVerticalResolver;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, ReleaseDrivenSpec, ReleaseVerticalSpec,
};
use trajecta_case::model::time::{Direction, Timestamp};
use trajecta_met::auxiliary::gmted2010::Gmted2010;
use trajecta_met::derive::domain_fill::AirMassDeriver;
use trajecta_met::field::{CanonicalField, FieldQuality};
use trajecta_met::query::engine::{ExecutionContext, MetEngine};

use crate::clock::{SignedDuration, SimulationClock, StepBoundary, add_timestamp};
use crate::manifest::MassLedgerRecord;
use crate::particle::{ParticleBatch, ParticleId, ParticleStatus, TerminationReason};
use crate::reference::{
    ReferenceError, flexpart_pv60_ozone_mass_kg, mass_balance_tolerance_kg, neumaier_sum,
};
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
        /// Initialization mass below one complete carrier.
        initial_residual_mass_kg: f64,
        /// Equal carrier mass represented by every domain-fill particle.
        carrier_mass_per_particle_kg: f64,
        /// Number of particles seeded so far.
        seeded_particles: u64,
        /// Adjudicated per-step mass records.
        mass_ledger: Vec<MassLedgerRecord>,
    },
}

/// Residual dry-air mass by deterministic boundary-face identifier.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundaryMassAccumulator {
    /// Residual kilograms below one particle threshold.
    pub residual_mass_kg: BTreeMap<u64, f64>,
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

    /// Plans meteorology-dependent dynamic boundaries inside one already
    /// statically capped step. Implementations must be idempotent for repeated
    /// calls at the same physical time.
    fn dynamic_step_boundaries(
        &mut self,
        _context: &mut PopulationContext<'_>,
        _requested: SignedDuration,
    ) -> Result<Vec<StepBoundary>, PopulationError> {
        Ok(Vec::new())
    }

    /// Returns manifest-ready domain-fill mass records, if any.
    fn mass_ledger_records(&self) -> Vec<MassLedgerRecord> {
        Vec::new()
    }

    /// Plans and materializes every exact birth inside one runner macro step.
    ///
    /// The returned rows retain their individual `birth_time`; the runner
    /// appends them once and advances each row only from that local time. The
    /// default implementation preserves release/custom-population behavior by
    /// collecting existing population boundaries without exposing them to the
    /// global [`crate::clock::StepPlanner`]. Domain-fill overrides this method
    /// to aggregate its mass accounting over the complete macro step.
    fn prepare_cohort_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<ParticleBatch, PopulationError> {
        let step = context.step.ok_or(PopulationError::InvalidConfiguration)?;
        self.before_step(context, particles)?;
        let mut times = self
            .step_boundaries(
                SimulationClock {
                    current: context.time,
                    direction: context.direction,
                },
                step,
            )?
            .into_iter()
            .map(|boundary| boundary.time())
            .collect::<BTreeSet<_>>();
        times.extend(
            self.dynamic_step_boundaries(context, step)?
                .into_iter()
                .map(|boundary| boundary.time()),
        );
        let mut times = times.into_iter().collect::<Vec<_>>();
        if context.direction == Direction::Backward {
            times.reverse();
        }
        let original_time = context.time;
        let emitted = (|| {
            let mut emitted = ParticleBatch::default();
            for time in times {
                context.time = time;
                emitted
                    .append(self.emit_particles(context)?)
                    .map_err(|_| PopulationError::InvalidParticleBatch)?;
            }
            Ok(emitted)
        })();
        context.time = original_time;
        emitted
    }

    /// Completes population accounting after every cohort has reached the
    /// common macro-step endpoint.
    fn complete_cohort_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        self.after_advection(context, particles)?;
        self.apply_boundary_maintenance(context, particles)
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
    ledger: DomainFillMassLedger,
    carrier_mass_per_particle_kg: Option<f64>,
    initial_residual_mass_kg: f64,
    residual_mass: BoundaryMassAccumulator,
    finite_domain: bool,
    active_inflow_plan: Option<air_mass::BoundaryInflowPlan>,
    next_boundary_lifecycle_event_index: u64,
    opening_step: Option<DomainFillStepOpening>,
    pending_emissions: ParticleBatch,
    gmted2010: Option<Arc<Gmted2010>>,
}

#[derive(Clone, Debug)]
struct DomainFillStepOpening {
    step_index: u64,
    end_time: Timestamp,
    opening_active_kg: f64,
    opening_residual_kg: f64,
    incoming_by_face_kg: BTreeMap<u64, f64>,
    incoming_kg: f64,
    particle_status: BTreeMap<ParticleId, ParticleStatus>,
    cohort_birth_ids: BTreeSet<ParticleId>,
    cohort_birth_count: u64,
    cohort_residual_applied: bool,
}

impl DomainFillAirMass {
    /// Constructs an uninitialized dry-air domain-fill lifecycle.
    #[must_use]
    pub fn new(specification: DomainFillAirMassSpec, gmted2010: Option<Arc<Gmted2010>>) -> Self {
        Self {
            specification,
            state: PopulationState::Uninitialized,
            ledger: DomainFillMassLedger::default(),
            carrier_mass_per_particle_kg: None,
            initial_residual_mass_kg: 0.0,
            residual_mass: BoundaryMassAccumulator::default(),
            finite_domain: false,
            active_inflow_plan: None,
            next_boundary_lifecycle_event_index: 0,
            opening_step: None,
            pending_emissions: ParticleBatch::default(),
            gmted2010,
        }
    }

    fn sync_state(&mut self, seeded_particles: u64) {
        self.state = PopulationState::DomainFill {
            residual_mass: self.residual_mass.clone(),
            initial_residual_mass_kg: self.initial_residual_mass_kg,
            carrier_mass_per_particle_kg: self.carrier_mass_per_particle_kg.unwrap_or(0.0),
            seeded_particles,
            mass_ledger: self.ledger.records().to_vec(),
        };
    }

    fn seeded_particles(&self) -> Result<u64, PopulationError> {
        match &self.state {
            PopulationState::DomainFill {
                seeded_particles, ..
            } => Ok(*seeded_particles),
            PopulationState::Uninitialized | PopulationState::ReleaseDriven { .. } => {
                Err(PopulationError::InvalidConfiguration)
            }
        }
    }
}

/// Stratospheric-ozone domain-filling population.
pub struct DomainFillStratosphericOzone {
    /// Portable Case configuration.
    pub specification: DomainFillStratosphericOzoneSpec,
    /// Persistent runtime state.
    pub state: PopulationState,
    inner: DomainFillAirMass,
    rule: Arc<dyn OzoneAssignmentRule>,
}

impl DomainFillStratosphericOzone {
    /// Constructs an uninitialized ozone lifecycle with one resolved rule.
    pub fn new(
        specification: DomainFillStratosphericOzoneSpec,
        rule: Arc<dyn OzoneAssignmentRule>,
        gmted2010: Option<Arc<Gmted2010>>,
    ) -> Result<Self, PopulationError> {
        if specification.ozone_rule != rule.model_id()
            || specification.ozone_substance.0.trim().is_empty()
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        let inner = DomainFillAirMass::new(specification.air_mass.clone(), gmted2010);
        Ok(Self {
            specification,
            state: PopulationState::Uninitialized,
            inner,
            rule,
        })
    }

    fn sync_state(&mut self) {
        self.state = self.inner.snapshot_state();
    }
}

impl PopulationStrategy for DomainFillStratosphericOzone {
    fn model_id(&self) -> &'static str {
        crate::science::OZONE_DOMAIN_FILL_ID
    }

    fn snapshot_state(&self) -> PopulationState {
        self.state.clone()
    }

    fn dynamic_step_boundaries(
        &mut self,
        context: &mut PopulationContext<'_>,
        requested: SignedDuration,
    ) -> Result<Vec<StepBoundary>, PopulationError> {
        if !matches!(self.state, PopulationState::DomainFill { .. })
            || context.step != Some(requested)
            || context.step_index.is_none()
            || requested.0 == 0
            || context.domain.as_ref() != Some(&self.specification.air_mass.domain_id)
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        if !self.inner.finite_domain {
            return Ok(Vec::new());
        }
        if self
            .inner
            .active_inflow_plan
            .as_ref()
            .is_some_and(|plan| plan.end_time == context.time)
        {
            self.inner.active_inflow_plan = None;
        }
        if let Some(plan) = &self.inner.active_inflow_plan {
            if !plan
                .contains_interval(context.time, requested)
                .map_err(map_air_mass_error)?
            {
                return Err(PopulationError::InvalidConfiguration);
            }
            return Ok(plan
                .remaining_birth_times(context.time)
                .into_iter()
                .map(|time| StepBoundary::Population {
                    time,
                    event_id: format!("domain-fill-boundary/{}", plan.lifecycle_event_index),
                })
                .collect());
        }
        let midpoint = add_timestamp(context.time, SignedDuration(requested.0 / 2))
            .map_err(|_| PopulationError::TimeOverflow)?;
        let window = context
            .meteorology
            .prepare_for_domain(midpoint, &self.specification.air_mass.domain_id)
            .map_err(|_| PopulationError::MissingMeteorology)?;
        let snapshot = AirMassDeriver
            .derive_window(&window)
            .map_err(|error| PopulationError::AirMassDerivation(error.code().into()))?;
        let carrier_mass = self
            .inner
            .carrier_mass_per_particle_kg
            .ok_or(PopulationError::InvalidConfiguration)?;
        let plan = air_mass::plan_ozone_boundary_inflow(
            &self.specification,
            &snapshot,
            self.rule.as_ref(),
            carrier_mass,
            &self.inner.residual_mass.residual_mass_kg,
            context.direction,
            context.time,
            requested,
            self.inner.next_boundary_lifecycle_event_index,
            context.random_seed,
            self.inner.gmted2010.as_deref(),
        )?;
        self.inner.next_boundary_lifecycle_event_index = self
            .inner
            .next_boundary_lifecycle_event_index
            .checked_add(1)
            .ok_or(PopulationError::ResourceLimit)?;
        let boundaries = plan
            .remaining_birth_times(context.time)
            .into_iter()
            .map(|time| StepBoundary::Population {
                time,
                event_id: format!("domain-fill-boundary/{}", plan.lifecycle_event_index),
            })
            .collect();
        self.inner.active_inflow_plan = Some(plan);
        Ok(boundaries)
    }

    fn prepare_cohort_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<ParticleBatch, PopulationError> {
        let step = context.step.ok_or(PopulationError::InvalidConfiguration)?;
        let _ = self.dynamic_step_boundaries(context, step)?;
        let emitted = self.inner.prepare_cohort_step(context, particles)?;
        self.sync_state();
        Ok(emitted)
    }

    fn complete_cohort_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        self.inner.complete_cohort_step(context, particles)?;
        self.sync_state();
        Ok(())
    }

    fn mass_ledger_records(&self) -> Vec<MassLedgerRecord> {
        self.inner.mass_ledger_records()
    }

    fn initialize(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        if self.state != PopulationState::Uninitialized
            || self.inner.state != PopulationState::Uninitialized
            || !particles.is_empty()
            || context.step.is_some()
            || context.step_index.is_some()
            || context.domain.as_ref() != Some(&self.specification.air_mass.domain_id)
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        let window = context
            .meteorology
            .prepare_for_domain(context.time, &self.specification.air_mass.domain_id)
            .map_err(|_| PopulationError::MissingMeteorology)?;
        let snapshot = AirMassDeriver
            .derive_window(&window)
            .map_err(|error| PopulationError::AirMassDerivation(error.code().into()))?;
        let seeded = seed_initial_stratospheric_ozone(
            &self.specification,
            &snapshot,
            self.rule.as_ref(),
            context.random_seed,
            self.inner.gmted2010.as_deref(),
        )?;
        let seeded_particles = u64::try_from(
            seeded
                .particles
                .len()
                .map_err(|_| PopulationError::InvalidParticleBatch)?,
        )
        .map_err(|_| PopulationError::ResourceLimit)?;
        self.inner.carrier_mass_per_particle_kg = Some(seeded.carrier_mass_per_particle_kg);
        self.inner.initial_residual_mass_kg = seeded.residual_mass_kg;
        self.inner.finite_domain = !snapshot.boundary_faces.is_empty();
        particles
            .append(seeded.particles)
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        self.inner.sync_state(seeded_particles);
        self.sync_state();
        Ok(())
    }

    fn before_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        self.inner.before_step(context, particles)
    }

    fn emit_particles(
        &mut self,
        context: &mut PopulationContext<'_>,
    ) -> Result<ParticleBatch, PopulationError> {
        self.inner.emit_particles(context)
    }

    fn after_advection(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        self.inner.after_advection(context, particles)
    }

    fn apply_boundary_maintenance(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        self.inner.apply_boundary_maintenance(context, particles)?;
        self.sync_state();
        Ok(())
    }

    fn finalize(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        self.inner.finalize(context, particles)?;
        self.sync_state();
        Ok(())
    }
}

impl PopulationStrategy for DomainFillAirMass {
    fn model_id(&self) -> &'static str {
        crate::science::DRY_AIR_DOMAIN_FILL_ID
    }

    fn snapshot_state(&self) -> PopulationState {
        self.state.clone()
    }

    fn dynamic_step_boundaries(
        &mut self,
        context: &mut PopulationContext<'_>,
        requested: SignedDuration,
    ) -> Result<Vec<StepBoundary>, PopulationError> {
        if !matches!(self.state, PopulationState::DomainFill { .. })
            || context.step != Some(requested)
            || context.step_index.is_none()
            || requested.0 == 0
            || context.domain.as_ref() != Some(&self.specification.domain_id)
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        if !self.finite_domain {
            return Ok(Vec::new());
        }
        if self
            .active_inflow_plan
            .as_ref()
            .is_some_and(|plan| plan.end_time == context.time)
        {
            self.active_inflow_plan = None;
        }
        if let Some(plan) = &self.active_inflow_plan {
            if !plan
                .contains_interval(context.time, requested)
                .map_err(map_air_mass_error)?
            {
                return Err(PopulationError::InvalidConfiguration);
            }
            return Ok(plan
                .remaining_birth_times(context.time)
                .into_iter()
                .map(|time| StepBoundary::Population {
                    time,
                    event_id: format!("domain-fill-boundary/{}", plan.lifecycle_event_index),
                })
                .collect());
        }

        let midpoint = add_timestamp(context.time, SignedDuration(requested.0 / 2))
            .map_err(|_| PopulationError::TimeOverflow)?;
        let window = context
            .meteorology
            .prepare_for_domain(midpoint, &self.specification.domain_id)
            .map_err(|_| PopulationError::MissingMeteorology)?;
        let snapshot = AirMassDeriver
            .derive_window(&window)
            .map_err(|error| PopulationError::AirMassDerivation(error.code().into()))?;
        let carrier_mass = self
            .carrier_mass_per_particle_kg
            .ok_or(PopulationError::InvalidConfiguration)?;
        let plan = air_mass::plan_boundary_inflow(
            &self.specification,
            &snapshot,
            carrier_mass,
            &self.residual_mass.residual_mass_kg,
            context.direction,
            context.time,
            requested,
            self.next_boundary_lifecycle_event_index,
            context.random_seed,
            self.gmted2010.as_deref(),
        )
        .map_err(map_air_mass_error)?;
        self.next_boundary_lifecycle_event_index = self
            .next_boundary_lifecycle_event_index
            .checked_add(1)
            .ok_or(PopulationError::ResourceLimit)?;
        let boundaries = plan
            .remaining_birth_times(context.time)
            .into_iter()
            .map(|time| StepBoundary::Population {
                time,
                event_id: format!("domain-fill-boundary/{}", plan.lifecycle_event_index),
            })
            .collect();
        self.active_inflow_plan = Some(plan);
        Ok(boundaries)
    }

    fn prepare_cohort_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<ParticleBatch, PopulationError> {
        let step = context.step.ok_or(PopulationError::InvalidConfiguration)?;
        if !self.pending_emissions.is_empty() {
            return Err(PopulationError::InvalidConfiguration);
        }
        let _ = self.dynamic_step_boundaries(context, step)?;
        self.before_step(context, particles)?;

        let births = self
            .active_inflow_plan
            .as_ref()
            .map(|plan| plan.births.clone())
            .unwrap_or_default();
        let incoming_by_face = self
            .opening_step
            .as_ref()
            .ok_or(PopulationError::InvalidConfiguration)?
            .incoming_by_face_kg
            .clone();
        for (face_id, incoming) in incoming_by_face {
            let residual = self
                .residual_mass
                .residual_mass_kg
                .entry(face_id)
                .or_insert(0.0);
            *residual += incoming;
            if !residual.is_finite() || *residual < 0.0 {
                return Err(PopulationError::InvalidConfiguration);
            }
        }

        let carrier_mass = self
            .carrier_mass_per_particle_kg
            .ok_or(PopulationError::InvalidConfiguration)?;
        let tolerance = mass_balance_tolerance_kg(carrier_mass, false)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        for birth in &births {
            let residual = self
                .residual_mass
                .residual_mass_kg
                .get_mut(&birth.face_id)
                .ok_or(PopulationError::InvalidConfiguration)?;
            *residual -= carrier_mass;
            if *residual < -tolerance {
                return Err(PopulationError::MassImbalance);
            }
            if *residual < 0.0 {
                *residual = 0.0;
            }
        }
        if self
            .residual_mass
            .residual_mass_kg
            .values()
            .any(|residual| {
                !residual.is_finite() || *residual < 0.0 || *residual >= carrier_mass + tolerance
            })
        {
            return Err(PopulationError::MassImbalance);
        }

        let cohort_birth_ids = births
            .iter()
            .map(|birth| birth.particle.id)
            .collect::<BTreeSet<_>>();
        if cohort_birth_ids.len() != births.len() {
            return Err(PopulationError::InvalidParticleBatch);
        }
        let cohort_birth_count =
            u64::try_from(births.len()).map_err(|_| PopulationError::ResourceLimit)?;
        let opening = self
            .opening_step
            .as_mut()
            .ok_or(PopulationError::InvalidConfiguration)?;
        opening.cohort_birth_ids = cohort_birth_ids;
        opening.cohort_birth_count = cohort_birth_count;
        opening.cohort_residual_applied = true;

        air_mass::particle_batch_from_states(
            births.into_iter().map(|birth| birth.particle).collect(),
        )
        .map_err(map_air_mass_error)
    }

    fn complete_cohort_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        self.after_advection(context, particles)?;
        let opening = self
            .opening_step
            .take()
            .ok_or(PopulationError::InvalidConfiguration)?;
        if !opening.cohort_residual_applied
            || opening.end_time != context.time
            || context.step_index != Some(opening.step_index)
            || !self.pending_emissions.is_empty()
        {
            return Err(PopulationError::InvalidConfiguration);
        }

        let mut outgoing = Vec::new();
        let mut normal_terminated = Vec::new();
        let mut abnormal_terminated = Vec::new();
        let len = particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        for index in 0..len {
            if particles.population_id[index] != self.specification.id {
                continue;
            }
            let opening_alive =
                opening.particle_status.get(&particles.id[index]) == Some(&ParticleStatus::Alive);
            if !opening_alive && !opening.cohort_birth_ids.contains(&particles.id[index]) {
                continue;
            }
            if matches!(
                particles.status[index],
                ParticleStatus::Terminated {
                    reason: TerminationReason::OutsideDomain
                }
            ) {
                particles.status[index] = ParticleStatus::Terminated {
                    reason: TerminationReason::PopulationOutflow,
                };
            }
            let ParticleStatus::Terminated { reason } = &particles.status[index] else {
                continue;
            };
            match reason {
                TerminationReason::PopulationOutflow => {
                    outgoing.push(particles.dry_air_mass_kg[index]);
                }
                reason if reason.class() == crate::particle::TerminationClass::Normal => {
                    normal_terminated.push(particles.dry_air_mass_kg[index]);
                }
                _ => abnormal_terminated.push(particles.dry_air_mass_kg[index]),
            }
        }
        particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;

        let outgoing_kg = neumaier_sum(&outgoing)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let normal_terminated_kg = neumaier_sum(&normal_terminated)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let abnormal_terminated_kg = neumaier_sum(&abnormal_terminated)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let closing_active_kg = active_carrier_mass_kg(particles, &self.specification.id)
            .map_err(map_air_mass_error)?;
        let closing_residual_kg = residual_mass_kg(
            self.initial_residual_mass_kg,
            &self.residual_mass.residual_mass_kg,
        )
        .map_err(map_air_mass_error)?;
        self.ledger
            .record_step(MassLedgerInput {
                step_index: opening.step_index,
                time: context.time,
                opening_active_kg: opening.opening_active_kg,
                opening_residual_kg: opening.opening_residual_kg,
                incoming_kg: opening.incoming_kg,
                outgoing_kg,
                normal_terminated_kg,
                abnormal_terminated_kg,
                closing_active_kg,
                closing_residual_kg,
            })
            .map_err(map_air_mass_error)?;
        let seeded_particles = self
            .seeded_particles()?
            .checked_add(opening.cohort_birth_count)
            .ok_or(PopulationError::ResourceLimit)?;
        if self
            .active_inflow_plan
            .as_ref()
            .is_some_and(|plan| plan.end_time == context.time)
        {
            self.active_inflow_plan = None;
        }
        self.sync_state(seeded_particles);
        Ok(())
    }

    fn mass_ledger_records(&self) -> Vec<MassLedgerRecord> {
        self.ledger.records().to_vec()
    }

    fn initialize(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        if self.state != PopulationState::Uninitialized
            || !particles.is_empty()
            || context.step.is_some()
            || context.step_index.is_some()
            || context.domain.as_ref() != Some(&self.specification.domain_id)
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        let window = context
            .meteorology
            .prepare_for_domain(context.time, &self.specification.domain_id)
            .map_err(|_| PopulationError::MissingMeteorology)?;
        let snapshot = AirMassDeriver
            .derive_window(&window)
            .map_err(|error| PopulationError::AirMassDerivation(error.code().into()))?;
        let seeded = seed_initial_air_mass(
            &self.specification,
            &snapshot,
            context.random_seed,
            self.gmted2010.as_deref(),
        )
        .map_err(map_air_mass_error)?;
        let seeded_particles = u64::try_from(
            seeded
                .particles
                .len()
                .map_err(|_| PopulationError::InvalidParticleBatch)?,
        )
        .map_err(|_| PopulationError::ResourceLimit)?;
        self.carrier_mass_per_particle_kg = Some(seeded.carrier_mass_per_particle_kg);
        self.initial_residual_mass_kg = seeded.residual_mass_kg;
        self.finite_domain = !snapshot.boundary_faces.is_empty();
        particles
            .append(seeded.particles)
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        self.sync_state(seeded_particles);
        Ok(())
    }

    fn before_step(
        &mut self,
        context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        let step = context.step.ok_or(PopulationError::InvalidConfiguration)?;
        let step_index = context
            .step_index
            .ok_or(PopulationError::InvalidConfiguration)?;
        if self.opening_step.is_some() || !self.pending_emissions.is_empty() {
            return Err(PopulationError::InvalidConfiguration);
        }
        let end_time =
            add_timestamp(context.time, step).map_err(|_| PopulationError::TimeOverflow)?;
        let incoming_by_face_kg = if self.finite_domain {
            let plan = self
                .active_inflow_plan
                .as_ref()
                .ok_or(PopulationError::InvalidConfiguration)?;
            if !plan
                .contains_interval(context.time, step)
                .map_err(map_air_mass_error)?
            {
                return Err(PopulationError::InvalidConfiguration);
            }
            plan.incoming_by_face(step).map_err(map_air_mass_error)?
        } else {
            BTreeMap::new()
        };
        let incoming_values = incoming_by_face_kg.values().copied().collect::<Vec<_>>();
        let incoming_kg = neumaier_sum(&incoming_values)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let opening_active_kg = active_carrier_mass_kg(particles, &self.specification.id)
            .map_err(map_air_mass_error)?;
        let opening_residual_kg = residual_mass_kg(
            self.initial_residual_mass_kg,
            &self.residual_mass.residual_mass_kg,
        )
        .map_err(map_air_mass_error)?;
        let len = particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        let particle_status = (0..len)
            .filter(|index| particles.population_id[*index] == self.specification.id)
            .map(|index| (particles.id[index], particles.status[index].clone()))
            .collect();
        self.opening_step = Some(DomainFillStepOpening {
            step_index,
            end_time,
            opening_active_kg,
            opening_residual_kg,
            incoming_by_face_kg,
            incoming_kg,
            particle_status,
            cohort_birth_ids: BTreeSet::new(),
            cohort_birth_count: 0,
            cohort_residual_applied: false,
        });
        Ok(())
    }

    fn emit_particles(
        &mut self,
        context: &mut PopulationContext<'_>,
    ) -> Result<ParticleBatch, PopulationError> {
        let emitted = std::mem::take(&mut self.pending_emissions);
        if emitted
            .birth_time
            .iter()
            .any(|birth_time| *birth_time != context.time)
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        Ok(emitted)
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
        context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        let opening = self
            .opening_step
            .take()
            .ok_or(PopulationError::InvalidConfiguration)?;
        if opening.end_time != context.time || context.step_index != Some(opening.step_index) {
            return Err(PopulationError::InvalidConfiguration);
        }
        let mut outgoing = Vec::new();
        let mut normal_terminated = Vec::new();
        let mut abnormal_terminated = Vec::new();
        let len = particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        for index in 0..len {
            if particles.population_id[index] != self.specification.id
                || opening.particle_status.get(&particles.id[index]) != Some(&ParticleStatus::Alive)
            {
                continue;
            }
            if matches!(
                particles.status[index],
                ParticleStatus::Terminated {
                    reason: TerminationReason::OutsideDomain
                }
            ) {
                particles.status[index] = ParticleStatus::Terminated {
                    reason: TerminationReason::PopulationOutflow,
                };
            }
            let ParticleStatus::Terminated { reason } = &particles.status[index] else {
                continue;
            };
            match reason {
                TerminationReason::PopulationOutflow => {
                    outgoing.push(particles.dry_air_mass_kg[index]);
                }
                reason if reason.class() == crate::particle::TerminationClass::Normal => {
                    normal_terminated.push(particles.dry_air_mass_kg[index]);
                }
                _ => abnormal_terminated.push(particles.dry_air_mass_kg[index]),
            }
        }
        particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;

        for (face_id, incoming) in &opening.incoming_by_face_kg {
            let residual = self
                .residual_mass
                .residual_mass_kg
                .entry(*face_id)
                .or_insert(0.0);
            *residual += incoming;
            if !residual.is_finite() || *residual < 0.0 {
                return Err(PopulationError::InvalidConfiguration);
            }
        }
        let births = self
            .active_inflow_plan
            .as_ref()
            .map(|plan| plan.births_at(context.time))
            .unwrap_or_default();
        let carrier_mass = self
            .carrier_mass_per_particle_kg
            .ok_or(PopulationError::InvalidConfiguration)?;
        for birth in &births {
            let residual = self
                .residual_mass
                .residual_mass_kg
                .get_mut(&birth.face_id)
                .ok_or(PopulationError::InvalidConfiguration)?;
            *residual -= carrier_mass;
            let tolerance = mass_balance_tolerance_kg(carrier_mass, false)
                .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
            if *residual < -tolerance {
                return Err(PopulationError::MassImbalance);
            }
            if *residual < 0.0 {
                *residual = 0.0;
            }
        }
        for residual in self.residual_mass.residual_mass_kg.values() {
            let tolerance = mass_balance_tolerance_kg(carrier_mass, false)
                .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
            if !residual.is_finite() || *residual < 0.0 || *residual >= carrier_mass + tolerance {
                return Err(PopulationError::MassImbalance);
            }
        }
        self.pending_emissions = air_mass::particle_batch_from_states(
            births.into_iter().map(|birth| birth.particle).collect(),
        )
        .map_err(map_air_mass_error)?;

        let outgoing_kg = neumaier_sum(&outgoing)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let normal_terminated_kg = neumaier_sum(&normal_terminated)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let abnormal_terminated_kg = neumaier_sum(&abnormal_terminated)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let existing_active_kg = active_carrier_mass_kg(particles, &self.specification.id)
            .map_err(map_air_mass_error)?;
        let pending_active_kg = neumaier_sum(&self.pending_emissions.dry_air_mass_kg)
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let closing_active_kg = neumaier_sum(&[existing_active_kg, pending_active_kg])
            .map_err(|error| PopulationError::AirMassReference(error.code().into()))?;
        let closing_residual_kg = residual_mass_kg(
            self.initial_residual_mass_kg,
            &self.residual_mass.residual_mass_kg,
        )
        .map_err(map_air_mass_error)?;
        self.ledger
            .record_step(MassLedgerInput {
                step_index: opening.step_index,
                time: context.time,
                opening_active_kg: opening.opening_active_kg,
                opening_residual_kg: opening.opening_residual_kg,
                incoming_kg: opening.incoming_kg,
                outgoing_kg,
                normal_terminated_kg,
                abnormal_terminated_kg,
                closing_active_kg,
                closing_residual_kg,
            })
            .map_err(map_air_mass_error)?;
        let emitted_count = u64::try_from(
            self.pending_emissions
                .len()
                .map_err(|_| PopulationError::InvalidParticleBatch)?,
        )
        .map_err(|_| PopulationError::ResourceLimit)?;
        let seeded_particles = self
            .seeded_particles()?
            .checked_add(emitted_count)
            .ok_or(PopulationError::ResourceLimit)?;
        if self
            .active_inflow_plan
            .as_ref()
            .is_some_and(|plan| plan.end_time == context.time)
        {
            self.active_inflow_plan = None;
        }
        self.sync_state(seeded_particles);
        Ok(())
    }

    fn finalize(
        &mut self,
        _context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        if self.opening_step.is_some()
            || self.active_inflow_plan.is_some()
            || !self.pending_emissions.is_empty()
        {
            return Err(PopulationError::InvalidConfiguration);
        }
        particles
            .validate()
            .map_err(|_| PopulationError::InvalidParticleBatch)?;
        self.ledger.finalize().map_err(map_air_mass_error)?;
        let carrier_mass = self
            .carrier_mass_per_particle_kg
            .ok_or(PopulationError::InvalidConfiguration)?;
        if self.initial_residual_mass_kg >= carrier_mass {
            return Err(PopulationError::MassImbalance);
        }
        let seeded_particles = self.seeded_particles()?;
        self.sync_state(seeded_particles);
        Ok(())
    }
}

fn map_air_mass_error(error: AirMassPopulationError) -> PopulationError {
    match error {
        AirMassPopulationError::MassImbalance => PopulationError::MassImbalance,
        AirMassPopulationError::ResourceLimit => PopulationError::ResourceLimit,
        AirMassPopulationError::TimeOverflow => PopulationError::TimeOverflow,
        other => PopulationError::AirMass(other),
    }
}

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
        if matches!(
            self.state,
            PopulationState::ReleaseDriven {
                emitted_birth_count
            } if emitted_birth_count == self.births.len()
        ) {
            return Ok(Vec::new());
        }
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
        validate_release_batch_shape(particles)
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
            let birth_times = pending[cursor..end]
                .iter()
                .map(|index| self.births[*index].time)
                .collect::<Vec<_>>();
            let height_asl_m = self.vertical_resolver.resolve_asl(
                event,
                &birth_times,
                &horizontal,
                &sampled_vertical,
                context,
            )?;
            let allocated = ReleaseAllocator::allocate(ReleaseAllocationRequest {
                population_id: &self.specification.id,
                event,
                seed: context.random_seed,
                first_ordinal: first.ordinal,
                horizontal: &horizontal,
                height_asl_m: &height_asl_m,
            })
            .map_err(PopulationError::Release)?;
            if emitted_batch.is_empty() {
                emitted_batch = allocated;
            } else {
                emitted_batch
                    .append(allocated)
                    .map_err(|_| PopulationError::InvalidParticleBatch)?;
            }
            cursor = end;
        }
        let emitted_birth_count = match self.state {
            PopulationState::ReleaseDriven {
                emitted_birth_count,
            } => emitted_birth_count
                .checked_add(pending.len())
                .ok_or(PopulationError::ResourceLimit)?,
            _ => return Err(PopulationError::InvalidConfiguration),
        };
        for index in pending {
            self.emitted[index] = true;
        }
        self.state = PopulationState::ReleaseDriven {
            emitted_birth_count,
        };
        Ok(emitted_batch)
    }

    fn after_advection(
        &mut self,
        _context: &mut PopulationContext<'_>,
        particles: &ParticleBatch,
    ) -> Result<(), PopulationError> {
        validate_release_batch_shape(particles)
    }

    fn apply_boundary_maintenance(
        &mut self,
        _context: &mut PopulationContext<'_>,
        particles: &mut ParticleBatch,
    ) -> Result<(), PopulationError> {
        validate_release_batch_shape(particles)
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

fn validate_release_batch_shape(particles: &ParticleBatch) -> Result<(), PopulationError> {
    particles
        .len()
        .map(|_| ())
        .map_err(|_| PopulationError::InvalidParticleBatch)
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
pub trait OzoneAssignmentRule: Send + Sync + 'static {
    /// Returns the stable rule identifier.
    fn model_id(&self) -> &'static str;

    /// Returns exact canonical meteorological field dependencies.
    fn required_fields(&self) -> &'static [CanonicalField];

    /// Assigns ozone mass without mutating carrier mass.
    fn assign(&self, input: OzoneAssignmentInput) -> Result<OzoneAssignment, PopulationError>;
}

/// Stable registry of named ozone assignment rules.
#[derive(Clone, Default)]
pub struct OzoneAssignmentRuleRegistry {
    rules: BTreeMap<String, Arc<dyn OzoneAssignmentRule>>,
}

impl OzoneAssignmentRuleRegistry {
    /// Constructs the production registry with all M4-supported rules.
    #[must_use]
    pub fn builtins() -> Self {
        let mut registry = Self::default();
        registry.rules.insert(
            crate::science::FLEXPART_PV60_OZONE_ID.into(),
            Arc::new(FlexpartPv60OzoneRule),
        );
        registry
    }

    /// Registers one rule and rejects empty or duplicate identities.
    pub fn register(&mut self, rule: Arc<dyn OzoneAssignmentRule>) -> Result<(), PopulationError> {
        let id = rule.model_id();
        if id.trim().is_empty() || self.rules.contains_key(id) {
            return Err(PopulationError::InvalidConfiguration);
        }
        self.rules.insert(id.into(), rule);
        Ok(())
    }

    /// Resolves one exact rule identity.
    #[must_use]
    pub fn resolve(&self, id: &str) -> Option<Arc<dyn OzoneAssignmentRule>> {
        self.rules.get(id).cloned()
    }

    /// Returns stable sorted rule identities for diagnostics.
    pub fn model_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.keys().map(String::as_str)
    }
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
    /// Deterministic dry-air seeding or accounting failed.
    AirMass(AirMassPopulationError),
    /// Meteorological dry-air finite-volume derivation failed.
    AirMassDerivation(String),
    /// Independent dry-air scalar reference formula failed.
    AirMassReference(String),
    /// Required domain-fill meteorology is unavailable.
    MissingMeteorology,
    /// Mass conservation exceeds the declared tolerance.
    MassImbalance,
    /// Particle batch is inconsistent.
    InvalidParticleBatch,
    /// A named ozone rule received invalid physical input.
    OzoneReference(ReferenceError),
    /// Required native-grid PV support is absent or invalid.
    MissingOzoneDiagnostic,
    /// No dry-air control-volume mass satisfies the strict ozone mask.
    NoEligibleOzoneMass,
    /// A sample selected from eligible mass failed the same assignment mask.
    OzoneEligibilityMismatch,
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
            Self::AirMass(error) => error.code(),
            Self::AirMassDerivation(_) => "population.air_mass.derivation",
            Self::AirMassReference(_) => "population.air_mass.reference",
            Self::MissingMeteorology => "population.missing_meteorology",
            Self::MassImbalance => "population.mass_imbalance",
            Self::InvalidParticleBatch => "population.invalid_particle_batch",
            Self::OzoneReference(_) => "population.ozone_reference",
            Self::MissingOzoneDiagnostic => "population.ozone_missing_diagnostic",
            Self::NoEligibleOzoneMass => "population.ozone_no_eligible_mass",
            Self::OzoneEligibilityMismatch => "population.ozone_eligibility_mismatch",
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
        DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, GeoJsonGeometry, GeoJsonSource,
        PopulationId, ReleaseEventId, ReleaseEventSpec,
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
    use crate::synthetic::{SyntheticWind, constant_wind_stack};

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

    fn domain_fill_spec(domain: &str, count: u64) -> DomainFillAirMassSpec {
        DomainFillAirMassSpec {
            id: PopulationId("air".into()),
            domain_id: DomainId(domain.into()),
            target_dry_air_mass_per_particle: None,
            target_particle_count: Some(count),
        }
    }

    fn ozone_domain_fill_spec(domain: &str, count: u64) -> DomainFillStratosphericOzoneSpec {
        DomainFillStratosphericOzoneSpec {
            air_mass: domain_fill_spec(domain, count),
            ozone_rule: crate::science::FLEXPART_PV60_OZONE_ID.into(),
            ozone_substance: SubstanceId("ozone".into()),
        }
    }

    #[test]
    fn global_domain_fill_initializes_and_records_closed_mass_step() {
        let times = [
            Timestamp::new(-10, 0).unwrap(),
            Timestamp::UNIX_EPOCH,
            Timestamp::new(10, 0).unwrap(),
            Timestamp::new(20, 0).unwrap(),
        ];
        let mut stack = constant_wind_stack(
            "global",
            &times,
            SyntheticWind {
                eastward_m_s: 0.0,
                northward_m_s: 0.0,
                vertical_m_s: 0.0,
            },
            0.0,
            20_000.0,
            true,
        )
        .unwrap();
        let mut population = DomainFillAirMass::new(domain_fill_spec("global", 8), None);
        let mut particles = ParticleBatch::default();
        {
            let mut context = PopulationContext {
                time: Timestamp::UNIX_EPOCH,
                direction: Direction::Forward,
                step: None,
                step_index: None,
                random_seed: 17,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population.initialize(&mut context, &mut particles).unwrap();
            assert!(population.emit_particles(&mut context).unwrap().is_empty());
        }
        assert_eq!(particles.len().unwrap(), 8);
        let step = SignedDuration(10_000_000_000);
        {
            let mut context = PopulationContext {
                time: Timestamp::UNIX_EPOCH,
                direction: Direction::Forward,
                step: Some(step),
                step_index: Some(0),
                random_seed: 17,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            assert!(
                population
                    .dynamic_step_boundaries(&mut context, step)
                    .unwrap()
                    .is_empty()
            );
            population.before_step(&mut context, &particles).unwrap();
        }
        {
            let mut context = PopulationContext {
                time: Timestamp::new(10, 0).unwrap(),
                direction: Direction::Forward,
                step: Some(step),
                step_index: Some(0),
                random_seed: 17,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population
                .after_advection(&mut context, &particles)
                .unwrap();
            population
                .apply_boundary_maintenance(&mut context, &mut particles)
                .unwrap();
            assert!(population.emit_particles(&mut context).unwrap().is_empty());
        }
        {
            let mut context = PopulationContext {
                time: Timestamp::new(10, 0).unwrap(),
                direction: Direction::Forward,
                step: None,
                step_index: None,
                random_seed: 17,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population.finalize(&mut context, &particles).unwrap();
        }
        assert_eq!(population.mass_ledger_records().len(), 1);
        assert_eq!(population.mass_ledger_records()[0].imbalance_kg, 0.0);
    }

    #[test]
    fn global_ozone_domain_fill_keeps_exact_count_and_persistent_ozone_mass() {
        let times = [
            Timestamp::new(-10, 0).unwrap(),
            Timestamp::UNIX_EPOCH,
            Timestamp::new(10, 0).unwrap(),
        ];
        let mut stack = constant_wind_stack(
            "global-ozone",
            &times,
            SyntheticWind {
                eastward_m_s: 0.0,
                northward_m_s: 0.0,
                vertical_m_s: 0.0,
            },
            0.0,
            20_000.0,
            true,
        )
        .unwrap();
        let specification = ozone_domain_fill_spec("global-ozone", 16);
        let rule = OzoneAssignmentRuleRegistry::builtins()
            .resolve(&specification.ozone_rule)
            .unwrap();
        let mut population = DomainFillStratosphericOzone::new(specification, rule, None).unwrap();
        let mut particles = ParticleBatch::default();
        {
            let mut context = PopulationContext {
                time: Timestamp::UNIX_EPOCH,
                direction: Direction::Forward,
                step: None,
                step_index: None,
                random_seed: 29,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population.initialize(&mut context, &mut particles).unwrap();
        }
        assert_eq!(particles.len().unwrap(), 16);
        assert!(
            particles
                .height_asl_m
                .iter()
                .all(|height| *height > 3_000.0)
        );
        let ozone_before = particles
            .mass
            .mass_kg
            .get(&SubstanceId("ozone".into()))
            .unwrap()
            .clone();
        assert!(ozone_before.iter().all(|mass| *mass > 0.0));

        // Ozone is assigned at birth and remains a carried substance if a
        // particle later moves below the initialization mask.
        particles.height_asl_m[0] = 1_000.0;
        let step = SignedDuration(10_000_000_000);
        {
            let mut context = PopulationContext {
                time: Timestamp::UNIX_EPOCH,
                direction: Direction::Forward,
                step: Some(step),
                step_index: Some(0),
                random_seed: 29,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            assert!(
                population
                    .dynamic_step_boundaries(&mut context, step)
                    .unwrap()
                    .is_empty()
            );
            population.before_step(&mut context, &particles).unwrap();
        }
        {
            let mut context = PopulationContext {
                time: Timestamp::new(10, 0).unwrap(),
                direction: Direction::Forward,
                step: Some(step),
                step_index: Some(0),
                random_seed: 29,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population
                .after_advection(&mut context, &particles)
                .unwrap();
            population
                .apply_boundary_maintenance(&mut context, &mut particles)
                .unwrap();
        }
        assert_eq!(
            particles
                .mass
                .mass_kg
                .get(&SubstanceId("ozone".into()))
                .unwrap(),
            &ozone_before
        );
        {
            let mut context = PopulationContext {
                time: Timestamp::new(10, 0).unwrap(),
                direction: Direction::Forward,
                step: None,
                step_index: None,
                random_seed: 29,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population.finalize(&mut context, &particles).unwrap();
        }
        assert_eq!(population.mass_ledger_records().len(), 1);
    }

    #[test]
    fn finite_domain_birth_is_cohort_local_and_records_one_macro_step() {
        let times = [
            Timestamp::new(-10, 0).unwrap(),
            Timestamp::UNIX_EPOCH,
            Timestamp::new(10, 0).unwrap(),
            Timestamp::new(20, 0).unwrap(),
        ];
        let mut stack = constant_wind_stack(
            "finite",
            &times,
            SyntheticWind {
                eastward_m_s: 100.0,
                northward_m_s: 0.0,
                vertical_m_s: 0.0,
            },
            0.0,
            20_000.0,
            false,
        )
        .unwrap();
        let mut population = DomainFillAirMass::new(domain_fill_spec("finite", 4), None);
        let mut particles = ParticleBatch::default();
        {
            let mut context = PopulationContext {
                time: Timestamp::UNIX_EPOCH,
                direction: Direction::Forward,
                step: None,
                step_index: None,
                random_seed: 23,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population.initialize(&mut context, &mut particles).unwrap();
        }
        let midpoint = Timestamp::new(5, 0).unwrap();
        let window = stack
            .engine
            .prepare_for_domain(midpoint, &stack.domain)
            .unwrap();
        let snapshot = AirMassDeriver.derive_window(&window).unwrap();
        let west = snapshot
            .boundary_faces
            .iter()
            .find(|face| {
                face.side == trajecta_met::derive::domain_fill::BoundarySide::West
                    && face.inflow_rate_kg_s(Direction::Forward).unwrap() > 0.0
            })
            .unwrap();
        let rate = west.inflow_rate_kg_s(Direction::Forward).unwrap();
        let carrier = population.carrier_mass_per_particle_kg.unwrap();
        let required = (rate * 5.0).min(carrier * 0.5);
        population
            .residual_mass
            .residual_mass_kg
            .insert(west.face_id, carrier - required);
        population.sync_state(4);

        let base_step = SignedDuration(10_000_000_000);
        let births = {
            let mut context = PopulationContext {
                time: Timestamp::UNIX_EPOCH,
                direction: Direction::Forward,
                step: Some(base_step),
                step_index: Some(0),
                random_seed: 23,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population
                .prepare_cohort_step(&mut context, &particles)
                .unwrap()
        };
        assert_eq!(births.len().unwrap(), 1);
        let birth_time = births.birth_time[0];
        assert!(birth_time > Timestamp::UNIX_EPOCH);
        let end = Timestamp::new(10, 0).unwrap();
        assert!(birth_time <= end);
        particles.append(births).unwrap();
        let newborn_index = particles.len().unwrap() - 1;
        particles.status[newborn_index] = ParticleStatus::Terminated {
            reason: TerminationReason::PopulationOutflow,
        };
        {
            let mut context = PopulationContext {
                time: end,
                direction: Direction::Forward,
                step: Some(base_step),
                step_index: Some(0),
                random_seed: 23,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population
                .complete_cohort_step(&mut context, &mut particles)
                .unwrap();
        }
        {
            let mut context = PopulationContext {
                time: end,
                direction: Direction::Forward,
                step: None,
                step_index: None,
                random_seed: 23,
                meteorology: &mut stack.engine,
                execution: stack.execution.as_ref(),
                domain: Some(stack.domain.clone()),
            };
            population.finalize(&mut context, &particles).unwrap();
        }
        assert_eq!(particles.len().unwrap(), 5);
        let records = population.mass_ledger_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].step_index, 0);
        assert_eq!(records[0].outgoing_kg, carrier);
        assert!(records[0].imbalance_kg.abs() <= records[0].tolerance_kg);
    }
}
