//! # Contract: meteorological source I/O
//!
//! Readers perform exact source lookup, decoding, missing-mask recovery, scan
//! normalization, and source metadata reporting. They do not infer Case
//! semantics, choose fallbacks, interpolate particles, or perform network I/O.

pub mod grib;
pub mod inventory;
pub mod netcdf;
pub mod reader;
