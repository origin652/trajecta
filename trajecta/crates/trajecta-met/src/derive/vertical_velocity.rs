//! # Contract: vertical-velocity derivation
//!
//! Converts supported pressure or hybrid-coordinate tendencies into geometric
//! vertical velocity using an explicit local-column convention.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Complete local kinematic terms for a moving native coordinate surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KinematicVerticalVelocityInput {
    /// Local layer-height time derivative in metres per second.
    pub height_time_derivative_m_s: f64,
    /// Eastward wind in metres per second.
    pub eastward_wind_m_s: f64,
    /// Northward wind in metres per second.
    pub northward_wind_m_s: f64,
    /// Eastward layer-height slope in metres per metre.
    pub height_eastward_gradient: f64,
    /// Northward layer-height slope in metres per metre.
    pub height_northward_gradient: f64,
    /// Native-coordinate material velocity.
    pub coordinate_velocity: f64,
    /// Layer-height derivative with respect to the native coordinate.
    pub height_coordinate_derivative: f64,
}

/// Evaluates `dz/dt + U dz/dx + V dz/dy + Cdot dz/dC` in fixed order.
pub fn geometric_vertical_velocity_m_s(
    input: KinematicVerticalVelocityInput,
) -> Result<f64, VerticalVelocityError> {
    let terms = [
        input.height_time_derivative_m_s,
        input.eastward_wind_m_s,
        input.northward_wind_m_s,
        input.height_eastward_gradient,
        input.height_northward_gradient,
        input.coordinate_velocity,
        input.height_coordinate_derivative,
    ];
    if terms.iter().any(|value| !value.is_finite()) {
        return Err(VerticalVelocityError::NonFiniteInput);
    }
    let eastward = input.eastward_wind_m_s * input.height_eastward_gradient;
    let northward = input.northward_wind_m_s * input.height_northward_gradient;
    let crossing = input.coordinate_velocity * input.height_coordinate_derivative;
    let result = ((input.height_time_derivative_m_s + eastward) + northward) + crossing;
    if !result.is_finite() {
        return Err(VerticalVelocityError::NumericalFailure);
    }
    Ok(result)
}

/// Derives native-coordinate velocity from pressure tendency on moving surfaces.
pub fn native_coordinate_velocity_from_omega(
    pressure_vertical_velocity_pa_s: f64,
    pressure_time_derivative_pa_s: f64,
    eastward_wind_m_s: f64,
    northward_wind_m_s: f64,
    pressure_eastward_gradient_pa_m: f64,
    pressure_northward_gradient_pa_m: f64,
    pressure_coordinate_derivative_pa: f64,
) -> Result<f64, VerticalVelocityError> {
    let values = [
        pressure_vertical_velocity_pa_s,
        pressure_time_derivative_pa_s,
        eastward_wind_m_s,
        northward_wind_m_s,
        pressure_eastward_gradient_pa_m,
        pressure_northward_gradient_pa_m,
        pressure_coordinate_derivative_pa,
    ];
    if values.iter().any(|value| !value.is_finite()) {
        return Err(VerticalVelocityError::NonFiniteInput);
    }
    if pressure_coordinate_derivative_pa == 0.0 {
        return Err(VerticalVelocityError::ZeroCoordinateDerivative);
    }
    let horizontal_pressure_tendency = eastward_wind_m_s.mul_add(
        pressure_eastward_gradient_pa_m,
        northward_wind_m_s * pressure_northward_gradient_pa_m,
    );
    let velocity = (pressure_vertical_velocity_pa_s
        - pressure_time_derivative_pa_s
        - horizontal_pressure_tendency)
        / pressure_coordinate_derivative_pa;
    if !velocity.is_finite() {
        return Err(VerticalVelocityError::NumericalFailure);
    }
    Ok(velocity)
}

/// Evaluates the terrain-following no-penetration boundary velocity.
pub fn terrain_following_surface_velocity_m_s(
    eastward_wind_m_s: f64,
    northward_wind_m_s: f64,
    terrain_eastward_gradient: f64,
    terrain_northward_gradient: f64,
) -> Result<f64, VerticalVelocityError> {
    let values = [
        eastward_wind_m_s,
        northward_wind_m_s,
        terrain_eastward_gradient,
        terrain_northward_gradient,
    ];
    if values.iter().any(|value| !value.is_finite()) {
        return Err(VerticalVelocityError::NonFiniteInput);
    }
    let velocity = eastward_wind_m_s.mul_add(
        terrain_eastward_gradient,
        northward_wind_m_s * terrain_northward_gradient,
    );
    if !velocity.is_finite() {
        return Err(VerticalVelocityError::NumericalFailure);
    }
    Ok(velocity)
}

/// Complete vertical-velocity formula failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalVelocityError {
    /// At least one input is NaN or infinite.
    NonFiniteInput,
    /// The pressure-to-native-coordinate Jacobian is zero.
    ZeroCoordinateDerivative,
    /// Arithmetic did not produce a finite result.
    NumericalFailure,
}

/// Pressure, hybrid, and geometric vertical-velocity deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct VerticalVelocityDeriver;

impl FieldDeriver for VerticalVelocityDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_sloping_surface_uses_every_kinematic_term() {
        let input = KinematicVerticalVelocityInput {
            height_time_derivative_m_s: 1.0,
            eastward_wind_m_s: 10.0,
            northward_wind_m_s: -4.0,
            height_eastward_gradient: 0.1,
            height_northward_gradient: 0.25,
            coordinate_velocity: 2.0,
            height_coordinate_derivative: 3.0,
        };
        assert_eq!(geometric_vertical_velocity_m_s(input), Ok(7.0));
    }

    #[test]
    fn omega_conversion_keeps_local_pressure_motion_and_slopes() {
        let coordinate_velocity =
            native_coordinate_velocity_from_omega(-2.0, 1.0, 10.0, -5.0, 0.2, 0.4, 100.0);
        assert_eq!(coordinate_velocity, Ok(-0.03));
        let surface =
            terrain_following_surface_velocity_m_s(10.0, 5.0, 0.1, -0.2).unwrap_or(f64::INFINITY);
        assert!(surface.abs() < 1.0e-15);
    }
}
