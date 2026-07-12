//! # Contract: substances
//!
//! Substances have stable identifiers and explicit properties. Property keys
//! are part of the selected physics-module contract; this layer does not infer
//! missing chemistry or deposition behavior.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::quantity::QuantityInput;

/// Stable substance identifier used in particle mass arrays.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SubstanceId(pub String);

/// Declarative substance metadata and physical properties.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubstanceSpec {
    /// Stable identifier.
    pub id: SubstanceId,
    /// Human-readable name.
    pub display_name: String,
    /// Explicit named quantities consumed by physics modules.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, QuantityInput>,
}
