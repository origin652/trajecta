//! # Contract: backend-neutral batch layout
//!
//! Layout groups points by selected domain, horizontal cell, and derived tile,
//! while retaining exact forward and inverse permutations. Automatic chunking
//! obeys a hard memory budget and cannot change numerical results.

use trajecta_case::model::meteorology::DomainId;

use crate::grid::CellId;

/// Mapping between caller order and internal grouped order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Permutation {
    /// Internal index to original caller index.
    pub forward: Vec<usize>,
    /// Original caller index to internal index.
    pub inverse: Vec<usize>,
}

/// Contiguous range of points assigned to one selected domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainGroup {
    /// Selected domain.
    pub domain: DomainId,
    /// Inclusive start in internal order.
    pub start: usize,
    /// Exclusive end in internal order.
    pub end: usize,
}

/// Contiguous range assigned to one horizontal cell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CellGroup {
    /// Horizontal cell.
    pub cell: CellId,
    /// Inclusive start in internal order.
    pub start: usize,
    /// Exclusive end in internal order.
    pub end: usize,
}

/// Contiguous range sharing one halo-derived tile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TileGroup {
    /// Stable tile identifier within the selected backend.
    pub tile_id: u64,
    /// Inclusive start in internal order.
    pub start: usize,
    /// Exclusive end in internal order.
    pub end: usize,
}

/// Deterministic execution chunk boundaries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkPlan {
    /// Monotonic boundaries including zero and total point count.
    pub boundaries: Vec<usize>,
}

/// Complete backend-neutral grouping result.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BatchLayout {
    /// Domain ID per internal point.
    pub domain_ids: Vec<DomainId>,
    /// Cell ID per internal point.
    pub cell_ids: Vec<CellId>,
    /// Domain groups.
    pub domain_groups: Vec<DomainGroup>,
    /// Cell groups.
    pub cell_groups: Vec<CellGroup>,
    /// Tile groups.
    pub tile_groups: Vec<TileGroup>,
    /// Caller/internal order mapping.
    pub permutation: Permutation,
    /// Hard-budget chunking plan.
    pub chunks: ChunkPlan,
}

/// Layout construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// Layout and chunking algorithms have not been implemented yet.
    NotImplemented,
    /// No domain safely covers one or more points.
    OutOfDomain(Vec<usize>),
    /// A single required chunk exceeds the hard memory budget.
    InsufficientMemory,
}
