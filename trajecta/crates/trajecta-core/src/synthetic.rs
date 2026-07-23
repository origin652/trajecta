//! # Contract: synthetic constant-field meteorology for M4-A1 gates
//!
//! Builds immutable RawMetFrame stacks and MetEngine instances used by
//! engineering hard gates without reading external datasets.

use std::collections::BTreeMap;
use std::sync::Arc;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::time::Timestamp;
use trajecta_case::quantity::{Dimension, Unit};
use trajecta_met::field::{
    CanonicalField, Capability, CapabilitySet, FieldDescriptor, FieldKey, FieldQuality,
    FieldRegistry,
};
use trajecta_met::frame::{
    ArrayLayout, FrameMetadata, RawField, RawFieldStore, RawMetFrame, TemporalSupport,
};
use trajecta_met::grid::DomainGeometry;
use trajecta_met::io::inventory::{DomainCatalog, LogicalFrameId, MetCatalog};
use trajecta_met::profile::document::ProfileCatalog;
use trajecta_met::profile::graph::{ExecutionPlan, GraphUnit};
use trajecta_met::provenance::{ProvenanceRecord, ProvenanceTable};
use trajecta_met::query::cache::MemoryBudget;
use trajecta_met::query::engine::{
    ExecutionContext, MetEngine, MetEngineConfig, RayonExecutionContext,
};
use trajecta_met::query::request::{ExplainMode, TransportPlan, TransportPlanRequest};
use trajecta_met::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerRegistry};
use trajecta_met::vertical::{PressureLevels, VerticalTopology};

use crate::runner::RunError;

/// Complete synthetic meteorology stack for builder injection.
pub struct SyntheticStack {
    /// Domain identity.
    pub domain: DomainId,
    /// Prepared engine with cached frames.
    pub engine: MetEngine,
    /// Compiled transport plan.
    pub transport_plan: TransportPlan,
    /// Execution context.
    pub execution: Box<dyn ExecutionContext>,
}

/// Constant wind specification for synthetic frames.
#[derive(Clone, Copy, Debug)]
pub struct SyntheticWind {
    /// Eastward wind m/s.
    pub eastward_m_s: f64,
    /// Northward wind m/s.
    pub northward_m_s: f64,
    /// Geometric vertical velocity m/s (stored via kinematic field path when available).
    pub vertical_m_s: f64,
}

/// Convenience wrapper returning [`SyntheticStack`].
pub fn constant_wind_stack(
    domain_id: &str,
    times: &[Timestamp],
    wind: SyntheticWind,
    terrain_asl_m: f64,
    model_top_asl_m: f64,
    periodic_longitude: bool,
) -> Result<SyntheticStack, RunError> {
    if times.is_empty() {
        return Err(RunError::InvalidConfiguration(
            "synthetic times empty".into(),
        ));
    }
    let domain = DomainId(domain_id.into());
    let domain_catalog = DomainCatalog {
        domain: Some(domain.clone()),
        frames: BTreeMap::new(),
        coverage: Default::default(),
    };
    let _ = (times, periodic_longitude);
    let mut catalog = MetCatalog::default();
    catalog.domains.insert(domain.clone(), domain_catalog);
    catalog.capabilities = CapabilitySet::new()
        .with(Capability::Transport)
        .with(Capability::NearSurfaceTransport);

    let fields = transport_field_registry()?;
    let mut surface_layers = SurfaceLayerRegistry::new();
    surface_layers
        .register(Arc::new(MoninObukhovBusingerDyer::default()))
        .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;

    let memory_budget = MemoryBudget::new(64 * 1024 * 1024, 32 * 1024 * 1024)
        .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;

    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles: ProfileCatalog::default(),
        fields,
        surface_layers,
        memory_budget,
    });

    for time in times {
        let frame = constant_frame(
            &domain,
            *time,
            wind,
            terrain_asl_m,
            model_top_asl_m,
            periodic_longitude,
        )?;
        engine
            .cache_frame(frame)
            .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;
    }

    let transport_plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Disabled,
                surface_layer_model: ModelId(MoninObukhovBusingerDyer::MODEL_ID.into()),
            },
            &ExecutionPlan::default(),
        )
        .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;

    Ok(SyntheticStack {
        domain,
        engine,
        transport_plan,
        execution: Box::new(RayonExecutionContext { worker_threads: 1 }),
    })
}

