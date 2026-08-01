//! # Contract: trajecta.provenance-bundle/v1
//!
//! Formal, versioned, content-addressed five-field provenance artifact.
//! Replaces the non-acceptance `provenance-table.json` engineering dump.
//! See `docs/engineering/TRAJECTA_M4_PROVENANCE_BUNDLE_V1.md`.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use trajecta_met::field::{CanonicalField, FieldKey, FieldQuality};
use trajecta_met::performance::{PerformanceScope, PerformanceStage};
use trajecta_met::provenance::{ProvenanceRecord, TransformRecord};

use crate::output::OutputError;
use crate::science::{
    PROVENANCE_BUNDLE_FILE_NAME, PROVENANCE_BUNDLE_SCHEMA_ID, PROVENANCE_RECORD_HASH_ALGORITHM,
    SQLITE_SCHEMA_VERSION,
};

/// Explicit in-memory cap for unique provenance records (samples spool to disk).
///
/// A five-field set can introduce up to five distinct records. The previous
/// 64k record cap contradicted the independently allowed 64k field sets and
/// failed the frozen 100k matrix before the field-set cap was approached.
pub const MAX_UNIQUE_RECORDS: usize = 128_000;
/// Cap on unique five-field sets retained in memory.
pub const MAX_UNIQUE_FIELD_SETS: usize = 64_000;
/// Production sample-spool sort chunk (lines per sorted run).
pub const DEFAULT_SAMPLE_CHUNK_LINES: usize = 4_096;
/// Production k-way merge fan-in upper bound (open files).
pub const DEFAULT_MERGE_FAN_IN: usize = 32;
/// Test-only reduced chunk to force multi-round merges.
pub const TEST_SAMPLE_CHUNK_LINES: usize = 3;
/// Test-only reduced fan-in.
pub const TEST_MERGE_FAN_IN: usize = 2;
/// Normalized provenance content digest algorithm id.
pub const PROVENANCE_CONTENT_DIGEST_ID: &str = "trajecta.provenance-content/v1";
/// Canonical output digest algorithm id.
pub const CANONICAL_OUTPUT_DIGEST_ID: &str = "trajecta.canonical-output/v1";

const PROVENANCE_BUNDLE_WRITE_BUFFER_BYTES: usize = 128 * 1024;

const LOCKSTEP_PARTICLE_STATE_SQL: &str = "SELECT particle_id, sample_sequence,
            eastward_wind_m_s IS NOT NULL,
            northward_wind_m_s IS NOT NULL,
            geometric_vertical_velocity_m_s IS NOT NULL,
            air_pressure_pa IS NOT NULL,
            air_temperature_k IS NOT NULL,
            wind_quality, pressure_quality, temperature_quality
     FROM particle_state
     WHERE run_id = ?1
     ORDER BY particle_id, sample_sequence";

const SLOT_EASTWARD: &str = "eastward_wind";
const SLOT_NORTHWARD: &str = "northward_wind";
const SLOT_VERTICAL: &str = "geometric_vertical_velocity";
const SLOT_PRESSURE: &str = "air_pressure";
const SLOT_TEMPERATURE: &str = "air_temperature";

pub use crate::manifest::ProvenanceBundleIdentity;

/// Stable JSON token for a field key (snake_case canonical or `{namespace,name}`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldKeyToken {
    /// Built-in canonical field as snake_case string.
    Canonical(String),
    /// Extension field object.
    Extension {
        /// Namespace.
        namespace: String,
        /// Name within namespace.
        name: String,
    },
}

impl FieldKeyToken {
    /// Converts a runtime [`FieldKey`] into the stable bundle token.
    pub fn from_field_key(key: &FieldKey) -> Result<Self, OutputError> {
        match key {
            FieldKey::Canonical(field) => {
                let value = serde_json::to_value(field).map_err(|error| {
                    OutputError::Encoding(format!("canonical field token: {error}"))
                })?;
                let Some(name) = value.as_str() else {
                    return Err(OutputError::Encoding(
                        "canonical FieldKey did not serialize to snake_case string".into(),
                    ));
                };
                Ok(Self::Canonical(name.to_owned()))
            }
            FieldKey::Extension(ext) => Ok(Self::Extension {
                namespace: ext.namespace.clone(),
                name: ext.name.clone(),
            }),
        }
    }

    fn expected_slot(&self) -> Option<&'static str> {
        match self {
            Self::Canonical(name) => match name.as_str() {
                SLOT_EASTWARD => Some(SLOT_EASTWARD),
                SLOT_NORTHWARD => Some(SLOT_NORTHWARD),
                SLOT_VERTICAL => Some(SLOT_VERTICAL),
                SLOT_PRESSURE => Some(SLOT_PRESSURE),
                SLOT_TEMPERATURE => Some(SLOT_TEMPERATURE),
                _ => None,
            },
            Self::Extension { .. } => None,
        }
    }
}

/// One transform step in bundle JSON form.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleTransform {
    /// Stable operation id.
    pub operation: String,
    /// Parameters sorted by `(name, value)` at emit time.
    pub parameters: Vec<BundleParameter>,
}

/// Name/value transform parameter (values always strings).
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleParameter {
    /// Parameter name.
    pub name: String,
    /// Parameter value as exact string.
    pub value: String,
}

/// Provenance record body (hash input, without outer sha wrapper).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleRecordBody {
    /// Field token.
    pub field: FieldKeyToken,
    /// Quality label.
    pub quality: String,
    /// Locked source identities (scientific order preserved).
    pub sources: Vec<String>,
    /// Transform chain (execution order preserved).
    pub transforms: Vec<BundleTransform>,
    /// Optional fallback reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    /// Exact profile content SHA-256.
    pub profile_sha256: String,
}

/// Content-addressed record entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleRecordEntry {
    /// SHA-256 of RFC 8785 canonical JSON of `record`.
    pub sha256: String,
    /// Record body.
    pub record: BundleRecordBody,
}

/// Five-field set body (hash input).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleFieldSetBody {
    /// Eastward wind record SHA or null.
    pub eastward_wind: Option<String>,
    /// Northward wind record SHA or null.
    pub northward_wind: Option<String>,
    /// Geometric vertical velocity record SHA or null.
    pub geometric_vertical_velocity: Option<String>,
    /// Air pressure record SHA or null.
    pub air_pressure: Option<String>,
    /// Air temperature record SHA or null.
    pub air_temperature: Option<String>,
}

/// Content-addressed field-set entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleFieldSetEntry {
    /// SHA-256 of RFC 8785 canonical JSON of `fields`.
    pub sha256: String,
    /// Five slots.
    pub fields: BundleFieldSetBody,
}

/// One sample → field-set assignment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSampleAssignment {
    /// Particle id (i64 domain of SQLite).
    pub particle_id: i64,
    /// Per-particle sample sequence.
    pub sample_sequence: i64,
    /// Referenced field-set SHA.
    pub field_set_sha256: String,
}

/// Full on-disk bundle document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceBundleDocument {
    /// Schema id.
    pub schema_version: String,
    /// Run UUID string.
    pub run_id: String,
    /// SQLite artifact pointer.
    pub sqlite: BundleSqliteRef,
    /// Hash algorithm id.
    pub record_hash_algorithm: String,
    /// Records sorted by sha256.
    pub records: Vec<BundleRecordEntry>,
    /// Field sets sorted by sha256.
    pub field_sets: Vec<BundleFieldSetEntry>,
    /// Samples sorted by (particle_id, sample_sequence).
    pub samples: Vec<BundleSampleAssignment>,
}

/// SQLite file reference inside the bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleSqliteRef {
    /// Relative path.
    pub relative_path: String,
    /// Schema user_version.
    pub schema_version: u32,
    /// Exact file SHA-256.
    pub sha256: String,
}

/// Five optional runtime provenance records for one sample.
#[derive(Clone, Debug, Default)]
pub struct FiveFieldRecords {
    /// Eastward wind.
    pub eastward_wind: Option<ProvenanceRecord>,
    /// Northward wind.
    pub northward_wind: Option<ProvenanceRecord>,
    /// Geometric vertical velocity.
    pub geometric_vertical_velocity: Option<ProvenanceRecord>,
    /// Air pressure.
    pub air_pressure: Option<ProvenanceRecord>,
    /// Air temperature.
    pub air_temperature: Option<ProvenanceRecord>,
}

/// Borrowed five-field provenance records for allocation-free hot-path interning.
#[derive(Clone, Copy, Debug, Default)]
pub struct FiveFieldRecordRefs<'a> {
    /// Eastward wind.
    pub eastward_wind: Option<&'a ProvenanceRecord>,
    /// Northward wind.
    pub northward_wind: Option<&'a ProvenanceRecord>,
    /// Geometric vertical velocity.
    pub geometric_vertical_velocity: Option<&'a ProvenanceRecord>,
    /// Air pressure.
    pub air_pressure: Option<&'a ProvenanceRecord>,
    /// Air temperature.
    pub air_temperature: Option<&'a ProvenanceRecord>,
}

/// Streaming builder: records/field-sets interned in memory; samples spooled.
pub struct ProvenanceBundleBuilder {
    run_id: String,
    run_dir: PathBuf,
    records_by_sha: BTreeMap<String, BundleRecordEntry>,
    /// Runtime-record memoization avoids repeated RFC 8785 conversion and hashing.
    record_sha_by_runtime: BTreeMap<ProvenanceRecord, String>,
    /// Detect sha collision against different canonical bytes.
    record_canonical_by_sha: BTreeMap<String, String>,
    field_sets_by_sha: BTreeMap<String, BundleFieldSetEntry>,
    /// Five-slot memoization avoids hashing the same field set for every particle row.
    field_set_sha_by_slots: BTreeMap<[Option<String>; 5], String>,
    field_set_canonical_by_sha: BTreeMap<String, String>,
    sample_spool_path: PathBuf,
    sample_spool: Option<BufWriter<File>>,
    sample_count: u64,
    identity: Option<ProvenanceBundleIdentity>,
    /// Bounded external-sort chunk size (lines).
    chunk_lines: usize,
    /// k-way merge fan-in upper bound.
    merge_fan_in: usize,
    /// Temp run files created during external sort (for RAII cleanup).
    temp_paths: Vec<PathBuf>,
    /// Bundle tmp path if present.
    bundle_tmp_path: PathBuf,
    /// Final on-disk bundle path.
    bundle_final_path: PathBuf,
    /// True after successful finalize (Drop keeps final artifact).
    finalized_ok: bool,
    /// Optional fault injection for lifecycle tests.
    fault: Option<BundleFaultInject>,
    /// Last computed normalized content digest (tests / diagnostics).
    pub last_content_digest: Option<String>,
    /// Last computed canonical-output digest when available (tests / diagnostics).
    pub last_canonical_output_digest: Option<String>,
}

/// Fault injection points for abort/cleanup tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleFaultInject {
    /// Fail after semantic validation, before tmp write.
    BeforeTmpWrite,
    /// Fail after tmp sync, before atomic rename.
    BeforeRename,
    /// Fail after rename, before identity return (forensic final left behind).
    AfterRename,
}

impl ProvenanceBundleBuilder {
    /// Starts a builder rooted at the run directory (same dir as particles.sqlite).
    pub fn begin(run_dir: &Path, run_id: &str) -> Result<Self, OutputError> {
        Self::begin_with_limits(
            run_dir,
            run_id,
            DEFAULT_SAMPLE_CHUNK_LINES,
            DEFAULT_MERGE_FAN_IN,
        )
    }

