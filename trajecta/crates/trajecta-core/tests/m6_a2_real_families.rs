//! M6-A2 production-run matrix over the three frozen meteorology families.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;
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
    ModelId, PhysicsModuleId, PhysicsPreset, PhysicsSelectionSpec, ResolvedPhysicsSpec,
    resolve_physics,
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
    GLOBAL_PERIODIC_ID, LIMITED_DOMAIN_TERMINATE_ID, MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID,
    SURFACE_REFLECT_ID,
};
use trajecta_core::verification::{VerificationMode, verify_run_directory};
use trajecta_met::auxiliary::gmted2010::{GMTED2010_DATASET_ID, build_global_dataset_lock};
use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::ProfileCatalog;

const PARTICLES: u64 = 256;
const DURATION_SECONDS: i64 = 300;

struct CellData<'a> {
    fixture: &'a Path,
    lock: &'a Path,
    gmted_prepared: &'a Path,
    gmted_lock: &'a Path,
    root: &'a Path,
}

#[derive(Clone, Copy)]
struct Family {
    id: &'static str,
    profile: &'static str,
    fixture: &'static str,
    first_anchor: i64,
    last_anchor: i64,
    forward_start: i64,
    backward_start: i64,
    periodic: bool,
    release: [f64; 2],
}

