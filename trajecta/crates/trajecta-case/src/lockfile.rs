//! # Contract: immutable dataset lock
//!
//! A lock names every file used by one logical dataset and records enough
//! identity to reject silent changes in content, grid, vertical topology, or
//! interpretation profile before a run starts.
//!
//! ## Exposed interface
//!
//! | Type | Role |
//! |---|---|
//! | [`DatasetLock`] | Full lock document |
//! | [`parse_dataset_lock_json`] / [`parse_dataset_lock_yaml`] | Loaders |
//! | [`DatasetLock::validate_shape`] | Shape-only checks |
//! | [`DatasetLock::verify_local_files`] | Streaming size + SHA-256 verification |
//!
//! Meteorological payloads are digested with a fixed-size streaming buffer and
//! never loaded fully into memory.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostic::{Diagnostic, DiagnosticBag, DiagnosticPath};
use crate::model::meteorology::DatasetRef;
use crate::model::time::Timestamp;
use crate::resolver::{
    ResolveError, is_lowercase_sha256_hex, path_is_within, sha256_file_streaming,
};
use crate::schema::CURRENT_SCHEMA_VERSION;

/// Logical dataset provenance independent of local paths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetIdentity {
    /// Logical dataset identifier used by Cases.
    pub id: DatasetRef,
    /// Human-readable upstream provider or product.
    pub source: String,
    /// Optional ownership or organizational attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

/// Exact interpretation profile used for a locked dataset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileIdentity {
    /// Unique profile name in the active catalog.
    pub name: String,
    /// SHA-256 digest of normalized profile content.
    pub sha256: String,
}

/// Identity of one immutable local source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedFile {
    /// Stable role within a logical frame or dataset.
    pub role: String,
    /// Path relative to the lockfile location.
    pub relative_path: PathBuf,
    /// Optional physical validity time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_time: Option<Timestamp>,
    /// Exact file length.
    pub size_bytes: u64,
    /// Lowercase SHA-256 hexadecimal digest.
    pub sha256: String,
}

/// Stable regular-grid summary used to reject topology changes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridSignature {
    /// Number of longitude or x points.
    pub nx: usize,
    /// Number of latitude or y points.
    pub ny: usize,
    /// Whether longitude wraps periodically.
    pub periodic_longitude: bool,
    /// Digest of normalized coordinate and grid-mapping metadata.
    pub sha256: String,
}

/// Stable native vertical-coordinate summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VerticalSignature {
    /// Hybrid pressure coordinate with interface A/B coefficients.
    HybridPressure {
        /// Number of full model layers.
        full_level_count: usize,
        /// Digest of the complete interface coefficient arrays.
        coefficients_sha256: String,
    },
    /// Fixed pressure levels.
    PressureLevels {
        /// Number of pressure levels.
        level_count: usize,
        /// Digest of the exact ordered pressure values.
        levels_sha256: String,
    },
}

/// Immutable file and interpretation lock for one logical dataset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetLock {
    /// Lock schema version.
    pub schema_version: u32,
    /// Logical provenance.
    pub identity: DatasetIdentity,
    /// Exact profile content identity.
    pub profile: ProfileIdentity,
    /// Deterministically ordered source files.
    pub files: Vec<LockedFile>,
    /// Stable horizontal grid signature.
    pub grid: GridSignature,
    /// Stable native vertical-coordinate signature.
    pub vertical: VerticalSignature,
}

