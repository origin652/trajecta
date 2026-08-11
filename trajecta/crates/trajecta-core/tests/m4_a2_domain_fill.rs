//! M4-A2 production-builder hard gates for dry-air domain filling.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use tempfile::tempdir;
use trajecta_case::document::{
    ExecutionSpec, MeteorologyReaderBackend, ResolvedCase, ResolvedRunProfile,
};
use trajecta_case::model::metadata::Metadata;
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::numerics::{BoundarySpec, IntegratorSpec, NumericsSpec};
use trajecta_case::model::output::default_particle_state_output;
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, ParticlePopulationSpec, PopulationId,
};
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Quantity, Time as TimeDimension, Unit};
use trajecta_core::output::provenance_bundle::{
    canonical_output_digest, file_sha256, validate_bundle_file_semantics_loose,
};
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::particle::{ParticleOrigin, TerminationReason};
use trajecta_core::runner::{RunOutcome, RunnerBuildKnobs, build_runner, build_runner_with_knobs};
use trajecta_core::science::{
    DRY_AIR_DOMAIN_FILL_ID, LIMITED_DOMAIN_TERMINATE_ID, MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID,
    SURFACE_REFLECT_ID,
};
use trajecta_core::synthetic::{
    SyntheticWind, constant_wind_hybrid_stack, constant_wind_multidomain_stack, constant_wind_stack,
};

