//! # Contract: typed CLI commands
//!
//! Each submodule converts one command family into calls to public MIT crate
//! APIs. Commands do not duplicate validation, decoding, interpolation, cache,
//! migration, or particle algorithms.

pub mod case;
pub mod data;
pub mod met;
pub mod run;

pub use case::CaseCommand;
pub use data::DataCommand;
pub use met::MetCommand;
pub use run::RunCommand;

/// Typed top-level command family.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// Case and RunProfile validation or resolution.
    Case(CaseCommand),
    /// Dataset lock and source inspection.
    Data(DataCommand),
    /// Meteorology probe or replay.
    Met(MetCommand),
    /// Minimal particle simulation.
    Run(RunCommand),
}
