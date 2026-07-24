//! Explicit M4-A2 real-data smoke/full matrix for dry-air domain filling.
//!
//! This gate is ignored by default because the same entry point scales from a
//! 64-particle engineering smoke to the formal 10,000-particle A2 matrix. Run
//! it explicitly with `--ignored`; missing frozen fixtures are hard failures.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    DomainFillAirMassSpec, GeoJsonGeometry, GeoJsonSource, ParticlePopulationSpec, PopulationId,
    ReleaseDrivenSpec, ReleaseEventId, ReleaseEventSpec, ReleaseVerticalSpec,
};
use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Length, Mass, Quantity, Time as TimeDimension, Unit};
use trajecta_core::runner::{RunOutcome, build_runner};
use trajecta_core::science::{
    DRY_AIR_DOMAIN_FILL_ID, GLOBAL_PERIODIC_ID, LIMITED_DOMAIN_TERMINATE_ID,
    MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};
use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::ProfileCatalog;

const PRESSURE_00: i64 = 1_543_622_400;
const CFSR_00: i64 = 1_230_768_000;

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
    run_id: String,
    status: &'static str,
    numerical_steps: u64,
    seeded_particles: u64,
    final_particle_rows: usize,
    mass_ledger_records: usize,
    maximum_ledger_fraction: f64,
    abnormal_terminations: u64,
    elapsed_milliseconds: u128,
    manifest_relative_path: PathBuf,
}

fn seconds(value: f64) -> Quantity<TimeDimension> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn metres(value: f64) -> Quantity<Length> {
    Quantity::from_si(value, Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap()).unwrap()
}

fn kilograms(value: f64) -> Quantity<Mass> {
    Quantity::from_si(value, Unit::new("kg", Dimension::Mass, 1.0, 0.0).unwrap()).unwrap()
}

fn capabilities() -> CapabilitySet {
    CapabilitySet::new()
        .with(Capability::Transport)
        .with(Capability::NearSurfaceTransport)
        .with(Capability::DomainFill)
}

fn fixture_directory(family: Family) -> PathBuf {
    std::env::var_os(family.environment_variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(family.relative_fixture))
}

fn target_particle_count() -> u64 {
    std::env::var("TRAJECTA_M4_A2_PARTICLES")
        .ok()
        .map(|value| value.parse::<u64>().expect("TRAJECTA_M4_A2_PARTICLES"))
        .unwrap_or(64)
}

fn run_duration_seconds() -> i64 {
    std::env::var("TRAJECTA_M4_A2_DURATION_SECONDS")
        .ok()
        .map(|value| {
            value
                .parse::<i64>()
                .expect("TRAJECTA_M4_A2_DURATION_SECONDS")
        })
        .unwrap_or(600)
}

fn selected_family(family: Family) -> bool {
    std::env::var("TRAJECTA_M4_A2_FAMILY")
        .ok()
        .is_none_or(|selected| selected == family.id)
}

fn selected_direction(direction: Direction) -> bool {
    std::env::var("TRAJECTA_M4_A2_DIRECTION")
        .ok()
        .is_none_or(|selected| {
            selected
                == match direction {
                    Direction::Forward => "forward",
                    Direction::Backward => "backward",
                }
        })
}

