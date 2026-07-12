//! # Contract: particle-population intent
//!
//! Population configuration selects one full lifecycle strategy. Release and
//! domain-filling strategies are not interchangeable mid-run.
//!
//! ## Fields
//!
//! | Variant / type | Fields |
//! |---|---|
//! | `ReleaseDriven` | id, schedule |
//! | `DomainFillAirMass` | id, exactly one of target mass/count |
//! | `DomainFillStratosphericOzone` | air_mass + non-empty ozone_rule |

use serde::{Deserialize, Serialize};

use crate::quantity::{Mass, Quantity};

/// Stable identifier for a particle population.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PopulationId(pub String);

/// Release-driven population configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseDrivenSpec {
    /// Population identifier.
    pub id: PopulationId,
    /// Stable release schedule identifier resolved by the execution layer.
    pub schedule: String,
}

/// Dry-air domain-filling configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainFillAirMassSpec {
    /// Population identifier.
    pub id: PopulationId,
    /// Optional target dry-air mass represented by one particle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_particle_mass: Option<Quantity<Mass>>,
    /// Optional target particle count when mass-per-particle is not specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_particle_count: Option<u64>,
}

/// Stratospheric-ozone domain-filling configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainFillStratosphericOzoneSpec {
    /// Shared dry-air population configuration.
    pub air_mass: DomainFillAirMassSpec,
    /// Stable identifier of the named ozone assignment rule.
    pub ozone_rule: String,
}

/// Complete particle-population strategy selected by a Case.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParticlePopulationSpec {
    /// Discrete scheduled releases.
    ReleaseDriven(ReleaseDrivenSpec),
    /// Domain filling weighted by dry-air mass.
    DomainFillAirMass(DomainFillAirMassSpec),
    /// Domain filling with a named stratospheric ozone rule.
    DomainFillStratosphericOzone(DomainFillStratosphericOzoneSpec),
}
