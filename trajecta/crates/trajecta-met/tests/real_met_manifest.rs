//! Self-check for `testdata/REAL_MET_MANIFEST.json` against on-disk anchors.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn require_real_met() -> bool {
    matches!(
        std::env::var("TRAJECTA_REQUIRE_REAL_MET").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("trajecta workspace root")
        .to_path_buf()
}

fn monorepo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("monorepo root")
        .to_path_buf()
}

/// Resolve a manifest-relative path against both the Trajecta workspace root and
/// the monorepo root (`E:/flexpart`). Real GRIB anchors live under
/// `tools/flexctl/...` on the monorepo root, while NetCDF offline anchors live
/// under `trajecta/target/test-data/...`.
fn resolve_existing_path(rel: &str) -> Option<PathBuf> {
    let candidates = [
        monorepo_root().join(rel),
        workspace_root().join(rel),
        PathBuf::from(rel),
    ];
    candidates.into_iter().find(|path| path.exists())
}

fn resolve_existing_dir(rel: &str) -> Option<PathBuf> {
    resolve_existing_path(rel).filter(|path| path.is_dir())
}

fn manifest_path() -> PathBuf {
    workspace_root().join("testdata/REAL_MET_MANIFEST.json")
}

fn resolve_dataset_dir(dataset: &serde_json::Value) -> Option<PathBuf> {
    if let Some(env_name) = dataset.get("environment_variable").and_then(|v| v.as_str()) {
        if let Ok(path) = std::env::var(env_name) {
            let candidate = PathBuf::from(path);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
    }
    for fallback in dataset
        .get("fallback_paths")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let Some(rel) = fallback.as_str() else {
            continue;
        };
        if let Some(dir) = resolve_existing_dir(rel) {
            return Some(dir);
        }
    }
    None
}

fn large_real_met_enabled() -> bool {
    require_real_met()
        || matches!(
            std::env::var("TRAJECTA_LARGE_REAL_MET").as_deref(),
            Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
        )
}

fn sha256_file(path: &Path) -> String {
    use std::io::{BufReader, Read};
    let file =
        fs::File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 1 << 20];
    loop {
        let n = reader
            .read(&mut buf)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    hex::encode(hasher.finalize())
}

fn assert_file_hash(label: &str, path: &Path, expected_size: u64, expected_sha: &str) {
    assert!(path.is_file(), "{label}: missing file {}", path.display());
    let meta = fs::metadata(path).unwrap();
    assert_eq!(
        meta.len(),
        expected_size,
        "{label}: size mismatch for {}",
        path.display()
    );
    let actual_sha = sha256_file(path);
    assert_eq!(
        actual_sha,
        expected_sha,
        "{label}: sha256 mismatch for {}",
        path.display()
    );
}

