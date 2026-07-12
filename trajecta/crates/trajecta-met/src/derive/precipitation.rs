//! # Contract: accumulation derivation
//!
//! Converts accumulated fields to interval quantities and rates while keeping
//! interval bounds and reset behavior in provenance.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Accumulation de-accumulation and rate deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct AccumulationDeriver;

impl FieldDeriver for AccumulationDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
