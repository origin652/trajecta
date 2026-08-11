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
use trajecta_core::output::provenance_bundle::file_sha256;
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_job::history::JobAttemptHistory;
use trajecta_job::model::JobState;
use uuid::Uuid;

use crate::cli::OutputMode;
use crate::command::staged::{ProcessSelection, TrajectorySelection};

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

/// Queries M6 physical-process summaries and optional discrete events.
pub(crate) fn processes(
    input: &ResultProductInput,
    selection: &ProcessSelection,
    output: OutputMode,
) -> Result<i32, ProductError> {
    let root = canonical_run_directory(&input.run_directory)?;
    let manifest_path = required_artifact(&root, Path::new(MANIFEST_NAME))?;
    let manifest = read_manifest(&manifest_path)?;
    catalog_summary(input.catalog.as_ref(), &manifest, &root)?;
    if matches!(
        manifest.status,
        RunLifecycleStatus::Running | RunLifecycleStatus::Failed | RunLifecycleStatus::Interrupted
    ) {
        return Err(ProductError::new(
            "result.processes_not_terminal",
            "process results require a completed attempt",
        ));
    }
    let sqlite_relative = normal_relative_path(&manifest.sqlite.relative_path)?;
    let sqlite_path = required_artifact(&root, &sqlite_relative)?;
    let connection = ParticleStateSqliteSink::open_readonly(&sqlite_path)
        .map_err(|error| ProductError::new("result.sqlite_invalid", format!("{error:?}")))?;
    let sqlite_version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(sql_error)?;
    if sqlite_version != 2 || manifest.sqlite.schema_version != 2 {
        return Err(ProductError::new(
            "result.process_schema_unavailable",
            "result does not use the M6 SQLite v2 process schema",
        ));
    }
    let (sqlite_run_id, direction): (String, String) = connection
        .query_row(
            "SELECT run_id, direction FROM run WHERE run_id=?1",
            [&manifest.run_id.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sql_error)?;
    if sqlite_run_id != manifest.run_id.0 || !matches!(direction.as_str(), "forward" | "backward") {
        return Err(ProductError::new(
            "result.sqlite_identity_mismatch",
            "process database identity or direction is invalid",
        ));
    }
    validate_requested_particles(&connection, &manifest.run_id.0, &selection.particle_ids)?;

    let groups = load_process_groups(&connection, &manifest.run_id.0, &direction, selection)?;
    let event_count_total = count_process_events(&connection, &manifest.run_id.0, selection)?;
    let event_count_returned = if selection.events {
        selection
            .max_records
            .map_or(event_count_total, |limit| event_count_total.min(limit))
    } else {
        0
    };
    let truncated = selection.events && event_count_returned < event_count_total;
    let filters = process_filters_value(selection);
    let identity = json!({
        "run_id": manifest.run_id,
        "manifest_sha256": sha256_bytes(&fs::read(&manifest_path).map_err(io_error)?),
        "sqlite_sha256": file_sha256(&sqlite_path)
            .map_err(|error| ProductError::new("result.sqlite_invalid", format!("{error:?}")))?,
        "sqlite_user_version": sqlite_version,
    });
    let header = json!({
        "schema_version": "trajecta.process-query/v1",
        "result": identity,
        "filters": filters,
        "direction": direction.clone(),
        "groups": groups,
        "event_count_total": event_count_total,
        "event_count_returned": event_count_returned,
        "truncated": truncated,
    });

    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let (spool_guard, mut spool) = if output == OutputMode::Json {
        let (guard, file) = create_temporary_file("process-json")?;
        (Some(guard), Some(file))
    } else {
        (None, None)
    };
    match output {
        OutputMode::Human => render_process_groups(&mut stdout, &header)?,
        OutputMode::Jsonl => {
            writeln!(
                stdout,
                "{}",
                crate::app::render_stream_data("result processes", 1, header.clone())
                    .map_err(json_error)?
            )
            .map_err(io_error)?;
        }
        OutputMode::Json => {}
    }

    let result = (|| -> Result<u64, ProductError> {
        if !selection.events {
            return Ok(0);
        }
        let (sql, parameters) =
            process_event_query(&manifest.run_id.0, selection, selection.max_records)?;
        let mut statement = connection.prepare(&sql).map_err(sql_error)?;
        let mut rows = statement
            .query(rusqlite::params_from_iter(parameters.iter()))
            .map_err(sql_error)?;
        let mut returned = 0_u64;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let event = process_event_value(row, &direction)?;
            match output {
                OutputMode::Human => render_process_event(&mut stdout, &event)?,
                OutputMode::Jsonl => {
                    writeln!(
                        stdout,
                        "{}",
                        crate::app::render_stream_data("result processes", returned + 2, event,)
                            .map_err(json_error)?
                    )
                    .map_err(io_error)?;
                }
                OutputMode::Json => {
                    let file = spool.as_mut().ok_or_else(|| {
                        ProductError::new("result.temp_spool_failed", "process spool is absent")
                    })?;
                    if returned != 0 {
                        file.write_all(b",").map_err(io_error)?;
                    }
                    serde_json::to_writer(file, &event).map_err(json_error)?;
                }
            }
            returned = returned.checked_add(1).ok_or_else(|| {
                ProductError::new("result.row_count_overflow", "process event count overflow")
            })?;
        }
        Ok(returned)
    })();
    let returned = match result {
        Ok(returned) => returned,
        Err(error) if output == OutputMode::Json => return Err(error),
        Err(error) => return Err(error.after_stream_started()),
    };
    if returned != event_count_returned {
        return Err(ProductError::new(
            "result.process_count_changed",
            "process event count changed during the read-only query",
        ));
    }
    match output {
        OutputMode::Json => {
            let mut file = spool.take().ok_or_else(|| {
                ProductError::new("result.temp_spool_failed", "process spool is absent")
            })?;
            file.flush().map_err(io_error)?;
            drop(file);
            let guard = spool_guard.as_ref().ok_or_else(|| {
                ProductError::new("result.temp_spool_failed", "process spool path is absent")
            })?;
            let mut reader = fs::File::open(&guard.path).map_err(io_error)?;
            write_json_processes(
                &mut stdout,
                &header,
                &mut reader,
                manifest.status.run_success(),
            )
            .map_err(ProductError::after_stream_started)?;
        }
        OutputMode::Jsonl => {
            writeln!(
                stdout,
                "{}",
                crate::app::render_stream_summary(
                    "result processes",
                    returned + 2,
                    true,
                    Some(manifest.status.run_success()),
                )
                .map_err(json_error)?
            )
            .map_err(io_error)?;
        }
        OutputMode::Human => {
            writeln!(
                stdout,
                "events returned={returned} total={event_count_total} truncated={truncated}"
            )
            .map_err(io_error)?;
        }
    }
    stdout.flush().map_err(io_error).map_err(|error| {
        if output == OutputMode::Json {
            error.after_stream_started()
        } else {
            error
        }
    })?;
    Ok(0)
}

