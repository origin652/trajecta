//! Executable checks for foundational Case contracts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use trajecta_case::CONTRACT_VERSION;
use trajecta_case::document::{
    CaseDocument, DocumentKind, ExecutionSpec, Metadata, ResolvedCase, RunProfileDocument,
};
use trajecta_case::intent::{CaseRequirements, IntentValidator, ValidationIntent};
use trajecta_case::lockfile::{
    DatasetIdentity, DatasetLock, GridSignature, LockedFile, ProfileIdentity, VerticalSignature,
    parse_dataset_lock_json,
};
use trajecta_case::model::meteorology::{DatasetRef, DomainId, DomainSpec, MeteorologySpec};
use trajecta_case::model::numerics::{BoundarySpec, IntegratorSpec, NumericsSpec};
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::population::{ParticlePopulationSpec, PopulationId, ReleaseDrivenSpec};
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Quantity, QuantityInput, Time, UnitRegistry};
use trajecta_case::reference::{ComponentRef, RefPath};
use trajecta_case::resolver::{LocalRefResolver, RefResolver, sha256_hex};
use trajecta_case::schema::{
    CURRENT_SCHEMA_VERSION, SchemaDocument, parse_case_yaml, parse_run_profile_json,
};

#[test]
fn schema_zero_and_local_reference_boundary_are_explicit() {
    assert_eq!(CURRENT_SCHEMA_VERSION, 0);
    assert_eq!(CONTRACT_VERSION, 0);
    assert!(RefPath::new(PathBuf::from("components/time.yaml")).is_ok());
    assert!(RefPath::new(PathBuf::from("../outside.yaml")).is_err());
}

#[test]
fn simulation_intent_reports_missing_runtime_components() {
    let case = ResolvedCase {
        metadata: Metadata::default(),
        time: None,
        meteorology: None,
        particle_population: None,
        substances: Vec::new(),
        numerics: None,
        physics: Vec::new(),
        outputs: Vec::new(),
        sources: Vec::new(),
    };
    let diagnostics = IntentValidator::validate(&case, ValidationIntent::Simulation);
    assert!(diagnostics.has_errors());
    assert_eq!(diagnostics.len(), 4);
}

#[test]
fn end_to_end_case_parse_validate_and_intent() {
    let yaml = r#"
schema_version: 0
kind: case
metadata:
  name: demo-case
  authors: ["Origin"]
time:
  start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 7200, nanosecond: 0 }
  direction: forward
meteorology:
  domains:
    - id: global
      dataset: era5
      priority: 1
      horizontal_halo_cells: 1
particle_population:
  strategy: release_driven
  id: release-a
  schedule: continuous
numerics:
  time_step: { value: 10, unit: min }
  integrator:
    model: rk2_spherical/v0
  boundaries:
    policies: [ground_reflect, model_top_terminate]
"#;
    let doc = parse_case_yaml(yaml).expect("parse case");
    let shape = doc.validate_shape().expect("shape");
    assert!(!shape.has_errors(), "{:?}", shape.sorted());

    let resolved = ResolvedCase {
        metadata: doc.metadata.clone(),
        time: match doc.time.clone() {
            Some(ComponentRef::Inline(v)) => Some(v),
            _ => None,
        },
        meteorology: match doc.meteorology.clone() {
            Some(ComponentRef::Inline(v)) => Some(v),
            _ => None,
        },
        particle_population: match doc.particle_population.clone() {
            Some(ComponentRef::Inline(v)) => Some(v),
            _ => None,
        },
        substances: Vec::new(),
        numerics: match doc.numerics.clone() {
            Some(ComponentRef::Inline(v)) => Some(v),
            _ => None,
        },
        physics: Vec::new(),
        outputs: Vec::new(),
        sources: Vec::new(),
    };
    let probe = IntentValidator::validate(&resolved, ValidationIntent::MetProbe);
    assert!(probe.is_empty());
    let sim = IntentValidator::validate(&resolved, ValidationIntent::Simulation);
    assert!(sim.is_empty());
    assert!(CaseRequirements::for_intent(ValidationIntent::Simulation).numerics);
}

#[test]
fn quantity_registry_and_numerics_si_normalization() {
    let registry = UnitRegistry::standard();
    let step = registry
        .resolve::<Time>(&QuantityInput::object(10.0, "min"))
        .unwrap();
    assert!((step.value_si() - 600.0).abs() < 1e-12);
    let numerics = NumericsSpec {
        time_step: step,
        integrator: IntegratorSpec {
            model: ModelId("rk2_spherical/v0".into()),
            parameters: Default::default(),
        },
        boundaries: BoundarySpec {
            policies: vec![ModelId("ground_reflect".into())],
        },
    };
    let json = serde_json::to_string(&numerics).unwrap();
    let back: NumericsSpec = serde_json::from_str(&json).unwrap();
    assert!((back.time_step.value_si() - 600.0).abs() < 1e-12);
}

