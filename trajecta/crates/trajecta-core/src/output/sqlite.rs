//! # Contract: particle_state_sqlite/v1 typed sink
//!
//! Implements the public SQLite schema with WAL, NORMAL sync, foreign keys,
//! atomic events, bounded lifecycle transactions, and live-read inspection.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{CachedStatement, Connection, params};
use sha2::{Digest, Sha256};
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::FieldQuality;
use trajecta_met::performance::{PerformanceScope, PerformanceStage};
use trajecta_met::provenance::ProvenanceTable;
use trajecta_met::query::output::{QueryOutput, SampleStatus};

use crate::manifest::ProvenanceBundleIdentity;
use crate::manifest::{RunManifest, SqliteOutputSummary};
use crate::output::provenance_bundle::{
    FiveFieldRecordRefs, ProvenanceBundleBuilder, file_sha256, five_field_slot,
    quarantine_bundle_file,
};
use crate::output::{OutputError, ParticleStateSink};
use crate::particle::{ParticleBatch, ParticleOrigin, ParticleStatus, TerminationClass};
use crate::science::SQLITE_SCHEMA_VERSION;

/// Embedded public schema used to initialize every particle-state database.
pub const SQLITE_SCHEMA_SQL: &str = include_str!("../../../../testdata/M4_SQLITE_SCHEMA.v1.sql");

const MAX_LIFECYCLE_EVENTS_PER_TRANSACTION: usize = 512;
const SQLITE_PAGE_SIZE_BYTES: i64 = 32 * 1024;

const CANONICAL_PARTICLE_SQL: &str = "SELECT particle_id, population_id, origin_kind,
            origin_event_id, origin_domain_id, origin_boundary_face_id,
            birth_seconds, birth_nanosecond, dry_air_mass_kg, sensitivity_weight
     FROM particle
     WHERE run_id = ?1
     ORDER BY particle_id";
const CANONICAL_PARTICLE_MASS_SQL: &str = "SELECT particle_id, substance_id, mass_kg
     FROM particle_mass
     WHERE run_id = ?1
     ORDER BY particle_id, substance_id";
const CANONICAL_OUTPUT_EVENT_SQL: &str =
    "SELECT event_sequence, physical_seconds, physical_nanosecond, event_kind
     FROM output_event
     WHERE run_id = ?1
     ORDER BY event_sequence";
// Unary `+` preserves INTEGER ordering while preventing SQLite from selecting
// the non-covering time index and randomly looking up every wide state row.
// The run-scoped primary-key range scan plus sort is faster on mounted output
// files and yields byte-identical canonical row order.
const CANONICAL_PARTICLE_STATE_SQL: &str = "SELECT particle_id, sample_sequence, event_sequence,
            physical_seconds, physical_nanosecond,
            integration_offset_ns, elapsed_age_ns,
            longitude_degrees, latitude_degrees, height_asl_m,
            particle_status, termination_reason,
            eastward_wind_m_s, northward_wind_m_s, geometric_vertical_velocity_m_s,
            air_pressure_pa, air_temperature_k,
            wind_validity, wind_quality,
            pressure_validity, pressure_quality,
            temperature_validity, temperature_quality
     FROM particle_state
     WHERE run_id = ?1
     ORDER BY +physical_seconds, physical_nanosecond, particle_id, sample_sequence";
const CANONICAL_TERMINATION_SQL: &str = "SELECT particle_id, reason, classification,
            physical_seconds, physical_nanosecond, intersection_fraction
     FROM termination
     WHERE run_id = ?1
     ORDER BY particle_id";

#[derive(Clone, Debug)]
struct WrittenEvent {
    sequence: i64,
    kind: String,
}

#[derive(Clone, Copy, Debug)]
struct WrittenSample {
    sample_sequence: i64,
    event_sequence: i64,
    terminal: bool,
}

/// Production SQLite particle-state sink.
#[derive(Default)]
pub struct ParticleStateSqliteSink {
    path: Option<PathBuf>,
    connection: Option<Connection>,
    transaction_open: bool,
    pending_lifecycle_events: usize,
    run_id: String,
    event_sequence: i64,
    sample_sequence_by_particle: HashMap<i64, i64>,
    seen_events: HashMap<(i64, u32), WrittenEvent>,
    written_samples: HashMap<(i64, i64, u32), WrittenSample>,
    particles_inserted: HashSet<i64>,
    /// Particles that already received a terminal sample row.
    terminated_written: HashSet<i64>,
    run_start: Option<Timestamp>,
    run_end: Option<Timestamp>,
    row_counts: BTreeMap<String, u64>,
    /// Run-stable provenance intern table (legacy single provenance_id column).
    provenance: ProvenanceTable,
    /// Formal provenance-bundle/v1 builder (samples spooled; records/field-sets interned).
    bundle: Option<ProvenanceBundleBuilder>,
    /// Identity after successful finish finalize.
    bundle_identity: Option<ProvenanceBundleIdentity>,
    /// Optional external-sort chunk size (production algorithm; test matrix knobs).
    bundle_chunk_lines: Option<usize>,
    /// Optional external-sort merge fan-in.
    bundle_merge_fan_in: Option<usize>,
    /// When true, scan particle indices in reverse (order stress; same particle_ids).
    reverse_particle_scan: bool,
}

impl ParticleStateSqliteSink {
    /// Creates a sink bound to the Case integration window for start/end labeling.
    #[must_use]
    pub fn with_time_bounds(run_start: Timestamp, run_end: Timestamp) -> Self {
        Self {
            run_start: Some(run_start),
            run_end: Some(run_end),
            ..Self::default()
        }
    }

    /// Test/production knobs: same external-sort implementation, different limits/order.
    #[must_use]
    pub fn with_bundle_sort_knobs(
        mut self,
        chunk_lines: Option<usize>,
        merge_fan_in: Option<usize>,
        reverse_particle_scan: bool,
    ) -> Self {
        self.bundle_chunk_lines = chunk_lines;
        self.bundle_merge_fan_in = merge_fan_in;
        self.reverse_particle_scan = reverse_particle_scan;
        self
    }

    /// Opens a read-only connection against an already-written database.
    pub fn open_readonly(path: &Path) -> Result<Connection, OutputError> {
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(io_err)
    }