fn build_lock(family: Family, fixture: &Path, control_root: &Path) -> (DatasetRef, PathBuf) {
    assert!(
        fixture.is_dir(),
        "missing {} fixture: {fixture:?}",
        family.id
    );
    let dataset = DatasetRef(format!("m4-a2-{}", family.id));
    let profiles = ProfileCatalog::load(&[]).expect("built-in profiles");
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: dataset.clone(),
            source: family.source.into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-core-m4-a2-real-matrix".into(),
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
            name: format!("m4-a2-real-{}-{direction:?}", family.id).to_ascii_lowercase(),
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
        particle_population: Some(ParticlePopulationSpec::DomainFillAirMass(
            DomainFillAirMassSpec {
                id: PopulationId("air".into()),
                domain_id: domain,
                target_dry_air_mass_per_particle: None,
                target_particle_count: Some(particles),
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
            random_seed: Some(4_202),
        }),
        physics: Vec::new(),
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn run_family(
    family: Family,
    direction: Direction,
    particles: u64,
    output_root: &Path,
    control_root: &Path,
) -> RunEvidence {
    let fixture = fixture_directory(family);
    let (dataset, lock_path) = build_lock(family, &fixture, control_root);
    let case = case(family, dataset.clone(), direction, particles);
    let case_path = control_root.join(format!("{}-{direction:?}-case.json", family.id));
    fs::write(&case_path, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
    let profile = ResolvedRunProfile {
        metadata: Metadata {
            name: format!("m4-a2-real-{}-profile", family.id),
            ..Metadata::default()
        },
        case_path,
        output_root: output_root.to_path_buf(),
        datasets: vec![DatasetBinding {
            dataset,
            lockfile: lock_path,
            cache_root: None,
            data_roots: BTreeMap::from([(DataRootId("met".into()), fixture)]),
            reader_backend: Some(MeteorologyReaderBackend::Rust),
        }],
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: 1,
            memory_budget_bytes: 2 * 1_024 * 1_024 * 1_024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
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
    assert_eq!(manifest.numerical.population, DRY_AIR_DOMAIN_FILL_ID);
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
        trajecta_core::population::PopulationState::DomainFill {
            seeded_particles, ..
        } => *seeded_particles,
        other => panic!("unexpected population state: {other:?}"),
    };
    let final_particle_rows = runner.state().particles.len().unwrap();
    runner.state().particles.validate().unwrap();
    let case_dir = output_root.join(&manifest.case_name);
    let manifest_path = case_dir.join(&manifest.run_id.0).join("run-manifest.json");
    assert!(manifest_path.is_file(), "missing {manifest_path:?}");
    RunEvidence {
        family: family.id,
        direction: match direction {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
        },
        run_id: manifest.run_id.0.clone(),
        status: "complete",
        numerical_steps: runner.state().numerical_step_index,
        seeded_particles,
        final_particle_rows,
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
#[ignore = "explicit M4-A2 real-data matrix; run with --ignored"]
fn real_air_mass_domain_fill_three_families_forward_backward() {
    let particles = target_particle_count();
    assert!(particles > 0);
    let temporary_output = tempdir().unwrap();
    let output_root = std::env::var_os("TRAJECTA_M4_A2_ARTIFACT_DIR")
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
            if !selected_direction(direction) {
                continue;
            }
            runs.push(run_family(
                family,
                direction,
                particles,
                &output_root,
                control.path(),
            ));
        }
    }
    assert!(
        !runs.is_empty(),
        "M4-A2 real-data matrix selection is empty"
    );
    let evidence = MatrixEvidence {
        schema_version: "trajecta.m4-a2-real-matrix/v1",
        target_particle_count: particles,
        runs,
    };
    fs::write(
        output_root.join("M4_A2_REAL_MATRIX_SUMMARY.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}

fn run_release_replay(family: Family, case_name: &str, points: &[(f64, f64, f64)]) {
    let fixture = fixture_directory(family);
    let control = tempdir().unwrap();
    let output = tempdir().unwrap();
    let (dataset, lock_path) = build_lock(family, &fixture, control.path());
    let domain = DomainId(family.id.into());
    let start = Timestamp::new(family.run_start, 0).unwrap();
    let horizontal_policy = if family.periodic_longitude {
        GLOBAL_PERIODIC_ID
    } else {
        LIMITED_DOMAIN_TERMINATE_ID
    };
    let case = ResolvedCase {
        metadata: Metadata {
            name: case_name.into(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start,
            end: Timestamp::new(family.run_start + 1, 0).unwrap(),
            direction: Direction::Forward,
        }),
        meteorology: Some(MeteorologySpec {
            domains: vec![DomainSpec {
                id: domain,
                dataset: dataset.clone(),
                priority: 0,
                parent: None,
                horizontal_halo_cells: 1,
            }],
        }),
        particle_population: Some(ParticlePopulationSpec::ReleaseDriven(ReleaseDrivenSpec {
            id: PopulationId("replay".into()),
            events: points
                .iter()
                .copied()
                .enumerate()
                .map(
                    |(index, (longitude_degrees, latitude_degrees, height_asl_m))| {
                        ReleaseEventSpec {
                            id: ReleaseEventId(format!("failure-{index}")),
                            start,
                            end: start,
                            particle_count: 1,
                            mass: BTreeMap::from([(SubstanceId("tracer".into()), kilograms(1.0))]),
                            geometry: GeoJsonSource::Inline {
                                geometry: GeoJsonGeometry::Point([
                                    longitude_degrees,
                                    latitude_degrees,
                                ]),
                            },
                            vertical: ReleaseVerticalSpec::AboveSeaLevel {
                                lower: metres(height_asl_m),
                                upper: None,
                            },
                        }
                    },
                )
                .collect(),
        })),
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
            random_seed: Some(4_202),
        }),
        physics: Vec::new(),
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    };
    let profile = ResolvedRunProfile {
        metadata: Metadata {
            name: format!("{case_name}-profile"),
            ..Metadata::default()
        },
        case_path: control.path().join("case.json"),
        output_root: output.path().to_path_buf(),
        datasets: vec![DatasetBinding {
            dataset,
            lockfile: lock_path,
            cache_root: None,
            data_roots: BTreeMap::from([(DataRootId("met".into()), fixture)]),
            reader_backend: Some(MeteorologyReaderBackend::Rust),
        }],
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: 1,
            memory_budget_bytes: 2 * 1_024 * 1_024 * 1_024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
        },
        sources: Vec::new(),
    };
    let mut runner = build_runner(case, profile, None).expect("production RunnerBuilder");
    assert_eq!(
        runner.run(),
        Ok(RunOutcome::Complete),
        "{:?}",
        runner.manifest()
    );
}

#[test]
#[ignore = "explicit real hybrid boundary replay"]
fn real_hybrid_short_path_boundary_replay() {
    run_release_replay(
        FAMILIES[1],
        "m4-a2-real-hybrid-boundary-replay",
        &[(
            6.657_371_397_113_28,
            46.309_587_272_666_5,
            1_389.278_980_462_048_1,
        )],
    );
}

#[test]
#[ignore = "explicit real CFSR domain-fill invalid-meteorology replay"]
fn real_cfsr_domain_fill_invalid_meteorology_replay() {
    run_release_replay(
        FAMILIES[2],
        "m4-a2-real-cfsr-domain-fill-invalid-meteorology-replay",
        &[
            (
                -64.088_730_697_028_32,
                3.187_865_479_266_631,
                975.636_606_620_666_8,
            ),
            (
                -79.688_505_999_155_04,
                0.174_651_465_196_873_1,
                1_392.514_179_134_517,
            ),
            (
                101.909_190_593_158_6,
                1.366_565_096_947_062_9,
                273.597_439_709_298_4,
            ),
            (
                -56.384_774_109_941_08,
                0.768_596_839_867_680_2,
                440.084_542_966_699_9,
            ),
            (
                114.652_518_661_216_62,
                1.043_053_887_728_922_8,
                822.369_818_825_158_5,
            ),
            (
                103.906_864_998_516_31,
                19.663_812_305_334_258,
                681.834_477_150_912_9,
            ),
        ],
    );
}
