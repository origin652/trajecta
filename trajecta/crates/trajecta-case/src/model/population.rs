//! # Contract: particle-population intent
//!
//! Population configuration selects one complete lifecycle strategy. Release
//! events are explicit and portable; domain filling names exactly one
//! meteorological domain and exactly one target-count convention.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::meteorology::DomainId;
use crate::model::substance::SubstanceId;
use crate::model::time::Timestamp;
use crate::quantity::{Length, Mass, Pressure, Quantity};

/// Stable identifier for a particle population.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PopulationId(pub String);

/// Stable identifier for one explicit release event.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReleaseEventId(pub String);

/// Supported GeoJSON geometry values for a release event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "coordinates", deny_unknown_fields)]
pub enum GeoJsonGeometry {
    /// One longitude/latitude coordinate.
    Point([f64; 2]),
    /// Multiple equally weighted points.
    MultiPoint(Vec<[f64; 2]>),
    /// One geodesic polyline.
    LineString(Vec<[f64; 2]>),
    /// Multiple geodesic polylines.
    MultiLineString(Vec<Vec<[f64; 2]>>),
    /// One polygon encoded as exterior ring followed by optional holes.
    Polygon(Vec<Vec<[f64; 2]>>),
    /// Multiple polygons, each encoded as exterior ring plus optional holes.
    MultiPolygon(Vec<Vec<Vec<[f64; 2]>>>),
}

/// Inline or local-file GeoJSON source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum GeoJsonSource {
    /// Geometry is embedded directly in the Case.
    Inline {
        /// Typed GeoJSON geometry.
        geometry: GeoJsonGeometry,
    },
    /// Geometry is loaded from a local `.geojson` file relative to the Case.
    File {
        /// Relative local path; remote URLs are rejected during resolution.
        path: PathBuf,
    },
}

/// Vertical release coordinate and optional uniform interval.
///
/// `upper = None` denotes a fixed value. `upper = Some(value)` denotes a
/// uniform distribution over the closed numeric interval in the named SI
/// coordinate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "coordinate", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReleaseVerticalSpec {
    /// Geometric height above mean sea level.
    AboveSeaLevel {
        /// Fixed value or lower interval bound.
        lower: Quantity<Length>,
        /// Optional upper interval bound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upper: Option<Quantity<Length>>,
    },
    /// Geometric height above local terrain.
    AboveGround {
        /// Fixed value or lower interval bound.
        lower: Quantity<Length>,
        /// Optional upper interval bound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upper: Option<Quantity<Length>>,
    },
    /// Atmospheric pressure in pascals.
    Pressure {
        /// Fixed value or lower interval bound.
        lower: Quantity<Pressure>,
        /// Optional upper interval bound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        upper: Option<Quantity<Pressure>>,
    },
}

/// One exact ordinary-release event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEventSpec {
    /// Stable identity within the population.
    pub id: ReleaseEventId,
    /// Inclusive physical start time.
    pub start: Timestamp,
    /// Inclusive physical end time; equal to `start` for an instantaneous event.
    pub end: Timestamp,
    /// Exact number of particles allocated by this event.
    pub particle_count: u64,
    /// Total released mass by substance, divided equally over event particles.
    pub mass: BTreeMap<SubstanceId, Quantity<Mass>>,
    /// Horizontal release geometry.
    pub geometry: GeoJsonSource,
    /// Vertical release coordinate and distribution.
    pub vertical: ReleaseVerticalSpec,
}

/// Release-driven population configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseDrivenSpec {
    /// Population identifier.
    pub id: PopulationId,
    /// Explicit events in declarative order; runtime ordering is deterministic.
    pub events: Vec<ReleaseEventSpec>,
}

/// Dry-air domain-filling configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainFillAirMassSpec {
    /// Population identifier.
    pub id: PopulationId,
    /// Single meteorological domain whose safe core is filled.
    pub domain_id: DomainId,
    /// Optional dry-air carrier mass represented by one particle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_dry_air_mass_per_particle: Option<Quantity<Mass>>,
    /// Optional exact initial particle count when mass-per-particle is omitted.
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
    /// Substance column that receives assigned ozone mass.
    pub ozone_substance: SubstanceId,
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
