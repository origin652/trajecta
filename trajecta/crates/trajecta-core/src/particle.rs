//! # Contract: particle state and SoA storage
//!
//! Particle batches use stable identifiers and structure-of-arrays storage.
//! Physical mass is always non-negative. Backward integration changes the
//! signed integration offset, never mass or elapsed age.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::population::{PopulationId, ReleaseEventId};
use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::Timestamp;

/// Stable particle identifier independent of storage order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParticleId(pub u64);

impl ParticleId {
    /// Deterministically derives a release-particle ID in SQLite's signed range.
    #[must_use]
    pub fn for_release(
        population_id: &PopulationId,
        event_id: &ReleaseEventId,
        ordinal: u64,
    ) -> Self {
        stable_particle_id(
            b"release\0",
            &[population_id.0.as_str(), event_id.0.as_str()],
            &[ordinal],
        )
    }

    /// Deterministically derives an initial domain-fill particle ID.
    #[must_use]
    pub fn for_domain_initial(
        population_id: &PopulationId,
        domain_id: &DomainId,
        ordinal: u64,
    ) -> Self {
        stable_particle_id(
            b"domain-initial\0",
            &[population_id.0.as_str(), domain_id.0.as_str()],
            &[ordinal],
        )
    }

    /// Deterministically derives a dynamic boundary-birth particle ID.
    #[must_use]
    pub fn for_domain_boundary(
        population_id: &PopulationId,
        domain_id: &DomainId,
        boundary_face_id: BoundaryFaceId,
        lifecycle_event_index: u64,
        ordinal: u64,
    ) -> Self {
        stable_particle_id(
            b"domain-boundary\0",
            &[population_id.0.as_str(), domain_id.0.as_str()],
            &[boundary_face_id.0, lifecycle_event_index, ordinal],
        )
    }
}

/// Stable domain-boundary face identity used for dynamic births.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BoundaryFaceId(pub u64);

/// Typed origin of a particle.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParticleOrigin {
    /// Particle was emitted by an explicit release event.
    Release {
        /// Stable event identity.
        event_id: ReleaseEventId,
    },
    /// Particle was created during initial domain filling.
    DomainInitial {
        /// Filled meteorological domain.
        domain_id: DomainId,
    },
    /// Particle was created from accumulated boundary inflow mass.
    DomainBoundary {
        /// Filled meteorological domain.
        domain_id: DomainId,
        /// Stable face/layer identity.
        boundary_face_id: BoundaryFaceId,
    },
}

/// Scientific classification of a particle termination.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationClass {
    /// Expected lifecycle or physical-domain exit.
    Normal,
    /// Unexpected numerical or in-domain data failure.
    Abnormal,
}

/// Reason a particle no longer participates in transport.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminationReason {
    /// Particle crossed the native meteorology model top.
    ModelTop,
    /// Particle left a limited horizontal domain.
    OutsideDomain,
    /// Population policy removed an outflow particle.
    PopulationOutflow,
    /// A user-selected terminating boundary policy acted normally.
    UserBoundary {
        /// Stable boundary-policy identifier.
        policy_id: String,
    },
    /// Deterministic arithmetic failed.
    NumericalFailure,
    /// Meteorology was invalid inside an expected-valid domain.
    InvalidMeteorology,
    /// Surface collision root finding or reflection exceeded its limit.
    ReflectionLimit,
    /// A particle coordinate, mass, or derived state became non-finite.
    NonFiniteState,
}

impl TerminationReason {
    /// Returns the frozen normal/abnormal classification.
    #[must_use]
    pub const fn class(&self) -> TerminationClass {
        match self {
            Self::ModelTop
            | Self::OutsideDomain
            | Self::PopulationOutflow
            | Self::UserBoundary { .. } => TerminationClass::Normal,
            Self::NumericalFailure
            | Self::InvalidMeteorology
            | Self::ReflectionLimit
            | Self::NonFiniteState => TerminationClass::Abnormal,
        }
    }

