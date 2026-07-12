//! # Contract: local reference resolution
//!
//! Resolution expands validated local references, records source identities,
//! and rejects cycles. It never performs network I/O.
//!
//! ## Exposed interface
//!
//! | Item | Role |
//! |---|---|
//! | [`SourceDigest`] | Path + size + SHA-256 |
//! | [`ResolvedSource`] | Bytes + digest |
//! | [`RefResolver`] | Object-safe resolve trait |
//! | [`LocalRefResolver`] | Root-scoped local reader |
//! | [`ResolutionGraph`] | Cycle-aware edge store |
//! | [`sha256_hex`] / [`sha256_file_streaming`] | Digest helpers |
//! | [`ResolveError`] | Resolution failures |

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::reference::RefPath;

/// Immutable identity of a source document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDigest {
    /// Canonical source path recorded for provenance.
    pub path: PathBuf,
    /// Source length in bytes.
    pub size_bytes: u64,
    /// Lowercase SHA-256 hexadecimal digest.
    pub sha256: String,
}

/// Bytes and identity returned by a reference resolver.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSource {
    /// Exact source bytes.
    pub bytes: Vec<u8>,
    /// Immutable source identity.
    pub digest: SourceDigest,
}

/// Object-safe interface for resolving a validated component reference.
pub trait RefResolver: Send + Sync {
    /// Resolves a reference relative to a containing document.
    fn resolve(
        &self,
        containing_file: &Path,
        reference: &RefPath,
    ) -> Result<ResolvedSource, ResolveError>;
}

/// Local-only resolver used by v0.
#[derive(Clone, Debug)]
pub struct LocalRefResolver {
    root: PathBuf,
}

impl LocalRefResolver {
    /// Restricts all resolution to one local root.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the configured resolution root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves an absolute or root-relative path into a canonical path under root.
    pub fn canonicalize_under_root(&self, path: &Path) -> Result<PathBuf, ResolveError> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let canonical = fs::canonicalize(&absolute).map_err(|error| ResolveError::Io {
            path: absolute.clone(),
            message: error.to_string(),
        })?;
        let root = fs::canonicalize(&self.root).map_err(|error| ResolveError::Io {
            path: self.root.clone(),
            message: error.to_string(),
        })?;
        if !path_is_within(&canonical, &root) {
            return Err(ResolveError::EscapesRoot(canonical));
        }
        Ok(canonical)
    }

    /// Reads and digests a file known to live under the resolution root.
    ///
    /// Small configuration documents are loaded into memory. Large scientific
    /// payloads should use [`sha256_file_streaming`] instead.
    pub fn read_file(&self, path: &Path) -> Result<ResolvedSource, ResolveError> {
        let canonical = self.canonicalize_under_root(path)?;
        let mut file = fs::File::open(&canonical).map_err(|error| ResolveError::Io {
            path: canonical.clone(),
            message: error.to_string(),
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| ResolveError::Io {
                path: canonical.clone(),
                message: error.to_string(),
            })?;
        let digest = SourceDigest {
            path: canonical,
            size_bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
        };
        Ok(ResolvedSource { bytes, digest })
    }
}

impl RefResolver for LocalRefResolver {
    fn resolve(
        &self,
        containing_file: &Path,
        reference: &RefPath,
    ) -> Result<ResolvedSource, ResolveError> {
        let containing = self.canonicalize_under_root(containing_file)?;
        let parent = containing
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        if reference_path_escapes(reference.as_path()) {
            return Err(ResolveError::EscapesRoot(parent.join(reference.as_path())));
        }
        let candidate = parent.join(reference.as_path());
        self.read_file(&candidate)
    }
}

/// Directed source graph used to detect cycles and retain provenance chains.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolutionGraph {
    edges: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
}

impl ResolutionGraph {
    /// Creates an empty graph.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            edges: BTreeMap::new(),
        }
    }

    /// Records one directed reference edge.
    pub fn add_edge(&mut self, from: PathBuf, to: PathBuf) {
        self.edges.entry(from).or_default().insert(to);
    }

    /// Records an edge and rejects the operation when it would create a cycle.
    pub fn add_edge_checking_cycle(
        &mut self,
        from: PathBuf,
        to: PathBuf,
    ) -> Result<(), ResolveError> {
        if from == to || self.reaches(&to, &from) {
            let mut cycle = vec![from.clone()];
            if from != to {
                cycle.push(to.clone());
                cycle.push(from.clone());
            }
            return Err(ResolveError::Cycle(cycle));
        }
        self.add_edge(from, to);
        Ok(())
    }

    /// Returns true when `from` can reach `target` through existing edges.
    #[must_use]
    pub fn reaches(&self, from: &Path, target: &Path) -> bool {
        let mut stack = vec![from.to_path_buf()];
        let mut seen = BTreeSet::new();
        while let Some(node) = stack.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            if node == target {
                return true;
            }
            if let Some(children) = self.edges.get(&node) {
                stack.extend(children.iter().cloned());
            }
        }
        false
    }

    /// Returns true when the graph currently contains a cycle.
    #[must_use]
    pub fn has_cycle(&self) -> bool {
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        for node in self.edges.keys() {
            if dfs_cycle(node, &self.edges, &mut visiting, &mut visited) {
                return true;
            }
        }
        false
    }

    /// Returns the deterministic edge map.
    #[must_use]
    pub const fn edges(&self) -> &BTreeMap<PathBuf, BTreeSet<PathBuf>> {
        &self.edges
    }
}

