//! Real ERA5 pressure/hybrid full M3 query chains (not lock→frame only).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::{
    CanonicalField, Capability, CapabilitySet, FieldKey, FieldQuality, FieldRegistry,
};
use trajecta_met::frame::RawMetFrame;
use trajecta_met::io::frame_loader::{FrameLoadRequest, FrameLoader};
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder, MetCatalog};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::query::cache::MemoryBudget;
use trajecta_met::query::engine::{
    BatchWorkspace, MetEngine, MetEngineConfig, RayonExecutionContext,
};
use trajecta_met::query::output::{SampleStatus, TransportOutput};
use trajecta_met::query::request::{
    ExplainMode, QueryBatch, QueryPointArrays, TransportPlanRequest, VerticalQuery,
};
use trajecta_met::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerRegistry};

fn require_real_met() -> bool {
    std::env::var("TRAJECTA_REQUIRE_REAL_MET").ok().as_deref() == Some("1")
}

fn skip_or_fail(missing: &str) {
    if require_real_met() {
        panic!("required real met missing: {missing}");
    }
}

fn era5_pressure_ready_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_ERA5_PRESSURE_OFFICIAL_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-data/era5-cds-pressure-official/ready")
}

fn era5_hybrid_ready_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_ERA5_HYBRID137_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-data/era5-cds-hybrid137-official/ready")
}

fn transport_capabilities() -> CapabilitySet {
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(Capability::Transport);
    capabilities.insert(Capability::NearSurfaceTransport);
    capabilities
}

fn default_surface_layers() -> SurfaceLayerRegistry {
    let mut registry = SurfaceLayerRegistry::new();
    registry
        .register(Arc::new(MoninObukhovBusingerDyer::default()))
        .unwrap();
    registry
}

fn load_era5_ready_frames(
    directory: &Path,
    start: Timestamp,
    end: Timestamp,
    dataset_id: &str,
) -> Option<(ProfileCatalog, MetCatalog, Vec<Arc<RawMetFrame>>)> {
    if !directory.is_dir() {
        skip_or_fail(&directory.display().to_string());
        return None;
    }
    let isolated = tempfile::tempdir().expect("temp data root");
    let mut copied = 0usize;
    for entry in std::fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("nc") {
            std::fs::copy(&path, isolated.path().join(path.file_name().unwrap())).unwrap();
            copied += 1;
        }
    }
    if copied == 0 {
        skip_or_fail("no netcdf ready files");
        return None;
    }
    let profiles = ProfileCatalog::load(&[]).expect("profiles");
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let capabilities = transport_capabilities();
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef(dataset_id.into()),
            source: "Copernicus CDS ERA5".into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-met-test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), isolated.path().to_path_buf())]),
        coverage: LockCoverageRequest {
            start,
            end,
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities,
        force_rehash: true,
        preferred_profile: None,
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    assert!(outcome.is_success(), "{:?}", outcome.diagnostics);
    let lock = outcome.lock.expect("lock");
    let domain = DomainId("era5".into());
    let roots = BTreeMap::from([(DataRootId("met".into()), isolated.path().to_path_buf())]);
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir: isolated.path(),
        data_roots: &roots,
        domain: &domain,
        required_capabilities: capabilities,
    });
    assert!(
        inventory.is_success(),
        "{:?}",
        inventory.diagnostics.sorted()
    );
    let catalog = inventory.catalog.expect("catalog");
    assert!(
        catalog
            .capabilities
            .contains(Capability::NearSurfaceTransport)
    );
    let profile = profiles
        .get(&ProfileName(lock.profile.name.clone()))
        .expect("profile");
    let mut frames = Vec::new();
    for descriptor in catalog
        .domains
        .get(&domain)
        .expect("domain")
        .frames
        .values()
    {
        let frame = FrameLoader::load(FrameLoadRequest {
            descriptor,
            profile,
            required_capabilities: capabilities,
            backend: MeteorologyReaderBackend::Rust,
            previous_frame: frames.last().map(Arc::as_ref),
        })
        .expect("frame load");
        for field in [
            CanonicalField::SensibleHeatFlux,
            CanonicalField::LatentHeatFlux,
            CanonicalField::TwoMetreSpecificHumidity,
        ] {
            let f = frame
                .fields()
                .get(&FieldKey::Canonical(field))
                .unwrap_or_else(|| panic!("missing {field:?}"));
            assert_eq!(f.quality(), FieldQuality::Derived, "{field:?}");
        }
        frames.push(Arc::new(frame));
    }
    let _ = isolated;
    Some((profiles, catalog, frames))
}

