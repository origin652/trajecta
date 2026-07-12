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
//! | [`DatasetBinding`] | Logical dataset → lockfile |
//! | [`ExecutionSpec`] | Threads / memory / executor |
//!
//! Unknown fields are rejected on all configuration objects.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use crate::model::metadata::Metadata;
use crate::model::meteorology::{DatasetRef, MeteorologySpec};
use crate::model::numerics::NumericsSpec;
use crate::model::output::OutputProductSpec;
use crate::model::physics::PhysicsModuleSpec;
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
    pub physics: Option<ComponentRef<Vec<PhysicsModuleSpec>>>,
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
    /// Logical dataset bindings.
    #[serde(default)]
    pub datasets: Vec<DatasetBinding>,
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
    /// Normalized physics modules.
    #[serde(default)]
    pub physics: Vec<PhysicsModuleSpec>,
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
    /// Canonical local dataset bindings.
    pub datasets: Vec<DatasetBinding>,
    /// Validated machine resources.
    pub execution: ExecutionSpec,
    /// Every source document and immutable digest used during resolution.
    #[serde(default)]
    pub sources: Vec<SourceDigest>,
}
