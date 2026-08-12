//! # Contract: complete thermodynamic columns for deep convection
//!
//! Each successful row contains one physically valid, top-to-surface local
//! column at the exact query time. Missing or discontinuous vertical support
//! remains a typed status; no layer is synthesized or filled with zero.

use crate::query::output::{SampleStatus, StatusColumn};
use crate::vertical::ColumnGeometry;

/// I/O-free complete columns returned in caller order.
#[derive(Clone, Debug, PartialEq)]
pub struct ConvectionColumnOutput {
    columns: Vec<Option<ColumnGeometry>>,
    status: StatusColumn,
}

impl ConvectionColumnOutput {
    pub(crate) fn new(
        columns: Vec<Option<ColumnGeometry>>,
        status: StatusColumn,
    ) -> Result<Self, ConvectionColumnOutputError> {
        if columns.len() != status.len()
            || columns
                .iter()
                .zip(status.values())
                .any(|(column, status)| column.is_some() != (*status == SampleStatus::Ok))
        {
            return Err(ConvectionColumnOutputError::InvalidRows);
        }
        Ok(Self { columns, status })
    }

    /// Returns typed sample status in caller order.
    #[must_use]
    pub const fn status(&self) -> &StatusColumn {
        &self.status
    }

    /// Returns a complete column only when the corresponding status is `Ok`.
    #[must_use]
    pub fn column(&self, index: usize) -> Option<&ColumnGeometry> {
        self.columns.get(index).and_then(Option::as_ref)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConvectionColumnOutputError {
    InvalidRows,
}
