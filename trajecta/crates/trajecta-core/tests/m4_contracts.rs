//! Machine-readable M4-A0 contract checks.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use serde_json::Value;
use trajecta_case::model::output::{PARTICLE_STATE_PRODUCT_ID, PARTICLE_STATE_SQLITE_SINK_ID};
use trajecta_core::rng::{
    COUNTER_RNG_ALGORITHM_ID, DOMAIN_FILL_BOUNDARY_TANGENTIAL_DIMENSION,
    DOMAIN_FILL_BOUNDARY_VERTICAL_DIMENSION, DOMAIN_FILL_LATITUDE_DIMENSION,
    DOMAIN_FILL_LONGITUDE_DIMENSION, DOMAIN_FILL_MASS_STRATUM_DIMENSION,
    DOMAIN_FILL_PRESSURE_DIMENSION, RELEASE_BIRTH_TIME_DIMENSION,
    RELEASE_HORIZONTAL_COMPONENT_DIMENSION, RELEASE_HORIZONTAL_U_DIMENSION,
    RELEASE_HORIZONTAL_V_DIMENSION, RELEASE_VERTICAL_DIMENSION,
};
use trajecta_core::science::{
    COHORT_LOCAL_SCHEDULER_ID, DOMAIN_FILL_MASS_LEDGER_ID, DRY_AIR_BOUNDARY_BIRTH_ID,
    DRY_AIR_DOMAIN_FILL_ID, DRY_AIR_FLUX_TIME_ID, DRY_AIR_INITIAL_SAMPLING_ID,
    DRY_AIR_MOLAR_MASS_G_MOL, DRY_AIR_TRANSPORT_FLOOR_FOLLOWING_RELOCATION_ID,
    DRY_AIR_TRANSPORT_FLOOR_PRESSURE_ID, ERTEL_PV_SPHERICAL_ID, FLEXPART_PV60_OZONE_ID,
    GEOMETRY_AREA_RELATIVE_TOLERANCE, GLOBAL_PERIODIC_ID, GREAT_CIRCLE_PATH_BASIS_ID,
    LIMITED_DOMAIN_TERMINATE_ID, M4_CONSTANTS, M4_PARTICLE_LOOP_CONTRACT_ID,
    MASS_BALANCE_FINAL_RELATIVE_TOLERANCE, MASS_BALANCE_STEP_RELATIVE_TOLERANCE,
    MASS_BALANCE_ULP_FLOOR, MODEL_TOP_TERMINATE_ID, OZONE_DOMAIN_FILL_ID, OZONE_MOLAR_MASS_G_MOL,
    OZONE_RULE_MINIMUM_HEIGHT_ASL_M, OZONE_RULE_MINIMUM_PV_PVU, OZONE_RULE_PPBV_PER_PVU,
    PRESSURE_LOG_INTERFACE_ID, PROVENANCE_BUNDLE_FILE_NAME, PROVENANCE_BUNDLE_SCHEMA_ID,
    PROVENANCE_RECORD_HASH_ALGORITHM, RELEASE_BIRTH_STRATA_ID, RELEASE_DRIVEN_POPULATION_ID,
    RK2_DOMAIN_EXIT_BRIDGE_ID, RK2_MINIMUM_CONVERGENCE_ORDER, RK2_SPHERICAL_ID,
    RUN_MANIFEST_SCHEMA_ID, SPHERICAL_CELL_AREA_ID, SQLITE_SCHEMA_VERSION, SURFACE_REFLECT_ID,
};
use trajecta_met::derive::domain_fill::{
    AIR_MASS_GRID_ALGORITHM_ID, BOUNDARY_MASS_FLUX_ALGORITHM_ID,
};
use trajecta_met::science::{
    AERODYNAMIC_ROUGHNESS_ZERO_PROJECTION_ALGORITHM_ID,
    LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID,
    SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID,
    TEN_METRE_ANCHORED_SURFACE_WIND_ALGORITHM_ID, TWO_METRE_ANCHORED_SURFACE_SCALAR_ALGORITHM_ID,
    ZERO_AERODYNAMIC_ROUGHNESS_REPLACEMENT_M,
};
use trajecta_met::surface_layer::MoninObukhovBusingerDyer;

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
        ("cohort_scheduler", COHORT_LOCAL_SCHEDULER_ID),
        ("great_circle_path_basis", GREAT_CIRCLE_PATH_BASIS_ID),
        ("limited_domain_exit_bridge", RK2_DOMAIN_EXIT_BRIDGE_ID),
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
        ("air_mass_grid", AIR_MASS_GRID_ALGORITHM_ID),
        (
            "specific_humidity_nonnegative_projection",
            SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID,
        ),
        (
            "aerodynamic_roughness_zero_projection",
            AERODYNAMIC_ROUGHNESS_ZERO_PROJECTION_ALGORITHM_ID,
        ),
        ("surface_layer_model", MoninObukhovBusingerDyer::MODEL_ID),
        (
            "surface_layer_wind",
            TEN_METRE_ANCHORED_SURFACE_WIND_ALGORITHM_ID,
        ),
        (
            "surface_layer_scalar",
            TWO_METRE_ANCHORED_SURFACE_SCALAR_ALGORITHM_ID,
        ),
        (
            "lowest_complete_transport_anchor",
            LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID,
        ),
        ("air_mass_initial_sampling", DRY_AIR_INITIAL_SAMPLING_ID),
        (
            "air_mass_transport_floor_relocation",
            DRY_AIR_TRANSPORT_FLOOR_FOLLOWING_RELOCATION_ID,
        ),
        (
            "air_mass_transport_floor_pressure",
            DRY_AIR_TRANSPORT_FLOOR_PRESSURE_ID,
        ),
        ("boundary_mass_flux", BOUNDARY_MASS_FLUX_ALGORITHM_ID),
        ("boundary_flux_time", DRY_AIR_FLUX_TIME_ID),
        ("boundary_birth_time", DRY_AIR_BOUNDARY_BIRTH_ID),
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
            "zero_aerodynamic_roughness_replacement_m",
            ZERO_AERODYNAMIC_ROUGHNESS_REPLACEMENT_M,
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
            "domain_fill_mass_stratum_random_dimension",
            f64::from(DOMAIN_FILL_MASS_STRATUM_DIMENSION),
        ),
        (
            "domain_fill_longitude_random_dimension",
            f64::from(DOMAIN_FILL_LONGITUDE_DIMENSION),
        ),
        (
            "domain_fill_latitude_random_dimension",
            f64::from(DOMAIN_FILL_LATITUDE_DIMENSION),
        ),
        (
            "domain_fill_pressure_random_dimension",
            f64::from(DOMAIN_FILL_PRESSURE_DIMENSION),
        ),
        (
            "domain_fill_boundary_tangential_random_dimension",
            f64::from(DOMAIN_FILL_BOUNDARY_TANGENTIAL_DIMENSION),
        ),
        (
            "domain_fill_boundary_vertical_random_dimension",
            f64::from(DOMAIN_FILL_BOUNDARY_VERTICAL_DIMENSION),
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
    let required = manifest.get("required").and_then(Value::as_array).unwrap();
    for field in ["job_series_id", "attempt", "run_id"] {
        assert!(required.iter().any(|value| value.as_str() == Some(field)));
    }
    let statuses = manifest
        .pointer("/properties/status/enum")
        .and_then(Value::as_array)
        .unwrap();
    for status in ["cancelled", "interrupted"] {
        assert!(statuses.iter().any(|value| value.as_str() == Some(status)));
    }
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
    assert!(sql.contains("PRAGMA wal_autocheckpoint = 0"));
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
