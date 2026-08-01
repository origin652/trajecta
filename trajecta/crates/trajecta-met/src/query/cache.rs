//! # Contract: byte-budgeted column and tile caches
//!
//! Caches are deterministic performance devices only. A hit or miss cannot
//! change values. Preparation may update LRU state and pin entries; execution
//! cannot change LRU order, evict pinned entries, or exceed the hard budget.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;

use crate::field::CapabilitySet;
use crate::grid::CellId;
use crate::io::inventory::LogicalFrameId;
use crate::query::output::TransportOutput;
use crate::query::request::{QueryBatch, TransportPlan, VerticalQuery};
use crate::vertical::ColumnStencil;

/// One-entry exact transport cache shared by windows from a live engine.
pub(crate) type LastTransportQueryCache = Arc<Mutex<Option<CachedTransportQuery>>>;

/// Pinned frame-window identity for exact transport reuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExactTransportWindowKey {
    /// Exact physical query time.
    pub(crate) query_time: Timestamp,
    /// Selected meteorology domain.
    pub(crate) domain: DomainId,
    /// Earlier pinned frame identity.
    pub(crate) before_frame: LogicalFrameId,
    /// Later pinned frame identity.
    pub(crate) after_frame: LogicalFrameId,
    /// Previous derivative-support frame identity on an exact-frame query.
    pub(crate) previous_frame: Option<LogicalFrameId>,
    /// Next derivative-support frame identity on an exact-frame query.
    pub(crate) next_frame: Option<LogicalFrameId>,
    /// IEEE-754 bits of the earlier-frame temporal weight.
    pub(crate) before_weight_bits: u64,
    /// IEEE-754 bits of the later-frame temporal weight.
    pub(crate) after_weight_bits: u64,
}

/// Bit-exact request identity. Coordinates use IEEE-754 bits so `-0.0` is not
/// silently merged with `+0.0`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExactTransportCacheKey {
    window: ExactTransportWindowKey,
    plan: TransportPlan,
    vertical_coordinate: VerticalQuery,
    longitude_bits: Vec<u64>,
    latitude_bits: Vec<u64>,
    vertical_bits: Vec<u64>,
}

impl ExactTransportCacheKey {
    pub(crate) fn new(
        window: ExactTransportWindowKey,
        plan: TransportPlan,
        batch: &QueryBatch,
    ) -> Self {
        Self {
            window,
            plan,
            vertical_coordinate: batch.vertical_coordinate,
            longitude_bits: batch
                .points
                .longitude_degrees
                .iter()
                .map(|value| value.to_bits())
                .collect(),
            latitude_bits: batch
                .points
                .latitude_degrees
                .iter()
                .map(|value| value.to_bits())
                .collect(),
            vertical_bits: batch
                .points
                .vertical
                .iter()
                .map(|value| value.to_bits())
                .collect(),
        }
    }

    /// Returns the cached row indices when this request is an exact ordered
    /// subset of a previous request with the same physical/plan identity.
    pub(crate) fn selection_from_cached(&self, cached: &Self) -> Option<Vec<usize>> {
        if self.window != cached.window
            || self.plan != cached.plan
            || self.vertical_coordinate != cached.vertical_coordinate
        {
            return None;
        }
        let requested_len = self.longitude_bits.len();
        if self.latitude_bits.len() != requested_len || self.vertical_bits.len() != requested_len {
            return None;
        }
        let cached_len = cached.longitude_bits.len();
        if cached.latitude_bits.len() != cached_len
            || cached.vertical_bits.len() != cached_len
            || requested_len > cached_len
        {
            return None;
        }
        let mut selection = Vec::with_capacity(requested_len);
        let mut cursor = 0;
        for requested in 0..requested_len {
            let mut found = None;
            while cursor < cached_len {
                if self.longitude_bits[requested] == cached.longitude_bits[cursor]
                    && self.latitude_bits[requested] == cached.latitude_bits[cursor]
                    && self.vertical_bits[requested] == cached.vertical_bits[cursor]
                {
                    found = Some(cursor);
                    cursor += 1;
                    break;
                }
                cursor += 1;
            }
            selection.push(found?);
        }
        Some(selection)
    }
}

