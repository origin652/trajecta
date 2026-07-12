//! Executable checks for foundational particle contracts.

use trajecta_core::particle::{ParticleBatch, ParticleError, ParticleId};

#[test]
fn particle_soa_rejects_mismatched_columns() {
    let batch = ParticleBatch {
        id: vec![ParticleId(1)],
        ..ParticleBatch::default()
    };
    assert_eq!(batch.len(), Err(ParticleError::LengthMismatch));
}