/// Lock parse or verification failure that is not a soft diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockError {
    /// Unsupported schema version.
    UnsupportedVersion(u32),
    /// Document text could not be decoded.
    Parse(String),
    /// Local file I/O failed during verification.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Sanitized message.
        message: String,
    },
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(f, "unsupported lock schema_version: {v}"),
            Self::Parse(message) => write!(f, "lock parse error: {message}"),
            Self::Io { path, message } => {
                write!(f, "lock I/O error at {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for LockError {}

impl From<ResolveError> for LockError {
    fn from(value: ResolveError) -> Self {
        match value {
            ResolveError::Io { path, message } => Self::Io { path, message },
            other => Self::Io {
                path: PathBuf::from("<resolve>"),
                message: other.to_string(),
            },
        }
    }
}

impl DatasetLock {
    /// Validates lock shape without reading meteorological payloads.
    #[must_use]
    pub fn validate_shape(&self) -> DiagnosticBag {
        let mut diagnostics = DiagnosticBag::new();
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            diagnostics.push(
                Diagnostic::error(
                    "lock.unsupported_version",
                    format!(
                        "lock schema_version {} is not supported (expected {CURRENT_SCHEMA_VERSION})",
                        self.schema_version
                    ),
                )
                .at(DiagnosticPath::root().field("schema_version")),
            );
        }
        if self.identity.id.0.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error("lock.identity.id_empty", "identity.id must not be empty")
                    .at(DiagnosticPath::root().field("identity").field("id")),
            );
        }
        if self.identity.source.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error(
                    "lock.identity.source_empty",
                    "identity.source must not be empty",
                )
                .at(DiagnosticPath::root().field("identity").field("source")),
            );
        }
        if self.profile.name.trim().is_empty() {
            diagnostics.push(
                Diagnostic::error("lock.profile.name_empty", "profile.name must not be empty")
                    .at(DiagnosticPath::root().field("profile").field("name")),
            );
        }
        if !is_lowercase_sha256_hex(&self.profile.sha256) {
            diagnostics.push(
                Diagnostic::error(
                    "lock.profile.sha256_invalid",
                    "profile.sha256 must be 64 lowercase hex characters",
                )
                .at(DiagnosticPath::root().field("profile").field("sha256")),
            );
        }
        if self.files.is_empty() {
            diagnostics.push(
                Diagnostic::error("lock.files_empty", "files must contain at least one entry")
                    .at(DiagnosticPath::root().field("files")),
            );
        }

        let mut roles = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut normalized_paths = Vec::new();
        for (index, file) in self.files.iter().enumerate() {
            validate_locked_file(file, index, &mut diagnostics);
            if !file.role.trim().is_empty() && !roles.insert(file.role.clone()) {
                diagnostics.push(
                    Diagnostic::error(
                        "lock.file.role_duplicate",
                        format!("duplicate file role '{}'", file.role),
                    )
                    .at(DiagnosticPath::root()
                        .field("files")
                        .index(index)
                        .field("role")),
                );
            }
            if is_safe_relative(&file.relative_path) {
                let key = normalize_relative_display(&file.relative_path);
                if !paths.insert(key.clone()) {
                    diagnostics.push(
                        Diagnostic::error(
                            "lock.file.path_duplicate",
                            format!("duplicate relative_path '{key}'"),
                        )
                        .at(DiagnosticPath::root()
                            .field("files")
                            .index(index)
                            .field("relative_path")),
                    );
                }
                normalized_paths.push(key);
            }
        }
        let mut sorted_paths = normalized_paths.clone();
        sorted_paths.sort();
        if normalized_paths != sorted_paths {
            diagnostics.push(
                Diagnostic::error(
                    "lock.files_not_sorted",
                    "files must be ordered deterministically by relative_path",
                )
                .at(DiagnosticPath::root().field("files"))
                .with_hint("sort locked files by relative_path ascending"),
            );
        }

        if self.grid.nx == 0 || self.grid.ny == 0 {
            diagnostics.push(
                Diagnostic::error(
                    "lock.grid.extent_zero",
                    "grid.nx and grid.ny must be positive",
                )
                .at(DiagnosticPath::root().field("grid")),
            );
        }
        if !is_lowercase_sha256_hex(&self.grid.sha256) {
            diagnostics.push(
                Diagnostic::error(
                    "lock.grid.sha256_invalid",
                    "grid.sha256 must be 64 lowercase hex characters",
                )
                .at(DiagnosticPath::root().field("grid").field("sha256")),
            );
        }
        match &self.vertical {
            VerticalSignature::HybridPressure {
                full_level_count,
                coefficients_sha256,
            } => {
                if *full_level_count == 0 {
                    diagnostics.push(
                        Diagnostic::error(
                            "lock.vertical.level_count_zero",
                            "hybrid full_level_count must be positive",
                        )
                        .at(DiagnosticPath::root().field("vertical")),
                    );
                }
                if !is_lowercase_sha256_hex(coefficients_sha256) {
                    diagnostics.push(
                        Diagnostic::error(
                            "lock.vertical.sha256_invalid",
                            "coefficients_sha256 must be 64 lowercase hex characters",
                        )
                        .at(DiagnosticPath::root().field("vertical")),
                    );
                }
            }
            VerticalSignature::PressureLevels {
                level_count,
                levels_sha256,
            } => {
                if *level_count == 0 {
                    diagnostics.push(
                        Diagnostic::error(
                            "lock.vertical.level_count_zero",
                            "pressure level_count must be positive",
                        )
                        .at(DiagnosticPath::root().field("vertical")),
                    );
                }
                if !is_lowercase_sha256_hex(levels_sha256) {
                    diagnostics.push(
                        Diagnostic::error(
                            "lock.vertical.sha256_invalid",
                            "levels_sha256 must be 64 lowercase hex characters",
                        )
                        .at(DiagnosticPath::root().field("vertical")),
                    );
                }
            }
        }
        diagnostics
    }

    /// Verifies locked files under `lockfile_dir` with streaming SHA-256.
    ///
    /// Both the lock directory and each candidate file are canonicalized.
    /// Paths that escape the lock directory via `..`, symlinks, or junctions
    /// are rejected.
    pub fn verify_local_files(&self, lockfile_dir: &Path) -> Result<DiagnosticBag, LockError> {
        let mut diagnostics = self.validate_shape();
        let root = fs::canonicalize(lockfile_dir).map_err(|error| LockError::Io {
            path: lockfile_dir.to_path_buf(),
            message: error.to_string(),
        })?;

        for (index, file) in self.files.iter().enumerate() {
            if !is_safe_relative(&file.relative_path) {
                diagnostics.push(
                    Diagnostic::error(
                        "lock.file.path_unsafe",
                        "relative_path must be relative and must not contain '..'",
                    )
                    .at(DiagnosticPath::root()
                        .field("files")
                        .index(index)
                        .field("relative_path")),
                );
                continue;
            }

            let joined = root.join(&file.relative_path);
            let canonical = match fs::canonicalize(&joined) {
                Ok(path) => path,
                Err(error) => {
                    diagnostics.push(
                        Diagnostic::error(
                            "lock.file.missing",
                            format!("failed to open {}: {error}", joined.display()),
                        )
                        .at(DiagnosticPath::root()
                            .field("files")
                            .index(index)
                            .field("relative_path")),
                    );
                    continue;
                }
            };

            if !path_is_within(&canonical, &root) {
                diagnostics.push(
                    Diagnostic::error(
                        "lock.file.path_escapes_root",
                        format!(
                            "canonical path {} escapes lock directory {}",
                            canonical.display(),
                            root.display()
                        ),
                    )
                    .at(DiagnosticPath::root()
                        .field("files")
                        .index(index)
                        .field("relative_path")),
                );
                continue;
            }

            let (size, digest) = sha256_file_streaming(&canonical)?;
            if size != file.size_bytes {
                diagnostics.push(
                    Diagnostic::error(
                        "lock.file.size_mismatch",
                        format!(
                            "size mismatch: lock {} bytes, actual {size} bytes",
                            file.size_bytes
                        ),
                    )
                    .at(DiagnosticPath::root()
                        .field("files")
                        .index(index)
                        .field("size_bytes")),
                );
            }
            if digest != file.sha256 {
                diagnostics.push(
                    Diagnostic::error(
                        "lock.file.sha256_mismatch",
                        format!("sha256 mismatch for {}", file.relative_path.display()),
                    )
                    .at(DiagnosticPath::root()
                        .field("files")
                        .index(index)
                        .field("sha256"))
                    .with_hint(format!("actual={digest}")),
                );
            }
        }
        Ok(diagnostics)
    }
}

