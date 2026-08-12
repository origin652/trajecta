//! # Contract: quick and full run-result verification
//!
//! Verification reads finished artifacts without modifying them. Quick mode
//! checks identities and file integrity; full mode additionally audits rows,
//! lifecycle ordering, quality fields, terminations, and the mass ledger.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{JobSeriesId, RunId, RunLifecycleStatus, RunManifest};
use crate::output::provenance_bundle::{
    canonical_output_digest, file_sha256, validate_bundle_file_semantics_loose,
};
use crate::output::sqlite::ParticleStateSqliteSink;

/// Stable machine identity for a successful verification record.
pub const RESULT_VERIFICATION_SCHEMA_ID: &str = "trajecta.result-verification/v1";
const MANIFEST_FILE_NAME: &str = "run-manifest.json";
const MAXIMUM_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// Verification depth selected by `result verify`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationMode {
    /// Manifest, SHA, SQLite, WAL, provenance structure, and identity checks.
    Quick,
    /// Quick checks plus lifecycle, finite, quality, mass, and row audits.
    Full,
}

/// Additional counters established by full verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullVerificationSummary {
    /// Particle identities audited.
    pub particle_count: u64,
    /// Particle-state rows audited in sequence order.
    pub sample_count: u64,
    /// Terminal particle rows audited.
    pub termination_count: u64,
    /// Output-event rows covered by foreign keys.
    pub output_event_count: u64,
    /// Manifest mass-ledger records whose imbalance is within tolerance.
    pub mass_ledger_count: u64,
}

/// Successful quick or full verification of one immutable run directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultVerification {
    /// Must equal [`RESULT_VERIFICATION_SCHEMA_ID`].
    pub schema_version: String,
    /// Verification depth that completed.
    pub mode: VerificationMode,
    /// Whether the numerical run itself completed without particle errors.
    pub run_success: bool,
    /// Declared terminal lifecycle state.
    pub status: RunLifecycleStatus,
    /// Logical job-series identity.
    pub job_series_id: JobSeriesId,
    /// One-based attempt number.
    pub attempt: u32,
    /// Unique run identity.
    pub run_id: RunId,
    /// SHA-256 of exact manifest bytes.
    pub manifest_sha256: String,
    /// SHA-256 of exact SQLite bytes.
    pub sqlite_sha256: String,
    /// Canonical ordered SQL digest.
    pub sqlite_sql_sha256: String,
    /// SHA-256 of exact provenance-bundle bytes.
    pub provenance_sha256: String,
    /// Normalized provenance content digest.
    pub provenance_content_sha256: String,
    /// Canonical output digest derived from SQL and provenance content.
    pub canonical_output_sha256: String,
    /// Independently observed public table row counts.
    pub row_counts: BTreeMap<String, u64>,
    /// Full-only audit counters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<FullVerificationSummary>,
}

/// Stable verification failure at an artifact trust boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResultVerificationError {
    code: &'static str,
    message: String,
}