    /// Returns path/size/sha/integrity diagnostics for a finished database.
    ///
    /// Size uses metadata; SHA uses fixed-buffer streaming `file_sha256` — never
    /// full-file `fs::read` on the production inspection path.
    pub fn inspect(path: &Path) -> Result<SqliteInspection, OutputError> {
        let size_bytes = fs::metadata(path)
            .map_err(|error| OutputError::Io(error.to_string()))?
            .len();
        let sha256 = file_sha256(path)?;
        let connection = Self::open_readonly(path)?;
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(io_err)?;
        if integrity != "ok" {
            return Err(OutputError::Io(format!("integrity_check={integrity}")));
        }
        let user_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(io_err)?;
        if user_version != i64::from(SQLITE_SCHEMA_VERSION) {
            return Err(OutputError::Io(format!(
                "user_version={user_version} expected {}",
                SQLITE_SCHEMA_VERSION
            )));
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
                .map_err(io_err)?;
            row_counts.insert(table.to_owned(), u64::try_from(count).unwrap_or(0));
        }
        if row_counts.get("run").copied() != Some(1) {
            return Err(OutputError::Io(format!(
                "expected exactly one run row, found {}",
                row_counts.get("run").copied().unwrap_or(0)
            )));
        }
        let run_id: String = connection
            .query_row("SELECT run_id FROM run", [], |row| row.get(0))
            .map_err(io_err)?;
        let canonical_sql = canonical_sql_digest(&connection, &run_id)?;
        Ok(SqliteInspection {
            path: path.to_path_buf(),
            size_bytes,
            sha256,
            integrity,
            row_counts,
            canonical_sql_sha256: canonical_sql,
        })
    }

    fn begin_transaction(&mut self) -> Result<(), OutputError> {
        if self.transaction_open {
            return Ok(());
        }
        self.connection
            .as_mut()
            .ok_or(OutputError::InvalidInput)?
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(io_err)?;
        self.transaction_open = true;
        self.pending_lifecycle_events = 0;
        Ok(())
    }

    fn commit_transaction(&mut self) -> Result<(), OutputError> {
        if !self.transaction_open {
            return Ok(());
        }
        self.connection
            .as_mut()
            .ok_or(OutputError::InvalidInput)?
            .execute_batch("COMMIT")
            .map_err(io_err)?;
        self.transaction_open = false;
        self.pending_lifecycle_events = 0;
        Ok(())
    }
}

/// One finished SQLite artifact inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteInspection {
    /// Database path.
    pub path: PathBuf,
    /// Byte size.
    pub size_bytes: u64,
    /// Content SHA-256.
    pub sha256: String,
    /// `PRAGMA integrity_check` result.
    pub integrity: String,
    /// Row counts by table.
    pub row_counts: BTreeMap<String, u64>,
    /// Canonical ordered SQL export digest.
    pub canonical_sql_sha256: String,
}

