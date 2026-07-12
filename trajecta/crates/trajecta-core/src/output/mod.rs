//! # Contract: simulation output products and encoders
//!
//! Products decide scientific sampling; encoders decide representation. The
//! runner calls products at explicit event boundaries. v0 contracts include
//! particle state and meteorology replay, while concentration/deposition
//! products can be added without changing the runner interface.

use std::path::Path;

use trajecta_case::model::time::Timestamp;
use trajecta_met::query::output::QueryOutput;

use crate::manifest::RunManifest;
use crate::particle::ParticleBatch;

/// Representation encoder used by an output product.
pub trait OutputEncoder: Send {
    /// Opens or initializes an output target.
    fn begin(&mut self, target: &Path) -> Result<(), OutputError>;

    /// Writes one already-serialized logical record.
    fn write_record(&mut self, record: &str) -> Result<(), OutputError>;

    /// Flushes and closes the target.
    fn finish(&mut self) -> Result<(), OutputError>;
}

/// Scientific output product sampled by the simulation runner.
pub trait OutputProduct: Send {
    /// Initializes product state for one run.
    fn begin(&mut self, manifest: &RunManifest) -> Result<(), OutputError>;

    /// Samples one explicit output event.
    fn sample(
        &mut self,
        time: Timestamp,
        particles: &ParticleBatch,
        meteorology: Option<&QueryOutput>,
    ) -> Result<(), OutputError>;

    /// Finalizes product summaries and output targets.
    fn finish(&mut self) -> Result<(), OutputError>;
}

/// Particle-state output product.
#[derive(Default)]
pub struct ParticleStateProduct;

impl OutputProduct for ParticleStateProduct {
    fn begin(&mut self, _manifest: &RunManifest) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn sample(
        &mut self,
        _time: Timestamp,
        _particles: &ParticleBatch,
        _meteorology: Option<&QueryOutput>,
    ) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn finish(&mut self) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }
}

/// Deterministic meteorology-replay output product.
#[derive(Default)]
pub struct MeteorologyReplayProduct;

impl OutputProduct for MeteorologyReplayProduct {
    fn begin(&mut self, _manifest: &RunManifest) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn sample(
        &mut self,
        _time: Timestamp,
        _particles: &ParticleBatch,
        _meteorology: Option<&QueryOutput>,
    ) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn finish(&mut self) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }
}

/// Stable JSON Lines encoder contract for fixtures and automation.
#[derive(Default)]
pub struct JsonlEncoder;

impl OutputEncoder for JsonlEncoder {
    fn begin(&mut self, _target: &Path) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn write_record(&mut self, _record: &str) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn finish(&mut self) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }
}

/// Human-analysis CSV encoder contract.
#[derive(Default)]
pub struct CsvEncoder;

impl OutputEncoder for CsvEncoder {
    fn begin(&mut self, _target: &Path) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn write_record(&mut self, _record: &str) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }

    fn finish(&mut self) -> Result<(), OutputError> {
        Err(OutputError::NotImplemented)
    }
}

/// Explicit sampling and averaging event scheduler.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutputScheduler {
    /// Ordered physical sample times.
    pub sample_times: Vec<Timestamp>,
}

/// Output product or encoding failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutputError {
    /// Output implementation has not been written yet.
    NotImplemented,
    /// Product received an inconsistent particle or meteorology batch.
    InvalidInput,
    /// Output target could not be written.
    Io(String),
    /// Encoding failed for a typed record.
    Encoding(String),
}
