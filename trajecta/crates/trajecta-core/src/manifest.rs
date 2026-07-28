//! # Contract: reproducible run manifest
//!
//! A manifest records immutable inputs, generated seed, numerical identities,
//! outputs, lifecycle state, and final diagnostics without embedding secrets.
//! The on-disk JSON contract is `trajecta.run-manifest/v1`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::model::output::{OutputProductSpec, OutputSchedule};
use trajecta_case::model::time::Timestamp;

use crate::science::{RUN_MANIFEST_SCHEMA_ID, SQLITE_SCHEMA_VERSION};

/// Stable run identifier, represented as a UUID-v7 string by the runner.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(pub String);

/// Stable logical job-series identifier, represented as a UUID-v7 string.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobSeriesId(pub String);

/// Persistent lifecycle state of a run.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycleStatus {
    /// Run directory exists and execution has not finalized.
    #[default]
    Running,
    /// Run completed without abnormal particle terminations.
    Complete,
    /// Run completed but at least one particle terminated abnormally.
    CompletedWithParticleErrors,
    /// A fatal run-level error prevented completion.
    Failed,
    /// The user requested a safe macro-step-boundary cancellation and outputs finalized.
    Cancelled,
    /// Execution disappeared or was force-stopped before safe output finalization.
    Interrupted,
}

impl RunLifecycleStatus {
    /// Returns whether no further execution transition is permitted.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }

    /// Returns whether this status represents a successful simulation.
    #[must_use]
    pub const fn run_success(self) -> bool {
        matches!(self, Self::Complete)
    }

    /// Returns the frozen foreground-run and `job wait` terminal exit code.
    #[must_use]
    pub const fn wait_exit_code(self) -> Option<i32> {
        match self {
            Self::Running => None,
            Self::Complete => Some(0),
            Self::CompletedWithParticleErrors
            | Self::Failed
            | Self::Cancelled
            | Self::Interrupted => Some(1),
        }
    }
}

/// Software versions and source revision used by a run.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftwareIdentity {
    /// Package versions by crate name.
    pub crate_versions: BTreeMap<String, String>,
    /// Optional Git commit of the Trajecta repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
}

/// Immutable hashes of all portable and machine input documents.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputIdentity {
    /// Resolved Case content hash.
    pub case_sha256: String,
    /// Resolved RunProfile content hash.
    pub run_profile_sha256: String,
    /// Dataset lock hashes by logical dataset ID.
    pub dataset_lock_sha256: BTreeMap<String, String>,
    /// Dataset profile hashes by logical dataset ID.
    pub dataset_profile_sha256: BTreeMap<String, String>,
    /// Frozen content hashes by logical file identity.
    #[serde(default)]
    pub dataset_content_sha256: BTreeMap<String, String>,
}

/// Runtime resources and measured execution statistics.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSummary {
    /// Worker count.
    pub worker_threads: usize,
    /// Hard memory budget in bytes.
    pub memory_budget_bytes: u64,
    /// Stable executor identifier.
    pub executor: String,
    /// Effective reader backend by logical dataset ID.
    pub reader_backends: BTreeMap<String, MeteorologyReaderBackend>,
    /// Total measured wall time in nanoseconds after finalization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_ns: Option<u64>,
    /// Peak resident working set in bytes, when measurable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    /// Frozen I/O counter values by stable counter name.
    #[serde(default)]
    pub io_counters: BTreeMap<String, u64>,
}

/// Numerical algorithms and reproducibility controls.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericalSummary {
    /// Exact declared or generated random seed.
    pub random_seed: u64,
    /// Stable integrator identifier.
    pub integrator: String,
    /// Stable boundary-policy identifiers in application order.
    pub boundary_policies: Vec<String>,
    /// Stable population implementation identifier.
    pub population: String,
    /// Optional named ozone rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ozone_rule: Option<String>,
    /// Stable particle-state sink identifier.
    pub particle_state_sink: String,
    /// Stable tolerance registry identity.
    pub tolerance_registry: String,
    /// Declared numeric tolerances copied into the manifest.
    pub tolerances: BTreeMap<String, f64>,
    /// Whether deterministic execution checks were enabled.
    pub deterministic: bool,
}

/// One resolved and canonicalized release-geometry identity.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeometryIdentity {
    /// Stable release event ID.
    pub event_id: String,
    /// Optional canonical local source path for file-backed GeoJSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
    /// Source byte count for file-backed GeoJSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_size_bytes: Option<u64>,
    /// Source SHA-256 for file-backed GeoJSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    /// SHA-256 of canonical normalized geometry.
    pub canonical_geometry_sha256: String,
    /// Canonical spherical area in square metres; zero for point/line geometry.
    pub spherical_area_m2: f64,
}

