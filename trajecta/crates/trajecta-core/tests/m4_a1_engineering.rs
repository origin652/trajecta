//! M4-A1 engineering hard gates: RunnerBuilder -> SimulationRunner -> SQLite.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use tempfile::tempdir;
use trajecta_case::document::{
    ExecutionSpec, MeteorologyReaderBackend, ResolvedCase, ResolvedRunProfile,
};
use trajecta_case::model::metadata::Metadata;
use trajecta_case::model::numerics::{BoundarySpec, IntegratorSpec, NumericsSpec};
use trajecta_case::model::output::{
    OutputProductSpec, OutputSchedule, OutputSinkSpec, PARTICLE_STATE_PRODUCT_ID,
    PARTICLE_STATE_SQLITE_SINK_ID, default_particle_state_output,
};
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::population::{
    GeoJsonGeometry, GeoJsonSource, ParticlePopulationSpec, PopulationId, ReleaseDrivenSpec,
    ReleaseEventId, ReleaseEventSpec, ReleaseVerticalSpec,
};
use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Length, Mass, Quantity, Time as TimeDimension, Unit};
use trajecta_core::manifest::RunManifest;
use trajecta_core::output::provenance_bundle::{
    arm_quarantine_rename_fault, clear_quarantine_rename_fault, file_sha256,
    validate_bundle_file_semantics_loose,
};
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::release::geometry::SphericalGeometrySampler;
use trajecta_core::release::{GeometrySampler, ReleaseEvent, ReleaseSamplingRequest};
use trajecta_core::runner::{
    RunManifestStore, RunOutcome, RunnerBuildKnobs, RunnerBuilder, build_runner,
    build_runner_with_knobs, build_runner_with_manifest_store,
};
use trajecta_core::science::{
    GLOBAL_PERIODIC_ID, MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};
use trajecta_core::synthetic::{SyntheticWind, constant_wind_stack};

fn metres(value: f64) -> Quantity<Length> {
    Quantity::from_si(value, Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap()).unwrap()
}

fn seconds(value: f64) -> Quantity<TimeDimension> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn kilograms(value: f64) -> Quantity<Mass> {
    Quantity::from_si(value, Unit::new("kg", Dimension::Mass, 1.0, 0.0).unwrap()).unwrap()
}

