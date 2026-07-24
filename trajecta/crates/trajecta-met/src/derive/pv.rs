//! # Contract: potential-vorticity derivation
//!
//! Produces explicitly defined potential vorticity and required gradients on
//! the native grid before query interpolation.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::derive::pressure::hybrid_pressure_column;
use crate::field::{CanonicalField, FieldKey, FieldQuality};
use crate::frame::{ArrayLayout, RawField, ValidityMask};
use crate::grid::DomainGeometry;
use crate::profile::graph::GraphUnit;
use crate::provenance::{ProvenanceRecord, TransformRecord};
use crate::science::{
    EARTH_ROTATION_RATE_RAD_S, ERTEL_PV_SPHERICAL_ALGORITHM_ID, M3_CONSTANTS,
    POTENTIAL_TEMPERATURE_REFERENCE_PRESSURE_PA,
};
use crate::vertical::VerticalTopology;

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Local pressure-coordinate derivatives consumed by the scalar Ertel-PV reference kernel.
///
/// Angular derivatives are with respect to longitude/latitude in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ErtelPvDerivatives {
    /// Potential-temperature derivative with respect to pressure, K Pa-1.
    pub dtheta_dp: f64,
    /// Eastward-wind derivative with respect to pressure, m s-1 Pa-1.
    pub du_dp: f64,
    /// Northward-wind derivative with respect to pressure, m s-1 Pa-1.
    pub dv_dp: f64,
    /// Potential-temperature derivative with respect to latitude, K rad-1.
    pub dtheta_dphi: f64,
    /// Potential-temperature derivative with respect to longitude, K rad-1.
    pub dtheta_dlambda: f64,
    /// Northward-wind derivative with respect to longitude, m s-1 rad-1.
    pub dv_dlambda: f64,
    /// Derivative of `u cos(phi)` with respect to latitude, m s-1 rad-1.
    pub d_u_cos_phi_dphi: f64,
}

impl ErtelPvDerivatives {
    fn all_finite(self) -> bool {
        [
            self.dtheta_dp,
            self.du_dp,
            self.dv_dp,
            self.dtheta_dphi,
            self.dtheta_dlambda,
            self.dv_dlambda,
            self.d_u_cos_phi_dphi,
        ]
        .into_iter()
        .all(f64::is_finite)
    }
}

/// Failure in the independent Ertel-PV numerical reference kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PotentialVorticityError {
    /// Temperature, pressure, coordinates, or derivatives are non-finite or non-physical.
    InvalidInput,
    /// The exact geographic poles have no unique local eastward basis.
    PolarSingularity,
    /// Three-point derivative support is duplicated or numerically singular.
    DegenerateStencil,
    /// A pressure column has fewer than three levels or is not strictly monotonic.
    InvalidPressureColumn,
    /// Input arrays disagree with the declared native-grid shape.
    ShapeMismatch,
    /// The regular grid is too small for three-point native-grid derivatives.
    InsufficientGrid,
    /// Latitude or non-periodic longitude lacks one real neighbor cell.
    MissingHorizontalHalo,
    /// Horizontal grid metadata violates the regular-grid contract.
    InvalidGridGeometry,
    /// Finite inputs produced a non-finite result.
    NumericalFailure,
}

impl PotentialVorticityError {
    /// Returns the stable machine-readable failure code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidInput => "pv.invalid_input",
            Self::PolarSingularity => "pv.polar_singularity",
            Self::DegenerateStencil => "pv.degenerate_stencil",
            Self::InvalidPressureColumn => "pv.invalid_pressure_column",
            Self::ShapeMismatch => "pv.shape_mismatch",
            Self::InsufficientGrid => "pv.insufficient_grid",
            Self::MissingHorizontalHalo => "pv.missing_horizontal_halo",
            Self::InvalidGridGeometry => "pv.invalid_grid_geometry",
            Self::NumericalFailure => "pv.numerical_failure",
        }
    }
}

/// One finite native-grid field plus an independent validity mask.
#[derive(Clone, Copy, Debug)]
pub struct NativePvField<'a> {
    /// Flat `[level][y][x]` values.
    pub values: &'a [f64],
    /// Structural validity in the same canonical order.
    pub valid: &'a [bool],
}

/// Native pressure coordinate used by Ertel-PV derivation.
#[derive(Clone, Copy, Debug)]
pub enum NativePvPressure<'a> {
    /// One fixed pressure value per full level, shared by every column.
    SharedLevels(&'a [f64]),
    /// One pressure value per native full-level grid point, used by hybrid coordinates.
    FullGrid(NativePvField<'a>),
}

/// Complete immutable input for native-grid Ertel-PV derivation.
#[derive(Clone, Copy, Debug)]
pub struct NativePvGridRequest<'a> {
    /// Regular native latitude/longitude geometry.
    pub geometry: &'a DomainGeometry,
    /// Native full-level count.
    pub levels: usize,
    /// Actual pressure coordinate in pascals.
    pub pressure: NativePvPressure<'a>,
    /// Air temperature in kelvin.
    pub temperature: NativePvField<'a>,
    /// Eastward wind in metres per second.
    pub eastward_wind: NativePvField<'a>,
    /// Northward wind in metres per second.
    pub northward_wind: NativePvField<'a>,
}

/// Native-grid Ertel PV in canonical `[level][y][x]` order.
#[derive(Clone, Debug, PartialEq)]
pub struct NativePvGrid {
    /// Finite PV values in PVU; invalid slots contain zero and must use `valid`.
    pub values_pvu: Vec<f64>,
    /// Independent local validity mask.
    pub valid: Vec<bool>,
    /// Native full-level count.
    pub levels: usize,
    /// Native latitude-row count.
    pub ny: usize,
    /// Native longitude-column count.
    pub nx: usize,
}

