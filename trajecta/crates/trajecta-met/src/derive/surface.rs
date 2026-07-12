//! # Contract: surface-field derivation
//!
//! Produces named surface diagnostics and only applies fallbacks explicitly
//! permitted by the active dataset profile.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Surface diagnostic and permitted-fallback deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceFieldDeriver;

impl FieldDeriver for SurfaceFieldDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
