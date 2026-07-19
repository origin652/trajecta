//! # Contract: staged immutable dataset-lock construction
//!
//! Lock construction scans explicit named roots, recognizes containers by
//! magic bytes, inspects metadata, selects only the Case interval plus declared
//! buffers, matches one exact Profile, validates capabilities and stable
//! topology, and finally streams payload hashes. Each stage is gated so a
//! failed inspection does not produce cascades of profile or assembly errors.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{
    DatasetIdentity, DatasetLock, GeneratorInfo, GridSignature, LockedFile, ProfileIdentity,
    VerticalSignature,
};
use trajecta_case::model::time::Timestamp;
use trajecta_case::resolver::sha256_file_streaming;
use trajecta_case::schema::CURRENT_SCHEMA_VERSION;

use crate::field::CapabilitySet;
use crate::io::reader::{
    DecodeError, ReaderFactory, SourceFormat, SourceMetadata, detect_source_format,
};
use crate::profile::document::{DatasetProfile, ProfileCatalog, ProfileName};

/// Metadata-only inspection backend used by lock construction.
pub trait SourceMetadataInspector: Send + Sync {
    /// Inspects one magic-identified source without decoding complete fields.
    fn inspect(&self, path: &Path, format: SourceFormat) -> Result<SourceMetadata, DecodeError>;
}

/// Metadata inspector backed by one explicit ReaderFactory backend.
#[derive(Clone, Copy, Debug)]
pub struct ReaderMetadataInspector {
    backend: MeteorologyReaderBackend,
}

impl ReaderMetadataInspector {
    /// Creates an inspector without any runtime fallback policy.
    #[must_use]
    pub const fn new(backend: MeteorologyReaderBackend) -> Self {
        Self { backend }
    }
}

impl SourceMetadataInspector for ReaderMetadataInspector {
    fn inspect(&self, path: &Path, format: SourceFormat) -> Result<SourceMetadata, DecodeError> {
        let reader = ReaderFactory::create(format, self.backend)?;
        let reader: Box<dyn crate::io::reader::MetReader> =
            if let Some(counters) = crate::io::metrics::active_io_counters() {
                Box::new(crate::io::counting_reader::CountingReader::new(
                    reader, counters,
                ))
            } else {
                reader
            };
        reader.inspect(path)
    }
}

/// Inclusive physical coverage and frame buffers requested by one Case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LockCoverageRequest {
    /// Earliest physical Case instant.
    pub start: Timestamp,
    /// Latest physical Case instant.
    pub end: Timestamp,
    /// Additional preceding frames required for interpolation.
    pub interpolation_before_frames: usize,
    /// Additional following frames required for interpolation.
    pub interpolation_after_frames: usize,
}

/// Complete immutable-lock build request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetLockRequest {
    /// Logical upstream identity.
    pub identity: DatasetIdentity,
    /// Generator identity recorded into the lock.
    pub generator: GeneratorInfo,
    /// Explicit named roots to scan recursively.
    pub data_roots: BTreeMap<DataRootId, PathBuf>,
    /// Physical interval and interpolation buffers.
    pub coverage: LockCoverageRequest,
    /// Capabilities required by the resolved Case.
    pub required_capabilities: CapabilitySet,
    /// Ignore matching cache stamps and stream every payload again.
    pub force_rehash: bool,
    /// Optional exact Profile name. When set, only files matching this Profile
    /// participate in topology/coverage selection; other products in a mixed
    /// root are skipped with non-fatal notes.
    pub preferred_profile: Option<String>,
}

/// Ordered lock-construction stage.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LockBuildStage {
    /// Magic recognition and metadata inspection.
    Inspect,
    /// Exact Profile selection.
    ProfileMatch,
    /// Immutable indexing and payload hashing.
    Index,
    /// Logical-frame and stable-topology assembly.
    Assemble,
    /// Required-capability validation.
    CapabilityValidate,
}

/// One independent, stage-scoped lock construction failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockBuildDiagnostic {
    /// Stage that owns the root cause.
    pub stage: LockBuildStage,
    /// Stable machine-readable code.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Affected local path, when applicable.
    pub path: Option<PathBuf>,
}

impl LockBuildDiagnostic {
    fn new(stage: LockBuildStage, code: &str, message: impl Into<String>) -> Self {
        Self {
            stage,
            code: code.into(),
            message: message.into(),
            path: None,
        }
    }

    fn at_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
}

/// Counts from a bounded root scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LockScanSummary {
    /// Regular or symbolic-link files encountered.
    pub files_seen: usize,
    /// Supported meteorological containers recognized by magic bytes.
    pub meteorology_files: usize,
    /// Non-meteorological sidecars ignored.
    pub sidecar_files: usize,
    /// Directory symlinks skipped to prevent traversal cycles.
    pub directory_symlinks_skipped: usize,
    /// Payload digests reused from a matching verification cache stamp.
    pub hash_cache_hits: usize,
    /// Payloads streamed through SHA-256 during this build.
    pub files_hashed: usize,
}

/// Lock construction result with deterministic diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetLockBuildOutcome {
    /// Completed lock; absent when any stage failed.
    pub lock: Option<DatasetLock>,
    /// Independent root-cause diagnostics (fatal).
    pub diagnostics: Vec<LockBuildDiagnostic>,
    /// Non-fatal notes (for example mixed-root files skipped by Profile filter).
    pub notes: Vec<LockBuildDiagnostic>,
    /// Scan and hashing summary.
    pub summary: LockScanSummary,
}