    /// Returns the stable manifest/output reason identifier.
    #[must_use]
    pub fn code(&self) -> String {
        match self {
            Self::ModelTop => "model_top".into(),
            Self::OutsideDomain => "outside_domain".into(),
            Self::PopulationOutflow => "population_outflow".into(),
            Self::UserBoundary { policy_id } => format!("user_boundary:{policy_id}"),
            Self::NumericalFailure => "numerical_failure".into(),
            Self::InvalidMeteorology => "invalid_meteorology".into(),
            Self::ReflectionLimit => "reflection_limit".into(),
            Self::NonFiniteState => "non_finite_state".into(),
        }
    }
}

/// Active or terminated particle status.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParticleStatus {
    /// Particle participates in transport.
    Alive,
    /// Particle is retained for audit but no longer transported.
    Terminated {
        /// Typed termination reason.
        reason: TerminationReason,
    },
}

/// Exact lifecycle metadata for a particle that stopped transport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticleTermination {
    /// Physical instant at which transport stopped.
    pub time: Timestamp,
    /// Located fraction of the particle-local numerical step for geometric
    /// boundary intersections. Integration-local failures leave this absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intersection_fraction: Option<f64>,
}

impl ParticleTermination {
    /// Validates the optional normalized boundary fraction.
    pub fn validate(&self) -> Result<(), ParticleError> {
        if self
            .intersection_fraction
            .is_some_and(|fraction| !fraction.is_finite() || !(0.0..=1.0).contains(&fraction))
        {
            return Err(ParticleError::InvalidTermination);
        }
        Ok(())
    }
}

/// Convenient owned view of one particle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticleState {
    /// Stable particle identity.
    pub id: ParticleId,
    /// Owning population.
    pub population_id: PopulationId,
    /// Typed particle origin.
    pub origin: ParticleOrigin,
    /// Exact physical birth time.
    pub birth_time: Timestamp,
    /// Longitude in degrees east.
    pub longitude_degrees: f64,
    /// Latitude in degrees north.
    pub latitude_degrees: f64,
    /// Geometric height above mean sea level in metres.
    pub height_asl_m: f64,
    /// Signed integration displacement from birth in nanoseconds.
    pub integration_offset_ns: i64,
    /// Non-negative elapsed wall-clock age in nanoseconds.
    pub elapsed_age_ns: u64,
    /// Dry-air carrier mass represented by this particle in kilograms.
    pub dry_air_mass_kg: f64,
    /// Substance mass in kilograms by stable substance identifier.
    pub mass_kg: BTreeMap<SubstanceId, f64>,
    /// Optional signed source-receptor sensitivity weight, separate from mass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitivity_weight: Option<f64>,
    /// Current transport status.
    pub status: ParticleStatus,
    /// Exact terminal lifecycle metadata, when transport has stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination: Option<ParticleTermination>,
}

impl ParticleState {
    /// Validates finite coordinates, physical mass, and optional weight.
    pub fn validate(&self) -> Result<(), ParticleError> {
        if !self.longitude_degrees.is_finite()
            || !self.latitude_degrees.is_finite()
            || !self.height_asl_m.is_finite()
            || !(-90.0..=90.0).contains(&self.latitude_degrees)
        {
            return Err(ParticleError::InvalidCoordinate);
        }
        if !self.dry_air_mass_kg.is_finite() || self.dry_air_mass_kg < 0.0 {
            return Err(ParticleError::InvalidMass);
        }
        if self
            .mass_kg
            .values()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(ParticleError::InvalidMass);
        }
        if self
            .sensitivity_weight
            .is_some_and(|value| !value.is_finite())
        {
            return Err(ParticleError::InvalidSensitivityWeight);
        }
        if self.termination.is_some() && self.status == ParticleStatus::Alive {
            return Err(ParticleError::InvalidTermination);
        }
        if let Some(termination) = &self.termination {
            termination.validate()?;
        }
        Ok(())
    }
}

/// Substance-major mass arrays with one value per particle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubstanceMassStore {
    /// Mass columns by stable substance identifier.
    pub mass_kg: BTreeMap<SubstanceId, Vec<f64>>,
}

