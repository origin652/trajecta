//! # Contract: independent M4 scalar reference formulas
//!
//! Scalar formulas used by M4 acceptance fixtures.
//!
//! These routines favor explicit operation order and validation over runtime
//! throughput. Production vectorized implementations must agree with these
//! formulas on their declared domains.

use std::f64::consts::PI;

use crate::science::{
    DRY_AIR_MOLAR_MASS_G_MOL, M4_CONSTANTS, MASS_BALANCE_FINAL_RELATIVE_TOLERANCE,
    MASS_BALANCE_STEP_RELATIVE_TOLERANCE, MASS_BALANCE_ULP_FLOOR, OZONE_MOLAR_MASS_G_MOL,
    OZONE_RULE_MINIMUM_HEIGHT_ASL_M, OZONE_RULE_MINIMUM_PV_PVU, OZONE_RULE_PPBV_PER_PVU,
};

/// One geometric particle position used by scalar references.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SphericalPosition {
    /// Longitude in degrees east.
    pub longitude_degrees: f64,
    /// Latitude in degrees north.
    pub latitude_degrees: f64,
    /// Geometric height above mean sea level in metres.
    pub height_asl_m: f64,
}

/// Local east, north, and geometric-up velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalWind {
    /// Eastward velocity in metres per second.
    pub eastward_m_s: f64,
    /// Northward velocity in metres per second.
    pub northward_m_s: f64,
    /// Geometric vertical velocity in metres per second.
    pub upward_m_s: f64,
}

/// Midpoint query position and final state of one frozen RK2 step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SphericalRk2ReferenceStep {
    /// Position and height at the exact half-step query instant.
    pub midpoint: SphericalPosition,
    /// Final position advanced from the start with midpoint wind.
    pub end: SphericalPosition,
}

/// Executes the frozen embedding-space spherical midpoint formula.
pub fn spherical_rk2_step(
    start: SphericalPosition,
    start_wind: LocalWind,
    midpoint_wind: LocalWind,
    signed_dt_seconds: f64,
) -> Result<SphericalRk2ReferenceStep, ReferenceError> {
    validate_position(start)?;
    validate_wind(start_wind)?;
    validate_wind(midpoint_wind)?;
    if !signed_dt_seconds.is_finite() {
        return Err(ReferenceError::NonFiniteInput);
    }
    let start_vector = unit_vector(start)?;
    let start_tangent = local_tangent(start, start_wind)?;
    let midpoint_vector = normalized(add_scaled(
        start_vector,
        start_tangent,
        signed_dt_seconds / (2.0 * M4_CONSTANTS.earth_radius_m),
    ))?;
    let midpoint = position_from_vector(
        midpoint_vector,
        start.height_asl_m + 0.5 * signed_dt_seconds * start_wind.upward_m_s,
    )?;
    let midpoint_tangent = local_tangent(midpoint, midpoint_wind)?;
    let end_vector = normalized(add_scaled(
        start_vector,
        midpoint_tangent,
        signed_dt_seconds / M4_CONSTANTS.earth_radius_m,
    ))?;
    let end = position_from_vector(
        end_vector,
        start.height_asl_m + signed_dt_seconds * midpoint_wind.upward_m_s,
    )?;
    Ok(SphericalRk2ReferenceStep { midpoint, end })
}

/// Exact spherical area of an unwrapped latitude/longitude cell.
///
/// `east - west` must be in `(0, 360]`; antimeridian cells are represented by
/// unwrapping the eastern longitude above the western longitude.
pub fn spherical_lat_lon_cell_area_m2(
    west_degrees: f64,
    east_degrees: f64,
    south_degrees: f64,
    north_degrees: f64,
) -> Result<f64, ReferenceError> {
    if [west_degrees, east_degrees, south_degrees, north_degrees]
        .iter()
        .any(|value| !value.is_finite())
        || !(-90.0..=90.0).contains(&south_degrees)
        || !(-90.0..=90.0).contains(&north_degrees)
        || north_degrees <= south_degrees
    {
        return Err(ReferenceError::InvalidGeometry);
    }
    let longitude_span = east_degrees - west_degrees;
    if !(0.0..=360.0).contains(&longitude_span) || longitude_span == 0.0 {
        return Err(ReferenceError::InvalidGeometry);
    }
    let delta_lambda = longitude_span.to_radians();
    let sine_span = north_degrees.to_radians().sin() - south_degrees.to_radians().sin();
    let area = M4_CONSTANTS.earth_radius_m.powi(2) * delta_lambda * sine_span;
    if area.is_finite() && area > 0.0 {
        Ok(area)
    } else {
        Err(ReferenceError::InvalidGeometry)
    }
}

