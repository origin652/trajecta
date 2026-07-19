//! # Contract: meteorological source I/O
//!
//! Readers perform exact source lookup, decoding, missing-mask recovery, scan
//! normalization, and source metadata reporting. They do not infer Case
//! semantics, choose fallbacks, interpolate particles, or perform network I/O.

pub mod counting_reader;
pub mod frame_loader;
pub mod grib;
pub mod inventory;
pub mod lock_builder;
pub mod metrics;
pub mod netcdf;
mod netcdf4_rust;
#[cfg(feature = "native-netcdf")]
mod netcdf_native;
pub mod reader;