fn engine_from_frames(
    profiles: ProfileCatalog,
    catalog: MetCatalog,
    frames: &[Arc<RawMetFrame>],
) -> MetEngine {
    let budget = MemoryBudget::new(2_u64 * 1024 * 1024 * 1024, 256_u64 * 1024 * 1024).unwrap();
    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles,
        fields: FieldRegistry::canonical().unwrap(),
        surface_layers: default_surface_layers(),
        memory_budget: budget,
    });
    for frame in frames {
        engine.cache_frame(Arc::clone(frame)).unwrap();
    }
    engine
}

fn assert_transport_row_complete(output: &TransportOutput, index: usize) {
    let row = output.row(index).unwrap();
    assert_eq!(row.status(), SampleStatus::Ok, "point {index}");
    for (name, value) in [
        ("u", row.eastward_wind_m_s()),
        ("v", row.northward_wind_m_s()),
        ("w", row.geometric_vertical_velocity_m_s()),
        ("p", row.air_pressure_pa()),
        ("T", row.air_temperature_k()),
        ("q", row.specific_humidity()),
        ("rho", row.air_density_kg_m3()),
        ("terrain", row.terrain_height_asl_m()),
    ] {
        let value = value.unwrap_or_else(|| panic!("ok row {index} missing {name}"));
        assert!(value.is_finite(), "{name} non-finite at {index}");
    }
}

fn assert_transport_outputs_identical(a: &TransportOutput, b: &TransportOutput) {
    assert_eq!(a.status().values(), b.status().values(), "status");
    let ca = a.columns();
    let cb = b.columns();
    for (name, left, right) in [
        (
            "u",
            ca.eastward_wind_m_s.values(),
            cb.eastward_wind_m_s.values(),
        ),
        (
            "v",
            ca.northward_wind_m_s.values(),
            cb.northward_wind_m_s.values(),
        ),
        (
            "w",
            ca.geometric_vertical_velocity_m_s.values(),
            cb.geometric_vertical_velocity_m_s.values(),
        ),
        (
            "p",
            ca.air_pressure_pa.values(),
            cb.air_pressure_pa.values(),
        ),
        (
            "T",
            ca.air_temperature_k.values(),
            cb.air_temperature_k.values(),
        ),
        (
            "q",
            ca.specific_humidity.values(),
            cb.specific_humidity.values(),
        ),
        (
            "rho",
            ca.air_density_kg_m3.values(),
            cb.air_density_kg_m3.values(),
        ),
        (
            "terrain",
            ca.terrain_height_asl_m.values(),
            cb.terrain_height_asl_m.values(),
        ),
    ] {
        assert_eq!(left, right, "{name} values");
    }
    for (name, left, right) in [
        ("u", &ca.eastward_wind_m_s, &cb.eastward_wind_m_s),
        ("v", &ca.northward_wind_m_s, &cb.northward_wind_m_s),
        (
            "w",
            &ca.geometric_vertical_velocity_m_s,
            &cb.geometric_vertical_velocity_m_s,
        ),
        ("p", &ca.air_pressure_pa, &cb.air_pressure_pa),
        ("T", &ca.air_temperature_k, &cb.air_temperature_k),
        ("q", &ca.specific_humidity, &cb.specific_humidity),
        ("rho", &ca.air_density_kg_m3, &cb.air_density_kg_m3),
        (
            "terrain",
            &ca.terrain_height_asl_m,
            &cb.terrain_height_asl_m,
        ),
    ] {
        assert_eq!(left.validity(), right.validity(), "{name} validity");
        assert_eq!(left.quality(), right.quality(), "{name} quality");
        assert_eq!(left.provenance(), right.provenance(), "{name} provenance");
    }
    let ta = a.provenance();
    let tb = b.provenance();
    // Every provenance id on A must resolve and match B's record for same id.
    for col in [
        &ca.eastward_wind_m_s,
        &ca.northward_wind_m_s,
        &ca.geometric_vertical_velocity_m_s,
        &ca.air_pressure_pa,
        &ca.air_temperature_k,
        &ca.specific_humidity,
        &ca.air_density_kg_m3,
        &ca.terrain_height_asl_m,
    ] {
        for id in col.provenance() {
            let ra = ta.get(*id).expect("a prov");
            let rb = tb.get(*id).expect("b prov");
            assert_eq!(ra, rb, "provenance record drift for {:?}", id);
        }
    }
}

