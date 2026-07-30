//! Read-only M5 result products and the derived deterministic run report.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use rusqlite::{Connection, Rows};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use trajecta_core::manifest::{RunLifecycleStatus, RunManifest};
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_job::history::JobAttemptHistory;
use trajecta_job::model::JobState;
use uuid::Uuid;

use crate::cli::OutputMode;
use crate::command::staged::TrajectorySelection;

const MANIFEST_NAME: &str = "run-manifest.json";
const REPORT_NAME: &str = "run-report.md";
const FORENSIC_PREFIX: &str = "provenance-bundle.json.forensic-aborted";

/// A resolved run directory and optional durable catalog identity.
#[derive(Clone, Debug)]
pub(crate) struct ResultProductInput {
    /// Attempt root containing the result artifacts.
    pub run_directory: PathBuf,
    /// Exact catalog attempt when the user selected the result by ID.
    pub catalog: Option<JobAttemptHistory>,
}

/// One non-fatal inspection diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProductWarning {
    /// Stable machine diagnostic code.
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

/// Result inspection data plus explicit partial-inspection warnings.
#[derive(Clone, Debug)]
pub(crate) struct InspectionProduct {
    /// Stable `trajecta.result-inspection/v1` payload.
    pub data: Value,
    /// Non-fatal warnings rendered by the command envelope.
    pub warnings: Vec<ProductWarning>,
}

/// Stable product error rendered through the normal command envelope.
#[derive(Clone, Debug)]
pub(crate) struct ProductError {
    /// Stable machine diagnostic code.
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
    /// Whether a human/JSONL trajectory stream already emitted bytes.
    pub stream_started: bool,
}

impl ProductError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            stream_started: false,
        }
    }

    fn after_stream_started(mut self) -> Self {
        self.stream_started = true;
        self
    }
}

#[derive(Debug)]
struct SqliteProductSummary {
    row_counts: BTreeMap<String, u64>,
    quality: Value,
    termination_by_reason: BTreeMap<String, u64>,
    normal_termination_count: u64,
    abnormal_termination_count: u64,
}

/// Reads the stable inspection product without modifying result artifacts.
pub(crate) fn inspect(input: &ResultProductInput) -> Result<InspectionProduct, ProductError> {
    let root = canonical_run_directory(&input.run_directory)?;
    let manifest_path = required_artifact(&root, Path::new(MANIFEST_NAME))?;
    let manifest = read_manifest(&manifest_path)?;
    let catalog = catalog_summary(input.catalog.as_ref(), &manifest, &root)?;

    let sqlite_relative = normal_relative_path(&manifest.sqlite.relative_path)?;
    let sqlite_path = root.join(&sqlite_relative);
    let formal_status = matches!(
        manifest.status,
        RunLifecycleStatus::Complete
            | RunLifecycleStatus::CompletedWithParticleErrors
            | RunLifecycleStatus::Cancelled
    );
    if formal_status && !sqlite_path.is_file() {
        return Err(ProductError::new(
            "result.artifact_missing",
            format!(
                "required artifact missing: {}",
                slash_path(&sqlite_relative)?
            ),
        ));
    }

    let provenance_relative = manifest
        .provenance
        .as_ref()
        .map(|identity| normal_relative_path(Path::new(&identity.relative_path)))
        .transpose()?
        .unwrap_or_else(|| PathBuf::from("provenance-bundle.json"));
    if formal_status
        && (manifest.provenance.is_none() || !root.join(&provenance_relative).is_file())
    {
        return Err(ProductError::new(
            "result.artifact_missing",
            "required terminal provenance bundle is absent",
        ));
    }

    let mut warnings = Vec::new();
    let sqlite = if sqlite_path.is_file() {
        match inspect_sqlite(&sqlite_path, manifest.sqlite.schema_version) {
            Ok(summary) => {
                if !manifest.sqlite.row_counts.is_empty()
                    && manifest.sqlite.row_counts != summary.row_counts
                {
                    return Err(ProductError::new(
                        "result.manifest_sqlite_count_mismatch",
                        "manifest SQLite row counts do not match the on-disk artifact",
                    ));
                }
                Some(summary)
            }
            Err(error) => {
                warnings.push(ProductWarning {
                    code: "result.inspect_sqlite_unavailable",
                    message: error.message,
                });
                None
            }
        }
    } else {
        None
    };

    let particles = sqlite.as_ref().map(particle_summary);
    let quality = sqlite
        .as_ref()
        .map_or(Value::Null, |summary| summary.quality.clone());
    let mass_ledger = mass_ledger_summary(&manifest)?;

    let sqlite_wal = PathBuf::from(format!("{}-wal", slash_path(&sqlite_relative)?));
    let mut artifacts = vec![
        artifact_descriptor(&root, "manifest", Path::new(MANIFEST_NAME), None)?,
        artifact_descriptor(&root, "sqlite", &sqlite_relative, None)?,
        artifact_descriptor(&root, "sqlite_wal", &sqlite_wal, None)?,
        artifact_descriptor(
            &root,
            "provenance",
            &provenance_relative,
            manifest
                .provenance
                .as_ref()
                .map(|identity| identity.sha256.as_str()),
        )?,
        artifact_descriptor(&root, "run_report", Path::new(REPORT_NAME), None)?,
    ];
    artifacts.extend(forensic_descriptors(&root)?);

    let result_path = root.to_str().ok_or_else(|| {
        ProductError::new(
            "result.path_not_utf8",
            "canonical result directory is not valid UTF-8",
        )
    })?;
    Ok(InspectionProduct {
        data: json!({
            "schema_version": "trajecta.result-inspection/v1",
            "result_path": result_path,
            "identity": {
                "job_series_id": manifest.job_series_id,
                "run_id": manifest.run_id,
                "attempt": manifest.attempt,
                "case_name": manifest.case_name,
            },
            "lifecycle": {
                "status": manifest.status,
                "run_success": manifest.status.run_success(),
                "started_at": manifest.started_at,
                "finished_at": manifest.finished_at,
                "failure": manifest.failure,
            },
            "software": manifest.software,
            "inputs": manifest.inputs,
            "execution": manifest.execution,
            "numerical": manifest.numerical,
            "particles": particles,
            "quality": quality,
            "mass_ledger": mass_ledger,
            "artifacts": artifacts,
            "catalog": catalog,
        }),
        warnings,
    })
}

