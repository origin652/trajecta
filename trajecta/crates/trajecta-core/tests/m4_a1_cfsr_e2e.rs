//! Real CFSR 00/06 to 03 UTC production RunnerBuilder coverage E2E.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::type_complexity
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    GeoJsonGeometry, GeoJsonSource, ParticlePopulationSpec, PopulationId, ReleaseDrivenSpec,
    ReleaseEventId, ReleaseEventSpec, ReleaseVerticalSpec,
};
use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Length, Mass, Quantity, Time as TimeDimension, Unit};
use trajecta_core::output::provenance_bundle::file_sha256;
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::runner::{RunOutcome, build_runner};
use trajecta_core::science::{
    GLOBAL_PERIODIC_ID, MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};
use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::ProfileCatalog;

fn metres(value: f64) -> Quantity<Length> {
    Quantity::from_si(value, Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap()).unwrap()
}
fn seconds(value: f64) -> Quantity<TimeDimension> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}
fn kilograms(value: f64) -> Quantity<Mass> {
    Quantity::from_si(value, Unit::new("kg", Dimension::Mass, 1.0, 0.0).unwrap()).unwrap()
}

fn fixture_directory() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_CFSR_DIR") {
        return PathBuf::from(path);
    }
    // trajecta-core -> crates -> trajecta -> flexpart
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("flexpart root")
        .join("tools/flexctl/target/test-data/cfsr/20090101/raw")
}