fn process_filters_value(selection: &ProcessSelection) -> Value {
    json!({
        "particle_ids": selection.particle_ids,
        "module_ids": selection.module_ids,
        "substance_ids": selection.substance_ids,
        "start": selection.start,
        "end": selection.end,
        "events": selection.events,
        "max_records": selection.max_records,
    })
}

fn load_process_groups(
    connection: &Connection,
    run_id: &str,
    direction: &str,
    selection: &ProcessSelection,
) -> Result<Vec<Value>, ProductError> {
    let mut statement = connection
        .prepare(
            "SELECT module_id, substance_id, event_count,
                    initial_mass_kg, positive_mass_delta_kg, negative_mass_delta_kg,
                    final_mass_kg, initial_adjoint_weight,
                    survival_multiplier_product, source_sensitivity,
                    convection_importance_product, final_adjoint_weight, closure_residual
             FROM process_summary WHERE run_id=?1 ORDER BY module_id, substance_id",
        )
        .map_err(sql_error)?;
    let mut rows = statement.query([run_id]).map_err(sql_error)?;
    let mut groups = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let module = row.get::<_, String>(0).map_err(sql_error)?;
        let substance = row.get::<_, String>(1).map_err(sql_error)?;
        if (!selection.module_ids.is_empty() && !selection.module_ids.contains(&module))
            || (!selection.substance_ids.is_empty()
                && !selection.substance_ids.contains(&substance))
        {
            continue;
        }
        if selection.particle_ids.is_empty() && selection.start.is_none() && selection.end.is_none()
        {
            groups.push(process_summary_group(row, direction, module, substance)?);
            continue;
        }
        let particles = if selection.particle_ids.is_empty() {
            vec![None]
        } else {
            selection.particle_ids.iter().copied().map(Some).collect()
        };
        for particle in particles {
            groups.push(recomputed_process_group(
                connection, run_id, direction, selection, &module, &substance, particle,
            )?);
        }
    }
    Ok(groups)
}

