//! # Contract: release scheduling and deterministic geometry sampling
//!
//! Release events carry exact time, geometry, vertical-coordinate mode, mass,
//! and particle count. Sampling is deterministic and independent of worker
//! order. GeoJSON parsing and scientific allocation algorithms follow later.

use std::collections::BTreeMap;

use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::Timestamp;
use trajecta_met::query::request::VerticalQuery;

use crate::particle::ParticleBatch;

/// Ordered collection of release events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReleaseSchedule {
    /// Events in deterministic schedule order.
    pub events: Vec<ReleaseEvent>,
}

/// One exact release event.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseEvent {
    /// Stable event identifier.
    pub id: String,
    /// Inclusive release start time.
    pub start: Timestamp,
    /// Inclusive release end time.
    pub end: Timestamp,
    /// GeoJSON geometry text or stable local component identity.
    pub geometry: String,
    /// Vertical-coordinate interpretation.
    pub vertical_coordinate: VerticalQuery,
    /// Lower vertical bound in the selected coordinate.
    pub vertical_lower: f64,
    /// Upper vertical bound in the selected coordinate.
    pub vertical_upper: f64,
    /// Requested particle count.
    pub particle_count: usize,
    /// Total released mass in kilograms by substance.
    pub mass_kg: BTreeMap<SubstanceId, f64>,
}

/// Deterministic horizontal geometry sampler.
pub trait GeometrySampler: Send + Sync {
    /// Samples longitude/latitude pairs for one exact event and count.
    fn sample_horizontal(
        &self,
        event: &ReleaseEvent,
        count: usize,
    ) -> Result<Vec<(f64, f64)>, ReleaseError>;
}

/// Deterministic vertical-coordinate sampler.
pub trait VerticalSampler: Send + Sync {
    /// Samples vertical coordinates for one exact event and count.
    fn sample_vertical(&self, event: &ReleaseEvent, count: usize)
    -> Result<Vec<f64>, ReleaseError>;
}

/// Allocates event mass and stable particle identities.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReleaseAllocator;

impl ReleaseAllocator {
    /// Builds a complete particle batch for one event.
    pub fn allocate(_event: &ReleaseEvent) -> Result<ParticleBatch, ReleaseError> {
        Err(ReleaseError::NotImplemented)
    }
}

/// Release schedule, sampling, or allocation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReleaseError {
    /// Release algorithms have not been implemented yet.
    NotImplemented,
    /// Event interval or particle count is invalid.
    InvalidEvent(String),
    /// Geometry cannot be interpreted by the selected sampler.
    InvalidGeometry,
    /// Released mass is negative or non-finite.
    InvalidMass,
}
