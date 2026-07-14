//! # Contract: dataset interpretation Profile documents
//!
//! Profiles are schema-checked YAML/JSON documents. They match source metadata
//! exactly, map source identities into canonical fields, declare time semantics
//! and explicit fallbacks, and contain typed derived-field expressions. A
//! Profile never performs I/O and never executes arbitrary user code.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::field::{
    CanonicalField, Capability, ExtensionFieldId, FieldKey, FieldQuality, FieldShape,
};
use crate::io::reader::{SourceFormat, SourceMetadata};
use crate::profile::catalog::ProfileOrigin;
use crate::profile::expression::{CompiledProfileGraph, ExpressionError, compile_profile_graph};
use crate::profile::graph::ExecutionStage;

/// Current development-stage Profile schema version.
pub const PROFILE_SCHEMA_VERSION: u32 = 0;

/// Profile document discriminator.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileDocumentKind {
    /// Dataset interpretation Profile.
    DatasetProfile,
}

/// Unique Profile name in the active catalog.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileName(pub String);

/// Canonical or explicitly namespaced output-field reference.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldReference {
    /// Built-in canonical field encoded as a snake-case string.
    Canonical(CanonicalField),
    /// Namespaced extension field.
    Extension {
        /// Owning namespace.
        namespace: String,
        /// Name unique within the namespace.
        name: String,
    },
}

impl FieldReference {
    /// Converts the document reference into the runtime field key.
    #[must_use]
    pub fn to_field_key(&self) -> FieldKey {
        match self {
            Self::Canonical(field) => FieldKey::Canonical(*field),
            Self::Extension { namespace, name } => FieldKey::Extension(ExtensionFieldId {
                namespace: namespace.clone(),
                name: name.clone(),
            }),
        }
    }
}

/// Exact dataset-level attributes required by a Profile.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetFingerprint {
    /// Exact normalized identity attributes.
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
}

/// Exact GRIB source matcher.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GribSourceMatcher {
    /// Optional originating centre identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub centre: Option<u16>,
    /// Optional sub-centre identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_centre: Option<u16>,
    /// Optional generating-process identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generating_process: Option<u16>,
    /// Exact additional key/value constraints.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

/// Exact NetCDF source matcher.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetCdfSourceMatcher {
    /// Exact required global attributes.
    #[serde(default)]
    pub global_attributes: BTreeMap<String, String>,
    /// Exact required dimensions and lengths.
    #[serde(default)]
    pub dimensions: BTreeMap<String, usize>,
}

/// File-format-specific exact source matcher.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceMatcher {
    /// GRIB edition 1 source.
    Grib1 {
        /// Exact GRIB constraints.
        #[serde(flatten)]
        matcher: GribSourceMatcher,
    },
    /// GRIB edition 2 source.
    Grib2 {
        /// Exact GRIB constraints.
        #[serde(flatten)]
        matcher: GribSourceMatcher,
    },
    /// Classic NetCDF source.
    Netcdf3 {
        /// Exact NetCDF constraints.
        #[serde(flatten)]
        matcher: NetCdfSourceMatcher,
    },
    /// HDF5-backed NetCDF source.
    Netcdf4 {
        /// Exact NetCDF constraints.
        #[serde(flatten)]
        matcher: NetCdfSourceMatcher,
    },
}

impl SourceMatcher {
    fn matches(&self, metadata: &SourceMetadata) -> bool {
        match self {
            Self::Grib1 { matcher } if metadata.format == SourceFormat::Grib1 => {
                matches_grib(matcher, metadata)
            }
            Self::Grib2 { matcher } if metadata.format == SourceFormat::Grib2 => {
                matches_grib(matcher, metadata)
            }
            Self::Netcdf3 { matcher } if metadata.format == SourceFormat::NetCdf3 => {
                matches_netcdf(matcher, metadata)
            }
            Self::Netcdf4 { matcher } if metadata.format == SourceFormat::NetCdf4 => {
                matches_netcdf(matcher, metadata)
            }
            _ => false,
        }
    }
}

/// One exact source alternative for a direct field mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSource {
    /// Exact format-specific field identity.
    pub identity: BTreeMap<String, String>,
    /// Exact source-unit symbol before conversion.
    pub unit: String,
    /// Whether this is the ordinary source or an explicitly allowed fallback.
    #[serde(default)]
    pub role: SourceRole,
    /// Quality assigned when this alternative is used.
    #[serde(default = "source_quality")]
    pub quality: FieldQuality,
}