#[test]
fn run_profile_and_lock_pipeline() {
    let profile = r#"
{
  "schema_version": 0,
  "kind": "run_profile",
  "metadata": { "name": "local" },
  "case_path": "cases/demo.yaml",
  "datasets": [
    {
      "dataset": "era5",
      "lockfile": "locks/era5.lock.json"
    }
  ],
  "execution": {
    "worker_threads": 2,
    "memory_budget_bytes": 1048576,
    "executor": "cpu"
  }
}
"#;
    let doc = parse_run_profile_json(profile).unwrap();
    assert!(!doc.validate_shape().unwrap().has_errors());

    let payload = b"frame-bytes";
    let lock = DatasetLock {
        schema_version: 0,
        identity: DatasetIdentity {
            id: DatasetRef("era5".into()),
            source: "ECMWF".into(),
            attribution: None,
        },
        profile: ProfileIdentity {
            name: "era5".into(),
            sha256: sha256_hex(b"profile"),
        },
        files: vec![LockedFile {
            role: "analysis".into(),
            relative_path: PathBuf::from("a.grib"),
            valid_time: None,
            size_bytes: payload.len() as u64,
            sha256: sha256_hex(payload),
        }],
        grid: GridSignature {
            nx: 4,
            ny: 4,
            periodic_longitude: false,
            sha256: sha256_hex(b"grid"),
        },
        vertical: VerticalSignature::PressureLevels {
            level_count: 3,
            levels_sha256: sha256_hex(b"levels"),
        },
    };
    let encoded = serde_json::to_string(&lock).unwrap();
    let parsed = parse_dataset_lock_json(&encoded).unwrap();
    assert!(!parsed.validate_shape().has_errors());
}

#[test]
fn local_ref_resolver_reads_under_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("components")).unwrap();
    std::fs::write(root.join("case.yaml"), b"schema_version: 0\n").unwrap();
    std::fs::write(root.join("components/time.yaml"), b"hello").unwrap();
    let resolver = LocalRefResolver::new(root);
    let resolved = resolver
        .resolve(
            &root.join("case.yaml"),
            &RefPath::new("components/time.yaml").unwrap(),
        )
        .unwrap();
    assert_eq!(resolved.bytes, b"hello");
    assert_eq!(resolved.digest.size_bytes, 5);
}

#[test]
fn population_and_domain_specs_roundtrip() {
    let pop = ParticlePopulationSpec::ReleaseDriven(ReleaseDrivenSpec {
        id: PopulationId("p0".into()),
        schedule: "s0".into(),
    });
    let yaml = serde_yml::to_string(&pop).unwrap();
    let back: ParticlePopulationSpec = serde_yml::from_str(&yaml).unwrap();
    assert_eq!(pop, back);

    let met = MeteorologySpec {
        domains: vec![DomainSpec {
            id: DomainId("d0".into()),
            dataset: DatasetRef("era5".into()),
            priority: 5,
            parent: None,
            horizontal_halo_cells: 1,
        }],
    };
    let case = CaseDocument {
        schema_version: 0,
        kind: DocumentKind::Case,
        metadata: Metadata {
            name: "x".into(),
            ..Metadata::default()
        },
        time: Some(ComponentRef::Inline(TimeSpec {
            start: Timestamp::UNIX_EPOCH,
            end: Timestamp::new(1, 0).unwrap(),
            direction: Direction::Forward,
        })),
        meteorology: Some(ComponentRef::Inline(met)),
        particle_population: None,
        substances: None,
        numerics: Some(ComponentRef::Inline(NumericsSpec {
            time_step: Quantity::<Time>::resolve(
                &QuantityInput::text("900 s"),
                &UnitRegistry::standard(),
            )
            .unwrap(),
            integrator: IntegratorSpec {
                model: ModelId("rk2".into()),
                parameters: Default::default(),
            },
            boundaries: BoundarySpec { policies: vec![] },
        })),
        physics: None,
        outputs: None,
    };
    assert!(!case.validate_shape().unwrap().has_errors());

    let _run = RunProfileDocument {
        schema_version: 0,
        kind: DocumentKind::RunProfile,
        metadata: Metadata {
            name: "r".into(),
            ..Metadata::default()
        },
        case_path: PathBuf::from("case.yaml"),
        datasets: vec![],
        execution: ExecutionSpec {
            worker_threads: 1,
            memory_budget_bytes: 1024,
            executor: "cpu".into(),
        },
    };
}