/// Structure-of-arrays particle storage used by hot execution paths.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticleBatch {
    /// Stable particle IDs.
    pub id: Vec<ParticleId>,
    /// Owning population per particle.
    pub population_id: Vec<PopulationId>,
    /// Typed origin per particle.
    pub origin: Vec<ParticleOrigin>,
    /// Exact physical birth time per particle.
    pub birth_time: Vec<Timestamp>,
    /// Longitude in degrees east.
    pub longitude_degrees: Vec<f64>,
    /// Latitude in degrees north.
    pub latitude_degrees: Vec<f64>,
    /// Geometric height above mean sea level in metres.
    pub height_asl_m: Vec<f64>,
    /// Signed integration offsets in nanoseconds.
    pub integration_offset_ns: Vec<i64>,
    /// Non-negative elapsed ages in nanoseconds.
    pub elapsed_age_ns: Vec<u64>,
    /// Dry-air carrier mass in kilograms.
    pub dry_air_mass_kg: Vec<f64>,
    /// Optional signed sensitivity weight per particle.
    pub sensitivity_weight: Vec<Option<f64>>,
    /// Typed particle status.
    pub status: Vec<ParticleStatus>,
    /// Exact terminal lifecycle metadata per particle.
    pub termination: Vec<Option<ParticleTermination>>,
    /// Substance-major mass arrays.
    pub mass: SubstanceMassStore,
}

impl ParticleBatch {
    /// Returns the particle count when every SoA column is consistent.
    pub fn len(&self) -> Result<usize, ParticleError> {
        let len = self.id.len();
        let primary_lengths = [
            self.population_id.len(),
            self.origin.len(),
            self.birth_time.len(),
            self.longitude_degrees.len(),
            self.latitude_degrees.len(),
            self.height_asl_m.len(),
            self.integration_offset_ns.len(),
            self.elapsed_age_ns.len(),
            self.dry_air_mass_kg.len(),
            self.sensitivity_weight.len(),
            self.status.len(),
            self.termination.len(),
        ];
        if primary_lengths.into_iter().any(|value| value != len)
            || self.mass.mass_kg.values().any(|values| values.len() != len)
        {
            return Err(ParticleError::LengthMismatch);
        }
        Ok(len)
    }

