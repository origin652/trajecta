//! # Contract: recursive document expansion
//!
//! Expands local component references into fully resolved Case and RunProfile
//! documents, recording deterministic source digests and rejecting cycles.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`expand_case_file`] | Read Case path and expand all component refs |
//! | [`expand_case_document`] | Expand an already-parsed Case |
//! | [`expand_run_profile_file`] | Read RunProfile and canonicalize paths |
//! | [`expand_run_profile_document`] | Expand an already-parsed RunProfile |
//! | [`ExpandError`] | Expansion failures |

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::document::{
    CaseDocument, DatasetBinding, ResolvedCase, ResolvedRunProfile, RunProfileDocument,
};
use crate::model::meteorology::MeteorologySpec;
use crate::model::numerics::NumericsSpec;
use crate::model::output::OutputProductSpec;
use crate::model::physics::PhysicsModuleSpec;
use crate::model::population::ParticlePopulationSpec;
use crate::model::substance::SubstanceSpec;
use crate::model::time::TimeSpec;
use crate::reference::{ComponentRef, RefPath};
use crate::resolver::{
    LocalRefResolver, RefResolver, ResolutionGraph, ResolveError, ResolvedSource, SourceDigest,
};
use crate::schema::{
    SchemaError, parse_case_json, parse_case_yaml, parse_run_profile_json, parse_run_profile_yaml,
};

