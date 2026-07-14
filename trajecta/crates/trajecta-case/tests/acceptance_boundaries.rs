//! Boundary acceptance tests required after the B-tier review.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use tempfile::tempdir;
use trajecta_case::document::{DataRootId, DocumentKind};
use trajecta_case::expand::{expand_case_file, expand_run_profile_file};
use trajecta_case::lockfile::{
    DatasetIdentity, DatasetLock, GeneratorInfo, GridSignature, LockedFile, ProfileIdentity,
    VerticalSignature,
};
use trajecta_case::model::meteorology::DatasetRef;
use trajecta_case::model::time::TimeSpec;
use trajecta_case::model::time::Timestamp;
use trajecta_case::quantity::Unit;
use trajecta_case::reference::ComponentRef;
use trajecta_case::resolver::{LocalRefResolver, sha256_hex};
use trajecta_case::schema::{SchemaDocument, parse_case_json, parse_case_yaml};

#[test]
fn timestamp_nanosecond_boundary_rejected_in_yaml_and_json() {
    let yaml = "seconds_since_unix_epoch: 0\nnanosecond: 1000000000\n";
    assert!(serde_yml::from_str::<Timestamp>(yaml).is_err());
    let json = r#"{"seconds_since_unix_epoch":0,"nanosecond":1000000000}"#;
    assert!(serde_json::from_str::<Timestamp>(json).is_err());
}

#[test]
fn unit_zero_scale_rejected_in_yaml_and_json() {
    let yaml = r#"
symbol: bad
dimension: time
scale_to_si: 0
offset_to_si: 0
"#;
    assert!(serde_yml::from_str::<Unit>(yaml).is_err());
    let json = r#"{"symbol":"bad","dimension":"time","scale_to_si":0.0,"offset_to_si":0.0}"#;
    assert!(serde_json::from_str::<Unit>(json).is_err());
}

#[test]
fn case_unknown_field_and_mixed_ref_inline_rejected() {
    let unknown = r#"
schema_version: 0
kind: case
metadata: { name: x }
unknown_top: 1
"#;
    assert!(parse_case_yaml(unknown).is_err());

    let mixed = r#"
schema_version: 0
kind: case
metadata: { name: x }
time:
  ref: components/time.yaml
  direction: forward
"#;
    assert!(parse_case_yaml(mixed).is_err());

    let nested_unknown = r#"
{
  "schema_version": 0,
  "kind": "case",
  "metadata": { "name": "x", "nope": true }
}
"#;
    assert!(parse_case_json(nested_unknown).is_err());
}

#[test]
fn target_particle_count_zero_rejected_by_shape() {
    let yaml = r#"
schema_version: 0
kind: case
metadata: { name: p }
particle_population:
  strategy: domain_fill_air_mass
  id: p0
  target_particle_count: 0
"#;
    let doc = parse_case_yaml(yaml).unwrap();
    let bag = doc.validate_shape().unwrap();
    assert!(
        bag.iter()
            .any(|d| d.code() == "case.population.target_count_zero")
    );
}

#[test]
fn lock_symlink_escape_rejected_when_supported() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("lockroot");
    fs::create_dir_all(&root).unwrap();
    let outside = dir.path().join("secret.bin");
    let payload = b"secret-bytes";
    fs::write(&outside, payload).unwrap();
    let link = root.join("linked.bin");

    let symlink_ok = {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, &link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(&outside, &link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    };
    if !symlink_ok {
        // Fallback: parent path must still be rejected by shape validation.
        let mut lock = sample_lock("x.bin", payload);
        lock.files[0].relative_path = PathBuf::from("../secret.bin");
        assert!(
            lock.validate_shape()
                .iter()
                .any(|d| d.code() == "lock.file.path_unsafe")
        );
        return;
    }

    let lock = sample_lock("linked.bin", payload);
    let bag = lock.verify_local_files(&root).unwrap();
    assert!(
        bag.iter()
            .any(|d| d.code() == "lock.file.path_escapes_root"),
        "{:?}",
        bag.sorted()
    );
}

#[test]
fn expand_case_resolves_refs_deterministically() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("components")).unwrap();
    fs::write(
        root.join("case.yaml"),
        r#"
schema_version: 0
kind: case
metadata: { name: expanded }
time: { ref: components/time.yaml }
meteorology: { ref: components/met.yaml }
"#,
    )
    .unwrap();
    fs::write(
        root.join("components/time.yaml"),
        r#"
start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
end: { seconds_since_unix_epoch: 5, nanosecond: 0 }
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
    assert_eq!(resolved.meteorology.unwrap().domains[0].id.0, "d0");
    assert_eq!(resolved.sources.len(), 3);
}

#[test]
fn component_ref_only_ref_key_allowed() {
    let json = r#"{"ref":"components/time.yaml","extra":1}"#;
    assert!(serde_json::from_str::<ComponentRef<TimeSpec>>(json).is_err());
    let ok = r#"{"ref":"components/time.yaml"}"#;
    let parsed: ComponentRef<TimeSpec> = serde_json::from_str(ok).unwrap();
    assert!(matches!(parsed, ComponentRef::Ref(_)));
}

fn sample_lock(file_name: &str, bytes: &[u8]) -> DatasetLock {
    DatasetLock {
        schema_version: 0,
        identity: DatasetIdentity {
            id: DatasetRef("era5".into()),
            source: "ECMWF".into(),
            source_url: None,
            attribution: None,
        },
        profile: ProfileIdentity {
            name: "era5".into(),
            sha256: sha256_hex(b"profile"),
        },
        generator: GeneratorInfo {
            tool: "trajecta-data-lock".into(),
            version: "0.0.0".into(),
        },
        files: vec![LockedFile {
            roles: vec!["analysis".into()],
            root_id: DataRootId(DataRootId::LOCKFILE.into()),
            relative_path: PathBuf::from(file_name),
            valid_times: Vec::new(),
            size_bytes: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        }],
        grid: GridSignature {
            nx: 2,
            ny: 2,
            periodic_longitude: false,
            sha256: sha256_hex(b"grid"),
        },
        vertical: VerticalSignature::PressureLevels {
            level_count: 1,
            levels_sha256: sha256_hex(b"levels"),
        },
    }
}

#[test]
fn expand_run_profile_records_sources() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("cases")).unwrap();
    fs::write(
        root.join("cases/demo.yaml"),
        "schema_version: 0\nkind: case\nmetadata: { name: x }\n",
    )
    .unwrap();
    fs::write(
        root.join("run.yaml"),
        r#"
schema_version: 0
kind: run_profile
metadata: { name: local }
case_path: cases/demo.yaml
datasets: []
execution:
  worker_threads: 1
  memory_budget_bytes: 1024
  executor: cpu
"#,
    )
    .unwrap();
    let resolver = LocalRefResolver::new(root);
    let resolved = expand_run_profile_file(&root.join("run.yaml"), &resolver).unwrap();
    assert_eq!(resolved.execution.executor, "cpu");
    assert!(!resolved.sources.is_empty());
    let _ = DocumentKind::RunProfile;
}
