//! # Contract: output products
//!
//! Outputs declare scientific products, event schedules, and typed sinks.
//! Particle-state output is instantaneous; M4 does not support averaging
//! particle trajectories.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::physics::ModelId;
use crate::model::time::Timestamp;
use crate::quantity::{Quantity, Time as TimeDimension};

/// Output sampling schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputSchedule {
    /// Save birth/start and end/termination states only.
    Endpoints,
    /// Save endpoints plus every aligned interval event.
    Interval {
        /// Positive interval between output events.
        interval: Quantity<TimeDimension>,
        /// Optional UTC alignment origin; simulation start is used when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<Timestamp>,
    },
}

/// Sink selection for one product.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSinkSpec {
    /// Stable sink implementation identifier.
    pub model: ModelId,
    /// Explicit sink-specific parameters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
}

/// One requested scientific output product.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputProductSpec {
    /// Stable scientific product identifier.
    pub product: ModelId,
    /// Exact event schedule.
    pub schedule: OutputSchedule,
    /// Selected representation sink.
    pub sink: OutputSinkSpec,
}

/// Stable particle-state product identifier.
pub const PARTICLE_STATE_PRODUCT_ID: &str = "particle_state/v1";

/// Stable SQLite particle-state sink identifier.
pub const PARTICLE_STATE_SQLITE_SINK_ID: &str = "particle_state_sqlite/v1";

/// Returns the output injected when a Case omits `outputs` entirely.
#[must_use]
pub fn default_particle_state_output() -> OutputProductSpec {
    OutputProductSpec {
        product: ModelId(PARTICLE_STATE_PRODUCT_ID.into()),
        schedule: OutputSchedule::Endpoints,
        sink: OutputSinkSpec {
            model: ModelId(PARTICLE_STATE_SQLITE_SINK_ID.into()),
            parameters: BTreeMap::new(),
        },
    }
}
