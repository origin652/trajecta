//! Explicit M4-A3 real-data matrix for PV60 stratospheric-ozone filling.
//!
//! The test is ignored by default because it loads all three frozen datasets.
//! Run it explicitly with `--ignored`; missing fixtures are hard failures.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use tempfile::tempdir;
use trajecta_case::document::{
    DataRootId, DatasetBinding, ExecutionSpec, MeteorologyReaderBackend, ResolvedCase,
    ResolvedRunProfile,
};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::metadata::Metadata;
use trajecta_case::model::meteorology::{DatasetRef, DomainId, DomainSpec, MeteorologySpec};
use trajecta_case::model::numerics::{BoundarySpec, IntegratorSpec, NumericsSpec};
use trajecta_case::model::output::default_particle_state_output;
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, ParticlePopulationSpec, PopulationId,
};
use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Quantity, Time as TimeDimension, Unit};
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::population::PopulationState;
use trajecta_core::runner::{RunOutcome, build_runner};
use trajecta_core::science::{
    FLEXPART_PV60_OZONE_ID, GLOBAL_PERIODIC_ID, LIMITED_DOMAIN_TERMINATE_ID,
    MODEL_TOP_TERMINATE_ID, OZONE_DOMAIN_FILL_ID, RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};
use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::ProfileCatalog;

const PRESSURE_00: i64 = 1_543_622_400;
const CFSR_00: i64 = 1_230_768_000;
const OZONE_SUBSTANCE_ID: &str = "ozone";

#[derive(Clone, Copy)]
struct Family {
    id: &'static str,
    environment_variable: &'static str,
    relative_fixture: &'static str,
    profile: &'static str,
    source: &'static str,
    coverage_start: i64,
    coverage_end: i64,
    run_start: i64,
    periodic_longitude: bool,
}

const FAMILIES: [Family; 3] = [
    Family {
        id: "era5-pressure",
        environment_variable: "TRAJECTA_REAL_ERA5_PRESSURE_OFFICIAL_DIR",
        relative_fixture: "../../target/test-data/era5-cds-pressure-official/ready",
        profile: "era5-cf-pressure-netcdf-v0",
        source: "Copernicus CDS ERA5 pressure levels",
        coverage_start: PRESSURE_00,
        coverage_end: PRESSURE_00 + 12 * 3_600,
        run_start: PRESSURE_00 + 3 * 3_600,
        periodic_longitude: false,
    },
    Family {
        id: "era5-hybrid",
        environment_variable: "TRAJECTA_REAL_ERA5_HYBRID137_DIR",
        relative_fixture: "../../target/test-data/era5-cds-hybrid137-official/ready",
        profile: "era5-cds-hybrid137-v0",
        source: "Copernicus CDS ERA5 hybrid 137 levels",
        coverage_start: PRESSURE_00,
        coverage_end: PRESSURE_00 + 6 * 3_600,
        run_start: PRESSURE_00 + 3 * 3_600,
        periodic_longitude: false,
    },
    Family {
        id: "cfsr-pressure",
        environment_variable: "TRAJECTA_REAL_CFSR_DIR",
        relative_fixture: "../../target/test-data/cfsr-ncei-pgbl-official",
        profile: "cfsr-pgbl-pressure-v0",
        source: "NOAA NCEI CFSR pgbl",
        coverage_start: CFSR_00,
        coverage_end: CFSR_00 + 12 * 3_600,
        run_start: CFSR_00 + 3 * 3_600,
        periodic_longitude: true,
    },
];

#[derive(Serialize)]
struct MatrixEvidence {
    schema_version: &'static str,
    target_particle_count: u64,
    runs: Vec<RunEvidence>,
}

#[derive(Serialize)]
struct RunEvidence {
    family: &'static str,
    direction: &'static str,
    reader_backend: &'static str,
    run_id: String,
    status: &'static str,
    numerical_steps: u64,
    seeded_particles: u64,
    final_particle_rows: usize,
    ozone_mass_rows: i64,
    total_ozone_mass_kg: f64,
    mass_ledger_records: usize,
    maximum_ledger_fraction: f64,
    abnormal_terminations: u64,
    elapsed_milliseconds: u128,
    manifest_relative_path: PathBuf,
}

