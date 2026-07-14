//! # Contract: native vertical coordinates and local columns
//!
//! Hybrid pressure and fixed pressure levels remain native. A query builds a
//! local curved column after horizontal interpolation, validates monotonicity,
//! and locates a vertical bracket without converting whole files to a legacy
//! intermediate representation.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::frame::RawMetFrame;
use crate::grid::CellId;

/// Native vertical-coordinate topology.
#[derive(Clone, Debug, PartialEq)]
pub enum VerticalTopology {
    /// Hybrid pressure using interface A/B coefficients.
    HybridPressure(HybridPressureTopology),
    /// Fixed pressure levels.
    PressureLevels(PressureLevels),
}

/// Vertical staggering of an array.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Complete hybrid coordinate plus the full model layers carried by a frame.
///
/// GRIB files may retain the complete coordinate definition while carrying a
/// contiguous subset of model-level data. Model levels are one-based: full
/// level `n` lies between interface coefficient entries `n - 1` and `n`.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridPressureTopology {
    /// Complete interface coefficients for the native model coordinate.
    pub coefficients: HybridCoefficients,
    /// Strictly increasing, contiguous one-based full levels present in arrays.
    pub active_full_levels: Arc<[u16]>,
}

/// Canonical fixed pressure levels ordered from model top toward the surface.
#[derive(Clone, Debug, PartialEq)]
pub struct PressureLevels {
    /// Strictly increasing, finite, positive pressure values in pascals.
    pub pressure_pa: Arc<[f64]>,
}

impl PressureLevels {
    /// Creates a canonical pressure-level vector.
    pub fn new(pressure_pa: Arc<[f64]>) -> Result<Self, PressureLevelsError> {
        let levels = Self { pressure_pa };
        levels.validate()?;
        Ok(levels)
    }

    /// Validates the canonical top-to-bottom pressure ordering contract.
    pub fn validate(&self) -> Result<(), PressureLevelsError> {
        if self.pressure_pa.is_empty() {
            return Err(PressureLevelsError::Empty);
        }
        if self.pressure_pa.iter().any(|value| !value.is_finite()) {
            return Err(PressureLevelsError::NonFinite);
        }
        if self.pressure_pa.iter().any(|value| *value <= 0.0) {
            return Err(PressureLevelsError::NonPositive);
        }
        if self
            .pressure_pa
            .windows(2)
            .any(|levels| levels[1] <= levels[0])
        {
            return Err(PressureLevelsError::NotStrictlyIncreasing);
        }
        Ok(())
    }
}

/// Invalid canonical pressure-level topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PressureLevelsError {
    /// No pressure levels were supplied.
    Empty,
    /// At least one pressure value is NaN or infinite.
    NonFinite,
    /// At least one pressure value is zero or negative.
    NonPositive,
    /// Pressure does not increase strictly from model top toward the surface.
    NotStrictlyIncreasing,
}

impl std::fmt::Display for PressureLevelsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("pressure-level topology is empty"),
            Self::NonFinite => formatter.write_str("pressure-level topology is not finite"),
            Self::NonPositive => formatter.write_str("pressure levels must be positive"),
            Self::NotStrictlyIncreasing => formatter.write_str(
                "pressure levels must be strictly increasing in Pa from model top to surface",
            ),
        }
    }
}

impl std::error::Error for PressureLevelsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_levels_enforce_top_to_surface_order() {
        assert!(PressureLevels::new(Arc::from([50_000.0, 100_000.0])).is_ok());
        assert_eq!(
            PressureLevels::new(Arc::from([100_000.0, 50_000.0])).err(),
            Some(PressureLevelsError::NotStrictlyIncreasing)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([0.0, 50_000.0])).err(),
            Some(PressureLevelsError::NonPositive)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([])).err(),
            Some(PressureLevelsError::Empty)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([50_000.0, f64::NAN])).err(),
            Some(PressureLevelsError::NonFinite)
        );
        assert_eq!(
            PressureLevels::new(Arc::from([50_000.0, 50_000.0])).err(),
            Some(PressureLevelsError::NotStrictlyIncreasing)
        );
    }
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
