//! # Contract: document metadata
//!
//! Metadata is descriptive and never changes scientific execution semantics.
//!
//! ## Fields
//!
//! | Field | Type | Notes |
//! |---|---|---|
//! | `name` | `String` | Human-readable unique name |
//! | `description` | `Option<String>` | Optional longer text |
//! | `labels` | `BTreeMap<String,String>` | Deterministic labels |
//! | `authors` | `Vec<String>` | Optional people/orgs |

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Descriptive metadata shared by Case and RunProfile documents.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// Human-readable unique name in the current working set.
    #[serde(default)]
    pub name: String,
    /// Optional longer description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Deterministically ordered labels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Optional author or organization names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
}
