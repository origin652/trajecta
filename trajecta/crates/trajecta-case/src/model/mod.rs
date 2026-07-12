//! # Contract: Case science model
//!
//! These modules describe scientific intent only. Machine paths, thread
//! counts, caches, and credentials belong to [`crate::document::RunProfileDocument`].
//!
//! ## Modules
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`metadata`] | Descriptive labels |
//! | [`time`] | UTC range and direction |
//! | [`meteorology`] | Logical domains and datasets |
//! | [`population`] | Particle lifecycle strategy |
//! | [`substance`] | Named species |
//! | [`numerics`] | Integrator and boundaries |
//! | [`physics`] | Optional physics modules |
//! | [`output`] | Output products |

pub mod metadata;
pub mod meteorology;
pub mod numerics;
pub mod output;
pub mod physics;
pub mod population;
pub mod substance;
pub mod time;