impl ParticleStateSink for ParticleStateSqliteSink {
    fn sink_id(&self) -> &'static str {
        trajecta_case::model::output::PARTICLE_STATE_SQLITE_SINK_ID
    }

    fn begin(&mut self, target: &Path, manifest: &RunManifest) -> Result<(), OutputError> {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| OutputError::Io(error.to_string()))?;
        }
        if target.exists() {
            fs::remove_file(target).map_err(|error| OutputError::Io(error.to_string()))?;
        }
        let connection = Connection::open(target).map_err(io_err)?;
        connection
            .execute_batch(SQLITE_SCHEMA_SQL)
            .map_err(io_err)?;
        validate_pragmas(&connection)?;
        let started = manifest.started_at;
        connection
            .execute(
                "INSERT INTO run (
                    run_id, manifest_schema, case_name, status,
                    started_seconds, started_nanosecond, finished_seconds, finished_nanosecond
                ) VALUES (?1, ?2, ?3, 'running', ?4, ?5, NULL, NULL)",
                params![
                    manifest.run_id.0,
                    manifest.schema_version,
                    manifest.case_name,
                    started.seconds_since_unix_epoch(),
                    started.nanosecond(),
                ],
            )
            .map_err(io_err)?;
        self.path = Some(target.to_path_buf());
        self.connection = Some(connection);
        self.transaction_open = false;
        self.pending_lifecycle_events = 0;
        self.run_id = manifest.run_id.0.clone();
        self.event_sequence = 0;
        self.sample_sequence_by_particle.clear();
        self.seen_events.clear();
        self.written_samples.clear();
        self.particles_inserted.clear();
        self.terminated_written.clear();
        self.row_counts.clear();
        self.provenance = ProvenanceTable::new();
        self.bundle_identity = None;
        let run_dir = target
            .parent()
            .ok_or_else(|| OutputError::Io("sqlite target has no parent run dir".into()))?;
        self.bundle = Some(match (self.bundle_chunk_lines, self.bundle_merge_fan_in) {
            (Some(chunk), Some(fan)) => {
                ProvenanceBundleBuilder::begin_with_limits(run_dir, &manifest.run_id.0, chunk, fan)?
            }
            (Some(chunk), None) => ProvenanceBundleBuilder::begin_with_limits(
                run_dir,
                &manifest.run_id.0,
                chunk,
                crate::output::provenance_bundle::DEFAULT_MERGE_FAN_IN,
            )?,
            (None, Some(fan)) => ProvenanceBundleBuilder::begin_with_limits(
                run_dir,
                &manifest.run_id.0,
                crate::output::provenance_bundle::DEFAULT_SAMPLE_CHUNK_LINES,
                fan,
            )?,
            (None, None) => ProvenanceBundleBuilder::begin(run_dir, &manifest.run_id.0)?,
        });
        Ok(())
    }

    fn write_event(
        &mut self,
        time: Timestamp,
        particles: &ParticleBatch,
        meteorology: Option<&QueryOutput>,
    ) -> Result<(), OutputError> {
        let run_id = self.run_id.clone();
        let count = particles.len().map_err(|_| OutputError::InvalidInput)?;
        if let Some(met) = meteorology {
            if met.status().len() != count {
                return Err(OutputError::InvalidInput);
            }
        }

        // One logical event key per coincident birth/start/interval/end/termination family.
        let mut any_birth = false;
        let mut newly_terminated = false;
        let count_scan = particles.len().map_err(|_| OutputError::InvalidInput)?;
        for index in 0..count_scan {
            if particles.birth_time[index] == time {
                any_birth = true;
            }
            if matches!(particles.status[index], ParticleStatus::Terminated { .. }) {
                let pid = i64::try_from(particles.id[index].0)
                    .map_err(|_| OutputError::Encoding("particle_id".into()))?;
                if !self.terminated_written.contains(&pid) {
                    if particles.termination[index]
                        .as_ref()
                        .is_some_and(|termination| termination.time != time)
                    {
                        return Err(OutputError::InvalidInput);
                    }
                    newly_terminated = true;
                }
            }
        }
        let event_kind = infer_event_kind(
            particles,
            time,
            self.run_start,
            self.run_end,
            newly_terminated,
            any_birth,
        )?;
        let lifecycle_event = matches!(event_kind, "birth" | "termination");
        if !lifecycle_event {
            self.commit_transaction()?;
        }
        self.begin_transaction()?;
        let connection = self.connection.as_mut().ok_or(OutputError::InvalidInput)?;
        let mut statements = EventStatements::prepare(connection)?;
        let event_key = (time.seconds_since_unix_epoch(), time.nanosecond());
        let event_sequence = if let Some(existing) = self.seen_events.get_mut(&event_key) {
            if event_kind_priority(event_kind) > event_kind_priority(&existing.kind) {
                let updated = statements
                    .output_event_kind
                    .execute(params![run_id, existing.sequence, event_kind])
                    .map_err(io_err)?;
                if updated != 1 {
                    return Err(OutputError::InvalidInput);
                }
                existing.kind = event_kind.to_owned();
            }
            existing.sequence
        } else {
            let sequence = self.event_sequence;
            statements
                .output_event
                .execute(params![
                    run_id,
                    sequence,
                    time.seconds_since_unix_epoch(),
                    time.nanosecond(),
                    event_kind,
                ])
                .map_err(io_err)?;
            self.seen_events.insert(
                event_key,
                WrittenEvent {
                    sequence,
                    kind: event_kind.to_owned(),
                },
            );
            self.event_sequence = self
                .event_sequence
                .checked_add(1)
                .ok_or_else(|| OutputError::Io("event sequence overflow".into()))?;
            sequence
        };

        let mut indices: Vec<usize> = (0..count).collect();
        indices.sort_unstable_by_key(|index| particles.id[*index]);
        if self.reverse_particle_scan {
            indices.reverse();
        }
        for index in indices {
            let particle_id = i64::try_from(particles.id[index].0)
                .map_err(|_| OutputError::Encoding("particle_id outside i64".into()))?;
            if matches!(particles.status[index], ParticleStatus::Terminated { .. })
                && self.terminated_written.contains(&particle_id)
            {
                continue;
            }
            if !self.particles_inserted.contains(&particle_id) {
                insert_particle(&mut statements, &run_id, particles, index)?;
                self.particles_inserted.insert(particle_id);
            }
            let sample_key = (
                particle_id,
                time.seconds_since_unix_epoch(),
                time.nanosecond(),
            );
            if let Some(existing) = self.written_samples.get(&sample_key).copied() {
                if existing.event_sequence != event_sequence {
                    return Err(OutputError::InvalidInput);
                }
                match &particles.status[index] {
                    ParticleStatus::Alive if existing.terminal => {
                        return Err(OutputError::InvalidInput);
                    }
                    ParticleStatus::Alive => continue,
                    ParticleStatus::Terminated { reason } => {
                        let updated = statements
                            .particle_state_terminal
                            .execute(params![
                                run_id,
                                particle_id,
                                existing.sample_sequence,
                                event_sequence,
                                time.seconds_since_unix_epoch(),
                                time.nanosecond(),
                                particles.integration_offset_ns[index],
                                i64::try_from(particles.elapsed_age_ns[index])
                                    .map_err(|_| OutputError::Encoding("elapsed_age".into()))?,
                                particles.longitude_degrees[index],
                                particles.latitude_degrees[index],
                                particles.height_asl_m[index],
                                reason.code(),
                            ])
                            .map_err(io_err)?;
                        if updated != 1 {
                            return Err(OutputError::InvalidInput);
                        }
                        insert_termination(
                            &mut statements,
                            &run_id,
                            particle_id,
                            particles.termination[index].as_ref(),
                            time,
                            reason,
                        )?;
                        self.terminated_written.insert(particle_id);
                        self.written_samples.insert(
                            sample_key,
                            WrittenSample {
                                terminal: true,
                                ..existing
                            },
                        );
                        continue;
                    }
                }
            }
            let sample_sequence = self
                .sample_sequence_by_particle
                .entry(particle_id)
                .or_insert(0);
            let current_sample = *sample_sequence;
            *sample_sequence = sample_sequence
                .checked_add(1)
                .ok_or_else(|| OutputError::Io("sample sequence overflow".into()))?;

            let (particle_status, termination_reason) = match &particles.status[index] {
                ParticleStatus::Alive => ("alive", None),
                ParticleStatus::Terminated { reason, .. } => ("terminated", Some(reason.code())),
            };

            let (
                eastward,
                northward,
                vertical,
                pressure,
                temperature,
                wind_validity,
                wind_quality,
                pressure_validity,
                pressure_quality,
                temperature_validity,
                temperature_quality,
                provenance_id,
                five_fields,
            ) = meteorology_columns(meteorology, index, &mut self.provenance)?;

            if let Some(bundle) = self.bundle.as_mut() {
                bundle.push_sample_refs(particle_id, current_sample, &five_fields)?;
            }

            statements
                .particle_state
                .execute(params![
                    run_id,
                    particle_id,
                    current_sample,
                    event_sequence,
                    time.seconds_since_unix_epoch(),
                    time.nanosecond(),
                    particles.integration_offset_ns[index],
                    i64::try_from(particles.elapsed_age_ns[index])
                        .map_err(|_| OutputError::Encoding("elapsed_age".into()))?,
                    particles.longitude_degrees[index],
                    particles.latitude_degrees[index],
                    particles.height_asl_m[index],
                    particle_status,
                    termination_reason,
                    eastward,
                    northward,
                    vertical,
                    pressure,
                    temperature,
                    wind_validity,
                    wind_quality,
                    pressure_validity,
                    pressure_quality,
                    temperature_validity,
                    temperature_quality,
                    provenance_id,
                ])
                .map_err(io_err)?;

            self.written_samples.insert(
                sample_key,
                WrittenSample {
                    sample_sequence: current_sample,
                    event_sequence,
                    terminal: matches!(particles.status[index], ParticleStatus::Terminated { .. }),
                },
            );

            if let ParticleStatus::Terminated { reason } = &particles.status[index] {
                self.terminated_written.insert(particle_id);
                insert_termination(
                    &mut statements,
                    &run_id,
                    particle_id,
                    particles.termination[index].as_ref(),
                    time,
                    reason,
                )?;
            }
        }
        drop(statements);
        if lifecycle_event {
            self.pending_lifecycle_events = self
                .pending_lifecycle_events
                .checked_add(1)
                .ok_or_else(|| OutputError::Io("lifecycle transaction count overflow".into()))?;
            if self.pending_lifecycle_events >= MAX_LIFECYCLE_EVENTS_PER_TRANSACTION {
                self.commit_transaction()?;
            }
        } else {
            self.commit_transaction()?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), OutputError> {
        self.commit_transaction()?;
        let connection = self.connection.as_mut().ok_or(OutputError::InvalidInput)?;
        let sqlite_audit = PerformanceScope::enter(PerformanceStage::OutputFinishSqliteAudit);
        let now = system_timestamp()?;
        connection
            .execute(
                "UPDATE run SET status = CASE
                    WHEN EXISTS (
                        SELECT 1 FROM termination WHERE run_id = run.run_id AND classification = 'abnormal'
                    ) THEN 'completed_with_particle_errors'
                    ELSE 'complete'
                 END,
                 finished_seconds = ?1,
                 finished_nanosecond = ?2
                 WHERE run_id = ?3",
                params![now.seconds_since_unix_epoch(), now.nanosecond(), self.run_id],
            )
            .map_err(io_err)?;
        // Provable terminal WAL checkpoint: TRUNCATE + inspect (busy,log,checkpointed).
        let (busy, log, checkpointed): (i64, i64, i64) = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(io_err)?;
        if busy != 0 {
            return Err(OutputError::Io(format!(
                "wal_checkpoint TRUNCATE busy={busy} log={log} checkpointed={checkpointed}"
            )));
        }
        if log != 0 || checkpointed != 0 {
            // After successful TRUNCATE both should be zero (no residual WAL frames).
            return Err(OutputError::Io(format!(
                "wal_checkpoint TRUNCATE incomplete log={log} checkpointed={checkpointed}"
            )));
        }
        validate_pragmas(connection)?;
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(io_err)?;
        if integrity != "ok" {
            return Err(OutputError::Io(format!("integrity_check={integrity}")));
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
                .map_err(io_err)?;
            row_counts.insert(table.to_owned(), u64::try_from(count).unwrap_or(0));
        }
        self.row_counts = row_counts;
        drop(sqlite_audit);
        // Canonical SQL digest before closing the writer (no run UUID in formula).
        let sqlite_sql_sha = {
            let _performance = PerformanceScope::enter(PerformanceStage::OutputFinishSqlDigest);
            canonical_sql_digest(connection, &self.run_id)?
        };
        // Drop writer so readers and inspect can reopen cleanly.
        self.connection = None;
        let path = self
            .path
            .clone()
            .ok_or_else(|| OutputError::Io("sqlite path missing at finish".into()))?;
        // Standalone main DB only: non-empty -wal after close is a hard fail.
        let wal_path = PathBuf::from(format!("{}-wal", path.display()));
        let shm_path = PathBuf::from(format!("{}-shm", path.display()));
        if wal_path.exists() {
            let len = fs::metadata(&wal_path)
                .map_err(|e| OutputError::Io(e.to_string()))?
                .len();
            if len != 0 {
                return Err(OutputError::Io(format!(
                    "non-empty WAL remains after TRUNCATE checkpoint: {} bytes",
                    len
                )));
            }
            let _ = fs::remove_file(&wal_path);
        }
        let _ = fs::remove_file(&shm_path);
        let sqlite_sha = {
            let _performance = PerformanceScope::enter(PerformanceStage::OutputFinishSqliteHash);
            file_sha256(&path)?
        };
        // The legacy integer provenance table is needed only while rows are
        // being encoded. Release its cloned records before the formal bundle
        // performs its own bounded finalization and semantic re-read.
        self.provenance = ProvenanceTable::new();
        let mut bundle = self
            .bundle
            .take()
            .ok_or_else(|| OutputError::Encoding("provenance bundle builder missing".into()))?;
        let identity = {
            let _performance =
                PerformanceScope::enter(PerformanceStage::OutputFinishProvenanceBundle);
            match bundle.finalize_after_sqlite(&path, &sqlite_sha, &sqlite_sql_sha) {
                Ok(identity) => identity,
                Err(error) => {
                    // Immediate abort on bundle finalize failure (do not wait for Drop).
                    if let Err(abort_err) = bundle.abort() {
                        return Err(OutputError::Encoding(format!(
                            "bundle finalize failed: {error:?}; abort also failed: {abort_err:?}"
                        )));
                    }
                    return Err(error);
                }
            }
        };
        self.bundle_identity = Some(identity);
        Ok(())
    }

    fn abort(&mut self) -> Result<(), OutputError> {
        let rollback_result = if self.transaction_open {
            self.connection
                .as_mut()
                .ok_or(OutputError::InvalidInput)?
                .execute_batch("ROLLBACK")
                .map_err(io_err)
        } else {
            Ok(())
        };
        self.transaction_open = false;
        self.pending_lifecycle_events = 0;
        self.connection = None;
        let bundle_result = self
            .bundle
            .take()
            .map_or(Ok(()), |mut bundle| bundle.abort());
        match (rollback_result, bundle_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(rollback), Err(bundle)) => Err(OutputError::Io(format!(
                "rollback failed: {rollback:?}; bundle abort failed: {bundle:?}"
            ))),
        }
    }

    fn quarantine_forensic(&mut self) -> Result<(), OutputError> {
        if let Some(mut bundle) = self.bundle.take() {
            bundle.quarantine_forensic()?;
            return Ok(());
        }
        // Bundle already finalized into identity; quarantine on-disk final path.
        if let Some(path) = self.path.as_ref() {
            let run_dir = path
                .parent()
                .ok_or_else(|| OutputError::Io("sqlite parent missing".into()))?;
            quarantine_bundle_file(run_dir)?;
        }
        // Clear published identity so complete manifests cannot cite it after failure.
        self.bundle_identity = None;
        Ok(())
    }

    fn row_counts(&self) -> Option<BTreeMap<String, u64>> {
        if self.row_counts.is_empty() {
            None
        } else {
            Some(self.row_counts.clone())
        }
    }

    fn provenance_identity(&self) -> Option<ProvenanceBundleIdentity> {
        self.bundle_identity.clone()
    }
}

