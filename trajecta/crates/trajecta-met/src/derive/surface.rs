//! # Contract: surface-field derivation
//!
//! Produces named surface diagnostics and only applies fallbacks explicitly
//! permitted by the active dataset profile.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};
use crate::derive::thermo::virtual_temperature_k;
use crate::science::M3_CONSTANTS;

/// Local state used to derive Monin-Obukhov surface exchange scales.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceExchangeInput {
    /// Moist-air density near the surface in kilograms per cubic metre.
    pub air_density_kg_m3: f64,
    /// Near-surface air temperature in kelvin.
    pub air_temperature_k: f64,
    /// Near-surface specific humidity.
    pub specific_humidity: f64,
    /// Exact momentum input selected from the available dataset fields.
    pub momentum: SurfaceMomentumInput,
    /// Upward-positive sensible heat flux in watts per square metre.
    pub sensible_heat_flux_w_m2: f64,
    /// Upward-positive latent heat flux in watts per square metre.
    pub latent_heat_flux_w_m2: f64,
}

/// Mutually exclusive momentum inputs for deriving surface exchange scales.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceMomentumInput {
    /// Diagnose friction velocity from the two stress components.
    SurfaceStress {
        /// Eastward stress on the atmosphere in pascals.
        eastward_pa: f64,
        /// Northward stress on the atmosphere in pascals.
        northward_pa: f64,
    },
    /// Use a source friction velocity directly.
    FrictionVelocity {
        /// Friction velocity in metres per second.
        friction_velocity_m_s: f64,
    },
}

/// Deterministic surface scales consumed by the similarity model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceExchangeScales {
    /// Friction velocity in metres per second.
    pub friction_velocity_m_s: f64,
    /// Temperature scale in kelvin.
    pub temperature_scale_k: f64,
    /// Specific-humidity scale.
    pub humidity_scale: f64,
    /// Finite Monin-Obukhov length carrier for non-neutral states.
    pub monin_obukhov_length_m: f64,
    /// Exact branch marker for zero virtual buoyancy flux.
    pub neutral_stability: bool,
}

/// Derives `u*`, scalar scales, and Monin-Obukhov length from locked fluxes.
pub fn surface_exchange_scales(
    input: SurfaceExchangeInput,
) -> Result<SurfaceExchangeScales, SurfaceExchangeError> {
    let values = [
        input.air_density_kg_m3,
        input.air_temperature_k,
        input.specific_humidity,
        input.sensible_heat_flux_w_m2,
        input.latent_heat_flux_w_m2,
    ];
    if values.iter().any(|value| !value.is_finite())
        || input.air_density_kg_m3 <= 0.0
        || input.air_temperature_k <= 0.0
        || !(0.0..1.0).contains(&input.specific_humidity)
    {
        return Err(SurfaceExchangeError::InvalidPhysicalState);
    }
    let friction_velocity_m_s = match input.momentum {
        SurfaceMomentumInput::SurfaceStress {
            eastward_pa,
            northward_pa,
        } => {
            if !eastward_pa.is_finite() || !northward_pa.is_finite() {
                return Err(SurfaceExchangeError::InvalidPhysicalState);
            }
            (eastward_pa.hypot(northward_pa) / input.air_density_kg_m3).sqrt()
        }
        SurfaceMomentumInput::FrictionVelocity {
            friction_velocity_m_s,
        } => friction_velocity_m_s,
    };
    if friction_velocity_m_s < 0.0 {
        return Err(SurfaceExchangeError::InvalidPhysicalState);
    }
    if !friction_velocity_m_s.is_finite() {
        return Err(SurfaceExchangeError::NumericalFailure);
    }
    if friction_velocity_m_s == 0.0 {
        if input.sensible_heat_flux_w_m2 == 0.0 && input.latent_heat_flux_w_m2 == 0.0 {
            return Ok(SurfaceExchangeScales {
                friction_velocity_m_s: 0.0,
                temperature_scale_k: 0.0,
                humidity_scale: 0.0,
                monin_obukhov_length_m: 1.0,
                neutral_stability: true,
            });
        }
        return Err(SurfaceExchangeError::CalmFluxInconsistency);
    }
    let temperature_scale_k = -input.sensible_heat_flux_w_m2
        / (input.air_density_kg_m3
            * M3_CONSTANTS.dry_air_heat_capacity_j_kg_k
            * friction_velocity_m_s);
    let humidity_scale = -input.latent_heat_flux_w_m2
        / (input.air_density_kg_m3
            * M3_CONSTANTS.latent_heat_vaporization_j_kg
            * friction_velocity_m_s);
    let virtual_temperature =
        virtual_temperature_k(input.air_temperature_k, input.specific_humidity)
            .map_err(|_| SurfaceExchangeError::InvalidPhysicalState)?;
    let humidity_factor = M3_CONSTANTS.water_vapour_gas_constant_j_kg_k
        / M3_CONSTANTS.dry_air_gas_constant_j_kg_k
        - 1.0;
    let virtual_temperature_scale = temperature_scale_k
        * (1.0 + humidity_factor * input.specific_humidity)
        + humidity_factor * input.air_temperature_k * humidity_scale;
    let neutral_stability = virtual_temperature_scale == 0.0;
    let monin_obukhov_length_m = if neutral_stability {
        1.0
    } else {
        friction_velocity_m_s.powi(2) * virtual_temperature
            / (M3_CONSTANTS.von_karman
                * M3_CONSTANTS.standard_gravity_m_s2
                * virtual_temperature_scale)
    };
    let outputs = [temperature_scale_k, humidity_scale, monin_obukhov_length_m];
    if outputs.iter().any(|value| !value.is_finite()) {
        return Err(SurfaceExchangeError::NumericalFailure);
    }
    Ok(SurfaceExchangeScales {
        friction_velocity_m_s,
        temperature_scale_k,
        humidity_scale,
        monin_obukhov_length_m,
        neutral_stability,
    })
}

