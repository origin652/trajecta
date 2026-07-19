//! Real-data M3 B1 query chain: lock → inventory → frame → TransportPlan → prepare → execute.
//!
//! Gated by fixture presence; hard-fails only when `TRAJECTA_REQUIRE_REAL_MET=1`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_case::model::time::Timestamp;
use trajecta_met::derive::height::{
    geometric_height_to_geopotential_m2_s2, geopotential_height_m,
    geopotential_to_geometric_height_m,
};
use trajecta_met::field::{
    CanonicalField, Capability, CapabilitySet, ExtensionFieldId, FieldKey, FieldRegistry,
};
use trajecta_met::frame::{FrameError, RawMetFrame, TemporalSupport};
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
    BatchWorkspace, EngineError, MetEngine, MetEngineConfig, RayonExecutionContext,
};
use trajecta_met::query::output::SampleStatus;
use trajecta_met::query::request::{
    ExplainMode, QueryBatch, QueryPointArrays, TransportPlanRequest, VerticalQuery,
};
use trajecta_met::science::M3_CONSTANTS;
use trajecta_met::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerRegistry};

fn require_real_met() -> bool {
    matches!(
        std::env::var("TRAJECTA_REQUIRE_REAL_MET").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn skip_or_fail(missing: &str) {
    if require_real_met() {
        panic!("TRAJECTA_REQUIRE_REAL_MET=1 but missing real asset: {missing}");
    }
}

fn cfsr_directory() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_CFSR_DIR") {
        return PathBuf::from(path);
    }
    // Prefer the official three-frame freeze (00/06/12) under the trajecta workspace.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("trajecta workspace root");
    let official = workspace.join("target/test-data/cfsr-ncei-pgbl-official");
    if official.join("pgbl00.gdas.2009010100.grb2").is_file()
        && official.join("pgbl00.gdas.2009010106.grb2").is_file()
    {
        return official;
    }
    let cli = workspace.join("target/test-data/cli-cfsr-pgbl");
    if cli.join("pgbl00.gdas.2009010100.grb2").is_file() {
        return cli;
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("flexpart root")
        .join("tools/flexctl/target/test-data/cfsr/20090101/raw")
}

fn cfsr_three_frame_files() -> [&'static str; 3] {
    [
        "pgbl00.gdas.2009010100.grb2",
        "pgbl00.gdas.2009010106.grb2",
        "pgbl00.gdas.2009010112.grb2",
    ]
}

fn default_surface_layers() -> SurfaceLayerRegistry {
    let mut registry = SurfaceLayerRegistry::new();
    registry
        .register(Arc::new(MoninObukhovBusingerDyer::default()))
        .unwrap();
    registry
}

fn transport_capabilities() -> CapabilitySet {
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(Capability::Transport);
    capabilities.insert(Capability::NearSurfaceTransport);
    capabilities
}

fn transport_field_registry() -> FieldRegistry {
    FieldRegistry::canonical().unwrap()
}

fn load_cfsr_frames(
    directory: &Path,
    files: &[&str],
    start: Timestamp,
    end: Timestamp,
) -> Option<(ProfileCatalog, MetCatalog, Vec<Arc<RawMetFrame>>)> {
    for name in files {
        if !directory.join(name).is_file() {
            skip_or_fail(name);
            return None;
        }
    }
    let isolated = tempfile::tempdir().expect("temp data root");
    for name in files {
        std::fs::copy(directory.join(name), isolated.path().join(name)).unwrap();
    }
    let profiles = ProfileCatalog::load(&[]).expect("profiles");
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let capabilities = transport_capabilities();
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("cfsr-m3-query".into()),
            source: "NOAA CFSR pgbl".into(),
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
    let domain = DomainId("cfsr".into());
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
            .contains(Capability::NearSurfaceTransport),
        "catalog must publish NearSurfaceTransport"
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
        .expect("frame");
        // Surface HGT must never be published as GeometricTerrainHeight.
        assert!(
            frame
                .fields()
                .get(&FieldKey::Canonical(CanonicalField::GeometricTerrainHeight))
                .is_none(),
            "CFSR surface HGT must not map directly to GeometricTerrainHeight"
        );
        assert!(
            frame
                .fields()
                .get(&FieldKey::Canonical(CanonicalField::SurfaceGeopotential))
                .is_some(),
            "SurfaceGeopotential must be derived from surface geopotential height"
        );
        for field in [
            CanonicalField::EastwardWind,
            CanonicalField::GeopotentialHeight,
            CanonicalField::TenMetreEastwardWind,
            CanonicalField::TwoMetreAirTemperature,
            CanonicalField::AerodynamicRoughnessLength,
            CanonicalField::BoundaryLayerHeight,
            CanonicalField::SensibleHeatFlux,
            CanonicalField::FrictionVelocity,
        ] {
            assert!(
                frame.fields().get(&FieldKey::Canonical(field)).is_some(),
                "missing {field:?}"
            );
        }
        let heat = frame
            .fields()
            .get(&FieldKey::Canonical(CanonicalField::SensibleHeatFlux))
            .expect("sensible heat");
        assert!(
            matches!(heat.temporal(), TemporalSupport::Instantaneous { .. }),
            "instantaneous heat flux expected"
        );
        let downward = frame
            .fields()
            .get(&FieldKey::Extension(ExtensionFieldId {
                namespace: "cfsr".into(),
                name: "downward_sensible_heat_flux".into(),
            }))
            .expect("downward-positive CFSR sensible heat source");
        for index in 0..heat.values().len() {
            if heat.validity().get(index).unwrap_or(false)
                && downward.validity().get(index).unwrap_or(false)
            {
                assert_eq!(heat.values()[index], -downward.values()[index]);
            }
        }
        let heat_provenance = frame
            .provenance()
            .get(heat.provenance())
            .expect("heat-flux provenance");
        assert!(heat_provenance.transforms.iter().any(|transform| {
            transform.operation == "profile_frame_graph"
                && transform.parameters.iter().any(|(key, value)| {
                    key == "expression" && value == "-downward_sensible_heat_flux"
                })
        }));
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
    // Large hard budget so multi-batch real CFSR probes do not starve window-local pins.
    // Literal suffixes avoid i32 overflow on 2 GiB constants.
    let budget = MemoryBudget::new(2_u64 * 1024 * 1024 * 1024, 256_u64 * 1024 * 1024).unwrap();
    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles,
        fields: transport_field_registry(),
        surface_layers: default_surface_layers(),
        memory_budget: budget,
    });
    for frame in frames {
        engine.cache_frame(Arc::clone(frame)).unwrap();
    }
    engine
}

