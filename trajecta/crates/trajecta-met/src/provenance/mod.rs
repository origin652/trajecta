//! # Contract: meteorological provenance
//!
//! Hot arrays store compact provenance identifiers; descriptive records are
//! deduplicated per frame and expanded only for diagnostics or explanation.

use std::collections::BTreeMap;

use crate::field::{FieldKey, FieldQuality};

/// Compact index into a frame-level provenance table.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProvenanceId(pub u32);

/// One unit conversion or named derivation step.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TransformRecord {
    /// Stable operation identifier.
    pub operation: String,
    /// Exact normalized parameters.
    pub parameters: Vec<(String, String)>,
}

/// Deduplicated scientific origin of a field array or sample.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProvenanceRecord {
    /// Output field.
    pub field: FieldKey,
    /// Source, derived, or estimated quality.
    pub quality: FieldQuality,
    /// Locked source identities.
    pub sources: Vec<String>,
    /// Deterministic transform chain.
    pub transforms: Vec<TransformRecord>,
    /// Optional explicit fallback reason.
    pub fallback_reason: Option<String>,
    /// Exact profile hash.
    pub profile_sha256: String,
}

/// Frame-level deduplicated provenance records.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProvenanceTable {
    records: Vec<ProvenanceRecord>,
    ids: BTreeMap<ProvenanceRecord, ProvenanceId>,
}

impl ProvenanceTable {
    /// Creates an empty table.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
            ids: BTreeMap::new(),
        }
    }

    /// Returns an existing ID or inserts one deterministic new record.
    pub fn intern(&mut self, record: ProvenanceRecord) -> Result<ProvenanceId, ProvenanceError> {
        if let Some(id) = self.ids.get(&record) {
            return Ok(*id);
        }
        let raw_id = u32::try_from(self.records.len())
            .map_err(|_| ProvenanceError::TooManyRecords(self.records.len()))?;
        let id = ProvenanceId(raw_id);
        self.records.push(record.clone());
        self.ids.insert(record, id);
        Ok(id)
    }

    /// Resolves one compact identifier.
    #[must_use]
    pub fn get(&self, id: ProvenanceId) -> Option<&ProvenanceRecord> {
        self.records.get(id.0 as usize)
    }
}

/// Provenance-table construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProvenanceError {
    /// More records exist than a compact identifier can represent.
    TooManyRecords(usize),
}
