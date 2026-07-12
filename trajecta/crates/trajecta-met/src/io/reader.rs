//! # Contract: format-neutral readers
//!
//! A reader separates cheap inspection, immutable indexing, and exact field
//! decoding. Source indexes are safe to share. Decode requests must identify a
//! field exactly; fuzzy matching is outside the contract.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use trajecta_case::model::time::Timestamp;
use trajecta_case::quantity::Unit;

use crate::field::FieldShape;

/// Source container format.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SourceFormat {
    /// GRIB edition 1.
    Grib1,
    /// GRIB edition 2.
    Grib2,
    /// Classic NetCDF version 3.
    NetCdf3,
    /// NetCDF version 4 backed by HDF5.
    NetCdf4,
}

/// Lightweight metadata returned without decoding complete fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceMetadata {
    /// Local source path.
    pub path: PathBuf,
    /// Detected format.
    pub format: SourceFormat,
    /// Exact normalized attributes used by profile matching.
    pub attributes: Vec<(String, String)>,
}

/// Exact reader-specific field selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodeRequest {
    /// Stable key/value identity defined by a dataset profile.
    pub source_identity: Vec<(String, String)>,
    /// Optional physical validity time.
    pub valid_time: Option<Timestamp>,
}

/// Immutable message or variable index.
pub trait SourceIndex: Send + Sync {
    /// Number of indexed source records.
    fn len(&self) -> usize;

    /// Returns whether no source records are indexed.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Normalized decoded field and source-level metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedField {
    /// Normalized flat field values in canonical scan order.
    pub values: Arc<[f64]>,
    /// True where a source value is valid.
    pub valid: Arc<[bool]>,
    /// Declared source unit before profile conversion.
    pub source_unit: Unit,
    /// Decoded array shape.
    pub shape: FieldShape,
    /// Exact source identity that was decoded.
    pub source_identity: Vec<(String, String)>,
}

/// Object-safe, format-neutral meteorological reader.
pub trait MetReader: Send + Sync {
    /// Inspects source metadata without decoding complete fields.
    fn inspect(&self, file: &Path) -> Result<SourceMetadata, DecodeError>;

    /// Builds an immutable exact-lookup index.
    fn build_index(&self, file: &Path) -> Result<Box<dyn SourceIndex>, DecodeError>;

    /// Decodes exactly one requested source field.
    fn decode(
        &self,
        file: &Path,
        index: &dyn SourceIndex,
        request: &DecodeRequest,
    ) -> Result<DecodedField, DecodeError>;
}

/// Selects a format reader from trusted lock/profile metadata.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReaderFactory;

impl ReaderFactory {
    /// Constructs the reader selected by an exact source format.
    pub fn create(_format: SourceFormat) -> Result<Box<dyn MetReader>, DecodeError> {
        Err(DecodeError::NotImplemented)
    }
}

/// Typed source inspection, indexing, or decoding failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The requested reader operation has not been implemented yet.
    NotImplemented,
    /// Source format is unsupported by the selected reader.
    UnsupportedFormat,
    /// Exact source identity is absent.
    MissingField,
    /// More than one record matches an exact identity.
    AmbiguousField,
    /// Source metadata or array shape is inconsistent.
    InvalidMetadata(String),
    /// Local file access failed.
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Stable, sanitized I/O message.
        message: String,
    },
}
