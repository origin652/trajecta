//! # Contract: component references
//!
//! A Case component is either inline or a reference to a controlled local
//! relative path. References cannot be absolute, escape through `..`, or
//! imply network access.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`ComponentRef`] | Inline value or local ref |
//! | [`RefPath`] | Validated relative path |
//! | [`ReferenceError`] | Path validation failure |
//!
//! ## Serde shape
//!
//! - Inline: the bare component value
//! - Ref: object with **only** `{ "ref": "relative/path.yaml" }`
//! - Objects that mix `ref` with inline fields are rejected

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::de::{self, DeserializeOwned, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

/// Inline value or deferred local component reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentRef<T> {
    /// Component embedded directly in the containing document.
    Inline(T),
    /// Component stored in another local document.
    Ref(RefPath),
}

impl<T: Serialize> Serialize for ComponentRef<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Inline(value) => value.serialize(serializer),
            Self::Ref(path) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("ref", &path.as_path().to_string_lossy())?;
                map.end()
            }
        }
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for ComponentRef<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_yml::Value::deserialize(deserializer)?;
        match value {
            serde_yml::Value::Mapping(map) => {
                let has_ref = map
                    .keys()
                    .any(|key| matches!(key, serde_yml::Value::String(s) if s == "ref"));
                if has_ref {
                    if map.len() != 1 {
                        return Err(de::Error::custom(
                            "component ref object must contain only the 'ref' field",
                        ));
                    }
                    let path_value = map
                        .get(serde_yml::Value::String("ref".into()))
                        .ok_or_else(|| de::Error::missing_field("ref"))?;
                    let path = match path_value {
                        serde_yml::Value::String(s) => s.clone(),
                        other => {
                            return Err(de::Error::custom(format!(
                                "ref must be a string path, got {other:?}"
                            )));
                        }
                    };
                    let ref_path = RefPath::new(path).map_err(de::Error::custom)?;
                    return Ok(Self::Ref(ref_path));
                }
                let inline: T = serde_yml::from_value(serde_yml::Value::Mapping(map))
                    .map_err(de::Error::custom)?;
                Ok(Self::Inline(inline))
            }
            other => {
                let inline: T = serde_yml::from_value(other).map_err(de::Error::custom)?;
                Ok(Self::Inline(inline))
            }
        }
    }
}

/// Validated local relative path used by component references.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RefPath(PathBuf);

impl RefPath {
    /// Validates a relative path without parent traversal.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ReferenceError> {
        let path = path.into();
        if path.as_os_str().is_empty() || path.is_absolute() {
            return Err(ReferenceError::InvalidPath(path));
        }
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(ReferenceError::InvalidPath(path));
        }
        Ok(Self(path))
    }

    /// Returns the validated relative path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RefPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let path = PathBuf::deserialize(deserializer)?;
        RefPath::new(path).map_err(de::Error::custom)
    }
}

impl TryFrom<PathBuf> for RefPath {
    type Error = ReferenceError;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RefPath> for PathBuf {
    fn from(value: RefPath) -> Self {
        value.0
    }
}

impl fmt::Display for RefPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}

/// Component-reference validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReferenceError {
    /// The path is empty, absolute, or contains parent traversal.
    InvalidPath(PathBuf),
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => {
                write!(f, "invalid component reference path: {}", path.display())
            }
        }
    }
}

impl std::error::Error for ReferenceError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::model::time::{Direction, TimeSpec, Timestamp};

    #[test]
    fn rejects_absolute_and_parent_paths() {
        assert!(RefPath::new("ok/time.yaml").is_ok());
        assert!(RefPath::new("").is_err());
        assert!(RefPath::new("/abs.yaml").is_err());
        assert!(RefPath::new("../escape.yaml").is_err());
        assert!(RefPath::new("a/../b.yaml").is_err());
    }

    #[test]
    fn component_ref_serde_inline_and_ref() {
        let time = TimeSpec {
            start: Timestamp::UNIX_EPOCH,
            end: Timestamp::new(3600, 0).unwrap(),
            direction: Direction::Forward,
        };
        let inline = ComponentRef::Inline(time);
        let yaml = serde_yml::to_string(&inline).unwrap();
        let back: ComponentRef<TimeSpec> = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(back, inline);

        let reference =
            ComponentRef::<TimeSpec>::Ref(RefPath::new("components/time.yaml").unwrap());
        let yaml = serde_yml::to_string(&reference).unwrap();
        assert!(yaml.contains("ref:"));
        let back: ComponentRef<TimeSpec> = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(back, reference);

        let json = serde_json::to_string(&reference).unwrap();
        let back: ComponentRef<TimeSpec> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reference);
    }

    #[test]
    fn rejects_mixed_ref_and_inline_fields() {
        let yaml = r#"
ref: components/time.yaml
start:
  seconds_since_unix_epoch: 0
  nanosecond: 0
"#;
        let err = serde_yml::from_str::<ComponentRef<TimeSpec>>(yaml).unwrap_err();
        assert!(err.to_string().contains("only the 'ref' field"));

        let json = r#"{"ref":"components/time.yaml","direction":"forward"}"#;
        assert!(serde_json::from_str::<ComponentRef<TimeSpec>>(json).is_err());
    }
}