#[test]
fn era5_pressure_transport_full_query_chain() {
    let start = Timestamp::new(1_543_622_400, 0).unwrap();
    let end = Timestamp::new(1_543_665_600, 0).unwrap();
    let Some((profiles, catalog, frames)) =
        load_era5_ready_frames(&era5_pressure_ready_dir(), start, end, "era5-pressure-m3")
    else {
        return;
    };
    assert_eq!(frames.len(), 3, "00/06/12");
    assert!(
        frames[0]
            .fields()
            .get(&FieldKey::Canonical(CanonicalField::GeopotentialHeight))
            .is_some()
    );

    let mut engine = engine_from_frames(profiles, catalog, &frames);
    let plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Full,
                ..TransportPlanRequest::default()
            },
            &ExecutionPlan::default(),
        )
        .expect("transport plan");

    // 06 UTC is the interior frame (00/12 endpoints hit MissingSymmetricTimeSupport).
    let on_frame = Timestamp::new(1_543_644_000, 0).unwrap();
    // Between 00 and 06: not exact analysis time for 6h pressure product.
    let mid = Timestamp::new(1_543_633_200, 0).unwrap();
    for time in [on_frame, mid] {
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveGround,
            points: QueryPointArrays {
                // Stay well inside the CDS 0.25° box [lon 0..6, lat 48..52].
                longitude_degrees: vec![3.0, 2.0, 4.0],
                latitude_degrees: vec![50.0, 49.5, 50.5],
                vertical: vec![10.0, 30.0, 80.0],
            },
        };
        let window = engine.prepare(time).expect("prepare");
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_transport_batch(&plan, batch, &mut workspace)
            .expect("prepare_transport_batch")
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .expect("execute");
        for index in 0..3 {
            assert_transport_row_complete(&output, index);
        }
    }

    let window = engine.prepare(on_frame).expect("prepare 00");
    let mut workspace = BatchWorkspace::default();
    let asl = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveSeaLevel,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0],
            latitude_degrees: vec![50.0],
            vertical: vec![1_500.0],
        },
    };
    let out = window
        .prepare_transport_batch(&plan, asl, &mut workspace)
        .expect("asl")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("asl exec");
    assert_transport_row_complete(&out, 0);

    let pa = QueryBatch {
        vertical_coordinate: VerticalQuery::Pressure,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0],
            latitude_degrees: vec![50.0],
            vertical: vec![70_000.0],
        },
    };
    let out = window
        .prepare_transport_batch(&plan, pa, &mut workspace)
        .expect("pa")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("pa exec");
    assert_transport_row_complete(&out, 0);
}

#[test]
fn era5_hybrid137_transport_full_query_chain() {
    let start = Timestamp::new(1_543_622_400, 0).unwrap();
    let end = Timestamp::new(1_543_644_000, 0).unwrap();
    let Some((profiles, catalog, frames)) =
        load_era5_ready_frames(&era5_hybrid_ready_dir(), start, end, "era5-hybrid-m3")
    else {
        return;
    };
    assert_eq!(frames.len(), 3, "00/03/06");
    let sp = frames[0]
        .fields()
        .get(&FieldKey::Canonical(CanonicalField::SurfacePressure))
        .expect("sp");
    assert_eq!(sp.quality(), FieldQuality::Derived);

    let mut engine = engine_from_frames(profiles, catalog, &frames);
    let plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Full,
                ..TransportPlanRequest::default()
            },
            &ExecutionPlan::default(),
        )
        .expect("transport plan");
    // Interior 03 UTC on-frame; between 00/03 for mid-time interpolation.
    let on_frame = Timestamp::new(1_543_633_200, 0).unwrap();
    let mid = Timestamp::new(1_543_627_800, 0).unwrap(); // 01:30 UTC
    for time in [on_frame, mid] {
        let batch = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveGround,
            points: QueryPointArrays {
                longitude_degrees: vec![3.0, 2.0],
                latitude_degrees: vec![50.0, 49.5],
                vertical: vec![10.0, 80.0],
            },
        };
        let window = engine.prepare(time).expect("prepare");
        let mut workspace = BatchWorkspace::default();
        let output = window
            .prepare_transport_batch(&plan, batch, &mut workspace)
            .expect("prepare batch")
            .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
            .expect("execute");
        for index in 0..2 {
            assert_transport_row_complete(&output, index);
        }
    }
}

