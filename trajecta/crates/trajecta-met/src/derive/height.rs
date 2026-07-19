//! # Contract: hydrostatic height derivation
//!
//! Integrates geometric or geopotential height with declared gravity and
//! virtual-temperature conventions, validating column monotonicity.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};
use crate::derive::thermo::{ThermodynamicError, virtual_temperature_k};
use crate::science::M3_CONSTANTS;

/// Hydrostatic geopotential at full levels and bounding interfaces.
#[derive(Clone, Debug, PartialEq)]
pub struct HydrostaticGeopotentialColumn {
    /// Full-level geopotential in square metres per square second.
    pub full_level_m2_s2: Vec<f64>,
    /// Interface geopotential in the same top-to-surface order as pressure.
    pub interface_m2_s2: Vec<f64>,
}

/// Applies the ECMWF hydrostatic/alpha recurrence from the lower interface upward.
pub fn hydrostatic_full_level_geopotential(
    interface_pressure_pa: &[f64],
    air_temperature_k: &[f64],
    specific_humidity: &[f64],
    lower_interface_geopotential_m2_s2: f64,
) -> Result<HydrostaticGeopotentialColumn, HeightDerivationError> {
    let levels = air_temperature_k.len();
    if levels == 0 || specific_humidity.len() != levels || interface_pressure_pa.len() != levels + 1
    {
        return Err(HeightDerivationError::LengthMismatch);
    }
    if !lower_interface_geopotential_m2_s2.is_finite() {
        return Err(HeightDerivationError::InvalidGeopotential);
    }
    if interface_pressure_pa
        .iter()
        .any(|pressure| !pressure.is_finite() || *pressure < 0.0)
        || interface_pressure_pa
            .windows(2)
            .any(|pressures| pressures[1] <= pressures[0])
    {
        return Err(HeightDerivationError::InvalidPressureColumn);
    }

    let mut full = vec![0.0; levels];
    let mut interfaces = vec![0.0; levels + 1];
    interfaces[levels] = lower_interface_geopotential_m2_s2;
    let mut lower_geopotential = lower_interface_geopotential_m2_s2;

    for level in (0..levels).rev() {
        let upper_pressure = interface_pressure_pa[level];
        let lower_pressure = interface_pressure_pa[level + 1];
        let (log_pressure_ratio, alpha) = if upper_pressure == 0.0 {
            ((lower_pressure / 0.1).ln(), std::f64::consts::LN_2)
        } else {
            let ratio_log = (lower_pressure / upper_pressure).ln();
            let alpha = 1.0 - upper_pressure / (lower_pressure - upper_pressure) * ratio_log;
            (ratio_log, alpha)
        };
        if !log_pressure_ratio.is_finite() || !alpha.is_finite() || alpha <= 0.0 {
            return Err(HeightDerivationError::NumericalFailure);
        }
        let virtual_temperature =
            virtual_temperature_k(air_temperature_k[level], specific_humidity[level])
                .map_err(HeightDerivationError::Thermodynamic)?;
        let gas_temperature = M3_CONSTANTS.dry_air_gas_constant_j_kg_k * virtual_temperature;
        full[level] = gas_temperature.mul_add(alpha, lower_geopotential);
        lower_geopotential = gas_temperature.mul_add(log_pressure_ratio, lower_geopotential);
        if !full[level].is_finite() || !lower_geopotential.is_finite() {
            return Err(HeightDerivationError::NumericalFailure);
        }
        interfaces[level] = lower_geopotential;
    }

    if full.windows(2).any(|values| values[1] >= values[0]) {
        return Err(HeightDerivationError::NonMonotonicGeopotential);
    }
    Ok(HydrostaticGeopotentialColumn {
        full_level_m2_s2: full,
        interface_m2_s2: interfaces,
    })
}

