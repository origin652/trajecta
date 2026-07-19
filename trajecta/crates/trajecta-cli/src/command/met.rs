//! # Contract: meteorology probe and replay commands
//!
//! Commands construct a meteorology engine from local data roots, prepare
//! explicit query batches, and render output. They never implement reader or
//! interpolation details themselves.
//!
//! Streaming contract:
//! - Input is consumed in bounded point chunks.
//! - Each chunk may regroup by time/vertical coordinate for execution.
//! - Results are written in the caller's chunk-local order before the next
//!   chunk is loaded.
//! - Summary retains only counters, never the full sample set.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::{Capability, CapabilitySet, FieldRegistry};
use trajecta_met::frame::FrameError;
use trajecta_met::io::frame_loader::{FrameLoadRequest, FrameLoader};
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::provenance::{ProvenanceId, ProvenanceRecord};
use trajecta_met::query::cache::MemoryBudget;
use trajecta_met::query::engine::{
    BatchWorkspace, EngineError, MetEngine, MetEngineConfig, RayonExecutionContext,
};
use trajecta_met::query::output::{SampleStatus, TransportOutput};
use trajecta_met::query::request::{
    ExplainMode, QueryBatch, QueryPlanError, QueryPointArrays, TransportPlan, TransportPlanRequest,
    VerticalQuery,
};
use trajecta_met::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerRegistry};
use trajecta_met::vertical::VerticalBounds;

/// Maximum points retained in memory for one streaming chunk.
pub const STREAM_CHUNK_POINTS: usize = 1_024;

/// Stable diagnostic codes for machine consumers.
pub mod diagnostic {
    /// Exact-frame transport lacks previous and next physical frames for kinematic W.
    pub const MISSING_SYMMETRIC_TIME_SUPPORT: &str = "met.missing_symmetric_time_support";
    /// Generic runtime failure while preparing or executing meteorology.
    pub const RUNTIME: &str = "met.runtime";
    /// Caller argument or path problem.
    pub const USAGE: &str = "met.usage";
}

/// Meteorology command selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetCommand {
    /// Single-time probe with optional JSONL points and summary/explain.
    Probe {
        /// Local meteorology data root.
        data_root: PathBuf,
        /// Exact built-in or external Profile name.
        profile: String,
        /// Query time as Unix seconds.
        time_unix: i64,
        /// Optional JSONL query points path (`-` = stdin).
        points: Option<PathBuf>,
        /// Reader backend.
        backend: MeteorologyReaderBackend,
        /// Coverage start Unix seconds (inclusive). Defaults to `time_unix - 12h`.
        coverage_start_unix: Option<i64>,
        /// Coverage end Unix seconds (inclusive). Defaults to `time_unix + 12h`.
        coverage_end_unix: Option<i64>,
        /// Whether estimated fields may be planned.
        allow_estimated: bool,
        /// Emit probe_summary.
        summary: bool,
        /// Attach explain records.
        explain: bool,
        /// Write machine JSONL to this path (`-` = stdout).
        output: Option<PathBuf>,
    },
    /// Streamed multi-time JSONL replay with stable input-order recovery.
    Replay {
        /// Local meteorology data root.
        data_root: PathBuf,
        /// Exact Profile name.
        profile: String,
        /// JSONL input path (`-` = stdin).
        input: PathBuf,
        /// JSONL output path (`-` = stdout).
        output: PathBuf,
        /// Reader backend.
        backend: MeteorologyReaderBackend,
        /// Coverage start Unix seconds.
        coverage_start_unix: Option<i64>,
        /// Coverage end Unix seconds.
        coverage_end_unix: Option<i64>,
        /// Whether estimated fields may be planned.
        allow_estimated: bool,
        /// Attach explain records.
        explain: bool,
        /// Always emit a final probe_summary.
        summary: bool,
    },
}

/// One JSONL query point (schema version 0).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ProbePointRecord {
    /// Stable caller identity restored in output order.
    pub id: String,
    /// Unix seconds of the query time. Missing/null is invalid; `0` is epoch.
    pub time_unix: Option<i64>,
    /// Longitude degrees.
    pub longitude_degrees: f64,
    /// Latitude degrees.
    pub latitude_degrees: f64,
    /// Vertical coordinate kind.
    pub vertical_coordinate: String,
    /// Vertical coordinate value.
    pub vertical: f64,
}

/// JSON-friendly vertical bounds.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct BoundsRecord {
    /// Terrain height ASL m.
    pub terrain_asl_m: f64,
    /// Lowest transport height AGL m.
    pub minimum_transport_agl_m: f64,
    /// Available top ASL m.
    pub available_top_asl_m: f64,
    /// Physical model top ASL m when known.
    pub physical_model_top_asl_m: Option<f64>,
    /// Maximum/local surface pressure Pa.
    pub maximum_pressure_pa: f64,
    /// Minimum queryable pressure Pa.
    pub minimum_pressure_pa: f64,
}

/// Independently resolvable provenance payload.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ProvenanceJson {
    /// Compact table identifier.
    pub id: u32,
    /// Field key debug form.
    pub field: String,
    /// Quality label.
    pub quality: String,
    /// Source identities.
    pub sources: Vec<String>,
    /// Transform chain.
    pub transforms: Vec<TransformJson>,
    /// Optional fallback reason.
    pub fallback_reason: Option<String>,
    /// Profile fingerprint.
    pub profile_sha256: String,
}

/// One transform step in provenance.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TransformJson {
    /// Operation id.
    pub operation: String,
    /// Parameters.
    pub parameters: Vec<(String, String)>,
}

