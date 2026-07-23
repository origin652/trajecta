//! # Contract: particle_state_sqlite/v1 typed sink
//!
//! Implements the public SQLite schema with WAL, NORMAL sync, foreign keys,
//! per-event transactions, and live-read inspection.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, Transaction, params};
use sha2::{Digest, Sha256};
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::FieldQuality;
use trajecta_met::provenance::ProvenanceTable;
use trajecta_met::query::output::{QueryOutput, SampleStatus};

use crate::manifest::ProvenanceBundleIdentity;
use crate::manifest::{RunManifest, SqliteOutputSummary};
use crate::output::provenance_bundle::{
    FiveFieldRecords, ProvenanceBundleBuilder, file_sha256, five_field_slot, quarantine_bundle_file,
};
use crate::output::{OutputError, ParticleStateSink};
use crate::particle::{ParticleBatch, ParticleOrigin, ParticleStatus, TerminationClass};
use crate::science::SQLITE_SCHEMA_VERSION;

/// Embedded public schema used to initialize every particle-state database.
pub const SQLITE_SCHEMA_SQL: &str = include_str!("../../../../testdata/M4_SQLITE_SCHEMA.v1.sql");

/// Production SQLite particle-state sink.
#[derive(Default)]
pub struct ParticleStateSqliteSink {
    path: Option<PathBuf>,
    connection: Option<Connection>,
    run_id: String,
    event_sequence: i64,
    sample_sequence_by_particle: BTreeMap<i64, i64>,
    seen_event_keys: BTreeMap<(i64, u32, String), i64>,
    particles_inserted: BTreeMap<i64, bool>,
    /// Particles that already received a terminal sample row.
    terminated_written: BTreeMap<i64, bool>,
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
        let uri = format!("file:{}?mode=ro", path.display());
        Connection::open_with_flags(
            uri,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
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
        let canonical_sql = canonical_sql_digest(&connection)?;
        Ok(SqliteInspection {
            path: path.to_path_buf(),
            size_bytes,
            sha256,
            integrity,
            row_counts,
            canonical_sql_sha256: canonical_sql,
        })
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
        self.run_id = manifest.run_id.0.clone();
        self.event_sequence = 0;
        self.sample_sequence_by_particle.clear();
        self.seen_event_keys.clear();
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
        let connection = self.connection.as_mut().ok_or(OutputError::InvalidInput)?;
        let tx = connection.unchecked_transaction().map_err(io_err)?;
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
            let state = particles
                .state(index)
                .map_err(|_| OutputError::InvalidInput)?;
            if state.birth_time == time {
                any_birth = true;
            }
            if matches!(state.status, ParticleStatus::Terminated { .. }) {
                let pid = i64::try_from(state.id.0)
                    .map_err(|_| OutputError::Encoding("particle_id".into()))?;
                if !self.terminated_written.contains_key(&pid) {
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
        let event_key = (
            time.seconds_since_unix_epoch(),
            time.nanosecond(),
            event_kind.to_owned(),
        );
        let event_sequence = if let Some(existing) = self.seen_event_keys.get(&event_key) {
            *existing
        } else {
            let sequence = self.event_sequence;
            tx.execute(
                "INSERT INTO output_event (
                    run_id, event_sequence, physical_seconds, physical_nanosecond, event_kind
                ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    run_id,
                    sequence,
                    time.seconds_since_unix_epoch(),
                    time.nanosecond(),
                    event_kind,
                ],
            )
            .map_err(io_err)?;
            self.seen_event_keys.insert(event_key, sequence);
            self.event_sequence = self
                .event_sequence
                .checked_add(1)
                .ok_or_else(|| OutputError::Io("event sequence overflow".into()))?;
            sequence
        };

        let mut indices: Vec<usize> = (0..count).collect();
        if self.reverse_particle_scan {
            indices.reverse();
        }
        for index in indices {
            let state = particles
                .state(index)
                .map_err(|_| OutputError::InvalidInput)?;
            let particle_id = i64::try_from(state.id.0)
                .map_err(|_| OutputError::Encoding("particle_id outside i64".into()))?;
            if matches!(state.status, ParticleStatus::Terminated { .. })
                && self.terminated_written.contains_key(&particle_id)
            {
                continue;
            }
            if let std::collections::btree_map::Entry::Vacant(entry) =
                self.particles_inserted.entry(particle_id)
            {
                insert_particle(&tx, &run_id, &state)?;
                entry.insert(true);
            }
            let sample_sequence = self
                .sample_sequence_by_particle
                .entry(particle_id)
                .or_insert(0);
            let current_sample = *sample_sequence;
            *sample_sequence = sample_sequence
                .checked_add(1)
                .ok_or_else(|| OutputError::Io("sample sequence overflow".into()))?;

            let (particle_status, termination_reason) = match &state.status {
                ParticleStatus::Alive => ("alive".to_owned(), None),
                ParticleStatus::Terminated { reason, .. } => {
                    ("terminated".to_owned(), Some(reason.code()))
                }
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
                bundle.push_sample(particle_id, current_sample, &five_fields)?;
            }

            tx.execute(
                "INSERT INTO particle_state (
                    run_id, particle_id, sample_sequence, event_sequence,
                    physical_seconds, physical_nanosecond, integration_offset_ns, elapsed_age_ns,
                    longitude_degrees, latitude_degrees, height_asl_m,
                    particle_status, termination_reason,
                    eastward_wind_m_s, northward_wind_m_s, geometric_vertical_velocity_m_s,
                    air_pressure_pa, air_temperature_k,
                    wind_validity, wind_quality, pressure_validity, pressure_quality,
                    temperature_validity, temperature_quality, provenance_id
                ) VALUES (
                    ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25
                )",
                params![
                    run_id,
                    particle_id,
                    current_sample,
                    event_sequence,
                    time.seconds_since_unix_epoch(),
                    time.nanosecond(),
                    state.integration_offset_ns,
                    i64::try_from(state.elapsed_age_ns)
                        .map_err(|_| OutputError::Encoding("elapsed_age".into()))?,
                    state.longitude_degrees,
                    state.latitude_degrees,
                    state.height_asl_m,
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
                ],
            )
            .map_err(io_err)?;

            if let ParticleStatus::Terminated { reason } = &state.status {
                self.terminated_written.insert(particle_id, true);
                let class = match reason.class() {
                    TerminationClass::Normal => "normal",
                    TerminationClass::Abnormal => "abnormal",
                };
                tx.execute(
                    "INSERT OR IGNORE INTO termination (
                        run_id, particle_id, reason, classification,
                        physical_seconds, physical_nanosecond, intersection_fraction
                    ) VALUES (?1,?2,?3,?4,?5,?6,NULL)",
                    params![
                        run_id,
                        particle_id,
                        reason.code(),
                        class,
                        time.seconds_since_unix_epoch(),
                        time.nanosecond(),
                    ],
                )
                .map_err(io_err)?;
            }
        }
        tx.commit().map_err(io_err)?;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), OutputError> {
        let connection = self.connection.as_mut().ok_or(OutputError::InvalidInput)?;
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
        // Canonical SQL digest before closing the writer (no run UUID in formula).
        let sqlite_sql_sha = canonical_sql_digest(connection)?;
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
        let sqlite_sha = file_sha256(&path)?;
        let mut bundle = self
            .bundle
            .take()
            .ok_or_else(|| OutputError::Encoding("provenance bundle builder missing".into()))?;
        let identity = match bundle.finalize_after_sqlite(&path, &sqlite_sha, &sqlite_sql_sha) {
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
        };
        self.bundle_identity = Some(identity);
        Ok(())
    }

    fn abort(&mut self) -> Result<(), OutputError> {
        self.connection = None;
        if let Some(mut bundle) = self.bundle.take() {
            bundle.abort()?;
        }
        Ok(())
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

fn insert_particle(
    tx: &Transaction<'_>,
    run_id: &str,
    state: &crate::particle::ParticleState,
) -> Result<(), OutputError> {
    let particle_id =
        i64::try_from(state.id.0).map_err(|_| OutputError::Encoding("particle_id".into()))?;
    let (origin_kind, origin_event_id, origin_domain_id, origin_boundary_face_id) = match &state
        .origin
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
    tx.execute(
        "INSERT INTO particle (
            run_id, particle_id, population_id, origin_kind, origin_event_id,
            origin_domain_id, origin_boundary_face_id, birth_seconds, birth_nanosecond,
            dry_air_mass_kg, sensitivity_weight
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![
            run_id,
            particle_id,
            state.population_id.0,
            origin_kind,
            origin_event_id,
            origin_domain_id,
            origin_boundary_face_id,
            state.birth_time.seconds_since_unix_epoch(),
            state.birth_time.nanosecond(),
            state.dry_air_mass_kg,
            state.sensitivity_weight,
        ],
    )
    .map_err(io_err)?;
    for (substance, mass) in &state.mass_kg {
        tx.execute(
            "INSERT INTO particle_mass (run_id, particle_id, substance_id, mass_kg)
             VALUES (?1,?2,?3,?4)",
            params![run_id, particle_id, substance.0, mass],
        )
        .map_err(io_err)?;
    }
    Ok(())
}

#[allow(clippy::type_complexity)]
fn meteorology_columns(
    meteorology: Option<&QueryOutput>,
    index: usize,
    provenance_table: &mut ProvenanceTable,
) -> Result<
    (
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        String,
        Option<String>,
        String,
        Option<String>,
        String,
        Option<String>,
        Option<i64>,
        FiveFieldRecords,
    ),
    OutputError,
> {
    let empty_five = FiveFieldRecords::default();
    let Some(output) = meteorology else {
        return Ok((
            None,
            None,
            None,
            None,
            None,
            "missing".into(),
            None,
            "missing".into(),
            None,
            "missing".into(),
            None,
            None,
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
    let mut five = FiveFieldRecords::default();

    for field in output.fields() {
        let key = field.field().clone();
        let samples = field.samples();
        let value = samples.value(index);
        let quality = samples.quality().get(index).copied();
        let prov = row.provenance_record(&key).cloned();
        match five_field_slot(&key) {
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
        base_validity.clone()
    } else if eastward.is_none() && northward.is_none() && vertical.is_none() {
        "missing".into()
    } else {
        "partial".into()
    };
    let pressure_validity = if pressure.is_some() {
        base_validity.clone()
    } else {
        "missing".into()
    };
    let temperature_validity = if temperature.is_some() {
        base_validity
    } else {
        "missing".into()
    };

    // Legacy single provenance_id: intern first present five-field record (sorted by slot name).
    let provenance_id = {
        let mut ids = Vec::new();
        for record in [
            five.eastward_wind.clone(),
            five.northward_wind.clone(),
            five.geometric_vertical_velocity.clone(),
            five.air_pressure.clone(),
            five.air_temperature.clone(),
        ]
        .into_iter()
        .flatten()
        {
            let id = provenance_table
                .intern(record)
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

fn quality_label(q: FieldQuality) -> String {
    match q {
        FieldQuality::Source => "source".into(),
        FieldQuality::Derived => "derived".into(),
        FieldQuality::Estimated => "estimated".into(),
    }
}

fn merge_wind_quality(
    east: Option<FieldQuality>,
    north: Option<FieldQuality>,
    vert: Option<FieldQuality>,
) -> Result<Option<String>, OutputError> {
    let present: Vec<FieldQuality> = [east, north, vert].into_iter().flatten().collect();
    if present.is_empty() {
        return Ok(None);
    }
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
    let Some(worst) = present.into_iter().max_by_key(|q| rank(*q)) else {
        return Ok(None);
    };
    Ok(Some(quality_label(worst)))
}

fn status_to_validity(status: SampleStatus) -> String {
    match status {
        SampleStatus::Ok => "ok".into(),
        other => format!("{other:?}").to_ascii_lowercase(),
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

fn validate_pragmas(connection: &Connection) -> Result<(), OutputError> {
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

fn canonical_sql_digest(connection: &Connection) -> Result<String, OutputError> {
    let mut hasher = Sha256::new();
    // particle origin / birth / masses
    {
        let mut stmt = connection
            .prepare(
                "SELECT particle_id, population_id, origin_kind,
                        origin_event_id, origin_domain_id, origin_boundary_face_id,
                        birth_seconds, birth_nanosecond, dry_air_mass_kg, sensitivity_weight
                 FROM particle
                 ORDER BY particle_id",
            )
            .map_err(io_err)?;
        hasher.update(b"TABLE particle\n");
        let rows = stmt
            .query_map([], |row| {
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
            .prepare(
                "SELECT particle_id, substance_id, mass_kg
                 FROM particle_mass
                 ORDER BY particle_id, substance_id",
            )
            .map_err(io_err)?;
        hasher.update(b"TABLE particle_mass\n");
        let rows = stmt
            .query_map([], |row| {
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
            .prepare(
                "SELECT event_sequence, physical_seconds, physical_nanosecond, event_kind
                 FROM output_event
                 ORDER BY event_sequence",
            )
            .map_err(io_err)?;
        hasher.update(b"TABLE output_event\n");
        let rows = stmt
            .query_map([], |row| {
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
            .prepare(
                "SELECT particle_id, sample_sequence, event_sequence,
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
                 ORDER BY physical_seconds, physical_nanosecond, particle_id, sample_sequence",
            )
            .map_err(io_err)?;
        hasher.update(b"TABLE particle_state\n");
        let rows = stmt
            .query_map([], |row| {
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
            .prepare(
                "SELECT particle_id, reason, classification,
                        physical_seconds, physical_nanosecond, intersection_fraction
                 FROM termination
                 ORDER BY particle_id",
            )
            .map_err(io_err)?;
        hasher.update(b"TABLE termination\n");
        let rows = stmt
            .query_map([], |row| {
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
}