    /// Performs complete structural and finite-value validation.
    pub fn validate(&self) -> Result<usize, ParticleError> {
        let len = self.len()?;
        let mut ids = BTreeSet::new();
        for index in 0..len {
            if !ids.insert(self.id[index]) {
                return Err(ParticleError::DuplicateParticleId(self.id[index]));
            }
            if !self.longitude_degrees[index].is_finite()
                || !self.latitude_degrees[index].is_finite()
                || !self.height_asl_m[index].is_finite()
                || !(-90.0..=90.0).contains(&self.latitude_degrees[index])
            {
                return Err(ParticleError::InvalidCoordinate);
            }
            if !self.dry_air_mass_kg[index].is_finite() || self.dry_air_mass_kg[index] < 0.0 {
                return Err(ParticleError::InvalidMass);
            }
            if self.sensitivity_weight[index].is_some_and(|value| !value.is_finite()) {
                return Err(ParticleError::InvalidSensitivityWeight);
            }
            if self.termination[index].is_some() && self.status[index] == ParticleStatus::Alive {
                return Err(ParticleError::InvalidTermination);
            }
            if let Some(termination) = &self.termination[index] {
                termination.validate()?;
            }
        }
        if self
            .mass
            .mass_kg
            .values()
            .flatten()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(ParticleError::InvalidMass);
        }
        Ok(len)
    }

    /// Returns an owned particle view at `index`.
    pub fn state(&self, index: usize) -> Result<ParticleState, ParticleError> {
        let len = self.len()?;
        if index >= len {
            return Err(ParticleError::IndexOutOfBounds { index, len });
        }
        Ok(ParticleState {
            id: self.id[index],
            population_id: self.population_id[index].clone(),
            origin: self.origin[index].clone(),
            birth_time: self.birth_time[index],
            longitude_degrees: self.longitude_degrees[index],
            latitude_degrees: self.latitude_degrees[index],
            height_asl_m: self.height_asl_m[index],
            integration_offset_ns: self.integration_offset_ns[index],
            elapsed_age_ns: self.elapsed_age_ns[index],
            dry_air_mass_kg: self.dry_air_mass_kg[index],
            mass_kg: self
                .mass
                .mass_kg
                .iter()
                .map(|(substance, values)| (substance.clone(), values[index]))
                .collect(),
            sensitivity_weight: self.sensitivity_weight[index],
            status: self.status[index].clone(),
            termination: self.termination[index].clone(),
        })
    }

    /// Replaces one particle while preserving the substance-major layout.
    pub fn set_state(&mut self, index: usize, state: ParticleState) -> Result<(), ParticleError> {
        let len = self.len()?;
        if index >= len {
            return Err(ParticleError::IndexOutOfBounds { index, len });
        }
        state.validate()?;
        // Integration and boundary hot paths replace a row while preserving
        // its stable identity. Scanning the complete ID column for every such
        // update turns a linear particle step into quadratic work. A duplicate
        // can only be introduced here when the replacement changes the ID.
        if state.id != self.id[index]
            && self
                .id
                .iter()
                .enumerate()
                .any(|(other, id)| other != index && *id == state.id)
        {
            return Err(ParticleError::DuplicateParticleId(state.id));
        }

        let all_substances = self
            .mass
            .mass_kg
            .keys()
            .cloned()
            .chain(state.mass_kg.keys().cloned())
            .collect::<BTreeSet<_>>();
        for substance in all_substances {
            let column = self
                .mass
                .mass_kg
                .entry(substance.clone())
                .or_insert_with(|| vec![0.0; len]);
            column[index] = state.mass_kg.get(&substance).copied().unwrap_or(0.0);
        }

        self.id[index] = state.id;
        self.population_id[index] = state.population_id;
        self.origin[index] = state.origin;
        self.birth_time[index] = state.birth_time;
        self.longitude_degrees[index] = state.longitude_degrees;
        self.latitude_degrees[index] = state.latitude_degrees;
        self.height_asl_m[index] = state.height_asl_m;
        self.integration_offset_ns[index] = state.integration_offset_ns;
        self.elapsed_age_ns[index] = state.elapsed_age_ns;
        self.dry_air_mass_kg[index] = state.dry_air_mass_kg;
        self.sensitivity_weight[index] = state.sensitivity_weight;
        self.status[index] = state.status;
        self.termination[index] = state.termination;
        Ok(())
    }

    /// Selects a stable ordered subset without changing particle identities.
    ///
    /// This is used for lifecycle-only output events: a dynamic birth or
    /// termination writes the affected particles, not a duplicate full-domain
    /// snapshot. Duplicate or out-of-range indices are rejected by the same
    /// structural validation as any other batch.
    pub fn select_indices(&self, indices: &[usize]) -> Result<Self, ParticleError> {
        let len = self.len()?;
        if let Some(index) = indices.iter().copied().find(|index| *index >= len) {
            return Err(ParticleError::IndexOutOfBounds { index, len });
        }
        let selected = Self {
            id: indices.iter().map(|index| self.id[*index]).collect(),
            population_id: indices
                .iter()
                .map(|index| self.population_id[*index].clone())
                .collect(),
            origin: indices
                .iter()
                .map(|index| self.origin[*index].clone())
                .collect(),
            birth_time: indices
                .iter()
                .map(|index| self.birth_time[*index])
                .collect(),
            longitude_degrees: indices
                .iter()
                .map(|index| self.longitude_degrees[*index])
                .collect(),
            latitude_degrees: indices
                .iter()
                .map(|index| self.latitude_degrees[*index])
                .collect(),
            height_asl_m: indices
                .iter()
                .map(|index| self.height_asl_m[*index])
                .collect(),
            integration_offset_ns: indices
                .iter()
                .map(|index| self.integration_offset_ns[*index])
                .collect(),
            elapsed_age_ns: indices
                .iter()
                .map(|index| self.elapsed_age_ns[*index])
                .collect(),
            dry_air_mass_kg: indices
                .iter()
                .map(|index| self.dry_air_mass_kg[*index])
                .collect(),
            sensitivity_weight: indices
                .iter()
                .map(|index| self.sensitivity_weight[*index])
                .collect(),
            status: indices
                .iter()
                .map(|index| self.status[*index].clone())
                .collect(),
            termination: indices
                .iter()
                .map(|index| self.termination[*index].clone())
                .collect(),
            mass: SubstanceMassStore {
                mass_kg: self
                    .mass
                    .mass_kg
                    .iter()
                    .map(|(substance, values)| {
                        (
                            substance.clone(),
                            indices.iter().map(|index| values[*index]).collect(),
                        )
                    })
                    .collect(),
            },
        };
        selected.validate()?;
        Ok(selected)
    }

    /// Appends another validated batch, rejecting stable-ID collisions.
    pub fn append(&mut self, other: ParticleBatch) -> Result<(), ParticleError> {
        let original_len = self.validate()?;
        let added_len = other.validate()?;
        let mut ids = self.id.iter().copied().collect::<BTreeSet<_>>();
        if let Some(duplicate) = other.id.iter().copied().find(|id| !ids.insert(*id)) {
            return Err(ParticleError::DuplicateParticleId(duplicate));
        }

        let all_substances = self
            .mass
            .mass_kg
            .keys()
            .cloned()
            .chain(other.mass.mass_kg.keys().cloned())
            .collect::<BTreeSet<_>>();
        for substance in all_substances {
            self.mass
                .mass_kg
                .entry(substance.clone())
                .or_insert_with(|| vec![0.0; original_len]);
            let incoming = other
                .mass
                .mass_kg
                .get(&substance)
                .cloned()
                .unwrap_or_else(|| vec![0.0; added_len]);
            self.mass
                .mass_kg
                .get_mut(&substance)
                .ok_or(ParticleError::LengthMismatch)?
                .extend(incoming);
        }

        self.id.extend(other.id);
        self.population_id.extend(other.population_id);
        self.origin.extend(other.origin);
        self.birth_time.extend(other.birth_time);
        self.longitude_degrees.extend(other.longitude_degrees);
        self.latitude_degrees.extend(other.latitude_degrees);
        self.height_asl_m.extend(other.height_asl_m);
        self.integration_offset_ns
            .extend(other.integration_offset_ns);
        self.elapsed_age_ns.extend(other.elapsed_age_ns);
        self.dry_air_mass_kg.extend(other.dry_air_mass_kg);
        self.sensitivity_weight.extend(other.sensitivity_weight);
        self.status.extend(other.status);
        self.termination.extend(other.termination);
        self.validate()?;
        Ok(())
    }

    /// Returns true when every SoA and substance column is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
            && self.population_id.is_empty()
            && self.origin.is_empty()
            && self.birth_time.is_empty()
            && self.longitude_degrees.is_empty()
            && self.latitude_degrees.is_empty()
            && self.height_asl_m.is_empty()
            && self.integration_offset_ns.is_empty()
            && self.elapsed_age_ns.is_empty()
            && self.dry_air_mass_kg.is_empty()
            && self.sensitivity_weight.is_empty()
            && self.status.is_empty()
            && self.termination.is_empty()
            && self.mass.mass_kg.values().all(Vec::is_empty)
    }
}