/// Builds pressure interfaces from strictly increasing top-to-bottom centres.
///
/// Interior interfaces are geometric means. Top and bottom interfaces use the
/// same half log-pressure spacing extrapolated from the adjacent centre pair.
pub fn pressure_log_midpoint_interfaces_pa(
    centre_pressure_pa: &[f64],
) -> Result<Vec<f64>, ReferenceError> {
    if centre_pressure_pa.len() < 2
        || centre_pressure_pa
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        || centre_pressure_pa.windows(2).any(|pair| pair[1] <= pair[0])
    {
        return Err(ReferenceError::InvalidPressureColumn);
    }
    let mut interfaces = Vec::with_capacity(centre_pressure_pa.len() + 1);
    let top_ratio = (centre_pressure_pa[1] / centre_pressure_pa[0]).sqrt();
    interfaces.push(centre_pressure_pa[0] / top_ratio);
    interfaces.extend(
        centre_pressure_pa
            .windows(2)
            .map(|pair| (pair[0] * pair[1]).sqrt()),
    );
    let last = centre_pressure_pa.len() - 1;
    let bottom_ratio = (centre_pressure_pa[last] / centre_pressure_pa[last - 1]).sqrt();
    interfaces.push(centre_pressure_pa[last] * bottom_ratio);
    if interfaces
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
        || interfaces.windows(2).any(|pair| pair[1] <= pair[0])
    {
        return Err(ReferenceError::InvalidPressureColumn);
    }
    Ok(interfaces)
}

/// Dry-air column mass for paired positive layer pressure thickness and q.
pub fn dry_air_column_mass_kg(
    cell_area_m2: f64,
    layer_pressure_thickness_pa: &[f64],
    specific_humidity: &[f64],
) -> Result<f64, ReferenceError> {
    if !cell_area_m2.is_finite()
        || cell_area_m2 <= 0.0
        || layer_pressure_thickness_pa.len() != specific_humidity.len()
        || layer_pressure_thickness_pa.is_empty()
    {
        return Err(ReferenceError::InvalidMassInput);
    }
    let mut terms = Vec::with_capacity(layer_pressure_thickness_pa.len());
    for (&delta_p, &q) in layer_pressure_thickness_pa
        .iter()
        .zip(specific_humidity.iter())
    {
        if !delta_p.is_finite() || delta_p <= 0.0 || !q.is_finite() || !(0.0..1.0).contains(&q) {
            return Err(ReferenceError::InvalidMassInput);
        }
        terms.push((1.0 - q) * delta_p);
    }
    let pressure_integral = neumaier_sum(&terms)?;
    let mass = cell_area_m2 / M4_CONSTANTS.standard_gravity_m_s2 * pressure_integral;
    if mass.is_finite() && mass >= 0.0 {
        Ok(mass)
    } else {
        Err(ReferenceError::InvalidMassInput)
    }
}

/// Dry-air partial density for pressure, temperature, and specific humidity.
pub fn dry_air_density_kg_m3(
    pressure_pa: f64,
    temperature_k: f64,
    specific_humidity: f64,
) -> Result<f64, ReferenceError> {
    if !pressure_pa.is_finite()
        || pressure_pa <= 0.0
        || !temperature_k.is_finite()
        || temperature_k <= 0.0
        || !specific_humidity.is_finite()
        || !(0.0..1.0).contains(&specific_humidity)
    {
        return Err(ReferenceError::InvalidMassInput);
    }
    let moist_gas_constant = (1.0 - specific_humidity) * M4_CONSTANTS.dry_air_gas_constant_j_kg_k
        + specific_humidity * M4_CONSTANTS.water_vapour_gas_constant_j_kg_k;
    let total_density = pressure_pa / (moist_gas_constant * temperature_k);
    let dry_density = (1.0 - specific_humidity) * total_density;
    if dry_density.is_finite() && dry_density >= 0.0 {
        Ok(dry_density)
    } else {
        Err(ReferenceError::InvalidMassInput)
    }
}

