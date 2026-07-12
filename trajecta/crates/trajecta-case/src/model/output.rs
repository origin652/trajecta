//! # Contract: output products
//!
//! Outputs declare scientific products, schedules, and encoders. Encoding and
//! file I/O remain execution-layer responsibilities.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::physics::ModelId;
use crate::quantity::{Quantity, Time as TimeDimension};

/// Output sampling schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSchedule {
    /// Interval between output events.
    pub interval: Quantity<TimeDimension>,
    /// Optional averaging interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub averaging_interval: Option<Quantity<TimeDimension>>,
}

/// Encoder selection for one product.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncoderSpec {
    /// Stable encoder identifier.
    pub model: ModelId,
    /// Explicit encoder parameters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
}

/// One requested scientific output product.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputProductSpec {
    /// Stable product identifier.
    pub product: ModelId,
    /// Sampling and averaging schedule.
    pub schedule: OutputSchedule,
    /// Selected representation.
    pub encoder: EncoderSpec,
}
