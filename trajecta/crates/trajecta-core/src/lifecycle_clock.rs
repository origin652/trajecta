//! # Contract: wall-clock lifecycle timestamps
//!
//! Provides UTC timestamps used only for run-manifest started/finished fields.

use std::time::{SystemTime, UNIX_EPOCH};

use trajecta_case::model::time::Timestamp;

use crate::runner::LifecycleClock;

/// UTC system clock used only for manifest started/finished timestamps.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLifecycleClock;

impl LifecycleClock for SystemLifecycleClock {
    fn now(&mut self) -> Result<Timestamp, String> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?;
        let seconds = i64::try_from(duration.as_secs()).map_err(|error| error.to_string())?;
        Timestamp::new(seconds, duration.subsec_nanos()).map_err(|error| format!("{error:?}"))
    }
}

/// Deterministic test clock.
#[derive(Clone, Debug)]
pub struct FixedLifecycleClock {
    /// Timestamp returned by every call.
    pub timestamp: Timestamp,
}

impl LifecycleClock for FixedLifecycleClock {
    fn now(&mut self) -> Result<Timestamp, String> {
        Ok(self.timestamp)
    }
}