/// Role of one source alternative in an exact fallback chain.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRole {
    /// Ordinary source used when present.
    #[default]
    Primary,
    /// Ordered, explicitly authorized fallback.
    Fallback,
}

/// Direct source-to-canonical field mapping.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldMapping {
    /// Stable symbolic name used by expressions.
    pub id: String,
    /// Target runtime field.
    pub target: FieldReference,
    /// Ordered source alternatives. One entry is the ordinary case.
    pub sources: Vec<FieldSource>,
    /// Time interpretation of the normalized direct field.
    #[serde(default)]
    pub temporal: TemporalSemantics,
}

/// Explicit static type for one namespaced extension field.
///
/// Direct extension mappings have no built-in canonical descriptor, so a
/// Profile must declare their normalized unit and array shape before they can
/// participate in the typed computation graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionFieldDescriptor {
    /// Exact namespaced extension target described by this entry.
    pub target: FieldReference,
    /// Normalized graph unit used after source conversion.
    pub unit: String,
    /// Canonical runtime array shape.
    pub shape: FieldShape,
}

/// Source-time semantics carried by a field.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalKind {
    /// Value is valid at one physical instant.
    #[default]
    Instantaneous,
    /// Value represents an average or integral over a closed-open interval.
    Interval,
    /// Value accumulates from an explicitly declared reset origin.
    Accumulation,
}

/// Explicit reset rule for accumulated source fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccumulationReset {
    /// Reset follows the source forecast-reference time.
    ForecastReference,
    /// Reset occurs at exact UTC hours.
    FixedUtcHours {
        /// Unique hours in the inclusive range 0..=23.
        hours: Vec<u8>,
    },
    /// Source is monotonic across the locked coverage.
    Never,
}

/// Time interpretation for one mapped or derived field.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalSemantics {
    /// Source-time category.
    #[serde(default)]
    pub kind: TemporalKind,
    /// Fixed interval length when the product does not encode bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
    /// Accumulation reset rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<AccumulationReset>,
    /// Number of preceding logical frames required by Frame-stage evaluation.
    #[serde(default)]
    pub warmup_frames: u16,
}

/// One derived canonical field expressed in the Profile DSL.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedField {
    /// Stable symbolic node name.
    pub id: String,
    /// Target runtime field.
    pub target: FieldReference,
    /// Type-checked expression text.
    pub expression: String,
    /// Declared output unit symbol.
    pub unit: String,
    /// Declared output array shape.
    pub shape: FieldShape,
    /// Earliest legal execution stage.
    pub stage: ExecutionStage,
    /// Provenance quality assigned to the result.
    pub quality: FieldQuality,
    /// Output time semantics.
    #[serde(default)]
    pub temporal: TemporalSemantics,
}

/// Complete author-facing Profile document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetProfileDocument {
    /// Must equal [`PROFILE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Must be `dataset_profile`.
    pub kind: ProfileDocumentKind,
    /// Unique name in the active catalog.
    pub name: ProfileName,
    /// Dataset-level exact fingerprint.
    #[serde(default)]
    pub fingerprint: DatasetFingerprint,
    /// Accepted exact source-container matchers.
    pub source_matchers: Vec<SourceMatcher>,
    /// Expected regular logical-frame spacing in whole seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_interval_seconds: Option<u64>,
    /// Explicit types for namespaced extension fields used by this Profile.
    #[serde(default)]
    pub extension_fields: Vec<ExtensionFieldDescriptor>,
    /// Direct source field mappings.
    #[serde(default)]
    pub fields: Vec<FieldMapping>,
    /// Derived fields compiled into the typed computation graph.
    #[serde(default)]
    pub derived_fields: Vec<DerivedField>,
    /// Capability-to-required-output mapping.
    #[serde(default)]
    pub capabilities: BTreeMap<Capability, Vec<FieldReference>>,
}

/// Validated Profile with stable semantic content identity.
#[derive(Clone, Debug, PartialEq)]
pub struct DatasetProfile {
    /// Validated author-facing document.
    pub document: DatasetProfileDocument,
    /// Lowercase SHA-256 of canonical semantic JSON.
    pub sha256: String,
    /// Fully type-checked immutable computation graph and stage plan.
    pub compiled: CompiledProfileGraph,
}

impl DatasetProfile {
    /// Returns the stable Profile name.
    #[must_use]
    pub const fn name(&self) -> &ProfileName {
        &self.document.name
    }