/// Renders a concise deterministic human inspection summary.
pub(crate) fn render_inspection_human(product: &InspectionProduct) -> String {
    let data = &product.data;
    let identity = &data["identity"];
    let lifecycle = &data["lifecycle"];
    let particles = &data["particles"];
    let execution = &data["execution"];
    let catalog = &data["catalog"];
    let mut lines = vec![format!(
        "result run_id={} series={} attempt={} status={} success={}",
        display_value(&identity["run_id"]),
        display_value(&identity["job_series_id"]),
        display_value(&identity["attempt"]),
        display_value(&lifecycle["status"]),
        display_value(&lifecycle["run_success"]),
    )];
    lines.push(format!(
        "particles={} states={} terminations={} abnormal={}",
        display_value(&particles["particle_count"]),
        display_value(&particles["state_count"]),
        display_value(&particles["termination_count"]),
        display_value(&particles["abnormal_termination_count"]),
    ));
    lines.push(format!(
        "resources workers={} memory_budget_bytes={} wall_time_ns={}",
        display_value(&execution["worker_threads"]),
        display_value(&execution["memory_budget_bytes"]),
        display_value(&execution["wall_time_ns"]),
    ));
    lines.push(format!(
        "verification={} superseded_by={}",
        display_value(&catalog["full_verification"]),
        display_value(&catalog["superseded_by"]),
    ));
    lines.push(format!("path={}", display_value(&data["result_path"])));
    for warning in &product.warnings {
        lines.push(format!("warning {}: {}", warning.code, warning.message));
    }
    lines.join("\n")
}