/// Computes Ertel PV on the native grid before any query interpolation.
///
/// Hybrid-pressure horizontal derivatives first remap each neighboring column
/// to the centre point's actual pressure with three-point Lagrange
/// interpolation. No vertical extrapolation is allowed.
pub fn derive_native_ertel_pv(
    request: NativePvGridRequest<'_>,
) -> Result<NativePvGrid, PotentialVorticityError> {
    request
        .geometry
        .validate()
        .map_err(|_| PotentialVorticityError::InvalidGridGeometry)?;
    let nx = request.geometry.nx;
    let ny = request.geometry.ny;
    if request.levels < 3 || nx < 3 || ny < 3 {
        return Err(PotentialVorticityError::InsufficientGrid);
    }
    if request.geometry.halo_cells < 1 {
        return Err(PotentialVorticityError::MissingHorizontalHalo);
    }
    let horizontal = nx
        .checked_mul(ny)
        .ok_or(PotentialVorticityError::ShapeMismatch)?;
    let count = horizontal
        .checked_mul(request.levels)
        .ok_or(PotentialVorticityError::ShapeMismatch)?;
    for field in [
        request.temperature,
        request.eastward_wind,
        request.northward_wind,
    ] {
        validate_native_field(field, count)?;
    }
    match request.pressure {
        NativePvPressure::SharedLevels(pressure_pa) => {
            if pressure_pa.len() != request.levels {
                return Err(PotentialVorticityError::ShapeMismatch);
            }
            validate_pressure_column(pressure_pa)?;
        }
        NativePvPressure::FullGrid(pressure) => {
            validate_native_field(pressure, count)?;
            validate_full_pressure_grid(request, horizontal)?;
        }
    }

    let mut theta = vec![0.0; count];
    let mut theta_valid = vec![false; count];
    for index in 0..count {
        if !request.temperature.valid[index] || !pressure_valid(request.pressure, index) {
            continue;
        }
        let pressure_pa = pressure_value(request.pressure, index, horizontal)?;
        theta[index] = potential_temperature_k(request.temperature.values[index], pressure_pa)?;
        theta_valid[index] = true;
    }
    let theta_field = NativePvField {
        values: &theta,
        valid: &theta_valid,
    };
    let mut output = NativePvGrid {
        values_pvu: vec![0.0; count],
        valid: vec![false; count],
        levels: request.levels,
        ny,
        nx,
    };
    let longitude_step = request.geometry.longitude_spacing_degrees.to_radians();
    for level in 0..request.levels {
        for y in 1..ny - 1 {
            let latitude_degrees = request.geometry.latitude_origin_degrees
                + request.geometry.latitude_spacing_degrees * y as f64;
            if latitude_degrees.abs() == 90.0 {
                continue;
            }
            let latitude_coordinates = [y - 1, y, y + 1].map(|row| {
                (request.geometry.latitude_origin_degrees
                    + request.geometry.latitude_spacing_degrees * row as f64)
                    .to_radians()
            });
            let x_start = usize::from(!request.geometry.periodic_longitude);
            let x_end = nx - usize::from(!request.geometry.periodic_longitude);
            for x in x_start..x_end {
                let index = flat_index(level, y, x, horizontal, nx)?;
                let target_pressure = pressure_value(request.pressure, index, horizontal)?;
                let Some(vertical) =
                    vertical_derivatives_at(request, theta_field, level, y, x, horizontal)?
                else {
                    continue;
                };
                let x_indices = [
                    if x == 0 { nx - 1 } else { x - 1 },
                    x,
                    if x + 1 == nx { 0 } else { x + 1 },
                ];
                let longitude_coordinates = [-longitude_step, 0.0, longitude_step];
                let Some(theta_lon) = horizontal_samples(
                    request,
                    theta_field,
                    level,
                    y,
                    x_indices,
                    target_pressure,
                    horizontal,
                )?
                else {
                    continue;
                };
                let Some(v_lon) = horizontal_samples(
                    request,
                    request.northward_wind,
                    level,
                    y,
                    x_indices,
                    target_pressure,
                    horizontal,
                )?
                else {
                    continue;
                };
                let y_indices = [y - 1, y, y + 1];
                let Some(theta_lat) = vertical_plane_samples(
                    request,
                    theta_field,
                    level,
                    y_indices,
                    x,
                    target_pressure,
                    horizontal,
                )?
                else {
                    continue;
                };
                let Some(u_lat) = vertical_plane_samples(
                    request,
                    request.eastward_wind,
                    level,
                    y_indices,
                    x,
                    target_pressure,
                    horizontal,
                )?
                else {
                    continue;
                };
                let u_cos_lat = [0, 1, 2]
                    .map(|position| u_lat[position] * latitude_coordinates[position].cos());
                let derivatives = ErtelPvDerivatives {
                    dtheta_dp: vertical.dtheta_dp,
                    du_dp: vertical.du_dp,
                    dv_dp: vertical.dv_dp,
                    dtheta_dphi: lagrange_derivative_three(
                        latitude_coordinates,
                        theta_lat,
                        latitude_coordinates[1],
                    )?,
                    dtheta_dlambda: lagrange_derivative_three(
                        longitude_coordinates,
                        theta_lon,
                        0.0,
                    )?,
                    dv_dlambda: lagrange_derivative_three(longitude_coordinates, v_lon, 0.0)?,
                    d_u_cos_phi_dphi: lagrange_derivative_three(
                        latitude_coordinates,
                        u_cos_lat,
                        latitude_coordinates[1],
                    )?,
                };
                output.values_pvu[index] =
                    ertel_potential_vorticity_pvu(latitude_degrees, derivatives)?;
                output.valid[index] = true;
            }
        }
    }
    Ok(output)
}

#[derive(Clone, Copy)]
struct VerticalDerivatives {
    dtheta_dp: f64,
    du_dp: f64,
    dv_dp: f64,
}

fn validate_native_field(
    field: NativePvField<'_>,
    expected: usize,
) -> Result<(), PotentialVorticityError> {
    if field.values.len() != expected || field.valid.len() != expected {
        return Err(PotentialVorticityError::ShapeMismatch);
    }
    if field.values.iter().any(|value| !value.is_finite()) {
        return Err(PotentialVorticityError::InvalidInput);
    }
    Ok(())
}

fn validate_full_pressure_grid(
    request: NativePvGridRequest<'_>,
    horizontal: usize,
) -> Result<(), PotentialVorticityError> {
    let NativePvPressure::FullGrid(pressure) = request.pressure else {
        return Ok(());
    };
    for horizontal_index in 0..horizontal {
        if (0..request.levels).all(|level| pressure.valid[level * horizontal + horizontal_index]) {
            validate_pressure_column_iter(
                request.levels,
                (0..request.levels)
                    .map(|level| pressure.values[level * horizontal + horizontal_index]),
            )?;
        }
    }
    Ok(())
}

fn pressure_valid(pressure: NativePvPressure<'_>, index: usize) -> bool {
    match pressure {
        NativePvPressure::SharedLevels(_) => true,
        NativePvPressure::FullGrid(field) => field.valid[index],
    }
}

fn pressure_value(
    pressure: NativePvPressure<'_>,
    index: usize,
    horizontal: usize,
) -> Result<f64, PotentialVorticityError> {
    match pressure {
        NativePvPressure::SharedLevels(levels) => levels
            .get(index / horizontal)
            .copied()
            .ok_or(PotentialVorticityError::ShapeMismatch),
        NativePvPressure::FullGrid(field) => field
            .values
            .get(index)
            .copied()
            .ok_or(PotentialVorticityError::ShapeMismatch),
    }
}

fn flat_index(
    level: usize,
    y: usize,
    x: usize,
    horizontal: usize,
    nx: usize,
) -> Result<usize, PotentialVorticityError> {
    level
        .checked_mul(horizontal)
        .and_then(|base| y.checked_mul(nx).and_then(|row| base.checked_add(row)))
        .and_then(|base| base.checked_add(x))
        .ok_or(PotentialVorticityError::ShapeMismatch)
}