fn seconds(value: f64) -> Quantity<TimeDimension> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn case(direction: Direction, target_particle_count: u64) -> ResolvedCase {
    let (start, end) = match direction {
        Direction::Forward => (
            Timestamp::new(0, 0).unwrap(),
            Timestamp::new(10, 0).unwrap(),
        ),
        Direction::Backward => (
            Timestamp::new(10, 0).unwrap(),
            Timestamp::new(0, 0).unwrap(),
        ),
    };
    ResolvedCase {
        metadata: Metadata {
            name: format!("m4-a2-domain-fill-{direction:?}").to_ascii_lowercase(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start,
            end,
            direction,
        }),
        meteorology: None,
        particle_population: Some(ParticlePopulationSpec::DomainFillAirMass(
            DomainFillAirMassSpec {
                id: PopulationId("air".into()),
                domain_id: DomainId("finite".into()),
                target_dry_air_mass_per_particle: None,
                target_particle_count: Some(target_particle_count),
            },
        )),
        substances: Vec::new(),
        numerics: Some(NumericsSpec {
            time_step: seconds(10.0),
            integrator: IntegratorSpec {
                model: ModelId(RK2_SPHERICAL_ID.into()),
                parameters: BTreeMap::new(),
            },
            boundaries: BoundarySpec {
                policies: vec![
                    ModelId(SURFACE_REFLECT_ID.into()),
                    ModelId(MODEL_TOP_TERMINATE_ID.into()),
                    ModelId(LIMITED_DOMAIN_TERMINATE_ID.into()),
                ],
            },
            random_seed: Some(71),
        }),
        physics: None,
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn profile(output_root: PathBuf) -> ResolvedRunProfile {
    ResolvedRunProfile {
        metadata: Metadata {
            name: "m4-a2-domain-fill-profile".into(),
            ..Metadata::default()
        },
        case_path: PathBuf::from("case.yaml"),
        output_root,
        datasets: Vec::new(),
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: 1,
            memory_budget_bytes: 64 * 1024 * 1024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
        },
        sources: Vec::new(),
    }
}

fn find_named_file(root: &std::path::Path, name: &str) -> Option<PathBuf> {
    if root.is_file() && root.file_name().and_then(|value| value.to_str()) == Some(name) {
        return Some(root.to_path_buf());
    }
    if root.is_dir() {
        for entry in std::fs::read_dir(root).ok()? {
            if let Some(found) = find_named_file(&entry.ok()?.path(), name) {
                return Some(found);
            }
        }
    }
    None
}

fn run(direction: Direction, hybrid: bool) {
    let output = tempdir().unwrap();
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(10, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    let selected_wind = SyntheticWind {
        eastward_m_s: 100_000.0,
        northward_m_s: 0.0,
        vertical_m_s: 0.0,
    };
    let stack = if hybrid {
        constant_wind_hybrid_stack("finite", &times, selected_wind, 0.0, 20_000.0, false).unwrap()
    } else {
        constant_wind_multidomain_stack(
            "finite",
            &[
                ("finite", selected_wind),
                (
                    "decoy",
                    SyntheticWind {
                        eastward_m_s: -1.0,
                        northward_m_s: 1.0,
                        vertical_m_s: 0.0,
                    },
                ),
            ],
            &times,
            0.0,
            20_000.0,
            false,
        )
        .unwrap()
    };
    let mut runner = build_runner(
        case(direction, 64),
        profile(output.path().into()),
        Some(stack),
    )
    .expect("domain-fill production builder");
    let outcome = runner.run();
    let invalid = (0..runner.state().particles.len().unwrap())
        .filter_map(|index| {
            let state = runner.state().particles.state(index).unwrap();
            matches!(
                state.status,
                trajecta_core::particle::ParticleStatus::Terminated {
                    reason: TerminationReason::InvalidMeteorology
                }
            )
            .then_some(state)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        outcome,
        Ok(RunOutcome::Complete),
        "terminations={:?} invalid={invalid:?}",
        runner.manifest().terminations,
    );
    let manifest = runner.manifest();
    assert_eq!(manifest.numerical.population, DRY_AIR_DOMAIN_FILL_ID);
    assert_eq!(
        manifest.mass_ledger.len(),
        usize::try_from(runner.state().numerical_step_index).unwrap()
    );
    assert!(
        manifest
            .mass_ledger
            .iter()
            .map(|record| record.incoming_kg)
            .sum::<f64>()
            > 0.0
    );
    assert_eq!(manifest.terminations.abnormal_count, 0);
    assert!(
        manifest
            .numerical
            .tolerances
            .contains_key("domain_fill_step_relative")
    );
    if !hybrid {
        assert!(
            runner
                .state()
                .particles
                .origin
                .iter()
                .any(|origin| matches!(origin, ParticleOrigin::DomainBoundary { .. }))
        );
    }
    assert!(
        manifest
            .terminations
            .by_reason
            .contains_key(&TerminationReason::PopulationOutflow.code())
    );
}

#[test]
fn production_builder_domain_fill_forward_and_backward_are_complete() {
    run(Direction::Forward, false);
    run(Direction::Backward, false);
}

#[test]
fn production_builder_hybrid_domain_fill_is_complete() {
    run(Direction::Forward, true);
    run(Direction::Backward, true);
}

#[test]
fn builder_rejects_domain_fill_domain_mismatch() {
    let output = tempdir().unwrap();
    let times = [
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(10, 0).unwrap(),
    ];
    let stack = constant_wind_stack(
        "other",
        &times,
        SyntheticWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        0.0,
        20_000.0,
        false,
    )
    .unwrap();
    let result = build_runner(
        case(Direction::Forward, 64),
        profile(output.path().into()),
        Some(stack),
    );
    assert!(result.is_err());
}

#[test]
fn domain_fill_digest_is_stable_across_workers_order_and_bundle_chunks() {
    let execute = |workers: usize, reverse: bool, chunk: usize| {
        let output = tempdir().unwrap();
        let times = [
            Timestamp::new(-3_600, 0).unwrap(),
            Timestamp::new(0, 0).unwrap(),
            Timestamp::new(10, 0).unwrap(),
            Timestamp::new(3_600, 0).unwrap(),
        ];
        let wind = SyntheticWind {
            eastward_m_s: 100_000.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        };
        let mut stack = constant_wind_multidomain_stack(
            "finite",
            &[
                ("finite", wind),
                (
                    "decoy",
                    SyntheticWind {
                        eastward_m_s: -1.0,
                        northward_m_s: 1.0,
                        vertical_m_s: 0.0,
                    },
                ),
            ],
            &times,
            0.0,
            20_000.0,
            false,
        )
        .unwrap();
        stack.execution = Box::new(trajecta_met::query::engine::RayonExecutionContext {
            worker_threads: workers,
        });
        let mut run_profile = profile(output.path().into());
        run_profile.execution.worker_threads = workers;
        let mut runner = build_runner_with_knobs(
            case(Direction::Forward, 64),
            run_profile,
            Some(stack),
            RunnerBuildKnobs {
                bundle_chunk_lines: Some(chunk),
                bundle_merge_fan_in: Some(2),
                reverse_particle_scan: reverse,
            },
        )
        .unwrap();
        assert_eq!(runner.run(), Ok(RunOutcome::Complete));
        let ledger = runner.manifest().mass_ledger.clone();
        let sqlite = find_named_file(output.path(), "particles.sqlite").unwrap();
        let bundle = sqlite.with_file_name("provenance-bundle.json");
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(sqlite.with_file_name("run-manifest.json")).unwrap(),
        )
        .unwrap();
        let run_id = manifest["run_id"].as_str().unwrap();
        let sqlite_sha256 = file_sha256(&sqlite).unwrap();
        let bundle_sha256 = file_sha256(&bundle).unwrap();
        let summary =
            validate_bundle_file_semantics_loose(&bundle, run_id, &sqlite_sha256).unwrap();
        let inspection = ParticleStateSqliteSink::inspect(&sqlite).unwrap();
        let canonical =
            canonical_output_digest(&inspection.canonical_sql_sha256, &summary.content_sha256)
                .unwrap();
        (
            bundle_sha256,
            summary.content_sha256,
            inspection.canonical_sql_sha256,
            canonical,
            ledger,
        )
    };

    let a = execute(1, false, 3);
    let b = execute(4, true, 7);
    assert_ne!(a.0, b.0, "exact bundle includes distinct run identity");
    assert_eq!(a.1, b.1, "normalized content digest");
    assert_eq!(a.2, b.2, "canonical SQL digest");
    assert_eq!(a.3, b.3, "canonical output digest");
    assert_eq!(a.4, b.4, "mass ledger");
}
