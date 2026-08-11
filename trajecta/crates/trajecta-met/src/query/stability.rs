//! # Contract: local dry static stability
//!
//! The sub-grid terrain correction consumes a Brunt-Vaisala frequency derived
//! from the complete local temperature, pressure, and geometric-height column.
//! Invalid columns remain typed failures; no missing layer is replaced by zero.

use crate::query::output::{SampleStatus, StatusColumn};

/// Stable identity of the M6-A2 dry-column stability calculation.
pub const DRY_BRUNT_VAISALA_ALGORITHM_ID: &str = "dry_potential_temperature_column_gradient/v1";

/// I/O-free local dry static stability returned in caller order.
#[derive(Clone, Debug, PartialEq)]
pub struct StabilityOutput {
    brunt_vaisala_frequency_squared_s2: Vec<f64>,
    status: StatusColumn,
}

impl StabilityOutput {
    pub(crate) fn new(
        values: Vec<f64>,
        status: StatusColumn,
    ) -> Result<Self, StabilityOutputError> {
        if values.len() != status.len() {
            return Err(StabilityOutputError::LengthMismatch);
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(StabilityOutputError::NonFiniteValue);
        }
        Ok(Self {
            brunt_vaisala_frequency_squared_s2: values,
            status,
        })
    }

    /// Returns typed sample status in caller order.
    #[must_use]
    pub const fn status(&self) -> &StatusColumn {
        &self.status
    }

    /// Returns local `N^2` only for a valid point.
    #[must_use]
    pub fn brunt_vaisala_frequency_squared_s2(&self, index: usize) -> Option<f64> {
        (self.status.get(index) == Some(SampleStatus::Ok))
            .then(|| self.brunt_vaisala_frequency_squared_s2.get(index).copied())
            .flatten()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StabilityOutputError {
    LengthMismatch,
    NonFiniteValue,
}
