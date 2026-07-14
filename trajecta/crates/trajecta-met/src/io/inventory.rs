//! # Contract: validated meteorology inventory
//!
//! Inventory construction verifies locked payloads, exact Profile identity,
//! required capabilities, temporal coverage, grid and vertical signatures,
//! and deterministic multi-file logical-frame assembly before query runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use trajecta_case::diagnostic::{Diagnostic, DiagnosticBag};
use trajecta_case::document::DataRootId;
use trajecta_case::lockfile::{DatasetLock, GridSignature, LockedFile, VerticalSignature};
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;
use trajecta_case::resolver::path_is_within;

use crate::field::CapabilitySet;
use crate::profile::document::{ProfileCatalog, ProfileName};

/// Stable identity of one logical meteorological frame.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LogicalFrameId {
    /// Domain containing the frame.
    pub domain: DomainId,
    /// Physical validity time.
    pub valid_time: Timestamp,
    /// Exact profile content hash.
    pub profile_sha256: String,
    /// Aggregate hash of frame files and roles.
    pub content_sha256: String,
}

/// Exact local files and metadata forming one logical frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameDescriptor {
    /// Stable frame identity.
    pub id: LogicalFrameId,
    /// Deterministic file-role mapping.
    pub files: BTreeMap<String, PathBuf>,
    /// Verified horizontal grid signature.
    pub grid: GridSignature,
    /// Verified native vertical signature.
    pub vertical: VerticalSignature,
}

/// Time coverage and gaps for one domain.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoverageReport {
    /// First available frame time.
    pub first: Option<Timestamp>,
    /// Last available frame time.
    pub last: Option<Timestamp>,
    /// Missing expected validity times.
    pub gaps: Vec<Timestamp>,
}

/// Validated frame catalog for one logical domain.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DomainCatalog {
    /// Domain identity.
    pub domain: Option<DomainId>,
    /// Frames by exact validity time.
    pub frames: BTreeMap<Timestamp, FrameDescriptor>,
    /// Verified coverage summary.
    pub coverage: CoverageReport,
}

/// Fully validated catalog used to construct a meteorology engine.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MetCatalog {
    /// Domains by stable identifier.
    pub domains: BTreeMap<DomainId, DomainCatalog>,
    /// Capabilities guaranteed by all relevant frames and profiles.
    pub capabilities: CapabilitySet,
}

/// Inputs for building one domain catalog from an immutable lock.
#[derive(Clone, Copy, Debug)]
pub struct InventoryBuildRequest<'a> {
    /// Immutable dataset lock.
    pub lock: &'a DatasetLock,
    /// Directory containing the lock document and built-in `lockfile` root.
    pub lockfile_dir: &'a Path,
    /// Explicit machine-local named roots.
    pub data_roots: &'a BTreeMap<DataRootId, PathBuf>,
    /// Logical Case domain assigned to every assembled frame.
    pub domain: &'a DomainId,
    /// Capabilities required by this resolved run.
    pub required_capabilities: CapabilitySet,
}

/// Inventory result; warnings may accompany a usable catalog.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InventoryBuildOutcome {
    /// Completed catalog; absent when any error diagnostic exists.
    pub catalog: Option<MetCatalog>,
    /// Deterministic errors and warnings.
    pub diagnostics: DiagnosticBag,
}

impl InventoryBuildOutcome {
    /// Returns whether a usable catalog was produced.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.catalog.is_some() && !self.diagnostics.has_errors()
    }
}

/// Deterministic lock-to-catalog builder.
#[derive(Clone, Copy, Debug)]
pub struct InventoryBuilder<'a> {
    profiles: &'a ProfileCatalog,
}

impl<'a> InventoryBuilder<'a> {
    /// Creates a builder using one already-loaded exact Profile catalog.
    #[must_use]
    pub const fn new(profiles: &'a ProfileCatalog) -> Self {
        Self { profiles }
    }

