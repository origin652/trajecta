//! # Contract: query requests and compiled plans
//!
//! A query plan resolves fields, capabilities, derivations, interpolation
//! policies, and surface-layer requirements before runtime. Every batch uses
//! exactly one vertical-coordinate kind and equal-length SoA columns.

use trajecta_case::model::physics::ModelId;

use crate::field::{CapabilitySet, FieldKey};
use crate::profile::graph::ExecutionPlan;

/// Vertical coordinate shared by every point in a query batch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VerticalQuery {
    /// Geometric height above mean sea level in metres.
    AboveSeaLevel,
    /// Geometric height above local ground in metres.
    AboveGround,
    /// Pressure in pascals.
    Pressure,
}

/// Structure-of-arrays geographic query points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryPointArrays {
    /// Longitude in degrees east.
    pub longitude_degrees: Vec<f64>,
    /// Latitude in degrees north.
    pub latitude_degrees: Vec<f64>,
    /// Vertical value interpreted by `QueryBatch::vertical_coordinate`.
    pub vertical: Vec<f64>,
}

impl QueryPointArrays {
    /// Returns the number of points when all columns have equal length.
    pub fn len(&self) -> Result<usize, QueryPlanError> {
        let len = self.longitude_degrees.len();
        if self.latitude_degrees.len() != len || self.vertical.len() != len {
            return Err(QueryPlanError::LengthMismatch);
        }
        Ok(len)
    }

    /// Returns whether every column is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.longitude_degrees.is_empty()
            && self.latitude_degrees.is_empty()
            && self.vertical.is_empty()
    }
}

/// One homogeneous vertical-coordinate query batch.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryBatch {
    /// Shared vertical-coordinate interpretation.
    pub vertical_coordinate: VerticalQuery,
    /// Geographic SoA columns.
    pub points: QueryPointArrays,
}

/// Immutable field and dependency plan compiled before query execution.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryPlan {
    /// Exact output fields in stable order.
    pub fields: Vec<FieldKey>,
    /// Meteorology capabilities required by the plan.
    pub capabilities: CapabilitySet,
    /// Whether explicitly profiled estimated fallbacks are allowed.
    pub allow_estimated: bool,
    /// Optional selected near-surface model.
    pub surface_layer_model: Option<ModelId>,
    /// Compiled deterministic derivation graph.
    pub derivations: ExecutionPlan,
}

/// Builder that validates field, capability, profile, and model availability.
#[derive(Clone, Debug, Default)]
pub struct QueryPlanBuilder {
    /// Requested output fields.
    pub fields: Vec<FieldKey>,
    /// Whether estimated fallbacks are acceptable.
    pub allow_estimated: bool,
    /// Optional surface-layer model selection.
    pub surface_layer_model: Option<ModelId>,
}

impl QueryPlanBuilder {
    /// Compiles all dependencies before a simulation or replay starts.
    pub fn build(self) -> Result<QueryPlan, QueryPlanError> {
        Err(QueryPlanError::NotImplemented)
    }
}

/// Query-plan or batch validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryPlanError {
    /// Plan compilation has not been implemented yet.
    NotImplemented,
    /// SoA columns have inconsistent lengths.
    LengthMismatch,
    /// A requested field is not registered.
    UnknownField(FieldKey),
    /// Required meteorological capabilities are missing.
    MissingCapabilities,
    /// An estimated field is forbidden by the caller.
    EstimatedFieldForbidden(FieldKey),
    /// The selected surface-layer model is unavailable or lacks inputs.
    SurfaceLayerUnavailable,
}
