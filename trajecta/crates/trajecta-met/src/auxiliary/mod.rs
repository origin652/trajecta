//! # Contract: time-invariant auxiliary datasets
//!
//! Auxiliary datasets have explicit immutable identities, remain separate
//! from time-varying meteorology, and are opened only from verified locks.
//! Physical modules receive read-only handles and perform no implicit fetches.

pub mod gmted2010;