fn process_summary_group(
    row: &rusqlite::Row<'_>,
    direction: &str,
    module: String,
    substance: String,
) -> Result<Value, ProductError> {
    let closure = if direction == "forward" {
        json!({
            "kind": "forward_mass",
            "initial_mass_kg": required_sql_value::<f64>(row, 3, "initial_mass_kg")?,
            "positive_delta_kg": required_sql_value::<f64>(row, 4, "positive_mass_delta_kg")?,
            "negative_delta_kg": required_sql_value::<f64>(row, 5, "negative_mass_delta_kg")?,
            "final_mass_kg": required_sql_value::<f64>(row, 6, "final_mass_kg")?,
            "residual_kg": row.get::<_, f64>(12).map_err(sql_error)?,
        })
    } else {
        json!({
            "kind": "backward_adjoint",
            "initial_adjoint_weight": required_sql_value::<f64>(row, 7, "initial_adjoint_weight")?,
            "survival_product": required_sql_value::<f64>(row, 8, "survival_multiplier_product")?,
            "source_sensitivity": required_sql_value::<f64>(row, 9, "source_sensitivity")?,
            "convection_importance_product": required_sql_value::<f64>(row, 10, "convection_importance_product")?,
            "final_adjoint_weight": required_sql_value::<f64>(row, 11, "final_adjoint_weight")?,
            "residual": row.get::<_, f64>(12).map_err(sql_error)?,
        })
    };
    Ok(json!({
        "module_id": module,
        "substance_id": substance,
        "particle_id": Value::Null,
        "event_count": row.get::<_, u64>(2).map_err(sql_error)?,
        "closure": closure,
    }))
}

fn required_sql_value<T: rusqlite::types::FromSql>(
    row: &rusqlite::Row<'_>,
    index: usize,
    name: &str,
) -> Result<T, ProductError> {
    row.get::<_, Option<T>>(index)
        .map_err(sql_error)?
        .ok_or_else(|| {
            ProductError::new("result.process_summary_invalid", format!("missing {name}"))
        })
}

#[derive(Clone, Copy)]
struct ProcessAggregate {
    count: u64,
    positive: f64,
    negative: f64,
    survival: f64,
    source: f64,
    importance: f64,
}

fn recomputed_process_group(
    connection: &Connection,
    run_id: &str,
    direction: &str,
    selection: &ProcessSelection,
    module: &str,
    substance: &str,
    particle: Option<u64>,
) -> Result<Value, ProductError> {
    let aggregate = aggregate_process_events(
        connection, run_id, selection, module, substance, particle, direction,
    )?;
    let (initial, final_value, stored_source) =
        directional_state_totals(connection, run_id, direction, substance, particle)?;
    let closure = if direction == "forward" {
        json!({
            "kind": "forward_mass",
            "initial_mass_kg": initial,
            "positive_delta_kg": aggregate.positive,
            "negative_delta_kg": aggregate.negative,
            "final_mass_kg": final_value,
            "residual_kg": final_value - initial - aggregate.positive - aggregate.negative,
        })
    } else {
        json!({
            "kind": "backward_adjoint",
            "initial_adjoint_weight": initial,
            "survival_product": aggregate.survival,
            "source_sensitivity": aggregate.source + stored_source,
            "convection_importance_product": aggregate.importance,
            "final_adjoint_weight": final_value,
            "residual": final_value
                - initial * aggregate.survival * aggregate.importance
                - aggregate.source
                - stored_source,
        })
    };
    Ok(json!({
        "module_id": module,
        "substance_id": substance,
        "particle_id": particle,
        "event_count": aggregate.count,
        "closure": closure,
    }))
}

