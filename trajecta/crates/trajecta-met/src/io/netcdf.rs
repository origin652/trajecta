//! # Contract: NetCDF3/4 and HDF5-backed reader
//!
//! CF coordinates, calendars, missing masks, formula terms, and multi-file
//! assembly are compared exactly. Inconsistent logical-frame components fail;
//! this module never silently resamples or guesses coordinate roles.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, MetReader, SourceIndex, SourceMetadata,
};

/// Indexed NetCDF variable definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfVariable {
    /// Exact variable name.
    pub name: String,
    /// Ordered dimension names.
    pub dimensions: Vec<String>,
    /// Exact normalized attributes.
    pub attributes: BTreeMap<String, String>,
}

/// Immutable NetCDF variable and dimension index.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetCdfVariableIndex {
    /// Indexed variables by exact name.
    pub variables: BTreeMap<String, NetCdfVariable>,
    /// Dimension lengths by exact name.
    pub dimensions: BTreeMap<String, usize>,
}

impl SourceIndex for NetCdfVariableIndex {
    fn len(&self) -> usize {
        self.variables.len()
    }
}

/// One logical meteorological frame assembled from exact file roles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfAssembly {
    /// Deterministic role-to-file mapping.
    pub files_by_role: BTreeMap<String, PathBuf>,
    /// Digest of all compared coordinate definitions.
    pub coordinate_signature: String,
}

/// Resolved CF axis and formula-term roles.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CfCoordinateResolver {
    /// Canonical role to exact variable name.
    pub axes: BTreeMap<String, String>,
    /// Formula-term role to exact variable name.
    pub formula_terms: BTreeMap<String, String>,
}

/// Explicit NetCDF missing-value mask.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfMissingMask {
    /// True where a decoded value is valid.
    pub valid: Vec<bool>,
}

/// Decoded CF time-axis definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfTimeAxis {
    /// Exact CF units string.
    pub units: String,
    /// Exact calendar name.
    pub calendar: String,
    /// Number of coordinate values.
    pub length: usize,
}

/// NetCDF3/4 implementation of the format-neutral reader contract.
#[derive(Clone, Copy, Debug, Default)]
pub struct NetCdfReader;

impl MetReader for NetCdfReader {
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
