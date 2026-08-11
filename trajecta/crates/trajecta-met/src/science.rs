//! # Contract: frozen meteorological algorithms and constants
//!
//! Scientific code imports constants from this module rather than repeating
//! literals. Changing an identifier or value is a scientific-contract change
//! that requires regenerating the M3 acceptance report and A-level review.

/// Stable identifier for the complete M3 meteorology-query science contract.
pub const M3_MET_QUERY_ALGORITHM_ID: &str = "trajecta/met_query/m3/v0";

/// Stable identifier for hybrid-pressure construction.
pub const HYBRID_PRESSURE_ALGORITHM_ID: &str = "trajecta/hybrid_pressure/v0";

/// Stable identifier for ECMWF hydrostatic full-level geopotential.
pub const HYDROSTATIC_GEOPOTENTIAL_ALGORITHM_ID: &str = "trajecta/ecmwf_hydrostatic_alpha/v0";

/// Stable identifier for geometric vertical velocity.
pub const GEOMETRIC_VERTICAL_VELOCITY_ALGORITHM_ID: &str = "trajecta/kinematic_geometric_w/v0";

/// Stable identifier for spherical regular-grid interpolation.
pub const REGULAR_LAT_LON_ALGORITHM_ID: &str = "trajecta/regular_lat_lon_sphere/v0";

/// Stable identifier for IFS water-surface 2 m specific humidity from dewpoint.
pub const TWO_METRE_SPECIFIC_HUMIDITY_FROM_DEWPOINT_ALGORITHM_ID: &str =
    "trajecta/ifs_q2m_from_dewpoint/v0";

/// Stable identifier for upward latent heat flux from downward moisture flux.
pub const LATENT_HEAT_FROM_MOISTURE_FLUX_ALGORITHM_ID: &str =
    "trajecta/latent_heat_from_moisture_flux/v0";

/// Stable identifier for upward heat flux from ECMWF downward-positive flux.
pub const UPWARD_HEAT_FLUX_FROM_DOWNWARD_ALGORITHM_ID: &str =
    "trajecta/upward_heat_flux_from_downward/v0";

/// Stable identifier for surface pressure from logarithmic surface pressure.
pub const SURFACE_PRESSURE_FROM_LOG_ALGORITHM_ID: &str = "trajecta/surface_pressure_from_log/v0";

/// Stable identifier for spherical pressure-coordinate Ertel potential vorticity.
pub const ERTEL_PV_SPHERICAL_ALGORITHM_ID: &str = "ertel_pv_spherical/v1";

/// Earth rotation rate used by the frozen Ertel-PV contract, in radians per second.
pub const EARTH_ROTATION_RATE_RAD_S: f64 = 7.292_115_0e-5;

/// Reference pressure used by potential temperature in the frozen Ertel-PV contract.
pub const POTENTIAL_TEMPERATURE_REFERENCE_PRESSURE_PA: f64 = 100_000.0;

/// Multiplicative conversion from one potential-vorticity unit to SI.
pub const PVU_SCALE_TO_SI: f64 = 1.0e-6;

/// Stable projection used when physical consumers receive finite negative source humidity.
pub const SPECIFIC_HUMIDITY_NONNEGATIVE_PROJECTION_ALGORITHM_ID: &str =
    "specific_humidity_nonnegative_projection/v1";

/// Stable projection used when physical surface-layer consumers receive exact zero roughness.
pub const AERODYNAMIC_ROUGHNESS_ZERO_PROJECTION_ALGORITHM_ID: &str =
    "aerodynamic_roughness_zero_projection/v1";

/// Stable 10 m-vector-anchored Businger-Dyer wind-profile identity.
pub const TEN_METRE_ANCHORED_SURFACE_WIND_ALGORITHM_ID: &str =
    "ten_metre_anchored_businger_dyer_wind/v1";

/// Stable 2 m-anchored Businger-Dyer temperature/moisture profile identity.
pub const TWO_METRE_ANCHORED_SURFACE_SCALAR_ALGORITHM_ID: &str =
    "two_metre_anchored_businger_dyer_scalar/v1";

/// Stable joint U/V/W lowest-complete transport-anchor selection identity.
pub const LOWEST_COMPLETE_TRANSPORT_ANCHOR_ALGORITHM_ID: &str =
    "lowest_complete_transport_anchor/v1";

/// Stable identity for surface-exchange scales diagnosed from near-surface fields.
pub const SURFACE_EXCHANGE_SCALES_ALGORITHM_ID: &str = "surface_exchange_scales/v1";

/// Positive replacement for exact zero source roughness in physical surface-layer consumers.
///
/// The frozen CFSR SFCR messages use a `1e-4 m` decimal quantum and encode open-water points as
/// exact zero. Raw query fields preserve that zero; only logarithmic surface-layer consumers use
/// this smallest positive source quantum.
pub const ZERO_AERODYNAMIC_ROUGHNESS_REPLACEMENT_M: f64 = 1.0e-4;

/// IFS CY41R2 water-surface saturation vapour pressure constants (Part IV eq. 7.4/7.5).
pub const IFS_WATER_SATURATION_T0_K: f64 = 273.16;
/// Saturation vapour pressure reference (Pa).
pub const IFS_WATER_SATURATION_A1_PA: f64 = 611.21;
/// Saturation vapour pressure slope coefficient a3 (water).
pub const IFS_WATER_SATURATION_A3: f64 = 17.502;
/// Saturation vapour pressure offset coefficient a4 (K, water).
pub const IFS_WATER_SATURATION_A4_K: f64 = 32.19;