/// Public SQLite output contract and measured row counts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqliteOutputSummary {
    /// Public SQLite `user_version`.
    pub schema_version: u32,
    /// Relative path inside the run directory.
    pub relative_path: PathBuf,
    /// Required journal mode.
    pub journal_mode: String,
    /// Required synchronous mode.
    pub synchronous: String,
    /// Final logical rows by public table name.
    #[serde(default)]
    pub row_counts: BTreeMap<String, u64>,
}

impl Default for SqliteOutputSummary {
    fn default() -> Self {
        Self {
            schema_version: SQLITE_SCHEMA_VERSION,
            relative_path: PathBuf::from("particles.sqlite"),
            journal_mode: "WAL".into(),
            synchronous: "NORMAL".into(),
            row_counts: BTreeMap::new(),
        }
    }
}

/// Normal and abnormal particle termination counts.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminationSummary {
    /// Expected normal terminations.
    pub normal_count: u64,
    /// Unexpected abnormal terminations.
    pub abnormal_count: u64,
    /// Counts by stable termination-reason string.
    #[serde(default)]
    pub by_reason: BTreeMap<String, u64>,
}

/// One auditable domain-fill mass ledger record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MassLedgerRecord {
    /// Stable zero-based numerical-step index.
    pub step_index: u64,
    /// Physical time after the ledger transition.
    pub time: Timestamp,
    /// Opening active carrier mass.
    pub opening_active_kg: f64,
    /// Opening residual boundary mass.
    pub opening_residual_kg: f64,
    /// Incoming boundary mass.
    pub incoming_kg: f64,
    /// Carrier mass removed as normal outflow.
    pub outgoing_kg: f64,
    /// Carrier mass removed by other normal termination.
    pub normal_terminated_kg: f64,
    /// Carrier mass removed by abnormal termination.
    pub abnormal_terminated_kg: f64,
    /// Closing active carrier mass.
    pub closing_active_kg: f64,
    /// Closing residual boundary mass.
    pub closing_residual_kg: f64,
    /// Signed ledger residual before tolerance adjudication.
    pub imbalance_kg: f64,
    /// Absolute hard-gate tolerance used for this record.
    pub tolerance_kg: f64,
}

/// Fatal failure retained in a failed manifest.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFailure {
    /// Stable machine-readable error code.
    pub code: String,
    /// Human-readable diagnostic without credentials.
    pub message: String,
}

/// Complete immutable payload required before writing a running manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct RunManifestStart {
    /// Unique run identity.
    pub run_id: RunId,
    /// Sanitized Case name.
    pub case_name: String,
    /// Physical start/creation time.
    pub started_at: Timestamp,
    /// Software identity.
    pub software: SoftwareIdentity,
    /// Immutable input identities.
    pub inputs: InputIdentity,
    /// Resolved execution resources and reader backends.
    pub execution: ExecutionSummary,
    /// Generated/declared seed and all algorithm identities.
    pub numerical: NumericalSummary,
    /// Canonical release geometry identities.
    pub geometries: Vec<GeometryIdentity>,
    /// Effective typed output configuration after default injection.
    pub effective_outputs: Vec<OutputProductSpec>,
}

/// Terminal identity of the formal provenance-bundle/v1 artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceBundleIdentity {
    /// Must equal trajecta.provenance-bundle/v1.
    pub schema_version: String,
    /// Always provenance-bundle.json.
    pub relative_path: String,
    /// SHA-256 of exact on-disk bundle bytes (single-run audit).
    pub sha256: String,
    /// SHA-256 of final particles.sqlite bytes (single-run audit).
    pub sqlite_sha256: String,
    /// Normalized provenance content digest (`trajecta.provenance-content/v1`).
    pub content_sha256: String,
    /// Canonical ordered SQL digest (science columns; excludes run UUID).
    pub sqlite_sql_sha256: String,
    /// Canonical output digest (`trajecta.canonical-output/v1`).
    pub canonical_output_sha256: String,
    /// Unique record count.
    pub record_count: u64,
    /// Unique field-set count.
    pub field_set_count: u64,
    /// Sample assignment count.
    pub sample_count: u64,
}