/// Cached result for the immediately preceding exact transport request.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CachedTransportQuery {
    pub(crate) key: ExactTransportCacheKey,
    pub(crate) output: Arc<TransportOutput>,
}

/// Hard byte budget and fixed reserved resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryBudget {
    /// Total bytes available to the meteorology runtime.
    pub total_bytes: u64,
    /// Bytes reserved for source frames and fixed structures.
    pub fixed_bytes: u64,
}

impl MemoryBudget {
    /// Validates a total budget and its fixed frame/resource reservation.
    pub const fn new(total_bytes: u64, fixed_bytes: u64) -> Result<Self, CacheError> {
        if fixed_bytes > total_bytes {
            return Err(CacheError::InvalidBudget);
        }
        Ok(Self {
            total_bytes,
            fixed_bytes,
        })
    }

    /// Applies the M3 default 50% policy and an optional lower hard cap.
    pub const fn from_available_memory(
        available_bytes: u64,
        fixed_bytes: u64,
        hard_cap_bytes: Option<u64>,
    ) -> Result<Self, CacheError> {
        let default_total = available_bytes / 2;
        let total_bytes = match hard_cap_bytes {
            Some(cap) if cap < default_total => cap,
            Some(_) | None => default_total,
        };
        Self::new(total_bytes, fixed_bytes)
    }

    /// Returns bytes available to columns, workspaces, and chunk execution.
    #[must_use]
    pub const fn dynamic_bytes(self) -> u64 {
        self.total_bytes.saturating_sub(self.fixed_bytes)
    }
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

impl<T> CacheEntry<T> {
    /// Creates one measured immutable cache entry.
    #[must_use]
    pub const fn new(value: Arc<T>, size_bytes: u64) -> Self {
        Self { value, size_bytes }
    }
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

    /// Returns the measured resident size retained by this guard.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.entry.size_bytes
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

#[derive(Clone, Debug)]
struct ColumnCacheRecord {
    entry: Arc<CacheEntry<ColumnStencil>>,
    last_access: u64,
}

/// Byte-budgeted deterministic local-column LRU owner.
#[derive(Debug, Default)]
pub struct ColumnCache {
    budget_bytes: u64,
    entries: BTreeMap<CacheKey, ColumnCacheRecord>,
    access_clock: u64,
    metrics: CacheMetrics,
}

impl ColumnCache {
    /// Creates an empty column cache with a hard byte budget.
    #[must_use]
    pub const fn new(budget_bytes: u64) -> Self {
        Self {
            budget_bytes,
            entries: BTreeMap::new(),
            access_clock: 0,
            metrics: CacheMetrics {
                hits: 0,
                misses: 0,
                evictions: 0,
                resident_bytes: 0,
            },
        }
    }

    /// Returns the configured hard budget.
    #[must_use]
    pub const fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

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

    /// Pins one exact entry and updates preparation-time LRU state.
    pub fn get(&mut self, key: &CacheKey) -> Option<PinGuard<ColumnStencil>> {
        self.access_clock = self.access_clock.saturating_add(1);
        if let Some(record) = self.entries.get_mut(key) {
            record.last_access = self.access_clock;
            self.metrics.hits = self.metrics.hits.saturating_add(1);
            return Some(PinGuard::new(record.entry.clone()));
        }
        self.metrics.misses = self.metrics.misses.saturating_add(1);
        None
    }

