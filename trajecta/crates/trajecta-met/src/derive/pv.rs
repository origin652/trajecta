//! # Contract: potential-vorticity derivation
//!
//! Produces explicitly defined potential vorticity and required gradients on
//! the native grid before query interpolation.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Potential-vorticity and gradient deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct PotentialVorticityDeriver;

impl FieldDeriver for PotentialVorticityDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