    /// Verifies payloads and assembles immutable logical frames.
    pub fn build(&self, request: InventoryBuildRequest<'_>) -> InventoryBuildOutcome {
        let mut diagnostics = match request
            .lock
            .verify_local_files_with_roots(request.lockfile_dir, request.data_roots)
        {
            Ok(values) => values,
            Err(error) => {
                let mut values = DiagnosticBag::new();
                values.push(Diagnostic::error(
                    "inventory.lock.verify_io",
                    error.to_string(),
                ));
                return InventoryBuildOutcome {
                    catalog: None,
                    diagnostics: values,
                };
            }
        };
        if diagnostics.has_errors() {
            return InventoryBuildOutcome {
                catalog: None,
                diagnostics,
            };
        }

        let profile_name = ProfileName(request.lock.profile.name.clone());
        let Some(profile) = self.profiles.get(&profile_name) else {
            diagnostics.push(Diagnostic::error(
                "inventory.profile.name_missing",
                format!(
                    "locked Profile '{}' is not loaded in the active catalog",
                    request.lock.profile.name
                ),
            ));
            return InventoryBuildOutcome {
                catalog: None,
                diagnostics,
            };
        };
        if profile.sha256 != request.lock.profile.sha256 {
            diagnostics.push(
                Diagnostic::warning(
                    "inventory.profile.hash_changed",
                    format!(
                        "Profile '{}' content changed since lock creation",
                        request.lock.profile.name
                    ),
                )
                .with_hint(format!(
                    "locked={}, active={}",
                    request.lock.profile.sha256, profile.sha256
                )),
            );
        }
        for capability in request.required_capabilities.iter() {
            if !profile.document.capabilities.contains_key(&capability) {
                diagnostics.push(Diagnostic::error(
                    "inventory.capability.missing",
                    format!(
                        "Profile '{}' does not provide required capability {capability:?}",
                        profile.name().0
                    ),
                ));
            }
        }
        if diagnostics.has_errors() {
            return InventoryBuildOutcome {
                catalog: None,
                diagnostics,
            };
        }

        let resolved_paths = resolve_locked_paths(
            request.lock,
            request.lockfile_dir,
            request.data_roots,
            &mut diagnostics,
        );
        if diagnostics.has_errors() {
            return InventoryBuildOutcome {
                catalog: None,
                diagnostics,
            };
        }
        let times = request
            .lock
            .files
            .iter()
            .flat_map(|file| file.valid_times.iter().copied())
            .collect::<BTreeSet<_>>();
        if times.is_empty() {
            diagnostics.push(Diagnostic::error(
                "inventory.coverage.no_times",
                "locked meteorology contains no physical validity times",
            ));
            return InventoryBuildOutcome {
                catalog: None,
                diagnostics,
            };
        }

        let mut frames = BTreeMap::new();
        for valid_time in times {
            let mut files = BTreeMap::new();
            let mut frame_files = Vec::new();
            for (index, locked) in request.lock.files.iter().enumerate() {
                if !locked.valid_times.is_empty() && !locked.valid_times.contains(&valid_time) {
                    continue;
                }
                let Some(path) = resolved_paths.get(&index) else {
                    diagnostics.push(Diagnostic::error(
                        "inventory.path.internal_missing",
                        format!("resolved path missing for lock file index {index}"),
                    ));
                    continue;
                };
                for role in &locked.roles {
                    if let Some(previous) = files.insert(role.clone(), path.clone()) {
                        diagnostics.push(Diagnostic::error(
                            "inventory.frame.role_duplicate",
                            format!(
                                "valid time {:?} maps role '{}' to both '{}' and '{}'",
                                valid_time,
                                role,
                                previous.display(),
                                path.display()
                            ),
                        ));
                    }
                }
                frame_files.push(locked);
            }
            if files.is_empty() {
                diagnostics.push(Diagnostic::error(
                    "inventory.frame.empty",
                    format!("valid time {valid_time:?} has no locked source roles"),
                ));
                continue;
            }
            let content_sha256 = frame_content_hash(valid_time, &frame_files);
            let id = LogicalFrameId {
                domain: request.domain.clone(),
                valid_time,
                profile_sha256: profile.sha256.clone(),
                content_sha256,
            };
            frames.insert(
                valid_time,
                FrameDescriptor {
                    id,
                    files,
                    grid: request.lock.grid.clone(),
                    vertical: request.lock.vertical.clone(),
                },
            );
        }
        if diagnostics.has_errors() {
            return InventoryBuildOutcome {
                catalog: None,
                diagnostics,
            };
        }

        let coverage = coverage_report(
            frames.keys().copied().collect(),
            profile.document.frame_interval_seconds,
            &mut diagnostics,
        );
        if !coverage.gaps.is_empty() {
            diagnostics.push(Diagnostic::error(
                "inventory.coverage.gaps",
                format!(
                    "logical frame coverage contains {} missing expected times",
                    coverage.gaps.len()
                ),
            ));
        }
        if diagnostics.has_errors() {
            return InventoryBuildOutcome {
                catalog: None,
                diagnostics,
            };
        }

        let mut capabilities = CapabilitySet::new();
        for capability in profile.document.capabilities.keys() {
            capabilities.insert(*capability);
        }
        let domain_catalog = DomainCatalog {
            domain: Some(request.domain.clone()),
            frames,
            coverage,
        };
        InventoryBuildOutcome {
            catalog: Some(MetCatalog {
                domains: BTreeMap::from([(request.domain.clone(), domain_catalog)]),
                capabilities,
            }),
            diagnostics,
        }
    }
}