/// Particle storage failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParticleError {
    /// SoA or substance columns have inconsistent lengths.
    LengthMismatch,
    /// Particle identifiers are not unique.
    DuplicateParticleId(ParticleId),
    /// A coordinate is non-finite or latitude is outside the physical range.
    InvalidCoordinate,
    /// A carrier or substance mass is negative or non-finite.
    InvalidMass,
    /// A separate sensitivity weight is non-finite.
    InvalidSensitivityWeight,
    /// Termination metadata is non-finite, outside the local step, or attached
    /// to an active particle.
    InvalidTermination,
    /// Requested particle index is outside the batch.
    IndexOutOfBounds {
        /// Requested index.
        index: usize,
        /// Particle count.
        len: usize,
    },
}

fn stable_particle_id(domain_separator: &[u8], strings: &[&str], integers: &[u64]) -> ParticleId {
    let mut hasher = Sha256::new();
    hasher.update(domain_separator);
    for value in strings {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    for value in integers {
        hasher.update(value.to_be_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    ParticleId(u64::from_be_bytes(bytes) & i64::MAX as u64)
}

impl ParticleError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::LengthMismatch => "particle.length_mismatch",
            Self::DuplicateParticleId(_) => "particle.duplicate_id",
            Self::InvalidCoordinate => "particle.invalid_coordinate",
            Self::InvalidMass => "particle.invalid_mass",
            Self::InvalidSensitivityWeight => "particle.invalid_sensitivity_weight",
            Self::InvalidTermination => "particle.invalid_termination",
            Self::IndexOutOfBounds { .. } => "particle.index_out_of_bounds",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn stable_ids_are_order_independent_distinct_and_sqlite_safe() {
        let population = PopulationId("p".into());
        let event = ReleaseEventId("e".into());
        let domain = DomainId("d".into());
        let release = ParticleId::for_release(&population, &event, 1);
        assert_eq!(release, ParticleId::for_release(&population, &event, 1));
        assert_ne!(release, ParticleId::for_release(&population, &event, 2));
        assert_ne!(
            release,
            ParticleId::for_domain_initial(&population, &domain, 1)
        );
        assert_ne!(
            ParticleId::for_domain_boundary(&population, &domain, BoundaryFaceId(4), 5, 6,),
            ParticleId::for_domain_boundary(&population, &domain, BoundaryFaceId(4), 5, 7,)
        );
        assert!(release.0 <= i64::MAX as u64);
    }

    #[test]
    fn owned_particle_state_preserves_substance_major_mass() {
        let population = PopulationId("p".into());
        let event = ReleaseEventId("e".into());
        let substance = SubstanceId("tracer".into());
        let batch = ParticleBatch {
            id: vec![ParticleId::for_release(&population, &event, 0)],
            population_id: vec![population],
            origin: vec![ParticleOrigin::Release { event_id: event }],
            birth_time: vec![Timestamp::UNIX_EPOCH],
            longitude_degrees: vec![0.0],
            latitude_degrees: vec![0.0],
            height_asl_m: vec![100.0],
            integration_offset_ns: vec![0],
            elapsed_age_ns: vec![0],
            dry_air_mass_kg: vec![1.0],
            sensitivity_weight: vec![None],
            status: vec![ParticleStatus::Alive],
            termination: vec![None],
            mass: SubstanceMassStore {
                mass_kg: BTreeMap::from([(substance.clone(), vec![2.0])]),
            },
        };
        let state = batch.state(0).unwrap();
        assert_eq!(state.mass_kg.get(&substance), Some(&2.0));
        state.validate().unwrap();
    }

    #[test]
    fn set_state_preserves_stable_id_and_rejects_changed_duplicate() {
        let population = PopulationId("p".into());
        let event = ReleaseEventId("e".into());
        let first_id = ParticleId::for_release(&population, &event, 0);
        let second_id = ParticleId::for_release(&population, &event, 1);
        let mut batch = ParticleBatch {
            id: vec![first_id, second_id],
            population_id: vec![population.clone(), population],
            origin: vec![
                ParticleOrigin::Release {
                    event_id: event.clone(),
                },
                ParticleOrigin::Release { event_id: event },
            ],
            birth_time: vec![Timestamp::UNIX_EPOCH; 2],
            longitude_degrees: vec![0.0, 1.0],
            latitude_degrees: vec![0.0; 2],
            height_asl_m: vec![100.0; 2],
            integration_offset_ns: vec![0; 2],
            elapsed_age_ns: vec![0; 2],
            dry_air_mass_kg: vec![1.0; 2],
            sensitivity_weight: vec![None; 2],
            status: vec![ParticleStatus::Alive; 2],
            termination: vec![None; 2],
            mass: SubstanceMassStore::default(),
        };

        let mut same_identity = batch.state(0).unwrap();
        same_identity.longitude_degrees = 2.0;
        batch.set_state(0, same_identity).unwrap();
        assert_eq!(batch.id, vec![first_id, second_id]);
        assert_eq!(batch.longitude_degrees[0], 2.0);

        let mut duplicate_identity = batch.state(0).unwrap();
        duplicate_identity.id = second_id;
        assert_eq!(
            batch.set_state(0, duplicate_identity),
            Err(ParticleError::DuplicateParticleId(second_id))
        );
    }
}