/// Frozen physical constants used by M3 algorithms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScienceConstants {
    /// Conventional standard gravity in metres per second squared.
    pub standard_gravity_m_s2: f64,
    /// Spherical Earth radius in metres.
    pub earth_radius_m: f64,
    /// Reference pressure used to label the ECMWF hybrid eta coordinate.
    pub hybrid_reference_pressure_pa: f64,
    /// Dry-air specific gas constant in joules per kilogram kelvin.
    pub dry_air_gas_constant_j_kg_k: f64,
    /// Water-vapour specific gas constant in joules per kilogram kelvin.
    pub water_vapour_gas_constant_j_kg_k: f64,
    /// Dry-air heat capacity at constant pressure in joules per kilogram kelvin.
    pub dry_air_heat_capacity_j_kg_k: f64,
    /// Frozen latent heat of vaporization in joules per kilogram.
    pub latent_heat_vaporization_j_kg: f64,
    /// Von Karman constant used by the M3 surface-layer model.
    pub von_karman: f64,
}

/// Frozen M3 constants.
pub const M3_CONSTANTS: ScienceConstants = ScienceConstants {
    standard_gravity_m_s2: 9.806_65,
    earth_radius_m: 6_371_229.0,
    hybrid_reference_pressure_pa: 101_325.0,
    dry_air_gas_constant_j_kg_k: 287.06,
    water_vapour_gas_constant_j_kg_k: 461.53,
    dry_air_heat_capacity_j_kg_k: 1_004.709,
    latent_heat_vaporization_j_kg: 2_500_000.0,
    von_karman: 0.4,
};

/// Returns `R_v / R_d - 1`, the frozen specific-humidity virtual-temperature factor.
#[must_use]
pub fn virtual_temperature_humidity_factor() -> f64 {
    M3_CONSTANTS.water_vapour_gas_constant_j_kg_k / M3_CONSTANTS.dry_air_gas_constant_j_kg_k - 1.0
}

/// Frozen Rd/Rv for IFS specific-humidity conversion.
#[must_use]
pub fn dry_over_vapour_gas_constant_ratio() -> f64 {
    M3_CONSTANTS.dry_air_gas_constant_j_kg_k / M3_CONSTANTS.water_vapour_gas_constant_j_kg_k
}

/// IFS CY41R2 water-surface saturation vapour pressure from dewpoint temperature (K).
///
/// `e(Td) = a1 * exp(a3 * (Td - T0) / (Td - a4))` with water coefficients.
#[must_use]
pub fn ifs_water_saturation_vapour_pressure_pa(dewpoint_k: f64) -> Option<f64> {
    if !dewpoint_k.is_finite() {
        return None;
    }
    let denominator = dewpoint_k - IFS_WATER_SATURATION_A4_K;
    if denominator == 0.0 || !denominator.is_finite() {
        return None;
    }
    let exponent = IFS_WATER_SATURATION_A3 * (dewpoint_k - IFS_WATER_SATURATION_T0_K) / denominator;
    if !exponent.is_finite() {
        return None;
    }
    let e = IFS_WATER_SATURATION_A1_PA * exponent.exp();
    if e.is_finite() && e > 0.0 {
        Some(e)
    } else {
        None
    }
}

/// 2 m specific humidity from dewpoint and surface pressure (IFS water surface).
///
/// `q = epsilon * e / (sp - (1 - epsilon) * e)` with `epsilon = Rd/Rv`.
/// Requires finite inputs, `sp > e > 0`, and `0 <= q < 1`.
#[must_use]
pub fn two_metre_specific_humidity_from_dewpoint_si(
    dewpoint_k: f64,
    surface_pressure_pa: f64,
) -> Option<f64> {
    if !dewpoint_k.is_finite() || !surface_pressure_pa.is_finite() || surface_pressure_pa <= 0.0 {
        return None;
    }
    let e = ifs_water_saturation_vapour_pressure_pa(dewpoint_k)?;
    if !(surface_pressure_pa > e && e > 0.0) {
        return None;
    }
    let epsilon = dry_over_vapour_gas_constant_ratio();
    let denominator = surface_pressure_pa - (1.0 - epsilon) * e;
    if denominator <= 0.0 || !denominator.is_finite() {
        return None;
    }
    let q = epsilon * e / denominator;
    if q.is_finite() && (0.0..1.0).contains(&q) {
        Some(q)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_frozen_and_physical() {
        assert_eq!(M3_CONSTANTS.standard_gravity_m_s2, 9.806_65);
        assert_eq!(M3_CONSTANTS.earth_radius_m, 6_371_229.0);
        assert!(virtual_temperature_humidity_factor() > 0.6);
        assert!(virtual_temperature_humidity_factor() < 0.61);
    }

    #[test]
    fn q2m_from_dewpoint_accepts_physical_and_rejects_illegal() {
        let q = two_metre_specific_humidity_from_dewpoint_si(280.0, 100_000.0);
        assert!(q.is_some());
        assert!((0.0..1.0).contains(&q.unwrap_or(f64::NAN)));
        // Illegal: sp <= e (very high dewpoint relative to pressure).
        assert!(two_metre_specific_humidity_from_dewpoint_si(300.0, 500.0).is_none());
        // Low dewpoint still finite and small.
        let dry = two_metre_specific_humidity_from_dewpoint_si(230.0, 100_000.0);
        assert!(dry.is_some_and(|value| value < 1e-3));
        assert!(two_metre_specific_humidity_from_dewpoint_si(f64::NAN, 100_000.0).is_none());
    }
}
