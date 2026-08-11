//! M6-A1 end-to-end gates for the first production physical-process slice.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tempfile::{TempDir, tempdir};
use trajecta_case::diagnostic::DiagnosticPath;
use trajecta_case::document::{
    ExecutionSpec, MeteorologyReaderBackend, ResolvedCase, ResolvedRunProfile,
};
use trajecta_case::model::metadata::Metadata;
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
use trajecta_core::manifest::RunManifest;
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::runner::{RunError, RunOutcome, build_runner};
use trajecta_core::science::{GLOBAL_PERIODIC_ID, MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID};
use trajecta_core::synthetic::{SyntheticWind, constant_wind_stack};
use trajecta_core::verification::{VerificationMode, verify_run_directory};
use trajecta_met::query::engine::RayonExecutionContext;

const WATER: &str = "water";
const DURATION_SECONDS: i64 = 75;

fn metres(value: f64) -> Quantity<Length> {
    Quantity::from_si(value, Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap()).unwrap()
}

fn seconds(value: f64) -> Quantity<Time> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn kilograms(value: f64) -> Quantity<Mass> {
    Quantity::from_si(value, Unit::new("kg", Dimension::Mass, 1.0, 0.0).unwrap()).unwrap()
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

fn full_water_preset() -> ResolvedPhysicsSpec {
    resolve_physics(
        &PhysicsSelectionSpec {
            preset: PhysicsPreset::WaterVaporTracking,
            remove: Vec::new(),
            overrides: Vec::new(),
            modules: Vec::new(),
        },
        DiagnosticPath::root().field("physics"),
    )
    .unwrap()
}

fn case(
    direction: Direction,
    continuous_release: bool,
    physics: Option<ResolvedPhysicsSpec>,
) -> ResolvedCase {
    let (start, end) = match direction {
        Direction::Forward => (0, DURATION_SECONDS),
        Direction::Backward => (DURATION_SECONDS, 0),
    };
    let (release_start, release_end) = if continuous_release {
        (0, DURATION_SECONDS)
    } else {
        (start, start)
    };
    ResolvedCase {
        metadata: Metadata {
            name: "m6-a1-synthetic".into(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start: Timestamp::new(start, 0).unwrap(),
            end: Timestamp::new(end, 0).unwrap(),
            direction,
        }),
        meteorology: None,
        particle_population: Some(ParticlePopulationSpec::ReleaseDriven(ReleaseDrivenSpec {
            id: PopulationId("release".into()),
            events: vec![ReleaseEventSpec {
                id: ReleaseEventId("source".into()),
                start: Timestamp::new(release_start, 0).unwrap(),
                end: Timestamp::new(release_end, 0).unwrap(),
                particle_count: 3,
                mass: BTreeMap::from([(SubstanceId(WATER.into()), kilograms(3.0))]),
                geometry: GeoJsonSource::Inline {
                    geometry: GeoJsonGeometry::Point([0.0, 0.0]),
                },
                vertical: ReleaseVerticalSpec::AboveSeaLevel {
                    lower: metres(100.0),
                    upper: None,
                },
            }],
        })),
        substances: vec![SubstanceSpec::WaterVapor {
            id: SubstanceId(WATER.into()),
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
                    ModelId(MODEL_TOP_TERMINATE_ID.into()),
                    ModelId(GLOBAL_PERIODIC_ID.into()),
                ],
            },
            random_seed: Some(17),
        }),
        physics,
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn profile(output_root: PathBuf, workers: usize) -> ResolvedRunProfile {
    ResolvedRunProfile {
        metadata: Metadata {
            name: "m6-a1-profile".into(),
            ..Metadata::default()
        },
        case_path: PathBuf::from("case.yaml"),
        output_root,
        datasets: Vec::new(),
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: workers,
            memory_budget_bytes: 64 * 1024 * 1024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
        },
        sources: Vec::new(),
    }
}

struct CompletedRun {
    _root: TempDir,
    sqlite: PathBuf,
    manifest: RunManifest,
}