fn vertical_derivatives_at(
    request: NativePvGridRequest<'_>,
    theta: NativePvField<'_>,
    level: usize,
    y: usize,
    x: usize,
    horizontal: usize,
) -> Result<Option<VerticalDerivatives>, PotentialVorticityError> {
    let start = derivative_stencil_start(level, request.levels);
    let mut pressure = [0.0; 3];
    let mut theta_values = [0.0; 3];
    let mut u_values = [0.0; 3];
    let mut v_values = [0.0; 3];
    for offset in 0..3 {
        let sample_level = start + offset;
        let index = flat_index(sample_level, y, x, horizontal, request.geometry.nx)?;
        if !pressure_valid(request.pressure, index)
            || !theta.valid[index]
            || !request.eastward_wind.valid[index]
            || !request.northward_wind.valid[index]
        {
            return Ok(None);
        }
        pressure[offset] = pressure_value(request.pressure, index, horizontal)?;
        theta_values[offset] = theta.values[index];
        u_values[offset] = request.eastward_wind.values[index];
        v_values[offset] = request.northward_wind.values[index];
    }
    let evaluation = pressure[level - start];
    Ok(Some(VerticalDerivatives {
        dtheta_dp: lagrange_derivative_three(pressure, theta_values, evaluation)?,
        du_dp: lagrange_derivative_three(pressure, u_values, evaluation)?,
        dv_dp: lagrange_derivative_three(pressure, v_values, evaluation)?,
    }))
}

fn derivative_stencil_start(level: usize, levels: usize) -> usize {
    if level == 0 {
        0
    } else if level + 1 == levels {
        levels - 3
    } else {
        level - 1
    }
}

fn horizontal_samples(
    request: NativePvGridRequest<'_>,
    field: NativePvField<'_>,
    level: usize,
    y: usize,
    x_indices: [usize; 3],
    target_pressure: f64,
    horizontal: usize,
) -> Result<Option<[f64; 3]>, PotentialVorticityError> {
    let mut values = [0.0; 3];
    for (position, x) in x_indices.into_iter().enumerate() {
        let Some(value) =
            sample_column_at_pressure(request, field, level, y, x, target_pressure, horizontal)?
        else {
            return Ok(None);
        };
        values[position] = value;
    }
    Ok(Some(values))
}

fn vertical_plane_samples(
    request: NativePvGridRequest<'_>,
    field: NativePvField<'_>,
    level: usize,
    y_indices: [usize; 3],
    x: usize,
    target_pressure: f64,
    horizontal: usize,
) -> Result<Option<[f64; 3]>, PotentialVorticityError> {
    let mut values = [0.0; 3];
    for (position, y) in y_indices.into_iter().enumerate() {
        let Some(value) =
            sample_column_at_pressure(request, field, level, y, x, target_pressure, horizontal)?
        else {
            return Ok(None);
        };
        values[position] = value;
    }
    Ok(Some(values))
}

fn sample_column_at_pressure(
    request: NativePvGridRequest<'_>,
    field: NativePvField<'_>,
    level: usize,
    y: usize,
    x: usize,
    target_pressure: f64,
    horizontal: usize,
) -> Result<Option<f64>, PotentialVorticityError> {
    if matches!(request.pressure, NativePvPressure::SharedLevels(_)) {
        let index = flat_index(level, y, x, horizontal, request.geometry.nx)?;
        return Ok(field.valid[index].then_some(field.values[index]));
    }
    if !target_pressure.is_finite() || target_pressure <= 0.0 {
        return Err(PotentialVorticityError::InvalidInput);
    }
    let NativePvPressure::FullGrid(pressure) = request.pressure else {
        return Err(PotentialVorticityError::ShapeMismatch);
    };
    let horizontal_index = y
        .checked_mul(request.geometry.nx)
        .and_then(|row| row.checked_add(x))
        .ok_or(PotentialVorticityError::ShapeMismatch)?;
    let index_at = |sample_level: usize| {
        sample_level
            .checked_mul(horizontal)
            .and_then(|base| base.checked_add(horizontal_index))
            .ok_or(PotentialVorticityError::ShapeMismatch)
    };
    let value_at = |sample_level: usize| -> Result<(f64, f64, bool), PotentialVorticityError> {
        let index = index_at(sample_level)?;
        Ok((
            pressure.values[index],
            field.values[index],
            pressure.valid[index] && field.valid[index],
        ))
    };

    // The centre column is queried at its own native pressure. Taking this
    // exact-hit path first avoids even a logarithmic search for one third of
    // all horizontal-remap samples.
    let native = value_at(level)?;
    if native.0 == target_pressure {
        return Ok(native.2.then_some(native.1));
    }

    let first = value_at(0)?.0;
    let last = value_at(request.levels - 1)?.0;
    let increasing = last > first;
    if first == last
        || (increasing && !(first..=last).contains(&target_pressure))
        || (!increasing && !(last..=first).contains(&target_pressure))
    {
        return Ok(None);
    }

    let mut lower = 0;
    let mut upper = request.levels - 1;
    while upper - lower > 1 {
        let middle = lower + (upper - lower) / 2;
        let sample = value_at(middle)?;
        if sample.0 == target_pressure {
            return Ok(sample.2.then_some(sample.1));
        }
        if (increasing && sample.0 < target_pressure) || (!increasing && sample.0 > target_pressure)
        {
            lower = middle;
        } else {
            upper = middle;
        }
    }
    let lower_pressure = value_at(lower)?.0;
    let upper_pressure = value_at(upper)?.0;
    let nearest =
        if (target_pressure - lower_pressure).abs() <= (target_pressure - upper_pressure).abs() {
            lower
        } else {
            upper
        };
    let start = nearest.saturating_sub(1).min(request.levels - 3);
    let mut coordinates = [0.0; 3];
    let mut values = [0.0; 3];
    for offset in 0..3 {
        let sample = value_at(start + offset)?;
        if !sample.2 {
            return Ok(None);
        }
        coordinates[offset] = sample.0;
        values[offset] = sample.1;
    }
    lagrange_value_three(coordinates, values, target_pressure).map(Some)
}

fn lagrange_value_three(
    coordinates: [f64; 3],
    values: [f64; 3],
    evaluation_coordinate: f64,
) -> Result<f64, PotentialVorticityError> {
    if coordinates
        .into_iter()
        .chain(values)
        .chain([evaluation_coordinate])
        .any(|value| !value.is_finite())
    {
        return Err(PotentialVorticityError::InvalidInput);
    }
    let [x0, x1, x2] = coordinates;
    let denominators = [
        (x0 - x1) * (x0 - x2),
        (x1 - x0) * (x1 - x2),
        (x2 - x0) * (x2 - x1),
    ];
    if denominators
        .into_iter()
        .any(|denominator| denominator == 0.0 || !denominator.is_finite())
    {
        return Err(PotentialVorticityError::DegenerateStencil);
    }
    let weights = [
        (evaluation_coordinate - x1) * (evaluation_coordinate - x2) / denominators[0],
        (evaluation_coordinate - x0) * (evaluation_coordinate - x2) / denominators[1],
        (evaluation_coordinate - x0) * (evaluation_coordinate - x1) / denominators[2],
    ];
    let value = weights[0].mul_add(
        values[0],
        weights[1].mul_add(values[1], weights[2] * values[2]),
    );
    if value.is_finite() {
        Ok(value)
    } else {
        Err(PotentialVorticityError::NumericalFailure)
    }
}

