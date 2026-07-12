//! # Contract: prepared meteorological queries
//!
//! Queries use explicit preparation, one vertical-coordinate kind per batch,
//! structure-of-arrays input/output, and no hidden source I/O. Preparation may
//! mutate deterministic LRU state; execution uses only pinned immutable data.

pub mod cache;
pub mod engine;
pub mod interpolate;
pub mod layout;
pub mod output;
pub mod request;