/// Surface exchange derivation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceExchangeError {
    /// A thermodynamic or flux input is non-finite or outside its physical domain.
    InvalidPhysicalState,
    /// Non-zero heat flux cannot coexist with zero friction velocity in this model.
    CalmFluxInconsistency,
    /// Deterministic arithmetic did not remain finite.
    NumericalFailure,
}

/// Surface diagnostic and permitted-fallback deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceFieldDeriver;

impl FieldDeriver for SurfaceFieldDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn upward_heating_is_unstable_and_freezes_flux_signs() {
        let scales = surface_exchange_scales(SurfaceExchangeInput {
            air_density_kg_m3: 1.2,
            air_temperature_k: 290.0,
            specific_humidity: 0.005,
            momentum: SurfaceMomentumInput::SurfaceStress {
                eastward_pa: 0.12,
                northward_pa: 0.0,
            },
            sensible_heat_flux_w_m2: 100.0,
            latent_heat_flux_w_m2: 50.0,
        })
        .unwrap();
        assert!(scales.friction_velocity_m_s > 0.0);
        assert!(scales.temperature_scale_k < 0.0);
        assert!(scales.humidity_scale < 0.0);
        assert!(scales.monin_obukhov_length_m < 0.0);
        assert!(!scales.neutral_stability);
    }

    #[test]
    fn exact_calm_zero_flux_uses_neutral_branch_without_infinity() {
        let scales = surface_exchange_scales(SurfaceExchangeInput {
            air_density_kg_m3: 1.2,
            air_temperature_k: 290.0,
            specific_humidity: 0.005,
            momentum: SurfaceMomentumInput::SurfaceStress {
                eastward_pa: 0.0,
                northward_pa: 0.0,
            },
            sensible_heat_flux_w_m2: 0.0,
            latent_heat_flux_w_m2: 0.0,
        })
        .unwrap();
        assert!(scales.neutral_stability);
        assert_eq!(scales.monin_obukhov_length_m, 1.0);
    }

    #[test]
    fn source_friction_velocity_recomputes_every_dependent_scale() {
        let input = SurfaceExchangeInput {
            air_density_kg_m3: 1.2,
            air_temperature_k: 290.0,
            specific_humidity: 0.005,
            momentum: SurfaceMomentumInput::SurfaceStress {
                eastward_pa: 0.12,
                northward_pa: 0.0,
            },
            sensible_heat_flux_w_m2: 100.0,
            latent_heat_flux_w_m2: 50.0,
        };
        let diagnosed = surface_exchange_scales(input).unwrap();
        let override_velocity = 2.0 * diagnosed.friction_velocity_m_s;
        let overridden = surface_exchange_scales(SurfaceExchangeInput {
            momentum: SurfaceMomentumInput::FrictionVelocity {
                friction_velocity_m_s: override_velocity,
            },
            ..input
        })
        .unwrap();
        assert_eq!(overridden.friction_velocity_m_s, override_velocity);
        assert!(
            (overridden.temperature_scale_k / diagnosed.temperature_scale_k - 0.5).abs() < 1.0e-14
        );
        assert!((overridden.humidity_scale / diagnosed.humidity_scale - 0.5).abs() < 1.0e-14);
        assert!(
            (overridden.monin_obukhov_length_m / diagnosed.monin_obukhov_length_m - 8.0).abs()
                < 1.0e-12
        );
    }
}
