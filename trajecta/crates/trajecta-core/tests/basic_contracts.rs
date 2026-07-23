//! Executable checks for foundational particle contracts.

use trajecta_core::particle::{
    ParticleBatch, ParticleError, ParticleId, TerminationClass, TerminationReason,
};

#[test]
fn particle_soa_rejects_mismatched_columns() {
    let batch = ParticleBatch {
        id: vec![ParticleId(1)],
        ..ParticleBatch::default()
    };
    assert_eq!(batch.len(), Err(ParticleError::LengthMismatch));
}

#[test]
fn termination_classification_cannot_hide_particle_errors() {
    assert_eq!(
        TerminationReason::OutsideDomain.class(),
        TerminationClass::Normal
    );
    assert_eq!(
        TerminationReason::PopulationOutflow.class(),
        TerminationClass::Normal
    );
    assert_eq!(
        TerminationReason::InvalidMeteorology.class(),
        TerminationClass::Abnormal
    );
    assert_eq!(
        TerminationReason::ReflectionLimit.class(),
        TerminationClass::Abnormal
    );
}
