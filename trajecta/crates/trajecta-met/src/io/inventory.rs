//! # Contract: validated meteorology inventory
//!
//! Inventory construction validates locked files, exact profile selection,
//! required capabilities, temporal coverage, grid and vertical signatures,
//! missing source data, and nested-domain relationships before query runtime.

use std::collections::BTreeMap;
use std::path::PathBuf;

use trajecta_case::lockfile::{GridSignature, VerticalSignature};
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;

use crate::field::CapabilitySet;

/// Stable identity of one logical meteorological frame.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LogicalFrameId {
    /// Domain containing the frame.
    pub domain: DomainId,
    /// Physical validity time.
    pub valid_time: Timestamp,
    /// Exact profile content hash.
    pub profile_sha256: String,
    /// Aggregate hash of frame files and roles.
    pub content_sha256: String,
}

/// Exact local files and metadata forming one logical frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameDescriptor {
    /// Stable frame identity.
    pub id: LogicalFrameId,
    /// Deterministic file-role mapping.
    pub files: BTreeMap<String, PathBuf>,
    /// Verified horizontal grid signature.
    pub grid: GridSignature,
    /// Verified native vertical signature.
    pub vertical: VerticalSignature,
}

/// Time coverage and gaps for one domain.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoverageReport {
    /// First available frame time.
    pub first: Option<Timestamp>,
    /// Last available frame time.
    pub last: Option<Timestamp>,
    /// Missing expected validity times.
    pub gaps: Vec<Timestamp>,
}

/// Validated frame catalog for one logical domain.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DomainCatalog {
    /// Domain identity.
    pub domain: Option<DomainId>,
    /// Frames by exact validity time.
    pub frames: BTreeMap<Timestamp, FrameDescriptor>,
    /// Verified coverage summary.
    pub coverage: CoverageReport,
}

/// Fully validated catalog used to construct a meteorology engine.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetCatalog {
    /// Domains by stable identifier.
    pub domains: BTreeMap<DomainId, DomainCatalog>,
    /// Capabilities guaranteed by all relevant frames and profiles.
    pub capabilities: CapabilitySet,
}

/// Inventory build failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryError {
    /// Inventory validation has not been implemented yet.
    NotImplemented,
    /// Locked local file is absent or changed.
    InvalidLockedFile(PathBuf),
    /// Requested time coverage contains gaps.
    TimeCoverage(CoverageReport),
    /// Grid signatures change within a logical domain.
    GridMismatch(DomainId),
    /// Vertical signatures change within a logical domain.
    VerticalMismatch(DomainId),
    /// Required capabilities are unavailable.
    MissingCapabilities,
}
