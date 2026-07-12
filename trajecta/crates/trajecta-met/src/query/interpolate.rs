//! # Contract: field-specific interpolation policies
//!
//! Canonical descriptors choose policies. Continuous scalars, categories,
//! spherical vectors, pressure-level masks, vertical coordinates, and temporal
//! semantics are distinct contracts and cannot be silently interchanged.

use crate::field::FieldDescriptor;

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
        _descriptor: &FieldDescriptor,
        _input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError> {
        Err(InterpolationError::NotImplemented)
    }
}

/// Categorical or nearest-neighbor interpolation policy.
#[derive(Clone, Copy, Debug, Default)]
pub struct CategoricalPolicy;

impl InterpolationPolicy for CategoricalPolicy {
    fn interpolate(
        &self,
        _descriptor: &FieldDescriptor,
        _input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError> {
        Err(InterpolationError::NotImplemented)
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
        Err(InterpolationError::NotImplemented)
    }
}

/// Mask-aware valid-triangle interpolation for pressure-level terrain.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaskedTrianglePolicy;

impl InterpolationPolicy for MaskedTrianglePolicy {
    fn interpolate(
        &self,
        _descriptor: &FieldDescriptor,
        _input: ScalarInterpolationInput<'_>,
    ) -> Result<InterpolatedValue, InterpolationError> {
        Err(InterpolationError::NotImplemented)
    }
}

/// Explicit vertical interpolation mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalPolicy {
    /// Linear in geometric height.
    LinearHeight,
    /// Linear in logarithmic pressure.
    LogPressure,
    /// Nearest valid native level.
    NearestLevel,
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
    /// Policy implementation has not been written yet.
    NotImplemented,
    /// Descriptor and selected policy are incompatible.
    WrongPolicy,
    /// No physically valid interpolation support exists.
    InvalidSupport,
    /// A numerical precondition failed.
    NumericalFailure,
}
