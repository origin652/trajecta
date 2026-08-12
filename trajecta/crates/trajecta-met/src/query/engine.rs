//! # Contract: meteorology runtime orchestration
//!
//! `MetEngine::prepare` may select and load frames and update deterministic
//! caches. `PreparedBatch::execute` receives only pinned data and cannot invoke
//! readers or providers. Callers own the parallel execution policy.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;

use crate::derive::pv::potential_temperature_k;
use crate::derive::surface::{
    SurfaceExchangeInput, SurfaceExchangeScales, SurfaceMomentumInput, surface_exchange_scales,
};
use crate::derive::thermo::{moist_air_density_kg_m3, project_specific_humidity_nonnegative};
use crate::derive::vertical_velocity::{
    KinematicVerticalVelocityInput, geometric_vertical_velocity_m_s,
    native_coordinate_velocity_from_omega, terrain_following_surface_velocity_m_s,
};
use crate::field::{CanonicalField, Capability, FieldKey, FieldQuality, FieldRegistry};
use crate::frame::{
    ArrayLayout, FrameCache, FrameError, PreparedWindow, RawField, RawMetFrame, WindowManager,
};
use crate::grid::{
    GridBackend, GridError, GridPoint, HorizontalWeights, RegularLatLonGrid, SphericalBasis,
    interpolate_spherical_vector, interpolate_spherical_vector_with_bases,
    spherical_vector_query_basis,
};
use crate::io::inventory::MetCatalog;
use crate::performance::{PerformanceScope, PerformanceStage};
use crate::profile::document::ProfileCatalog;
use crate::profile::graph::ExecutionPlan;
use crate::provenance::{
    ProvenanceError, ProvenanceId, ProvenanceRecord, ProvenanceTable, TransformRecord,
};
use crate::query::cache::{
    CacheError, CacheKey, CacheMetrics, CachedTransportQuery, ColumnCache, ExactTransportCacheKey,
    ExactTransportWindowKey, LastTransportQueryCache, MemoryBudget, PinGuard, TileCache,
};
use crate::query::convection::ConvectionColumnOutput;
use crate::query::layout::{BatchLayout, ChunkMemoryModel, LayoutError, PointPlacement};
use crate::query::mesoscale::{MesoscaleStatisticsOutput, WeightedVelocity, weighted_variance};
use crate::query::metrics::{
    ExactQueryKey, QueryCallCounters, active_query_counters, active_query_origin,
};
use crate::query::output::{
    BoundaryQueryOutput, BoundsColumn, ExplainField, ExplainHorizontalMethod,
    ExplainHorizontalSupport, ExplainRecord, ExplainVerticalPath, ExplainVerticalSupport,
    FieldColumn, OutputError, QueryOutput, SampleStatus, StatusColumn, TransportColumns,
    TransportOutput, ValidityMask, ValueColumn,
};
use crate::query::request::{
    ExplainMode, QueryBatch, QueryPlan, QueryPlanBuilder, QueryPlanError, QueryPlanRequest,
    TransportPlan, TransportPlanRequest, VerticalQuery,
};
use crate::query::stability::StabilityOutput;
use crate::science::M3_CONSTANTS;
use crate::science::{
    AERODYNAMIC_ROUGHNESS_ZERO_PROJECTION_ALGORITHM_ID,
    LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID, M3_MET_QUERY_ALGORITHM_ID,
    SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID, SURFACE_EXCHANGE_SCALES_ALGORITHM_ID,
    TEN_METRE_ANCHORED_SURFACE_WIND_ALGORITHM_ID, TWO_METRE_ANCHORED_SURFACE_SCALAR_ALGORITHM_ID,
};
use crate::surface_layer::{
    SurfaceLayerInput, SurfaceLayerRegistry, SurfaceLayerStatus, minimum_transport_height_agl_m,
    project_aerodynamic_roughness_for_physics,
};
use crate::vertical::{
    ColumnGeometry, ColumnLevelSample, ColumnRequest, ColumnShape, ColumnStencil, HybridColumnView,
    HybridLevelGeometry, NativeCoordinateKind, VerticalBounds, VerticalBracket, VerticalError,
    VerticalTopology,
};

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

const PARALLEL_TRANSPORT_MIN_POINTS: usize = 256;

fn execution_pool(worker_count: usize) -> Result<Arc<ThreadPool>, EngineError> {
    static POOLS: OnceLock<Mutex<BTreeMap<usize, Arc<ThreadPool>>>> = OnceLock::new();

    let pools = POOLS.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(pool) = pools
        .lock()
        .map_err(|_| EngineError::ExecutionPoolUnavailable)?
        .get(&worker_count)
        .cloned()
    {
        return Ok(pool);
    }

    let pool = Arc::new(
        ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(move |index| format!("trajecta-met-{worker_count}-{index}"))
            .build()
            .map_err(|_| EngineError::ExecutionPoolUnavailable)?,
    );
    let mut guard = pools
        .lock()
        .map_err(|_| EngineError::ExecutionPoolUnavailable)?;
    Ok(Arc::clone(
        guard
            .entry(worker_count)
            .or_insert_with(|| Arc::clone(&pool)),
    ))
}

/// Reusable caller-owned scratch buffers.
#[derive(Clone, Debug, Default)]
pub struct BatchWorkspace {
    /// Reusable floating-point scratch.
    pub floating: Vec<f64>,
    /// Reusable integer scratch.
    pub indices: Vec<usize>,
    boundary_stencils: BoundaryStencilSession,
}

#[derive(Clone, Debug, Default)]
struct BoundaryStencilSession {
    entries: BTreeMap<CacheKey, PinGuard<ColumnStencil>>,
    resident_bytes: u64,
}

impl BoundaryStencilSession {
    fn insert(&mut self, key: CacheKey, pin: PinGuard<ColumnStencil>) {
        if self.entries.contains_key(&key) {
            return;
        }
        self.resident_bytes = self.resident_bytes.saturating_add(pin.size_bytes());
        self.entries.insert(key, pin);
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.resident_bytes = 0;
    }
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
    column_cache: Arc<Mutex<ColumnCache>>,
    tile_cache: TileCache,
    last_transport_query: LastTransportQueryCache,
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
            frame_cache: FrameCache::new(config.memory_budget.fixed_bytes),
            column_cache: Arc::new(Mutex::new(ColumnCache::new(
                config.memory_budget.dynamic_bytes(),
            ))),
            tile_cache: TileCache::default(),
            last_transport_query: Arc::new(Mutex::new(None)),
            memory_budget: config.memory_budget,
        }
    }

    /// Performs all frame selection and source I/O needed for one time.
    pub fn prepare(&mut self, time: Timestamp) -> Result<PreparedWindow, EngineError> {
        let mut domains = self.catalog.domains.keys();
        let domain = domains.next().cloned().ok_or(EngineError::NoDomains)?;
        if domains.next().is_some() {
            return Err(EngineError::AmbiguousDomain);
        }
        self.prepare_for_domain(time, &domain)
    }

    /// Pins a cached frame window for one explicit domain without source I/O.
    pub fn prepare_for_domain(
        &mut self,
        time: Timestamp,
        domain: &DomainId,
    ) -> Result<PreparedWindow, EngineError> {
        let _performance = PerformanceScope::enter(PerformanceStage::MetWindowPrepare);
        let frames = self.frame_cache.frames_for_domain(domain);
        let mut window = self
            .window_manager
            .prepare_from_sorted_frames(time, &frames, self.memory_budget.dynamic_bytes())
            .map_err(EngineError::Frame)?;
        window.attach_column_cache(&self.column_cache);
        window.attach_transport_cache(&self.last_transport_query);
        Ok(window)
    }

    /// Publishes one already-loaded immutable frame into the engine cache.
    pub fn cache_frame(&mut self, frame: Arc<RawMetFrame>) -> Result<(), EngineError> {
        *self
            .last_transport_query
            .lock()
            .map_err(|_| EngineError::TransportCachePoisoned)? = None;
        self.frame_cache
            .insert(frame)
            .map(|_| ())
            .map_err(EngineError::Frame)
    }

    /// Compiles a generic query contract without reading source data.
    pub fn compile_plan(
        &self,
        request: QueryPlanRequest,
        derivations: &ExecutionPlan,
    ) -> Result<QueryPlan, EngineError> {
        QueryPlanBuilder::new(
            &self.fields,
            self.catalog.capabilities,
            &self.surface_layers,
            derivations,
        )
        .build(request)
        .map_err(EngineError::QueryPlan)
    }

    /// Compiles the fixed complete transport contract consumed by M4.
    pub fn compile_transport_plan(
        &self,
        request: TransportPlanRequest,
        derivations: &ExecutionPlan,
    ) -> Result<TransportPlan, EngineError> {
        QueryPlanBuilder::new(
            &self.fields,
            self.catalog.capabilities,
            &self.surface_layers,
            derivations,
        )
        .build_transport(request)
        .map_err(EngineError::QueryPlan)
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
    pub fn column_cache_metrics(&self) -> Result<CacheMetrics, EngineError> {
        let cache = self
            .column_cache
            .lock()
            .map_err(|_| EngineError::ColumnCachePoisoned)?;
        Ok(cache.metrics())
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
        plan: &QueryPlan,
        batch: QueryBatch,
        _workspace: &mut BatchWorkspace,
    ) -> Result<PreparedBatch, EngineError> {
        batch.validate().map_err(EngineError::QueryPlan)?;
        let requires_geometric_w = plan.surface_layer_model().is_some()
            || plan.fields().iter().any(|field| {
                field == &FieldKey::Canonical(CanonicalField::GeometricVerticalVelocity)
            });
        if requires_geometric_w {
            self.validate_transport_time_support()
                .map_err(EngineError::Frame)?;
        }
        let (placements, initial_status) = locate_points(self, &batch)?;
        let stencils = prepare_stencils(
            self,
            &batch,
            &placements,
            plan.capabilities(),
            requires_geometric_w,
        )?;
        let layout = prepare_layout(
            self,
            placements,
            initial_status.len(),
            plan,
            stencils.resident_bytes,
            0,
        )?;
        let query_counter = active_query_counters();
        let query_origin = active_query_origin();
        let exact_query_key = query_counter.as_ref().map(|counter| {
            counter.record_logical_request(
                query_origin,
                self.query_time,
                &self.frames.before.metadata().domain,
                plan,
                &batch,
            )
        });
        Ok(PreparedBatch {
            plan: plan.clone(),
            window: self.clone(),
            batch,
            layout,
            initial_status,
            stencils,
            query_counter,
            exact_query_key,
        })
    }

    /// Pins layout, cache entries, and chunks for complete transport output.
    pub fn prepare_transport_batch(
        &self,
        plan: &TransportPlan,
        batch: QueryBatch,
        _workspace: &mut BatchWorkspace,
    ) -> Result<PreparedTransportBatch, EngineError> {
        batch.validate().map_err(EngineError::QueryPlan)?;
        self.validate_transport_time_support()
            .map_err(EngineError::Frame)?;
        let (placements, initial_status, horizontal_support) =
            locate_points_with_weights(self, &batch)?;
        let stencils = prepare_stencils(
            self,
            &batch,
            &placements,
            plan.query_plan().capabilities(),
            true,
        )?;
        let layout = prepare_layout(
            self,
            placements,
            initial_status.len(),
            plan.query_plan(),
            stencils.resident_bytes,
            u64::try_from(
                std::mem::size_of::<Result<TransportPointResult, EngineError>>()
                    + std::mem::size_of::<Option<HorizontalWeights>>(),
            )
            .map_err(|_| EngineError::MemoryEstimateOverflow)?,
        )?;
        let query_counter = active_query_counters();
        let query_origin = active_query_origin();
        let exact_query_key = query_counter.as_ref().map(|counter| {
            counter.record_logical_request(
                query_origin,
                self.query_time,
                &self.frames.before.metadata().domain,
                plan.query_plan(),
                &batch,
            )
        });
        let transport_cache_key = ExactTransportCacheKey::new(
            ExactTransportWindowKey {
                query_time: self.query_time,
                domain: self.frames.before.metadata().domain.clone(),
                before_frame: self.frames.before.metadata().id.clone(),
                after_frame: self.frames.after.metadata().id.clone(),
                previous_frame: self
                    .previous
                    .as_ref()
                    .map(|frame| frame.metadata().id.clone()),
                next_frame: self.next.as_ref().map(|frame| frame.metadata().id.clone()),
                before_weight_bits: self.before_weight.to_bits(),
                after_weight_bits: self.after_weight.to_bits(),
            },
            plan.clone(),
            &batch,
        );
        Ok(PreparedTransportBatch {
            plan: plan.clone(),
            window: self.clone(),
            batch,
            layout,
            initial_status,
            horizontal_support,
            stencils,
            query_counter,
            exact_query_key,
            transport_cache: self.transport_cache(),
            transport_cache_key,
            populate_transport_cache: query_origin.populates_transport_cache(),
        })
    }

    /// Pins the two-time, four-corner, two-level support used by M6 mesoscale motion.
    pub fn prepare_mesoscale_batch(
        &self,
        batch: QueryBatch,
        _workspace: &mut BatchWorkspace,
    ) -> Result<PreparedMesoscaleBatch, EngineError> {
        let point_count = batch.validate().map_err(EngineError::QueryPlan)?;
        self.validate_transport_time_support()
            .map_err(EngineError::Frame)?;
        let (placements, initial_status, horizontal_support) =
            locate_points_with_weights(self, &batch)?;
        let capabilities = crate::field::CapabilitySet::new().with(Capability::Transport);
        let stencils = prepare_stencils(self, &batch, &placements, capabilities, true)?;
        let layout = BatchLayout::build(
            point_count,
            placements,
            self.execution_budget_bytes,
            ChunkMemoryModel {
                fixed_bytes: stencils.resident_bytes,
                bytes_per_point: 256,
                preferred_chunk_points: 65_536,
            },
        )
        .map_err(EngineError::Layout)?;
        Ok(PreparedMesoscaleBatch {
            window: self.clone(),
            batch,
            layout,
            initial_status,
            horizontal_support,
            stencils,
        })
    }

    /// Pins complete local thermodynamic columns for M6 terrain stability.
    pub fn prepare_stability_batch(
        &self,
        batch: QueryBatch,
        _workspace: &mut BatchWorkspace,
    ) -> Result<PreparedStabilityBatch, EngineError> {
        let point_count = batch.validate().map_err(EngineError::QueryPlan)?;
        let (placements, initial_status, horizontal_support) =
            locate_points_with_weights(self, &batch)?;
        let capabilities = crate::field::CapabilitySet::new().with(Capability::Transport);
        let stencils = prepare_stencils(self, &batch, &placements, capabilities, false)?;
        let layout = BatchLayout::build(
            point_count,
            placements,
            self.execution_budget_bytes,
            ChunkMemoryModel {
                fixed_bytes: stencils.resident_bytes,
                bytes_per_point: 192,
                preferred_chunk_points: 65_536,
            },
        )
        .map_err(EngineError::Layout)?;
        Ok(PreparedStabilityBatch {
            window: self.clone(),
            batch,
            layout,
            initial_status,
            horizontal_support,
            stencils,
        })
    }

    /// Pins complete local thermodynamic columns for M6 deep convection.
    pub fn prepare_convection_batch(
        &self,
        batch: QueryBatch,
        _workspace: &mut BatchWorkspace,
    ) -> Result<PreparedConvectionBatch, EngineError> {
        let point_count = batch.validate().map_err(EngineError::QueryPlan)?;
        let (placements, initial_status) = locate_points(self, &batch)?;
        let capabilities = crate::field::CapabilitySet::new().with(Capability::Transport);
        let stencils = prepare_stencils(self, &batch, &placements, capabilities, false)?;
        let layout = BatchLayout::build(
            point_count,
            placements,
            self.execution_budget_bytes,
            ChunkMemoryModel {
                fixed_bytes: stencils.resident_bytes,
                bytes_per_point: 4_096,
                preferred_chunk_points: 8_192,
            },
        )
        .map_err(EngineError::Layout)?;
        Ok(PreparedConvectionBatch {
            window: self.clone(),
            batch,
            layout,
            initial_status,
            stencils,
        })
    }

    /// Pins the minimal I/O-free geometry required by continuous boundaries.
    ///
    /// Unlike complete transport preparation, this does not pin derivative
    /// frames used only by geometric vertical velocity.
    pub fn prepare_boundary_batch(
        &self,
        plan: &TransportPlan,
        batch: QueryBatch,
        workspace: &mut BatchWorkspace,
    ) -> Result<PreparedBoundaryBatch, EngineError> {
        batch.validate().map_err(EngineError::QueryPlan)?;
        let (placements, initial_status) = locate_points(self, &batch)?;
        let stencils = prepare_boundary_stencils(
            self,
            &batch,
            &placements,
            plan.query_plan().capabilities(),
            workspace,
        )?;
        let layout = prepare_layout(
            self,
            placements,
            initial_status.len(),
            plan.query_plan(),
            stencils.resident_bytes,
            0,
        )?;
        let non_cache_execution_bytes = layout
            .chunks
            .maximum_chunk_bytes
            .saturating_sub(stencils.resident_bytes);
        finalize_boundary_stencil_session(self, workspace, &stencils, non_cache_execution_bytes)?;
        let query_counter = active_query_counters();
        let query_origin = active_query_origin();
        let exact_query_key = query_counter.as_ref().map(|counter| {
            counter.record_logical_request(
                query_origin,
                self.query_time,
                &self.frames.before.metadata().domain,
                plan.query_plan(),
                &batch,
            )
        });
        Ok(PreparedBoundaryBatch {
            plan: plan.clone(),
            window: self.clone(),
            batch,
            layout,
            initial_status,
            stencils,
            query_counter,
            exact_query_key,
        })
    }

    /// Executes one minimal boundary batch directly from this pinned window.
    ///
    /// This is the scalar/small-batch production path used by continuous
    /// boundary root finding. It preserves the prepared-query I/O boundary but
    /// avoids constructing a general grouped `BatchLayout` for one or four
    /// points.
    pub fn query_boundary_batch(
        &self,
        plan: &TransportPlan,
        batch: QueryBatch,
        context: &dyn ExecutionContext,
        workspace: &mut BatchWorkspace,
    ) -> Result<BoundaryQueryOutput, EngineError> {
        let _query_performance = PerformanceScope::enter(PerformanceStage::BoundaryQueryTotal);
        batch.validate().map_err(EngineError::QueryPlan)?;
        if context.worker_count() == 0 {
            return Err(EngineError::InvalidExecutionContext);
        }
        let (placements, mut status, horizontal_support) = {
            let _performance = PerformanceScope::enter(PerformanceStage::BoundaryGridLocate);
            locate_points_with_weights(self, &batch)?
        };
        let stencils = {
            let _performance = PerformanceScope::enter(PerformanceStage::BoundaryStencilPrepare);
            prepare_boundary_stencils(
                self,
                &batch,
                &placements,
                plan.query_plan().capabilities(),
                workspace,
            )?
        };
        let non_cache_execution_bytes = boundary_direct_execution_bytes(&stencils, status.len())?;
        if stencils
            .resident_bytes
            .checked_add(non_cache_execution_bytes)
            .is_none_or(|required| required > self.execution_budget_bytes)
        {
            return Err(EngineError::Layout(LayoutError::InsufficientMemory {
                required_bytes: stencils
                    .resident_bytes
                    .saturating_add(non_cache_execution_bytes),
                budget_bytes: self.execution_budget_bytes,
            }));
        }
        {
            let _performance = PerformanceScope::enter(PerformanceStage::BoundaryStencilPrepare);
            finalize_boundary_stencil_session(
                self,
                workspace,
                &stencils,
                non_cache_execution_bytes,
            )?;
        }
        let query_counter = active_query_counters();
        let query_origin = active_query_origin();
        let exact_query_key = query_counter.as_ref().map(|counter| {
            counter.record_logical_request(
                query_origin,
                self.query_time,
                &self.frames.before.metadata().domain,
                plan.query_plan(),
                &batch,
            )
        });
        if let (Some(counter), Some(key)) = (&query_counter, exact_query_key) {
            counter.record_execution(key);
        }
        let point_count = status.len();
        let mut bounds = vec![None; point_count];
        let mut terrain_height_asl_m = vec![None; point_count];
        for placement in placements {
            let weights = horizontal_support
                .get(placement.original_index)
                .copied()
                .flatten()
                .ok_or(EngineError::InvalidPreparedState)?;
            match sample_boundary_point_with_weights(
                self,
                &stencils,
                &batch,
                placement.cell,
                placement.original_index,
                weights,
            ) {
                Ok(point) => {
                    status[placement.original_index] = point.status;
                    bounds[placement.original_index] = Some(point.bounds);
                    terrain_height_asl_m[placement.original_index] = point.terrain_height_asl_m;
                }
                Err(error) => {
                    if let Some(local_status) = local_status_for_error(&error) {
                        status[placement.original_index] = local_status;
                    } else {
                        return Err(error);
                    }
                }
            }
        }
        BoundaryQueryOutput::new(
            StatusColumn::new(status),
            BoundsColumn::new(bounds),
            terrain_height_asl_m,
        )
        .map_err(EngineError::Output)
    }
}

fn locate_points(
    window: &PreparedWindow,
    batch: &QueryBatch,
) -> Result<(Vec<PointPlacement>, Vec<SampleStatus>), EngineError> {
    let point_count = batch.points.len().map_err(EngineError::QueryPlan)?;
    let grid = RegularLatLonGrid::new(window.frames.before.metadata().grid.clone())
        .map_err(EngineError::Grid)?;
    let mut placements = Vec::with_capacity(point_count);
    let mut initial_status = vec![SampleStatus::Ok; point_count];
    for (index, status) in initial_status.iter_mut().enumerate() {
        let longitude = batch.points.longitude_degrees[index];
        let latitude = batch.points.latitude_degrees[index];
        match grid.locate_cell(longitude, latitude) {
            Ok(cell) => placements.push(PointPlacement {
                original_index: index,
                domain: window.frames.before.metadata().domain.clone(),
                cell,
                tile_id: cell.0,
            }),
            Err(GridError::OutOfDomain) => *status = SampleStatus::OutOfDomain,
            Err(GridError::PolarSingularity) => {
                *status = SampleStatus::PolarSingularity;
            }
            Err(error) => return Err(EngineError::Grid(error)),
        }
    }
    Ok((placements, initial_status))
}

type BoundaryPointLocations = (
    Vec<PointPlacement>,
    Vec<SampleStatus>,
    Vec<Option<HorizontalWeights>>,
);

fn locate_points_with_weights(
    window: &PreparedWindow,
    batch: &QueryBatch,
) -> Result<BoundaryPointLocations, EngineError> {
    let point_count = batch.points.len().map_err(EngineError::QueryPlan)?;
    let grid = RegularLatLonGrid::new(window.frames.before.metadata().grid.clone())
        .map_err(EngineError::Grid)?;
    let mut placements = Vec::with_capacity(point_count);
    let mut initial_status = vec![SampleStatus::Ok; point_count];
    let mut horizontal_support = vec![None; point_count];
    for (index, status) in initial_status.iter_mut().enumerate() {
        let longitude = batch.points.longitude_degrees[index];
        let latitude = batch.points.latitude_degrees[index];
        match grid.locate_cell_and_weights(longitude, latitude) {
            Ok((cell, weights)) => {
                placements.push(PointPlacement {
                    original_index: index,
                    domain: window.frames.before.metadata().domain.clone(),
                    cell,
                    tile_id: cell.0,
                });
                horizontal_support[index] = Some(weights);
            }
            Err(GridError::OutOfDomain) => *status = SampleStatus::OutOfDomain,
            Err(GridError::PolarSingularity) => {
                *status = SampleStatus::PolarSingularity;
            }
            Err(error) => return Err(EngineError::Grid(error)),
        }
    }
    Ok((placements, initial_status, horizontal_support))
}

fn prepare_layout(
    window: &PreparedWindow,
    placements: Vec<PointPlacement>,
    point_count: usize,
    plan: &QueryPlan,
    fixed_bytes: u64,
    execution_scratch_bytes_per_point: u64,
) -> Result<BatchLayout, EngineError> {
    let field_count = plan.fields().len();
    let field_bytes = u64::try_from(field_count)
        .ok()
        .and_then(|count| count.checked_mul(32))
        .ok_or(EngineError::MemoryEstimateOverflow)?;
    let explain_bytes = if plan.explain_mode() == ExplainMode::Full {
        let fixed = std::mem::size_of::<Option<ExplainRecord>>()
            .saturating_add(std::mem::size_of::<ExplainRecord>())
            .saturating_add(512);
        let fields = field_count.saturating_mul(std::mem::size_of::<ExplainField>() + 32);
        let before = &window.frames.before.metadata().id;
        let after = &window.frames.after.metadata().id;
        let mut strings = window.frames.before.metadata().domain.0.len()
            + before.domain.0.len()
            + before.profile_sha256.len()
            + before.content_sha256.len()
            + after.domain.0.len()
            + after.profile_sha256.len()
            + after.content_sha256.len()
            + plan.surface_layer_model().map_or(0, |model| model.0.len());
        for field in plan.fields() {
            if let FieldKey::Extension(extension) = field {
                strings = strings
                    .saturating_add(extension.namespace.len())
                    .saturating_add(extension.name.len());
            }
        }
        u64::try_from(fixed.saturating_add(fields).saturating_add(strings))
            .map_err(|_| EngineError::MemoryEstimateOverflow)?
    } else {
        0
    };
    let bytes_per_point = 96_u64
        .checked_add(field_bytes)
        .and_then(|value| value.checked_add(explain_bytes))
        .and_then(|value| value.checked_add(execution_scratch_bytes_per_point))
        .ok_or(EngineError::MemoryEstimateOverflow)?;
    let layout = BatchLayout::build(
        point_count,
        placements,
        window.execution_budget_bytes,
        ChunkMemoryModel {
            fixed_bytes,
            bytes_per_point,
            preferred_chunk_points: 65_536,
        },
    )
    .map_err(EngineError::Layout)?;
    Ok(layout)
}

#[derive(Clone, Debug)]
struct PreparedFrameStencils {
    frame: Arc<RawMetFrame>,
    by_cell: BTreeMap<crate::grid::CellId, PinGuard<ColumnStencil>>,
}

#[derive(Clone, Debug, Default)]
struct PreparedStencils {
    frames: Vec<PreparedFrameStencils>,
    resident_bytes: u64,
}

impl PreparedStencils {
    fn frame(&self, frame: &RawMetFrame) -> Option<&PreparedFrameStencils> {
        self.frames
            .iter()
            .find(|prepared| std::ptr::eq(prepared.frame.as_ref(), frame))
    }

    fn insert(
        &mut self,
        frame: Arc<RawMetFrame>,
        cell: crate::grid::CellId,
        pin: PinGuard<ColumnStencil>,
    ) {
        if let Some(prepared) = self
            .frames
            .iter_mut()
            .find(|prepared| Arc::ptr_eq(&prepared.frame, &frame))
        {
            prepared.by_cell.insert(cell, pin);
        } else {
            self.frames.push(PreparedFrameStencils {
                frame,
                by_cell: BTreeMap::from([(cell, pin)]),
            });
        }
    }
}

fn prepare_stencils(
    window: &PreparedWindow,
    batch: &QueryBatch,
    placements: &[PointPlacement],
    capabilities: crate::field::CapabilitySet,
    include_derivative_frames: bool,
) -> Result<PreparedStencils, EngineError> {
    let cache = window
        .column_cache()
        .ok_or(EngineError::ColumnCacheUnavailable)?;
    let mut cache = cache.lock().map_err(|_| EngineError::ColumnCachePoisoned)?;
    let representatives = stencil_representatives(placements);
    let frames = stencil_frames(window, include_derivative_frames);
    let mut prepared = PreparedStencils::default();
    for frame in frames {
        let mut by_cell = BTreeMap::new();
        for (cell, original_index) in &representatives {
            let key = stencil_cache_key(&frame, *cell, capabilities);
            let pin = if let Some(pin) = cache.get(&key) {
                pin
            } else {
                let stencil = build_column_stencil(&frame, *cell, *original_index, batch)?;
                cache.insert(key, stencil).map_err(EngineError::Cache)?
            };
            prepared.resident_bytes = prepared
                .resident_bytes
                .checked_add(pin.size_bytes())
                .ok_or(EngineError::MemoryEstimateOverflow)?;
            by_cell.insert(*cell, pin);
        }
        prepared
            .frames
            .push(PreparedFrameStencils { frame, by_cell });
    }
    cache
        .trim_unpinned_to(prepared.resident_bytes)
        .map_err(EngineError::Cache)?;
    Ok(prepared)
}

fn stencil_representatives(placements: &[PointPlacement]) -> BTreeMap<crate::grid::CellId, usize> {
    placements.iter().fold(
        BTreeMap::<crate::grid::CellId, usize>::new(),
        |mut values, placement| {
            values
                .entry(placement.cell)
                .or_insert(placement.original_index);
            values
        },
    )
}

fn stencil_frames(
    window: &PreparedWindow,
    include_derivative_frames: bool,
) -> Vec<Arc<RawMetFrame>> {
    let mut frames = vec![window.frames.before.clone()];
    if window.frames.after.metadata().id != window.frames.before.metadata().id {
        frames.push(window.frames.after.clone());
    }
    if include_derivative_frames {
        if let Some(frame) = &window.previous {
            frames.push(frame.clone());
        }
        if let Some(frame) = &window.next {
            frames.push(frame.clone());
        }
    }
    let mut seen = BTreeSet::new();
    frames.retain(|frame| seen.insert(frame.metadata().id.clone()));
    frames
}

fn stencil_cache_key(
    frame: &RawMetFrame,
    cell: crate::grid::CellId,
    capabilities: crate::field::CapabilitySet,
) -> CacheKey {
    let topology = match &frame.metadata().vertical {
        VerticalTopology::HybridPressure(_) => "hybrid",
        VerticalTopology::PressureLevels(_) => "pressure",
    };
    CacheKey {
        frame: frame.metadata().id.clone(),
        domain: frame.metadata().domain.clone(),
        cell,
        capabilities,
        algorithm: format!("{M3_MET_QUERY_ALGORITHM_ID}/column_stencil/{topology}"),
    }
}

fn build_column_stencil(
    frame: &RawMetFrame,
    cell: crate::grid::CellId,
    original_index: usize,
    batch: &QueryBatch,
) -> Result<Arc<ColumnStencil>, EngineError> {
    let _performance = PerformanceScope::enter(PerformanceStage::ColumnStencilBuild);
    ColumnStencil::build(ColumnRequest {
        frame,
        cell,
        longitude_degrees: batch.points.longitude_degrees[original_index],
        latitude_degrees: batch.points.latitude_degrees[original_index],
    })
    .map(Arc::new)
    .map_err(EngineError::Vertical)
}

fn prepare_boundary_stencils(
    window: &PreparedWindow,
    batch: &QueryBatch,
    placements: &[PointPlacement],
    capabilities: crate::field::CapabilitySet,
    workspace: &mut BatchWorkspace,
) -> Result<PreparedStencils, EngineError> {
    match prepare_boundary_stencils_once(window, batch, placements, capabilities, workspace) {
        Err(EngineError::Cache(CacheError::InsufficientBudget { .. })) => {
            workspace.boundary_stencils.clear();
            prepare_boundary_stencils_once(window, batch, placements, capabilities, workspace)
        }
        result => result,
    }
}

fn prepare_boundary_stencils_once(
    window: &PreparedWindow,
    batch: &QueryBatch,
    placements: &[PointPlacement],
    capabilities: crate::field::CapabilitySet,
    workspace: &mut BatchWorkspace,
) -> Result<PreparedStencils, EngineError> {
    let representatives = stencil_representatives(placements);
    let requirements = stencil_frames(window, false)
        .into_iter()
        .flat_map(|frame| {
            representatives.iter().map(move |(cell, original_index)| {
                let key = stencil_cache_key(&frame, *cell, capabilities);
                (frame.clone(), *cell, *original_index, key)
            })
        })
        .collect::<Vec<_>>();
    if requirements
        .iter()
        .any(|(_, _, _, key)| !workspace.boundary_stencils.entries.contains_key(key))
    {
        let cache = window
            .column_cache()
            .ok_or(EngineError::ColumnCacheUnavailable)?;
        let wait = PerformanceScope::enter(PerformanceStage::ColumnCacheLockWait);
        let mut cache = cache.lock().map_err(|_| EngineError::ColumnCachePoisoned)?;
        drop(wait);
        let _hold = PerformanceScope::enter(PerformanceStage::ColumnCacheLockHold);
        for (frame, cell, original_index, key) in &requirements {
            if workspace.boundary_stencils.entries.contains_key(key) {
                continue;
            }
            let pin = if let Some(pin) = cache.get(key) {
                pin
            } else {
                let stencil = build_column_stencil(frame, *cell, *original_index, batch)?;
                cache
                    .insert(key.clone(), stencil)
                    .map_err(EngineError::Cache)?
            };
            workspace.boundary_stencils.insert(key.clone(), pin);
        }
    }
    let mut prepared = PreparedStencils::default();
    for (frame, cell, _, key) in requirements {
        let pin = workspace
            .boundary_stencils
            .entries
            .get(&key)
            .cloned()
            .ok_or(EngineError::InvalidPreparedState)?;
        prepared.resident_bytes = prepared
            .resident_bytes
            .checked_add(pin.size_bytes())
            .ok_or(EngineError::MemoryEstimateOverflow)?;
        prepared.insert(frame, cell, pin);
    }
    Ok(prepared)
}

fn finalize_boundary_stencil_session(
    window: &PreparedWindow,
    workspace: &mut BatchWorkspace,
    stencils: &PreparedStencils,
    non_cache_execution_bytes: u64,
) -> Result<(), EngineError> {
    let required_bytes = stencils
        .resident_bytes
        .checked_add(non_cache_execution_bytes)
        .ok_or(EngineError::MemoryEstimateOverflow)?;
    if required_bytes > window.execution_budget_bytes {
        return Err(EngineError::Layout(LayoutError::InsufficientMemory {
            required_bytes,
            budget_bytes: window.execution_budget_bytes,
        }));
    }
    let allowed_cache_bytes = window
        .execution_budget_bytes
        .saturating_sub(non_cache_execution_bytes);
    if workspace.boundary_stencils.resident_bytes > allowed_cache_bytes {
        workspace.boundary_stencils.clear();
    }
    // Retain exact-key stencils from earlier boundary paths whenever the
    // current execution scratch leaves room for them. The cache itself owns
    // these bytes, so trimming it to the full remaining execution allowance
    // preserves the hard budget without discarding reusable unpinned entries.
    let target_bytes = allowed_cache_bytes;
    let cache = window
        .column_cache()
        .ok_or(EngineError::ColumnCacheUnavailable)?;
    let wait = PerformanceScope::enter(PerformanceStage::ColumnCacheLockWait);
    let mut cache = cache.lock().map_err(|_| EngineError::ColumnCachePoisoned)?;
    drop(wait);
    let _hold = PerformanceScope::enter(PerformanceStage::ColumnCacheLockHold);
    cache
        .trim_unpinned_to(target_bytes)
        .map(|_| ())
        .map_err(EngineError::Cache)
}

fn boundary_direct_execution_bytes(
    stencils: &PreparedStencils,
    point_count: usize,
) -> Result<u64, EngineError> {
    let maximum_levels = stencils
        .frames
        .iter()
        .flat_map(|frame| frame.by_cell.values())
        .map(|pin| pin.value().level_count())
        .max()
        .unwrap_or(0);
    // At most before, after, and blended boundary columns coexist. Each level
    // carries two f64 values plus one validity byte; 64 bytes/level is a
    // conservative allocation/accounting ceiling including Vec capacity.
    let column_scratch = u64::try_from(maximum_levels)
        .ok()
        .and_then(|levels| levels.checked_mul(64))
        .ok_or(EngineError::MemoryEstimateOverflow)?;
    let point_bytes = std::mem::size_of::<PointPlacement>()
        .saturating_add(std::mem::size_of::<Option<HorizontalWeights>>())
        .saturating_add(std::mem::size_of::<SampleStatus>() * 2)
        .saturating_add(std::mem::size_of::<Option<VerticalBounds>>())
        .saturating_add(std::mem::size_of::<Option<f64>>())
        .saturating_add(64);
    let output_scratch = u64::try_from(point_count)
        .ok()
        .and_then(|points| points.checked_mul(u64::try_from(point_bytes).ok()?))
        .ok_or(EngineError::MemoryEstimateOverflow)?;
    4_096_u64
        .checked_add(column_scratch)
        .and_then(|bytes| bytes.checked_add(output_scratch))
        .ok_or(EngineError::MemoryEstimateOverflow)
}

const TRANSPORT_FIELD_COUNT: usize = 8;

#[derive(Clone, Copy, Debug)]
struct FieldMetadata {
    quality: FieldQuality,
    provenance: ProvenanceId,
}

