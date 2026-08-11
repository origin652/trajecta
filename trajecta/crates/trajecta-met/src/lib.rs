//! # Contract: meteorology crate
//!
//! This crate owns source interpretation, native grids and vertical
//! coordinates, derivation, interpolation, query preparation, and immutable
//! meteorology outputs. Query execution never performs hidden file I/O.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod auxiliary;
pub mod data_provider;
pub mod derive;
pub mod field;
pub mod frame;
pub mod grid;
pub mod io;
pub mod performance;
pub mod profile;
pub mod provenance;
pub mod query;
pub mod science;
pub mod surface_layer;
pub mod validation;
pub mod vertical;

/// Version of the compiler-checked public code contracts.
pub const CONTRACT_VERSION: u32 = 0;