const FAMILIES: [Family; 3] = [
    Family {
        id: "era5-pressure",
        profile: "era5-cf-pressure-netcdf-v0",
        fixture: "target/m5-a4/fixtures/era5-pressure-4frame/ready",
        first_anchor: 1_543_622_400,
        last_anchor: 1_543_687_200,
        forward_start: 1_543_644_000,
        backward_start: 1_543_665_600,
        periodic: false,
        release: [5.0, 49.0],
    },
    Family {
        id: "era5-hybrid",
        profile: "era5-cds-hybrid137-v0",
        fixture: "target/m5-a4/fixtures/era5-hybrid-4frame/ready",
        first_anchor: 1_543_622_400,
        last_anchor: 1_543_654_800,
        forward_start: 1_543_633_200,
        backward_start: 1_543_644_000,
        periodic: false,
        release: [5.0, 49.0],
    },
    Family {
        id: "cfsr-pressure",
        profile: "cfsr-pgbl-pressure-v0",
        fixture: "target/m5-a4/fixtures/cfsr-pressure-4frame",
        first_anchor: 1_230_768_000,
        last_anchor: 1_230_832_800,
        forward_start: 1_230_789_600,
        backward_start: 1_230_811_200,
        periodic: true,
        release: [0.0, 0.0],
    },
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn formal_required() -> bool {
    std::env::var("TRAJECTA_REQUIRE_M6_A2_FORMAL")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn metres(value: f64) -> Quantity<Length> {
    Quantity::from_si(value, Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap()).unwrap()
}

fn seconds(value: f64) -> Quantity<Time> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn kilograms(value: f64) -> Quantity<Mass> {
    Quantity::from_si(value, Unit::new("kg", Dimension::Mass, 1.0, 0.0).unwrap()).unwrap()
}

fn physics() -> ResolvedPhysicsSpec {
    resolve_physics(
        &PhysicsSelectionSpec {
            preset: PhysicsPreset::WaterVaporTracking,
            remove: vec![
                PhysicsModuleId::DeepConvectionColumn,
                PhysicsModuleId::WaterVaporExchange,
            ],
            overrides: Vec::new(),
            modules: Vec::new(),
        },
        DiagnosticPath::root().field("physics"),
    )
    .unwrap()
}

fn build_gmted_lock(prepared: &Path, root: &Path) -> PathBuf {
    let data_roots = BTreeMap::from([(DataRootId("gmted".into()), prepared.to_path_buf())]);
    let lock = build_global_dataset_lock(
        &data_roots,
        GeneratorInfo {
            tool: "trajecta-core-m6-a2-test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    )
    .unwrap();
    let path = root.join("gmted2010.lock.json");
    fs::write(&path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
    path
}

fn case(family: Family, direction: Direction) -> ResolvedCase {
    let start_seconds = match direction {
        Direction::Forward => family.forward_start,
        Direction::Backward => family.backward_start,
    };
    let end_seconds = start_seconds
        + match direction {
            Direction::Forward => DURATION_SECONDS,
            Direction::Backward => -DURATION_SECONDS,
        };
    let start = Timestamp::new(start_seconds, 0).unwrap();
    let domain = DomainId(if family.periodic { "global" } else { "limited" }.into());
    ResolvedCase {
        metadata: Metadata {
            name: format!("m6-a2-{}-{direction:?}", family.id).to_lowercase(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start,
            end: Timestamp::new(end_seconds, 0).unwrap(),
            direction,
        }),
        meteorology: Some(MeteorologySpec {
            domains: vec![DomainSpec {
                id: domain,
                dataset: DatasetRef(family.id.into()),
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
                particle_count: PARTICLES,
                mass: BTreeMap::from([(SubstanceId("water".into()), kilograms(1.0))]),
                geometry: GeoJsonSource::Inline {
                    geometry: GeoJsonGeometry::Point(family.release),
                },
                vertical: ReleaseVerticalSpec::AboveSeaLevel {
                    lower: metres(1_000.0),
                    upper: None,
                },
            }],
        })),
        substances: vec![SubstanceSpec::WaterVapor {
            id: SubstanceId("water".into()),
            display_name: "Water vapour".into(),
        }],
        numerics: Some(NumericsSpec {
            time_step: seconds(DURATION_SECONDS as f64),
            integrator: IntegratorSpec {
                model: ModelId(RK2_SPHERICAL_ID.into()),
                parameters: BTreeMap::new(),
            },
            boundaries: BoundarySpec {
                policies: vec![
                    ModelId(SURFACE_REFLECT_ID.into()),
                    ModelId(MODEL_TOP_TERMINATE_ID.into()),
                    ModelId(
                        if family.periodic {
                            GLOBAL_PERIODIC_ID
                        } else {
                            LIMITED_DOMAIN_TERMINATE_ID
                        }
                        .into(),
                    ),
                ],
            },
            random_seed: Some(6_202),
        }),
        physics: Some(physics()),
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn build_lock(family: Family, fixture: &Path, root: &Path) -> PathBuf {
    let profiles = ProfileCatalog::load(&[]).unwrap();
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hashes = FileHashCache::new();
    let outcome =
        DatasetLockBuilder::new(&profiles, &inspector, &mut hashes).build(&DatasetLockRequest {
            identity: DatasetIdentity {
                id: DatasetRef(family.id.into()),
                source: format!("M6-A2 frozen {} fixture", family.id),
                source_url: None,
                attribution: None,
            },
            generator: GeneratorInfo {
                tool: "trajecta-core-m6-a2-test".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            data_roots: BTreeMap::from([(DataRootId("met".into()), fixture.to_path_buf())]),
            coverage: LockCoverageRequest {
                start: Timestamp::new(family.first_anchor, 0).unwrap(),
                end: Timestamp::new(family.last_anchor, 0).unwrap(),
                interpolation_before_frames: 0,
                interpolation_after_frames: 0,
            },
            required_capabilities: CapabilitySet::new()
                .with(Capability::Transport)
                .with(Capability::NearSurfaceTransport),
            force_rehash: true,
            preferred_profile: Some(family.profile.into()),
        });
    assert!(
        outcome.is_success(),
        "{}: {:?}",
        family.id,
        outcome.diagnostics
    );
    let lock = outcome.lock.unwrap();
    assert_eq!(lock.profile.name, family.profile);
    let path = root.join(format!("{}.lock.json", family.id));
    fs::write(&path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
    path
}

fn run_cell(
    family: Family,
    direction: Direction,
    workers: usize,
    data: &CellData<'_>,
) -> [String; 3] {
    let resolved_case = case(family, direction);
    let cell = format!("{}-{direction:?}-w{workers}", family.id).to_lowercase();
    let case_path = data.root.join(format!("{cell}.case.json"));
    fs::write(
        &case_path,
        serde_json::to_vec_pretty(&resolved_case).unwrap(),
    )
    .unwrap();
    let output_root = data.root.join(format!("{cell}.runs"));
    let profile = ResolvedRunProfile {
        metadata: Metadata {
            name: cell,
            ..Metadata::default()
        },
        case_path,
        output_root: output_root.clone(),
        datasets: vec![
            DatasetBinding {
                dataset: DatasetRef(family.id.into()),
                lockfile: data.lock.to_path_buf(),
                cache_root: None,
                data_roots: BTreeMap::from([(
                    DataRootId("met".into()),
                    data.fixture.to_path_buf(),
                )]),
                reader_backend: Some(MeteorologyReaderBackend::Rust),
            },
            DatasetBinding {
                dataset: DatasetRef(GMTED2010_DATASET_ID.into()),
                lockfile: data.gmted_lock.to_path_buf(),
                cache_root: None,
                data_roots: BTreeMap::from([(
                    DataRootId("gmted".into()),
                    data.gmted_prepared.to_path_buf(),
                )]),
                reader_backend: None,
            },
        ],
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: workers,
            memory_budget_bytes: 512 * 1024 * 1024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
        },
        sources: Vec::new(),
    };
    let mut runner = build_runner(resolved_case, profile, None).unwrap();
    assert_eq!(runner.run().unwrap(), RunOutcome::Complete);
    let manifest = runner.manifest();
    assert_eq!(manifest.terminations.abnormal_count, 0, "{}", family.id);
    let provenance = manifest.provenance.as_ref().unwrap();
    let sqlite = find_file(&output_root, "particles.sqlite").unwrap();
    assert!(
        verify_run_directory(sqlite.parent().unwrap(), VerificationMode::Full)
            .unwrap()
            .run_success
    );
    let connection = ParticleStateSqliteSink::open_readonly(&sqlite).unwrap();
    let modules: Vec<String> = connection
        .prepare("SELECT module_id FROM process_summary ORDER BY module_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        modules,
        [
            "boundary_layer_langevin",
            "mesoscale_markov",
            "subgrid_orography"
        ]
    );
    assert!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM particle_state
                 WHERE boundary_layer_random_eastward_m_s IS NOT NULL
                   AND boundary_layer_random_northward_m_s IS NOT NULL
                   AND boundary_layer_random_vertical_m_s IS NOT NULL
                   AND mesoscale_random_eastward_m_s IS NOT NULL
                   AND mesoscale_random_northward_m_s IS NOT NULL
                   AND mesoscale_random_vertical_m_s IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap()
            > 0
    );
    [
        provenance.content_sha256.clone(),
        provenance.sqlite_sql_sha256.clone(),
        provenance.canonical_output_sha256.clone(),
    ]
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

#[test]
fn all_real_families_are_directional_and_worker_deterministic() {
    let workspace = workspace_root();
    let gmted_prepared = workspace.join("target/m6-a2-gmted/prepared");
    let mut missing = FAMILIES
        .iter()
        .map(|family| workspace.join(family.fixture))
        .filter(|path| !path.is_dir())
        .collect::<Vec<_>>();
    if !gmted_prepared.is_dir() {
        missing.push(gmted_prepared.clone());
    }
    if !missing.is_empty() {
        assert!(!formal_required(), "missing formal fixtures: {missing:?}");
        eprintln!("skip: M6-A2 real-family fixtures are unavailable: {missing:?}");
        return;
    }

    let work = tempdir().unwrap();
    let gmted_lock = build_gmted_lock(&gmted_prepared, work.path());
    for family in FAMILIES {
        let fixture = workspace.join(family.fixture);
        let lock = build_lock(family, &fixture, work.path());
        let data = CellData {
            fixture: &fixture,
            lock: &lock,
            gmted_prepared: &gmted_prepared,
            gmted_lock: &gmted_lock,
            root: work.path(),
        };
        for direction in [Direction::Forward, Direction::Backward] {
            let serial = run_cell(family, direction, 1, &data);
            let parallel = run_cell(family, direction, 4, &data);
            assert_eq!(serial, parallel, "{} {direction:?}", family.id);
        }
    }
}
