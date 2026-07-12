//! # Contract: simulation time
//!
//! Runtime instants are UTC Unix timestamps with nanosecond precision.
//! Direction is explicit and does not alter the meteorological value at an
//! identical physical instant.
//!
//! ## Fields
//!
//! | Type | Fields |
//! |---|---|
//! | [`Timestamp`] | `seconds_since_unix_epoch`, `nanosecond` |
//! | [`TimeSpec`] | `start`, `end`, `direction` |
//!
//! Deserialization always goes through [`Timestamp::new`]; invalid nanoseconds
//! cannot bypass the constructor.

use serde::de::{self, Deserializer};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt;

/// UTC instant represented without a timezone-dependent library type.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Timestamp {
    seconds_since_unix_epoch: i64,
    nanosecond: u32,
}

impl Timestamp {
    /// Unix epoch.
    pub const UNIX_EPOCH: Self = Self {
        seconds_since_unix_epoch: 0,
        nanosecond: 0,
    };

    /// Creates a normalized UTC instant.
    pub const fn new(
        seconds_since_unix_epoch: i64,
        nanosecond: u32,
    ) -> Result<Self, TimestampError> {
        if nanosecond >= 1_000_000_000 {
            return Err(TimestampError::InvalidNanosecond(nanosecond));
        }
        Ok(Self {
            seconds_since_unix_epoch,
            nanosecond,
        })
    }

    /// Returns whole seconds since 1970-01-01T00:00:00Z.
    #[must_use]
    pub const fn seconds_since_unix_epoch(self) -> i64 {
        self.seconds_since_unix_epoch
    }

    /// Returns the sub-second nanosecond component.
    #[must_use]
    pub const fn nanosecond(self) -> u32 {
        self.nanosecond
    }
}

impl Serialize for Timestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Timestamp", 2)?;
        state.serialize_field("seconds_since_unix_epoch", &self.seconds_since_unix_epoch)?;
        state.serialize_field("nanosecond", &self.nanosecond)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct TimestampWire {
            seconds_since_unix_epoch: i64,
            nanosecond: u32,
        }

        let wire = TimestampWire::deserialize(deserializer)?;
        Timestamp::new(wire.seconds_since_unix_epoch, wire.nanosecond).map_err(de::Error::custom)
    }
}

/// Timestamp construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimestampError {
    /// Nanoseconds must be below one billion.
    InvalidNanosecond(u32),
}

impl fmt::Display for TimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNanosecond(ns) => {
                write!(f, "nanosecond out of range: {ns}")
            }
        }
    }
}

impl std::error::Error for TimestampError {}

/// Direction in which a simulation clock advances.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Increasing UTC time.
    Forward,
    /// Decreasing UTC time.
    Backward,
}

/// Inclusive simulation time range and explicit integration direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeSpec {
    /// First physical instant visited by the run.
    pub start: Timestamp,
    /// Final physical instant visited by the run.
    pub end: Timestamp,
    /// Clock direction.
    pub direction: Direction,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn rejects_invalid_nanosecond_constructor() {
        assert!(Timestamp::new(0, 1_000_000_000).is_err());
        assert_eq!(Timestamp::new(1, 2).unwrap().nanosecond(), 2);
    }

    #[test]
    fn rejects_invalid_nanosecond_yaml_and_json() {
        let yaml = "seconds_since_unix_epoch: 0\nnanosecond: 1000000000\n";
        let err = serde_yml::from_str::<Timestamp>(yaml).unwrap_err();
        assert!(err.to_string().contains("nanosecond"));

        let json = r#"{"seconds_since_unix_epoch":0,"nanosecond":1000000000}"#;
        let err = serde_json::from_str::<Timestamp>(json).unwrap_err();
        assert!(err.to_string().contains("nanosecond"));
    }

    #[test]
    fn rejects_unknown_timestamp_field() {
        let json = r#"{"seconds_since_unix_epoch":0,"nanosecond":1,"extra":true}"#;
        assert!(serde_json::from_str::<Timestamp>(json).is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let spec = TimeSpec {
            start: Timestamp::UNIX_EPOCH,
            end: Timestamp::new(10, 5).unwrap(),
            direction: Direction::Backward,
        };
        let yaml = serde_yml::to_string(&spec).unwrap();
        let back: TimeSpec = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(spec, back);
    }
}