/// Computes dry-air potential temperature using the frozen M4 reference pressure.
pub fn potential_temperature_k(
    temperature_k: f64,
    pressure_pa: f64,
) -> Result<f64, PotentialVorticityError> {
    if !temperature_k.is_finite()
        || temperature_k <= 0.0
        || !pressure_pa.is_finite()
        || pressure_pa <= 0.0
    {
        return Err(PotentialVorticityError::InvalidInput);
    }
    let exponent =
        M3_CONSTANTS.dry_air_gas_constant_j_kg_k / M3_CONSTANTS.dry_air_heat_capacity_j_kg_k;
    let theta =
        temperature_k * (POTENTIAL_TEMPERATURE_REFERENCE_PRESSURE_PA / pressure_pa).powf(exponent);
    if theta.is_finite() && theta > 0.0 {
        Ok(theta)
    } else {
        Err(PotentialVorticityError::NumericalFailure)
    }
}

/// Evaluates the frozen spherical pressure-coordinate Ertel-PV formula in PVU.
pub fn ertel_potential_vorticity_pvu(
    latitude_degrees: f64,
    derivatives: ErtelPvDerivatives,
) -> Result<f64, PotentialVorticityError> {
    if !latitude_degrees.is_finite()
        || !(-90.0..=90.0).contains(&latitude_degrees)
        || !derivatives.all_finite()
    {
        return Err(PotentialVorticityError::InvalidInput);
    }
    if latitude_degrees.abs() == 90.0 {
        return Err(PotentialVorticityError::PolarSingularity);
    }
    let latitude = latitude_degrees.to_radians();
    let (sin_latitude, cos_latitude) = latitude.sin_cos();
    let radius = M3_CONSTANTS.earth_radius_m;
    let relative_vorticity =
        (derivatives.dv_dlambda - derivatives.d_u_cos_phi_dphi) / (radius * cos_latitude);
    let absolute_vorticity = relative_vorticity + 2.0 * EARTH_ROTATION_RATE_RAD_S * sin_latitude;
    let bracket = absolute_vorticity.mul_add(
        derivatives.dtheta_dp,
        (derivatives.du_dp / radius).mul_add(
            derivatives.dtheta_dphi,
            -(derivatives.dv_dp / (radius * cos_latitude)) * derivatives.dtheta_dlambda,
        ),
    );
    let pv_pvu = -M3_CONSTANTS.standard_gravity_m_s2 * bracket * 1.0e6;
    if pv_pvu.is_finite() {
        Ok(pv_pvu)
    } else {
        Err(PotentialVorticityError::NumericalFailure)
    }
}

/// Evaluates a three-point Lagrange derivative at an arbitrary support coordinate.
pub fn lagrange_derivative_three(
    coordinates: [f64; 3],
    values: [f64; 3],
    evaluation_coordinate: f64,
) -> Result<f64, PotentialVorticityError> {
    if coordinates
        .into_iter()
        .chain(values)
        .chain([evaluation_coordinate])
        .any(|value| !value.is_finite())
    {
        return Err(PotentialVorticityError::InvalidInput);
    }
    let [x0, x1, x2] = coordinates;
    let denominators = [
        (x0 - x1) * (x0 - x2),
        (x1 - x0) * (x1 - x2),
        (x2 - x0) * (x2 - x1),
    ];
    if denominators
        .into_iter()
        .any(|denominator| denominator == 0.0 || !denominator.is_finite())
    {
        return Err(PotentialVorticityError::DegenerateStencil);
    }
    let weights = [
        (2.0 * evaluation_coordinate - x1 - x2) / denominators[0],
        (2.0 * evaluation_coordinate - x0 - x2) / denominators[1],
        (2.0 * evaluation_coordinate - x0 - x1) / denominators[2],
    ];
    let derivative = weights[0].mul_add(
        values[0],
        weights[1].mul_add(values[1], weights[2] * values[2]),
    );
    if derivative.is_finite() {
        Ok(derivative)
    } else {
        Err(PotentialVorticityError::NumericalFailure)
    }
}

/// Computes one vertical derivative using centred or top/bottom one-sided three-point support.
pub fn pressure_derivative_three(
    pressure_pa: &[f64],
    values: &[f64],
    level: usize,
) -> Result<f64, PotentialVorticityError> {
    if pressure_pa.len() != values.len() || level >= pressure_pa.len() {
        return Err(PotentialVorticityError::InvalidInput);
    }
    validate_pressure_column(pressure_pa)?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err(PotentialVorticityError::InvalidInput);
    }
    let start = if level == 0 {
        0
    } else if level + 1 == pressure_pa.len() {
        pressure_pa.len() - 3
    } else {
        level - 1
    };
    lagrange_derivative_three(
        [
            pressure_pa[start],
            pressure_pa[start + 1],
            pressure_pa[start + 2],
        ],
        [values[start], values[start + 1], values[start + 2]],
        pressure_pa[level],
    )
}

fn validate_pressure_column(pressure_pa: &[f64]) -> Result<(), PotentialVorticityError> {
    validate_pressure_column_iter(pressure_pa.len(), pressure_pa.iter().copied())
}

fn validate_pressure_column_iter(
    levels: usize,
    pressure_pa: impl IntoIterator<Item = f64>,
) -> Result<(), PotentialVorticityError> {
    if levels < 3 {
        return Err(PotentialVorticityError::InvalidPressureColumn);
    }
    let mut values = pressure_pa.into_iter();
    let Some(mut previous) = values.next() else {
        return Err(PotentialVorticityError::InvalidPressureColumn);
    };
    if !previous.is_finite() || previous <= 0.0 {
        return Err(PotentialVorticityError::InvalidPressureColumn);
    }
    let Some(current) = values.next() else {
        return Err(PotentialVorticityError::InvalidPressureColumn);
    };
    if !current.is_finite() || current <= 0.0 || current == previous {
        return Err(PotentialVorticityError::InvalidPressureColumn);
    }
    let increasing = current > previous;
    previous = current;
    let mut observed = 2;
    for current in values {
        if !current.is_finite()
            || current <= 0.0
            || (increasing && current <= previous)
            || (!increasing && current >= previous)
        {
            return Err(PotentialVorticityError::InvalidPressureColumn);
        }
        previous = current;
        observed += 1;
    }
    if observed == levels {
        Ok(())
    } else {
        Err(PotentialVorticityError::InvalidPressureColumn)
    }
}

/// Potential-vorticity and gradient deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct PotentialVorticityDeriver;

impl FieldDeriver for PotentialVorticityDeriver {
    fn derive(&self, request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        let output_key = FieldKey::Canonical(CanonicalField::PotentialVorticity);
        if !request.outputs.contains(&output_key) {
            return Ok(Vec::new());
        }
        derive_frame_potential_vorticity(request.frame).map(|field| vec![field])
    }
}