fn seconds(value: f64) -> Quantity<TimeDimension> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn capabilities() -> CapabilitySet {
    CapabilitySet::new()
        .with(Capability::Transport)
        .with(Capability::NearSurfaceTransport)
        .with(Capability::DomainFill)
        .with(Capability::Diagnostics)
}

fn fixture_directory(family: Family) -> PathBuf {
    std::env::var_os(family.environment_variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(family.relative_fixture))
}

fn target_particle_count() -> u64 {
    std::env::var("TRAJECTA_M4_A3_PARTICLES")
        .ok()
        .map(|value| value.parse::<u64>().expect("TRAJECTA_M4_A3_PARTICLES"))
        .unwrap_or(1_000)
}

fn run_duration_seconds() -> i64 {
    std::env::var("TRAJECTA_M4_A3_DURATION_SECONDS")
        .ok()
        .map(|value| {
            value
                .parse::<i64>()
                .expect("TRAJECTA_M4_A3_DURATION_SECONDS")
        })
        .unwrap_or(600)
}

fn selected_family(family: Family) -> bool {
    std::env::var("TRAJECTA_M4_A3_FAMILY")
        .ok()
        .is_none_or(|selected| selected == family.id)
}

fn selected_direction(direction: Direction) -> bool {
    std::env::var("TRAJECTA_M4_A3_DIRECTION")
        .ok()
        .is_none_or(|selected| {
            selected
                == match direction {
                    Direction::Forward => "forward",
                    Direction::Backward => "backward",
                }
        })
}

fn selected_reader_backend() -> MeteorologyReaderBackend {
    match std::env::var("TRAJECTA_M4_A3_READER_BACKEND")
        .as_deref()
        .unwrap_or("rust")
    {
        "rust" => MeteorologyReaderBackend::Rust,
        "native" => MeteorologyReaderBackend::Native,
        other => panic!("unsupported TRAJECTA_M4_A3_READER_BACKEND={other:?}"),
    }
}

const fn reader_backend_label(backend: MeteorologyReaderBackend) -> &'static str {
    match backend {
        MeteorologyReaderBackend::Native => "native",
        MeteorologyReaderBackend::Rust => "rust",
    }
}

