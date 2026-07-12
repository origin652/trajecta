//! # Contract: signed simulation clock and event boundaries
//!
//! Forward and backward runs use the same physical instants and signed steps.
//! The step planner splits requested steps at meteorology-frame, release, and
//! output boundaries so no event is crossed implicitly.

use trajecta_case::model::time::{Direction, Timestamp};

/// Signed duration in nanoseconds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SignedDuration(pub i64);

/// Current physical time and configured direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimulationClock {
    /// Current physical UTC instant.
    pub current: Timestamp,
    /// Integration direction.
    pub direction: Direction,
}

/// Event boundary that a numerical step may not cross.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepBoundary {
    /// Meteorology frame validity time.
    MeteorologyFrame(Timestamp),
    /// Scheduled particle release event.
    Release {
        /// Event time.
        time: Timestamp,
        /// Stable schedule event identifier.
        event_id: String,
    },
    /// Scheduled output event.
    Output {
        /// Event time.
        time: Timestamp,
        /// Stable output-product identifier.
        product_id: String,
    },
    /// Final simulation time.
    End(Timestamp),
}

/// Deterministic splitter of requested steps at ordered event boundaries.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepPlanner;

impl StepPlanner {
    /// Splits a requested step without changing its total signed duration.
    pub fn plan(
        _clock: SimulationClock,
        _requested: SignedDuration,
        _boundaries: &[StepBoundary],
    ) -> Result<Vec<SignedDuration>, ClockError> {
        Err(ClockError::NotImplemented)
    }
}

/// Clock or event-planning failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClockError {
    /// Step-planning algorithm has not been implemented yet.
    NotImplemented,
    /// Signed step conflicts with configured direction.
    DirectionMismatch,
    /// Event boundaries are not ordered in the configured direction.
    UnorderedBoundaries,
    /// Timestamp arithmetic would overflow the contract representation.
    Overflow,
}
