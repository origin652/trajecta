//! # Contract: low-disturbance performance attribution
//!
//! Production code exposes stable timing boundaries, but records nothing until
//! a caller installs counters on the current thread. Audit runs aggregate
//! durations and bounded logarithmic histograms without per-call logging or
//! execution-time I/O.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const HISTOGRAM_BUCKETS: usize = 64;

thread_local! {
    static ACTIVE_PERFORMANCE_COUNTERS: RefCell<Option<Arc<PerformanceCounters>>> =
        const { RefCell::new(None) };
}

/// Stable wall-clock attribution boundary.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(usize)]
pub enum PerformanceStage {
    /// Complete `SimulationRunner::run` lifecycle.
    RunnerTotal,
    /// Population initialization, emissions, and lifecycle callbacks.
    RunnerPopulation,
    /// Numerical integrator advance.
    RunnerIntegrator,
    /// Continuous-boundary construction and policy application.
    RunnerBoundary,
    /// Particle-state meteorology queries and output product sampling.
    RunnerOutput,
    /// Complete meteorology query initiated by particle-state output.
    RunnerOutputMeteorology,
    /// Preparation/layout/stencil work for one output meteorology query.
    RunnerOutputQueryPrepare,
    /// Numerical execution of one prepared output meteorology query.
    RunnerOutputQueryExecute,
    /// Output-product sampling and persistence for one event.
    RunnerOutputSink,
    /// Output-product terminal finish/checkpoint work.
    RunnerOutputFinish,
    /// SQLite status update, WAL checkpoint, integrity, and row-count audit.
    OutputFinishSqliteAudit,
    /// Canonical normalized SQL digest over the terminal database.
    OutputFinishSqlDigest,
    /// Byte SHA-256 of the terminal standalone SQLite file.
    OutputFinishSqliteHash,
    /// Complete provenance-bundle finalization after SQLite closes.
    OutputFinishProvenanceBundle,
    /// Bounded external sort of the provenance sample spool.
    ProvenanceExternalSort,
    /// Streaming provenance JSON write and lockstep SQLite coverage.
    ProvenanceStreamWrite,
    /// Streaming semantic re-read and normalized content digest.
    ProvenanceSemanticValidate,
    /// Final provenance artifact SHA and canonical output identity.
    ProvenanceFinalHash,
    /// Successful provenance spool/run cleanup.
    ProvenanceCleanup,
    /// Meteorology frame-window selection and preparation.
    MetWindowPrepare,
    /// Complete construction of one meteorological boundary path sampler.
    BoundaryPathBuild,
    /// Grid/time/extremum certification used to split one path.
    BoundaryPathCertification,
    /// Interval residual isolation and certified root solving.
    BoundaryRootCertificate,
    /// One scalar continuous-boundary meteorology sample.
    BoundarySample,
    /// One four-corner boundary certification sample.
    BoundaryCornerSample,
    /// Complete direct boundary query.
    BoundaryQueryTotal,
    /// Boundary point location and horizontal-support calculation.
    BoundaryGridLocate,
    /// Boundary stencil acquisition and session preparation.
    BoundaryStencilPrepare,
    /// Time waiting to acquire the shared column-cache mutex.
    ColumnCacheLockWait,
    /// Time holding the shared column-cache mutex.
    ColumnCacheLockHold,
    /// Construction of one cache-missing column stencil.
    ColumnStencilBuild,
    /// Sampling and temporal blending of boundary column geometry.
    BoundaryColumnSample,
    /// Roughness, transport-floor, vertical-bounds, and status evaluation.
    BoundarySurfaceBounds,
}