impl ResultVerificationError {
    /// Stable machine-readable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ResultVerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ResultVerificationError {}

/// Verifies one run directory without writing to it.
pub fn verify_run_directory(
    run_directory: &Path,
    mode: VerificationMode,
) -> Result<ResultVerification, ResultVerificationError> {
    let run_directory = fs::canonicalize(run_directory).map_err(|error| {
        ResultVerificationError::new(
            "result.run_directory_invalid",
            format!("open run directory {}: {error}", run_directory.display()),
        )
    })?;
    if !run_directory.is_dir() {
        return Err(ResultVerificationError::new(
            "result.run_directory_invalid",
            "result path is not a directory",
        ));
    }

    let manifest_path = contained_file(&run_directory, Path::new(MANIFEST_FILE_NAME))?;
    let manifest_metadata = fs::metadata(&manifest_path).map_err(io_error("inspect manifest"))?;
    if manifest_metadata.len() > MAXIMUM_MANIFEST_BYTES {
        return Err(ResultVerificationError::new(
            "result.manifest_too_large",
            format!("manifest exceeds {MAXIMUM_MANIFEST_BYTES} bytes"),
        ));
    }
    let manifest_bytes = fs::read(&manifest_path).map_err(io_error("read manifest"))?;
    let mut deserializer = serde_json::Deserializer::from_slice(&manifest_bytes);
    let manifest = RunManifest::deserialize(&mut deserializer).map_err(|error| {
        ResultVerificationError::new("result.manifest_invalid", error.to_string())
    })?;
    deserializer.end().map_err(|error| {
        ResultVerificationError::new("result.manifest_invalid", error.to_string())
    })?;
    manifest.validate().map_err(|error| {
        ResultVerificationError::new(error.code(), "run manifest validation failed")
    })?;
    match manifest.status {
        RunLifecycleStatus::Running => {
            return Err(ResultVerificationError::new(
                "result.not_terminal",
                "running result cannot be verified",
            ));
        }
        RunLifecycleStatus::Failed => {
            return Err(ResultVerificationError::new(
                "result.failed",
                "failed result has no formal provenance product",
            ));
        }
        RunLifecycleStatus::Interrupted => {
            return Err(ResultVerificationError::new(
                "result.interrupted",
                "interrupted result is forensic and cannot pass verification",
            ));
        }
        RunLifecycleStatus::Complete
        | RunLifecycleStatus::CompletedWithParticleErrors
        | RunLifecycleStatus::Cancelled => {}
    }

    let sqlite_path = contained_file(&run_directory, &manifest.sqlite.relative_path)?;
    ensure_terminal_wal(&sqlite_path)?;
    let sqlite = ParticleStateSqliteSink::inspect(&sqlite_path).map_err(|error| {
        ResultVerificationError::new("result.sqlite_invalid", format!("{error:?}"))
    })?;
    if sqlite.row_counts != manifest.sqlite.row_counts {
        return Err(ResultVerificationError::new(
            "result.sqlite_row_counts_mismatch",
            "manifest and SQLite row counts differ",
        ));
    }
    verify_sqlite_run_row(&sqlite_path, &manifest)?;

    let provenance = manifest.provenance.as_ref().ok_or_else(|| {
        ResultVerificationError::new("result.provenance_missing", "manifest has no provenance")
    })?;
    if sqlite.sha256 != provenance.sqlite_sha256 {
        return Err(ResultVerificationError::new(
            "result.sqlite_sha_mismatch",
            "SQLite SHA-256 does not match manifest provenance identity",
        ));
    }
    if sqlite.canonical_sql_sha256 != provenance.sqlite_sql_sha256 {
        return Err(ResultVerificationError::new(
            "result.sqlite_digest_mismatch",
            "canonical SQL digest does not match manifest provenance identity",
        ));
    }

    let bundle_path = contained_file(&run_directory, Path::new(&provenance.relative_path))?;
    let provenance_sha256 = file_sha256(&bundle_path).map_err(|error| {
        ResultVerificationError::new("result.provenance_invalid", format!("{error:?}"))
    })?;
    if provenance_sha256 != provenance.sha256 {
        return Err(ResultVerificationError::new(
            "result.provenance_sha_mismatch",
            "provenance-bundle SHA-256 does not match manifest",
        ));
    }
    let bundle =
        validate_bundle_file_semantics_loose(&bundle_path, &manifest.run_id.0, &sqlite.sha256)
            .map_err(|error| {
                ResultVerificationError::new("result.provenance_invalid", format!("{error:?}"))
            })?;
    if bundle.record_count != provenance.record_count
        || bundle.field_set_count != provenance.field_set_count
        || bundle.sample_count != provenance.sample_count
        || bundle.content_sha256 != provenance.content_sha256
    {
        return Err(ResultVerificationError::new(
            "result.provenance_identity_mismatch",
            "streamed provenance summary does not match manifest",
        ));
    }
    let canonical_output = canonical_output_digest(
        &sqlite.canonical_sql_sha256,
        &bundle.content_sha256,
    )
    .map_err(|error| {
        ResultVerificationError::new("result.canonical_output_invalid", format!("{error:?}"))
    })?;
    if canonical_output != provenance.canonical_output_sha256 {
        return Err(ResultVerificationError::new(
            "result.canonical_output_mismatch",
            "canonical output digest does not match manifest",
        ));
    }

    let full = match mode {
        VerificationMode::Quick => None,
        VerificationMode::Full => Some(verify_full(&sqlite_path, &manifest)?),
    };
    Ok(ResultVerification {
        schema_version: RESULT_VERIFICATION_SCHEMA_ID.into(),
        mode,
        run_success: manifest.status == RunLifecycleStatus::Complete,
        status: manifest.status,
        job_series_id: manifest.job_series_id,
        attempt: manifest.attempt,
        run_id: manifest.run_id,
        manifest_sha256: format!("{:x}", Sha256::digest(&manifest_bytes)),
        sqlite_sha256: sqlite.sha256,
        sqlite_sql_sha256: sqlite.canonical_sql_sha256,
        provenance_sha256,
        provenance_content_sha256: bundle.content_sha256,
        canonical_output_sha256: canonical_output,
        row_counts: sqlite.row_counts,
        full,
    })
}

fn contained_file(
    run_directory: &Path,
    relative_path: &Path,
) -> Result<PathBuf, ResultVerificationError> {
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::CurDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(ResultVerificationError::new(
            "result.artifact_path_invalid",
            "artifact path is not a normalized relative path",
        ));
    }
    let path = fs::canonicalize(run_directory.join(relative_path)).map_err(|error| {
        ResultVerificationError::new(
            "result.artifact_missing",
            format!("open {}: {error}", relative_path.display()),
        )
    })?;
    if !path.starts_with(run_directory) || !path.is_file() {
        return Err(ResultVerificationError::new(
            "result.artifact_path_invalid",
            "artifact resolves outside the run directory or is not a file",
        ));
    }
    Ok(path)
}