struct EventStatements<'connection> {
    output_event: CachedStatement<'connection>,
    output_event_kind: CachedStatement<'connection>,
    particle: CachedStatement<'connection>,
    particle_mass: CachedStatement<'connection>,
    particle_state: CachedStatement<'connection>,
    particle_state_terminal: CachedStatement<'connection>,
    termination: CachedStatement<'connection>,
}

impl<'connection> EventStatements<'connection> {
    fn prepare(connection: &'connection Connection) -> Result<Self, OutputError> {
        Ok(Self {
            output_event: connection
                .prepare_cached(
                    "INSERT INTO output_event (
                        run_id, event_sequence, physical_seconds, physical_nanosecond, event_kind
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(io_err)?,
            output_event_kind: connection
                .prepare_cached(
                    "UPDATE output_event
                     SET event_kind = ?3
                     WHERE run_id = ?1 AND event_sequence = ?2",
                )
                .map_err(io_err)?,
            particle: connection
                .prepare_cached(
                    "INSERT INTO particle (
                        run_id, particle_id, population_id, origin_kind, origin_event_id,
                        origin_domain_id, origin_boundary_face_id, birth_seconds, birth_nanosecond,
                        dry_air_mass_kg, sensitivity_weight
                    ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                )
                .map_err(io_err)?,
            particle_mass: connection
                .prepare_cached(
                    "INSERT INTO particle_mass (run_id, particle_id, substance_id, mass_kg)
                     VALUES (?1,?2,?3,?4)",
                )
                .map_err(io_err)?,
            particle_state: connection
                .prepare_cached(
                    "INSERT INTO particle_state (
                        run_id, particle_id, sample_sequence, event_sequence,
                        physical_seconds, physical_nanosecond, integration_offset_ns,
                        elapsed_age_ns, longitude_degrees, latitude_degrees, height_asl_m,
                        particle_status, termination_reason, eastward_wind_m_s,
                        northward_wind_m_s, geometric_vertical_velocity_m_s, air_pressure_pa,
                        air_temperature_k, wind_validity, wind_quality, pressure_validity,
                        pressure_quality, temperature_validity, temperature_quality, provenance_id
                    ) VALUES (
                        ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,
                        ?19,?20,?21,?22,?23,?24,?25
                    )",
                )
                .map_err(io_err)?,
            particle_state_terminal: connection
                .prepare_cached(
                    "UPDATE particle_state
                     SET event_sequence = ?4,
                         physical_seconds = ?5,
                         physical_nanosecond = ?6,
                         integration_offset_ns = ?7,
                         elapsed_age_ns = ?8,
                         longitude_degrees = ?9,
                         latitude_degrees = ?10,
                         height_asl_m = ?11,
                         particle_status = 'terminated',
                         termination_reason = ?12
                     WHERE run_id = ?1 AND particle_id = ?2 AND sample_sequence = ?3",
                )
                .map_err(io_err)?,
            termination: connection
                .prepare_cached(
                    "INSERT OR IGNORE INTO termination (
                        run_id, particle_id, reason, classification,
                        physical_seconds, physical_nanosecond, intersection_fraction
                    ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                )
                .map_err(io_err)?,
        })
    }
}

