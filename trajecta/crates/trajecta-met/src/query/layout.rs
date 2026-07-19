//! # Contract: backend-neutral batch layout
//!
//! Layout groups points by selected domain, horizontal cell, derived tile, and
//! original index while retaining exact forward and inverse permutations.
//! Automatic chunking obeys a hard memory budget; chunk boundaries never alter
//! point order or numerical operation order within a sample.

use std::collections::BTreeSet;

use trajecta_case::model::meteorology::DomainId;

use crate::grid::CellId;

/// Mapping between caller order and internal grouped order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Permutation {
    /// Internal index to original caller index.
    pub forward: Vec<usize>,
    /// Original caller index to internal index.
    pub inverse: Vec<Option<usize>>,
}

impl Permutation {
    /// Scatters executable internal results into a prefilled caller-order slice.
    pub fn scatter_into<T: Clone>(
        &self,
        internal: &[T],
        output: &mut [T],
    ) -> Result<(), LayoutError> {
        if internal.len() != self.forward.len() {
            return Err(LayoutError::InvalidPermutation);
        }
        if output.len() != self.inverse.len() {
            return Err(LayoutError::InvalidPermutation);
        }
        for (internal_index, original_index) in self.forward.iter().copied().enumerate() {
            output[original_index] = internal[internal_index].clone();
        }
        Ok(())
    }
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

/// Frozen execution-memory estimate used to choose chunks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkMemoryModel {
    /// Fixed bytes retained for the complete prepared batch.
    pub fixed_bytes: u64,
    /// Additional scratch/output bytes required per point in one chunk.
    pub bytes_per_point: u64,
    /// Preferred maximum points per chunk; zero means budget capacity.
    pub preferred_chunk_points: usize,
}

/// Deterministic execution chunk boundaries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkPlan {
    /// Monotonic boundaries including zero and total point count.
    pub boundaries: Vec<usize>,
    /// Frozen hard budget used to create this plan.
    pub budget_bytes: u64,
    /// Maximum measured bytes for any planned chunk.
    pub maximum_chunk_bytes: u64,
}

impl ChunkPlan {
    /// Builds deterministic chunks that fit the declared hard budget.
    pub fn build(
        point_count: usize,
        budget_bytes: u64,
        memory: ChunkMemoryModel,
    ) -> Result<Self, LayoutError> {
        if memory.fixed_bytes > budget_bytes {
            return Err(LayoutError::InsufficientMemory {
                required_bytes: memory.fixed_bytes,
                budget_bytes,
            });
        }
        if point_count == 0 {
            return Ok(Self {
                boundaries: vec![0],
                budget_bytes,
                maximum_chunk_bytes: memory.fixed_bytes,
            });
        }
        let dynamic = budget_bytes - memory.fixed_bytes;
        let capacity = dynamic
            .checked_div(memory.bytes_per_point)
            .map_or(point_count, |raw| {
                usize::try_from(raw).unwrap_or(usize::MAX).min(point_count)
            });
        if capacity == 0 {
            return Err(LayoutError::InsufficientMemory {
                required_bytes: memory.fixed_bytes.saturating_add(memory.bytes_per_point),
                budget_bytes,
            });
        }
        let chunk_points = if memory.preferred_chunk_points == 0 {
            capacity
        } else {
            capacity.min(memory.preferred_chunk_points)
        };
        let mut boundaries = Vec::with_capacity(point_count / chunk_points + 2);
        boundaries.push(0);
        let mut end = chunk_points;
        while end < point_count {
            boundaries.push(end);
            end = end.saturating_add(chunk_points);
        }
        boundaries.push(point_count);
        let largest_points = boundaries
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap_or(0);
        let maximum_chunk_bytes = memory
            .fixed_bytes
            .saturating_add(memory.bytes_per_point.saturating_mul(largest_points as u64));
        Ok(Self {
            boundaries,
            budget_bytes,
            maximum_chunk_bytes,
        })
    }
}

/// Domain/cell/tile placement already computed during preparation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PointPlacement {
    /// Original caller index.
    pub original_index: usize,
    /// Selected domain.
    pub domain: DomainId,
    /// Located horizontal cell.
    pub cell: CellId,
    /// Stable derived-tile identity.
    pub tile_id: u64,
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

impl BatchLayout {
    /// Sorts placements by stable execution keys and freezes chunk boundaries.
    pub fn build(
        point_count: usize,
        mut placements: Vec<PointPlacement>,
        budget_bytes: u64,
        memory: ChunkMemoryModel,
    ) -> Result<Self, LayoutError> {
        let indices = placements
            .iter()
            .map(|placement| placement.original_index)
            .collect::<BTreeSet<_>>();
        if indices.len() != placements.len() || indices.iter().any(|index| *index >= point_count) {
            return Err(LayoutError::InvalidPermutation);
        }
        placements.sort_by(|left, right| {
            (&left.domain, left.cell, left.tile_id, left.original_index).cmp(&(
                &right.domain,
                right.cell,
                right.tile_id,
                right.original_index,
            ))
        });
        let forward = placements
            .iter()
            .map(|placement| placement.original_index)
            .collect::<Vec<_>>();
        let mut inverse = vec![None; point_count];
        for (internal, original) in forward.iter().copied().enumerate() {
            inverse[original] = Some(internal);
        }
        let domain_ids = placements
            .iter()
            .map(|placement| placement.domain.clone())
            .collect::<Vec<_>>();
        let cell_ids = placements
            .iter()
            .map(|placement| placement.cell)
            .collect::<Vec<_>>();
        let domain_groups = group_domains(&placements);
        let cell_groups = group_cells(&placements);
        let tile_groups = group_tiles(&placements);
        let chunks = ChunkPlan::build(placements.len(), budget_bytes, memory)?;
        Ok(Self {
            domain_ids,
            cell_ids,
            domain_groups,
            cell_groups,
            tile_groups,
            permutation: Permutation { forward, inverse },
            chunks,
        })
    }
}

