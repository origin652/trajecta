//! # Contract: production I/O call counters
//!
//! Arc-shared atomic counters for inspect, index, decode, frame load, and
//! format-detection open attempts at real reader/provider entry points.
//!
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    static ACTIVE_IO_COUNTERS: RefCell<Option<Arc<IoCallCounters>>> = const { RefCell::new(None) };
}

/// Install counters for the current thread (ordinary production entry points).
pub fn install_io_counters(counters: Arc<IoCallCounters>) {
    ACTIVE_IO_COUNTERS.with(|slot| {
        *slot.borrow_mut() = Some(counters);
    });
}

/// Clear the current-thread instrumentation context.
pub fn clear_io_counters() {
    ACTIVE_IO_COUNTERS.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Returns the current-thread counters, if installed.
#[must_use]
pub fn active_io_counters() -> Option<Arc<IoCallCounters>> {
    ACTIVE_IO_COUNTERS.with(|slot| slot.borrow().clone())
}

/// Atomic counters for meteorological source I/O.
#[derive(Debug, Default)]
pub struct IoCallCounters {
    inspect: AtomicU64,
    build_index: AtomicU64,
    decode: AtomicU64,
    provider_frame_load: AtomicU64,
    /// Format-detection open attempts only (not a complete OS open tally).
    format_detection_open_attempt: AtomicU64,
}

impl IoCallCounters {
    /// Creates a zeroed counter set.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Immutable snapshot of all counters.
    #[must_use]
    pub fn snapshot(&self) -> IoCallSnapshot {
        IoCallSnapshot {
            inspect: self.inspect.load(Ordering::Relaxed),
            build_index: self.build_index.load(Ordering::Relaxed),
            decode: self.decode.load(Ordering::Relaxed),
            provider_frame_load: self.provider_frame_load.load(Ordering::Relaxed),
            format_detection_open_attempt: self
                .format_detection_open_attempt
                .load(Ordering::Relaxed),
        }
    }

    /// Records one metadata inspect call (success or failure).
    pub fn record_inspect(&self) {
        self.inspect.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one index build call (success or failure).
    pub fn record_build_index(&self) {
        self.build_index.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one field decode call (success or failure).
    pub fn record_decode(&self) {
        self.decode.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one logical frame publish/load attempt through the provider path.
    pub fn record_provider_frame_load(&self) {
        self.provider_frame_load.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one format-detection open attempt (not every OS open).
    pub fn record_format_detection_open_attempt(&self) {
        self.format_detection_open_attempt
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Deprecated alias kept for transitional call sites.
    #[deprecated(note = "use record_format_detection_open_attempt")]
    pub fn record_filesystem_open(&self) {
        self.record_format_detection_open_attempt();
    }
}

/// Immutable counter values at one instant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IoCallSnapshot {
    /// `MetReader::inspect` invocations.
    pub inspect: u64,
    /// `MetReader::build_index` invocations.
    pub build_index: u64,
    /// `MetReader::decode` invocations.
    pub decode: u64,
    /// Logical frame load attempts through `FrameLoader`.
    pub provider_frame_load: u64,
    /// Format detection open attempts (not total filesystem opens).
    pub format_detection_open_attempt: u64,
}

impl IoCallSnapshot {
    /// Total counted I/O operations.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.inspect
            .saturating_add(self.build_index)
            .saturating_add(self.decode)
            .saturating_add(self.provider_frame_load)
            .saturating_add(self.format_detection_open_attempt)
    }

    /// Component-wise difference (`after - before`) using saturating subtraction.
    #[must_use]
    pub const fn saturating_sub(self, before: Self) -> Self {
        Self {
            inspect: self.inspect.saturating_sub(before.inspect),
            build_index: self.build_index.saturating_sub(before.build_index),
            decode: self.decode.saturating_sub(before.decode),
            provider_frame_load: self
                .provider_frame_load
                .saturating_sub(before.provider_frame_load),
            format_detection_open_attempt: self
                .format_detection_open_attempt
                .saturating_sub(before.format_detection_open_attempt),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_independent_and_atomic_increments_accumulate() {
        let counters = IoCallCounters::new();
        let before = counters.snapshot();
        assert_eq!(before.total(), 0);
        counters.record_inspect();
        counters.record_build_index();
        counters.record_decode();
        counters.record_provider_frame_load();
        counters.record_format_detection_open_attempt();
        let after = counters.snapshot();
        assert_eq!(after.inspect, 1);
        assert_eq!(after.build_index, 1);
        assert_eq!(after.decode, 1);
        assert_eq!(after.provider_frame_load, 1);
        assert_eq!(after.format_detection_open_attempt, 1);
        assert_eq!(after.saturating_sub(before).total(), 5);
    }
}