fn aggregate_process_events(
    connection: &Connection,
    run_id: &str,
    selection: &ProcessSelection,
    module: &str,
    substance: &str,
    particle: Option<u64>,
    direction: &str,
) -> Result<ProcessAggregate, ProductError> {
    let (mut sql, mut parameters) = process_event_where(run_id, selection);
    sql.push_str(" AND e.module_id=? AND e.substance_id=?");
    parameters.push(rusqlite::types::Value::Text(module.to_owned()));
    parameters.push(rusqlite::types::Value::Text(substance.to_owned()));
    if let Some(particle) = particle {
        sql.push_str(" AND e.particle_id=?");
        parameters.push(rusqlite::types::Value::Integer(
            i64::try_from(particle).map_err(|_| {
                ProductError::new("result.process_filter_invalid", "particle ID out of range")
            })?,
        ));
    }
    let query = format!(
        "SELECT e.mass_delta_kg, e.survival_multiplier,
                e.source_sensitivity, e.importance_weight FROM process_event e {sql}"
    );
    let mut statement = connection.prepare(&query).map_err(sql_error)?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(parameters.iter()))
        .map_err(sql_error)?;
    let mut aggregate = ProcessAggregate {
        count: 0,
        positive: 0.0,
        negative: 0.0,
        survival: 1.0,
        source: 0.0,
        importance: 1.0,
    };
    while let Some(row) = rows.next().map_err(sql_error)? {
        if direction == "forward" {
            let delta = required_sql_value::<f64>(row, 0, "mass_delta_kg")?;
            if delta >= 0.0 {
                aggregate.positive += delta;
            } else {
                aggregate.negative += delta;
            }
        } else {
            aggregate.survival *= required_sql_value::<f64>(row, 1, "survival_multiplier")?;
            aggregate.source += required_sql_value::<f64>(row, 2, "source_sensitivity")?;
            aggregate.importance *= required_sql_value::<f64>(row, 3, "importance_weight")?;
        }
        aggregate.count = aggregate.count.checked_add(1).ok_or_else(|| {
            ProductError::new("result.row_count_overflow", "process event count overflow")
        })?;
    }
    Ok(aggregate)
}