    fn matches(&self, metadata: &SourceMetadata) -> bool {
        required_pairs_match(&self.document.fingerprint.attributes, &metadata.attributes)
            && self
                .document
                .source_matchers
                .iter()
                .any(|matcher| matcher.matches(metadata))
    }
}

/// Machine-editable starting point returned for an unmatched source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileDraft {
    /// Suggested non-authoritative name.
    pub suggested_name: ProfileName,
    /// Inspected source metadata.
    pub source: SourceMetadata,
}

/// Deterministic collection of built-in and private Profiles.
#[derive(Clone, Debug, Default)]
pub struct ProfileCatalog {
    profiles: BTreeMap<ProfileName, DatasetProfile>,
    origins: BTreeMap<ProfileName, ProfileOrigin>,
}

impl ProfileCatalog {
    /// Creates an empty catalog.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            profiles: BTreeMap::new(),
            origins: BTreeMap::new(),
        }
    }

    /// Registers one validated uniquely named Profile.
    pub fn register(&mut self, profile: DatasetProfile) -> Result<(), ProfileError> {
        self.register_with_origin(profile, ProfileOrigin::Programmatic)
    }

    /// Registers one validated Profile together with exact loading provenance.
    pub(crate) fn register_with_origin(
        &mut self,
        profile: DatasetProfile,
        origin: ProfileOrigin,
    ) -> Result<(), ProfileError> {
        let name = profile.name().clone();
        if self.profiles.contains_key(&name) {
            return Err(ProfileError::DuplicateName(name));
        }
        self.origins.insert(name.clone(), origin);
        self.profiles.insert(name, profile);
        Ok(())
    }

    /// Validates and registers one document.
    pub fn register_document(
        &mut self,
        document: DatasetProfileDocument,
    ) -> Result<&DatasetProfile, ProfileError> {
        let profile = validate_profile(document)?;
        let name = profile.name().clone();
        self.register(profile)?;
        self.profiles
            .get(&name)
            .ok_or(ProfileError::CatalogInvariant(name))
    }

    /// Returns a Profile by exact name.
    #[must_use]
    pub fn get(&self, name: &ProfileName) -> Option<&DatasetProfile> {
        self.profiles.get(name)
    }

    /// Returns exact loading provenance for a registered Profile.
    #[must_use]
    pub fn origin(&self, name: &ProfileName) -> Option<&ProfileOrigin> {
        self.origins.get(name)
    }

    /// Selects exactly one Profile from normalized source metadata.
    pub fn match_source(&self, metadata: &SourceMetadata) -> Result<&DatasetProfile, ProfileError> {
        let matches = self
            .profiles
            .values()
            .filter(|profile| profile.matches(metadata))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Err(ProfileError::NoMatch(Box::new(profile_draft(metadata)))),
            [profile] => Ok(profile),
            _ => Err(ProfileError::AmbiguousMatch(
                matches
                    .iter()
                    .map(|profile| profile.name().clone())
                    .collect(),
            )),
        }
    }

    /// Returns registered Profiles in deterministic name order.
    pub fn iter(&self) -> impl Iterator<Item = &DatasetProfile> {
        self.profiles.values()
    }
}

/// Parses and validates a YAML Profile.
pub fn parse_profile_yaml(input: &str) -> Result<DatasetProfile, ProfileError> {
    let document =
        serde_yml::from_str(input).map_err(|error| ProfileError::Parse(error.to_string()))?;
    validate_profile(document)
}

/// Parses and validates a JSON Profile.
pub fn parse_profile_json(input: &str) -> Result<DatasetProfile, ProfileError> {
    let document =
        serde_json::from_str(input).map_err(|error| ProfileError::Parse(error.to_string()))?;
    validate_profile(document)
}

/// Validates one Profile document and computes its semantic identity.
pub fn validate_profile(document: DatasetProfileDocument) -> Result<DatasetProfile, ProfileError> {
    validate_profile_shape(&document)?;
    let bytes = serde_json::to_vec(&document)
        .map_err(|error| ProfileError::Serialize(error.to_string()))?;
    let sha256 = hex::encode(Sha256::digest(bytes));
    let compiled = compile_profile_graph(&document).map_err(ProfileError::Expression)?;
    Ok(DatasetProfile {
        document,
        sha256,
        compiled,
    })
}

