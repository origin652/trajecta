//! # Contract: pressure derivation
//!
//! Builds hybrid interface and full-level pressure from complete A/B
//! coefficients and local surface pressure without truncating PV metadata.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// One local hybrid-pressure column ordered from model top to surface.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridPressureColumn {
    /// Interface pressures in pascals; length is full-level count plus one.
    pub interface_pa: Vec<f64>,
    /// Arithmetic-mean full-level pressures in pascals.
    pub full_pa: Vec<f64>,
}

/// Constructs interface and full-level pressure from complete A/B coefficients.
pub fn hybrid_pressure_column(
    a_half_pa: &[f64],
    b_half: &[f64],
    surface_pressure_pa: f64,
) -> Result<HybridPressureColumn, PressureDerivationError> {
    if a_half_pa.len() != b_half.len() || a_half_pa.len() < 2 {
        return Err(PressureDerivationError::InvalidCoefficientLength);
    }
    if !surface_pressure_pa.is_finite() || surface_pressure_pa <= 0.0 {
        return Err(PressureDerivationError::InvalidSurfacePressure);
    }
    let mut interface_pa = Vec::with_capacity(a_half_pa.len());
    for (a, b) in a_half_pa.iter().copied().zip(b_half.iter().copied()) {
        if !a.is_finite() || !b.is_finite() || a < 0.0 || b < 0.0 {
            return Err(PressureDerivationError::InvalidCoefficient);
        }
        let pressure = b.mul_add(surface_pressure_pa, a);
        if !pressure.is_finite() || pressure < 0.0 {
            return Err(PressureDerivationError::InvalidInterfacePressure);
        }
        interface_pa.push(pressure);
    }
    if interface_pa
        .windows(2)
        .any(|pressures| pressures[1] <= pressures[0])
    {
        return Err(PressureDerivationError::NonMonotonicInterfacePressure);
    }
    let full_pa = interface_pa
        .windows(2)
        .map(|pressures| 0.5 * (pressures[0] + pressures[1]))
        .collect::<Vec<_>>();
    if full_pa.iter().any(|pressure| *pressure <= 0.0) {
        return Err(PressureDerivationError::InvalidFullLevelPressure);
    }
    Ok(HybridPressureColumn {
        interface_pa,
        full_pa,
    })
}

/// Hybrid-pressure construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressureDerivationError {
    /// A/B arrays differ in length or contain fewer than two interfaces.
    InvalidCoefficientLength,
    /// Surface pressure is non-finite, zero, or negative.
    InvalidSurfacePressure,
    /// An A/B coefficient is non-finite or negative.
    InvalidCoefficient,
    /// A computed interface pressure is non-finite or negative.
    InvalidInterfacePressure,
    /// Interface pressure does not increase strictly from model top to surface.
    NonMonotonicInterfacePressure,
    /// A computed full-level pressure is zero or negative.
    InvalidFullLevelPressure,
}

/// Hybrid and fixed-level pressure deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct PressureDeriver;

impl FieldDeriver for PressureDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn hybrid_pressure_uses_complete_half_level_coefficients() {
        let column =
            hybrid_pressure_column(&[0.0, 1_000.0, 0.0], &[0.0, 0.4, 1.0], 100_000.0).unwrap();
        assert_eq!(column.interface_pa, vec![0.0, 41_000.0, 100_000.0]);
        assert_eq!(column.full_pa, vec![20_500.0, 70_500.0]);
    }

    #[test]
    fn hybrid_pressure_rejects_non_monotonic_columns() {
        assert_eq!(
            hybrid_pressure_column(&[0.0, 0.0, 0.0], &[0.0, 1.0, 0.5], 100_000.0),
            Err(PressureDerivationError::NonMonotonicInterfacePressure)
        );
    }
}
