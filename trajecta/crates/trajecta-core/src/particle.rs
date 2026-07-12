//! # Contract: particle state and SoA storage
//!
//! Particle batches use stable identifiers and structure-of-arrays storage.
//! Status and termination reason are typed values, while substance mass is
//! indexed by stable substance identity and particle position.

use std::collections::BTreeMap;

use trajecta_case::model::population::PopulationId;
use trajecta_case::model::substance::SubstanceId;

/// Stable particle identifier independent of storage order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ParticleId(pub u64);

/// Reason a particle no longer participates in transport.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminationReason {
    /// Policy terminated the particle at the lower surface.
    Surface,
    /// Particle crossed the native meteorology model top.
    ModelTop,
    /// Particle left a limited horizontal domain.
    OutsideDomain,
    /// Population policy removed an outflow particle.
    PopulationOutflow,
    /// A typed numerical failure prevented continued transport.
    NumericalFailure,
}

/// Active or terminated particle status.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ParticleStatus {
    /// Particle participates in transport.
    Alive,
    /// Particle is retained for audit but no longer transported.
    Terminated(TerminationReason),
}

/// Convenient owned view of one particle.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleState {
    /// Stable particle identity.
    pub id: ParticleId,
    /// Longitude in degrees east.
    pub longitude_degrees: f64,
    /// Latitude in degrees north.
    pub latitude_degrees: f64,
    /// Geometric height above mean sea level in metres.
    pub height_asl_m: f64,
    /// Signed age in nanoseconds from emission or initialization.
    pub age_nanoseconds: i64,
    /// Current transport status.
    pub status: ParticleStatus,
    /// Owning population.
    pub population_id: PopulationId,
}

/// Substance-major mass arrays with one value per particle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubstanceMassStore {
    /// Mass columns by stable substance identifier.
    pub mass_kg: BTreeMap<SubstanceId, Vec<f64>>,
}

/// Structure-of-arrays particle storage used by hot execution paths.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParticleBatch {
    /// Stable particle IDs.
    pub id: Vec<ParticleId>,
    /// Longitude in degrees east.
    pub longitude_degrees: Vec<f64>,
    /// Latitude in degrees north.
    pub latitude_degrees: Vec<f64>,
    /// Geometric height above mean sea level in metres.
    pub height_asl_m: Vec<f64>,
    /// Signed particle ages in nanoseconds.
    pub age_nanoseconds: Vec<i64>,
    /// Typed particle status.
    pub status: Vec<ParticleStatus>,
    /// Owning population per particle.
    pub population_id: Vec<PopulationId>,
    /// Substance-major mass arrays.
    pub mass: SubstanceMassStore,
}

impl ParticleBatch {
    /// Returns the particle count when every SoA column is consistent.
    pub fn len(&self) -> Result<usize, ParticleError> {
        let len = self.id.len();
        let primary_lengths = [
            self.longitude_degrees.len(),
            self.latitude_degrees.len(),
            self.height_asl_m.len(),
            self.age_nanoseconds.len(),
            self.status.len(),
            self.population_id.len(),
        ];
        if primary_lengths.into_iter().any(|value| value != len)
            || self.mass.mass_kg.values().any(|values| values.len() != len)
        {
            return Err(ParticleError::LengthMismatch);
        }
        Ok(len)
    }

    /// Returns true when every primary SoA column is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
            && self.longitude_degrees.is_empty()
            && self.latitude_degrees.is_empty()
            && self.height_asl_m.is_empty()
            && self.age_nanoseconds.is_empty()
            && self.status.is_empty()
            && self.population_id.is_empty()
    }
}

/// Particle storage failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParticleError {
    /// SoA or substance columns have inconsistent lengths.
    LengthMismatch,
    /// Particle identifiers are not unique.
    DuplicateParticleId(ParticleId),
    /// A mass value is negative or non-finite.
    InvalidMass,
}