/// Complete auditable record of one run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunManifest {
    /// Must equal [`RUN_MANIFEST_SCHEMA_ID`].
    pub schema_version: String,
    /// Stable logical series shared by explicit rerun attempts.
    pub job_series_id: JobSeriesId,
    /// One-based attempt number within the logical job series.
    pub attempt: u32,
    /// Unique run identity.
    pub run_id: RunId,
    /// Sanitized human Case name used in the run directory.
    pub case_name: String,
    /// Persistent lifecycle state.
    pub status: RunLifecycleStatus,
    /// Physical manifest creation/start time.
    pub started_at: Timestamp,
    /// Physical finalization time when execution has ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    /// Software identity.
    pub software: SoftwareIdentity,
    /// Immutable input identity.
    pub inputs: InputIdentity,
    /// Runtime execution summary.
    pub execution: ExecutionSummary,
    /// Numerical choices.
    pub numerical: NumericalSummary,
    /// Canonical release geometries.
    #[serde(default)]
    pub geometries: Vec<GeometryIdentity>,
    /// Effective typed output configuration after default injection.
    #[serde(default)]
    pub effective_outputs: Vec<OutputProductSpec>,
    /// SQLite output identity and row counts.
    pub sqlite: SqliteOutputSummary,
    /// Formal provenance-bundle/v1 identity (required for terminal success statuses).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ProvenanceBundleIdentity>,
    /// Particle termination counts.
    pub terminations: TerminationSummary,
    /// Per-step and optional final domain-fill mass records.
    #[serde(default)]
    pub mass_ledger: Vec<MassLedgerRecord>,
    /// Fatal failure details when status is `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
}

impl RunManifest {
    /// Creates a running manifest with frozen schema and SQLite defaults.
    #[must_use]
    pub fn running(start: RunManifestStart) -> Self {
        let job_series_id = JobSeriesId(start.run_id.0.clone());
        Self::running_in_series(start, job_series_id, 1)
    }

    /// Creates a running manifest for an explicit logical job-series attempt.
    #[must_use]
    pub fn running_in_series(
        start: RunManifestStart,
        job_series_id: JobSeriesId,
        attempt: u32,
    ) -> Self {
        Self {
            schema_version: RUN_MANIFEST_SCHEMA_ID.into(),
            job_series_id,
            attempt,
            run_id: start.run_id,
            case_name: start.case_name,
            status: RunLifecycleStatus::Running,
            started_at: start.started_at,
            finished_at: None,
            software: start.software,
            inputs: start.inputs,
            execution: start.execution,
            numerical: start.numerical,
            geometries: start.geometries,
            effective_outputs: start.effective_outputs,
            sqlite: SqliteOutputSummary::default(),
            provenance: None,
            terminations: TerminationSummary::default(),
            mass_ledger: Vec::new(),
            failure: None,
        }
    }

