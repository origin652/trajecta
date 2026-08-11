//! Formal M6-A1 production-path gates using the frozen four-frame CFSR fixture.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::{TempDir, tempdir};
use trajecta_case::diagnostic::DiagnosticPath;
use trajecta_case::document::{
    DataRootId, DatasetBinding, ExecutionSpec, MeteorologyReaderBackend, ResolvedCase,
    ResolvedRunProfile,
};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::metadata::Metadata;
use trajecta_case::model::meteorology::{DatasetRef, DomainId, DomainSpec, MeteorologySpec};
use trajecta_case::model::numerics::{BoundarySpec, IntegratorSpec, NumericsSpec};
use trajecta_case::model::output::default_particle_state_output;
use trajecta_case::model::physics::{
    ModelId, PhysicsModuleConfig, PhysicsModuleId, PhysicsPreset, PhysicsSelectionSpec,
    ResolvedPhysicsSpec, resolve_physics,
};
use trajecta_case::model::population::{
    GeoJsonGeometry, GeoJsonSource, ParticlePopulationSpec, PopulationId, ReleaseDrivenSpec,
    ReleaseEventId, ReleaseEventSpec, ReleaseVerticalSpec,
};
use trajecta_case::model::substance::{SubstanceId, SubstanceSpec};
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Length, Mass, Quantity, Time, Unit};
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::runner::{RunOutcome, build_runner};
use trajecta_core::science::{
    GLOBAL_PERIODIC_ID, MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};
use trajecta_core::verification::{VerificationMode, verify_run_directory};
use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::ProfileCatalog;

const T00: i64 = 1_230_768_000;
const START: i64 = 1_230_789_600;
const M5_COMMON_ADVECTION_SHA256: &str =
    "950e16e5bb6a584ddcaa8ac13da54359a9423a616c863f7d76c829e0bad9a277";

fn metres(value: f64) -> Quantity<Length> {
    Quantity::from_si(value, Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap()).unwrap()
}

fn seconds(value: f64) -> Quantity<Time> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn kilograms(value: f64) -> Quantity<Mass> {
    Quantity::from_si(value, Unit::new("kg", Dimension::Mass, 1.0, 0.0).unwrap()).unwrap()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("trajecta workspace")
        .to_path_buf()
}

fn fixture_directory() -> PathBuf {
    std::env::var_os("TRAJECTA_REAL_CFSR_DIR").map_or_else(
        || workspace_root().join("target/m5-a4/fixtures/cfsr-pressure-4frame"),
        PathBuf::from,
    )
}

fn require_formal_fixture() -> bool {
    matches!(
        std::env::var("TRAJECTA_REQUIRE_M6_A1_FORMAL").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn has_four_frames(directory: &Path) -> bool {
    ["00", "06", "12", "18"].into_iter().all(|hour| {
        directory
            .join(format!("pgbl00.gdas.20090101{hour}.grb2"))
            .is_file()
    })
}

fn build_lock(directory: &Path, work: &Path) -> PathBuf {
    let profiles = ProfileCatalog::load(&[]).unwrap();
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hashes = FileHashCache::new();
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("cfsr-pressure".into()),
            source: "NOAA CFSR pgbl M6-A1 frozen fixture".into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-core-m6-a1-test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), directory.to_path_buf())]),
        coverage: LockCoverageRequest {
            start: Timestamp::new(T00, 0).unwrap(),
            end: Timestamp::new(T00 + 18 * 3_600, 0).unwrap(),
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: CapabilitySet::new()
            .with(Capability::Transport)
            .with(Capability::NearSurfaceTransport),
        force_rehash: true,
        preferred_profile: Some("cfsr-pgbl-pressure-v0".into()),
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hashes).build(&request);
    assert!(outcome.is_success(), "{:?}", outcome.diagnostics);
    let path = work.join("cfsr-pressure.lock.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&outcome.lock.unwrap()).unwrap(),
    )
    .unwrap();
    path
}

fn boundary_layer_physics() -> ResolvedPhysicsSpec {
    resolve_physics(
        &PhysicsSelectionSpec {
            preset: PhysicsPreset::WaterVaporTracking,
            remove: vec![
                PhysicsModuleId::SubgridOrography,
                PhysicsModuleId::MesoscaleMarkov,
                PhysicsModuleId::DeepConvectionColumn,
                PhysicsModuleId::WaterVaporExchange,
            ],
            overrides: vec![PhysicsModuleConfig {
                model: PhysicsModuleId::BoundaryLayerLangevin,
                order: None,
                maximum_substep: Some(seconds(30.0)),
                correlation_interval_fraction: None,
            }],
            modules: Vec::new(),
        },
        DiagnosticPath::root().field("physics"),
    )
    .unwrap()
}