fn build_lock(
    family: Family,
    fixture: &Path,
    control_root: &Path,
    reader_backend: MeteorologyReaderBackend,
) -> (DatasetRef, PathBuf) {
    assert!(
        fixture.is_dir(),
        "missing {} fixture: {fixture:?}",
        family.id
    );
    let dataset = DatasetRef(format!("m4-a3-{}", family.id));
    let profiles = ProfileCatalog::load(&[]).expect("built-in profiles");
    let inspector = ReaderMetadataInspector::new(reader_backend);
    let mut hash_cache = FileHashCache::new();
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: dataset.clone(),
            source: family.source.into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-core-m4-a3-real-matrix".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), fixture.to_path_buf())]),
        coverage: LockCoverageRequest {
            start: Timestamp::new(family.coverage_start, 0).unwrap(),
            end: Timestamp::new(family.coverage_end, 0).unwrap(),
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities(),
        force_rehash: true,
        preferred_profile: Some(family.profile.into()),
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    assert!(
        outcome.is_success(),
        "{} lock failed: {:?}",
        family.id,
        outcome.diagnostics
    );
    let lock = outcome.lock.expect("successful lock");
    assert_eq!(lock.profile.name, family.profile);
    let lock_path = control_root.join(format!("{}-dataset-lock.json", family.id));
    fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
    (dataset, lock_path)
}

fn case(family: Family, dataset: DatasetRef, direction: Direction, particles: u64) -> ResolvedCase {
    let start = Timestamp::new(family.run_start, 0).unwrap();
    let duration_seconds = run_duration_seconds();
    assert!(duration_seconds > 0, "run duration must be positive");
    let end_seconds = match direction {
        Direction::Forward => family.run_start + duration_seconds,
        Direction::Backward => family.run_start - duration_seconds,
    };
    let domain = DomainId(family.id.into());
    let horizontal_policy = if family.periodic_longitude {
        GLOBAL_PERIODIC_ID
    } else {
        LIMITED_DOMAIN_TERMINATE_ID
    };
    ResolvedCase {
        metadata: Metadata {
            name: format!("m4-a3-real-{}-{direction:?}", family.id).to_ascii_lowercase(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start,
            end: Timestamp::new(end_seconds, 0).unwrap(),
            direction,
        }),
        meteorology: Some(MeteorologySpec {
            domains: vec![DomainSpec {
                id: domain.clone(),
                dataset,
                priority: 0,
                parent: None,
                horizontal_halo_cells: 1,
            }],
        }),
        particle_population: Some(ParticlePopulationSpec::DomainFillStratosphericOzone(
            DomainFillStratosphericOzoneSpec {
                air_mass: DomainFillAirMassSpec {
                    id: PopulationId("ozone".into()),
                    domain_id: domain,
                    target_dry_air_mass_per_particle: None,
                    target_particle_count: Some(particles),
                },
                ozone_rule: FLEXPART_PV60_OZONE_ID.into(),
                ozone_substance: SubstanceId(OZONE_SUBSTANCE_ID.into()),
            },
        )),
        substances: Vec::new(),
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
                    ModelId(horizontal_policy.into()),
                ],
            },
            random_seed: Some(4_203),
        }),
        physics: Vec::new(),
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn run_family(
    family: Family,
    direction: Direction,
    reader_backend: MeteorologyReaderBackend,
    particles: u64,
    output_root: &Path,
    control_root: &Path,
) -> RunEvidence {
    let fixture = fixture_directory(family);
    let (dataset, lock_path) = build_lock(family, &fixture, control_root, reader_backend);
    let case = case(family, dataset.clone(), direction, particles);
    let case_path = control_root.join(format!("{}-{direction:?}-case.json", family.id));
    fs::write(&case_path, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
    let profile = ResolvedRunProfile {
        metadata: Metadata {
            name: format!("m4-a3-real-{}-profile", family.id),
            ..Metadata::default()
        },
        case_path,
        output_root: output_root.to_path_buf(),
        datasets: vec![DatasetBinding {
            dataset,
            lockfile: lock_path,
            cache_root: None,
            data_roots: BTreeMap::from([(DataRootId("met".into()), fixture)]),
            reader_backend: Some(reader_backend),
        }],
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: 1,
            memory_budget_bytes: 2 * 1_024 * 1_024 * 1_024,
            executor: "rayon".into(),
            meteorology_reader: reader_backend,
        },
        sources: Vec::new(),
    };

    let started = Instant::now();
    let mut runner = build_runner(case, profile, None).expect("production RunnerBuilder");
    assert_eq!(
        runner.run(),
        Ok(RunOutcome::Complete),
        "{:?}",
        runner.manifest()
    );
    let elapsed = started.elapsed();
    let manifest = runner.manifest();
    assert_eq!(manifest.numerical.population, OZONE_DOMAIN_FILL_ID);
    assert_eq!(
        manifest.numerical.ozone_rule.as_deref(),
        Some(FLEXPART_PV60_OZONE_ID)
    );
    assert_eq!(manifest.terminations.abnormal_count, 0);
    assert!(!manifest.inputs.dataset_lock_sha256.is_empty());
    assert!(!manifest.inputs.dataset_profile_sha256.is_empty());
    assert!(!manifest.inputs.dataset_content_sha256.is_empty());
    assert_eq!(
        manifest.mass_ledger.len(),
        usize::try_from(runner.state().numerical_step_index).unwrap()
    );
    assert!(!manifest.mass_ledger.is_empty());
    let maximum_ledger_fraction = manifest
        .mass_ledger
        .iter()
        .map(|record| {
            assert!(record.imbalance_kg.abs() <= record.tolerance_kg);
            if record.tolerance_kg == 0.0 {
                0.0
            } else {
                record.imbalance_kg.abs() / record.tolerance_kg
            }
        })
        .fold(0.0_f64, f64::max);
    let seeded_particles = match &runner.state().population_state {
        PopulationState::DomainFill {
            seeded_particles, ..
        } => *seeded_particles,
        other => panic!("unexpected population state: {other:?}"),
    };
    assert_eq!(seeded_particles, particles);
    let final_particle_rows = runner.state().particles.validate().unwrap();
    let ozone_mass = runner
        .state()
        .particles
        .mass
        .mass_kg
        .get(&SubstanceId(OZONE_SUBSTANCE_ID.into()))
        .expect("ozone mass column");
    assert_eq!(ozone_mass.len(), final_particle_rows);
    assert!(
        ozone_mass
            .iter()
            .all(|mass| mass.is_finite() && *mass > 0.0)
    );

    let run_dir = output_root
        .join(&manifest.case_name)
        .join(&manifest.run_id.0);
    let manifest_path = run_dir.join("run-manifest.json");
    let sqlite_path = run_dir.join("particles.sqlite");
    assert!(manifest_path.is_file(), "missing {manifest_path:?}");
    let inspection = ParticleStateSqliteSink::inspect(&sqlite_path).unwrap();
    assert_eq!(inspection.integrity, "ok");
    let connection = Connection::open_with_flags(&sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("read-only particle database");
    let (ozone_mass_rows, distinct_particles, total_ozone_mass_kg, minimum_ozone_mass_kg): (
        i64,
        i64,
        f64,
        f64,
    ) = connection
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT particle_id), SUM(mass_kg), MIN(mass_kg)\
             FROM particle_mass WHERE substance_id = ?1",
            [OZONE_SUBSTANCE_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(ozone_mass_rows, final_particle_rows as i64);
    assert_eq!(distinct_particles, final_particle_rows as i64);
    assert!(total_ozone_mass_kg.is_finite() && total_ozone_mass_kg > 0.0);
    assert!(minimum_ozone_mass_kg.is_finite() && minimum_ozone_mass_kg > 0.0);

    RunEvidence {
        family: family.id,
        direction: match direction {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
        },
        reader_backend: reader_backend_label(reader_backend),
        run_id: manifest.run_id.0.clone(),
        status: "complete",
        numerical_steps: runner.state().numerical_step_index,
        seeded_particles,
        final_particle_rows,
        ozone_mass_rows,
        total_ozone_mass_kg,
        mass_ledger_records: manifest.mass_ledger.len(),
        maximum_ledger_fraction,
        abnormal_terminations: manifest.terminations.abnormal_count,
        elapsed_milliseconds: elapsed.as_millis(),
        manifest_relative_path: manifest_path
            .strip_prefix(output_root)
            .expect("manifest under output root")
            .to_path_buf(),
    }
}

#[test]
#[ignore = "explicit M4-A3 real-data matrix; run with --ignored"]
fn real_stratospheric_ozone_three_families_forward_backward() {
    let particles = target_particle_count();
    let reader_backend = selected_reader_backend();
    assert!(particles > 0);
    let temporary_output = tempdir().unwrap();
    let output_root = std::env::var_os("TRAJECTA_M4_A3_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| temporary_output.path().to_path_buf());
    fs::create_dir_all(&output_root).unwrap();
    let control = tempdir().unwrap();
    let mut runs = Vec::new();
    for family in FAMILIES
        .into_iter()
        .filter(|family| selected_family(*family))
    {
        for direction in [Direction::Forward, Direction::Backward] {
            if selected_direction(direction) {
                runs.push(run_family(
                    family,
                    direction,
                    reader_backend,
                    particles,
                    &output_root,
                    control.path(),
                ));
            }
        }
    }
    assert!(
        !runs.is_empty(),
        "M4-A3 real-data matrix selection is empty"
    );
    let evidence = MatrixEvidence {
        schema_version: "trajecta.m4-a3-real-matrix/v1",
        target_particle_count: particles,
        runs,
    };
    fs::write(
        output_root.join("M4_A3_REAL_MATRIX_SUMMARY.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}
