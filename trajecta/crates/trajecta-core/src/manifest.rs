//! # Contract: reproducible run manifest
//!
//! A manifest records software, immutable inputs, execution resources, and
//! numerical choices without embedding credentials or mutable local state.

use std::collections::BTreeMap;

/// Software versions and source revision used by a run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SoftwareIdentity {
    /// Package versions by crate name.
    pub crate_versions: BTreeMap<String, String>,
    /// Optional Git commit of the Trajecta repository.
    pub git_commit: Option<String>,
}

/// Immutable hashes of all portable and machine input documents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputIdentity {
    /// Resolved Case content hash.
    pub case_sha256: String,
    /// Resolved RunProfile content hash.
    pub run_profile_sha256: String,
    /// Dataset lock hashes.
    pub dataset_lock_sha256: BTreeMap<String, String>,
    /// Dataset profile hashes.
    pub dataset_profile_sha256: BTreeMap<String, String>,
}

/// Runtime resources and completion status.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExecutionSummary {
    /// Worker count.
    pub worker_threads: usize,
    /// Hard memory budget in bytes.
    pub memory_budget_bytes: u64,
    /// Stable executor identifier.
    pub executor: String,
    /// Stable final exit status.
    pub exit_status: String,
}

/// Numerical algorithms and reproducibility controls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NumericalSummary {
    /// Stable integrator identifier.
    pub integrator: String,
    /// Stable boundary-policy identifiers in application order.
    pub boundary_policies: Vec<String>,
    /// Declared numeric tolerances.
    pub tolerances: BTreeMap<String, f64>,
    /// Whether deterministic execution checks were enabled.
    pub deterministic: bool,
}

/// Complete auditable record of one run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunManifest {
    /// Software identity.
    pub software: SoftwareIdentity,
    /// Immutable input identity.
    pub inputs: InputIdentity,
    /// Runtime execution summary.
    pub execution: ExecutionSummary,
    /// Numerical choices.
    pub numerical: NumericalSummary,
}
