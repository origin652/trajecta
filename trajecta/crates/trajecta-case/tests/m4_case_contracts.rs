//! M4-A0 Case and RunProfile contract acceptance tests.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;

use tempfile::tempdir;
use trajecta_case::expand::{ExpandError, expand_case_file, expand_run_profile_file};
use trajecta_case::model::output::{PARTICLE_STATE_PRODUCT_ID, PARTICLE_STATE_SQLITE_SINK_ID};
use trajecta_case::resolver::LocalRefResolver;
use trajecta_case::schema::{SchemaDocument, parse_case_yaml};

fn valid_release_case() -> &'static str {
    r#"
schema_version: 0
kind: case
metadata: { name: m4-release }
time:
  start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 3600, nanosecond: 0 }
  direction: forward
meteorology:
  domains:
    - id: global
      dataset: era5
      priority: 1
      horizontal_halo_cells: 1
substances:
  - id: tracer
    display_name: Tracer
particle_population:
  strategy: release_driven
  id: releases
  events:
    - id: e0
      start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
      end: { seconds_since_unix_epoch: 600, nanosecond: 0 }
      particle_count: 10
      mass:
        tracer: { value: 1, unit: kg }
      geometry:
        source: inline
        geometry:
          type: Polygon
          coordinates:
            - [[179, 10], [-179, 10], [-179, 11], [179, 11], [179, 10]]
      vertical:
        coordinate: above_ground
        lower: { value: 0, unit: m }
        upper: { value: 100, unit: m }
numerics:
  time_step: { value: 10, unit: min }
  integrator: { model: rk2_spherical/v0 }
  boundaries:
    policies: [surface_reflect/v0, model_top_terminate/v0, global_periodic/v0]
  random_seed: 7
"#
}

#[test]
fn omitted_outputs_inject_frozen_particle_sqlite_endpoints() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("case.yaml"), valid_release_case()).unwrap();
    let resolver = LocalRefResolver::new(dir.path());
    let resolved = expand_case_file(&dir.path().join("case.yaml"), &resolver).unwrap();
    assert_eq!(resolved.outputs.len(), 1);
    assert_eq!(resolved.outputs[0].product.0, PARTICLE_STATE_PRODUCT_ID);
    assert_eq!(
        resolved.outputs[0].sink.model.0,
        PARTICLE_STATE_SQLITE_SINK_ID
    );
}

#[test]
fn release_cross_component_errors_are_hard_failures() {
    let dir = tempdir().unwrap();
    let invalid = valid_release_case()
        .replace(
            "seconds_since_unix_epoch: 600",
            "seconds_since_unix_epoch: 7200",
        )
        .replace(
            "tracer: { value: 1, unit: kg }",
            "missing: { value: 1, unit: kg }",
        );
    fs::write(dir.path().join("case.yaml"), invalid).unwrap();
    let resolver = LocalRefResolver::new(dir.path());
    let error = expand_case_file(&dir.path().join("case.yaml"), &resolver).unwrap_err();
    let ExpandError::Shape(diagnostics) = error else {
        panic!("expected shape diagnostics");
    };
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "case.release.outside_simulation_time")
    );
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "case.release.substance_unknown")
    );
}

#[test]
fn domain_fill_requires_known_single_domain_and_exactly_one_target() {
    let dir = tempdir().unwrap();
    let case = r#"
schema_version: 0
kind: case
metadata: { name: domain-fill }
time:
  start: { seconds_since_unix_epoch: 0, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 1, nanosecond: 0 }
  direction: forward
meteorology:
  domains:
    - id: d0
      dataset: era5
      priority: 1
      horizontal_halo_cells: 1
particle_population:
  strategy: domain_fill_air_mass
  id: fill
  domain_id: missing
  target_particle_count: 100
  target_dry_air_mass_per_particle: { value: 1, unit: kg }
numerics:
  time_step: { value: 1, unit: s }
  integrator: { model: rk2_spherical/v0 }
  boundaries: { policies: [] }
"#;
    fs::write(dir.path().join("case.yaml"), case).unwrap();
    let resolver = LocalRefResolver::new(dir.path());
    let error = expand_case_file(&dir.path().join("case.yaml"), &resolver).unwrap_err();
    let ExpandError::Shape(diagnostics) = error else {
        panic!("expected shape diagnostics");
    };
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "case.population.target_both")
    );
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "case.population.domain_unknown")
    );
}

#[test]
fn run_profile_requires_and_normalizes_output_root() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("case.yaml"),
        "schema_version: 0\nkind: case\nmetadata: { name: x }\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("run.yaml"),
        r#"
schema_version: 0
kind: run_profile
metadata: { name: local }
case_path: case.yaml
output_root: output/../runs
datasets: []
execution:
  worker_threads: 1
  memory_budget_bytes: 1024
  executor: cpu
"#,
    )
    .unwrap();
    let resolver = LocalRefResolver::new(dir.path());
    let resolved = expand_run_profile_file(&dir.path().join("run.yaml"), &resolver).unwrap();
    assert!(resolved.output_root.is_absolute());
    assert!(resolved.output_root.ends_with("runs"));
    assert!(!resolved.output_root.exists());
}

#[test]
fn duplicate_boundaries_and_non_geojson_release_files_are_rejected() {
    let invalid = valid_release_case()
        .replace(
            "policies: [surface_reflect/v0, model_top_terminate/v0, global_periodic/v0]",
            "policies: [surface_reflect/v0, surface_reflect/v0]",
        )
        .replace(
            "source: inline\n        geometry:\n          type: Polygon\n          coordinates:\n            - [[179, 10], [-179, 10], [-179, 11], [179, 11], [179, 10]]",
            "source: file\n        path: release.json",
        );
    let document = parse_case_yaml(&invalid).unwrap();
    let diagnostics = document.validate_shape().unwrap();
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "case.numerics.boundary_duplicate")
    );
    assert!(
        diagnostics
            .iter()
            .any(|item| item.code() == "case.release.geometry_path_unsafe")
    );
}