/// Streams selected trajectory records without whole-result buffering.
pub(crate) fn trajectory(
    input: &ResultProductInput,
    selection: &TrajectorySelection,
    output: OutputMode,
) -> Result<i32, ProductError> {
    let root = canonical_run_directory(&input.run_directory)?;
    let manifest = read_manifest(&required_artifact(&root, Path::new(MANIFEST_NAME))?)?;
    catalog_summary(input.catalog.as_ref(), &manifest, &root)?;
    let sqlite_relative = normal_relative_path(&manifest.sqlite.relative_path)?;
    let sqlite_path = required_artifact(&root, &sqlite_relative)?;
    let connection = ParticleStateSqliteSink::open_readonly(&sqlite_path)
        .map_err(|error| ProductError::new("result.sqlite_invalid", format!("{error:?}")))?;

    let ids = match selection {
        TrajectorySelection::All => Vec::new(),
        TrajectorySelection::ParticleIds(ids) => ids.clone(),
    };
    validate_requested_particles(&connection, &manifest.run_id.0, &ids)?;

    let header = json!({
        "schema_version": "trajecta.trajectory-stream/v1",
        "job_series_id": manifest.job_series_id,
        "run_id": manifest.run_id,
        "attempt": manifest.attempt,
        "status": manifest.status,
        "run_success": manifest.status.run_success(),
        "selection": match selection {
            TrajectorySelection::All => json!({"mode": "all", "particle_ids": []}),
            TrajectorySelection::ParticleIds(ids) => {
                json!({"mode": "particle_ids", "particle_ids": ids})
            }
        },
    });

    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let (spool_guard, mut spool) = if output == OutputMode::Json {
        let (guard, file) = create_temporary_file("trajectory-json")?;
        (Some(guard), Some(file))
    } else {
        (None, None)
    };
    if output == OutputMode::Jsonl {
        writeln!(
            stdout,
            "{}",
            crate::app::render_stream_data("result trajectory", 1, header.clone())
                .map_err(json_error)?
        )
        .map_err(io_error)?;
    } else if output == OutputMode::Human {
        writeln!(
            stdout,
            "trajectory run_id={} attempt={} status={}",
            manifest.run_id.0,
            manifest.attempt,
            display_value(&header["status"]),
        )
        .map_err(io_error)?;
    }

    let result = (|| -> Result<(), ProductError> {
        let mut record_count = 0_u64;
        let mut last_particle_id = None;
        {
            let mut emitter = TrajectoryEmitter {
                connection: &connection,
                manifest: &manifest,
                output,
                stdout: &mut stdout,
                spool: &mut spool,
                record_count: &mut record_count,
                last_particle_id: &mut last_particle_id,
            };
            if ids.is_empty() {
                let sql = format!(
                    "{TRAJECTORY_SELECT} WHERE s.run_id = ?1 ORDER BY s.particle_id, s.sample_sequence"
                );
                let mut statement = connection.prepare(&sql).map_err(sql_error)?;
                let mut rows = statement
                    .query([manifest.run_id.0.as_str()])
                    .map_err(sql_error)?;
                emitter.emit_rows(&mut rows)?;
            } else {
                let sql = format!(
                    "{TRAJECTORY_SELECT} WHERE s.run_id = ?1 AND s.particle_id = ?2 ORDER BY s.sample_sequence"
                );
                let mut statement = connection.prepare(&sql).map_err(sql_error)?;
                for particle_id in ids {
                    let particle_id = i64::try_from(particle_id).map_err(|_| {
                        ProductError::new(
                            "result.trajectory_particle_id_out_of_range",
                            "particle ID exceeds the SQLite i64 range",
                        )
                    })?;
                    let mut rows = statement
                        .query(rusqlite::params![manifest.run_id.0, particle_id])
                        .map_err(sql_error)?;
                    emitter.emit_rows(&mut rows)?;
                }
            }
        }
        if record_count == 0 {
            return Err(ProductError::new(
                "result.trajectory_empty",
                "selection matched no particle states",
            ));
        }

        match output {
            OutputMode::Json => {
                let mut spool_file = spool.take().ok_or_else(|| {
                    ProductError::new("result.temp_spool_failed", "trajectory spool is absent")
                })?;
                spool_file.flush().map_err(io_error)?;
                drop(spool_file);
                let guard = spool_guard.as_ref().ok_or_else(|| {
                    ProductError::new(
                        "result.temp_spool_failed",
                        "trajectory spool path is absent",
                    )
                })?;
                let mut reader = fs::File::open(&guard.path).map_err(io_error)?;
                write_json_trajectory(&mut stdout, &header, &mut reader)
                    .map_err(ProductError::after_stream_started)?;
            }
            OutputMode::Jsonl => {
                writeln!(
                    stdout,
                    "{}",
                    crate::app::render_stream_summary(
                        "result trajectory",
                        record_count + 2,
                        true,
                        Some(manifest.status.run_success()),
                    )
                    .map_err(json_error)?
                )
                .map_err(io_error)?;
            }
            OutputMode::Human => {}
        }
        stdout.flush().map_err(io_error).map_err(|error| {
            if output == OutputMode::Json {
                error.after_stream_started()
            } else {
                error
            }
        })?;
        Ok(())
    })();
    match result {
        Err(error) if output != OutputMode::Json => Err(error.after_stream_started()),
        Err(error) => Err(error),
        Ok(()) => Ok(0),
    }
}

const TRAJECTORY_SELECT: &str = "
SELECT s.run_id,
       p.particle_id,
       p.population_id,
       p.origin_kind,
       p.origin_event_id,
       p.origin_domain_id,
       p.origin_boundary_face_id,
       p.birth_seconds,
       p.birth_nanosecond,
       p.dry_air_mass_kg,
       p.sensitivity_weight,
       s.sample_sequence,
       s.event_sequence,
       s.physical_seconds,
       s.physical_nanosecond,
       s.integration_offset_ns,
       s.elapsed_age_ns,
       s.longitude_degrees,
       s.latitude_degrees,
       s.height_asl_m,
       s.particle_status,
       s.eastward_wind_m_s,
       s.northward_wind_m_s,
       s.geometric_vertical_velocity_m_s,
       s.air_pressure_pa,
       s.air_temperature_k,
       s.wind_validity,
       s.wind_quality,
       s.pressure_validity,
       s.pressure_quality,
       s.temperature_validity,
       s.temperature_quality,
       s.provenance_id,
       e.event_kind,
       t.reason AS terminal_reason,
       t.classification AS terminal_classification,
       t.physical_seconds AS terminal_seconds,
       t.physical_nanosecond AS terminal_nanosecond,
       t.intersection_fraction AS terminal_fraction
FROM particle_state s
JOIN particle p
  ON p.run_id = s.run_id AND p.particle_id = s.particle_id
JOIN output_event e
  ON e.run_id = s.run_id AND e.event_sequence = s.event_sequence
LEFT JOIN termination t
  ON t.run_id = s.run_id AND t.particle_id = s.particle_id";

struct TrajectoryEmitter<'data, 'output, W: Write> {
    connection: &'data Connection,
    manifest: &'data RunManifest,
    output: OutputMode,
    stdout: &'output mut W,
    spool: &'output mut Option<fs::File>,
    record_count: &'output mut u64,
    last_particle_id: &'output mut Option<i64>,
}

