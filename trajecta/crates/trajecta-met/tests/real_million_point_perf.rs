//! CFSR million-point A2 performance matrix (measurement + hard gates).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use sha2::{Digest, Sha256};
use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::{Capability, CapabilitySet, FieldRegistry};
use trajecta_met::frame::RawMetFrame;
use trajecta_met::io::frame_loader::{FrameLoadRequest, FrameLoader};
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder, MetCatalog};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::io::metrics::IoCallCounters;
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::query::cache::MemoryBudget;
use trajecta_met::query::engine::{
    BatchWorkspace, MetEngine, MetEngineConfig, RayonExecutionContext,
};
use trajecta_met::query::output::SampleStatus;
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

fn cfsr_directory() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_CFSR_DIR") {
        return PathBuf::from(path);
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let official = workspace.join("target/test-data/cfsr-ncei-pgbl-official");
    if official.join("pgbl00.gdas.2009010100.grb2").is_file() {
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

#[allow(clippy::type_complexity)]
fn load_cfsr_frames(
    directory: &Path,
    files: &[&str],
    start: Timestamp,
    end: Timestamp,
) -> Option<(
    ProfileCatalog,
    MetCatalog,
    Vec<Arc<RawMetFrame>>,
    Arc<IoCallCounters>,
)> {
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
            id: DatasetRef("cfsr-m3-million-perf".into()),
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
    let profile = profiles
        .get(&ProfileName(lock.profile.name.clone()))
        .expect("profile");
    let io_counters = IoCallCounters::new();
    let mut frames = Vec::new();
    for descriptor in catalog
        .domains
        .get(&domain)
        .expect("domain")
        .frames
        .values()
    {
        let frame = FrameLoader::load_with_io(
            FrameLoadRequest {
                descriptor,
                profile,
                required_capabilities: capabilities,
                backend: MeteorologyReaderBackend::Rust,
                previous_frame: frames.last().map(Arc::as_ref),
            },
            Some(Arc::clone(&io_counters)),
        )
        .expect("frame");
        frames.push(Arc::new(frame));
    }
    let _ = isolated;
    Some((profiles, catalog, frames, io_counters))
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

fn point_at(global: usize) -> (f64, f64, f64) {
    let lon = 0.0 + (global % 40) as f64 * 0.05;
    let lat = 48.0 + ((global / 40) % 40) as f64 * 0.05;
    (lon, lat, 5_000.0)
}

/// Fixed-seed Fisher–Yates: bijective on `0..n` (every index appears exactly once).
fn permutation_table(n: usize) -> Vec<usize> {
    let mut table: Vec<usize> = (0..n).collect();
    let mut state = 0xC0FFEE_u64 ^ (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for i in (1..n).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        table.swap(i, j);
    }
    debug_assert_eq!(table.len(), n);
    debug_assert_eq!(
        {
            let mut sorted = table.clone();
            sorted.sort_unstable();
            sorted
        },
        (0..n).collect::<Vec<_>>()
    );
    table
}

fn digest_columns(cols: &MillionPointColumns) -> String {
    let mut hasher = Sha256::new();
    for status in &cols.status {
        hasher.update(format!("{status:?}").as_bytes());
    }
    for series in [
        &cols.eastward_wind_m_s,
        &cols.northward_wind_m_s,
        &cols.geometric_vertical_velocity_m_s,
        &cols.air_pressure_pa,
        &cols.air_temperature_k,
        &cols.specific_humidity,
        &cols.air_density_kg_m3,
        &cols.terrain_height_asl_m,
    ] {
        for value in series {
            hasher.update(value.to_bits().to_le_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

fn peak_working_set_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id()),
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .ok()
    }
    #[cfg(not(windows))]
    {
        let text = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
                return Some(kb.saturating_mul(1024));
            }
        }
        None
    }
}

/// A2 performance matrix: chunk sizes, permutation, cold/hot, 1/4 threads, N vs 2N.
#[test]
fn cfsr_pgbl_million_point_performance_matrix() {
    let start = Timestamp::new(1_230_768_000, 0).unwrap();
    let end = Timestamp::new(1_230_811_200, 0).unwrap();
    let files = cfsr_three_frame_files();
    let Some((profiles, catalog, frames, io_counters)) =
        load_cfsr_frames(&cfsr_directory(), &files, start, end)
    else {
        return;
    };
    let load_snapshot = io_counters.snapshot();
    assert!(
        load_snapshot.total() > 0,
        "frame load must record real reader/provider I/O: {load_snapshot:?}"
    );
    assert!(load_snapshot.build_index > 0, "expected build_index calls");
    assert!(load_snapshot.decode > 0, "expected decode calls");
    assert!(
        load_snapshot.provider_frame_load > 0,
        "expected provider frame loads"
    );

    let budget = MemoryBudget::new(512_u64 * 1024 * 1024, 64_u64 * 1024 * 1024).unwrap();
    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles,
        fields: FieldRegistry::canonical().unwrap(),
        surface_layers: default_surface_layers(),
        memory_budget: budget,
    });
    for frame in &frames {
        engine.cache_frame(Arc::clone(frame)).unwrap();
    }

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

    // Matrix N is large enough for multi-chunk streaming but keeps wall time practical.
    // Full >1e6 gate remains in `cfsr_pgbl_million_point_budget_and_threading`.
    const BASE_N: usize = 48_000 + 17;
    const TOTAL_2N: usize = BASE_N * 2;
    // One full million-point hot pass for digest/linearity evidence.
    const FULL_MILLION: usize = 1_024_000 + 17;
    let mid = Timestamp::new(1_230_778_800, 0).unwrap();
    let io_before_prepare = io_counters.snapshot();
    let window = engine.prepare(mid).expect("prepare");
    let io_after_prepare = io_counters.snapshot();
    // Pre-cached frames: prepare itself must not open readers.
    assert_eq!(
        io_after_prepare, io_before_prepare,
        "prepare must not perform reader I/O when frames are already cached"
    );
    let mut workspace = BatchWorkspace::default();

    let mut run_points = |total: usize,
                          chunk: usize,
                          workers: usize,
                          order: Option<&[usize]>|
     -> (MillionPointColumns, u64, u64, f64) {
        let metrics_start = engine.column_cache_metrics().unwrap();
        let t0 = Instant::now();
        let mut status = Vec::with_capacity(total);
        let mut u = Vec::with_capacity(total);
        let mut v = Vec::with_capacity(total);
        let mut w = Vec::with_capacity(total);
        let mut p = Vec::with_capacity(total);
        let mut temperature = Vec::with_capacity(total);
        let mut q = Vec::with_capacity(total);
        let mut rho = Vec::with_capacity(total);
        let mut terrain = Vec::with_capacity(total);
        let mut offset = 0usize;
        let mut max_resident = 0u64;
        while offset < total {
            let take = (total - offset).min(chunk);
            let mut lon = Vec::with_capacity(take);
            let mut lat = Vec::with_capacity(take);
            let mut vert = Vec::with_capacity(take);
            for index in 0..take {
                let sequential = offset + index;
                let global = order.map(|table| table[sequential]).unwrap_or(sequential);
                let (x, y, z) = point_at(global);
                lon.push(x);
                lat.push(y);
                vert.push(z);
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
            let metrics_after_prepare = engine.column_cache_metrics().unwrap();
            let io_before_exec = io_counters.snapshot();
            let out = prepared
                .execute(
                    &RayonExecutionContext {
                        worker_threads: workers,
                    },
                    &mut workspace,
                )
                .expect("exec chunk");
            let io_after_exec = io_counters.snapshot();
            assert_eq!(
                io_after_exec, io_before_exec,
                "PreparedBatch::execute must not perform reader/provider I/O"
            );
            let metrics_after_exec = engine.column_cache_metrics().unwrap();
            // Proxy only: column-cache miss count (kept for continuity with prior reports).
            assert_eq!(
                metrics_after_exec.misses, metrics_after_prepare.misses,
                "execute phase must not incur column-cache misses (proxy for no re-fetch)"
            );
            max_resident = max_resident.max(metrics_after_exec.resident_bytes);
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
        let elapsed = t0.elapsed().as_secs_f64();
        let metrics_end = engine.column_cache_metrics().unwrap();
        (
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
            },
            metrics_end.misses.saturating_sub(metrics_start.misses),
            max_resident,
            elapsed,
        )
    };

    let order_n = permutation_table(BASE_N);
    assert_eq!(order_n.len(), BASE_N);
    {
        let mut seen = vec![false; BASE_N];
        for &idx in &order_n {
            assert!(idx < BASE_N && !seen[idx], "permutation not bijective");
            seen[idx] = true;
        }
        assert!(seen.into_iter().all(|v| v));
    }

    let (cold, cold_misses, cold_resident, cold_secs) = run_points(BASE_N, 4_096, 1, None);
    assert_eq!(cold.status.len(), BASE_N);
    assert!(
        cold_misses > 0,
        "cold pass should miss into the column cache"
    );

    let mut report_rows = Vec::new();
    for &chunk in &[1_024usize, 4_096, 16_384] {
        for (permute_label, order) in [
            ("identity", None),
            ("fisher_yates", Some(order_n.as_slice())),
        ] {
            let metrics_before = engine.column_cache_metrics().unwrap();
            let (one, one_misses, one_resident, one_secs) = run_points(BASE_N, chunk, 1, order);
            let metrics_after_one = engine.column_cache_metrics().unwrap();
            assert_eq!(
                metrics_after_one.misses, metrics_before.misses,
                "hot 1-thread must not add column-cache misses"
            );
            assert_eq!(one_misses, 0);

            let (four, four_misses, four_resident, four_secs) = run_points(BASE_N, chunk, 4, order);
            assert_eq!(four_misses, 0);
            assert_eq!(four.status, one.status);
            assert_eq!(four.eastward_wind_m_s, one.eastward_wind_m_s);
            assert_eq!(four.northward_wind_m_s, one.northward_wind_m_s);
            assert_eq!(
                four.geometric_vertical_velocity_m_s,
                one.geometric_vertical_velocity_m_s
            );
            assert_eq!(four.air_pressure_pa, one.air_pressure_pa);
            assert_eq!(four.air_temperature_k, one.air_temperature_k);
            assert_eq!(four.specific_humidity, one.specific_humidity);
            assert_eq!(four.air_density_kg_m3, one.air_density_kg_m3);
            assert_eq!(four.terrain_height_asl_m, one.terrain_height_asl_m);

            report_rows.push(serde_json::json!({
                "chunk": chunk,
                "order": permute_label,
                "permutation_bijective": true,
                "n": BASE_N,
                "workers_1_secs": one_secs,
                "workers_4_secs": four_secs,
                "engine_column_cache_resident_bytes_1": one_resident,
                "engine_column_cache_resident_bytes_4": four_resident,
                "digest_sha256": digest_columns(&one),
                "reader_zero_calls_claim": false,
                "execute_column_cache_miss_delta": 0,
                "execute_cache_note": "zero execute-phase column-cache misses is a proxy, not a counted reader/provider call meter",
            }));
        }
    }

    // Inverse-permute fisher_yates results and compare bit-for-bit to identity order.
    {
        let (identity_cols, _, _, _) = run_points(BASE_N, 4_096, 1, None);
        let (perm_cols, _, _, _) = run_points(BASE_N, 4_096, 1, Some(order_n.as_slice()));
        // perm_cols[i] corresponds to global index order_n[i].
        // So identity_cols[order_n[i]] must equal perm_cols[i].
        for (i, &g) in order_n.iter().enumerate() {
            assert_eq!(
                identity_cols.status[g], perm_cols.status[i],
                "status inv at {i}->{g}"
            );
            assert_eq!(
                identity_cols.eastward_wind_m_s[g], perm_cols.eastward_wind_m_s[i],
                "u inv"
            );
            assert_eq!(
                identity_cols.northward_wind_m_s[g], perm_cols.northward_wind_m_s[i],
                "v inv"
            );
            assert_eq!(
                identity_cols.geometric_vertical_velocity_m_s[g],
                perm_cols.geometric_vertical_velocity_m_s[i],
                "w inv"
            );
            assert_eq!(
                identity_cols.air_pressure_pa[g], perm_cols.air_pressure_pa[i],
                "p inv"
            );
            assert_eq!(
                identity_cols.air_temperature_k[g], perm_cols.air_temperature_k[i],
                "T inv"
            );
            assert_eq!(
                identity_cols.specific_humidity[g], perm_cols.specific_humidity[i],
                "q inv"
            );
            assert_eq!(
                identity_cols.air_density_kg_m3[g], perm_cols.air_density_kg_m3[i],
                "rho inv"
            );
            assert_eq!(
                identity_cols.terrain_height_asl_m[g], perm_cols.terrain_height_asl_m[i],
                "terrain inv"
            );
        }
    }

    let peak_before_scale = peak_working_set_bytes();
    let (_, _, n_resident, n_secs) = run_points(BASE_N, 4_096, 1, None);
    let peak_after_n = peak_working_set_bytes();
    let (two_n_cols, _, two_n_resident, two_n_secs) = run_points(TOTAL_2N, 4_096, 1, None);
    let peak_after_2n = peak_working_set_bytes();
    let _ = n_resident;
    assert_eq!(two_n_cols.status.len(), TOTAL_2N);
    assert!(
        two_n_secs < n_secs * 8.0 + 1.0,
        "2N pathologically super-linear: n={n_secs} 2n={two_n_secs}"
    );

    // Full million-point hot pass: complete 8-column digest (not sampling).
    let metrics_before_m = engine.column_cache_metrics().unwrap();
    let (million_cols, million_misses, million_resident, million_secs) =
        run_points(FULL_MILLION, 4_096, 1, None);
    let metrics_after_m = engine.column_cache_metrics().unwrap();
    assert_eq!(million_cols.status.len(), FULL_MILLION);
    assert_eq!(million_misses, 0, "hot million pass must not re-miss");
    assert_eq!(metrics_after_m.misses, metrics_before_m.misses);

    let ratio = if n_secs > 0.0 {
        serde_json::json!(two_n_secs / n_secs)
    } else {
        serde_json::json!(null)
    };
    let peak_rss = peak_working_set_bytes();
    let report = serde_json::json!({
        "status": "measured",
        "dataset": "cfsr_pgbl_pressure_00_06_12",
        "query_time_unix": 1_230_778_800,
        "dynamic_budget_bytes": engine.memory_budget().dynamic_bytes(),
        "cold": {
            "n": BASE_N,
            "chunk": 4096,
            "misses": cold_misses,
            "secs": cold_secs,
            "engine_column_cache_resident_bytes": cold_resident,
            "digest_sha256": digest_columns(&cold),
        },
        "matrix": report_rows,
        "scaling": {
            "n": BASE_N,
            "n_secs": n_secs,
            "two_n": TOTAL_2N,
            "two_n_secs": two_n_secs,
            "ratio_two_n_over_n": ratio,
            "two_n_engine_column_cache_resident_bytes": two_n_resident,
            "process_peak_ws_before": peak_before_scale,
            "process_peak_ws_after_n": peak_after_n,
            "process_peak_ws_after_2n": peak_after_2n,
            "temp_memory_note": "PeakWorkingSet is process-level cumulative peak, not allocator-isolated temporary bytes for N vs 2N",
            "inverse_permutation_checked": true,
        },
        "full_million_hot": {
            "n": FULL_MILLION,
            "secs": million_secs,
            "engine_column_cache_resident_bytes": million_resident,
            "digest_sha256": digest_columns(&million_cols),
        },
        "process_peak_working_set_bytes": peak_rss,
        "process_peak_method": if cfg!(windows) {
            "powershell Get-Process PeakWorkingSet64"
        } else {
            "/proc/self/status VmHWM"
        },
        "io_call_counters": {
            "after_frame_load": {
                "inspect": load_snapshot.inspect,
                "build_index": load_snapshot.build_index,
                "decode": load_snapshot.decode,
                "provider_frame_load": load_snapshot.provider_frame_load,
                "format_detection_open_attempt": load_snapshot.format_detection_open_attempt,
                "total": load_snapshot.total(),
            },
            "execute_delta_must_be_zero": true,
            "note": "Counters are Arc-shared production meters on FrameLoader/MetReader, not test mocks."
        },
        "notes": [
            "Throughput recorded only; no absolute cross-machine threshold frozen.",
            "Execute-phase IoCallCounters delta is hard-asserted zero (true reader/provider meter).",
            "Column-cache miss delta remains a secondary proxy only.",
            "Peak working set is process-level, not pure allocator accounting.",
        ],
    });
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/m3-million-point-perf.json");
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap() + "\n").unwrap();
    eprintln!("wrote {}", out.display());
}
