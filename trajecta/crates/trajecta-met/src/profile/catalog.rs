//! # Contract: deterministic Profile loading
//!
//! Built-in Profiles are syntax-checked by the build script, embedded in the
//! binary, and fully schema/type checked when the catalog is constructed.
//! Private sources are explicit files or non-recursive directories. Directory
//! order and filesystem enumeration order cannot affect catalog contents.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use trajecta_case::document::ProfileSource;

use crate::profile::document::{
    DatasetProfile, ProfileCatalog, ProfileError, parse_profile_json, parse_profile_yaml,
};

include!(concat!(env!("OUT_DIR"), "/built_in_profiles.rs"));

const MAX_PROFILE_BYTES: u64 = 4 * 1024 * 1024;

/// Provenance of one loaded Profile document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileOrigin {
    /// Embedded repository resource.
    BuiltIn(String),
    /// Explicit machine-local file.
    File(PathBuf),
    /// Profile registered directly through the library API.
    Programmatic,
}

impl ProfileCatalog {
    /// Loads all embedded repository Profiles and then explicit private sources.
    pub fn load(profile_sources: &[ProfileSource]) -> Result<Self, ProfileLoadError> {
        let mut catalog = Self::new();
        catalog.load_built_ins()?;
        catalog.load_sources(profile_sources)?;
        Ok(catalog)
    }

    /// Loads all build-time embedded Profiles.
    pub fn load_built_ins(&mut self) -> Result<(), ProfileLoadError> {
        for (resource, text) in BUILT_IN_PROFILE_DOCUMENTS {
            let profile = parse_profile_text(Path::new(resource), text).map_err(|error| {
                ProfileLoadError::InvalidProfile {
                    path: PathBuf::from(resource),
                    message: error.to_string(),
                }
            })?;
            self.register_with_origin(profile, ProfileOrigin::BuiltIn((*resource).into()))
                .map_err(|error| ProfileLoadError::InvalidProfile {
                    path: PathBuf::from(resource),
                    message: error.to_string(),
                })?;
        }
        Ok(())
    }

    /// Loads explicit files and non-recursive directories in declared order.
    pub fn load_sources(&mut self, sources: &[ProfileSource]) -> Result<(), ProfileLoadError> {
        for source in sources {
            match source {
                ProfileSource::File { path } => self.load_file(path)?,
                ProfileSource::Directory { path } => self.load_directory(path)?,
            }
        }
        Ok(())
    }