/// Converts geopotential to geometric height above mean sea level.
pub fn geopotential_to_geometric_height_m(
    geopotential_m2_s2: f64,
) -> Result<f64, HeightDerivationError> {
    if !geopotential_m2_s2.is_finite() {
        return Err(HeightDerivationError::InvalidGeopotential);
    }
    let gravity_radius = M3_CONSTANTS.standard_gravity_m_s2 * M3_CONSTANTS.earth_radius_m;
    let denominator = gravity_radius - geopotential_m2_s2;
    if denominator <= 0.0 {
        return Err(HeightDerivationError::GeometricHeightSingularity);
    }
    let height = M3_CONSTANTS.earth_radius_m * geopotential_m2_s2 / denominator;
    if !height.is_finite() {
        return Err(HeightDerivationError::NumericalFailure);
    }
    Ok(height)
}

/// Converts geometric height above mean sea level to geopotential.
pub fn geometric_height_to_geopotential_m2_s2(
    geometric_height_m: f64,
) -> Result<f64, HeightDerivationError> {
    if !geometric_height_m.is_finite() || geometric_height_m <= -M3_CONSTANTS.earth_radius_m {
        return Err(HeightDerivationError::InvalidGeometricHeight);
    }
    let geopotential =
        M3_CONSTANTS.standard_gravity_m_s2 * M3_CONSTANTS.earth_radius_m * geometric_height_m
            / (M3_CONSTANTS.earth_radius_m + geometric_height_m);
    if !geopotential.is_finite() {
        return Err(HeightDerivationError::NumericalFailure);
    }
    Ok(geopotential)
}

/// Converts geopotential to geopotential height using conventional gravity.
pub fn geopotential_height_m(geopotential_m2_s2: f64) -> Result<f64, HeightDerivationError> {
    if !geopotential_m2_s2.is_finite() {
        return Err(HeightDerivationError::InvalidGeopotential);
    }
    Ok(geopotential_m2_s2 / M3_CONSTANTS.standard_gravity_m_s2)
}

/// Hydrostatic or geometric-height derivation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeightDerivationError {
    /// Pressure, temperature, and humidity lengths are inconsistent or empty.
    LengthMismatch,
    /// Interface pressure is non-finite, negative, or non-monotonic.
    InvalidPressureColumn,
    /// Geopotential input is NaN or infinite.
    InvalidGeopotential,
    /// Geometric height is non-finite or lies at/below the Earth-centre singularity.
    InvalidGeometricHeight,
    /// Geopotential reaches the geometric-height transformation singularity.
    GeometricHeightSingularity,
    /// A thermodynamic input is invalid.
    Thermodynamic(ThermodynamicError),
    /// Full-level geopotential does not decrease from model top toward surface.
    NonMonotonicGeopotential,
    /// Arithmetic did not produce a finite physical result.
    NumericalFailure,
}

/// Hydrostatic height and geopotential deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct HydrostaticHeightDeriver;

impl FieldDeriver for HydrostaticHeightDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn one_level_top_uses_ecmwf_ln2_alpha_convention() {
        let column =
            hydrostatic_full_level_geopotential(&[0.0, 100_000.0], &[300.0], &[0.0], 0.0).unwrap();
        let expected = M3_CONSTANTS.dry_air_gas_constant_j_kg_k * 300.0 * std::f64::consts::LN_2;
        assert!((column.full_level_m2_s2[0] - expected).abs() < 1.0e-10);
        assert!(column.interface_m2_s2[0] > column.full_level_m2_s2[0]);
    }

    #[test]
    fn geometric_and_geopotential_height_are_not_conflated() {
        let geopotential = geometric_height_to_geopotential_m2_s2(10_000.0).unwrap();
        let geometric = geopotential_to_geometric_height_m(geopotential).unwrap();
        let geopotential_height = geopotential_height_m(geopotential).unwrap();
        assert!((geometric - 10_000.0).abs() < 1.0e-9);
        assert!(geopotential_height < geometric);
    }
}
