//! # Contract: logical meteorology domains
//!
//! A Case names logical datasets and nested domains. It does not contain
//! machine-local paths; those are supplied by a RunProfile dataset binding.
//!
//! ## Fields
//!
//! | Type | Fields |
//! |---|---|
//! | [`DomainId`] | stable string id |
//! | [`DatasetRef`] | logical dataset id |
//! | [`DomainSpec`] | id, dataset, priority, parent, halo |
//! | [`MeteorologySpec`] | domains[] |

use serde::{Deserialize, Serialize};

/// Stable logical domain identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DomainId(pub String);

/// Stable logical dataset identifier resolved by a RunProfile.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct DatasetRef(pub String);

/// One logical meteorological domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainSpec {
    /// Stable domain identifier.
    pub id: DomainId,
    /// Logical dataset used by this domain.
    pub dataset: DatasetRef,
    /// Larger values are preferred when multiple domains cover a point.
    pub priority: i32,
    /// Optional containing domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<DomainId>,
    /// Required safe interpolation halo in horizontal grid cells.
    pub horizontal_halo_cells: u32,
}

/// Complete logical meteorology selection for a Case.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeteorologySpec {
    /// Candidate domains in declarative order.
    #[serde(default)]
    pub domains: Vec<DomainSpec>,
}