fn transport_field_registry() -> Result<FieldRegistry, RunError> {
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
        CanonicalField::GeometricHeight,
        CanonicalField::SurfacePressure,
        CanonicalField::SurfaceGeopotential,
        CanonicalField::PressureVerticalVelocity,
        CanonicalField::TenMetreEastwardWind,
        CanonicalField::TenMetreNorthwardWind,
        CanonicalField::TwoMetreAirTemperature,
        CanonicalField::TwoMetreSpecificHumidity,
        CanonicalField::AerodynamicRoughnessLength,
        CanonicalField::BoundaryLayerHeight,
        CanonicalField::EastwardSurfaceStress,
        CanonicalField::NorthwardSurfaceStress,
        CanonicalField::SensibleHeatFlux,
        CanonicalField::LatentHeatFlux,
    ] {
        let semantics = canonical.semantics();
        registry
            .register(FieldDescriptor {
                key: FieldKey::Canonical(canonical),
                unit: Unit::new(semantics.unit, semantics.dimension, 1.0, 0.0)
                    .map_err(|error| RunError::InvalidConfiguration(format!("unit: {error:?}")))?,
                shape: semantics.shape,
                vertical_stagger: semantics.vertical_stagger,
                quality: FieldQuality::Source,
                required_capability: semantics.required_capability,
                interpolation: semantics.interpolation,
            })
            .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;
    }
    Ok(registry)
}

fn constant_frame(
    domain: &DomainId,
    time: Timestamp,
    wind: SyntheticWind,
    terrain_asl_m: f64,
    model_top_asl_m: f64,
    periodic_longitude: bool,
) -> Result<Arc<RawMetFrame>, RunError> {
    let ny = 5_usize;
    let nx = 8_usize;
    let levels = 3_usize;
    let id = LogicalFrameId {
        domain: domain.clone(),
        valid_time: time,
        // Stable fake SHA-256 (64 lowercase hex) — required by provenance-bundle/v1.
        profile_sha256: "aa".repeat(32),
        content_sha256: format!(
            "synthetic-{}-{}-{}-{}",
            time.seconds_since_unix_epoch(),
            wind.eastward_m_s,
            wind.northward_m_s,
            wind.vertical_m_s
        ),
    };
    let grid = DomainGeometry {
        domain: domain.clone(),
        longitude_origin_degrees: if periodic_longitude { -180.0 } else { -20.0 },
        latitude_origin_degrees: -40.0,
        longitude_spacing_degrees: if periodic_longitude { 45.0 } else { 10.0 },
        latitude_spacing_degrees: 20.0,
        nx,
        ny,
        periodic_longitude,
        halo_cells: 0,
    };
    let full = ArrayLayout::Full3D { levels, ny, nx };
    let horizontal = ArrayLayout::Horizontal2D { ny, nx };
    let mut fields = RawFieldStore::new();
    let mut provenance = ProvenanceTable::new();

    let height_levels = [model_top_asl_m, 5_000.0, terrain_asl_m + 200.0];
    let mut heights = Vec::with_capacity(levels * ny * nx);
    for height in height_levels {
        for _ in 0..(ny * nx) {
            heights.push(height);
        }
    }
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::GeometricHeight,
        heights,
        full,
        time,
        "m",
        Dimension::LENGTH,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::EastwardWind,
        vec![wind.eastward_m_s; levels * ny * nx],
        full,
        time,
        "m/s",
        Dimension::VELOCITY,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::NorthwardWind,
        vec![wind.northward_m_s; levels * ny * nx],
        full,
        time,
        "m/s",
        Dimension::VELOCITY,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::AirTemperature,
        vec![250.0; levels * ny * nx],
        full,
        time,
        "K",
        Dimension::TEMPERATURE,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::SpecificHumidity,
        vec![0.001; levels * ny * nx],
        full,
        time,
        "1",
        Dimension::DIMENSIONLESS,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::PressureVerticalVelocity,
        vec![0.0; levels * ny * nx],
        full,
        time,
        "Pa/s",
        Dimension::PRESSURE_TENDENCY,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::GeometricTerrainHeight,
        vec![terrain_asl_m; ny * nx],
        horizontal,
        time,
        "m",
        Dimension::LENGTH,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::SurfacePressure,
        vec![100_000.0; ny * nx],
        horizontal,
        time,
        "Pa",
        Dimension::PRESSURE,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::SurfaceGeopotential,
        vec![terrain_asl_m * 9.80665; ny * nx],
        horizontal,
        time,
        "m2 s-2",
        Dimension::GEOPOTENTIAL,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::TenMetreEastwardWind,
        vec![0.0; ny * nx],
        horizontal,
        time,
        "m/s",
        Dimension::VELOCITY,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::TenMetreNorthwardWind,
        vec![0.0; ny * nx],
        horizontal,
        time,
        "m/s",
        Dimension::VELOCITY,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::TwoMetreAirTemperature,
        vec![288.0; ny * nx],
        horizontal,
        time,
        "K",
        Dimension::TEMPERATURE,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::TwoMetreSpecificHumidity,
        vec![0.005; ny * nx],
        horizontal,
        time,
        "1",
        Dimension::DIMENSIONLESS,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::AerodynamicRoughnessLength,
        vec![0.1; ny * nx],
        horizontal,
        time,
        "m",
        Dimension::LENGTH,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::BoundaryLayerHeight,
        vec![1_000.0; ny * nx],
        horizontal,
        time,
        "m",
        Dimension::LENGTH,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::EastwardSurfaceStress,
        vec![0.12; ny * nx],
        horizontal,
        time,
        "Pa",
        Dimension::PRESSURE,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::NorthwardSurfaceStress,
        vec![0.0; ny * nx],
        horizontal,
        time,
        "Pa",
        Dimension::PRESSURE,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::SensibleHeatFlux,
        vec![100.0; ny * nx],
        horizontal,
        time,
        "W m-2",
        Dimension::ENERGY_FLUX,
    )?;
    insert_field(
        &mut fields,
        &mut provenance,
        &id,
        CanonicalField::LatentHeatFlux,
        vec![50.0; ny * nx],
        horizontal,
        time,
        "W m-2",
        Dimension::ENERGY_FLUX,
    )?;

    let metadata = FrameMetadata {
        id,
        domain: domain.clone(),
        valid_time: time,
        grid,
        vertical: VerticalTopology::PressureLevels(
            PressureLevels::new(Arc::from([10_000.0_f64, 50_000.0, 90_000.0]))
                .map_err(|error| RunError::Meteorology(format!("pressure levels: {error:?}")))?,
        ),
    };
    RawMetFrame::publish(metadata, fields, Arc::new(provenance))
        .map(Arc::new)
        .map_err(|error| RunError::Meteorology(format!("{error:?}")))
}