#[test]
fn real_met_manifest_paths_and_hashes_match_disk() {
    let path = manifest_path();
    assert!(path.is_file(), "missing {}", path.display());
    let raw = fs::read_to_string(&path).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&raw).expect("manifest json");
    let datasets = manifest
        .get("datasets")
        .and_then(|v| v.as_object())
        .expect("datasets object");

    let mut checked = 0usize;
    let mut missing_dirs = BTreeMap::new();
    for (name, dataset) in datasets {
        let files = dataset
            .get("files")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if files.is_empty() {
            continue;
        }
        // Multi-hundred-MB official year files: only hash under explicit large/real switches.
        let is_large = dataset
            .get("kind")
            .and_then(|v| v.as_str())
            .is_some_and(|k| k == "official_psl_original")
            || name.contains("noaa_psl_ncep_reanalysis1");
        if is_large && !large_real_met_enabled() {
            continue;
        }
        let Some(dir) = resolve_dataset_dir(dataset) else {
            missing_dirs.insert(name.clone(), dataset.clone());
            continue;
        };
        for file in files {
            let file_name = file
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{name}: file entry missing name"));
            let expected_sha = file
                .get("sha256")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("{name}/{file_name}: missing sha256"));
            let expected_size = file
                .get("size")
                .and_then(|v| v.as_u64())
                .unwrap_or_else(|| panic!("{name}/{file_name}: missing size"));
            let on_disk = dir.join(file_name);
            assert_file_hash(
                &format!("{name}/{file_name}"),
                &on_disk,
                expected_size,
                expected_sha,
            );

            // Optional source GRIB must exist and, when frozen, match size/hash.
            if let Some(source) = file.get("source_grib").and_then(|v| v.as_str()) {
                let source_path = resolve_existing_path(source).unwrap_or_else(|| {
                    panic!(
                        "{name}/{file_name}: source_grib not found under monorepo or workspace: {source}"
                    )
                });
                if let (Some(size), Some(sha)) = (
                    file.get("source_grib_size").and_then(|v| v.as_u64()),
                    file.get("source_grib_sha256").and_then(|v| v.as_str()),
                ) {
                    assert_file_hash(
                        &format!("{name}/{file_name}/source_grib"),
                        &source_path,
                        size,
                        sha,
                    );
                } else {
                    assert!(
                        source_path.is_file(),
                        "{name}/{file_name}: source_grib missing {}",
                        source_path.display()
                    );
                }
            }
            checked += 1;
        }
    }

    if require_real_met() {
        assert!(
            missing_dirs.is_empty(),
            "TRAJECTA_REQUIRE_REAL_MET set but dataset dirs missing: {:?}. Tried monorepo_root={} workspace_root={}",
            missing_dirs.keys().collect::<Vec<_>>(),
            monorepo_root().display(),
            workspace_root().display()
        );
        assert!(
            checked > 0,
            "TRAJECTA_REQUIRE_REAL_MET set but no manifest files were checked"
        );
    } else if checked == 0 {
        // Soft path: no local anchors present.
        return;
    }
    assert!(
        checked >= 14,
        "expected to verify at least the CFSR-derived multifile set, checked={checked}"
    );
}

/// FETCH_MANIFEST.json produced by tools/fetch_noaa_psl_ncep_r1.py must agree with
/// REAL_MET_MANIFEST.json on size/sha for each official PSL file (when present).
#[test]
fn real_met_fetch_manifest_matches_real_met_manifest() {
    let real_path = manifest_path();
    let real: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&real_path).unwrap()).expect("real manifest");
    let Some(dataset) =
        real.pointer("/datasets/noaa_psl_ncep_reanalysis1_official_multifile_netcdf")
    else {
        return;
    };
    let fetch_candidates = [
        workspace_root()
            .join("target/test-data/noaa-psl-ncep-reanalysis1-official/FETCH_MANIFEST.json"),
        monorepo_root().join(
            "trajecta/target/test-data/noaa-psl-ncep-reanalysis1-official/FETCH_MANIFEST.json",
        ),
    ];
    let Some(fetch_path) = fetch_candidates.into_iter().find(|p| p.is_file()) else {
        if large_real_met_enabled() {
            panic!("TRAJECTA_REQUIRE_REAL_MET/LARGE set but FETCH_MANIFEST.json missing");
        }
        return;
    };
    let fetch: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&fetch_path).unwrap()).expect("fetch manifest");
    let real_files = dataset
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let fetch_files = fetch
        .get("files")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut fetch_by_name = BTreeMap::new();
    for file in fetch_files {
        let name = file
            .get("name")
            .and_then(|v| v.as_str())
            .expect("fetch file name")
            .to_owned();
        fetch_by_name.insert(name, file);
    }
    assert!(!real_files.is_empty(), "REAL_MET_MANIFEST PSL files empty");
    for file in real_files {
        let name = file
            .get("name")
            .and_then(|v| v.as_str())
            .expect("real file name");
        let fetch_file = fetch_by_name
            .get(name)
            .unwrap_or_else(|| panic!("FETCH_MANIFEST missing {name}"));
        assert_eq!(
            file.get("size"),
            fetch_file.get("size"),
            "{name} size mismatch between REAL and FETCH"
        );
        assert_eq!(
            file.get("sha256"),
            fetch_file.get("sha256"),
            "{name} sha256 mismatch between REAL and FETCH"
        );
    }
}
