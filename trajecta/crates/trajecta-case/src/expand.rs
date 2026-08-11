//! # Contract: single-level document expansion
//!
//! v0 expands **one level** of local Case component references into a fully
//! resolved Case, and normalizes RunProfile machine paths. Nested component
//! files must be concrete values; they are not re-expanded.
//!
//! Path policy:
//!
//! - Case component `ref` paths remain jailed under [`LocalRefResolver`]'s root.
//! - RunProfile `case_path`, dataset `lockfile`, `cache_root`, named data roots,
//!   and Profile sources are machine
//!   paths and may live outside the Case root.
//! - `case_path` / `lockfile` must exist as regular files.
//! - Existing `cache_root` must be a directory; missing cache roots are allowed
//!   and are lexically normalized to an absolute path without residual `..`.
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
use std::path::{Component, Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::diagnostic::{Diagnostic, DiagnosticBag, DiagnosticPath};
use crate::document::{
    CaseDocument, DatasetBinding, ProfileSource, ResolvedCase, ResolvedRunProfile,
    RunProfileDocument,
};
use crate::model::meteorology::MeteorologySpec;
use crate::model::numerics::NumericsSpec;
use crate::model::output::{OutputProductSpec, default_particle_state_output};
use crate::model::physics::{PhysicsSelectionSpec, resolve_physics};
use crate::model::population::ParticlePopulationSpec;
use crate::model::substance::SubstanceSpec;
use crate::model::time::TimeSpec;
use crate::reference::{ComponentRef, RefPath};
use crate::resolver::{
    LocalRefResolver, RefResolver, ResolutionGraph, ResolveError, ResolvedSource, SourceDigest,
    read_local_file,
};
use crate::schema::{
    SchemaDocument, SchemaError, parse_case_json, parse_case_yaml, parse_run_profile_json,
    parse_run_profile_yaml, validate_resolved_case,
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
    /// Expanded document failed shared shape validation.
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
                write!(
                    f,
                    "expanded document shape invalid ({} diagnostics)",
                    bag.len()
                )
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
    .unwrap_or_default()
    .into_iter()
    .enumerate()
    .map(|(index, substance): (usize, SubstanceSpec)| {
        substance.normalized_to_si().map_err(|error| {
            let mut diagnostics = DiagnosticBag::new();
            diagnostics.push(
                Diagnostic::error(
                    "case.substance.normalization_failed",
                    format!("substance quantity normalization failed: {error}"),
                )
                .at(DiagnosticPath::root().field("substances").index(index)),
            );
            ExpandError::Shape(diagnostics)
        })
    })
    .collect::<Result<Vec<_>, _>>()?;
    let numerics = expand_component(
        &document.numerics,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?;
    let physics_selection = expand_component(
        &document.physics,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?;
    let physics = physics_selection
        .as_ref()
        .map(|selection| {
            resolve_physics(
                selection,
                crate::diagnostic::DiagnosticPath::root().field("physics"),
            )
        })
        .transpose()
        .map_err(ExpandError::Shape)?;
    let outputs = expand_component(
        &document.outputs,
        case_path,
        resolver,
        &mut graph,
        &mut sources,
    )?
    .unwrap_or_else(|| vec![default_particle_state_output()]);

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
    expand_run_profile_document(&document, &source.digest.path.clone(), source.digest)
}

/// Normalizes RunProfile machine paths and records source digests.
///
/// Machine paths (`case_path`, `lockfile`, `cache_root`, data roots, and Profile
/// sources) are **not** jailed to the Case component root.
///
/// - Document shape is validated before path materialization.
/// - `case_path` and each `lockfile` must exist and be regular files.
/// - Existing `cache_root` must be a directory; missing cache roots are allowed
///   and are lexically normalized to an absolute path without residual `..`.
/// - Named data roots and Profile directories must already exist as directories.
/// - Profile files must already exist as regular files.
/// - Permission and other non-NotFound I/O errors are never treated as absence.
pub fn expand_run_profile_document(
    document: &RunProfileDocument,
    profile_path: &Path,
    profile_digest: SourceDigest,
) -> Result<ResolvedRunProfile, ExpandError> {
    let shape = document.validate_shape()?;
    if shape.has_errors() {
        return Err(ExpandError::Shape(shape));
    }

    let case_path = require_existing_regular_file(profile_path, &document.case_path)?;
    let output_root = absolutize_machine_path(profile_path, &document.output_root)?;
    let mut datasets = Vec::with_capacity(document.datasets.len());
    for binding in &document.datasets {
        let lockfile = require_existing_regular_file(profile_path, &binding.lockfile)?;
        let cache_root = match &binding.cache_root {
            Some(path) => Some(resolve_cache_root(profile_path, path)?),
            None => None,
        };
        let mut data_roots = BTreeMap::new();
        for (root_id, path) in &binding.data_roots {
            let resolved = require_existing_directory(profile_path, path)?;
            data_roots.insert(root_id.clone(), resolved);
        }
        datasets.push(DatasetBinding {
            dataset: binding.dataset.clone(),
            lockfile,
            cache_root,
            data_roots,
            reader_backend: binding.reader_backend,
        });
    }

    let mut profile_sources = Vec::with_capacity(document.profile_sources.len());
    for source in &document.profile_sources {
        let resolved = match source {
            ProfileSource::File { path } => ProfileSource::File {
                path: require_existing_regular_file(profile_path, path)?,
            },
            ProfileSource::Directory { path } => ProfileSource::Directory {
                path: require_existing_directory(profile_path, path)?,
            },
        };
        profile_sources.push(resolved);
    }

    let mut sources = BTreeMap::new();
    sources.insert(profile_digest.path.clone(), profile_digest);

    // Always record the Case identity; do not swallow I/O or hash failures.
    let case_source = read_local_file(&case_path)?;
    sources.insert(case_source.digest.path.clone(), case_source.digest);

    Ok(ResolvedRunProfile {
        metadata: document.metadata.clone(),
        case_path,
        output_root,
        datasets,
        profile_sources,
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

fn require_existing_regular_file(base_file: &Path, path: &Path) -> Result<PathBuf, ExpandError> {
    let candidate = machine_path(base_file, path);
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        ExpandError::Resolve(ResolveError::Io {
            path: candidate.clone(),
            message: error.to_string(),
        })
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        ExpandError::Resolve(ResolveError::Io {
            path: canonical.clone(),
            message: error.to_string(),
        })
    })?;
    if !metadata.is_file() {
        return Err(ExpandError::Resolve(ResolveError::Io {
            path: canonical,
            message: "path exists but is not a regular file".into(),
        }));
    }
    Ok(canonical)
}

fn require_existing_directory(base_file: &Path, path: &Path) -> Result<PathBuf, ExpandError> {
    let candidate = machine_path(base_file, path);
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        ExpandError::Resolve(ResolveError::Io {
            path: candidate.clone(),
            message: error.to_string(),
        })
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        ExpandError::Resolve(ResolveError::Io {
            path: canonical.clone(),
            message: error.to_string(),
        })
    })?;
    if !metadata.is_dir() {
        return Err(ExpandError::Resolve(ResolveError::Io {
            path: canonical,
            message: "path exists but is not a directory".into(),
        }));
    }
    Ok(canonical)
}

fn resolve_cache_root(base_file: &Path, path: &Path) -> Result<PathBuf, ExpandError> {
    let candidate = absolutize_machine_path(base_file, path)?;
    match fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            if metadata.file_type().is_dir() {
                fs::canonicalize(&candidate).map_err(|error| {
                    ExpandError::Resolve(ResolveError::Io {
                        path: candidate,
                        message: error.to_string(),
                    })
                })
            } else {
                Err(ExpandError::Resolve(ResolveError::Io {
                    path: candidate,
                    message: "cache_root exists but is not a directory".into(),
                }))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Missing cache roots are allowed; keep a fully normalized absolute path.
            Ok(candidate)
        }
        Err(error) => Err(ExpandError::Resolve(ResolveError::Io {
            path: candidate,
            message: error.to_string(),
        })),
    }
}

fn absolutize_machine_path(base_file: &Path, path: &Path) -> Result<PathBuf, ExpandError> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let parent = base_file.parent().unwrap_or_else(|| Path::new("."));
        let parent_canon = fs::canonicalize(parent).map_err(|error| {
            ExpandError::Resolve(ResolveError::Io {
                path: parent.to_path_buf(),
                message: error.to_string(),
            })
        })?;
        parent_canon.join(path)
    };
    normalize_lexically(&joined)
}