    /// Validates lifecycle coherence and finite non-negative manifest metrics.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != RUN_MANIFEST_SCHEMA_ID {
            return Err(ManifestError::UnsupportedSchema);
        }
        if !is_uuid_v7(&self.job_series_id.0)
            || self.attempt == 0
            || !is_uuid_v7(&self.run_id.0)
            || !is_sanitized_case_name(&self.case_name)
        {
            return Err(ManifestError::MissingIdentity);
        }
        if self.software.crate_versions.is_empty()
            || self
                .software
                .crate_versions
                .iter()
                .any(|(name, version)| name.trim().is_empty() || version.trim().is_empty())
            || self
                .software
                .git_commit
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            || !is_sha256(&self.inputs.case_sha256)
            || !is_sha256(&self.inputs.run_profile_sha256)
            || !valid_sha_map(&self.inputs.dataset_lock_sha256)
            || !valid_sha_map(&self.inputs.dataset_profile_sha256)
            || !valid_sha_map(&self.inputs.dataset_content_sha256)
            || self
                .inputs
                .dataset_lock_sha256
                .keys()
                .ne(self.inputs.dataset_profile_sha256.keys())
        {
            return Err(ManifestError::InvalidConfiguration);
        }
        let mut output_products = BTreeSet::new();
        let outputs_invalid = self.effective_outputs.is_empty()
            || self.effective_outputs.iter().any(|output| {
                !output_products.insert(output.product.0.as_str()) || !valid_output(output)
            })
            || !self.effective_outputs.iter().any(|output| {
                output.product.0 == trajecta_case::model::output::PARTICLE_STATE_PRODUCT_ID
                    && output.sink.model.0 == self.numerical.particle_state_sink
            });
        if self.execution.worker_threads == 0
            || self.execution.memory_budget_bytes == 0
            || self.execution.executor.trim().is_empty()
            || self
                .execution
                .reader_backends
                .keys()
                .any(|value| value.trim().is_empty())
            || self
                .execution
                .reader_backends
                .keys()
                .ne(self.inputs.dataset_lock_sha256.keys())
            || self
                .execution
                .io_counters
                .keys()
                .any(|value| value.trim().is_empty())
            || self.numerical.integrator.trim().is_empty()
            || self.numerical.population.trim().is_empty()
            || self.numerical.particle_state_sink.trim().is_empty()
            || self.numerical.tolerance_registry.trim().is_empty()
            || self
                .numerical
                .ozone_rule
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            || outputs_invalid
            || self
                .numerical
                .boundary_policies
                .iter()
                .any(|value| value.trim().is_empty())
            || has_duplicates(&self.numerical.boundary_policies)
            || self.numerical.tolerances.is_empty()
            || self
                .numerical
                .tolerances
                .keys()
                .any(|value| value.trim().is_empty())
            || self
                .numerical
                .tolerances
                .values()
                .any(|value| !value.is_finite() || *value < 0.0)
            || self.sqlite.schema_version != SQLITE_SCHEMA_VERSION
            || !is_normal_relative_path(&self.sqlite.relative_path)
            || self.sqlite.journal_mode != "WAL"
            || self.sqlite.synchronous != "NORMAL"
            || self
                .sqlite
                .row_counts
                .keys()
                .any(|value| value.trim().is_empty())
        {
            return Err(ManifestError::InvalidConfiguration);
        }
        match self.status {
            RunLifecycleStatus::Running => {
                if self.finished_at.is_some() || self.failure.is_some() || self.provenance.is_some()
                {
                    return Err(ManifestError::InvalidLifecycle);
                }
            }
            RunLifecycleStatus::Complete
            | RunLifecycleStatus::CompletedWithParticleErrors
            | RunLifecycleStatus::Cancelled => {
                if self.finished_at.is_none() || self.failure.is_some() {
                    return Err(ManifestError::InvalidLifecycle);
                }
                match self.provenance.as_ref() {
                    Some(provenance) if valid_provenance_identity(provenance) => {}
                    _ => return Err(ManifestError::InvalidLifecycle),
                }
            }
            RunLifecycleStatus::Failed | RunLifecycleStatus::Interrupted => {
                if self.finished_at.is_none() || self.failure.is_none() || self.provenance.is_some()
                {
                    return Err(ManifestError::InvalidLifecycle);
                }
            }
        }
        if self.status == RunLifecycleStatus::Complete && self.terminations.abnormal_count != 0 {
            return Err(ManifestError::InvalidLifecycle);
        }
        if self.status == RunLifecycleStatus::CompletedWithParticleErrors
            && self.terminations.abnormal_count == 0
        {
            return Err(ManifestError::InvalidLifecycle);
        }
        if self
            .finished_at
            .is_some_and(|finished| finished < self.started_at)
            || self
                .failure
                .as_ref()
                .is_some_and(|failure| failure.code.trim().is_empty())
            || !valid_termination_summary(&self.terminations)
        {
            return Err(ManifestError::InvalidLifecycle);
        }
        let mut geometry_events = BTreeSet::new();
        if self.geometries.iter().any(|geometry| {
            let source_fields = [
                geometry.source_path.is_some(),
                geometry.source_size_bytes.is_some(),
                geometry.source_sha256.is_some(),
            ];
            geometry.event_id.trim().is_empty()
                || !geometry_events.insert(geometry.event_id.as_str())
                || !is_sha256(&geometry.canonical_geometry_sha256)
                || !geometry.spherical_area_m2.is_finite()
                || geometry.spherical_area_m2 < 0.0
                || (source_fields.iter().any(|present| *present)
                    && !source_fields.iter().all(|present| *present))
                || geometry
                    .source_path
                    .as_deref()
                    .is_some_and(|path| !is_normal_absolute_path(path))
                || geometry
                    .source_sha256
                    .as_deref()
                    .is_some_and(|value| !is_sha256(value))
        }) || self.mass_ledger.iter().enumerate().any(|(index, record)| {
            record.step_index != index as u64
                || [
                    record.opening_active_kg,
                    record.opening_residual_kg,
                    record.incoming_kg,
                    record.outgoing_kg,
                    record.normal_terminated_kg,
                    record.abnormal_terminated_kg,
                    record.closing_active_kg,
                    record.closing_residual_kg,
                    record.tolerance_kg,
                ]
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
                || !record.imbalance_kg.is_finite()
        }) {
            return Err(ManifestError::InvalidNumericSummary);
        }
        Ok(())
    }
}