    /// Test/production entry with explicit external-sort limits.
    pub fn begin_with_limits(
        run_dir: &Path,
        run_id: &str,
        chunk_lines: usize,
        merge_fan_in: usize,
    ) -> Result<Self, OutputError> {
        if chunk_lines == 0 || merge_fan_in == 0 || merge_fan_in > DEFAULT_MERGE_FAN_IN {
            return Err(OutputError::Encoding(format!(
                "invalid external-sort limits chunk={chunk_lines} fan_in={merge_fan_in}"
            )));
        }
        fs::create_dir_all(run_dir).map_err(|error| OutputError::Io(error.to_string()))?;
        let bundle_path = run_dir.join(PROVENANCE_BUNDLE_FILE_NAME);
        let tmp_path = run_dir.join(format!("{PROVENANCE_BUNDLE_FILE_NAME}.tmp"));
        let spool_path = run_dir.join(format!("{PROVENANCE_BUNDLE_FILE_NAME}.samples.spool"));
        let runs_glob_prefix = run_dir.join(format!("{PROVENANCE_BUNDLE_FILE_NAME}.samples.run."));
        let stale_sidecar = run_dir.join("provenance-table.json");
        for path in [&bundle_path, &tmp_path, &spool_path, &stale_sidecar] {
            if path.exists() {
                fs::remove_file(path).map_err(|error| OutputError::Io(error.to_string()))?;
            }
        }
        // Clear leftover external-sort runs from a prior aborted finalize.
        if let Ok(rd) = fs::read_dir(run_dir) {
            for ent in rd.flatten() {
                let p = ent.path();
                if p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.starts_with("provenance-bundle.json.samples.run."))
                {
                    let _ = fs::remove_file(&p);
                }
            }
        }
        let _ = runs_glob_prefix;
        let spool = BufWriter::new(
            File::create(&spool_path).map_err(|error| OutputError::Io(error.to_string()))?,
        );
        Ok(Self {
            run_id: run_id.to_owned(),
            run_dir: run_dir.to_path_buf(),
            records_by_sha: BTreeMap::new(),
            record_sha_by_runtime: BTreeMap::new(),
            record_canonical_by_sha: BTreeMap::new(),
            field_sets_by_sha: BTreeMap::new(),
            field_set_sha_by_slots: BTreeMap::new(),
            field_set_canonical_by_sha: BTreeMap::new(),
            sample_spool_path: spool_path,
            sample_spool: Some(spool),
            sample_count: 0,
            identity: None,
            chunk_lines,
            merge_fan_in,
            temp_paths: Vec::new(),
            bundle_tmp_path: tmp_path,
            bundle_final_path: bundle_path,
            finalized_ok: false,
            fault: None,
            last_content_digest: None,
            last_canonical_output_digest: None,
        })
    }

    /// Inject a fault for lifecycle tests.
    pub fn inject_fault(&mut self, fault: BundleFaultInject) {
        self.fault = Some(fault);
    }

    /// Explicit abort: drop spool/runs/tmp; never leaves a complete bundle contract.
    ///
    /// Quarantine failures are returned — callers must not swallow with `let _ =`.
    pub fn abort(&mut self) -> Result<(), OutputError> {
        self.sample_spool = None;
        self.cleanup_ephemeral();
        // If a final bundle was written without a successful identity handoff, quarantine it.
        if self.bundle_final_path.exists() && !self.finalized_ok {
            quarantine_path(&self.bundle_final_path, &self.run_dir)?;
        }
        self.identity = None;
        self.finalized_ok = false;
        Ok(())
    }

    /// Quarantine a formal final bundle after terminal manifest failure.
    ///
    /// Errors are not ignored: target conflicts get a unique suffix. Never leaves
    /// a seemingly formal `provenance-bundle.json` behind.
    pub fn quarantine_forensic(&mut self) -> Result<(), OutputError> {
        self.sample_spool = None;
        self.cleanup_ephemeral();
        if self.bundle_final_path.exists() {
            quarantine_path(&self.bundle_final_path, &self.run_dir)?;
        }
        self.identity = None;
        self.finalized_ok = false;
        Ok(())
    }

    fn cleanup_ephemeral(&mut self) {
        let _ = fs::remove_file(&self.sample_spool_path);
        let _ = fs::remove_file(&self.bundle_tmp_path);
        for p in self.temp_paths.drain(..) {
            let _ = fs::remove_file(p);
        }
        if let Ok(rd) = fs::read_dir(&self.run_dir) {
            for ent in rd.flatten() {
                let p = ent.path();
                if p.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|n| n.starts_with("provenance-bundle.json.samples.run."))
                {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }

    /// Interns five field records and appends one sample assignment to the spool.
    pub fn push_sample(
        &mut self,
        particle_id: i64,
        sample_sequence: i64,
        fields: &FiveFieldRecords,
    ) -> Result<(), OutputError> {
        self.push_sample_refs(
            particle_id,
            sample_sequence,
            &FiveFieldRecordRefs {
                eastward_wind: fields.eastward_wind.as_ref(),
                northward_wind: fields.northward_wind.as_ref(),
                geometric_vertical_velocity: fields.geometric_vertical_velocity.as_ref(),
                air_pressure: fields.air_pressure.as_ref(),
                air_temperature: fields.air_temperature.as_ref(),
            },
        )
    }

    /// Interns borrowed records and appends one sample assignment to the spool.
    pub fn push_sample_refs(
        &mut self,
        particle_id: i64,
        sample_sequence: i64,
        fields: &FiveFieldRecordRefs<'_>,
    ) -> Result<(), OutputError> {
        let body = BundleFieldSetBody {
            eastward_wind: self.intern_optional_record(fields.eastward_wind, SLOT_EASTWARD)?,
            northward_wind: self.intern_optional_record(fields.northward_wind, SLOT_NORTHWARD)?,
            geometric_vertical_velocity: self
                .intern_optional_record(fields.geometric_vertical_velocity, SLOT_VERTICAL)?,
            air_pressure: self.intern_optional_record(fields.air_pressure, SLOT_PRESSURE)?,
            air_temperature: self
                .intern_optional_record(fields.air_temperature, SLOT_TEMPERATURE)?,
        };
        let slots = [
            body.eastward_wind.clone(),
            body.northward_wind.clone(),
            body.geometric_vertical_velocity.clone(),
            body.air_pressure.clone(),
            body.air_temperature.clone(),
        ];
        let set_sha = if let Some(set_sha) = self.field_set_sha_by_slots.get(&slots) {
            set_sha.clone()
        } else {
            let (set_sha, set_canonical) = hash_field_set_body(&body)?;
            if let Some(prev) = self.field_set_canonical_by_sha.get(&set_sha) {
                if prev != &set_canonical {
                    return Err(OutputError::Encoding(format!(
                        "field-set sha collision for {set_sha}"
                    )));
                }
            }
            if self.field_sets_by_sha.len() >= MAX_UNIQUE_FIELD_SETS {
                return Err(OutputError::Encoding(format!(
                    "field-set dictionary exceeded hard cap {MAX_UNIQUE_FIELD_SETS}"
                )));
            }
            self.field_set_canonical_by_sha
                .insert(set_sha.clone(), set_canonical);
            self.field_sets_by_sha.insert(
                set_sha.clone(),
                BundleFieldSetEntry {
                    sha256: set_sha.clone(),
                    fields: body,
                },
            );
            self.field_set_sha_by_slots.insert(slots, set_sha.clone());
            set_sha
        };
        let spool = self
            .sample_spool
            .as_mut()
            .ok_or_else(|| OutputError::Encoding("sample spool closed".into()))?;
        // TSV line: particle_id \t sample_sequence \t field_set_sha
        writeln!(spool, "{particle_id}\t{sample_sequence}\t{set_sha}")
            .map_err(|error| OutputError::Io(error.to_string()))?;
        self.sample_count = self
            .sample_count
            .checked_add(1)
            .ok_or_else(|| OutputError::Encoding("sample_count overflow".into()))?;
        Ok(())
    }

    fn intern_optional_record(
        &mut self,
        record: Option<&ProvenanceRecord>,
        expected_slot: &str,
    ) -> Result<Option<String>, OutputError> {
        let Some(record) = record else {
            return Ok(None);
        };
        if let Some(sha) = self.record_sha_by_runtime.get(record) {
            return Ok(Some(sha.clone()));
        }
        let body = bundle_record_body_from_runtime(record)?;
        let token_slot = body.field.expected_slot().ok_or_else(|| {
            OutputError::Encoding(format!(
                "provenance field is not a five-field slot (expected {expected_slot})"
            ))
        })?;
        if token_slot != expected_slot {
            return Err(OutputError::Encoding(format!(
                "provenance field slot mismatch: got {token_slot}, expected {expected_slot}"
            )));
        }
        let (sha, canonical) = hash_record_body(&body)?;
        if let Some(prev) = self.record_canonical_by_sha.get(&sha) {
            if prev != &canonical {
                return Err(OutputError::Encoding(format!(
                    "record sha collision for {sha}"
                )));
            }
            self.record_sha_by_runtime
                .insert(record.clone(), sha.clone());
            return Ok(Some(sha));
        }
        if self.records_by_sha.len() >= MAX_UNIQUE_RECORDS {
            return Err(OutputError::Encoding(format!(
                "record dictionary exceeded hard cap {MAX_UNIQUE_RECORDS}"
            )));
        }
        self.record_canonical_by_sha.insert(sha.clone(), canonical);
        self.records_by_sha.insert(
            sha.clone(),
            BundleRecordEntry {
                sha256: sha.clone(),
                record: body,
            },
        );
        self.record_sha_by_runtime
            .insert(record.clone(), sha.clone());
        Ok(Some(sha))
    }

    /// Finalizes after SQLite writer is closed: bounded external sort, lockstep
    /// SQLite coverage, streaming pretty JSON + digest, semantic validation, atomic replace.
    pub fn finalize_after_sqlite(
        &mut self,
        sqlite_path: &Path,
        sqlite_sha256: &str,
        sqlite_sql_sha256: &str,
    ) -> Result<ProvenanceBundleIdentity, OutputError> {
        let external_sort = PerformanceScope::enter(PerformanceStage::ProvenanceExternalSort);
        if let Some(mut spool) = self.sample_spool.take() {
            spool
                .flush()
                .map_err(|error| OutputError::Io(error.to_string()))?;
        }
        if !is_sha256_hex(sqlite_sha256) {
            return Err(OutputError::Encoding(
                "sqlite_sha256 must be lowercase 64-hex".into(),
            ));
        }
        if !is_sha256_hex(sqlite_sql_sha256) {
            return Err(OutputError::Encoding(
                "sqlite_sql_sha256 must be lowercase 64-hex".into(),
            ));
        }

        // 1) Bounded external sort of sample spool → single sorted run path.
        let sorted_run = external_sort_sample_spool(
            &self.sample_spool_path,
            &self.run_dir,
            self.chunk_lines,
            self.merge_fan_in,
            &mut self.temp_paths,
        )?;
        drop(external_sort);

        // Incremental memo/collision tables are no longer needed once the sample
        // spool is closed. Drop them before final serialization so peak memory is
        // bounded by the canonical dictionaries, not several duplicate copies.
        self.record_sha_by_runtime.clear();
        self.record_canonical_by_sha.clear();
        self.field_set_sha_by_slots.clear();
        self.field_set_canonical_by_sha.clear();

        // 2) BTreeMap values are already sorted by their SHA keys and remain
        // within the hard caps enforced during interning.
        let record_count = self.records_by_sha.len() as u64;
        let field_set_count = self.field_sets_by_sha.len() as u64;

        // 3) Stream write bundle while lockstep-validating against SQLite cursor.
        //    Never materialize full samples vec or full bundle bytes.
        let tmp = self.bundle_tmp_path.clone();
        let final_path = self.bundle_final_path.clone();
        if matches!(self.fault, Some(BundleFaultInject::BeforeTmpWrite)) {
            self.cleanup_ephemeral();
            return Err(OutputError::Encoding("fault: BeforeTmpWrite".into()));
        }

        let (sample_count, tmp_sha) = {
            let _performance = PerformanceScope::enter(PerformanceStage::ProvenanceStreamWrite);
            let mut digest_writer = DigestingFile::create(&tmp)?;
            let sample_count = stream_write_bundle(
                &mut digest_writer,
                &self.run_id,
                sqlite_sha256,
                &sorted_run,
                sqlite_path,
                &self.field_sets_by_sha,
                &self.records_by_sha,
                self.sample_count,
            )?;
            digest_writer.sync_all()?;
            // Exact SHA is of final on-disk bytes after rename; recompute from tmp then rename.
            (sample_count, digest_writer.finalize_sha256())
        };

        // 4) Full semantic validation on tmp via streaming re-read (no full Vec samples).
        //    Content digest is recomputed from the actual on-disk document.
        let summary = {
            let _performance =
                PerformanceScope::enter(PerformanceStage::ProvenanceSemanticValidate);
            validate_bundle_file_semantics(
                &tmp,
                &self.run_id,
                sqlite_sha256,
                sample_count,
                record_count,
                field_set_count,
                &self.records_by_sha,
                &self.field_sets_by_sha,
            )?
        };
        let content_digest = summary.content_sha256;

        if matches!(self.fault, Some(BundleFaultInject::BeforeRename)) {
            let _ = fs::remove_file(&tmp);
            self.cleanup_ephemeral();
            return Err(OutputError::Encoding("fault: BeforeRename".into()));
        }

        let (bundle_sha, canonical_out) = {
            let _performance = PerformanceScope::enter(PerformanceStage::ProvenanceFinalHash);
            fs::rename(&tmp, &final_path).map_err(|error| {
                let _ = fs::remove_file(&tmp);
                OutputError::Io(format!("atomic replace provenance-bundle: {error}"))
            })?;

            // Re-hash final path with streaming buffer (must match tmp_sha).
            let bundle_sha = file_sha256(&final_path)?;
            if bundle_sha != tmp_sha {
                let _ = fs::remove_file(&final_path);
                self.cleanup_ephemeral();
                return Err(OutputError::Encoding(
                    "bundle SHA changed across atomic rename".into(),
                ));
            }

            // 5) Canonical output from on-disk content digest + real SQL digest.
            let canonical_out = canonical_output_digest(sqlite_sql_sha256, &content_digest)?;
            (bundle_sha, canonical_out)
        };
        self.last_content_digest = Some(content_digest.clone());
        self.last_canonical_output_digest = Some(canonical_out.clone());

        if matches!(self.fault, Some(BundleFaultInject::AfterRename)) {
            // Forensic: final bundle exists; quarantine must succeed (error not swallowed).
            self.cleanup_ephemeral();
            self.quarantine_forensic()?;
            return Err(OutputError::Encoding("fault: AfterRename".into()));
        }

        // Cleanup spool + runs only after success path identity is built.
        {
            let _performance = PerformanceScope::enter(PerformanceStage::ProvenanceCleanup);
            self.cleanup_ephemeral();
        }
        self.finalized_ok = true;

        let identity = ProvenanceBundleIdentity {
            schema_version: PROVENANCE_BUNDLE_SCHEMA_ID.into(),
            relative_path: PROVENANCE_BUNDLE_FILE_NAME.into(),
            sha256: bundle_sha,
            sqlite_sha256: sqlite_sha256.to_owned(),
            content_sha256: content_digest,
            sqlite_sql_sha256: sqlite_sql_sha256.to_owned(),
            canonical_output_sha256: canonical_out,
            record_count,
            field_set_count,
            sample_count,
        };
        self.identity = Some(identity.clone());
        Ok(identity)
    }

    /// Returns identity after successful finalize.
    #[must_use]
    pub fn identity(&self) -> Option<&ProvenanceBundleIdentity> {
        self.identity.as_ref()
    }
}

impl Drop for ProvenanceBundleBuilder {
    fn drop(&mut self) {
        if !self.finalized_ok {
            // Drop cannot propagate; best-effort. Production paths must call abort() explicitly.
            let _ = self.abort();
        }
    }
}

fn bundle_record_body_from_runtime(
    record: &ProvenanceRecord,
) -> Result<BundleRecordBody, OutputError> {
    let field = FieldKeyToken::from_field_key(&record.field)?;
    let quality = match record.quality {
        FieldQuality::Source => "source",
        FieldQuality::Derived => "derived",
        FieldQuality::Estimated => "estimated",
    }
    .to_owned();
    let mut transforms = Vec::with_capacity(record.transforms.len());
    for step in &record.transforms {
        transforms.push(normalize_transform(step)?);
    }
    if !is_sha256_hex(&record.profile_sha256) {
        return Err(OutputError::Encoding(format!(
            "invalid profile_sha256 {}",
            record.profile_sha256
        )));
    }
    Ok(BundleRecordBody {
        field,
        quality,
        sources: record.sources.clone(),
        transforms,
        fallback_reason: record.fallback_reason.clone(),
        profile_sha256: record.profile_sha256.clone(),
    })
}

fn normalize_transform(step: &TransformRecord) -> Result<BundleTransform, OutputError> {
    let mut parameters: Vec<BundleParameter> = step
        .parameters
        .iter()
        .map(|(name, value)| BundleParameter {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    parameters.sort();
    for window in parameters.windows(2) {
        if window[0].name == window[1].name && window[0].value == window[1].value {
            return Err(OutputError::Encoding(format!(
                "duplicate transform parameter {}={}",
                window[0].name, window[0].value
            )));
        }
    }
    Ok(BundleTransform {
        operation: step.operation.clone(),
        parameters,
    })
}

fn hash_record_body(body: &BundleRecordBody) -> Result<(String, String), OutputError> {
    let value = serde_json::to_value(body)
        .map_err(|error| OutputError::Encoding(format!("record json: {error}")))?;
    let canonical = rfc8785_canonicalize(&value)?;
    let mut digest = Sha256::new();
    digest.update(canonical.as_bytes());
    Ok((hex::encode(digest.finalize()), canonical))
}

fn hash_field_set_body(body: &BundleFieldSetBody) -> Result<(String, String), OutputError> {
    let value = serde_json::to_value(body)
        .map_err(|error| OutputError::Encoding(format!("field-set json: {error}")))?;
    let canonical = rfc8785_canonicalize(&value)?;
    let mut digest = Sha256::new();
    digest.update(canonical.as_bytes());
    Ok((hex::encode(digest.finalize()), canonical))
}

/// RFC 8785 canonical JSON for the restricted v1 value domain (no numbers).
pub fn rfc8785_canonicalize(value: &serde_json::Value) -> Result<String, OutputError> {
    let mut out = String::new();
    write_rfc8785(value, &mut out)?;
    Ok(out)
}

fn write_rfc8785(value: &serde_json::Value, out: &mut String) -> Result<(), OutputError> {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        serde_json::Value::Number(n) => {
            // v1 hash inputs must not contain non-finite numbers; reject all numbers
            // to force string parameters (contract).
            return Err(OutputError::Encoding(format!(
                "rfc8785 v1 forbids bare numbers in hash input ({n})"
            )));
        }
        serde_json::Value::String(s) => {
            out.push('"');
            for ch in s.chars() {
                match ch {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\u{08}' => out.push_str("\\b"),
                    '\u{0C}' => out.push_str("\\f"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => {
                        out.push_str(&format!("\\u{:04x}", c as u32));
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_rfc8785(item, out)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            // RFC 8785: sort members by UTF-16 code units. For pure ASCII keys
            // used in v1, UTF-8 byte order matches.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| {
                let au: Vec<u16> = a.encode_utf16().collect();
                let bu: Vec<u16> = b.encode_utf16().collect();
                au.cmp(&bu)
            });
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_rfc8785(&serde_json::Value::String((*key).clone()), out)?;
                out.push(':');
                write_rfc8785(&map[*key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

// ----- Bounded external sort + streaming bundle IO -----

struct DigestingFile {
    writer: BufWriter<File>,
    hasher: Sha256,
}

impl DigestingFile {
    fn create(path: &Path) -> Result<Self, OutputError> {
        let file = File::create(path).map_err(|error| OutputError::Io(error.to_string()))?;
        Ok(Self {
            writer: BufWriter::with_capacity(PROVENANCE_BUNDLE_WRITE_BUFFER_BYTES, file),
            hasher: Sha256::new(),
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), OutputError> {
        self.writer
            .write_all(bytes)
            .map_err(|error| OutputError::Io(error.to_string()))?;
        self.hasher.update(bytes);
        Ok(())
    }

    fn sync_all(&mut self) -> Result<(), OutputError> {
        self.writer
            .flush()
            .map_err(|error| OutputError::Io(error.to_string()))?;
        self.writer
            .get_ref()
            .sync_all()
            .map_err(|error| OutputError::Io(error.to_string()))
    }

    fn finalize_sha256(self) -> String {
        hex::encode(self.hasher.finalize())
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// Stream SHA-256 of a file with a fixed buffer (no full read).
pub fn file_sha256(path: &Path) -> Result<String, OutputError> {
    let mut file = File::open(path).map_err(|error| OutputError::Io(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|error| OutputError::Io(error.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SampleKey {
    particle_id: i64,
    sample_sequence: i64,
    field_set_sha256: String,
}

impl Ord for SampleKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.particle_id
            .cmp(&other.particle_id)
            .then(self.sample_sequence.cmp(&other.sample_sequence))
            .then(self.field_set_sha256.cmp(&other.field_set_sha256))
    }
}
impl PartialOrd for SampleKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_sample_line(line: &str, lineno: usize) -> Result<SampleKey, OutputError> {
    let mut parts = line.split('\t');
    let particle_id = parts
        .next()
        .ok_or_else(|| OutputError::Encoding(format!("spool line {lineno}: missing pid")))?
        .parse::<i64>()
        .map_err(|error| OutputError::Encoding(format!("spool line {lineno}: {error}")))?;
    let sample_sequence = parts
        .next()
        .ok_or_else(|| OutputError::Encoding(format!("spool line {lineno}: missing seq")))?
        .parse::<i64>()
        .map_err(|error| OutputError::Encoding(format!("spool line {lineno}: {error}")))?;
    let field_set_sha256 = parts
        .next()
        .ok_or_else(|| OutputError::Encoding(format!("spool line {lineno}: missing sha")))?
        .to_owned();
    if parts.next().is_some() {
        return Err(OutputError::Encoding(format!(
            "spool line {lineno}: extra columns"
        )));
    }
    if particle_id < 0 || sample_sequence < 0 {
        return Err(OutputError::Encoding(format!(
            "spool line {lineno}: negative key"
        )));
    }
    if !is_sha256_hex(&field_set_sha256) {
        return Err(OutputError::Encoding(format!(
            "spool line {lineno}: field_set_sha not lowercase 64-hex"
        )));
    }
    Ok(SampleKey {
        particle_id,
        sample_sequence,
        field_set_sha256,
    })
}

fn write_sample_line(file: &mut impl Write, s: &SampleKey) -> Result<(), OutputError> {
    writeln!(
        file,
        "{}\t{}\t{}",
        s.particle_id, s.sample_sequence, s.field_set_sha256
    )
    .map_err(|error| OutputError::Io(error.to_string()))
}

fn external_sort_sample_spool(
    spool_path: &Path,
    run_dir: &Path,
    chunk_lines: usize,
    merge_fan_in: usize,
    temp_paths: &mut Vec<PathBuf>,
) -> Result<PathBuf, OutputError> {
    let empty_run = run_dir.join("provenance-bundle.json.samples.run.sorted");
    if !spool_path.exists() {
        File::create(&empty_run).map_err(|error| OutputError::Io(error.to_string()))?;
        temp_paths.push(empty_run.clone());
        return Ok(empty_run);
    }
    let file = File::open(spool_path).map_err(|error| OutputError::Io(error.to_string()))?;
    let reader = BufReader::new(file);
    let mut runs: Vec<PathBuf> = Vec::new();
    let mut chunk: Vec<SampleKey> = Vec::with_capacity(chunk_lines);
    let mut lineno = 0usize;
    let mut run_idx = 0usize;
    for line in reader.lines() {
        let line = line.map_err(|error| OutputError::Io(error.to_string()))?;
        if line.is_empty() {
            continue;
        }
        lineno += 1;
        chunk.push(parse_sample_line(&line, lineno)?);
        if chunk.len() >= chunk_lines {
            let path = flush_sorted_run(run_dir, run_idx, &mut chunk, temp_paths)?;
            runs.push(path);
            run_idx += 1;
        }
    }
    if !chunk.is_empty() {
        let path = flush_sorted_run(run_dir, run_idx, &mut chunk, temp_paths)?;
        runs.push(path);
    }
    if runs.is_empty() {
        File::create(&empty_run).map_err(|error| OutputError::Io(error.to_string()))?;
        temp_paths.push(empty_run.clone());
        return Ok(empty_run);
    }
    let mut round = 0usize;
    while runs.len() > 1 {
        let mut next: Vec<PathBuf> = Vec::new();
        let mut i = 0usize;
        let mut out_idx = 0usize;
        while i < runs.len() {
            let end = (i + merge_fan_in).min(runs.len());
            let group = &runs[i..end];
            if group.len() == 1 {
                next.push(group[0].clone());
            } else {
                let out = run_dir.join(format!(
                    "provenance-bundle.json.samples.run.m{round}.{out_idx}"
                ));
                kway_merge_runs(group, &out)?;
                temp_paths.push(out.clone());
                next.push(out);
                out_idx += 1;
            }
            i = end;
        }
        runs = next;
        round += 1;
        if round > 64 {
            return Err(OutputError::Encoding(
                "external sort exceeded merge rounds".into(),
            ));
        }
    }
    Ok(runs[0].clone())
}

fn flush_sorted_run(
    run_dir: &Path,
    run_idx: usize,
    chunk: &mut Vec<SampleKey>,
    temp_paths: &mut Vec<PathBuf>,
) -> Result<PathBuf, OutputError> {
    chunk.sort();
    for w in chunk.windows(2) {
        if w[0].particle_id == w[1].particle_id && w[0].sample_sequence == w[1].sample_sequence {
            return Err(OutputError::Encoding(format!(
                "duplicate sample assignment ({}, {})",
                w[0].particle_id, w[0].sample_sequence
            )));
        }
    }
    let path = run_dir.join(format!("provenance-bundle.json.samples.run.{run_idx}"));
    let mut f = BufWriter::new(File::create(&path).map_err(|e| OutputError::Io(e.to_string()))?);
    for s in chunk.iter() {
        write_sample_line(&mut f, s)?;
    }
    f.flush().map_err(|e| OutputError::Io(e.to_string()))?;
    chunk.clear();
    temp_paths.push(path.clone());
    Ok(path)
}

#[derive(Eq, PartialEq)]
struct HeapItem {
    key: SampleKey,
    run_index: usize,
}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .key
            .cmp(&self.key)
            .then(other.run_index.cmp(&self.run_index))
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn kway_merge_runs(inputs: &[PathBuf], output: &Path) -> Result<(), OutputError> {
    if inputs.len() > DEFAULT_MERGE_FAN_IN {
        return Err(OutputError::Encoding(format!(
            "merge fan-in {} exceeds hard cap {}",
            inputs.len(),
            DEFAULT_MERGE_FAN_IN
        )));
    }
    let mut readers: Vec<std::io::Lines<BufReader<File>>> = Vec::with_capacity(inputs.len());
    for p in inputs {
        let f = File::open(p).map_err(|e| OutputError::Io(e.to_string()))?;
        readers.push(BufReader::new(f).lines());
    }
    let mut heap = BinaryHeap::new();
    for (idx, reader) in readers.iter_mut().enumerate() {
        loop {
            match reader.next() {
                None => break,
                Some(Err(e)) => return Err(OutputError::Io(e.to_string())),
                Some(Ok(line)) if line.is_empty() => continue,
                Some(Ok(line)) => {
                    let key = parse_sample_line(&line, 0)?;
                    heap.push(HeapItem {
                        key,
                        run_index: idx,
                    });
                    break;
                }
            }
        }
    }
    let mut out = BufWriter::new(File::create(output).map_err(|e| OutputError::Io(e.to_string()))?);
    let mut last: Option<(i64, i64)> = None;
    while let Some(item) = heap.pop() {
        let HeapItem { key, run_index } = item;
        if let Some((lp, ls)) = last {
            if lp == key.particle_id && ls == key.sample_sequence {
                return Err(OutputError::Encoding(format!(
                    "duplicate sample assignment across runs ({lp}, {ls})"
                )));
            }
            if (key.particle_id, key.sample_sequence) < (lp, ls) {
                return Err(OutputError::Encoding(
                    "merge produced out-of-order samples".into(),
                ));
            }
        }
        last = Some((key.particle_id, key.sample_sequence));
        write_sample_line(&mut out, &key)?;
        loop {
            match readers[run_index].next() {
                None => break,
                Some(Err(e)) => return Err(OutputError::Io(e.to_string())),
                Some(Ok(line)) if line.is_empty() => continue,
                Some(Ok(line)) => {
                    let next_key = parse_sample_line(&line, 0)?;
                    heap.push(HeapItem {
                        key: next_key,
                        run_index,
                    });
                    break;
                }
            }
        }
    }
    out.flush().map_err(|e| OutputError::Io(e.to_string()))?;
    Ok(())
}

fn sample_key_to_assignment(s: &SampleKey) -> BundleSampleAssignment {
    BundleSampleAssignment {
        particle_id: s.particle_id,
        sample_sequence: s.sample_sequence,
        field_set_sha256: s.field_set_sha256.clone(),
    }
}

fn write_json_pretty_value(
    w: &mut DigestingFile,
    value: &serde_json::Value,
    indent: usize,
) -> Result<(), OutputError> {
    let pad = "  ".repeat(indent);
    match value {
        serde_json::Value::Null => w.write_all(b"null"),
        serde_json::Value::Bool(b) => w.write_all(if *b { b"true" } else { b"false" }),
        serde_json::Value::Number(n) => w.write_all(n.to_string().as_bytes()),
        serde_json::Value::String(s) => w.write_all(json_string(s).as_bytes()),
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return w.write_all(b"[]");
            }
            w.write_all(b"[\n")?;
            for (i, item) in items.iter().enumerate() {
                w.write_all(format!("{pad}  ").as_bytes())?;
                write_json_pretty_value(w, item, indent + 1)?;
                if i + 1 != items.len() {
                    w.write_all(b",\n")?;
                } else {
                    w.write_all(b"\n")?;
                }
            }
            w.write_all(format!("{pad}]").as_bytes())
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                return w.write_all(b"{}");
            }
            w.write_all(b"{\n")?;
            let len = map.len();
            for (i, (k, v)) in map.iter().enumerate() {
                w.write_all(format!("{pad}  {}: ", json_string(k)).as_bytes())?;
                write_json_pretty_value(w, v, indent + 1)?;
                if i + 1 != len {
                    w.write_all(b",\n")?;
                } else {
                    w.write_all(b"\n")?;
                }
            }
            w.write_all(format!("{pad}}}").as_bytes())
        }
    }
}

fn write_json_pretty_array<'a, T: Serialize + 'a>(
    w: &mut DigestingFile,
    items: impl IntoIterator<Item = &'a T>,
    indent: usize,
    context: &str,
) -> Result<(), OutputError> {
    let pad = "  ".repeat(indent);
    let mut items = items.into_iter().peekable();
    if items.peek().is_none() {
        return w.write_all(b"[]");
    }
    w.write_all(b"[\n")?;
    while let Some(item) = items.next() {
        w.write_all(format!("{pad}  ").as_bytes())?;
        let value = serde_json::to_value(item)
            .map_err(|error| OutputError::Encoding(format!("{context} json: {error}")))?;
        write_json_pretty_value(w, &value, indent + 1)?;
        if items.peek().is_some() {
            w.write_all(b",\n")?;
        } else {
            w.write_all(b"\n")?;
        }
    }
    w.write_all(format!("{pad}]").as_bytes())
}

#[allow(clippy::too_many_arguments)]
fn stream_write_bundle(
    w: &mut DigestingFile,
    run_id: &str,
    sqlite_sha256: &str,
    sorted_run: &Path,
    sqlite_path: &Path,
    field_sets_by_sha: &BTreeMap<String, BundleFieldSetEntry>,
    records_by_sha: &BTreeMap<String, BundleRecordEntry>,
    expected_sample_count: u64,
) -> Result<u64, OutputError> {
    w.write_all(b"{\n")?;
    w.write_all(
        format!(
            "  \"schema_version\": {},\n",
            json_string(PROVENANCE_BUNDLE_SCHEMA_ID)
        )
        .as_bytes(),
    )?;
    w.write_all(format!("  \"run_id\": {},\n", json_string(run_id)).as_bytes())?;
    w.write_all(b"  \"sqlite\": {\n")?;
    w.write_all(
        format!(
            "    \"relative_path\": {},\n",
            json_string("particles.sqlite")
        )
        .as_bytes(),
    )?;
    w.write_all(format!("    \"schema_version\": {},\n", SQLITE_SCHEMA_VERSION).as_bytes())?;
    w.write_all(format!("    \"sha256\": {}\n", json_string(sqlite_sha256)).as_bytes())?;
    w.write_all(b"  },\n")?;
    w.write_all(
        format!(
            "  \"record_hash_algorithm\": {},\n",
            json_string(PROVENANCE_RECORD_HASH_ALGORITHM)
        )
        .as_bytes(),
    )?;
    w.write_all(b"  \"records\": ")?;
    write_json_pretty_array(w, records_by_sha.values(), 1, "records")?;
    w.write_all(b",\n")?;
    w.write_all(b"  \"field_sets\": ")?;
    write_json_pretty_array(w, field_sets_by_sha.values(), 1, "field_sets")?;
    w.write_all(b",\n")?;
    w.write_all(b"  \"samples\": [")?;
    let sample_count = lockstep_write_samples(
        w,
        run_id,
        sorted_run,
        sqlite_path,
        field_sets_by_sha,
        records_by_sha,
        expected_sample_count,
    )?;
    w.write_all(b"]\n}\n")?;
    Ok(sample_count)
}

fn lockstep_write_samples(
    w: &mut DigestingFile,
    run_id: &str,
    sorted_run: &Path,
    sqlite_path: &Path,
    field_sets_by_sha: &BTreeMap<String, BundleFieldSetEntry>,
    records_by_sha: &BTreeMap<String, BundleRecordEntry>,
    expected_sample_count: u64,
) -> Result<u64, OutputError> {
    let connection =
        Connection::open_with_flags(sqlite_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| OutputError::Io(error.to_string()))?;
    let mut stmt = connection
        .prepare(LOCKSTEP_PARTICLE_STATE_SQL)
        .map_err(|error| OutputError::Io(error.to_string()))?;
    let mut rows = stmt
        .query([run_id])
        .map_err(|error| OutputError::Io(error.to_string()))?;
    let run_file = File::open(sorted_run).map_err(|e| OutputError::Io(e.to_string()))?;
    let mut run_iter = BufReader::new(run_file).lines();
    let mut count = 0u64;
    let mut first = true;
    let mut lineno = 0usize;
    loop {
        let sql_row = rows
            .next()
            .map_err(|error| OutputError::Io(error.to_string()))?;
        // pull next non-empty run line
        let mut run_line = None;
        for item in run_iter.by_ref() {
            let line = item.map_err(|e| OutputError::Io(e.to_string()))?;
            if !line.is_empty() {
                run_line = Some(line);
                break;
            }
        }
        match (sql_row, run_line) {
            (None, None) => break,
            (None, Some(_)) => {
                return Err(OutputError::Encoding(
                    "bundle sample not present in SQLite particle_state".into(),
                ));
            }
            (Some(_), None) => {
                return Err(OutputError::Encoding(
                    "SQLite particle_state row missing bundle sample".into(),
                ));
            }
            (Some(row), Some(line)) => {
                lineno += 1;
                let pid: i64 = row.get(0).map_err(|e| OutputError::Io(e.to_string()))?;
                let seq: i64 = row.get(1).map_err(|e| OutputError::Io(e.to_string()))?;
                let nn_u: bool = row.get(2).map_err(|e| OutputError::Io(e.to_string()))?;
                let nn_v: bool = row.get(3).map_err(|e| OutputError::Io(e.to_string()))?;
                let nn_w: bool = row.get(4).map_err(|e| OutputError::Io(e.to_string()))?;
                let nn_p: bool = row.get(5).map_err(|e| OutputError::Io(e.to_string()))?;
                let nn_t: bool = row.get(6).map_err(|e| OutputError::Io(e.to_string()))?;
                let wind_q: Option<String> =
                    row.get(7).map_err(|e| OutputError::Io(e.to_string()))?;
                let pres_q: Option<String> =
                    row.get(8).map_err(|e| OutputError::Io(e.to_string()))?;
                let temp_q: Option<String> =
                    row.get(9).map_err(|e| OutputError::Io(e.to_string()))?;
                let sample = parse_sample_line(&line, lineno)?;
                if sample.particle_id != pid || sample.sample_sequence != seq {
                    return Err(OutputError::Encoding(format!(
                        "sample/SQLite key mismatch bundle=({},{}) sql=({pid},{seq})",
                        sample.particle_id, sample.sample_sequence
                    )));
                }
                let set = field_sets_by_sha
                    .get(&sample.field_set_sha256)
                    .ok_or_else(|| {
                        OutputError::Encoding(format!(
                            "sample references missing field_set {}",
                            sample.field_set_sha256
                        ))
                    })?;
                validate_field_set_slots(set, records_by_sha)?;
                check_nn(
                    nn_u,
                    set.fields.eastward_wind.as_ref(),
                    "eastward_wind",
                    pid,
                    seq,
                )?;
                check_nn(
                    nn_v,
                    set.fields.northward_wind.as_ref(),
                    "northward_wind",
                    pid,
                    seq,
                )?;
                check_nn(
                    nn_w,
                    set.fields.geometric_vertical_velocity.as_ref(),
                    "geometric_vertical_velocity",
                    pid,
                    seq,
                )?;
                check_nn(
                    nn_p,
                    set.fields.air_pressure.as_ref(),
                    "air_pressure",
                    pid,
                    seq,
                )?;
                check_nn(
                    nn_t,
                    set.fields.air_temperature.as_ref(),
                    "air_temperature",
                    pid,
                    seq,
                )?;
                check_slot_quality(
                    set.fields.air_pressure.as_ref(),
                    records_by_sha,
                    pres_q.as_deref(),
                    "pressure",
                )?;
                check_slot_quality(
                    set.fields.air_temperature.as_ref(),
                    records_by_sha,
                    temp_q.as_deref(),
                    "temperature",
                )?;
                check_wind_quality(set, records_by_sha, wind_q.as_deref())?;
                let assignment = sample_key_to_assignment(&sample);
                let val = serde_json::to_value(&assignment)
                    .map_err(|e| OutputError::Encoding(format!("sample json: {e}")))?;
                if !first {
                    w.write_all(b",")?;
                }
                first = false;
                w.write_all(
                    b"
    ",
                )?;
                write_json_pretty_value(w, &val, 2)?;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| OutputError::Encoding("sample_count overflow".into()))?;
            }
        }
    }
    if count != expected_sample_count {
        return Err(OutputError::Encoding(format!(
            "sample count mismatch memory={expected_sample_count} streamed={count}"
        )));
    }
    if count > 0 {
        w.write_all(
            b"
  ",
        )?;
    }
    Ok(count)
}

fn check_nn(
    sql_non_null: bool,
    slot: Option<&String>,
    name: &str,
    pid: i64,
    seq: i64,
) -> Result<(), OutputError> {
    if sql_non_null && slot.is_none() {
        return Err(OutputError::Encoding(format!(
            "SQL non-null {name} without provenance record at ({pid},{seq})"
        )));
    }
    Ok(())
}

fn check_slot_quality(
    slot: Option<&String>,
    records: &BTreeMap<String, BundleRecordEntry>,
    sql_quality: Option<&str>,
    label: &str,
) -> Result<(), OutputError> {
    let Some(sha) = slot else {
        return Ok(());
    };
    let rec = records
        .get(sha)
        .ok_or_else(|| OutputError::Encoding(format!("missing record {sha}")))?;
    if let Some(sq) = sql_quality {
        if rec.record.quality != sq {
            return Err(OutputError::Encoding(format!(
                "{label} quality mismatch sql={sq} record={}",
                rec.record.quality
            )));
        }
    }
    Ok(())
}

fn quality_rank(q: &str) -> i32 {
    match q {
        "source" => 0,
        "derived" => 1,
        "estimated" => 2,
        _ => -1,
    }
}

fn check_wind_quality(
    set: &BundleFieldSetEntry,
    records: &BTreeMap<String, BundleRecordEntry>,
    sql_wind_q: Option<&str>,
) -> Result<(), OutputError> {
    let mut worst: Option<&str> = None;
    for slot in [
        set.fields.eastward_wind.as_ref(),
        set.fields.northward_wind.as_ref(),
        set.fields.geometric_vertical_velocity.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        let rec = records
            .get(slot)
            .ok_or_else(|| OutputError::Encoding(format!("missing wind record {slot}")))?;
        let q = rec.record.quality.as_str();
        worst = Some(match worst {
            None => q,
            Some(prev) if quality_rank(q) > quality_rank(prev) => q,
            Some(prev) => prev,
        });
    }
    match (worst, sql_wind_q) {
        (None, None) | (None, Some(_)) => Ok(()),
        (Some(w), Some(s)) if w == s => Ok(()),
        (Some(w), Some(s)) => Err(OutputError::Encoding(format!(
            "wind quality mismatch sql={s} worst-of-UVW={w}"
        ))),
        (Some(w), None) => Err(OutputError::Encoding(format!(
            "wind quality missing in SQL but records present ({w})"
        ))),
    }
}

fn validate_field_set_slots(
    set: &BundleFieldSetEntry,
    records: &BTreeMap<String, BundleRecordEntry>,
) -> Result<(), OutputError> {
    let pairs = [
        (SLOT_EASTWARD, set.fields.eastward_wind.as_deref()),
        (SLOT_NORTHWARD, set.fields.northward_wind.as_deref()),
        (
            SLOT_VERTICAL,
            set.fields.geometric_vertical_velocity.as_deref(),
        ),
        (SLOT_PRESSURE, set.fields.air_pressure.as_deref()),
        (SLOT_TEMPERATURE, set.fields.air_temperature.as_deref()),
    ];
    for (slot, sha) in pairs {
        let Some(sha) = sha else {
            continue;
        };
        if !is_sha256_hex(sha) {
            return Err(OutputError::Encoding(format!(
                "field-set slot {slot} sha not lowercase 64-hex"
            )));
        }
        let entry = records.get(sha).ok_or_else(|| {
            OutputError::Encoding(format!("field-set {} missing record {sha}", set.sha256))
        })?;
        let got = entry.record.field.expected_slot().ok_or_else(|| {
            OutputError::Encoding(format!("record {sha} field is not a five-field slot"))
        })?;
        if got != slot {
            return Err(OutputError::Encoding(format!(
                "field-set {} slot {slot} points to record field {got}",
                set.sha256
            )));
        }
    }
    Ok(())
}

fn validate_field_set_slots_in_sorted_records(
    set: &BundleFieldSetEntry,
    records: &[BundleRecordEntry],
) -> Result<(), OutputError> {
    let pairs = [
        (SLOT_EASTWARD, set.fields.eastward_wind.as_deref()),
        (SLOT_NORTHWARD, set.fields.northward_wind.as_deref()),
        (
            SLOT_VERTICAL,
            set.fields.geometric_vertical_velocity.as_deref(),
        ),
        (SLOT_PRESSURE, set.fields.air_pressure.as_deref()),
        (SLOT_TEMPERATURE, set.fields.air_temperature.as_deref()),
    ];
    for (slot, sha) in pairs {
        let Some(sha) = sha else {
            continue;
        };
        if !is_sha256_hex(sha) {
            return Err(OutputError::Encoding(format!(
                "field-set slot {slot} sha not lowercase 64-hex"
            )));
        }
        let index = records
            .binary_search_by(|entry| entry.sha256.as_str().cmp(sha))
            .map_err(|_| {
                OutputError::Encoding(format!("field-set {} missing record {sha}", set.sha256))
            })?;
        let got = records[index].record.field.expected_slot().ok_or_else(|| {
            OutputError::Encoding(format!("record {sha} field is not a five-field slot"))
        })?;
        if got != slot {
            return Err(OutputError::Encoding(format!(
                "field-set {} slot {slot} points to record field {got}",
                set.sha256
            )));
        }
    }
    Ok(())
}

/// Compact summary from streaming on-disk validation (never carries samples).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleValidationSummary {
    /// Unique record count observed on disk.
    pub record_count: u64,
    /// Unique field-set count observed on disk.
    pub field_set_count: u64,
    /// Sample assignment count observed on disk.
    pub sample_count: u64,
    /// Normalized provenance content digest recomputed from on-disk stream.
    pub content_sha256: String,
}

/// Full semantic validation of an on-disk tmp/final bundle before publish.
///
/// Never materializes `samples: Vec<_>`. records/field_sets use capped streaming
/// seeds (fail at MAX+1). After one document, `de.end()` rejects trailing bytes.
/// Returns [`BundleValidationSummary`] including on-disk content digest.
#[allow(clippy::too_many_arguments)]
pub fn validate_bundle_file_semantics(
    path: &Path,
    expected_run_id: &str,
    expected_sqlite_sha: &str,
    expected_sample_count: u64,
    expected_record_count: u64,
    expected_field_set_count: u64,
    records_by_sha: &BTreeMap<String, BundleRecordEntry>,
    field_sets_by_sha: &BTreeMap<String, BundleFieldSetEntry>,
) -> Result<BundleValidationSummary, OutputError> {
    let file = File::open(path).map_err(|e| OutputError::Io(e.to_string()))?;
    let reader = BufReader::new(file);
    let mut de = serde_json::Deserializer::from_reader(reader);
    let seed = BundleStreamingValidateSeed {
        expected_run_id,
        expected_sqlite_sha,
        expected_sample_count,
        expected_record_count,
        expected_field_set_count,
        records_by_sha,
        field_sets_by_sha,
    };
    let summary = seed
        .deserialize(&mut de)
        .map_err(|e| OutputError::Encoding(format!("bundle semantic stream: {e}")))?;
    // Exactly one JSON value — reject trailing `{}`, garbage, second documents.
    de.end()
        .map_err(|e| OutputError::Encoding(format!("bundle trailing input: {e}")))?;
    Ok(summary)
}

/// Independent on-disk recompute used by inspection / CFSR (no builder dict required
/// when `records_by_sha`/`field_sets_by_sha` are empty maps — dictionary checks skipped).
#[allow(clippy::too_many_arguments)]
pub fn validate_bundle_file_semantics_loose(
    path: &Path,
    expected_run_id: &str,
    expected_sqlite_sha: &str,
) -> Result<BundleValidationSummary, OutputError> {
    // Counts unknown a priori: pass u64::MAX as "no expected" and skip exact count match
    // by using a dedicated free-form path.
    let file = File::open(path).map_err(|e| OutputError::Io(e.to_string()))?;
    let reader = BufReader::new(file);
    let mut de = serde_json::Deserializer::from_reader(reader);
    let empty_r = BTreeMap::new();
    let empty_f = BTreeMap::new();
    let seed = BundleStreamingValidateSeed {
        expected_run_id,
        expected_sqlite_sha,
        expected_sample_count: u64::MAX, // sentinel: any count OK
        expected_record_count: u64::MAX,
        expected_field_set_count: u64::MAX,
        records_by_sha: &empty_r,
        field_sets_by_sha: &empty_f,
    };
    let summary = seed
        .deserialize(&mut de)
        .map_err(|e| OutputError::Encoding(format!("bundle semantic stream: {e}")))?;
    de.end()
        .map_err(|e| OutputError::Encoding(format!("bundle trailing input: {e}")))?;
    Ok(summary)
}

struct BundleStreamingValidateSeed<'a> {
    expected_run_id: &'a str,
    expected_sqlite_sha: &'a str,
    expected_sample_count: u64,
    expected_record_count: u64,
    expected_field_set_count: u64,
    records_by_sha: &'a BTreeMap<String, BundleRecordEntry>,
    field_sets_by_sha: &'a BTreeMap<String, BundleFieldSetEntry>,
}

impl<'de, 'a> DeserializeSeed<'de> for BundleStreamingValidateSeed<'a> {
    type Value = BundleValidationSummary;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(self)
    }
}

impl<'de, 'a> Visitor<'de> for BundleStreamingValidateSeed<'a> {
    type Value = BundleValidationSummary;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("provenance-bundle object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<BundleValidationSummary, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut schema_version: Option<String> = None;
        let mut run_id: Option<String> = None;
        let mut sqlite: Option<BundleSqliteRef> = None;
        let mut algo: Option<String> = None;
        let mut records: Option<Vec<BundleRecordEntry>> = None;
        let mut field_sets: Option<Vec<BundleFieldSetEntry>> = None;
        let mut sample_count: Option<u64> = None;
        let content = RefCell::new(Sha256::new());
        {
            let mut h = content.borrow_mut();
            h.update(format!("{PROVENANCE_CONTENT_DIGEST_ID}\n").as_bytes());
            h.update(b"record_hash_algorithm=sha256-rfc8785\n");
        }

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "schema_version" => {
                    if schema_version.replace(map.next_value()?).is_some() {
                        return Err(de::Error::custom("duplicate schema_version"));
                    }
                }
                "run_id" => {
                    if run_id.replace(map.next_value()?).is_some() {
                        return Err(de::Error::custom("duplicate run_id"));
                    }
                }
                "sqlite" => {
                    if sqlite.replace(map.next_value()?).is_some() {
                        return Err(de::Error::custom("duplicate sqlite"));
                    }
                }
                "record_hash_algorithm" => {
                    if algo.replace(map.next_value()?).is_some() {
                        return Err(de::Error::custom("duplicate record_hash_algorithm"));
                    }
                }
                "records" => {
                    if records.is_some() {
                        return Err(de::Error::custom("duplicate records"));
                    }
                    records = Some(map.next_value_seed(CappedRecordVecSeed {
                        max: MAX_UNIQUE_RECORDS,
                        expected: self.expected_record_count,
                        builder: self.records_by_sha,
                        content: &content,
                    })?);
                }
                "field_sets" => {
                    if field_sets.is_some() {
                        return Err(de::Error::custom("duplicate field_sets"));
                    }
                    let recs = records
                        .as_ref()
                        .ok_or_else(|| de::Error::custom("records must precede field_sets"))?;
                    field_sets = Some(map.next_value_seed(CappedFieldSetVecSeed {
                        max: MAX_UNIQUE_FIELD_SETS,
                        expected: self.expected_field_set_count,
                        record_shas: recs,
                        builder_sets: self.field_sets_by_sha,
                        content: &content,
                    })?);
                }
                "samples" => {
                    if sample_count.is_some() {
                        return Err(de::Error::custom("duplicate samples"));
                    }
                    let field_sets = field_sets
                        .as_ref()
                        .ok_or_else(|| de::Error::custom("field_sets must precede samples"))?;
                    let fs_keys: std::collections::BTreeSet<&str> =
                        field_sets.iter().map(|f| f.sha256.as_str()).collect();
                    sample_count = Some(map.next_value_seed(SamplesStreamingSeed {
                        field_set_keys: &fs_keys,
                        expected_count: self.expected_sample_count,
                        content: &content,
                    })?);
                }
                other => {
                    return Err(de::Error::custom(format!("unknown bundle field {other}")));
                }
            }
        }

        let schema_version =
            schema_version.ok_or_else(|| de::Error::custom("missing schema_version"))?;
        let run_id = run_id.ok_or_else(|| de::Error::custom("missing run_id"))?;
        let sqlite = sqlite.ok_or_else(|| de::Error::custom("missing sqlite"))?;
        let algo = algo.ok_or_else(|| de::Error::custom("missing record_hash_algorithm"))?;
        let records = records.ok_or_else(|| de::Error::custom("missing records"))?;
        let field_sets = field_sets.ok_or_else(|| de::Error::custom("missing field_sets"))?;
        let sample_count = sample_count.ok_or_else(|| de::Error::custom("missing samples"))?;
        if schema_version != PROVENANCE_BUNDLE_SCHEMA_ID {
            return Err(de::Error::custom(format!(
                "schema_version {schema_version}"
            )));
        }
        if run_id != self.expected_run_id {
            return Err(de::Error::custom("run_id mismatch"));
        }
        if algo != PROVENANCE_RECORD_HASH_ALGORITHM {
            return Err(de::Error::custom("record_hash_algorithm mismatch"));
        }
        if sqlite.relative_path != "particles.sqlite" {
            return Err(de::Error::custom("sqlite.relative_path"));
        }
        if sqlite.schema_version != SQLITE_SCHEMA_VERSION {
            return Err(de::Error::custom("sqlite.schema_version"));
        }
        if !is_sha256_hex(&sqlite.sha256) || sqlite.sha256 != self.expected_sqlite_sha {
            return Err(de::Error::custom("sqlite.sha256 invalid/mismatch"));
        }
        Ok(BundleValidationSummary {
            record_count: records.len() as u64,
            field_set_count: field_sets.len() as u64,
            sample_count,
            content_sha256: hex::encode(content.into_inner().finalize()),
        })
    }
}

struct CappedRecordVecSeed<'a> {
    max: usize,
    expected: u64,
    builder: &'a BTreeMap<String, BundleRecordEntry>,
    content: &'a RefCell<Sha256>,
}

impl<'de, 'a> DeserializeSeed<'de> for CappedRecordVecSeed<'a> {
    type Value = Vec<BundleRecordEntry>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, 'a> Visitor<'de> for CappedRecordVecSeed<'a> {
    type Value = Vec<BundleRecordEntry>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("records array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        let mut prev: Option<String> = None;
        while let Some(entry) = seq.next_element::<BundleRecordEntry>()? {
            if out.len() >= self.max {
                return Err(de::Error::custom(format!(
                    "records exceeded hard cap {}",
                    self.max
                )));
            }
            if !is_sha256_hex(&entry.sha256) {
                return Err(de::Error::custom("record sha uppercase/invalid"));
            }
            if prev
                .as_ref()
                .is_some_and(|p| entry.sha256.as_str() <= p.as_str())
            {
                return Err(de::Error::custom("records not strictly sorted/unique"));
            }
            let (sha, _) =
                hash_record_body(&entry.record).map_err(|e| de::Error::custom(format!("{e:?}")))?;
            if sha != entry.sha256 {
                return Err(de::Error::custom(format!(
                    "record hash mismatch {}",
                    entry.sha256
                )));
            }
            validate_record_body_semantics(&entry.record)
                .map_err(|e| de::Error::custom(format!("{e:?}")))?;
            if !self.builder.is_empty() && !self.builder.contains_key(&entry.sha256) {
                return Err(de::Error::custom("record not in builder dict"));
            }
            self.content
                .borrow_mut()
                .update(format!("record={}\n", entry.sha256).as_bytes());
            prev = Some(entry.sha256.clone());
            out.push(entry);
        }
        if self.expected != u64::MAX && out.len() as u64 != self.expected {
            return Err(de::Error::custom("bundle record count mismatch"));
        }
        Ok(out)
    }
}

struct CappedFieldSetVecSeed<'a> {
    max: usize,
    expected: u64,
    record_shas: &'a [BundleRecordEntry],
    builder_sets: &'a BTreeMap<String, BundleFieldSetEntry>,
    content: &'a RefCell<Sha256>,
}

impl<'de, 'a> DeserializeSeed<'de> for CappedFieldSetVecSeed<'a> {
    type Value = Vec<BundleFieldSetEntry>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, 'a> Visitor<'de> for CappedFieldSetVecSeed<'a> {
    type Value = Vec<BundleFieldSetEntry>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("field_sets array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        let mut prev: Option<String> = None;
        while let Some(entry) = seq.next_element::<BundleFieldSetEntry>()? {
            if out.len() >= self.max {
                return Err(de::Error::custom(format!(
                    "field_sets exceeded hard cap {}",
                    self.max
                )));
            }
            if !is_sha256_hex(&entry.sha256) {
                return Err(de::Error::custom("field_set sha invalid"));
            }
            if prev
                .as_ref()
                .is_some_and(|p| entry.sha256.as_str() <= p.as_str())
            {
                return Err(de::Error::custom("field_sets not strictly sorted/unique"));
            }
            let (sha, _) = hash_field_set_body(&entry.fields)
                .map_err(|e| de::Error::custom(format!("{e:?}")))?;
            if sha != entry.sha256 {
                return Err(de::Error::custom(format!(
                    "field-set hash mismatch {}",
                    entry.sha256
                )));
            }
            validate_field_set_slots_in_sorted_records(&entry, self.record_shas)
                .map_err(|e| de::Error::custom(format!("{e:?}")))?;
            if !self.builder_sets.is_empty() && !self.builder_sets.contains_key(&entry.sha256) {
                return Err(de::Error::custom("field_set not in builder dict"));
            }
            self.content
                .borrow_mut()
                .update(format!("field_set={}\n", entry.sha256).as_bytes());
            prev = Some(entry.sha256.clone());
            out.push(entry);
        }
        if self.expected != u64::MAX && out.len() as u64 != self.expected {
            return Err(de::Error::custom("bundle field_set count mismatch"));
        }
        Ok(out)
    }
}

struct SamplesStreamingSeed<'a> {
    field_set_keys: &'a std::collections::BTreeSet<&'a str>,
    expected_count: u64,
    content: &'a RefCell<Sha256>,
}

impl<'de, 'a> DeserializeSeed<'de> for SamplesStreamingSeed<'a> {
    type Value = u64;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, 'a> Visitor<'de> for SamplesStreamingSeed<'a> {
    type Value = u64;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("samples array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<u64, A::Error> {
        let mut count = 0u64;
        let mut prev_key: Option<(i64, i64)> = None;
        while let Some(sample) = seq.next_element::<BundleSampleAssignment>()? {
            if sample.particle_id < 0 || sample.sample_sequence < 0 {
                return Err(de::Error::custom("negative sample key"));
            }
            if !is_sha256_hex(&sample.field_set_sha256) {
                return Err(de::Error::custom("sample field_set sha invalid"));
            }
            if !self
                .field_set_keys
                .contains(sample.field_set_sha256.as_str())
            {
                return Err(de::Error::custom("sample missing field_set ref"));
            }
            let key = (sample.particle_id, sample.sample_sequence);
            if prev_key.is_some_and(|p| key <= p) {
                return Err(de::Error::custom("samples not strictly sorted/unique"));
            }
            prev_key = Some(key);
            self.content.borrow_mut().update(
                format!(
                    "sample={},{},{}\n",
                    sample.particle_id, sample.sample_sequence, sample.field_set_sha256
                )
                .as_bytes(),
            );
            count = count
                .checked_add(1)
                .ok_or_else(|| de::Error::custom("sample count overflow"))?;
        }
        if self.expected_count != u64::MAX && count != self.expected_count {
            return Err(de::Error::custom(format!(
                "sample count mismatch expected={} got={count}",
                self.expected_count
            )));
        }
        Ok(count)
    }
}

// One-shot quarantine rename fault (default off). Production never arms this.
std::thread_local! {
    static QUARANTINE_RENAME_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Arm a one-shot quarantine rename failure for harness tests. Production default: off.
pub fn arm_quarantine_rename_fault() {
    QUARANTINE_RENAME_FAULT.with(|c| c.set(true));
}

/// Clear quarantine rename fault.
pub fn clear_quarantine_rename_fault() {
    QUARANTINE_RENAME_FAULT.with(|c| c.set(false));
}

/// Rename formal bundle to a unique forensic name; never leave formal path behind.
pub fn quarantine_path(path: &Path, run_dir: &Path) -> Result<(), OutputError> {
    if !path.exists() {
        return Ok(());
    }
    let armed = QUARANTINE_RENAME_FAULT.with(|c| {
        let v = c.get();
        if v {
            c.set(false);
        }
        v
    });
    if armed {
        return Err(OutputError::Io(
            "forensic rename: fault-injected quarantine rename failure".into(),
        ));
    }
    for n in 0..1000u32 {
        let name = if n == 0 {
            format!("{PROVENANCE_BUNDLE_FILE_NAME}.forensic-aborted")
        } else {
            format!("{PROVENANCE_BUNDLE_FILE_NAME}.forensic-aborted.{n}")
        };
        let dest = run_dir.join(name);
        if dest.exists() {
            continue;
        }
        fs::rename(path, &dest).map_err(|e| OutputError::Io(format!("forensic rename: {e}")))?;
        if path.exists() {
            return Err(OutputError::Io(
                "formal bundle still present after forensic rename".into(),
            ));
        }
        return Ok(());
    }
    Err(OutputError::Io(
        "forensic rename exhausted unique names".into(),
    ))
}

/// Quarantine formal bundle in a run directory (sink-level helper).
pub fn quarantine_bundle_file(run_dir: &Path) -> Result<(), OutputError> {
    quarantine_path(&run_dir.join(PROVENANCE_BUNDLE_FILE_NAME), run_dir)
}

fn validate_record_body_semantics(body: &BundleRecordBody) -> Result<(), OutputError> {
    if body.sources.is_empty() {
        return Err(OutputError::Encoding("record sources empty".into()));
    }
    let mut seen = std::collections::BTreeSet::new();
    for s in &body.sources {
        if s.is_empty() {
            return Err(OutputError::Encoding("empty source".into()));
        }
        if !seen.insert(s.clone()) {
            return Err(OutputError::Encoding(format!("duplicate source {s}")));
        }
    }
    if let Some(fb) = &body.fallback_reason {
        if fb.is_empty() {
            return Err(OutputError::Encoding("empty fallback_reason".into()));
        }
    }
    for step in &body.transforms {
        if step.operation.is_empty() {
            return Err(OutputError::Encoding("empty operation".into()));
        }
        let mut prev_name: Option<&str> = None;
        let mut names = std::collections::BTreeSet::new();
        for p in &step.parameters {
            if p.name.is_empty() {
                return Err(OutputError::Encoding("empty parameter name".into()));
            }
            if !names.insert(p.name.clone()) {
                return Err(OutputError::Encoding(format!(
                    "duplicate parameter {}",
                    p.name
                )));
            }
            if prev_name.is_some_and(|n| p.name.as_str() < n) {
                return Err(OutputError::Encoding(
                    "parameters not sorted by name".into(),
                ));
            }
            prev_name = Some(&p.name);
        }
    }
    if !is_sha256_hex(&body.profile_sha256) {
        return Err(OutputError::Encoding("profile_sha256 invalid".into()));
    }
    Ok(())
}

/// Normalized provenance content digest (excludes run_id / file SHAs).
pub fn normalized_provenance_content_digest_from_parts(
    records: &[BundleRecordEntry],
    field_sets: &[BundleFieldSetEntry],
    sorted_run: &Path,
) -> Result<String, OutputError> {
    let mut hasher = Sha256::new();
    hasher.update(format!("{PROVENANCE_CONTENT_DIGEST_ID}\n").as_bytes());
    hasher.update(b"record_hash_algorithm=sha256-rfc8785\n");
    for r in records {
        hasher.update(format!("record={}\n", r.sha256).as_bytes());
    }
    for f in field_sets {
        hasher.update(format!("field_set={}\n", f.sha256).as_bytes());
    }
    if sorted_run.exists() {
        let f = File::open(sorted_run).map_err(|e| OutputError::Io(e.to_string()))?;
        for (i, line) in BufReader::new(f).lines().enumerate() {
            let line = line.map_err(|e| OutputError::Io(e.to_string()))?;
            if line.is_empty() {
                continue;
            }
            let s = parse_sample_line(&line, i + 1)?;
            hasher.update(
                format!(
                    "sample={},{},{}\n",
                    s.particle_id, s.sample_sequence, s.field_set_sha256
                )
                .as_bytes(),
            );
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Canonical output digest = SQL digest + normalized provenance content digest.
pub fn canonical_output_digest(
    sqlite_sql_sha256: &str,
    provenance_content_sha256: &str,
) -> Result<String, OutputError> {
    if !is_sha256_hex(sqlite_sql_sha256) || !is_sha256_hex(provenance_content_sha256) {
        return Err(OutputError::Encoding(
            "canonical_output_digest inputs must be lowercase 64-hex".into(),
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(format!("{CANONICAL_OUTPUT_DIGEST_ID}\n").as_bytes());
    hasher.update(format!("sqlite_sql_sha256={sqlite_sql_sha256}\n").as_bytes());
    hasher.update(format!("provenance_content_sha256={provenance_content_sha256}\n").as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Maps a field key to one of the five frozen particle-state slots.
pub fn five_field_slot(key: &FieldKey) -> Option<&'static str> {
    match key {
        FieldKey::Canonical(CanonicalField::EastwardWind) => Some(SLOT_EASTWARD),
        FieldKey::Canonical(CanonicalField::NorthwardWind) => Some(SLOT_NORTHWARD),
        FieldKey::Canonical(CanonicalField::GeometricVerticalVelocity) => Some(SLOT_VERTICAL),
        FieldKey::Canonical(CanonicalField::AirPressure) => Some(SLOT_PRESSURE),
        FieldKey::Canonical(CanonicalField::AirTemperature) => Some(SLOT_TEMPERATURE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use trajecta_met::field::FieldKey;
    use trajecta_met::provenance::TransformRecord;

    fn rec(field: CanonicalField, op: &str) -> ProvenanceRecord {
        ProvenanceRecord {
            field: FieldKey::Canonical(field),
            quality: FieldQuality::Source,
            sources: vec!["content-sha256:aa".into()],
            transforms: if op.is_empty() {
                Vec::new()
            } else {
                vec![TransformRecord {
                    operation: op.into(),
                    parameters: vec![("b".into(), "2".into()), ("a".into(), "1".into())],
                }]
            },
            fallback_reason: None,
            profile_sha256: "ff".repeat(32),
        }
    }

    #[test]
    fn transform_parameters_sorted_and_hashed_deterministically() {
        let body =
            bundle_record_body_from_runtime(&rec(CanonicalField::EastwardWind, "scale")).unwrap();
        assert_eq!(body.transforms[0].parameters[0].name, "a");
        assert_eq!(body.transforms[0].parameters[1].name, "b");
        let (sha1, c1) = hash_record_body(&body).unwrap();
        let (sha2, c2) = hash_record_body(&body).unwrap();
        assert_eq!(sha1, sha2);
        assert_eq!(c1, c2);
        assert!(!c1.contains(' '));
    }

    #[test]
    fn digesting_file_buffer_preserves_bytes_and_sha_across_capacity_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("digesting-buffer.bin");
        let chunks = [
            vec![b'a'; 17],
            vec![b'b'; PROVENANCE_BUNDLE_WRITE_BUFFER_BYTES - 17],
            vec![b'c'; PROVENANCE_BUNDLE_WRITE_BUFFER_BYTES + 31],
            b"tail\n".to_vec(),
        ];
        let expected: Vec<u8> = chunks.iter().flatten().copied().collect();
        let expected_sha = hex::encode(Sha256::digest(&expected));

        let mut output = DigestingFile::create(&path).unwrap();
        for chunk in &chunks {
            output.write_all(chunk).unwrap();
        }
        output.sync_all().unwrap();
        let observed_sha = output.finalize_sha256();

        assert_eq!(std::fs::read(&path).unwrap(), expected);
        assert_eq!(observed_sha, expected_sha);
        assert_eq!(file_sha256(&path).unwrap(), expected_sha);
    }

    #[test]
    fn field_key_token_is_snake_case_not_debug() {
        let token =
            FieldKeyToken::from_field_key(&FieldKey::Canonical(CanonicalField::AirPressure))
                .unwrap();
        match token {
            FieldKeyToken::Canonical(s) => assert_eq!(s, "air_pressure"),
            FieldKeyToken::Extension { .. } => unreachable!("expected canonical"),
        }
    }

    #[test]
    fn record_sha_collision_different_bytes_hard_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mut b =
            ProvenanceBundleBuilder::begin(dir.path(), "018f0000-0000-7000-8000-000000000001")
                .unwrap();
        // Manually inject collision.
        b.record_canonical_by_sha
            .insert("aa".repeat(32), "{\"a\":1}".into());
        // Force intern path by hacking — use push with normal record then overwrite map.
        // Instead call intern through public push after poisoning sha map with wrong bytes
        // for a sha we will recompute — simpler: unit-level check of collision detect.
        let body = bundle_record_body_from_runtime(&rec(CanonicalField::EastwardWind, "")).unwrap();
        let (sha, canonical) = hash_record_body(&body).unwrap();
        b.record_canonical_by_sha
            .insert(sha.clone(), "not-the-same".into());
        b.records_by_sha.insert(
            sha.clone(),
            BundleRecordEntry {
                sha256: sha,
                record: body,
            },
        );
        let err = b
            .push_sample(
                1,
                0,
                &FiveFieldRecords {
                    eastward_wind: Some(rec(CanonicalField::EastwardWind, "")),
                    ..FiveFieldRecords::default()
                },
            )
            .unwrap_err();
        assert!(format!("{err:?}").contains("collision"), "{err:?}");
        let _ = canonical;
    }

    #[test]
    fn duplicate_transform_parameter_rejected() {
        let mut record = rec(CanonicalField::EastwardWind, "x");
        record.transforms[0].parameters = vec![("a".into(), "1".into()), ("a".into(), "1".into())];
        let err = bundle_record_body_from_runtime(&record).unwrap_err();
        assert!(format!("{err:?}").contains("duplicate"), "{err:?}");
    }

    fn five_fields() -> FiveFieldRecords {
        FiveFieldRecords {
            eastward_wind: Some(rec(CanonicalField::EastwardWind, "u")),
            northward_wind: Some(rec(CanonicalField::NorthwardWind, "v")),
            geometric_vertical_velocity: Some(rec(CanonicalField::GeometricVerticalVelocity, "w")),
            air_pressure: Some(rec(CanonicalField::AirPressure, "p")),
            air_temperature: Some(rec(CanonicalField::AirTemperature, "t")),
        }
    }

    fn write_mini_sqlite(path: &std::path::Path, run_id: &str, keys: &[(i64, i64)]) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE particle_state (
               run_id TEXT NOT NULL,
               particle_id INTEGER NOT NULL,
               sample_sequence INTEGER NOT NULL,
               eastward_wind_m_s REAL,
               northward_wind_m_s REAL,
               geometric_vertical_velocity_m_s REAL,
               air_pressure_pa REAL,
               air_temperature_k REAL,
               wind_quality TEXT,
               pressure_quality TEXT,
               temperature_quality TEXT,
               PRIMARY KEY (run_id, particle_id, sample_sequence)
             ) WITHOUT ROWID;",
        )
        .unwrap();
        for (pid, seq) in keys {
            conn.execute(
                "INSERT INTO particle_state VALUES (?1,?2,?3,1.0,2.0,0.0,1000.0,280.0,'source','source','source')",
                rusqlite::params![run_id, pid, seq],
            )
            .unwrap();
        }
    }

    #[test]
    fn lockstep_scan_is_run_scoped_and_uses_primary_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let sqlite_path = dir.path().join("particles.sqlite");
        let mut builder =
            ProvenanceBundleBuilder::begin(dir.path(), "018f0000-0000-7000-8000-0000000000a0")
                .unwrap();
        builder.push_sample(1, 0, &five_fields()).unwrap();
        write_mini_sqlite(&sqlite_path, &builder.run_id, &[(1, 0)]);

        let connection = Connection::open(&sqlite_path).unwrap();
        connection
            .execute(
                "INSERT INTO particle_state VALUES
                 ('foreign-run',2,0,1.0,2.0,0.0,1000.0,280.0,'source','source','source')",
                [],
            )
            .unwrap();
        let explain_sql = format!("EXPLAIN QUERY PLAN {LOCKSTEP_PARTICLE_STATE_SQL}");
        let details = connection
            .prepare(&explain_sql)
            .unwrap()
            .query_map([builder.run_id.as_str()], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(details.iter().any(|detail| detail.contains("PRIMARY KEY")));
        assert!(
            details.iter().all(|detail| !detail.contains("TEMP B-TREE")),
            "{details:?}"
        );
        drop(connection);

        let sqlite_sha = file_sha256(&sqlite_path).unwrap();
        let identity = builder
            .finalize_after_sqlite(&sqlite_path, &sqlite_sha, &"ab".repeat(32))
            .unwrap();
        assert_eq!(identity.sample_count, 1);
    }

    #[test]
    fn external_sort_multi_round_and_reverse_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = ProvenanceBundleBuilder::begin_with_limits(
            dir.path(),
            "018f0000-0000-7000-8000-0000000000aa",
            TEST_SAMPLE_CHUNK_LINES,
            TEST_MERGE_FAN_IN,
        )
        .unwrap();
        // Reverse order keys to force sort; > chunk*fan rounds.
        let mut keys = Vec::new();
        for pid in (0..12i64).rev() {
            keys.push((pid, 0i64));
            b.push_sample(pid, 0, &five_fields()).unwrap();
        }
        write_mini_sqlite(&dir.path().join("particles.sqlite"), &b.run_id, &keys);
        let sha = file_sha256(&dir.path().join("particles.sqlite")).unwrap();
        let id = b
            .finalize_after_sqlite(&dir.path().join("particles.sqlite"), &sha, &"ab".repeat(32))
            .unwrap();
        assert_eq!(id.sample_count, 12);
        assert_eq!(id.content_sha256.len(), 64);
        assert_eq!(id.canonical_output_sha256.len(), 64);
        assert_eq!(id.sqlite_sql_sha256, "ab".repeat(32));
        assert!(dir.path().join("provenance-bundle.json").is_file());
        // spool/runs cleaned
        assert!(
            !dir.path()
                .join("provenance-bundle.json.samples.spool")
                .exists()
        );
        let doc: ProvenanceBundleDocument = serde_json::from_slice(
            &std::fs::read(dir.path().join("provenance-bundle.json")).unwrap(),
        )
        .unwrap();
        let mut prev = (-1i64, -1i64);
        for s in &doc.samples {
            assert!((s.particle_id, s.sample_sequence) > prev);
            prev = (s.particle_id, s.sample_sequence);
        }
        assert!(b.last_content_digest.as_ref().unwrap().len() == 64);
    }

    #[test]
    fn duplicate_sample_key_across_chunks_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = ProvenanceBundleBuilder::begin_with_limits(
            dir.path(),
            "018f0000-0000-7000-8000-0000000000bb",
            TEST_SAMPLE_CHUNK_LINES,
            TEST_MERGE_FAN_IN,
        )
        .unwrap();
        let f = five_fields();
        b.push_sample(1, 0, &f).unwrap();
        b.push_sample(2, 0, &f).unwrap();
        b.push_sample(1, 0, &f).unwrap(); // dup
        write_mini_sqlite(
            &dir.path().join("particles.sqlite"),
            &b.run_id,
            &[(1, 0), (2, 0)],
        );
        let sha = file_sha256(&dir.path().join("particles.sqlite")).unwrap();
        let err = b
            .finalize_after_sqlite(&dir.path().join("particles.sqlite"), &sha, &"ab".repeat(32))
            .unwrap_err();
        assert!(format!("{err:?}").contains("duplicate"), "{err:?}");
    }

    #[test]
    fn sqlite_missing_row_fails_lockstep() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = ProvenanceBundleBuilder::begin_with_limits(
            dir.path(),
            "018f0000-0000-7000-8000-0000000000cc",
            TEST_SAMPLE_CHUNK_LINES,
            TEST_MERGE_FAN_IN,
        )
        .unwrap();
        let f = five_fields();
        b.push_sample(1, 0, &f).unwrap();
        b.push_sample(2, 0, &f).unwrap();
        // SQLite only has one row
        write_mini_sqlite(&dir.path().join("particles.sqlite"), &b.run_id, &[(1, 0)]);
        let sha = file_sha256(&dir.path().join("particles.sqlite")).unwrap();
        let err = b
            .finalize_after_sqlite(&dir.path().join("particles.sqlite"), &sha, &"ab".repeat(32))
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("missing") || msg.contains("not present") || msg.contains("mismatch"),
            "{msg}"
        );
    }

    #[test]
    fn fault_before_rename_leaves_no_final_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let mut b =
            ProvenanceBundleBuilder::begin(dir.path(), "018f0000-0000-7000-8000-0000000000dd")
                .unwrap();
        b.inject_fault(BundleFaultInject::BeforeRename);
        b.push_sample(1, 0, &five_fields()).unwrap();
        write_mini_sqlite(&dir.path().join("particles.sqlite"), &b.run_id, &[(1, 0)]);
        let sha = file_sha256(&dir.path().join("particles.sqlite")).unwrap();
        let err = b
            .finalize_after_sqlite(&dir.path().join("particles.sqlite"), &sha, &"ab".repeat(32))
            .unwrap_err();
        assert!(format!("{err:?}").contains("BeforeRename"), "{err:?}");
        assert!(!dir.path().join("provenance-bundle.json").exists());
        assert!(!dir.path().join("provenance-bundle.json.tmp").exists());
    }

    #[test]
    fn normalized_digest_stable_across_run_ids() {
        let mk = |run_id: &str, dir: &std::path::Path| {
            let mut b = ProvenanceBundleBuilder::begin(dir, run_id).unwrap();
            b.push_sample(1, 0, &five_fields()).unwrap();
            write_mini_sqlite(&dir.join("particles.sqlite"), &b.run_id, &[(1, 0)]);
            let sha = file_sha256(&dir.join("particles.sqlite")).unwrap();
            let id = b
                .finalize_after_sqlite(&dir.join("particles.sqlite"), &sha, &"ab".repeat(32))
                .unwrap();
            (id.sha256, id.content_sha256, id.canonical_output_sha256)
        };
        let d1 = tempfile::tempdir().unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let (exact1, c1, out1) = mk("018f0000-0000-7000-8000-0000000000e1", d1.path());
        let (exact2, c2, out2) = mk("018f0000-0000-7000-8000-0000000000e2", d2.path());
        assert_ne!(exact1, exact2, "exact bundle SHA must differ with run_id");
        assert_eq!(c1, c2, "normalized content digest must match");
        assert_eq!(
            out1, out2,
            "canonical output must match for same SQL digest"
        );
        let out3 = canonical_output_digest(&("cd".repeat(32)), &c1).unwrap();
        assert_ne!(out1, out3);
    }

    fn write_valid_bundle_via_builder(dir: &std::path::Path) -> (String, BundleValidationSummary) {
        let mut b =
            ProvenanceBundleBuilder::begin(dir, "018f0000-0000-7000-8000-0000000000ff").unwrap();
        b.push_sample(1, 0, &five_fields()).unwrap();
        write_mini_sqlite(&dir.join("particles.sqlite"), &b.run_id, &[(1, 0)]);
        let sha = file_sha256(&dir.join("particles.sqlite")).unwrap();
        let id = b
            .finalize_after_sqlite(&dir.join("particles.sqlite"), &sha, &"ab".repeat(32))
            .unwrap();
        let summary = validate_bundle_file_semantics_loose(
            &dir.join("provenance-bundle.json"),
            "018f0000-0000-7000-8000-0000000000ff",
            &sha,
        )
        .unwrap();
        assert_eq!(summary.content_sha256, id.content_sha256);
        (sha, summary)
    }

    #[test]
    fn streaming_validator_rejects_trailing_json_value() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, _) = write_valid_bundle_via_builder(dir.path());
        let path = dir.path().join("provenance-bundle.json");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"\n{}\n");
        std::fs::write(&path, &bytes).unwrap();
        let err = validate_bundle_file_semantics_loose(
            &path,
            "018f0000-0000-7000-8000-0000000000ff",
            &sha,
        )
        .unwrap_err();
        assert!(
            format!("{err:?}").contains("trailing"),
            "expected trailing reject: {err:?}"
        );
    }

    #[test]
    fn streaming_validator_rejects_duplicate_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, _) = write_valid_bundle_via_builder(dir.path());
        let path = dir.path().join("provenance-bundle.json");
        let text = std::fs::read_to_string(&path).unwrap();
        // Inject a second schema_version after the first occurrence.
        let injected = text.replacen(
            "\"schema_version\": \"trajecta.provenance-bundle/v1\",",
            "\"schema_version\": \"trajecta.provenance-bundle/v1\",\n  \"schema_version\": \"trajecta.provenance-bundle/v1\",",
            1,
        );
        std::fs::write(&path, injected).unwrap();
        let err = validate_bundle_file_semantics_loose(
            &path,
            "018f0000-0000-7000-8000-0000000000ff",
            &sha,
        )
        .unwrap_err();
        assert!(
            format!("{err:?}").contains("duplicate"),
            "expected duplicate reject: {err:?}"
        );
    }

    #[test]
    fn streaming_validator_rejects_trailing_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, _) = write_valid_bundle_via_builder(dir.path());
        let path = dir.path().join("provenance-bundle.json");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"NOT-JSON");
        std::fs::write(&path, &bytes).unwrap();
        let err = validate_bundle_file_semantics_loose(
            &path,
            "018f0000-0000-7000-8000-0000000000ff",
            &sha,
        )
        .unwrap_err();
        assert!(
            format!("{err:?}").contains("trailing") || format!("{err:?}").contains("stream"),
            "{err:?}"
        );
    }

    #[test]
    fn quarantine_chooses_unique_forensic_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let formal = dir.path().join(PROVENANCE_BUNDLE_FILE_NAME);
        std::fs::write(&formal, b"{}").unwrap();
        let pre = dir
            .path()
            .join(format!("{PROVENANCE_BUNDLE_FILE_NAME}.forensic-aborted"));
        std::fs::write(&pre, b"old").unwrap();
        quarantine_path(&formal, dir.path()).unwrap();
        assert!(!formal.exists());
        assert!(
            dir.path()
                .join(format!("{PROVENANCE_BUNDLE_FILE_NAME}.forensic-aborted.1"))
                .exists()
        );
    }

    #[test]
    fn streaming_validator_table_driven_valid_duplicates_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, _) = write_valid_bundle_via_builder(dir.path());
        let path = dir.path().join("provenance-bundle.json");
        let base = std::fs::read_to_string(&path).unwrap();
        let run_id = "018f0000-0000-7000-8000-0000000000ff";

        // Valid-value duplicates (not null). Each insertion reuses the first field's
        // own on-disk text so the first parse succeeds and the second hits duplicate.
        let cases: &[(&str, &str)] = &[
            (
                "schema_version",
                "\"schema_version\": \"trajecta.provenance-bundle/v1\",",
            ),
            (
                "run_id",
                "\"run_id\": \"018f0000-0000-7000-8000-0000000000ff\",",
            ),
            (
                "record_hash_algorithm",
                "\"record_hash_algorithm\": \"sha256-rfc8785\",",
            ),
        ];
        for (key, line) in cases {
            let injected = base.replacen(line, &format!("{line}\n  {line}"), 1);
            assert_ne!(injected, base, "failed to inject duplicate {key}");
            std::fs::write(&path, &injected).unwrap();
            let err = validate_bundle_file_semantics_loose(&path, run_id, &sha).unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains(&format!("duplicate {key}")),
                "key={key} err={msg}"
            );
        }

        // Object/array fields: splice a second copy immediately after the first value.
        for key in ["sqlite", "records", "field_sets", "samples"] {
            let needle = format!("\"{key}\":");
            let key_at = base.find(&needle).expect(key);
            let after_key = key_at + needle.len();
            let rest = &base[after_key..];
            let value = scan_json_value(rest);
            let value_rel = value.as_ptr() as usize - rest.as_ptr() as usize;
            let value_end = after_key + value_rel + value.len();
            let fragment = format!(",{needle}{value}");
            let mut injected = base.clone();
            injected.insert_str(value_end, &fragment);
            std::fs::write(&path, &injected).unwrap();
            let err = validate_bundle_file_semantics_loose(&path, run_id, &sha).unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains(&format!("duplicate {key}")),
                "key={key} err={msg}"
            );
        }

        std::fs::write(&path, &base[..base.len() / 3]).unwrap();
        assert!(validate_bundle_file_semantics_loose(&path, run_id, &sha).is_err());

        let with_unknown = base.replacen('{', "{\"extra\": true,", 1);
        std::fs::write(&path, with_unknown).unwrap();
        let err = validate_bundle_file_semantics_loose(&path, run_id, &sha).unwrap_err();
        assert!(format!("{err:?}").contains("unknown"), "{err:?}");
    }

    #[test]
    fn streaming_validator_rejects_missing_required_top_level_fields() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, _) = write_valid_bundle_via_builder(dir.path());
        let path = dir.path().join("provenance-bundle.json");
        let base = std::fs::read_to_string(&path).unwrap();
        let run_id = "018f0000-0000-7000-8000-0000000000ff";
        for key in [
            "schema_version",
            "run_id",
            "sqlite",
            "record_hash_algorithm",
            "records",
            "field_sets",
            "samples",
        ] {
            let needle = format!("\"{key}\":");
            let removed = base.replacen(&needle, "\"__removed_field__\":", 1);
            std::fs::write(&path, removed).unwrap();
            let err = validate_bundle_file_semantics_loose(&path, run_id, &sha).unwrap_err();
            let msg = format!("{err:?}");
            assert!(
                msg.contains(&format!("missing {key}"))
                    || msg.contains("unknown")
                    || msg.contains("missing"),
                "missing {key}: {msg}"
            );
        }
    }

    #[test]
    fn streaming_validator_rejects_records_and_field_sets_cap_plus_one() {
        let dir = tempfile::tempdir().unwrap();
        let (sha, _) = write_valid_bundle_via_builder(dir.path());
        let path = dir.path().join("provenance-bundle.json");
        let run_id = "018f0000-0000-7000-8000-0000000000ff";

        fn write_bundle_header(f: &mut impl std::io::Write, run_id: &str, sha: &str) {
            writeln!(f, "{{").unwrap();
            writeln!(
                f,
                "  \"schema_version\": \"trajecta.provenance-bundle/v1\","
            )
            .unwrap();
            writeln!(f, "  \"run_id\": \"{run_id}\",").unwrap();
            writeln!(f, "  \"sqlite\": {{").unwrap();
            writeln!(f, "    \"relative_path\": \"particles.sqlite\",").unwrap();
            writeln!(f, "    \"schema_version\": 1,").unwrap();
            writeln!(f, "    \"sha256\": \"{sha}\"").unwrap();
            writeln!(f, "  }},").unwrap();
            writeln!(f, "  \"record_hash_algorithm\": \"sha256-rfc8785\",").unwrap();
        }

        // --- records cap + 1 ---
        {
            let mut entries = Vec::with_capacity(MAX_UNIQUE_RECORDS + 1);
            for i in 0..=MAX_UNIQUE_RECORDS {
                let body = BundleRecordBody {
                    field: FieldKeyToken::Canonical("eastward_wind".into()),
                    quality: "exact".into(),
                    sources: vec![format!("src-{i:05}")],
                    transforms: Vec::new(),
                    fallback_reason: None,
                    profile_sha256: "aa".repeat(32),
                };
                let (entry_sha, _) = hash_record_body(&body).unwrap();
                entries.push(BundleRecordEntry {
                    sha256: entry_sha,
                    record: body,
                });
            }
            entries.sort_by(|a, b| a.sha256.cmp(&b.sha256));
            let mut f = std::fs::File::create(&path).unwrap();
            write_bundle_header(&mut f, run_id, &sha);
            writeln!(f, "  \"records\": [").unwrap();
            for (i, rec) in entries.iter().enumerate() {
                if i > 0 {
                    write!(f, ",").unwrap();
                }
                writeln!(f).unwrap();
                write!(f, "    {}", serde_json::to_string(rec).unwrap()).unwrap();
            }
            writeln!(f).unwrap();
            writeln!(f, "  ],").unwrap();
            writeln!(f, "  \"field_sets\": [],").unwrap();
            writeln!(f, "  \"samples\": []").unwrap();
            writeln!(f, "}}").unwrap();
        }
        let err = validate_bundle_file_semantics_loose(&path, run_id, &sha).unwrap_err();
        assert!(
            format!("{err:?}").contains("records exceeded hard cap"),
            "records cap+1: {err:?}"
        );

        // --- field_sets cap + 1 (records stay under cap) ---
        {
            // Keep records at MAX: (MAX-1) eastward + 1 northward.
            // field_sets: (MAX-1) one-slot + 2 multi-slot = MAX+1.
            let east_n = MAX_UNIQUE_FIELD_SETS - 1;
            let mut records = Vec::with_capacity(east_n + 1);
            let mut field_sets = Vec::with_capacity(MAX_UNIQUE_FIELD_SETS + 1);
            let mut east_shas = Vec::with_capacity(east_n);
            for i in 0..east_n {
                let body = BundleRecordBody {
                    field: FieldKeyToken::Canonical("eastward_wind".into()),
                    quality: "exact".into(),
                    sources: vec![format!("cap-src-{i:05}")],
                    transforms: Vec::new(),
                    fallback_reason: None,
                    profile_sha256: "aa".repeat(32),
                };
                let (rsha, _) = hash_record_body(&body).unwrap();
                east_shas.push(rsha.clone());
                records.push(BundleRecordEntry {
                    sha256: rsha.clone(),
                    record: body,
                });
                let fs_body = BundleFieldSetBody {
                    eastward_wind: Some(rsha),
                    northward_wind: None,
                    geometric_vertical_velocity: None,
                    air_pressure: None,
                    air_temperature: None,
                };
                let (fsha, _) = hash_field_set_body(&fs_body).unwrap();
                field_sets.push(BundleFieldSetEntry {
                    sha256: fsha,
                    fields: fs_body,
                });
            }
            let north_body = BundleRecordBody {
                field: FieldKeyToken::Canonical("northward_wind".into()),
                quality: "exact".into(),
                sources: vec!["cap-north".into()],
                transforms: Vec::new(),
                fallback_reason: None,
                profile_sha256: "aa".repeat(32),
            };
            let (nsha, _) = hash_record_body(&north_body).unwrap();
            records.push(BundleRecordEntry {
                sha256: nsha.clone(),
                record: north_body,
            });
            for e in east_shas.iter().take(2) {
                let extra = BundleFieldSetBody {
                    eastward_wind: Some(e.clone()),
                    northward_wind: Some(nsha.clone()),
                    geometric_vertical_velocity: None,
                    air_pressure: None,
                    air_temperature: None,
                };
                let (fsha, _) = hash_field_set_body(&extra).unwrap();
                field_sets.push(BundleFieldSetEntry {
                    sha256: fsha,
                    fields: extra,
                });
            }
            assert_eq!(field_sets.len(), MAX_UNIQUE_FIELD_SETS + 1);
            assert!(records.len() <= MAX_UNIQUE_RECORDS);
            records.sort_by(|a, b| a.sha256.cmp(&b.sha256));
            field_sets.sort_by(|a, b| a.sha256.cmp(&b.sha256));

            let mut f = std::fs::File::create(&path).unwrap();
            write_bundle_header(&mut f, run_id, &sha);
            writeln!(f, "  \"records\": [").unwrap();
            for (i, rec) in records.iter().enumerate() {
                if i > 0 {
                    write!(f, ",").unwrap();
                }
                writeln!(f).unwrap();
                write!(f, "    {}", serde_json::to_string(rec).unwrap()).unwrap();
            }
            writeln!(f).unwrap();
            writeln!(f, "  ],").unwrap();
            writeln!(f, "  \"field_sets\": [").unwrap();
            for (i, fs) in field_sets.iter().enumerate() {
                if i > 0 {
                    write!(f, ",").unwrap();
                }
                writeln!(f).unwrap();
                write!(f, "    {}", serde_json::to_string(fs).unwrap()).unwrap();
            }
            writeln!(f).unwrap();
            writeln!(f, "  ],").unwrap();
            writeln!(f, "  \"samples\": []").unwrap();
            writeln!(f, "}}").unwrap();
        }
        let err = validate_bundle_file_semantics_loose(&path, run_id, &sha).unwrap_err();
        assert!(
            format!("{err:?}").contains("field_sets exceeded hard cap"),
            "field_sets cap+1: {err:?}"
        );
    }

    /// Scan one JSON value starting at `s` (leading whitespace allowed); return the slice
    /// covering that value including nested structures/strings.
    fn scan_json_value(s: &str) -> &str {
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        match b.get(i) {
            Some(b'"') => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            Some(b'{') | Some(b'[') => {
                let open = b[i];
                let close = if open == b'{' { b'}' } else { b']' };
                let mut depth = 0i32;
                while i < b.len() {
                    let c = b[i];
                    if c == b'"' {
                        i += 1;
                        while i < b.len() {
                            if b[i] == b'\\' {
                                i += 2;
                                continue;
                            }
                            if b[i] == b'"' {
                                i += 1;
                                break;
                            }
                            i += 1;
                        }
                        continue;
                    }
                    if c == open {
                        depth += 1;
                    } else if c == close {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    i += 1;
                }
            }
            Some(b't') | Some(b'f') | Some(b'n') => {
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
            }
            Some(c) if c.is_ascii_digit() || *c == b'-' => {
                while i < b.len()
                    && (b[i].is_ascii_digit() || matches!(b[i], b'-' | b'+' | b'.' | b'e' | b'E'))
                {
                    i += 1;
                }
            }
            _ => panic!("cannot scan json value at {}", &s[..s.len().min(40)]),
        }
        &s[start..i]
    }
}