impl PerformanceStage {
    const ALL: [Self; 33] = [
        Self::RunnerTotal,
        Self::RunnerPopulation,
        Self::RunnerIntegrator,
        Self::RunnerBoundary,
        Self::RunnerOutput,
        Self::RunnerOutputMeteorology,
        Self::RunnerOutputQueryPrepare,
        Self::RunnerOutputQueryExecute,
        Self::RunnerOutputSink,
        Self::RunnerOutputFinish,
        Self::OutputFinishSqliteAudit,
        Self::OutputFinishSqlDigest,
        Self::OutputFinishSqliteHash,
        Self::OutputFinishProvenanceBundle,
        Self::ProvenanceExternalSort,
        Self::ProvenanceStreamWrite,
        Self::ProvenanceSemanticValidate,
        Self::ProvenanceFinalHash,
        Self::ProvenanceCleanup,
        Self::MetWindowPrepare,
        Self::BoundaryPathBuild,
        Self::BoundaryPathCertification,
        Self::BoundaryRootCertificate,
        Self::BoundarySample,
        Self::BoundaryCornerSample,
        Self::BoundaryQueryTotal,
        Self::BoundaryGridLocate,
        Self::BoundaryStencilPrepare,
        Self::ColumnCacheLockWait,
        Self::ColumnCacheLockHold,
        Self::ColumnStencilBuild,
        Self::BoundaryColumnSample,
        Self::BoundarySurfaceBounds,
    ];

    /// Stable machine label used by A4.5 artifacts.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::RunnerTotal => "runner_total",
            Self::RunnerPopulation => "runner_population",
            Self::RunnerIntegrator => "runner_integrator",
            Self::RunnerBoundary => "runner_boundary",
            Self::RunnerOutput => "runner_output",
            Self::RunnerOutputMeteorology => "runner_output_meteorology",
            Self::RunnerOutputQueryPrepare => "runner_output_query_prepare",
            Self::RunnerOutputQueryExecute => "runner_output_query_execute",
            Self::RunnerOutputSink => "runner_output_sink",
            Self::RunnerOutputFinish => "runner_output_finish",
            Self::OutputFinishSqliteAudit => "output_finish_sqlite_audit",
            Self::OutputFinishSqlDigest => "output_finish_sql_digest",
            Self::OutputFinishSqliteHash => "output_finish_sqlite_hash",
            Self::OutputFinishProvenanceBundle => "output_finish_provenance_bundle",
            Self::ProvenanceExternalSort => "provenance_external_sort",
            Self::ProvenanceStreamWrite => "provenance_stream_write",
            Self::ProvenanceSemanticValidate => "provenance_semantic_validate",
            Self::ProvenanceFinalHash => "provenance_final_hash",
            Self::ProvenanceCleanup => "provenance_cleanup",
            Self::MetWindowPrepare => "met_window_prepare",
            Self::BoundaryPathBuild => "boundary_path_build",
            Self::BoundaryPathCertification => "boundary_path_certification",
            Self::BoundaryRootCertificate => "boundary_root_certificate",
            Self::BoundarySample => "boundary_sample",
            Self::BoundaryCornerSample => "boundary_corner_sample",
            Self::BoundaryQueryTotal => "boundary_query_total",
            Self::BoundaryGridLocate => "boundary_grid_locate",
            Self::BoundaryStencilPrepare => "boundary_stencil_prepare",
            Self::ColumnCacheLockWait => "column_cache_lock_wait",
            Self::ColumnCacheLockHold => "column_cache_lock_hold",
            Self::ColumnStencilBuild => "column_stencil_build",
            Self::BoundaryColumnSample => "boundary_column_sample",
            Self::BoundarySurfaceBounds => "boundary_surface_bounds",
        }
    }
}

/// Stable bounded distribution recorded once per logical path or root search.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(usize)]
pub enum PerformanceDistribution {
    /// Distinct scalar samples retained by one completed boundary path.
    BoundarySamplesPerPath,
    /// Distinct four-corner samples retained by one completed boundary path.
    BoundaryCornerSamplesPerPath,
    /// Certified leaf segments emitted by one completed boundary path.
    BoundarySegmentsPerPath,
    /// Certification refinement passes used by one path.
    BoundaryCertificationPassesPerPath,
    /// Recursive interval nodes visited by one residual isolation.
    ResidualNodesPerIsolation,
    /// Maximum recursion depth reached by one residual isolation.
    ResidualMaxDepthPerIsolation,
}

