//! Machine-readable M4-A0 contract checks.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use serde_json::Value;
use trajecta_case::model::output::{PARTICLE_STATE_PRODUCT_ID, PARTICLE_STATE_SQLITE_SINK_ID};
use trajecta_core::rng::{
    COUNTER_RNG_ALGORITHM_ID, RELEASE_BIRTH_TIME_DIMENSION, RELEASE_HORIZONTAL_COMPONENT_DIMENSION,
    RELEASE_HORIZONTAL_U_DIMENSION, RELEASE_HORIZONTAL_V_DIMENSION, RELEASE_VERTICAL_DIMENSION,
};
use trajecta_core::science::{
    DOMAIN_FILL_MASS_LEDGER_ID, DRY_AIR_DOMAIN_FILL_ID, DRY_AIR_MOLAR_MASS_G_MOL,
    ERTEL_PV_SPHERICAL_ID, FLEXPART_PV60_OZONE_ID, GEOMETRY_AREA_RELATIVE_TOLERANCE,
    GLOBAL_PERIODIC_ID, LIMITED_DOMAIN_TERMINATE_ID, M4_CONSTANTS, M4_PARTICLE_LOOP_CONTRACT_ID,
    MASS_BALANCE_FINAL_RELATIVE_TOLERANCE, MASS_BALANCE_STEP_RELATIVE_TOLERANCE,
    MASS_BALANCE_ULP_FLOOR, MODEL_TOP_TERMINATE_ID, OZONE_DOMAIN_FILL_ID, OZONE_MOLAR_MASS_G_MOL,
    OZONE_RULE_MINIMUM_HEIGHT_ASL_M, OZONE_RULE_MINIMUM_PV_PVU, OZONE_RULE_PPBV_PER_PVU,
    PRESSURE_LOG_INTERFACE_ID, PROVENANCE_BUNDLE_FILE_NAME, PROVENANCE_BUNDLE_SCHEMA_ID,
    PROVENANCE_RECORD_HASH_ALGORITHM, RELEASE_BIRTH_STRATA_ID, RELEASE_DRIVEN_POPULATION_ID,
    RK2_MINIMUM_CONVERGENCE_ORDER, RK2_SPHERICAL_ID, RUN_MANIFEST_SCHEMA_ID,
    SPHERICAL_CELL_AREA_ID, SQLITE_SCHEMA_VERSION, SURFACE_REFLECT_ID,
};

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
}

#[test]
fn numerical_contract_json_matches_compiled_science_contract() {
    let path = workspace().join("testdata/M4_NUMERICAL_CONTRACT.v1.json");
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        value.get("contract_id").and_then(Value::as_str),
        Some(M4_PARTICLE_LOOP_CONTRACT_ID)
    );
    let algorithms = value.get("algorithms").and_then(Value::as_object).unwrap();
    let expected_algorithms = [
        ("counter_rng", COUNTER_RNG_ALGORITHM_ID),
        ("integrator", RK2_SPHERICAL_ID),
        ("surface_boundary", SURFACE_REFLECT_ID),
        ("model_top_boundary", MODEL_TOP_TERMINATE_ID),
        ("limited_domain_boundary", LIMITED_DOMAIN_TERMINATE_ID),
        ("global_boundary", GLOBAL_PERIODIC_ID),
        ("release_population", RELEASE_DRIVEN_POPULATION_ID),
        ("release_birth_time", RELEASE_BIRTH_STRATA_ID),
        ("air_mass_population", DRY_AIR_DOMAIN_FILL_ID),
        ("ozone_population", OZONE_DOMAIN_FILL_ID),
        ("cell_area", SPHERICAL_CELL_AREA_ID),
        ("pressure_interfaces", PRESSURE_LOG_INTERFACE_ID),
        ("mass_ledger", DOMAIN_FILL_MASS_LEDGER_ID),
        ("potential_vorticity", ERTEL_PV_SPHERICAL_ID),
        ("ozone_rule", FLEXPART_PV60_OZONE_ID),
        ("particle_product", PARTICLE_STATE_PRODUCT_ID),
        ("particle_sink", PARTICLE_STATE_SQLITE_SINK_ID),
    ];
    assert_eq!(algorithms.len(), expected_algorithms.len());
    for (name, expected) in expected_algorithms {
        assert_eq!(algorithms.get(name).and_then(Value::as_str), Some(expected));
    }

    let number = |pointer: &str| value.pointer(pointer).and_then(Value::as_f64).unwrap();
    let expected_constants = [
        ("standard_gravity_m_s2", M4_CONSTANTS.standard_gravity_m_s2),
        ("earth_radius_m", M4_CONSTANTS.earth_radius_m),
        (
            "dry_air_gas_constant_j_kg_k",
            M4_CONSTANTS.dry_air_gas_constant_j_kg_k,
        ),
        (
            "water_vapour_gas_constant_j_kg_k",
            M4_CONSTANTS.water_vapour_gas_constant_j_kg_k,
        ),
        (
            "dry_air_heat_capacity_j_kg_k",
            M4_CONSTANTS.dry_air_heat_capacity_j_kg_k,
        ),
        (
            "earth_rotation_rate_rad_s",
            M4_CONSTANTS.earth_rotation_rate_rad_s,
        ),
        (
            "potential_temperature_reference_pressure_pa",
            M4_CONSTANTS.potential_temperature_reference_pressure_pa,
        ),
        (
            "surface_clearance_min_m",
            M4_CONSTANTS.surface_clearance_min_m,
        ),
        (
            "surface_clearance_ulps",
            f64::from(M4_CONSTANTS.surface_clearance_ulps),
        ),
        (
            "maximum_surface_reflections_per_step",
            f64::from(M4_CONSTANTS.maximum_surface_reflections_per_step),
        ),
        (
            "boundary_root_bisection_iterations",
            f64::from(M4_CONSTANTS.boundary_root_bisection_iterations),
        ),
        (
            "release_birth_time_random_dimension",
            f64::from(RELEASE_BIRTH_TIME_DIMENSION),
        ),
        (
            "release_horizontal_component_random_dimension",
            f64::from(RELEASE_HORIZONTAL_COMPONENT_DIMENSION),
        ),
        (
            "release_horizontal_u_random_dimension",
            f64::from(RELEASE_HORIZONTAL_U_DIMENSION),
        ),
        (
            "release_horizontal_v_random_dimension",
            f64::from(RELEASE_HORIZONTAL_V_DIMENSION),
        ),
        (
            "release_vertical_random_dimension",
            f64::from(RELEASE_VERTICAL_DIMENSION),
        ),
        (
            "ozone_minimum_height_asl_m_strict",
            OZONE_RULE_MINIMUM_HEIGHT_ASL_M,
        ),
        (
            "ozone_minimum_hemisphere_pv_pvu_strict",
            OZONE_RULE_MINIMUM_PV_PVU,
        ),
        ("ozone_ppbv_per_pvu", OZONE_RULE_PPBV_PER_PVU),
        ("ozone_molar_mass_g_mol", OZONE_MOLAR_MASS_G_MOL),
        ("dry_air_molar_mass_g_mol", DRY_AIR_MOLAR_MASS_G_MOL),
    ];
    assert_eq!(
        value
            .get("constants")
            .and_then(Value::as_object)
            .unwrap()
            .len(),
        expected_constants.len()
    );
    for (name, expected) in expected_constants {
        assert_eq!(number(&format!("/constants/{name}")), expected);
    }

    let expected_tolerances = [
        (
            "domain_fill_step_relative",
            MASS_BALANCE_STEP_RELATIVE_TOLERANCE,
        ),
        (
            "domain_fill_final_relative",
            MASS_BALANCE_FINAL_RELATIVE_TOLERANCE,
        ),
        ("domain_fill_ulp_floor", f64::from(MASS_BALANCE_ULP_FLOOR)),
        ("geometry_area_relative", GEOMETRY_AREA_RELATIVE_TOLERANCE),
        (
            "rk2_minimum_convergence_order",
            RK2_MINIMUM_CONVERGENCE_ORDER,
        ),
    ];
    assert_eq!(
        value
            .get("tolerances")
            .and_then(Value::as_object)
            .unwrap()
            .len(),
        expected_tolerances.len()
    );
    for (name, expected) in expected_tolerances {
        assert_eq!(number(&format!("/tolerances/{name}")), expected);
    }
}