/// Stable machine record written by probe/replay.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ProbeResultRecord {
    /// Schema version.
    pub schema_version: u32,
    /// Record kind.
    pub kind: &'static str,
    /// Caller identity.
    pub id: String,
    /// Query time.
    pub time_unix: i64,
    /// Point status.
    pub status: String,
    /// Optional field values when valid.
    pub fields: BTreeMap<String, Option<f64>>,
    /// Optional quality labels.
    pub quality: BTreeMap<String, String>,
    /// Resolved provenance records keyed by field name.
    pub provenance: BTreeMap<String, ProvenanceJson>,
    /// Local vertical bounds when available.
    pub bounds: Option<BoundsRecord>,
    /// Optional explain blob.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain: Option<serde_json::Value>,
}

/// Final summary record.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ProbeSummaryRecord {
    /// Schema version.
    pub schema_version: u32,
    /// Record kind.
    pub kind: &'static str,
    /// Profile name.
    pub profile: String,
    /// Backend.
    pub backend: String,
    /// Points processed.
    pub point_count: usize,
    /// Ok count.
    pub ok_count: usize,
    /// Non-ok count.
    pub non_ok_count: usize,
    /// Distinct query times observed.
    pub time_count: usize,
}

/// Met command execution failure with stable exit semantics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetCommandError {
    /// User/argument problem (exit 2).
    Usage {
        /// Stable diagnostic code.
        code: &'static str,
        /// Human-readable message.
        message: String,
    },
    /// Data/runtime failure (exit 1).
    Runtime {
        /// Stable diagnostic code.
        code: &'static str,
        /// Human-readable message.
        message: String,
    },
}

impl MetCommandError {
    /// Stable process exit code.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Usage { .. } => 2,
            Self::Runtime { .. } => 1,
        }
    }

    /// Stable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Usage { code, .. } | Self::Runtime { code, .. } => code,
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        Self::Usage {
            code: diagnostic::USAGE,
            message: message.into(),
        }
    }

    fn runtime(code: &'static str, message: impl Into<String>) -> Self {
        Self::Runtime {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for MetCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage { code, message } | Self::Runtime { code, message } => {
                write!(formatter, "{code}: {message}")
            }
        }
    }
}

/// Executes one meteorology command and writes JSONL/summary.
pub fn execute(command: &MetCommand) -> Result<i32, MetCommandError> {
    match command {
        MetCommand::Probe {
            data_root,
            profile,
            time_unix,
            points,
            backend,
            coverage_start_unix,
            coverage_end_unix,
            allow_estimated,
            summary,
            explain,
            output,
        } => {
            let coverage_start = coverage_start_unix.unwrap_or(time_unix.saturating_sub(43_200));
            let coverage_end = coverage_end_unix.unwrap_or(time_unix.saturating_add(43_200));
            let mut engine =
                build_engine(data_root, profile, *backend, coverage_start, coverage_end)?;
            let plan = compile_plan(&mut engine, *allow_estimated, *explain)?;
            let mut writer = open_output(output.as_deref())?;
            let mut counters = SummaryCounters::default();
            let mut time_tracker = UniqueTimeTracker::create()?;
            stream_points(
                points.as_deref(),
                Some(*time_unix),
                &mut engine,
                &plan,
                *explain,
                &mut writer,
                &mut counters,
                &mut time_tracker,
            )?;
            if *summary {
                write_summary(
                    &mut writer,
                    profile,
                    backend_label(*backend),
                    &counters,
                    time_tracker.unique_count()?,
                )?;
            }
            Ok(0)
        }
        MetCommand::Replay {
            data_root,
            profile,
            input,
            output,
            backend,
            coverage_start_unix,
            coverage_end_unix,
            allow_estimated,
            explain,
            summary,
        } => {
            let (stream_path, coverage_start, coverage_end, spool_guard) =
                resolve_replay_input(input, *coverage_start_unix, *coverage_end_unix)?;
            let mut engine =
                build_engine(data_root, profile, *backend, coverage_start, coverage_end)?;
            let plan = compile_plan(&mut engine, *allow_estimated, *explain)?;
            let mut writer = open_output(Some(output.as_path()))?;
            let mut counters = SummaryCounters::default();
            let mut time_tracker = UniqueTimeTracker::create()?;
            stream_points(
                Some(stream_path.as_path()),
                None,
                &mut engine,
                &plan,
                *explain,
                &mut writer,
                &mut counters,
                &mut time_tracker,
            )?;
            drop(spool_guard);
            if *summary {
                write_summary(
                    &mut writer,
                    profile,
                    backend_label(*backend),
                    &counters,
                    time_tracker.unique_count()?,
                )?;
            }
            Ok(0)
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SummaryCounters {
    point_count: usize,
    ok_count: usize,
    non_ok_count: usize,
}

/// Maximum open run files during one external-merge fan-in round.
///
/// Keeps peak FD usage O(MERGE_FAN_IN) even for million-point spills
/// (~ceil(N / STREAM_CHUNK_POINTS) initial runs).
pub const MERGE_FAN_IN: usize = 32;

/// Tracks distinct query times with bounded memory and bounded open FDs.
///
/// Values are appended as little-endian i64 to a spill file. Unique counting:
/// 1. read fixed-size runs (`STREAM_CHUNK_POINTS`) — never `read_to_end`;
/// 2. multi-round k-way merge with fan-in `MERGE_FAN_IN`;
/// 3. RAII cleanup of every temporary run path.
struct UniqueTimeTracker {
    path: PathBuf,
    file: File,
    observed: usize,
    token: u128,
}

/// Owns temporary run files and deletes them on drop (including panic paths).
struct TempRunSet {
    paths: Vec<PathBuf>,
}

impl TempRunSet {
    fn new() -> Self {
        Self { paths: Vec::new() }
    }

    fn push(&mut self, path: PathBuf) {
        self.paths.push(path);
    }

    fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    fn clear_deleted(&mut self) {
        self.paths.clear();
    }
}

impl Drop for TempRunSet {
    fn drop(&mut self) {
        for path in self.paths.drain(..) {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl UniqueTimeTracker {
    fn create() -> Result<Self, MetCommandError> {
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "trajecta-met-times-{}-{token}.bin",
            std::process::id()
        ));
        let file = File::create(&path).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("create time tracker: {error}"))
        })?;
        Ok(Self {
            path,
            file,
            observed: 0,
            token,
        })
    }

    fn observe(&mut self, time_unix: i64) -> Result<(), MetCommandError> {
        self.file
            .write_all(&time_unix.to_le_bytes())
            .map_err(|error| {
                MetCommandError::runtime(
                    diagnostic::RUNTIME,
                    format!("write time tracker: {error}"),
                )
            })?;
        self.observed = self.observed.saturating_add(1);
        Ok(())
    }

    fn unique_count(mut self) -> Result<usize, MetCommandError> {
        self.file.flush().map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("flush time tracker: {error}"))
        })?;
        drop(self.file);

        if self.observed == 0 {
            let _ = std::fs::remove_file(&self.path);
            return Ok(0);
        }

        let mut runs = TempRunSet::new();
        let mut input = File::open(&self.path).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("open time tracker: {error}"))
        })?;
        let mut buffer = Vec::with_capacity(STREAM_CHUNK_POINTS);
        let mut raw = [0_u8; 8];
        let mut remaining = self.observed;
        let mut run_serial = 0usize;
        while remaining > 0 {
            let take = remaining.min(STREAM_CHUNK_POINTS);
            buffer.clear();
            for _ in 0..take {
                input.read_exact(&mut raw).map_err(|error| {
                    MetCommandError::runtime(
                        diagnostic::RUNTIME,
                        format!("read time tracker (truncated or short): {error}"),
                    )
                })?;
                buffer.push(i64::from_le_bytes(raw));
            }
            runs.push(write_sorted_run(&buffer, self.token, run_serial)?);
            run_serial += 1;
            remaining -= take;
        }
        let _ = std::fs::remove_file(&self.path);

        // Multi-round fan-in merge: each round opens at most MERGE_FAN_IN files.
        let mut level = 0usize;
        while runs.paths().len() > 1 {
            let mut next = TempRunSet::new();
            let current = runs.paths().to_vec();
            for (batch_index, batch) in current.chunks(MERGE_FAN_IN).enumerate() {
                let merged = merge_sorted_runs(batch, self.token, level, batch_index)?;
                next.push(merged);
            }
            // Drop previous level files via TempRunSet::drop.
            runs = next;
            level = level.saturating_add(1);
        }

        let Some(final_path) = runs.paths().first().cloned() else {
            return Ok(0);
        };
        let mut file = File::open(&final_path).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("open final time run: {error}"))
        })?;
        let mut unique = 0usize;
        let mut last: Option<i64> = None;
        while let Some(value) = read_i64(&mut file)? {
            if last != Some(value) {
                unique += 1;
                last = Some(value);
            }
        }
        // Explicit clear so Drop does not double-delete after we finish reading.
        drop(file);
        runs.clear_deleted();
        let _ = std::fs::remove_file(final_path);
        Ok(unique)
    }
}

