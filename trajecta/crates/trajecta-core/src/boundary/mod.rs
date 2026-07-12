//! # Contract: particle boundary policies
//!
//! Meteorology reports typed geometric/query status. Particle policies decide
//! whether to reflect, wrap, terminate, or retain a particle. Boundary logic
//! never changes meteorological source values.

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;
use trajecta_met::query::output::SampleStatus;

use crate::particle::ParticleState;

/// Context required to apply a boundary decision.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryContext {
    /// Physical step endpoint.
    pub time: Timestamp,
    /// Selected meteorology domain, when available.
    pub domain: Option<DomainId>,
    /// Meteorology status at the proposed endpoint.
    pub meteorology_status: SampleStatus,
    /// Optional local surface height above sea level in metres.
    pub surface_height_asl_m: Option<f64>,
    /// Optional native model-top height in metres.
    pub model_top_height_asl_m: Option<f64>,
}

/// One composable post-integration particle boundary policy.
pub trait BoundaryPolicy: Send + Sync {
    /// Applies a deterministic policy to one proposed particle state.
    fn apply(
        &self,
        particle: &mut ParticleState,
        context: &BoundaryContext,
    ) -> Result<(), BoundaryError>;
}

/// Reflects proposed states below the local physical surface.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceReflect;

impl BoundaryPolicy for SurfaceReflect {
    fn apply(
        &self,
        _particle: &mut ParticleState,
        _context: &BoundaryContext,
    ) -> Result<(), BoundaryError> {
        Err(BoundaryError::NotImplemented)
    }
}

/// Terminates particles that cross the native model top.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModelTopTerminate;

impl BoundaryPolicy for ModelTopTerminate {
    fn apply(
        &self,
        _particle: &mut ParticleState,
        _context: &BoundaryContext,
    ) -> Result<(), BoundaryError> {
        Err(BoundaryError::NotImplemented)
    }
}

/// Terminates particles outside a limited domain.
#[derive(Clone, Copy, Debug, Default)]
pub struct LimitedDomainTerminate;

impl BoundaryPolicy for LimitedDomainTerminate {
    fn apply(
        &self,
        _particle: &mut ParticleState,
        _context: &BoundaryContext,
    ) -> Result<(), BoundaryError> {
        Err(BoundaryError::NotImplemented)
    }
}

/// Normalizes longitude for a globally periodic domain.
#[derive(Clone, Copy, Debug, Default)]
pub struct GlobalPeriodicBoundary;

impl BoundaryPolicy for GlobalPeriodicBoundary {
    fn apply(
        &self,
        _particle: &mut ParticleState,
        _context: &BoundaryContext,
    ) -> Result<(), BoundaryError> {
        Err(BoundaryError::NotImplemented)
    }
}

/// Boundary-policy failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundaryError {
    /// Boundary algorithms have not been implemented yet.
    NotImplemented,
    /// Required context is absent for the selected policy.
    MissingContext,
    /// Proposed particle coordinates are non-finite.
    InvalidParticleState,
}