fn validate_profile_shape(document: &DatasetProfileDocument) -> Result<(), ProfileError> {
    if document.schema_version != PROFILE_SCHEMA_VERSION {
        return Err(ProfileError::UnsupportedVersion(document.schema_version));
    }
    if document.kind != ProfileDocumentKind::DatasetProfile {
        return Err(ProfileError::WrongKind(document.kind));
    }
    if document.name.0.trim().is_empty() {
        return Err(ProfileError::Invalid(
            "Profile name must not be empty".into(),
        ));
    }
    if document.source_matchers.is_empty() {
        return Err(ProfileError::Invalid(
            "source_matchers must contain at least one exact matcher".into(),
        ));
    }
    if document.frame_interval_seconds == Some(0) {
        return Err(ProfileError::Invalid(
            "frame_interval_seconds must be positive when declared".into(),
        ));
    }

    let mut extension_targets = BTreeSet::new();
    for descriptor in &document.extension_fields {
        validate_extension_reference(&descriptor.target, "extension field descriptor")?;
        if descriptor.unit.trim().is_empty() {
            return Err(ProfileError::Invalid(
                "extension field descriptor unit must not be empty".into(),
            ));
        }
        if !extension_targets.insert(descriptor.target.clone()) {
            return Err(ProfileError::Invalid(
                "extension field descriptors must have unique targets".into(),
            ));
        }
    }

    let mut ids = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for mapping in &document.fields {
        validate_expression_symbol(&mapping.id, "field mapping id")?;
        validate_field_reference(&mapping.target, "field mapping target")?;
        if matches!(mapping.target, FieldReference::Extension { .. })
            && !extension_targets.contains(&mapping.target)
        {
            return Err(ProfileError::Invalid(format!(
                "direct extension field {:?} has no registered descriptor",
                mapping.target
            )));
        }
        if !ids.insert(mapping.id.clone()) {
            return Err(ProfileError::Invalid(format!(
                "duplicate field or derived id '{}'",
                mapping.id
            )));
        }
        if !targets.insert(mapping.target.clone()) {
            return Err(ProfileError::Invalid(
                "one Profile output field may be produced only once".into(),
            ));
        }
        if mapping.sources.is_empty() {
            return Err(ProfileError::Invalid(format!(
                "field mapping '{}' has no source alternatives",
                mapping.id
            )));
        }
        if mapping.sources.len() > 1
            && (mapping.sources[0].role != SourceRole::Primary
                || mapping.sources[1..]
                    .iter()
                    .any(|source| source.role != SourceRole::Fallback))
        {
            return Err(ProfileError::Invalid(format!(
                "field mapping '{}' with multiple sources must declare one primary followed by explicit fallbacks",
                mapping.id
            )));
        }
        let mut source_identities = BTreeSet::new();
        for source in &mapping.sources {
            if source.identity.is_empty() || source.unit.trim().is_empty() {
                return Err(ProfileError::Invalid(format!(
                    "field mapping '{}' has an empty source identity or unit",
                    mapping.id
                )));
            }
            if !source_identities.insert(source.identity.clone()) {
                return Err(ProfileError::Invalid(format!(
                    "field mapping '{}' repeats a source identity",
                    mapping.id
                )));
            }
        }
        validate_temporal(&mapping.temporal)?;
    }

    for derived in &document.derived_fields {
        validate_expression_symbol(&derived.id, "derived field id")?;
        validate_field_reference(&derived.target, "derived field target")?;
        if !ids.insert(derived.id.clone()) {
            return Err(ProfileError::Invalid(format!(
                "duplicate field or derived id '{}'",
                derived.id
            )));
        }
        if !targets.insert(derived.target.clone()) {
            return Err(ProfileError::Invalid(
                "one Profile output field may be produced only once".into(),
            ));
        }
        if derived.expression.trim().is_empty() || derived.unit.trim().is_empty() {
            return Err(ProfileError::Invalid(format!(
                "derived field '{}' has an empty expression or unit",
                derived.id
            )));
        }
        validate_temporal(&derived.temporal)?;
    }

    for required in document.capabilities.values() {
        if required.is_empty() {
            return Err(ProfileError::Invalid(
                "capability field lists must not be empty".into(),
            ));
        }
        let unique = required.iter().collect::<BTreeSet<_>>();
        if unique.len() != required.len() {
            return Err(ProfileError::Invalid(
                "capability field lists must not repeat outputs".into(),
            ));
        }
        if required.iter().any(|field| !targets.contains(field)) {
            return Err(ProfileError::Invalid(
                "capability field lists may reference only produced outputs".into(),
            ));
        }
        for field in required {
            validate_field_reference(field, "capability field")?;
        }
    }
    Ok(())
}

