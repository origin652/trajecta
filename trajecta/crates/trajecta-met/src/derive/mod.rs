//! # Contract: derived meteorological fields
//!
//! Derivers are deterministic, typed transformations with explicit source
//! fields, quality, and provenance. They do not perform source I/O and do not
//! mutate published raw frames.

use std::sync::Arc;

use crate::field::{FieldKey, FieldQuality};
use crate::frame::RawMetFrame;
use crate::provenance::ProvenanceRecord;

pub mod cloud;
pub mod domain_fill;
pub mod height;
pub mod precipitation;
pub mod pressure;
pub mod pv;
pub mod surface;
pub mod thermo;
pub mod vertical_velocity;

/// Immutable request passed to a field deriver.
#[derive(Clone, Copy, Debug)]
pub struct DeriveRequest<'a> {
    /// Published source frame.
    pub frame: &'a RawMetFrame,
    /// Exact requested output fields.
    pub outputs: &'a [FieldKey],
}

/// One derived output array and complete scientific provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedField {
    /// Output field identity.
    pub key: FieldKey,
    /// Flat canonical-order values.
    pub values: Arc<[f64]>,
    /// Derived or estimated quality.
    pub quality: FieldQuality,
    /// Exact input field identities.
    pub inputs: Vec<FieldKey>,
    /// Deduplicatable provenance record.
    pub provenance: ProvenanceRecord,
}

/// Deterministic family of scientific field derivations.
pub trait FieldDeriver: Send + Sync {
    /// Produces exactly the requested fields it supports.
    fn derive(&self, request: DeriveRequest<'_>) -> Result<Vec<DerivedField>, DeriveError>;
}

/// Derived-field failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeriveError {
    /// Scientific derivation has not been implemented yet.
    NotImplemented,
    /// A required source field is absent.
    MissingInput(FieldKey),
    /// Source arrays have incompatible shapes.
    ShapeMismatch,
    /// A numerical precondition is violated.
    InvalidPhysicalState(String),
}