#[allow(clippy::too_many_arguments)]
fn insert_field(
    fields: &mut RawFieldStore,
    provenance: &mut ProvenanceTable,
    frame_id: &LogicalFrameId,
    field: CanonicalField,
    values: Vec<f64>,
    layout: ArrayLayout,
    time: Timestamp,
    unit_symbol: &str,
    dimension: Dimension,
) -> Result<(), RunError> {
    let key = FieldKey::Canonical(field);
    let prov_id = provenance
        .intern(ProvenanceRecord {
            field: key.clone(),
            quality: FieldQuality::Source,
            sources: vec![frame_id.content_sha256.clone()],
            transforms: Vec::new(),
            fallback_reason: None,
            profile_sha256: frame_id.profile_sha256.clone(),
        })
        .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;
    let count = layout
        .element_count()
        .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;
    let unit = GraphUnit::parse(unit_symbol)
        .or_else(|_| {
            // Fallback through Unit then GraphUnit parse of SI symbol.
            let _ = dimension;
            GraphUnit::parse(unit_symbol)
        })
        .map_err(|error| RunError::Meteorology(format!("graph unit {unit_symbol}: {error:?}")))?;
    let raw = RawField::new(
        Arc::<[f64]>::from(values),
        Arc::<[bool]>::from(vec![true; count]),
        unit,
        layout,
        TemporalSupport::Instantaneous { valid_time: time },
        FieldQuality::Source,
        prov_id,
    )
    .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;
    fields
        .insert(key, raw)
        .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;
    Ok(())
}