impl<W: Write> TrajectoryEmitter<'_, '_, W> {
    fn emit_rows(&mut self, rows: &mut Rows<'_>) -> Result<(), ProductError> {
        while let Some(row) = rows.next().map_err(sql_error)? {
            let particle_id = row.get::<_, i64>("particle_id").map_err(sql_error)?;
            if *self.last_particle_id != Some(particle_id) {
                let particle = particle_record(row, self.connection, self.manifest, particle_id)?;
                emit_trajectory_record(
                    self.output,
                    self.stdout,
                    self.spool,
                    &particle,
                    *self.record_count,
                )?;
                *self.record_count += 1;
                *self.last_particle_id = Some(particle_id);
            }
            let state = state_record(row, self.manifest, particle_id)?;
            emit_trajectory_record(
                self.output,
                self.stdout,
                self.spool,
                &state,
                *self.record_count,
            )?;
            *self.record_count += 1;
        }
        Ok(())
    }
}

fn particle_record(
    row: &rusqlite::Row<'_>,
    connection: &Connection,
    manifest: &RunManifest,
    particle_id: i64,
) -> Result<Value, ProductError> {
    Ok(json!({
        "schema_version": "trajecta.trajectory-record/v1",
        "record_kind": "particle",
        "job_series_id": manifest.job_series_id,
        "run_id": manifest.run_id,
        "attempt": manifest.attempt,
        "particle_id": particle_id,
        "population_id": row.get::<_, String>("population_id").map_err(sql_error)?,
        "origin": {
            "kind": row.get::<_, String>("origin_kind").map_err(sql_error)?,
            "event_id": row.get::<_, Option<String>>("origin_event_id").map_err(sql_error)?,
            "domain_id": row.get::<_, Option<String>>("origin_domain_id").map_err(sql_error)?,
            "boundary_face_id": row.get::<_, Option<i64>>("origin_boundary_face_id").map_err(sql_error)?,
        },
        "birth_time": {
            "seconds_since_unix_epoch": row.get::<_, i64>("birth_seconds").map_err(sql_error)?,
            "nanosecond": row.get::<_, i64>("birth_nanosecond").map_err(sql_error)?,
        },
        "dry_air_mass_kg": row.get::<_, f64>("dry_air_mass_kg").map_err(sql_error)?,
        "sensitivity_weight": row.get::<_, Option<f64>>("sensitivity_weight").map_err(sql_error)?,
        "substance_mass_kg": particle_mass_map(connection, &manifest.run_id.0, particle_id)?,
    }))
}

fn state_record(
    row: &rusqlite::Row<'_>,
    manifest: &RunManifest,
    particle_id: i64,
) -> Result<Value, ProductError> {
    let status = row.get::<_, String>("particle_status").map_err(sql_error)?;
    let termination = if status == "terminated" {
        Some(json!({
            "reason": required_column::<String>(row, "terminal_reason")?,
            "classification": required_column::<String>(row, "terminal_classification")?,
            "time": {
                "seconds_since_unix_epoch": required_column::<i64>(row, "terminal_seconds")?,
                "nanosecond": required_column::<i64>(row, "terminal_nanosecond")?,
            },
            "intersection_fraction": row.get::<_, Option<f64>>("terminal_fraction").map_err(sql_error)?,
        }))
    } else {
        None
    };
    Ok(json!({
        "schema_version": "trajecta.trajectory-record/v1",
        "record_kind": "state",
        "job_series_id": manifest.job_series_id,
        "run_id": manifest.run_id,
        "attempt": manifest.attempt,
        "particle_id": particle_id,
        "sample_sequence": row.get::<_, i64>("sample_sequence").map_err(sql_error)?,
        "event_sequence": row.get::<_, i64>("event_sequence").map_err(sql_error)?,
        "event_kind": row.get::<_, String>("event_kind").map_err(sql_error)?,
        "time": {
            "seconds_since_unix_epoch": row.get::<_, i64>("physical_seconds").map_err(sql_error)?,
            "nanosecond": row.get::<_, i64>("physical_nanosecond").map_err(sql_error)?,
        },
        "integration_offset_ns": row.get::<_, i64>("integration_offset_ns").map_err(sql_error)?,
        "elapsed_age_ns": row.get::<_, i64>("elapsed_age_ns").map_err(sql_error)?,
        "position": {
            "longitude_degrees": row.get::<_, f64>("longitude_degrees").map_err(sql_error)?,
            "latitude_degrees": row.get::<_, f64>("latitude_degrees").map_err(sql_error)?,
            "height_asl_m": row.get::<_, f64>("height_asl_m").map_err(sql_error)?,
        },
        "particle_status": status,
        "termination": termination,
        "meteorology": {
            "eastward_wind_m_s": row.get::<_, Option<f64>>("eastward_wind_m_s").map_err(sql_error)?,
            "northward_wind_m_s": row.get::<_, Option<f64>>("northward_wind_m_s").map_err(sql_error)?,
            "geometric_vertical_velocity_m_s": row.get::<_, Option<f64>>("geometric_vertical_velocity_m_s").map_err(sql_error)?,
            "air_pressure_pa": row.get::<_, Option<f64>>("air_pressure_pa").map_err(sql_error)?,
            "air_temperature_k": row.get::<_, Option<f64>>("air_temperature_k").map_err(sql_error)?,
            "wind_validity": row.get::<_, String>("wind_validity").map_err(sql_error)?,
            "wind_quality": row.get::<_, Option<String>>("wind_quality").map_err(sql_error)?,
            "pressure_validity": row.get::<_, String>("pressure_validity").map_err(sql_error)?,
            "pressure_quality": row.get::<_, Option<String>>("pressure_quality").map_err(sql_error)?,
            "temperature_validity": row.get::<_, String>("temperature_validity").map_err(sql_error)?,
            "temperature_quality": row.get::<_, Option<String>>("temperature_quality").map_err(sql_error)?,
        },
        "provenance_id": row.get::<_, Option<i64>>("provenance_id").map_err(sql_error)?,
    }))
}

