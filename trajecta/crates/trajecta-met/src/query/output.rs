//! # Contract: structure-of-arrays query output
//!
//! Floating-point columns contain scientific values only. Per-point status is
//! represented by a separate typed column; NaN, infinity, sentinel numbers,
//! and magic large values are never used as status channels.

use trajecta_case::model::meteorology::DomainId;

use crate::field::FieldKey;
use crate::io::inventory::LogicalFrameId;
use crate::provenance::ProvenanceId;

/// Recoverable status for one query point.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SampleStatus {
    /// Every requested field is valid.
    Ok,
    /// No domain safely covers the point.
    OutOfDomain,
    /// Requested AGL/ASL height is below the local surface.
    BelowGround,
    /// Requested height or pressure is above the native model top.
    AboveModelTop,
    /// Native vertical support is structurally invalid.
    InvalidVerticalColumn,
    /// One or both time endpoints lack required atmospheric data.
    MissingAtmosphericCoverage,
    /// Deterministic arithmetic failed a finite or monotonicity check.
    NumericalFailure,
}

/// One requested field as a contiguous SoA column.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldColumn {
    /// Field identity.
    pub field: FieldKey,
    /// One value per input point in caller order.
    pub values: Vec<f64>,
    /// One compact provenance identifier per point.
    pub provenance: Vec<ProvenanceId>,
}

/// One typed status per input point.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatusColumn {
    /// Status in caller order.
    pub values: Vec<SampleStatus>,
}

/// Optional explanation of domain, frames, weights, and provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct ExplainRecord {
    /// Selected domain.
    pub domain: DomainId,
    /// Earlier physical frame.
    pub before_frame: LogicalFrameId,
    /// Later physical frame.
    pub after_frame: LogicalFrameId,
    /// Earlier frame weight.
    pub before_weight: f64,
    /// Later frame weight.
    pub after_weight: f64,
    /// Field-level compact provenance identifiers.
    pub provenance: Vec<ProvenanceId>,
}

/// Complete SoA query result in original caller order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryOutput {
    /// Requested field columns.
    pub fields: Vec<FieldColumn>,
    /// Per-point typed status.
    pub status: StatusColumn,
    /// Optional per-point explanations when explicitly requested.
    pub explain: Option<Vec<ExplainRecord>>,
}

impl QueryOutput {
    /// Returns a read-only row view when the index is in range.
    #[must_use]
    pub fn row(&self, index: usize) -> Option<SampleView<'_>> {
        (index < self.status.values.len()).then_some(SampleView {
            output: self,
            index,
        })
    }
}

/// Read-only convenience view over one SoA output row.
#[derive(Clone, Copy, Debug)]
pub struct SampleView<'a> {
    output: &'a QueryOutput,
    index: usize,
}

impl SampleView<'_> {
    /// Returns the point status.
    #[must_use]
    pub fn status(self) -> SampleStatus {
        self.output.status.values[self.index]
    }

    /// Returns a field value by exact key when the column exists.
    #[must_use]
    pub fn value(self, field: &FieldKey) -> Option<f64> {
        self.output
            .fields
            .iter()
            .find(|column| &column.field == field)
            .and_then(|column| column.values.get(self.index).copied())
    }
}
