//! # Contract: immutable meteorological frames and prepared windows
//!
//! Decoded frames are immutable after construction. A prepared window pins two
//! frames from one selected domain for the entire query. Query threads cannot
//! swap frames or trigger I/O, and run direction cannot change values at an
//! identical physical instant.

use std::collections::BTreeMap;
use std::sync::Arc;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;

use crate::field::FieldKey;
use crate::io::inventory::LogicalFrameId;
use crate::vertical::VerticalTopology;

/// Structure-of-arrays storage for immutable decoded source fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawFieldStore {
    fields: BTreeMap<FieldKey, Arc<[f64]>>,
}

impl RawFieldStore {
    /// Creates an empty store during frame assembly.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    /// Inserts one unique decoded field before frame publication.
    pub fn insert(&mut self, key: FieldKey, values: Arc<[f64]>) -> Result<(), FrameError> {
        if self.fields.contains_key(&key) {
            return Err(FrameError::DuplicateField(key));
        }
        self.fields.insert(key, values);
        Ok(())
    }

    /// Returns an immutable field array.
    #[must_use]
    pub fn get(&self, key: &FieldKey) -> Option<&Arc<[f64]>> {
        self.fields.get(key)
    }
}

/// Identity and topology shared by all fields in one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameMetadata {
    /// Stable logical frame identity.
    pub id: LogicalFrameId,
    /// Selected domain.
    pub domain: DomainId,
    /// Physical validity time.
    pub valid_time: Timestamp,
    /// Native vertical topology.
    pub vertical: VerticalTopology,
    /// Stable provenance-table identity.
    pub provenance_table_id: String,
}

/// Immutable, fully assembled source-field frame.
#[derive(Clone, Debug, PartialEq)]
pub struct RawMetFrame {
    metadata: FrameMetadata,
    fields: RawFieldStore,
}

impl RawMetFrame {
    /// Publishes a completed immutable frame.
    #[must_use]
    pub const fn new(metadata: FrameMetadata, fields: RawFieldStore) -> Self {
        Self { metadata, fields }
    }

    /// Returns immutable frame metadata.
    #[must_use]
    pub const fn metadata(&self) -> &FrameMetadata {
        &self.metadata
    }

    /// Returns immutable source fields.
    #[must_use]
    pub const fn fields(&self) -> &RawFieldStore {
        &self.fields
    }
}

/// Two frames that bracket one physical query time.
#[derive(Clone, Debug, PartialEq)]
pub struct FramePair {
    /// Earlier physical frame.
    pub before: Arc<RawMetFrame>,
    /// Later physical frame.
    pub after: Arc<RawMetFrame>,
}

/// Pinned frame pair and deterministic temporal weights.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedWindow {
    /// Frames from one selected domain.
    pub frames: FramePair,
    /// Weight assigned to the earlier frame.
    pub before_weight: f64,
    /// Weight assigned to the later frame.
    pub after_weight: f64,
}

/// Mutable owner of frame transitions and prefetch policy.
#[derive(Clone, Debug, Default)]
pub struct WindowManager;

impl WindowManager {
    /// Pins the pair required for one physical query time.
    pub fn prepare(&mut self, _time: Timestamp) -> Result<PreparedWindow, FrameError> {
        Err(FrameError::NotImplemented)
    }
}

/// Byte-budgeted immutable raw-frame cache.
#[derive(Clone, Debug, Default)]
pub struct FrameCache {
    /// Configured hard byte budget.
    pub budget_bytes: u64,
    /// Current resident bytes.
    pub resident_bytes: u64,
}

/// Frame assembly, selection, or cache failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// Frame/window algorithms have not been implemented yet.
    NotImplemented,
    /// A field key occurs twice during assembly.
    DuplicateField(FieldKey),
    /// No frame pair covers the physical query time.
    MissingCoverage(Timestamp),
    /// Bracketing frames come from different domains.
    MixedDomains,
    /// Hard frame-cache memory budget is insufficient.
    InsufficientMemory {
        /// Required bytes.
        required_bytes: u64,
        /// Configured bytes.
        available_bytes: u64,
    },
}