fn validate_locked_file(file: &LockedFile, index: usize, diagnostics: &mut DiagnosticBag) {
    let base = DiagnosticPath::root().field("files").index(index);
    if file.role.trim().is_empty() {
        diagnostics.push(
            Diagnostic::error("lock.file.role_empty", "file role must not be empty")
                .at(base.clone().field("role")),
        );
    }
    if file.relative_path.as_os_str().is_empty() || !is_safe_relative(&file.relative_path) {
        diagnostics.push(
            Diagnostic::error(
                "lock.file.path_unsafe",
                "relative_path must be a non-empty safe relative path",
            )
            .at(base.clone().field("relative_path")),
        );
    }
    if !is_lowercase_sha256_hex(&file.sha256) {
        diagnostics.push(
            Diagnostic::error(
                "lock.file.sha256_invalid",
                "file sha256 must be 64 lowercase hex characters",
            )
            .at(base.field("sha256")),
        );
    }
}

fn is_safe_relative(path: &Path) -> bool {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return false;
    }
    !path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

fn normalize_relative_display(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Parses a dataset lock from JSON text.
pub fn parse_dataset_lock_json(input: &str) -> Result<DatasetLock, LockError> {
    let lock: DatasetLock =
        serde_json::from_str(input).map_err(|error| LockError::Parse(error.to_string()))?;
    if lock.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(LockError::UnsupportedVersion(lock.schema_version));
    }
    Ok(lock)
}

