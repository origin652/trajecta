//! # Contract: particle integration
//!
//! The v0 integrator is a spherical midpoint RK2 scheme. It queries wind at
//! the start and midpoint, advances longitude/latitude on a sphere, uses
//! geometric vertical velocity, supports signed time, and applies boundary
//! policies after proposing the full step.

use trajecta_case::model::time::Timestamp;
use trajecta_met::query::engine::{ExecutionContext, MetEngine};
use trajecta_met::query::request::QueryPlan;

use crate::boundary::BoundaryPolicy;
use crate::clock::SignedDuration;
use crate::particle::ParticleBatch;

/// Immutable particle state and signed step presented to an integrator.
#[derive(Clone, Copy, Debug)]
pub struct IntegratorInput<'a> {
    /// Current particle batch.
    pub particles: &'a ParticleBatch,
    /// Physical start time.
    pub time: Timestamp,
    /// Signed step duration.
    pub step: SignedDuration,
}

/// Mutable meteorology access and immutable execution policy for one step.
pub struct IntegratorContext<'a> {
    /// Meteorology engine used only through explicit prepare/query phases.
    pub meteorology: &'a mut MetEngine,
    /// Precompiled transport query plan.
    pub query_plan: &'a QueryPlan,
    /// Caller-owned execution context.
    pub execution: &'a dyn ExecutionContext,
    /// Ordered post-step boundary policy chain.
    pub boundaries: &'a [Box<dyn BoundaryPolicy>],
}

/// Proposed next particle batch and per-step diagnostics.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepResult {
    /// Particle state after integration and boundaries.
    pub particles: ParticleBatch,
    /// Number of particles terminated during this step.
    pub terminated_count: usize,
}

/// Deterministic particle-advance interface.
pub trait IntegratorModel: Send + Sync {
    /// Advances one batch by one signed step.
    fn advance(
        &self,
        input: IntegratorInput<'_>,
        context: &mut IntegratorContext<'_>,
    ) -> Result<StepResult, IntegratorError>;
}

/// v0 spherical midpoint integrator without random turbulence.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rk2Spherical;

impl IntegratorModel for Rk2Spherical {
    fn advance(
        &self,
        _input: IntegratorInput<'_>,
        _context: &mut IntegratorContext<'_>,
    ) -> Result<StepResult, IntegratorError> {
        Err(IntegratorError::NotImplemented)
    }
}

/// Particle-integration failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegratorError {
    /// Integrator algorithm has not been implemented yet.
    NotImplemented,
    /// Required meteorology could not be prepared or queried.
    Meteorology(String),
    /// Input particle SoA is inconsistent.
    InvalidParticleBatch,
    /// Spherical or vertical arithmetic produced a non-finite result.
    NumericalFailure(usize),
    /// A boundary policy failed.
    Boundary(String),
}