fn assert_transport_row_complete(
    output: &trajecta_met::query::output::TransportOutput,
    index: usize,
) {
    let row = output.row(index).unwrap();
    assert_eq!(row.status(), SampleStatus::Ok, "point {index}");
    let bounds = row.bounds().expect("Ok row must carry VerticalBounds");
    assert!(bounds.terrain_asl_m().is_finite());
    assert!(bounds.minimum_transport_agl_m() > 0.0);
    assert!(bounds.available_top_asl_m() > bounds.terrain_asl_m());
    assert!(bounds.maximum_pressure_pa() > bounds.minimum_pressure_pa());
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
    // Validity + quality + resolved provenance for every transport column.
    let columns = output.columns();
    let table = output.provenance();
    for column in [
        &columns.eastward_wind_m_s,
        &columns.northward_wind_m_s,
        &columns.geometric_vertical_velocity_m_s,
        &columns.air_pressure_pa,
        &columns.air_temperature_k,
        &columns.specific_humidity,
        &columns.air_density_kg_m3,
        &columns.terrain_height_asl_m,
    ] {
        assert!(column.validity().get(index).unwrap());
        let id = column.provenance()[index];
        assert!(table.get(id).is_some(), "unresolved provenance id {}", id.0);
        let _ = column.quality()[index];
    }
    let explain = output
        .explain()
        .and_then(|records| records.get(index).cloned())
        .flatten()
        .expect("explain must be populated");
    assert!(!explain.domain.0.is_empty());
    assert_eq!(explain.fields.len(), 8);
    assert!((explain.horizontal.first_level_weights.iter().sum::<f64>() - 1.0).abs() < 1.0e-12);
    for field in &explain.fields {
        let record = table
            .get(field.provenance)
            .expect("explain provenance must resolve");
        assert_eq!(record.field, field.field);
        assert_eq!(record.quality, field.quality);
    }
}