fn group_domains(placements: &[PointPlacement]) -> Vec<DomainGroup> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < placements.len() {
        let domain = placements[start].domain.clone();
        let mut end = start + 1;
        while end < placements.len() && placements[end].domain == domain {
            end += 1;
        }
        groups.push(DomainGroup { domain, start, end });
        start = end;
    }
    groups
}

fn group_cells(placements: &[PointPlacement]) -> Vec<CellGroup> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < placements.len() {
        let domain = &placements[start].domain;
        let cell = placements[start].cell;
        let mut end = start + 1;
        while end < placements.len()
            && &placements[end].domain == domain
            && placements[end].cell == cell
        {
            end += 1;
        }
        groups.push(CellGroup { cell, start, end });
        start = end;
    }
    groups
}

fn group_tiles(placements: &[PointPlacement]) -> Vec<TileGroup> {
    let mut groups = Vec::new();
    let mut start = 0;
    while start < placements.len() {
        let domain = &placements[start].domain;
        let cell = placements[start].cell;
        let tile_id = placements[start].tile_id;
        let mut end = start + 1;
        while end < placements.len()
            && &placements[end].domain == domain
            && placements[end].cell == cell
            && placements[end].tile_id == tile_id
        {
            end += 1;
        }
        groups.push(TileGroup {
            tile_id,
            start,
            end,
        });
        start = end;
    }
    groups
}

/// Layout construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// Original indices are duplicated, missing, or outside `0..point_count`.
    InvalidPermutation,
    /// No domain safely covers one or more points.
    OutOfDomain(Vec<usize>),
    /// Fixed resources plus one point exceed the hard memory budget.
    InsufficientMemory {
        /// Minimum required bytes.
        required_bytes: u64,
        /// Frozen hard budget.
        budget_bytes: u64,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn grouping_and_inverse_permutation_are_stable() {
        let placements = vec![
            PointPlacement {
                original_index: 0,
                domain: DomainId("outer".into()),
                cell: CellId(3),
                tile_id: 1,
            },
            PointPlacement {
                original_index: 1,
                domain: DomainId("inner".into()),
                cell: CellId(2),
                tile_id: 0,
            },
            PointPlacement {
                original_index: 2,
                domain: DomainId("inner".into()),
                cell: CellId(1),
                tile_id: 0,
            },
            PointPlacement {
                original_index: 3,
                domain: DomainId("inner".into()),
                cell: CellId(1),
                tile_id: 0,
            },
        ];
        let layout = BatchLayout::build(
            4,
            placements,
            1_000,
            ChunkMemoryModel {
                fixed_bytes: 100,
                bytes_per_point: 100,
                preferred_chunk_points: 2,
            },
        )
        .unwrap();
        assert_eq!(layout.permutation.forward, vec![2, 3, 1, 0]);
        assert_eq!(
            layout.permutation.inverse,
            vec![Some(3), Some(2), Some(0), Some(1)]
        );
        let mut restored = vec![-1; 4];
        layout
            .permutation
            .scatter_into(&[20, 30, 10, 0], &mut restored)
            .unwrap();
        assert_eq!(restored, vec![0, 10, 20, 30]);
        assert_eq!(layout.chunks.boundaries, vec![0, 2, 4]);
    }

    #[test]
    fn chunk_plan_never_crosses_the_hard_budget() {
        let plan = ChunkPlan::build(
            1_000_000,
            10_000,
            ChunkMemoryModel {
                fixed_bytes: 1_000,
                bytes_per_point: 9,
                preferred_chunk_points: 0,
            },
        )
        .unwrap();
        assert!(plan.boundaries.len() > 2);
        assert!(plan.maximum_chunk_bytes <= plan.budget_bytes);
        assert_eq!(plan.boundaries[0], 0);
        assert_eq!(plan.boundaries.last().copied(), Some(1_000_000));
    }

    #[test]
    fn minimum_one_point_failure_reports_required_bytes() {
        assert_eq!(
            ChunkPlan::build(
                1,
                100,
                ChunkMemoryModel {
                    fixed_bytes: 90,
                    bytes_per_point: 20,
                    preferred_chunk_points: 0,
                },
            ),
            Err(LayoutError::InsufficientMemory {
                required_bytes: 110,
                budget_bytes: 100,
            })
        );
    }

    #[test]
    fn rejected_points_have_no_internal_index_and_keep_prefilled_output() {
        let layout = BatchLayout::build(
            3,
            vec![PointPlacement {
                original_index: 1,
                domain: DomainId("global".into()),
                cell: CellId(0),
                tile_id: 0,
            }],
            1_000,
            ChunkMemoryModel {
                fixed_bytes: 0,
                bytes_per_point: 1,
                preferred_chunk_points: 0,
            },
        )
        .unwrap();
        assert_eq!(layout.permutation.inverse, vec![None, Some(0), None]);
        let mut output = vec![99; 3];
        layout.permutation.scatter_into(&[7], &mut output).unwrap();
        assert_eq!(output, vec![99, 7, 99]);
    }
}
