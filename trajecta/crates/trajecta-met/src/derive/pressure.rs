//! # Contract: pressure derivation
//!
//! Builds hybrid interface and full-level pressure from complete A/B
//! coefficients and local surface pressure without truncating PV metadata.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Hybrid and fixed-level pressure deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct PressureDeriver;

impl FieldDeriver for PressureDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