fn valid_output(output: &OutputProductSpec) -> bool {
    if output.product.0.trim().is_empty()
        || output.sink.model.0.trim().is_empty()
        || output
            .sink
            .parameters
            .keys()
            .any(|key| key.trim().is_empty())
    {
        return false;
    }
    match &output.schedule {
        OutputSchedule::Endpoints => true,
        OutputSchedule::Interval { interval, .. } => interval.is_positive_finite(),
    }
}

fn valid_sha_map(values: &BTreeMap<String, String>) -> bool {
    values
        .iter()
        .all(|(key, value)| !key.trim().is_empty() && is_sha256(value))
}

fn valid_provenance_identity(provenance: &ProvenanceBundleIdentity) -> bool {
    if provenance.schema_version != crate::science::PROVENANCE_BUNDLE_SCHEMA_ID
        || provenance.relative_path != crate::science::PROVENANCE_BUNDLE_FILE_NAME
        || !is_sha256(&provenance.sha256)
        || !is_sha256(&provenance.sqlite_sha256)
        || !is_sha256(&provenance.content_sha256)
        || !is_sha256(&provenance.sqlite_sql_sha256)
        || !is_sha256(&provenance.canonical_output_sha256)
    {
        return false;
    }
    crate::output::provenance_bundle::canonical_output_digest(
        &provenance.sqlite_sql_sha256,
        &provenance.content_sha256,
    )
    .is_ok_and(|expected| expected == provenance.canonical_output_sha256)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_uuid_v7(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
        && bytes[14] == b'7'
        && matches!(bytes[19].to_ascii_lowercase(), b'8' | b'9' | b'a' | b'b')
}

fn is_sanitized_case_name(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed != "."
        && trimmed != ".."
        && !trimmed
            .chars()
            .any(|character| matches!(character, '/' | '\\'))
}

fn is_normal_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_normal_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
}

fn has_duplicates(values: &[String]) -> bool {
    let mut unique = BTreeSet::new();
    values.iter().any(|value| !unique.insert(value.as_str()))
}

fn valid_termination_summary(summary: &TerminationSummary) -> bool {
    if summary
        .by_reason
        .keys()
        .any(|value| value.trim().is_empty())
    {
        return false;
    }
    summary
        .by_reason
        .values()
        .try_fold(0_u64, |total, value| total.checked_add(*value))
        .is_some_and(|total| {
            summary
                .normal_count
                .checked_add(summary.abnormal_count)
                .is_some_and(|expected| total == expected)
        })
}

/// Run-manifest validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestError {
    /// Manifest schema identity is not supported.
    UnsupportedSchema,
    /// Job series, attempt, run, or Case identity is invalid.
    MissingIdentity,
    /// Lifecycle status conflicts with final time, failure, or termination counts.
    InvalidLifecycle,
    /// Geometry or mass-ledger numeric fields are non-finite or physically invalid.
    InvalidNumericSummary,
    /// Execution, numerical, output, or tolerance configuration is incomplete.
    InvalidConfiguration,
}