/// Expansion failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpandError {
    /// Underlying local resolution failure.
    Resolve(ResolveError),
    /// Schema / parse failure for a document or component.
    Schema(SchemaError),
    /// A component file could not be decoded as the expected type.
    ComponentParse {
        /// Component path.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolve(error) => write!(f, "{error}"),
            Self::Schema(error) => write!(f, "{error}"),
            Self::ComponentParse { path, message } => {
                write!(f, "component parse error at {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ExpandError {}

impl From<ResolveError> for ExpandError {
    fn from(value: ResolveError) -> Self {
        Self::Resolve(value)
    }
}

impl From<SchemaError> for ExpandError {
    fn from(value: SchemaError) -> Self {
        Self::Schema(value)
    }
}

/// Reads a Case file under `resolver` root and expands every component ref.
pub fn expand_case_file(
    case_path: &Path,
    resolver: &LocalRefResolver,
) -> Result<ResolvedCase, ExpandError> {
    let source = resolver.read_file(case_path)?;
    let text = bytes_to_str(&source.bytes, &source.digest.path)?;
    let document = parse_case_text(text, &source.digest.path)?;
    expand_case_document(
        &document,
        &source.digest.path.clone(),
        resolver,
        source.digest,
    )
}

/// Expands component references from an already-parsed Case document.
pub fn expand_case_document(
    document: &CaseDocument,
    case_path: &Path,
    resolver: &LocalRefResolver,
    case_digest: SourceDigest,
) -> Result<ResolvedCase, ExpandError> {
    let mut graph = ResolutionGraph::new();
    let mut sources = BTreeMap::<PathBuf, SourceDigest>::new();
    sources.insert(case_digest.path.clone(), case_digest);

    let time = expand_component(
        &document.time,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?;
    let meteorology = expand_component(
        &document.meteorology,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?;
    let particle_population = expand_component(
        &document.particle_population,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?;
    let substances = expand_component(
        &document.substances,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?
    .unwrap_or_default();
    let numerics = expand_component(
        &document.numerics,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?;
    let physics = expand_component(
        &document.physics,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?
    .unwrap_or_default();
    let outputs = expand_component(
        &document.outputs,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?
    .unwrap_or_default();

    Ok(ResolvedCase {
        metadata: document.metadata.clone(),
        time,
        meteorology,
        particle_population,
        substances,
        numerics,
        physics,
        outputs,
        sources: sources.into_values().collect(),
    })
}

/// Reads a RunProfile file and produces a path-normalized resolved profile.
pub fn expand_run_profile_file(
    profile_path: &Path,
    resolver: &LocalRefResolver,
) -> Result<ResolvedRunProfile, ExpandError> {
    let source = resolver.read_file(profile_path)?;
    let text = bytes_to_str(&source.bytes, &source.digest.path)?;
    let document = parse_run_profile_text(text, &source.digest.path)?;
    expand_run_profile_document(
        &document,
        &source.digest.path.clone(),
        resolver,
        source.digest,
    )
}

/// Canonicalizes RunProfile paths and records the profile source digest.
pub fn expand_run_profile_document(
    document: &RunProfileDocument,
    profile_path: &Path,
    resolver: &LocalRefResolver,
    profile_digest: SourceDigest,
) -> Result<ResolvedRunProfile, ExpandError> {
    let case_path =
        resolver.canonicalize_under_root(&join_relative(profile_path, &document.case_path))?;
    let mut datasets = Vec::with_capacity(document.datasets.len());
    for binding in &document.datasets {
        let lockfile =
            resolver.canonicalize_under_root(&join_relative(profile_path, &binding.lockfile))?;
        let cache_root = match &binding.cache_root {
            Some(path) => {
                Some(resolver.canonicalize_under_root(&join_relative(profile_path, path))?)
            }
            None => None,
        };
        datasets.push(DatasetBinding {
            dataset: binding.dataset.clone(),
            lockfile,
            cache_root,
        });
    }

    let mut sources = BTreeMap::new();
    sources.insert(profile_digest.path.clone(), profile_digest);
    // Record the case file identity when it exists under root.
    if let Ok(case_source) = resolver.read_file(&case_path) {
        sources.insert(case_source.digest.path.clone(), case_source.digest);
    }

    Ok(ResolvedRunProfile {
        metadata: document.metadata.clone(),
        case_path,
        datasets,
        execution: document.execution.clone(),
        sources: sources.into_values().collect(),
    })
}

fn expand_component<T>(
    component: &Option<ComponentRef<T>>,
    containing_file: &Path,
    resolver: &LocalRefResolver,
    graph: &mut ResolutionGraph,
    sources: &mut BTreeMap<PathBuf, SourceDigest>,
) -> Result<Option<T>, ExpandError>
where
    T: DeserializeOwned + Clone,
{
    match component {
        None => Ok(None),
        Some(ComponentRef::Inline(value)) => Ok(Some(value.clone())),
        Some(ComponentRef::Ref(reference)) => {
            let resolved = resolve_with_cycle(containing_file, reference, resolver, graph)?;
            sources.insert(resolved.digest.path.clone(), resolved.digest.clone());
            let text = bytes_to_str(&resolved.bytes, &resolved.digest.path)?;
            let value = parse_component_text::<T>(text, &resolved.digest.path)?;
            Ok(Some(value))
        }
    }
}

fn resolve_with_cycle(
    containing_file: &Path,
    reference: &RefPath,
    resolver: &LocalRefResolver,
    graph: &mut ResolutionGraph,
) -> Result<ResolvedSource, ExpandError> {
    let containing = resolver.canonicalize_under_root(containing_file)?;
    let resolved = resolver.resolve(&containing, reference)?;
    graph.add_edge_checking_cycle(containing, resolved.digest.path.clone())?;
    Ok(resolved)
}

fn join_relative(base_file: &Path, relative: &Path) -> PathBuf {
    match base_file.parent() {
        Some(parent) => parent.join(relative),
        None => relative.to_path_buf(),
    }
}

fn bytes_to_str<'a>(bytes: &'a [u8], path: &Path) -> Result<&'a str, ExpandError> {
    std::str::from_utf8(bytes).map_err(|error| ExpandError::ComponentParse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn parse_case_text(text: &str, path: &Path) -> Result<CaseDocument, ExpandError> {
    if is_json_path(path) {
        parse_case_json(text).map_err(ExpandError::from)
    } else {
        parse_case_yaml(text).map_err(ExpandError::from)
    }
}

fn parse_run_profile_text(text: &str, path: &Path) -> Result<RunProfileDocument, ExpandError> {
    if is_json_path(path) {
        parse_run_profile_json(text).map_err(ExpandError::from)
    } else {
        parse_run_profile_yaml(text).map_err(ExpandError::from)
    }
}

fn parse_component_text<T: DeserializeOwned>(text: &str, path: &Path) -> Result<T, ExpandError> {
    if is_json_path(path) {
        serde_json::from_str(text).map_err(|error| ExpandError::ComponentParse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
    } else {
        serde_yml::from_str(text).map_err(|error| ExpandError::ComponentParse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
    }
}

fn is_json_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

// Keep type references used by expand_component monomorphizations discoverable.
#[allow(dead_code)]
type _ComponentTypes = (
    TimeSpec,
    MeteorologySpec,
    ParticlePopulationSpec,
    Vec<SubstanceSpec>,
    NumericsSpec,
    Vec<PhysicsModuleSpec>,
    Vec<OutputProductSpec>,
);

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn expands_nested_component_refs_and_records_sources() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::write(
            root.join("case.yaml"),
            r#"
schema_version: 0
kind: case
metadata:
  name: expanded
time:
  ref: components/time.yaml
meteorology:
  ref: components/met.yaml
"#,
        )
        .unwrap();
        fs::write(
            root.join("components/time.yaml"),
            r#"
start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
end: { seconds_since_unix_epoch: 10, nanosecond: 0 }
direction: forward
"#,
        )
        .unwrap();
        fs::write(
            root.join("components/met.yaml"),
            r#"
domains:
  - id: d0
    dataset: era5
    priority: 1
    horizontal_halo_cells: 1
"#,
        )
        .unwrap();

        let resolver = LocalRefResolver::new(root);
        let resolved = expand_case_file(&root.join("case.yaml"), &resolver).unwrap();
        assert!(resolved.time.is_some());
        assert_eq!(resolved.meteorology.as_ref().unwrap().domains.len(), 1);
        assert_eq!(resolved.sources.len(), 3);
        let mut paths: Vec<_> = resolved
            .sources
            .iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        paths.sort();
        assert_eq!(paths, vec!["case.yaml", "met.yaml", "time.yaml"]);
    }

    #[test]
    fn detects_component_reference_cycle_in_graph() {
        let mut graph = ResolutionGraph::new();
        let a = PathBuf::from("a");
        let b = PathBuf::from("b");
        graph.add_edge_checking_cycle(a.clone(), b.clone()).unwrap();
        assert!(graph.add_edge_checking_cycle(b, a).is_err());
    }

    #[test]
    fn expand_run_profile_canonicalizes_paths() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("cases")).unwrap();
        fs::create_dir_all(root.join("locks")).unwrap();
        fs::write(
            root.join("cases/demo.yaml"),
            b"schema_version: 0\nkind: case\nmetadata: {name: x}\n",
        )
        .unwrap();
        fs::write(root.join("locks/era5.lock.json"), b"{}").unwrap();
        fs::write(
            root.join("run.json"),
            r#"
{
  "schema_version": 0,
  "kind": "run_profile",
  "metadata": { "name": "local" },
  "case_path": "cases/demo.yaml",
  "datasets": [
    { "dataset": "era5", "lockfile": "locks/era5.lock.json" }
  ],
  "execution": {
    "worker_threads": 1,
    "memory_budget_bytes": 1024,
    "executor": "cpu"
  }
}
"#,
        )
        .unwrap();

        let resolver = LocalRefResolver::new(root);
        let resolved = expand_run_profile_file(&root.join("run.json"), &resolver).unwrap();
        assert!(resolved.case_path.ends_with("demo.yaml"));
        assert!(resolved.datasets[0].lockfile.ends_with("era5.lock.json"));
        assert!(!resolved.sources.is_empty());
    }
}
