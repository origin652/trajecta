//! # Contract: numerical configuration
//!
//! Numerical choices are explicit model identifiers. The execution crate owns
//! implementations and validates that a selected identifier is available.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::physics::ModelId;
use crate::quantity::{Quantity, Time as TimeDimension};

/// Integrator selection and model-specific parameters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegratorSpec {
    /// Stable integrator implementation identifier.
    pub model: ModelId,
    /// Explicit model-specific parameters.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
}

/// Ordered particle-boundary policy selection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundarySpec {
    /// Policies applied after an integration step.
    #[serde(default)]
    pub policies: Vec<ModelId>,
}

/// Numerical controls required by a simulation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumericsSpec {
    /// Requested outer time step, split at event boundaries when required.
    pub time_step: Quantity<TimeDimension>,
    /// Particle integrator.
    pub integrator: IntegratorSpec,
    /// Boundary policy chain.
    pub boundaries: BoundarySpec,
}
