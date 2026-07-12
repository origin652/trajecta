//! # Contract: order-independent deterministic sampling
//!
//! Random values are keyed by seed, stable particle identity, and event index,
//! never by thread or iteration order. Low-discrepancy release/domain-fill
//! sampling uses the same explicit reproducibility boundary.

use crate::particle::ParticleId;

/// Complete key for one counter-based random value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RandomKey {
    /// User or Case seed.
    pub seed: u64,
    /// Stable particle identity.
    pub particle: ParticleId,
    /// Stable lifecycle event index.
    pub event_index: u64,
    /// Independent sample lane within the event.
    pub lane: u32,
}

/// Counter-based random-number contract independent of execution order.
#[derive(Clone, Copy, Debug, Default)]
pub struct CounterRng;

impl CounterRng {
    /// Produces one deterministic 64-bit value for an exact key.
    pub fn sample_u64(_key: RandomKey) -> Result<u64, RngError> {
        Err(RngError::NotImplemented)
    }

    /// Produces one deterministic value in the half-open interval `[0, 1)`.
    pub fn sample_unit(_key: RandomKey) -> Result<f64, RngError> {
        Err(RngError::NotImplemented)
    }
}

/// Deterministic low-discrepancy sampler for geometric seeding.
#[derive(Clone, Copy, Debug, Default)]
pub struct LowDiscrepancySampler;

impl LowDiscrepancySampler {
    /// Produces a deterministic point in a requested number of dimensions.
    pub fn sample(_index: u64, _dimensions: usize) -> Result<Vec<f64>, RngError> {
        Err(RngError::NotImplemented)
    }
}

/// Deterministic-sampling failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RngError {
    /// Sampling algorithms have not been implemented yet.
    NotImplemented,
    /// Requested dimensionality is unsupported.
    UnsupportedDimensions(usize),
}
