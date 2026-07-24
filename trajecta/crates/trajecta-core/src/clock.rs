//! # Contract: signed simulation clock and event boundaries
//!
//! Forward and backward runs use the same physical instants and signed steps.
//! The step planner splits requested steps at meteorology-frame, release, and
//! output boundaries so no event is crossed implicitly.

use trajecta_case::model::time::{Direction, Timestamp};

/// Signed duration in nanoseconds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SignedDuration(pub i64);

impl SignedDuration {
    /// Zero duration.
    pub const ZERO: Self = Self(0);

    /// Returns the signed nanosecond representation.
    #[must_use]
    pub const fn as_nanoseconds(self) -> i64 {
        self.0
    }
}

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
    /// Exact dynamic population event such as a finite-domain inflow birth.
    Population {
        /// Event physical time.
        time: Timestamp,
        /// Stable population-local event description.
        event_id: String,
    },
    /// Final simulation time.
    End(Timestamp),
}

/// Deterministic splitter of requested steps at ordered event boundaries.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepPlanner;

impl StepPlanner {
    /// Splits a requested step at every event instant in the requested span.
    ///
    /// Duplicate event instants do not create zero-length steps. An `End`
    /// boundary is a hard cap, so a request that crosses it stops exactly at
    /// the simulation end.
    pub fn plan(
        clock: SimulationClock,
        requested: SignedDuration,
        boundaries: &[StepBoundary],
    ) -> Result<Vec<SignedDuration>, ClockError> {
        if requested == SignedDuration::ZERO {
            return Ok(Vec::new());
        }
        let sign_matches = match clock.direction {
            Direction::Forward => requested.0 > 0,
            Direction::Backward => requested.0 < 0,
        };
        if !sign_matches {
            return Err(ClockError::DirectionMismatch);
        }

        let current_ns = timestamp_nanoseconds(clock.current);
        let target_ns = current_ns
            .checked_add(i128::from(requested.0))
            .ok_or(ClockError::Overflow)?;
        validate_timestamp_range(target_ns)?;

        let mut previous_boundary_ns = current_ns;
        let mut segment_start_ns = current_ns;
        let mut steps = Vec::new();
        for boundary in boundaries {
            let boundary_ns = timestamp_nanoseconds(boundary.time());
            let ordered = match clock.direction {
                Direction::Forward => boundary_ns >= previous_boundary_ns,
                Direction::Backward => boundary_ns <= previous_boundary_ns,
            };
            if !ordered {
                return Err(ClockError::UnorderedBoundaries);
            }
            previous_boundary_ns = boundary_ns;

            if matches!(boundary, StepBoundary::End(_)) && boundary_ns == current_ns {
                return Ok(steps);
            }

            let ahead = match clock.direction {
                Direction::Forward => boundary_ns > current_ns,
                Direction::Backward => boundary_ns < current_ns,
            };
            if !ahead {
                continue;
            }
            let inside_request = match clock.direction {
                Direction::Forward => boundary_ns <= target_ns,
                Direction::Backward => boundary_ns >= target_ns,
            };
            if !inside_request {
                continue;
            }
            if boundary_ns != segment_start_ns {
                steps.push(duration_between(segment_start_ns, boundary_ns)?);
                segment_start_ns = boundary_ns;
            }
            if matches!(boundary, StepBoundary::End(_)) {
                return Ok(steps);
            }
        }

        if segment_start_ns != target_ns {
            steps.push(duration_between(segment_start_ns, target_ns)?);
        }
        Ok(steps)
    }
}

impl StepBoundary {
    /// Returns the exact physical event instant.
    #[must_use]
    pub const fn time(&self) -> Timestamp {
        match self {
            Self::MeteorologyFrame(time) | Self::End(time) => *time,
            Self::Release { time, .. }
            | Self::Output { time, .. }
            | Self::Population { time, .. } => *time,
        }
    }
}

/// Adds one signed nanosecond duration to a physical timestamp.
pub fn add_timestamp(
    timestamp: Timestamp,
    duration: SignedDuration,
) -> Result<Timestamp, ClockError> {
    let value = timestamp_nanoseconds(timestamp)
        .checked_add(i128::from(duration.0))
        .ok_or(ClockError::Overflow)?;
    validate_timestamp_range(value)?;
    let seconds =
        i64::try_from(value.div_euclid(1_000_000_000)).map_err(|_| ClockError::Overflow)?;
    let nanosecond =
        u32::try_from(value.rem_euclid(1_000_000_000)).map_err(|_| ClockError::Overflow)?;
    Timestamp::new(seconds, nanosecond).map_err(|_| ClockError::Overflow)
}

