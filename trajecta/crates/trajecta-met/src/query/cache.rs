//! # Contract: byte-budgeted column and tile caches
//!
//! Caches are deterministic performance devices only. A hit or miss cannot
//! change values. Preparation may update LRU state and pin entries; execution
//! cannot change LRU order, evict pinned entries, or exceed the hard budget.

use std::collections::BTreeMap;
use std::sync::Arc;

use trajecta_case::model::meteorology::DomainId;

use crate::field::CapabilitySet;
use crate::grid::CellId;
use crate::io::inventory::LogicalFrameId;
use crate::vertical::ColumnGeometry;

/// Hard byte budget and fixed reserved resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryBudget {
    /// Total bytes available to the meteorology runtime.
    pub total_bytes: u64,
    /// Bytes reserved for source frames and fixed structures.
    pub fixed_bytes: u64,
}

/// Deterministic cache identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CacheKey {
    /// Source frame.
    pub frame: LogicalFrameId,
    /// Selected domain.
    pub domain: DomainId,
    /// Horizontal cell.
    pub cell: CellId,
    /// Capabilities represented by the entry.
    pub capabilities: CapabilitySet,
    /// Stable algorithm identifier and version.
    pub algorithm: String,
}

/// Immutable measured cache value.
#[derive(Clone, Debug)]
pub struct CacheEntry<T> {
    /// Shared immutable value.
    pub value: Arc<T>,
    /// Measured resident byte size.
    pub size_bytes: u64,
}

/// Guard that keeps an entry resident for the current prepared batch.
#[derive(Clone, Debug)]
pub struct PinGuard<T> {
    entry: Arc<CacheEntry<T>>,
}

impl<T> PinGuard<T> {
    /// Pins an immutable measured entry.
    #[must_use]
    pub const fn new(entry: Arc<CacheEntry<T>>) -> Self {
        Self { entry }
    }

    /// Returns the pinned value.
    #[must_use]
    pub fn value(&self) -> &T {
        &self.entry.value
    }
}

/// Observable deterministic cache counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheMetrics {
    /// Successful lookups.
    pub hits: u64,
    /// Missing lookups.
    pub misses: u64,
    /// Entries evicted during preparation.
    pub evictions: u64,
    /// Current resident bytes.
    pub resident_bytes: u64,
}

/// Byte-budgeted local-column LRU owner.
#[derive(Clone, Debug, Default)]
pub struct ColumnCache {
    entries: BTreeMap<CacheKey, Arc<CacheEntry<ColumnGeometry>>>,
    metrics: CacheMetrics,
}

impl ColumnCache {
    /// Returns current metrics without mutating LRU state.
    #[must_use]
    pub const fn metrics(&self) -> CacheMetrics {
        self.metrics
    }

    /// Returns the number of resident entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no entries are resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Immutable halo-derived tile placeholder.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DerivedTile {
    /// Flat derived values in backend-defined canonical tile order.
    pub values: Vec<f64>,
    /// Halo width in horizontal cells.
    pub halo_cells: usize,
}

/// Byte-budgeted derived-tile LRU owner.
#[derive(Clone, Debug, Default)]
pub struct TileCache {
    entries: BTreeMap<CacheKey, Arc<CacheEntry<DerivedTile>>>,
    metrics: CacheMetrics,
}

impl TileCache {
    /// Returns current metrics without mutating LRU state.
    #[must_use]
    pub const fn metrics(&self) -> CacheMetrics {
        self.metrics
    }

    /// Returns the number of resident entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no entries are resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Cache preparation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CacheError {
    /// Cache algorithms have not been implemented yet.
    NotImplemented,
    /// An entry is larger than the available hard budget.
    EntryTooLarge {
        /// Measured entry size.
        entry_bytes: u64,
        /// Available cache budget.
        budget_bytes: u64,
    },
    /// An attempted operation would evict a pinned entry.
    PinnedEntry,
}
