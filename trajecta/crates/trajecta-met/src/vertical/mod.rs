//! # Contract: native vertical coordinates and local columns
//!
//! Hybrid pressure and fixed pressure levels remain native. A query builds a
//! local curved column after horizontal interpolation, validates monotonicity,
//! and locates a vertical bracket without converting whole files to a legacy
//! intermediate representation.

use std::sync::Arc;

use crate::frame::RawMetFrame;
use crate::grid::CellId;

/// Native vertical-coordinate topology.
#[derive(Clone, Debug, PartialEq)]
pub enum VerticalTopology {
    /// Hybrid pressure using interface A/B coefficients.
    HybridPressure(HybridCoefficients),
    /// Fixed pressure levels.
    PressureLevels(PressureLevels),
}

/// Vertical staggering of an array.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VerticalStagger {
    /// Values at full model layers.
    Full,
    /// Values at layer interfaces.
    Interface,
}

/// Complete hybrid interface coefficients.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridCoefficients {
    /// Interface A coefficients in pascals.
    pub a_half_pa: Arc<[f64]>,
    /// Dimensionless interface B coefficients.
    pub b_half: Arc<[f64]>,
}

/// Ordered native pressure levels.
#[derive(Clone, Debug, PartialEq)]
pub struct PressureLevels {
    /// Pressure values in pascals.
    pub pressure_pa: Arc<[f64]>,
}

/// Per-level structural validity for one local column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerticalValidity {
    /// True where the complete interpolation support is physically valid.
    pub valid: Arc<[bool]>,
}

/// Query-ready local vertical column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnGeometry {
    /// Full-level pressure in pascals.
    pub pressure_pa: Arc<[f64]>,
    /// Full-level geometric height above mean sea level in metres.
    pub height_asl_m: Arc<[f64]>,
    /// Full-level air density in kilograms per cubic metre.
    pub density_kg_m3: Arc<[f64]>,
    /// Structural validity mask.
    pub validity: VerticalValidity,
}

/// Inputs required to construct one local column.
#[derive(Clone, Copy, Debug)]
pub struct ColumnRequest<'a> {
    /// Immutable source frame.
    pub frame: &'a RawMetFrame,
    /// Horizontal cell containing the query point.
    pub cell: CellId,
    /// Query longitude in degrees east.
    pub longitude_degrees: f64,
    /// Query latitude in degrees north.
    pub latitude_degrees: f64,
}

/// Format-neutral local-column builder.
pub trait ColumnBuilder: Send + Sync {
    /// Builds and validates one query-ready local column.
    fn build(&self, request: ColumnRequest<'_>) -> Result<ColumnGeometry, VerticalError>;
}

/// Hybrid-pressure local-column builder.
#[derive(Clone, Copy, Debug, Default)]
pub struct HybridColumnBuilder;

impl ColumnBuilder for HybridColumnBuilder {
    fn build(&self, _request: ColumnRequest<'_>) -> Result<ColumnGeometry, VerticalError> {
        Err(VerticalError::NotImplemented)
    }
}

/// Pressure-level local-column builder with structural underground masks.
#[derive(Clone, Copy, Debug, Default)]
pub struct PressureColumnBuilder;

impl ColumnBuilder for PressureColumnBuilder {
    fn build(&self, _request: ColumnRequest<'_>) -> Result<ColumnGeometry, VerticalError> {
        Err(VerticalError::NotImplemented)
    }
}

/// Located vertical support and deterministic interpolation weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerticalBracket {
    /// Lower full-level index.
    pub lower: usize,
    /// Upper full-level index.
    pub upper: usize,
    /// Weight assigned to the upper level.
    pub upper_weight: f64,
}

/// Vertical-column construction or lookup failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerticalError {
    /// Vertical algorithms have not been implemented yet.
    NotImplemented,
    /// Coefficient or level arrays have invalid lengths.
    InvalidTopology,
    /// Pressure or height is not monotonic where required.
    NonMonotonicColumn,
    /// Query point is below the physical surface.
    BelowGround,
    /// Query point is above the model top.
    AboveModelTop,
    /// No structurally valid pressure-level interpolation support exists.
    InvalidPressureSupport,
}
