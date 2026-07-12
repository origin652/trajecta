//! # Contract: particle simulation crate
//!
//! This crate owns clocks, particle state, integration, boundary decisions,
//! population lifecycles, releases, outputs, manifests, and run orchestration.
//! It consumes meteorology only through `trajecta-met` public contracts.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod boundary;
pub mod clock;
pub mod integrator;
pub mod manifest;
pub mod output;
pub mod particle;
pub mod population;
pub mod release;
pub mod rng;
pub mod runner;

/// Version of the compiler-checked public code contracts.
pub const CONTRACT_VERSION: u32 = 0;