/// Applies the frozen empirical PV60 ozone rule.
///
/// Returns `Ok(None)` outside the strict rule mask and ozone mass in kilograms
/// inside it. Southern-hemisphere PV is sign-normalized before thresholding.
pub fn flexpart_pv60_ozone_mass_kg(
    carrier_dry_air_mass_kg: f64,
    height_asl_m: f64,
    latitude_degrees: f64,
    potential_vorticity_pvu: f64,
) -> Result<Option<f64>, ReferenceError> {
    if !carrier_dry_air_mass_kg.is_finite()
        || carrier_dry_air_mass_kg < 0.0
        || !height_asl_m.is_finite()
        || !latitude_degrees.is_finite()
        || !(-90.0..=90.0).contains(&latitude_degrees)
        || !potential_vorticity_pvu.is_finite()
    {
        return Err(ReferenceError::InvalidOzoneInput);
    }
    let hemisphere_pv = if latitude_degrees < 0.0 {
        -potential_vorticity_pvu
    } else {
        potential_vorticity_pvu
    };
    if height_asl_m <= OZONE_RULE_MINIMUM_HEIGHT_ASL_M || hemisphere_pv <= OZONE_RULE_MINIMUM_PV_PVU
    {
        return Ok(None);
    }
    let mole_fraction = hemisphere_pv * OZONE_RULE_PPBV_PER_PVU * 1.0e-9;
    let ozone_mass = carrier_dry_air_mass_kg
        * mole_fraction
        * (OZONE_MOLAR_MASS_G_MOL / DRY_AIR_MOLAR_MASS_G_MOL);
    if ozone_mass.is_finite() && ozone_mass >= 0.0 {
        Ok(Some(ozone_mass))
    } else {
        Err(ReferenceError::InvalidOzoneInput)
    }
}

/// Absolute mass-ledger tolerance for a declared positive scale.
pub fn mass_balance_tolerance_kg(
    scale_kg: f64,
    final_accumulated: bool,
) -> Result<f64, ReferenceError> {
    if !scale_kg.is_finite() || scale_kg < 0.0 {
        return Err(ReferenceError::InvalidMassInput);
    }
    let relative = if final_accumulated {
        MASS_BALANCE_FINAL_RELATIVE_TOLERANCE
    } else {
        MASS_BALANCE_STEP_RELATIVE_TOLERANCE
    } * scale_kg;
    let ulp_floor = f64::from(MASS_BALANCE_ULP_FLOOR) * positive_ulp(scale_kg);
    let tolerance = relative.max(ulp_floor);
    if tolerance.is_finite() && tolerance > 0.0 {
        Ok(tolerance)
    } else {
        Err(ReferenceError::InvalidMassInput)
    }
}

/// Fixed-order Neumaier compensated sum.
pub fn neumaier_sum(values: &[f64]) -> Result<f64, ReferenceError> {
    let mut sum = 0.0_f64;
    let mut correction = 0.0_f64;
    for &value in values {
        if !value.is_finite() {
            return Err(ReferenceError::NonFiniteInput);
        }
        let next = sum + value;
        if sum.abs() >= value.abs() {
            correction += (sum - next) + value;
        } else {
            correction += (value - next) + sum;
        }
        sum = next;
    }
    let result = sum + correction;
    if result.is_finite() {
        Ok(result)
    } else {
        Err(ReferenceError::NonFiniteInput)
    }
}

fn validate_position(position: SphericalPosition) -> Result<(), ReferenceError> {
    if !position.longitude_degrees.is_finite()
        || !position.latitude_degrees.is_finite()
        || !position.height_asl_m.is_finite()
        || !(-90.0..=90.0).contains(&position.latitude_degrees)
    {
        return Err(ReferenceError::NonFiniteInput);
    }
    if position.latitude_degrees.abs() == 90.0 {
        return Err(ReferenceError::PolarSingularity);
    }
    Ok(())
}

