//! # Contract: typed CLI commands
//!
//! Each submodule converts one command family into calls to public MIT crate
//! APIs. Commands do not duplicate validation, decoding, interpolation, cache,
//! migration, or particle algorithms.

pub mod case;
pub mod config;
pub mod data;
pub mod doctor;
pub mod met;
pub mod project;
pub mod run;
pub mod staged;

pub use case::CaseCommand;
pub use config::ConfigCommand;
pub use data::DataCommand;
pub use doctor::DoctorCommand;
pub use met::MetCommand;
pub use project::ProjectCommand;
pub use run::RunCommand;
pub use staged::{JobCommand, ResultCommand, StagedRunCommand};

/// Typed top-level command family.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// Local machine configuration.
    Config(ConfigCommand),
    /// Progressive local project editing.
    Project(ProjectCommand),
    /// Case and RunProfile validation or resolution.
    Case(CaseCommand),
    /// Dataset lock and source inspection.
    Data(DataCommand),
    /// Meteorology probe or replay.
    Met(MetCommand),
    /// Local A1 readiness inspection.
    Doctor(DoctorCommand),
    /// Legacy minimal particle command form retained for compatibility.
    Run(RunCommand),
    /// Fully parsed run command awaiting the M5-A2 daemon.
    StagedRun(StagedRunCommand),
    /// Fully parsed job command awaiting M5-A2/A3.
    Job(JobCommand),
    /// Fully parsed result command awaiting M5-A3.
    Result(ResultCommand),
}