/// Expanded acceptance matrix for pressure: bounds, mountain/plain, repeat window, provenance IDs.
#[test]
fn era5_pressure_acceptance_matrix() {
    let start = Timestamp::new(1_543_622_400, 0).unwrap();
    let end = Timestamp::new(1_543_665_600, 0).unwrap();
    let Some((profiles, catalog, frames)) = load_era5_ready_frames(
        &era5_pressure_ready_dir(),
        start,
        end,
        "era5-pressure-matrix",
    ) else {
        return;
    };
    // Provenance algorithm IDs on derived NearSurface fields.
    let frame0 = &frames[0];
    let table = frame0.provenance();
    for (field, needle) in [
        (
            CanonicalField::SensibleHeatFlux,
            "trajecta/upward_heat_flux_from_downward/v0",
        ),
        (
            CanonicalField::LatentHeatFlux,
            "trajecta/latent_heat_from_moisture_flux/v0",
        ),
        (
            CanonicalField::TwoMetreSpecificHumidity,
            "trajecta/ifs_q2m_from_dewpoint/v0",
        ),
    ] {
        let raw = frame0
            .fields()
            .get(&FieldKey::Canonical(field))
            .unwrap_or_else(|| panic!("missing {field:?}"));
        let rec = table.get(raw.provenance()).expect("prov");
        assert_eq!(rec.quality, FieldQuality::Derived, "{field:?}");
        assert!(
            rec.transforms.iter().any(|t| {
                t.parameters
                    .iter()
                    .any(|(k, v)| k == "algorithm" && v == needle)
            }),
            "{field:?} missing algorithm {needle}; transforms={:?}",
            rec.transforms
        );
    }

    let mut engine = engine_from_frames(profiles, catalog, &frames);
    let plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Full,
                ..TransportPlanRequest::default()
            },
            &ExecutionPlan::default(),
        )
        .expect("plan");
    let t = Timestamp::new(1_543_644_000, 0).unwrap(); // 06 UTC interior
    let window = engine.prepare(t).expect("prepare");
    let mut workspace = BatchWorkspace::default();

    // Plain / higher-terrain contrast inside the small box (same lon, different lat).
    let mixed = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveGround,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0, 3.0, 1.5, 4.5],
            latitude_degrees: vec![49.0, 51.0, 50.0, 50.0],
            vertical: vec![10.0, 10.0, 50.0, 100.0],
        },
    };
    let out1 = window
        .prepare_transport_batch(&plan, mixed.clone(), &mut workspace)
        .expect("prep1")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("exec1");
    for i in 0..4 {
        assert_transport_row_complete(&out1, i);
    }
    // Repeat same PreparedWindow must be bitwise identical on transport columns.
    let out2 = window
        .prepare_transport_batch(&plan, mixed, &mut workspace)
        .expect("prep2")
        .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
        .expect("exec2");
    assert_transport_outputs_identical(&out1, &out2);

    // Below ground (negative AGL).
    let below = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveGround,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0],
            latitude_degrees: vec![50.0],
            vertical: vec![-50.0],
        },
    };
    let below_out = window
        .prepare_transport_batch(&plan, below, &mut workspace)
        .expect("below prep")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("below exec");
    assert_eq!(
        below_out.row(0).unwrap().status(),
        SampleStatus::BelowGround
    );

    // Far above available top.
    let above = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveSeaLevel,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0],
            latitude_degrees: vec![50.0],
            vertical: vec![80_000.0],
        },
    };
    let above_out = window
        .prepare_transport_batch(&plan, above, &mut workspace)
        .expect("above prep")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("above exec");
    let st = above_out.row(0).unwrap().status();
    assert!(
        matches!(
            st,
            SampleStatus::AboveAvailableTop | SampleStatus::AboveModelTop
        ),
        "expected top bound status, got {st:?}"
    );

    // Endpoint 00 UTC: document MissingSymmetricTimeSupport (not a silent pass).
    let endpoint = Timestamp::new(1_543_622_400, 0).unwrap();
    match engine.prepare(endpoint) {
        Ok(window_ep) => {
            let batch = QueryBatch {
                vertical_coordinate: VerticalQuery::AboveGround,
                points: QueryPointArrays {
                    longitude_degrees: vec![3.0],
                    latitude_degrees: vec![50.0],
                    vertical: vec![10.0],
                },
            };
            let mut ws = BatchWorkspace::default();
            let result = window_ep.prepare_transport_batch(&plan, batch, &mut ws);
            assert!(
                result.is_err(),
                "00 UTC endpoint is expected to lack symmetric time support"
            );
        }
        Err(_) => {
            // prepare itself may fail at endpoint; also acceptable evidence.
        }
    }
}