    /// Loads exactly one YAML or JSON Profile file.
    pub fn load_file(&mut self, path: &Path) -> Result<(), ProfileLoadError> {
        let metadata = fs::metadata(path).map_err(|error| ProfileLoadError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if !metadata.is_file() {
            return Err(ProfileLoadError::NotFile(path.to_path_buf()));
        }
        if metadata.len() > MAX_PROFILE_BYTES {
            return Err(ProfileLoadError::TooLarge {
                path: path.to_path_buf(),
                size_bytes: metadata.len(),
                limit_bytes: MAX_PROFILE_BYTES,
            });
        }
        let text = fs::read_to_string(path).map_err(|error| ProfileLoadError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let profile =
            parse_profile_text(path, &text).map_err(|error| ProfileLoadError::InvalidProfile {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        self.register_with_origin(profile, ProfileOrigin::File(path.to_path_buf()))
            .map_err(|error| ProfileLoadError::InvalidProfile {
                path: path.to_path_buf(),
                message: error.to_string(),
            })
    }

    /// Loads regular Profile files from one directory without recursion.
    pub fn load_directory(&mut self, path: &Path) -> Result<(), ProfileLoadError> {
        let metadata = fs::metadata(path).map_err(|error| ProfileLoadError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if !metadata.is_dir() {
            return Err(ProfileLoadError::NotDirectory(path.to_path_buf()));
        }
        let mut files = fs::read_dir(path)
            .map_err(|error| ProfileLoadError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?
            .map(|entry| {
                entry
                    .map(|value| value.path())
                    .map_err(|error| ProfileLoadError::Io {
                        path: path.to_path_buf(),
                        message: error.to_string(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        files.retain(|candidate| candidate.is_file() && is_profile_extension(candidate));
        files.sort();
        for file in files {
            self.load_file(&file)?;
        }
        Ok(())
    }
}

fn parse_profile_text(path: &Path, text: &str) -> Result<DatasetProfile, ProfileError> {
    match normalized_extension(path).as_deref() {
        Some("yaml" | "yml") => parse_profile_yaml(text),
        Some("json") => parse_profile_json(text),
        _ => Err(ProfileError::Parse(format!(
            "unsupported Profile extension for '{}'",
            path.display()
        ))),
    }
}

fn is_profile_extension(path: &Path) -> bool {
    matches!(
        normalized_extension(path).as_deref(),
        Some("yaml" | "yml" | "json")
    )
}

fn normalized_extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_str()?.to_ascii_lowercase())
}

/// Profile filesystem or embedded-resource loading failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileLoadError {
    /// Filesystem operation failed.
    Io {
        /// Affected path.
        path: PathBuf,
        /// Sanitized platform message.
        message: String,
    },
    /// An explicit file source is not a regular file.
    NotFile(PathBuf),
    /// An explicit directory source is not a directory.
    NotDirectory(PathBuf),
    /// A Profile exceeds the bounded document size.
    TooLarge {
        /// Affected path.
        path: PathBuf,
        /// Actual byte size.
        size_bytes: u64,
        /// Accepted byte limit.
        limit_bytes: u64,
    },
    /// Parsing, schema validation, type checking, or duplicate-name registration failed.
    InvalidProfile {
        /// Affected resource or path.
        path: PathBuf,
        /// Stable explanation.
        message: String,
    },
}

impl fmt::Display for ProfileLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(
                    formatter,
                    "cannot load Profile '{}': {message}",
                    path.display()
                )
            }
            Self::NotFile(path) => write!(
                formatter,
                "Profile source '{}' is not a file",
                path.display()
            ),
            Self::NotDirectory(path) => {
                write!(
                    formatter,
                    "Profile source '{}' is not a directory",
                    path.display()
                )
            }
            Self::TooLarge {
                path,
                size_bytes,
                limit_bytes,
            } => write!(
                formatter,
                "Profile '{}' has {size_bytes} bytes, exceeding {limit_bytes}",
                path.display()
            ),
            Self::InvalidProfile { path, message } => {
                write!(formatter, "invalid Profile '{}': {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ProfileLoadError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::profile::document::ProfileName;

    use super::*;

    fn unique_directory() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("trajecta-profile-{}-{nonce}", std::process::id()))
    }

    fn private_profile(name: &str) -> String {
        format!(
            r#"schema_version: 0
kind: dataset_profile
name: {name}
source_matchers:
  - format: grib2
    centre: 7
fields:
  - id: temperature
    target: air_temperature
    sources:
      - identity: {{ discipline: "0", category: "0", parameter: "0" }}
        unit: K
"#
        )
    }

    #[test]
    fn built_in_profiles_are_embedded_and_fully_validated() {
        let catalog = ProfileCatalog::load(&[]).unwrap();
        let name = ProfileName("era5-flex-extract-hybrid-v0".into());
        assert!(catalog.get(&name).is_some());
        assert!(matches!(
            catalog.origin(&name),
            Some(ProfileOrigin::BuiltIn(_))
        ));
    }

    #[test]
    fn private_directories_are_non_recursive_sorted_and_extension_filtered() {
        let root = unique_directory();
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("b.yaml"), private_profile("private-b")).unwrap();
        fs::write(root.join("a.yml"), private_profile("private-a")).unwrap();
        fs::write(root.join("notes.txt"), "ignored").unwrap();
        fs::write(
            root.join("nested").join("hidden.yaml"),
            private_profile("hidden"),
        )
        .unwrap();

        let catalog =
            ProfileCatalog::load(&[ProfileSource::Directory { path: root.clone() }]).unwrap();
        assert!(catalog.get(&ProfileName("private-a".into())).is_some());
        assert!(catalog.get(&ProfileName("private-b".into())).is_some());
        assert!(catalog.get(&ProfileName("hidden".into())).is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