impl PerformanceDistribution {
    const ALL: [Self; 6] = [
        Self::BoundarySamplesPerPath,
        Self::BoundaryCornerSamplesPerPath,
        Self::BoundarySegmentsPerPath,
        Self::BoundaryCertificationPassesPerPath,
        Self::ResidualNodesPerIsolation,
        Self::ResidualMaxDepthPerIsolation,
    ];

    /// Stable machine label used by A4.5 artifacts.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::BoundarySamplesPerPath => "boundary_samples_per_path",
            Self::BoundaryCornerSamplesPerPath => "boundary_corner_samples_per_path",
            Self::BoundarySegmentsPerPath => "boundary_segments_per_path",
            Self::BoundaryCertificationPassesPerPath => "boundary_certification_passes_per_path",
            Self::ResidualNodesPerIsolation => "residual_nodes_per_isolation",
            Self::ResidualMaxDepthPerIsolation => "residual_max_depth_per_isolation",
        }
    }
}

#[derive(Debug)]
struct ObservationState {
    observations: AtomicU64,
    total: AtomicU64,
    maximum: AtomicU64,
    histogram: Vec<AtomicU64>,
}

impl Default for ObservationState {
    fn default() -> Self {
        Self {
            observations: AtomicU64::new(0),
            total: AtomicU64::new(0),
            maximum: AtomicU64::new(0),
            histogram: (0..HISTOGRAM_BUCKETS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

impl ObservationState {
    fn record(&self, value: u64) {
        self.observations.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(value, Ordering::Relaxed);
        self.maximum.fetch_max(value, Ordering::Relaxed);
        self.histogram[histogram_bucket(value)].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> PerformanceObservationSnapshot {
        PerformanceObservationSnapshot {
            observations: self.observations.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            maximum: self.maximum.load(Ordering::Relaxed),
            log2_histogram: self
                .histogram
                .iter()
                .map(|value| value.load(Ordering::Relaxed))
                .collect(),
        }
    }
}

/// Shareable counters installed only for an explicit attribution run.
#[derive(Debug)]
pub struct PerformanceCounters {
    stages: Vec<ObservationState>,
    distributions: Vec<ObservationState>,
}

impl Default for PerformanceCounters {
    fn default() -> Self {
        Self {
            stages: PerformanceStage::ALL
                .iter()
                .map(|_| ObservationState::default())
                .collect(),
            distributions: PerformanceDistribution::ALL
                .iter()
                .map(|_| ObservationState::default())
                .collect(),
        }
    }
}

impl PerformanceCounters {
    /// Creates an empty shareable attribution counter set.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn record_stage(&self, stage: PerformanceStage, nanoseconds: u64) {
        self.stages[stage as usize].record(nanoseconds);
    }

    fn record_distribution(&self, distribution: PerformanceDistribution, value: u64) {
        self.distributions[distribution as usize].record(value);
    }

    /// Takes an immutable snapshot without resetting counters.
    #[must_use]
    pub fn snapshot(&self) -> PerformanceSnapshot {
        PerformanceSnapshot {
            stages: PerformanceStage::ALL
                .iter()
                .map(|stage| (*stage, self.stages[*stage as usize].snapshot()))
                .collect(),
            distributions: PerformanceDistribution::ALL
                .iter()
                .map(|distribution| {
                    (
                        *distribution,
                        self.distributions[*distribution as usize].snapshot(),
                    )
                })
                .collect(),
        }
    }
}

/// One immutable count/total/maximum and bounded log2 histogram.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PerformanceObservationSnapshot {
    /// Number of recorded observations.
    pub observations: u64,
    /// Sum in nanoseconds for stages, or raw units for distributions.
    pub total: u64,
    /// Maximum observed value.
    pub maximum: u64,
    /// Bucket `i` counts values whose floor-log2 is `i`; zero uses bucket zero.
    pub log2_histogram: Vec<u64>,
}

/// Immutable complete performance-attribution snapshot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PerformanceSnapshot {
    /// Per-stage duration observations in nanoseconds.
    pub stages: BTreeMap<PerformanceStage, PerformanceObservationSnapshot>,
    /// Per-path and per-root bounded distributions.
    pub distributions: BTreeMap<PerformanceDistribution, PerformanceObservationSnapshot>,
}

/// Installs counters for the current thread.
pub fn install_performance_counters(counters: Arc<PerformanceCounters>) {
    ACTIVE_PERFORMANCE_COUNTERS.with(|slot| *slot.borrow_mut() = Some(counters));
}

/// Clears current-thread attribution counters.
pub fn clear_performance_counters() {
    ACTIVE_PERFORMANCE_COUNTERS.with(|slot| *slot.borrow_mut() = None);
}

/// Records one bounded path/root distribution value when attribution is active.
pub fn record_performance_distribution(distribution: PerformanceDistribution, value: u64) {
    ACTIVE_PERFORMANCE_COUNTERS.with(|slot| {
        if let Some(counters) = slot.borrow().as_ref() {
            counters.record_distribution(distribution, value);
        }
    });
}

/// RAII timer for one stable attribution stage.
#[must_use]
pub struct PerformanceScope {
    stage: PerformanceStage,
    started: Option<Instant>,
}

impl PerformanceScope {
    /// Starts a timer only when counters are installed on this thread.
    pub fn enter(stage: PerformanceStage) -> Self {
        let enabled = ACTIVE_PERFORMANCE_COUNTERS.with(|slot| slot.borrow().is_some());
        Self {
            stage,
            started: enabled.then(Instant::now),
        }
    }
}

impl Drop for PerformanceScope {
    fn drop(&mut self) {
        let Some(started) = self.started else {
            return;
        };
        let nanoseconds = u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        ACTIVE_PERFORMANCE_COUNTERS.with(|slot| {
            if let Some(counters) = slot.borrow().as_ref() {
                counters.record_stage(self.stage, nanoseconds);
            }
        });
    }
}

fn histogram_bucket(value: u64) -> usize {
    if value == 0 {
        0
    } else {
        usize::try_from(u64::BITS - 1 - value.leading_zeros())
            .unwrap_or(HISTOGRAM_BUCKETS - 1)
            .min(HISTOGRAM_BUCKETS - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_observation_records_nothing() {
        clear_performance_counters();
        let counters = PerformanceCounters::new();
        {
            let _scope = PerformanceScope::enter(PerformanceStage::RunnerTotal);
        }
        assert_eq!(
            counters.snapshot().stages[&PerformanceStage::RunnerTotal].observations,
            0
        );
    }

    #[test]
    fn installed_observation_records_bounded_stage_and_distribution() {
        let counters = PerformanceCounters::new();
        install_performance_counters(Arc::clone(&counters));
        {
            let _scope = PerformanceScope::enter(PerformanceStage::RunnerTotal);
            std::hint::black_box(1_u64);
        }
        record_performance_distribution(PerformanceDistribution::BoundarySegmentsPerPath, 7);
        clear_performance_counters();

        let snapshot = counters.snapshot();
        let stage = &snapshot.stages[&PerformanceStage::RunnerTotal];
        assert_eq!(stage.observations, 1);
        assert_eq!(stage.log2_histogram.iter().sum::<u64>(), 1);
        let distribution =
            &snapshot.distributions[&PerformanceDistribution::BoundarySegmentsPerPath];
        assert_eq!(distribution.observations, 1);
        assert_eq!(distribution.total, 7);
        assert_eq!(distribution.maximum, 7);
        assert_eq!(distribution.log2_histogram[2], 1);
    }
}