fn write_sorted_run(
    values: &[i64],
    token: u128,
    run_serial: usize,
) -> Result<PathBuf, MetCommandError> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let run_path = std::env::temp_dir().join(format!(
        "trajecta-met-time-run-{}-{token}-{run_serial}.bin",
        std::process::id()
    ));
    let mut out = File::create(&run_path).map_err(|error| {
        MetCommandError::runtime(diagnostic::RUNTIME, format!("create time run: {error}"))
    })?;
    for value in sorted {
        out.write_all(&value.to_le_bytes()).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("write time run: {error}"))
        })?;
    }
    Ok(run_path)
}

fn merge_sorted_runs(
    inputs: &[PathBuf],
    token: u128,
    level: usize,
    batch_index: usize,
) -> Result<PathBuf, MetCommandError> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    if inputs.len() == 1 {
        // Promote path ownership to the next TempRunSet by renaming into a new name
        // only when needed; single-input batch reuses the path by hard-linking copy.
        let out = std::env::temp_dir().join(format!(
            "trajecta-met-time-merge-{}-{token}-L{level}-B{batch_index}.bin",
            std::process::id()
        ));
        std::fs::copy(&inputs[0], &out).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("copy single run: {error}"))
        })?;
        return Ok(out);
    }

    let out_path = std::env::temp_dir().join(format!(
        "trajecta-met-time-merge-{}-{token}-L{level}-B{batch_index}.bin",
        std::process::id()
    ));
    let mut out = File::create(&out_path).map_err(|error| {
        MetCommandError::runtime(diagnostic::RUNTIME, format!("create merge run: {error}"))
    })?;

    // At most MERGE_FAN_IN open readers.
    debug_assert!(inputs.len() <= MERGE_FAN_IN);
    let mut heap = BinaryHeap::new();
    let mut readers = Vec::with_capacity(inputs.len());
    for (index, path) in inputs.iter().enumerate() {
        let mut file = File::open(path).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("open time run: {error}"))
        })?;
        if let Some(value) = read_i64(&mut file)? {
            heap.push(Reverse((value, index)));
        }
        readers.push(file);
    }

    let mut last: Option<i64> = None;
    while let Some(Reverse((value, run_index))) = heap.pop() {
        // Dedup across runs while merging so final pass is unique-only when one run remains.
        // Intermediate levels keep duplicates so later unique_count final scan is correct either way;
        // deduping early reduces intermediate size and is safe for unique counting.
        if last != Some(value) {
            out.write_all(&value.to_le_bytes()).map_err(|error| {
                MetCommandError::runtime(diagnostic::RUNTIME, format!("write merge: {error}"))
            })?;
            last = Some(value);
        }
        if let Some(next) = read_i64(&mut readers[run_index])? {
            heap.push(Reverse((next, run_index)));
        }
    }
    Ok(out_path)
}

