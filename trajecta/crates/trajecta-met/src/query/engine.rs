//! # Contract: meteorology runtime orchestration
//!
//! `MetEngine::prepare` may select and load frames and update deterministic
//! caches. `PreparedBatch::execute` receives only pinned data and cannot invoke
//! readers or providers. Callers own the parallel execution policy.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;

use crate::derive::surface::{
    SurfaceExchangeInput, SurfaceExchangeScales, SurfaceMomentumInput, surface_exchange_scales,
};
use crate::derive::thermo::{moist_air_density_kg_m3, project_specific_humidity_nonnegative};
use crate::derive::vertical_velocity::{
    KinematicVerticalVelocityInput, geometric_vertical_velocity_m_s,
    native_coordinate_velocity_from_omega, terrain_following_surface_velocity_m_s,
};
use crate::field::{CanonicalField, FieldKey, FieldQuality, FieldRegistry};
use crate::frame::{
    ArrayLayout, FrameCache, FrameError, PreparedWindow, RawField, RawMetFrame, WindowManager,
};
use crate::grid::{
    GridBackend, GridError, GridPoint, RegularLatLonGrid, interpolate_spherical_vector,
};
use crate::io::inventory::MetCatalog;
use crate::profile::document::ProfileCatalog;
use crate::profile::graph::ExecutionPlan;
use crate::provenance::{
    ProvenanceError, ProvenanceId, ProvenanceRecord, ProvenanceTable, TransformRecord,
};
use crate::query::cache::{
    CacheError, CacheKey, CacheMetrics, ColumnCache, MemoryBudget, PinGuard, TileCache,
};
use crate::query::layout::{BatchLayout, ChunkMemoryModel, LayoutError, PointPlacement};
use crate::query::output::{
    BoundsColumn, ExplainField, ExplainHorizontalMethod, ExplainHorizontalSupport, ExplainRecord,
    ExplainVerticalPath, ExplainVerticalSupport, FieldColumn, OutputError, QueryOutput,
    SampleStatus, StatusColumn, TransportColumns, TransportOutput, ValidityMask, ValueColumn,
};
use crate::query::request::{
    ExplainMode, QueryBatch, QueryPlan, QueryPlanBuilder, QueryPlanError, QueryPlanRequest,
    TransportPlan, TransportPlanRequest, VerticalQuery,
};
use crate::science::{
    AERODYNAMIC_ROUGHNESS_ZERO_PROJECTION_ALGORITHM_ID,
    LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID, M3_MET_QUERY_ALGORITHM_ID,
    SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID,
    TEN_METRE_ANCHORED_SURFACE_WIND_ALGORITHM_ID, TWO_METRE_ANCHORED_SURFACE_SCALAR_ALGORITHM_ID,
};
use crate::surface_layer::{
    SurfaceLayerInput, SurfaceLayerRegistry, SurfaceLayerStatus, minimum_transport_height_agl_m,
    project_aerodynamic_roughness_for_physics,
};
use crate::vertical::{
    ColumnGeometry, ColumnRequest, ColumnStencil, NativeCoordinateKind, VerticalBounds,
    VerticalBracket, VerticalError, VerticalTopology,
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
    column_cache: Arc<Mutex<ColumnCache>>,
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
            frame_cache: FrameCache::new(config.memory_budget.fixed_bytes),
            column_cache: Arc::new(Mutex::new(ColumnCache::new(
                config.memory_budget.dynamic_bytes(),
            ))),
            tile_cache: TileCache::default(),
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
        let frames = self.frame_cache.frames_for_domain(domain);
        let mut window = self
            .window_manager
            .prepare_from_sorted_frames(time, &frames, self.memory_budget.dynamic_bytes())
            .map_err(EngineError::Frame)?;
        window.attach_column_cache(&self.column_cache);
        Ok(window)
    }

    /// Publishes one already-loaded immutable frame into the engine cache.
    pub fn cache_frame(&mut self, frame: Arc<RawMetFrame>) -> Result<(), EngineError> {
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
        )?;
        Ok(PreparedBatch {
            plan: plan.clone(),
            window: self.clone(),
            batch,
            layout,
            initial_status,
            stencils,
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
        let (placements, initial_status) = locate_points(self, &batch)?;
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
        )?;
        Ok(PreparedTransportBatch {
            plan: plan.clone(),
            window: self.clone(),
            batch,
            layout,
            initial_status,
            stencils,
        })
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

fn prepare_layout(
    window: &PreparedWindow,
    placements: Vec<PointPlacement>,
    point_count: usize,
    plan: &QueryPlan,
    fixed_bytes: u64,
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
    by_frame: BTreeMap<crate::io::inventory::LogicalFrameId, PreparedFrameStencils>,
    resident_bytes: u64,
}

impl PreparedStencils {
    fn frame(
        &self,
        frame: &crate::io::inventory::LogicalFrameId,
    ) -> Option<&PreparedFrameStencils> {
        self.by_frame.get(frame)
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
    let representatives = placements.iter().fold(
        BTreeMap::<crate::grid::CellId, usize>::new(),
        |mut values, placement| {
            values
                .entry(placement.cell)
                .or_insert(placement.original_index);
            values
        },
    );
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
    let mut prepared = PreparedStencils::default();
    for frame in frames {
        let mut by_cell = BTreeMap::new();
        let topology = match &frame.metadata().vertical {
            VerticalTopology::HybridPressure(_) => "hybrid",
            VerticalTopology::PressureLevels(_) => "pressure",
        };
        for (cell, original_index) in &representatives {
            let key = CacheKey {
                frame: frame.metadata().id.clone(),
                domain: frame.metadata().domain.clone(),
                cell: *cell,
                capabilities,
                algorithm: format!("{M3_MET_QUERY_ALGORITHM_ID}/column_stencil/{topology}"),
            };
            let pin = if let Some(pin) = cache.get(&key) {
                pin
            } else {
                let stencil = Arc::new(
                    ColumnStencil::build(ColumnRequest {
                        frame: &frame,
                        cell: *cell,
                        longitude_degrees: batch.points.longitude_degrees[*original_index],
                        latitude_degrees: batch.points.latitude_degrees[*original_index],
                    })
                    .map_err(EngineError::Vertical)?,
                );
                cache.insert(key, stencil).map_err(EngineError::Cache)?
            };
            prepared.resident_bytes = prepared
                .resident_bytes
                .checked_add(pin.size_bytes())
                .ok_or(EngineError::MemoryEstimateOverflow)?;
            by_cell.insert(*cell, pin);
        }
        prepared.by_frame.insert(
            frame.metadata().id.clone(),
            PreparedFrameStencils { frame, by_cell },
        );
    }
    cache
        .trim_unpinned_to(prepared.resident_bytes)
        .map_err(EngineError::Cache)?;
    Ok(prepared)
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
}

fn frame_stencils<'a>(
    prepared: &'a PreparedStencils,
    frame: &RawMetFrame,
) -> Result<&'a PreparedFrameStencils, EngineError> {
    prepared
        .frame(&frame.metadata().id)
        .ok_or(EngineError::InvalidPreparedState)
}