fn insert_particle(
    statements: &mut EventStatements<'_>,
    run_id: &str,
    particles: &ParticleBatch,
    index: usize,
) -> Result<(), OutputError> {
    let particle_id = i64::try_from(particles.id[index].0)
        .map_err(|_| OutputError::Encoding("particle_id".into()))?;
    let (origin_kind, origin_event_id, origin_domain_id, origin_boundary_face_id) = match &particles
        .origin[index]
    {
        ParticleOrigin::Release { event_id } => ("release", Some(event_id.0.as_str()), None, None),
        ParticleOrigin::DomainInitial { domain_id } => {
            ("domain_initial", None, Some(domain_id.0.as_str()), None)
        }
        ParticleOrigin::DomainBoundary {
            domain_id,
            boundary_face_id,
        } => (
            "domain_boundary",
            None,
            Some(domain_id.0.as_str()),
            Some(i64::try_from(boundary_face_id.0).map_err(|_| {
                OutputError::Encoding(format!("boundary_face_id overflow: {}", boundary_face_id.0))
            })?),
        ),
    };
    statements
        .particle
        .execute(params![
            run_id,
            particle_id,
            particles.population_id[index].0.as_str(),
            origin_kind,
            origin_event_id,
            origin_domain_id,
            origin_boundary_face_id,
            particles.birth_time[index].seconds_since_unix_epoch(),
            particles.birth_time[index].nanosecond(),
            particles.dry_air_mass_kg[index],
            particles.sensitivity_weight[index],
        ])
        .map_err(io_err)?;
    for (substance, masses) in &particles.mass.mass_kg {
        statements
            .particle_mass
            .execute(params![
                run_id,
                particle_id,
                substance.0.as_str(),
                masses[index]
            ])
            .map_err(io_err)?;
    }
    Ok(())
}

fn insert_termination(
    statements: &mut EventStatements<'_>,
    run_id: &str,
    particle_id: i64,
    termination: Option<&crate::particle::ParticleTermination>,
    event_time: Timestamp,
    reason: &crate::particle::TerminationReason,
) -> Result<(), OutputError> {
    let class = match reason.class() {
        TerminationClass::Normal => "normal",
        TerminationClass::Abnormal => "abnormal",
    };
    let termination_time = termination.map_or(event_time, |value| value.time);
    let intersection_fraction = termination.and_then(|value| value.intersection_fraction);
    statements
        .termination
        .execute(params![
            run_id,
            particle_id,
            reason.code(),
            class,
            termination_time.seconds_since_unix_epoch(),
            termination_time.nanosecond(),
            intersection_fraction,
        ])
        .map_err(io_err)?;
    Ok(())
}

#[allow(clippy::type_complexity)]
fn meteorology_columns<'a>(
    meteorology: Option<&'a QueryOutput>,
    index: usize,
    provenance_table: &mut ProvenanceTable,
) -> Result<
    (
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        &'static str,
        Option<&'static str>,
        &'static str,
        Option<&'static str>,
        &'static str,
        Option<&'static str>,
        Option<i64>,
        FiveFieldRecordRefs<'a>,
    ),
    OutputError,
> {
    let empty_five = FiveFieldRecordRefs::default();
    let Some(output) = meteorology else {
        return Ok((
            None, None, None, None, None, "missing", None, "missing", None, "missing", None, None,
            empty_five,
        ));
    };
    let row = output.row(index).ok_or(OutputError::InvalidInput)?;
    let status = row.status();
    let base_validity = status_to_validity(status);

    let mut eastward = None;
    let mut northward = None;
    let mut vertical = None;
    let mut pressure = None;
    let mut temperature = None;
    let mut q_east = None;
    let mut q_north = None;
    let mut q_vert = None;
    let mut q_pres = None;
    let mut q_temp = None;
    let mut five = FiveFieldRecordRefs::default();

    for field in output.fields() {
        let samples = field.samples();
        let value = samples.value(index);
        let quality = samples.quality().get(index).copied();
        let prov = samples
            .provenance()
            .get(index)
            .and_then(|id| output.provenance().get(*id));
        match five_field_slot(field.field()) {
            Some("eastward_wind") => {
                eastward = value;
                q_east = quality;
                five.eastward_wind = prov;
            }
            Some("northward_wind") => {
                northward = value;
                q_north = quality;
                five.northward_wind = prov;
            }
            Some("geometric_vertical_velocity") => {
                vertical = value;
                q_vert = quality;
                five.geometric_vertical_velocity = prov;
            }
            Some("air_pressure") => {
                pressure = value;
                q_pres = quality;
                five.air_pressure = prov;
            }
            Some("air_temperature") => {
                temperature = value;
                q_temp = quality;
                five.air_temperature = prov;
            }
            _ => {}
        }
    }

    let wind_quality = merge_wind_quality(q_east, q_north, q_vert)?;
    let pressure_quality = q_pres.map(quality_label);
    let temperature_quality = q_temp.map(quality_label);

    let wind_validity = if eastward.is_some() && northward.is_some() && vertical.is_some() {
        base_validity
    } else if eastward.is_none() && northward.is_none() && vertical.is_none() {
        "missing"
    } else {
        "partial"
    };
    let pressure_validity = if pressure.is_some() {
        base_validity
    } else {
        "missing"
    };
    let temperature_validity = if temperature.is_some() {
        base_validity
    } else {
        "missing"
    };

    // Legacy single provenance_id: intern first present five-field record (sorted by slot name).
    let provenance_id = {
        let mut ids = Vec::new();
        for record in [
            five.eastward_wind,
            five.northward_wind,
            five.geometric_vertical_velocity,
            five.air_pressure,
            five.air_temperature,
        ]
        .into_iter()
        .flatten()
        {
            let id = provenance_table
                .intern_ref(record)
                .map_err(|error| OutputError::Encoding(format!("provenance intern: {error:?}")))?;
            ids.push(id);
        }
        if ids.is_empty() {
            None
        } else {
            ids.sort_by_key(|id| id.0);
            ids.dedup();
            Some(i64::from(ids[0].0))
        }
    };

    Ok((
        eastward,
        northward,
        vertical,
        pressure,
        temperature,
        wind_validity,
        wind_quality,
        pressure_validity,
        pressure_quality,
        temperature_validity,
        temperature_quality,
        provenance_id,
        five,
    ))
}