fn resolve_locked_paths(
    lock: &DatasetLock,
    lockfile_dir: &Path,
    data_roots: &BTreeMap<DataRootId, PathBuf>,
    diagnostics: &mut DiagnosticBag,
) -> BTreeMap<usize, PathBuf> {
    let mut roots = BTreeMap::new();
    match fs::canonicalize(lockfile_dir) {
        Ok(root) => {
            roots.insert(DataRootId(DataRootId::LOCKFILE.into()), root);
        }
        Err(error) => {
            diagnostics.push(Diagnostic::error(
                "inventory.root.lockfile_unreadable",
                error.to_string(),
            ));
            return BTreeMap::new();
        }
    }
    for (root_id, path) in data_roots {
        match fs::canonicalize(path) {
            Ok(root) => {
                roots.insert(root_id.clone(), root);
            }
            Err(error) => diagnostics.push(Diagnostic::error(
                "inventory.root.unreadable",
                format!("cannot resolve root '{}': {error}", root_id.0),
            )),
        }
    }
    let mut resolved = BTreeMap::new();
    for (index, file) in lock.files.iter().enumerate() {
        let Some(root) = roots.get(&file.root_id) else {
            diagnostics.push(Diagnostic::error(
                "inventory.root.unknown",
                format!("unknown data root '{}'", file.root_id.0),
            ));
            continue;
        };
        match fs::canonicalize(root.join(&file.relative_path)) {
            Ok(path)
                if roots
                    .values()
                    .any(|authorized| path_is_within(&path, authorized)) =>
            {
                resolved.insert(index, path);
            }
            Ok(path) => diagnostics.push(Diagnostic::error(
                "inventory.path.escape",
                format!(
                    "resolved path '{}' escapes authorized roots",
                    path.display()
                ),
            )),
            Err(error) => diagnostics.push(Diagnostic::error(
                "inventory.path.unreadable",
                format!(
                    "cannot resolve '{}:{}': {error}",
                    file.root_id.0,
                    file.relative_path.display()
                ),
            )),
        }
    }
    resolved
}