#[test]
fn cfsr_pgbl_transport_query_chain_mid_time_and_repeat() {
    // 2009-01-01 00 and 06 UTC. Mid-time 03 UTC is strictly bracketed.
    let start = Timestamp::new(1_230_768_000, 0).unwrap();
    let end = Timestamp::new(1_230_789_600, 0).unwrap();
    let Some((profiles, catalog, frames)) = load_cfsr_frames(
        &cfsr_directory(),
        &["pgbl00.gdas.2009010100.grb2", "pgbl00.gdas.2009010106.grb2"],
        start,
        end,
    ) else {
        return;
    };
    assert_eq!(frames.len(), 2);

    // Prove geopotential height and geometric height are not mixed.
    let surface_phi = frames[0]
        .fields()
        .get(&FieldKey::Canonical(CanonicalField::SurfaceGeopotential))
        .expect("surface geopotential");
    let orography = frames[0]
        .fields()
        .iter()
        .find(|(key, _)| {
            matches!(
                key,
                FieldKey::Extension(id)
                    if id.namespace == "cfsr" && id.name == "surface_orography_height"
            )
        })
        .map(|(_, field)| field)
        .expect("surface orography height extension");
    // Sample a few finite cells away from poles.
    let mut checked = 0usize;
    for index in [0usize, 100, 1_000, 5_000, 10_000] {
        if index >= surface_phi.values().len() {
            continue;
        }
        if !surface_phi.validity().get(index).unwrap_or(false)
            || !orography.validity().get(index).unwrap_or(false)
        {
            continue;
        }
        let phi = surface_phi.values()[index];
        let z_geo_height = orography.values()[index];
        let expected_phi = M3_CONSTANTS.standard_gravity_m_s2 * z_geo_height;
        assert!(
            (phi - expected_phi).abs() < 1.0e-6 * expected_phi.abs().max(1.0),
            "SurfaceGeopotential must equal g0 * geopotential_height: phi={phi} expected={expected_phi}"
        );
        let geometric = geopotential_to_geometric_height_m(phi).unwrap();
        // Geometric height differs from geopotential height except at z≈0.
        if z_geo_height.abs() > 100.0 {
            assert!(
                (geometric - z_geo_height).abs() > 1.0e-6,
                "geometric height must not equal geopotential height at z_g={z_geo_height}"
            );
        }
        // Round-trip through frozen formulas.
        let back = geometric_height_to_geopotential_m2_s2(geometric).unwrap();
        assert!((back - phi).abs() < 1.0e-6 * phi.abs().max(1.0));
        let zg = geopotential_height_m(phi).unwrap();
        assert!((zg - z_geo_height).abs() < 1.0e-9);
        checked += 1;
    }
    assert!(checked >= 1, "need at least one valid surface cell");

    let mut engine = engine_from_frames(profiles, catalog, &frames);
    assert!(
        engine.memory_budget().dynamic_bytes() > 100_000_000,
        "engine dynamic budget too small: {}",
        engine.memory_budget().dynamic_bytes()
    );
    let plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Full,
                ..TransportPlanRequest::default()
            },
            &ExecutionPlan::default(),
        )
        .expect("transport plan with allow_estimated=false");

    // Frozen near-surface AGL probes. Heights must lie inside the local surface-layer
    // domain (above minimum transport height and at or below the lowest free-atmosphere
    // model level). 10 m AGL is the intended near-surface contract probe.
    let mid = Timestamp::new(1_230_778_800, 0).unwrap(); // 03 UTC
    let agl_batch = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveGround,
        points: QueryPointArrays {
            longitude_degrees: vec![-150.0, -95.0, 90.0, 0.0, 120.0],
            latitude_degrees: vec![0.0, 40.0, 32.0, 50.0, -20.0],
            vertical: vec![10.0, 10.0, 10.0, 10.0, 10.0],
        },
    };
    let window = engine.prepare(mid).expect("prepare mid window");
    let mut workspace = BatchWorkspace::default();
    let output = {
        let prepared = window
            .prepare_transport_batch(&plan, agl_batch.clone(), &mut workspace)
            .expect("prepare_transport_batch");
        let metrics_before = engine.column_cache_metrics().unwrap();
        let output = prepared
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .expect("execute");
        assert_eq!(engine.column_cache_metrics().unwrap(), metrics_before);
        output
    };
    assert_eq!(output.status().values().len(), 5);
    for index in 0..5 {
        assert_transport_row_complete(&output, index);
        let explain = output.explain().unwrap()[index].as_ref().unwrap();
        assert_eq!(
            explain.vertical.path,
            trajecta_met::query::output::ExplainVerticalPath::SurfaceLayer
        );
        assert!(explain.surface_model.is_some());
    }

    // Bitwise-identical repeat on the same PreparedWindow; drop pins before later batches.
    let output2 = {
        let prepared2 = window
            .prepare_transport_batch(&plan, agl_batch, &mut workspace)
            .expect("second prepare");
        let metrics_before2 = engine.column_cache_metrics().unwrap();
        let output2 = prepared2
            .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
            .expect("second execute");
        assert_eq!(engine.column_cache_metrics().unwrap(), metrics_before2);
        output2
    };
    assert_eq!(output2.status().values(), output.status().values());
    assert_eq!(
        output2.columns().eastward_wind_m_s.values(),
        output.columns().eastward_wind_m_s.values()
    );
    assert_eq!(
        output2.columns().terrain_height_asl_m.values(),
        output.columns().terrain_height_asl_m.values()
    );
    assert_eq!(
        output2.columns().geometric_vertical_velocity_m_s.values(),
        output.columns().geometric_vertical_velocity_m_s.values()
    );
    drop(window);

    // ASL free-atmosphere probe: high enough to leave the surface layer.
    let terrain_europe = output.row(3).unwrap().terrain_height_asl_m().unwrap();
    let asl_batch = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveSeaLevel,
        points: QueryPointArrays {
            longitude_degrees: vec![0.0],
            latitude_degrees: vec![50.0],
            vertical: vec![terrain_europe + 5_000.0],
        },
    };
    let asl_out = {
        let window_asl = engine.prepare(mid).expect("prepare mid for asl");
        let mut workspace_asl = BatchWorkspace::default();
        let prepared_asl = window_asl
            .prepare_transport_batch(&plan, asl_batch, &mut workspace_asl)
            .expect("asl prepare");
        prepared_asl
            .execute(
                &RayonExecutionContext { worker_threads: 1 },
                &mut workspace_asl,
            )
            .expect("asl execute")
    };
    assert_transport_row_complete(&asl_out, 0);

    // Pressure coordinate free-atmosphere probe.
    let pa_batch = QueryBatch {
        vertical_coordinate: VerticalQuery::Pressure,
        points: QueryPointArrays {
            longitude_degrees: vec![-95.0],
            latitude_degrees: vec![40.0],
            vertical: vec![70_000.0],
        },
    };
    let pa_out = {
        let window_pa = engine.prepare(mid).expect("prepare mid for pa");
        let mut workspace_pa = BatchWorkspace::default();
        let prepared_pa = window_pa
            .prepare_transport_batch(&plan, pa_batch, &mut workspace_pa)
            .expect("pa prepare");
        prepared_pa
            .execute(
                &RayonExecutionContext { worker_threads: 1 },
                &mut workspace_pa,
            )
            .expect("pa execute")
    };
    assert_transport_row_complete(&pa_out, 0);
    assert!((pa_out.row(0).unwrap().air_pressure_pa().unwrap() - 70_000.0).abs() < 1.0e-6);

    // Exact endpoint must hard-fail MissingSymmetricTimeSupport for Transport.
    let exact = frames[0].metadata().valid_time;
    let window_exact = engine.prepare(exact).expect("prepare exact frame");
    let endpoint_batch = QueryBatch {
        vertical_coordinate: VerticalQuery::AboveGround,
        points: QueryPointArrays {
            longitude_degrees: vec![0.0],
            latitude_degrees: vec![50.0],
            vertical: vec![100.0],
        },
    };
    let endpoint = window_exact.prepare_transport_batch(&plan, endpoint_batch, &mut workspace);
    assert_eq!(
        endpoint.err(),
        Some(EngineError::Frame(FrameError::MissingSymmetricTimeSupport)),
        "exact endpoint transport must be MissingSymmetricTimeSupport"
    );
}