impl ManifestError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "manifest.unsupported_schema",
            Self::MissingIdentity => "manifest.missing_identity",
            Self::InvalidLifecycle => "manifest.invalid_lifecycle",
            Self::InvalidNumericSummary => "manifest.invalid_numeric_summary",
            Self::InvalidConfiguration => "manifest.invalid_configuration",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn start() -> RunManifestStart {
        RunManifestStart {
            run_id: RunId("018f0000-0000-7000-8000-000000000000".into()),
            case_name: "case".into(),
            started_at: Timestamp::UNIX_EPOCH,
            software: SoftwareIdentity {
                crate_versions: BTreeMap::from([("trajecta-core".into(), "0.0.0".into())]),
                git_commit: None,
            },
            inputs: InputIdentity {
                case_sha256: "0".repeat(64),
                run_profile_sha256: "1".repeat(64),
                ..InputIdentity::default()
            },
            execution: ExecutionSummary {
                worker_threads: 1,
                memory_budget_bytes: 1,
                executor: "cpu".into(),
                ..ExecutionSummary::default()
            },
            numerical: NumericalSummary {
                random_seed: 0,
                integrator: crate::science::RK2_SPHERICAL_ID.into(),
                boundary_policies: Vec::new(),
                population: crate::science::RELEASE_DRIVEN_POPULATION_ID.into(),
                ozone_rule: None,
                particle_state_sink: trajecta_case::model::output::PARTICLE_STATE_SQLITE_SINK_ID
                    .into(),
                tolerance_registry: "trajecta.m4.numerical-contract/v1".into(),
                tolerances: BTreeMap::from([("domain_fill_step_relative".into(), 1.0e-12)]),
                deterministic: true,
            },
            geometries: Vec::new(),
            effective_outputs: vec![trajecta_case::model::output::default_particle_state_output()],
        }
    }

    fn provenance() -> ProvenanceBundleIdentity {
        let content = "cc".repeat(32);
        let sql = "dd".repeat(32);
        let canonical = crate::output::provenance_bundle::canonical_output_digest(&sql, &content)
            .unwrap_or_else(|_| "00".repeat(32));
        ProvenanceBundleIdentity {
            schema_version: crate::science::PROVENANCE_BUNDLE_SCHEMA_ID.into(),
            relative_path: crate::science::PROVENANCE_BUNDLE_FILE_NAME.into(),
            sha256: "aa".repeat(32),
            sqlite_sha256: "bb".repeat(32),
            content_sha256: content,
            sqlite_sql_sha256: sql,
            canonical_output_sha256: canonical,
            record_count: 0,
            field_set_count: 0,
            sample_count: 0,
        }
    }

    #[test]
    fn running_manifest_roundtrips_and_rejects_unknown_fields() {
        let manifest = RunManifest::running(start());
        manifest.validate().unwrap();
        let json = serde_json::to_string(&manifest).unwrap();
        let decoded: RunManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, manifest);

        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<RunManifest>(value).is_err());
    }

    #[test]
    fn complete_cannot_hide_abnormal_particle_termination() {
        let mut manifest = RunManifest::running(start());
        manifest.status = RunLifecycleStatus::Complete;
        manifest.finished_at = Some(Timestamp::UNIX_EPOCH);
        manifest.terminations.abnormal_count = 1;
        manifest
            .terminations
            .by_reason
            .insert("invalid_meteorology".into(), 1);
        manifest.provenance = Some(provenance());
        assert_eq!(manifest.validate(), Err(ManifestError::InvalidLifecycle));
        manifest.status = RunLifecycleStatus::CompletedWithParticleErrors;
        manifest.validate().unwrap();
    }

    #[test]
    fn series_attempt_and_terminal_exit_semantics_are_frozen() {
        let manifest = RunManifest::running_in_series(
            start(),
            JobSeriesId("018f0000-0000-7000-8000-000000000001".into()),
            2,
        );
        assert_eq!(manifest.attempt, 2);
        assert_eq!(manifest.status.wait_exit_code(), None);
        assert_eq!(RunLifecycleStatus::Complete.wait_exit_code(), Some(0));
        for status in [
            RunLifecycleStatus::CompletedWithParticleErrors,
            RunLifecycleStatus::Failed,
            RunLifecycleStatus::Cancelled,
            RunLifecycleStatus::Interrupted,
        ] {
            assert!(status.is_terminal());
            assert!(!status.run_success());
            assert_eq!(status.wait_exit_code(), Some(1));
        }

        let mut invalid = manifest;
        invalid.attempt = 0;
        assert_eq!(invalid.validate(), Err(ManifestError::MissingIdentity));
    }

    #[test]
    fn cancelled_requires_provenance_and_interrupted_forbids_it() {
        let mut cancelled = RunManifest::running(start());
        cancelled.status = RunLifecycleStatus::Cancelled;
        cancelled.finished_at = Some(Timestamp::UNIX_EPOCH);
        assert_eq!(cancelled.validate(), Err(ManifestError::InvalidLifecycle));
        cancelled.provenance = Some(provenance());
        cancelled.validate().unwrap();

        let mut interrupted = RunManifest::running(start());
        interrupted.status = RunLifecycleStatus::Interrupted;
        interrupted.finished_at = Some(Timestamp::UNIX_EPOCH);
        interrupted.failure = Some(RunFailure {
            code: "run.interrupted".into(),
            message: "worker disappeared before safe finalization".into(),
        });
        interrupted.validate().unwrap();
        interrupted.provenance = Some(provenance());
        assert_eq!(interrupted.validate(), Err(ManifestError::InvalidLifecycle));
    }
}
