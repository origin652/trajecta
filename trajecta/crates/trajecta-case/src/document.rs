//! # Contract: top-level documents
//!
//! Case documents contain portable scientific intent. RunProfile documents
//! contain machine paths and resource choices. Resolution removes component
//! references and records source digests before meteorology or execution sees
//! the configuration.
//!
//! ## Exposed interface
//!
//! | Type | Role |
//! |---|---|
//! | [`DocumentKind`] | `case` / `run_profile` |
//! | [`CaseDocument`] | Portable scientific Case |
//! | [`RunProfileDocument`] | Machine profile |
//! | [`ResolvedCase`] | Fully expanded Case |
//! | [`ResolvedRunProfile`] | Path-resolved profile |
//! | [`DatasetBinding`] | Logical dataset → lockfile and named roots |
//! | [`ProfileSource`] | Explicit local Profile file or directory |
//! | [`MeteorologyReaderBackend`] | Native / Rust reader selection |
//! | [`ExecutionSpec`] | Threads / memory / executor |
//!
//! Unknown fields are rejected on all configuration objects.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use crate::model::metadata::Metadata;
use crate::model::meteorology::{DatasetRef, MeteorologySpec};
use crate::model::numerics::NumericsSpec;
use crate::model::output::OutputProductSpec;
use crate::model::physics::{PhysicsSelectionSpec, ResolvedPhysicsSpec};
use crate::model::population::ParticlePopulationSpec;
use crate::model::substance::SubstanceSpec;
use crate::model::time::TimeSpec;
use crate::reference::ComponentRef;
use crate::resolver::SourceDigest;

/// Top-level document discriminator.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    /// Portable scientific Case document.
    Case,
    /// Machine-specific RunProfile document.
    RunProfile,
}

/// Stable identifier for one machine-local data root.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataRootId(pub String);

impl DataRootId {
    /// Built-in root resolved relative to the DatasetLock file.
    pub const LOCKFILE: &'static str = "lockfile";
}

/// Explicit source of user-authored meteorology profiles.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProfileSource {
    /// Load exactly one YAML or JSON Profile document.
    File {
        /// Local Profile path.
        path: PathBuf,
    },
    /// Load Profile documents from one directory without recursion.
    Directory {
        /// Local directory containing Profile documents.
        path: PathBuf,
    },
}

impl ProfileSource {
    /// Returns the configured machine path.
    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        match self {
            Self::File { path } | Self::Directory { path } => path,
        }
    }
}

/// Meteorology source-reader implementation selected by machine configuration.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeteorologyReaderBackend {
    /// ecCodes and netCDF-C/HDF5 based reader.
    #[default]
    Native,
    /// Pure Rust GRIB and NetCDF reader.
    Rust,
}

/// Development-stage Trajecta Case document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseDocument {
    /// Must equal `schema::CURRENT_SCHEMA_VERSION`.
    pub schema_version: u32,
    /// Must be `DocumentKind::Case`.
    pub kind: DocumentKind,
    /// Descriptive metadata.
    #[serde(default)]
    pub metadata: Metadata,
    /// Optional time component, inline or locally referenced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<ComponentRef<TimeSpec>>,
    /// Optional meteorology component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meteorology: Option<ComponentRef<MeteorologySpec>>,
    /// Optional full particle-population lifecycle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub particle_population: Option<ComponentRef<ParticlePopulationSpec>>,
    /// Optional substance catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub substances: Option<ComponentRef<Vec<SubstanceSpec>>>,
    /// Optional numerical controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numerics: Option<ComponentRef<NumericsSpec>>,
    /// Optional physics modules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<ComponentRef<PhysicsSelectionSpec>>,
    /// Optional output products.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<ComponentRef<Vec<OutputProductSpec>>>,
}

/// Mapping from a logical dataset to machine-local immutable data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetBinding {
    /// Logical identifier used by a Case domain.
    pub dataset: DatasetRef,
    /// Local dataset lockfile.
    pub lockfile: PathBuf,
    /// Optional local cache root reserved for future providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_root: Option<PathBuf>,
    /// Explicit machine roots referenced by locked files.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub data_roots: BTreeMap<DataRootId, PathBuf>,
    /// Optional dataset-specific reader override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_backend: Option<MeteorologyReaderBackend>,
}

/// Machine-local execution resources.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSpec {
    /// Requested worker count; zero is invalid.
    pub worker_threads: usize,
    /// Hard memory budget in bytes.
    pub memory_budget_bytes: u64,
    /// Stable execution backend identifier.
    pub executor: String,
    /// Default meteorology reader for dataset bindings without an override.
    #[serde(default)]
    pub meteorology_reader: MeteorologyReaderBackend,
}

/// Development-stage machine-specific RunProfile document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProfileDocument {
    /// Must equal `schema::CURRENT_SCHEMA_VERSION`.
    pub schema_version: u32,
    /// Must be `DocumentKind::RunProfile`.
    pub kind: DocumentKind,
    /// Descriptive metadata.
    #[serde(default)]
    pub metadata: Metadata,
    /// Local Case path selected for a run.
    pub case_path: PathBuf,
    /// Root under which unique run directories are created.
    pub output_root: PathBuf,
    /// Logical dataset bindings.
    #[serde(default)]
    pub datasets: Vec<DatasetBinding>,
    /// Explicit local Profile files or non-recursive directories.
    #[serde(default)]
    pub profile_sources: Vec<ProfileSource>,
    /// Machine resource choices.
    pub execution: ExecutionSpec,
}

/// Fully expanded and normalized Case passed to runtime crates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCase {
    /// Original descriptive metadata.
    pub metadata: Metadata,
    /// Optional normalized time specification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeSpec>,
    /// Optional normalized meteorology specification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meteorology: Option<MeteorologySpec>,
    /// Optional normalized population strategy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub particle_population: Option<ParticlePopulationSpec>,
    /// Normalized substances.
    #[serde(default)]
    pub substances: Vec<SubstanceSpec>,
    /// Optional normalized numerical controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numerics: Option<NumericsSpec>,
    /// Fully expanded physics selection; absence means pure advection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<ResolvedPhysicsSpec>,
    /// Normalized output products.
    #[serde(default)]
    pub outputs: Vec<OutputProductSpec>,
    /// Every source document and immutable digest used during resolution.
    #[serde(default)]
    pub sources: Vec<SourceDigest>,
}

/// Fully path-resolved machine profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRunProfile {
    /// Original descriptive metadata.
    pub metadata: Metadata,
    /// Canonical Case path.
    pub case_path: PathBuf,
    /// Absolute normalized output root; it need not exist before the run.
    pub output_root: PathBuf,
    /// Canonical local dataset bindings.
    pub datasets: Vec<DatasetBinding>,
    /// Canonical local Profile files and directories.
    #[serde(default)]
    pub profile_sources: Vec<ProfileSource>,
    /// Machine resources validated during RunProfile expansion.
    pub execution: ExecutionSpec,
    /// Every source document and immutable digest used during resolution.
    #[serde(default)]
    pub sources: Vec<SourceDigest>,
}
