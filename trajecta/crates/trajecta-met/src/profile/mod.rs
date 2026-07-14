//! # Contract: dataset interpretation profiles
//!
//! Profiles are declarative data. They identify sources exactly, map source
//! fields to canonical fields, and compile a whitelisted computation graph.
//! They cannot execute arbitrary code or perform file/network I/O.

pub mod catalog;
pub mod document;
pub mod expression;
pub mod graph;