fn read_i64(file: &mut File) -> Result<Option<i64>, MetCommandError> {
    let mut buf = [0_u8; 8];
    match file.read_exact(&mut buf) {
        Ok(()) => Ok(Some(i64::from_le_bytes(buf))),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(MetCommandError::runtime(
            diagnostic::RUNTIME,
            format!("read time run value: {error}"),
        )),
    }
}

struct SpoolGuard {
    path: Option<PathBuf>,
}

impl Drop for SpoolGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn resolve_replay_input(
    input: &Path,
    coverage_start_unix: Option<i64>,
    coverage_end_unix: Option<i64>,
) -> Result<(PathBuf, i64, i64, SpoolGuard), MetCommandError> {
    match (coverage_start_unix, coverage_end_unix) {
        (Some(start), Some(end)) => {
            Ok((input.to_path_buf(), start, end, SpoolGuard { path: None }))
        }
        _ => {
            // Single-pass: spool the entire stream once, derive coverage bounds,
            // then the caller re-reads the spool (never re-reads stdin).
            let mut reader = open_reader(Some(input))?;
            let (spool, min_time, max_time) = spool_jsonl_for_coverage(&mut reader)?;
            Ok((
                spool.clone(),
                coverage_start_unix.unwrap_or(min_time.saturating_sub(43_200)),
                coverage_end_unix.unwrap_or(max_time.saturating_add(43_200)),
                SpoolGuard { path: Some(spool) },
            ))
        }
    }
}

/// Copy JSONL from `reader` to a temporary spool while computing min/max times.
///
/// This is the coverage-omitted branch used by `replay --input -` without
/// `--coverage-start/end`. Peak memory is O(one line).
fn spool_jsonl_for_coverage(
    reader: &mut dyn BufRead,
) -> Result<(PathBuf, i64, i64), MetCommandError> {
    let spool = std::env::temp_dir().join(format!(
        "trajecta-met-spool-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut writer = File::create(&spool).map_err(|error| {
        MetCommandError::runtime(diagnostic::RUNTIME, format!("create spool: {error}"))
    })?;
    let mut min_time = None;
    let mut max_time = None;
    let mut line = String::new();
    let mut saw = false;
    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("read input: {error}"))
        })?;
        if read == 0 {
            break;
        }
        writer.write_all(line.as_bytes()).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("write spool: {error}"))
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let record: ProbePointRecord = serde_json::from_str(&line)
            .map_err(|error| MetCommandError::usage(format!("invalid JSONL point: {error}")))?;
        let time = require_time(&record)?;
        min_time = Some(min_time.map_or(time, |value: i64| value.min(time)));
        max_time = Some(max_time.map_or(time, |value: i64| value.max(time)));
        saw = true;
    }
    if !saw {
        let _ = std::fs::remove_file(&spool);
        return Err(MetCommandError::usage("replay input JSONL is empty"));
    }
    let min_time = min_time.ok_or_else(|| MetCommandError::usage("replay input JSONL is empty"))?;
    let max_time = max_time.ok_or_else(|| MetCommandError::usage("replay input JSONL is empty"))?;
    Ok((spool, min_time, max_time))
}

fn require_time(record: &ProbePointRecord) -> Result<i64, MetCommandError> {
    record.time_unix.ok_or_else(|| {
        MetCommandError::usage(format!(
            "point `{}` is missing required time_unix (0 is valid epoch and must be explicit)",
            record.id
        ))
    })
}

fn backend_label(backend: MeteorologyReaderBackend) -> &'static str {
    match backend {
        MeteorologyReaderBackend::Rust => "rust",
        MeteorologyReaderBackend::Native => "native",
    }
}

/// Emit non-fatal lock notes as machine-readable JSONL on stderr.
fn emit_lock_notes(notes: &[trajecta_met::io::lock_builder::LockBuildDiagnostic]) {
    use std::io::Write as _;
    let mut stderr = std::io::stderr().lock();
    for note in notes {
        let path = note
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let line = serde_json::json!({
            "schema_version": 0,
            "kind": "lock_note",
            "stage": format!("{:?}", note.stage),
            "code": note.code,
            "message": note.message,
            "path": path,
        });
        let _ = writeln!(stderr, "{line}");
    }
}

fn compile_plan(
    engine: &mut MetEngine,
    allow_estimated: bool,
    explain: bool,
) -> Result<TransportPlan, MetCommandError> {
    engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated,
                explain: if explain {
                    ExplainMode::Full
                } else {
                    ExplainMode::Disabled
                },
                ..TransportPlanRequest::default()
            },
            &ExecutionPlan::default(),
        )
        .map_err(|error| {
            MetCommandError::runtime(
                diagnostic::RUNTIME,
                format!("compile_transport_plan: {error:?}"),
            )
        })
}

fn transport_field_registry() -> Result<FieldRegistry, MetCommandError> {
    FieldRegistry::canonical().map_err(|error| {
        MetCommandError::runtime(
            diagnostic::RUNTIME,
            format!("build canonical field registry: {error:?}"),
        )
    })
}

