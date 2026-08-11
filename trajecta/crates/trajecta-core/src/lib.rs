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
pub mod lifecycle_clock;
pub mod manifest;
pub mod manifest_store;
pub mod output;
pub mod particle;
pub mod physics;
pub mod population;
pub mod reference;
pub mod release;
pub mod rng;
pub mod runner;
pub mod science;
pub mod synthetic;
pub mod verification;

/// Version of the compiler-checked public code contracts.
pub const CONTRACT_VERSION: u32 = 1;
