//! # Contract: single-level document expansion
//!
//! v0 expands **one level** of local Case component references into a fully
//! resolved Case, and normalizes RunProfile machine paths. Nested component
//! files must be concrete values; they are not re-expanded.
//!
//! Path policy:
//!
//! - Case component `ref` paths remain jailed under [`LocalRefResolver`]'s root.
//! - RunProfile `case_path`, dataset `lockfile`, and `cache_root` are machine
//!   paths and may live outside the Case root.
//! - `cache_root` may not exist yet and is still recorded as an absolute path.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`expand_case_file`] | Read Case path and expand single-level component refs |
//! | [`expand_case_document`] | Expand an already-parsed Case |
//! | [`expand_run_profile_file`] | Read RunProfile and normalize machine paths |
//! | [`expand_run_profile_document`] | Expand an already-parsed RunProfile |
//! | [`ExpandError`] | Expansion failures |

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::diagnostic::DiagnosticBag;
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
    read_local_file,
};
use crate::schema::{
    SchemaError, parse_case_json, parse_case_yaml, parse_run_profile_json, parse_run_profile_yaml,
    validate_resolved_case,
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
    /// Expanded Case failed shared shape validation.
    Shape(DiagnosticBag),
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resolve(error) => write!(f, "{error}"),
            Self::Schema(error) => write!(f, "{error}"),
            Self::ComponentParse { path, message } => {
                write!(f, "component parse error at {}: {message}", path.display())
            }
            Self::Shape(bag) => {
                write!(f, "expanded case shape invalid ({} diagnostics)", bag.len())
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

/// Reads a Case file under `resolver` root and expands single-level component refs.
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

/// Expands single-level component references from an already-parsed Case document.
///
/// After expansion, runs [`validate_resolved_case`] so referenced component files
/// receive the same semantic checks as inline values.
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

    let resolved = ResolvedCase {
        metadata: document.metadata.clone(),
        time,
        meteorology,
        particle_population,
        substances,
        numerics,
        physics,
        outputs,
        sources: sources.into_values().collect(),
    };
    let shape = validate_resolved_case(&resolved);
    if shape.has_errors() {
        return Err(ExpandError::Shape(shape));
    }
    Ok(resolved)
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

/// Normalizes RunProfile machine paths and records source digests.
///
/// Machine paths (`case_path`, `lockfile`, `cache_root`) are **not** jailed to
/// the Case component root. `case_path` and each `lockfile` must exist;
/// `cache_root` may be absent and is still absolutized when possible.
pub fn expand_run_profile_document(
    document: &RunProfileDocument,
    profile_path: &Path,
    _resolver: &LocalRefResolver,
    profile_digest: SourceDigest,
) -> Result<ResolvedRunProfile, ExpandError> {
    let case_path = require_existing_machine_path(profile_path, &document.case_path)?;
    let mut datasets = Vec::with_capacity(document.datasets.len());
    for binding in &document.datasets {
        let lockfile = require_existing_machine_path(profile_path, &binding.lockfile)?;
        let cache_root = match &binding.cache_root {
            Some(path) => Some(optional_machine_directory(profile_path, path)?),
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

    // Always record the Case identity; do not swallow I/O or hash failures.
    let case_source = read_local_file(&case_path)?;
    sources.insert(case_source.digest.path.clone(), case_source.digest);

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
            let resolved = resolve_single_level(containing_file, reference, resolver, graph)?;
            sources.insert(resolved.digest.path.clone(), resolved.digest.clone());
            let text = bytes_to_str(&resolved.bytes, &resolved.digest.path)?;
            let value = parse_component_text::<T>(text, &resolved.digest.path)?;
            Ok(Some(value))
        }
    }
}

fn resolve_single_level(
    containing_file: &Path,
    reference: &RefPath,
    resolver: &LocalRefResolver,
    graph: &mut ResolutionGraph,
) -> Result<ResolvedSource, ExpandError> {
    let containing = resolver.canonicalize_under_root(containing_file)?;
    let resolved = resolver.resolve(&containing, reference)?;
    // Single-level only: record the edge and reject self-reference cycles.
    graph.add_edge_checking_cycle(containing, resolved.digest.path.clone())?;
    Ok(resolved)
}

fn join_relative(base_file: &Path, relative: &Path) -> PathBuf {
    match base_file.parent() {
        Some(parent) => parent.join(relative),
        None => relative.to_path_buf(),
    }
}

fn machine_path(base_file: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        join_relative(base_file, path)
    }
}

fn require_existing_machine_path(base_file: &Path, path: &Path) -> Result<PathBuf, ExpandError> {
    let candidate = machine_path(base_file, path);
    fs::canonicalize(&candidate).map_err(|error| {
        ExpandError::Resolve(ResolveError::Io {
            path: candidate,
            message: error.to_string(),
        })
    })
}

fn optional_machine_directory(base_file: &Path, path: &Path) -> Result<PathBuf, ExpandError> {
    let candidate = machine_path(base_file, path);
    if let Ok(canonical) = fs::canonicalize(&candidate) {
        return Ok(canonical);
    }
    // Directory may not exist yet. Absolutize against the profile parent when possible.
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let parent = base_file.parent().unwrap_or_else(|| Path::new("."));
    match fs::canonicalize(parent) {
        Ok(parent_canon) => Ok(parent_canon.join(path)),
        Err(error) => Err(ExpandError::Resolve(ResolveError::Io {
            path: parent.to_path_buf(),
            message: error.to_string(),
        })),
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
    fn expands_single_level_component_refs_and_records_sources() {
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
    }

    #[test]
    fn referenced_component_shape_errors_are_reported() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("components")).unwrap();
        fs::write(
            root.join("case.yaml"),
            r#"
schema_version: 0
kind: case
metadata: { name: bad-ref }
time: { ref: components/time.yaml }
particle_population: { ref: components/pop.yaml }
"#,
        )
        .unwrap();
        fs::write(
            root.join("components/time.yaml"),
            r#"
start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
end: { seconds_since_unix_epoch: 10, nanosecond: 0 }
direction: backward
"#,
        )
        .unwrap();
        fs::write(
            root.join("components/pop.yaml"),
            r#"
strategy: domain_fill_air_mass
id: p0
target_particle_count: 0
"#,
        )
        .unwrap();

        let resolver = LocalRefResolver::new(root);
        let err = expand_case_file(&root.join("case.yaml"), &resolver).unwrap_err();
        match err {
            ExpandError::Shape(bag) => {
                assert!(
                    bag.iter()
                        .any(|d| d.code() == "case.time.range_inconsistent")
                );
                assert!(
                    bag.iter()
                        .any(|d| d.code() == "case.population.target_count_zero")
                );
            }
            other => panic!("expected shape error, got {other}"),
        }
    }

    #[test]
    fn run_profile_allows_external_lockfile_and_missing_cache_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        let data = dir.path().join("data-disk");
        fs::create_dir_all(root.join("cases")).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::write(
            root.join("cases/demo.yaml"),
            "schema_version: 0\nkind: case\nmetadata: { name: x }\n",
        )
        .unwrap();
        fs::write(data.join("era5.lock.json"), b"{}").unwrap();
        fs::write(
            root.join("run.yaml"),
            format!(
                r#"
schema_version: 0
kind: run_profile
metadata: {{ name: local }}
case_path: cases/demo.yaml
datasets:
  - dataset: era5
    lockfile: {}
    cache_root: {}
execution:
  worker_threads: 1
  memory_budget_bytes: 1024
  executor: cpu
"#,
                data.join("era5.lock.json")
                    .display()
                    .to_string()
                    .replace('\\', "/"),
                data.join("cache-not-created-yet")
                    .display()
                    .to_string()
                    .replace('\\', "/")
            ),
        )
        .unwrap();

        let resolver = LocalRefResolver::new(&root);
        let resolved = expand_run_profile_file(&root.join("run.yaml"), &resolver).unwrap();
        assert!(resolved.case_path.ends_with("demo.yaml"));
        assert!(resolved.datasets[0].lockfile.ends_with("era5.lock.json"));
        let cache = resolved.datasets[0].cache_root.as_ref().unwrap();
        assert!(cache.ends_with("cache-not-created-yet"));
        assert!(!cache.exists());
        assert_eq!(resolved.sources.len(), 2);
    }

    #[test]
    fn run_profile_propagates_missing_case_hash_failure() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("run.yaml"),
            r#"
schema_version: 0
kind: run_profile
metadata: { name: local }
case_path: missing-case.yaml
datasets: []
execution:
  worker_threads: 1
  memory_budget_bytes: 1024
  executor: cpu
"#,
        )
        .unwrap();
        let resolver = LocalRefResolver::new(root);
        let err = expand_run_profile_file(&root.join("run.yaml"), &resolver).unwrap_err();
        assert!(matches!(err, ExpandError::Resolve(ResolveError::Io { .. })));
    }

    #[test]
    fn detects_self_reference_cycle_on_single_level_edge() {
        let mut graph = ResolutionGraph::new();
        let a = PathBuf::from("a");
        assert!(graph.add_edge_checking_cycle(a.clone(), a).is_err());
    }
}