fn required_column<T: rusqlite::types::FromSql>(
    row: &rusqlite::Row<'_>,
    name: &str,
) -> Result<T, ProductError> {
    row.get::<_, Option<T>>(name)
        .map_err(sql_error)?
        .ok_or_else(|| {
            ProductError::new(
                "result.sqlite_invalid",
                format!("terminated state has no `{name}` value"),
            )
        })
}

fn validate_requested_particles(
    connection: &Connection,
    run_id: &str,
    ids: &[u64],
) -> Result<(), ProductError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut statement = connection
        .prepare("SELECT EXISTS(SELECT 1 FROM particle WHERE run_id=?1 AND particle_id=?2)")
        .map_err(sql_error)?;
    let mut missing = Vec::new();
    for id in ids {
        let sqlite_id = i64::try_from(*id).map_err(|_| {
            ProductError::new(
                "result.trajectory_particle_id_out_of_range",
                "particle ID exceeds the SQLite i64 range",
            )
        })?;
        let exists: bool = statement
            .query_row(rusqlite::params![run_id, sqlite_id], |row| row.get(0))
            .map_err(sql_error)?;
        if !exists {
            missing.push(*id);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ProductError::new(
            "result.particle_not_found",
            format!("requested particle IDs are absent: {missing:?}"),
        ))
    }
}

fn particle_mass_map(
    connection: &Connection,
    run_id: &str,
    particle_id: i64,
) -> Result<Map<String, Value>, ProductError> {
    let mut statement = connection
        .prepare(
            "SELECT substance_id, mass_kg FROM particle_mass
             WHERE run_id=?1 AND particle_id=?2 ORDER BY substance_id",
        )
        .map_err(sql_error)?;
    let mut rows = statement
        .query(rusqlite::params![run_id, particle_id])
        .map_err(sql_error)?;
    let mut masses = Map::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        masses.insert(
            row.get::<_, String>(0).map_err(sql_error)?,
            json!(row.get::<_, f64>(1).map_err(sql_error)?),
        );
    }
    Ok(masses)
}

fn emit_trajectory_record(
    output: OutputMode,
    stdout: &mut impl Write,
    spool: &mut Option<fs::File>,
    record: &Value,
    count: u64,
) -> Result<(), ProductError> {
    match output {
        OutputMode::Human => {
            if record["record_kind"] == "particle" {
                writeln!(
                    stdout,
                    "particle id={} population={} birth={} mass_kg={}",
                    display_value(&record["particle_id"]),
                    display_value(&record["population_id"]),
                    display_value(&record["birth_time"]),
                    display_value(&record["dry_air_mass_kg"]),
                )
                .map_err(io_error)
            } else {
                writeln!(
                    stdout,
                    "state particle={} sample={} time={} position={} status={}",
                    display_value(&record["particle_id"]),
                    display_value(&record["sample_sequence"]),
                    display_value(&record["time"]),
                    display_value(&record["position"]),
                    display_value(&record["particle_status"]),
                )
                .map_err(io_error)
            }
        }
        OutputMode::Jsonl => writeln!(
            stdout,
            "{}",
            crate::app::render_stream_data("result trajectory", count + 2, record.clone())
                .map_err(json_error)?
        )
        .map_err(io_error),
        OutputMode::Json => {
            let spool = spool.as_mut().ok_or_else(|| {
                ProductError::new("result.temp_spool_failed", "trajectory spool is absent")
            })?;
            if count > 0 {
                spool.write_all(b",").map_err(io_error)?;
            }
            serde_json::to_writer(spool, record).map_err(json_error)
        }
    }
}

fn write_json_trajectory(
    stdout: &mut impl Write,
    header: &Value,
    spool: &mut fs::File,
) -> Result<(), ProductError> {
    let run_success = header["run_success"].as_bool().ok_or_else(|| {
        ProductError::new("result.encoding", "trajectory header lacks run_success")
    })?;
    let header = serde_json::to_string(header).map_err(json_error)?;
    let header = header.strip_suffix('}').ok_or_else(|| {
        ProductError::new("result.encoding", "trajectory header is not a JSON object")
    })?;
    write!(
        stdout,
        "{{\"schema_version\":\"trajecta.cli-output/v1\",\"command\":\"result trajectory\",\"ok\":true,\"run_success\":{},\"data\":{header},\"records\":[",
        run_success
    )
    .map_err(io_error)?;
    io::copy(spool, stdout).map_err(io_error)?;
    writeln!(stdout, "]}},\"diagnostics\":[]}}").map_err(io_error)
}

