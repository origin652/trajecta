//! # Contract: particle_state/v1 output product
//!
//! Bridges the runner OutputProduct interface to a typed ParticleStateSink.

use std::path::PathBuf;

use trajecta_case::model::time::Timestamp;
use trajecta_met::query::output::QueryOutput;

use crate::manifest::RunManifest;
use crate::output::{ConvectionProcessEvent, OutputError, OutputProduct, ParticleStateSink};
use crate::particle::ParticleBatch;

/// Production particle-state product.
pub struct ParticleStateProduct {
    sink: Box<dyn ParticleStateSink>,
    target: PathBuf,
    begun: bool,
}

impl ParticleStateProduct {
    /// Creates a product that writes through the supplied sink.
    #[must_use]
    pub fn new(sink: Box<dyn ParticleStateSink>, target: PathBuf) -> Self {
        Self {
            sink,
            target,
            begun: false,
        }
    }
}

impl OutputProduct for ParticleStateProduct {
    fn product_id(&self) -> &'static str {
        trajecta_case::model::output::PARTICLE_STATE_PRODUCT_ID
    }

    fn begin(&mut self, manifest: &RunManifest) -> Result<(), OutputError> {
        self.sink.begin(&self.target, manifest)?;
        self.begun = true;
        Ok(())
    }

    fn sample(
        &mut self,
        time: Timestamp,
        particles: &ParticleBatch,
        meteorology: Option<&QueryOutput>,
    ) -> Result<(), OutputError> {
        if !self.begun {
            return Err(OutputError::InvalidInput);
        }
        self.sink.write_event(time, particles, meteorology)
    }

    fn write_convection_events(
        &mut self,
        events: &[ConvectionProcessEvent],
    ) -> Result<(), OutputError> {
        if !self.begun {
            return Err(OutputError::InvalidInput);
        }
        self.sink.write_convection_events(events)
    }

    fn finish(&mut self) -> Result<(), OutputError> {
        if !self.begun {
            return Err(OutputError::InvalidInput);
        }
        self.sink.finish()
    }

    fn contribute_manifest(
        &mut self,
        manifest: &mut crate::manifest::RunManifest,
    ) -> Result<(), OutputError> {
        if let Some(counts) = self.sink.row_counts() {
            manifest.sqlite.row_counts = counts;
        }
        if let Some(identity) = self.sink.provenance_identity() {
            manifest.provenance = Some(identity);
        }
        Ok(())
    }

    fn abort(&mut self) -> Result<(), OutputError> {
        self.sink.abort()
    }

    fn quarantine_forensic(&mut self) -> Result<(), OutputError> {
        self.sink.quarantine_forensic()
    }
}
