//! # Contract: simulation output products and encoders
//!
//! Products decide scientific sampling; encoders decide representation. The
//! runner calls products at explicit event boundaries. v0 contracts include
//! particle state and meteorology replay, while concentration/deposition
//! products can be added without changing the runner interface.

use std::path::Path;

pub mod particle_state;
pub mod provenance_bundle;
pub mod sqlite;

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
    /// Returns the stable scientific product identifier.
    fn product_id(&self) -> &'static str;

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

    /// Optionally contributes product-owned summaries into the terminal manifest.
    ///
    /// Default is a no-op so existing products remain valid. SQLite sinks use this
    /// to publish integrity-checked row counts before the final manifest write.
    fn contribute_manifest(
        &mut self,
        _manifest: &mut crate::manifest::RunManifest,
    ) -> Result<(), OutputError> {
        Ok(())
    }

    /// Immediate abort of product/sink ephemeral state. Default no-op.
    fn abort(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    /// Quarantine formal artifacts after terminal manifest persistence failure.
    fn quarantine_forensic(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

pub use particle_state::ParticleStateProduct;
pub use provenance_bundle::{ProvenanceBundleBuilder, ProvenanceBundleIdentity};
pub use sqlite::{ParticleStateSqliteSink, SQLITE_SCHEMA_SQL, SqliteInspection, sqlite_summary};

/// Deterministic meteorology-replay output product.
#[derive(Default)]
pub struct MeteorologyReplayProduct;

impl OutputProduct for MeteorologyReplayProduct {
    fn product_id(&self) -> &'static str {
        "meteorology_replay/v1"
    }

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

/// Typed sink for particle-state events.
pub trait ParticleStateSink: Send {
    /// Returns the stable sink implementation identifier.
    fn sink_id(&self) -> &'static str;

    /// Opens one run target after the running manifest exists.
    fn begin(&mut self, target: &Path, manifest: &RunManifest) -> Result<(), OutputError>;

    /// Atomically writes one logical event for all supplied particles.
    ///
    /// Meteorology, when present, must have been queried at `time` and the
    /// exact stored particle positions rather than reused from an RK2 midpoint.
    fn write_event(
        &mut self,
        time: Timestamp,
        particles: &ParticleBatch,
        meteorology: Option<&QueryOutput>,
    ) -> Result<(), OutputError>;

    /// Finalizes indexes, integrity checks, and target metadata.
    fn finish(&mut self) -> Result<(), OutputError>;

    /// Optional published row counts after a successful finish.
    fn row_counts(&self) -> Option<std::collections::BTreeMap<String, u64>> {
        None
    }

    /// Formal provenance-bundle/v1 identity after a successful finish.
    fn provenance_identity(&self) -> Option<crate::manifest::ProvenanceBundleIdentity> {
        None
    }

    /// Immediate abort of in-flight sink state (spool/tmp/runs). Default no-op.
    fn abort(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    /// Quarantine a published formal bundle after terminal manifest failure.
    ///
    /// Must not leave a seemingly formal `provenance-bundle.json` in place.
    fn quarantine_forensic(&mut self) -> Result<(), OutputError> {
        Ok(())
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

impl OutputError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented => "output.not_implemented",
            Self::InvalidInput => "output.invalid_input",
            Self::Io(_) => "output.io",
            Self::Encoding(_) => "output.encoding",
        }
    }
}