fn base_case(
    direction: Direction,
    start: i64,
    end: i64,
    wind_point: [f64; 2],
    height_m: f64,
    particle_count: u64,
) -> ResolvedCase {
    ResolvedCase {
        metadata: Metadata {
            name: "m4-a1-synthetic".into(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start: Timestamp::new(start, 0).unwrap(),
            end: Timestamp::new(end, 0).unwrap(),
            direction,
        }),
        meteorology: None,
        particle_population: Some(ParticlePopulationSpec::ReleaseDriven(ReleaseDrivenSpec {
            id: PopulationId("release0".into()),
            events: vec![ReleaseEventSpec {
                id: ReleaseEventId("e0".into()),
                start: Timestamp::new(start, 0).unwrap(),
                end: Timestamp::new(start, 0).unwrap(),
                particle_count,
                mass: BTreeMap::from([(SubstanceId("tracer".into()), kilograms(1.0))]),
                geometry: GeoJsonSource::Inline {
                    geometry: GeoJsonGeometry::Point(wind_point),
                },
                vertical: ReleaseVerticalSpec::AboveSeaLevel {
                    lower: metres(height_m),
                    upper: None,
                },
            }],
        })),
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
                    ModelId(GLOBAL_PERIODIC_ID.into()),
                ],
            },
            random_seed: Some(7),
        }),
        physics: Vec::new(),
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn base_profile(output_root: PathBuf) -> ResolvedRunProfile {
    ResolvedRunProfile {
        metadata: Metadata {
            name: "m4-a1-profile".into(),
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

fn run_synthetic(
    direction: Direction,
    wind: SyntheticWind,
    point: [f64; 2],
    height_m: f64,
) -> (RunOutcome, PathBuf) {
    let dir = tempdir().unwrap();
    let case = if direction == Direction::Forward {
        base_case(direction, 0, 100, point, height_m, 4)
    } else {
        base_case(direction, 100, 0, point, height_m, 4)
    };
    let profile = base_profile(dir.path().to_path_buf());
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(100, 0).unwrap(),
        Timestamp::new(200, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    let stack = constant_wind_stack("synthetic", &times, wind, 0.0, 20_000.0, true).unwrap();
    let mut runner = build_runner(case, profile, Some(stack)).unwrap();
    let outcome = runner.run().unwrap();
    // Layout: <output_root>/<case>/<uuid-v7>/particles.sqlite
    let kept = dir.keep();
    let sqlite_path = find_particles_sqlite(&kept).expect("particles.sqlite");
    assert!(sqlite_path.is_file());
    assert!(sqlite_path.with_file_name("resolved-case.json").is_file());
    assert!(
        sqlite_path
            .with_file_name("resolved-run-profile.json")
            .is_file()
    );
    assert!(sqlite_path.with_file_name("run-manifest.json").is_file());
    // Terminal manifest must publish sqlite row counts.
    assert!(!runner.manifest().sqlite.row_counts.is_empty());
    (outcome, sqlite_path)
}

fn find_particles_sqlite(root: &std::path::Path) -> Option<PathBuf> {
    fn walk(path: &std::path::Path) -> Option<PathBuf> {
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("particles.sqlite") {
            return Some(path.to_path_buf());
        }
        if path.is_dir() {
            for entry in std::fs::read_dir(path).ok()? {
                let entry = entry.ok()?;
                if let Some(found) = walk(&entry.path()) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(root)
}

fn find_run_dir(root: &std::path::Path) -> Option<PathBuf> {
    fn walk(path: &std::path::Path) -> Option<PathBuf> {
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("resolved-case.json")
        {
            return path.parent().map(|p| p.to_path_buf());
        }
        if path.is_dir() {
            for entry in std::fs::read_dir(path).ok()? {
                let entry = entry.ok()?;
                if let Some(found) = walk(&entry.path()) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(root)
}

fn base_profile_workers(output_root: PathBuf, workers: usize) -> ResolvedRunProfile {
    let mut p = base_profile(output_root);
    p.execution.worker_threads = workers;
    p
}

#[test]
fn geometry_chunk_and_seed_independence() {
    let event = ReleaseEvent {
        id: ReleaseEventId("e0".into()),
        start: Timestamp::UNIX_EPOCH,
        end: Timestamp::UNIX_EPOCH,
        geometry: GeoJsonGeometry::LineString(vec![[0.0, 10.0], [5.0, 10.0]]),
        vertical: ReleaseVerticalSpec::AboveSeaLevel {
            lower: metres(1000.0),
            upper: None,
        },
        particle_count: 8,
        mass_kg: BTreeMap::from([(SubstanceId("t".into()), 1.0)]),
    };
    let sampler = SphericalGeometrySampler::new(event.geometry.clone()).unwrap();
    let pop = PopulationId("p".into());
    let all = sampler
        .sample_horizontal(ReleaseSamplingRequest {
            population_id: &pop,
            event: &event,
            seed: 11,
            first_ordinal: 0,
            count: 8,
        })
        .unwrap();
    let left = sampler
        .sample_horizontal(ReleaseSamplingRequest {
            population_id: &pop,
            event: &event,
            seed: 11,
            first_ordinal: 0,
            count: 3,
        })
        .unwrap();
    let right = sampler
        .sample_horizontal(ReleaseSamplingRequest {
            population_id: &pop,
            event: &event,
            seed: 11,
            first_ordinal: 3,
            count: 5,
        })
        .unwrap();
    assert_eq!(all[..3], left[..]);
    assert_eq!(all[3..], right[..]);
}

#[test]
fn builder_zero_wind_forward_writes_sqlite() {
    let (outcome, sqlite) = run_synthetic(
        Direction::Forward,
        SyntheticWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        [0.0, 0.0],
        1_000.0,
    );
    assert_eq!(outcome, RunOutcome::Complete);
    let inspection = ParticleStateSqliteSink::inspect(&sqlite).unwrap();
    assert_eq!(inspection.integrity, "ok");
    assert!(inspection.row_counts.get("particle").copied().unwrap_or(0) >= 1);
    assert!(
        inspection
            .row_counts
            .get("particle_state")
            .copied()
            .unwrap_or(0)
            >= 1
    );

    let conn = ParticleStateSqliteSink::open_readonly(&sqlite).unwrap();
    let mut stmt = conn
        .prepare("SELECT event_kind FROM output_event ORDER BY event_sequence")
        .unwrap();
    let kinds: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|value| value.unwrap())
        .collect();
    assert!(
        kinds.iter().any(|kind| kind == "birth"),
        "expected birth event, got {kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| kind == "end"),
        "expected end event at case end, got {kinds:?}"
    );
}

#[test]
fn builder_constant_east_and_backward() {
    let (outcome_f, sqlite_f) = run_synthetic(
        Direction::Forward,
        SyntheticWind {
            eastward_m_s: 10.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        [0.0, 0.0],
        1_500.0,
    );
    assert_eq!(outcome_f, RunOutcome::Complete);
    let forward = ParticleStateSqliteSink::inspect(&sqlite_f).unwrap();

    let (outcome_b, sqlite_b) = run_synthetic(
        Direction::Backward,
        SyntheticWind {
            eastward_m_s: 10.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        [0.0, 0.0],
        1_500.0,
    );
    assert_eq!(outcome_b, RunOutcome::Complete);
    let backward = ParticleStateSqliteSink::inspect(&sqlite_b).unwrap();
    assert_eq!(forward.integrity, "ok");
    assert_eq!(backward.integrity, "ok");
}

#[test]
fn rejects_unknown_integrator_id() {
    let dir = tempdir().unwrap();
    let mut case = base_case(Direction::Forward, 0, 10, [0.0, 0.0], 1000.0, 1);
    case.numerics.as_mut().unwrap().integrator.model = ModelId("nope".into());
    let profile = base_profile(dir.path().to_path_buf());
    let times = [
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(3600, 0).unwrap(),
    ];
    let stack = constant_wind_stack(
        "synthetic",
        &times,
        SyntheticWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        0.0,
        20_000.0,
        true,
    )
    .unwrap();
    let failed = build_runner(case, profile, Some(stack)).is_err();
    assert!(failed, "expected unknown integrator failure");
}

#[test]
fn runner_builder_struct_still_public() {
    // Compile-time presence check of the production entry type.
    let _ = std::mem::size_of::<RunnerBuilder>();
    let _ = PARTICLE_STATE_PRODUCT_ID;
    let _ = PARTICLE_STATE_SQLITE_SINK_ID;
    let _ = OutputProductSpec {
        product: ModelId(PARTICLE_STATE_PRODUCT_ID.into()),
        schedule: OutputSchedule::Endpoints,
        sink: OutputSinkSpec {
            model: ModelId(PARTICLE_STATE_SQLITE_SINK_ID.into()),
            parameters: BTreeMap::new(),
        },
    };
}

/// Fails on the N-th persist (1-based).
struct FailOnNthPersist {
    n: usize,
    count: usize,
    inner: Option<std::path::PathBuf>,
}

impl RunManifestStore for FailOnNthPersist {
    fn persist(&mut self, manifest: &RunManifest) -> Result<(), String> {
        self.count += 1;
        if self.count == self.n {
            return Err("forced terminal persist failure".into());
        }
        // On success, optionally write via atomic path if set.
        if let Some(path) = &self.inner {
            let body = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
            std::fs::write(path, body).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[test]
fn real_sqlite_terminal_persist_fail_quarantines_bundle() {
    let dir = tempdir().unwrap();
    let case = base_case(Direction::Forward, 0, 100, [10.0, 20.0], 1000.0, 2);
    let profile = base_profile(dir.path().to_path_buf());
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(100, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    let stack = constant_wind_stack(
        "synthetic",
        &times,
        SyntheticWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        0.0,
        20_000.0,
        true,
    )
    .unwrap();
    let store = FailOnNthPersist {
        n: 2,
        count: 0,
        inner: Some(dir.path().join("run-manifest.json")),
    };
    let mut runner =
        build_runner_with_manifest_store(case, profile, Some(stack), Box::new(store)).unwrap();
    // After build, run directory exists — pre-place frozen forensic content.
    let run_dir = find_run_dir(dir.path()).expect("run dir after build");
    let forensic_0 = run_dir.join("provenance-bundle.json.forensic-aborted");
    let frozen_old = b"FROZEN-OLD-FORENSIC-v1";
    std::fs::write(&forensic_0, frozen_old).unwrap();
    let err = runner.run().expect_err("terminal persist must fail");
    assert!(
        format!("{err:?}").contains("persist") || format!("{err:?}").contains("forced"),
        "{err:?}"
    );
    assert_eq!(
        runner.manifest().status,
        trajecta_core::manifest::RunLifecycleStatus::Failed
    );
    assert!(runner.manifest().finished_at.is_some());
    assert!(runner.manifest().provenance.is_none());
    runner.manifest().validate().expect("Failed legal");

    let sqlite =
        find_particles_sqlite(dir.path()).expect("sqlite still on disk as forensic evidence");
    let run_dir = sqlite.parent().unwrap();
    assert!(
        !run_dir.join("provenance-bundle.json").exists(),
        "formal bundle must not remain"
    );
    let forensic_0 = run_dir.join("provenance-bundle.json.forensic-aborted");
    let forensic_1 = run_dir.join("provenance-bundle.json.forensic-aborted.1");
    assert!(forensic_0.exists(), "old forensic must remain");
    assert_eq!(std::fs::read(&forensic_0).unwrap(), frozen_old);
    assert!(
        forensic_1.exists(),
        "quarantine must choose .1 when .forensic-aborted exists"
    );
    // No spool/tmp/runs
    let names: Vec<_> = std::fs::read_dir(run_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n.contains(".samples.") || n.ends_with(".tmp")),
        "ephemeral leftovers: {names:?}"
    );
    if let Ok(text) = std::fs::read_to_string(dir.path().join("run-manifest.json")) {
        assert!(
            !text.contains("\"status\": \"complete\""),
            "complete manifest must not remain"
        );
    }
}

#[test]
fn real_sqlite_terminal_persist_and_quarantine_rename_both_fail() {
    let dir = tempdir().unwrap();
    let case = base_case(Direction::Forward, 0, 100, [10.0, 20.0], 1000.0, 2);
    let profile = base_profile(dir.path().to_path_buf());
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(100, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    let stack = constant_wind_stack(
        "synthetic",
        &times,
        SyntheticWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        0.0,
        20_000.0,
        true,
    )
    .unwrap();
    let store = FailOnNthPersist {
        n: 2,
        count: 0,
        inner: Some(dir.path().join("run-manifest-store.json")),
    };
    let mut runner =
        build_runner_with_manifest_store(case, profile, Some(stack), Box::new(store)).unwrap();
    arm_quarantine_rename_fault();
    let err = runner.run().expect_err("must fail terminal+quarantine");
    clear_quarantine_rename_fault();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("persist") || msg.contains("forced"),
        "primary error missing: {msg}"
    );
    assert!(
        msg.contains("quarantine") || msg.contains("forensic rename"),
        "quarantine error missing: {msg}"
    );
    assert_eq!(
        runner.manifest().status,
        trajecta_core::manifest::RunLifecycleStatus::Failed
    );
    assert!(runner.manifest().finished_at.is_some());
    assert!(runner.manifest().provenance.is_none());
    runner.manifest().validate().expect("Failed legal");
    // Formal bundle may remain because quarantine rename failed — must match error.
    let sqlite = find_particles_sqlite(dir.path()).expect("sqlite");
    let run_dir = sqlite.parent().unwrap();
    let formal = run_dir.join("provenance-bundle.json");
    assert!(
        formal.exists(),
        "formal bundle must remain when quarantine rename fails"
    );
    // Do not claim formal was cleaned.
}

#[test]
fn production_digest_matrix_two_runs_content_and_sql_match() {
    // Retained UUID-normalization stability probe (same workers/order/chunk).
    let mk = |seed: u64| {
        let dir = tempdir().unwrap();
        let mut case = base_case(Direction::Forward, 0, 100, [10.0, 20.0], 1000.0, 2);
        case.numerics.as_mut().unwrap().random_seed = Some(seed);
        let profile = base_profile(dir.path().to_path_buf());
        let times = [
            Timestamp::new(-3_600, 0).unwrap(),
            Timestamp::new(0, 0).unwrap(),
            Timestamp::new(100, 0).unwrap(),
            Timestamp::new(3_600, 0).unwrap(),
        ];
        let stack = constant_wind_stack(
            "synthetic",
            &times,
            SyntheticWind {
                eastward_m_s: 1.0,
                northward_m_s: 0.0,
                vertical_m_s: 0.0,
            },
            0.0,
            20_000.0,
            true,
        )
        .unwrap();
        let mut runner = build_runner(case, profile, Some(stack)).unwrap();
        assert_eq!(runner.run().unwrap(), RunOutcome::Complete);
        let sqlite = find_particles_sqlite(dir.path()).unwrap();
        let run_dir = sqlite.parent().unwrap().to_path_buf();
        let bundle = run_dir.join("provenance-bundle.json");
        let manifest_path = run_dir.join("run-manifest.json");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        let prov = manifest["provenance"].as_object().unwrap().clone();
        let sqlite_sha = file_sha256(&sqlite).unwrap();
        let bundle_sha = file_sha256(&bundle).unwrap();
        let summary = validate_bundle_file_semantics_loose(
            &bundle,
            manifest["run_id"].as_str().unwrap(),
            &sqlite_sha,
        )
        .unwrap();
        let inspect = ParticleStateSqliteSink::inspect(&sqlite).unwrap();
        (
            dir,
            manifest["run_id"].as_str().unwrap().to_string(),
            bundle_sha,
            sqlite_sha,
            summary.content_sha256,
            inspect.canonical_sql_sha256,
            prov["canonical_output_sha256"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    };
    let (_d1, _u1, e1, s1, c1, q1, o1) = mk(7);
    let (_d2, _u2, e2, s2, c2, q2, o2) = mk(7);
    assert_eq!(c1, c2, "content digest");
    assert_eq!(q1, q2, "sql digest");
    assert_eq!(o1, o2, "canonical output");
    let _ = (e1, e2, s1, s2);
}

#[test]
fn production_digest_matrix_workers_order_chunk() {
    // Fixup5 frozen matrix:
    // A: workers=1, identity order, small chunk A
    // B: workers=4, reverse particle scan, different small chunk B
    let mk = |workers: usize, reverse: bool, chunk: usize| {
        let dir = tempdir().unwrap();
        let mut case = base_case(Direction::Forward, 0, 100, [10.0, 20.0], 1000.0, 4);
        case.numerics.as_mut().unwrap().random_seed = Some(7);
        let profile = base_profile_workers(dir.path().to_path_buf(), workers);
        let times = [
            Timestamp::new(-3_600, 0).unwrap(),
            Timestamp::new(0, 0).unwrap(),
            Timestamp::new(100, 0).unwrap(),
            Timestamp::new(3_600, 0).unwrap(),
        ];
        let mut stack = constant_wind_stack(
            "synthetic",
            &times,
            SyntheticWind {
                eastward_m_s: 1.0,
                northward_m_s: 0.0,
                vertical_m_s: 0.0,
            },
            0.0,
            20_000.0,
            true,
        )
        .unwrap();
        stack.execution = Box::new(trajecta_met::query::engine::RayonExecutionContext {
            worker_threads: workers,
        });
        let knobs = RunnerBuildKnobs {
            bundle_chunk_lines: Some(chunk),
            bundle_merge_fan_in: Some(2),
            reverse_particle_scan: reverse,
        };
        let mut runner = build_runner_with_knobs(case, profile, Some(stack), knobs).unwrap();
        assert_eq!(runner.run().unwrap(), RunOutcome::Complete);
        let sqlite = find_particles_sqlite(dir.path()).unwrap();
        let run_dir = sqlite.parent().unwrap().to_path_buf();
        let bundle = run_dir.join("provenance-bundle.json");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(run_dir.join("run-manifest.json")).unwrap())
                .unwrap();
        let run_id = manifest["run_id"].as_str().unwrap().to_string();
        let sqlite_sha = file_sha256(&sqlite).unwrap();
        let bundle_sha = file_sha256(&bundle).unwrap();
        let summary = validate_bundle_file_semantics_loose(&bundle, &run_id, &sqlite_sha).unwrap();
        let inspect = ParticleStateSqliteSink::inspect(&sqlite).unwrap();
        let canon = trajecta_core::output::provenance_bundle::canonical_output_digest(
            &inspect.canonical_sql_sha256,
            &summary.content_sha256,
        )
        .unwrap();
        assert_eq!(
            manifest["provenance"]["content_sha256"].as_str(),
            Some(summary.content_sha256.as_str())
        );
        assert_eq!(
            manifest["provenance"]["sqlite_sql_sha256"].as_str(),
            Some(inspect.canonical_sql_sha256.as_str())
        );
        assert_eq!(
            manifest["provenance"]["canonical_output_sha256"].as_str(),
            Some(canon.as_str())
        );
        (
            dir,
            run_id,
            workers,
            reverse,
            chunk,
            bundle_sha,
            sqlite_sha,
            summary.content_sha256,
            inspect.canonical_sql_sha256,
            canon,
        )
    };
    let (_da, ua, wa, ra, ca, ea, sa, cta, qa, oa) = mk(1, false, 3);
    let (_db, ub, wb, rb, cb, eb, sb, ctb, qb, ob) = mk(4, true, 7);
    assert_ne!(ua, ub, "UUID A != UUID B");
    assert_eq!(wa, 1);
    assert_eq!(wb, 4);
    assert!(!ra);
    assert!(rb);
    assert_eq!(ca, 3);
    assert_eq!(cb, 7);
    // Exact bundle SHA differs because run_id differs.
    assert_ne!(ea, eb, "exact bundle SHA must differ across run_id");
    // Normalized science digests match.
    assert_eq!(cta, ctb, "content");
    assert_eq!(qa, qb, "sql");
    assert_eq!(oa, ob, "canonical output");
    // Record exact SQLite SHA equality without prescribing it.
    let sqlite_sha_same = sa == sb;
    let _ = sqlite_sha_same;
}

#[test]
fn production_canonical_output_sensitive_to_sample_change() {
    // Negative: alter one sample assignment in a finished bundle → content/canon change.
    let dir = tempdir().unwrap();
    let case = base_case(Direction::Forward, 0, 100, [10.0, 20.0], 1000.0, 2);
    let profile = base_profile(dir.path().to_path_buf());
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(100, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    let stack = constant_wind_stack(
        "synthetic",
        &times,
        SyntheticWind {
            eastward_m_s: 1.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        0.0,
        20_000.0,
        true,
    )
    .unwrap();
    let mut runner = build_runner(case, profile, Some(stack)).unwrap();
    assert_eq!(runner.run().unwrap(), RunOutcome::Complete);
    let sqlite = find_particles_sqlite(dir.path()).unwrap();
    let run_dir = sqlite.parent().unwrap();
    let bundle = run_dir.join("provenance-bundle.json");
    let sqlite_sha = file_sha256(&sqlite).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("run-manifest.json")).unwrap()).unwrap();
    let run_id = manifest["run_id"].as_str().unwrap();
    let before = validate_bundle_file_semantics_loose(&bundle, run_id, &sqlite_sha).unwrap();
    let inspect = ParticleStateSqliteSink::inspect(&sqlite).unwrap();
    let before_canon = trajecta_core::output::provenance_bundle::canonical_output_digest(
        &inspect.canonical_sql_sha256,
        &before.content_sha256,
    )
    .unwrap();
    // Flip one field_set_sha hex nibble in the samples array text.
    let text = std::fs::read_to_string(&bundle).unwrap();
    let marker = "\"field_set_sha256\": \"";
    let Some(idx) = text.find(marker) else {
        panic!("no field_set_sha256 in bundle");
    };
    let start = idx + marker.len();
    let mut altered = text.into_bytes();
    if altered[start] == b'a' {
        altered[start] = b'b';
    } else {
        altered[start] = b'a';
    }
    std::fs::write(&bundle, &altered).unwrap();
    // Semantics validator should fail (rehash/coverage) OR if it somehow passes,
    // content digest must differ. Prefer hard fail on tamper.
    let after = validate_bundle_file_semantics_loose(&bundle, run_id, &sqlite_sha);
    match after {
        Err(_) => {}
        Ok(summary) => {
            assert_ne!(summary.content_sha256, before.content_sha256);
            let after_canon = trajecta_core::output::provenance_bundle::canonical_output_digest(
                &inspect.canonical_sql_sha256,
                &summary.content_sha256,
            )
            .unwrap();
            assert_ne!(after_canon, before_canon);
        }
    }
}