/// Hybrid matrix: ASL/Pa, lnsp algorithm provenance, repeat window, bounds.
#[test]
fn era5_hybrid137_acceptance_matrix() {
    let start = Timestamp::new(1_543_622_400, 0).unwrap();
    let end = Timestamp::new(1_543_644_000, 0).unwrap();
    let Some((profiles, catalog, frames)) =
        load_era5_ready_frames(&era5_hybrid_ready_dir(), start, end, "era5-hybrid-matrix")
    else {
        return;
    };
    let frame0 = &frames[0];
    let sp = frame0
        .fields()
        .get(&FieldKey::Canonical(CanonicalField::SurfacePressure))
        .expect("sp");
    assert_eq!(sp.quality(), FieldQuality::Derived);
    let rec = frame0.provenance().get(sp.provenance()).expect("sp prov");
    assert!(
        rec.transforms.iter().any(|t| {
            t.parameters
                .iter()
                .any(|(k, v)| k == "algorithm" && v == "trajecta/surface_pressure_from_log/v0")
        }),
        "sp must record surface_pressure_from_log algorithm; got {:?}",
        rec.transforms
    );
    assert!(
        rec.transforms.iter().any(|t| {
            t.parameters
                .iter()
                .any(|(k, v)| k == "source_variable" && v == "lnsp")
        }),
        "sp provenance must trace source_variable=lnsp; got {:?}",
        rec.transforms
    );
    assert!(
        rec.sources
            .iter()
            .any(|s| s.contains("lnsp") || s.contains("variable=lnsp")),
        "sp sources should mention lnsp: {:?}",
        rec.sources
    );

    let mut engine = engine_from_frames(profiles, catalog, &frames);
    let plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Full,
                ..TransportPlanRequest::default()
            },
            &ExecutionPlan::default(),
        )
        .expect("plan");
    let t = Timestamp::new(1_543_633_200, 0).unwrap(); // 03 UTC
    let window = engine.prepare(t).expect("prepare");
    let mut workspace = BatchWorkspace::default();

    let asl = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveSeaLevel,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0, 2.0],
            latitude_degrees: vec![50.0, 49.5],
            vertical: vec![1_500.0, 3_000.0],
        },
    };
    let asl_out = window
        .prepare_transport_batch(&plan, asl.clone(), &mut workspace)
        .expect("asl")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("asl exec");
    for i in 0..2 {
        assert_transport_row_complete(&asl_out, i);
    }
    let asl_repeat = window
        .prepare_transport_batch(&plan, asl, &mut workspace)
        .expect("asl2")
        .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
        .expect("asl2 exec");
    assert_transport_outputs_identical(&asl_out, &asl_repeat);

    let pa = QueryBatch {
        vertical_coordinate: VerticalQuery::Pressure,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0],
            latitude_degrees: vec![50.0],
            vertical: vec![70_000.0],
        },
    };
    let pa_out = window
        .prepare_transport_batch(&plan, pa, &mut workspace)
        .expect("pa")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("pa exec");
    assert_transport_row_complete(&pa_out, 0);

    let below = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveGround,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0],
            latitude_degrees: vec![50.0],
            vertical: vec![-20.0],
        },
    };
    let below_out = window
        .prepare_transport_batch(&plan, below, &mut workspace)
        .expect("below")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("below exec");
    assert_eq!(
        below_out.row(0).unwrap().status(),
        SampleStatus::BelowGround
    );

    let above = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveSeaLevel,
        points: QueryPointArrays {
            longitude_degrees: vec![3.0],
            latitude_degrees: vec![50.0],
            vertical: vec![80_000.0],
        },
    };
    let above_out = window
        .prepare_transport_batch(&plan, above, &mut workspace)
        .expect("above")
        .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
        .expect("above exec");
    let st = above_out.row(0).unwrap().status();
    assert!(
        matches!(
            st,
            SampleStatus::AboveAvailableTop | SampleStatus::AboveModelTop
        ),
        "expected top bound, got {st:?}"
    );
}

