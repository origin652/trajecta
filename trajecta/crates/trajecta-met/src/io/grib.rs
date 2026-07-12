//! # Contract: GRIB1/2 reader
//!
//! GRIB decoding preserves exact parameter-table identity, scan direction,
//! bitmap validity, and the complete ECMWF hybrid PV coefficient array. It
//! must distinguish data-layer subsets from the full vertical definition.

use std::path::Path;

use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, MetReader, SourceIndex, SourceMetadata,
};

/// Exact GRIB parameter, layer, and generating-process identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GribFieldIdentity {
    /// GRIB edition.
    pub edition: u8,
    /// Originating center.
    pub centre: u16,
    /// Parameter table or discipline/category tuple encoded as stable text.
    pub parameter: String,
    /// Exact level type.
    pub level_type: String,
    /// Exact level value or index.
    pub level: i32,
}

/// One indexed GRIB message location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GribIndexEntry {
    /// Exact field identity.
    pub identity: GribFieldIdentity,
    /// Byte offset of the message.
    pub offset: u64,
    /// Encoded message length.
    pub length: u64,
}

/// Immutable GRIB message index.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GribFileIndex {
    /// Indexed messages in file order.
    pub entries: Vec<GribIndexEntry>,
}

impl SourceIndex for GribFileIndex {
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Decoded regular latitude-longitude grid definition.
#[derive(Clone, Debug, PartialEq)]
pub struct GribGridDecoder {
    /// Number of x/longitude points.
    pub nx: usize,
    /// Number of y/latitude points.
    pub ny: usize,
    /// First longitude in degrees east.
    pub first_longitude_degrees: f64,
    /// First latitude in degrees north.
    pub first_latitude_degrees: f64,
    /// Positive longitude increment in degrees.
    pub longitude_increment_degrees: f64,
    /// Signed latitude increment in degrees.
    pub latitude_increment_degrees: f64,
}

/// Complete hybrid-interface A/B coefficients decoded from GRIB PV metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridPvDecoder {
    /// Interface A coefficients in pascals.
    pub a_half_pa: Vec<f64>,
    /// Dimensionless interface B coefficients.
    pub b_half: Vec<f64>,
}

/// Explicit GRIB missing-value bitmap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GribBitmap {
    /// True where the decoded source value is valid.
    pub valid: Vec<bool>,
}

/// GRIB1/2 implementation of the format-neutral reader contract.
#[derive(Clone, Copy, Debug, Default)]
pub struct GribReader;

impl MetReader for GribReader {
    fn inspect(&self, _file: &Path) -> Result<SourceMetadata, DecodeError> {
        Err(DecodeError::NotImplemented)
    }

    fn build_index(&self, _file: &Path) -> Result<Box<dyn SourceIndex>, DecodeError> {
        Err(DecodeError::NotImplemented)
    }

    fn decode(
        &self,
        _file: &Path,
        _index: &dyn SourceIndex,
        _request: &DecodeRequest,
    ) -> Result<DecodedField, DecodeError> {
        Err(DecodeError::NotImplemented)
    }
}
