//! # Contract: thermodynamic derivation
//!
//! Produces declared temperature, humidity, virtual temperature, and density
//! quantities with explicit phase and moisture conventions.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Thermodynamic field deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThermodynamicDeriver;

impl FieldDeriver for ThermodynamicDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