#[derive(Clone, Debug)]
struct ExecutionMetadata {
    table: Arc<ProvenanceTable>,
    upper_transport: [FieldMetadata; TRANSPORT_FIELD_COUNT],
    surface_transport: [FieldMetadata; TRANSPORT_FIELD_COUNT],
    mixed_transport: [FieldMetadata; TRANSPORT_FIELD_COUNT],
    generic_upper: Vec<FieldMetadata>,
    generic_surface: Vec<FieldMetadata>,
    generic_mixed: Vec<FieldMetadata>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QueryRoute {
    UpperAir,
    SurfaceLayer,
    TemporalMixedSurfaceUpper,
}

#[derive(Clone, Copy, Debug)]
struct FramePointSample {
    eastward_wind_m_s: f64,
    northward_wind_m_s: f64,
    pressure_pa: f64,
    temperature_k: f64,
    specific_humidity: f64,
    physical_specific_humidity: f64,
}

#[derive(Clone, Copy, Debug)]
struct TransportSample {
    eastward_wind_m_s: f64,
    northward_wind_m_s: f64,
    geometric_vertical_velocity_m_s: f64,
    pressure_pa: f64,
    temperature_k: f64,
    specific_humidity: f64,
    physical_specific_humidity: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowEndpoint {
    Before,
    After,
}

#[derive(Clone, Debug)]
struct TransportPointResult {
    values: [f64; TRANSPORT_FIELD_COUNT],
    valid: [bool; TRANSPORT_FIELD_COUNT],
    quality: [FieldQuality; TRANSPORT_FIELD_COUNT],
    provenance: [ProvenanceId; TRANSPORT_FIELD_COUNT],
    status: SampleStatus,
    bounds: Option<VerticalBounds>,
}

#[derive(Clone, Copy, Debug)]
struct BoundaryPointResult {
    status: SampleStatus,
    bounds: VerticalBounds,
    terrain_height_asl_m: Option<f64>,
}

impl TransportPointResult {
    fn invalid(
        status: SampleStatus,
        bounds: Option<VerticalBounds>,
        metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
    ) -> Self {
        Self {
            values: [0.0; TRANSPORT_FIELD_COUNT],
            valid: [false; TRANSPORT_FIELD_COUNT],
            quality: std::array::from_fn(|index| metadata[index].quality),
            provenance: std::array::from_fn(|index| metadata[index].provenance),
            status,
            bounds,
        }
    }
}

impl PreparedFrameStencils {
    fn stencil(&self, cell: crate::grid::CellId) -> Result<&ColumnStencil, EngineError> {
        self.by_cell
            .get(&cell)
            .map(PinGuard::value)
            .ok_or(EngineError::InvalidPreparedState)
    }

    fn column(
        &self,
        cell: crate::grid::CellId,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<ColumnGeometry, EngineError> {
        self.stencil(cell)?
            .sample(ColumnRequest {
                frame: &self.frame,
                cell,
                longitude_degrees,
                latitude_degrees,
            })
            .map_err(EngineError::Vertical)
    }

    fn shape(
        &self,
        cell: crate::grid::CellId,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<ColumnShape, EngineError> {
        self.stencil(cell)?
            .sample_shape(ColumnRequest {
                frame: &self.frame,
                cell,
                longitude_degrees,
                latitude_degrees,
            })
            .map_err(EngineError::Vertical)
    }

    fn shape_with_weights(
        &self,
        cell: crate::grid::CellId,
        weights: HorizontalWeights,
    ) -> Result<ColumnShape, EngineError> {
        self.stencil(cell)?
            .sample_shape_with_weights(&self.frame.metadata().id, cell, weights)
            .map_err(EngineError::Vertical)
    }

    fn level_sample_with_weights(
        &self,
        cell: crate::grid::CellId,
        weights: HorizontalWeights,
        shape: &ColumnShape,
        level: usize,
    ) -> Result<ColumnLevelSample, EngineError> {
        self.stencil(cell)?
            .sample_level_with_weights(&self.frame.metadata().id, cell, weights, shape, level)
            .map_err(EngineError::Vertical)
    }

    fn level_horizontal_weights_with_weights(
        &self,
        cell: crate::grid::CellId,
        weights: HorizontalWeights,
        shape: &ColumnShape,
        level: usize,
    ) -> Result<[f64; 4], EngineError> {
        self.stencil(cell)?
            .level_horizontal_weights_with_weights(
                &self.frame.metadata().id,
                cell,
                weights,
                shape,
                level,
            )
            .map_err(EngineError::Vertical)
    }
}

fn frame_stencils<'a>(
    prepared: &'a PreparedStencils,
    frame: &RawMetFrame,
) -> Result<&'a PreparedFrameStencils, EngineError> {
    prepared
        .frame(frame)
        .ok_or(EngineError::InvalidPreparedState)
}

fn blend_value(before: f64, after: f64, window: &PreparedWindow) -> Result<f64, EngineError> {
    let value = after.mul_add(window.after_weight, before * window.before_weight);
    value
        .is_finite()
        .then_some(value)
        .ok_or(EngineError::NumericalFailure)
}

fn blend_columns(
    before: &ColumnGeometry,
    after: &ColumnGeometry,
    window: &PreparedWindow,
) -> Result<ColumnGeometry, EngineError> {
    if before.pressure_pa().len() != after.pressure_pa().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    if window.is_exact_frame() {
        return Ok(before.clone());
    }
    let levels = before.pressure_pa().len();
    let mut pressure_pa = Vec::with_capacity(levels);
    let mut height_asl_m = Vec::with_capacity(levels);
    let mut temperature_k = Vec::with_capacity(levels);
    let mut specific_humidity = Vec::with_capacity(levels);
    let mut level_weights = Vec::with_capacity(levels);
    let mut valid = Vec::with_capacity(levels);
    for level in 0..levels {
        pressure_pa.push(blend_value(
            before.pressure_pa()[level],
            after.pressure_pa()[level],
            window,
        )?);
        height_asl_m.push(blend_value(
            before.height_asl_m()[level],
            after.height_asl_m()[level],
            window,
        )?);
        temperature_k.push(blend_value(
            before.temperature_k()[level],
            after.temperature_k()[level],
            window,
        )?);
        specific_humidity.push(blend_value(
            before.specific_humidity()[level],
            after.specific_humidity()[level],
            window,
        )?);
        let mut weights = [0.0; 4];
        for (corner, value) in weights.iter_mut().enumerate() {
            *value = blend_value(
                before.level_horizontal_weights()[level][corner],
                after.level_horizontal_weights()[level][corner],
                window,
            )?;
        }
        level_weights.push(weights);
        valid.push(before.validity().valid[level] && after.validity().valid[level]);
    }
    let physical_top = match (
        before.physical_model_top_asl_m(),
        after.physical_model_top_asl_m(),
    ) {
        (Some(left), Some(right)) => Some(blend_value(left, right, window)?),
        _ => None,
    };
    ColumnGeometry::new(
        Arc::from(pressure_pa),
        Arc::from(height_asl_m),
        Arc::from(temperature_k),
        Arc::from(specific_humidity),
        Arc::from(level_weights),
        crate::vertical::VerticalValidity {
            valid: Arc::from(valid),
        },
        blend_value(before.terrain_asl_m(), after.terrain_asl_m(), window)?,
        blend_value(
            before.surface_pressure_pa(),
            after.surface_pressure_pa(),
            window,
        )?,
        physical_top,
    )
    .map_err(EngineError::Vertical)
}

struct TargetColumnGeometry {
    geometry: ColumnGeometry,
    before: ColumnGeometry,
    after: ColumnGeometry,
    restrictive_transport_top_asl_m: f64,
    restrictive_transport_top_pressure_pa: f64,
}

struct TargetTransportColumn {
    before: ColumnShape,
    after: Option<ColumnShape>,
    blended: Option<ColumnShape>,
    restrictive_transport_top_asl_m: f64,
    restrictive_transport_top_pressure_pa: f64,
}

impl TargetTransportColumn {
    fn geometry(&self) -> &ColumnShape {
        self.blended.as_ref().unwrap_or(&self.before)
    }

    fn after(&self) -> &ColumnShape {
        self.after.as_ref().unwrap_or(&self.before)
    }
}

#[derive(Clone, Copy)]
struct SampledHybridColumn<'a> {
    view: HybridColumnView<'a>,
    terrain_asl_m: f64,
    surface_pressure_pa: f64,
    top: HybridLevelGeometry,
    bottom: HybridLevelGeometry,
    physical_model_top_asl_m: Option<f64>,
}

impl<'a> SampledHybridColumn<'a> {
    fn new(view: HybridColumnView<'a>) -> Result<Self, EngineError> {
        let levels = view.level_count();
        if levels < 2 {
            return Err(EngineError::InvalidPreparedState);
        }
        let top = view.level_geometry(0).map_err(EngineError::Vertical)?;
        let bottom = view
            .level_geometry(levels - 1)
            .map_err(EngineError::Vertical)?;
        Ok(Self {
            view,
            terrain_asl_m: view.terrain_asl_m().map_err(EngineError::Vertical)?,
            surface_pressure_pa: view.surface_pressure_pa().map_err(EngineError::Vertical)?,
            top,
            bottom,
            physical_model_top_asl_m: view.carries_physical_top().then_some(top.height_asl_m),
        })
    }
}

struct TargetHybridColumn<'a> {
    before: SampledHybridColumn<'a>,
    after: Option<SampledHybridColumn<'a>>,
    terrain_asl_m: f64,
    surface_pressure_pa: f64,
    top: HybridLevelGeometry,
    bottom: HybridLevelGeometry,
    physical_model_top_asl_m: Option<f64>,
    restrictive_transport_top_asl_m: f64,
    restrictive_transport_top_pressure_pa: f64,
}

impl TargetHybridColumn<'_> {
    fn after(&self) -> SampledHybridColumn<'_> {
        self.after.unwrap_or(self.before)
    }

    fn level_count(&self) -> usize {
        self.before.view.level_count()
    }

    fn level_geometry(
        &self,
        window: &PreparedWindow,
        level: usize,
    ) -> Result<HybridLevelGeometry, EngineError> {
        Ok(HybridLevelGeometry {
            pressure_pa: self.pressure_pa(window, level)?,
            height_asl_m: self.height_asl_m(window, level)?,
        })
    }

    fn pressure_pa(&self, window: &PreparedWindow, level: usize) -> Result<f64, EngineError> {
        let before = self
            .before
            .view
            .pressure_pa(level)
            .map_err(EngineError::Vertical)?;
        if window.is_exact_frame() {
            return Ok(before);
        }
        let after = self
            .after()
            .view
            .pressure_pa(level)
            .map_err(EngineError::Vertical)?;
        blend_value(before, after, window)
    }

    fn height_asl_m(&self, window: &PreparedWindow, level: usize) -> Result<f64, EngineError> {
        let before = self
            .before
            .view
            .height_asl_m(level)
            .map_err(EngineError::Vertical)?;
        if window.is_exact_frame() {
            return Ok(before);
        }
        let after = self
            .after()
            .view
            .height_asl_m(level)
            .map_err(EngineError::Vertical)?;
        blend_value(before, after, window)
    }
}

fn target_hybrid_column<'a>(
    window: &PreparedWindow,
    stencils: &'a PreparedStencils,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
) -> Result<Option<TargetHybridColumn<'a>>, EngineError> {
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let Some(before_view) = before_stencils
        .stencil(cell)?
        .hybrid_view_with_weights(&before_stencils.frame.metadata().id, cell, weights)
        .map_err(EngineError::Vertical)?
    else {
        return Ok(None);
    };
    let before = SampledHybridColumn::new(before_view)?;
    if window.is_exact_frame() {
        return Ok(Some(TargetHybridColumn {
            terrain_asl_m: before.terrain_asl_m,
            surface_pressure_pa: before.surface_pressure_pa,
            top: before.top,
            bottom: before.bottom,
            physical_model_top_asl_m: before.physical_model_top_asl_m,
            restrictive_transport_top_asl_m: before.top.height_asl_m,
            restrictive_transport_top_pressure_pa: before.top.pressure_pa,
            before,
            after: None,
        }));
    }
    let after_stencils = frame_stencils(stencils, &window.frames.after)?;
    let after_view = after_stencils
        .stencil(cell)?
        .hybrid_view_with_weights(&after_stencils.frame.metadata().id, cell, weights)
        .map_err(EngineError::Vertical)?
        .ok_or(EngineError::InvalidPreparedState)?;
    let after = SampledHybridColumn::new(after_view)?;
    if before.view.level_count() != after.view.level_count() {
        return Err(EngineError::InvalidPreparedState);
    }
    let top = HybridLevelGeometry {
        pressure_pa: blend_value(before.top.pressure_pa, after.top.pressure_pa, window)?,
        height_asl_m: blend_value(before.top.height_asl_m, after.top.height_asl_m, window)?,
    };
    let bottom = HybridLevelGeometry {
        pressure_pa: blend_value(before.bottom.pressure_pa, after.bottom.pressure_pa, window)?,
        height_asl_m: blend_value(
            before.bottom.height_asl_m,
            after.bottom.height_asl_m,
            window,
        )?,
    };
    Ok(Some(TargetHybridColumn {
        terrain_asl_m: blend_value(before.terrain_asl_m, after.terrain_asl_m, window)?,
        surface_pressure_pa: blend_value(
            before.surface_pressure_pa,
            after.surface_pressure_pa,
            window,
        )?,
        top,
        bottom,
        physical_model_top_asl_m: match (
            before.physical_model_top_asl_m,
            after.physical_model_top_asl_m,
        ) {
            (Some(before), Some(after)) => Some(blend_value(before, after, window)?),
            _ => None,
        },
        restrictive_transport_top_asl_m: before.top.height_asl_m.min(after.top.height_asl_m),
        restrictive_transport_top_pressure_pa: before.top.pressure_pa.max(after.top.pressure_pa),
        before,
        after: Some(after),
    }))
}

