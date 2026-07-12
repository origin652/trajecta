//! # Contract: hydrostatic height derivation
//!
//! Integrates geometric or geopotential height with declared gravity and
//! virtual-temperature conventions, validating column monotonicity.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Hydrostatic height and geopotential deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct HydrostaticHeightDeriver;

impl FieldDeriver for HydrostaticHeightDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