fn case(
    name: &str,
    particles: u64,
    height_m: f64,
    physics: Option<ResolvedPhysicsSpec>,
) -> ResolvedCase {
    let start = Timestamp::new(START, 0).unwrap();
    ResolvedCase {
        metadata: Metadata {
            name: name.into(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start,
            end: Timestamp::new(START + 600, 0).unwrap(),
            direction: Direction::Forward,
        }),
        meteorology: Some(MeteorologySpec {
            domains: vec![DomainSpec {
                id: DomainId("global".into()),
                dataset: DatasetRef("cfsr-pressure".into()),
                priority: 1,
                parent: None,
                horizontal_halo_cells: 1,
            }],
        }),
        particle_population: Some(ParticlePopulationSpec::ReleaseDriven(ReleaseDrivenSpec {
            id: PopulationId("release".into()),
            events: vec![ReleaseEventSpec {
                id: ReleaseEventId("event".into()),
                start,
                end: start,
                particle_count: particles,
                mass: BTreeMap::from([(SubstanceId("tracer".into()), kilograms(1.0))]),
                geometry: GeoJsonSource::Inline {
                    geometry: GeoJsonGeometry::Point([0.0, 0.0]),
                },
                vertical: ReleaseVerticalSpec::AboveSeaLevel {
                    lower: metres(height_m),
                    upper: None,
                },
            }],
        })),
        substances: vec![SubstanceSpec::WaterVapor {
            id: SubstanceId("tracer".into()),
            display_name: "Tracer".into(),
        }],
        numerics: Some(NumericsSpec {
            time_step: seconds(300.0),
            integrator: IntegratorSpec {
                model: ModelId(RK2_SPHERICAL_ID.into()),
                parameters: BTreeMap::new(),
            },
            boundaries: BoundarySpec {
                policies: vec![
                    ModelId(SURFACE_REFLECT_ID.into()),
                    ModelId(MODEL_TOP_TERMINATE_ID.into()),
                    ModelId(GLOBAL_PERIODIC_ID.into()),
                ],
            },
            random_seed: Some(4_201),
        }),
        physics,
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn run(root: &TempDir, fixture: &Path, lock: &Path, case: ResolvedCase) -> PathBuf {
    let case_path = root.path().join(format!("{}.json", case.metadata.name));
    fs::write(&case_path, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
    let output_root = root.path().join(format!("runs-{}", case.metadata.name));
    let profile = ResolvedRunProfile {
        metadata: Metadata {
            name: "m6-a1-cfsr".into(),
            ..Metadata::default()
        },
        case_path,
        output_root: output_root.clone(),
        datasets: vec![DatasetBinding {
            dataset: DatasetRef("cfsr-pressure".into()),
            lockfile: lock.to_path_buf(),
            cache_root: None,
            data_roots: BTreeMap::from([(DataRootId("met".into()), fixture.to_path_buf())]),
            reader_backend: Some(MeteorologyReaderBackend::Rust),
        }],
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: 1,
            memory_budget_bytes: 512 * 1024 * 1024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
        },
        sources: Vec::new(),
    };
    let mut runner = build_runner(case, profile, None).unwrap();
    assert_eq!(runner.run().unwrap(), RunOutcome::Complete);
    find_file(&output_root, "particles.sqlite").expect("particles.sqlite")
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    if root.is_file() {
        return (root.file_name()?.to_str()? == name).then(|| root.to_path_buf());
    }
    for entry in fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if let Some(found) = find_file(&path, name) {
            return Some(found);
        }
    }
    None
}

fn frozen_m5_sqlite() -> PathBuf {
    workspace_root().join(
        "target/m5-a4/a-review/cfsr-input-identity-closeout-attempt-1/cells/\
windows-x86_64__cfsr-pressure__release__forward__rust__p1000__w1/attempt-1/project/runs/\
m5-a4-cfsr-pressure-release-forward/019faf56-9d90-7340-a9b5-1a5992cb12c7/particles.sqlite",
    )
}

#[test]
fn cfsr_production_boundary_layer_and_m5_advection_continuity() {
    let fixture = fixture_directory();
    if !has_four_frames(&fixture) {
        assert!(
            !require_formal_fixture(),
            "M6-A1 formal CFSR fixture is missing under {}",
            fixture.display()
        );
        eprintln!("skip: M6-A1 four-frame CFSR fixture missing");
        return;
    }

    let frozen = frozen_m5_sqlite();
    if frozen.is_file() {
        let frozen = fs::canonicalize(frozen).unwrap();
        assert_eq!(
            ParticleStateSqliteSink::common_advection_projection_sha256(&frozen).unwrap(),
            M5_COMMON_ADVECTION_SHA256
        );
    } else {
        assert!(
            !require_formal_fixture(),
            "frozen M5 v1 SQLite baseline is missing"
        );
    }

    let work = tempdir().unwrap();
    let lock = build_lock(&fixture, work.path());

    let advection_sqlite = run(
        &work,
        &fixture,
        &lock,
        case("m6-a1-cfsr-advection", 1_000, 1_000.0, None),
    );
    assert!(
        verify_run_directory(advection_sqlite.parent().unwrap(), VerificationMode::Full)
            .unwrap()
            .run_success
    );
    assert_eq!(
        ParticleStateSqliteSink::common_advection_projection_sha256(&advection_sqlite).unwrap(),
        M5_COMMON_ADVECTION_SHA256
    );

    let boundary_layer_sqlite = run(
        &work,
        &fixture,
        &lock,
        case(
            "m6-a1-cfsr-boundary-layer",
            32,
            100.0,
            Some(boundary_layer_physics()),
        ),
    );
    assert!(
        verify_run_directory(
            boundary_layer_sqlite.parent().unwrap(),
            VerificationMode::Full
        )
        .unwrap()
        .run_success
    );
    let connection = ParticleStateSqliteSink::open_readonly(&boundary_layer_sqlite).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM process_summary", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        1
    );
    assert!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM particle_state
                 WHERE boundary_layer_random_eastward_m_s IS NOT NULL
                   AND boundary_layer_random_northward_m_s IS NOT NULL
                   AND boundary_layer_random_vertical_m_s IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap()
            > 0
    );
}
