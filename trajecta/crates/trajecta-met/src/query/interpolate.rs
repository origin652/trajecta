//! # Contract: field-specific interpolation policies
//!
//! Canonical descriptors choose policies. Continuous scalars, categories,
//! spherical vectors, pressure-level masks, vertical coordinates, and temporal
//! semantics are distinct contracts and cannot be silently interchanged.

use crate::field::FieldDescriptor;
use crate::grid::{GridError, interpolate_spherical_vector};

/// Scalar interpolation inputs for one point and one time endpoint.
#[derive(Clone, Copy, Debug)]
pub struct ScalarInterpolationInput<'a> {
    /// Four horizontal corner values.
    pub values: &'a [f64; 4],
    /// Four horizontal weights.
    pub weights: &'a [f64; 4],
    /// Structural validity mask.
    pub valid: &'a [bool; 4],
}

/// Result produced by a field-specific interpolation policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InterpolatedValue {
    /// One scalar result.
    Scalar(f64),
    /// Eastward and northward tangent-vector result.
    HorizontalVector {
        /// Eastward component.
        eastward: f64,
        /// Northward component.
        northward: f64,
    },
}

/// Field-specific horizontal interpolation contract.
pub trait InterpolationPolicy: Send + Sync {
    /// Evaluates one point without source I/O or shared mutable state.
    fn interpolate(
        &self,
        descriptor: &FieldDescriptor,
        input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError>;
}

/// Continuous scalar interpolation policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScalarLinearPolicy;

impl InterpolationPolicy for ScalarLinearPolicy {
    fn interpolate(
        &self,
        descriptor: &FieldDescriptor,
        input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError> {
        if descriptor.interpolation != crate::field::InterpolationKind::ScalarLinear {
            return Err(InterpolationError::WrongPolicy);
        }
        validate_scalar_input(input)?;
        if input.valid.iter().any(|valid| !valid) {
            return Err(InterpolationError::InvalidSupport);
        }
        Ok(InterpolatedValue::Scalar(weighted_sum(
            input.values,
            input.weights,
        )?))
    }
}

/// Categorical or nearest-neighbor interpolation policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct CategoricalPolicy;

impl InterpolationPolicy for CategoricalPolicy {
    fn interpolate(
        &self,
        descriptor: &FieldDescriptor,
        input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError> {
        if descriptor.interpolation != crate::field::InterpolationKind::Categorical {
            return Err(InterpolationError::WrongPolicy);
        }
        validate_scalar_input(input)?;
        let selected = (0..4)
            .filter(|index| input.valid[*index])
            .max_by(|left, right| {
                input.weights[*left]
                    .total_cmp(&input.weights[*right])
                    .then_with(|| right.cmp(left))
            })
            .ok_or(InterpolationError::InvalidSupport)?;
        Ok(InterpolatedValue::Scalar(input.values[selected]))
    }
}

/// Earth-centered tangent-vector wind interpolation policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct SphericalVectorPolicy;

impl InterpolationPolicy for SphericalVectorPolicy {
    fn interpolate(
        &self,
        _descriptor: &FieldDescriptor,
        _input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError> {
        Err(InterpolationError::WrongPolicy)
    }
}

/// Complete input for Earth-centred tangent-vector wind interpolation.
#[derive(Clone, Copy, Debug)]
pub struct SphericalVectorInterpolationInput {
    /// Source longitudes in horizontal-corner order.
    pub source_longitude_degrees: [f64; 4],
    /// Source latitudes in horizontal-corner order.
    pub source_latitude_degrees: [f64; 4],
    /// Eastward source components.
    pub eastward_m_s: [f64; 4],
    /// Northward source components.
    pub northward_m_s: [f64; 4],
    /// Horizontal interpolation weights.
    pub weights: [f64; 4],
    /// Query longitude.
    pub query_longitude_degrees: f64,
    /// Query latitude.
    pub query_latitude_degrees: f64,
}

impl SphericalVectorPolicy {
    /// Interpolates paired wind components using the frozen spherical policy.
    pub fn interpolate_vector(
        &self,
        input: SphericalVectorInterpolationInput,
    ) -> Result<InterpolatedValue, InterpolationError> {
        let (eastward, northward) = interpolate_spherical_vector(
            input.source_longitude_degrees,
            input.source_latitude_degrees,
            input.eastward_m_s,
            input.northward_m_s,
            input.weights,
            input.query_longitude_degrees,
            input.query_latitude_degrees,
        )
        .map_err(map_grid_error)?;
        Ok(InterpolatedValue::HorizontalVector {
            eastward,
            northward,
        })
    }
}

/// Mask-aware valid-triangle interpolation for pressure-level terrain.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaskedTrianglePolicy;

impl InterpolationPolicy for MaskedTrianglePolicy {
    fn interpolate(
        &self,
        descriptor: &FieldDescriptor,
        input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError> {
        if descriptor.interpolation != crate::field::InterpolationKind::MaskedTriangle {
            return Err(InterpolationError::WrongPolicy);
        }
        validate_scalar_input(input)?;
        let valid_count = input.valid.iter().filter(|valid| **valid).count();
        if valid_count == 4 {
            return Ok(InterpolatedValue::Scalar(weighted_sum(
                input.values,
                input.weights,
            )?));
        }
        if valid_count != 3 {
            return Err(InterpolationError::InvalidSupport);
        }
        let fx = input.weights[1] + input.weights[3];
        let fy = input.weights[2] + input.weights[3];
        let tolerance = 64.0 * f64::EPSILON;
        let missing = input
            .valid
            .iter()
            .position(|valid| !valid)
            .ok_or(InterpolationError::InvalidSupport)?;
        let triangle_weights = match missing {
            0 if fx + fy >= 1.0 - tolerance => [0.0, 1.0 - fy, 1.0 - fx, fx + fy - 1.0],
            1 if fy >= fx - tolerance => [1.0 - fy, 0.0, fy - fx, fx],
            2 if fx >= fy - tolerance => [1.0 - fx, fx - fy, 0.0, fy],
            3 if fx + fy <= 1.0 + tolerance => [1.0 - fx - fy, fx, fy, 0.0],
            _ => return Err(InterpolationError::InvalidSupport),
        };
        Ok(InterpolatedValue::Scalar(weighted_sum(
            input.values,
            &triangle_weights,
        )?))
    }
}

/// Explicit vertical interpolation mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalPolicy {
    /// Linear in geometric height.
    LinearHeight,
    /// Linear in logarithmic pressure.
    LogPressure,
}

/// Explicit temporal interpolation mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimePolicy {
    /// Linear interpolation of instantaneous values.
    InstantaneousLinear,
    /// Piecewise constant interval-average rate.
    IntervalRate,
    /// Discrete state selected by a declared tie rule.
    Discrete,
}

/// Interpolation failure for one point.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterpolationError {
    /// Descriptor and selected policy are incompatible.
    WrongPolicy,
    /// No physically valid interpolation support exists.
    InvalidSupport,
    /// A numerical precondition failed.
    NumericalFailure,
    /// Exact pole has no unique local east direction.
    PolarSingularity,
}

fn validate_scalar_input(input: ScalarInterpolationInput<'_>) -> Result<(), InterpolationError> {
    if input.values.iter().any(|value| !value.is_finite())
        || input
            .weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight < 0.0)
    {
        return Err(InterpolationError::NumericalFailure);
    }
    let sum = input.weights.iter().sum::<f64>();
    if (sum - 1.0).abs() > 1.0e-12 {
        return Err(InterpolationError::NumericalFailure);
    }
    Ok(())
}

fn weighted_sum(values: &[f64; 4], weights: &[f64; 4]) -> Result<f64, InterpolationError> {
    let value = values[0].mul_add(
        weights[0],
        values[1].mul_add(
            weights[1],
            values[2].mul_add(weights[2], values[3] * weights[3]),
        ),
    );
    if !value.is_finite() {
        return Err(InterpolationError::NumericalFailure);
    }
    Ok(value)
}

fn map_grid_error(error: GridError) -> InterpolationError {
    match error {
        GridError::PolarSingularity => InterpolationError::PolarSingularity,
        GridError::InvalidWeights | GridError::InvalidCoordinate | GridError::NumericalFailure => {
            InterpolationError::NumericalFailure
        }
        GridError::OutOfDomain | GridError::InvalidGeometry(_) | GridError::IndexOverflow => {
            InterpolationError::InvalidSupport
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use trajecta_case::quantity::{Dimension, Unit};

    use super::*;
    use crate::field::{Capability, FieldKey, FieldQuality, FieldShape, InterpolationKind};

    fn descriptor(interpolation: InterpolationKind) -> FieldDescriptor {
        FieldDescriptor {
            key: FieldKey::Extension(crate::field::ExtensionFieldId {
                namespace: "test".into(),
                name: "scalar".into(),
            }),
            unit: Unit::new("1", Dimension::Dimensionless, 1.0, 0.0).unwrap(),
            shape: FieldShape::Horizontal2D,
            vertical_stagger: None,
            quality: FieldQuality::Source,
            required_capability: Capability::Diagnostics,
            interpolation,
        }
    }

    #[test]
    fn masked_triangle_uses_plane_weights_and_refuses_outside_point() {
        let policy = MaskedTrianglePolicy;
        let values = [0.0, 1.0, 2.0, 3.0];
        let inside_weights = [0.5, 0.25, 0.25, 0.0];
        let inside = policy
            .interpolate(
                &descriptor(InterpolationKind::MaskedTriangle),
                ScalarInterpolationInput {
                    values: &values,
                    weights: &inside_weights,
                    valid: &[true, true, true, false],
                },
            )
            .unwrap();
        assert_eq!(inside, InterpolatedValue::Scalar(0.75));

        let outside_weights = [0.0625, 0.1875, 0.1875, 0.5625];
        assert_eq!(
            policy.interpolate(
                &descriptor(InterpolationKind::MaskedTriangle),
                ScalarInterpolationInput {
                    values: &values,
                    weights: &outside_weights,
                    valid: &[true, true, true, false],
                },
            ),
            Err(InterpolationError::InvalidSupport)
        );
    }

    #[test]
    fn categorical_ties_choose_lowest_stable_corner() {
        let policy = CategoricalPolicy;
        let values = [10.0, 20.0, 30.0, 40.0];
        let weights = [0.4, 0.4, 0.1, 0.1];
        let result = policy
            .interpolate(
                &descriptor(InterpolationKind::Categorical),
                ScalarInterpolationInput {
                    values: &values,
                    weights: &weights,
                    valid: &[true; 4],
                },
            )
            .unwrap();
        assert_eq!(result, InterpolatedValue::Scalar(10.0));
    }
}
