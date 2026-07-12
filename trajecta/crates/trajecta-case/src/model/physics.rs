//! # Contract: named physics modules
//!
//! Physics is selected by stable model identifiers and explicit parameters.
//! The Case may describe a module before an executor exists; validation intent
//! determines whether that makes a requested operation unrunnable.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Stable identifier for an algorithm or pluggable model.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ModelId(pub String);

/// Declarative configuration of one physics module.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsModuleSpec {
    /// Selected implementation identifier.
    pub model: ModelId,
    /// Whether the module participates in the run.
    pub enabled: bool,
    /// Explicit string-valued parameters interpreted by the named model.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
}
