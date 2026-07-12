//! # Contract: full particle-population lifecycle
//!
//! A population strategy owns initialization, pre-step work, emission,
//! post-advection accounting, boundary maintenance, and finalization. Domain
//! filling preserves residual boundary mass across steps and chooses inflow
//! according to integration direction.

use std::collections::BTreeMap;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, ReleaseDrivenSpec,
};
use trajecta_case::model::time::Timestamp;
use trajecta_met::query::engine::{ExecutionContext, MetEngine};

use crate::particle::ParticleBatch;

/// Mutable services and physical time supplied to a population strategy.
pub struct PopulationContext<'a> {
    /// Current physical time.
    pub time: Timestamp,
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
        /// Next stable event index.
        next_event_index: usize,
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
#[derive(Clone, Debug)]
pub struct ReleaseDrivenPopulation {
    /// Portable Case configuration.
    pub specification: ReleaseDrivenSpec,
    /// Persistent runtime state.
    pub state: PopulationState,
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
    ($type_name:ty) => {
        impl PopulationStrategy for $type_name {
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

unimplemented_population!(ReleaseDrivenPopulation);
unimplemented_population!(DomainFillAirMass);
unimplemented_population!(DomainFillStratosphericOzone);

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
}