fn validate_extension_reference(
    reference: &FieldReference,
    context: &str,
) -> Result<(), ProfileError> {
    if !matches!(reference, FieldReference::Extension { .. }) {
        return Err(ProfileError::Invalid(format!(
            "{context} must target a namespaced extension field"
        )));
    }
    validate_field_reference(reference, context)
}

fn validate_field_reference(reference: &FieldReference, context: &str) -> Result<(), ProfileError> {
    if let FieldReference::Extension { namespace, name } = reference {
        if namespace.trim().is_empty() || name.trim().is_empty() {
            return Err(ProfileError::Invalid(format!(
                "{context} has an empty extension namespace or name"
            )));
        }
    }
    Ok(())
}

fn validate_temporal(temporal: &TemporalSemantics) -> Result<(), ProfileError> {
    match temporal.kind {
        TemporalKind::Instantaneous => {
            if temporal.reset.is_some() || temporal.warmup_frames != 0 {
                return Err(ProfileError::Invalid(
                    "instantaneous fields cannot declare reset or warmup".into(),
                ));
            }
        }
        TemporalKind::Interval => {
            if temporal.interval_seconds == Some(0) || temporal.reset.is_some() {
                return Err(ProfileError::Invalid(
                    "interval fields require a positive interval when declared and no reset".into(),
                ));
            }
        }
        TemporalKind::Accumulation => {
            if temporal.reset.is_none() || temporal.warmup_frames == 0 {
                return Err(ProfileError::Invalid(
                    "accumulation fields require a reset rule and warmup_frames >= 1".into(),
                ));
            }
        }
    }
    if let Some(AccumulationReset::FixedUtcHours { hours }) = &temporal.reset {
        let unique = hours.iter().copied().collect::<BTreeSet<_>>();
        if hours.is_empty() || unique.len() != hours.len() || hours.iter().any(|hour| *hour > 23) {
            return Err(ProfileError::Invalid(
                "fixed UTC reset hours must be unique values in 0..=23".into(),
            ));
        }
    }
    Ok(())
}

fn validate_expression_symbol(value: &str, label: &str) -> Result<(), ProfileError> {
    if value.trim().is_empty()
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        || !value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
    {
        return Err(ProfileError::Invalid(format!(
            "{label} must be an ASCII expression identifier using letters, digits, and '_'"
        )));
    }
    Ok(())
}

fn profile_draft(metadata: &SourceMetadata) -> ProfileDraft {
    let stem = metadata
        .path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("unmatched-source")
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    ProfileDraft {
        suggested_name: ProfileName(stem.trim_matches('-').to_owned()),
        source: metadata.clone(),
    }
}

fn source_quality() -> FieldQuality {
    FieldQuality::Source
}

fn matches_grib(matcher: &GribSourceMatcher, metadata: &SourceMetadata) -> bool {
    optional_u16_matches("centre", matcher.centre, metadata)
        && optional_u16_matches("sub_centre", matcher.sub_centre, metadata)
        && optional_u16_matches("generating_process", matcher.generating_process, metadata)
        && required_pairs_match(&matcher.keys, &metadata.attributes)
}

fn optional_u16_matches(key: &str, expected: Option<u16>, metadata: &SourceMetadata) -> bool {
    expected.is_none_or(|value| metadata.attributes.get(key) == Some(&value.to_string()))
}

fn matches_netcdf(matcher: &NetCdfSourceMatcher, metadata: &SourceMetadata) -> bool {
    required_pairs_match(&matcher.global_attributes, &metadata.attributes)
        && matcher
            .dimensions
            .iter()
            .all(|(name, length)| metadata.dimensions.get(name) == Some(length))
}

fn required_pairs_match(
    required: &BTreeMap<String, String>,
    actual: &BTreeMap<String, String>,
) -> bool {
    required
        .iter()
        .all(|(key, value)| actual.get(key) == Some(value))
}

