//! # Contract: vertical-velocity derivation
//!
//! Converts supported pressure or hybrid-coordinate tendencies into geometric
//! vertical velocity using an explicit local-column convention.

use super::{DeriveError, DeriveRequest, DerivedField, FieldDeriver};

/// Pressure, hybrid, and geometric vertical-velocity deriver.
#[derive(Clone, Copy, Debug, Default)]
pub struct VerticalVelocityDeriver;

impl FieldDeriver for VerticalVelocityDeriver {
    fn derive(&self, _request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError> {
        Err(DeriveError::NotImplemented)
    }
}