fn validate_wind(wind: LocalWind) -> Result<(), ReferenceError> {
    if [wind.eastward_m_s, wind.northward_m_s, wind.upward_m_s]
        .iter()
        .all(|value| value.is_finite())
    {
        Ok(())
    } else {
        Err(ReferenceError::NonFiniteInput)
    }
}

fn unit_vector(position: SphericalPosition) -> Result<[f64; 3], ReferenceError> {
    let longitude = position.longitude_degrees.to_radians();
    let latitude = position.latitude_degrees.to_radians();
    let cos_latitude = latitude.cos();
    normalized([
        cos_latitude * longitude.cos(),
        cos_latitude * longitude.sin(),
        latitude.sin(),
    ])
}

fn local_tangent(position: SphericalPosition, wind: LocalWind) -> Result<[f64; 3], ReferenceError> {
    validate_position(position)?;
    let longitude = position.longitude_degrees.to_radians();
    let latitude = position.latitude_degrees.to_radians();
    let east = [-longitude.sin(), longitude.cos(), 0.0];
    let north = [
        -latitude.sin() * longitude.cos(),
        -latitude.sin() * longitude.sin(),
        latitude.cos(),
    ];
    Ok([
        wind.eastward_m_s * east[0] + wind.northward_m_s * north[0],
        wind.eastward_m_s * east[1] + wind.northward_m_s * north[1],
        wind.eastward_m_s * east[2] + wind.northward_m_s * north[2],
    ])
}

fn add_scaled(base: [f64; 3], increment: [f64; 3], scale: f64) -> [f64; 3] {
    [
        base[0] + increment[0] * scale,
        base[1] + increment[1] * scale,
        base[2] + increment[2] * scale,
    ]
}

fn normalized(vector: [f64; 3]) -> Result<[f64; 3], ReferenceError> {
    let norm_squared = vector[0].mul_add(
        vector[0],
        vector[1].mul_add(vector[1], vector[2] * vector[2]),
    );
    if !norm_squared.is_finite() || norm_squared <= 0.0 {
        return Err(ReferenceError::DegenerateVector);
    }
    let inverse_norm = norm_squared.sqrt().recip();
    let result = [
        vector[0] * inverse_norm,
        vector[1] * inverse_norm,
        vector[2] * inverse_norm,
    ];
    if result.iter().all(|value| value.is_finite()) {
        Ok(result)
    } else {
        Err(ReferenceError::DegenerateVector)
    }
}

fn position_from_vector(
    vector: [f64; 3],
    height_asl_m: f64,
) -> Result<SphericalPosition, ReferenceError> {
    if !height_asl_m.is_finite() {
        return Err(ReferenceError::NonFiniteInput);
    }
    let normalized = normalized(vector)?;
    let latitude = normalized[2].clamp(-1.0, 1.0).asin() * 180.0 / PI;
    if latitude.abs() == 90.0 {
        return Err(ReferenceError::PolarSingularity);
    }
    let longitude = normalize_longitude_degrees(normalized[1].atan2(normalized[0]) * 180.0 / PI);
    Ok(SphericalPosition {
        longitude_degrees: longitude,
        latitude_degrees: latitude,
        height_asl_m,
    })
}

fn normalize_longitude_degrees(longitude: f64) -> f64 {
    let normalized = (longitude + 180.0).rem_euclid(360.0) - 180.0;
    if normalized == 180.0 {
        -180.0
    } else {
        normalized
    }
}

fn positive_ulp(value: f64) -> f64 {
    if value == 0.0 {
        return f64::from_bits(1);
    }
    f64::from_bits(value.to_bits() + 1) - value
}