fn run(
    direction: Direction,
    continuous_release: bool,
    physics: Option<ResolvedPhysicsSpec>,
    workers: usize,
    wind: SyntheticWind,
) -> CompletedRun {
    run_at_height(direction, continuous_release, physics, workers, wind, 100.0)
}

fn run_at_height(
    direction: Direction,
    continuous_release: bool,
    physics: Option<ResolvedPhysicsSpec>,
    workers: usize,
    wind: SyntheticWind,
    height_m: f64,
) -> CompletedRun {
    let root = tempdir().unwrap();
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(DURATION_SECONDS, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    let mut stack = constant_wind_stack("synthetic", &times, wind, 0.0, 20_000.0, true).unwrap();
    stack.execution = Box::new(RayonExecutionContext {
        worker_threads: workers,
    });
    let mut resolved_case = case(direction, continuous_release, physics);
    let ParticlePopulationSpec::ReleaseDriven(population) =
        resolved_case.particle_population.as_mut().unwrap()
    else {
        unreachable!()
    };
    population.events[0].vertical = ReleaseVerticalSpec::AboveSeaLevel {
        lower: metres(height_m),
        upper: None,
    };
    let mut runner = build_runner(
        resolved_case,
        profile(root.path().to_path_buf(), workers),
        Some(stack),
    )
    .unwrap();
    assert_eq!(runner.run().unwrap(), RunOutcome::Complete);
    let manifest = runner.manifest().clone();
    let sqlite = find_file(root.path(), "particles.sqlite").expect("particles.sqlite");
    CompletedRun {
        _root: root,
        sqlite,
        manifest,
    }
}

fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    if root.is_file() {
        return (root.file_name()?.to_str()? == name).then(|| root.to_path_buf());
    }
    for entry in std::fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if let Some(found) = find_file(&path, name) {
            return Some(found);
        }
    }
    None
}

fn calm() -> SyntheticWind {
    SyntheticWind {
        eastward_m_s: 0.0,
        northward_m_s: 0.0,
        vertical_m_s: 0.0,
    }
}

#[test]
fn boundary_layer_publishes_v2_directional_state_and_continuous_summary() {
    for direction in [Direction::Forward, Direction::Backward] {
        let completed = run(direction, false, Some(boundary_layer_physics()), 1, calm());
        assert_eq!(completed.manifest.sqlite.schema_version, 2);
        let verification =
            verify_run_directory(completed.sqlite.parent().unwrap(), VerificationMode::Full)
                .unwrap();
        assert!(verification.run_success);
        let connection = ParticleStateSqliteSink::open_readonly(&completed.sqlite).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            2
        );
        let direction_name = match direction {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
        };
        let summary: (String, String, String, u64, f64) = connection
            .query_row(
                "SELECT module_id,substance_id,direction,event_count,closure_residual
                 FROM process_summary",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(summary.0, "boundary_layer_langevin");
        assert_eq!(summary.1, WATER);
        assert_eq!(summary.2, direction_name);
        assert_eq!(summary.3, 0);
        assert_eq!(summary.4, 0.0);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM process_event", [], |row| row
                    .get::<_, u64>(0))
                .unwrap(),
            0
        );
        let velocities: Vec<(f64, f64, f64)> = connection
            .prepare(
                "SELECT boundary_layer_random_eastward_m_s,
                        boundary_layer_random_northward_m_s,
                        boundary_layer_random_vertical_m_s
                 FROM particle_state
                 WHERE boundary_layer_random_eastward_m_s IS NOT NULL",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!velocities.is_empty());
        assert!(
            velocities
                .iter()
                .all(|(u, v, w)| { u.is_finite() && v.is_finite() && w.is_finite() })
        );

        let mass_count = connection
            .query_row("SELECT COUNT(*) FROM particle_mass", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap();
        let adjoint_count = connection
            .query_row("SELECT COUNT(*) FROM particle_adjoint", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap();
        match direction {
            Direction::Forward => assert_eq!((mass_count, adjoint_count), (3, 0)),
            Direction::Backward => assert_eq!((mass_count, adjoint_count), (0, 3)),
        }
    }
}

#[test]
fn continuous_births_evolve_only_for_their_directional_active_interval() {
    for direction in [Direction::Forward, Direction::Backward] {
        let completed = run(direction, true, Some(boundary_layer_physics()), 1, calm());
        let connection = ParticleStateSqliteSink::open_readonly(&completed.sqlite).unwrap();
        let rows: Vec<(i64, i64, i64, u64)> = connection
            .prepare(
                "SELECT p.birth_seconds,p.birth_nanosecond,
                        s.integration_offset_ns,s.elapsed_age_ns
                 FROM particle p JOIN particle_state s
                   ON s.run_id=p.run_id AND s.particle_id=p.particle_id
                 JOIN output_event e
                   ON e.run_id=s.run_id AND e.event_sequence=s.event_sequence
                 WHERE e.event_kind='end' ORDER BY p.particle_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), 3);
        for (birth_seconds, birth_nanosecond, offset_ns, age_ns) in rows {
            let birth_ns = birth_seconds * 1_000_000_000 + birth_nanosecond;
            let expected = match direction {
                Direction::Forward => DURATION_SECONDS * 1_000_000_000 - birth_ns,
                Direction::Backward => -birth_ns,
            };
            assert_eq!(offset_ns, expected);
            assert_eq!(age_ns, expected.unsigned_abs());
        }
    }
}