fn directional_state_totals(
    connection: &Connection,
    run_id: &str,
    direction: &str,
    substance: &str,
    particle: Option<u64>,
) -> Result<(f64, f64, f64), ProductError> {
    let (table, initial, final_value, source) = if direction == "forward" {
        ("particle_mass", "initial_mass_kg", "mass_kg", "0.0")
    } else {
        (
            "particle_adjoint",
            "initial_adjoint_weight",
            "adjoint_weight",
            "source_sensitivity",
        )
    };
    let mut sql = format!(
        "SELECT COALESCE(SUM({initial}),0.0), COALESCE(SUM({final_value}),0.0),
                COALESCE(SUM({source}),0.0)
         FROM {table} WHERE run_id=? AND substance_id=?"
    );
    let mut parameters = vec![
        rusqlite::types::Value::Text(run_id.to_owned()),
        rusqlite::types::Value::Text(substance.to_owned()),
    ];
    if let Some(particle) = particle {
        sql.push_str(" AND particle_id=?");
        parameters.push(rusqlite::types::Value::Integer(
            i64::try_from(particle).map_err(|_| {
                ProductError::new("result.process_filter_invalid", "particle ID out of range")
            })?,
        ));
    }
    connection
        .query_row(&sql, rusqlite::params_from_iter(parameters.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(sql_error)
}

fn process_event_where(
    run_id: &str,
    selection: &ProcessSelection,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut sql = String::from("WHERE e.run_id=?");
    let mut parameters = vec![rusqlite::types::Value::Text(run_id.to_owned())];
    append_integer_filter(
        &mut sql,
        &mut parameters,
        "e.particle_id",
        &selection.particle_ids,
    );
    append_text_filter(
        &mut sql,
        &mut parameters,
        "e.module_id",
        &selection.module_ids,
    );
    append_text_filter(
        &mut sql,
        &mut parameters,
        "e.substance_id",
        &selection.substance_ids,
    );
    if let Some(start) = selection.start {
        sql.push_str(" AND (e.physical_seconds,e.physical_nanosecond) >= (?,?)");
        parameters.push(rusqlite::types::Value::Integer(
            start.seconds_since_unix_epoch(),
        ));
        parameters.push(rusqlite::types::Value::Integer(i64::from(
            start.nanosecond(),
        )));
    }
    if let Some(end) = selection.end {
        sql.push_str(" AND (e.physical_seconds,e.physical_nanosecond) <= (?,?)");
        parameters.push(rusqlite::types::Value::Integer(
            end.seconds_since_unix_epoch(),
        ));
        parameters.push(rusqlite::types::Value::Integer(i64::from(end.nanosecond())));
    }
    (sql, parameters)
}

fn append_integer_filter(
    sql: &mut String,
    parameters: &mut Vec<rusqlite::types::Value>,
    column: &str,
    values: &[u64],
) {
    if values.is_empty() {
        return;
    }
    sql.push_str(&format!(
        " AND {column} IN ({})",
        placeholders(values.len())
    ));
    parameters.extend(
        values
            .iter()
            .map(|value| rusqlite::types::Value::Integer(*value as i64)),
    );
}

fn append_text_filter(
    sql: &mut String,
    parameters: &mut Vec<rusqlite::types::Value>,
    column: &str,
    values: &[String],
) {
    if values.is_empty() {
        return;
    }
    sql.push_str(&format!(
        " AND {column} IN ({})",
        placeholders(values.len())
    ));
    parameters.extend(values.iter().cloned().map(rusqlite::types::Value::Text));
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

fn count_process_events(
    connection: &Connection,
    run_id: &str,
    selection: &ProcessSelection,
) -> Result<u64, ProductError> {
    let (where_sql, parameters) = process_event_where(run_id, selection);
    let query = format!("SELECT COUNT(*) FROM process_event e {where_sql}");
    connection
        .query_row(
            &query,
            rusqlite::params_from_iter(parameters.iter()),
            |row| row.get(0),
        )
        .map_err(sql_error)
}

fn process_event_query(
    run_id: &str,
    selection: &ProcessSelection,
    limit: Option<u64>,
) -> Result<(String, Vec<rusqlite::types::Value>), ProductError> {
    let (where_sql, mut parameters) = process_event_where(run_id, selection);
    let mut sql = format!(
        "SELECT event_sequence, particle_id, macro_step, module_id, substance_id,
                physical_seconds, physical_nanosecond, detail_kind, mass_delta_kg,
                survival_multiplier, source_sensitivity, importance_weight
         FROM process_event e {where_sql}
         ORDER BY physical_seconds, physical_nanosecond, particle_id,
                  module_id, substance_id, event_sequence"
    );
    if let Some(limit) = limit {
        sql.push_str(" LIMIT ?");
        parameters.push(rusqlite::types::Value::Integer(
            i64::try_from(limit).map_err(|_| {
                ProductError::new("result.process_filter_invalid", "event limit out of range")
            })?,
        ));
    }
    Ok((sql, parameters))
}

fn process_event_value(row: &rusqlite::Row<'_>, direction: &str) -> Result<Value, ProductError> {
    let (forward, backward) = if direction == "forward" {
        (
            json!({"mass_delta_kg": required_sql_value::<f64>(row, 8, "mass_delta_kg")?}),
            Value::Null,
        )
    } else {
        (
            Value::Null,
            json!({
                "survival_multiplier": required_sql_value::<f64>(row, 9, "survival_multiplier")?,
                "source_sensitivity": required_sql_value::<f64>(row, 10, "source_sensitivity")?,
                "importance_weight": required_sql_value::<f64>(row, 11, "importance_weight")?,
            }),
        )
    };
    Ok(json!({
        "event_sequence": row.get::<_, u64>(0).map_err(sql_error)?,
        "particle_id": row.get::<_, u64>(1).map_err(sql_error)?,
        "macro_step": row.get::<_, u64>(2).map_err(sql_error)?,
        "module_id": row.get::<_, String>(3).map_err(sql_error)?,
        "substance_id": row.get::<_, String>(4).map_err(sql_error)?,
        "physical_time": {
            "seconds_since_unix_epoch": row.get::<_, i64>(5).map_err(sql_error)?,
            "nanosecond": row.get::<_, u32>(6).map_err(sql_error)?,
        },
        "detail_kind": row.get::<_, String>(7).map_err(sql_error)?,
        "forward": forward,
        "backward": backward,
    }))
}

fn render_process_groups(output: &mut impl Write, header: &Value) -> Result<(), ProductError> {
    writeln!(
        output,
        "processes run_id={} direction={}",
        display_value(&header["result"]["run_id"]),
        display_value(&header["direction"]),
    )
    .map_err(io_error)?;
    for group in header["groups"].as_array().into_iter().flatten() {
        writeln!(
            output,
            "group module={} substance={} particle={} events={} closure={}",
            display_value(&group["module_id"]),
            display_value(&group["substance_id"]),
            display_value(&group["particle_id"]),
            display_value(&group["event_count"]),
            serde_json::to_string(&group["closure"]).map_err(json_error)?,
        )
        .map_err(io_error)?;
    }
    Ok(())
}

fn render_process_event(output: &mut impl Write, event: &Value) -> Result<(), ProductError> {
    writeln!(
        output,
        "event sequence={} time={}:{} particle={} module={} substance={} kind={}",
        display_value(&event["event_sequence"]),
        display_value(&event["physical_time"]["seconds_since_unix_epoch"]),
        display_value(&event["physical_time"]["nanosecond"]),
        display_value(&event["particle_id"]),
        display_value(&event["module_id"]),
        display_value(&event["substance_id"]),
        display_value(&event["detail_kind"]),
    )
    .map_err(io_error)
}

fn write_json_processes(
    output: &mut impl Write,
    header: &Value,
    events: &mut fs::File,
    run_success: bool,
) -> Result<(), ProductError> {
    let header = serde_json::to_string(header).map_err(json_error)?;
    let header = header.strip_suffix('}').ok_or_else(|| {
        ProductError::new("result.encoding", "process header is not a JSON object")
    })?;
    write!(
        output,
        "{{\"schema_version\":\"trajecta.cli-output/v1\",\"command\":\"result processes\",\"ok\":true,\"run_success\":{run_success},\"data\":{header},\"events\":["
    )
    .map_err(io_error)?;
    io::copy(events, output).map_err(io_error)?;
    writeln!(output, "]}},\"diagnostics\":[]}}").map_err(io_error)
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
       s.boundary_layer_random_eastward_m_s,
       s.boundary_layer_random_northward_m_s,
       s.boundary_layer_random_vertical_m_s,
       s.mesoscale_random_eastward_m_s,
       s.mesoscale_random_northward_m_s,
       s.mesoscale_random_vertical_m_s,
       s.gravitational_settling_m_s,
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
        "substance_mass_kg": particle_mass_map(connection, &manifest.run_id.0, particle_id)?,
        "substance_adjoint_weight": particle_adjoint_map(
            connection,
            &manifest.run_id.0,
            particle_id,
        )?,
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
        "process_velocity_m_s": {
            "boundary_layer": {
                "eastward": row.get::<_, Option<f64>>("boundary_layer_random_eastward_m_s").map_err(sql_error)?,
                "northward": row.get::<_, Option<f64>>("boundary_layer_random_northward_m_s").map_err(sql_error)?,
                "vertical": row.get::<_, Option<f64>>("boundary_layer_random_vertical_m_s").map_err(sql_error)?,
            },
            "mesoscale": {
                "eastward": row.get::<_, Option<f64>>("mesoscale_random_eastward_m_s").map_err(sql_error)?,
                "northward": row.get::<_, Option<f64>>("mesoscale_random_northward_m_s").map_err(sql_error)?,
                "vertical": row.get::<_, Option<f64>>("mesoscale_random_vertical_m_s").map_err(sql_error)?,
            },
            "gravitational_settling": row.get::<_, Option<f64>>("gravitational_settling_m_s").map_err(sql_error)?,
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

fn particle_adjoint_map(
    connection: &Connection,
    run_id: &str,
    particle_id: i64,
) -> Result<Map<String, Value>, ProductError> {
    let mut statement = connection
        .prepare(
            "SELECT substance_id, adjoint_weight FROM particle_adjoint
             WHERE run_id=?1 AND particle_id=?2 ORDER BY substance_id",
        )
        .map_err(sql_error)?;
    let mut rows = statement
        .query(rusqlite::params![run_id, particle_id])
        .map_err(sql_error)?;
    let mut weights = Map::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        weights.insert(
            row.get::<_, String>(0).map_err(sql_error)?,
            json!(row.get::<_, f64>(1).map_err(sql_error)?),
        );
    }
    Ok(weights)
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
        "particle_adjoint",
        "particle_aerosol_property",
        "output_event",
        "particle_state",
        "termination",
        "process_summary",
        "process_event",
        "water_vapor_event",
        "deposition_event",
        "chemistry_event",
        "emission_event",
        "convection_event",
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
        "particle_adjoint_count": count("particle_adjoint"),
        "state_count": count("particle_state"),
        "output_event_count": count("output_event"),
        "process_summary_count": count("process_summary"),
        "process_event_count": count("process_event"),
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
    use trajecta_case::document::MeteorologyReaderBackend;
    use trajecta_case::model::output::default_particle_state_output;
    use trajecta_case::model::time::Timestamp;
    use trajecta_core::manifest::{
        ExecutionSummary, InputIdentity, NumericalSummary, RunId, RunManifestStart,
        SoftwareIdentity,
    };
    use trajecta_core::output::ParticleStateSink;

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

    fn process_test_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../../../testdata/M6_SQLITE_SCHEMA.v2.sql"))
            .unwrap();
        connection
    }

    fn insert_process_test_run(connection: &Connection, run_id: &str, direction: &str) {
        connection
            .execute(
                "INSERT INTO run (
                    run_id, job_series_id, attempt, manifest_schema, case_name,
                    direction, status, started_seconds, started_nanosecond
                 ) VALUES (?1,?1,1,'trajecta.run-manifest/v1','process-test',?2,
                           'running',0,0)",
                rusqlite::params![run_id, direction],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO particle (
                    run_id, particle_id, population_id, origin_kind, origin_event_id,
                    birth_seconds, birth_nanosecond, dry_air_mass_kg
                 ) VALUES (?1,7,'release','release','source',0,0,1.0)",
                [run_id],
            )
            .unwrap();
    }

    fn empty_process_selection() -> ProcessSelection {
        ProcessSelection {
            particle_ids: Vec::new(),
            module_ids: Vec::new(),
            substance_ids: Vec::new(),
            start: None,
            end: None,
            events: false,
            max_records: None,
        }
    }

    fn write_legacy_v1_result(directory: &Path) -> ResultProductInput {
        let mut manifest = RunManifest::running(RunManifestStart {
            run_id: RunId("018f0000-0000-7000-8000-000000000103".into()),
            case_name: "legacy-process-test".into(),
            started_at: Timestamp::UNIX_EPOCH,
            software: SoftwareIdentity {
                crate_versions: BTreeMap::from([("trajecta-core".into(), "0.1.0-alpha.1".into())]),
                git_commit: None,
            },
            inputs: InputIdentity {
                case_sha256: "0".repeat(64),
                run_profile_sha256: "1".repeat(64),
                dataset_lock_sha256: BTreeMap::new(),
                dataset_profile_sha256: BTreeMap::new(),
                dataset_content_sha256: BTreeMap::new(),
            },
            execution: ExecutionSummary {
                worker_threads: 1,
                memory_budget_bytes: 1024 * 1024,
                executor: "test".into(),
                reader_backends: BTreeMap::<String, MeteorologyReaderBackend>::new(),
                wall_time_ns: None,
                peak_rss_bytes: None,
                io_counters: BTreeMap::new(),
            },
            numerical: NumericalSummary {
                random_seed: 7,
                integrator: "test_integrator".into(),
                boundary_policies: Vec::new(),
                population: "test_population".into(),
                ozone_rule: None,
                particle_state_sink: trajecta_case::model::output::PARTICLE_STATE_SQLITE_SINK_ID
                    .into(),
                tolerance_registry: "trajecta.m4.numerical-contract/v1".into(),
                tolerances: BTreeMap::from([("test".into(), 0.0)]),
                deterministic: true,
            },
            geometries: Vec::new(),
            effective_outputs: vec![default_particle_state_output()],
        });
        let sqlite_path = directory.join("particles.sqlite");
        let mut sink = ParticleStateSqliteSink::with_time_bounds(
            Timestamp::UNIX_EPOCH,
            Timestamp::new(1, 0).unwrap(),
        );
        sink.begin(&sqlite_path, &manifest).unwrap();
        sink.finish().unwrap();
        manifest.sqlite.row_counts = sink.row_counts().unwrap();
        manifest.provenance = sink.provenance_identity();
        manifest.sqlite.schema_version = 1;
        manifest.status = RunLifecycleStatus::Complete;
        manifest.finished_at = Some(Timestamp::new(1, 0).unwrap());
        manifest.validate().unwrap();
        fs::write(
            directory.join(MANIFEST_NAME),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let connection = Connection::open(&sqlite_path).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);
        ResultProductInput {
            run_directory: directory.to_path_buf(),
            catalog: None,
        }
    }

    #[test]
    fn legacy_v1_result_reports_process_schema_unavailable_before_streaming() {
        let directory = tempfile::TempDir::new().unwrap();
        let input = write_legacy_v1_result(directory.path());

        let error = processes(&input, &empty_process_selection(), OutputMode::Json).unwrap_err();

        assert_eq!(error.code, "result.process_schema_unavailable");
        assert!(!error.stream_started);
    }

    #[test]
    fn process_queries_preserve_direction_filters_order_and_limit() {
        let connection = process_test_connection();
        let forward = "018f0000-0000-7000-8000-000000000101";
        insert_process_test_run(&connection, forward, "forward");
        connection
            .execute(
                "INSERT INTO particle_mass VALUES (?1,7,'water',1.0,1.1)",
                [forward],
            )
            .unwrap();
        for (sequence, macro_step, seconds, delta) in
            [(0_i64, 0_i64, 10_i64, 0.2_f64), (1, 1, 20, -0.1)]
        {
            connection
                .execute(
                    "INSERT INTO process_event (
                        run_id,event_sequence,particle_id,macro_step,module_id,substance_id,
                        physical_seconds,physical_nanosecond,direction,detail_kind,mass_delta_kg
                     ) VALUES (?1,?2,7,?3,'water_vapor_exchange','water',?4,0,
                               'forward','water_vapor',?5)",
                    rusqlite::params![forward, sequence, macro_step, seconds, delta],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO water_vapor_event VALUES (?1,?2,?3,0.0,0.0,NULL,NULL,NULL)",
                    rusqlite::params![forward, sequence, delta],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO process_summary (
                    run_id,module_id,substance_id,direction,event_count,
                    initial_mass_kg,positive_mass_delta_kg,negative_mass_delta_kg,
                    final_mass_kg,closure_residual
                 ) VALUES (?1,'water_vapor_exchange','water','forward',2,
                           1.0,0.2,-0.1,1.1,0.0)",
                [forward],
            )
            .unwrap();

        let groups =
            load_process_groups(&connection, forward, "forward", &empty_process_selection())
                .unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["event_count"], 2);
        assert_eq!(groups[0]["closure"]["kind"], "forward_mass");

        let selection = ProcessSelection {
            particle_ids: vec![7],
            module_ids: vec!["water_vapor_exchange".into()],
            substance_ids: vec!["water".into()],
            start: Some(Timestamp::new(15, 0).unwrap()),
            end: Some(Timestamp::new(25, 0).unwrap()),
            events: true,
            max_records: Some(1),
        };
        assert_eq!(
            count_process_events(&connection, forward, &selection).unwrap(),
            1
        );
        let (sql, parameters) = process_event_query(forward, &selection, Some(1)).unwrap();
        let mut statement = connection.prepare(&sql).unwrap();
        let mut rows = statement
            .query(rusqlite::params_from_iter(parameters.iter()))
            .unwrap();
        let event = process_event_value(rows.next().unwrap().unwrap(), "forward").unwrap();
        assert_eq!(event["event_sequence"], 1);
        assert_eq!(event["forward"]["mass_delta_kg"], -0.1);
        assert!(event["backward"].is_null());
        assert!(rows.next().unwrap().is_none());

        let backward = "018f0000-0000-7000-8000-000000000102";
        insert_process_test_run(&connection, backward, "backward");
        connection
            .execute(
                "INSERT INTO particle_adjoint VALUES (?1,7,'gas',1.0,0.9,0.1)",
                [backward],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO process_summary (
                    run_id,module_id,substance_id,direction,event_count,
                    initial_adjoint_weight,survival_multiplier_product,source_sensitivity,
                    convection_importance_product,final_adjoint_weight,closure_residual
                 ) VALUES (?1,'first_order_decay','gas','backward',1,
                           1.0,0.8,0.1,1.0,0.9,0.0)",
                [backward],
            )
            .unwrap();
        let groups = load_process_groups(
            &connection,
            backward,
            "backward",
            &empty_process_selection(),
        )
        .unwrap();
        assert_eq!(groups[0]["closure"]["kind"], "backward_adjoint");
        assert_eq!(groups[0]["closure"]["source_sensitivity"], 0.1);
    }

    #[test]
    fn process_json_uses_one_standard_envelope_and_embeds_events() {
        let (guard, mut spool) = create_temporary_file("process-test").unwrap();
        spool
            .write_all(br#"{"event_sequence":3,"particle_id":7}"#)
            .unwrap();
        spool.flush().unwrap();
        drop(spool);
        let mut spool = fs::File::open(&guard.path).unwrap();
        let mut output = Vec::new();
        write_json_processes(
            &mut output,
            &json!({
                "schema_version": "trajecta.process-query/v1",
                "result": {},
                "filters": {},
                "direction": "forward",
                "groups": [],
                "event_count_total": 1,
                "event_count_returned": 1,
                "truncated": false,
            }),
            &mut spool,
            true,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["schema_version"], "trajecta.cli-output/v1");
        assert_eq!(value["command"], "result processes");
        assert_eq!(value["data"]["events"][0]["event_sequence"], 3);
        assert_eq!(value["diagnostics"], json!([]));
        assert!(value.get("exit_code").is_none());
    }
}
