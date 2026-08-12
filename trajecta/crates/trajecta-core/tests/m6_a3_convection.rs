//! M6-A3 deep-convection validation against frozen ARM TWP-ICE event C.

#![allow(clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use trajecta_core::physics::DeepConvectionKernel;
use trajecta_met::vertical::{ColumnGeometry, VerticalValidity};

#[derive(Deserialize)]
struct Asset {
    schema_version: String,
    validation_normalization: Normalization,
    columns: Vec<ValidationColumn>,
}

#[derive(Deserialize)]
struct Normalization {
    mass_flux_scale_kg_m2_s: f64,
    minimum_profile_observations: usize,
}

#[derive(Deserialize)]
struct ValidationColumn {
    time_utc: String,
    thermodynamic_column: ThermodynamicColumn,
    independent_reference: ReferenceColumn,
}

#[derive(Deserialize)]
struct ThermodynamicColumn {
    surface_pressure_pa: f64,
    terrain_height_asl_m: f64,
    pressure_pa: Vec<f64>,
    height_asl_m: Vec<f64>,
    temperature_k: Vec<f64>,
    specific_humidity_kg_kg: Vec<f64>,
    pressure_layer_mass_kg_m2: Vec<f64>,
    valid: Vec<bool>,
}

#[derive(Deserialize)]
struct ReferenceColumn {
    cloud_base_mass_flux_kg_m2_s: f64,
    upward_interface_mass_flux_kg_m2_s: Vec<f64>,
    downward_interface_mass_flux_kg_m2_s: Vec<f64>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn load_asset() -> Asset {
    let bytes =
        fs::read(workspace_root().join("testdata/m6-a3/M6_ARM_TWP_ICE_COLUMN.v1.json")).unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(&bytes)),
        "144ac0a0e0f4043df5fb5b1fe57ddebe57e2c65c9cc5df6b33c5bf727e86b259"
    );
    serde_json::from_slice(&bytes).unwrap()
}

fn geometry(input: &ThermodynamicColumn) -> ColumnGeometry {
    let count = input.pressure_pa.len();
    assert_eq!(input.height_asl_m.len(), count);
    assert_eq!(input.temperature_k.len(), count);
    assert_eq!(input.specific_humidity_kg_kg.len(), count);
    assert_eq!(input.pressure_layer_mass_kg_m2.len(), count);
    assert_eq!(input.valid.len(), count);
    ColumnGeometry::new(
        Arc::from(input.pressure_pa.clone()),
        Arc::from(input.height_asl_m.clone()),
        Arc::from(input.temperature_k.clone()),
        Arc::from(input.specific_humidity_kg_kg.clone()),
        Arc::from(vec![[0.25; 4]; count]),
        VerticalValidity {
            valid: Arc::from(input.valid.clone()),
        },
        input.terrain_height_asl_m,
        input.surface_pressure_pa,
        Some(input.height_asl_m[0] + 1_000.0),
    )
    .unwrap()
}

fn dot(left: &[f64], right: &[f64]) -> f64 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

#[test]
fn twpice_event_c_closes_and_matches_the_independent_reference() {
    let asset = load_asset();
    assert_eq!(asset.schema_version, "trajecta.m6.twp-ice-convection/v1");
    assert_eq!(asset.columns.len(), 7);
    let mut profile_differences = Vec::new();
    let mut cloud_base_differences = Vec::new();
    let mut observation_count = 0_usize;
    for (column_index, validation) in asset.columns.iter().enumerate() {
        let kernel =
            DeepConvectionKernel::diagnose(&geometry(&validation.thermodynamic_column), 300.0)
                .unwrap_or_else(|error| panic!("{}: {error}", validation.time_utc));
        assert!(!kernel.is_identity(), "{}", validation.time_utc);
        let diagnostics = kernel.diagnostics().unwrap();
        assert!(diagnostics.cloud_top_layer < diagnostics.cloud_base_layer);
        assert!(diagnostics.cloud_base_layer < diagnostics.origin_layer);
        assert_eq!(
            diagnostics.upward_interface_mass_flux_kg_m2_s.len(),
            validation.thermodynamic_column.pressure_pa.len() - 1
        );
        for (upward, downward) in diagnostics
            .upward_interface_mass_flux_kg_m2_s
            .iter()
            .zip(&diagnostics.downward_interface_mass_flux_kg_m2_s)
        {
            assert!(
                (upward - downward).abs() <= 1.0e-12,
                "{} interface mass flux does not close",
                validation.time_utc
            );
        }
        cloud_base_differences.push(
            diagnostics.cloud_base_mass_flux_kg_m2_s
                - validation
                    .independent_reference
                    .cloud_base_mass_flux_kg_m2_s,
        );
        for (actual, expected) in diagnostics
            .upward_interface_mass_flux_kg_m2_s
            .iter()
            .zip(
                &validation
                    .independent_reference
                    .upward_interface_mass_flux_kg_m2_s,
            )
            .chain(
                diagnostics.downward_interface_mass_flux_kg_m2_s.iter().zip(
                    &validation
                        .independent_reference
                        .downward_interface_mass_flux_kg_m2_s,
                ),
            )
        {
            profile_differences.push(actual - expected);
            observation_count += 1;
        }

        let count = kernel.layer_count();
        for source in 0..count {
            let column_sum = (0..count)
                .map(|destination| kernel.transition_probability(destination, source).unwrap())
                .sum::<f64>();
            assert!((column_sum - 1.0).abs() <= 1.0e-12);
        }
        let forward_state = (0..count)
            .map(|index| (index + 1) as f64 / count as f64)
            .collect::<Vec<_>>();
        let adjoint_state = (0..count)
            .map(|index| 1.0 + (column_index + index) as f64 / count as f64)
            .collect::<Vec<_>>();
        let forward = kernel.apply_forward(&forward_state).unwrap();
        let adjoint = kernel.apply_adjoint(&adjoint_state).unwrap();
        let left = dot(&forward, &adjoint_state);
        let right = dot(&forward_state, &adjoint);
        assert!((left - right).abs() <= 1.0e-12 + 1.0e-8 * left.abs().max(right.abs()));
    }
    assert!(observation_count >= asset.validation_normalization.minimum_profile_observations);
    let scale = asset.validation_normalization.mass_flux_scale_kg_m2_s;
    let normalized_bias =
        profile_differences.iter().sum::<f64>() / profile_differences.len() as f64 / scale;
    let normalized_rmse = (profile_differences
        .iter()
        .map(|difference| difference * difference)
        .sum::<f64>()
        / profile_differences.len() as f64)
        .sqrt()
        / scale;
    let cloud_base_rmse = (cloud_base_differences
        .iter()
        .map(|difference| difference * difference)
        .sum::<f64>()
        / cloud_base_differences.len() as f64)
        .sqrt()
        / scale;
    assert!(
        normalized_bias.abs() <= 0.20,
        "normalized bias={normalized_bias}"
    );
    assert!(normalized_rmse <= 0.30, "normalized RMSE={normalized_rmse}");
    assert!(
        cloud_base_rmse <= 1.0e-3,
        "cloud-base RMSE={cloud_base_rmse}"
    );
}