/// Lexically resolve `.` / `..` without requiring the final path to exist.
fn normalize_lexically(path: &Path) -> Result<PathBuf, ExpandError> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                _ => {
                    return Err(ExpandError::Resolve(ResolveError::Io {
                        path: path.to_path_buf(),
                        message: "path escapes its base after normalization".into(),
                    }));
                }
            },
            Component::Normal(part) => out.push(part),
        }
    }
    Ok(out)
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
    PhysicsSelectionSpec,
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
domain_id: d0
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
    fn run_profile_rejects_invalid_execution_shape() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join("case.yaml"),
            "schema_version: 0\nkind: case\nmetadata: { name: x }\n",
        )
        .unwrap();
        fs::write(
            root.join("run.yaml"),
            r#"
schema_version: 0
kind: run_profile
metadata: { name: local }
case_path: case.yaml
output_root: output
datasets: []
execution:
  worker_threads: 0
  memory_budget_bytes: 0
  executor: ""
"#,
        )
        .unwrap();
        let resolver = LocalRefResolver::new(root);
        let err = expand_run_profile_file(&root.join("run.yaml"), &resolver).unwrap_err();
        match err {
            ExpandError::Shape(bag) => {
                assert!(bag.has_errors());
                assert!(
                    bag.iter()
                        .any(|d| d.code() == "run_profile.worker_threads_zero")
                );
            }
            other => panic!("expected shape error, got {other}"),
        }
    }

    #[test]
    fn run_profile_rejects_directory_case_path_and_file_cache_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("cases")).unwrap();
        fs::write(root.join("lock.json"), b"{}").unwrap();
        fs::write(root.join("not-a-dir.txt"), b"x").unwrap();
        fs::write(
            root.join("run.yaml"),
            r#"
schema_version: 0
kind: run_profile
metadata: { name: local }
case_path: cases
output_root: output
datasets:
  - dataset: era5
    lockfile: lock.json
    cache_root: not-a-dir.txt
execution:
  worker_threads: 1
  memory_budget_bytes: 1024
  executor: cpu
"#,
        )
        .unwrap();
        let resolver = LocalRefResolver::new(root);
        let err = expand_run_profile_file(&root.join("run.yaml"), &resolver).unwrap_err();
        match err {
            ExpandError::Resolve(ResolveError::Io { message, .. }) => {
                assert!(
                    message.contains("not a regular file"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected resolve io error, got {other}"),
        }
    }

    #[test]
    fn run_profile_allows_external_lockfile_and_normalizes_missing_cache_root() {
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
        let missing_cache = data.join("nested").join("..").join("cache-not-created-yet");
        fs::write(
            root.join("run.yaml"),
            format!(
                r#"
schema_version: 0
kind: run_profile
metadata: {{ name: local }}
case_path: cases/demo.yaml
output_root: output
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
                missing_cache.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        let resolver = LocalRefResolver::new(&root);
        let resolved = expand_run_profile_file(&root.join("run.yaml"), &resolver).unwrap();
        assert!(resolved.case_path.ends_with("demo.yaml"));
        assert!(resolved.datasets[0].lockfile.ends_with("era5.lock.json"));
        let cache = resolved.datasets[0].cache_root.as_ref().unwrap();
        assert!(cache.ends_with("cache-not-created-yet"));
        assert!(
            !cache
                .components()
                .any(|c| matches!(c, Component::ParentDir))
        );
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
output_root: output
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
    fn lexical_normalization_collapses_parent_segments() {
        let dir = tempdir().unwrap();
        let base = dir.path().join("run.yaml");
        fs::write(&base, b"x").unwrap();
        let normalized = absolutize_machine_path(&base, Path::new("a/b/../c/./d")).unwrap();
        assert!(normalized.ends_with(Path::new("a").join("c").join("d")));
        assert!(
            !normalized
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        );
    }

    #[test]
    fn detects_self_reference_cycle_on_single_level_edge() {
        let mut graph = ResolutionGraph::new();
        let a = PathBuf::from("a");
        assert!(graph.add_edge_checking_cycle(a.clone(), a).is_err());
    }
}