fn dfs_cycle(
    node: &Path,
    edges: &BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    visiting: &mut BTreeSet<PathBuf>,
    visited: &mut BTreeSet<PathBuf>,
) -> bool {
    if visited.contains(node) {
        return false;
    }
    if !visiting.insert(node.to_path_buf()) {
        return true;
    }
    if let Some(children) = edges.get(node) {
        for child in children {
            if dfs_cycle(child, edges, visiting, visited) {
                return true;
            }
        }
    }
    visiting.remove(node);
    visited.insert(node.to_path_buf());
    false
}

fn reference_path_escapes(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    })
}

/// Returns true when `path` is equal to or nested under `root` after canonicalize.
#[must_use]
pub fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

/// Computes the lowercase SHA-256 hex digest of raw bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

/// Streams a file through SHA-256 using a fixed-size buffer.
///
/// This is the only supported way to digest potentially multi-gigabyte
/// meteorological payloads. It never loads the full file into memory.
pub fn sha256_file_streaming(path: &Path) -> Result<(u64, String), ResolveError> {
    let mut file = fs::File::open(path).map_err(|error| ResolveError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| ResolveError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total.saturating_add(read as u64);
    }
    Ok((total, hex::encode(hasher.finalize())))
}

/// Returns true when `value` is a 64-character lowercase hexadecimal string.
#[must_use]
pub fn is_lowercase_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Reference-resolution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveError {
    /// A reference escapes the configured root.
    EscapesRoot(PathBuf),
    /// A cycle was detected in the source graph.
    Cycle(Vec<PathBuf>),
    /// A local source could not be read.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Stable, sanitized I/O message.
        message: String,
    },
    /// A component document could not be parsed.
    Parse {
        /// Path that failed.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EscapesRoot(path) => {
                write!(f, "reference escapes resolution root: {}", path.display())
            }
            Self::Cycle(paths) => {
                write!(f, "reference cycle detected: ")?;
                for (i, path) in paths.iter().enumerate() {
                    if i > 0 {
                        write!(f, " -> ")?;
                    }
                    write!(f, "{}", path.display())?;
                }
                Ok(())
            }
            Self::Io { path, message } => {
                write!(f, "I/O error at {}: {message}", path.display())
            }
            Self::Parse { path, message } => {
                write!(f, "parse error at {}: {message}", path.display())
            }
        }
    }
}

impl std::error::Error for ResolveError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn sha256_helper_is_lowercase_hex() {
        let digest = sha256_hex(b"trajecta");
        assert!(is_lowercase_sha256_hex(&digest));
        assert!(!is_lowercase_sha256_hex("abc"));
    }

    #[test]
    fn streaming_digest_matches_memory_digest() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let payload = vec![7_u8; 200_000];
        fs::write(&path, &payload).unwrap();
        let (size, digest) = sha256_file_streaming(&path).unwrap();
        assert_eq!(size, payload.len() as u64);
        assert_eq!(digest, sha256_hex(&payload));
    }

    #[test]
    fn local_resolver_reads_relative_component() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let case_path = root.join("case.yaml");
        let component_path = root.join("components").join("time.yaml");
        fs::create_dir_all(component_path.parent().unwrap()).unwrap();
        fs::write(&case_path, b"kind: case\n").unwrap();
        let mut file = fs::File::create(&component_path).unwrap();
        writeln!(file, "payload: 1").unwrap();

        let resolver = LocalRefResolver::new(root);
        let reference = RefPath::new("components/time.yaml").unwrap();
        let resolved = resolver.resolve(&case_path, &reference).unwrap();
        assert_eq!(resolved.bytes, b"payload: 1\n");
        assert!(is_lowercase_sha256_hex(&resolved.digest.sha256));
    }

    #[test]
    fn local_resolver_rejects_escape() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("secret.txt");
        fs::write(&outside, b"nope").unwrap();
        let resolver = LocalRefResolver::new(&root);
        let result = resolver.read_file(&outside);
        assert!(matches!(result, Err(ResolveError::EscapesRoot(_))));
    }

    #[test]
    fn resolution_graph_detects_cycles() {
        let mut graph = ResolutionGraph::new();
        let a = PathBuf::from("a.yaml");
        let b = PathBuf::from("b.yaml");
        graph.add_edge_checking_cycle(a.clone(), b.clone()).unwrap();
        let err = graph.add_edge_checking_cycle(b, a).unwrap_err();
        assert!(matches!(err, ResolveError::Cycle(_)));
    }
}