    /// Inserts and pins one immutable column after transactional eviction planning.
    pub fn insert(
        &mut self,
        key: CacheKey,
        value: Arc<ColumnStencil>,
    ) -> Result<PinGuard<ColumnStencil>, CacheError> {
        if let Some(record) = self.entries.get(&key) {
            if record.entry.value.as_ref() == value.as_ref() {
                return Ok(PinGuard::new(record.entry.clone()));
            }
            return Err(CacheError::DuplicateKey);
        }
        let size_bytes = value.resident_bytes();
        if size_bytes > self.budget_bytes {
            return Err(CacheError::EntryTooLarge {
                entry_bytes: size_bytes,
                budget_bytes: self.budget_bytes,
            });
        }
        let required = self.metrics.resident_bytes.checked_add(size_bytes).ok_or(
            CacheError::InsufficientBudget {
                required_bytes: u64::MAX,
                budget_bytes: self.budget_bytes,
            },
        )?;
        if required > self.budget_bytes {
            let mut candidates = self
                .entries
                .iter()
                .filter(|(_, record)| Arc::strong_count(&record.entry) == 1)
                .map(|(key, record)| (record.last_access, key.clone(), record.entry.size_bytes))
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
            let mut projected = required;
            let mut evict = Vec::new();
            for (_, candidate, bytes) in candidates {
                if projected <= self.budget_bytes {
                    break;
                }
                projected = projected.saturating_sub(bytes);
                evict.push(candidate);
            }
            if projected > self.budget_bytes {
                return Err(CacheError::InsufficientBudget {
                    required_bytes: required,
                    budget_bytes: self.budget_bytes,
                });
            }
            for candidate in evict {
                if let Some(record) = self.entries.remove(&candidate) {
                    self.metrics.resident_bytes = self
                        .metrics
                        .resident_bytes
                        .saturating_sub(record.entry.size_bytes);
                    self.metrics.evictions = self.metrics.evictions.saturating_add(1);
                }
            }
        }
        self.access_clock = self.access_clock.saturating_add(1);
        let entry = Arc::new(CacheEntry::new(value, size_bytes));
        self.entries.insert(
            key,
            ColumnCacheRecord {
                entry: entry.clone(),
                last_access: self.access_clock,
            },
        );
        self.metrics.resident_bytes = self.metrics.resident_bytes.saturating_add(size_bytes);
        Ok(PinGuard::new(entry))
    }

    /// Invalidates every unpinned entry derived from one frame.
    pub fn invalidate_frame(&mut self, frame: &LogicalFrameId) -> Result<usize, CacheError> {
        if self
            .entries
            .iter()
            .any(|(key, record)| &key.frame == frame && Arc::strong_count(&record.entry) > 1)
        {
            return Err(CacheError::PinnedEntry);
        }
        let keys = self
            .entries
            .keys()
            .filter(|key| &key.frame == frame)
            .cloned()
            .collect::<Vec<_>>();
        let count = keys.len();
        for key in keys {
            if let Some(record) = self.entries.remove(&key) {
                self.metrics.resident_bytes = self
                    .metrics
                    .resident_bytes
                    .saturating_sub(record.entry.size_bytes);
            }
        }
        Ok(count)
    }