enum PreparedPressure<'a> {
    Shared(&'a [f64]),
    FullGrid { values: Vec<f64>, valid: Vec<bool> },
}

fn derive_frame_potential_vorticity(
    frame: &crate::frame::RawMetFrame,
) -> Result<DerivedField, DeriveError> {
    let eastward_key = FieldKey::Canonical(CanonicalField::EastwardWind);
    let northward_key = FieldKey::Canonical(CanonicalField::NorthwardWind);
    let temperature_key = FieldKey::Canonical(CanonicalField::AirTemperature);
    let eastward = required_field(frame, &eastward_key)?;
    let northward = required_field(frame, &northward_key)?;
    let temperature = required_field(frame, &temperature_key)?;
    validate_unit(eastward, "m/s")?;
    validate_unit(northward, "m/s")?;
    validate_unit(temperature, "K")?;
    let ArrayLayout::Full3D { levels, ny, nx } = temperature.layout() else {
        return Err(DeriveError::ShapeMismatch);
    };
    if eastward.layout() != temperature.layout()
        || northward.layout() != temperature.layout()
        || temperature.temporal() != eastward.temporal()
        || temperature.temporal() != northward.temporal()
        || nx != frame.metadata().grid.nx
        || ny != frame.metadata().grid.ny
    {
        return Err(DeriveError::ShapeMismatch);
    }
    let horizontal = nx.checked_mul(ny).ok_or(DeriveError::ShapeMismatch)?;
    let prepared_pressure = match &frame.metadata().vertical {
        VerticalTopology::PressureLevels(topology) => {
            if topology.pressure_pa.len() != levels {
                return Err(DeriveError::ShapeMismatch);
            }
            PreparedPressure::Shared(&topology.pressure_pa)
        }
        VerticalTopology::HybridPressure(topology) => {
            let surface_key = FieldKey::Canonical(CanonicalField::SurfacePressure);
            let surface = required_field(frame, &surface_key)?;
            validate_unit(surface, "Pa")?;
            if surface.layout() != (ArrayLayout::Horizontal2D { ny, nx })
                || surface.temporal() != temperature.temporal()
            {
                return Err(DeriveError::ShapeMismatch);
            }
            let count = horizontal
                .checked_mul(levels)
                .ok_or(DeriveError::ShapeMismatch)?;
            let mut values = vec![0.0; count];
            let mut valid = vec![false; count];
            for horizontal_index in 0..horizontal {
                if surface.validity().get(horizontal_index) != Some(true) {
                    continue;
                }
                let surface_pressure = surface
                    .values()
                    .get(horizontal_index)
                    .copied()
                    .ok_or(DeriveError::ShapeMismatch)?;
                let column = hybrid_pressure_column(
                    &topology.coefficients.a_half_pa,
                    &topology.coefficients.b_half,
                    surface_pressure,
                )
                .map_err(|error| DeriveError::InvalidPhysicalState(format!("{error:?}")))?;
                for (level, native_level) in topology.active_full_levels.iter().enumerate() {
                    let native_index = usize::from(*native_level)
                        .checked_sub(1)
                        .ok_or(DeriveError::ShapeMismatch)?;
                    let pressure = column
                        .full_pa
                        .get(native_index)
                        .copied()
                        .ok_or(DeriveError::ShapeMismatch)?;
                    let index = level
                        .checked_mul(horizontal)
                        .and_then(|base| base.checked_add(horizontal_index))
                        .ok_or(DeriveError::ShapeMismatch)?;
                    values[index] = pressure;
                    valid[index] = true;
                }
            }
            PreparedPressure::FullGrid { values, valid }
        }
    };
    let pressure = match &prepared_pressure {
        PreparedPressure::Shared(levels) => NativePvPressure::SharedLevels(levels),
        PreparedPressure::FullGrid { values, valid } => {
            NativePvPressure::FullGrid(NativePvField { values, valid })
        }
    };
    let grid = derive_native_ertel_pv(NativePvGridRequest {
        geometry: &frame.metadata().grid,
        levels,
        pressure,
        temperature: native_field(temperature),
        eastward_wind: native_field(eastward),
        northward_wind: native_field(northward),
    })
    .map_err(|error| DeriveError::InvalidPhysicalState(error.code().into()))?;

    let mut inputs = vec![eastward_key, northward_key, temperature_key];
    if matches!(&prepared_pressure, PreparedPressure::FullGrid { .. }) {
        inputs.push(FieldKey::Canonical(CanonicalField::SurfacePressure));
    }
    let sources = inputs
        .iter()
        .filter_map(|key| frame.fields().get(key))
        .filter_map(|field| frame.provenance().get(field.provenance()))
        .flat_map(|record| record.sources.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let vertical_coordinate = match prepared_pressure {
        PreparedPressure::Shared(_) => "pressure_levels",
        PreparedPressure::FullGrid { .. } => "hybrid_pressure",
    };
    let output_key = FieldKey::Canonical(CanonicalField::PotentialVorticity);
    let provenance = ProvenanceRecord {
        field: output_key.clone(),
        quality: FieldQuality::Derived,
        sources,
        transforms: vec![TransformRecord {
            operation: ERTEL_PV_SPHERICAL_ALGORITHM_ID.into(),
            parameters: vec![
                ("vertical_coordinate".into(), vertical_coordinate.into()),
                (
                    "horizontal_derivative".into(),
                    "three_point_lagrange".into(),
                ),
                (
                    "vertical_derivative".into(),
                    "actual_pressure_three_point_lagrange".into(),
                ),
                (
                    "hybrid_horizontal_remap".into(),
                    "centre_pressure_three_point_lagrange_no_extrapolation".into(),
                ),
                ("output_unit".into(), "PVU".into()),
            ],
        }],
        fallback_reason: None,
        profile_sha256: frame.metadata().id.profile_sha256.clone(),
    };
    Ok(DerivedField {
        key: output_key,
        values: Arc::from(grid.values_pvu),
        validity: ValidityMask::new(Arc::from(grid.valid)),
        unit: GraphUnit::parse("PVU")
            .map_err(|error| DeriveError::InvalidPhysicalState(format!("{error:?}")))?,
        layout: ArrayLayout::Full3D { levels, ny, nx },
        temporal: temperature.temporal(),
        quality: FieldQuality::Derived,
        inputs,
        provenance,
    })
}

fn required_field<'a>(
    frame: &'a crate::frame::RawMetFrame,
    key: &FieldKey,
) -> Result<&'a RawField, DeriveError> {
    frame
        .fields()
        .get(key)
        .ok_or_else(|| DeriveError::MissingInput(key.clone()))
}

fn native_field(field: &RawField) -> NativePvField<'_> {
    NativePvField {
        values: field.values(),
        valid: field.validity().as_arc(),
    }
}