fn build_engine(
    data_root: &Path,
    profile_name: &str,
    backend: MeteorologyReaderBackend,
    coverage_start_unix: i64,
    coverage_end_unix: i64,
) -> Result<MetEngine, MetCommandError> {
    if !data_root.is_dir() {
        return Err(MetCommandError::usage(format!(
            "data root is not a directory: {}",
            data_root.display()
        )));
    }
    let profiles = ProfileCatalog::load(&[]).map_err(|error| {
        MetCommandError::runtime(
            diagnostic::RUNTIME,
            format!("load Profile catalog: {error:?}"),
        )
    })?;
    if profiles.get(&ProfileName(profile_name.into())).is_none() {
        return Err(MetCommandError::usage(format!(
            "unknown profile `{profile_name}`"
        )));
    }
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(Capability::Transport);
    capabilities.insert(Capability::NearSurfaceTransport);
    let inspector = ReaderMetadataInspector::new(backend);
    let mut hash_cache = FileHashCache::new();
    let start = Timestamp::new(coverage_start_unix, 0)
        .map_err(|error| MetCommandError::usage(format!("coverage start: {error:?}")))?;
    let end = Timestamp::new(coverage_end_unix, 0)
        .map_err(|error| MetCommandError::usage(format!("coverage end: {error:?}")))?;
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("cli-met".into()),
            source: profile_name.into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-cli".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), data_root.to_path_buf())]),
        coverage: LockCoverageRequest {
            start,
            end,
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities,
        force_rehash: false,
        preferred_profile: Some(profile_name.into()),
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    emit_lock_notes(&outcome.notes);
    if !outcome.is_success() {
        return Err(MetCommandError::runtime(
            diagnostic::RUNTIME,
            format!("dataset lock failed: {:?}", outcome.diagnostics),
        ));
    }
    let lock = outcome
        .lock
        .ok_or_else(|| MetCommandError::runtime(diagnostic::RUNTIME, "dataset lock missing"))?;
    if lock.profile.name != profile_name {
        return Err(MetCommandError::runtime(
            diagnostic::RUNTIME,
            format!(
                "locked profile `{}` differs from requested `{profile_name}`",
                lock.profile.name
            ),
        ));
    }
    let domain = DomainId("cli".into());
    let roots = BTreeMap::from([(DataRootId("met".into()), data_root.to_path_buf())]);
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir: data_root,
        data_roots: &roots,
        domain: &domain,
        required_capabilities: capabilities,
    });
    if !inventory.is_success() {
        return Err(MetCommandError::runtime(
            diagnostic::RUNTIME,
            format!("inventory failed: {:?}", inventory.diagnostics.sorted()),
        ));
    }
    let catalog = inventory.catalog.ok_or_else(|| {
        MetCommandError::runtime(diagnostic::RUNTIME, "inventory catalog missing")
    })?;
    let mut frames = Vec::new();
    {
        let profile = profiles
            .get(&ProfileName(profile_name.into()))
            .ok_or_else(|| MetCommandError::usage(format!("unknown profile `{profile_name}`")))?;
        for descriptor in catalog
            .domains
            .get(&domain)
            .into_iter()
            .flat_map(|d| d.frames.values())
        {
            let frame = FrameLoader::load(FrameLoadRequest {
                descriptor,
                profile,
                required_capabilities: capabilities,
                backend,
                previous_frame: frames.last().map(Arc::as_ref),
            })
            .map_err(|error| {
                MetCommandError::runtime(diagnostic::RUNTIME, format!("frame load: {error:?}"))
            })?;
            frames.push(Arc::new(frame));
        }
    }
    if frames.is_empty() {
        return Err(MetCommandError::runtime(
            diagnostic::RUNTIME,
            "no frames loaded for requested coverage",
        ));
    }
    let mut surface_layers = SurfaceLayerRegistry::new();
    surface_layers
        .register(Arc::new(MoninObukhovBusingerDyer::default()))
        .map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("surface layer: {error:?}"))
        })?;
    let budget = MemoryBudget::new(512 * 1024 * 1024, 64 * 1024 * 1024).map_err(|error| {
        MetCommandError::runtime(diagnostic::RUNTIME, format!("memory budget: {error:?}"))
    })?;
    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles,
        fields: transport_field_registry()?,
        surface_layers,
        memory_budget: budget,
    });
    for frame in frames {
        engine.cache_frame(frame).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("cache_frame: {error:?}"))
        })?;
    }
    Ok(engine)
}

fn open_reader(path: Option<&Path>) -> Result<Box<dyn BufRead>, MetCommandError> {
    match path {
        None => Ok(Box::new(std::io::Cursor::new(Vec::<u8>::new()))),
        Some(path) if path.as_os_str() == "-" => Ok(Box::new(BufReader::new(std::io::stdin()))),
        Some(path) => Ok(Box::new(BufReader::new(File::open(path).map_err(
            |error| MetCommandError::usage(format!("open points {}: {error}", path.display())),
        )?))),
    }
}

fn open_output(path: Option<&Path>) -> Result<Box<dyn Write>, MetCommandError> {
    match path {
        None => Ok(Box::new(BufWriter::new(std::io::stdout()))),
        Some(path) if path.as_os_str() == "-" => Ok(Box::new(BufWriter::new(std::io::stdout()))),
        Some(path) => Ok(Box::new(BufWriter::new(File::create(path).map_err(
            |error| MetCommandError::usage(format!("open output {}: {error}", path.display())),
        )?))),
    }
}

fn write_json_line<T: serde::Serialize>(
    writer: &mut dyn Write,
    value: &T,
) -> Result<(), MetCommandError> {
    serde_json::to_writer(&mut *writer, value).map_err(|error| {
        MetCommandError::runtime(diagnostic::RUNTIME, format!("write json: {error}"))
    })?;
    writer.write_all(b"\n").map_err(|error| {
        MetCommandError::runtime(diagnostic::RUNTIME, format!("write newline: {error}"))
    })?;
    Ok(())
}

fn write_summary(
    writer: &mut dyn Write,
    profile: &str,
    backend: &str,
    counters: &SummaryCounters,
    time_count: usize,
) -> Result<(), MetCommandError> {
    write_json_line(
        writer,
        &ProbeSummaryRecord {
            schema_version: 0,
            kind: "probe_summary",
            profile: profile.into(),
            backend: backend.into(),
            point_count: counters.point_count,
            ok_count: counters.ok_count,
            non_ok_count: counters.non_ok_count,
            time_count,
        },
    )
}