const fn quality_label(q: FieldQuality) -> &'static str {
    match q {
        FieldQuality::Source => "source",
        FieldQuality::Derived => "derived",
        FieldQuality::Estimated => "estimated",
    }
}

fn merge_wind_quality(
    east: Option<FieldQuality>,
    north: Option<FieldQuality>,
    vert: Option<FieldQuality>,
) -> Result<Option<&'static str>, OutputError> {
    // Frozen merge: worst quality wins (Estimated > Derived > Source).
    // Silent overwrite with a single hard-coded label is forbidden; this is an
    // explicit lattice join over the observed component qualities.
    fn rank(q: FieldQuality) -> u8 {
        match q {
            FieldQuality::Source => 0,
            FieldQuality::Derived => 1,
            FieldQuality::Estimated => 2,
        }
    }
    let Some(worst) = [east, north, vert]
        .into_iter()
        .flatten()
        .max_by_key(|q| rank(*q))
    else {
        return Ok(None);
    };
    Ok(Some(quality_label(worst)))
}

const fn status_to_validity(status: SampleStatus) -> &'static str {
    match status {
        SampleStatus::Ok => "ok",
        SampleStatus::OutOfDomain => "outofdomain",
        SampleStatus::PolarSingularity => "polarsingularity",
        SampleStatus::BelowGround => "belowground",
        SampleStatus::SurfaceLayerUndefined => "surfacelayerundefined",
        SampleStatus::AboveAvailableTop => "aboveavailabletop",
        SampleStatus::AboveModelTop => "abovemodeltop",
        SampleStatus::InvalidVerticalColumn => "invalidverticalcolumn",
        SampleStatus::NumericalFailure => "numericalfailure",
    }
}

fn infer_event_kind(
    particles: &ParticleBatch,
    time: Timestamp,
    run_start: Option<Timestamp>,
    run_end: Option<Timestamp>,
    newly_terminated: bool,
    any_birth: bool,
) -> Result<&'static str, OutputError> {
    // Coincident event priority (single row):
    // birth > termination > start > end > interval
    if any_birth {
        return Ok("birth");
    }
    if newly_terminated {
        return Ok("termination");
    }
    if run_start == Some(time) {
        return Ok("start");
    }
    if run_end == Some(time) {
        return Ok("end");
    }
    let _ = particles;
    Ok("interval")
}

fn event_kind_priority(kind: &str) -> u8 {
    match kind {
        "birth" => 4,
        "termination" => 3,
        "start" => 2,
        "end" => 1,
        "interval" => 0,
        _ => 0,
    }
}

fn validate_pragmas(connection: &Connection) -> Result<(), OutputError> {
    let page_size: i64 = connection
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .map_err(io_err)?;
    if page_size != SQLITE_PAGE_SIZE_BYTES {
        return Err(OutputError::Io(format!(
            "page_size={page_size} expected {SQLITE_PAGE_SIZE_BYTES}"
        )));
    }
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(io_err)?;
    if !journal.eq_ignore_ascii_case("wal") {
        return Err(OutputError::Io(format!("journal_mode={journal}")));
    }
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .map_err(io_err)?;
    // NORMAL == 1
    if synchronous != 1 {
        return Err(OutputError::Io(format!("synchronous={synchronous}")));
    }
    let wal_autocheckpoint: i64 = connection
        .query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))
        .map_err(io_err)?;
    if wal_autocheckpoint != 0 {
        return Err(OutputError::Io(format!(
            "wal_autocheckpoint={wal_autocheckpoint}"
        )));
    }
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .map_err(io_err)?;
    if foreign_keys != 1 {
        return Err(OutputError::Io(format!("foreign_keys={foreign_keys}")));
    }
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(io_err)?;
    if user_version != i64::from(SQLITE_SCHEMA_VERSION) {
        return Err(OutputError::Io(format!("user_version={user_version}")));
    }
    Ok(())
}

fn canonical_sql_digest(connection: &Connection, run_id: &str) -> Result<String, OutputError> {
    let mut hasher = Sha256::new();
    // particle origin / birth / masses
    {
        let mut stmt = connection.prepare(CANONICAL_PARTICLE_SQL).map_err(io_err)?;
        hasher.update(b"TABLE particle\n");
        let rows = stmt
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, f64>(8)?,
                    row.get::<_, Option<f64>>(9)?,
                ))
            })
            .map_err(io_err)?;
        for row in rows {
            let (pid, pop, kind, ev, dom, face, bs, bn, dry, sens) = row.map_err(io_err)?;
            hasher.update(
                format!(
                    "{pid}|{pop}|{kind}|{ev:?}|{dom:?}|{face:?}|{bs}|{bn}|{dry:.17}|{sens:?}\n"
                )
                .as_bytes(),
            );
        }
    }
    {
        let mut stmt = connection
            .prepare(CANONICAL_PARTICLE_MASS_SQL)
            .map_err(io_err)?;
        hasher.update(b"TABLE particle_mass\n");
        let rows = stmt
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })
            .map_err(io_err)?;
        for row in rows {
            let (pid, sub, mass) = row.map_err(io_err)?;
            hasher.update(format!("{pid}|{sub}|{mass:.17}\n").as_bytes());
        }
    }
    {
        let mut stmt = connection
            .prepare(CANONICAL_OUTPUT_EVENT_SQL)
            .map_err(io_err)?;
        hasher.update(b"TABLE output_event\n");
        let rows = stmt
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(io_err)?;
        for row in rows {
            let (seq, s, n, kind) = row.map_err(io_err)?;
            hasher.update(format!("{seq}|{s}|{n}|{kind}\n").as_bytes());
        }
    }
    {
        let mut stmt = connection
            .prepare(CANONICAL_PARTICLE_STATE_SQL)
            .map_err(io_err)?;
        hasher.update(b"TABLE particle_state\n");
        let rows = stmt
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, f64>(7)?,
                    row.get::<_, f64>(8)?,
                    row.get::<_, f64>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<f64>>(12)?,
                    row.get::<_, Option<f64>>(13)?,
                    row.get::<_, Option<f64>>(14)?,
                    row.get::<_, Option<f64>>(15)?,
                    row.get::<_, Option<f64>>(16)?,
                    row.get::<_, String>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, String>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, String>(21)?,
                    row.get::<_, Option<String>>(22)?,
                ))
            })
            .map_err(io_err)?;
        for row in rows {
            let (
                pid,
                sample,
                event,
                sec,
                nano,
                offset,
                age,
                lon,
                lat,
                height,
                status,
                term,
                u,
                v,
                w,
                p,
                temp,
                wind_v,
                wind_q,
                pressure_v,
                pressure_q,
                temp_v,
                temp_q,
            ) = row.map_err(io_err)?;
            // provenance_id intentionally excluded: five-field identity lives in
            // provenance-bundle.json and terminal manifest provenance block.
            hasher.update(
                format!(
                    "{pid}|{sample}|{event}|{sec}|{nano}|{offset}|{age}|{lon:.17}|{lat:.17}|{height:.17}|{status}|{term:?}|{u:?}|{v:?}|{w:?}|{p:?}|{temp:?}|{wind_v}|{wind_q:?}|{pressure_v}|{pressure_q:?}|{temp_v}|{temp_q:?}\n"
                )
                .as_bytes(),
            );
        }
    }
    {
        let mut stmt = connection
            .prepare(CANONICAL_TERMINATION_SQL)
            .map_err(io_err)?;
        hasher.update(b"TABLE termination\n");
        let rows = stmt
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                ))
            })
            .map_err(io_err)?;
        for row in rows {
            let (pid, reason, class, s, n, frac) = row.map_err(io_err)?;
            hasher.update(format!("{pid}|{reason}|{class}|{s}|{n}|{frac:?}\n").as_bytes());
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn system_timestamp() -> Result<Timestamp, OutputError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| OutputError::Io(error.to_string()))?;
    Timestamp::new(
        i64::try_from(duration.as_secs()).map_err(|_| OutputError::Io("time".into()))?,
        duration.subsec_nanos(),
    )
    .map_err(|error| OutputError::Io(format!("{error:?}")))
}