/// Parses a dataset lock from YAML text.
pub fn parse_dataset_lock_yaml(input: &str) -> Result<DatasetLock, LockError> {
    let lock: DatasetLock =
        serde_yml::from_str(input).map_err(|error| LockError::Parse(error.to_string()))?;
    if lock.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(LockError::UnsupportedVersion(lock.schema_version));
    }
    Ok(lock)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::resolver::sha256_hex;
    use tempfile::tempdir;

    fn sample_hex(seed: u8) -> String {
        hex::encode([seed; 32])
    }

    fn sample_lock(file_name: &str, bytes: &[u8]) -> DatasetLock {
        DatasetLock {
            schema_version: 0,
            identity: DatasetIdentity {
                id: DatasetRef("era5".into()),
                source: "ECMWF ERA5".into(),
                attribution: Some("CDS".into()),
            },
            profile: ProfileIdentity {
                name: "era5_hybrid_grib".into(),
                sha256: sample_hex(1),
            },
            files: vec![LockedFile {
                role: "analysis".into(),
                relative_path: PathBuf::from(file_name),
                valid_time: Some(Timestamp::UNIX_EPOCH),
                size_bytes: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            }],
            grid: GridSignature {
                nx: 360,
                ny: 181,
                periodic_longitude: true,
                sha256: sample_hex(2),
            },
            vertical: VerticalSignature::HybridPressure {
                full_level_count: 137,
                coefficients_sha256: sample_hex(3),
            },
        }
    }

    #[test]
    fn lock_json_roundtrip_and_shape_ok() {
        let lock = sample_lock("met/file.grib", b"grib-bytes");
        let json = serde_json::to_string_pretty(&lock).unwrap();
        let back = parse_dataset_lock_json(&json).unwrap();
        assert_eq!(back, lock);
        assert!(!back.validate_shape().has_errors());
    }

    #[test]
    fn verify_local_files_streaming_detects_tampering() {
        let dir = tempdir().unwrap();
        let payload = vec![9_u8; 120_000];
        let lock = sample_lock("data.bin", &payload);
        fs::write(dir.path().join("data.bin"), &payload).unwrap();
        let ok = lock.verify_local_files(dir.path()).unwrap();
        assert!(!ok.has_errors(), "{:?}", ok.sorted());

        fs::write(dir.path().join("data.bin"), b"tampered").unwrap();
        let bad = lock.verify_local_files(dir.path()).unwrap();
        assert!(bad.has_errors());
    }

    #[test]
    fn rejects_parent_path_escape() {
        let mut lock = sample_lock("x.bin", b"x");
        lock.files[0].relative_path = PathBuf::from("../escape.bin");
        let bag = lock.validate_shape();
        assert!(bag.iter().any(|d| d.code() == "lock.file.path_unsafe"));
    }

    #[test]
    fn rejects_symlink_escape_when_supported() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("lockroot");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("secret.bin");
        fs::write(&outside, b"secret-bytes").unwrap();
        let link = root.join("linked.bin");

        let symlink_ok = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&outside, &link).is_ok()
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(&outside, &link).is_ok()
            }
            #[cfg(not(any(unix, windows)))]
            {
                false
            }
        };

        if !symlink_ok {
            // Environments without symlink privilege still exercise parent escape above.
            return;
        }

        let lock = sample_lock("linked.bin", b"secret-bytes");
        let bag = lock.verify_local_files(&root).unwrap();
        assert!(
            bag.iter()
                .any(|d| d.code() == "lock.file.path_escapes_root"),
            "{:?}",
            bag.sorted()
        );
    }

    #[test]
    fn rejects_duplicate_roles_and_unsorted_paths() {
        let mut lock = sample_lock("b.bin", b"b");
        lock.files.push(LockedFile {
            role: "analysis".into(),
            relative_path: PathBuf::from("a.bin"),
            valid_time: None,
            size_bytes: 1,
            sha256: sample_hex(9),
        });
        let bag = lock.validate_shape();
        assert!(bag.iter().any(|d| d.code() == "lock.file.role_duplicate"));
        assert!(bag.iter().any(|d| d.code() == "lock.files_not_sorted"));
    }
}