#[test]
fn boundary_layer_science_is_byte_identical_for_one_and_four_workers() {
    let one = run(
        Direction::Forward,
        false,
        Some(boundary_layer_physics()),
        1,
        calm(),
    );
    let four = run(
        Direction::Forward,
        false,
        Some(boundary_layer_physics()),
        4,
        calm(),
    );
    let one_provenance = one.manifest.provenance.unwrap();
    let four_provenance = four.manifest.provenance.unwrap();
    assert_eq!(
        one_provenance.content_sha256,
        four_provenance.content_sha256
    );
    assert_eq!(
        one_provenance.sqlite_sql_sha256,
        four_provenance.sqlite_sql_sha256
    );
    assert_eq!(
        one_provenance.canonical_output_sha256,
        four_provenance.canonical_output_sha256
    );
}

#[test]
fn pure_advection_keeps_process_state_absent_and_the_constant_wind_solution() {
    let completed = run_at_height(
        Direction::Forward,
        false,
        None,
        1,
        SyntheticWind {
            eastward_m_s: 10.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        1_500.0,
    );
    let connection = ParticleStateSqliteSink::open_readonly(&completed.sqlite).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM process_summary", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM particle_state
                 WHERE boundary_layer_random_eastward_m_s IS NOT NULL
                    OR boundary_layer_random_northward_m_s IS NOT NULL
                    OR boundary_layer_random_vertical_m_s IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );
    let longitude = connection
        .query_row(
            "SELECT longitude_degrees FROM particle_state s
             JOIN output_event e ON e.run_id=s.run_id AND e.event_sequence=s.event_sequence
             WHERE e.event_kind='end' ORDER BY particle_id LIMIT 1",
            [],
            |row| row.get::<_, f64>(0),
        )
        .unwrap();
    let expected = (750.0 / trajecta_core::science::M4_CONSTANTS.earth_radius_m).to_degrees();
    assert!(
        (longitude - expected).abs() <= 2.0e-7,
        "{longitude} != {expected}"
    );
}

#[test]
fn modules_owned_by_later_stages_fail_before_execution() {
    let root = tempdir().unwrap();
    let times = [
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(DURATION_SECONDS, 0).unwrap(),
    ];
    let stack = constant_wind_stack("synthetic", &times, calm(), 0.0, 20_000.0, true).unwrap();
    let error = match build_runner(
        case(Direction::Forward, false, Some(full_water_preset())),
        profile(root.path().to_path_buf(), 1),
        Some(stack),
    ) {
        Ok(_) => panic!("later-stage module unexpectedly built"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "physics.module_stage_not_available");
    assert!(matches!(error, RunError::Physics(_)));
}