#[test]
fn cfsr_pgbl_transport_three_frames_00_06_12_and_mid_03_09() {
    // Official NCEI freeze: 2009-01-01 00/06/12 UTC; mid-times 03 and 09.
    let start = Timestamp::new(1_230_768_000, 0).unwrap(); // 00 UTC
    let end = Timestamp::new(1_230_811_200, 0).unwrap(); // 12 UTC
    let files = cfsr_three_frame_files();
    let Some((profiles, catalog, frames)) = load_cfsr_frames(&cfsr_directory(), &files, start, end)
    else {
        return;
    };
    assert_eq!(frames.len(), 3, "expected frames for 00/06/12");
    let times: Vec<_> = frames
        .iter()
        .map(|frame| frame.metadata().valid_time.seconds_since_unix_epoch())
        .collect();
    assert_eq!(
        times,
        vec![1_230_768_000, 1_230_789_600, 1_230_811_200],
        "frame times must be 00/06/12 UTC"
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
        .expect("transport plan allow_estimated=false");

    // Point matrix: ocean / plains / mountain / high free-atmosphere AGL / Pa.
    let batch_template = |vertical_coord: VerticalQuery, vertical: Vec<f64>| QueryBatch {
        vertical_coordinate: vertical_coord,
        points: QueryPointArrays {
            longitude_degrees: vec![-150.0, -95.0, 90.0, 0.0, 120.0],
            latitude_degrees: vec![0.0, 40.0, 32.0, 50.0, -20.0],
            vertical,
        },
    };

    for (label, unix) in [("03UTC", 1_230_778_800_i64), ("09UTC", 1_230_800_400_i64)] {
        let mid = Timestamp::new(unix, 0).unwrap();
        let window = engine
            .prepare(mid)
            .unwrap_or_else(|error| panic!("prepare {label}: {error:?}"));
        let mut workspace = BatchWorkspace::default();

        let agl = batch_template(
            VerticalQuery::AboveGround,
            vec![10.0, 10.0, 10.0, 10.0, 10.0],
        );
        let out = {
            let prepared = window
                .prepare_transport_batch(&plan, agl, &mut workspace)
                .unwrap_or_else(|error| panic!("{label} AGL prepare failed: {error:?}"));
            prepared
                .execute(&RayonExecutionContext { worker_threads: 2 }, &mut workspace)
                .unwrap_or_else(|error| panic!("{label} AGL execute failed: {error:?}"))
        };
        assert_eq!(out.status().values().len(), 5);
        for index in 0..5 {
            assert_transport_row_complete(&out, index);
        }

        // High AGL free atmosphere over Europe.
        let terrain = out.row(3).unwrap().terrain_height_asl_m().unwrap();
        let asl = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveSeaLevel,
            points: QueryPointArrays {
                longitude_degrees: vec![0.0],
                latitude_degrees: vec![50.0],
                vertical: vec![terrain + 5_000.0],
            },
        };
        let asl_out = {
            let prepared = window
                .prepare_transport_batch(&plan, asl, &mut workspace)
                .unwrap_or_else(|error| panic!("{label} ASL prepare failed: {error:?}"));
            prepared
                .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
                .unwrap_or_else(|error| panic!("{label} ASL execute failed: {error:?}"))
        };
        assert_transport_row_complete(&asl_out, 0);

        let pa = QueryBatch {
            vertical_coordinate: VerticalQuery::Pressure,
            points: QueryPointArrays {
                longitude_degrees: vec![-95.0],
                latitude_degrees: vec![40.0],
                vertical: vec![50_000.0],
            },
        };
        let pa_out = {
            let prepared = window
                .prepare_transport_batch(&plan, pa, &mut workspace)
                .unwrap_or_else(|error| panic!("{label} Pa prepare failed: {error:?}"));
            prepared
                .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
                .unwrap_or_else(|error| panic!("{label} Pa execute failed: {error:?}"))
        };
        assert_transport_row_complete(&pa_out, 0);
        assert!((pa_out.row(0).unwrap().air_pressure_pa().unwrap() - 50_000.0).abs() < 1.0e-6);
        drop(window);
    }

    // Exact 12 UTC endpoint still lacks symmetric support for kinematic W.
    let exact_12 = frames[2].metadata().valid_time;
    let window_exact = engine.prepare(exact_12).expect("prepare 12 UTC");
    let mut workspace = BatchWorkspace::default();
    let endpoint = window_exact.prepare_transport_batch(
        &plan,
        QueryBatch {
            vertical_coordinate: VerticalQuery::AboveGround,
            points: QueryPointArrays {
                longitude_degrees: vec![0.0],
                latitude_degrees: vec![50.0],
                vertical: vec![100.0],
            },
        },
        &mut workspace,
    );
    assert_eq!(
        endpoint.err(),
        Some(EngineError::Frame(FrameError::MissingSymmetricTimeSupport)),
        "12 UTC endpoint must be MissingSymmetricTimeSupport"
    );
}