fn require_real_met() -> bool {
    matches!(
        std::env::var("TRAJECTA_REQUIRE_REAL_MET").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn walkdir_simple(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(path: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(path) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    walk(root, &mut out);
    out
}

/// 2009-01-01 03:00 UTC short case must load CFSR 00 and 06 via production path.
#[test]
fn cfsr_03utc_runnerbuilder_loads_00_06_and_writes_sqlite() {
    let source_directory = fixture_directory();
    let file_00 = source_directory.join("pgbl00.gdas.2009010100.grb2");
    let file_06 = source_directory.join("pgbl00.gdas.2009010106.grb2");
    let file_12 = source_directory.join("pgbl00.gdas.2009010112.grb2");
    if !file_00.is_file() || !file_06.is_file() {
        if require_real_met() {
            panic!(
                "TRAJECTA_REQUIRE_REAL_MET=1 but missing CFSR pgbl 00/06 under {source_directory:?}"
            );
        }
        eprintln!(
            "skip: CFSR fixtures missing at {}",
            source_directory.display()
        );
        return;
    }

    let data_dir = tempdir().unwrap();
    for name in ["pgbl00.gdas.2009010100.grb2", "pgbl00.gdas.2009010106.grb2"] {
        fs::copy(source_directory.join(name), data_dir.path().join(name)).unwrap();
    }
    if file_12.is_file() {
        fs::copy(
            &file_12,
            data_dir.path().join("pgbl00.gdas.2009010112.grb2"),
        )
        .unwrap();
    }

    let profiles = ProfileCatalog::load(&[]).expect("profiles");
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(Capability::Transport);

    let t00 = Timestamp::new(1_230_768_000, 0).unwrap();
    let t03 = Timestamp::new(1_230_768_000 + 3 * 3600, 0).unwrap();
    let t12 = Timestamp::new(1_230_768_000 + 12 * 3600, 0).unwrap();

    let lock_req = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("cfsr-pgbl-m4a1".into()),
            source: "NOAA CFSR pgbl".into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-core-m4a1-test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), data_dir.path().to_path_buf())]),
        coverage: LockCoverageRequest {
            start: t00,
            end: t12,
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities,
        force_rehash: true,
        preferred_profile: None,
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&lock_req);
    assert!(outcome.is_success(), "{:?}", outcome.diagnostics);
    let lock = outcome.lock.expect("lock");
    let lock_path = data_dir.path().join("dataset-lock.json");
    fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();

    let out_dir = tempdir().unwrap();
    let domain = DomainId("cfsr".into());
    let dataset = DatasetRef("cfsr-pgbl-m4a1".into());

    let case = ResolvedCase {
        metadata: Metadata {
            name: "m4-a1-cfsr-03utc".into(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start: t03,
            end: Timestamp::new(t03.seconds_since_unix_epoch() + 600, 0).unwrap(),
            direction: Direction::Forward,
        }),
        meteorology: Some(MeteorologySpec {
            domains: vec![DomainSpec {
                id: domain.clone(),
                dataset: dataset.clone(),
                priority: 0,
                parent: None,
                horizontal_halo_cells: 1,
            }],
        }),
        particle_population: Some(ParticlePopulationSpec::ReleaseDriven(ReleaseDrivenSpec {
            id: PopulationId("release0".into()),
            events: vec![ReleaseEventSpec {
                id: ReleaseEventId("e0".into()),
                start: t03,
                end: t03,
                particle_count: 2,
                mass: BTreeMap::from([(SubstanceId("tracer".into()), kilograms(1.0))]),
                geometry: GeoJsonSource::Inline {
                    geometry: GeoJsonGeometry::Point([0.0, 0.0]),
                },
                vertical: ReleaseVerticalSpec::AboveSeaLevel {
                    lower: metres(1500.0),
                    upper: None,
                },
            }],
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
                    ModelId(GLOBAL_PERIODIC_ID.into()),
                ],
            },
            random_seed: Some(7),
        }),
        physics: None,
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    };

    let profile = ResolvedRunProfile {
        metadata: Metadata {
            name: "m4-a1-cfsr-profile".into(),
            ..Metadata::default()
        },
        case_path: data_dir.path().join("case.json"),
        output_root: out_dir.path().to_path_buf(),
        datasets: vec![DatasetBinding {
            dataset: dataset.clone(),
            lockfile: lock_path,
            cache_root: None,
            data_roots: BTreeMap::from([(DataRootId("met".into()), data_dir.path().to_path_buf())]),
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
    fs::write(
        &profile.case_path,
        serde_json::to_vec_pretty(&case).unwrap(),
    )
    .unwrap();

    let mut runner = build_runner(case, profile, None).expect("production RunnerBuilder");
    let outcome = runner.run().expect("run");
    // Formal CFSR anchor: no abnormal particle termination may be masked.
    assert_eq!(outcome, RunOutcome::Complete, "{outcome:?}");

    let manifest_path = walkdir_simple(out_dir.path())
        .into_iter()
        .find(|p| p.ends_with("run-manifest.json"))
        .expect("run-manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(
        manifest["status"].as_str(),
        Some("complete"),
        "status={:?}",
        manifest["status"]
    );
    assert_eq!(
        manifest["terminations"]["abnormal_count"].as_u64(),
        Some(0),
        "abnormal terminations must be 0: {:?}",
        manifest["terminations"]
    );
    let content = manifest["inputs"]["dataset_content_sha256"]
        .as_object()
        .expect("content sha map");
    assert!(
        content.keys().any(|k| k.contains("2009010100")),
        "00 UTC missing: {content:?}"
    );
    assert!(
        content.keys().any(|k| k.contains("2009010106")),
        "06 UTC missing: {content:?}"
    );
    assert!(
        !manifest["execution"]["reader_backends"]
            .as_object()
            .map(|m| m.is_empty())
            .unwrap_or(true),
        "reader_backends empty"
    );

    let sqlite_path = walkdir_simple(out_dir.path())
        .into_iter()
        .find(|p| p.ends_with("particles.sqlite"))
        .expect("particles.sqlite");
    let bundle_path = walkdir_simple(out_dir.path())
        .into_iter()
        .find(|p| p.ends_with("provenance-bundle.json"))
        .expect("provenance-bundle.json");
    let bundle: serde_json::Value =
        serde_json::from_slice(&fs::read(&bundle_path).unwrap()).unwrap();
    assert_eq!(
        bundle["schema_version"].as_str(),
        Some("trajecta.provenance-bundle/v1")
    );
    assert!(
        bundle["samples"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "bundle samples empty"
    );
    let prov = manifest["provenance"]
        .as_object()
        .expect("manifest.provenance required");
    assert_eq!(
        prov["relative_path"].as_str(),
        Some("provenance-bundle.json")
    );
    let bundle_sha_disk = file_sha256(&bundle_path).expect("bundle sha");
    let sqlite_sha_disk = file_sha256(&sqlite_path).expect("sqlite sha");
    assert_eq!(
        prov["sha256"].as_str(),
        Some(bundle_sha_disk.as_str()),
        "manifest bundle sha must equal on-disk stream SHA"
    );
    assert_eq!(
        prov["sqlite_sha256"].as_str(),
        Some(sqlite_sha_disk.as_str()),
        "manifest sqlite sha must equal on-disk stream SHA"
    );
    assert_eq!(
        bundle["sqlite"]["sha256"].as_str(),
        Some(sqlite_sha_disk.as_str()),
        "bundle.sqlite.sha256 mismatch"
    );
    assert_eq!(
        bundle["run_id"].as_str(),
        manifest["run_id"].as_str(),
        "run_id mismatch bundle/manifest"
    );
    let samples = bundle["samples"].as_array().expect("samples");
    let records = bundle["records"].as_array().expect("records");
    let field_sets = bundle["field_sets"].as_array().expect("field_sets");
    assert_eq!(prov["sample_count"].as_u64(), Some(samples.len() as u64));
    assert_eq!(prov["record_count"].as_u64(), Some(records.len() as u64));
    assert_eq!(
        prov["field_set_count"].as_u64(),
        Some(field_sets.len() as u64)
    );
    // Production digests must be published (exact + content + SQL + canonical-output).
    for key in [
        "content_sha256",
        "sqlite_sql_sha256",
        "canonical_output_sha256",
    ] {
        assert_eq!(
            prov[key].as_str().map(|s| s.len()),
            Some(64),
            "missing/invalid {key}"
        );
    }
    let inspect = ParticleStateSqliteSink::inspect(&sqlite_path).expect("inspect");
    assert_eq!(
        prov["sqlite_sql_sha256"].as_str(),
        Some(inspect.canonical_sql_sha256.as_str()),
        "manifest sqlite_sql digest must match inspect recompute"
    );
    assert_eq!(inspect.sha256, sqlite_sha_disk);
    // Independent on-disk content + canonical-output recompute (not length-only).
    let run_id = manifest["run_id"].as_str().expect("run_id");
    let summary = trajecta_core::output::provenance_bundle::validate_bundle_file_semantics_loose(
        &bundle_path,
        run_id,
        &sqlite_sha_disk,
    )
    .expect("stream validate on-disk bundle");
    assert_eq!(
        prov["content_sha256"].as_str(),
        Some(summary.content_sha256.as_str()),
        "manifest content digest must match on-disk streaming recompute"
    );
    let canon = trajecta_core::output::provenance_bundle::canonical_output_digest(
        inspect.canonical_sql_sha256.as_str(),
        summary.content_sha256.as_str(),
    )
    .expect("canonical output");
    assert_eq!(
        prov["canonical_output_sha256"].as_str(),
        Some(canon.as_str()),
        "manifest canonical_output must match recompute"
    );
    // Every sample field_set ref exists; five slots only.
    let fs_shas: std::collections::BTreeSet<&str> = field_sets
        .iter()
        .filter_map(|f| f["sha256"].as_str())
        .collect();
    let rec_shas: std::collections::BTreeSet<&str> = records
        .iter()
        .filter_map(|r| r["sha256"].as_str())
        .collect();
    for s in samples {
        let fs = s["field_set_sha256"].as_str().expect("fs sha");
        assert!(fs_shas.contains(fs), "missing field_set {fs}");
    }
    for fs in field_sets {
        let fields = fs["fields"].as_object().expect("fields");
        for slot in [
            "eastward_wind",
            "northward_wind",
            "geometric_vertical_velocity",
            "air_pressure",
            "air_temperature",
        ] {
            assert!(fields.contains_key(slot), "missing slot {slot}");
            if let Some(sha) = fields[slot].as_str() {
                assert!(rec_shas.contains(sha), "slot {slot} -> missing record");
            }
        }
    }
    // SQLite row count equals sample count (full coverage).
    let sql_rows: i64 = {
        let c = ParticleStateSqliteSink::open_readonly(&sqlite_path).unwrap();
        c.query_row("SELECT COUNT(*) FROM particle_state", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(sql_rows as u64, samples.len() as u64);
    // No engineering sidecar as acceptance evidence.
    assert!(
        !walkdir_simple(out_dir.path())
            .into_iter()
            .any(|p| p.ends_with("provenance-table.json")),
        "legacy provenance-table.json must not be written"
    );
    assert_eq!(inspect.integrity, "ok");
    assert!(
        inspect
            .row_counts
            .get("particle_state")
            .copied()
            .unwrap_or(0)
            >= 2
    );

    let conn = ParticleStateSqliteSink::open_readonly(&sqlite_path).unwrap();
    let (u, v, w, p, temp): (
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
    ) = conn
        .query_row(
            "SELECT eastward_wind_m_s, northward_wind_m_s, geometric_vertical_velocity_m_s,
                    air_pressure_pa, air_temperature_k
             FROM particle_state LIMIT 1",
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
    assert!(u.is_some() && v.is_some(), "U/V missing {u:?}/{v:?}");
    assert!(w.is_some(), "W missing");
    assert!(p.is_some() && temp.is_some(), "P/T missing {p:?}/{temp:?}");
}