    /// Evicts unpinned entries in deterministic LRU order until at most the
    /// requested resident bytes remain.
    pub fn trim_unpinned_to(&mut self, target_bytes: u64) -> Result<usize, CacheError> {
        if self.metrics.resident_bytes <= target_bytes {
            return Ok(0);
        }
        let mut candidates = self
            .entries
            .iter()
            .filter(|(_, record)| Arc::strong_count(&record.entry) == 1)
            .map(|(key, record)| (record.last_access, key.clone()))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
        let mut removed = 0_usize;
        for (_, key) in candidates {
            if self.metrics.resident_bytes <= target_bytes {
                break;
            }
            if let Some(record) = self.entries.remove(&key) {
                self.metrics.resident_bytes = self
                    .metrics
                    .resident_bytes
                    .saturating_sub(record.entry.size_bytes);
                self.metrics.evictions = self.metrics.evictions.saturating_add(1);
                removed += 1;
            }
        }
        if self.metrics.resident_bytes > target_bytes {
            return Err(CacheError::InsufficientBudget {
                required_bytes: self.metrics.resident_bytes,
                budget_bytes: target_bytes,
            });
        }
        Ok(removed)
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
    /// Fixed reservation exceeds total memory budget.
    InvalidBudget,
    /// An entry is larger than the available hard budget.
    EntryTooLarge {
        /// Measured entry size.
        entry_bytes: u64,
        /// Available cache budget.
        budget_bytes: u64,
    },
    /// An attempted operation would evict a pinned entry.
    PinnedEntry,
    /// The same stable key was reused for different immutable content.
    DuplicateKey,
    /// Enough unpinned entries do not exist to satisfy the hard budget.
    InsufficientBudget {
        /// Bytes required before eviction.
        required_bytes: u64,
        /// Configured hard budget.
        budget_bytes: u64,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use trajecta_case::model::time::Timestamp;

    use super::*;

    fn frame_id() -> LogicalFrameId {
        LogicalFrameId {
            domain: DomainId("global".into()),
            valid_time: Timestamp::UNIX_EPOCH,
            profile_sha256: "profile".into(),
            content_sha256: "content".into(),
        }
    }

    fn key(cell: u64) -> CacheKey {
        CacheKey {
            frame: frame_id(),
            domain: DomainId("global".into()),
            cell: CellId(cell),
            capabilities: CapabilitySet::new(),
            algorithm: "column/v0".into(),
        }
    }

    fn column(offset: f64) -> Arc<ColumnStencil> {
        Arc::new(ColumnStencil::synthetic_for_cache(offset))
    }

    #[test]
    fn memory_budget_applies_default_fraction_and_lower_cap() {
        assert_eq!(
            MemoryBudget::from_available_memory(1_000, 100, None).unwrap(),
            MemoryBudget {
                total_bytes: 500,
                fixed_bytes: 100,
            }
        );
        assert_eq!(
            MemoryBudget::from_available_memory(1_000, 100, Some(300)).unwrap(),
            MemoryBudget {
                total_bytes: 300,
                fixed_bytes: 100,
            }
        );
        assert_eq!(MemoryBudget::new(99, 100), Err(CacheError::InvalidBudget));
    }

    #[test]
    fn column_cache_keeps_pins_and_uses_stable_lru_eviction() {
        let sample = column(0.0);
        let size = sample.resident_bytes();
        let mut cache = ColumnCache::new(size * 2);
        drop(cache.insert(key(0), sample).unwrap());
        drop(cache.insert(key(1), column(1.0)).unwrap());
        let pin0 = cache.get(&key(0)).unwrap();
        drop(cache.insert(key(2), column(2.0)).unwrap());
        assert!(cache.get(&key(1)).is_none());
        assert!(cache.get(&key(2)).is_some());
        assert_eq!(
            cache.invalidate_frame(&frame_id()),
            Err(CacheError::PinnedEntry)
        );
        drop(pin0);
        assert_eq!(cache.invalidate_frame(&frame_id()), Ok(2));
        assert_eq!(cache.metrics().resident_bytes, 0);
    }

    #[test]
    fn column_cache_grows_without_eviction_below_budget() {
        let size = column(0.0).resident_bytes();
        let mut cache = ColumnCache::new(size * 4);
        for cell in 0..3 {
            drop(cache.insert(key(cell), column(cell as f64)).unwrap());
        }

        assert_eq!(cache.len(), 3);
        assert_eq!(cache.metrics().resident_bytes, size * 3);
        assert_eq!(cache.metrics().evictions, 0);
    }

    #[test]
    fn cache_budget_failures_do_not_evict_pinned_entries_or_partially_insert() {
        let size = column(0.0).resident_bytes();
        let mut too_small = ColumnCache::new(size.saturating_sub(1));
        assert!(matches!(
            too_small.insert(key(0), column(0.0)),
            Err(CacheError::EntryTooLarge { .. })
        ));
        assert!(too_small.is_empty());
        assert_eq!(too_small.metrics().resident_bytes, 0);

        let mut cache = ColumnCache::new(size * 2);
        let first = cache.insert(key(0), column(0.0)).unwrap();
        let second = cache.insert(key(1), column(1.0)).unwrap();
        let before = cache.metrics();
        assert!(matches!(
            cache.insert(key(2), column(2.0)),
            Err(CacheError::InsufficientBudget { .. })
        ));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.metrics(), before);
        assert!(matches!(
            cache.trim_unpinned_to(size),
            Err(CacheError::InsufficientBudget { .. })
        ));
        drop((first, second));
        assert_eq!(cache.trim_unpinned_to(size).unwrap(), 1);
    }
}