fn parse_vertical(kind: &str) -> Result<VerticalQuery, MetCommandError> {
    match kind {
        "agl" | "AGL" | "above_ground" => Ok(VerticalQuery::AboveGround),
        "asl" | "ASL" | "above_sea_level" => Ok(VerticalQuery::AboveSeaLevel),
        "pa" | "Pa" | "pressure" => Ok(VerticalQuery::Pressure),
        other => Err(MetCommandError::usage(format!(
            "unsupported vertical_coordinate `{other}` (expected agl|asl|pa)"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_points(
    path: Option<&Path>,
    default_time_unix: Option<i64>,
    engine: &mut MetEngine,
    plan: &TransportPlan,
    explain: bool,
    writer: &mut dyn Write,
    counters: &mut SummaryCounters,
    time_tracker: &mut UniqueTimeTracker,
) -> Result<(), MetCommandError> {
    if path.is_none() {
        let time = default_time_unix.ok_or_else(|| {
            MetCommandError::usage("probe requires --time when --points is omitted")
        })?;
        let chunk = vec![ProbePointRecord {
            id: "default-0".into(),
            time_unix: Some(time),
            longitude_degrees: 0.0,
            latitude_degrees: 50.0,
            vertical_coordinate: "agl".into(),
            vertical: 100.0,
        }];
        return process_chunk(
            &chunk,
            engine,
            plan,
            explain,
            writer,
            counters,
            time_tracker,
        );
    }

    let mut reader = open_reader(path)?;
    let mut chunk = Vec::with_capacity(STREAM_CHUNK_POINTS);
    let mut line_no = 0usize;
    let mut saw_any = false;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|error| {
            MetCommandError::runtime(diagnostic::RUNTIME, format!("read points: {error}"))
        })?;
        if read == 0 {
            break;
        }
        line_no += 1;
        if line.trim().is_empty() {
            continue;
        }
        let record: ProbePointRecord = serde_json::from_str(&line).map_err(|error| {
            MetCommandError::usage(format!("invalid JSONL point at line {line_no}: {error}"))
        })?;
        let _ = require_time(&record)?;
        chunk.push(record);
        saw_any = true;
        if chunk.len() >= STREAM_CHUNK_POINTS {
            process_chunk(
                &chunk,
                engine,
                plan,
                explain,
                writer,
                counters,
                time_tracker,
            )?;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        process_chunk(
            &chunk,
            engine,
            plan,
            explain,
            writer,
            counters,
            time_tracker,
        )?;
    }
    if !saw_any {
        if let Some(time) = default_time_unix {
            let chunk = vec![ProbePointRecord {
                id: "default-0".into(),
                time_unix: Some(time),
                longitude_degrees: 0.0,
                latitude_degrees: 50.0,
                vertical_coordinate: "agl".into(),
                vertical: 100.0,
            }];
            process_chunk(
                &chunk,
                engine,
                plan,
                explain,
                writer,
                counters,
                time_tracker,
            )?;
        } else {
            return Err(MetCommandError::usage("input JSONL is empty"));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_chunk(
    records: &[ProbePointRecord],
    engine: &mut MetEngine,
    plan: &TransportPlan,
    explain: bool,
    writer: &mut dyn Write,
    counters: &mut SummaryCounters,
    time_tracker: &mut UniqueTimeTracker,
) -> Result<(), MetCommandError> {
    let mut groups: BTreeMap<(i64, String), Vec<usize>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        let time = require_time(record)?;
        groups
            .entry((time, record.vertical_coordinate.clone()))
            .or_default()
            .push(index);
        time_tracker.observe(time)?;
    }
    let mut results: Vec<Option<ProbeResultRecord>> = vec![None; records.len()];
    let mut workspace = BatchWorkspace::default();
    for ((time_unix, vertical_kind), indices) in groups {
        let time = Timestamp::new(time_unix, 0)
            .map_err(|error| MetCommandError::usage(format!("point time: {error:?}")))?;
        let vertical = parse_vertical(&vertical_kind)?;
        let batch = QueryBatch {
            vertical_coordinate: vertical,
            points: QueryPointArrays {
                longitude_degrees: indices
                    .iter()
                    .map(|index| records[*index].longitude_degrees)
                    .collect(),
                latitude_degrees: indices
                    .iter()
                    .map(|index| records[*index].latitude_degrees)
                    .collect(),
                vertical: indices
                    .iter()
                    .map(|index| records[*index].vertical)
                    .collect(),
            },
        };
        let window = engine
            .prepare(time)
            .map_err(|error| map_engine_error("prepare", error))?;
        let prepared = window
            .prepare_transport_batch(plan, batch, &mut workspace)
            .map_err(|error| map_engine_error("prepare_transport_batch", error))?;
        let output = prepared
            .execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            .map_err(|error| map_engine_error("execute", error))?;
        for (batch_index, original_index) in indices.into_iter().enumerate() {
            let record =
                transport_result_record(&records[original_index], &output, batch_index, explain)?;
            results[original_index] = Some(record);
        }
    }
    for record in results.into_iter().flatten() {
        if record.status == "ok" {
            counters.ok_count += 1;
        } else {
            counters.non_ok_count += 1;
        }
        counters.point_count += 1;
        write_json_line(writer, &record)?;
    }
    Ok(())
}

fn map_engine_error(stage: &str, error: EngineError) -> MetCommandError {
    match error {
        EngineError::Frame(FrameError::MissingSymmetricTimeSupport) => MetCommandError::runtime(
            diagnostic::MISSING_SYMMETRIC_TIME_SUPPORT,
            format!(
                "{stage}: exact-frame transport requires previous and next physical frames for kinematic W"
            ),
        ),
        EngineError::QueryPlan(QueryPlanError::EstimatedFieldForbidden(field)) => {
            MetCommandError::runtime(
                diagnostic::RUNTIME,
                format!(
                    "{stage}: estimated field forbidden under allow_estimated=false: {field:?}"
                ),
            )
        }
        other => MetCommandError::runtime(diagnostic::RUNTIME, format!("{stage}: {other:?}")),
    }
}

fn transport_result_record(
    input: &ProbePointRecord,
    output: &TransportOutput,
    index: usize,
    explain: bool,
) -> Result<ProbeResultRecord, MetCommandError> {
    let row = output
        .row(index)
        .ok_or_else(|| MetCommandError::runtime(diagnostic::RUNTIME, "missing transport row"))?;
    let status = status_label(row.status());
    let mut fields = BTreeMap::new();
    fields.insert("eastward_wind_m_s".into(), row.eastward_wind_m_s());
    fields.insert("northward_wind_m_s".into(), row.northward_wind_m_s());
    fields.insert(
        "geometric_vertical_velocity_m_s".into(),
        row.geometric_vertical_velocity_m_s(),
    );
    fields.insert("air_pressure_pa".into(), row.air_pressure_pa());
    fields.insert("air_temperature_k".into(), row.air_temperature_k());
    fields.insert("specific_humidity".into(), row.specific_humidity());
    fields.insert("air_density_kg_m3".into(), row.air_density_kg_m3());
    fields.insert("terrain_height_asl_m".into(), row.terrain_height_asl_m());

    let columns = output.columns();
    let table = output.provenance();
    let mut quality = BTreeMap::new();
    let mut provenance = BTreeMap::new();
    for (name, column) in [
        ("eastward_wind_m_s", &columns.eastward_wind_m_s),
        ("northward_wind_m_s", &columns.northward_wind_m_s),
        (
            "geometric_vertical_velocity_m_s",
            &columns.geometric_vertical_velocity_m_s,
        ),
        ("air_pressure_pa", &columns.air_pressure_pa),
        ("air_temperature_k", &columns.air_temperature_k),
        ("specific_humidity", &columns.specific_humidity),
        ("air_density_kg_m3", &columns.air_density_kg_m3),
        ("terrain_height_asl_m", &columns.terrain_height_asl_m),
    ] {
        quality.insert(name.into(), format!("{:?}", column.quality()[index]));
        let id = column.provenance()[index];
        provenance.insert(name.into(), resolve_provenance(table, id)?);
    }

    let bounds = row.bounds().map(bounds_record);
    let explain_value = if explain {
        output
            .explain()
            .and_then(|records| records.get(index))
            .and_then(|record| record.as_ref())
            .map(|record| {
                serde_json::json!({
                    "domain": record.domain.0,
                    "cell": record.cell.0,
                    "before_frame": format!("{:?}", record.before_frame),
                    "after_frame": format!("{:?}", record.after_frame),
                    "before_weight": record.before_weight,
                    "after_weight": record.after_weight,
                    "horizontal": {
                        "points": record.horizontal.points.iter().map(|point| {
                            serde_json::json!({"x": point.x, "y": point.y})
                        }).collect::<Vec<_>>(),
                        "first_level_weights": record.horizontal.first_level_weights,
                        "first_level_method": explain_horizontal_method_label(
                            record.horizontal.first_level_method,
                        ),
                        "second_level_weights": record.horizontal.second_level_weights,
                        "second_level_method": record.horizontal.second_level_method.map(
                            explain_horizontal_method_label,
                        ),
                    },
                    "vertical": {
                        "path": explain_vertical_path_label(record.vertical.path),
                        "first_level": record.vertical.first_level,
                        "second_level": record.vertical.second_level,
                        "second_weight": record.vertical.second_weight,
                    },
                    "surface_model": record.surface_model.as_ref().map(|id| id.0.as_str()),
                    "fields": record.fields.iter().map(|field| {
                        serde_json::json!({
                            "field": format!("{:?}", field.field),
                            "quality": format!("{:?}", field.quality),
                            "provenance": field.provenance.0,
                        })
                    }).collect::<Vec<_>>(),
                })
            })
    } else {
        None
    };

    Ok(ProbeResultRecord {
        schema_version: 0,
        kind: "probe_result",
        id: input.id.clone(),
        time_unix: require_time(input)?,
        status: status.into(),
        fields,
        quality,
        provenance,
        bounds,
        explain: explain_value,
    })
}

fn explain_horizontal_method_label(
    method: trajecta_met::query::output::ExplainHorizontalMethod,
) -> &'static str {
    use trajecta_met::query::output::ExplainHorizontalMethod;

    match method {
        ExplainHorizontalMethod::Bilinear => "bilinear",
        ExplainHorizontalMethod::ValidTriangle => "valid_triangle",
        ExplainHorizontalMethod::TimeBlended => "time_blended",
    }
}

fn explain_vertical_path_label(
    path: trajecta_met::query::output::ExplainVerticalPath,
) -> &'static str {
    use trajecta_met::query::output::ExplainVerticalPath;

    match path {
        ExplainVerticalPath::LinearHeight => "linear_height",
        ExplainVerticalPath::LogPressure => "log_pressure",
        ExplainVerticalPath::SurfaceLayer => "surface_layer",
        ExplainVerticalPath::Unavailable => "unavailable",
    }
}

fn resolve_provenance(
    table: &trajecta_met::provenance::ProvenanceTable,
    id: ProvenanceId,
) -> Result<ProvenanceJson, MetCommandError> {
    let record = table.get(id).ok_or_else(|| {
        MetCommandError::runtime(
            diagnostic::RUNTIME,
            format!("unresolved provenance id {}", id.0),
        )
    })?;
    Ok(provenance_json(id, record))
}

fn provenance_json(id: ProvenanceId, record: &ProvenanceRecord) -> ProvenanceJson {
    ProvenanceJson {
        id: id.0,
        field: format!("{:?}", record.field),
        quality: format!("{:?}", record.quality),
        sources: record.sources.clone(),
        transforms: record
            .transforms
            .iter()
            .map(|transform| TransformJson {
                operation: transform.operation.clone(),
                parameters: transform.parameters.clone(),
            })
            .collect(),
        fallback_reason: record.fallback_reason.clone(),
        profile_sha256: record.profile_sha256.clone(),
    }
}

fn status_label(status: SampleStatus) -> &'static str {
    match status {
        SampleStatus::Ok => "ok",
        SampleStatus::OutOfDomain => "out_of_domain",
        SampleStatus::PolarSingularity => "polar_singularity",
        SampleStatus::BelowGround => "below_ground",
        SampleStatus::SurfaceLayerUndefined => "surface_layer_undefined",
        SampleStatus::AboveAvailableTop => "above_available_top",
        SampleStatus::AboveModelTop => "above_model_top",
        SampleStatus::InvalidVerticalColumn => "invalid_vertical_column",
        SampleStatus::NumericalFailure => "numerical_failure",
    }
}

fn bounds_record(bounds: VerticalBounds) -> BoundsRecord {
    BoundsRecord {
        terrain_asl_m: bounds.terrain_asl_m(),
        minimum_transport_agl_m: bounds.minimum_transport_agl_m(),
        available_top_asl_m: bounds.available_top_asl_m(),
        physical_model_top_asl_m: bounds.physical_model_top_asl_m(),
        maximum_pressure_pa: bounds.maximum_pressure_pa(),
        minimum_pressure_pa: bounds.minimum_pressure_pa(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use trajecta_met::field::{CanonicalField, FieldKey};

    #[test]
    fn missing_symmetric_maps_to_stable_code() {
        let error = map_engine_error(
            "prepare_transport_batch",
            EngineError::Frame(FrameError::MissingSymmetricTimeSupport),
        );
        assert_eq!(error.code(), diagnostic::MISSING_SYMMETRIC_TIME_SUPPORT);
        assert_eq!(error.exit_code(), 1);
    }

    #[test]
    fn stream_chunk_constant_is_bounded() {
        const {
            assert!(STREAM_CHUNK_POINTS <= 4_096);
            assert!(STREAM_CHUNK_POINTS >= 64);
        }
    }

    #[test]
    fn transport_registry_registers_core_fields() {
        let registry = transport_field_registry().unwrap();
        assert!(
            registry
                .get(&FieldKey::Canonical(CanonicalField::EastwardWind))
                .is_some()
        );
        assert!(
            registry
                .get(&FieldKey::Canonical(CanonicalField::GeometricTerrainHeight))
                .is_some()
        );
    }

    #[test]
    fn unique_time_tracker_counts_many_values_with_chunked_runs() {
        let mut tracker = UniqueTimeTracker::create().unwrap();
        // More than one run; includes duplicates and epoch 0.
        let total = STREAM_CHUNK_POINTS * 3 + 11;
        for index in 0..total {
            let time = i64::try_from(index % 97).unwrap(); // 97 unique including 0
            tracker.observe(time).unwrap();
        }
        assert_eq!(tracker.unique_count().unwrap(), 97);
    }

    #[test]
    fn require_time_accepts_epoch_zero() {
        let record = ProbePointRecord {
            id: "e".into(),
            time_unix: Some(0),
            longitude_degrees: 0.0,
            latitude_degrees: 0.0,
            vertical_coordinate: "agl".into(),
            vertical: 1.0,
        };
        assert_eq!(require_time(&record).unwrap(), 0);
    }

    #[test]
    fn unique_time_tracker_multi_round_fan_in_merge() {
        // Enough initial runs to force at least two merge rounds under MERGE_FAN_IN.
        let mut tracker = UniqueTimeTracker::create().unwrap();
        let run_count = MERGE_FAN_IN * 2 + 3;
        let total = STREAM_CHUNK_POINTS * run_count;
        let unique_mod = 131i64;
        for index in 0..total {
            tracker
                .observe(i64::try_from(index).unwrap() % unique_mod)
                .unwrap();
        }
        assert_eq!(tracker.unique_count().unwrap(), unique_mod as usize);
    }

    #[test]
    fn spool_jsonl_for_coverage_single_pass_derives_bounds() {
        let body = "{\"id\":\"a\",\"time_unix\":10,\"longitude_degrees\":0.0,\"latitude_degrees\":0.0,\"vertical_coordinate\":\"agl\",\"vertical\":1.0}
{\"id\":\"b\",\"time_unix\":0,\"longitude_degrees\":1.0,\"latitude_degrees\":1.0,\"vertical_coordinate\":\"agl\",\"vertical\":1.0}
{\"id\":\"c\",\"time_unix\":42,\"longitude_degrees\":2.0,\"latitude_degrees\":2.0,\"vertical_coordinate\":\"agl\",\"vertical\":1.0}
";
        let mut cursor = std::io::Cursor::new(body.as_bytes());
        let (spool, min_time, max_time) = spool_jsonl_for_coverage(&mut cursor).unwrap();
        assert_eq!(min_time, 0);
        assert_eq!(max_time, 42);
        let text = std::fs::read_to_string(&spool).unwrap();
        assert_eq!(text.lines().count(), 3);
        // Second pass can re-read the spool (stdin simulation).
        let mut second = std::io::BufReader::new(std::fs::File::open(&spool).unwrap());
        let mut line = String::new();
        let mut ids = Vec::new();
        loop {
            line.clear();
            if second.read_line(&mut line).unwrap() == 0 {
                break;
            }
            let record: ProbePointRecord = serde_json::from_str(&line).unwrap();
            ids.push(record.id);
        }
        assert_eq!(ids, vec!["a", "b", "c"]);
        let _ = std::fs::remove_file(spool);
    }
}
