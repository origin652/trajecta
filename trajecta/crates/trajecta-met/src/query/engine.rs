//! # Contract: meteorology runtime orchestration
//!
//! `MetEngine::prepare` may select and load frames and update deterministic
//! caches. `PreparedBatch::execute` receives only pinned data and cannot invoke
//! readers or providers. Callers own the parallel execution policy.

use trajecta_case::model::time::Timestamp;

use crate::field::FieldRegistry;
use crate::frame::{FrameCache, FrameError, PreparedWindow, WindowManager};
use crate::io::inventory::MetCatalog;
use crate::profile::document::ProfileCatalog;
use crate::query::cache::{ColumnCache, MemoryBudget, TileCache};
use crate::query::layout::BatchLayout;
use crate::query::output::QueryOutput;
use crate::query::request::{QueryBatch, QueryPlan};
use crate::surface_layer::SurfaceLayerRegistry;

/// Caller-provided parallel execution policy.
pub trait ExecutionContext: Send + Sync {
    /// Maximum worker count available to a prepared batch.
    fn worker_count(&self) -> usize;
}

/// Default CPU execution context; Rayon integration is added with execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RayonExecutionContext {
    /// Non-zero requested worker count.
    pub worker_threads: usize,
}

impl ExecutionContext for RayonExecutionContext {
    fn worker_count(&self) -> usize {
        self.worker_threads
    }
}

/// Reusable caller-owned scratch buffers.
#[derive(Clone, Debug, Default)]
pub struct BatchWorkspace {
    /// Reusable floating-point scratch.
    pub floating: Vec<f64>,
    /// Reusable integer scratch.
    pub indices: Vec<usize>,
}

/// Immutable configuration required to construct a meteorology engine.
pub struct MetEngineConfig {
    /// Fully validated source inventory.
    pub catalog: MetCatalog,
    /// Exact loaded profile catalog.
    pub profiles: ProfileCatalog,
    /// Stable field registry.
    pub fields: FieldRegistry,
    /// Named pure near-surface models.
    pub surface_layers: SurfaceLayerRegistry,
    /// Hard runtime memory budget.
    pub memory_budget: MemoryBudget,
}

/// Mutable owner of frame transitions and preparation-time caches.
pub struct MetEngine {
    catalog: MetCatalog,
    profiles: ProfileCatalog,
    fields: FieldRegistry,
    surface_layers: SurfaceLayerRegistry,
    window_manager: WindowManager,
    frame_cache: FrameCache,
    column_cache: ColumnCache,
    tile_cache: TileCache,
    memory_budget: MemoryBudget,
}

impl MetEngine {
    /// Constructs an engine without reading source files.
    #[must_use]
    pub fn new(config: MetEngineConfig) -> Self {
        Self {
            catalog: config.catalog,
            profiles: config.profiles,
            fields: config.fields,
            surface_layers: config.surface_layers,
            window_manager: WindowManager,
            frame_cache: FrameCache::default(),
            column_cache: ColumnCache::default(),
            tile_cache: TileCache::default(),
            memory_budget: config.memory_budget,
        }
    }

    /// Performs all frame selection and source I/O needed for one time.
    pub fn prepare(&mut self, time: Timestamp) -> Result<PreparedWindow, EngineError> {
        self.window_manager
            .prepare(time)
            .map_err(EngineError::Frame)
    }

    /// Returns the validated source catalog.
    #[must_use]
    pub const fn catalog(&self) -> &MetCatalog {
        &self.catalog
    }

    /// Returns the loaded profile catalog.
    #[must_use]
    pub const fn profiles(&self) -> &ProfileCatalog {
        &self.profiles
    }

    /// Returns the field registry.
    #[must_use]
    pub const fn fields(&self) -> &FieldRegistry {
        &self.fields
    }

    /// Returns the named surface-layer registry.
    #[must_use]
    pub const fn surface_layers(&self) -> &SurfaceLayerRegistry {
        &self.surface_layers
    }

    /// Returns the hard memory budget.
    #[must_use]
    pub const fn memory_budget(&self) -> MemoryBudget {
        self.memory_budget
    }

    /// Returns raw-frame cache state.
    #[must_use]
    pub const fn frame_cache(&self) -> &FrameCache {
        &self.frame_cache
    }

    /// Returns column-cache state.
    #[must_use]
    pub const fn column_cache(&self) -> &ColumnCache {
        &self.column_cache
    }

    /// Returns tile-cache state.
    #[must_use]
    pub const fn tile_cache(&self) -> &TileCache {
        &self.tile_cache
    }
}

impl PreparedWindow {
    /// Pins layout, cache entries, and chunk boundaries for one query batch.
    pub fn prepare_batch(
        &self,
        _plan: &QueryPlan,
        _batch: QueryBatch,
        _workspace: &mut BatchWorkspace,
    ) -> Result<PreparedBatch, EngineError> {
        Err(EngineError::NotImplemented)
    }
}

/// Fully pinned, I/O-free batch execution state.
#[derive(Clone, Debug)]
pub struct PreparedBatch {
    /// Immutable field/dependency plan.
    pub plan: QueryPlan,
    /// Immutable pinned time window.
    pub window: PreparedWindow,
    /// Original query batch.
    pub batch: QueryBatch,
    /// Backend-neutral deterministic layout.
    pub layout: BatchLayout,
}

impl PreparedBatch {
    /// Executes without reader, provider, or cache mutation.
    pub fn execute(
        &self,
        _context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<QueryOutput, EngineError> {
        Err(EngineError::NotImplemented)
    }
}

/// Stateless chunk executor selected by a prepared batch.
#[derive(Clone, Copy, Debug, Default)]
pub struct QueryExecutor;

/// Meteorology preparation or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineError {
    /// Query preparation/execution has not been implemented yet.
    NotImplemented,
    /// Frame preparation failed before query execution.
    Frame(FrameError),
    /// Execution context reports zero workers.
    InvalidExecutionContext,
    /// Prepared layout or output columns are inconsistent.
    InvalidPreparedState,
}