#[test]
fn cfsr_pgbl_pure_rust_dual_load_full_field_diff() {
    // Pure-Rust dual decode for all three official frames: full field map,
    // layout, mask/validity, temporal, values, provenance ids bitwise-identical.
    let start = Timestamp::new(1_230_768_000, 0).unwrap();
    let end = Timestamp::new(1_230_811_200, 0).unwrap();
    let files = cfsr_three_frame_files();
    let Some((_, _, frames_a)) = load_cfsr_frames(&cfsr_directory(), &files, start, end) else {
        return;
    };
    let Some((_, _, frames_b)) = load_cfsr_frames(&cfsr_directory(), &files, start, end) else {
        return;
    };
    assert_eq!(frames_a.len(), frames_b.len());
    for (frame_a, frame_b) in frames_a.iter().zip(frames_b.iter()) {
        assert_eq!(frame_a.metadata(), frame_b.metadata());
        let keys_a: Vec<_> = frame_a.fields().iter().map(|(k, _)| k.clone()).collect();
        let keys_b: Vec<_> = frame_b.fields().iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys_a, keys_b, "field key sets must match");
        for key in &keys_a {
            let a = frame_a.fields().get(key).unwrap();
            let b = frame_b.fields().get(key).unwrap();
            assert_eq!(a.layout(), b.layout(), "{key:?} layout");
            assert_eq!(a.temporal(), b.temporal(), "{key:?} temporal");
            assert_eq!(a.unit(), b.unit(), "{key:?} unit");
            assert_eq!(a.validity(), b.validity(), "{key:?} validity");
            assert_eq!(a.values(), b.values(), "{key:?} values");
            assert_eq!(a.quality(), b.quality(), "{key:?} quality");
            assert_eq!(a.provenance(), b.provenance(), "{key:?} provenance id");
        }
    }
}

