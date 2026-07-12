//! # Contract: cloud-field derivation
//!
//! Produces declared cloud water, ice, and cloud-boundary diagnostics without
//! inventing phase assumptions outside the selected profile.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Cloud water and boundary deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct CloudDeriver;

impl FieldDeriver for CloudDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