/// Scalar reference-formula failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceError {
    /// An input or output is NaN or infinite.
    NonFiniteInput,
    /// Exact poles have no unique local east direction.
    PolarSingularity,
    /// A three-dimensional direction cannot be normalized.
    DegenerateVector,
    /// Latitude/longitude cell bounds are invalid.
    InvalidGeometry,
    /// Pressure centres or derived interfaces are invalid or non-monotonic.
    InvalidPressureColumn,
    /// Dry-air mass or density inputs are outside their physical domain.
    InvalidMassInput,
    /// Ozone-rule inputs are outside their physical domain.
    InvalidOzoneInput,
}

impl ReferenceError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NonFiniteInput => "reference.non_finite_input",
            Self::PolarSingularity => "reference.polar_singularity",
            Self::DegenerateVector => "reference.degenerate_vector",
            Self::InvalidGeometry => "reference.invalid_geometry",
            Self::InvalidPressureColumn => "reference.invalid_pressure_column",
            Self::InvalidMassInput => "reference.invalid_mass_input",
            Self::InvalidOzoneInput => "reference.invalid_ozone_input",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn spherical_rk2_is_stationary_for_zero_wind_and_signed_for_reverse_time() {
        let start = SphericalPosition {
            longitude_degrees: 12.0,
            latitude_degrees: 45.0,
            height_asl_m: 100.0,
        };
        let zero = LocalWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            upward_m_s: 0.0,
        };
        let stationary = spherical_rk2_step(start, zero, zero, 60.0).unwrap();
        assert!((stationary.end.longitude_degrees - start.longitude_degrees).abs() < 1e-14);
        assert!((stationary.end.latitude_degrees - start.latitude_degrees).abs() < 1e-14);

        let east = LocalWind {
            eastward_m_s: 10.0,
            ..zero
        };
        let forward = spherical_rk2_step(start, east, east, 60.0).unwrap();
        let backward = spherical_rk2_step(start, east, east, -60.0).unwrap();
        assert!(forward.end.longitude_degrees > start.longitude_degrees);
        assert!(backward.end.longitude_degrees < start.longitude_degrees);
    }

    #[test]
    fn global_cell_area_is_full_sphere() {
        let area = spherical_lat_lon_cell_area_m2(0.0, 360.0, -90.0, 90.0).unwrap();
        let expected = 4.0 * PI * M4_CONSTANTS.earth_radius_m.powi(2);
        assert!((area - expected).abs() <= expected * 1e-15);
    }

    #[test]
    fn pressure_interfaces_are_log_midpoints_with_extrapolated_edges() {
        let interfaces = pressure_log_midpoint_interfaces_pa(&[100.0, 400.0, 1_600.0]).unwrap();
        assert_eq!(interfaces, vec![50.0, 200.0, 800.0, 3_200.0]);
    }

    #[test]
    fn dry_mass_density_and_ozone_rules_match_scalar_definitions() {
        let mass = dry_air_column_mass_kg(1.0, &[100.0, 200.0], &[0.0, 0.5]).unwrap();
        assert!((mass - 200.0 / M4_CONSTANTS.standard_gravity_m_s2).abs() < 1e-14);
        let density = dry_air_density_kg_m3(100_000.0, 300.0, 0.0).unwrap();
        assert!(density > 1.0 && density < 1.2);

        assert_eq!(
            flexpart_pv60_ozone_mass_kg(1.0, 3_000.0, 45.0, 3.0).unwrap(),
            None
        );
        let ozone = flexpart_pv60_ozone_mass_kg(1.0, 4_000.0, -45.0, -3.0)
            .unwrap()
            .unwrap();
        let expected = 3.0 * 60.0e-9 * 48.0 / 29.0;
        assert!((ozone - expected).abs() < 1e-20);
    }

    #[test]
    fn mass_gate_uses_relative_or_ulp_floor_whichever_is_larger() {
        let step = mass_balance_tolerance_kg(1.0e12, false).unwrap();
        let final_tolerance = mass_balance_tolerance_kg(1.0e12, true).unwrap();
        assert_eq!(step, 1.0);
        assert_eq!(final_tolerance, 10.0);
        assert!(mass_balance_tolerance_kg(0.0, false).unwrap() > 0.0);
        assert_eq!(
            mass_balance_tolerance_kg(f64::MAX, false),
            Err(ReferenceError::InvalidMassInput)
        );
    }
}
