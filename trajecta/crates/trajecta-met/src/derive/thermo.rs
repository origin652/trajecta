//! # Contract: thermodynamic derivation
//!
//! Produces declared temperature, humidity, virtual temperature, and density
//! quantities with explicit phase and moisture conventions.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};
use crate::science::{M3_CONSTANTS, virtual_temperature_humidity_factor};

/// Projects a finite source specific humidity into the physical half-open range `[0, 1)`.
///
/// Negative source values are retained by raw query fields; this projection is only for
/// physical consumers such as density, hydrostatic geometry, surface similarity, and M4
/// domain-fill mass.
pub fn project_specific_humidity_nonnegative(
    specific_humidity: f64,
) -> Result<f64, ThermodynamicError> {
    if !specific_humidity.is_finite() || specific_humidity >= 1.0 {
        return Err(ThermodynamicError::InvalidSpecificHumidity);
    }
    Ok(specific_humidity.max(0.0))
}

/// Computes moist-air density after the frozen non-negative source-humidity projection.
pub fn moist_air_density_from_source_humidity_kg_m3(
    pressure_pa: f64,
    air_temperature_k: f64,
    source_specific_humidity: f64,
) -> Result<f64, ThermodynamicError> {
    moist_air_density_kg_m3(
        pressure_pa,
        air_temperature_k,
        project_specific_humidity_nonnegative(source_specific_humidity)?,
    )
}

/// Computes moist-air virtual temperature from temperature and specific humidity.
pub fn virtual_temperature_k(
    air_temperature_k: f64,
    specific_humidity: f64,
) -> Result<f64, ThermodynamicError> {
    if !air_temperature_k.is_finite() || air_temperature_k <= 0.0 {
        return Err(ThermodynamicError::InvalidTemperature);
    }
    if !specific_humidity.is_finite() || !(0.0..1.0).contains(&specific_humidity) {
        return Err(ThermodynamicError::InvalidSpecificHumidity);
    }
    let virtual_temperature =
        air_temperature_k * virtual_temperature_humidity_factor().mul_add(specific_humidity, 1.0);
    if !virtual_temperature.is_finite() || virtual_temperature <= 0.0 {
        return Err(ThermodynamicError::NumericalFailure);
    }
    Ok(virtual_temperature)
}

/// Computes moist-air density from pressure, temperature, and specific humidity.
pub fn moist_air_density_kg_m3(
    pressure_pa: f64,
    air_temperature_k: f64,
    specific_humidity: f64,
) -> Result<f64, ThermodynamicError> {
    if !pressure_pa.is_finite() || pressure_pa <= 0.0 {
        return Err(ThermodynamicError::InvalidPressure);
    }
    let virtual_temperature = virtual_temperature_k(air_temperature_k, specific_humidity)?;
    let density = pressure_pa / (M3_CONSTANTS.dry_air_gas_constant_j_kg_k * virtual_temperature);
    if !density.is_finite() || density <= 0.0 {
        return Err(ThermodynamicError::NumericalFailure);
    }
    Ok(density)
}

/// Thermodynamic scalar-formula failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThermodynamicError {
    /// Air pressure is non-finite, zero, or negative.
    InvalidPressure,
    /// Air temperature is non-finite, zero, or negative.
    InvalidTemperature,
    /// Specific humidity is outside the half-open interval [0, 1).
    InvalidSpecificHumidity,
    /// A finite positive result could not be produced.
    NumericalFailure,
}

/// Thermodynamic field deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThermodynamicDeriver;

impl FieldDeriver for ThermodynamicDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn dry_air_and_moist_air_follow_frozen_equations() {
        assert_eq!(virtual_temperature_k(300.0, 0.0), Ok(300.0));
        let moist = virtual_temperature_k(300.0, 0.01).unwrap();
        assert!(moist > 301.8 && moist < 301.9);
        let density = moist_air_density_kg_m3(100_000.0, 300.0, 0.01).unwrap();
        let expected = 100_000.0 / (M3_CONSTANTS.dry_air_gas_constant_j_kg_k * moist);
        assert!((density - expected).abs() < 1.0e-15);
    }

    #[test]
    fn invalid_thermodynamic_states_are_typed() {
        assert_eq!(
            moist_air_density_kg_m3(0.0, 300.0, 0.0),
            Err(ThermodynamicError::InvalidPressure)
        );
        assert_eq!(
            virtual_temperature_k(300.0, 1.0),
            Err(ThermodynamicError::InvalidSpecificHumidity)
        );
    }

    #[test]
    fn source_humidity_projection_preserves_raw_semantics_for_physical_consumers() {
        assert_eq!(project_specific_humidity_nonnegative(-1.0e-9), Ok(0.0));
        assert_eq!(project_specific_humidity_nonnegative(0.01), Ok(0.01));
        assert_eq!(
            moist_air_density_from_source_humidity_kg_m3(100_000.0, 300.0, -1.0e-9),
            moist_air_density_kg_m3(100_000.0, 300.0, 0.0)
        );
        assert_eq!(
            project_specific_humidity_nonnegative(1.0),
            Err(ThermodynamicError::InvalidSpecificHumidity)
        );
    }
}
