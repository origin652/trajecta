//! # Contract: format-neutral readers
//!
//! A reader separates cheap inspection, immutable indexing, and exact field
//! decoding. Source indexes are safe to share. Decode requests must identify a
//! field exactly; fuzzy matching is outside the contract.

use std::any::Any;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::lockfile::{GridSignature, VerticalSignature};
use trajecta_case::model::time::Timestamp;

use crate::frame::{ArrayLayout, TemporalSupport};
use crate::profile::graph::GraphUnit;
use crate::vertical::VerticalTopology;

/// Source container format.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Detects a supported meteorological container from its leading magic bytes.
///
/// Unknown regular files return `Ok(None)` so directory scans can summarize
/// sidecar checksums, README files, and indexes without treating them as broken
/// meteorology.
pub fn detect_source_format(file: &Path) -> Result<Option<SourceFormat>, DecodeError> {
    let mut source = fs::File::open(file).map_err(|error| DecodeError::Io {
        path: file.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut magic = [0_u8; 8];
    let length = source.read(&mut magic).map_err(|error| DecodeError::Io {
        path: file.to_path_buf(),
        message: error.to_string(),
    })?;
    let bytes = &magic[..length];
    if bytes.starts_with(b"GRIB") {
        if length < 8 {
            return Err(DecodeError::InvalidMetadata(
                "truncated GRIB indicator section".into(),
            ));
        }
        return match magic[7] {
            1 => Ok(Some(SourceFormat::Grib1)),
            2 => Ok(Some(SourceFormat::Grib2)),
            edition => Err(DecodeError::InvalidMetadata(format!(
                "unsupported GRIB edition {edition}"
            ))),
        };
    }
    if bytes.starts_with(b"CDF\x01")
        || bytes.starts_with(b"CDF\x02")
        || bytes.starts_with(b"CDF\x05")
    {
        return Ok(Some(SourceFormat::NetCdf3));
    }
    if bytes.starts_with(b"\x89HDF\r\n\x1a\n") {
        return Ok(Some(SourceFormat::NetCdf4));
    }
    Ok(None)
}

/// Lightweight metadata returned without decoding complete fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceMetadata {
    /// Local source path.
    pub path: PathBuf,
    /// Detected format.
    pub format: SourceFormat,
    /// Exact normalized attributes used by profile matching.
    pub attributes: BTreeMap<String, String>,
    /// Exact normalized dimension lengths used by Profile matching.
    pub dimensions: BTreeMap<String, usize>,
    /// Sorted unique physical validity times contained by the source.
    pub valid_times: Vec<Timestamp>,
    /// Stable logical roles contributed by this source container.
    pub roles: Vec<String>,
    /// Normalized horizontal-grid signature, when the file carries gridded data.
    pub grid: Option<GridSignature>,
    /// Native vertical signature, when the file defines vertical structure.
    pub vertical: Option<VerticalSignature>,
}

/// Exact reader-specific field selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodeRequest {
    /// Stable key/value identity defined by a dataset profile.
    pub source_identity: Vec<(String, String)>,
    /// Optional physical validity time.
    pub valid_time: Option<Timestamp>,
}

/// Domain-neutral, queryable source-grid geometry retained by an index.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceGridGeometry {
    /// First canonical x/longitude coordinate in degrees east.
    pub longitude_origin_degrees: f64,
    /// First canonical y/latitude coordinate in degrees north.
    pub latitude_origin_degrees: f64,
    /// Signed x/longitude spacing in degrees.
    pub longitude_spacing_degrees: f64,
    /// Signed y/latitude spacing in degrees.
    pub latitude_spacing_degrees: f64,
    /// Longitude or x point count.
    pub nx: usize,
    /// Latitude or y point count.
    pub ny: usize,
    /// Whether longitude wraps without a duplicated endpoint.
    pub periodic_longitude: bool,
}

/// Immutable message or variable index.
pub trait SourceIndex: Send + Sync {
    /// Number of indexed source records.
    fn len(&self) -> usize;

    /// Exposes the concrete immutable index to its owning reader backend.
    fn as_any(&self) -> &dyn Any;

    /// Returns normalized queryable grid geometry when the index provides it.
    fn grid_geometry(&self) -> Option<&SourceGridGeometry> {
        None
    }

    /// Returns complete native vertical topology and active data levels.
    fn vertical_topology(&self) -> Option<&VerticalTopology> {
        None
    }

    /// Returns the exact normalized grid signature retained by the index.
    fn grid_signature(&self) -> Option<&GridSignature> {
        None
    }

    /// Returns the exact normalized vertical signature retained by the index.
    fn vertical_signature(&self) -> Option<&VerticalSignature> {
        None
    }

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
    /// Parsed source unit before Profile conversion.
    pub source_unit: GraphUnit,
    /// Canonical decoded array layout.
    pub layout: ArrayLayout,
    /// Explicit source-time support.
    pub temporal: TemporalSupport,
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
    /// Constructs exactly the requested format/backend pair without fallback.
    pub fn create(
        format: SourceFormat,
        backend: MeteorologyReaderBackend,
    ) -> Result<Box<dyn MetReader>, DecodeError> {
        match format {
            SourceFormat::Grib1 | SourceFormat::Grib2 => {
                Ok(Box::new(crate::io::grib::GribReader::new(backend)))
            }
            SourceFormat::NetCdf3 | SourceFormat::NetCdf4 => {
                Ok(Box::new(crate::io::netcdf::NetCdfReader::new(backend)))
            }
        }
    }
}

/// Typed source inspection, indexing, or decoding failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The requested reader operation has not been implemented yet.
    NotImplemented,
    /// The explicitly requested implementation is not compiled or loadable.
    BackendUnavailable {
        /// Requested backend.
        backend: MeteorologyReaderBackend,
        /// Stable dependency or build explanation.
        message: String,
    },
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