/// Atomically writes the deterministic derived report at `run-report.md`.
pub(crate) fn write_report(input: &ResultProductInput) -> Result<Value, ProductError> {
    let inspection = inspect(input)?;
    let report = render_run_report(&inspection.data)?;
    let target = canonical_run_directory(&input.run_directory)?.join(REPORT_NAME);
    let (guard, mut file) = create_temporary_file_in(
        target.parent().ok_or_else(|| {
            ProductError::new("report.write_failed", "report path has no parent directory")
        })?,
        "run-report",
    )?;
    let result = (|| {
        file.write_all(report.as_bytes()).map_err(io_error)?;
        file.flush().map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        fs::rename(&guard.path, &target).map_err(io_error)
    })();
    result?;
    guard.keep();
    Ok(json!({
        "schema_version": "trajecta.run-report/v1",
        "run_id": inspection.data["identity"]["run_id"],
        "path": REPORT_NAME,
        "sha256": sha256_bytes(report.as_bytes()),
        "size_bytes": report.len(),
    }))
}

/// Renders deterministic Markdown which is outside the scientific digest identity.
pub(crate) fn render_run_report(inspection: &Value) -> Result<String, ProductError> {
    let artifacts = inspection["artifacts"]
        .as_array()
        .map(|items| {
            Value::Array(
                items
                    .iter()
                    .filter(|item| item["role"] != "run_report")
                    .cloned()
                    .collect(),
            )
        })
        .unwrap_or(Value::Null);
    let sections = [
        ("Identity", inspection["identity"].clone()),
        ("Lifecycle", inspection["lifecycle"].clone()),
        (
            "Inputs and software",
            json!({
                "inputs": inspection["inputs"],
                "software": inspection["software"],
                "numerical": inspection["numerical"],
            }),
        ),
        ("Execution resources", inspection["execution"].clone()),
        (
            "Particles and terminations",
            inspection["particles"].clone(),
        ),
        ("Quality summary", inspection["quality"].clone()),
        ("Mass ledger", inspection["mass_ledger"].clone()),
        (
            "Verification and supersession",
            inspection["catalog"].clone(),
        ),
        ("Artifacts and forensic pointers", artifacts),
    ];
    let mut report = String::from("# Trajecta run report\n\n");
    report.push_str(
        "run-report.md is a derived human-readable view and is not part of the scientific digest identity.\n",
    );
    for (title, value) in sections {
        report.push_str("\n## ");
        report.push_str(title);
        report.push_str("\n\n");
        let text = serde_json::to_string_pretty(&value).map_err(json_error)?;
        for line in text.lines() {
            report.push_str("    ");
            report.push_str(line);
            report.push('\n');
        }
    }
    while report.ends_with("\n\n") {
        report.pop();
    }
    Ok(report)
}

fn catalog_summary(
    history: Option<&JobAttemptHistory>,
    manifest: &RunManifest,
    root: &Path,
) -> Result<Value, ProductError> {
    let Some(history) = history else {
        return Ok(Value::Null);
    };
    let snapshot = &history.snapshot;
    let output_matches = snapshot
        .output_directory
        .as_ref()
        .map(fs::canonicalize)
        .transpose()
        .map_err(io_error)?
        .is_some_and(|path| path == root);
    if snapshot.job_series_id != manifest.job_series_id
        || snapshot.run_id != manifest.run_id
        || snapshot.attempt != manifest.attempt
        || snapshot.state != JobState::from(manifest.status)
        || !output_matches
    {
        return Err(ProductError::new(
            "result.catalog_identity_mismatch",
            "manifest identity or status does not match the durable catalog attempt",
        ));
    }
    Ok(json!({
        "visible_in_routine_list": history.visible_in_routine_list,
        "full_verification": history.full_verification,
        "superseded_by": history.superseded_by,
    }))
}

fn inspect_sqlite(
    path: &Path,
    expected_version: u32,
) -> Result<SqliteProductSummary, ProductError> {
    let connection = ParticleStateSqliteSink::open_readonly(path)
        .map_err(|error| ProductError::new("result.sqlite_invalid", format!("{error:?}")))?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(sql_error)?;
    if integrity != "ok" {
        return Err(ProductError::new(
            "result.sqlite_invalid",
            format!("integrity_check={integrity}"),
        ));
    }
    let user_version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql_error)?;
    if user_version != expected_version {
        return Err(ProductError::new(
            "result.sqlite_invalid",
            format!("user_version={user_version} expected {expected_version}"),
        ));
    }
    let mut row_counts = BTreeMap::new();
    for table in [
        "run",
        "particle",
        "particle_mass",
        "output_event",
        "particle_state",
        "termination",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(sql_error)?;
        row_counts.insert(
            table.to_owned(),
            u64::try_from(count).map_err(|_| {
                ProductError::new(
                    "result.sqlite_invalid",
                    "SQLite returned a negative row count",
                )
            })?,
        );
    }
    let quality = json!({
        "wind": quality_buckets(&connection, "wind_validity", "wind_quality")?,
        "pressure": quality_buckets(&connection, "pressure_validity", "pressure_quality")?,
        "temperature": quality_buckets(&connection, "temperature_validity", "temperature_quality")?,
    });
    let mut termination_by_reason = BTreeMap::new();
    let mut normal_termination_count = 0_u64;
    let mut abnormal_termination_count = 0_u64;
    let mut statement = connection
        .prepare(
            "SELECT classification, reason, COUNT(*) FROM termination
             GROUP BY classification, reason ORDER BY classification, reason",
        )
        .map_err(sql_error)?;
    let mut rows = statement.query([]).map_err(sql_error)?;
    while let Some(row) = rows.next().map_err(sql_error)? {
        let classification = row.get::<_, String>(0).map_err(sql_error)?;
        let reason = row.get::<_, String>(1).map_err(sql_error)?;
        let count = u64::try_from(row.get::<_, i64>(2).map_err(sql_error)?).map_err(|_| {
            ProductError::new(
                "result.sqlite_invalid",
                "SQLite returned a negative row count",
            )
        })?;
        *termination_by_reason.entry(reason).or_insert(0) += count;
        match classification.as_str() {
            "normal" => normal_termination_count += count,
            "abnormal" => abnormal_termination_count += count,
            _ => {
                return Err(ProductError::new(
                    "result.sqlite_invalid",
                    format!("unknown termination classification `{classification}`"),
                ));
            }
        }
    }
    Ok(SqliteProductSummary {
        row_counts,
        quality,
        termination_by_reason,
        normal_termination_count,
        abnormal_termination_count,
    })
}