fn target_column(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<TargetColumnGeometry, EngineError> {
    let before = frame_stencils(stencils, &window.frames.before)?.column(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    if window.is_exact_frame() {
        let top = before
            .available_top_index()
            .map_err(EngineError::Vertical)?;
        return Ok(TargetColumnGeometry {
            restrictive_transport_top_asl_m: before.height_asl_m()[top],
            restrictive_transport_top_pressure_pa: before.pressure_pa()[top],
            geometry: before.clone(),
            after: before.clone(),
            before,
        });
    }
    let after = frame_stencils(stencils, &window.frames.after)?.column(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let geometry = blend_columns(&before, &after, window)?;
    let top = geometry
        .available_top_index()
        .map_err(EngineError::Vertical)?;
    Ok(TargetColumnGeometry {
        restrictive_transport_top_asl_m: before.height_asl_m()[top].min(after.height_asl_m()[top]),
        restrictive_transport_top_pressure_pa: before.pressure_pa()[top]
            .max(after.pressure_pa()[top]),
        geometry,
        before,
        after,
    })
}

fn target_transport_column(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
) -> Result<TargetTransportColumn, EngineError> {
    let before =
        frame_stencils(stencils, &window.frames.before)?.shape_with_weights(cell, weights)?;
    if window.is_exact_frame() {
        let top = before.first_valid_index().map_err(EngineError::Vertical)?;
        return Ok(TargetTransportColumn {
            restrictive_transport_top_asl_m: before.height_asl_m()[top],
            restrictive_transport_top_pressure_pa: before.pressure_pa()[top],
            before,
            after: None,
            blended: None,
        });
    }
    let after =
        frame_stencils(stencils, &window.frames.after)?.shape_with_weights(cell, weights)?;
    let blended = blend_shapes(&before, &after, window)?;
    let top = blended.first_valid_index().map_err(EngineError::Vertical)?;
    let restrictive_transport_top_asl_m = before.height_asl_m()[top].min(after.height_asl_m()[top]);
    let restrictive_transport_top_pressure_pa =
        before.pressure_pa()[top].max(after.pressure_pa()[top]);
    Ok(TargetTransportColumn {
        before,
        after: Some(after),
        blended: Some(blended),
        restrictive_transport_top_asl_m,
        restrictive_transport_top_pressure_pa,
    })
}

fn blend_shapes(
    before: &ColumnShape,
    after: &ColumnShape,
    window: &PreparedWindow,
) -> Result<ColumnShape, EngineError> {
    if before.pressure_pa().len() != after.pressure_pa().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    if window.is_exact_frame() {
        return Ok(before.clone());
    }
    let levels = before.pressure_pa().len();
    let mut pressure_pa = Vec::with_capacity(levels);
    let mut height_asl_m = Vec::with_capacity(levels);
    let mut valid = Vec::with_capacity(levels);
    for level in 0..levels {
        pressure_pa.push(blend_value(
            before.pressure_pa()[level],
            after.pressure_pa()[level],
            window,
        )?);
        height_asl_m.push(blend_value(
            before.height_asl_m()[level],
            after.height_asl_m()[level],
            window,
        )?);
        valid.push(before.validity()[level] && after.validity()[level]);
    }
    let physical_top = match (
        before.physical_model_top_asl_m(),
        after.physical_model_top_asl_m(),
    ) {
        (Some(left), Some(right)) => Some(blend_value(left, right, window)?),
        _ => None,
    };
    Ok(ColumnShape::from_sampled(
        pressure_pa,
        height_asl_m,
        valid,
        blend_value(before.terrain_asl_m(), after.terrain_asl_m(), window)?,
        blend_value(
            before.surface_pressure_pa(),
            after.surface_pressure_pa(),
            window,
        )?,
        physical_top,
    ))
}

struct TargetBoundaryColumnGeometry {
    geometry: ColumnShape,
    restrictive_transport_top_asl_m: f64,
    restrictive_transport_top_pressure_pa: f64,
}

fn target_boundary_column(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<TargetBoundaryColumnGeometry, EngineError> {
    let before = frame_stencils(stencils, &window.frames.before)?.shape(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    if window.is_exact_frame() {
        let top = before.first_valid_index().map_err(EngineError::Vertical)?;
        return Ok(TargetBoundaryColumnGeometry {
            restrictive_transport_top_asl_m: before.height_asl_m()[top],
            restrictive_transport_top_pressure_pa: before.pressure_pa()[top],
            geometry: before,
        });
    }
    let after = frame_stencils(stencils, &window.frames.after)?.shape(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let geometry = blend_shapes(&before, &after, window)?;
    let top = geometry
        .first_valid_index()
        .map_err(EngineError::Vertical)?;
    Ok(TargetBoundaryColumnGeometry {
        restrictive_transport_top_asl_m: before.height_asl_m()[top].min(after.height_asl_m()[top]),
        restrictive_transport_top_pressure_pa: before.pressure_pa()[top]
            .max(after.pressure_pa()[top]),
        geometry,
    })
}

fn target_boundary_column_with_weights(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
) -> Result<TargetBoundaryColumnGeometry, EngineError> {
    let before =
        frame_stencils(stencils, &window.frames.before)?.shape_with_weights(cell, weights)?;
    if window.is_exact_frame() {
        let top = before.first_valid_index().map_err(EngineError::Vertical)?;
        return Ok(TargetBoundaryColumnGeometry {
            restrictive_transport_top_asl_m: before.height_asl_m()[top],
            restrictive_transport_top_pressure_pa: before.pressure_pa()[top],
            geometry: before,
        });
    }
    let after =
        frame_stencils(stencils, &window.frames.after)?.shape_with_weights(cell, weights)?;
    let geometry = blend_shapes(&before, &after, window)?;
    let top = geometry
        .first_valid_index()
        .map_err(EngineError::Vertical)?;
    Ok(TargetBoundaryColumnGeometry {
        restrictive_transport_top_asl_m: before.height_asl_m()[top].min(after.height_asl_m()[top]),
        restrictive_transport_top_pressure_pa: before.pressure_pa()[top]
            .max(after.pressure_pa()[top]),
        geometry,
    })
}

fn vertical_bracket(
    column: &ColumnGeometry,
    coordinate: VerticalQuery,
    value: f64,
) -> Result<VerticalBracket, VerticalError> {
    match coordinate {
        VerticalQuery::AboveSeaLevel => column.locate_height_asl_m(value),
        VerticalQuery::AboveGround => column.locate_height_agl_m(value),
        VerticalQuery::Pressure => column.locate_pressure_pa(value),
    }
}

fn vertical_bracket_shape(
    column: &ColumnShape,
    coordinate: VerticalQuery,
    value: f64,
) -> Result<VerticalBracket, VerticalError> {
    match coordinate {
        VerticalQuery::AboveSeaLevel => column.locate_height_asl_m(value),
        VerticalQuery::AboveGround => column.locate_height_agl_m(value),
        VerticalQuery::Pressure => column.locate_pressure_pa(value),
    }
}

#[allow(clippy::too_many_arguments)]
fn locate_hybrid_vertical(
    levels: usize,
    terrain_asl_m: f64,
    surface_pressure_pa: f64,
    physical_model_top_asl_m: Option<f64>,
    top: HybridLevelGeometry,
    bottom: HybridLevelGeometry,
    coordinate: VerticalQuery,
    value: f64,
    mut level_coordinate: impl FnMut(usize) -> Result<f64, EngineError>,
) -> Result<VerticalBracket, EngineError> {
    if levels < 2 {
        return Err(EngineError::InvalidPreparedState);
    }
    let (query, descending) = match coordinate {
        VerticalQuery::AboveSeaLevel => (value, true),
        VerticalQuery::AboveGround => (terrain_asl_m + value, true),
        VerticalQuery::Pressure => (value, false),
    };
    if !query.is_finite() || (!descending && query <= 0.0) {
        return Err(EngineError::Vertical(VerticalError::NumericalFailure));
    }
    if descending {
        if query <= terrain_asl_m {
            return Err(EngineError::Vertical(VerticalError::BelowGround));
        }
        if physical_model_top_asl_m.is_some_and(|top| query > top) {
            return Err(EngineError::Vertical(VerticalError::AboveModelTop));
        }
        if query > top.height_asl_m {
            return Err(EngineError::Vertical(VerticalError::AboveAvailableTop));
        }
        if query < bottom.height_asl_m {
            return Err(EngineError::Vertical(VerticalError::SurfaceLayerRequired));
        }
    } else {
        if query > surface_pressure_pa {
            return Err(EngineError::Vertical(VerticalError::BelowGround));
        }
        if query < top.pressure_pa {
            return Err(EngineError::Vertical(VerticalError::AboveAvailableTop));
        }
        if query > bottom.pressure_pa {
            return Err(EngineError::Vertical(VerticalError::SurfaceLayerRequired));
        }
    }

    let mut low = 0usize;
    let mut high = levels;
    while low < high {
        let middle = low + (high - low) / 2;
        let coordinate = level_coordinate(middle)?;
        if (descending && coordinate > query) || (!descending && coordinate < query) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if low >= levels {
        return Err(EngineError::Vertical(VerticalError::NoValidBracket));
    }
    let second_coordinate = level_coordinate(low)?;
    if second_coordinate == query {
        return Ok(VerticalBracket {
            first: low,
            second: low,
            second_weight: 0.0,
        });
    }
    if low == 0 {
        return Err(EngineError::Vertical(VerticalError::NoValidBracket));
    }
    let first = low - 1;
    let first_coordinate = level_coordinate(first)?;
    let second_weight = if descending {
        (first_coordinate - query) / (first_coordinate - second_coordinate)
    } else {
        let lower_log = first_coordinate.ln();
        let upper_log = second_coordinate.ln();
        (query.ln() - lower_log) / (upper_log - lower_log)
    };
    if !second_weight.is_finite() {
        return Err(EngineError::Vertical(VerticalError::NumericalFailure));
    }
    Ok(VerticalBracket {
        first,
        second: low,
        second_weight,
    })
}

fn locate_sampled_hybrid_vertical(
    column: SampledHybridColumn<'_>,
    coordinate: VerticalQuery,
    value: f64,
) -> Result<VerticalBracket, EngineError> {
    locate_hybrid_vertical(
        column.view.level_count(),
        column.terrain_asl_m,
        column.surface_pressure_pa,
        column.physical_model_top_asl_m,
        column.top,
        column.bottom,
        coordinate,
        value,
        |level| match coordinate {
            VerticalQuery::Pressure => column
                .view
                .pressure_pa(level)
                .map_err(EngineError::Vertical),
            VerticalQuery::AboveSeaLevel | VerticalQuery::AboveGround => column
                .view
                .height_asl_m(level)
                .map_err(EngineError::Vertical),
        },
    )
}

fn locate_target_hybrid_vertical(
    window: &PreparedWindow,
    column: &TargetHybridColumn<'_>,
    coordinate: VerticalQuery,
    value: f64,
) -> Result<VerticalBracket, EngineError> {
    locate_hybrid_vertical(
        column.level_count(),
        column.terrain_asl_m,
        column.surface_pressure_pa,
        column.physical_model_top_asl_m,
        column.top,
        column.bottom,
        coordinate,
        value,
        |level| match coordinate {
            VerticalQuery::Pressure => column.pressure_pa(window, level),
            VerticalQuery::AboveSeaLevel | VerticalQuery::AboveGround => {
                column.height_asl_m(window, level)
            }
        },
    )
}

fn interpolate_logarithmic(bracket: VerticalBracket, values: &[f64]) -> Result<f64, EngineError> {
    let first = values
        .get(bracket.first)
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or(EngineError::NumericalFailure)?;
    let second = if bracket.first == bracket.second {
        first
    } else {
        values
            .get(bracket.second)
            .copied()
            .filter(|value| value.is_finite() && *value > 0.0)
            .ok_or(EngineError::NumericalFailure)?
    };
    interpolate_logarithmic_pair(first, second, bracket.second_weight)
}

fn interpolate_logarithmic_pair(
    first: f64,
    second: f64,
    second_weight: f64,
) -> Result<f64, EngineError> {
    if !first.is_finite()
        || first <= 0.0
        || !second.is_finite()
        || second <= 0.0
        || !second_weight.is_finite()
    {
        return Err(EngineError::NumericalFailure);
    }
    let first = first.ln();
    let logarithm = if second_weight == 0.0 {
        first
    } else {
        (second.ln() - first).mul_add(second_weight, first)
    };
    let value = logarithm.exp();
    value
        .is_finite()
        .then_some(value)
        .ok_or(EngineError::NumericalFailure)
}

fn field_3d_value(
    field: &RawField,
    level: usize,
    point: GridPoint,
) -> Result<(f64, bool), EngineError> {
    let ArrayLayout::Full3D { levels, ny, nx } = field.layout() else {
        return Err(EngineError::InvalidPreparedState);
    };
    if level >= levels || point.x >= nx || point.y >= ny {
        return Err(EngineError::InvalidPreparedState);
    }
    let index = (level * ny + point.y) * nx + point.x;
    let value = field
        .values()
        .get(index)
        .copied()
        .ok_or(EngineError::InvalidPreparedState)?;
    let valid = field
        .validity()
        .get(index)
        .ok_or(EngineError::InvalidPreparedState)?;
    Ok((value, valid))
}

fn field_2d_value(field: &RawField, point: GridPoint) -> Result<(f64, bool), EngineError> {
    let ArrayLayout::Horizontal2D { ny, nx } = field.layout() else {
        return Err(EngineError::InvalidPreparedState);
    };
    if point.x >= nx || point.y >= ny {
        return Err(EngineError::InvalidPreparedState);
    }
    let index = point.y * nx + point.x;
    let value = field
        .values()
        .get(index)
        .copied()
        .ok_or(EngineError::InvalidPreparedState)?;
    let valid = field
        .validity()
        .get(index)
        .ok_or(EngineError::InvalidPreparedState)?;
    Ok((value, valid))
}

fn source_coordinates(frame: &RawMetFrame, points: &[GridPoint; 4]) -> ([f64; 4], [f64; 4]) {
    let geometry = &frame.metadata().grid;
    (
        std::array::from_fn(|index| {
            geometry.longitude_origin_degrees
                + geometry.longitude_spacing_degrees * points[index].x as f64
        }),
        std::array::from_fn(|index| {
            geometry.latitude_origin_degrees
                + geometry.latitude_spacing_degrees * points[index].y as f64
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn sample_level_vector(
    frame_stencils: &PreparedFrameStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    level: usize,
    eastward_field: CanonicalField,
    northward_field: CanonicalField,
) -> Result<(f64, f64), EngineError> {
    if !column.validity().valid.get(level).copied().unwrap_or(false) {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    }
    let query_basis = spherical_vector_query_basis(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    sample_level_vector_with_weights(
        frame_stencils,
        cell,
        query_basis,
        level,
        column.level_horizontal_weights()[level],
        eastward_field,
        northward_field,
    )
}

#[allow(clippy::too_many_arguments)]
fn sample_level_vector_with_weights(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    query_basis: SphericalBasis,
    level: usize,
    horizontal_weights: [f64; 4],
    eastward_field: CanonicalField,
    northward_field: CanonicalField,
) -> Result<(f64, f64), EngineError> {
    let stencil = frame_stencils.stencil(cell)?;
    let eastward = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(eastward_field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(
            eastward_field,
        )))?;
    let northward = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(northward_field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(
            northward_field,
        )))?;
    let mut eastward_values = [0.0; 4];
    let mut northward_values = [0.0; 4];
    for (corner, point) in stencil.points().iter().copied().enumerate() {
        let (u, u_valid) = field_3d_value(eastward, level, point)?;
        let (v, v_valid) = field_3d_value(northward, level, point)?;
        if horizontal_weights[corner] > 0.0 && (!u_valid || !v_valid) {
            return Err(EngineError::Vertical(
                VerticalError::InvalidHorizontalSupport,
            ));
        }
        eastward_values[corner] = u;
        northward_values[corner] = v;
    }
    interpolate_spherical_vector_with_bases(
        stencil.source_bases(),
        query_basis,
        eastward_values,
        northward_values,
        horizontal_weights,
    )
    .map_err(EngineError::Grid)
}

fn sample_level_scalar(
    frame_stencils: &PreparedFrameStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    level: usize,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    if !column.validity().valid.get(level).copied().unwrap_or(false) {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    }
    sample_level_scalar_with_weights(
        frame_stencils,
        cell,
        level,
        column.level_horizontal_weights()[level],
        field,
    )
}

fn sample_level_scalar_with_weights(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    level: usize,
    horizontal_weights: [f64; 4],
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let source = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(field)))?;
    let stencil = frame_stencils.stencil(cell)?;
    let mut value = 0.0;
    for (corner, point) in stencil.points().iter().copied().enumerate() {
        let (corner_value, valid) = field_3d_value(source, level, point)?;
        let weight = horizontal_weights[corner];
        if weight > 0.0 && !valid {
            return Err(EngineError::Vertical(
                VerticalError::InvalidHorizontalSupport,
            ));
        }
        value = corner_value.mul_add(weight, value);
    }
    value
        .is_finite()
        .then_some(value)
        .ok_or(EngineError::NumericalFailure)
}

fn sample_frame_point(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<FramePointSample, EngineError> {
    let column = frame_stencils.column(cell, longitude_degrees, latitude_degrees)?;
    sample_frame_point_from_column(
        frame_stencils,
        &column,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )
}

#[allow(clippy::too_many_arguments)]
fn sample_frame_point_from_column(
    frame_stencils: &PreparedFrameStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<FramePointSample, EngineError> {
    let bracket =
        vertical_bracket(column, coordinate, vertical_value).map_err(EngineError::Vertical)?;
    let pressure_pa = if coordinate == VerticalQuery::Pressure {
        vertical_value
    } else {
        interpolate_logarithmic(bracket, column.pressure_pa())?
    };
    let temperature_k = bracket
        .interpolate(column.temperature_k())
        .map_err(EngineError::Vertical)?;
    let specific_humidity = bracket
        .interpolate(column.specific_humidity())
        .map_err(EngineError::Vertical)?;
    let first_physical_humidity = column
        .physical_specific_humidity_at(bracket.first)
        .map_err(EngineError::Vertical)?;
    let physical_specific_humidity = if bracket.first == bracket.second {
        first_physical_humidity
    } else {
        let second_physical_humidity = column
            .physical_specific_humidity_at(bracket.second)
            .map_err(EngineError::Vertical)?;
        (second_physical_humidity - first_physical_humidity)
            .mul_add(bracket.second_weight, first_physical_humidity)
    };
    let first_vector = sample_level_vector(
        frame_stencils,
        column,
        cell,
        longitude_degrees,
        latitude_degrees,
        bracket.first,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    let second_vector = if bracket.second == bracket.first {
        first_vector
    } else {
        sample_level_vector(
            frame_stencils,
            column,
            cell,
            longitude_degrees,
            latitude_degrees,
            bracket.second,
            CanonicalField::EastwardWind,
            CanonicalField::NorthwardWind,
        )?
    };
    let eastward_wind_m_s =
        (second_vector.0 - first_vector.0).mul_add(bracket.second_weight, first_vector.0);
    let northward_wind_m_s =
        (second_vector.1 - first_vector.1).mul_add(bracket.second_weight, first_vector.1);
    Ok(FramePointSample {
        eastward_wind_m_s,
        northward_wind_m_s,
        pressure_pa,
        temperature_k,
        specific_humidity,
        physical_specific_humidity,
    })
}

#[allow(clippy::too_many_arguments)]
fn sample_frame_point_from_shape(
    frame_stencils: &PreparedFrameStencils,
    shape: &ColumnShape,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<FramePointSample, EngineError> {
    let query_basis = spherical_vector_query_basis(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    let bracket =
        vertical_bracket_shape(shape, coordinate, vertical_value).map_err(EngineError::Vertical)?;
    let pressure_pa = if coordinate == VerticalQuery::Pressure {
        vertical_value
    } else {
        interpolate_logarithmic(bracket, shape.pressure_pa())?
    };
    let first = frame_stencils.level_sample_with_weights(cell, weights, shape, bracket.first)?;
    let second = if bracket.first == bracket.second {
        first
    } else {
        frame_stencils.level_sample_with_weights(cell, weights, shape, bracket.second)?
    };
    let temperature_k = (second.temperature_k - first.temperature_k)
        .mul_add(bracket.second_weight, first.temperature_k);
    let specific_humidity = (second.specific_humidity - first.specific_humidity)
        .mul_add(bracket.second_weight, first.specific_humidity);
    let first_physical_humidity = project_specific_humidity_nonnegative(first.specific_humidity)
        .map_err(|_| EngineError::NumericalFailure)?;
    let second_physical_humidity = project_specific_humidity_nonnegative(second.specific_humidity)
        .map_err(|_| EngineError::NumericalFailure)?;
    let physical_specific_humidity = (second_physical_humidity - first_physical_humidity)
        .mul_add(bracket.second_weight, first_physical_humidity);
    let first_vector = sample_level_vector_with_weights(
        frame_stencils,
        cell,
        query_basis,
        bracket.first,
        first.horizontal_weights,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    let second_vector = if bracket.first == bracket.second {
        first_vector
    } else {
        sample_level_vector_with_weights(
            frame_stencils,
            cell,
            query_basis,
            bracket.second,
            second.horizontal_weights,
            CanonicalField::EastwardWind,
            CanonicalField::NorthwardWind,
        )?
    };
    Ok(FramePointSample {
        eastward_wind_m_s: (second_vector.0 - first_vector.0)
            .mul_add(bracket.second_weight, first_vector.0),
        northward_wind_m_s: (second_vector.1 - first_vector.1)
            .mul_add(bracket.second_weight, first_vector.1),
        pressure_pa,
        temperature_k,
        specific_humidity,
        physical_specific_humidity,
    })
}

#[allow(clippy::too_many_arguments)]
fn sample_hybrid_frame_point(
    frame_stencils: &PreparedFrameStencils,
    column: SampledHybridColumn<'_>,
    cell: crate::grid::CellId,
    query_basis: SphericalBasis,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bracket: Result<VerticalBracket, EngineError>,
) -> Result<FramePointSample, EngineError> {
    let bracket = bracket?;
    let first_geometry = column
        .view
        .level_geometry(bracket.first)
        .map_err(EngineError::Vertical)?;
    let second_geometry = if bracket.first == bracket.second {
        first_geometry
    } else {
        column
            .view
            .level_geometry(bracket.second)
            .map_err(EngineError::Vertical)?
    };
    let pressure_pa = if coordinate == VerticalQuery::Pressure {
        vertical_value
    } else {
        interpolate_logarithmic_pair(
            first_geometry.pressure_pa,
            second_geometry.pressure_pa,
            bracket.second_weight,
        )?
    };
    let first = column
        .view
        .level_sample(bracket.first)
        .map_err(EngineError::Vertical)?;
    let second = if bracket.first == bracket.second {
        first
    } else {
        column
            .view
            .level_sample(bracket.second)
            .map_err(EngineError::Vertical)?
    };
    let temperature_k = (second.temperature_k - first.temperature_k)
        .mul_add(bracket.second_weight, first.temperature_k);
    let specific_humidity = (second.specific_humidity - first.specific_humidity)
        .mul_add(bracket.second_weight, first.specific_humidity);
    let first_physical_humidity = project_specific_humidity_nonnegative(first.specific_humidity)
        .map_err(|_| EngineError::NumericalFailure)?;
    let second_physical_humidity = project_specific_humidity_nonnegative(second.specific_humidity)
        .map_err(|_| EngineError::NumericalFailure)?;
    let physical_specific_humidity = (second_physical_humidity - first_physical_humidity)
        .mul_add(bracket.second_weight, first_physical_humidity);
    let first_vector = sample_level_vector_with_weights(
        frame_stencils,
        cell,
        query_basis,
        bracket.first,
        first.horizontal_weights,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    let second_vector = if bracket.first == bracket.second {
        first_vector
    } else {
        sample_level_vector_with_weights(
            frame_stencils,
            cell,
            query_basis,
            bracket.second,
            second.horizontal_weights,
            CanonicalField::EastwardWind,
            CanonicalField::NorthwardWind,
        )?
    };
    Ok(FramePointSample {
        eastward_wind_m_s: (second_vector.0 - first_vector.0)
            .mul_add(bracket.second_weight, first_vector.0),
        northward_wind_m_s: (second_vector.1 - first_vector.1)
            .mul_add(bracket.second_weight, first_vector.1),
        pressure_pa,
        temperature_k,
        specific_humidity,
        physical_specific_humidity,
    })
}

fn sample_surface_scalar_frame(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let grid = RegularLatLonGrid::new(frame_stencils.frame.metadata().grid.clone())
        .map_err(EngineError::Grid)?;
    let weights = grid
        .horizontal_weights(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    if grid
        .locate_cell(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?
        != cell
    {
        return Err(EngineError::InvalidPreparedState);
    }
    sample_surface_scalar_frame_with_weights(frame_stencils, cell, weights, field)
}

fn sample_surface_scalar_frame_with_weights(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    if weights.points != *frame_stencils.stencil(cell)?.points() {
        return Err(EngineError::InvalidPreparedState);
    }
    let source = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(field)))?;
    let mut value = 0.0;
    for (corner, point) in weights.points.iter().copied().enumerate() {
        let (corner_value, valid) = field_2d_value(source, point)?;
        if !valid {
            return Err(EngineError::Vertical(
                VerticalError::InvalidHorizontalSupport,
            ));
        }
        value = corner_value.mul_add(weights.weights[corner], value);
    }
    value
        .is_finite()
        .then_some(value)
        .ok_or(EngineError::NumericalFailure)
}

fn sample_surface_vector_frame(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    eastward_field: CanonicalField,
    northward_field: CanonicalField,
) -> Result<(f64, f64), EngineError> {
    let eastward = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(eastward_field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(
            eastward_field,
        )))?;
    let northward = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(northward_field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(
            northward_field,
        )))?;
    let grid = RegularLatLonGrid::new(frame_stencils.frame.metadata().grid.clone())
        .map_err(EngineError::Grid)?;
    let weights = grid
        .horizontal_weights(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    if grid
        .locate_cell(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?
        != cell
    {
        return Err(EngineError::InvalidPreparedState);
    }
    let mut eastward_values = [0.0; 4];
    let mut northward_values = [0.0; 4];
    for (corner, point) in weights.points.iter().copied().enumerate() {
        let (u, u_valid) = field_2d_value(eastward, point)?;
        let (v, v_valid) = field_2d_value(northward, point)?;
        if !u_valid || !v_valid {
            return Err(EngineError::Vertical(
                VerticalError::InvalidHorizontalSupport,
            ));
        }
        eastward_values[corner] = u;
        northward_values[corner] = v;
    }
    let (source_longitude_degrees, source_latitude_degrees) =
        source_coordinates(&frame_stencils.frame, &weights.points);
    interpolate_spherical_vector(
        source_longitude_degrees,
        source_latitude_degrees,
        eastward_values,
        northward_values,
        weights.weights,
        longitude_degrees,
        latitude_degrees,
    )
    .map_err(EngineError::Grid)
}

fn sample_surface_scalar_time(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let before = sample_surface_scalar_frame(
        frame_stencils(stencils, &window.frames.before)?,
        cell,
        longitude_degrees,
        latitude_degrees,
        field,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_surface_scalar_frame(
        frame_stencils(stencils, &window.frames.after)?,
        cell,
        longitude_degrees,
        latitude_degrees,
        field,
    )?;
    blend_value(before, after, window)
}

fn sample_surface_scalar_time_with_weights(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let before = sample_surface_scalar_frame_with_weights(
        frame_stencils(stencils, &window.frames.before)?,
        cell,
        weights,
        field,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_surface_scalar_frame_with_weights(
        frame_stencils(stencils, &window.frames.after)?,
        cell,
        weights,
        field,
    )?;
    blend_value(before, after, window)
}

fn sample_physical_two_metre_specific_humidity_frame(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<f64, EngineError> {
    let source = sample_surface_scalar_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TwoMetreSpecificHumidity,
    )?;
    project_specific_humidity_nonnegative(source).map_err(|_| EngineError::NumericalFailure)
}

fn sample_physical_two_metre_specific_humidity_time(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<f64, EngineError> {
    let before = sample_physical_two_metre_specific_humidity_frame(
        frame_stencils(stencils, &window.frames.before)?,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_physical_two_metre_specific_humidity_frame(
        frame_stencils(stencils, &window.frames.after)?,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    blend_value(before, after, window)
}

fn sample_surface_vector_time(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    eastward_field: CanonicalField,
    northward_field: CanonicalField,
) -> Result<(f64, f64), EngineError> {
    let before = sample_surface_vector_frame(
        frame_stencils(stencils, &window.frames.before)?,
        cell,
        longitude_degrees,
        latitude_degrees,
        eastward_field,
        northward_field,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_surface_vector_frame(
        frame_stencils(stencils, &window.frames.after)?,
        cell,
        longitude_degrees,
        latitude_degrees,
        eastward_field,
        northward_field,
    )?;
    Ok((
        blend_value(before.0, after.0, window)?,
        blend_value(before.1, after.1, window)?,
    ))
}

fn seconds_between(
    before: trajecta_case::model::time::Timestamp,
    after: trajecta_case::model::time::Timestamp,
) -> Result<f64, EngineError> {
    let seconds = (after.seconds_since_unix_epoch() - before.seconds_since_unix_epoch()) as f64
        + (f64::from(after.nanosecond()) - f64::from(before.nanosecond())) * 1.0e-9;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(EngineError::InvalidPreparedState);
    }
    Ok(seconds)
}

fn derivative_at(values: &[f64], coordinate: &[f64], index: usize) -> Result<f64, EngineError> {
    if values.len() != coordinate.len() || values.len() < 2 || index >= values.len() {
        return Err(EngineError::InvalidPreparedState);
    }
    let (left, right) = if index == 0 {
        (0, 1)
    } else if index + 1 == values.len() {
        (index - 1, index)
    } else {
        (index - 1, index + 1)
    };
    let denominator = coordinate[right] - coordinate[left];
    if !denominator.is_finite() || denominator == 0.0 {
        return Err(EngineError::NumericalFailure);
    }
    let derivative = (values[right] - values[left]) / denominator;
    derivative
        .is_finite()
        .then_some(derivative)
        .ok_or(EngineError::NumericalFailure)
}

#[allow(clippy::too_many_arguments)]
fn time_level_vector(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    before_column: &ColumnGeometry,
    after_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    level: usize,
) -> Result<(f64, f64), EngineError> {
    let before = sample_level_vector(
        frame_stencils(stencils, &window.frames.before)?,
        before_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        level,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_level_vector(
        frame_stencils(stencils, &window.frames.after)?,
        after_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        level,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    Ok((
        blend_value(before.0, after.0, window)?,
        blend_value(before.1, after.1, window)?,
    ))
}

fn time_level_scalar(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    before_column: &ColumnGeometry,
    after_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    level: usize,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let before = sample_level_scalar(
        frame_stencils(stencils, &window.frames.before)?,
        before_column,
        cell,
        level,
        field,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_level_scalar(
        frame_stencils(stencils, &window.frames.after)?,
        after_column,
        cell,
        level,
        field,
    )?;
    blend_value(before, after, window)
}

fn target_level_geometry(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    level: usize,
) -> Result<(crate::vertical::NativeLevelGeometry, f64, f64), EngineError> {
    let grid = RegularLatLonGrid::new(window.frames.before.metadata().grid.clone())
        .map_err(EngineError::Grid)?;
    let (located, weights) = grid
        .locate_cell_and_weights(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    if located != cell {
        return Err(EngineError::InvalidPreparedState);
    }
    target_level_geometry_with_weights(window, stencils, cell, weights, latitude_degrees, level)
}

fn target_level_geometry_with_weights(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
    latitude_degrees: f64,
    level: usize,
) -> Result<(crate::vertical::NativeLevelGeometry, f64, f64), EngineError> {
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let before_geometry = before_stencils
        .stencil(cell)?
        .level_geometry_with_weights(
            &before_stencils.frame,
            cell,
            weights,
            latitude_degrees,
            level,
        )
        .map_err(EngineError::Vertical)?;
    if !window.is_exact_frame() {
        let after_stencils = frame_stencils(stencils, &window.frames.after)?;
        let after_geometry = after_stencils
            .stencil(cell)?
            .level_geometry_with_weights(
                &after_stencils.frame,
                cell,
                weights,
                latitude_degrees,
                level,
            )
            .map_err(EngineError::Vertical)?;
        let seconds = seconds_between(
            window.frames.before.metadata().valid_time,
            window.frames.after.metadata().valid_time,
        )?;
        let blend = |left, right| blend_value(left, right, window);
        return Ok((
            crate::vertical::NativeLevelGeometry {
                pressure_pa: blend(before_geometry.pressure_pa, after_geometry.pressure_pa)?,
                height_asl_m: blend(before_geometry.height_asl_m, after_geometry.height_asl_m)?,
                pressure_eastward_gradient_pa_m: blend(
                    before_geometry.pressure_eastward_gradient_pa_m,
                    after_geometry.pressure_eastward_gradient_pa_m,
                )?,
                pressure_northward_gradient_pa_m: blend(
                    before_geometry.pressure_northward_gradient_pa_m,
                    after_geometry.pressure_northward_gradient_pa_m,
                )?,
                height_eastward_gradient: blend(
                    before_geometry.height_eastward_gradient,
                    after_geometry.height_eastward_gradient,
                )?,
                height_northward_gradient: blend(
                    before_geometry.height_northward_gradient,
                    after_geometry.height_northward_gradient,
                )?,
            },
            (after_geometry.height_asl_m - before_geometry.height_asl_m) / seconds,
            (after_geometry.pressure_pa - before_geometry.pressure_pa) / seconds,
        ));
    }
    let previous = window
        .previous
        .as_ref()
        .ok_or(EngineError::InvalidPreparedState)?;
    let next = window
        .next
        .as_ref()
        .ok_or(EngineError::InvalidPreparedState)?;
    let previous_stencils = frame_stencils(stencils, previous)?;
    let next_stencils = frame_stencils(stencils, next)?;
    let previous_geometry = previous_stencils
        .stencil(cell)?
        .level_geometry_with_weights(
            &previous_stencils.frame,
            cell,
            weights,
            latitude_degrees,
            level,
        )
        .map_err(EngineError::Vertical)?;
    let next_geometry = next_stencils
        .stencil(cell)?
        .level_geometry_with_weights(&next_stencils.frame, cell, weights, latitude_degrees, level)
        .map_err(EngineError::Vertical)?;
    let left_seconds = seconds_between(
        previous.metadata().valid_time,
        window.frames.before.metadata().valid_time,
    )?;
    let right_seconds = seconds_between(
        window.frames.before.metadata().valid_time,
        next.metadata().valid_time,
    )?;
    Ok((
        before_geometry,
        0.5 * ((before_geometry.height_asl_m - previous_geometry.height_asl_m) / left_seconds
            + (next_geometry.height_asl_m - before_geometry.height_asl_m) / right_seconds),
        0.5 * ((before_geometry.pressure_pa - previous_geometry.pressure_pa) / left_seconds
            + (next_geometry.pressure_pa - before_geometry.pressure_pa) / right_seconds),
    ))
}

struct GeometricWContext<'a> {
    window: &'a PreparedWindow,
    stencils: &'a PreparedStencils,
    target_column: &'a ColumnGeometry,
    before_column: &'a ColumnGeometry,
    after_column: &'a ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate_kind: NativeCoordinateKind,
    native_coordinate: &'a [f64],
    vertical_field: CanonicalField,
}

fn vertical_velocity_field(
    frame_stencils: &PreparedFrameStencils,
    coordinate_kind: NativeCoordinateKind,
) -> CanonicalField {
    match coordinate_kind {
        NativeCoordinateKind::PressurePa => CanonicalField::PressureVerticalVelocity,
        NativeCoordinateKind::HybridEta
            if frame_stencils
                .frame
                .fields()
                .get(&FieldKey::Canonical(CanonicalField::HybridVerticalVelocity))
                .is_some() =>
        {
            CanonicalField::HybridVerticalVelocity
        }
        NativeCoordinateKind::HybridEta => CanonicalField::PressureVerticalVelocity,
    }
}

fn prepare_geometric_w_context<'a>(
    window: &'a PreparedWindow,
    stencils: &'a PreparedStencils,
    target_column: &'a TargetColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<GeometricWContext<'a>, EngineError> {
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let stencil = before_stencils.stencil(cell)?;
    let (coordinate_kind, native_coordinate) = stencil.native_coordinate();
    if native_coordinate.len() != target_column.geometry.pressure_pa().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    let vertical_field = vertical_velocity_field(before_stencils, coordinate_kind);
    Ok(GeometricWContext {
        window,
        stencils,
        target_column: &target_column.geometry,
        before_column: &target_column.before,
        after_column: &target_column.after,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate_kind,
        native_coordinate,
        vertical_field,
    })
}

#[allow(clippy::too_many_arguments)]
fn derive_geometric_w_at_level(
    coordinate_kind: NativeCoordinateKind,
    vertical_field: CanonicalField,
    geometry: crate::vertical::NativeLevelGeometry,
    height_time_derivative_m_s: f64,
    pressure_time_derivative_pa_s: f64,
    eastward_wind_m_s: f64,
    northward_wind_m_s: f64,
    source_vertical_velocity: f64,
    height_coordinate_derivative: f64,
    pressure_coordinate_derivative: Option<f64>,
) -> Result<f64, EngineError> {
    let coordinate_velocity = match coordinate_kind {
        NativeCoordinateKind::PressurePa => source_vertical_velocity,
        NativeCoordinateKind::HybridEta
            if vertical_field == CanonicalField::HybridVerticalVelocity =>
        {
            source_vertical_velocity
        }
        NativeCoordinateKind::HybridEta => native_coordinate_velocity_from_omega(
            source_vertical_velocity,
            pressure_time_derivative_pa_s,
            eastward_wind_m_s,
            northward_wind_m_s,
            geometry.pressure_eastward_gradient_pa_m,
            geometry.pressure_northward_gradient_pa_m,
            pressure_coordinate_derivative.ok_or(EngineError::InvalidPreparedState)?,
        )
        .map_err(|_| EngineError::NumericalFailure)?,
    };
    geometric_vertical_velocity_m_s(KinematicVerticalVelocityInput {
        height_time_derivative_m_s,
        eastward_wind_m_s,
        northward_wind_m_s,
        height_eastward_gradient: geometry.height_eastward_gradient,
        height_northward_gradient: geometry.height_northward_gradient,
        coordinate_velocity,
        height_coordinate_derivative,
    })
    .map_err(|_| EngineError::NumericalFailure)
}

fn geometric_w_level(
    context: &GeometricWContext<'_>,
    level: usize,
) -> Result<Option<f64>, EngineError> {
    if !context
        .target_column
        .validity()
        .valid
        .get(level)
        .copied()
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let result = (|| {
        let (geometry, height_time_derivative_m_s, pressure_time_derivative_pa_s) =
            target_level_geometry(
                context.window,
                context.stencils,
                context.cell,
                context.longitude_degrees,
                context.latitude_degrees,
                level,
            )?;
        let (eastward_wind_m_s, northward_wind_m_s) = time_level_vector(
            context.window,
            context.stencils,
            context.before_column,
            context.after_column,
            context.cell,
            context.longitude_degrees,
            context.latitude_degrees,
            level,
        )?;
        let source_vertical_velocity = time_level_scalar(
            context.window,
            context.stencils,
            context.before_column,
            context.after_column,
            context.cell,
            level,
            context.vertical_field,
        )?;
        let height_coordinate_derivative = derivative_at(
            context.target_column.height_asl_m(),
            context.native_coordinate,
            level,
        )?;
        let pressure_coordinate_derivative = (context.coordinate_kind
            == NativeCoordinateKind::HybridEta
            && context.vertical_field != CanonicalField::HybridVerticalVelocity)
            .then(|| {
                derivative_at(
                    context.target_column.pressure_pa(),
                    context.native_coordinate,
                    level,
                )
            })
            .transpose()?;
        derive_geometric_w_at_level(
            context.coordinate_kind,
            context.vertical_field,
            geometry,
            height_time_derivative_m_s,
            pressure_time_derivative_pa_s,
            eastward_wind_m_s,
            northward_wind_m_s,
            source_vertical_velocity,
            height_coordinate_derivative,
            pressure_coordinate_derivative,
        )
    })();
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if local_status_for_error(&error).is_some() => Ok(None),
        Err(error) => Err(error),
    }
}

fn geometric_w_profile(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &TargetColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<(Vec<f64>, Vec<bool>), EngineError> {
    let context = prepare_geometric_w_context(
        window,
        stencils,
        target_column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let levels = target_column.geometry.pressure_pa().len();
    let mut values = vec![0.0; levels];
    let mut valid = vec![false; levels];
    for level in 0..levels {
        if let Some(value) = geometric_w_level(&context, level)? {
            values[level] = value;
            valid[level] = true;
        }
    }
    Ok((values, valid))
}

fn geometric_w_at_bracket(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &TargetColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    bracket: VerticalBracket,
) -> Result<Option<f64>, EngineError> {
    let context = prepare_geometric_w_context(
        window,
        stencils,
        target_column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let Some(first) = geometric_w_level(&context, bracket.first)? else {
        return Ok(None);
    };
    if bracket.first == bracket.second {
        return Ok(Some(first));
    }
    let Some(second) = geometric_w_level(&context, bracket.second)? else {
        return Ok(None);
    };
    let value = (second - first).mul_add(bracket.second_weight, first);
    value
        .is_finite()
        .then_some(Some(value))
        .ok_or(EngineError::NumericalFailure)
}

#[allow(clippy::too_many_arguments)]
fn time_level_vector_from_shapes(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetTransportColumn,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    level: usize,
) -> Result<(f64, f64), EngineError> {
    let query_basis = spherical_vector_query_basis(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let before_weights = before_stencils.level_horizontal_weights_with_weights(
        cell,
        weights,
        &column.before,
        level,
    )?;
    let before = sample_level_vector_with_weights(
        before_stencils,
        cell,
        query_basis,
        level,
        before_weights,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after_stencils = frame_stencils(stencils, &window.frames.after)?;
    let after_weights = after_stencils.level_horizontal_weights_with_weights(
        cell,
        weights,
        column.after(),
        level,
    )?;
    let after = sample_level_vector_with_weights(
        after_stencils,
        cell,
        query_basis,
        level,
        after_weights,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    Ok((
        blend_value(before.0, after.0, window)?,
        blend_value(before.1, after.1, window)?,
    ))
}

fn time_level_scalar_from_shapes(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetTransportColumn,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    level: usize,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let before_weights = before_stencils.level_horizontal_weights_with_weights(
        cell,
        weights,
        &column.before,
        level,
    )?;
    let before =
        sample_level_scalar_with_weights(before_stencils, cell, level, before_weights, field)?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after_stencils = frame_stencils(stencils, &window.frames.after)?;
    let after_weights = after_stencils.level_horizontal_weights_with_weights(
        cell,
        weights,
        column.after(),
        level,
    )?;
    let after =
        sample_level_scalar_with_weights(after_stencils, cell, level, after_weights, field)?;
    blend_value(before, after, window)
}

#[allow(clippy::too_many_arguments)]
fn time_level_vector_from_hybrid(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    query_basis: SphericalBasis,
    level: usize,
) -> Result<(f64, f64), EngineError> {
    let before = sample_level_vector_with_weights(
        frame_stencils(stencils, &window.frames.before)?,
        cell,
        query_basis,
        level,
        weights.weights,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_level_vector_with_weights(
        frame_stencils(stencils, &window.frames.after)?,
        cell,
        query_basis,
        level,
        weights.weights,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    Ok((
        blend_value(before.0, after.0, window)?,
        blend_value(before.1, after.1, window)?,
    ))
}

fn time_level_scalar_from_hybrid(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    level: usize,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let before = sample_level_scalar_with_weights(
        frame_stencils(stencils, &window.frames.before)?,
        cell,
        level,
        weights.weights,
        field,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_level_scalar_with_weights(
        frame_stencils(stencils, &window.frames.after)?,
        cell,
        level,
        weights.weights,
        field,
    )?;
    blend_value(before, after, window)
}

fn target_hybrid_coordinate_derivatives(
    window: &PreparedWindow,
    column: &TargetHybridColumn<'_>,
    level: usize,
) -> Result<(f64, f64), EngineError> {
    let coordinate = column.before.view.native_coordinate();
    if coordinate.len() != column.level_count() || coordinate.len() < 2 || level >= coordinate.len()
    {
        return Err(EngineError::InvalidPreparedState);
    }
    let (left, right) = if level == 0 {
        (0, 1)
    } else if level + 1 == coordinate.len() {
        (level - 1, level)
    } else {
        (level - 1, level + 1)
    };
    let denominator = coordinate[right] - coordinate[left];
    if !denominator.is_finite() || denominator == 0.0 {
        return Err(EngineError::NumericalFailure);
    }
    let left = column.level_geometry(window, left)?;
    let right = column.level_geometry(window, right)?;
    let height = (right.height_asl_m - left.height_asl_m) / denominator;
    let pressure = (right.pressure_pa - left.pressure_pa) / denominator;
    if !height.is_finite() || !pressure.is_finite() {
        return Err(EngineError::NumericalFailure);
    }
    Ok((height, pressure))
}

#[allow(clippy::too_many_arguments)]
fn geometric_w_level_from_hybrid(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetHybridColumn<'_>,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    query_basis: SphericalBasis,
    latitude_degrees: f64,
    level: usize,
) -> Result<Option<f64>, EngineError> {
    let result = (|| {
        let before_stencils = frame_stencils(stencils, &window.frames.before)?;
        let (coordinate_kind, native_coordinate) =
            before_stencils.stencil(cell)?.native_coordinate();
        if coordinate_kind != NativeCoordinateKind::HybridEta
            || native_coordinate.len() != column.level_count()
        {
            return Err(EngineError::InvalidPreparedState);
        }
        let vertical_field = vertical_velocity_field(before_stencils, coordinate_kind);
        let (geometry, height_time_derivative_m_s, pressure_time_derivative_pa_s) =
            target_level_geometry_with_weights(
                window,
                stencils,
                cell,
                weights,
                latitude_degrees,
                level,
            )?;
        let (eastward_wind_m_s, northward_wind_m_s) =
            time_level_vector_from_hybrid(window, stencils, weights, cell, query_basis, level)?;
        let source_vertical_velocity =
            time_level_scalar_from_hybrid(window, stencils, weights, cell, level, vertical_field)?;
        let (height_coordinate_derivative, pressure_coordinate_derivative) =
            target_hybrid_coordinate_derivatives(window, column, level)?;
        derive_geometric_w_at_level(
            coordinate_kind,
            vertical_field,
            geometry,
            height_time_derivative_m_s,
            pressure_time_derivative_pa_s,
            eastward_wind_m_s,
            northward_wind_m_s,
            source_vertical_velocity,
            height_coordinate_derivative,
            (vertical_field != CanonicalField::HybridVerticalVelocity)
                .then_some(pressure_coordinate_derivative),
        )
    })();
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if local_status_for_error(&error).is_some() => Ok(None),
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn geometric_w_at_bracket_from_hybrid(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetHybridColumn<'_>,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    query_basis: SphericalBasis,
    latitude_degrees: f64,
    bracket: VerticalBracket,
) -> Result<Option<f64>, EngineError> {
    let Some(first) = geometric_w_level_from_hybrid(
        window,
        stencils,
        column,
        weights,
        cell,
        query_basis,
        latitude_degrees,
        bracket.first,
    )?
    else {
        return Ok(None);
    };
    if bracket.first == bracket.second {
        return Ok(Some(first));
    }
    let Some(second) = geometric_w_level_from_hybrid(
        window,
        stencils,
        column,
        weights,
        cell,
        query_basis,
        latitude_degrees,
        bracket.second,
    )?
    else {
        return Ok(None);
    };
    let value = (second - first).mul_add(bracket.second_weight, first);
    value
        .is_finite()
        .then_some(Some(value))
        .ok_or(EngineError::NumericalFailure)
}

#[allow(clippy::too_many_arguments)]
fn geometric_w_level_from_shapes(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetTransportColumn,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    level: usize,
) -> Result<Option<f64>, EngineError> {
    if !column
        .geometry()
        .validity()
        .get(level)
        .copied()
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let result = (|| {
        let before_stencils = frame_stencils(stencils, &window.frames.before)?;
        let stencil = before_stencils.stencil(cell)?;
        let (coordinate_kind, native_coordinate) = stencil.native_coordinate();
        if native_coordinate.len() != column.geometry().pressure_pa().len() {
            return Err(EngineError::InvalidPreparedState);
        }
        let vertical_field = vertical_velocity_field(before_stencils, coordinate_kind);
        let (geometry, height_time_derivative_m_s, pressure_time_derivative_pa_s) =
            target_level_geometry_with_weights(
                window,
                stencils,
                cell,
                weights,
                latitude_degrees,
                level,
            )?;
        let (eastward_wind_m_s, northward_wind_m_s) = time_level_vector_from_shapes(
            window,
            stencils,
            column,
            weights,
            cell,
            longitude_degrees,
            latitude_degrees,
            level,
        )?;
        let source_vertical_velocity = time_level_scalar_from_shapes(
            window,
            stencils,
            column,
            weights,
            cell,
            level,
            vertical_field,
        )?;
        let height_coordinate_derivative =
            derivative_at(column.geometry().height_asl_m(), native_coordinate, level)?;
        let pressure_coordinate_derivative = (coordinate_kind == NativeCoordinateKind::HybridEta
            && vertical_field != CanonicalField::HybridVerticalVelocity)
            .then(|| derivative_at(column.geometry().pressure_pa(), native_coordinate, level))
            .transpose()?;
        derive_geometric_w_at_level(
            coordinate_kind,
            vertical_field,
            geometry,
            height_time_derivative_m_s,
            pressure_time_derivative_pa_s,
            eastward_wind_m_s,
            northward_wind_m_s,
            source_vertical_velocity,
            height_coordinate_derivative,
            pressure_coordinate_derivative,
        )
    })();
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if local_status_for_error(&error).is_some() => Ok(None),
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn geometric_w_at_bracket_from_shapes(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetTransportColumn,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    bracket: VerticalBracket,
) -> Result<Option<f64>, EngineError> {
    let Some(first) = geometric_w_level_from_shapes(
        window,
        stencils,
        column,
        weights,
        cell,
        longitude_degrees,
        latitude_degrees,
        bracket.first,
    )?
    else {
        return Ok(None);
    };
    if bracket.first == bracket.second {
        return Ok(Some(first));
    }
    let Some(second) = geometric_w_level_from_shapes(
        window,
        stencils,
        column,
        weights,
        cell,
        longitude_degrees,
        latitude_degrees,
        bracket.second,
    )?
    else {
        return Ok(None);
    };
    let value = (second - first).mul_add(bracket.second_weight, first);
    value
        .is_finite()
        .then_some(Some(value))
        .ok_or(EngineError::NumericalFailure)
}

fn terrain_vertical_velocity(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<f64, EngineError> {
    let geometry_for =
        |frame: &RawMetFrame| -> Result<crate::vertical::TerrainGeometry, EngineError> {
            let prepared = frame_stencils(stencils, frame)?;
            prepared
                .stencil(cell)?
                .terrain_geometry(ColumnRequest {
                    frame: &prepared.frame,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                })
                .map_err(EngineError::Vertical)
        };
    let before = geometry_for(&window.frames.before)?;
    let terrain = if window.is_exact_frame() {
        before
    } else {
        let after = geometry_for(&window.frames.after)?;
        crate::vertical::TerrainGeometry {
            height_asl_m: blend_value(before.height_asl_m, after.height_asl_m, window)?,
            eastward_gradient: blend_value(
                before.eastward_gradient,
                after.eastward_gradient,
                window,
            )?,
            northward_gradient: blend_value(
                before.northward_gradient,
                after.northward_gradient,
                window,
            )?,
        }
    };
    let wind = sample_surface_vector_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TenMetreEastwardWind,
        CanonicalField::TenMetreNorthwardWind,
    )?;
    terrain_following_surface_velocity_m_s(
        wind.0,
        wind.1,
        terrain.eastward_gradient,
        terrain.northward_gradient,
    )
    .map_err(|_| EngineError::NumericalFailure)
}

fn terrain_vertical_velocity_frame(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<f64, EngineError> {
    let terrain = frame_stencils
        .stencil(cell)?
        .terrain_geometry(ColumnRequest {
            frame: &frame_stencils.frame,
            cell,
            longitude_degrees,
            latitude_degrees,
        })
        .map_err(EngineError::Vertical)?;
    let wind = sample_surface_vector_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TenMetreEastwardWind,
        CanonicalField::TenMetreNorthwardWind,
    )?;
    terrain_following_surface_velocity_m_s(
        wind.0,
        wind.1,
        terrain.eastward_gradient,
        terrain.northward_gradient,
    )
    .map_err(|_| EngineError::NumericalFailure)
}

#[allow(clippy::too_many_arguments)]
fn endpoint_geometric_vertical_velocity_m_s(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    level: usize,
) -> Result<f64, EngineError> {
    if window.is_exact_frame() || !column.validity().valid.get(level).copied().unwrap_or(false) {
        return Err(EngineError::InvalidPreparedState);
    }
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let after_stencils = frame_stencils(stencils, &window.frames.after)?;
    let level_geometry = |prepared: &PreparedFrameStencils| {
        prepared
            .stencil(cell)?
            .level_geometry(
                ColumnRequest {
                    frame: &prepared.frame,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                },
                level,
            )
            .map_err(EngineError::Vertical)
    };
    let before_geometry = level_geometry(before_stencils)?;
    let after_geometry = level_geometry(after_stencils)?;
    let seconds = seconds_between(
        window.frames.before.metadata().valid_time,
        window.frames.after.metadata().valid_time,
    )?;
    let height_time_derivative_m_s =
        (after_geometry.height_asl_m - before_geometry.height_asl_m) / seconds;
    let pressure_time_derivative_pa_s =
        (after_geometry.pressure_pa - before_geometry.pressure_pa) / seconds;
    let prepared = match endpoint {
        WindowEndpoint::Before => before_stencils,
        WindowEndpoint::After => after_stencils,
    };
    let geometry = match endpoint {
        WindowEndpoint::Before => before_geometry,
        WindowEndpoint::After => after_geometry,
    };
    let (eastward_wind_m_s, northward_wind_m_s) = sample_level_vector(
        prepared,
        column,
        cell,
        longitude_degrees,
        latitude_degrees,
        level,
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
    )?;
    let vertical_field = vertical_velocity_source(window);
    let source_vertical_velocity =
        sample_level_scalar(prepared, column, cell, level, vertical_field)?;
    let (coordinate_kind, native_coordinate) = prepared.stencil(cell)?.native_coordinate();
    if native_coordinate.len() != column.height_asl_m().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    let height_coordinate_derivative =
        derivative_at(column.height_asl_m(), native_coordinate, level)?;
    let coordinate_velocity = match coordinate_kind {
        NativeCoordinateKind::PressurePa => source_vertical_velocity,
        NativeCoordinateKind::HybridEta
            if vertical_field == CanonicalField::HybridVerticalVelocity =>
        {
            source_vertical_velocity
        }
        NativeCoordinateKind::HybridEta => {
            let pressure_coordinate_derivative =
                derivative_at(column.pressure_pa(), native_coordinate, level)?;
            native_coordinate_velocity_from_omega(
                source_vertical_velocity,
                pressure_time_derivative_pa_s,
                eastward_wind_m_s,
                northward_wind_m_s,
                geometry.pressure_eastward_gradient_pa_m,
                geometry.pressure_northward_gradient_pa_m,
                pressure_coordinate_derivative,
            )
            .map_err(|_| EngineError::NumericalFailure)?
        }
    };
    geometric_vertical_velocity_m_s(KinematicVerticalVelocityInput {
        height_time_derivative_m_s,
        eastward_wind_m_s,
        northward_wind_m_s,
        height_eastward_gradient: geometry.height_eastward_gradient,
        height_northward_gradient: geometry.height_northward_gradient,
        coordinate_velocity,
        height_coordinate_derivative,
    })
    .map_err(|_| EngineError::NumericalFailure)
}

fn is_incomplete_transport_level(error: &EngineError) -> bool {
    matches!(
        error,
        EngineError::Vertical(
            VerticalError::InvalidHorizontalSupport | VerticalError::InvalidVerticalColumn
        )
    )
}

#[derive(Clone, Copy, Debug)]
struct EndpointTransportAnchor {
    level: usize,
    wind_m_s: (f64, f64),
    geometric_vertical_velocity_m_s: f64,
}

#[allow(clippy::too_many_arguments)]
fn lowest_complete_endpoint_transport_anchor(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    frame_stencils: &PreparedFrameStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<EndpointTransportAnchor, EngineError> {
    for level in (0..column.pressure_pa().len()).rev() {
        if !column.validity().valid[level] {
            continue;
        }
        let wind_m_s = match sample_level_vector(
            frame_stencils,
            column,
            cell,
            longitude_degrees,
            latitude_degrees,
            level,
            CanonicalField::EastwardWind,
            CanonicalField::NorthwardWind,
        ) {
            Ok(value) => value,
            Err(error) if is_incomplete_transport_level(&error) => continue,
            Err(error) => return Err(error),
        };
        let geometric_vertical_velocity_m_s = match endpoint_geometric_vertical_velocity_m_s(
            window,
            stencils,
            endpoint,
            column,
            cell,
            longitude_degrees,
            latitude_degrees,
            level,
        ) {
            Ok(value) => value,
            Err(error) if is_incomplete_transport_level(&error) => continue,
            Err(error) => return Err(error),
        };
        return Ok(EndpointTransportAnchor {
            level,
            wind_m_s,
            geometric_vertical_velocity_m_s,
        });
    }
    Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn))
}

#[allow(clippy::too_many_arguments)]
fn lowest_complete_time_transport_anchor(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &ColumnGeometry,
    before_column: &ColumnGeometry,
    after_column: &ColumnGeometry,
    w_valid: &[bool],
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<(usize, (f64, f64)), EngineError> {
    for level in (0..target_column.pressure_pa().len()).rev() {
        if !target_column.validity().valid[level] || !w_valid.get(level).copied().unwrap_or(false) {
            continue;
        }
        match time_level_vector(
            window,
            stencils,
            before_column,
            after_column,
            cell,
            longitude_degrees,
            latitude_degrees,
            level,
        ) {
            Ok(wind_m_s) => return Ok((level, wind_m_s)),
            Err(error) if is_incomplete_transport_level(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn))
}

fn has_field_at_query_time(window: &PreparedWindow, field: CanonicalField) -> bool {
    let key = FieldKey::Canonical(field);
    window.frames.before.fields().get(&key).is_some()
        && (window.is_exact_frame() || window.frames.after.fields().get(&key).is_some())
}

fn transport_field(index: usize) -> CanonicalField {
    [
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
        CanonicalField::GeometricVerticalVelocity,
        CanonicalField::AirPressure,
        CanonicalField::AirTemperature,
        CanonicalField::SpecificHumidity,
        CanonicalField::AirDensity,
        CanonicalField::GeometricTerrainHeight,
    ][index]
}

fn vertical_velocity_source(window: &PreparedWindow) -> CanonicalField {
    if has_field_at_query_time(window, CanonicalField::HybridVerticalVelocity) {
        CanonicalField::HybridVerticalVelocity
    } else {
        CanonicalField::PressureVerticalVelocity
    }
}

fn terrain_source(window: &PreparedWindow) -> CanonicalField {
    if has_field_at_query_time(window, CanonicalField::GeometricTerrainHeight) {
        CanonicalField::GeometricTerrainHeight
    } else {
        CanonicalField::SurfaceGeopotential
    }
}

fn column_input_fields(window: &PreparedWindow) -> Vec<CanonicalField> {
    let mut fields = vec![
        CanonicalField::AirTemperature,
        CanonicalField::SpecificHumidity,
        CanonicalField::SurfacePressure,
    ];
    match &window.frames.before.metadata().vertical {
        VerticalTopology::HybridPressure(_) => fields.push(CanonicalField::SurfaceGeopotential),
        VerticalTopology::PressureLevels(_) => {
            fields.push(
                if has_field_at_query_time(window, CanonicalField::GeometricHeight) {
                    CanonicalField::GeometricHeight
                } else {
                    CanonicalField::GeopotentialHeight
                },
            );
            fields.push(terrain_source(window));
        }
    }
    fields
}

fn upper_w_input_fields(window: &PreparedWindow) -> Vec<CanonicalField> {
    let mut fields = column_input_fields(window);
    fields.extend([
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
        vertical_velocity_source(window),
    ]);
    fields
}

fn surface_input_fields(window: &PreparedWindow) -> Vec<CanonicalField> {
    let mut fields = upper_w_input_fields(window);
    fields.extend([
        CanonicalField::TenMetreEastwardWind,
        CanonicalField::TenMetreNorthwardWind,
        CanonicalField::TwoMetreAirTemperature,
        CanonicalField::TwoMetreSpecificHumidity,
        CanonicalField::AerodynamicRoughnessLength,
        CanonicalField::BoundaryLayerHeight,
        CanonicalField::SensibleHeatFlux,
        CanonicalField::LatentHeatFlux,
    ]);
    if has_field_at_query_time(window, CanonicalField::FrictionVelocity) {
        fields.push(CanonicalField::FrictionVelocity);
    } else {
        fields.extend([
            CanonicalField::EastwardSurfaceStress,
            CanonicalField::NorthwardSurfaceStress,
        ]);
    }
    for optional in [CanonicalField::MoninObukhovLength] {
        if has_field_at_query_time(window, optional) {
            fields.push(optional);
        }
    }
    fields
}

fn surface_exchange_input_fields(window: &PreparedWindow) -> Vec<CanonicalField> {
    let mut fields = vec![
        CanonicalField::SurfacePressure,
        CanonicalField::TwoMetreAirTemperature,
        CanonicalField::TwoMetreSpecificHumidity,
        CanonicalField::SensibleHeatFlux,
        CanonicalField::LatentHeatFlux,
    ];
    if has_field_at_query_time(window, CanonicalField::FrictionVelocity) {
        fields.push(CanonicalField::FrictionVelocity);
    } else {
        fields.extend([
            CanonicalField::EastwardSurfaceStress,
            CanonicalField::NorthwardSurfaceStress,
        ]);
    }
    if has_field_at_query_time(window, CanonicalField::MoninObukhovLength) {
        fields.push(CanonicalField::MoninObukhovLength);
    }
    fields
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn query_provenance_frames(window: &PreparedWindow) -> Vec<&RawMetFrame> {
    let mut frames = vec![window.frames.before.as_ref()];
    if !window.is_exact_frame() {
        frames.push(window.frames.after.as_ref());
    }
    frames
}

fn append_source_history(
    frame: &RawMetFrame,
    field: CanonicalField,
    sources: &mut BTreeSet<String>,
    transforms: &mut Vec<TransformRecord>,
) -> Result<FieldQuality, EngineError> {
    let key = FieldKey::Canonical(field);
    let source = frame
        .fields()
        .get(&key)
        .ok_or_else(|| EngineError::MissingField(key.clone()))?;
    let record = frame
        .provenance()
        .get(source.provenance())
        .ok_or(EngineError::InvalidPreparedState)?;
    sources.insert(frame.metadata().id.content_sha256.clone());
    sources.extend(record.sources.iter().cloned());
    for transform in &record.transforms {
        push_unique(transforms, transform.clone());
    }
    Ok(source.quality())
}

fn query_transform(
    window: &PreparedWindow,
    route: QueryRoute,
    field: CanonicalField,
) -> TransformRecord {
    let time_scheme = if window.is_exact_frame() {
        if field == CanonicalField::GeometricVerticalVelocity {
            "exact_frame_symmetric_slope"
        } else {
            "exact_frame"
        }
    } else {
        "bracketed_linear"
    };
    TransformRecord {
        operation: match route {
            QueryRoute::UpperAir => "trajecta.m3.query.upper_air".into(),
            QueryRoute::SurfaceLayer
                if matches!(
                    field,
                    CanonicalField::EastwardWind
                        | CanonicalField::NorthwardWind
                        | CanonicalField::GeometricVerticalVelocity
                        | CanonicalField::AirTemperature
                        | CanonicalField::SpecificHumidity
                        | CanonicalField::AirDensity
                ) =>
            {
                "trajecta.m3.query.monin_obukhov_businger_dyer".into()
            }
            QueryRoute::SurfaceLayer => "trajecta.m3.query.surface_column".into(),
            QueryRoute::TemporalMixedSurfaceUpper => {
                "trajecta.m3.query.temporal_mixed_surface_upper".into()
            }
        },
        parameters: vec![
            ("algorithm".into(), M3_MET_QUERY_ALGORITHM_ID.into()),
            ("field".into(), format!("{field:?}")),
            ("time_scheme".into(), time_scheme.into()),
            (
                "before_weight_bits".into(),
                format!("{:016x}", window.before_weight.to_bits()),
            ),
            (
                "after_weight_bits".into(),
                format!("{:016x}", window.after_weight.to_bits()),
            ),
        ],
    }
}

fn append_physical_projection_transforms(
    transforms: &mut Vec<TransformRecord>,
    route: QueryRoute,
    field: CanonicalField,
) {
    let surface_physical_output = route != QueryRoute::UpperAir
        && matches!(
            field,
            CanonicalField::EastwardWind
                | CanonicalField::NorthwardWind
                | CanonicalField::GeometricVerticalVelocity
                | CanonicalField::AirTemperature
                | CanonicalField::SpecificHumidity
                | CanonicalField::AirDensity
        );
    if field == CanonicalField::AirDensity || surface_physical_output {
        push_unique(
            transforms,
            TransformRecord {
                operation: SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID.into(),
                parameters: vec![("scope".into(), "physical_consumer_only".into())],
            },
        );
    }
    if surface_physical_output {
        push_unique(
            transforms,
            TransformRecord {
                operation: AERODYNAMIC_ROUGHNESS_ZERO_PROJECTION_ALGORITHM_ID.into(),
                parameters: vec![("scope".into(), "surface_layer_physics_only".into())],
            },
        );
        push_unique(
            transforms,
            TransformRecord {
                operation: LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID.into(),
                parameters: vec![("required_components".into(), "u,v,w".into())],
            },
        );
    }
    if route != QueryRoute::UpperAir
        && matches!(
            field,
            CanonicalField::EastwardWind | CanonicalField::NorthwardWind
        )
    {
        push_unique(
            transforms,
            TransformRecord {
                operation: TEN_METRE_ANCHORED_SURFACE_WIND_ALGORITHM_ID.into(),
                parameters: vec![("reference_height_m".into(), "10".into())],
            },
        );
    }
    if route != QueryRoute::UpperAir
        && matches!(
            field,
            CanonicalField::AirTemperature
                | CanonicalField::SpecificHumidity
                | CanonicalField::AirDensity
        )
    {
        push_unique(
            transforms,
            TransformRecord {
                operation: TWO_METRE_ANCHORED_SURFACE_SCALAR_ALGORITHM_ID.into(),
                parameters: vec![("reference_height_m".into(), "2".into())],
            },
        );
    }
}

fn field_inputs_and_quality(
    window: &PreparedWindow,
    field: CanonicalField,
    route: QueryRoute,
) -> (Vec<CanonicalField>, FieldQuality, bool) {
    if matches!(
        field,
        CanonicalField::FrictionVelocity | CanonicalField::MoninObukhovLength
    ) && !has_field_at_query_time(window, field)
    {
        return (
            surface_exchange_input_fields(window),
            FieldQuality::Derived,
            false,
        );
    }
    if route != QueryRoute::UpperAir
        && matches!(
            field,
            CanonicalField::EastwardWind
                | CanonicalField::NorthwardWind
                | CanonicalField::GeometricVerticalVelocity
                | CanonicalField::AirTemperature
                | CanonicalField::SpecificHumidity
                | CanonicalField::AirDensity
        )
    {
        return (
            surface_input_fields(window),
            FieldQuality::Derived,
            field == CanonicalField::GeometricVerticalVelocity,
        );
    }
    match field {
        CanonicalField::EastwardWind
        | CanonicalField::NorthwardWind
        | CanonicalField::AirTemperature
        | CanonicalField::SpecificHumidity
        | CanonicalField::SurfacePressure
        | CanonicalField::PressureVerticalVelocity
        | CanonicalField::HybridVerticalVelocity
        | CanonicalField::TenMetreEastwardWind
        | CanonicalField::TenMetreNorthwardWind
        | CanonicalField::TwoMetreAirTemperature
        | CanonicalField::TwoMetreSpecificHumidity
        | CanonicalField::AerodynamicRoughnessLength
        | CanonicalField::BoundaryLayerHeight
        | CanonicalField::EastwardSurfaceStress
        | CanonicalField::NorthwardSurfaceStress
        | CanonicalField::SensibleHeatFlux
        | CanonicalField::LatentHeatFlux
        | CanonicalField::FrictionVelocity
        | CanonicalField::MoninObukhovLength => (vec![field], FieldQuality::Source, false),
        CanonicalField::GeometricVerticalVelocity => {
            (upper_w_input_fields(window), FieldQuality::Derived, true)
        }
        CanonicalField::AirPressure | CanonicalField::GeometricHeight => {
            (column_input_fields(window), FieldQuality::Derived, false)
        }
        CanonicalField::AirDensity => {
            let mut inputs = column_input_fields(window);
            inputs.extend([
                CanonicalField::AirTemperature,
                CanonicalField::SpecificHumidity,
            ]);
            (inputs, FieldQuality::Derived, false)
        }
        CanonicalField::GeometricTerrainHeight => {
            let source = terrain_source(window);
            (
                vec![source],
                if source == CanonicalField::GeometricTerrainHeight {
                    FieldQuality::Source
                } else {
                    FieldQuality::Derived
                },
                false,
            )
        }
        _ => (vec![field], FieldQuality::Source, false),
    }
}

fn build_field_metadata(
    table: &mut ProvenanceTable,
    window: &PreparedWindow,
    field: CanonicalField,
    route: QueryRoute,
) -> Result<FieldMetadata, EngineError> {
    let (inputs, mut quality, include_derivative_frames) =
        field_inputs_and_quality(window, field, route);
    let mut unique_inputs = Vec::new();
    for input in inputs {
        push_unique(&mut unique_inputs, input);
    }
    let mut sources = BTreeSet::new();
    let mut transforms = Vec::new();
    for frame in query_provenance_frames(window) {
        for input in &unique_inputs {
            quality = quality.max(append_source_history(
                frame,
                *input,
                &mut sources,
                &mut transforms,
            )?);
        }
    }
    if include_derivative_frames && window.is_exact_frame() {
        let derivative_inputs = column_input_fields(window);
        for frame in [window.previous.as_deref(), window.next.as_deref()]
            .into_iter()
            .flatten()
        {
            for input in &derivative_inputs {
                quality = quality.max(append_source_history(
                    frame,
                    *input,
                    &mut sources,
                    &mut transforms,
                )?);
            }
        }
    }
    append_physical_projection_transforms(&mut transforms, route, field);
    if matches!(
        field,
        CanonicalField::FrictionVelocity | CanonicalField::MoninObukhovLength
    ) && !has_field_at_query_time(window, field)
    {
        transforms.push(TransformRecord {
            operation: SURFACE_EXCHANGE_SCALES_ALGORITHM_ID.into(),
            parameters: vec![("output".into(), format!("{field:?}"))],
        });
    }
    transforms.push(query_transform(window, route, field));
    let provenance = table
        .intern(ProvenanceRecord {
            field: FieldKey::Canonical(field),
            quality,
            sources: sources.into_iter().collect(),
            transforms,
            fallback_reason: None,
            profile_sha256: window.frames.before.metadata().id.profile_sha256.clone(),
        })
        .map_err(EngineError::Provenance)?;
    Ok(FieldMetadata {
        quality,
        provenance,
    })
}

fn build_execution_metadata(
    plan: &QueryPlan,
    window: &PreparedWindow,
) -> Result<ExecutionMetadata, EngineError> {
    let mut table = ProvenanceTable::new();
    let mut upper_transport = [FieldMetadata {
        quality: FieldQuality::Derived,
        provenance: ProvenanceId(0),
    }; TRANSPORT_FIELD_COUNT];
    let mut surface_transport = upper_transport;
    let mut mixed_transport = upper_transport;
    for index in 0..TRANSPORT_FIELD_COUNT {
        let field = transport_field(index);
        upper_transport[index] =
            build_field_metadata(&mut table, window, field, QueryRoute::UpperAir)?;
        surface_transport[index] = if plan.surface_layer_model().is_some() {
            build_field_metadata(&mut table, window, field, QueryRoute::SurfaceLayer)?
        } else {
            upper_transport[index]
        };
        mixed_transport[index] = if plan.surface_layer_model().is_some() {
            build_field_metadata(
                &mut table,
                window,
                field,
                QueryRoute::TemporalMixedSurfaceUpper,
            )?
        } else {
            upper_transport[index]
        };
    }
    let mut generic_upper = Vec::with_capacity(plan.fields().len());
    let mut generic_surface = Vec::with_capacity(plan.fields().len());
    let mut generic_mixed = Vec::with_capacity(plan.fields().len());
    for field in plan.fields() {
        if let Some(index) = transport_field_index(field) {
            generic_upper.push(upper_transport[index]);
            generic_surface.push(surface_transport[index]);
            generic_mixed.push(mixed_transport[index]);
            continue;
        }
        let FieldKey::Canonical(canonical) = field else {
            return Err(EngineError::UnsupportedQueryField(field.clone()));
        };
        generic_upper.push(build_field_metadata(
            &mut table,
            window,
            *canonical,
            QueryRoute::UpperAir,
        )?);
        generic_surface.push(if plan.surface_layer_model().is_some() {
            build_field_metadata(&mut table, window, *canonical, QueryRoute::SurfaceLayer)?
        } else {
            *generic_upper
                .last()
                .ok_or(EngineError::InvalidPreparedState)?
        });
        generic_mixed.push(
            *generic_surface
                .last()
                .ok_or(EngineError::InvalidPreparedState)?,
        );
    }
    let metadata = ExecutionMetadata {
        table: Arc::new(table),
        upper_transport,
        surface_transport,
        mixed_transport,
        generic_upper,
        generic_surface,
        generic_mixed,
    };
    if !plan.allow_estimated() {
        for (index, field) in plan.fields().iter().enumerate() {
            if metadata.generic_upper[index].quality == FieldQuality::Estimated
                || (plan.surface_layer_model().is_some()
                    && metadata.generic_surface[index].quality == FieldQuality::Estimated)
                || (plan.surface_layer_model().is_some()
                    && metadata.generic_mixed[index].quality == FieldQuality::Estimated)
            {
                return Err(EngineError::QueryPlan(
                    QueryPlanError::EstimatedFieldForbidden(field.clone()),
                ));
            }
        }
    }
    Ok(metadata)
}

fn blend_frame_points(
    before: FramePointSample,
    after: FramePointSample,
    window: &PreparedWindow,
) -> Result<FramePointSample, EngineError> {
    Ok(FramePointSample {
        eastward_wind_m_s: blend_value(before.eastward_wind_m_s, after.eastward_wind_m_s, window)?,
        northward_wind_m_s: blend_value(
            before.northward_wind_m_s,
            after.northward_wind_m_s,
            window,
        )?,
        pressure_pa: blend_value(before.pressure_pa, after.pressure_pa, window)?,
        temperature_k: blend_value(before.temperature_k, after.temperature_k, window)?,
        specific_humidity: blend_value(before.specific_humidity, after.specific_humidity, window)?,
        physical_specific_humidity: blend_value(
            before.physical_specific_humidity,
            after.physical_specific_humidity,
            window,
        )?,
    })
}

#[allow(clippy::too_many_arguments)]
fn target_frame_point_from_columns(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    before_column: &ColumnGeometry,
    after_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<FramePointSample, EngineError> {
    let before = sample_frame_point_from_column(
        frame_stencils(stencils, &window.frames.before)?,
        before_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_frame_point_from_column(
        frame_stencils(stencils, &window.frames.after)?,
        after_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    blend_frame_points(before, after, window)
}

#[allow(clippy::too_many_arguments)]
fn target_frame_point_from_shapes(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetTransportColumn,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<FramePointSample, EngineError> {
    let before = sample_frame_point_from_shape(
        frame_stencils(stencils, &window.frames.before)?,
        &column.before,
        weights,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_frame_point_from_shape(
        frame_stencils(stencils, &window.frames.after)?,
        column.after(),
        weights,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    blend_frame_points(before, after, window)
}

#[allow(clippy::too_many_arguments)]
fn target_hybrid_frame_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetHybridColumn<'_>,
    cell: crate::grid::CellId,
    query_basis: SphericalBasis,
    coordinate: VerticalQuery,
    vertical_value: f64,
    before_bracket: Result<VerticalBracket, EngineError>,
    after_bracket: Result<VerticalBracket, EngineError>,
) -> Result<FramePointSample, EngineError> {
    let before = sample_hybrid_frame_point(
        frame_stencils(stencils, &window.frames.before)?,
        column.before,
        cell,
        query_basis,
        coordinate,
        vertical_value,
        before_bracket,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_hybrid_frame_point(
        frame_stencils(stencils, &window.frames.after)?,
        column.after(),
        cell,
        query_basis,
        coordinate,
        vertical_value,
        after_bracket,
    )?;
    blend_frame_points(before, after, window)
}

fn vertical_bounds_for_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<VerticalBounds, EngineError> {
    let minimum = minimum_transport_agl_for_point(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    column
        .vertical_bounds(minimum)
        .map_err(|_| EngineError::Vertical(VerticalError::InvalidVerticalColumn))
}

fn complete_transport_bounds_for_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<VerticalBounds, EngineError> {
    let bounds = vertical_bounds_for_point(
        window,
        stencils,
        &column.geometry,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    restrict_vertical_bounds_to_complete_transport(
        bounds,
        column.restrictive_transport_top_asl_m,
        column.restrictive_transport_top_pressure_pa,
    )
}

fn complete_transport_bounds_for_shape(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetTransportColumn,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
) -> Result<VerticalBounds, EngineError> {
    let minimum = minimum_transport_agl_with_weights(window, stencils, cell, weights)?;
    let bounds = column
        .geometry()
        .vertical_bounds(minimum)
        .map_err(|_| EngineError::Vertical(VerticalError::InvalidVerticalColumn))?;
    restrict_vertical_bounds_to_complete_transport(
        bounds,
        column.restrictive_transport_top_asl_m,
        column.restrictive_transport_top_pressure_pa,
    )
}

fn complete_transport_bounds_for_hybrid(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetHybridColumn<'_>,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
) -> Result<VerticalBounds, EngineError> {
    let minimum = minimum_transport_agl_with_weights(window, stencils, cell, weights)?;
    let bounds = VerticalBounds::new(
        column.terrain_asl_m,
        minimum,
        column.terrain_asl_m + minimum,
        column.top.height_asl_m,
        column.physical_model_top_asl_m,
        column.top.pressure_pa,
        column.surface_pressure_pa,
    )
    .map_err(|_| EngineError::Vertical(VerticalError::InvalidVerticalColumn))?;
    restrict_vertical_bounds_to_complete_transport(
        bounds,
        column.restrictive_transport_top_asl_m,
        column.restrictive_transport_top_pressure_pa,
    )
}

fn restrict_vertical_bounds_to_complete_transport(
    bounds: VerticalBounds,
    restrictive_top_asl_m: f64,
    restrictive_top_pressure_pa: f64,
) -> Result<VerticalBounds, EngineError> {
    let effective_top_asl_m = restrictive_top_asl_m.min(
        bounds
            .physical_model_top_asl_m()
            .unwrap_or(restrictive_top_asl_m),
    );
    VerticalBounds::new(
        bounds.terrain_asl_m(),
        bounds.minimum_transport_agl_m(),
        bounds.minimum_transport_asl_m(),
        effective_top_asl_m,
        bounds.physical_model_top_asl_m(),
        restrictive_top_pressure_pa,
        bounds.maximum_pressure_pa(),
    )
    .map_err(|_| EngineError::Vertical(VerticalError::InvalidVerticalColumn))
}

fn minimum_transport_agl_for_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<f64, EngineError> {
    let roughness = sample_surface_scalar_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::AerodynamicRoughnessLength,
    )?;
    minimum_transport_height_agl_m(roughness).map_err(|_| EngineError::NumericalFailure)
}

fn minimum_transport_agl_with_weights(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
) -> Result<f64, EngineError> {
    let roughness = sample_surface_scalar_time_with_weights(
        window,
        stencils,
        cell,
        weights,
        CanonicalField::AerodynamicRoughnessLength,
    )?;
    minimum_transport_height_agl_m(roughness).map_err(|_| EngineError::NumericalFailure)
}

#[allow(clippy::too_many_arguments)]
fn sample_upper_transport(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &TargetColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bracket: VerticalBracket,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
    let state = target_frame_point_from_columns(
        window,
        stencils,
        &target_column.before,
        &target_column.after,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    let Some(geometric_vertical_velocity_m_s) = geometric_w_at_bracket(
        window,
        stencils,
        target_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        bracket,
    )?
    else {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    };
    let density = moist_air_density_kg_m3(
        state.pressure_pa,
        state.temperature_k,
        state.physical_specific_humidity,
    )
    .map_err(|_| EngineError::NumericalFailure)?;
    Ok(TransportPointResult {
        values: [
            state.eastward_wind_m_s,
            state.northward_wind_m_s,
            geometric_vertical_velocity_m_s,
            state.pressure_pa,
            state.temperature_k,
            state.specific_humidity,
            density,
            target_column.geometry.terrain_asl_m(),
        ],
        valid: [true; TRANSPORT_FIELD_COUNT],
        quality: std::array::from_fn(|index| metadata[index].quality),
        provenance: std::array::from_fn(|index| metadata[index].provenance),
        status: SampleStatus::Ok,
        bounds: Some(bounds),
    })
}

#[allow(clippy::too_many_arguments)]
fn sample_upper_transport_from_shapes(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &TargetTransportColumn,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bracket: VerticalBracket,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
    let state = target_frame_point_from_shapes(
        window,
        stencils,
        target_column,
        weights,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    let Some(geometric_vertical_velocity_m_s) = geometric_w_at_bracket_from_shapes(
        window,
        stencils,
        target_column,
        weights,
        cell,
        longitude_degrees,
        latitude_degrees,
        bracket,
    )?
    else {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    };
    let density = moist_air_density_kg_m3(
        state.pressure_pa,
        state.temperature_k,
        state.physical_specific_humidity,
    )
    .map_err(|_| EngineError::NumericalFailure)?;
    Ok(TransportPointResult {
        values: [
            state.eastward_wind_m_s,
            state.northward_wind_m_s,
            geometric_vertical_velocity_m_s,
            state.pressure_pa,
            state.temperature_k,
            state.specific_humidity,
            density,
            target_column.geometry().terrain_asl_m(),
        ],
        valid: [true; TRANSPORT_FIELD_COUNT],
        quality: std::array::from_fn(|index| metadata[index].quality),
        provenance: std::array::from_fn(|index| metadata[index].provenance),
        status: SampleStatus::Ok,
        bounds: Some(bounds),
    })
}

#[allow(clippy::too_many_arguments)]
fn sample_upper_transport_from_hybrid(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &TargetHybridColumn<'_>,
    weights: HorizontalWeights,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bracket: VerticalBracket,
    before_bracket: Result<VerticalBracket, EngineError>,
    after_bracket: Result<VerticalBracket, EngineError>,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
    let query_basis = spherical_vector_query_basis(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    let state = target_hybrid_frame_point(
        window,
        stencils,
        target_column,
        cell,
        query_basis,
        coordinate,
        vertical_value,
        before_bracket,
        after_bracket,
    )?;
    let Some(geometric_vertical_velocity_m_s) = geometric_w_at_bracket_from_hybrid(
        window,
        stencils,
        target_column,
        weights,
        cell,
        query_basis,
        latitude_degrees,
        bracket,
    )?
    else {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    };
    let density = moist_air_density_kg_m3(
        state.pressure_pa,
        state.temperature_k,
        state.physical_specific_humidity,
    )
    .map_err(|_| EngineError::NumericalFailure)?;
    Ok(TransportPointResult {
        values: [
            state.eastward_wind_m_s,
            state.northward_wind_m_s,
            geometric_vertical_velocity_m_s,
            state.pressure_pa,
            state.temperature_k,
            state.specific_humidity,
            density,
            target_column.terrain_asl_m,
        ],
        valid: [true; TRANSPORT_FIELD_COUNT],
        quality: std::array::from_fn(|index| metadata[index].quality),
        provenance: std::array::from_fn(|index| metadata[index].provenance),
        status: SampleStatus::Ok,
        bounds: Some(bounds),
    })
}

#[allow(clippy::too_many_arguments)]
fn derive_surface_exchange_scales(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    surface_pressure_pa: f64,
    two_metre_temperature_k: f64,
    two_metre_specific_humidity: f64,
) -> Result<SurfaceExchangeScales, EngineError> {
    derive_surface_exchange_scales_with_sampler(
        surface_pressure_pa,
        two_metre_temperature_k,
        two_metre_specific_humidity,
        has_field_at_query_time(window, CanonicalField::FrictionVelocity),
        has_field_at_query_time(window, CanonicalField::MoninObukhovLength),
        |field| {
            sample_surface_scalar_time(
                window,
                stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                field,
            )
        },
    )
}

fn query_surface_exchange_scales(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<SurfaceExchangeScales, EngineError> {
    let two_metre_temperature_k = sample_surface_scalar_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TwoMetreAirTemperature,
    )?;
    let two_metre_specific_humidity = sample_physical_two_metre_specific_humidity_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    derive_surface_exchange_scales(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        column.surface_pressure_pa(),
        two_metre_temperature_k,
        two_metre_specific_humidity,
    )
}

fn derive_surface_exchange_scales_frame(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    surface_pressure_pa: f64,
    two_metre_temperature_k: f64,
    two_metre_specific_humidity: f64,
) -> Result<SurfaceExchangeScales, EngineError> {
    let has = |field| {
        frame_stencils
            .frame
            .fields()
            .get(&FieldKey::Canonical(field))
            .is_some()
    };
    derive_surface_exchange_scales_with_sampler(
        surface_pressure_pa,
        two_metre_temperature_k,
        two_metre_specific_humidity,
        has(CanonicalField::FrictionVelocity),
        has(CanonicalField::MoninObukhovLength),
        |field| {
            sample_surface_scalar_frame(
                frame_stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                field,
            )
        },
    )
}

fn derive_surface_exchange_scales_with_sampler<F>(
    surface_pressure_pa: f64,
    two_metre_temperature_k: f64,
    two_metre_specific_humidity: f64,
    has_friction_velocity: bool,
    has_monin_obukhov_length: bool,
    mut sample: F,
) -> Result<SurfaceExchangeScales, EngineError>
where
    F: FnMut(CanonicalField) -> Result<f64, EngineError>,
{
    let density = moist_air_density_kg_m3(
        surface_pressure_pa,
        two_metre_temperature_k,
        two_metre_specific_humidity,
    )
    .map_err(|_| EngineError::NumericalFailure)?;
    let momentum = if has_friction_velocity {
        SurfaceMomentumInput::FrictionVelocity {
            friction_velocity_m_s: sample(CanonicalField::FrictionVelocity)?,
        }
    } else {
        SurfaceMomentumInput::SurfaceStress {
            eastward_pa: sample(CanonicalField::EastwardSurfaceStress)?,
            northward_pa: sample(CanonicalField::NorthwardSurfaceStress)?,
        }
    };
    let mut scales = surface_exchange_scales(SurfaceExchangeInput {
        air_density_kg_m3: density,
        air_temperature_k: two_metre_temperature_k,
        specific_humidity: two_metre_specific_humidity,
        momentum,
        sensible_heat_flux_w_m2: sample(CanonicalField::SensibleHeatFlux)?,
        latent_heat_flux_w_m2: sample(CanonicalField::LatentHeatFlux)?,
    })
    .map_err(|_| EngineError::NumericalFailure)?;
    if has_monin_obukhov_length {
        let monin_obukhov_length_m = sample(CanonicalField::MoninObukhovLength)?;
        if monin_obukhov_length_m == 0.0 {
            scales.monin_obukhov_length_m = 1.0;
            scales.neutral_stability = true;
        } else {
            scales.monin_obukhov_length_m = monin_obukhov_length_m;
            scales.neutral_stability = false;
        }
    }
    Ok(scales)
}

fn surface_query_height_agl_m(
    column: &ColumnGeometry,
    coordinate: VerticalQuery,
    vertical_value: f64,
    lowest: usize,
) -> Result<f64, EngineError> {
    let lowest_height_agl_m = column.height_asl_m()[lowest] - column.terrain_asl_m();
    let value = match coordinate {
        VerticalQuery::AboveGround => vertical_value,
        VerticalQuery::AboveSeaLevel => vertical_value - column.terrain_asl_m(),
        VerticalQuery::Pressure => {
            let denominator = column.surface_pressure_pa().ln() - column.pressure_pa()[lowest].ln();
            if !denominator.is_finite() || denominator <= 0.0 {
                return Err(EngineError::NumericalFailure);
            }
            lowest_height_agl_m * (column.surface_pressure_pa().ln() - vertical_value.ln())
                / denominator
        }
    };
    value
        .is_finite()
        .then_some(value)
        .ok_or(EngineError::NumericalFailure)
}

fn surface_query_pressure_pa(
    column: &ColumnGeometry,
    coordinate: VerticalQuery,
    vertical_value: f64,
    query_height_agl_m: f64,
    lowest: usize,
) -> Result<f64, EngineError> {
    if coordinate == VerticalQuery::Pressure {
        return Ok(vertical_value);
    }
    let lowest_height_agl_m = column.height_asl_m()[lowest] - column.terrain_asl_m();
    if !lowest_height_agl_m.is_finite() || lowest_height_agl_m <= 0.0 {
        return Err(EngineError::NumericalFailure);
    }
    let fraction = query_height_agl_m / lowest_height_agl_m;
    let logarithm = (column.pressure_pa()[lowest].ln() - column.surface_pressure_pa().ln())
        .mul_add(fraction, column.surface_pressure_pa().ln());
    let pressure = logarithm.exp();
    pressure
        .is_finite()
        .then_some(pressure)
        .ok_or(EngineError::NumericalFailure)
}

#[allow(clippy::too_many_arguments)]
fn sample_surface_transport(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &TargetColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
    let geometry = &target_column.geometry;
    let roughness = sample_surface_scalar_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::AerodynamicRoughnessLength,
    )?;
    let physical_roughness = project_aerodynamic_roughness_for_physics(roughness)
        .map_err(|_| EngineError::NumericalFailure)?;
    let minimum = minimum_transport_height_agl_m(physical_roughness)
        .map_err(|_| EngineError::NumericalFailure)?;
    let (w_profile, w_valid) = geometric_w_profile(
        window,
        stencils,
        target_column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let (lowest, lowest_wind) = lowest_complete_time_transport_anchor(
        window,
        stencils,
        geometry,
        &target_column.before,
        &target_column.after,
        &w_valid,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let lowest_height_agl_m = geometry.height_asl_m()[lowest] - geometry.terrain_asl_m();
    let query_height_agl_m =
        surface_query_height_agl_m(geometry, coordinate, vertical_value, lowest)?;
    if !query_height_agl_m.is_finite()
        || query_height_agl_m < minimum
        || query_height_agl_m > lowest_height_agl_m
    {
        return Ok(TransportPointResult::invalid(
            SampleStatus::SurfaceLayerUndefined,
            Some(bounds),
            metadata,
        ));
    }
    let ten_metre_wind = sample_surface_vector_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TenMetreEastwardWind,
        CanonicalField::TenMetreNorthwardWind,
    )?;
    let two_metre_temperature_k = sample_surface_scalar_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TwoMetreAirTemperature,
    )?;
    let two_metre_specific_humidity = sample_physical_two_metre_specific_humidity_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let scales = derive_surface_exchange_scales(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        geometry.surface_pressure_pa(),
        two_metre_temperature_k,
        two_metre_specific_humidity,
    )?;
    let terrain_w =
        terrain_vertical_velocity(window, stencils, cell, longitude_degrees, latitude_degrees)?;
    let boundary_layer_height_m = sample_surface_scalar_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::BoundaryLayerHeight,
    )?;
    let model = plan
        .surface_layer()
        .ok_or(EngineError::InvalidPreparedState)?;
    let neutral = [scales.neutral_stability];
    let output = model
        .evaluate(SurfaceLayerInput {
            query_height_agl_m: &[query_height_agl_m],
            minimum_height_agl_m: &[minimum],
            ten_metre_eastward_wind_m_s: &[ten_metre_wind.0],
            ten_metre_northward_wind_m_s: &[ten_metre_wind.1],
            two_metre_air_temperature_k: &[two_metre_temperature_k],
            two_metre_specific_humidity: &[two_metre_specific_humidity],
            roughness_length_m: &[physical_roughness],
            monin_obukhov_length_m: &[scales.monin_obukhov_length_m],
            neutral_stability: &neutral,
            friction_velocity_m_s: &[scales.friction_velocity_m_s],
            temperature_scale_k: &[scales.temperature_scale_k],
            humidity_scale: &[scales.humidity_scale],
            boundary_layer_height_m: &[boundary_layer_height_m],
            lowest_model_height_agl_m: &[lowest_height_agl_m],
            lowest_model_eastward_wind_m_s: &[lowest_wind.0],
            lowest_model_northward_wind_m_s: &[lowest_wind.1],
            lowest_model_air_temperature_k: &[geometry.temperature_k()[lowest]],
            lowest_model_specific_humidity: &[geometry
                .physical_specific_humidity_at(lowest)
                .map_err(EngineError::Vertical)?],
            terrain_vertical_velocity_m_s: &[terrain_w],
            lowest_model_geometric_vertical_velocity_m_s: &[w_profile[lowest]],
        })
        .map_err(|_| EngineError::InvalidPreparedState)?;
    match output.status[0] {
        SurfaceLayerStatus::Undefined => {
            return Ok(TransportPointResult::invalid(
                SampleStatus::SurfaceLayerUndefined,
                Some(bounds),
                metadata,
            ));
        }
        SurfaceLayerStatus::InvalidPhysicalState => {
            return Ok(TransportPointResult::invalid(
                SampleStatus::InvalidVerticalColumn,
                Some(bounds),
                metadata,
            ));
        }
        SurfaceLayerStatus::NumericalFailure => {
            return Ok(TransportPointResult::invalid(
                SampleStatus::NumericalFailure,
                Some(bounds),
                metadata,
            ));
        }
        SurfaceLayerStatus::Ok => {}
    }
    let pressure_pa = surface_query_pressure_pa(
        geometry,
        coordinate,
        vertical_value,
        query_height_agl_m,
        lowest,
    )?;
    let density = moist_air_density_kg_m3(
        pressure_pa,
        output.air_temperature_k[0],
        output.specific_humidity[0],
    )
    .map_err(|_| EngineError::NumericalFailure)?;
    Ok(TransportPointResult {
        values: [
            output.eastward_wind_m_s[0],
            output.northward_wind_m_s[0],
            output.geometric_vertical_velocity_m_s[0],
            pressure_pa,
            output.air_temperature_k[0],
            output.specific_humidity[0],
            density,
            geometry.terrain_asl_m(),
        ],
        valid: [true; TRANSPORT_FIELD_COUNT],
        quality: std::array::from_fn(|index| metadata[index].quality),
        provenance: std::array::from_fn(|index| metadata[index].provenance),
        status: SampleStatus::Ok,
        bounds: Some(bounds),
    })
}

#[allow(clippy::too_many_arguments)]
fn sample_upper_transport_endpoint(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    frame_stencils: &PreparedFrameStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bracket: VerticalBracket,
) -> Result<TransportSample, EngineError> {
    let state = sample_frame_point(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    let first_w = endpoint_geometric_vertical_velocity_m_s(
        window,
        stencils,
        endpoint,
        column,
        cell,
        longitude_degrees,
        latitude_degrees,
        bracket.first,
    )?;
    let second_w = if bracket.second == bracket.first {
        first_w
    } else {
        endpoint_geometric_vertical_velocity_m_s(
            window,
            stencils,
            endpoint,
            column,
            cell,
            longitude_degrees,
            latitude_degrees,
            bracket.second,
        )?
    };
    let geometric_vertical_velocity_m_s =
        (second_w - first_w).mul_add(bracket.second_weight, first_w);
    Ok(TransportSample {
        eastward_wind_m_s: state.eastward_wind_m_s,
        northward_wind_m_s: state.northward_wind_m_s,
        geometric_vertical_velocity_m_s,
        pressure_pa: state.pressure_pa,
        temperature_k: state.temperature_k,
        specific_humidity: state.specific_humidity,
        physical_specific_humidity: state.physical_specific_humidity,
    })
}

#[allow(clippy::too_many_arguments)]
fn sample_surface_transport_endpoint(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    frame_stencils: &PreparedFrameStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<Result<TransportSample, SampleStatus>, EngineError> {
    let roughness = sample_surface_scalar_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::AerodynamicRoughnessLength,
    )?;
    let physical_roughness = project_aerodynamic_roughness_for_physics(roughness)
        .map_err(|_| EngineError::NumericalFailure)?;
    let minimum = minimum_transport_height_agl_m(physical_roughness)
        .map_err(|_| EngineError::NumericalFailure)?;
    let anchor = lowest_complete_endpoint_transport_anchor(
        window,
        stencils,
        endpoint,
        frame_stencils,
        column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let lowest = anchor.level;
    let lowest_height_agl_m = column.height_asl_m()[lowest] - column.terrain_asl_m();
    let query_height_agl_m =
        surface_query_height_agl_m(column, coordinate, vertical_value, lowest)?;
    if !query_height_agl_m.is_finite()
        || query_height_agl_m < minimum
        || query_height_agl_m > lowest_height_agl_m
    {
        return Ok(Err(SampleStatus::SurfaceLayerUndefined));
    }
    let ten_metre_wind = sample_surface_vector_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TenMetreEastwardWind,
        CanonicalField::TenMetreNorthwardWind,
    )?;
    let two_metre_temperature_k = sample_surface_scalar_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::TwoMetreAirTemperature,
    )?;
    let two_metre_specific_humidity = sample_physical_two_metre_specific_humidity_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let scales = derive_surface_exchange_scales_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        column.surface_pressure_pa(),
        two_metre_temperature_k,
        two_metre_specific_humidity,
    )?;
    let lowest_wind = anchor.wind_m_s;
    let lowest_w = anchor.geometric_vertical_velocity_m_s;
    let terrain_w =
        terrain_vertical_velocity_frame(frame_stencils, cell, longitude_degrees, latitude_degrees)?;
    let boundary_layer_height_m = sample_surface_scalar_frame(
        frame_stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::BoundaryLayerHeight,
    )?;
    let model = plan
        .surface_layer()
        .ok_or(EngineError::InvalidPreparedState)?;
    let neutral = [scales.neutral_stability];
    let output = model
        .evaluate(SurfaceLayerInput {
            query_height_agl_m: &[query_height_agl_m],
            minimum_height_agl_m: &[minimum],
            ten_metre_eastward_wind_m_s: &[ten_metre_wind.0],
            ten_metre_northward_wind_m_s: &[ten_metre_wind.1],
            two_metre_air_temperature_k: &[two_metre_temperature_k],
            two_metre_specific_humidity: &[two_metre_specific_humidity],
            roughness_length_m: &[physical_roughness],
            monin_obukhov_length_m: &[scales.monin_obukhov_length_m],
            neutral_stability: &neutral,
            friction_velocity_m_s: &[scales.friction_velocity_m_s],
            temperature_scale_k: &[scales.temperature_scale_k],
            humidity_scale: &[scales.humidity_scale],
            boundary_layer_height_m: &[boundary_layer_height_m],
            lowest_model_height_agl_m: &[lowest_height_agl_m],
            lowest_model_eastward_wind_m_s: &[lowest_wind.0],
            lowest_model_northward_wind_m_s: &[lowest_wind.1],
            lowest_model_air_temperature_k: &[column.temperature_k()[lowest]],
            lowest_model_specific_humidity: &[column
                .physical_specific_humidity_at(lowest)
                .map_err(EngineError::Vertical)?],
            terrain_vertical_velocity_m_s: &[terrain_w],
            lowest_model_geometric_vertical_velocity_m_s: &[lowest_w],
        })
        .map_err(|_| EngineError::InvalidPreparedState)?;
    let status = match output.status[0] {
        SurfaceLayerStatus::Ok => None,
        SurfaceLayerStatus::Undefined => Some(SampleStatus::SurfaceLayerUndefined),
        SurfaceLayerStatus::InvalidPhysicalState => Some(SampleStatus::InvalidVerticalColumn),
        SurfaceLayerStatus::NumericalFailure => Some(SampleStatus::NumericalFailure),
    };
    if let Some(status) = status {
        return Ok(Err(status));
    }
    let pressure_pa = surface_query_pressure_pa(
        column,
        coordinate,
        vertical_value,
        query_height_agl_m,
        lowest,
    )?;
    Ok(Ok(TransportSample {
        eastward_wind_m_s: output.eastward_wind_m_s[0],
        northward_wind_m_s: output.northward_wind_m_s[0],
        geometric_vertical_velocity_m_s: output.geometric_vertical_velocity_m_s[0],
        pressure_pa,
        temperature_k: output.air_temperature_k[0],
        specific_humidity: output.specific_humidity[0],
        physical_specific_humidity: output.specific_humidity[0],
    }))
}

#[allow(clippy::too_many_arguments)]
fn sample_transport_endpoint(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    frame_stencils: &PreparedFrameStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<Result<TransportSample, SampleStatus>, EngineError> {
    match vertical_bracket(column, coordinate, vertical_value) {
        Ok(bracket) => {
            let upper = sample_upper_transport_endpoint(
                window,
                stencils,
                endpoint,
                frame_stencils,
                column,
                cell,
                longitude_degrees,
                latitude_degrees,
                coordinate,
                vertical_value,
                bracket,
            );
            match upper {
                Ok(value) => Ok(Ok(value)),
                Err(error)
                    if is_incomplete_transport_level(&error)
                        && column.lowest_valid_index().ok() == Some(bracket.second) =>
                {
                    // A pressure endpoint can have thermodynamic geometry at its
                    // nominal bottom level while one transport component is
                    // underground or masked there.  In the bottommost interval,
                    // the complete transport domain therefore starts at the next
                    // jointly supported level and the explicit surface route owns
                    // the query.  Higher-level support holes remain hard errors.
                    match sample_surface_transport_endpoint(
                        plan,
                        window,
                        stencils,
                        endpoint,
                        frame_stencils,
                        column,
                        cell,
                        longitude_degrees,
                        latitude_degrees,
                        coordinate,
                        vertical_value,
                    ) {
                        Ok(Ok(value)) => Ok(Ok(value)),
                        Ok(Err(SampleStatus::SurfaceLayerUndefined)) => Err(error),
                        Ok(Err(status)) => Ok(Err(status)),
                        Err(surface_error) if is_incomplete_transport_level(&surface_error) => {
                            Err(error)
                        }
                        Err(surface_error) => Err(surface_error),
                    }
                }
                Err(error) => Err(error),
            }
        }
        Err(VerticalError::SurfaceLayerRequired) => sample_surface_transport_endpoint(
            plan,
            window,
            stencils,
            endpoint,
            frame_stencils,
            column,
            cell,
            longitude_degrees,
            latitude_degrees,
            coordinate,
            vertical_value,
        ),
        Err(error) => Ok(Err(sample_status_for_vertical_error(&error))),
    }
}

#[allow(clippy::too_many_arguments)]
fn sample_temporal_mixed_transport(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &ColumnGeometry,
    before_column: &ColumnGeometry,
    after_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let after_stencils = frame_stencils(stencils, &window.frames.after)?;
    let before_result = sample_transport_endpoint(
        plan,
        window,
        stencils,
        WindowEndpoint::Before,
        before_stencils,
        before_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    );
    let before = match before_result? {
        Ok(value) => value,
        Err(status) => {
            return Ok(TransportPointResult::invalid(
                status,
                Some(bounds),
                metadata,
            ));
        }
    };
    let after_result = sample_transport_endpoint(
        plan,
        window,
        stencils,
        WindowEndpoint::After,
        after_stencils,
        after_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    );
    let after = match after_result? {
        Ok(value) => value,
        Err(status) => {
            return Ok(TransportPointResult::invalid(
                status,
                Some(bounds),
                metadata,
            ));
        }
    };
    let eastward_wind_m_s = blend_value(before.eastward_wind_m_s, after.eastward_wind_m_s, window)?;
    let northward_wind_m_s =
        blend_value(before.northward_wind_m_s, after.northward_wind_m_s, window)?;
    let geometric_vertical_velocity_m_s = blend_value(
        before.geometric_vertical_velocity_m_s,
        after.geometric_vertical_velocity_m_s,
        window,
    )?;
    let pressure_pa = blend_value(before.pressure_pa, after.pressure_pa, window)?;
    let temperature_k = blend_value(before.temperature_k, after.temperature_k, window)?;
    let specific_humidity = blend_value(before.specific_humidity, after.specific_humidity, window)?;
    let physical_specific_humidity = blend_value(
        before.physical_specific_humidity,
        after.physical_specific_humidity,
        window,
    )?;
    let density = moist_air_density_kg_m3(pressure_pa, temperature_k, physical_specific_humidity)
        .map_err(|_| EngineError::NumericalFailure)?;
    Ok(TransportPointResult {
        values: [
            eastward_wind_m_s,
            northward_wind_m_s,
            geometric_vertical_velocity_m_s,
            pressure_pa,
            temperature_k,
            specific_humidity,
            density,
            target_column.terrain_asl_m(),
        ],
        valid: [true; TRANSPORT_FIELD_COUNT],
        quality: std::array::from_fn(|index| metadata[index].quality),
        provenance: std::array::from_fn(|index| metadata[index].provenance),
        status: SampleStatus::Ok,
        bounds: Some(bounds),
    })
}

fn endpoint_routes_are_mixed(
    before: &Result<VerticalBracket, VerticalError>,
    after: &Result<VerticalBracket, VerticalError>,
) -> bool {
    matches!(
        (before, after),
        (Ok(_), Err(VerticalError::SurfaceLayerRequired))
            | (Err(VerticalError::SurfaceLayerRequired), Ok(_))
    )
}

fn temporal_mixed_columns<'a>(
    window: &PreparedWindow,
    column: &'a TargetColumnGeometry,
    coordinate: VerticalQuery,
    vertical_value: f64,
    target_bracket: &Result<VerticalBracket, VerticalError>,
) -> Option<(&'a ColumnGeometry, &'a ColumnGeometry)> {
    if window.is_exact_frame()
        || !matches!(
            target_bracket,
            Ok(_) | Err(VerticalError::SurfaceLayerRequired)
        )
    {
        return None;
    }
    let before_bracket = vertical_bracket(&column.before, coordinate, vertical_value);
    let after_bracket = vertical_bracket(&column.after, coordinate, vertical_value);
    endpoint_routes_are_mixed(&before_bracket, &after_bracket)
        .then_some((&column.before, &column.after))
}

#[allow(clippy::too_many_arguments)]
fn sample_pressure_transport_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    weights: HorizontalWeights,
    metadata: &ExecutionMetadata,
) -> Result<TransportPointResult, EngineError> {
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let vertical_value = batch.points.vertical[original_index];
    let column = target_transport_column(window, stencils, cell, weights)?;
    let bounds = complete_transport_bounds_for_shape(window, stencils, &column, cell, weights)?;
    let target_bracket =
        vertical_bracket_shape(column.geometry(), batch.vertical_coordinate, vertical_value);
    let bracket = match target_bracket {
        Ok(bracket) => {
            let endpoint_routes_mixed = !window.is_exact_frame()
                && endpoint_routes_are_mixed(
                    &vertical_bracket_shape(
                        &column.before,
                        batch.vertical_coordinate,
                        vertical_value,
                    ),
                    &vertical_bracket_shape(
                        column.after(),
                        batch.vertical_coordinate,
                        vertical_value,
                    ),
                );
            let boundary_adjacent = column
                .geometry()
                .last_valid_index()
                .map_err(EngineError::Vertical)?
                == bracket.second;
            if endpoint_routes_mixed || boundary_adjacent {
                return sample_boundary_adjacent_transport_point(
                    plan,
                    window,
                    stencils,
                    batch,
                    cell,
                    original_index,
                    metadata,
                );
            }
            bracket
        }
        Err(VerticalError::SurfaceLayerRequired) => {
            return sample_boundary_adjacent_transport_point(
                plan,
                window,
                stencils,
                batch,
                cell,
                original_index,
                metadata,
            );
        }
        Err(error) => {
            return Ok(TransportPointResult::invalid(
                sample_status_for_vertical_error(&error),
                Some(bounds),
                &metadata.upper_transport,
            ));
        }
    };
    resolve_local_transport_result(
        sample_upper_transport_from_shapes(
            window,
            stencils,
            &column,
            weights,
            cell,
            longitude_degrees,
            latitude_degrees,
            batch.vertical_coordinate,
            vertical_value,
            bracket,
            bounds,
            &metadata.upper_transport,
        ),
        bounds,
        &metadata.upper_transport,
    )
}

fn hybrid_endpoint_routes_are_mixed(
    before: &Result<VerticalBracket, EngineError>,
    after: &Result<VerticalBracket, EngineError>,
) -> bool {
    matches!(
        (before, after),
        (
            Ok(_),
            Err(EngineError::Vertical(VerticalError::SurfaceLayerRequired))
        ) | (
            Err(EngineError::Vertical(VerticalError::SurfaceLayerRequired)),
            Ok(_)
        )
    )
}

#[allow(clippy::too_many_arguments)]
fn sample_hybrid_transport_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    weights: HorizontalWeights,
    metadata: &ExecutionMetadata,
    column: TargetHybridColumn<'_>,
) -> Result<TransportPointResult, EngineError> {
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let vertical_value = batch.points.vertical[original_index];
    let bounds = complete_transport_bounds_for_hybrid(window, stencils, &column, cell, weights)?;
    let target_bracket =
        locate_target_hybrid_vertical(window, &column, batch.vertical_coordinate, vertical_value);
    let bracket = match target_bracket {
        Ok(bracket) => bracket,
        Err(EngineError::Vertical(VerticalError::SurfaceLayerRequired)) => {
            return sample_boundary_adjacent_transport_point(
                plan,
                window,
                stencils,
                batch,
                cell,
                original_index,
                metadata,
            );
        }
        Err(EngineError::Vertical(error)) => {
            return Ok(TransportPointResult::invalid(
                sample_status_for_vertical_error(&error),
                Some(bounds),
                &metadata.upper_transport,
            ));
        }
        Err(error) => return Err(error),
    };
    let before_bracket = if window.is_exact_frame() {
        Ok(bracket)
    } else {
        locate_sampled_hybrid_vertical(column.before, batch.vertical_coordinate, vertical_value)
    };
    let after_bracket = if window.is_exact_frame() {
        Ok(bracket)
    } else {
        locate_sampled_hybrid_vertical(column.after(), batch.vertical_coordinate, vertical_value)
    };
    if hybrid_endpoint_routes_are_mixed(&before_bracket, &after_bracket)
        || bracket.second + 1 == column.level_count()
    {
        return sample_boundary_adjacent_transport_point(
            plan,
            window,
            stencils,
            batch,
            cell,
            original_index,
            metadata,
        );
    }
    resolve_local_transport_result(
        sample_upper_transport_from_hybrid(
            window,
            stencils,
            &column,
            weights,
            cell,
            longitude_degrees,
            latitude_degrees,
            batch.vertical_coordinate,
            vertical_value,
            bracket,
            before_bracket,
            after_bracket,
            bounds,
            &metadata.upper_transport,
        ),
        bounds,
        &metadata.upper_transport,
    )
}

#[allow(clippy::too_many_arguments)]
fn sample_transport_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    weights: HorizontalWeights,
    metadata: &ExecutionMetadata,
) -> Result<TransportPointResult, EngineError> {
    if let Some(column) = target_hybrid_column(window, stencils, cell, weights)? {
        sample_hybrid_transport_point(
            plan,
            window,
            stencils,
            batch,
            cell,
            original_index,
            weights,
            metadata,
            column,
        )
    } else {
        sample_pressure_transport_point(
            plan,
            window,
            stencils,
            batch,
            cell,
            original_index,
            weights,
            metadata,
        )
    }
}

fn resolve_local_transport_result(
    result: Result<TransportPointResult, EngineError>,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => match local_status_for_error(&error) {
            Some(status) => Ok(TransportPointResult::invalid(
                status,
                Some(bounds),
                metadata,
            )),
            None => Err(error),
        },
    }
}

fn sample_boundary_adjacent_transport_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    metadata: &ExecutionMetadata,
) -> Result<TransportPointResult, EngineError> {
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let vertical_value = batch.points.vertical[original_index];
    let column = target_column(window, stencils, cell, longitude_degrees, latitude_degrees)?;
    let bounds = complete_transport_bounds_for_point(
        window,
        stencils,
        &column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let target_bracket =
        vertical_bracket(&column.geometry, batch.vertical_coordinate, vertical_value);
    let mixed_columns = temporal_mixed_columns(
        window,
        &column,
        batch.vertical_coordinate,
        vertical_value,
        &target_bracket,
    );
    let result = if let Some((before_column, after_column)) = mixed_columns {
        sample_temporal_mixed_transport(
            plan,
            window,
            stencils,
            &column.geometry,
            before_column,
            after_column,
            cell,
            longitude_degrees,
            latitude_degrees,
            batch.vertical_coordinate,
            vertical_value,
            bounds,
            &metadata.mixed_transport,
        )
    } else {
        match target_bracket {
            Ok(bracket) => {
                let upper = sample_upper_transport(
                    window,
                    stencils,
                    &column,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                    batch.vertical_coordinate,
                    vertical_value,
                    bracket,
                    bounds,
                    &metadata.upper_transport,
                );
                match upper {
                    Ok(value) => Ok(value),
                    Err(error)
                        if is_incomplete_transport_level(&error)
                            && column.geometry.lowest_valid_index().ok()
                                == Some(bracket.second) =>
                    {
                        match sample_surface_transport(
                            plan,
                            window,
                            stencils,
                            &column,
                            cell,
                            longitude_degrees,
                            latitude_degrees,
                            batch.vertical_coordinate,
                            vertical_value,
                            bounds,
                            &metadata.surface_transport,
                        ) {
                            Ok(value) if value.status == SampleStatus::SurfaceLayerUndefined => {
                                Err(error)
                            }
                            Ok(value) => Ok(value),
                            Err(surface_error) if is_incomplete_transport_level(&surface_error) => {
                                Err(error)
                            }
                            Err(surface_error) => Err(surface_error),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            Err(VerticalError::SurfaceLayerRequired) => sample_surface_transport(
                plan,
                window,
                stencils,
                &column,
                cell,
                longitude_degrees,
                latitude_degrees,
                batch.vertical_coordinate,
                vertical_value,
                bounds,
                &metadata.surface_transport,
            ),
            Err(error) => Ok(TransportPointResult::invalid(
                sample_status_for_vertical_error(&error),
                Some(bounds),
                &metadata.upper_transport,
            )),
        }
    };
    resolve_local_transport_result(result, bounds, &metadata.upper_transport)
}

fn sample_boundary_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
) -> Result<BoundaryPointResult, EngineError> {
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let vertical_value = batch.points.vertical[original_index];
    let column =
        target_boundary_column(window, stencils, cell, longitude_degrees, latitude_degrees)?;
    let minimum = minimum_transport_agl_for_point(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    finish_boundary_point(column, minimum, batch.vertical_coordinate, vertical_value)
}

fn sample_boundary_point_with_weights(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    weights: HorizontalWeights,
) -> Result<BoundaryPointResult, EngineError> {
    let vertical_value = batch.points.vertical[original_index];
    let column = {
        let _performance = PerformanceScope::enter(PerformanceStage::BoundaryColumnSample);
        target_boundary_column_with_weights(window, stencils, cell, weights)?
    };
    let _performance = PerformanceScope::enter(PerformanceStage::BoundarySurfaceBounds);
    let minimum = minimum_transport_agl_with_weights(window, stencils, cell, weights)?;
    finish_boundary_point(column, minimum, batch.vertical_coordinate, vertical_value)
}

fn finish_boundary_point(
    column: TargetBoundaryColumnGeometry,
    minimum_transport_agl_m: f64,
    vertical_coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<BoundaryPointResult, EngineError> {
    let bounds = column
        .geometry
        .vertical_bounds(minimum_transport_agl_m)
        .map_err(|_| EngineError::Vertical(VerticalError::InvalidVerticalColumn))?;
    let bounds = restrict_vertical_bounds_to_complete_transport(
        bounds,
        column.restrictive_transport_top_asl_m,
        column.restrictive_transport_top_pressure_pa,
    )?;
    let location = match vertical_coordinate {
        VerticalQuery::AboveSeaLevel => column.geometry.locate_height_asl_m(vertical_value),
        VerticalQuery::AboveGround => column.geometry.locate_height_agl_m(vertical_value),
        VerticalQuery::Pressure => column.geometry.locate_pressure_pa(vertical_value),
    };
    let above_restrictive_transport_top = match vertical_coordinate {
        VerticalQuery::AboveSeaLevel => vertical_value > bounds.available_top_asl_m(),
        VerticalQuery::AboveGround => {
            column.geometry.terrain_asl_m() + vertical_value > bounds.available_top_asl_m()
        }
        VerticalQuery::Pressure => vertical_value < bounds.minimum_pressure_pa(),
    };
    let status = match location {
        Ok(_) if above_restrictive_transport_top => SampleStatus::AboveAvailableTop,
        Ok(_) => SampleStatus::Ok,
        Err(VerticalError::SurfaceLayerRequired) => {
            let lowest = column
                .geometry
                .last_valid_index()
                .map_err(EngineError::Vertical)?;
            let lowest_height_agl_m =
                column.geometry.height_asl_m()[lowest] - column.geometry.terrain_asl_m();
            let query_height_agl_m = boundary_surface_query_height_agl_m(
                &column.geometry,
                vertical_coordinate,
                vertical_value,
                lowest,
            )?;
            if query_height_agl_m < bounds.minimum_transport_agl_m()
                || query_height_agl_m > lowest_height_agl_m
            {
                SampleStatus::SurfaceLayerUndefined
            } else {
                SampleStatus::Ok
            }
        }
        Err(error) => sample_status_for_vertical_error(&error),
    };
    Ok(BoundaryPointResult {
        status,
        bounds,
        terrain_height_asl_m: (status == SampleStatus::Ok)
            .then_some(column.geometry.terrain_asl_m()),
    })
}

fn boundary_surface_query_height_agl_m(
    column: &ColumnShape,
    coordinate: VerticalQuery,
    vertical_value: f64,
    lowest: usize,
) -> Result<f64, EngineError> {
    let lowest_height_agl_m = column.height_asl_m()[lowest] - column.terrain_asl_m();
    let value = match coordinate {
        VerticalQuery::AboveGround => vertical_value,
        VerticalQuery::AboveSeaLevel => vertical_value - column.terrain_asl_m(),
        VerticalQuery::Pressure => {
            let denominator = column.surface_pressure_pa().ln() - column.pressure_pa()[lowest].ln();
            if !denominator.is_finite() || denominator <= 0.0 {
                return Err(EngineError::NumericalFailure);
            }
            lowest_height_agl_m * (column.surface_pressure_pa().ln() - vertical_value.ln())
                / denominator
        }
    };
    value
        .is_finite()
        .then_some(value)
        .ok_or(EngineError::NumericalFailure)
}

fn sample_status_for_vertical_error(error: &VerticalError) -> SampleStatus {
    match error {
        VerticalError::BelowGround => SampleStatus::BelowGround,
        VerticalError::SurfaceLayerRequired => SampleStatus::SurfaceLayerUndefined,
        VerticalError::AboveAvailableTop => SampleStatus::AboveAvailableTop,
        VerticalError::AboveModelTop => SampleStatus::AboveModelTop,
        VerticalError::NumericalFailure => SampleStatus::NumericalFailure,
        _ => SampleStatus::InvalidVerticalColumn,
    }
}

fn local_status_for_error(error: &EngineError) -> Option<SampleStatus> {
    match error {
        EngineError::Vertical(error) => Some(sample_status_for_vertical_error(error)),
        EngineError::Grid(GridError::OutOfDomain) => Some(SampleStatus::OutOfDomain),
        EngineError::Grid(GridError::PolarSingularity) => Some(SampleStatus::PolarSingularity),
        EngineError::NumericalFailure => Some(SampleStatus::NumericalFailure),
        _ => None,
    }
}

fn explain_horizontal_method(
    weights: [f64; 4],
    bilinear_weights: [f64; 4],
) -> ExplainHorizontalMethod {
    let is_bilinear = weights
        .iter()
        .zip(bilinear_weights)
        .all(|(actual, expected)| (actual - expected).abs() <= 1.0e-12);
    if is_bilinear {
        ExplainHorizontalMethod::Bilinear
    } else if weights.iter().filter(|weight| **weight > 0.0).count() <= 3 {
        ExplainHorizontalMethod::ValidTriangle
    } else {
        ExplainHorizontalMethod::TimeBlended
    }
}

#[allow(clippy::too_many_arguments)]
fn explain_record_for_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    quality: &[FieldQuality],
    provenance: &[ProvenanceId],
    transport_mode: bool,
) -> Result<ExplainRecord, EngineError> {
    if quality.len() != plan.fields().len() || provenance.len() != plan.fields().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let vertical_value = batch.points.vertical[original_index];
    let grid = RegularLatLonGrid::new(window.frames.before.metadata().grid.clone())
        .map_err(EngineError::Grid)?;
    let base = grid
        .horizontal_weights(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    if grid
        .locate_cell(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?
        != cell
    {
        return Err(EngineError::InvalidPreparedState);
    }
    let column = target_column(window, stencils, cell, longitude_degrees, latitude_degrees)?;
    let target_bracket =
        vertical_bracket(&column.geometry, batch.vertical_coordinate, vertical_value);
    let mixed_route = generic_needs_surface_model(plan)
        && plan.surface_layer_model().is_some()
        && temporal_mixed_columns(
            window,
            &column,
            batch.vertical_coordinate,
            vertical_value,
            &target_bracket,
        )
        .is_some();
    let incomplete_bottom_surface_route = if transport_mode && !mixed_route {
        match &target_bracket {
            Ok(bracket) if column.geometry.lowest_valid_index().ok() == Some(bracket.second) => {
                geometric_w_at_bracket(
                    window,
                    stencils,
                    &column,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                    *bracket,
                )?
                .is_none()
            }
            _ => false,
        }
    } else {
        false
    };
    let (horizontal, vertical) = if mixed_route {
        (
            ExplainHorizontalSupport {
                points: base.points,
                first_level_weights: base.weights,
                first_level_method: ExplainHorizontalMethod::TimeBlended,
                second_level_weights: None,
                second_level_method: None,
            },
            ExplainVerticalSupport {
                path: ExplainVerticalPath::TemporalMixedSurfaceUpper,
                first_level: None,
                second_level: None,
                second_weight: None,
            },
        )
    } else {
        match target_bracket {
            Ok(_) if incomplete_bottom_surface_route => (
                ExplainHorizontalSupport {
                    points: base.points,
                    first_level_weights: base.weights,
                    first_level_method: ExplainHorizontalMethod::Bilinear,
                    second_level_weights: None,
                    second_level_method: None,
                },
                ExplainVerticalSupport {
                    path: ExplainVerticalPath::SurfaceLayer,
                    first_level: None,
                    second_level: None,
                    second_weight: None,
                },
            ),
            Ok(bracket) => {
                let first_weights = column.geometry.level_horizontal_weights()[bracket.first];
                let (second_level_weights, second_level_method) = if bracket.second == bracket.first
                {
                    (None, None)
                } else {
                    let weights = column.geometry.level_horizontal_weights()[bracket.second];
                    (
                        Some(weights),
                        Some(explain_horizontal_method(weights, base.weights)),
                    )
                };
                (
                    ExplainHorizontalSupport {
                        points: base.points,
                        first_level_weights: first_weights,
                        first_level_method: explain_horizontal_method(first_weights, base.weights),
                        second_level_weights,
                        second_level_method,
                    },
                    ExplainVerticalSupport {
                        path: if batch.vertical_coordinate == VerticalQuery::Pressure {
                            ExplainVerticalPath::LogPressure
                        } else {
                            ExplainVerticalPath::LinearHeight
                        },
                        first_level: Some(bracket.first),
                        second_level: Some(bracket.second),
                        second_weight: Some(bracket.second_weight),
                    },
                )
            }
            Err(VerticalError::SurfaceLayerRequired) => (
                ExplainHorizontalSupport {
                    points: base.points,
                    first_level_weights: base.weights,
                    first_level_method: ExplainHorizontalMethod::Bilinear,
                    second_level_weights: None,
                    second_level_method: None,
                },
                ExplainVerticalSupport {
                    path: ExplainVerticalPath::SurfaceLayer,
                    first_level: None,
                    second_level: None,
                    second_weight: None,
                },
            ),
            Err(_) => (
                ExplainHorizontalSupport {
                    points: base.points,
                    first_level_weights: base.weights,
                    first_level_method: ExplainHorizontalMethod::Bilinear,
                    second_level_weights: None,
                    second_level_method: None,
                },
                ExplainVerticalSupport {
                    path: ExplainVerticalPath::Unavailable,
                    first_level: None,
                    second_level: None,
                    second_weight: None,
                },
            ),
        }
    };
    let surface_model = matches!(
        vertical.path,
        ExplainVerticalPath::SurfaceLayer | ExplainVerticalPath::TemporalMixedSurfaceUpper
    )
    .then(|| plan.surface_layer_model().cloned())
    .flatten();
    Ok(ExplainRecord {
        domain: window.frames.before.metadata().domain.clone(),
        cell,
        before_frame: window.frames.before.metadata().id.clone(),
        after_frame: window.frames.after.metadata().id.clone(),
        before_weight: window.before_weight,
        after_weight: window.after_weight,
        horizontal,
        vertical,
        surface_model,
        fields: plan
            .fields()
            .iter()
            .cloned()
            .zip(quality.iter().copied())
            .zip(provenance.iter().copied())
            .map(|((field, quality), provenance)| ExplainField {
                field,
                quality,
                provenance,
            })
            .collect(),
    })
}

#[derive(Clone, Debug)]
struct GenericPointResult {
    values: Vec<f64>,
    valid: Vec<bool>,
    quality: Vec<FieldQuality>,
    provenance: Vec<ProvenanceId>,
    status: SampleStatus,
    bounds: Option<VerticalBounds>,
}

fn transport_field_index(field: &FieldKey) -> Option<usize> {
    match field {
        FieldKey::Canonical(CanonicalField::EastwardWind) => Some(0),
        FieldKey::Canonical(CanonicalField::NorthwardWind) => Some(1),
        FieldKey::Canonical(CanonicalField::GeometricVerticalVelocity) => Some(2),
        FieldKey::Canonical(CanonicalField::AirPressure) => Some(3),
        FieldKey::Canonical(CanonicalField::AirTemperature) => Some(4),
        FieldKey::Canonical(CanonicalField::SpecificHumidity) => Some(5),
        FieldKey::Canonical(CanonicalField::AirDensity) => Some(6),
        FieldKey::Canonical(CanonicalField::GeometricTerrainHeight) => Some(7),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn sample_scalar_query_time(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let sample_frame = |prepared: &PreparedFrameStencils| -> Result<f64, EngineError> {
        let column = prepared.column(cell, longitude_degrees, latitude_degrees)?;
        let bracket =
            vertical_bracket(&column, coordinate, vertical_value).map_err(EngineError::Vertical)?;
        let first = sample_level_scalar(prepared, &column, cell, bracket.first, field)?;
        if bracket.first == bracket.second {
            return Ok(first);
        }
        let second = sample_level_scalar(prepared, &column, cell, bracket.second, field)?;
        let value = (second - first).mul_add(bracket.second_weight, first);
        value
            .is_finite()
            .then_some(value)
            .ok_or(EngineError::NumericalFailure)
    };
    let before = sample_frame(frame_stencils(stencils, &window.frames.before)?)?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_frame(frame_stencils(stencils, &window.frames.after)?)?;
    blend_value(before, after, window)
}

fn generic_bounds(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<VerticalBounds, EngineError> {
    if plan.surface_layer_model().is_some() {
        return vertical_bounds_for_point(
            window,
            stencils,
            column,
            cell,
            longitude_degrees,
            latitude_degrees,
        );
    }
    let lowest = column.lowest_valid_index().map_err(EngineError::Vertical)?;
    let minimum_agl_m = column.height_asl_m()[lowest] - column.terrain_asl_m();
    column
        .vertical_bounds(minimum_agl_m)
        .map_err(|_| EngineError::Vertical(VerticalError::InvalidVerticalColumn))
}

fn generic_needs_surface_model(plan: &QueryPlan) -> bool {
    plan.fields()
        .iter()
        .any(|field| matches!(transport_field_index(field), Some(0 | 1 | 2 | 4 | 5 | 6)))
}

#[allow(clippy::too_many_arguments)]
fn requested_surface_exchange_scales(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<Option<SurfaceExchangeScales>, EngineError> {
    let needs_derivation = plan.fields().iter().any(|field| {
        let FieldKey::Canonical(canonical) = field else {
            return false;
        };
        matches!(
            canonical,
            CanonicalField::FrictionVelocity | CanonicalField::MoninObukhovLength
        ) && !has_field_at_query_time(window, *canonical)
    });
    needs_derivation
        .then(|| {
            query_surface_exchange_scales(
                window,
                stencils,
                column,
                cell,
                longitude_degrees,
                latitude_degrees,
            )
        })
        .transpose()
}

#[allow(clippy::too_many_arguments)]
fn sample_generic_surface_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &TargetColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bounds: VerticalBounds,
    metadata: &ExecutionMetadata,
) -> Result<GenericPointResult, EngineError> {
    let transport = if generic_needs_surface_model(plan) && plan.surface_layer_model().is_some() {
        Some(sample_surface_transport(
            plan,
            window,
            stencils,
            column,
            cell,
            longitude_degrees,
            latitude_degrees,
            coordinate,
            vertical_value,
            bounds,
            &metadata.surface_transport,
        )?)
    } else {
        None
    };
    build_generic_surface_like_result(
        plan,
        window,
        stencils,
        &column.geometry,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
        bounds,
        &metadata.generic_surface,
        transport,
    )
}

#[allow(clippy::too_many_arguments)]
fn sample_generic_temporal_mixed_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &ColumnGeometry,
    before_column: &ColumnGeometry,
    after_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bounds: VerticalBounds,
    metadata: &ExecutionMetadata,
) -> Result<GenericPointResult, EngineError> {
    let transport = if generic_needs_surface_model(plan) && plan.surface_layer_model().is_some() {
        Some(sample_temporal_mixed_transport(
            plan,
            window,
            stencils,
            target_column,
            before_column,
            after_column,
            cell,
            longitude_degrees,
            latitude_degrees,
            coordinate,
            vertical_value,
            bounds,
            &metadata.mixed_transport,
        )?)
    } else {
        None
    };
    build_generic_surface_like_result(
        plan,
        window,
        stencils,
        target_column,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
        bounds,
        &metadata.generic_mixed,
        transport,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_generic_surface_like_result(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bounds: VerticalBounds,
    field_metadata: &[FieldMetadata],
    transport: Option<TransportPointResult>,
) -> Result<GenericPointResult, EngineError> {
    if field_metadata.len() != plan.fields().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    let lowest = column.lowest_valid_index().map_err(EngineError::Vertical)?;
    let query_height_agl_m =
        surface_query_height_agl_m(column, coordinate, vertical_value, lowest)?;
    let pressure_pa = surface_query_pressure_pa(
        column,
        coordinate,
        vertical_value,
        query_height_agl_m,
        lowest,
    )?;
    let surface_exchange = requested_surface_exchange_scales(
        plan,
        window,
        stencils,
        column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let mut result = GenericPointResult {
        values: vec![0.0; plan.fields().len()],
        valid: vec![false; plan.fields().len()],
        quality: field_metadata.iter().map(|value| value.quality).collect(),
        provenance: field_metadata
            .iter()
            .map(|value| value.provenance)
            .collect(),
        status: transport
            .as_ref()
            .map_or(SampleStatus::Ok, |value| value.status),
        bounds: Some(bounds),
    };
    for (index, field) in plan.fields().iter().enumerate() {
        if let Some(transport_index) = transport_field_index(field) {
            match transport_index {
                0 | 1 | 2 | 4 | 5 | 6 => {
                    if let Some(surface) = &transport {
                        result.values[index] = surface.values[transport_index];
                        result.valid[index] = surface.valid[transport_index];
                    }
                }
                3 => {
                    result.values[index] = pressure_pa;
                    result.valid[index] = true;
                }
                7 => {
                    result.values[index] = column.terrain_asl_m();
                    result.valid[index] = true;
                }
                _ => return Err(EngineError::InvalidPreparedState),
            }
            continue;
        }
        let FieldKey::Canonical(canonical) = field else {
            return Err(EngineError::UnsupportedQueryField(field.clone()));
        };
        let value = match canonical {
            CanonicalField::GeometricHeight => Some(column.terrain_asl_m() + query_height_agl_m),
            CanonicalField::SurfacePressure => Some(column.surface_pressure_pa()),
            CanonicalField::SurfaceGeopotential => Some(sample_surface_scalar_time(
                window,
                stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                *canonical,
            )?),
            CanonicalField::PressureVerticalVelocity
            | CanonicalField::HybridVerticalVelocity
            | CanonicalField::PotentialVorticity => None,
            CanonicalField::TenMetreEastwardWind | CanonicalField::TenMetreNorthwardWind => {
                let vector = sample_surface_vector_time(
                    window,
                    stencils,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                    CanonicalField::TenMetreEastwardWind,
                    CanonicalField::TenMetreNorthwardWind,
                )?;
                Some(if *canonical == CanonicalField::TenMetreEastwardWind {
                    vector.0
                } else {
                    vector.1
                })
            }
            CanonicalField::TwoMetreAirTemperature
            | CanonicalField::TwoMetreSpecificHumidity
            | CanonicalField::AerodynamicRoughnessLength
            | CanonicalField::BoundaryLayerHeight
            | CanonicalField::EastwardSurfaceStress
            | CanonicalField::NorthwardSurfaceStress
            | CanonicalField::SensibleHeatFlux
            | CanonicalField::LatentHeatFlux => Some(sample_surface_scalar_time(
                window,
                stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                *canonical,
            )?),
            CanonicalField::FrictionVelocity if !has_field_at_query_time(window, *canonical) => {
                Some(
                    surface_exchange
                        .ok_or(EngineError::InvalidPreparedState)?
                        .friction_velocity_m_s,
                )
            }
            CanonicalField::MoninObukhovLength if !has_field_at_query_time(window, *canonical) => {
                Some(
                    surface_exchange
                        .ok_or(EngineError::InvalidPreparedState)?
                        .monin_obukhov_length_m,
                )
            }
            CanonicalField::FrictionVelocity | CanonicalField::MoninObukhovLength => {
                Some(sample_surface_scalar_time(
                    window,
                    stencils,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                    *canonical,
                )?)
            }
            _ => return Err(EngineError::UnsupportedQueryField(field.clone())),
        };
        if let Some(value) = value {
            result.values[index] = value;
            result.valid[index] = true;
        }
    }
    if result.valid.iter().all(|valid| *valid) {
        result.status = SampleStatus::Ok;
    } else if result.status == SampleStatus::Ok {
        result.status = SampleStatus::SurfaceLayerUndefined;
    }
    Ok(result)
}

fn sample_generic_point(
    plan: &QueryPlan,
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    metadata: &ExecutionMetadata,
) -> Result<GenericPointResult, EngineError> {
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let vertical_value = batch.points.vertical[original_index];
    let column = target_column(window, stencils, cell, longitude_degrees, latitude_degrees)?;
    let bounds = generic_bounds(
        plan,
        window,
        stencils,
        &column.geometry,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let target_bracket =
        vertical_bracket(&column.geometry, batch.vertical_coordinate, vertical_value);
    if generic_needs_surface_model(plan) && plan.surface_layer_model().is_some() {
        if let Some((before_column, after_column)) = temporal_mixed_columns(
            window,
            &column,
            batch.vertical_coordinate,
            vertical_value,
            &target_bracket,
        ) {
            return sample_generic_temporal_mixed_point(
                plan,
                window,
                stencils,
                &column.geometry,
                before_column,
                after_column,
                cell,
                longitude_degrees,
                latitude_degrees,
                batch.vertical_coordinate,
                vertical_value,
                bounds,
                metadata,
            );
        }
    }
    let bracket = match target_bracket {
        Ok(bracket) => bracket,
        Err(VerticalError::SurfaceLayerRequired) => {
            return sample_generic_surface_point(
                plan,
                window,
                stencils,
                &column,
                cell,
                longitude_degrees,
                latitude_degrees,
                batch.vertical_coordinate,
                vertical_value,
                bounds,
                metadata,
            );
        }
        Err(error) => {
            return Ok(GenericPointResult {
                values: vec![0.0; plan.fields().len()],
                valid: vec![false; plan.fields().len()],
                quality: metadata
                    .generic_upper
                    .iter()
                    .map(|value| value.quality)
                    .collect(),
                provenance: metadata
                    .generic_upper
                    .iter()
                    .map(|value| value.provenance)
                    .collect(),
                status: sample_status_for_vertical_error(&error),
                bounds: Some(bounds),
            });
        }
    };
    let state = target_frame_point_from_columns(
        window,
        stencils,
        &column.before,
        &column.after,
        cell,
        longitude_degrees,
        latitude_degrees,
        batch.vertical_coordinate,
        vertical_value,
    )?;
    let density = moist_air_density_kg_m3(
        state.pressure_pa,
        state.temperature_k,
        state.physical_specific_humidity,
    )
    .map_err(|_| EngineError::NumericalFailure)?;
    let surface_exchange = requested_surface_exchange_scales(
        plan,
        window,
        stencils,
        &column.geometry,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let needs_w = plan
        .fields()
        .iter()
        .any(|field| field == &FieldKey::Canonical(CanonicalField::GeometricVerticalVelocity));
    let w = if needs_w {
        Some(
            geometric_w_at_bracket(
                window,
                stencils,
                &column,
                cell,
                longitude_degrees,
                latitude_degrees,
                bracket,
            )?
            .ok_or(EngineError::Vertical(VerticalError::InvalidVerticalColumn))?,
        )
    } else {
        None
    };
    let geometric_height = match batch.vertical_coordinate {
        VerticalQuery::AboveSeaLevel => vertical_value,
        VerticalQuery::AboveGround => column.geometry.terrain_asl_m() + vertical_value,
        VerticalQuery::Pressure => bracket
            .interpolate(column.geometry.height_asl_m())
            .map_err(EngineError::Vertical)?,
    };
    let mut result = GenericPointResult {
        values: vec![0.0; plan.fields().len()],
        valid: vec![true; plan.fields().len()],
        quality: metadata
            .generic_upper
            .iter()
            .map(|value| value.quality)
            .collect(),
        provenance: metadata
            .generic_upper
            .iter()
            .map(|value| value.provenance)
            .collect(),
        status: SampleStatus::Ok,
        bounds: Some(bounds),
    };
    for (index, field) in plan.fields().iter().enumerate() {
        if let Some(transport_index) = transport_field_index(field) {
            result.values[index] = match transport_index {
                0 => state.eastward_wind_m_s,
                1 => state.northward_wind_m_s,
                2 => w.ok_or(EngineError::InvalidPreparedState)?,
                3 => state.pressure_pa,
                4 => state.temperature_k,
                5 => state.specific_humidity,
                6 => density,
                7 => column.geometry.terrain_asl_m(),
                _ => return Err(EngineError::InvalidPreparedState),
            };
            continue;
        }
        let FieldKey::Canonical(canonical) = field else {
            return Err(EngineError::UnsupportedQueryField(field.clone()));
        };
        let value = match canonical {
            CanonicalField::GeometricHeight => geometric_height,
            CanonicalField::SurfacePressure => column.geometry.surface_pressure_pa(),
            CanonicalField::PressureVerticalVelocity
            | CanonicalField::HybridVerticalVelocity
            | CanonicalField::PotentialVorticity => sample_scalar_query_time(
                window,
                stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                batch.vertical_coordinate,
                vertical_value,
                *canonical,
            )?,
            CanonicalField::TenMetreEastwardWind | CanonicalField::TenMetreNorthwardWind => {
                let vector = sample_surface_vector_time(
                    window,
                    stencils,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                    CanonicalField::TenMetreEastwardWind,
                    CanonicalField::TenMetreNorthwardWind,
                )?;
                if *canonical == CanonicalField::TenMetreEastwardWind {
                    vector.0
                } else {
                    vector.1
                }
            }
            CanonicalField::SurfaceGeopotential
            | CanonicalField::TwoMetreAirTemperature
            | CanonicalField::TwoMetreSpecificHumidity
            | CanonicalField::AerodynamicRoughnessLength
            | CanonicalField::BoundaryLayerHeight
            | CanonicalField::EastwardSurfaceStress
            | CanonicalField::NorthwardSurfaceStress
            | CanonicalField::SensibleHeatFlux
            | CanonicalField::LatentHeatFlux => sample_surface_scalar_time(
                window,
                stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                *canonical,
            )?,
            CanonicalField::FrictionVelocity if !has_field_at_query_time(window, *canonical) => {
                surface_exchange
                    .ok_or(EngineError::InvalidPreparedState)?
                    .friction_velocity_m_s
            }
            CanonicalField::MoninObukhovLength if !has_field_at_query_time(window, *canonical) => {
                surface_exchange
                    .ok_or(EngineError::InvalidPreparedState)?
                    .monin_obukhov_length_m
            }
            CanonicalField::FrictionVelocity | CanonicalField::MoninObukhovLength => {
                sample_surface_scalar_time(
                    window,
                    stencils,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                    *canonical,
                )?
            }
            _ => return Err(EngineError::UnsupportedQueryField(field.clone())),
        };
        result.values[index] = value;
    }
    Ok(result)
}

fn mesoscale_native_interval_seconds(window: &PreparedWindow) -> Result<f64, EngineError> {
    if !window.is_exact_frame() {
        return seconds_between(
            window.frames.before.metadata().valid_time,
            window.frames.after.metadata().valid_time,
        );
    }
    let previous = window
        .previous
        .as_ref()
        .ok_or(EngineError::InvalidPreparedState)?;
    let next = window
        .next
        .as_ref()
        .ok_or(EngineError::InvalidPreparedState)?;
    let left = seconds_between(
        previous.metadata().valid_time,
        window.frames.before.metadata().valid_time,
    )?;
    let right = seconds_between(
        window.frames.before.metadata().valid_time,
        next.metadata().valid_time,
    )?;
    let interval = 0.5 * (left + right);
    interval
        .is_finite()
        .then_some(interval)
        .filter(|value| *value > 0.0)
        .ok_or(EngineError::InvalidPreparedState)
}

fn mesoscale_vertical_bracket(
    column: &ColumnShape,
    coordinate: VerticalQuery,
    value: f64,
) -> Result<VerticalBracket, VerticalError> {
    match vertical_bracket_shape(column, coordinate, value) {
        Ok(bracket) => Ok(bracket),
        Err(VerticalError::SurfaceLayerRequired) => {
            let level = column.last_valid_index()?;
            Ok(VerticalBracket {
                first: level,
                second: level,
                second_weight: 0.0,
            })
        }
        Err(error) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
fn mesoscale_corner_velocity(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    cell: crate::grid::CellId,
    query_weights: HorizontalWeights,
    query_basis: SphericalBasis,
    corner: usize,
    level: usize,
) -> Result<[f64; 3], EngineError> {
    let frame = match endpoint {
        WindowEndpoint::Before => &window.frames.before,
        WindowEndpoint::After => &window.frames.after,
    };
    let prepared = frame_stencils(stencils, frame)?;
    let stencil = prepared.stencil(cell)?;
    let point = *stencil
        .points()
        .get(corner)
        .ok_or(EngineError::InvalidPreparedState)?;
    let eastward = frame
        .fields()
        .get(&FieldKey::Canonical(CanonicalField::EastwardWind))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(
            CanonicalField::EastwardWind,
        )))?;
    let northward = frame
        .fields()
        .get(&FieldKey::Canonical(CanonicalField::NorthwardWind))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(
            CanonicalField::NorthwardWind,
        )))?;
    let (source_eastward, eastward_valid) = field_3d_value(eastward, level, point)?;
    let (source_northward, northward_valid) = field_3d_value(northward, level, point)?;
    if !eastward_valid || !northward_valid {
        return Err(EngineError::Vertical(
            VerticalError::InvalidHorizontalSupport,
        ));
    }
    let horizontal = query_basis
        .project(stencil.source_bases()[corner].embed(source_eastward, source_northward));
    let vertical = mesoscale_corner_vertical_velocity(
        window,
        stencils,
        endpoint,
        cell,
        query_weights,
        corner,
        level,
        source_eastward,
        source_northward,
    )?;
    let value = [horizontal.0, horizontal.1, vertical];
    value
        .into_iter()
        .all(f64::is_finite)
        .then_some(value)
        .ok_or(EngineError::NumericalFailure)
}

#[allow(clippy::too_many_arguments)]
fn mesoscale_corner_vertical_velocity(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    cell: crate::grid::CellId,
    query_weights: HorizontalWeights,
    corner: usize,
    level: usize,
    eastward_wind_m_s: f64,
    northward_wind_m_s: f64,
) -> Result<f64, EngineError> {
    let frame = match endpoint {
        WindowEndpoint::Before => &window.frames.before,
        WindowEndpoint::After => &window.frames.after,
    };
    let prepared = frame_stencils(stencils, frame)?;
    let point = *prepared
        .stencil(cell)?
        .points()
        .get(corner)
        .ok_or(EngineError::InvalidPreparedState)?;
    let mut corner_weights = [0.0; 4];
    corner_weights[corner] = 1.0;
    let one_hot = HorizontalWeights {
        points: query_weights.points,
        weights: corner_weights,
        valid: [true; 4],
    };
    let latitude_degrees = frame.metadata().grid.latitude_origin_degrees
        + frame.metadata().grid.latitude_spacing_degrees * point.y as f64;
    let shape = prepared.shape_with_weights(cell, one_hot)?;
    if !shape.validity().get(level).copied().unwrap_or(false) {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    }
    let stencil = prepared.stencil(cell)?;
    let (coordinate_kind, native_coordinate) = stencil.native_coordinate();
    if native_coordinate.len() != shape.height_asl_m().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    let vertical_field = vertical_velocity_field(prepared, coordinate_kind);
    let source_vertical_velocity =
        sample_level_scalar_with_weights(prepared, cell, level, corner_weights, vertical_field)?;
    let (geometry, height_time_derivative_m_s, pressure_time_derivative_pa_s) =
        mesoscale_endpoint_level_geometry(
            window,
            stencils,
            endpoint,
            cell,
            one_hot,
            latitude_degrees,
            level,
        )?;
    let pressure_coordinate_derivative = (coordinate_kind == NativeCoordinateKind::HybridEta)
        .then(|| derivative_at(shape.pressure_pa(), native_coordinate, level))
        .transpose()?;
    derive_geometric_w_at_level(
        coordinate_kind,
        vertical_field,
        geometry,
        height_time_derivative_m_s,
        pressure_time_derivative_pa_s,
        eastward_wind_m_s,
        northward_wind_m_s,
        source_vertical_velocity,
        derivative_at(shape.height_asl_m(), native_coordinate, level)?,
        pressure_coordinate_derivative,
    )
}

#[allow(clippy::too_many_arguments)]
fn mesoscale_endpoint_level_geometry(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    endpoint: WindowEndpoint,
    cell: crate::grid::CellId,
    weights: HorizontalWeights,
    latitude_degrees: f64,
    level: usize,
) -> Result<(crate::vertical::NativeLevelGeometry, f64, f64), EngineError> {
    if window.is_exact_frame() {
        return target_level_geometry_with_weights(
            window,
            stencils,
            cell,
            weights,
            latitude_degrees,
            level,
        );
    }
    let before = frame_stencils(stencils, &window.frames.before)?;
    let after = frame_stencils(stencils, &window.frames.after)?;
    let before_geometry = before
        .stencil(cell)?
        .level_geometry_with_weights(&before.frame, cell, weights, latitude_degrees, level)
        .map_err(EngineError::Vertical)?;
    let after_geometry = after
        .stencil(cell)?
        .level_geometry_with_weights(&after.frame, cell, weights, latitude_degrees, level)
        .map_err(EngineError::Vertical)?;
    let seconds = seconds_between(
        window.frames.before.metadata().valid_time,
        window.frames.after.metadata().valid_time,
    )?;
    let geometry = match endpoint {
        WindowEndpoint::Before => before_geometry,
        WindowEndpoint::After => after_geometry,
    };
    Ok((
        geometry,
        (after_geometry.height_asl_m - before_geometry.height_asl_m) / seconds,
        (after_geometry.pressure_pa - before_geometry.pressure_pa) / seconds,
    ))
}

fn sample_mesoscale_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    query_weights: HorizontalWeights,
) -> Result<[f64; 3], EngineError> {
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let vertical_value = batch.points.vertical[original_index];
    let query_basis = spherical_vector_query_basis(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    let column = target_transport_column(window, stencils, cell, query_weights)?;
    let bracket =
        mesoscale_vertical_bracket(column.geometry(), batch.vertical_coordinate, vertical_value)
            .map_err(EngineError::Vertical)?;
    let vertical = [
        (bracket.first, 1.0 - bracket.second_weight),
        (bracket.second, bracket.second_weight),
    ];
    let endpoints = [
        (WindowEndpoint::Before, window.before_weight),
        (WindowEndpoint::After, window.after_weight),
    ];
    let mut support = Vec::with_capacity(16);
    for (endpoint, time_weight) in endpoints {
        if time_weight == 0.0 {
            continue;
        }
        let frame = match endpoint {
            WindowEndpoint::Before => &window.frames.before,
            WindowEndpoint::After => &window.frames.after,
        };
        let frame_stencils = frame_stencils(stencils, frame)?;
        let shape = match endpoint {
            WindowEndpoint::Before => &column.before,
            WindowEndpoint::After => column.after(),
        };
        for (level, vertical_weight) in vertical {
            if vertical_weight == 0.0 {
                continue;
            }
            let horizontal_weights = frame_stencils.level_horizontal_weights_with_weights(
                cell,
                query_weights,
                shape,
                level,
            )?;
            for (corner, horizontal_weight) in horizontal_weights.into_iter().enumerate() {
                let weight = time_weight * vertical_weight * horizontal_weight;
                if weight == 0.0 {
                    continue;
                }
                support.push(WeightedVelocity {
                    weight,
                    velocity_m_s: mesoscale_corner_velocity(
                        window,
                        stencils,
                        endpoint,
                        cell,
                        query_weights,
                        query_basis,
                        corner,
                        level,
                    )?,
                });
            }
        }
    }
    weighted_variance(&support).map_err(|_| EngineError::NumericalFailure)
}

fn stability_target_height_asl_m(
    column: &ColumnShape,
    coordinate: VerticalQuery,
    value: f64,
) -> Result<f64, EngineError> {
    match coordinate {
        VerticalQuery::AboveSeaLevel => match column.locate_height_asl_m(value) {
            Ok(_) | Err(VerticalError::SurfaceLayerRequired) => Ok(value),
            Err(error) => Err(EngineError::Vertical(error)),
        },
        VerticalQuery::AboveGround => {
            let height = column.terrain_asl_m() + value;
            match column.locate_height_agl_m(value) {
                Ok(_) | Err(VerticalError::SurfaceLayerRequired) => Ok(height),
                Err(error) => Err(EngineError::Vertical(error)),
            }
        }
        VerticalQuery::Pressure => {
            let bracket = column
                .locate_pressure_pa(value)
                .map_err(EngineError::Vertical)?;
            bracket
                .interpolate(column.height_asl_m())
                .map_err(EngineError::Vertical)
        }
    }
}

fn interpolate_layer_value(
    target_height_asl_m: f64,
    midpoint_height_asl_m: &[f64],
    values: &[f64],
) -> Result<f64, EngineError> {
    if midpoint_height_asl_m.is_empty()
        || midpoint_height_asl_m.len() != values.len()
        || midpoint_height_asl_m
            .windows(2)
            .any(|pair| pair[1] >= pair[0])
    {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    }
    if target_height_asl_m >= midpoint_height_asl_m[0] {
        return Ok(values[0]);
    }
    if target_height_asl_m <= midpoint_height_asl_m[midpoint_height_asl_m.len() - 1] {
        return values
            .last()
            .copied()
            .ok_or(EngineError::InvalidPreparedState);
    }
    for (index, heights) in midpoint_height_asl_m.windows(2).enumerate() {
        if target_height_asl_m <= heights[0] && target_height_asl_m >= heights[1] {
            let fraction = (heights[0] - target_height_asl_m) / (heights[0] - heights[1]);
            let value = (values[index + 1] - values[index]).mul_add(fraction, values[index]);
            return value
                .is_finite()
                .then_some(value)
                .ok_or(EngineError::NumericalFailure);
        }
    }
    Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn))
}

fn sample_stability_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
    weights: HorizontalWeights,
) -> Result<f64, EngineError> {
    let column = target_transport_column(window, stencils, cell, weights)?;
    let geometry = column.geometry();
    let target_height_asl_m = stability_target_height_asl_m(
        geometry,
        batch.vertical_coordinate,
        batch.points.vertical[original_index],
    )?;
    let first = geometry
        .first_valid_index()
        .map_err(EngineError::Vertical)?;
    let last = geometry.last_valid_index().map_err(EngineError::Vertical)?;
    if last <= first || geometry.validity()[first..=last].iter().any(|valid| !valid) {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    }

    let mut theta = Vec::with_capacity(last - first + 1);
    for level in first..=last {
        let temperature_k = time_level_scalar_from_shapes(
            window,
            stencils,
            &column,
            weights,
            cell,
            level,
            CanonicalField::AirTemperature,
        )?;
        theta.push(
            potential_temperature_k(temperature_k, geometry.pressure_pa()[level])
                .map_err(|_| EngineError::NumericalFailure)?,
        );
    }

    let heights = &geometry.height_asl_m()[first..=last];
    let mut layer_midpoints = Vec::with_capacity(heights.len() - 1);
    let mut frequency_squared = Vec::with_capacity(heights.len() - 1);
    for (level, height_pair) in heights.windows(2).enumerate() {
        let dz = height_pair[1] - height_pair[0];
        let theta_mean = 0.5 * (theta[level] + theta[level + 1]);
        if !dz.is_finite() || dz >= 0.0 || !theta_mean.is_finite() || theta_mean <= 0.0 {
            return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
        }
        let value = M3_CONSTANTS.standard_gravity_m_s2 / theta_mean
            * ((theta[level + 1] - theta[level]) / dz);
        if !value.is_finite() {
            return Err(EngineError::NumericalFailure);
        }
        layer_midpoints.push(0.5 * (height_pair[0] + height_pair[1]));
        frequency_squared.push(value);
    }
    interpolate_layer_value(target_height_asl_m, &layer_midpoints, &frequency_squared)
}

fn sample_convection_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    batch: &QueryBatch,
    cell: crate::grid::CellId,
    original_index: usize,
) -> Result<ColumnGeometry, EngineError> {
    let longitude_degrees = batch.points.longitude_degrees[original_index];
    let latitude_degrees = batch.points.latitude_degrees[original_index];
    let column =
        target_column(window, stencils, cell, longitude_degrees, latitude_degrees)?.geometry;
    let first = column
        .available_top_index()
        .map_err(EngineError::Vertical)?;
    let last = column.lowest_valid_index().map_err(EngineError::Vertical)?;
    if last <= first
        || column.validity().valid[first..=last]
            .iter()
            .any(|valid| !valid)
    {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    }
    let specific_humidity = column
        .specific_humidity()
        .iter()
        .copied()
        .map(project_specific_humidity_nonnegative)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| EngineError::NumericalFailure)?;
    ColumnGeometry::new(
        column.pressure_pa().clone(),
        column.height_asl_m().clone(),
        column.temperature_k().clone(),
        Arc::from(specific_humidity),
        column.level_horizontal_weights().clone(),
        column.validity().clone(),
        column.terrain_asl_m(),
        column.surface_pressure_pa(),
        column.physical_model_top_asl_m(),
    )
    .map_err(EngineError::Vertical)
}

fn validate_execution_state(
    context: &dyn ExecutionContext,
    layout: &BatchLayout,
    point_count: usize,
) -> Result<(), EngineError> {
    if context.worker_count() == 0 {
        return Err(EngineError::InvalidExecutionContext);
    }
    let internal_count = layout.permutation.forward.len();
    if layout.cell_ids.len() != internal_count
        || layout.domain_ids.len() != internal_count
        || layout.permutation.inverse.len() != point_count
        || layout.chunks.boundaries.first().copied() != Some(0)
        || layout.chunks.boundaries.last().copied() != Some(internal_count)
        || layout
            .chunks
            .boundaries
            .windows(2)
            .any(|values| values[1] < values[0])
    {
        return Err(EngineError::InvalidPreparedState);
    }
    Ok(())
}

fn sample_transport_chunk(
    prepared: &PreparedTransportBatch,
    metadata: &ExecutionMetadata,
    start: usize,
    end: usize,
    worker_count: usize,
) -> Result<Vec<Result<TransportPointResult, EngineError>>, EngineError> {
    let sample = |internal_index: usize| {
        let original_index = prepared.layout.permutation.forward[internal_index];
        let cell = prepared.layout.cell_ids[internal_index];
        let weights = prepared
            .horizontal_support
            .get(original_index)
            .copied()
            .flatten()
            .ok_or(EngineError::InvalidPreparedState)?;
        sample_transport_point(
            prepared.plan.query_plan(),
            &prepared.window,
            &prepared.stencils,
            &prepared.batch,
            cell,
            original_index,
            weights,
            metadata,
        )
    };
    let point_count = end.saturating_sub(start);
    if worker_count == 1 || point_count < PARALLEL_TRANSPORT_MIN_POINTS {
        return Ok((start..end).map(sample).collect());
    }

    let pool = execution_pool(worker_count)?;
    Ok(pool.install(|| (start..end).into_par_iter().map(sample).collect()))
}

fn sample_mesoscale_chunk(
    prepared: &PreparedMesoscaleBatch,
    start: usize,
    end: usize,
    worker_count: usize,
) -> Result<Vec<Result<[f64; 3], EngineError>>, EngineError> {
    let sample = |internal_index: usize| {
        let original_index = prepared.layout.permutation.forward[internal_index];
        let cell = prepared.layout.cell_ids[internal_index];
        let weights = prepared
            .horizontal_support
            .get(original_index)
            .copied()
            .flatten()
            .ok_or(EngineError::InvalidPreparedState)?;
        sample_mesoscale_point(
            &prepared.window,
            &prepared.stencils,
            &prepared.batch,
            cell,
            original_index,
            weights,
        )
    };
    let point_count = end.saturating_sub(start);
    if worker_count == 1 || point_count < PARALLEL_TRANSPORT_MIN_POINTS {
        return Ok((start..end).map(sample).collect());
    }
    let pool = execution_pool(worker_count)?;
    Ok(pool.install(|| (start..end).into_par_iter().map(sample).collect()))
}

fn sample_stability_chunk(
    prepared: &PreparedStabilityBatch,
    start: usize,
    end: usize,
    worker_count: usize,
) -> Result<Vec<Result<f64, EngineError>>, EngineError> {
    let sample = |internal_index: usize| {
        let original_index = prepared.layout.permutation.forward[internal_index];
        let cell = prepared.layout.cell_ids[internal_index];
        let weights = prepared
            .horizontal_support
            .get(original_index)
            .copied()
            .flatten()
            .ok_or(EngineError::InvalidPreparedState)?;
        sample_stability_point(
            &prepared.window,
            &prepared.stencils,
            &prepared.batch,
            cell,
            original_index,
            weights,
        )
    };
    let point_count = end.saturating_sub(start);
    if worker_count == 1 || point_count < PARALLEL_TRANSPORT_MIN_POINTS {
        return Ok((start..end).map(sample).collect());
    }
    let pool = execution_pool(worker_count)?;
    Ok(pool.install(|| (start..end).into_par_iter().map(sample).collect()))
}

fn sample_convection_chunk(
    prepared: &PreparedConvectionBatch,
    start: usize,
    end: usize,
    worker_count: usize,
) -> Result<Vec<Result<ColumnGeometry, EngineError>>, EngineError> {
    let sample = |internal_index: usize| {
        let original_index = prepared.layout.permutation.forward[internal_index];
        sample_convection_point(
            &prepared.window,
            &prepared.stencils,
            &prepared.batch,
            prepared.layout.cell_ids[internal_index],
            original_index,
        )
    };
    let point_count = end.saturating_sub(start);
    if worker_count == 1 || point_count < PARALLEL_TRANSPORT_MIN_POINTS {
        return Ok((start..end).map(sample).collect());
    }
    let pool = execution_pool(worker_count)?;
    Ok(pool.install(|| (start..end).into_par_iter().map(sample).collect()))
}

/// Fully pinned local space-time wind support for M6 mesoscale motion.
#[derive(Clone, Debug)]
pub struct PreparedMesoscaleBatch {
    /// Immutable pinned time window.
    pub window: PreparedWindow,
    /// Original query batch.
    pub batch: QueryBatch,
    /// Backend-neutral deterministic layout.
    pub layout: BatchLayout,
    /// Preparation-time local statuses; executable points remain `Ok`.
    pub initial_status: Vec<SampleStatus>,
    horizontal_support: Vec<Option<HorizontalWeights>>,
    stencils: PreparedStencils,
}

impl PreparedMesoscaleBatch {
    /// Computes three-component weighted wind variance without source I/O.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<MesoscaleStatisticsOutput, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
        if self.horizontal_support.len() != self.initial_status.len() {
            return Err(EngineError::InvalidPreparedState);
        }
        let native_interval_seconds = mesoscale_native_interval_seconds(&self.window)?;
        let mut status = self.initial_status.clone();
        let mut variance_m2_s2 = vec![[0.0; 3]; status.len()];
        for boundaries in self.layout.chunks.boundaries.windows(2) {
            let sampled =
                sample_mesoscale_chunk(self, boundaries[0], boundaries[1], context.worker_count())?;
            for (internal_index, point) in (boundaries[0]..boundaries[1]).zip(sampled) {
                let original_index = self.layout.permutation.forward[internal_index];
                match point {
                    Ok(variance) => variance_m2_s2[original_index] = variance,
                    Err(error) => {
                        if let Some(local_status) = local_status_for_error(&error) {
                            status[original_index] = local_status;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        MesoscaleStatisticsOutput::new(
            variance_m2_s2,
            StatusColumn::new(status),
            native_interval_seconds,
        )
        .map_err(|_| EngineError::InvalidPreparedState)
    }
}

/// Fully pinned complete thermodynamic columns for M6 deep convection.
#[derive(Clone, Debug)]
pub struct PreparedConvectionBatch {
    /// Immutable pinned time window.
    pub window: PreparedWindow,
    /// Original query batch.
    pub batch: QueryBatch,
    /// Backend-neutral deterministic layout.
    pub layout: BatchLayout,
    /// Preparation-time local statuses; executable points remain `Ok`.
    pub initial_status: Vec<SampleStatus>,
    stencils: PreparedStencils,
}

impl PreparedConvectionBatch {
    /// Builds complete local columns without source I/O.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<ConvectionColumnOutput, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
        let mut status = self.initial_status.clone();
        let mut columns = vec![None; status.len()];
        for boundaries in self.layout.chunks.boundaries.windows(2) {
            let sampled = sample_convection_chunk(
                self,
                boundaries[0],
                boundaries[1],
                context.worker_count(),
            )?;
            for (internal_index, point) in (boundaries[0]..boundaries[1]).zip(sampled) {
                let original_index = self.layout.permutation.forward[internal_index];
                match point {
                    Ok(column) => columns[original_index] = Some(column),
                    Err(error) => {
                        if let Some(local_status) = local_status_for_error(&error) {
                            status[original_index] = local_status;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        ConvectionColumnOutput::new(columns, StatusColumn::new(status))
            .map_err(|_| EngineError::InvalidPreparedState)
    }
}

/// Fully pinned local thermodynamic columns for M6 terrain stability.
#[derive(Clone, Debug)]
pub struct PreparedStabilityBatch {
    /// Immutable pinned time window.
    pub window: PreparedWindow,
    /// Original query batch.
    pub batch: QueryBatch,
    /// Backend-neutral deterministic layout.
    pub layout: BatchLayout,
    /// Preparation-time local statuses; executable points remain `Ok`.
    pub initial_status: Vec<SampleStatus>,
    horizontal_support: Vec<Option<HorizontalWeights>>,
    stencils: PreparedStencils,
}

impl PreparedStabilityBatch {
    /// Computes dry-column Brunt-Vaisala frequency without source I/O.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<StabilityOutput, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
        if self.horizontal_support.len() != self.initial_status.len() {
            return Err(EngineError::InvalidPreparedState);
        }
        let mut status = self.initial_status.clone();
        let mut values = vec![0.0; status.len()];
        for boundaries in self.layout.chunks.boundaries.windows(2) {
            let sampled =
                sample_stability_chunk(self, boundaries[0], boundaries[1], context.worker_count())?;
            for (internal_index, point) in (boundaries[0]..boundaries[1]).zip(sampled) {
                let original_index = self.layout.permutation.forward[internal_index];
                match point {
                    Ok(value) => values[original_index] = value,
                    Err(error) => {
                        if let Some(local_status) = local_status_for_error(&error) {
                            status[original_index] = local_status;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        StabilityOutput::new(values, StatusColumn::new(status))
            .map_err(|_| EngineError::InvalidPreparedState)
    }
}

/// Fully pinned, I/O-free geometry for continuous boundary policies.
#[derive(Clone, Debug)]
pub struct PreparedBoundaryBatch {
    /// Immutable complete-transport identity supplying capabilities and floor policy.
    pub plan: TransportPlan,
    /// Immutable pinned time window.
    pub window: PreparedWindow,
    /// Original query batch.
    pub batch: QueryBatch,
    /// Backend-neutral deterministic layout.
    pub layout: BatchLayout,
    /// Preparation-time local statuses; executable points remain `Ok`.
    pub initial_status: Vec<SampleStatus>,
    stencils: PreparedStencils,
    query_counter: Option<Arc<QueryCallCounters>>,
    exact_query_key: Option<ExactQueryKey>,
}

impl PreparedBoundaryBatch {
    /// Executes only domain, terrain, transport-floor, and vertical-top geometry.
    ///
    /// No wind, thermodynamic, vertical-velocity, surface-layer, provenance, or
    /// explain columns are evaluated. Reader/provider I/O and cache mutation are
    /// forbidden, matching the other prepared execution paths.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<BoundaryQueryOutput, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
        if let (Some(counter), Some(key)) = (&self.query_counter, &self.exact_query_key) {
            counter.record_execution(*key);
        }
        let point_count = self.initial_status.len();
        let mut status = self.initial_status.clone();
        let mut bounds = vec![None; point_count];
        let mut terrain_height_asl_m = vec![None; point_count];
        for boundaries in self.layout.chunks.boundaries.windows(2) {
            for internal_index in boundaries[0]..boundaries[1] {
                let original_index = self.layout.permutation.forward[internal_index];
                let cell = self.layout.cell_ids[internal_index];
                match sample_boundary_point(
                    &self.window,
                    &self.stencils,
                    &self.batch,
                    cell,
                    original_index,
                ) {
                    Ok(point) => {
                        status[original_index] = point.status;
                        bounds[original_index] = Some(point.bounds);
                        terrain_height_asl_m[original_index] = point.terrain_height_asl_m;
                    }
                    Err(error) => {
                        if let Some(local_status) = local_status_for_error(&error) {
                            status[original_index] = local_status;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        BoundaryQueryOutput::new(
            StatusColumn::new(status),
            BoundsColumn::new(bounds),
            terrain_height_asl_m,
        )
        .map_err(EngineError::Output)
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
    /// Preparation-time local statuses; executable points remain `Ok`.
    pub initial_status: Vec<SampleStatus>,
    stencils: PreparedStencils,
    query_counter: Option<Arc<QueryCallCounters>>,
    exact_query_key: Option<ExactQueryKey>,
}

impl PreparedBatch {
    /// Executes without reader, provider, or cache mutation.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<QueryOutput, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
        if let (Some(counter), Some(key)) = (&self.query_counter, &self.exact_query_key) {
            counter.record_execution(*key);
        }
        let metadata = build_execution_metadata(&self.plan, &self.window)?;
        let point_count = self.initial_status.len();
        let field_count = self.plan.fields().len();
        let mut values = (0..field_count)
            .map(|_| vec![0.0; point_count])
            .collect::<Vec<_>>();
        let mut valid = (0..field_count)
            .map(|_| vec![false; point_count])
            .collect::<Vec<_>>();
        let mut quality = metadata
            .generic_upper
            .iter()
            .map(|value| vec![value.quality; point_count])
            .collect::<Vec<_>>();
        let mut provenance = metadata
            .generic_upper
            .iter()
            .map(|value| vec![value.provenance; point_count])
            .collect::<Vec<_>>();
        let mut status = self.initial_status.clone();
        let mut bounds = vec![None; point_count];
        let mut explain =
            (self.plan.explain_mode() == ExplainMode::Full).then(|| vec![None; point_count]);
        for boundaries in self.layout.chunks.boundaries.windows(2) {
            for internal_index in boundaries[0]..boundaries[1] {
                let original_index = self.layout.permutation.forward[internal_index];
                let cell = self.layout.cell_ids[internal_index];
                match sample_generic_point(
                    &self.plan,
                    &self.window,
                    &self.stencils,
                    &self.batch,
                    cell,
                    original_index,
                    &metadata,
                ) {
                    Ok(point) => {
                        status[original_index] = point.status;
                        bounds[original_index] = point.bounds;
                        for field_index in 0..field_count {
                            values[field_index][original_index] = point.values[field_index];
                            valid[field_index][original_index] = point.valid[field_index];
                            quality[field_index][original_index] = point.quality[field_index];
                            provenance[field_index][original_index] = point.provenance[field_index];
                        }
                        if let Some(records) = &mut explain {
                            let point_quality = quality
                                .iter()
                                .map(|column| column[original_index])
                                .collect::<Vec<_>>();
                            let point_provenance = provenance
                                .iter()
                                .map(|column| column[original_index])
                                .collect::<Vec<_>>();
                            records[original_index] = Some(explain_record_for_point(
                                &self.plan,
                                &self.window,
                                &self.stencils,
                                &self.batch,
                                cell,
                                original_index,
                                &point_quality,
                                &point_provenance,
                                false,
                            )?);
                        }
                    }
                    Err(error) => {
                        if let Some(local_status) = local_status_for_error(&error) {
                            status[original_index] = local_status;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        let fields = self
            .plan
            .fields()
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, field)| {
                ValueColumn::new(
                    std::mem::take(&mut values[index]),
                    ValidityMask::from_bools(&valid[index]),
                    std::mem::take(&mut quality[index]),
                    std::mem::take(&mut provenance[index]),
                )
                .map(|samples| FieldColumn::new(field, samples))
                .map_err(EngineError::Output)
            })
            .collect::<Result<Vec<_>, _>>()?;
        QueryOutput::new(
            fields,
            StatusColumn::new(status),
            BoundsColumn::new(bounds),
            metadata.table,
            explain,
        )
        .map_err(EngineError::Output)
    }
}

/// Fully pinned, I/O-free complete-transport execution state.
#[derive(Clone, Debug)]
pub struct PreparedTransportBatch {
    /// Immutable fixed transport field/dependency plan.
    pub plan: TransportPlan,
    /// Immutable pinned time window.
    pub window: PreparedWindow,
    /// Original query batch.
    pub batch: QueryBatch,
    /// Backend-neutral deterministic layout.
    pub layout: BatchLayout,
    /// Preparation-time local statuses; executable points remain `Ok`.
    pub initial_status: Vec<SampleStatus>,
    horizontal_support: Vec<Option<HorizontalWeights>>,
    stencils: PreparedStencils,
    query_counter: Option<Arc<QueryCallCounters>>,
    exact_query_key: Option<ExactQueryKey>,
    transport_cache: Option<LastTransportQueryCache>,
    transport_cache_key: ExactTransportCacheKey,
    populate_transport_cache: bool,
}

impl PreparedTransportBatch {
    /// Executes all eight transport columns without reader or cache mutation.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<Arc<TransportOutput>, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
        if self.horizontal_support.len() != self.initial_status.len() {
            return Err(EngineError::InvalidPreparedState);
        }
        if let Some(cache) = &self.transport_cache {
            let cached = cache
                .lock()
                .map_err(|_| EngineError::TransportCachePoisoned)?;
            let output = cached
                .as_ref()
                .and_then(|cached| {
                    self.transport_cache_key
                        .selection_from_cached(&cached.key)
                        .map(|selection| (cached, selection))
                })
                .map(|(cached, selection)| {
                    if selection.len() == cached.output.status().len()
                        && selection.iter().copied().eq(0..selection.len())
                    {
                        Ok(Arc::clone(&cached.output))
                    } else {
                        cached.output.select_rows(&selection).map(Arc::new)
                    }
                })
                .transpose()
                .map_err(EngineError::Output)?;
            if let Some(output) = output {
                if let (Some(counter), Some(key)) = (&self.query_counter, self.exact_query_key) {
                    counter.record_exact_key_reuse(key);
                }
                return Ok(output);
            }
        }
        if let (Some(counter), Some(key)) = (&self.query_counter, &self.exact_query_key) {
            counter.record_execution(*key);
        }
        let metadata = build_execution_metadata(self.plan.query_plan(), &self.window)?;
        let point_count = self.initial_status.len();
        let mut values: [Vec<f64>; TRANSPORT_FIELD_COUNT] =
            std::array::from_fn(|_| vec![0.0; point_count]);
        let mut valid: [Vec<bool>; TRANSPORT_FIELD_COUNT] =
            std::array::from_fn(|_| vec![false; point_count]);
        let mut quality: [Vec<FieldQuality>; TRANSPORT_FIELD_COUNT] =
            std::array::from_fn(|index| vec![metadata.upper_transport[index].quality; point_count]);
        let mut provenance: [Vec<ProvenanceId>; TRANSPORT_FIELD_COUNT] =
            std::array::from_fn(|index| {
                vec![metadata.upper_transport[index].provenance; point_count]
            });
        let mut status = self.initial_status.clone();
        let mut bounds = vec![None; point_count];
        let mut explain = (self.plan.query_plan().explain_mode() == ExplainMode::Full)
            .then(|| vec![None; point_count]);
        for boundaries in self.layout.chunks.boundaries.windows(2) {
            let sampled = sample_transport_chunk(
                self,
                &metadata,
                boundaries[0],
                boundaries[1],
                context.worker_count(),
            )?;
            for (internal_index, point) in (boundaries[0]..boundaries[1]).zip(sampled) {
                let original_index = self.layout.permutation.forward[internal_index];
                let cell = self.layout.cell_ids[internal_index];
                match point {
                    Ok(point) => {
                        status[original_index] = point.status;
                        bounds[original_index] = point.bounds;
                        for field in 0..TRANSPORT_FIELD_COUNT {
                            values[field][original_index] = point.values[field];
                            valid[field][original_index] = point.valid[field];
                            quality[field][original_index] = point.quality[field];
                            provenance[field][original_index] = point.provenance[field];
                        }
                        if let Some(records) = &mut explain {
                            let point_quality = (0..TRANSPORT_FIELD_COUNT)
                                .map(|field| quality[field][original_index])
                                .collect::<Vec<_>>();
                            let point_provenance = (0..TRANSPORT_FIELD_COUNT)
                                .map(|field| provenance[field][original_index])
                                .collect::<Vec<_>>();
                            records[original_index] = Some(explain_record_for_point(
                                self.plan.query_plan(),
                                &self.window,
                                &self.stencils,
                                &self.batch,
                                cell,
                                original_index,
                                &point_quality,
                                &point_provenance,
                                true,
                            )?);
                        }
                    }
                    Err(error) => {
                        if let Some(local_status) = local_status_for_error(&error) {
                            status[original_index] = local_status;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        let mut build_column = |index: usize| -> Result<ValueColumn, EngineError> {
            ValueColumn::new(
                std::mem::take(&mut values[index]),
                ValidityMask::from_bools(&valid[index]),
                std::mem::take(&mut quality[index]),
                std::mem::take(&mut provenance[index]),
            )
            .map_err(EngineError::Output)
        };
        let output = Arc::new(
            TransportOutput::new(
                TransportColumns {
                    eastward_wind_m_s: build_column(0)?,
                    northward_wind_m_s: build_column(1)?,
                    geometric_vertical_velocity_m_s: build_column(2)?,
                    air_pressure_pa: build_column(3)?,
                    air_temperature_k: build_column(4)?,
                    specific_humidity: build_column(5)?,
                    air_density_kg_m3: build_column(6)?,
                    terrain_height_asl_m: build_column(7)?,
                },
                StatusColumn::new(status),
                BoundsColumn::new(bounds),
                metadata.table,
                explain,
            )
            .map_err(EngineError::Output)?,
        );
        if let (true, Some(cache)) = (self.populate_transport_cache, &self.transport_cache) {
            *cache
                .lock()
                .map_err(|_| EngineError::TransportCachePoisoned)? = Some(CachedTransportQuery {
                key: self.transport_cache_key.clone(),
                output: Arc::clone(&output),
            });
        }
        Ok(output)
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
    /// The catalog contains no queryable domain.
    NoDomains,
    /// Implicit preparation is ambiguous because multiple domains exist.
    AmbiguousDomain,
    /// The window was not created by a live engine and has no preparation cache.
    ColumnCacheUnavailable,
    /// The engine-owned preparation cache lock was poisoned.
    ColumnCachePoisoned,
    /// The engine-owned exact transport cache lock was poisoned.
    TransportCachePoisoned,
    /// Column-cache budget or identity handling failed.
    Cache(CacheError),
    /// A field promised by the compiled capability is absent from a pinned frame.
    MissingField(FieldKey),
    /// The generic executor does not implement this field in the M3 slice.
    UnsupportedQueryField(FieldKey),
    /// Frame preparation failed before query execution.
    Frame(FrameError),
    /// Query plan or input batch is structurally invalid.
    QueryPlan(QueryPlanError),
    /// Horizontal geometry or location failed structurally.
    Grid(GridError),
    /// Local vertical stencil construction failed structurally.
    Vertical(VerticalError),
    /// Deterministic grouping or hard-budget chunking failed.
    Layout(LayoutError),
    /// Output columns violated the public finite-value or length contract.
    Output(OutputError),
    /// Output-level scientific provenance could not be interned.
    Provenance(ProvenanceError),
    /// Conservative per-point memory arithmetic overflowed.
    MemoryEstimateOverflow,
    /// Point-local deterministic arithmetic failed.
    NumericalFailure,
    /// Execution context reports zero workers.
    InvalidExecutionContext,
    /// The bounded parallel execution pool could not be constructed or accessed.
    ExecutionPoolUnavailable,
    /// Prepared layout or output columns are inconsistent.
    InvalidPreparedState,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use trajecta_case::model::meteorology::DomainId;
    use trajecta_case::model::physics::ModelId;
    use trajecta_case::model::time::Timestamp;
    use trajecta_case::quantity::Unit;

    use crate::frame::{FrameMetadata, RawFieldStore, TemporalSupport};
    use crate::grid::{CellId, DomainGeometry};
    use crate::io::inventory::{DomainCatalog, LogicalFrameId, MetCatalog};
    use crate::profile::graph::{ExecutionPlan, GraphUnit};
    use crate::provenance::{ProvenanceRecord, ProvenanceTable};
    use crate::query::metrics::{
        QueryCallCounters, QueryOrigin, QueryOriginScope, clear_query_counters,
        install_query_counters,
    };
    use crate::query::request::QueryPointArrays;
    use crate::science::M3_CONSTANTS;
    use crate::surface_layer::MoninObukhovBusingerDyer;
    use crate::vertical::{
        ColumnBuilder, HybridCoefficients, HybridColumnBuilder, HybridPressureTopology,
        PressureLevels,
    };

    const DX: f64 = 6_371_229.0 * std::f64::consts::PI / 180.0;

    fn add_field(
        store: &mut RawFieldStore,
        table: &mut ProvenanceTable,
        frame_id: &LogicalFrameId,
        field: CanonicalField,
        values: Vec<f64>,
        layout: ArrayLayout,
    ) {
        add_field_with_quality(
            store,
            table,
            frame_id,
            field,
            values,
            layout,
            FieldQuality::Source,
        );
    }

    fn add_field_with_quality(
        store: &mut RawFieldStore,
        table: &mut ProvenanceTable,
        frame_id: &LogicalFrameId,
        field: CanonicalField,
        values: Vec<f64>,
        layout: ArrayLayout,
        quality: FieldQuality,
    ) {
        let key = FieldKey::Canonical(field);
        let provenance = table
            .intern(ProvenanceRecord {
                field: key.clone(),
                quality,
                sources: vec![frame_id.content_sha256.clone()],
                transforms: Vec::new(),
                fallback_reason: None,
                profile_sha256: frame_id.profile_sha256.clone(),
            })
            .unwrap();
        let count = layout.element_count().unwrap();
        store
            .insert(
                key,
                RawField::new(
                    Arc::from(values),
                    Arc::from(vec![true; count]),
                    GraphUnit::parse(field.semantics().unit).unwrap(),
                    layout,
                    TemporalSupport::Instantaneous {
                        valid_time: frame_id.valid_time,
                    },
                    quality,
                    provenance,
                )
                .unwrap(),
            )
            .unwrap();
    }

    fn analytic_pressure_frame(seconds: i64) -> Arc<RawMetFrame> {
        analytic_pressure_frame_with_estimated(seconds, None)
    }

    fn flat_calm_pressure_frame(seconds: i64) -> Arc<RawMetFrame> {
        let frame = analytic_pressure_frame(seconds);
        let mut fields = RawFieldStore::new();
        for (key, field) in frame.fields().iter() {
            let values = match key {
                FieldKey::Canonical(CanonicalField::GeometricHeight) => Some(
                    vec![6_000.0; 4]
                        .into_iter()
                        .chain(vec![1_000.0; 4])
                        .collect(),
                ),
                FieldKey::Canonical(
                    CanonicalField::EastwardWind
                    | CanonicalField::NorthwardWind
                    | CanonicalField::PressureVerticalVelocity,
                ) => Some(vec![0.0; 8]),
                FieldKey::Canonical(CanonicalField::GeometricTerrainHeight) => Some(vec![100.0; 4]),
                _ => None,
            };
            let field = if let Some(values) = values {
                RawField::new(
                    Arc::from(values),
                    field.validity().as_arc().clone(),
                    field.unit().clone(),
                    field.layout(),
                    field.temporal(),
                    field.quality(),
                    field.provenance(),
                )
                .unwrap()
            } else {
                field.clone()
            };
            fields.insert(key.clone(), field).unwrap();
        }
        Arc::new(
            RawMetFrame::publish(frame.metadata().clone(), fields, frame.provenance().clone())
                .unwrap(),
        )
    }

    fn analytic_pressure_frame_with_low_bottom(
        seconds: i64,
        incomplete_bottom_transport: bool,
    ) -> Arc<RawMetFrame> {
        let frame = analytic_pressure_frame(seconds);
        let geometric_height = FieldKey::Canonical(CanonicalField::GeometricHeight);
        let terrain_height = FieldKey::Canonical(CanonicalField::GeometricTerrainHeight);
        let vertical_velocity = FieldKey::Canonical(CanonicalField::PressureVerticalVelocity);
        let mut fields = RawFieldStore::new();
        for (key, field) in frame.fields().iter() {
            let replacement = if key == &geometric_height {
                Some((
                    vec![1_000.0; 4].into_iter().chain(vec![100.0; 4]).collect(),
                    None,
                ))
            } else if key == &terrain_height {
                Some((vec![0.0; 4], None))
            } else if key == &vertical_velocity && incomplete_bottom_transport {
                if let ArrayLayout::Full3D { levels, ny, nx } = field.layout() {
                    let mut valid = field.validity().as_arc().to_vec();
                    valid[(levels - 1) * ny * nx..].fill(false);
                    Some((field.values().to_vec(), Some(valid)))
                } else {
                    None
                }
            } else {
                None
            };
            let field = if let Some((values, valid)) = replacement {
                RawField::new(
                    Arc::from(values),
                    Arc::from(valid.unwrap_or_else(|| field.validity().as_arc().to_vec())),
                    field.unit().clone(),
                    field.layout(),
                    field.temporal(),
                    field.quality(),
                    field.provenance(),
                )
                .unwrap()
            } else {
                field.clone()
            };
            fields.insert(key.clone(), field).unwrap();
        }
        Arc::new(
            RawMetFrame::publish(frame.metadata().clone(), fields, frame.provenance().clone())
                .unwrap(),
        )
    }

    fn analytic_pressure_frame_without_surface_layer_inputs(seconds: i64) -> Arc<RawMetFrame> {
        let frame = analytic_pressure_frame(seconds);
        let mut fields = RawFieldStore::new();
        for (key, field) in frame.fields().iter() {
            let is_surface_layer_input = matches!(
                key,
                &FieldKey::Canonical(
                    CanonicalField::TenMetreEastwardWind
                        | CanonicalField::TenMetreNorthwardWind
                        | CanonicalField::TwoMetreAirTemperature
                        | CanonicalField::TwoMetreSpecificHumidity
                        | CanonicalField::AerodynamicRoughnessLength
                        | CanonicalField::BoundaryLayerHeight
                        | CanonicalField::EastwardSurfaceStress
                        | CanonicalField::NorthwardSurfaceStress
                        | CanonicalField::SensibleHeatFlux
                        | CanonicalField::LatentHeatFlux
                        | CanonicalField::FrictionVelocity
                        | CanonicalField::MoninObukhovLength
                )
            );
            if !is_surface_layer_input {
                fields.insert(key.clone(), field.clone()).unwrap();
            }
        }
        Arc::new(
            RawMetFrame::publish(frame.metadata().clone(), fields, frame.provenance().clone())
                .unwrap(),
        )
    }

    fn analytic_pressure_frame_with_missing_temperature_level(seconds: i64) -> Arc<RawMetFrame> {
        let frame = analytic_pressure_frame(seconds);
        let temperature = FieldKey::Canonical(CanonicalField::AirTemperature);
        let mut fields = RawFieldStore::new();
        for (key, field) in frame.fields().iter() {
            let field = if key == &temperature {
                let mut valid = field.validity().as_arc().to_vec();
                valid[..4].fill(false);
                RawField::new(
                    field.values().clone(),
                    Arc::from(valid),
                    field.unit().clone(),
                    field.layout(),
                    field.temporal(),
                    field.quality(),
                    field.provenance(),
                )
                .unwrap()
            } else {
                field.clone()
            };
            fields.insert(key.clone(), field).unwrap();
        }
        Arc::new(
            RawMetFrame::publish(frame.metadata().clone(), fields, frame.provenance().clone())
                .unwrap(),
        )
    }

    fn analytic_pressure_frame_with_estimated(
        seconds: i64,
        estimated: Option<CanonicalField>,
    ) -> Arc<RawMetFrame> {
        analytic_pressure_frame_with_surface_humidity(seconds, estimated, 0.005)
    }

    fn analytic_pressure_frame_with_surface_humidity(
        seconds: i64,
        estimated: Option<CanonicalField>,
        two_metre_specific_humidity: f64,
    ) -> Arc<RawMetFrame> {
        let domain = DomainId("analytic".into());
        let valid_time = Timestamp::new(seconds, 0).unwrap();
        let id = LogicalFrameId {
            domain: domain.clone(),
            valid_time,
            profile_sha256: "analytic-profile".into(),
            content_sha256: format!("analytic-{seconds}"),
        };
        let geometry = DomainGeometry {
            domain: domain.clone(),
            longitude_origin_degrees: 0.0,
            latitude_origin_degrees: 0.0,
            longitude_spacing_degrees: 1.0,
            latitude_spacing_degrees: 1.0,
            nx: 2,
            ny: 2,
            periodic_longitude: false,
            halo_cells: 0,
        };
        let full = ArrayLayout::Full3D {
            levels: 2,
            ny: 2,
            nx: 2,
        };
        let horizontal = ArrayLayout::Horizontal2D { ny: 2, nx: 2 };
        let mut fields = RawFieldStore::new();
        let mut provenance = ProvenanceTable::new();
        let time_height = 0.2 * seconds as f64;
        let height = [6_000.0, 1_000.0]
            .into_iter()
            .flat_map(|base| {
                [
                    base + time_height,
                    base + time_height + 0.01 * DX,
                    base + time_height + 0.01 * DX,
                    base + time_height + 0.02 * DX,
                ]
            })
            .collect::<Vec<_>>();
        add_field_with_quality(
            &mut fields,
            &mut provenance,
            &id,
            CanonicalField::GeometricHeight,
            height,
            full,
            if estimated == Some(CanonicalField::GeometricHeight) {
                FieldQuality::Estimated
            } else {
                FieldQuality::Source
            },
        );
        for (field, values) in [
            (CanonicalField::EastwardWind, vec![10.0; 8]),
            (CanonicalField::NorthwardWind, vec![5.0; 8]),
            (
                CanonicalField::AirTemperature,
                vec![250.0, 250.0, 250.0, 250.0, 285.0, 285.0, 285.0, 285.0],
            ),
            (
                CanonicalField::SpecificHumidity,
                vec![0.002, 0.002, 0.002, 0.002, 0.006, 0.006, 0.006, 0.006],
            ),
            (CanonicalField::PressureVerticalVelocity, vec![0.0; 8]),
        ] {
            add_field_with_quality(
                &mut fields,
                &mut provenance,
                &id,
                field,
                values,
                full,
                if estimated == Some(field) {
                    FieldQuality::Estimated
                } else {
                    FieldQuality::Source
                },
            );
        }
        let terrain = vec![
            100.0,
            100.0 + 0.005 * DX,
            100.0 + 0.002 * DX,
            100.0 + 0.007 * DX,
        ];
        for (field, values) in [
            (CanonicalField::GeometricTerrainHeight, terrain),
            (CanonicalField::SurfacePressure, vec![100_000.0; 4]),
            (CanonicalField::TenMetreEastwardWind, vec![4.0; 4]),
            (CanonicalField::TenMetreNorthwardWind, vec![1.0; 4]),
            (CanonicalField::TwoMetreAirTemperature, vec![290.0; 4]),
            (
                CanonicalField::TwoMetreSpecificHumidity,
                vec![two_metre_specific_humidity; 4],
            ),
            (CanonicalField::AerodynamicRoughnessLength, vec![0.1; 4]),
            (CanonicalField::BoundaryLayerHeight, vec![1_000.0; 4]),
            (CanonicalField::EastwardSurfaceStress, vec![0.12; 4]),
            (CanonicalField::NorthwardSurfaceStress, vec![0.0; 4]),
            (CanonicalField::SensibleHeatFlux, vec![100.0; 4]),
            (CanonicalField::LatentHeatFlux, vec![50.0; 4]),
        ] {
            add_field_with_quality(
                &mut fields,
                &mut provenance,
                &id,
                field,
                values,
                horizontal,
                if estimated == Some(field) {
                    FieldQuality::Estimated
                } else {
                    FieldQuality::Source
                },
            );
        }
        Arc::new(
            RawMetFrame::publish(
                FrameMetadata {
                    id,
                    domain,
                    valid_time,
                    grid: geometry,
                    vertical: VerticalTopology::PressureLevels(
                        PressureLevels::new(Arc::from([50_000.0, 90_000.0])).unwrap(),
                    ),
                },
                fields,
                Arc::new(provenance),
            )
            .unwrap(),
        )
    }

    fn analytic_hybrid_frame(seconds: i64, use_eta_dot: bool) -> Arc<RawMetFrame> {
        let domain = DomainId("analytic-hybrid".into());
        let valid_time = Timestamp::new(seconds, 0).unwrap();
        let id = LogicalFrameId {
            domain: domain.clone(),
            valid_time,
            profile_sha256: "analytic-hybrid-profile".into(),
            content_sha256: format!("analytic-hybrid-{seconds}-{use_eta_dot}"),
        };
        let geometry = DomainGeometry {
            domain: domain.clone(),
            longitude_origin_degrees: 0.0,
            latitude_origin_degrees: 0.0,
            longitude_spacing_degrees: 1.0,
            latitude_spacing_degrees: 1.0,
            nx: 2,
            ny: 2,
            periodic_longitude: false,
            halo_cells: 0,
        };
        let full = ArrayLayout::Full3D {
            levels: 2,
            ny: 2,
            nx: 2,
        };
        let horizontal = ArrayLayout::Horizontal2D { ny: 2, nx: 2 };
        let mut fields = RawFieldStore::new();
        let mut provenance = ProvenanceTable::new();
        for (field, values) in [
            (CanonicalField::EastwardWind, vec![0.0; 8]),
            (CanonicalField::NorthwardWind, vec![0.0; 8]),
            (CanonicalField::AirTemperature, vec![280.0; 8]),
            (CanonicalField::SpecificHumidity, vec![0.0; 8]),
            (
                if use_eta_dot {
                    CanonicalField::HybridVerticalVelocity
                } else {
                    CanonicalField::PressureVerticalVelocity
                },
                vec![if use_eta_dot { 0.001 } else { 100.0 }; 8],
            ),
        ] {
            add_field(&mut fields, &mut provenance, &id, field, values, full);
        }
        for (field, values) in [
            (CanonicalField::SurfacePressure, vec![100_000.0; 4]),
            (CanonicalField::SurfaceGeopotential, vec![0.0; 4]),
            (CanonicalField::AerodynamicRoughnessLength, vec![0.1; 4]),
            (CanonicalField::TenMetreEastwardWind, vec![4.0; 4]),
            (CanonicalField::TenMetreNorthwardWind, vec![1.0; 4]),
            (CanonicalField::TwoMetreAirTemperature, vec![290.0; 4]),
            (CanonicalField::TwoMetreSpecificHumidity, vec![0.005; 4]),
            (CanonicalField::BoundaryLayerHeight, vec![1_000.0; 4]),
            (CanonicalField::EastwardSurfaceStress, vec![0.12; 4]),
            (CanonicalField::NorthwardSurfaceStress, vec![0.0; 4]),
            (CanonicalField::SensibleHeatFlux, vec![100.0; 4]),
            (CanonicalField::LatentHeatFlux, vec![50.0; 4]),
        ] {
            add_field(&mut fields, &mut provenance, &id, field, values, horizontal);
        }
        Arc::new(
            RawMetFrame::publish(
                FrameMetadata {
                    id,
                    domain,
                    valid_time,
                    grid: geometry,
                    vertical: VerticalTopology::HybridPressure(HybridPressureTopology {
                        coefficients: HybridCoefficients {
                            a_half_pa: Arc::from([0.0, 0.0, 0.0]),
                            b_half: Arc::from([0.2, 0.6, 1.0]),
                        },
                        active_full_levels: Arc::from([1_u16, 2_u16]),
                    }),
                },
                fields,
                Arc::new(provenance),
            )
            .unwrap(),
        )
    }

    fn transport_plan() -> TransportPlan {
        transport_plan_with_options(false, ExplainMode::Disabled)
    }

    fn transport_plan_with_estimated(allow_estimated: bool) -> TransportPlan {
        transport_plan_with_options(allow_estimated, ExplainMode::Disabled)
    }

    fn transport_plan_with_options(allow_estimated: bool, explain: ExplainMode) -> TransportPlan {
        let mut registry = FieldRegistry::new();
        for canonical in [
            CanonicalField::EastwardWind,
            CanonicalField::NorthwardWind,
            CanonicalField::GeometricVerticalVelocity,
            CanonicalField::AirPressure,
            CanonicalField::AirTemperature,
            CanonicalField::SpecificHumidity,
            CanonicalField::AirDensity,
            CanonicalField::GeometricTerrainHeight,
        ] {
            let semantics = canonical.semantics();
            registry
                .register(crate::field::FieldDescriptor {
                    key: FieldKey::Canonical(canonical),
                    unit: Unit::new(semantics.unit, semantics.dimension, 1.0, 0.0).unwrap(),
                    shape: semantics.shape,
                    vertical_stagger: semantics.vertical_stagger,
                    quality: FieldQuality::Derived,
                    required_capability: semantics.required_capability,
                    interpolation: semantics.interpolation,
                })
                .unwrap();
        }
        let mut surface_layers = SurfaceLayerRegistry::new();
        surface_layers
            .register(Arc::new(MoninObukhovBusingerDyer::default()))
            .unwrap();
        QueryPlanBuilder::new(
            &registry,
            crate::field::CapabilitySet::new()
                .with(crate::field::Capability::Transport)
                .with(crate::field::Capability::NearSurfaceTransport),
            &surface_layers,
            &ExecutionPlan::default(),
        )
        .build_transport(TransportPlanRequest {
            allow_estimated,
            surface_layer_model: ModelId(MoninObukhovBusingerDyer::MODEL_ID.into()),
            explain,
        })
        .unwrap()
    }

    fn generic_plan(fields: Vec<CanonicalField>, with_surface_layer: bool) -> QueryPlan {
        generic_plan_with_explain(fields, with_surface_layer, ExplainMode::Disabled)
    }

    fn generic_plan_with_explain(
        fields: Vec<CanonicalField>,
        with_surface_layer: bool,
        explain: ExplainMode,
    ) -> QueryPlan {
        let mut registry = FieldRegistry::new();
        for canonical in &fields {
            let semantics = canonical.semantics();
            registry
                .register(crate::field::FieldDescriptor {
                    key: FieldKey::Canonical(*canonical),
                    unit: Unit::new(semantics.unit, semantics.dimension, 1.0, 0.0).unwrap(),
                    shape: semantics.shape,
                    vertical_stagger: semantics.vertical_stagger,
                    quality: FieldQuality::Derived,
                    required_capability: semantics.required_capability,
                    interpolation: semantics.interpolation,
                })
                .unwrap();
        }
        let mut surface_layers = SurfaceLayerRegistry::new();
        surface_layers
            .register(Arc::new(MoninObukhovBusingerDyer::default()))
            .unwrap();
        QueryPlanBuilder::new(
            &registry,
            crate::field::CapabilitySet::new()
                .with(crate::field::Capability::Transport)
                .with(crate::field::Capability::NearSurfaceTransport),
            &surface_layers,
            &ExecutionPlan::default(),
        )
        .build(QueryPlanRequest {
            fields: fields.into_iter().map(FieldKey::Canonical).collect(),
            allow_estimated: false,
            surface_layer_model: with_surface_layer
                .then(|| ModelId(MoninObukhovBusingerDyer::MODEL_ID.into())),
            explain,
        })
        .unwrap()
    }

    fn attached_exact_window() -> (PreparedWindow, Arc<Mutex<ColumnCache>>) {
        let mut window = PreparedWindow::at_frame(
            Some(analytic_pressure_frame(0)),
            analytic_pressure_frame(3_600),
            Some(analytic_pressure_frame(7_200)),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        (window, cache)
    }

    fn attached_exact_window_with_transport_cache() -> (
        PreparedWindow,
        Arc<Mutex<ColumnCache>>,
        LastTransportQueryCache,
    ) {
        let (mut window, column_cache) = attached_exact_window();
        let transport_cache = Arc::new(Mutex::new(None));
        window.attach_transport_cache(&transport_cache);
        (window, column_cache, transport_cache)
    }

    fn pressure_batch(longitude_degrees: Vec<f64>) -> QueryBatch {
        let len = longitude_degrees.len();
        QueryBatch {
            vertical_coordinate: VerticalQuery::Pressure,
            points: crate::query::request::QueryPointArrays {
                longitude_degrees,
                latitude_degrees: (0..len).map(|index| 0.2 + index as f64 * 0.2).collect(),
                vertical: vec![70_000.0; len],
            },
        }
    }

    struct InstalledQueryCounters;

    impl InstalledQueryCounters {
        fn install() -> (Arc<QueryCallCounters>, Self) {
            clear_query_counters();
            let counters = QueryCallCounters::new();
            install_query_counters(Arc::clone(&counters));
            (counters, Self)
        }
    }

    impl Drop for InstalledQueryCounters {
        fn drop(&mut self) {
            clear_query_counters();
        }
    }

    fn engine_with_pressure_frames() -> MetEngine {
        let domain = DomainId("analytic".into());
        let mut catalog = MetCatalog::default();
        catalog.domains.insert(
            domain.clone(),
            DomainCatalog {
                domain: Some(domain),
                frames: BTreeMap::new(),
                coverage: Default::default(),
            },
        );
        let mut engine = MetEngine::new(MetEngineConfig {
            catalog,
            profiles: ProfileCatalog::default(),
            fields: FieldRegistry::new(),
            surface_layers: SurfaceLayerRegistry::new(),
            memory_budget: MemoryBudget::new(16 * 1024 * 1024, 8 * 1024 * 1024).unwrap(),
        });
        for seconds in [0, 3_600, 7_200] {
            engine
                .cache_frame(analytic_pressure_frame(seconds))
                .unwrap();
        }
        engine
    }

    fn attached_exact_window_without_surface_layer_inputs()
    -> (PreparedWindow, Arc<Mutex<ColumnCache>>) {
        let mut window = PreparedWindow::at_frame(
            Some(analytic_pressure_frame_without_surface_layer_inputs(0)),
            analytic_pressure_frame_without_surface_layer_inputs(3_600),
            Some(analytic_pressure_frame_without_surface_layer_inputs(7_200)),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        (window, cache)
    }

    fn assert_boundary_geometry_matches_transport(
        window: &PreparedWindow,
        plan: &TransportPlan,
        batch: QueryBatch,
    ) {
        let context = RayonExecutionContext { worker_threads: 1 };
        let mut workspace = BatchWorkspace::default();
        let boundary = window
            .query_boundary_batch(plan, batch.clone(), &context, &mut workspace)
            .unwrap();
        let transport = window
            .prepare_transport_batch(plan, batch, &mut workspace)
            .unwrap()
            .execute(&context, &mut workspace)
            .unwrap();
        assert_eq!(boundary.status(), transport.status());
        assert_eq!(boundary.bounds(), transport.bounds());
        for index in 0..transport.status().len() {
            let boundary_row = boundary.row(index).unwrap();
            let transport_row = transport.row(index).unwrap();
            assert_eq!(
                boundary_row.terrain_height_asl_m(),
                transport_row.terrain_height_asl_m(),
                "terrain mismatch at point {index} with status {:?}",
                transport_row.status()
            );
        }
    }

    #[test]
    fn pressure_transport_executes_full_kinematic_w_and_preserves_order() {
        let (window, cache) = attached_exact_window();
        let plan = transport_plan();
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::Pressure,
            points: crate::query::request::QueryPointArrays {
                longitude_degrees: vec![0.75, 0.25, 0.0],
                latitude_degrees: vec![0.75, 0.25, 90.0],
                vertical: vec![70_000.0; 3],
            },
        };
        let mut workspace = BatchWorkspace::default();
        let prepared = window
            .prepare_transport_batch(&plan, batch.clone(), &mut workspace)
            .unwrap();
        let metrics_before = cache.lock().unwrap().metrics();
        let output = prepared
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert!(output.explain().is_none());
        assert_eq!(cache.lock().unwrap().metrics(), metrics_before);
        assert_eq!(output.status().values()[2], SampleStatus::PolarSingularity);
        assert!(output.row(2).unwrap().air_pressure_pa().is_none());
        for index in 0..2 {
            let row = output.row(index).unwrap();
            assert_eq!(row.status(), SampleStatus::Ok);
            assert_eq!(row.air_pressure_pa(), Some(70_000.0));
            let u = row.eastward_wind_m_s().unwrap();
            let v = row.northward_wind_m_s().unwrap();
            let expected =
                0.2 + u * 0.01 / batch.points.latitude_degrees[index].to_radians().cos() + v * 0.01;
            assert!((row.geometric_vertical_velocity_m_s().unwrap() - expected).abs() < 1.0e-10);
        }
        assert_ne!(
            output.bounds().get(0).unwrap().terrain_asl_m(),
            output.bounds().get(1).unwrap().terrain_asl_m()
        );
        let w_id = output
            .columns()
            .geometric_vertical_velocity_m_s
            .provenance()[0];
        let w_record = output.provenance().get(w_id).unwrap();
        assert_eq!(
            w_record.field,
            FieldKey::Canonical(CanonicalField::GeometricVerticalVelocity)
        );
        for source in ["analytic-0", "analytic-3600", "analytic-7200"] {
            assert!(w_record.sources.iter().any(|value| value == source));
        }
        assert_eq!(
            prepared.execute(&RayonExecutionContext { worker_threads: 0 }, &mut workspace,),
            Err(EngineError::InvalidExecutionContext)
        );

        let prepared_again = window
            .prepare_transport_batch(&plan, batch, &mut workspace)
            .unwrap();
        assert!(cache.lock().unwrap().metrics().hits > metrics_before.hits);
        let replay = prepared_again
            .execute(&RayonExecutionContext { worker_threads: 4 }, &mut workspace)
            .unwrap();
        assert_eq!(output, replay);
    }

    #[test]
    fn mesoscale_statistics_keep_calm_uniform_support_exactly_zero() {
        let mut window = PreparedWindow::at_frame(
            Some(flat_calm_pressure_frame(0)),
            flat_calm_pressure_frame(3_600),
            Some(flat_calm_pressure_frame(7_200)),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::Pressure,
            points: QueryPointArrays {
                longitude_degrees: vec![0.5, 0.5],
                latitude_degrees: vec![0.5, 90.0],
                vertical: vec![70_000.0; 2],
            },
        };
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_mesoscale_batch(batch, &mut workspace)
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
            .unwrap();
        assert_eq!(output.native_interval_seconds(), 3_600.0);
        assert_eq!(output.variance_m2_s2(0), Some([0.0; 3]));
        assert_eq!(output.status().get(1), Some(SampleStatus::PolarSingularity));
        assert_eq!(output.variance_m2_s2(1), None);
    }

    #[test]
    fn pressure_stability_matches_the_manufactured_dry_column() {
        let (window, _cache) = attached_exact_window();
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_stability_batch(
                QueryBatch {
                    vertical_coordinate: VerticalQuery::Pressure,
                    points: QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![70_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let theta_top = potential_temperature_k(250.0, 50_000.0).unwrap();
        let theta_bottom = potential_temperature_k(285.0, 90_000.0).unwrap();
        let expected = M3_CONSTANTS.standard_gravity_m_s2 / (0.5 * (theta_top + theta_bottom))
            * ((theta_bottom - theta_top) / (1_000.0 - 6_000.0));
        let actual = output.brunt_vaisala_frequency_squared_s2(0).unwrap();
        assert!((actual - expected).abs() <= 1.0e-12 * expected.abs().max(1.0));
    }

    #[test]
    fn hybrid_stability_is_finite_and_worker_deterministic() {
        let mut window = PreparedWindow::between(
            analytic_hybrid_frame(0, true),
            analytic_hybrid_frame(3_600, true),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::Pressure,
            points: QueryPointArrays {
                longitude_degrees: vec![0.25, 0.75],
                latitude_degrees: vec![0.25, 0.75],
                vertical: vec![50_000.0, 70_000.0],
            },
        };
        let mut workspace = BatchWorkspace::default();
        let prepared = window
            .prepare_stability_batch(batch, &mut workspace)
            .unwrap();
        let serial = prepared
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let parallel = prepared
            .execute(&RayonExecutionContext { worker_threads: 4 }, &mut workspace)
            .unwrap();
        assert_eq!(parallel, serial);
        for index in 0..2 {
            assert!(
                serial
                    .brunt_vaisala_frequency_squared_s2(index)
                    .is_some_and(f64::is_finite)
            );
        }
    }

    #[test]
    fn convection_columns_preserve_caller_order_and_execute_without_cache_mutation() {
        let (window, cache) = attached_exact_window();
        let point_count = PARALLEL_TRANSPORT_MIN_POINTS * 2;
        let mut longitude_degrees = (0..point_count)
            .map(|index| 0.05 + (index % 89) as f64 * 0.01)
            .collect::<Vec<_>>();
        let mut latitude_degrees = (0..point_count)
            .map(|index| 0.05 + (index % 83) as f64 * 0.01)
            .collect::<Vec<_>>();
        longitude_degrees[point_count - 1] = 0.0;
        latitude_degrees[point_count - 1] = 90.0;
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveSeaLevel,
            points: QueryPointArrays {
                longitude_degrees,
                latitude_degrees,
                vertical: vec![1_000.0; point_count],
            },
        };
        let mut workspace = BatchWorkspace::default();
        let prepared = window
            .prepare_convection_batch(batch, &mut workspace)
            .unwrap();
        let metrics = cache.lock().unwrap().metrics();
        let serial = prepared
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(cache.lock().unwrap().metrics(), metrics);
        let parallel = prepared
            .execute(&RayonExecutionContext { worker_threads: 4 }, &mut workspace)
            .unwrap();
        assert_eq!(parallel, serial);
        assert_eq!(cache.lock().unwrap().metrics(), metrics);
        assert_eq!(
            serial.status().get(point_count - 1),
            Some(SampleStatus::PolarSingularity)
        );
        assert!(serial.column(point_count - 1).is_none());
        for index in 0..(point_count - 1) {
            let column = serial.column(index).unwrap();
            let first = column.available_top_index().unwrap();
            let last = column.lowest_valid_index().unwrap();
            assert!(last > first);
            assert!(
                column.validity().valid[first..=last]
                    .iter()
                    .all(|valid| *valid)
            );
            assert!(
                column.specific_humidity()[first..=last]
                    .iter()
                    .all(|value| value.is_finite() && *value >= 0.0)
            );
        }
    }

    #[test]
    fn convection_column_reports_a_missing_thermodynamic_layer_as_typed_status() {
        let mut window = PreparedWindow::at_frame(
            Some(analytic_pressure_frame_with_missing_temperature_level(0)),
            analytic_pressure_frame_with_missing_temperature_level(3_600),
            Some(analytic_pressure_frame_with_missing_temperature_level(
                7_200,
            )),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_convection_batch(
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveSeaLevel,
                    points: QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![1_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
            .unwrap();
        assert_eq!(
            output.status().values(),
            &[SampleStatus::InvalidVerticalColumn]
        );
        assert!(output.column(0).is_none());
    }

    #[test]
    fn stability_does_not_fill_a_missing_thermodynamic_layer() {
        let mut window = PreparedWindow::at_frame(
            Some(analytic_pressure_frame_with_missing_temperature_level(0)),
            analytic_pressure_frame_with_missing_temperature_level(3_600),
            Some(analytic_pressure_frame_with_missing_temperature_level(
                7_200,
            )),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_stability_batch(
                QueryBatch {
                    vertical_coordinate: VerticalQuery::Pressure,
                    points: QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![70_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(output.status().values(), &[SampleStatus::AboveAvailableTop]);
        assert_eq!(output.brunt_vaisala_frequency_squared_s2(0), None);
    }

    #[test]
    fn parallel_transport_matches_serial_output_exactly() {
        let (window, _cache) = attached_exact_window();
        let plan = transport_plan();
        let point_count = PARALLEL_TRANSPORT_MIN_POINTS * 4;
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::Pressure,
            points: QueryPointArrays {
                longitude_degrees: (0..point_count)
                    .map(|index| 0.05 + (index % 89) as f64 * 0.01)
                    .collect(),
                latitude_degrees: (0..point_count)
                    .map(|index| 0.05 + (index % 83) as f64 * 0.01)
                    .collect(),
                vertical: (0..point_count)
                    .map(|index| 60_000.0 + (index % 101) as f64 * 250.0)
                    .collect(),
            },
        };
        let mut workspace = BatchWorkspace::default();
        let serial = window
            .prepare_transport_batch(&plan, batch.clone(), &mut workspace)
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let parallel = window
            .prepare_transport_batch(&plan, batch, &mut workspace)
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 4 }, &mut workspace)
            .unwrap();

        assert_eq!(parallel, serial);
    }

    #[test]
    fn boundary_geometry_matches_complete_transport_for_pressure_and_hybrid_windows() {
        let plan = transport_plan();
        let (exact_pressure, _cache) = attached_exact_window();
        assert_boundary_geometry_matches_transport(
            &exact_pressure,
            &plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points: QueryPointArrays {
                    longitude_degrees: vec![0.5, 0.5, 0.5, 2.0, 0.5],
                    latitude_degrees: vec![0.5, 0.5, 0.5, 0.5, 90.0],
                    vertical: vec![5_000.0, 500.0, 400.0, 5_000.0, 5_000.0],
                },
            },
        );

        let mut between_pressure = PreparedWindow::between(
            analytic_pressure_frame(0),
            analytic_pressure_frame(3_600),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let pressure_cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        between_pressure.attach_column_cache(&pressure_cache);
        assert_boundary_geometry_matches_transport(
            &between_pressure,
            &plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points: QueryPointArrays {
                    longitude_degrees: vec![0.5, 0.5, 0.5],
                    latitude_degrees: vec![0.5; 3],
                    vertical: vec![5_000.0, 500.0, 10_000.0],
                },
            },
        );

        let mut between_hybrid = PreparedWindow::between(
            analytic_hybrid_frame(0, true),
            analytic_hybrid_frame(3_600, true),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let hybrid_cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        between_hybrid.attach_column_cache(&hybrid_cache);
        assert_boundary_geometry_matches_transport(
            &between_hybrid,
            &plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points: QueryPointArrays {
                    longitude_degrees: vec![0.5, 0.5, 0.5],
                    latitude_degrees: vec![0.5; 3],
                    vertical: vec![5_000.0, 10.0, 100_000.0],
                },
            },
        );
    }

    #[test]
    fn boundary_finalize_retains_prior_stencils_within_execution_budget() {
        let (mut window, cache) = attached_exact_window();
        let frame = window.frames.before.clone();
        let capabilities = transport_plan().query_plan().capabilities();
        let first_key = stencil_cache_key(&frame, CellId(0), capabilities);
        let second_key = stencil_cache_key(&frame, CellId(1), capabilities);
        let first_value = Arc::new(ColumnStencil::synthetic_for_cache(0.0));
        let second_value = Arc::new(ColumnStencil::synthetic_for_cache(1.0));
        let stencil_bytes = first_value.resident_bytes();
        assert_eq!(second_value.resident_bytes(), stencil_bytes);

        let (first_pin, second_pin) = {
            let mut cache = cache.lock().unwrap();
            (
                cache.insert(first_key.clone(), first_value).unwrap(),
                cache.insert(second_key.clone(), second_value).unwrap(),
            )
        };
        drop(second_pin);

        let stencils = PreparedStencils {
            frames: vec![PreparedFrameStencils {
                frame,
                by_cell: BTreeMap::from([(CellId(0), first_pin)]),
            }],
            resident_bytes: stencil_bytes,
        };
        let mut workspace = BatchWorkspace::default();
        let scratch_bytes = 4_096;

        finalize_boundary_stencil_session(&window, &mut workspace, &stencils, scratch_bytes)
            .unwrap();
        assert_eq!(cache.lock().unwrap().len(), 2);
        assert_eq!(cache.lock().unwrap().metrics().evictions, 0);

        window.execution_budget_bytes = stencil_bytes + scratch_bytes;
        finalize_boundary_stencil_session(&window, &mut workspace, &stencils, scratch_bytes)
            .unwrap();
        let mut cache = cache.lock().unwrap();
        assert_eq!(cache.len(), 1);
        assert!(cache.get(&first_key).is_some());
        assert!(cache.get(&second_key).is_none());
        assert_eq!(cache.metrics().resident_bytes, stencil_bytes);
        drop(cache);

        window.execution_budget_bytes = stencil_bytes + scratch_bytes - 1;
        assert!(matches!(
            finalize_boundary_stencil_session(
                &window,
                &mut workspace,
                &stencils,
                scratch_bytes
            ),
            Err(EngineError::Layout(LayoutError::InsufficientMemory {
                required_bytes,
                budget_bytes,
            })) if required_bytes == stencil_bytes + scratch_bytes
                && budget_bytes == window.execution_budget_bytes
        ));
    }

    #[test]
    fn exact_transport_cache_reuses_full_batch_and_ordered_subset_exactly() {
        let (window, _column_cache, transport_cache) = attached_exact_window_with_transport_cache();
        let plan = transport_plan_with_options(false, ExplainMode::Full);
        let full_batch = pressure_batch(vec![0.1, 0.3, 0.5]);
        let (counters, _installed) = InstalledQueryCounters::install();
        let context = RayonExecutionContext { worker_threads: 1 };
        let mut workspace = BatchWorkspace::default();

        let full = {
            let _origin = QueryOriginScope::enter(QueryOrigin::Output);
            window
                .prepare_transport_batch(&plan, full_batch.clone(), &mut workspace)
                .unwrap()
                .execute(&context, &mut workspace)
                .unwrap()
        };
        assert!(transport_cache.lock().unwrap().is_some());

        let replay = {
            let _origin = QueryOriginScope::enter(QueryOrigin::Integrator);
            window
                .prepare_transport_batch(&plan, full_batch.clone(), &mut workspace)
                .unwrap()
                .execute(&context, &mut workspace)
                .unwrap()
        };
        assert_eq!(replay, full);

        let subset_batch = QueryBatch {
            vertical_coordinate: full_batch.vertical_coordinate,
            points: crate::query::request::QueryPointArrays {
                longitude_degrees: vec![
                    full_batch.points.longitude_degrees[0],
                    full_batch.points.longitude_degrees[2],
                ],
                latitude_degrees: vec![
                    full_batch.points.latitude_degrees[0],
                    full_batch.points.latitude_degrees[2],
                ],
                vertical: vec![full_batch.points.vertical[0], full_batch.points.vertical[2]],
            },
        };
        let subset = {
            let _origin = QueryOriginScope::enter(QueryOrigin::Integrator);
            window
                .prepare_transport_batch(&plan, subset_batch, &mut workspace)
                .unwrap()
                .execute(&context, &mut workspace)
                .unwrap()
        };
        assert_eq!(subset.as_ref(), &full.select_rows(&[0, 2]).unwrap());

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.logical_requests, 3);
        assert_eq!(snapshot.executed_batches, 1);
        assert_eq!(snapshot.exact_key_reuses, 2);
        assert_eq!(snapshot.particle_loop_exact.unique_exact_keys, 1);
        assert_eq!(snapshot.particle_loop_exact.repeated_exact_executions, 0);
    }

    #[test]
    fn lifecycle_query_does_not_displace_bulk_transport_cache() {
        let (window, _column_cache, _transport_cache) =
            attached_exact_window_with_transport_cache();
        let plan = transport_plan();
        let full_batch = pressure_batch(vec![0.1, 0.3, 0.5]);
        let lifecycle_batch = pressure_batch(vec![0.9]);
        let (counters, _installed) = InstalledQueryCounters::install();
        let context = RayonExecutionContext { worker_threads: 1 };
        let mut workspace = BatchWorkspace::default();

        {
            let _origin = QueryOriginScope::enter(QueryOrigin::Output);
            window
                .prepare_transport_batch(&plan, full_batch.clone(), &mut workspace)
                .unwrap()
                .execute(&context, &mut workspace)
                .unwrap();
        }
        {
            let _origin = QueryOriginScope::enter(QueryOrigin::OutputLifecycle);
            window
                .prepare_transport_batch(&plan, lifecycle_batch, &mut workspace)
                .unwrap()
                .execute(&context, &mut workspace)
                .unwrap();
        }
        {
            let _origin = QueryOriginScope::enter(QueryOrigin::Integrator);
            window
                .prepare_transport_batch(&plan, full_batch, &mut workspace)
                .unwrap()
                .execute(&context, &mut workspace)
                .unwrap();
        }

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.logical_requests, 3);
        assert_eq!(snapshot.executed_batches, 2);
        assert_eq!(snapshot.exact_key_reuses, 1);
        assert_eq!(
            snapshot.by_origin[&QueryOrigin::OutputLifecycle].executed_batches,
            1
        );
        assert_eq!(
            snapshot.by_origin[&QueryOrigin::Integrator].exact_key_reuses,
            1
        );
        assert_eq!(snapshot.particle_loop_exact.unique_exact_keys, 1);
        assert_eq!(snapshot.particle_loop_exact.repeated_exact_executions, 0);
    }

    #[test]
    fn exact_transport_cache_keeps_signed_zero_distinct() {
        let (window, _column_cache, _transport_cache) =
            attached_exact_window_with_transport_cache();
        let plan = transport_plan();
        let (counters, _installed) = InstalledQueryCounters::install();
        let context = RayonExecutionContext { worker_threads: 1 };
        let mut workspace = BatchWorkspace::default();

        for (origin, longitude) in [
            (QueryOrigin::Output, 0.0_f64),
            (QueryOrigin::Integrator, -0.0_f64),
        ] {
            let _origin = QueryOriginScope::enter(origin);
            window
                .prepare_transport_batch(&plan, pressure_batch(vec![longitude]), &mut workspace)
                .unwrap()
                .execute(&context, &mut workspace)
                .unwrap();
        }

        let snapshot = counters.snapshot();
        assert_eq!(snapshot.logical_requests, 2);
        assert_eq!(snapshot.executed_batches, 2);
        assert_eq!(snapshot.exact_key_reuses, 0);
        assert_eq!(snapshot.particle_loop_exact.unique_exact_keys, 2);
    }

    #[test]
    fn frame_publish_invalidates_exact_transport_cache() {
        let mut engine = engine_with_pressure_frames();
        let domain = DomainId("analytic".into());
        let window = engine
            .prepare_for_domain(Timestamp::new(3_600, 0).unwrap(), &domain)
            .unwrap();
        let mut workspace = BatchWorkspace::default();
        window
            .prepare_transport_batch(
                &transport_plan(),
                pressure_batch(vec![0.25]),
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert!(engine.last_transport_query.lock().unwrap().is_some());

        engine.cache_frame(analytic_pressure_frame(10_800)).unwrap();
        assert!(engine.last_transport_query.lock().unwrap().is_none());
    }

    #[test]
    fn exact_transport_cache_key_includes_pinned_window_identity() {
        let (window, _column_cache) = attached_exact_window();
        let plan = transport_plan();
        let batch = pressure_batch(vec![0.25]);
        let original = ExactTransportCacheKey::new(
            ExactTransportWindowKey {
                query_time: window.query_time,
                domain: window.frames.before.metadata().domain.clone(),
                before_frame: window.frames.before.metadata().id.clone(),
                after_frame: window.frames.after.metadata().id.clone(),
                previous_frame: window
                    .previous
                    .as_ref()
                    .map(|frame| frame.metadata().id.clone()),
                next_frame: window
                    .next
                    .as_ref()
                    .map(|frame| frame.metadata().id.clone()),
                before_weight_bits: window.before_weight.to_bits(),
                after_weight_bits: window.after_weight.to_bits(),
            },
            plan.clone(),
            &batch,
        );
        let mut replaced_current = window.frames.before.metadata().id.clone();
        replaced_current.content_sha256.push_str("-replacement");
        let changed = ExactTransportCacheKey::new(
            ExactTransportWindowKey {
                query_time: window.query_time,
                domain: window.frames.before.metadata().domain.clone(),
                before_frame: replaced_current,
                after_frame: window.frames.after.metadata().id.clone(),
                previous_frame: window
                    .previous
                    .as_ref()
                    .map(|frame| frame.metadata().id.clone()),
                next_frame: window
                    .next
                    .as_ref()
                    .map(|frame| frame.metadata().id.clone()),
                before_weight_bits: window.before_weight.to_bits(),
                after_weight_bits: window.after_weight.to_bits(),
            },
            plan,
            &batch,
        );
        assert_eq!(changed.selection_from_cached(&original), None);
    }

    #[test]
    fn full_explain_is_opt_in_and_carries_interpolation_contract() {
        let (window, _cache) = attached_exact_window();
        let plan = transport_plan_with_options(false, ExplainMode::Full);
        let mut workspace = BatchWorkspace::default();
        let pressure = window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::Pressure,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.25],
                        latitude_degrees: vec![0.25],
                        vertical: vec![70_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let record = pressure.explain().unwrap()[0].as_ref().unwrap();
        assert_eq!(record.vertical.path, ExplainVerticalPath::LogPressure);
        assert!(record.vertical.first_level.is_some());
        assert!(record.vertical.second_level.is_some());
        assert!(record.surface_model.is_none());
        assert_eq!(record.fields.len(), TRANSPORT_FIELD_COUNT);
        assert!((record.horizontal.first_level_weights.iter().sum::<f64>() - 1.0).abs() < 1.0e-12);
        for field in &record.fields {
            let provenance = pressure.provenance().get(field.provenance).unwrap();
            assert_eq!(provenance.field, field.field);
            assert_eq!(provenance.quality, field.quality);
        }

        let surface = window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![10.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let record = surface.explain().unwrap()[0].as_ref().unwrap();
        assert_eq!(record.vertical.path, ExplainVerticalPath::SurfaceLayer);
        assert_eq!(
            record.surface_model.as_ref().unwrap().0,
            MoninObukhovBusingerDyer::MODEL_ID
        );
        assert_eq!(
            record.horizontal.first_level_method,
            ExplainHorizontalMethod::Bilinear
        );
    }

    #[test]
    fn surface_layer_and_between_frame_queries_are_complete() {
        let (window, _cache) = attached_exact_window();
        let plan = transport_plan();
        let mut workspace = BatchWorkspace::default();
        let near = window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![10.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
            .unwrap();
        let row = near.row(0).unwrap();
        assert_eq!(row.status(), SampleStatus::Ok);
        assert!((row.eastward_wind_m_s().unwrap() - 4.0).abs() < 1.0e-3);
        assert!(row.geometric_vertical_velocity_m_s().unwrap().is_finite());
        let near_q = near
            .provenance()
            .get(near.columns().specific_humidity.provenance()[0])
            .unwrap();
        assert!(near_q.transforms.iter().any(|transform| {
            transform.operation == SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID
        }));
        assert!(near_q.transforms.iter().any(|transform| {
            transform.operation == AERODYNAMIC_ROUGHNESS_ZERO_PROJECTION_ALGORITHM_ID
        }));
        assert!(near_q.transforms.iter().any(|transform| {
            transform.operation == LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID
        }));
        assert!(near_q.transforms.iter().any(|transform| {
            transform.operation == TWO_METRE_ANCHORED_SURFACE_SCALAR_ALGORITHM_ID
        }));
        let near_wind = near
            .provenance()
            .get(near.columns().eastward_wind_m_s.provenance()[0])
            .unwrap();
        assert!(near_wind.transforms.iter().any(|transform| {
            transform.operation == TEN_METRE_ANCHORED_SURFACE_WIND_ALGORITHM_ID
        }));
        assert!(near_wind.transforms.iter().any(|transform| {
            transform.operation == LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID
        }));

        let mut between = PreparedWindow::between(
            analytic_pressure_frame(0),
            analytic_pressure_frame(3_600),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        between.attach_column_cache(&cache);
        let intermediate = between
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::Pressure,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![70_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(intermediate.status().values(), &[SampleStatus::Ok]);
        assert!(
            intermediate
                .row(0)
                .unwrap()
                .geometric_vertical_velocity_m_s()
                .unwrap()
                .is_finite()
        );
    }

    #[test]
    fn exact_bottom_interval_uses_surface_when_transport_anchor_is_incomplete() {
        let mut window = PreparedWindow::at_frame(
            Some(analytic_pressure_frame_with_low_bottom(0, false)),
            analytic_pressure_frame_with_low_bottom(3_600, true),
            Some(analytic_pressure_frame_with_low_bottom(7_200, false)),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_transport_batch(
                &transport_plan_with_options(false, ExplainMode::Full),
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![200.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let row = output.row(0).unwrap();
        assert_eq!(row.status(), SampleStatus::Ok);
        assert!(row.geometric_vertical_velocity_m_s().unwrap().is_finite());
        let explain = output.explain().unwrap()[0].as_ref().unwrap();
        assert_eq!(explain.vertical.path, ExplainVerticalPath::SurfaceLayer);
        assert_eq!(
            explain.surface_model.as_ref().unwrap().0,
            MoninObukhovBusingerDyer::MODEL_ID
        );
    }

    #[test]
    fn surface_layer_projects_two_metre_humidity_per_frame_before_time_blending() {
        let before = analytic_pressure_frame_with_surface_humidity(0, None, -0.01);
        let after = analytic_pressure_frame_with_surface_humidity(3_600, None, 0.01);
        let mut window = PreparedWindow::between(
            before,
            after,
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_transport_batch(
                &transport_plan(),
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![2.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let row = output.row(0).unwrap();
        assert_eq!(row.status(), SampleStatus::Ok);
        let humidity = row.specific_humidity().unwrap();
        assert!((humidity - 0.005).abs() < 1.0e-15, "humidity={humidity}");
    }

    #[test]
    fn generic_query_derives_missing_surface_exchange_scales() {
        let plan = generic_plan(
            vec![
                CanonicalField::FrictionVelocity,
                CanonicalField::MoninObukhovLength,
                CanonicalField::AirTemperature,
                CanonicalField::AirDensity,
            ],
            true,
        );
        let mut window = PreparedWindow::between(
            analytic_pressure_frame(0),
            analytic_pressure_frame(3_600),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![100.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let row = output.row(0).unwrap();
        assert_eq!(row.status(), SampleStatus::Ok);

        let density = moist_air_density_kg_m3(100_000.0, 290.0, 0.005).unwrap();
        let expected = surface_exchange_scales(SurfaceExchangeInput {
            air_density_kg_m3: density,
            air_temperature_k: 290.0,
            specific_humidity: 0.005,
            momentum: SurfaceMomentumInput::SurfaceStress {
                eastward_pa: 0.12,
                northward_pa: 0.0,
            },
            sensible_heat_flux_w_m2: 100.0,
            latent_heat_flux_w_m2: 50.0,
        })
        .unwrap();
        let friction = FieldKey::Canonical(CanonicalField::FrictionVelocity);
        let monin_obukhov = FieldKey::Canonical(CanonicalField::MoninObukhovLength);
        assert_eq!(row.value(&friction), Some(expected.friction_velocity_m_s));
        assert_eq!(
            row.value(&monin_obukhov),
            Some(expected.monin_obukhov_length_m)
        );
        for field in [&friction, &monin_obukhov] {
            let record = row.provenance_record(field).unwrap();
            assert_eq!(record.quality, FieldQuality::Derived);
            assert!(
                record.transforms.iter().any(|transform| {
                    transform.operation == SURFACE_EXCHANGE_SCALES_ALGORITHM_ID
                })
            );
        }
    }

    #[test]
    fn hybrid_eta_dot_and_omega_follow_the_same_native_coordinate_chain() {
        let plan = transport_plan();
        let mut workspace = BatchWorkspace::default();
        let run = |use_eta_dot: bool,
                   workspace: &mut BatchWorkspace|
         -> (Arc<TransportOutput>, Arc<RawMetFrame>) {
            let before = analytic_hybrid_frame(0, use_eta_dot);
            let after = analytic_hybrid_frame(3_600, use_eta_dot);
            let mut window = PreparedWindow::between(
                before.clone(),
                after,
                Timestamp::new(1_800, 0).unwrap(),
                8 * 1024 * 1024,
            )
            .unwrap();
            let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
            window.attach_column_cache(&cache);
            let output = window
                .prepare_transport_batch(
                    &plan,
                    QueryBatch {
                        vertical_coordinate: VerticalQuery::Pressure,
                        points: crate::query::request::QueryPointArrays {
                            longitude_degrees: vec![0.5],
                            latitude_degrees: vec![0.5],
                            vertical: vec![60_000.0],
                        },
                    },
                    workspace,
                )
                .unwrap()
                .execute(&RayonExecutionContext { worker_threads: 1 }, workspace)
                .unwrap();
            (output, before)
        };
        let (eta_output, frame) = run(true, &mut workspace);
        let (omega_output, _) = run(false, &mut workspace);
        let eta_w = eta_output
            .row(0)
            .unwrap()
            .geometric_vertical_velocity_m_s()
            .unwrap();
        let omega_w = omega_output
            .row(0)
            .unwrap()
            .geometric_vertical_velocity_m_s()
            .unwrap();
        assert!((eta_w - omega_w).abs() < 1.0e-12);
        let grid = RegularLatLonGrid::new(frame.metadata().grid.clone()).unwrap();
        let column = HybridColumnBuilder
            .build(ColumnRequest {
                frame: &frame,
                cell: grid.locate_cell(0.5, 0.5).unwrap(),
                longitude_degrees: 0.5,
                latitude_degrees: 0.5,
            })
            .unwrap();
        let expected = 0.001 * (column.height_asl_m()[1] - column.height_asl_m()[0]) / 0.4;
        assert!((eta_w - expected).abs() < 1.0e-12);
        assert!(eta_w < 0.0);
    }

    #[test]
    fn generic_query_and_between_frame_provenance_are_self_contained() {
        let plan = generic_plan(
            vec![
                CanonicalField::SurfacePressure,
                CanonicalField::AirTemperature,
                CanonicalField::SpecificHumidity,
                CanonicalField::AirDensity,
            ],
            false,
        );
        let mut window = PreparedWindow::between(
            analytic_pressure_frame(0),
            analytic_pressure_frame(3_600),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::Pressure,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.75, 0.25],
                        latitude_degrees: vec![0.75, 0.25],
                        vertical: vec![70_000.0, 80_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
            .unwrap();
        assert_eq!(output.status().values(), &[SampleStatus::Ok; 2]);
        assert!(output.bounds().get(0).unwrap().minimum_transport_agl_m() > 100.0);
        let temperature = FieldKey::Canonical(CanonicalField::AirTemperature);
        assert!(output.row(0).unwrap().value(&temperature).unwrap() > 250.0);
        let record = output
            .row(0)
            .unwrap()
            .provenance_record(&temperature)
            .unwrap();
        assert!(record.sources.iter().any(|value| value == "analytic-0"));
        assert!(record.sources.iter().any(|value| value == "analytic-3600"));
        assert!(
            record
                .transforms
                .iter()
                .any(|value| value.operation == "trajecta.m3.query.upper_air")
        );
        let humidity = FieldKey::Canonical(CanonicalField::SpecificHumidity);
        let humidity_record = output.row(0).unwrap().provenance_record(&humidity).unwrap();
        assert!(!humidity_record.transforms.iter().any(|transform| {
            transform.operation == SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID
        }));
        let density = FieldKey::Canonical(CanonicalField::AirDensity);
        let density_record = output.row(0).unwrap().provenance_record(&density).unwrap();
        assert!(density_record.transforms.iter().any(|transform| {
            transform.operation == SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID
        }));
    }

    #[test]
    fn generic_surface_direct_fields_do_not_require_similarity_inputs() {
        let plan = generic_plan(
            vec![
                CanonicalField::SurfacePressure,
                CanonicalField::GeometricHeight,
                CanonicalField::AirPressure,
            ],
            false,
        );
        let (window, _cache) = attached_exact_window_without_surface_layer_inputs();
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![10.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let row = output.row(0).unwrap();
        assert_eq!(row.status(), SampleStatus::Ok);
        assert_eq!(
            row.value(&FieldKey::Canonical(CanonicalField::SurfacePressure)),
            Some(100_000.0)
        );
        let bounds = output.bounds().get(0).unwrap();
        let height = row
            .value(&FieldKey::Canonical(CanonicalField::GeometricHeight))
            .unwrap();
        assert!((height - (bounds.terrain_asl_m() + 10.0)).abs() < 1.0e-12);
        let pressure = row
            .value(&FieldKey::Canonical(CanonicalField::AirPressure))
            .unwrap();
        assert!(pressure > 90_000.0 && pressure < 100_000.0);
    }

    #[test]
    fn generic_surface_mixed_query_keeps_direct_fields_without_a_surface_model() {
        let plan = generic_plan(
            vec![
                CanonicalField::SurfacePressure,
                CanonicalField::EastwardWind,
            ],
            false,
        );
        let (window, _cache) = attached_exact_window_without_surface_layer_inputs();
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![10.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let row = output.row(0).unwrap();
        assert_eq!(row.status(), SampleStatus::SurfaceLayerUndefined);
        assert_eq!(
            row.value(&FieldKey::Canonical(CanonicalField::SurfacePressure)),
            Some(100_000.0)
        );
        assert!(
            row.value(&FieldKey::Canonical(CanonicalField::EastwardWind))
                .is_none()
        );
    }

    #[test]
    fn generic_temporal_mixed_surface_upper_matches_transport_and_explain() {
        let generic_plan = generic_plan_with_explain(
            vec![
                CanonicalField::EastwardWind,
                CanonicalField::SpecificHumidity,
                CanonicalField::AirDensity,
            ],
            true,
            ExplainMode::Full,
        );
        let transport_plan = transport_plan_with_options(false, ExplainMode::Full);
        let mut window = PreparedWindow::between(
            analytic_pressure_frame(0),
            analytic_pressure_frame(3_600),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveSeaLevel,
            points: crate::query::request::QueryPointArrays {
                longitude_degrees: vec![0.0],
                latitude_degrees: vec![0.0],
                vertical: vec![1_300.0],
            },
        };
        let mut workspace = BatchWorkspace::default();
        let generic = window
            .prepare_batch(&generic_plan, batch.clone(), &mut workspace)
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let transport = window
            .prepare_transport_batch(&transport_plan, batch, &mut workspace)
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let generic_row = generic.row(0).unwrap();
        let transport_row = transport.row(0).unwrap();
        assert_eq!(generic_row.status(), SampleStatus::Ok);
        assert_eq!(transport_row.status(), SampleStatus::Ok);
        for (field, expected) in [
            (
                CanonicalField::EastwardWind,
                transport_row.eastward_wind_m_s().unwrap(),
            ),
            (
                CanonicalField::SpecificHumidity,
                transport_row.specific_humidity().unwrap(),
            ),
            (
                CanonicalField::AirDensity,
                transport_row.air_density_kg_m3().unwrap(),
            ),
        ] {
            let actual = generic_row.value(&FieldKey::Canonical(field)).unwrap();
            assert!((actual - expected).abs() <= 1.0e-12 * expected.abs().max(1.0));
        }
        let generic_explain = generic.explain().unwrap()[0].as_ref().unwrap();
        let transport_explain = transport.explain().unwrap()[0].as_ref().unwrap();
        assert_eq!(
            generic_explain.vertical.path,
            ExplainVerticalPath::TemporalMixedSurfaceUpper
        );
        assert_eq!(
            transport_explain.vertical.path,
            ExplainVerticalPath::TemporalMixedSurfaceUpper
        );
        assert_eq!(
            generic_explain.surface_model.as_ref().unwrap().0,
            MoninObukhovBusingerDyer::MODEL_ID
        );
        let humidity = FieldKey::Canonical(CanonicalField::SpecificHumidity);
        let humidity_record = generic_row.provenance_record(&humidity).unwrap();
        assert!(humidity_record.transforms.iter().any(|transform| {
            transform.operation == "trajecta.m3.query.temporal_mixed_surface_upper"
        }));
    }

    #[test]
    fn generic_upper_query_can_return_surface_geopotential() {
        let plan = generic_plan(vec![CanonicalField::SurfaceGeopotential], false);
        let mut window = PreparedWindow::between(
            analytic_hybrid_frame(0, true),
            analytic_hybrid_frame(3_600, true),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::Pressure,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![60_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let row = output.row(0).unwrap();
        assert_eq!(row.status(), SampleStatus::Ok);
        assert_eq!(
            row.value(&FieldKey::Canonical(CanonicalField::SurfaceGeopotential)),
            Some(0.0)
        );
    }

    #[test]
    fn empty_batch_and_vertical_failure_statuses_remain_local() {
        let (window, _cache) = attached_exact_window();
        let plan = transport_plan();
        let mut workspace = BatchWorkspace::default();
        let empty = window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: crate::query::request::QueryPointArrays::default(),
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert!(empty.status().is_empty());

        let agl = window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveGround,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5, 0.5],
                        latitude_degrees: vec![0.5, 0.5],
                        vertical: vec![0.0, 10.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(agl.status().values()[0], SampleStatus::BelowGround);
        assert_eq!(agl.status().values()[1], SampleStatus::Ok);

        let pressure = window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::Pressure,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5, 0.5],
                        latitude_degrees: vec![0.5, 0.5],
                        vertical: vec![40_000.0, 110_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(
            pressure.status().values()[0],
            SampleStatus::AboveAvailableTop
        );
        assert_eq!(pressure.status().values()[1], SampleStatus::BelowGround);
        assert!(pressure.row(0).unwrap().air_pressure_pa().is_none());
        assert!(pressure.bounds().get(0).is_some());

        let mut hybrid_window = PreparedWindow::between(
            analytic_hybrid_frame(0, true),
            analytic_hybrid_frame(3_600, true),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let hybrid_cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        hybrid_window.attach_column_cache(&hybrid_cache);
        let above_model = hybrid_window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveSeaLevel,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![0.5],
                        latitude_degrees: vec![0.5],
                        vertical: vec![100_000.0],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(
            above_model.status().values(),
            &[SampleStatus::AboveModelTop]
        );
    }

    #[test]
    fn temporal_bounds_use_the_restrictive_complete_transport_top() {
        let mut window = PreparedWindow::between(
            analytic_pressure_frame(0),
            analytic_pressure_frame(3_600),
            Timestamp::new(1_800, 0).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let plan = transport_plan();
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveSeaLevel,
            points: crate::query::request::QueryPointArrays {
                longitude_degrees: vec![0.0],
                latitude_degrees: vec![0.0],
                vertical: vec![6_200.0],
            },
        };
        let mut workspace = BatchWorkspace::default();
        let transport = window
            .prepare_transport_batch(&plan, batch.clone(), &mut workspace)
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(
            transport.status().values(),
            &[SampleStatus::AboveAvailableTop]
        );
        assert_eq!(
            transport.bounds().get(0).unwrap().available_top_asl_m(),
            6_000.0
        );

        let boundary = window
            .query_boundary_batch(
                &plan,
                batch,
                &RayonExecutionContext { worker_threads: 1 },
                &mut workspace,
            )
            .unwrap();
        assert_eq!(
            boundary.status().values(),
            &[SampleStatus::AboveAvailableTop]
        );
        assert_eq!(
            boundary.bounds().get(0).unwrap().available_top_asl_m(),
            6_000.0
        );
    }

    #[test]
    fn prepared_queries_support_the_two_call_m4_rk2_shape() {
        let (start_window, _cache) = attached_exact_window();
        let plan = transport_plan();
        let mut workspace = BatchWorkspace::default();
        let start_longitude = 0.5_f64;
        let start_latitude = 0.5_f64;
        let start_height_asl_m = 5_000.0_f64;
        let start = start_window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveSeaLevel,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![start_longitude],
                        latitude_degrees: vec![start_latitude],
                        vertical: vec![start_height_asl_m],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        let row = start.row(0).unwrap();
        let half_seconds = 0.5_f64;
        let latitude_radians = start_latitude.to_radians();
        let mid_longitude = start_longitude
            + (row.eastward_wind_m_s().unwrap() * half_seconds
                / (M3_CONSTANTS.earth_radius_m * latitude_radians.cos()))
            .to_degrees();
        let mid_latitude = start_latitude
            + (row.northward_wind_m_s().unwrap() * half_seconds / M3_CONSTANTS.earth_radius_m)
                .to_degrees();
        let mid_height =
            start_height_asl_m + row.geometric_vertical_velocity_m_s().unwrap() * half_seconds;

        let mut midpoint_window = PreparedWindow::between(
            analytic_pressure_frame(3_600),
            analytic_pressure_frame(7_200),
            Timestamp::new(3_600, 500_000_000).unwrap(),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        midpoint_window.attach_column_cache(&cache);
        let midpoint = midpoint_window
            .prepare_transport_batch(
                &plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveSeaLevel,
                    points: crate::query::request::QueryPointArrays {
                        longitude_degrees: vec![mid_longitude],
                        latitude_degrees: vec![mid_latitude],
                        vertical: vec![mid_height],
                    },
                },
                &mut workspace,
            )
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(midpoint.status().values(), &[SampleStatus::Ok]);
        assert!(midpoint.row(0).unwrap().air_density_kg_m3().unwrap() > 0.0);
    }

    #[test]
    fn strict_transport_rejects_estimated_dependencies_not_just_outputs() {
        let current = analytic_pressure_frame_with_estimated(
            3_600,
            Some(CanonicalField::AerodynamicRoughnessLength),
        );
        let mut window = PreparedWindow::at_frame(
            Some(analytic_pressure_frame(0)),
            current,
            Some(analytic_pressure_frame(7_200)),
            8 * 1024 * 1024,
        )
        .unwrap();
        let cache = Arc::new(Mutex::new(ColumnCache::new(8 * 1024 * 1024)));
        window.attach_column_cache(&cache);
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveGround,
            points: crate::query::request::QueryPointArrays {
                longitude_degrees: vec![0.5],
                latitude_degrees: vec![0.5],
                vertical: vec![10.0],
            },
        };
        let mut workspace = BatchWorkspace::default();
        let strict = window
            .prepare_transport_batch(&transport_plan(), batch.clone(), &mut workspace)
            .unwrap();
        assert!(matches!(
            strict.execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace),
            Err(EngineError::QueryPlan(
                QueryPlanError::EstimatedFieldForbidden(_)
            ))
        ));

        let allowed = window
            .prepare_transport_batch(&transport_plan_with_estimated(true), batch, &mut workspace)
            .unwrap()
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .unwrap();
        assert_eq!(
            allowed.columns().eastward_wind_m_s.quality()[0],
            FieldQuality::Estimated
        );
    }
}