/// Automated CLI probe+replay against ready anchors (in-box point).
/// Builds `trajecta-cli` when missing so clean checkouts still execute.
#[test]
fn era5_cli_probe_and_replay_in_box() {
    use std::process::Command;
    let pressure_root = era5_pressure_ready_dir();
    let hybrid_root = era5_hybrid_ready_dir();
    if !pressure_root.join("era5_pressure_20181201.nc").is_file()
        || !hybrid_root
            .join("era5_hybrid137_prepared_20181201.nc")
            .is_file()
    {
        skip_or_fail("era5 ready fixtures");
        return;
    }

    let cli = ensure_trajecta_cli_binary();

    let tmp = tempfile::tempdir().unwrap();
    let points = tmp.path().join("points.jsonl");
    std::fs::write(
        &points,
        r#"{"id":"p0","time_unix":1543644000,"longitude_degrees":3.0,"latitude_degrees":50.0,"vertical":10.0,"vertical_coordinate":"above_ground"}
"#,
    )
    .unwrap();
    let points_hy = tmp.path().join("points_hy.jsonl");
    std::fs::write(
        &points_hy,
        r#"{"id":"p0","time_unix":1543633200,"longitude_degrees":3.0,"latitude_degrees":50.0,"vertical":10.0,"vertical_coordinate":"above_ground"}
"#,
    )
    .unwrap();

    // Pressure probe
    let out = Command::new(&cli)
        .args([
            "met",
            "probe",
            "--data-root",
            pressure_root.to_str().unwrap(),
            "--profile",
            "era5-cf-pressure-netcdf-v0",
            "--time",
            "1543644000",
            "--coverage-start",
            "1543622400",
            "--coverage-end",
            "1543665600",
            "--points",
            points.to_str().unwrap(),
        ])
        .output()
        .expect("run probe");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "probe failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        stdout
    );
    assert!(
        stdout.contains("\"status\":\"ok\""),
        "probe stdout: {stdout}"
    );

    // Pressure replay
    let out = Command::new(&cli)
        .args([
            "met",
            "replay",
            "--data-root",
            pressure_root.to_str().unwrap(),
            "--profile",
            "era5-cf-pressure-netcdf-v0",
            "--coverage-start",
            "1543622400",
            "--coverage-end",
            "1543665600",
            "--input",
            points.to_str().unwrap(),
        ])
        .output()
        .expect("run pressure replay");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "pressure replay failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        stdout
    );
    assert!(
        stdout.contains("\"status\":\"ok\""),
        "pressure replay: {stdout}"
    );

    // Hybrid probe
    let out = Command::new(&cli)
        .args([
            "met",
            "probe",
            "--data-root",
            hybrid_root.to_str().unwrap(),
            "--profile",
            "era5-cds-hybrid137-v0",
            "--time",
            "1543633200",
            "--coverage-start",
            "1543622400",
            "--coverage-end",
            "1543644000",
            "--points",
            points_hy.to_str().unwrap(),
        ])
        .output()
        .expect("run hybrid probe");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "hybrid probe failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        stdout
    );
    assert!(
        stdout.contains("\"status\":\"ok\""),
        "hybrid probe: {stdout}"
    );

    // Hybrid replay (previously missing)
    let out = Command::new(&cli)
        .args([
            "met",
            "replay",
            "--data-root",
            hybrid_root.to_str().unwrap(),
            "--profile",
            "era5-cds-hybrid137-v0",
            "--coverage-start",
            "1543622400",
            "--coverage-end",
            "1543644000",
            "--input",
            points_hy.to_str().unwrap(),
        ])
        .output()
        .expect("run hybrid replay");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "hybrid replay failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        stdout
    );
    assert!(
        stdout.contains("\"status\":\"ok\""),
        "hybrid replay: {stdout}"
    );
}

fn ensure_trajecta_cli_binary() -> PathBuf {
    use std::process::Command;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        })
        .unwrap_or_else(|| workspace.join("target"));
    let executable = format!("trajecta-cli{}", std::env::consts::EXE_SUFFIX);
    let candidates = [
        target_dir.join("debug").join(&executable),
        target_dir.join("release").join(&executable),
    ];
    if let Some(existing) = candidates.into_iter().find(|p| p.is_file()) {
        return existing;
    }
    let status = Command::new("cargo")
        .args(["build", "--offline", "-p", "trajecta-cli"])
        .current_dir(&workspace)
        .status()
        .expect("spawn cargo build -p trajecta-cli");
    assert!(
        status.success(),
        "failed to build trajecta-cli (exit {status})"
    );
    let built = target_dir.join("debug").join(executable);
    if built.is_file() {
        built
    } else {
        panic!("trajecta-cli binary missing after cargo build");
    }
}