fn point_column_request(
    frame: &RawMetFrame,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> ColumnRequest<'_> {
    ColumnRequest {
        frame,
        cell,
        longitude_degrees,
        latitude_degrees,
    }
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

fn target_column(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<ColumnGeometry, EngineError> {
    let before = frame_stencils(stencils, &window.frames.before)?.column(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = frame_stencils(stencils, &window.frames.after)?.column(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    blend_columns(&before, &after, window)
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

fn interpolate_logarithmic(bracket: VerticalBracket, values: &[f64]) -> Result<f64, EngineError> {
    let logarithms = values
        .iter()
        .map(|value| {
            if !value.is_finite() || *value <= 0.0 {
                return Err(EngineError::NumericalFailure);
            }
            Ok(value.ln())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let logarithm = bracket
        .interpolate(&logarithms)
        .map_err(EngineError::Vertical)?;
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
        if column.level_horizontal_weights()[level][corner] > 0.0 && (!u_valid || !v_valid) {
            return Err(EngineError::Vertical(
                VerticalError::InvalidHorizontalSupport,
            ));
        }
        eastward_values[corner] = u;
        northward_values[corner] = v;
    }
    let (source_longitude_degrees, source_latitude_degrees) =
        source_coordinates(&frame_stencils.frame, stencil.points());
    interpolate_spherical_vector(
        source_longitude_degrees,
        source_latitude_degrees,
        eastward_values,
        northward_values,
        column.level_horizontal_weights()[level],
        longitude_degrees,
        latitude_degrees,
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
    let source = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(field)))?;
    let stencil = frame_stencils.stencil(cell)?;
    let mut value = 0.0;
    for (corner, point) in stencil.points().iter().copied().enumerate() {
        let (corner_value, valid) = field_3d_value(source, level, point)?;
        let weight = column.level_horizontal_weights()[level][corner];
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
    let bracket =
        vertical_bracket(&column, coordinate, vertical_value).map_err(EngineError::Vertical)?;
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
    let physical_specific_humidity = bracket
        .interpolate(column.physical_specific_humidity())
        .map_err(EngineError::Vertical)?;
    let first_vector = sample_level_vector(
        frame_stencils,
        &column,
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
            &column,
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

fn sample_surface_scalar_frame(
    frame_stencils: &PreparedFrameStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    field: CanonicalField,
) -> Result<f64, EngineError> {
    let source = frame_stencils
        .frame
        .fields()
        .get(&FieldKey::Canonical(field))
        .ok_or(EngineError::MissingField(FieldKey::Canonical(field)))?;
    let grid = RegularLatLonGrid::new(frame_stencils.frame.metadata().grid.clone())
        .map_err(EngineError::Grid)?;
    let weights = grid
        .horizontal_weights(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?;
    if grid
        .locate_cell(longitude_degrees, latitude_degrees)
        .map_err(EngineError::Grid)?
        != cell
        || weights.points != *frame_stencils.stencil(cell)?.points()
    {
        return Err(EngineError::InvalidPreparedState);
    }
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
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let before_geometry = before_stencils
        .stencil(cell)?
        .level_geometry(
            point_column_request(
                &before_stencils.frame,
                cell,
                longitude_degrees,
                latitude_degrees,
            ),
            level,
        )
        .map_err(EngineError::Vertical)?;
    if !window.is_exact_frame() {
        let after_stencils = frame_stencils(stencils, &window.frames.after)?;
        let after_geometry = after_stencils
            .stencil(cell)?
            .level_geometry(
                point_column_request(
                    &after_stencils.frame,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                ),
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
        .level_geometry(
            point_column_request(
                &previous_stencils.frame,
                cell,
                longitude_degrees,
                latitude_degrees,
            ),
            level,
        )
        .map_err(EngineError::Vertical)?;
    let next_geometry = next_stencils
        .stencil(cell)?
        .level_geometry(
            point_column_request(
                &next_stencils.frame,
                cell,
                longitude_degrees,
                latitude_degrees,
            ),
            level,
        )
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

fn geometric_w_profile(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<(Vec<f64>, Vec<bool>), EngineError> {
    let before_stencils = frame_stencils(stencils, &window.frames.before)?;
    let before_column = before_stencils.column(cell, longitude_degrees, latitude_degrees)?;
    let after_column = if window.is_exact_frame() {
        before_column.clone()
    } else {
        frame_stencils(stencils, &window.frames.after)?.column(
            cell,
            longitude_degrees,
            latitude_degrees,
        )?
    };
    let stencil = before_stencils.stencil(cell)?;
    let (coordinate_kind, native_coordinate) = stencil.native_coordinate();
    if native_coordinate.len() != target_column.pressure_pa().len() {
        return Err(EngineError::InvalidPreparedState);
    }
    let vertical_field = match coordinate_kind {
        NativeCoordinateKind::PressurePa => CanonicalField::PressureVerticalVelocity,
        NativeCoordinateKind::HybridEta => {
            if before_stencils
                .frame
                .fields()
                .get(&FieldKey::Canonical(CanonicalField::HybridVerticalVelocity))
                .is_some()
            {
                CanonicalField::HybridVerticalVelocity
            } else {
                CanonicalField::PressureVerticalVelocity
            }
        }
    };
    let levels = target_column.pressure_pa().len();
    let mut values = vec![0.0; levels];
    let mut valid = vec![false; levels];
    for level in 0..levels {
        if !target_column.validity().valid[level] {
            continue;
        }
        let result = (|| {
            let (geometry, height_time_derivative_m_s, pressure_time_derivative_pa_s) =
                target_level_geometry(
                    window,
                    stencils,
                    cell,
                    longitude_degrees,
                    latitude_degrees,
                    level,
                )?;
            let (eastward_wind_m_s, northward_wind_m_s) = time_level_vector(
                window,
                stencils,
                &before_column,
                &after_column,
                cell,
                longitude_degrees,
                latitude_degrees,
                level,
            )?;
            let source_vertical_velocity = time_level_scalar(
                window,
                stencils,
                &before_column,
                &after_column,
                cell,
                level,
                vertical_field,
            )?;
            let height_coordinate_derivative =
                derivative_at(target_column.height_asl_m(), native_coordinate, level)?;
            let coordinate_velocity = match coordinate_kind {
                NativeCoordinateKind::PressurePa => source_vertical_velocity,
                NativeCoordinateKind::HybridEta
                    if vertical_field == CanonicalField::HybridVerticalVelocity =>
                {
                    source_vertical_velocity
                }
                NativeCoordinateKind::HybridEta => {
                    let pressure_coordinate_derivative =
                        derivative_at(target_column.pressure_pa(), native_coordinate, level)?;
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
        })();
        match result {
            Ok(value) => {
                values[level] = value;
                valid[level] = true;
            }
            Err(error) if local_status_for_error(&error).is_some() => {}
            Err(error) => return Err(error),
        }
    }
    Ok((values, valid))
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

fn target_frame_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
) -> Result<FramePointSample, EngineError> {
    let before = sample_frame_point(
        frame_stencils(stencils, &window.frames.before)?,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    if window.is_exact_frame() {
        return Ok(before);
    }
    let after = sample_frame_point(
        frame_stencils(stencils, &window.frames.after)?,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
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

fn vertical_bounds_for_point(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
) -> Result<VerticalBounds, EngineError> {
    let roughness = sample_surface_scalar_time(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        CanonicalField::AerodynamicRoughnessLength,
    )?;
    let minimum =
        minimum_transport_height_agl_m(roughness).map_err(|_| EngineError::NumericalFailure)?;
    column
        .vertical_bounds(minimum)
        .map_err(|_| EngineError::Vertical(VerticalError::InvalidVerticalColumn))
}

#[allow(clippy::too_many_arguments)]
fn sample_upper_transport(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    target_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bracket: VerticalBracket,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
    let state = target_frame_point(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        coordinate,
        vertical_value,
    )?;
    let (w_profile, w_valid) = geometric_w_profile(
        window,
        stencils,
        target_column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    if !w_valid.get(bracket.first).copied().unwrap_or(false)
        || !w_valid.get(bracket.second).copied().unwrap_or(false)
    {
        return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
    }
    let geometric_vertical_velocity_m_s = bracket
        .interpolate(&w_profile)
        .map_err(EngineError::Vertical)?;
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
            target_column.terrain_asl_m(),
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
    target_column: &ColumnGeometry,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    bounds: VerticalBounds,
    metadata: &[FieldMetadata; TRANSPORT_FIELD_COUNT],
) -> Result<TransportPointResult, EngineError> {
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
    let structural_lowest = target_column
        .lowest_valid_index()
        .map_err(EngineError::Vertical)?;
    let structural_lowest_height_agl_m =
        target_column.height_asl_m()[structural_lowest] - target_column.terrain_asl_m();
    let structural_query_height_agl_m =
        surface_query_height_agl_m(target_column, coordinate, vertical_value, structural_lowest)?;
    if !structural_query_height_agl_m.is_finite()
        || structural_query_height_agl_m < minimum
        || structural_query_height_agl_m > structural_lowest_height_agl_m
    {
        return Ok(TransportPointResult::invalid(
            SampleStatus::SurfaceLayerUndefined,
            Some(bounds),
            metadata,
        ));
    }
    let before_column = frame_stencils(stencils, &window.frames.before)?.column(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let after_column = if window.is_exact_frame() {
        before_column.clone()
    } else {
        frame_stencils(stencils, &window.frames.after)?.column(
            cell,
            longitude_degrees,
            latitude_degrees,
        )?
    };
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
        target_column,
        &before_column,
        &after_column,
        &w_valid,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let lowest_height_agl_m = target_column.height_asl_m()[lowest] - target_column.terrain_asl_m();
    let query_height_agl_m =
        surface_query_height_agl_m(target_column, coordinate, vertical_value, lowest)?;
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
        target_column.surface_pressure_pa(),
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
            lowest_model_air_temperature_k: &[target_column.temperature_k()[lowest]],
            lowest_model_specific_humidity: &[target_column.physical_specific_humidity()[lowest]],
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
        target_column,
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
            target_column.terrain_asl_m(),
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
            lowest_model_specific_humidity: &[column.physical_specific_humidity()[lowest]],
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

#[allow(clippy::too_many_arguments)]
fn temporal_mixed_columns(
    window: &PreparedWindow,
    stencils: &PreparedStencils,
    cell: crate::grid::CellId,
    longitude_degrees: f64,
    latitude_degrees: f64,
    coordinate: VerticalQuery,
    vertical_value: f64,
    target_bracket: &Result<VerticalBracket, VerticalError>,
) -> Result<Option<(ColumnGeometry, ColumnGeometry)>, EngineError> {
    if window.is_exact_frame()
        || !matches!(
            target_bracket,
            Ok(_) | Err(VerticalError::SurfaceLayerRequired)
        )
    {
        return Ok(None);
    }
    let before = frame_stencils(stencils, &window.frames.before)?.column(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let after = frame_stencils(stencils, &window.frames.after)?.column(
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let before_bracket = vertical_bracket(&before, coordinate, vertical_value);
    let after_bracket = vertical_bracket(&after, coordinate, vertical_value);
    Ok(endpoint_routes_are_mixed(&before_bracket, &after_bracket).then_some((before, after)))
}

fn sample_transport_point(
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
    let bounds = vertical_bounds_for_point(
        window,
        stencils,
        &column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let target_bracket = vertical_bracket(&column, batch.vertical_coordinate, vertical_value);
    let mixed_columns = temporal_mixed_columns(
        window,
        stencils,
        cell,
        longitude_degrees,
        latitude_degrees,
        batch.vertical_coordinate,
        vertical_value,
        &target_bracket,
    )?;
    let result = if let Some((before_column, after_column)) = mixed_columns {
        sample_temporal_mixed_transport(
            plan,
            window,
            stencils,
            &column,
            &before_column,
            &after_column,
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
            Ok(bracket) => sample_upper_transport(
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
            ),
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
    match result {
        Err(error) if local_status_for_error(&error).is_some() => {
            Ok(TransportPointResult::invalid(
                local_status_for_error(&error).unwrap_or(SampleStatus::NumericalFailure),
                Some(bounds),
                &metadata.upper_transport,
            ))
        }
        other => other,
    }
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
    let target_bracket = vertical_bracket(&column, batch.vertical_coordinate, vertical_value);
    let mixed_route = generic_needs_surface_model(plan)
        && plan.surface_layer_model().is_some()
        && temporal_mixed_columns(
            window,
            stencils,
            cell,
            longitude_degrees,
            latitude_degrees,
            batch.vertical_coordinate,
            vertical_value,
            &target_bracket,
        )?
        .is_some();
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
            Ok(bracket) => {
                let first_weights = column.level_horizontal_weights()[bracket.first];
                let (second_level_weights, second_level_method) = if bracket.second == bracket.first
                {
                    (None, None)
                } else {
                    let weights = column.level_horizontal_weights()[bracket.second];
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
fn sample_generic_surface_point(
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
        column,
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
            | CanonicalField::LatentHeatFlux
            | CanonicalField::FrictionVelocity
            | CanonicalField::MoninObukhovLength => Some(sample_surface_scalar_time(
                window,
                stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                *canonical,
            )?),
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
        &column,
        cell,
        longitude_degrees,
        latitude_degrees,
    )?;
    let target_bracket = vertical_bracket(&column, batch.vertical_coordinate, vertical_value);
    if generic_needs_surface_model(plan) && plan.surface_layer_model().is_some() {
        if let Some((before_column, after_column)) = temporal_mixed_columns(
            window,
            stencils,
            cell,
            longitude_degrees,
            latitude_degrees,
            batch.vertical_coordinate,
            vertical_value,
            &target_bracket,
        )? {
            return sample_generic_temporal_mixed_point(
                plan,
                window,
                stencils,
                &column,
                &before_column,
                &after_column,
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
    let state = target_frame_point(
        window,
        stencils,
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
    let needs_w = plan
        .fields()
        .iter()
        .any(|field| field == &FieldKey::Canonical(CanonicalField::GeometricVerticalVelocity));
    let w = if needs_w {
        let (profile, valid) = geometric_w_profile(
            window,
            stencils,
            &column,
            cell,
            longitude_degrees,
            latitude_degrees,
        )?;
        if !valid[bracket.first] || !valid[bracket.second] {
            return Err(EngineError::Vertical(VerticalError::InvalidVerticalColumn));
        }
        Some(
            bracket
                .interpolate(&profile)
                .map_err(EngineError::Vertical)?,
        )
    } else {
        None
    };
    let geometric_height = match batch.vertical_coordinate {
        VerticalQuery::AboveSeaLevel => vertical_value,
        VerticalQuery::AboveGround => column.terrain_asl_m() + vertical_value,
        VerticalQuery::Pressure => bracket
            .interpolate(column.height_asl_m())
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
                7 => column.terrain_asl_m(),
                _ => return Err(EngineError::InvalidPreparedState),
            };
            continue;
        }
        let FieldKey::Canonical(canonical) = field else {
            return Err(EngineError::UnsupportedQueryField(field.clone()));
        };
        let value = match canonical {
            CanonicalField::GeometricHeight => geometric_height,
            CanonicalField::SurfacePressure => column.surface_pressure_pa(),
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
            | CanonicalField::LatentHeatFlux
            | CanonicalField::FrictionVelocity
            | CanonicalField::MoninObukhovLength => sample_surface_scalar_time(
                window,
                stencils,
                cell,
                longitude_degrees,
                latitude_degrees,
                *canonical,
            )?,
            _ => return Err(EngineError::UnsupportedQueryField(field.clone())),
        };
        result.values[index] = value;
    }
    Ok(result)
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
}

impl PreparedBatch {
    /// Executes without reader, provider, or cache mutation.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<QueryOutput, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
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
    stencils: PreparedStencils,
}

impl PreparedTransportBatch {
    /// Executes all eight transport columns without reader or cache mutation.
    pub fn execute(
        &self,
        context: &dyn ExecutionContext,
        _workspace: &mut BatchWorkspace,
    ) -> Result<TransportOutput, EngineError> {
        validate_execution_state(context, &self.layout, self.initial_status.len())?;
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
            for internal_index in boundaries[0]..boundaries[1] {
                let original_index = self.layout.permutation.forward[internal_index];
                let cell = self.layout.cell_ids[internal_index];
                match sample_transport_point(
                    self.plan.query_plan(),
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
        .map_err(EngineError::Output)
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
    use crate::grid::DomainGeometry;
    use crate::io::inventory::LogicalFrameId;
    use crate::profile::graph::{ExecutionPlan, GraphUnit};
    use crate::provenance::{ProvenanceRecord, ProvenanceTable};
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
    fn hybrid_eta_dot_and_omega_follow_the_same_native_coordinate_chain() {
        let plan = transport_plan();
        let mut workspace = BatchWorkspace::default();
        let run = |use_eta_dot: bool,
                   workspace: &mut BatchWorkspace|
         -> (TransportOutput, Arc<RawMetFrame>) {
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