impl DatasetLockBuildOutcome {
    /// Returns whether a complete immutable lock was produced.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.lock.is_some() && self.diagnostics.is_empty()
    }
}

/// Persistent hash-cache entry keyed by canonical source path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedFileHash {
    size_bytes: u64,
    modified_unix_nanos: Option<i128>,
    created_unix_nanos: Option<i128>,
    platform_identity: String,
    sha256: String,
}

/// Machine-local verification cache; never serialized into a DatasetLock.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileHashCache {
    entries: BTreeMap<PathBuf, CachedFileHash>,
}

impl FileHashCache {
    /// Creates an empty cache.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Loads a JSON cache; a missing path yields an empty cache.
    pub fn load_json(path: &Path) -> Result<Self, HashCacheError> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| HashCacheError::Parse {
                path: path.to_path_buf(),
                message: error.to_string(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(error) => Err(HashCacheError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            }),
        }
    }

    /// Saves a deterministic JSON cache through a same-directory temporary file.
    pub fn save_json(&self, path: &Path) -> Result<(), HashCacheError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| HashCacheError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("hash-cache.json");
        let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
        fs::write(&temporary, bytes).map_err(|error| HashCacheError::Io {
            path: temporary.clone(),
            message: error.to_string(),
        })?;
        if let Err(error) = fs::rename(&temporary, path) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(HashCacheError::Io {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
            fs::remove_file(path).map_err(|remove_error| HashCacheError::Io {
                path: path.to_path_buf(),
                message: remove_error.to_string(),
            })?;
            fs::rename(&temporary, path).map_err(|rename_error| HashCacheError::Io {
                path: path.to_path_buf(),
                message: rename_error.to_string(),
            })?;
        }
        Ok(())
    }

    fn digest(&mut self, path: &Path, force: bool) -> Result<(u64, String, bool), HashCacheError> {
        let canonical = fs::canonicalize(path).map_err(|error| HashCacheError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let metadata = fs::metadata(&canonical).map_err(|error| HashCacheError::Io {
            path: canonical.clone(),
            message: error.to_string(),
        })?;
        let stamp = CachedFileHash {
            size_bytes: metadata.len(),
            modified_unix_nanos: metadata.modified().ok().and_then(system_time_nanos),
            created_unix_nanos: metadata.created().ok().and_then(system_time_nanos),
            platform_identity: platform_identity(&metadata),
            sha256: String::new(),
        };
        if !force
            && let Some(cached) = self.entries.get(&canonical)
            && cached.size_bytes == stamp.size_bytes
            && cached.modified_unix_nanos == stamp.modified_unix_nanos
            && cached.created_unix_nanos == stamp.created_unix_nanos
            && cached.platform_identity == stamp.platform_identity
        {
            return Ok((cached.size_bytes, cached.sha256.clone(), true));
        }
        let (size_bytes, sha256) =
            sha256_file_streaming(&canonical).map_err(|error| HashCacheError::Io {
                path: canonical.clone(),
                message: error.to_string(),
            })?;
        self.entries.insert(
            canonical,
            CachedFileHash {
                size_bytes,
                sha256: sha256.clone(),
                ..stamp
            },
        );
        Ok((size_bytes, sha256, false))
    }
}

fn system_time_nanos(value: SystemTime) -> Option<i128> {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok(),
        Err(error) => i128::try_from(error.duration().as_nanos())
            .ok()
            .and_then(i128::checked_neg),
    }
}

#[cfg(unix)]
fn platform_identity(metadata: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!("unix:{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(windows)]
fn platform_identity(metadata: &fs::Metadata) -> String {
    use std::os::windows::fs::MetadataExt;
    format!(
        "windows:{}:{}",
        metadata.creation_time(),
        metadata.file_size()
    )
}

#[cfg(not(any(unix, windows)))]
fn platform_identity(metadata: &fs::Metadata) -> String {
    format!("portable:{}", metadata.len())
}

/// Hash-cache loading, saving, or digest failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HashCacheError {
    /// Cache or payload I/O failed.
    Io {
        /// Affected path.
        path: PathBuf,
        /// Stable platform message.
        message: String,
    },
    /// Cache JSON could not be encoded or decoded.
    Parse {
        /// Affected path.
        path: PathBuf,
        /// Stable parser message.
        message: String,
    },
}

impl fmt::Display for HashCacheError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(
                    formatter,
                    "hash-cache I/O at '{}': {message}",
                    path.display()
                )
            }
            Self::Parse { path, message } => write!(
                formatter,
                "hash-cache parse at '{}': {message}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for HashCacheError {}

/// Deterministic, stage-gated lock builder.
pub struct DatasetLockBuilder<'a> {
    profiles: &'a ProfileCatalog,
    inspector: &'a dyn SourceMetadataInspector,
    hash_cache: &'a mut FileHashCache,
}

impl<'a> DatasetLockBuilder<'a> {
    /// Creates a builder from an exact Profile catalog and explicit inspector.
    pub fn new(
        profiles: &'a ProfileCatalog,
        inspector: &'a dyn SourceMetadataInspector,
        hash_cache: &'a mut FileHashCache,
    ) -> Self {
        Self {
            profiles,
            inspector,
            hash_cache,
        }
    }

