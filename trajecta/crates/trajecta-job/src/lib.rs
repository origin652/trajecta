//! # Contract: local job-control crate
//!
//! This crate owns typed control-plane contracts shared by the CLI, daemon,
//! and worker processes. It never implements meteorological, particle, or
//! output numerics. M5 implements only the local backend; the backend trait
//! preserves a future batch-scheduler boundary without dynamic plugins.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod backend;
pub mod catalog;
pub mod daemon;
pub mod history;
pub mod ipc;
pub mod model;
pub mod scheduler;

/// Version of the compiler-checked job-control contracts.
pub const CONTRACT_VERSION: u32 = 1;