fn validate_unit(field: &RawField, expected: &str) -> Result<(), DeriveError> {
    let expected = GraphUnit::parse(expected)
        .map_err(|error| DeriveError::InvalidPhysicalState(format!("{error:?}")))?;
    if field.unit().dimension() == expected.dimension()
        && field.unit().scale_to_si() == expected.scale_to_si()
        && field.unit().offset_to_si() == expected.offset_to_si()
    {
        Ok(())
    } else {
        Err(DeriveError::InvalidPhysicalState(
            "canonical input unit mismatch".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use trajecta_case::model::meteorology::DomainId;
    use trajecta_case::model::time::Timestamp;

    use super::*;
    use crate::frame::{FrameMetadata, RawFieldStore, TemporalSupport};
    use crate::io::inventory::LogicalFrameId;
    use crate::provenance::ProvenanceTable;
    use crate::vertical::PressureLevels;

    fn quadratic(x: f64) -> f64 {
        3.0 * x * x - 2.0 * x + 7.0
    }

    fn test_geometry() -> DomainGeometry {
        DomainGeometry {
            domain: DomainId("pv-test".into()),
            longitude_origin_degrees: -20.0,
            latitude_origin_degrees: -40.0,
            longitude_spacing_degrees: 10.0,
            latitude_spacing_degrees: 20.0,
            nx: 5,
            ny: 5,
            periodic_longitude: false,
            halo_cells: 1,
        }
    }

    fn temperature_from_theta(theta_k: f64, pressure_pa: f64) -> f64 {
        let exponent =
            M3_CONSTANTS.dry_air_gas_constant_j_kg_k / M3_CONSTANTS.dry_air_heat_capacity_j_kg_k;
        theta_k * (pressure_pa / POTENTIAL_TEMPERATURE_REFERENCE_PRESSURE_PA).powf(exponent)
    }

    fn pressure_test_frame() -> crate::frame::RawMetFrame {
        let geometry = test_geometry();
        let levels = [20_000.0, 50_000.0, 90_000.0];
        let horizontal = geometry.nx * geometry.ny;
        let layout = ArrayLayout::Full3D {
            levels: levels.len(),
            ny: geometry.ny,
            nx: geometry.nx,
        };
        let time = Timestamp::UNIX_EPOCH;
        let id = LogicalFrameId {
            domain: geometry.domain.clone(),
            valid_time: time,
            profile_sha256: "pv-profile".into(),
            content_sha256: "pv-content".into(),
        };
        let metadata = FrameMetadata {
            id: id.clone(),
            domain: geometry.domain.clone(),
            valid_time: time,
            grid: geometry,
            vertical: VerticalTopology::PressureLevels(
                PressureLevels::new(Arc::from(levels)).unwrap(),
            ),
        };
        let mut store = RawFieldStore::new();
        let mut provenance = ProvenanceTable::new();
        for (canonical, unit, values) in [
            (
                CanonicalField::AirTemperature,
                "K",
                levels
                    .into_iter()
                    .flat_map(|pressure| vec![temperature_from_theta(315.0, pressure); horizontal])
                    .collect::<Vec<_>>(),
            ),
            (
                CanonicalField::EastwardWind,
                "m/s",
                vec![0.0; levels.len() * horizontal],
            ),
            (
                CanonicalField::NorthwardWind,
                "m/s",
                vec![0.0; levels.len() * horizontal],
            ),
        ] {
            let key = FieldKey::Canonical(canonical);
            let provenance_id = provenance
                .intern(ProvenanceRecord {
                    field: key.clone(),
                    quality: FieldQuality::Source,
                    sources: vec![format!("source:{canonical:?}")],
                    transforms: Vec::new(),
                    fallback_reason: None,
                    profile_sha256: id.profile_sha256.clone(),
                })
                .unwrap();
            let count = values.len();
            store
                .insert(
                    key,
                    RawField::new(
                        Arc::from(values),
                        Arc::from(vec![true; count]),
                        GraphUnit::parse(unit).unwrap(),
                        layout,
                        TemporalSupport::Instantaneous { valid_time: time },
                        FieldQuality::Source,
                        provenance_id,
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        crate::frame::RawMetFrame::publish(metadata, store, Arc::new(provenance)).unwrap()
    }

    #[test]
    fn potential_temperature_uses_frozen_reference_pressure() {
        assert_eq!(potential_temperature_k(300.0, 100_000.0).unwrap(), 300.0);
        let theta = potential_temperature_k(250.0, 50_000.0).unwrap();
        let exponent =
            M3_CONSTANTS.dry_air_gas_constant_j_kg_k / M3_CONSTANTS.dry_air_heat_capacity_j_kg_k;
        assert_eq!(theta, 250.0 * 2.0_f64.powf(exponent));
        assert_eq!(
            potential_temperature_k(300.0, 0.0),
            Err(PotentialVorticityError::InvalidInput)
        );
    }

    #[test]
    fn nonuniform_three_point_derivative_is_exact_for_quadratics() {
        let x = [10_000.0, 35_000.0, 90_000.0];
        let y = x.map(quadratic);
        for evaluation in x {
            let derivative = lagrange_derivative_three(x, y, evaluation).unwrap();
            let expected = 6.0 * evaluation - 2.0;
            assert!((derivative - expected).abs() <= 2.0e-10 * expected.abs());
        }
    }

    #[test]
    fn vertical_derivative_uses_one_sided_and_centred_three_point_support() {
        let pressure = [10_000.0, 25_000.0, 60_000.0, 100_000.0];
        let values = pressure.map(quadratic);
        for level in 0..pressure.len() {
            let derivative = pressure_derivative_three(&pressure, &values, level).unwrap();
            let expected = 6.0 * pressure[level] - 2.0;
            assert!((derivative - expected).abs() <= 2.0e-10 * expected.abs());
        }
        assert_eq!(
            pressure_derivative_three(&[10_000.0, 9_000.0, 20_000.0], &[1.0, 2.0, 3.0], 1),
            Err(PotentialVorticityError::InvalidPressureColumn)
        );
    }

    #[test]
    fn constant_theta_and_zero_wind_have_zero_pv() {
        let pv = ertel_potential_vorticity_pvu(
            45.0,
            ErtelPvDerivatives {
                dtheta_dp: 0.0,
                du_dp: 0.0,
                dv_dp: 0.0,
                dtheta_dphi: 0.0,
                dtheta_dlambda: 0.0,
                dv_dlambda: 0.0,
                d_u_cos_phi_dphi: 0.0,
            },
        )
        .unwrap();
        assert_eq!(pv, 0.0);
    }

    #[test]
    fn solid_body_rotation_matches_closed_form() {
        let latitude_degrees: f64 = 37.0;
        let latitude = latitude_degrees.to_radians();
        let imposed_rotation = 2.5e-5;
        let dtheta_dp = -4.0e-4;
        let derivatives = ErtelPvDerivatives {
            dtheta_dp,
            du_dp: 0.0,
            dv_dp: 0.0,
            dtheta_dphi: 0.0,
            dtheta_dlambda: 0.0,
            dv_dlambda: 0.0,
            d_u_cos_phi_dphi: -2.0
                * imposed_rotation
                * M3_CONSTANTS.earth_radius_m
                * latitude.cos()
                * latitude.sin(),
        };
        let actual = ertel_potential_vorticity_pvu(latitude_degrees, derivatives).unwrap();
        let expected = -M3_CONSTANTS.standard_gravity_m_s2
            * (2.0 * (imposed_rotation + EARTH_ROTATION_RATE_RAD_S) * latitude.sin())
            * dtheta_dp
            * 1.0e6;
        assert!((actual - expected).abs() <= 4.0 * f64::EPSILON * expected.abs());
    }

    #[test]
    fn baroclinic_cross_terms_match_direct_formula() {
        let latitude_degrees: f64 = -28.0;
        let latitude = latitude_degrees.to_radians();
        let derivatives = ErtelPvDerivatives {
            dtheta_dp: -2.1e-4,
            du_dp: 3.0e-5,
            dv_dp: -1.5e-5,
            dtheta_dphi: 17.0,
            dtheta_dlambda: -9.0,
            dv_dlambda: 4.0,
            d_u_cos_phi_dphi: -3.0,
        };
        let relative_vorticity = (derivatives.dv_dlambda - derivatives.d_u_cos_phi_dphi)
            / (M3_CONSTANTS.earth_radius_m * latitude.cos());
        let expected = -M3_CONSTANTS.standard_gravity_m_s2
            * ((relative_vorticity + 2.0 * EARTH_ROTATION_RATE_RAD_S * latitude.sin())
                * derivatives.dtheta_dp
                + derivatives.du_dp / M3_CONSTANTS.earth_radius_m * derivatives.dtheta_dphi
                - derivatives.dv_dp / (M3_CONSTANTS.earth_radius_m * latitude.cos())
                    * derivatives.dtheta_dlambda)
            * 1.0e6;
        let actual = ertel_potential_vorticity_pvu(latitude_degrees, derivatives).unwrap();
        assert!((actual - expected).abs() <= 8.0 * f64::EPSILON * expected.abs());
        assert_eq!(
            ertel_potential_vorticity_pvu(90.0, derivatives),
            Err(PotentialVorticityError::PolarSingularity)
        );
    }

    #[test]
    fn pressure_grid_baroclinic_fixture_is_exact_in_the_safe_core() {
        let geometry = test_geometry();
        let pressure = [20_000.0, 50_000.0, 90_000.0];
        let horizontal = geometry.nx * geometry.ny;
        let count = pressure.len() * horizontal;
        let mut temperature = vec![0.0; count];
        let mut eastward = vec![0.0; count];
        let mut northward = vec![0.0; count];
        let valid = vec![true; count];
        let dtheta_dp: f64 = -1.3e-4;
        let dtheta_dphi: f64 = 7.0;
        let dtheta_dlambda: f64 = -5.0;
        let dv_dp: f64 = 1.7e-5;
        let dv_dlambda: f64 = 3.5;
        for (level, pressure_pa) in pressure.into_iter().enumerate() {
            for y in 0..geometry.ny {
                let phi = (geometry.latitude_origin_degrees
                    + geometry.latitude_spacing_degrees * y as f64)
                    .to_radians();
                for x in 0..geometry.nx {
                    let lambda = (geometry.longitude_origin_degrees
                        + geometry.longitude_spacing_degrees * x as f64)
                        .to_radians();
                    let index = level * horizontal + y * geometry.nx + x;
                    let theta = 320.0
                        + dtheta_dp * (pressure_pa - 50_000.0)
                        + dtheta_dphi * phi
                        + dtheta_dlambda * lambda;
                    temperature[index] = temperature_from_theta(theta, pressure_pa);
                    eastward[index] = 0.0;
                    northward[index] = dv_dp.mul_add(pressure_pa, dv_dlambda * lambda);
                }
            }
        }
        let output = derive_native_ertel_pv(NativePvGridRequest {
            geometry: &geometry,
            levels: pressure.len(),
            pressure: NativePvPressure::SharedLevels(&pressure),
            temperature: NativePvField {
                values: &temperature,
                valid: &valid,
            },
            eastward_wind: NativePvField {
                values: &eastward,
                valid: &valid,
            },
            northward_wind: NativePvField {
                values: &northward,
                valid: &valid,
            },
        })
        .unwrap();
        assert_eq!(output.valid.iter().filter(|value| **value).count(), 27);
        for level in 0..pressure.len() {
            for y in 1..geometry.ny - 1 {
                let latitude_degrees =
                    geometry.latitude_origin_degrees + geometry.latitude_spacing_degrees * y as f64;
                let expected = ertel_potential_vorticity_pvu(
                    latitude_degrees,
                    ErtelPvDerivatives {
                        dtheta_dp,
                        du_dp: 0.0,
                        dv_dp,
                        dtheta_dphi,
                        dtheta_dlambda,
                        dv_dlambda,
                        d_u_cos_phi_dphi: 0.0,
                    },
                )
                .unwrap();
                for x in 1..geometry.nx - 1 {
                    let index = level * horizontal + y * geometry.nx + x;
                    assert!(output.valid[index]);
                    assert!(
                        (output.values_pvu[index] - expected).abs()
                            <= 2.0e-10 * expected.abs().max(1.0)
                    );
                }
            }
        }
        assert!(!output.valid[0]);
        assert!(!output.valid[horizontal - 1]);
    }

    #[test]
    fn hybrid_grid_remaps_neighbors_to_the_centre_pressure() {
        let geometry = test_geometry();
        let base_pressure = [20_000.0, 40_000.0, 60_000.0, 80_000.0, 100_000.0];
        let horizontal = geometry.nx * geometry.ny;
        let count = base_pressure.len() * horizontal;
        let mut pressure = vec![0.0; count];
        let mut temperature = vec![0.0; count];
        let eastward = vec![0.0; count];
        let mut northward = vec![0.0; count];
        let valid = vec![true; count];
        let dtheta_dp: f64 = -1.1e-4;
        let dtheta_dphi: f64 = 6.0;
        let dtheta_dlambda: f64 = 4.0;
        let dv_dp: f64 = -1.2e-5;
        let dv_dlambda: f64 = 2.5;
        for (level, base) in base_pressure.into_iter().enumerate() {
            for y in 0..geometry.ny {
                let phi = (geometry.latitude_origin_degrees
                    + geometry.latitude_spacing_degrees * y as f64)
                    .to_radians();
                for x in 0..geometry.nx {
                    let lambda = (geometry.longitude_origin_degrees
                        + geometry.longitude_spacing_degrees * x as f64)
                        .to_radians();
                    let index = level * horizontal + y * geometry.nx + x;
                    let pressure_pa = base * (1.0 + 0.001 * (x + y) as f64);
                    pressure[index] = pressure_pa;
                    let theta = 315.0
                        + dtheta_dp * (pressure_pa - 60_000.0)
                        + dtheta_dphi * phi
                        + dtheta_dlambda * lambda;
                    temperature[index] = temperature_from_theta(theta, pressure_pa);
                    northward[index] = dv_dp.mul_add(pressure_pa, dv_dlambda * lambda);
                }
            }
        }
        let output = derive_native_ertel_pv(NativePvGridRequest {
            geometry: &geometry,
            levels: base_pressure.len(),
            pressure: NativePvPressure::FullGrid(NativePvField {
                values: &pressure,
                valid: &valid,
            }),
            temperature: NativePvField {
                values: &temperature,
                valid: &valid,
            },
            eastward_wind: NativePvField {
                values: &eastward,
                valid: &valid,
            },
            northward_wind: NativePvField {
                values: &northward,
                valid: &valid,
            },
        })
        .unwrap();
        let x = 2;
        let y = 2;
        let latitude_degrees =
            geometry.latitude_origin_degrees + geometry.latitude_spacing_degrees * y as f64;
        let expected = ertel_potential_vorticity_pvu(
            latitude_degrees,
            ErtelPvDerivatives {
                dtheta_dp,
                du_dp: 0.0,
                dv_dp,
                dtheta_dphi,
                dtheta_dlambda,
                dv_dlambda,
                d_u_cos_phi_dphi: 0.0,
            },
        )
        .unwrap();
        for level in 1..base_pressure.len() - 1 {
            let index = level * horizontal + y * geometry.nx + x;
            assert!(output.valid[index]);
            assert!(
                (output.values_pvu[index] - expected).abs() <= 5.0e-9 * expected.abs().max(1.0)
            );
        }
    }

    #[test]
    fn hybrid_column_view_interpolation_is_exact_without_allocation() {
        let geometry = test_geometry();
        let levels = 5;
        let horizontal = geometry.nx * geometry.ny;
        let count = levels * horizontal;
        let base_pressure = [20_000.0, 40_000.0, 60_000.0, 80_000.0, 100_000.0];
        let mut pressure = vec![0.0; count];
        let mut values = vec![0.0; count];
        let valid = vec![true; count];
        for (level, base_pressure_pa) in base_pressure.into_iter().enumerate() {
            for y in 0..geometry.ny {
                for x in 0..geometry.nx {
                    let index = level * horizontal + y * geometry.nx + x;
                    let pressure_pa = base_pressure_pa * (1.0 + 0.001 * (x + y) as f64);
                    pressure[index] = pressure_pa;
                    values[index] = quadratic(pressure_pa);
                }
            }
        }
        let request = NativePvGridRequest {
            geometry: &geometry,
            levels,
            pressure: NativePvPressure::FullGrid(NativePvField {
                values: &pressure,
                valid: &valid,
            }),
            temperature: NativePvField {
                values: &values,
                valid: &valid,
            },
            eastward_wind: NativePvField {
                values: &values,
                valid: &valid,
            },
            northward_wind: NativePvField {
                values: &values,
                valid: &valid,
            },
        };
        let target_pressure = pressure[2 * horizontal + 2 * geometry.nx + 2];
        let interpolated = sample_column_at_pressure(
            request,
            request.temperature,
            2,
            2,
            1,
            target_pressure,
            horizontal,
        )
        .unwrap()
        .unwrap();
        let expected = quadratic(target_pressure);
        assert!((interpolated - expected).abs() <= 8.0 * f64::EPSILON * expected.abs());

        let invalid_index = 2 * horizontal + 2 * geometry.nx + 1;
        let mut invalid_valid = valid.clone();
        invalid_valid[invalid_index] = false;
        let invalid_request = NativePvGridRequest {
            pressure: NativePvPressure::FullGrid(NativePvField {
                values: &pressure,
                valid: &invalid_valid,
            }),
            temperature: NativePvField {
                values: &values,
                valid: &invalid_valid,
            },
            ..request
        };
        assert_eq!(
            sample_column_at_pressure(
                invalid_request,
                invalid_request.temperature,
                2,
                2,
                1,
                target_pressure,
                horizontal,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn native_grid_rejects_missing_halo_and_nonmonotonic_pressure() {
        let mut geometry = test_geometry();
        geometry.halo_cells = 0;
        let levels = 3;
        let count = levels * geometry.nx * geometry.ny;
        let values = vec![280.0; count];
        let wind = vec![0.0; count];
        let valid = vec![true; count];
        let request = NativePvGridRequest {
            geometry: &geometry,
            levels,
            pressure: NativePvPressure::SharedLevels(&[20_000.0, 50_000.0, 90_000.0]),
            temperature: NativePvField {
                values: &values,
                valid: &valid,
            },
            eastward_wind: NativePvField {
                values: &wind,
                valid: &valid,
            },
            northward_wind: NativePvField {
                values: &wind,
                valid: &valid,
            },
        };
        assert_eq!(
            derive_native_ertel_pv(request),
            Err(PotentialVorticityError::MissingHorizontalHalo)
        );

        geometry.halo_cells = 1;
        let horizontal = geometry.nx * geometry.ny;
        let mut pressure = vec![0.0; count];
        for y in 0..geometry.ny {
            for x in 0..geometry.nx {
                let column = y * geometry.nx + x;
                pressure[column] = 20_000.0;
                pressure[horizontal + column] = 50_000.0;
                pressure[2 * horizontal + column] = 40_000.0;
            }
        }
        assert_eq!(
            derive_native_ertel_pv(NativePvGridRequest {
                geometry: &geometry,
                levels,
                pressure: NativePvPressure::FullGrid(NativePvField {
                    values: &pressure,
                    valid: &valid,
                }),
                temperature: NativePvField {
                    values: &values,
                    valid: &valid,
                },
                eastward_wind: NativePvField {
                    values: &wind,
                    valid: &valid,
                },
                northward_wind: NativePvField {
                    values: &wind,
                    valid: &valid,
                },
            }),
            Err(PotentialVorticityError::InvalidPressureColumn)
        );
    }

    #[test]
    fn field_deriver_publishes_structured_pvu_output_and_provenance() {
        let frame = pressure_test_frame();
        let output_key = FieldKey::Canonical(CanonicalField::PotentialVorticity);
        let derived = PotentialVorticityDeriver
            .derive(DeriveRequest {
                frame: &frame,
                outputs: std::slice::from_ref(&output_key),
            })
            .unwrap();
        assert_eq!(derived.len(), 1);
        let field = &derived[0];
        assert_eq!(field.key, output_key);
        assert_eq!(field.unit.symbol(), "PVU");
        assert_eq!(field.unit.scale_to_si(), crate::science::PVU_SCALE_TO_SI);
        assert_eq!(
            field.layout,
            ArrayLayout::Full3D {
                levels: 3,
                ny: 5,
                nx: 5,
            }
        );
        assert_eq!(
            field
                .validity
                .as_arc()
                .iter()
                .filter(|value| **value)
                .count(),
            27
        );
        assert!(
            field
                .values
                .iter()
                .zip(field.validity.as_arc().iter())
                .filter(|(_, valid)| **valid)
                .all(|(value, _)| value.abs() < 1.0e-10)
        );
        assert_eq!(field.quality, FieldQuality::Derived);
        assert_eq!(field.provenance.field, output_key);
        assert_eq!(
            field.provenance.transforms[0].operation,
            ERTEL_PV_SPHERICAL_ALGORITHM_ID
        );
    }
}