    /// Builds one immutable lock or returns only root-cause diagnostics.
    pub fn build(&mut self, request: &DatasetLockRequest) -> DatasetLockBuildOutcome {
        let mut diagnostics = Vec::new();
        let mut notes = Vec::new();
        let mut summary = LockScanSummary::default();
        if request.coverage.start > request.coverage.end {
            diagnostics.push(LockBuildDiagnostic::new(
                LockBuildStage::Inspect,
                "lock_build.coverage.reversed",
                "coverage.start must be no later than coverage.end",
            ));
            return failed(diagnostics, notes.clone(), summary);
        }

        let candidates = scan_roots(&request.data_roots, &mut summary, &mut diagnostics);
        if !diagnostics.is_empty() {
            return failed(diagnostics, notes.clone(), summary);
        }
        if candidates.is_empty() {
            diagnostics.push(LockBuildDiagnostic::new(
                LockBuildStage::Inspect,
                "lock_build.scan.no_meteorology",
                "no supported meteorological containers were found",
            ));
            return failed(diagnostics, notes.clone(), summary);
        }

        // Preferred Profile with candidate_path_globs: matching paths hard-fail
        // inspect; non-matching mixed products may be soft-skipped with notes.
        let preferred_globs = request
            .preferred_profile
            .as_ref()
            .and_then(|name| self.profiles.get(&ProfileName(name.clone())))
            .map(|profile| profile.document.candidate_path_globs.as_slice())
            .unwrap_or(&[]);
        let inspected = self.inspect_candidates_for_preferred(
            candidates,
            preferred_globs,
            request.preferred_profile.is_some(),
            &mut diagnostics,
            &mut notes,
        );
        if !diagnostics.is_empty() {
            return failed(diagnostics, notes.clone(), summary);
        }
        // Profile-aware filter first so mixed product roots do not poison topology.
        let inspected_refs = inspected.iter().collect::<Vec<_>>();
        let (profile_name, profile_sha256, profile_capabilities, warmup, profile_files) = match self
            .select_profile_group(
                &inspected_refs,
                request.preferred_profile.as_deref(),
                &mut diagnostics,
                &mut notes,
            ) {
            Some((profile, files)) if diagnostics.is_empty() => {
                let warmup = profile
                    .document
                    .fields
                    .iter()
                    .map(|field| usize::from(field.temporal.warmup_frames))
                    .chain(
                        profile
                            .document
                            .derived_fields
                            .iter()
                            .map(|field| usize::from(field.temporal.warmup_frames)),
                    )
                    .max()
                    .unwrap_or(0);
                (
                    profile.name().0.clone(),
                    profile.sha256.clone(),
                    profile.document.capabilities.clone(),
                    warmup,
                    files.into_iter().cloned().collect::<Vec<_>>(),
                )
            }
            _ => return failed(diagnostics, notes.clone(), summary),
        };
        let all_times = profile_files
            .iter()
            .flat_map(|candidate| candidate.metadata.valid_times.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let base_times = match select_times(
            &all_times,
            request.coverage,
            request.coverage.interpolation_before_frames,
        ) {
            Ok(times) => times,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                return failed(diagnostics, notes.clone(), summary);
            }
        };
        let base_candidates = select_candidates(&profile_files, &base_times);
        if base_candidates.is_empty() {
            diagnostics.push(LockBuildDiagnostic::new(
                LockBuildStage::Assemble,
                "lock_build.coverage.no_profile_files",
                format!("Profile `{profile_name}` has no files covering the requested interval"),
            ));
            return failed(diagnostics, notes.clone(), summary);
        }

        let before = match request
            .coverage
            .interpolation_before_frames
            .checked_add(warmup)
        {
            Some(value) => value,
            None => {
                diagnostics.push(LockBuildDiagnostic::new(
                    LockBuildStage::Assemble,
                    "lock_build.coverage.buffer_overflow",
                    "warm-up and interpolation frame counts overflow usize",
                ));
                return failed(diagnostics, notes.clone(), summary);
            }
        };
        let selected_times = match select_times(&all_times, request.coverage, before) {
            Ok(times) => times,
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                return failed(diagnostics, notes.clone(), summary);
            }
        };
        let selected = select_candidates(&profile_files, &selected_times);
        if selected.is_empty() {
            diagnostics.push(LockBuildDiagnostic::new(
                LockBuildStage::Assemble,
                "lock_build.coverage.no_selected_files",
                "no Profile-matched files remain after coverage selection",
            ));
            return failed(diagnostics, notes.clone(), summary);
        }

        let (grid, vertical) = match stable_topology(&selected, &mut diagnostics) {
            Some(value) if diagnostics.is_empty() => value,
            _ => return failed(diagnostics, notes.clone(), summary),
        };
        for capability in request.required_capabilities.iter() {
            if !profile_capabilities.contains_key(&capability) {
                diagnostics.push(LockBuildDiagnostic::new(
                    LockBuildStage::CapabilityValidate,
                    "lock_build.capability.missing",
                    format!(
                        "Profile '{profile_name}' does not provide required capability {capability:?}"
                    ),
                ));
            }
        }
        if !diagnostics.is_empty() {
            return failed(diagnostics, notes.clone(), summary);
        }

