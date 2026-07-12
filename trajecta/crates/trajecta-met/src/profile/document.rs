//! # Contract: profile documents and catalog
//!
//! Profile names are unique within one loaded catalog, while exact content is
//! locked by SHA-256. Automatic recognition must produce exactly one match;
//! ambiguous or absent matches are errors.

use std::collections::BTreeMap;

use crate::field::{FieldKey, FieldQuality};

/// Unique profile name in the current catalog.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProfileName(pub String);

/// Exact source identity used during profile matching.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DatasetFingerprint {
    /// Exact normalized identity attributes.
    pub attributes: BTreeMap<String, String>,
}

/// Exact GRIB source matcher.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GribSourceMatcher {
    /// Optional originating center identifier.
    pub centre: Option<u16>,
    /// Optional sub-center identifier.
    pub sub_centre: Option<u16>,
    /// Optional generating-process identifier.
    pub generating_process: Option<u16>,
    /// Exact additional key/value constraints.
    pub keys: BTreeMap<String, String>,
}

/// Exact NetCDF source matcher.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetCdfSourceMatcher {
    /// Exact required global attributes.
    pub global_attributes: BTreeMap<String, String>,
    /// Exact required dimensions and lengths.
    pub dimensions: BTreeMap<String, usize>,
}

/// File-format-specific exact source matcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceMatcher {
    /// GRIB key matcher.
    Grib(GribSourceMatcher),
    /// NetCDF/CF metadata matcher.
    NetCdf(NetCdfSourceMatcher),
}

/// Exact source selector and target canonical field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldMapping {
    /// Stable source field identity encoded by the profile format.
    pub source_identity: BTreeMap<String, String>,
    /// Target field key.
    pub target: FieldKey,
}

/// Explicitly permitted derived or estimated fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FallbackRule {
    /// Output field provided by the fallback.
    pub output: FieldKey,
    /// Required input fields.
    pub inputs: Vec<FieldKey>,
    /// Whitelisted computation-graph operation identifier.
    pub operation: String,
    /// Quality assigned to the result.
    pub quality: FieldQuality,
}

/// Complete dataset interpretation profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetProfile {
    /// Unique loaded name.
    pub name: ProfileName,
    /// SHA-256 of normalized profile content.
    pub sha256: String,
    /// Dataset-level exact fingerprint.
    pub fingerprint: DatasetFingerprint,
    /// Accepted exact source matchers.
    pub source_matchers: Vec<SourceMatcher>,
    /// Direct source-to-field mappings.
    pub field_mappings: Vec<FieldMapping>,
    /// Explicit fallback rules.
    pub fallback_rules: Vec<FallbackRule>,
}

/// Deterministic collection of built-in and local profiles.
#[derive(Clone, Debug, Default)]
pub struct ProfileCatalog {
    profiles: BTreeMap<ProfileName, DatasetProfile>,
}

impl ProfileCatalog {
    /// Creates an empty catalog.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            profiles: BTreeMap::new(),
        }
    }

    /// Registers a profile with a unique loaded name.
    pub fn register(&mut self, profile: DatasetProfile) -> Result<(), ProfileError> {
        if self.profiles.contains_key(&profile.name) {
            return Err(ProfileError::DuplicateName(profile.name));
        }
        self.profiles.insert(profile.name.clone(), profile);
        Ok(())
    }

    /// Returns a profile by exact name.
    #[must_use]
    pub fn get(&self, name: &ProfileName) -> Option<&DatasetProfile> {
        self.profiles.get(name)
    }
}

/// Profile catalog or matching failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    /// A loaded profile name is not unique.
    DuplicateName(ProfileName),
    /// No profile exactly matches a source.
    NoMatch,
    /// More than one profile exactly matches a source.
    AmbiguousMatch(Vec<ProfileName>),
}