/// Profile parse, validation, matching, or catalog failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileError {
    /// YAML/JSON decoding failed.
    Parse(String),
    /// Profile serialization for semantic hashing failed.
    Serialize(String),
    /// Profile schema version is unsupported.
    UnsupportedVersion(u32),
    /// Document kind is not a DatasetProfile.
    WrongKind(ProfileDocumentKind),
    /// Profile shape or declaration is invalid.
    Invalid(String),
    /// Typed expression or computation-graph compilation failed.
    Expression(ExpressionError),
    /// A loaded Profile name is not unique.
    DuplicateName(ProfileName),
    /// No Profile exactly matches a source.
    NoMatch(Box<ProfileDraft>),
    /// More than one Profile exactly matches a source.
    AmbiguousMatch(Vec<ProfileName>),
    /// Internal catalog state violated its insertion invariant.
    CatalogInvariant(ProfileName),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(message) => write!(formatter, "Profile parse error: {message}"),
            Self::Serialize(message) => write!(formatter, "Profile serialization error: {message}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported Profile schema version {version}")
            }
            Self::WrongKind(kind) => write!(formatter, "wrong Profile document kind: {kind:?}"),
            Self::Invalid(message) => write!(formatter, "invalid Profile: {message}"),
            Self::Expression(error) => write!(formatter, "invalid Profile expression: {error}"),
            Self::DuplicateName(name) => write!(formatter, "duplicate Profile name '{}'", name.0),
            Self::NoMatch(draft) => write!(
                formatter,
                "no Profile matches '{}'; draft name '{}'",
                draft.source.path.display(),
                draft.suggested_name.0
            ),
            Self::AmbiguousMatch(names) => write!(
                formatter,
                "source matches multiple Profiles: {}",
                names
                    .iter()
                    .map(|name| name.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::CatalogInvariant(name) => {
                write!(formatter, "Profile catalog lost inserted name '{}'", name.0)
            }
        }
    }
}

impl std::error::Error for ProfileError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::path::PathBuf;

    use trajecta_case::model::time::Timestamp;

    use super::*;

    fn profile_yaml() -> &'static str {
        r#"
schema_version: 0
kind: dataset_profile
name: era5-test
fingerprint:
  attributes:
    product: era5
source_matchers:
  - format: grib1
    centre: 98
fields:
  - id: temperature
    target: air_temperature
    sources:
      - identity: { parameter: "130", level_type: hybrid }
        unit: K
capabilities:
  transport: [air_temperature]
"#
    }

    #[test]
    fn yaml_and_json_have_the_same_semantic_hash() {
        let yaml = parse_profile_yaml(profile_yaml()).unwrap();
        let json = serde_json::to_string_pretty(&yaml.document).unwrap();
        let reparsed = parse_profile_json(&json).unwrap();
        assert_eq!(yaml.sha256, reparsed.sha256);
    }

    #[test]
    fn exact_matching_returns_one_profile_and_draft_on_no_match() {
        let mut catalog = ProfileCatalog::new();
        catalog
            .register(parse_profile_yaml(profile_yaml()).unwrap())
            .unwrap();
        let matched = SourceMetadata {
            path: PathBuf::from("EA18120100"),
            format: SourceFormat::Grib1,
            attributes: BTreeMap::from([
                ("product".into(), "era5".into()),
                ("centre".into(), "98".into()),
            ]),
            dimensions: BTreeMap::new(),
            valid_times: vec![Timestamp::UNIX_EPOCH],
            roles: vec!["analysis".into()],
            grid: None,
            vertical: None,
        };
        assert_eq!(
            catalog.match_source(&matched).unwrap().name().0,
            "era5-test"
        );

        let unmatched = SourceMetadata {
            attributes: BTreeMap::from([("product".into(), "private".into())]),
            ..matched
        };
        assert!(matches!(
            catalog.match_source(&unmatched),
            Err(ProfileError::NoMatch(_))
        ));
    }

    #[test]
    fn accumulation_requires_explicit_reset_and_warmup() {
        let mut profile = parse_profile_yaml(profile_yaml()).unwrap().document;
        profile.derived_fields.push(DerivedField {
            id: "rain-rate".into(),
            target: FieldReference::Canonical(CanonicalField::PrecipitationRate),
            expression: "deaccumulate(total_rain) / interval_seconds()".into(),
            unit: "kg/m2/s".into(),
            shape: FieldShape::Horizontal2D,
            stage: ExecutionStage::Frame,
            quality: FieldQuality::Derived,
            temporal: TemporalSemantics {
                kind: TemporalKind::Accumulation,
                interval_seconds: None,
                reset: None,
                warmup_frames: 0,
            },
        });
        assert!(matches!(
            validate_profile(profile),
            Err(ProfileError::Invalid(_))
        ));
    }
}