        let mut files = Vec::with_capacity(selected.len());
        for candidate in selected {
            let (size_bytes, sha256, cache_hit) = match self
                .hash_cache
                .digest(&candidate.absolute_path, request.force_rehash)
            {
                Ok(value) => value,
                Err(error) => {
                    diagnostics.push(
                        LockBuildDiagnostic::new(
                            LockBuildStage::Index,
                            "lock_build.hash.failed",
                            error.to_string(),
                        )
                        .at_path(candidate.absolute_path.clone()),
                    );
                    continue;
                }
            };
            if cache_hit {
                summary.hash_cache_hits += 1;
            } else {
                summary.files_hashed += 1;
            }
            let mut roles = candidate.metadata.roles.clone();
            roles.sort();
            roles.dedup();
            let valid_times = candidate
                .metadata
                .valid_times
                .iter()
                .copied()
                .filter(|time| selected_times.contains(time))
                .collect();
            files.push(LockedFile {
                roles,
                root_id: candidate.root_id.clone(),
                relative_path: candidate.relative_path.clone(),
                valid_times,
                size_bytes,
                sha256,
            });
        }
        if !diagnostics.is_empty() {
            return failed(diagnostics, notes.clone(), summary);
        }
        files.sort_by(|left, right| {
            (&left.root_id, &left.relative_path).cmp(&(&right.root_id, &right.relative_path))
        });
        let lock = DatasetLock {
            schema_version: CURRENT_SCHEMA_VERSION,
            identity: request.identity.clone(),
            profile: ProfileIdentity {
                name: profile_name,
                sha256: profile_sha256,
            },
            generator: request.generator.clone(),
            files,
            grid,
            vertical,
        };
        let shape = lock.validate_shape();
        if shape.has_errors() {
            diagnostics.extend(shape.iter().map(|diagnostic| {
                LockBuildDiagnostic::new(
                    LockBuildStage::Assemble,
                    diagnostic.code(),
                    diagnostic.message(),
                )
            }));
            return failed(diagnostics, notes.clone(), summary);
        }
        sort_diagnostics(&mut notes);
        DatasetLockBuildOutcome {
            lock: Some(lock),
            diagnostics,
            notes,
            summary,
        }
    }

    fn inspect_candidates_for_preferred(
        &self,
        candidates: Vec<SourceCandidate>,
        preferred_globs: &[String],
        preferred_active: bool,
        diagnostics: &mut Vec<LockBuildDiagnostic>,
        notes: &mut Vec<LockBuildDiagnostic>,
    ) -> Vec<InspectedCandidate> {
        candidates
            .into_iter()
            .filter_map(|candidate| {
                let selected_path = if preferred_active && !preferred_globs.is_empty() {
                    path_matches_any_glob(&candidate.relative_path, preferred_globs)
                } else {
                    // No path contract: every magic-recognized container is selected.
                    true
                };
                let mut hard = Vec::new();
                let result = self.inspect_one(candidate, &mut hard);
                if result.is_some() {
                    return result;
                }
                if selected_path {
                    diagnostics.extend(hard);
                } else {
                    for mut diagnostic in hard {
                        diagnostic.code = "lock_build.inspect.skipped_failed".into();
                        diagnostic.message = format!(
                            "skipping non-candidate meteorology container: {}",
                            diagnostic.message
                        );
                        notes.push(diagnostic);
                    }
                }
                None
            })
            .collect()
    }

    fn inspect_one(
        &self,
        candidate: SourceCandidate,
        diagnostics: &mut Vec<LockBuildDiagnostic>,
    ) -> Option<InspectedCandidate> {
        let metadata = match self
            .inspector
            .inspect(&candidate.absolute_path, candidate.format)
        {
            Ok(metadata) => metadata,
            Err(error) => {
                diagnostics.push(
                    LockBuildDiagnostic::new(
                        LockBuildStage::Inspect,
                        "lock_build.inspect.failed",
                        decode_message(&error),
                    )
                    .at_path(candidate.absolute_path),
                );
                return None;
            }
        };
        self.finish_inspected(candidate, metadata, diagnostics)
    }

    fn finish_inspected(
        &self,
        candidate: SourceCandidate,
        mut metadata: crate::io::reader::SourceMetadata,
        diagnostics: &mut Vec<LockBuildDiagnostic>,
    ) -> Option<InspectedCandidate> {
        let _ = self;
        if metadata.format != candidate.format {
            diagnostics.push(
                LockBuildDiagnostic::new(
                    LockBuildStage::Inspect,
                    "lock_build.inspect.format_mismatch",
                    format!(
                        "magic detected {:?} but inspector returned {:?}",
                        candidate.format, metadata.format
                    ),
                )
                .at_path(candidate.absolute_path),
            );
            return None;
        }
        metadata.path = candidate.absolute_path.clone();
        let mut times = metadata.valid_times.clone();
        times.sort();
        times.dedup();
        if times != metadata.valid_times {
            diagnostics.push(
                LockBuildDiagnostic::new(
                    LockBuildStage::Inspect,
                    "lock_build.inspect.times_not_sorted_unique",
                    "inspector valid_times must be sorted and unique",
                )
                .at_path(candidate.absolute_path),
            );
            return None;
        }
        let mut roles = metadata.roles.clone();
        roles.sort();
        roles.dedup();
        if roles.is_empty()
            || roles.iter().any(|role| role.trim().is_empty())
            || roles != metadata.roles
        {
            diagnostics.push(
                LockBuildDiagnostic::new(
                    LockBuildStage::Inspect,
                    "lock_build.inspect.roles_invalid",
                    "inspector roles must be non-empty, sorted, and unique",
                )
                .at_path(candidate.absolute_path),
            );
            return None;
        }
        Some(InspectedCandidate {
            root_id: candidate.root_id,
            relative_path: candidate.relative_path,
            absolute_path: candidate.absolute_path,
            metadata,
        })
    }

    /// Partitions inspected candidates by exact Profile match.
    ///
    /// Files that match no Profile are skipped with non-fatal notes so mixed
    /// product directories (for example CFSR `pgbl` + `flxl` + `spllnl`) can lock
    /// the requested product without manual isolation. Files that match a
    /// different Profile than the selected group are also skipped with notes.
    /// A file that should match the selected Profile but fails matching is a
    /// hard error only when it is the preferred/selected group itself.
    fn select_profile_group<'b>(
        &'b self,
        candidates: &[&'b InspectedCandidate],
        preferred_profile: Option<&str>,
        diagnostics: &mut Vec<LockBuildDiagnostic>,
        notes: &mut Vec<LockBuildDiagnostic>,
    ) -> Option<(&'b DatasetProfile, Vec<&'b InspectedCandidate>)> {
        let mut by_profile: BTreeMap<
            (ProfileName, String),
            (&'b DatasetProfile, Vec<&'b InspectedCandidate>),
        > = BTreeMap::new();
        for candidate in candidates {
            match self.profiles.match_source(&candidate.metadata) {
                Ok(profile) => {
                    let key = (profile.name().clone(), profile.sha256.clone());
                    by_profile
                        .entry(key)
                        .and_modify(|(_, list)| list.push(*candidate))
                        .or_insert_with(|| (profile, vec![*candidate]));
                }
                Err(error) => notes.push(
                    LockBuildDiagnostic::new(
                        LockBuildStage::ProfileMatch,
                        "lock_build.profile.unmatched_skipped",
                        format!("skipping file that matches no exact Profile: {error}"),
                    )
                    .at_path(candidate.absolute_path.clone()),
                ),
            }
        }
        if by_profile.is_empty() {
            diagnostics.push(LockBuildDiagnostic::new(
                LockBuildStage::ProfileMatch,
                "lock_build.profile.none_matched",
                "no inspected meteorological file matched an exact Profile",
            ));
            return None;
        }
        let selected_key = if let Some(preferred) = preferred_profile {
            match by_profile
                .keys()
                .find(|(name, _)| name.0 == preferred)
                .cloned()
            {
                Some(key) => key,
                None => {
                    diagnostics.push(LockBuildDiagnostic::new(
                        LockBuildStage::ProfileMatch,
                        "lock_build.profile.preferred_missing",
                        format!(
                            "preferred Profile `{preferred}` matched no file; available: {}",
                            by_profile
                                .keys()
                                .map(|(name, _)| name.0.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                    return None;
                }
            }
        } else if by_profile.len() == 1 {
            match by_profile.keys().next().cloned() {
                Some(key) => key,
                None => {
                    diagnostics.push(LockBuildDiagnostic::new(
                        LockBuildStage::ProfileMatch,
                        "lock_build.profile.none_matched",
                        "internal error: empty profile map after length check",
                    ));
                    return None;
                }
            }
        } else {
            diagnostics.push(LockBuildDiagnostic::new(
                LockBuildStage::ProfileMatch,
                "lock_build.profile.not_stable",
                format!(
                    "selected files require multiple Profiles: {}; set preferred_profile or isolate products",
                    by_profile
                        .keys()
                        .map(|(name, _)| name.0.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
            return None;
        };
        for ((name, _), (_, other_group)) in &by_profile {
            if name != &selected_key.0 {
                for candidate in other_group {
                    notes.push(
                        LockBuildDiagnostic::new(
                            LockBuildStage::ProfileMatch,
                            "lock_build.profile.other_product_skipped",
                            format!(
                                "skipping file matched by Profile `{}` while locking `{}`",
                                name.0, selected_key.0.0
                            ),
                        )
                        .at_path(candidate.absolute_path.clone()),
                    );
                }
            }
        }
        by_profile.remove(&selected_key)
    }
}

fn failed(
    mut diagnostics: Vec<LockBuildDiagnostic>,
    mut notes: Vec<LockBuildDiagnostic>,
    summary: LockScanSummary,
) -> DatasetLockBuildOutcome {
    sort_diagnostics(&mut diagnostics);
    sort_diagnostics(&mut notes);
    DatasetLockBuildOutcome {
        lock: None,
        diagnostics,
        notes,
        summary,
    }
}

fn sort_diagnostics(diagnostics: &mut [LockBuildDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        (left.stage, &left.code, &left.path, &left.message).cmp(&(
            right.stage,
            &right.code,
            &right.path,
            &right.message,
        ))
    });
}

#[derive(Clone, Debug)]
struct SourceCandidate {
    root_id: DataRootId,
    relative_path: PathBuf,
    absolute_path: PathBuf,
    format: SourceFormat,
}

#[derive(Clone, Debug)]
struct InspectedCandidate {
    root_id: DataRootId,
    relative_path: PathBuf,
    absolute_path: PathBuf,
    metadata: SourceMetadata,
}

// SourceMetadata is already Clone via reader contract.

fn scan_roots(
    roots: &BTreeMap<DataRootId, PathBuf>,
    summary: &mut LockScanSummary,
    diagnostics: &mut Vec<LockBuildDiagnostic>,
) -> Vec<SourceCandidate> {
    if roots.is_empty() {
        diagnostics.push(LockBuildDiagnostic::new(
            LockBuildStage::Inspect,
            "lock_build.roots.empty",
            "at least one named data root is required",
        ));
        return Vec::new();
    }
    let mut canonical_roots = BTreeMap::new();
    for (root_id, path) in roots {
        if root_id.0.trim().is_empty() {
            diagnostics.push(
                LockBuildDiagnostic::new(
                    LockBuildStage::Inspect,
                    "lock_build.root.id_empty",
                    "data-root id must not be empty",
                )
                .at_path(path.clone()),
            );
            continue;
        }
        match fs::canonicalize(path) {
            Ok(canonical) if canonical.is_dir() => {
                canonical_roots.insert(root_id.clone(), canonical);
            }
            Ok(_) => diagnostics.push(
                LockBuildDiagnostic::new(
                    LockBuildStage::Inspect,
                    "lock_build.root.not_directory",
                    "data root is not a directory",
                )
                .at_path(path.clone()),
            ),
            Err(error) => diagnostics.push(
                LockBuildDiagnostic::new(
                    LockBuildStage::Inspect,
                    "lock_build.root.unreadable",
                    error.to_string(),
                )
                .at_path(path.clone()),
            ),
        }
    }
    if !diagnostics.is_empty() {
        return Vec::new();
    }

    let mut by_canonical_path = BTreeMap::<PathBuf, SourceCandidate>::new();
    for (root_id, root) in &canonical_roots {
        let mut stack = vec![root.clone()];
        while let Some(directory) = stack.pop() {
            let mut entries = match fs::read_dir(&directory) {
                Ok(values) => values
                    .map(|entry| entry.map(|value| value.path()))
                    .collect::<Result<Vec<_>, _>>(),
                Err(error) => Err(error),
            };
            let Ok(ref mut entries) = entries else {
                let error = entries.err().map_or_else(
                    || "unknown directory read failure".into(),
                    |value| value.to_string(),
                );
                diagnostics.push(
                    LockBuildDiagnostic::new(
                        LockBuildStage::Inspect,
                        "lock_build.scan.read_directory",
                        error,
                    )
                    .at_path(directory),
                );
                continue;
            };
            entries.sort();
            for path in entries.iter() {
                let link_metadata = match fs::symlink_metadata(path) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        diagnostics.push(
                            LockBuildDiagnostic::new(
                                LockBuildStage::Inspect,
                                "lock_build.scan.metadata",
                                error.to_string(),
                            )
                            .at_path(path.clone()),
                        );
                        continue;
                    }
                };
                if link_metadata.file_type().is_symlink() {
                    match fs::metadata(path) {
                        Ok(target) if target.is_dir() => {
                            summary.directory_symlinks_skipped += 1;
                            continue;
                        }
                        Ok(target) if target.is_file() => {}
                        Ok(_) => continue,
                        Err(error) => {
                            diagnostics.push(
                                LockBuildDiagnostic::new(
                                    LockBuildStage::Inspect,
                                    "lock_build.scan.broken_symlink",
                                    error.to_string(),
                                )
                                .at_path(path.clone()),
                            );
                            continue;
                        }
                    }
                } else if link_metadata.is_dir() {
                    stack.push(path.clone());
                    continue;
                } else if !link_metadata.is_file() {
                    continue;
                }
                summary.files_seen += 1;
                let canonical = match fs::canonicalize(path) {
                    Ok(value) => value,
                    Err(error) => {
                        diagnostics.push(
                            LockBuildDiagnostic::new(
                                LockBuildStage::Inspect,
                                "lock_build.scan.canonicalize",
                                error.to_string(),
                            )
                            .at_path(path.clone()),
                        );
                        continue;
                    }
                };
                if !canonical_roots
                    .values()
                    .any(|authorized| canonical == *authorized || canonical.starts_with(authorized))
                {
                    diagnostics.push(
                        LockBuildDiagnostic::new(
                            LockBuildStage::Inspect,
                            "lock_build.scan.symlink_escape",
                            "file resolves outside all authorized data roots",
                        )
                        .at_path(path.clone()),
                    );
                    continue;
                }
                let format = match detect_source_format(path) {
                    Ok(Some(format)) => format,
                    Ok(None) => {
                        summary.sidecar_files += 1;
                        continue;
                    }
                    Err(error) => {
                        diagnostics.push(
                            LockBuildDiagnostic::new(
                                LockBuildStage::Inspect,
                                "lock_build.scan.magic",
                                decode_message(&error),
                            )
                            .at_path(path.clone()),
                        );
                        continue;
                    }
                };
                summary.meteorology_files += 1;
                let relative_path = match path.strip_prefix(root) {
                    Ok(value) => value.to_path_buf(),
                    Err(_) => {
                        diagnostics.push(
                            LockBuildDiagnostic::new(
                                LockBuildStage::Inspect,
                                "lock_build.scan.relative_path",
                                "scanned path is not lexically under its data root",
                            )
                            .at_path(path.clone()),
                        );
                        continue;
                    }
                };
                let candidate = SourceCandidate {
                    root_id: root_id.clone(),
                    relative_path,
                    absolute_path: path.clone(),
                    format,
                };
                if let Some(previous) = by_canonical_path.insert(canonical, candidate) {
                    diagnostics.push(
                        LockBuildDiagnostic::new(
                            LockBuildStage::Inspect,
                            "lock_build.scan.overlapping_roots",
                            format!(
                                "the same payload is reachable as '{}:{}' and through another root",
                                previous.root_id.0,
                                previous.relative_path.display()
                            ),
                        )
                        .at_path(previous.absolute_path),
                    );
                }
            }
        }
    }
    let mut candidates = by_canonical_path.into_values().collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (&left.root_id, &left.relative_path).cmp(&(&right.root_id, &right.relative_path))
    });
    candidates
}

fn select_times(
    all_times: &[Timestamp],
    coverage: LockCoverageRequest,
    before_frames: usize,
) -> Result<BTreeSet<Timestamp>, LockBuildDiagnostic> {
    if all_times.is_empty() {
        return Err(LockBuildDiagnostic::new(
            LockBuildStage::Assemble,
            "lock_build.coverage.no_times",
            "inspected meteorology contains no physical validity times",
        ));
    }
    let first_inside = all_times.partition_point(|time| *time < coverage.start);
    let after_inside = all_times.partition_point(|time| *time <= coverage.end);
    let (mut first, mut last) = if first_inside < after_inside {
        let first = if all_times[first_inside] > coverage.start && first_inside > 0 {
            first_inside - 1
        } else {
            first_inside
        };
        let last_inside = after_inside - 1;
        let last = if all_times[last_inside] < coverage.end && after_inside < all_times.len() {
            after_inside
        } else {
            last_inside
        };
        (first, last)
    } else if first_inside > 0 && first_inside < all_times.len() {
        (first_inside - 1, first_inside)
    } else {
        return Err(LockBuildDiagnostic::new(
            LockBuildStage::Assemble,
            "lock_build.coverage.outside_available",
            "available meteorology does not bracket the requested Case interval",
        ));
    };
    if first < before_frames {
        return Err(LockBuildDiagnostic::new(
            LockBuildStage::Assemble,
            "lock_build.coverage.missing_warmup",
            format!("requested {before_frames} preceding frames but only {first} are available"),
        ));
    }
    first -= before_frames;
    let remaining_after = all_times.len() - 1 - last;
    if remaining_after < coverage.interpolation_after_frames {
        return Err(LockBuildDiagnostic::new(
            LockBuildStage::Assemble,
            "lock_build.coverage.missing_following",
            format!(
                "requested {} following frames but only {remaining_after} are available",
                coverage.interpolation_after_frames
            ),
        ));
    }
    last += coverage.interpolation_after_frames;
    Ok(all_times[first..=last].iter().copied().collect())
}

/// Match relative path or basename against simple `*` globs (no character classes).
fn path_matches_any_glob(path: &Path, globs: &[String]) -> bool {
    let relative = path.to_string_lossy().replace('\\', "/");
    let basename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    globs.iter().any(|glob| {
        let normalized = glob.replace('\\', "/");
        glob_match(&normalized, &relative) || glob_match(&normalized, &basename)
    })
}

fn glob_match(pattern: &str, value: &str) -> bool {
    let mut pattern_chars = pattern.chars().peekable();
    let mut value_chars = value.chars().peekable();
    loop {
        match (pattern_chars.next(), value_chars.peek().copied()) {
            (Some('*'), _) => {
                // Greedy star: try remaining pattern at every position.
                let rest: String = pattern_chars.collect();
                if rest.is_empty() {
                    return true;
                }
                let value_rest: String = value_chars.collect();
                for index in 0..=value_rest.len() {
                    if value_rest.is_char_boundary(index) && glob_match(&rest, &value_rest[index..])
                    {
                        return true;
                    }
                }
                return false;
            }
            (Some(expected), Some(actual)) if expected == actual => {
                value_chars.next();
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn select_candidates<'a>(
    inspected: &'a [InspectedCandidate],
    selected_times: &BTreeSet<Timestamp>,
) -> Vec<&'a InspectedCandidate> {
    inspected
        .iter()
        .filter(|candidate| {
            candidate.metadata.valid_times.is_empty()
                || candidate
                    .metadata
                    .valid_times
                    .iter()
                    .any(|time| selected_times.contains(time))
        })
        .collect()
}

fn stable_topology(
    selected: &[&InspectedCandidate],
    diagnostics: &mut Vec<LockBuildDiagnostic>,
) -> Option<(GridSignature, VerticalSignature)> {
    let mut grids = BTreeSet::new();
    let mut verticals = BTreeSet::new();
    for candidate in selected {
        match &candidate.metadata.grid {
            Some(grid) => {
                grids.insert(grid.clone());
            }
            None => diagnostics.push(
                LockBuildDiagnostic::new(
                    LockBuildStage::Assemble,
                    "lock_build.grid.missing",
                    "selected meteorological file has no normalized grid signature",
                )
                .at_path(candidate.absolute_path.clone()),
            ),
        }
        if let Some(vertical) = &candidate.metadata.vertical {
            verticals.insert(vertical.clone());
        }
    }
    if grids.len() != 1 {
        diagnostics.push(LockBuildDiagnostic::new(
            LockBuildStage::Assemble,
            "lock_build.grid.not_stable",
            format!("selected files contain {} grid signatures", grids.len()),
        ));
    }
    if verticals.len() != 1 {
        diagnostics.push(LockBuildDiagnostic::new(
            LockBuildStage::Assemble,
            "lock_build.vertical.not_stable",
            format!(
                "selected files contain {} native vertical signatures",
                verticals.len()
            ),
        ));
    }
    if diagnostics.is_empty() {
        Some((grids.pop_first()?, verticals.pop_first()?))
    } else {
        None
    }
}

fn decode_message(error: &DecodeError) -> String {
    match error {
        DecodeError::NotImplemented => "reader operation is not implemented".into(),
        DecodeError::BackendUnavailable { backend, message } => {
            format!("reader backend {backend:?} is unavailable: {message}")
        }
        DecodeError::UnsupportedFormat => "reader does not support the detected format".into(),
        DecodeError::MissingField => "required metadata field is absent".into(),
        DecodeError::AmbiguousField => "metadata field identity is ambiguous".into(),
        DecodeError::InvalidMetadata(message) => message.clone(),
        DecodeError::Io { message, .. } => message.clone(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::sync::Mutex;

    use tempfile::tempdir;
    use trajecta_case::model::meteorology::DatasetRef;
    use trajecta_case::resolver::sha256_hex;

    use super::*;
    use crate::field::Capability;
    use crate::profile::document::ProfileName;

    #[derive(Default)]
    struct FakeInspector {
        metadata: Mutex<BTreeMap<PathBuf, SourceMetadata>>,
    }

    impl SourceMetadataInspector for FakeInspector {
        fn inspect(
            &self,
            path: &Path,
            _format: SourceFormat,
        ) -> Result<SourceMetadata, DecodeError> {
            self.metadata
                .lock()
                .map_err(|_| DecodeError::InvalidMetadata("poisoned test mutex".into()))?
                .get(path)
                .cloned()
                .ok_or(DecodeError::MissingField)
        }
    }

    fn write_grib2(path: &Path, marker: u8) {
        let mut bytes = b"GRIB\0\0\0\x02".to_vec();
        bytes.extend([marker; 32]);
        fs::write(path, bytes).unwrap();
    }

    fn metadata(path: PathBuf, times: Vec<Timestamp>) -> SourceMetadata {
        SourceMetadata {
            path,
            format: SourceFormat::Grib2,
            attributes: BTreeMap::from([
                ("dataset_family".into(), "era5_flex_extract_hybrid".into()),
                ("centre".into(), "98".into()),
            ]),
            dimensions: BTreeMap::new(),
            valid_times: times,
            roles: vec!["analysis".into()],
            grid: Some(GridSignature {
                nx: 6,
                ny: 6,
                periodic_longitude: false,
                sha256: sha256_hex(b"grid"),
            }),
            vertical: Some(VerticalSignature::HybridPressure {
                full_level_count: 137,
                coefficients_sha256: sha256_hex(b"pv"),
            }),
        }
    }

    fn request(root: &Path) -> DatasetLockRequest {
        let mut capabilities = CapabilitySet::new();
        capabilities.insert(Capability::Transport);
        DatasetLockRequest {
            identity: DatasetIdentity {
                id: DatasetRef("era5".into()),
                source: "ECMWF ERA5".into(),
                source_url: None,
                attribution: None,
            },
            generator: GeneratorInfo {
                tool: "trajecta-data-lock".into(),
                version: "0.0.0".into(),
            },
            data_roots: BTreeMap::from([(DataRootId("met".into()), root.to_path_buf())]),
            coverage: LockCoverageRequest {
                start: Timestamp::new(10_800, 0).unwrap(),
                end: Timestamp::new(21_600, 0).unwrap(),
                interpolation_before_frames: 1,
                interpolation_after_frames: 1,
            },
            required_capabilities: capabilities,
            force_rehash: false,
            preferred_profile: None,
        }
    }

    #[test]
    fn real_lock_pipeline_selects_buffers_and_reuses_hash_cache() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("README.md"), "sidecar").unwrap();
        let inspector = FakeInspector::default();
        for (index, seconds) in [0_i64, 10_800, 21_600, 32_400].into_iter().enumerate() {
            let path = directory.path().join(format!("frame-{index}.grib"));
            write_grib2(&path, index as u8);
            let canonical = fs::canonicalize(&path).unwrap();
            inspector.metadata.lock().unwrap().insert(
                canonical.clone(),
                metadata(canonical, vec![Timestamp::new(seconds, 0).unwrap()]),
            );
        }
        let profiles = ProfileCatalog::load(&[]).unwrap();
        assert!(
            profiles
                .get(&ProfileName("era5-flex-extract-hybrid-v0".into()))
                .is_some()
        );
        let mut cache = FileHashCache::new();
        let mut builder = DatasetLockBuilder::new(&profiles, &inspector, &mut cache);
        let first = builder.build(&request(directory.path()));
        assert!(first.is_success(), "{:?}", first.diagnostics);
        assert_eq!(first.lock.as_ref().unwrap().files.len(), 4);
        assert_eq!(first.summary.files_hashed, 4);
        assert_eq!(first.summary.sidecar_files, 1);

        let second = builder.build(&request(directory.path()));
        assert!(second.is_success(), "{:?}", second.diagnostics);
        assert_eq!(second.summary.hash_cache_hits, 4);
        assert_eq!(first.lock, second.lock);
    }

    #[test]
    fn stage_gate_suppresses_profile_cascade_after_inspection_failure() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("broken.grib");
        write_grib2(&path, 1);
        let profiles = ProfileCatalog::load(&[]).unwrap();
        let inspector = FakeInspector::default();
        let mut cache = FileHashCache::new();
        let mut builder = DatasetLockBuilder::new(&profiles, &inspector, &mut cache);
        let outcome = builder.build(&request(directory.path()));
        assert!(outcome.lock.is_none());
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].stage, LockBuildStage::Inspect);
    }
}
