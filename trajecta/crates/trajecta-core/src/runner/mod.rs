//! # Contract: complete simulation orchestration
//!
//! The runner owns the clock, particles, meteorology engine, population,
//! integrator, boundaries, and products. Its main loop respects every event
//! boundary and keeps all strategy calls in the declared lifecycle order.

use trajecta_case::document::{ResolvedCase, ResolvedRunProfile};
use trajecta_met::query::engine::MetEngine;

use crate::boundary::BoundaryPolicy;
use crate::clock::SimulationClock;
use crate::integrator::IntegratorModel;
use crate::manifest::RunManifest;
use crate::output::OutputProduct;
use crate::particle::ParticleBatch;
use crate::population::{PopulationState, PopulationStrategy};

/// Mutable checkpointable simulation state.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationState {
    /// Current physical simulation clock.
    pub clock: SimulationClock,
    /// Current particle SoA.
    pub particles: ParticleBatch,
    /// Persistent strategy state exposed for checkpointing.
    pub population_state: PopulationState,
    /// Next stable output event index.
    pub output_event_index: usize,
}

/// Complete single-run orchestrator.
pub struct SimulationRunner {
    state: SimulationState,
    meteorology: MetEngine,
    population: Box<dyn PopulationStrategy>,
    integrator: Box<dyn IntegratorModel>,
    boundaries: Vec<Box<dyn BoundaryPolicy>>,
    outputs: Vec<Box<dyn OutputProduct>>,
    manifest: RunManifest,
}

impl SimulationRunner {
    /// Executes the complete event-driven main loop.
    pub fn run(&mut self) -> Result<&RunManifest, RunError> {
        let _ = (
            &self.state,
            &self.meteorology,
            &self.population,
            &self.integrator,
            &self.boundaries,
            &self.outputs,
            &self.manifest,
        );
        Err(RunError::NotImplemented)
    }
}

/// Builder that resolves model identifiers and validates runtime capabilities.
pub struct RunnerBuilder {
    /// Portable resolved scientific intent.
    pub case: ResolvedCase,
    /// Resolved machine resources and dataset locations.
    pub run_profile: ResolvedRunProfile,
}

impl RunnerBuilder {
    /// Constructs a runnable simulation or returns complete typed diagnostics.
    pub fn build(self) -> Result<SimulationRunner, RunError> {
        Err(RunError::NotImplemented)
    }
}

/// Fatal run-construction or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunError {
    /// Runner construction/main loop has not been implemented yet.
    NotImplemented,
    /// Meteorology could not prepare the required time window.
    Meteorology(String),
    /// Population lifecycle failed.
    Population(String),
    /// Particle integration failed.
    Integration(String),
    /// Output product or encoder failed.
    Output(String),
    /// Domain-fill mass conservation exceeded tolerance.
    MassConservation,
    /// Requested memory or particle capacity cannot be satisfied.
    ResourceLimit,
}