fn quality_buckets(
    connection: &Connection,
    validity: &str,
    quality: &str,
) -> Result<Vec<Value>, ProductError> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT {validity}, {quality}, COUNT(*) FROM particle_state
             GROUP BY 1, 2 ORDER BY 1, 2"
        ))
        .map_err(sql_error)?;
    let mut rows = statement.query([]).map_err(sql_error)?;
    let mut buckets = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        buckets.push(json!({
            "validity": row.get::<_, String>(0).map_err(sql_error)?,
            "quality": row.get::<_, Option<String>>(1).map_err(sql_error)?,
            "count": row.get::<_, u64>(2).map_err(sql_error)?,
        }));
    }
    Ok(buckets)
}

fn particle_summary(summary: &SqliteProductSummary) -> Value {
    let count = |table: &str| summary.row_counts.get(table).copied().unwrap_or(0);
    json!({
        "particle_count": count("particle"),
        "particle_mass_count": count("particle_mass"),
        "state_count": count("particle_state"),
        "output_event_count": count("output_event"),
        "termination_count": count("termination"),
        "normal_termination_count": summary.normal_termination_count,
        "abnormal_termination_count": summary.abnormal_termination_count,
        "termination_by_reason": summary.termination_by_reason,
    })
}

fn mass_ledger_summary(manifest: &RunManifest) -> Result<Value, ProductError> {
    let maximum_absolute_imbalance_kg = manifest
        .mass_ledger
        .iter()
        .map(|record| record.imbalance_kg.abs())
        .fold(0.0_f64, f64::max);
    let mut maximum_tolerance_fraction = 0.0_f64;
    for record in &manifest.mass_ledger {
        if record.tolerance_kg == 0.0 {
            if record.imbalance_kg != 0.0 {
                return Err(ProductError::new(
                    "result.manifest_invalid",
                    "mass ledger has nonzero imbalance with zero tolerance",
                ));
            }
        } else {
            maximum_tolerance_fraction =
                maximum_tolerance_fraction.max(record.imbalance_kg.abs() / record.tolerance_kg);
        }
    }
    Ok(json!({
        "record_count": manifest.mass_ledger.len(),
        "maximum_absolute_imbalance_kg": maximum_absolute_imbalance_kg,
        "maximum_tolerance_fraction": maximum_tolerance_fraction,
    }))
}

fn artifact_descriptor(
    root: &Path,
    role: &str,
    relative: &Path,
    declared_sha256: Option<&str>,
) -> Result<Value, ProductError> {
    let relative = normal_relative_path(relative)?;
    let relative_text = slash_path(&relative)?;
    let path = root.join(&relative);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_error(error)),
    };
    if metadata.is_some() {
        let canonical = fs::canonicalize(&path).map_err(io_error)?;
        if !canonical.starts_with(root) {
            return Err(ProductError::new(
                "result.artifact_outside_run",
                format!("artifact escapes run directory: {relative_text}"),
            ));
        }
    }
    Ok(json!({
        "role": role,
        "relative_path": relative_text,
        "exists": metadata.as_ref().is_some_and(fs::Metadata::is_file),
        "size_bytes": metadata.as_ref().filter(|value| value.is_file()).map(fs::Metadata::len),
        "declared_sha256": declared_sha256,
    }))
}

fn forensic_descriptors(root: &Path) -> Result<Vec<Value>, ProductError> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name().into_string().map_err(|_| {
            ProductError::new(
                "result.path_not_utf8",
                "forensic artifact name is not valid UTF-8",
            )
        })?;
        if name.starts_with(FORENSIC_PREFIX) {
            names.push(name);
        }
    }
    names.sort();
    names
        .iter()
        .map(|name| artifact_descriptor(root, "forensic", Path::new(name), None))
        .collect()
}

fn canonical_run_directory(path: &Path) -> Result<PathBuf, ProductError> {
    let root = fs::canonicalize(path).map_err(io_error)?;
    if !root.is_dir() {
        return Err(ProductError::new(
            "result.path_invalid",
            "result path is not a directory",
        ));
    }
    Ok(root)
}

