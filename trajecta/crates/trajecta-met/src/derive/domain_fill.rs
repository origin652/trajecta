//! # Contract: dry-air mass derivation
//!
//! Computes cell, column, and boundary-face dry-air mass needed by domain-fill
//! population strategies with an auditable geometric convention.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Dry-air grid and column mass deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct AirMassDeriver;

impl FieldDeriver for AirMassDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