#[test]
fn manifest_numerical_and_provenance_schemas_are_frozen_draft_2020_12() {
    for (name, id) in [
        (
            "M4_RUN_MANIFEST.schema.json",
            "https://trajecta.dev/schema/m4-run-manifest-v1.json",
        ),
        (
            "M4_NUMERICAL_CONTRACT.schema.json",
            "https://trajecta.dev/schema/m4-numerical-contract-v1.json",
        ),
        (
            "M4_PROVENANCE_BUNDLE.schema.json",
            "https://trajecta.dev/schema/m4-provenance-bundle-v1.json",
        ),
    ] {
        let value: Value =
            serde_json::from_slice(&fs::read(workspace().join("testdata").join(name)).unwrap())
                .unwrap();
        assert_eq!(
            value.get("$schema").and_then(Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );
        assert_eq!(value.get("$id").and_then(Value::as_str), Some(id));
        assert_eq!(
            value.get("additionalProperties").and_then(Value::as_bool),
            Some(false)
        );
        assert!(value.get("$defs").and_then(Value::as_object).is_some());
    }
    let manifest: Value = serde_json::from_slice(
        &fs::read(workspace().join("testdata/M4_RUN_MANIFEST.schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest
            .pointer("/properties/schema_version/const")
            .and_then(Value::as_str),
        Some(RUN_MANIFEST_SCHEMA_ID)
    );
    let provenance: Value = serde_json::from_slice(
        &fs::read(workspace().join("testdata/M4_PROVENANCE_BUNDLE.schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        provenance
            .pointer("/properties/schema_version/const")
            .and_then(Value::as_str),
        Some(PROVENANCE_BUNDLE_SCHEMA_ID)
    );
    assert_eq!(
        provenance
            .pointer("/properties/record_hash_algorithm/const")
            .and_then(Value::as_str),
        Some(PROVENANCE_RECORD_HASH_ALGORITHM)
    );
    assert_eq!(PROVENANCE_BUNDLE_FILE_NAME, "provenance-bundle.json");
}

#[test]
fn sqlite_schema_freezes_wal_transactions_and_public_tables() {
    let sql = fs::read_to_string(workspace().join("testdata/M4_SQLITE_SCHEMA.v1.sql")).unwrap();
    assert!(sql.contains("PRAGMA journal_mode = WAL"));
    assert!(sql.contains("PRAGMA synchronous = NORMAL"));
    assert!(sql.contains(&format!("PRAGMA user_version = {SQLITE_SCHEMA_VERSION}")));
    for table in [
        "run",
        "particle",
        "particle_mass",
        "output_event",
        "particle_state",
        "termination",
    ] {
        assert!(sql.contains(&format!("CREATE TABLE {table} ")));
    }
    assert!(sql.contains("particle_state_by_time"));
    assert!(sql.contains("PRIMARY KEY (run_id, particle_id, sample_sequence)"));
    assert!(!sql.to_ascii_lowercase().contains("json blob"));
}