fn io_err(error: impl std::fmt::Display) -> OutputError {
    OutputError::Io(error.to_string())
}

/// Builds a summary record for the run manifest.
#[must_use]
pub fn sqlite_summary(path: &Path, row_counts: BTreeMap<String, u64>) -> SqliteOutputSummary {
    SqliteOutputSummary {
        schema_version: SQLITE_SCHEMA_VERSION,
        relative_path: path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("particles.sqlite")),
        journal_mode: "WAL".into(),
        synchronous: "NORMAL".into(),
        row_counts,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use trajecta_case::document::MeteorologyReaderBackend;
    use trajecta_case::model::output::default_particle_state_output;
    use trajecta_case::model::population::{PopulationId, ReleaseEventId};

    use crate::manifest::{
        ExecutionSummary, InputIdentity, NumericalSummary, RunId, RunManifestStart,
        SoftwareIdentity,
    };
    use crate::particle::{
        ParticleId, ParticleOrigin, ParticleTermination, SubstanceMassStore, TerminationReason,
    };

    fn running_manifest() -> RunManifest {
        RunManifest::running(RunManifestStart {
            run_id: RunId("018f0000-0000-7000-8000-000000000001".into()),
            case_name: "sqlite-lifecycle-test".into(),
            started_at: Timestamp::UNIX_EPOCH,
            software: SoftwareIdentity {
                crate_versions: BTreeMap::from([("trajecta-core".into(), "0.0.0".into())]),
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
        })
    }

    fn two_particles() -> ParticleBatch {
        ParticleBatch {
            id: vec![ParticleId(1), ParticleId(2)],
            population_id: vec![PopulationId("p".into()), PopulationId("p".into())],
            origin: vec![
                ParticleOrigin::Release {
                    event_id: ReleaseEventId("e".into()),
                },
                ParticleOrigin::Release {
                    event_id: ReleaseEventId("e".into()),
                },
            ],
            birth_time: vec![Timestamp::UNIX_EPOCH; 2],
            longitude_degrees: vec![0.0, 1.0],
            latitude_degrees: vec![0.0, 1.0],
            height_asl_m: vec![1_000.0, 2_000.0],
            integration_offset_ns: vec![0; 2],
            elapsed_age_ns: vec![0; 2],
            dry_air_mass_kg: vec![0.0; 2],
            sensitivity_weight: vec![None; 2],
            status: vec![ParticleStatus::Alive; 2],
            termination: vec![None; 2],
            mass: SubstanceMassStore::default(),
        }
    }

    #[test]
    fn canonical_digest_queries_are_run_scoped_without_noncovering_time_index() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SQLITE_SCHEMA_SQL).unwrap();

        for (table, sql) in [
            ("particle", CANONICAL_PARTICLE_SQL),
            ("particle_mass", CANONICAL_PARTICLE_MASS_SQL),
            ("output_event", CANONICAL_OUTPUT_EVENT_SQL),
            ("termination", CANONICAL_TERMINATION_SQL),
        ] {
            let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
            let mut statement = connection.prepare(&explain_sql).unwrap();
            let details: Vec<String> = statement
                .query_map(["run-a"], |row| row.get(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let plan = details.join("\n");
            assert!(
                details.iter().any(|detail| detail.starts_with("SEARCH ")),
                "{table} must use a run-scoped index search; plan:\n{plan}"
            );
            assert!(
                !plan.contains("TEMP B-TREE"),
                "{table} must not materialize an ORDER BY temp tree; plan:\n{plan}"
            );
        }

        let explain_sql = format!("EXPLAIN QUERY PLAN {CANONICAL_PARTICLE_STATE_SQL}");
        let mut statement = connection.prepare(&explain_sql).unwrap();
        let details: Vec<String> = statement
            .query_map(["run-a"], |row| row.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let plan = details.join("\n");
        assert!(
            plan.contains("USING PRIMARY KEY (run_id=?)"),
            "canonical particle-state export must scan the run's primary-key range; plan:\n{plan}"
        );
        assert!(
            plan.contains("TEMP B-TREE"),
            "time ordering must sort the sequential scan instead of random non-covering index lookups; plan:\n{plan}"
        );
        assert!(
            !plan.contains("particle_state_by_time"),
            "canonical export must not use the non-covering time index; plan:\n{plan}"
        );
    }

    #[test]
    fn canonical_digest_ignores_rows_from_other_runs() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SQLITE_SCHEMA_SQL).unwrap();
        for run_id in ["run-a", "run-b"] {
            connection
                .execute(
                    "INSERT INTO run (
                        run_id, manifest_schema, case_name, status,
                        started_seconds, started_nanosecond,
                        finished_seconds, finished_nanosecond
                     ) VALUES (?1, 'test/v1', 'test', 'complete', 0, 0, 1, 0)",
                    [run_id],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO particle (
                        run_id, particle_id, population_id, origin_kind,
                        origin_event_id, birth_seconds, birth_nanosecond,
                        dry_air_mass_kg
                     ) VALUES (?1, 1, 'population', 'release', 'release', 0, 0, 1.0)",
                    [run_id],
                )
                .unwrap();
        }

        let before = canonical_sql_digest(&connection, "run-a").unwrap();
        connection
            .execute(
                "UPDATE particle SET dry_air_mass_kg = 99.0 WHERE run_id = 'run-b'",
                [],
            )
            .unwrap();
        let after = canonical_sql_digest(&connection, "run-a").unwrap();
        assert_eq!(before, after);
        assert_ne!(after, canonical_sql_digest(&connection, "run-b").unwrap());
    }

    #[test]
    fn wal_truncate_leaves_standalone_main_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("particles.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 CREATE TABLE t(x INTEGER);
                 INSERT INTO t VALUES (1),(2),(3);",
            )
            .unwrap();
            let (busy, log, ckpt): (i64, i64, i64) = conn
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap();
            assert_eq!(busy, 0);
            assert_eq!(log, 0);
            assert_eq!(ckpt, 0);
        }
        let wal = PathBuf::from(format!("{}-wal", path.display()));
        if wal.exists() {
            assert_eq!(fs::metadata(&wal).unwrap().len(), 0);
        }
        // reopen main only
        let conn = Connection::open(&path).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
    }

    #[test]
    fn wal_checkpoint_busy_with_active_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("particles.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE t(x INTEGER);
                 INSERT INTO t VALUES (1);",
            )
            .unwrap();
        // Active read transaction on a second connection.
        let reader = Connection::open(&path).unwrap();
        reader.execute_batch("BEGIN; SELECT * FROM t;").unwrap();
        // Writer makes more WAL frames.
        writer.execute("INSERT INTO t VALUES (2)", []).unwrap();
        let (busy, log, ckpt): (i64, i64, i64) = writer
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        // With a live reader, busy is typically non-zero OR residual log remains.
        assert!(
            busy != 0 || log != 0,
            "expected busy or residual wal frames busy={busy} log={log} ckpt={ckpt}"
        );
        reader.execute_batch("COMMIT;").unwrap();
    }

    #[test]
    fn exact_termination_row_matches_state_time_fraction_and_lifecycle_subset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("particles.sqlite");
        let mut sink = ParticleStateSqliteSink::with_time_bounds(
            Timestamp::UNIX_EPOCH,
            Timestamp::new(1, 0).unwrap(),
        );
        sink.begin(&path, &running_manifest()).unwrap();
        let particles = two_particles();
        sink.write_event(Timestamp::UNIX_EPOCH, &particles, None)
            .unwrap();

        let termination_time = Timestamp::new(0, 250_000_000).unwrap();
        let mut terminated = particles.select_indices(&[1]).unwrap();
        let mut state = terminated.state(0).unwrap();
        state.integration_offset_ns = 250_000_000;
        state.elapsed_age_ns = 250_000_000;
        state.status = ParticleStatus::Terminated {
            reason: TerminationReason::OutsideDomain,
        };
        state.termination = Some(ParticleTermination {
            time: termination_time,
            intersection_fraction: Some(0.25),
        });
        terminated.set_state(0, state).unwrap();
        sink.write_event(termination_time, &terminated, None)
            .unwrap();

        let connection = sink.connection.as_ref().unwrap();
        let event_sequence: i64 = connection
            .query_row(
                "SELECT event_sequence FROM output_event WHERE event_kind = 'termination'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let state_row: (i64, i64, i64, i64) = connection
            .query_row(
                "SELECT particle_id, physical_seconds, physical_nanosecond, integration_offset_ns
                 FROM particle_state WHERE event_sequence = ?1",
                [event_sequence],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(state_row, (2, 0, 250_000_000, 250_000_000));
        let state_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM particle_state WHERE event_sequence = ?1",
                [event_sequence],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            state_count, 1,
            "lifecycle event wrote an unrelated particle"
        );
        let termination_row: (i64, i64, i64, f64) = connection
            .query_row(
                "SELECT particle_id, physical_seconds, physical_nanosecond, intersection_fraction
                 FROM termination",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(termination_row, (2, 0, 250_000_000, 0.25));
    }

    #[test]
    fn coincident_interval_and_termination_upgrade_one_event_and_one_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("particles.sqlite");
        let mut sink = ParticleStateSqliteSink::with_time_bounds(
            Timestamp::UNIX_EPOCH,
            Timestamp::new(2, 0).unwrap(),
        );
        sink.begin(&path, &running_manifest()).unwrap();
        let time = Timestamp::new(1, 0).unwrap();
        let particles = two_particles().select_indices(&[0]).unwrap();
        sink.write_event(time, &particles, None).unwrap();

        let mut terminated = particles;
        let mut state = terminated.state(0).unwrap();
        state.integration_offset_ns = 1_000_000_000;
        state.elapsed_age_ns = 1_000_000_000;
        state.status = ParticleStatus::Terminated {
            reason: TerminationReason::ModelTop,
        };
        state.termination = Some(ParticleTermination {
            time,
            intersection_fraction: Some(0.0),
        });
        terminated.set_state(0, state).unwrap();
        sink.write_event(time, &terminated, None).unwrap();

        let connection = sink.connection.as_ref().unwrap();
        let event: (i64, String) = connection
            .query_row("SELECT COUNT(*), event_kind FROM output_event", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(event, (1, "termination".into()));
        let state_row: (i64, String, Option<String>, i64) = connection
            .query_row(
                "SELECT COUNT(*), particle_status, termination_reason, sample_sequence
                 FROM particle_state",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            state_row,
            (1, "terminated".into(), Some("model_top".into()), 0)
        );
        let termination_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM termination", [], |row| row.get(0))
            .unwrap();
        assert_eq!(termination_count, 1);
    }

    #[test]
    fn coincident_birth_retains_priority_when_state_becomes_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("particles.sqlite");
        let mut sink = ParticleStateSqliteSink::with_time_bounds(
            Timestamp::UNIX_EPOCH,
            Timestamp::new(1, 0).unwrap(),
        );
        sink.begin(&path, &running_manifest()).unwrap();
        let particles = two_particles().select_indices(&[0]).unwrap();
        sink.write_event(Timestamp::UNIX_EPOCH, &particles, None)
            .unwrap();

        let mut terminated = particles;
        let mut state = terminated.state(0).unwrap();
        state.status = ParticleStatus::Terminated {
            reason: TerminationReason::ModelTop,
        };
        state.termination = Some(ParticleTermination {
            time: Timestamp::UNIX_EPOCH,
            intersection_fraction: Some(0.0),
        });
        terminated.set_state(0, state).unwrap();
        sink.write_event(Timestamp::UNIX_EPOCH, &terminated, None)
            .unwrap();

        let connection = sink.connection.as_ref().unwrap();
        let event_kind: String = connection
            .query_row("SELECT event_kind FROM output_event", [], |row| row.get(0))
            .unwrap();
        assert_eq!(event_kind, "birth");
        let state_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM particle_state", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state_count, 1);
    }

    #[test]
    fn first_terminal_write_rejects_event_time_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("particles.sqlite");
        let mut sink = ParticleStateSqliteSink::with_time_bounds(
            Timestamp::UNIX_EPOCH,
            Timestamp::new(1, 0).unwrap(),
        );
        sink.begin(&path, &running_manifest()).unwrap();
        let particles = two_particles();
        sink.write_event(Timestamp::UNIX_EPOCH, &particles, None)
            .unwrap();
        let mut terminated = particles.select_indices(&[0]).unwrap();
        let mut state = terminated.state(0).unwrap();
        state.status = ParticleStatus::Terminated {
            reason: TerminationReason::OutsideDomain,
        };
        state.termination = Some(ParticleTermination {
            time: Timestamp::new(0, 250_000_000).unwrap(),
            intersection_fraction: Some(0.25),
        });
        terminated.set_state(0, state).unwrap();
        assert_eq!(
            sink.write_event(Timestamp::new(0, 500_000_000).unwrap(), &terminated, None),
            Err(OutputError::InvalidInput)
        );
    }
}