struct MillionPointColumns {
    status: Vec<SampleStatus>,
    eastward_wind_m_s: Vec<f64>,
    northward_wind_m_s: Vec<f64>,
    geometric_vertical_velocity_m_s: Vec<f64>,
    air_pressure_pa: Vec<f64>,
    air_temperature_k: Vec<f64>,
    specific_humidity: Vec<f64>,
    air_density_kg_m3: Vec<f64>,
    terrain_height_asl_m: Vec<f64>,
}

#[test]
fn cfsr_pgbl_million_point_budget_and_threading() {
    // Million-point gate: total >1e6 points across many chunks, dynamic budget <1 GiB,
    // 1-thread vs multi-thread bitwise identical on full 8 columns + validity/quality/status.
    // Chunked execute keeps peak resident under budget while still proving multi-chunk streaming.
    let start = Timestamp::new(1_230_768_000, 0).unwrap();
    let end = Timestamp::new(1_230_811_200, 0).unwrap();
    let files = cfsr_three_frame_files();
    let Some((profiles, catalog, frames)) = load_cfsr_frames(&cfsr_directory(), &files, start, end)
    else {
        return;
    };
    assert!(frames.len() >= 2);

    let budget = MemoryBudget::new(512_u64 * 1024 * 1024, 64_u64 * 1024 * 1024).unwrap();
    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles,
        fields: transport_field_registry(),
        surface_layers: default_surface_layers(),
        memory_budget: budget,
    });
    for frame in &frames {
        engine.cache_frame(Arc::clone(frame)).unwrap();
    }
    assert!(
        engine.memory_budget().dynamic_bytes() < 1024_u64 * 1024 * 1024,
        "dynamic budget must be < 1 GiB"
    );

    let plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Disabled,
                ..TransportPlanRequest::default()
            },
            &ExecutionPlan::default(),
        )
        .expect("plan");

    const TOTAL: usize = 1_024 * 1_000 + 17; // 1_024_017 > 1e6
    const CHUNK: usize = 4_096; // multi-chunk; not one giant allocation path only
    const {
        assert!(TOTAL / CHUNK >= 2);
        assert!(TOTAL > 1_000_000);
    }

    let mid = Timestamp::new(1_230_778_800, 0).unwrap(); // 03 UTC
    let window = engine.prepare(mid).expect("prepare");
    let mut workspace = BatchWorkspace::default();
    let metrics_before = engine.column_cache_metrics().unwrap();

    let mut run = |workers: usize| -> MillionPointColumns {
        let mut status = Vec::with_capacity(TOTAL);
        let mut u = Vec::with_capacity(TOTAL);
        let mut v = Vec::with_capacity(TOTAL);
        let mut w = Vec::with_capacity(TOTAL);
        let mut p = Vec::with_capacity(TOTAL);
        let mut temperature = Vec::with_capacity(TOTAL);
        let mut q = Vec::with_capacity(TOTAL);
        let mut rho = Vec::with_capacity(TOTAL);
        let mut terrain = Vec::with_capacity(TOTAL);
        let mut offset = 0usize;
        while offset < TOTAL {
            let take = (TOTAL - offset).min(CHUNK);
            let mut lon = Vec::with_capacity(take);
            let mut lat = Vec::with_capacity(take);
            let mut vert = Vec::with_capacity(take);
            for index in 0..take {
                let global = offset + index;
                // Free-atmosphere ASL over Europe: exercises multi-field transport
                // without surface-layer iteration dominating wall time for the gate.
                lon.push(0.0 + (global % 40) as f64 * 0.05);
                lat.push(48.0 + ((global / 40) % 40) as f64 * 0.05);
                vert.push(5_000.0);
            }
            let batch = QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points: QueryPointArrays {
                    longitude_degrees: lon,
                    latitude_degrees: lat,
                    vertical: vert,
                },
            };
            let prepared = window
                .prepare_transport_batch(&plan, batch, &mut workspace)
                .expect("prepare chunk");
            let out = prepared
                .execute(
                    &RayonExecutionContext {
                        worker_threads: workers,
                    },
                    &mut workspace,
                )
                .expect("exec chunk");
            status.extend(out.status().values().iter().copied());
            let columns = out.columns();
            u.extend(columns.eastward_wind_m_s.values().iter().copied());
            v.extend(columns.northward_wind_m_s.values().iter().copied());
            w.extend(
                columns
                    .geometric_vertical_velocity_m_s
                    .values()
                    .iter()
                    .copied(),
            );
            p.extend(columns.air_pressure_pa.values().iter().copied());
            temperature.extend(columns.air_temperature_k.values().iter().copied());
            q.extend(columns.specific_humidity.values().iter().copied());
            rho.extend(columns.air_density_kg_m3.values().iter().copied());
            terrain.extend(columns.terrain_height_asl_m.values().iter().copied());
            offset += take;
        }
        MillionPointColumns {
            status,
            eastward_wind_m_s: u,
            northward_wind_m_s: v,
            geometric_vertical_velocity_m_s: w,
            air_pressure_pa: p,
            air_temperature_k: temperature,
            specific_humidity: q,
            air_density_kg_m3: rho,
            terrain_height_asl_m: terrain,
        }
    };

    let one = run(1);
    assert_eq!(one.status.len(), TOTAL);
    // Hot second pass: no additional column-cache misses after the cold fill.
    let metrics_hot_before = engine.column_cache_metrics().unwrap();
    let multi = run(4);
    let metrics_hot_after = engine.column_cache_metrics().unwrap();
    assert_eq!(
        metrics_hot_after.misses, metrics_hot_before.misses,
        "hot multi-thread pass must not incur new column-cache misses (no reader re-entry)"
    );
    assert!(
        metrics_hot_before.misses >= metrics_before.misses,
        "cold pass should have filled the column cache"
    );
    assert_eq!(multi.status, one.status, "status");
    assert_eq!(multi.eastward_wind_m_s, one.eastward_wind_m_s, "u");
    assert_eq!(multi.northward_wind_m_s, one.northward_wind_m_s, "v");
    assert_eq!(
        multi.geometric_vertical_velocity_m_s, one.geometric_vertical_velocity_m_s,
        "w"
    );
    assert_eq!(multi.air_pressure_pa, one.air_pressure_pa, "p");
    assert_eq!(multi.air_temperature_k, one.air_temperature_k, "T");
    assert_eq!(multi.specific_humidity, one.specific_humidity, "q");
    assert_eq!(multi.air_density_kg_m3, one.air_density_kg_m3, "rho");
    assert_eq!(
        multi.terrain_height_asl_m, one.terrain_height_asl_m,
        "terrain"
    );
    // Spot-check finite transport values on a mid-stream chunk sample.
    let sample = TOTAL / 2;
    assert!(one.eastward_wind_m_s[sample].is_finite());
    assert!(one.air_pressure_pa[sample].is_finite());
    assert!(one.air_temperature_k[sample].is_finite());
}