fn frame_content_hash(valid_time: Timestamp, files: &[&LockedFile]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(valid_time.seconds_since_unix_epoch().to_be_bytes());
    hasher.update(valid_time.nanosecond().to_be_bytes());
    for file in files {
        update_length_prefixed(&mut hasher, file.root_id.0.as_bytes());
        update_length_prefixed(&mut hasher, file.relative_path.to_string_lossy().as_bytes());
        update_length_prefixed(&mut hasher, file.sha256.as_bytes());
        for role in &file.roles {
            update_length_prefixed(&mut hasher, role.as_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

fn update_length_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn coverage_report(
    times: Vec<Timestamp>,
    interval_seconds: Option<u64>,
    diagnostics: &mut DiagnosticBag,
) -> CoverageReport {
    let first = times.first().copied();
    let last = times.last().copied();
    let Some(interval) = interval_seconds else {
        return CoverageReport {
            first,
            last,
            gaps: Vec::new(),
        };
    };
    let Ok(interval) = i64::try_from(interval) else {
        diagnostics.push(Diagnostic::error(
            "inventory.coverage.interval_too_large",
            "Profile frame_interval_seconds exceeds i64",
        ));
        return CoverageReport {
            first,
            last,
            gaps: Vec::new(),
        };
    };
    let mut gaps = Vec::new();
    for pair in times.windows(2) {
        let before = pair[0];
        let after = pair[1];
        if before.nanosecond() != after.nanosecond() {
            diagnostics.push(Diagnostic::error(
                "inventory.coverage.subsecond_irregular",
                "logical frame nanoseconds are not aligned",
            ));
            continue;
        }
        let Some(delta) = after
            .seconds_since_unix_epoch()
            .checked_sub(before.seconds_since_unix_epoch())
        else {
            diagnostics.push(Diagnostic::error(
                "inventory.coverage.delta_overflow",
                "logical frame time delta overflowed i64",
            ));
            continue;
        };
        if delta <= 0 || delta % interval != 0 {
            diagnostics.push(Diagnostic::error(
                "inventory.coverage.irregular_spacing",
                format!("frame spacing {delta} seconds is not a positive multiple of {interval}"),
            ));
            continue;
        }
        let mut missing = before.seconds_since_unix_epoch().checked_add(interval);
        while let Some(seconds) = missing {
            if seconds >= after.seconds_since_unix_epoch() {
                break;
            }
            if let Ok(time) = Timestamp::new(seconds, before.nanosecond()) {
                gaps.push(time);
            }
            missing = seconds.checked_add(interval);
        }
    }
    CoverageReport { first, last, gaps }
}

/// Inventory build failure retained for lower-level adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryError {
    /// Locked local file is absent or changed.
    InvalidLockedFile(PathBuf),
    /// Requested time coverage contains gaps.
    TimeCoverage(CoverageReport),
    /// Grid signatures change within a logical domain.
    GridMismatch(DomainId),
    /// Vertical signatures change within a logical domain.
    VerticalMismatch(DomainId),
    /// Required capabilities are unavailable.
    MissingCapabilities,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use tempfile::tempdir;
    use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo, ProfileIdentity};
    use trajecta_case::model::meteorology::DatasetRef;
    use trajecta_case::resolver::{sha256_file_streaming, sha256_hex};

    use super::*;
    use crate::field::Capability;

    fn lock(root_id: DataRootId, file: &Path, profile_sha256: String) -> DatasetLock {
        let (size_bytes, sha256) = sha256_file_streaming(file).unwrap();
        DatasetLock {
            schema_version: 0,
            identity: DatasetIdentity {
                id: DatasetRef("era5".into()),
                source: "ECMWF".into(),
                source_url: None,
                attribution: None,
            },
            profile: ProfileIdentity {
                name: "era5-flex-extract-hybrid-v0".into(),
                sha256: profile_sha256,
            },
            generator: GeneratorInfo {
                tool: "test".into(),
                version: "0".into(),
            },
            files: vec![LockedFile {
                roles: vec!["analysis".into()],
                root_id,
                relative_path: PathBuf::from("met.grib"),
                valid_times: vec![
                    Timestamp::new(0, 0).unwrap(),
                    Timestamp::new(10_800, 0).unwrap(),
                    Timestamp::new(21_600, 0).unwrap(),
                ],
                size_bytes,
                sha256,
            }],
            grid: GridSignature {
                nx: 6,
                ny: 6,
                periodic_longitude: false,
                sha256: sha256_hex(b"grid"),
            },
            vertical: VerticalSignature::HybridPressure {
                full_level_count: 137,
                coefficients_sha256: sha256_hex(b"pv"),
            },
        }
    }

    #[test]
    fn lock_builds_three_deterministic_frames_and_warns_on_profile_hash_change() {
        let directory = tempdir().unwrap();
        let file = directory.path().join("met.grib");
        fs::write(&file, b"immutable payload").unwrap();
        let profiles = ProfileCatalog::load(&[]).unwrap();
        let profile = profiles
            .get(&ProfileName("era5-flex-extract-hybrid-v0".into()))
            .unwrap();
        let mut locked = lock(DataRootId("met".into()), &file, profile.sha256.clone());
        let roots = BTreeMap::from([(DataRootId("met".into()), directory.path().to_path_buf())]);
        let domain = DomainId("global".into());
        let mut required = CapabilitySet::new();
        required.insert(Capability::Transport);
        let builder = InventoryBuilder::new(&profiles);
        let outcome = builder.build(InventoryBuildRequest {
            lock: &locked,
            lockfile_dir: directory.path(),
            data_roots: &roots,
            domain: &domain,
            required_capabilities: required,
        });
        assert!(outcome.is_success(), "{:?}", outcome.diagnostics.sorted());
        assert_eq!(
            outcome
                .catalog
                .as_ref()
                .unwrap()
                .domains
                .get(&domain)
                .unwrap()
                .frames
                .len(),
            3
        );

        locked.profile.sha256 = sha256_hex(b"old profile");
        let changed = builder.build(InventoryBuildRequest {
            lock: &locked,
            lockfile_dir: directory.path(),
            data_roots: &roots,
            domain: &domain,
            required_capabilities: required,
        });
        assert!(changed.is_success());
        assert!(changed.diagnostics.has_warnings());
    }
}