fn required_artifact(root: &Path, relative: &Path) -> Result<PathBuf, ProductError> {
    let relative = normal_relative_path(relative)?;
    let path = root.join(&relative);
    if !path.is_file() {
        return Err(ProductError::new(
            "result.artifact_missing",
            format!("required artifact missing: {}", slash_path(&relative)?),
        ));
    }
    let canonical = fs::canonicalize(&path).map_err(io_error)?;
    if !canonical.starts_with(root) {
        return Err(ProductError::new(
            "result.artifact_outside_run",
            format!("artifact escapes run directory: {}", slash_path(&relative)?),
        ));
    }
    Ok(canonical)
}

fn normal_relative_path(path: &Path) -> Result<PathBuf, ProductError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProductError::new(
            "result.artifact_outside_run",
            format!(
                "artifact path is not a normal relative path: {}",
                path.display()
            ),
        ));
    }
    Ok(path.to_path_buf())
}

fn slash_path(path: &Path) -> Result<String, ProductError> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(ProductError::new(
                "result.artifact_outside_run",
                "artifact path is not normal and relative",
            ));
        };
        parts.push(value.to_str().ok_or_else(|| {
            ProductError::new("result.path_not_utf8", "artifact path is not valid UTF-8")
        })?);
    }
    Ok(parts.join("/"))
}

fn read_manifest(path: &Path) -> Result<RunManifest, ProductError> {
    let text = fs::read_to_string(path).map_err(io_error)?;
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    let manifest = RunManifest::deserialize(&mut deserializer)
        .map_err(|error| ProductError::new("result.manifest_invalid", error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| ProductError::new("result.manifest_invalid", error.to_string()))?;
    manifest
        .validate()
        .map_err(|error| ProductError::new("result.manifest_invalid", format!("{error:?}")))?;
    Ok(manifest)
}

struct TemporaryFile {
    path: PathBuf,
    remove_on_drop: bool,
}

impl TemporaryFile {
    fn keep(mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn create_temporary_file(prefix: &str) -> Result<(TemporaryFile, fs::File), ProductError> {
    create_temporary_file_in(&std::env::temp_dir(), prefix)
}

fn create_temporary_file_in(
    directory: &Path,
    prefix: &str,
) -> Result<(TemporaryFile, fs::File), ProductError> {
    for _ in 0..8 {
        let path = directory.join(format!(
            ".trajecta-{prefix}-{}-{}.tmp",
            std::process::id(),
            Uuid::now_v7()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                return Ok((
                    TemporaryFile {
                        path,
                        remove_on_drop: true,
                    },
                    file,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(ProductError::new(
        "result.temp_spool_failed",
        "could not allocate a unique temporary file",
    ))
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".into(),
        _ => value.to_string(),
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn io_error(error: io::Error) -> ProductError {
    ProductError::new("result.io", error.to_string())
}

fn sql_error(error: rusqlite::Error) -> ProductError {
    ProductError::new("result.sqlite_invalid", error.to_string())
}

fn json_error(error: serde_json::Error) -> ProductError {
    ProductError::new("result.encoding", error.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn report_is_deterministic_markdown_safe_and_excludes_itself() {
        let inspection = json!({
            "identity": {"run_id": "run", "job_series_id": "series", "attempt": 1},
            "lifecycle": {"status": "failed", "failure": {"message": "```not a fence"}},
            "inputs": {}, "software": {}, "numerical": {}, "execution": {},
            "particles": null, "quality": null, "mass_ledger": {}, "catalog": null,
            "artifacts": [
                {"role": "manifest", "relative_path": "run-manifest.json"},
                {"role": "run_report", "relative_path": "run-report.md"}
            ]
        });
        let first = render_run_report(&inspection).unwrap();
        assert_eq!(first, render_run_report(&inspection).unwrap());
        assert!(first.ends_with('\n'));
        assert!(!first.ends_with("\n\n"));
        assert!(!first.contains("    \"relative_path\": \"run-report.md\""));
        assert!(first.contains("```not a fence"));
    }

    #[test]
    fn relative_artifacts_reject_escape_components() {
        for path in [Path::new(""), Path::new("."), Path::new("../x")] {
            assert!(normal_relative_path(path).is_err());
        }
        assert_eq!(
            normal_relative_path(Path::new("nested/result.json")).unwrap(),
            PathBuf::from("nested/result.json")
        );
    }

    #[test]
    fn trajectory_json_uses_the_standard_single_result_envelope() {
        let (guard, mut spool) = create_temporary_file("trajectory-test").unwrap();
        spool.write_all(br#"{"record_kind":"particle"}"#).unwrap();
        spool.flush().unwrap();
        drop(spool);
        let mut spool = fs::File::open(&guard.path).unwrap();
        let mut output = Vec::new();
        write_json_trajectory(
            &mut output,
            &json!({
                "schema_version": "trajecta.trajectory-stream/v1",
                "run_success": true
            }),
            &mut spool,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["schema_version"], "trajecta.cli-output/v1");
        assert_eq!(value["diagnostics"], json!([]));
        assert_eq!(value["data"]["records"].as_array().unwrap().len(), 1);
        assert!(value.get("exit_code").is_none());
    }
}