/// Returns the exact signed duration from `start` to `end`.
pub fn signed_duration_between(
    start: Timestamp,
    end: Timestamp,
) -> Result<SignedDuration, ClockError> {
    duration_between(timestamp_nanoseconds(start), timestamp_nanoseconds(end))
}

fn timestamp_nanoseconds(timestamp: Timestamp) -> i128 {
    i128::from(timestamp.seconds_since_unix_epoch()) * 1_000_000_000
        + i128::from(timestamp.nanosecond())
}

fn duration_between(start_ns: i128, end_ns: i128) -> Result<SignedDuration, ClockError> {
    let value = end_ns.checked_sub(start_ns).ok_or(ClockError::Overflow)?;
    i64::try_from(value)
        .map(SignedDuration)
        .map_err(|_| ClockError::Overflow)
}

fn validate_timestamp_range(nanoseconds: i128) -> Result<(), ClockError> {
    let seconds = nanoseconds.div_euclid(1_000_000_000);
    i64::try_from(seconds)
        .map(|_| ())
        .map_err(|_| ClockError::Overflow)
}

/// Clock or event-planning failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClockError {
    /// Signed step conflicts with configured direction.
    DirectionMismatch,
    /// Event boundaries are not ordered in the configured direction.
    UnorderedBoundaries,
    /// Timestamp arithmetic would overflow the contract representation.
    Overflow,
}

impl ClockError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DirectionMismatch => "clock.direction_mismatch",
            Self::UnorderedBoundaries => "clock.unordered_boundaries",
            Self::Overflow => "clock.overflow",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::new(seconds, 0).unwrap()
    }

    #[test]
    fn forward_and_backward_split_at_exact_events() {
        let forward = StepPlanner::plan(
            SimulationClock {
                current: timestamp(0),
                direction: Direction::Forward,
            },
            SignedDuration(10_000_000_000),
            &[
                StepBoundary::Release {
                    time: timestamp(3),
                    event_id: "r".into(),
                },
                StepBoundary::Output {
                    time: timestamp(7),
                    product_id: "o".into(),
                },
            ],
        )
        .unwrap();
        assert_eq!(
            forward,
            vec![
                SignedDuration(3_000_000_000),
                SignedDuration(4_000_000_000),
                SignedDuration(3_000_000_000)
            ]
        );

        let backward = StepPlanner::plan(
            SimulationClock {
                current: timestamp(10),
                direction: Direction::Backward,
            },
            SignedDuration(-10_000_000_000),
            &[
                StepBoundary::Output {
                    time: timestamp(7),
                    product_id: "o".into(),
                },
                StepBoundary::Release {
                    time: timestamp(3),
                    event_id: "r".into(),
                },
            ],
        )
        .unwrap();
        assert_eq!(
            backward,
            vec![
                SignedDuration(-3_000_000_000),
                SignedDuration(-4_000_000_000),
                SignedDuration(-3_000_000_000)
            ]
        );
    }

    #[test]
    fn duplicate_events_do_not_create_zero_steps_and_end_caps_request() {
        let steps = StepPlanner::plan(
            SimulationClock {
                current: timestamp(0),
                direction: Direction::Forward,
            },
            SignedDuration(20_000_000_000),
            &[
                StepBoundary::MeteorologyFrame(timestamp(5)),
                StepBoundary::Output {
                    time: timestamp(5),
                    product_id: "o".into(),
                },
                StepBoundary::End(timestamp(8)),
            ],
        )
        .unwrap();
        assert_eq!(
            steps,
            vec![SignedDuration(5_000_000_000), SignedDuration(3_000_000_000)]
        );

        let at_end = StepPlanner::plan(
            SimulationClock {
                current: timestamp(8),
                direction: Direction::Forward,
            },
            SignedDuration(1_000_000_000),
            &[StepBoundary::End(timestamp(8))],
        )
        .unwrap();
        assert!(at_end.is_empty());
    }

    #[test]
    fn rejects_direction_and_order_mismatch() {
        let clock = SimulationClock {
            current: timestamp(0),
            direction: Direction::Forward,
        };
        assert_eq!(
            StepPlanner::plan(clock, SignedDuration(-1), &[]),
            Err(ClockError::DirectionMismatch)
        );
        assert_eq!(
            StepPlanner::plan(
                clock,
                SignedDuration(10_000_000_000),
                &[
                    StepBoundary::MeteorologyFrame(timestamp(8)),
                    StepBoundary::MeteorologyFrame(timestamp(7)),
                ],
            ),
            Err(ClockError::UnorderedBoundaries)
        );
    }
}