fn ensure_terminal_wal(sqlite_path: &Path) -> Result<(), ResultVerificationError> {
    let mut wal = sqlite_path.as_os_str().to_os_string();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    match fs::metadata(&wal) {
        Ok(metadata) if metadata.len() != 0 => Err(ResultVerificationError::new(
            "result.sqlite_wal_not_empty",
            format!("terminal WAL contains {} bytes", metadata.len()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ResultVerificationError::new(
            "result.sqlite_invalid",
            format!("inspect terminal WAL: {error}"),
        )),
    }
}

fn verify_sqlite_run_row(
    sqlite_path: &Path,
    manifest: &RunManifest,
) -> Result<(), ResultVerificationError> {
    let connection = ParticleStateSqliteSink::open_readonly(sqlite_path).map_err(|error| {
        ResultVerificationError::new("result.sqlite_invalid", format!("{error:?}"))
    })?;
    let row = connection
        .query_row(
            "SELECT run_id, job_series_id, attempt, manifest_schema, case_name, status
             FROM run",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .map_err(sqlite_error("read SQLite run identity"))?;
    let expected_status = match manifest.status {
        RunLifecycleStatus::Complete => "complete",
        RunLifecycleStatus::CompletedWithParticleErrors => "completed_with_particle_errors",
        RunLifecycleStatus::Cancelled if manifest.terminations.abnormal_count == 0 => "complete",
        RunLifecycleStatus::Cancelled => "completed_with_particle_errors",
        RunLifecycleStatus::Running
        | RunLifecycleStatus::Failed
        | RunLifecycleStatus::Interrupted => {
            return Err(ResultVerificationError::new(
                "result.status_invalid",
                "manifest state has no verifiable SQLite product",
            ));
        }
    };
    if row.0 != manifest.run_id.0
        || row.1 != manifest.job_series_id.0
        || row.2 != manifest.attempt
        || row.3 != manifest.schema_version
        || row.4 != manifest.case_name
        || row.5 != expected_status
    {
        return Err(ResultVerificationError::new(
            "result.sqlite_identity_mismatch",
            "SQLite run identity or status does not match manifest",
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct TerminationRow {
    reason: String,
    classification: String,
    seconds: i64,
    nanosecond: i64,
}

fn verify_full(
    sqlite_path: &Path,
    manifest: &RunManifest,
) -> Result<FullVerificationSummary, ResultVerificationError> {
    for record in &manifest.mass_ledger {
        if record.imbalance_kg.abs() > record.tolerance_kg {
            return Err(ResultVerificationError::new(
                "result.mass_ledger_invalid",
                format!(
                    "mass imbalance exceeds tolerance at step {}",
                    record.step_index
                ),
            ));
        }
    }
    let connection = ParticleStateSqliteSink::open_readonly(sqlite_path).map_err(|error| {
        ResultVerificationError::new("result.sqlite_invalid", format!("{error:?}"))
    })?;
    let particle_count = audit_particle_rows(&connection)?;
    audit_process_products(&connection)?;
    let output_event_count = audit_output_events(&connection)?;
    let mut terminations = load_terminations(&connection)?;
    verify_termination_summary(&terminations, manifest)?;

    let mut statement = connection
        .prepare(
            "SELECT s.particle_id, s.sample_sequence, s.event_sequence,
                    s.physical_seconds, s.physical_nanosecond, s.elapsed_age_ns,
                    s.longitude_degrees, s.latitude_degrees, s.height_asl_m,
                    s.particle_status, s.termination_reason,
                    s.eastward_wind_m_s, s.northward_wind_m_s,
                    s.geometric_vertical_velocity_m_s, s.air_pressure_pa,
                    s.air_temperature_k, s.wind_validity, s.wind_quality,
                    s.pressure_validity, s.pressure_quality,
                    s.temperature_validity, s.temperature_quality,
                    e.physical_seconds, e.physical_nanosecond, e.event_kind,
                    p.birth_seconds, p.birth_nanosecond
             FROM particle_state s
             JOIN output_event e
               ON e.run_id = s.run_id AND e.event_sequence = s.event_sequence
             JOIN particle p
               ON p.run_id = s.run_id AND p.particle_id = s.particle_id
             ORDER BY s.particle_id ASC, s.sample_sequence ASC",
        )
        .map_err(sqlite_error("prepare particle-state audit"))?;
    let mut rows = statement
        .query([])
        .map_err(sqlite_error("query particle states"))?;
    let mut current_particle = None;
    let mut expected_sequence = 0_i64;
    let mut previous_event = None;
    let mut previous_age = None;
    let mut terminal_seen = false;
    let mut particles_with_samples = 0_u64;
    let mut sample_count = 0_u64;
    while let Some(row) = rows.next().map_err(sqlite_error("read particle state"))? {
        let particle_id = row.get::<_, i64>(0).map_err(sqlite_error("particle id"))?;
        if current_particle != Some(particle_id) {
            current_particle = Some(particle_id);
            expected_sequence = 0;
            previous_event = None;
            previous_age = None;
            terminal_seen = false;
            particles_with_samples = particles_with_samples.checked_add(1).ok_or_else(|| {
                ResultVerificationError::new("result.row_count_overflow", "particle count overflow")
            })?;
        }
        let sample_sequence = row
            .get::<_, i64>(1)
            .map_err(sqlite_error("sample sequence"))?;
        let event_sequence = row
            .get::<_, i64>(2)
            .map_err(sqlite_error("event sequence"))?;
        let seconds = row
            .get::<_, i64>(3)
            .map_err(sqlite_error("sample seconds"))?;
        let nanosecond = row
            .get::<_, i64>(4)
            .map_err(sqlite_error("sample nanosecond"))?;
        let elapsed_age = row.get::<_, i64>(5).map_err(sqlite_error("elapsed age"))?;
        if sample_sequence != expected_sequence
            || previous_event.is_some_and(|previous| event_sequence <= previous)
            || previous_age.is_some_and(|previous| elapsed_age <= previous)
            || terminal_seen
        {
            return Err(ResultVerificationError::new(
                "result.lifecycle_invalid",
                format!("particle {particle_id} has non-contiguous or non-monotonic states"),
            ));
        }
        expected_sequence = expected_sequence.checked_add(1).ok_or_else(|| {
            ResultVerificationError::new("result.row_count_overflow", "sample sequence overflow")
        })?;
        previous_event = Some(event_sequence);
        previous_age = Some(elapsed_age);

        let event_seconds = row
            .get::<_, i64>(22)
            .map_err(sqlite_error("output-event seconds"))?;
        let event_nanosecond = row
            .get::<_, i64>(23)
            .map_err(sqlite_error("output-event nanosecond"))?;
        let event_kind = row
            .get::<_, String>(24)
            .map_err(sqlite_error("output-event kind"))?;
        let birth_seconds = row
            .get::<_, i64>(25)
            .map_err(sqlite_error("birth seconds"))?;
        let birth_nanosecond = row
            .get::<_, i64>(26)
            .map_err(sqlite_error("birth nanosecond"))?;
        let state_time = timestamp_nanoseconds(seconds, nanosecond)?;
        let event_time = timestamp_nanoseconds(event_seconds, event_nanosecond)?;
        let birth_time = timestamp_nanoseconds(birth_seconds, birth_nanosecond)?;
        if state_time != event_time
            || state_time.abs_diff(birth_time)
                != u128::try_from(elapsed_age).map_err(|_| {
                    ResultVerificationError::new(
                        "result.lifecycle_invalid",
                        format!("particle {particle_id} has negative elapsed age"),
                    )
                })?
            || (sample_sequence == 0
                && (state_time != birth_time || elapsed_age != 0 || event_kind != "birth"))
        {
            return Err(ResultVerificationError::new(
                "result.lifecycle_invalid",
                format!("particle {particle_id} state time does not match its lifecycle event"),
            ));
        }

        let longitude = row.get::<_, f64>(6).map_err(sqlite_error("longitude"))?;
        let latitude = row.get::<_, f64>(7).map_err(sqlite_error("latitude"))?;
        let height = row.get::<_, f64>(8).map_err(sqlite_error("height"))?;
        if !longitude.is_finite()
            || !latitude.is_finite()
            || !height.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
        {
            return Err(ResultVerificationError::new(
                "result.nonfinite_state",
                format!("particle {particle_id} contains an invalid position"),
            ));
        }
        let particle_status = row
            .get::<_, String>(9)
            .map_err(sqlite_error("particle status"))?;
        let termination_reason = row
            .get::<_, Option<String>>(10)
            .map_err(sqlite_error("termination reason"))?;
        verify_quality_fields(row)?;
        if particle_status == "terminated" {
            let termination = terminations.remove(&particle_id).ok_or_else(|| {
                ResultVerificationError::new(
                    "result.lifecycle_invalid",
                    format!("particle {particle_id} has no termination row"),
                )
            })?;
            if termination_reason.as_deref() != Some(termination.reason.as_str())
                || seconds != termination.seconds
                || nanosecond != termination.nanosecond
            {
                return Err(ResultVerificationError::new(
                    "result.lifecycle_invalid",
                    format!("particle {particle_id} termination does not match its final state"),
                ));
            }
            terminal_seen = true;
        }
        sample_count = sample_count.checked_add(1).ok_or_else(|| {
            ResultVerificationError::new("result.row_count_overflow", "sample count overflow")
        })?;
    }
    if !terminations.is_empty() {
        return Err(ResultVerificationError::new(
            "result.lifecycle_invalid",
            "one or more termination rows have no terminal particle state",
        ));
    }

    if particles_with_samples != particle_count {
        return Err(ResultVerificationError::new(
            "result.lifecycle_invalid",
            "one or more particles have no state rows",
        ));
    }
    Ok(FullVerificationSummary {
        particle_count,
        sample_count,
        termination_count: manifest
            .terminations
            .normal_count
            .checked_add(manifest.terminations.abnormal_count)
            .ok_or_else(|| {
                ResultVerificationError::new(
                    "result.row_count_overflow",
                    "termination count overflow",
                )
            })?,
        output_event_count,
        mass_ledger_count: u64::try_from(manifest.mass_ledger.len()).map_err(|error| {
            ResultVerificationError::new("result.row_count_overflow", error.to_string())
        })?,
    })
}

fn audit_particle_rows(connection: &Connection) -> Result<u64, ResultVerificationError> {
    let mut statement = connection
        .prepare(
            "SELECT particle_id, dry_air_mass_kg
             FROM particle ORDER BY particle_id ASC",
        )
        .map_err(sqlite_error("prepare particle audit"))?;
    let mut rows = statement
        .query([])
        .map_err(sqlite_error("query particles"))?;
    let mut count = 0_u64;
    while let Some(row) = rows.next().map_err(sqlite_error("read particle"))? {
        let particle_id = row.get::<_, i64>(0).map_err(sqlite_error("particle id"))?;
        let dry_air_mass = row.get::<_, f64>(1).map_err(sqlite_error("dry-air mass"))?;
        if particle_id < 0 || !dry_air_mass.is_finite() || dry_air_mass < 0.0 {
            return Err(ResultVerificationError::new(
                "result.nonfinite_state",
                format!("particle {particle_id} has invalid immutable state"),
            ));
        }
        count = count.checked_add(1).ok_or_else(|| {
            ResultVerificationError::new("result.row_count_overflow", "particle count overflow")
        })?;
    }

    let invalid_masses = connection
        .query_row(
            "SELECT COUNT(*) FROM particle_mass
             WHERE NOT (initial_mass_kg >= 0.0) OR initial_mass_kg > 1.7976931348623157e308
                OR NOT (mass_kg >= 0.0) OR mass_kg > 1.7976931348623157e308",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(sqlite_error("audit particle masses"))?;
    if invalid_masses != 0 {
        return Err(ResultVerificationError::new(
            "result.nonfinite_state",
            "one or more particle masses are invalid",
        ));
    }
    let invalid_adjoint = connection
        .query_row(
            "SELECT COUNT(*) FROM particle_adjoint
             WHERE initial_adjoint_weight < -1.7976931348623157e308
                OR initial_adjoint_weight > 1.7976931348623157e308
                OR adjoint_weight < -1.7976931348623157e308
                OR adjoint_weight > 1.7976931348623157e308
                OR source_sensitivity < -1.7976931348623157e308
                OR source_sensitivity > 1.7976931348623157e308",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(sqlite_error("audit particle adjoints"))?;
    if invalid_adjoint != 0 {
        return Err(ResultVerificationError::new(
            "result.nonfinite_state",
            "one or more particle adjoint values are invalid",
        ));
    }
    let (direction, mass_count, adjoint_count): (String, u64, u64) = connection
        .query_row(
            "SELECT direction,
                    (SELECT COUNT(*) FROM particle_mass),
                    (SELECT COUNT(*) FROM particle_adjoint)
             FROM run",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(sqlite_error("audit directional substance state"))?;
    if (direction == "forward" && adjoint_count != 0)
        || (direction == "backward" && mass_count != 0)
        || !matches!(direction.as_str(), "forward" | "backward")
    {
        return Err(ResultVerificationError::new(
            "result.directional_state_invalid",
            "SQLite mixes forward mass and backward adjoint state",
        ));
    }
    Ok(count)
}

fn audit_process_products(connection: &Connection) -> Result<(), ResultVerificationError> {
    let foreign_key_failure = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error("audit SQLite foreign keys"))?;
    if foreign_key_failure {
        return Err(ResultVerificationError::new(
            "result.process_integrity_invalid",
            "SQLite process tables contain a foreign-key violation",
        ));
    }

    audit_continuous_motion_state(connection)?;

    let invalid_direction = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM process_summary s JOIN run r USING (run_id)
                 WHERE s.direction <> r.direction
                 UNION ALL
                 SELECT 1 FROM process_event e JOIN run r USING (run_id)
                 WHERE e.direction <> r.direction
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error("audit process direction"))?;
    if invalid_direction {
        return Err(ResultVerificationError::new(
            "result.process_direction_invalid",
            "process rows do not match the run direction",
        ));
    }

    let invalid_detail = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM process_event e
                 WHERE
                   (SELECT COUNT(*) FROM water_vapor_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence)
                 + (SELECT COUNT(*) FROM deposition_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence)
                 + (SELECT COUNT(*) FROM chemistry_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence)
                 + (SELECT COUNT(*) FROM emission_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence)
                 + (SELECT COUNT(*) FROM convection_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence) <> 1
                 OR (e.detail_kind='water_vapor' AND NOT EXISTS(
                    SELECT 1 FROM water_vapor_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence))
                 OR (e.detail_kind='deposition' AND NOT EXISTS(
                    SELECT 1 FROM deposition_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence))
                 OR (e.detail_kind='chemistry' AND NOT EXISTS(
                    SELECT 1 FROM chemistry_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence))
                 OR (e.detail_kind='emission' AND NOT EXISTS(
                    SELECT 1 FROM emission_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence))
                 OR (e.detail_kind='convection' AND NOT EXISTS(
                    SELECT 1 FROM convection_event d
                    WHERE d.run_id=e.run_id AND d.event_sequence=e.event_sequence))
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error("audit process details"))?;
    if invalid_detail {
        return Err(ResultVerificationError::new(
            "result.process_detail_invalid",
            "a process event is missing its unique matching detail row",
        ));
    }

    let invalid_convection = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM process_event e
                 JOIN convection_event d
                   ON d.run_id=e.run_id AND d.event_sequence=e.event_sequence
                 WHERE e.detail_kind <> 'convection'
                    OR e.module_id <> 'deep_convection_column'
                    OR ABS(d.column_residual) > 1.0e-12
                    OR (e.direction='forward' AND (
                        e.mass_delta_kg <> 0.0
                        OR d.importance_weight <> 1.0
                    ))
                    OR (e.direction='backward' AND (
                        e.survival_multiplier <> 1.0
                        OR e.source_sensitivity <> 0.0
                        OR ABS(e.importance_weight-d.importance_weight) > 1.0e-12
                    ))
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error("audit convection process contract"))?;
    if invalid_convection {
        return Err(ResultVerificationError::new(
            "result.convection_event_invalid",
            "a convection event violates the conservative column contract",
        ));
    }
    audit_convection_particle_closure(connection)?;

    let continuous_event_count = connection
        .query_row(
            "SELECT COUNT(*) FROM process_event
             WHERE module_id IN (
               'boundary_layer_langevin', 'mesoscale_markov',
               'subgrid_orography', 'gravitational_settling'
             )",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(sqlite_error("audit continuous process events"))?;
    if continuous_event_count != 0 {
        return Err(ResultVerificationError::new(
            "result.process_event_policy_invalid",
            "a continuous-only module emitted a discrete process event",
        ));
    }

    let invalid_summary_count = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM process_summary s
                 WHERE s.event_count <> (
                   SELECT COUNT(*) FROM process_event e
                   WHERE e.run_id=s.run_id AND e.module_id=s.module_id
                     AND e.substance_id=s.substance_id
                 )
                 UNION ALL
                 SELECT 1 FROM process_event e
                 WHERE NOT EXISTS (
                   SELECT 1 FROM process_summary s
                   WHERE s.run_id=e.run_id AND s.module_id=e.module_id
                     AND s.substance_id=e.substance_id
                 )
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error("audit process summary counts"))?;
    if invalid_summary_count {
        return Err(ResultVerificationError::new(
            "result.process_summary_invalid",
            "process events and summary counts do not form a complete mapping",
        ));
    }

    let mut statement = connection
        .prepare(
            "SELECT direction, event_count, initial_mass_kg,
                    positive_mass_delta_kg, negative_mass_delta_kg, final_mass_kg,
                    initial_adjoint_weight, survival_multiplier_product,
                    source_sensitivity, convection_importance_product,
                    final_adjoint_weight, closure_residual
             FROM process_summary ORDER BY module_id, substance_id",
        )
        .map_err(sqlite_error("prepare process closure audit"))?;
    let mut rows = statement
        .query([])
        .map_err(sqlite_error("query process closures"))?;
    while let Some(row) = rows.next().map_err(sqlite_error("read process closure"))? {
        let direction = row
            .get::<_, String>(0)
            .map_err(sqlite_error("process direction"))?;
        let event_count = row
            .get::<_, u64>(1)
            .map_err(sqlite_error("process event count"))?;
        let residual = row
            .get::<_, f64>(11)
            .map_err(sqlite_error("process closure residual"))?;
        let scale = match direction.as_str() {
            "forward" => {
                let initial = row.get::<_, f64>(2).map_err(sqlite_error("initial mass"))?;
                let positive = row
                    .get::<_, f64>(3)
                    .map_err(sqlite_error("positive mass"))?;
                let negative = row
                    .get::<_, f64>(4)
                    .map_err(sqlite_error("negative mass"))?;
                let final_mass = row.get::<_, f64>(5).map_err(sqlite_error("final mass"))?;
                let expected = final_mass - initial - positive - negative;
                if (residual - expected).abs() > 1.0e-12 {
                    return Err(ResultVerificationError::new(
                        "result.process_closure_invalid",
                        "forward process closure residual is inconsistent",
                    ));
                }
                initial.abs().max(final_mass.abs())
            }
            "backward" => {
                let initial = row
                    .get::<_, f64>(6)
                    .map_err(sqlite_error("initial adjoint"))?;
                let survival = row
                    .get::<_, f64>(7)
                    .map_err(sqlite_error("survival product"))?;
                let source = row
                    .get::<_, f64>(8)
                    .map_err(sqlite_error("source sensitivity"))?;
                let importance = row
                    .get::<_, f64>(9)
                    .map_err(sqlite_error("convection importance"))?;
                let final_weight = row
                    .get::<_, f64>(10)
                    .map_err(sqlite_error("final adjoint"))?;
                let expected = final_weight - initial * survival * importance - source;
                if (residual - expected).abs() > 1.0e-12 {
                    return Err(ResultVerificationError::new(
                        "result.process_closure_invalid",
                        "backward process closure residual is inconsistent",
                    ));
                }
                if event_count == 0
                    && ((survival - 1.0).abs() > 1.0e-12
                        || source.abs() > 1.0e-12
                        || (importance - 1.0).abs() > 1.0e-12
                        || (residual - (final_weight - initial)).abs() > 1.0e-12)
                {
                    return Err(ResultVerificationError::new(
                        "result.process_closure_invalid",
                        "continuous backward process closure is inconsistent",
                    ));
                }
                initial.abs().max(final_weight.abs())
            }
            _ => {
                return Err(ResultVerificationError::new(
                    "result.process_direction_invalid",
                    "unknown process direction",
                ));
            }
        };
        if !residual.is_finite() || residual.abs() > 1.0e-10 * scale.max(1.0) {
            return Err(ResultVerificationError::new(
                "result.process_closure_invalid",
                "process closure exceeds the frozen relative tolerance",
            ));
        }
    }
    Ok(())
}

fn audit_convection_particle_closure(
    connection: &Connection,
) -> Result<(), ResultVerificationError> {
    let has_convection = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM process_summary
                            WHERE module_id='deep_convection_column')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error("audit convection presence"))?;
    if !has_convection {
        return Ok(());
    }

    let direction = connection
        .query_row("SELECT direction FROM run", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sqlite_error("audit convection direction"))?;
    let (table, initial_column, final_column) = match direction.as_str() {
        "forward" => ("particle_mass", "initial_mass_kg", "mass_kg"),
        "backward" => (
            "particle_adjoint",
            "initial_adjoint_weight",
            "adjoint_weight",
        ),
        _ => {
            return Err(ResultVerificationError::new(
                "result.process_direction_invalid",
                "unknown convection direction",
            ));
        }
    };
    let sql = format!(
        "SELECT particle_id, substance_id, {initial_column}, {final_column}
         FROM {table} ORDER BY particle_id, substance_id"
    );
    let states = connection
        .prepare(&sql)
        .map_err(sqlite_error("prepare convection particle states"))?
        .query_map([], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })
        .map_err(sqlite_error("query convection particle states"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error("read convection particle states"))?;

    let mut event_statement = connection
        .prepare(
            "SELECT importance_weight FROM process_event
             WHERE particle_id=?1 AND module_id='deep_convection_column'
               AND substance_id=?2 ORDER BY macro_step",
        )
        .map_err(sqlite_error("prepare convection particle events"))?;
    for (particle_id, substance_id, initial, final_value) in states {
        let mut importance = 1.0;
        let mut rows = event_statement
            .query(rusqlite::params![particle_id, substance_id])
            .map_err(sqlite_error("query convection particle events"))?;
        while let Some(row) = rows
            .next()
            .map_err(sqlite_error("read convection particle event"))?
        {
            if direction == "backward" {
                importance *= row
                    .get::<_, f64>(0)
                    .map_err(sqlite_error("convection importance weight"))?;
            }
        }
        let expected = if direction == "forward" {
            initial
        } else {
            initial * importance
        };
        let scale = initial.abs().max(final_value.abs()).max(1.0);
        if !importance.is_finite()
            || !expected.is_finite()
            || (final_value - expected).abs() > 1.0e-10 * scale
        {
            return Err(ResultVerificationError::new(
                "result.convection_particle_closure_invalid",
                "convection events do not close against one particle's directional state",
            ));
        }
    }
    Ok(())
}

fn audit_continuous_motion_state(connection: &Connection) -> Result<(), ResultVerificationError> {
    let (has_boundary_layer, has_mesoscale): (bool, bool) = connection
        .query_row(
            "SELECT
               EXISTS(SELECT 1 FROM process_summary
                      WHERE module_id='boundary_layer_langevin'),
               EXISTS(SELECT 1 FROM process_summary
                      WHERE module_id='mesoscale_markov')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite_error("audit continuous motion modules"))?;
    let invalid = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM particle_state
                 WHERE
                   ((boundary_layer_random_eastward_m_s IS NULL)
                     <> (boundary_layer_random_northward_m_s IS NULL))
                   OR ((boundary_layer_random_eastward_m_s IS NULL)
                     <> (boundary_layer_random_vertical_m_s IS NULL))
                   OR ((mesoscale_random_eastward_m_s IS NULL)
                     <> (mesoscale_random_northward_m_s IS NULL))
                   OR ((mesoscale_random_eastward_m_s IS NULL)
                     <> (mesoscale_random_vertical_m_s IS NULL))
                   OR (?1 AND elapsed_age_ns > 0
                     AND boundary_layer_random_eastward_m_s IS NULL)
                   OR (NOT ?1 AND boundary_layer_random_eastward_m_s IS NOT NULL)
                   OR (?2 AND elapsed_age_ns > 0
                     AND mesoscale_random_eastward_m_s IS NULL)
                   OR (NOT ?2 AND mesoscale_random_eastward_m_s IS NOT NULL)
                   OR boundary_layer_random_eastward_m_s < -1.7976931348623157e308
                   OR boundary_layer_random_eastward_m_s > 1.7976931348623157e308
                   OR boundary_layer_random_northward_m_s < -1.7976931348623157e308
                   OR boundary_layer_random_northward_m_s > 1.7976931348623157e308
                   OR boundary_layer_random_vertical_m_s < -1.7976931348623157e308
                   OR boundary_layer_random_vertical_m_s > 1.7976931348623157e308
                   OR mesoscale_random_eastward_m_s < -1.7976931348623157e308
                   OR mesoscale_random_eastward_m_s > 1.7976931348623157e308
                   OR mesoscale_random_northward_m_s < -1.7976931348623157e308
                   OR mesoscale_random_northward_m_s > 1.7976931348623157e308
                   OR mesoscale_random_vertical_m_s < -1.7976931348623157e308
                   OR mesoscale_random_vertical_m_s > 1.7976931348623157e308
             )",
            rusqlite::params![has_boundary_layer, has_mesoscale],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error("audit continuous motion state"))?;
    if invalid {
        return Err(ResultVerificationError::new(
            "result.process_state_invalid",
            "continuous process state is missing, partial, non-finite, or undeclared",
        ));
    }
    Ok(())
}

fn audit_output_events(connection: &Connection) -> Result<u64, ResultVerificationError> {
    let mut statement = connection
        .prepare(
            "SELECT event_sequence, physical_seconds, physical_nanosecond
             FROM output_event ORDER BY event_sequence ASC",
        )
        .map_err(sqlite_error("prepare output-event audit"))?;
    let mut rows = statement
        .query([])
        .map_err(sqlite_error("query output events"))?;
    let mut count = 0_u64;
    while let Some(row) = rows.next().map_err(sqlite_error("read output event"))? {
        let sequence = row
            .get::<_, i64>(0)
            .map_err(sqlite_error("output-event sequence"))?;
        let seconds = row
            .get::<_, i64>(1)
            .map_err(sqlite_error("output-event seconds"))?;
        let nanosecond = row
            .get::<_, i64>(2)
            .map_err(sqlite_error("output-event nanosecond"))?;
        let expected = i64::try_from(count).map_err(|_| {
            ResultVerificationError::new("result.row_count_overflow", "event count overflow")
        })?;
        timestamp_nanoseconds(seconds, nanosecond)?;
        if sequence != expected {
            return Err(ResultVerificationError::new(
                "result.lifecycle_invalid",
                "output-event sequence is not contiguous from zero",
            ));
        }
        count = count.checked_add(1).ok_or_else(|| {
            ResultVerificationError::new("result.row_count_overflow", "event count overflow")
        })?;
    }
    let empty_events = connection
        .query_row(
            "SELECT COUNT(*) FROM output_event e
             WHERE NOT EXISTS (
                 SELECT 1 FROM particle_state s
                 WHERE s.run_id = e.run_id AND s.event_sequence = e.event_sequence
             )",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(sqlite_error("audit empty output events"))?;
    if empty_events != 0 {
        return Err(ResultVerificationError::new(
            "result.lifecycle_invalid",
            "one or more output events contain no particle states",
        ));
    }
    Ok(count)
}

fn timestamp_nanoseconds(seconds: i64, nanosecond: i64) -> Result<i128, ResultVerificationError> {
    if !(0..1_000_000_000).contains(&nanosecond) {
        return Err(ResultVerificationError::new(
            "result.lifecycle_invalid",
            "timestamp nanosecond is outside [0, 1e9)",
        ));
    }
    Ok(i128::from(seconds) * 1_000_000_000_i128 + i128::from(nanosecond))
}

fn load_terminations(
    connection: &Connection,
) -> Result<HashMap<i64, TerminationRow>, ResultVerificationError> {
    let mut statement = connection
        .prepare(
            "SELECT particle_id, reason, classification, physical_seconds, physical_nanosecond
             FROM termination ORDER BY particle_id ASC",
        )
        .map_err(sqlite_error("prepare termination audit"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                TerminationRow {
                    reason: row.get(1)?,
                    classification: row.get(2)?,
                    seconds: row.get(3)?,
                    nanosecond: row.get(4)?,
                },
            ))
        })
        .map_err(sqlite_error("query terminations"))?;
    let mut terminations = HashMap::new();
    for row in rows {
        let (particle_id, termination) = row.map_err(sqlite_error("read termination"))?;
        if terminations.insert(particle_id, termination).is_some() {
            return Err(ResultVerificationError::new(
                "result.lifecycle_invalid",
                "duplicate termination particle identity",
            ));
        }
    }
    Ok(terminations)
}

fn verify_termination_summary(
    terminations: &HashMap<i64, TerminationRow>,
    manifest: &RunManifest,
) -> Result<(), ResultVerificationError> {
    let mut normal = 0_u64;
    let mut abnormal = 0_u64;
    let mut by_reason = BTreeMap::new();
    for termination in terminations.values() {
        match termination.classification.as_str() {
            "normal" => normal += 1,
            "abnormal" => abnormal += 1,
            _ => {
                return Err(ResultVerificationError::new(
                    "result.termination_invalid",
                    "unknown termination classification",
                ));
            }
        }
        *by_reason.entry(termination.reason.clone()).or_insert(0_u64) += 1;
    }
    if normal != manifest.terminations.normal_count
        || abnormal != manifest.terminations.abnormal_count
        || by_reason != manifest.terminations.by_reason
    {
        return Err(ResultVerificationError::new(
            "result.termination_mismatch",
            "SQLite termination summary does not match manifest",
        ));
    }
    Ok(())
}

fn verify_quality_fields(row: &rusqlite::Row<'_>) -> Result<(), ResultVerificationError> {
    let east = optional_finite(row, 11, "eastward wind")?;
    let north = optional_finite(row, 12, "northward wind")?;
    let vertical = optional_finite(row, 13, "vertical velocity")?;
    let pressure = optional_finite(row, 14, "air pressure")?;
    let temperature = optional_finite(row, 15, "air temperature")?;
    if pressure.is_some_and(|value| value <= 0.0) || temperature.is_some_and(|value| value <= 0.0) {
        return Err(ResultVerificationError::new(
            "result.quality_invalid",
            "pressure and temperature must be positive when present",
        ));
    }
    let wind_validity = row
        .get::<_, String>(16)
        .map_err(sqlite_error("wind validity"))?;
    let wind_quality = row
        .get::<_, Option<String>>(17)
        .map_err(sqlite_error("wind quality"))?;
    let pressure_validity = row
        .get::<_, String>(18)
        .map_err(sqlite_error("pressure validity"))?;
    let pressure_quality = row
        .get::<_, Option<String>>(19)
        .map_err(sqlite_error("pressure quality"))?;
    let temperature_validity = row
        .get::<_, String>(20)
        .map_err(sqlite_error("temperature validity"))?;
    let temperature_quality = row
        .get::<_, Option<String>>(21)
        .map_err(sqlite_error("temperature quality"))?;
    let wind_count = [east, north, vertical]
        .into_iter()
        .filter(Option::is_some)
        .count();
    let wind_shape_valid = match wind_validity.as_str() {
        "missing" => wind_count == 0 && optional_quality_valid(wind_quality.as_deref()),
        "partial" => (1..3).contains(&wind_count) && valid_quality(wind_quality.as_deref()),
        value if valid_validity(value) => wind_count == 3 && valid_quality(wind_quality.as_deref()),
        _ => false,
    };
    if !wind_shape_valid
        || !scalar_quality_valid(pressure, &pressure_validity, pressure_quality.as_deref())
        || !scalar_quality_valid(
            temperature,
            &temperature_validity,
            temperature_quality.as_deref(),
        )
    {
        return Err(ResultVerificationError::new(
            "result.quality_invalid",
            "meteorology value, validity, and quality fields are inconsistent",
        ));
    }
    Ok(())
}

fn optional_finite(
    row: &rusqlite::Row<'_>,
    column: usize,
    label: &'static str,
) -> Result<Option<f64>, ResultVerificationError> {
    let value = row
        .get::<_, Option<f64>>(column)
        .map_err(sqlite_error(label))?;
    if value.is_some_and(|value| !value.is_finite()) {
        return Err(ResultVerificationError::new(
            "result.nonfinite_state",
            format!("{label} is not finite"),
        ));
    }
    Ok(value)
}

fn scalar_quality_valid(value: Option<f64>, validity: &str, quality: Option<&str>) -> bool {
    match value {
        None => validity == "missing" && optional_quality_valid(quality),
        Some(_) => valid_validity(validity) && valid_quality(quality),
    }
}

fn optional_quality_valid(quality: Option<&str>) -> bool {
    quality.is_none() || valid_quality(quality)
}

fn valid_quality(quality: Option<&str>) -> bool {
    matches!(quality, Some("source" | "derived" | "estimated"))
}

fn valid_validity(validity: &str) -> bool {
    matches!(
        validity,
        "ok" | "outofdomain"
            | "polarsingularity"
            | "belowground"
            | "surfacelayerundefined"
            | "aboveavailabletop"
            | "abovemodeltop"
            | "invalidverticalcolumn"
            | "numericalfailure"
    )
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> ResultVerificationError {
    move |error| ResultVerificationError::new("result.io_failed", format!("{context}: {error}"))
}

fn sqlite_error(context: &'static str) -> impl FnOnce(rusqlite::Error) -> ResultVerificationError {
    move |error| {
        ResultVerificationError::new("result.sqlite_invalid", format!("{context}: {error}"))
    }
}
