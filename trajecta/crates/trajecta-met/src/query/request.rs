//! # Contract: query requests and compiled plans
//!
//! A query plan resolves fields, capabilities, derivations, interpolation
//! policies, and surface-layer requirements before runtime. Every batch uses
//! exactly one physical time (owned by its prepared window), one vertical
//! coordinate kind, and equal-length finite SoA columns.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use trajecta_case::model::physics::ModelId;

use crate::field::{Capability, CapabilitySet, FieldKey, FieldQuality, FieldRegistry};
use crate::profile::graph::ExecutionPlan;
use crate::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerModel, SurfaceLayerRegistry};

/// Query explanation policy frozen into a compiled plan.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ExplainMode {
    /// Do not allocate or populate per-point explanation records.
    #[default]
    Disabled,
    /// Emit complete per-point interpolation and provenance explanations.
    Full,
}

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
    /// Longitude in degrees east; any finite wrap is accepted.
    pub longitude_degrees: Vec<f64>,
    /// Latitude in degrees north, within the closed interval [-90, 90].
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

impl QueryBatch {
    /// Validates finite coordinates and pressure-domain preconditions.
    pub fn validate(&self) -> Result<usize, QueryPlanError> {
        let len = self.points.len()?;
        for index in 0..len {
            let longitude = self.points.longitude_degrees[index];
            let latitude = self.points.latitude_degrees[index];
            let vertical = self.points.vertical[index];
            if !longitude.is_finite() || !latitude.is_finite() || !vertical.is_finite() {
                return Err(QueryPlanError::NonFiniteCoordinate(index));
            }
            if !(-90.0..=90.0).contains(&latitude) {
                return Err(QueryPlanError::InvalidLatitude(index));
            }
            if self.vertical_coordinate == VerticalQuery::Pressure && vertical <= 0.0 {
                return Err(QueryPlanError::NonPositivePressure(index));
            }
        }
        Ok(len)
    }
}

/// Immutable field and dependency plan compiled before query execution.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryPlan {
    fields: Vec<FieldKey>,
    capabilities: CapabilitySet,
    allow_estimated: bool,
    surface_layer: Option<SurfaceLayerSelection>,
    derivations: ExecutionPlan,
    explain: ExplainMode,
}

#[derive(Clone)]
struct SurfaceLayerSelection {
    id: ModelId,
    model: Arc<dyn SurfaceLayerModel>,
}

impl fmt::Debug for SurfaceLayerSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SurfaceLayerSelection")
            .field("id", &self.id)
            .finish()
    }
}

impl PartialEq for SurfaceLayerSelection {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl QueryPlan {
    /// Returns exact output fields in stable request order.
    #[must_use]
    pub fn fields(&self) -> &[FieldKey] {
        &self.fields
    }

    /// Returns all capabilities required by the plan.
    #[must_use]
    pub const fn capabilities(&self) -> CapabilitySet {
        self.capabilities
    }

    /// Returns whether profiled estimated fallbacks may be used.
    #[must_use]
    pub const fn allow_estimated(&self) -> bool {
        self.allow_estimated
    }

    /// Returns the selected near-surface model, if any.
    #[must_use]
    pub const fn surface_layer_model(&self) -> Option<&ModelId> {
        match &self.surface_layer {
            Some(selection) => Some(&selection.id),
            None => None,
        }
    }

    /// Returns the implementation pinned when this plan was compiled.
    #[must_use]
    pub(crate) fn surface_layer(&self) -> Option<&Arc<dyn SurfaceLayerModel>> {
        self.surface_layer
            .as_ref()
            .map(|selection| &selection.model)
    }

    /// Returns the compiled deterministic derivation graph.
    #[must_use]
    pub const fn derivations(&self) -> &ExecutionPlan {
        &self.derivations
    }

    /// Returns the explanation policy frozen into this plan.
    #[must_use]
    pub const fn explain_mode(&self) -> ExplainMode {
        self.explain
    }
}

/// User-facing generic query-plan request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryPlanRequest {
    /// Requested output fields in caller-selected stable order.
    pub fields: Vec<FieldKey>,
    /// Whether estimated fallbacks are acceptable.
    pub allow_estimated: bool,
    /// Optional named near-surface model.
    pub surface_layer_model: Option<ModelId>,
    /// Per-point explanation policy.
    pub explain: ExplainMode,
}

/// Builder backed by already-validated engine registries.
#[derive(Clone, Copy)]
pub struct QueryPlanBuilder<'a> {
    fields: &'a FieldRegistry,
    available_capabilities: CapabilitySet,
    surface_layers: &'a SurfaceLayerRegistry,
    derivations: &'a ExecutionPlan,
}

impl<'a> QueryPlanBuilder<'a> {
    /// Creates a compiler over immutable registries and one dependency plan.
    #[must_use]
    pub const fn new(
        fields: &'a FieldRegistry,
        available_capabilities: CapabilitySet,
        surface_layers: &'a SurfaceLayerRegistry,
        derivations: &'a ExecutionPlan,
    ) -> Self {
        Self {
            fields,
            available_capabilities,
            surface_layers,
            derivations,
        }
    }

    /// Compiles and validates all dependencies before preparation begins.
    pub fn build(&self, request: QueryPlanRequest) -> Result<QueryPlan, QueryPlanError> {
        if request.fields.is_empty() {
            return Err(QueryPlanError::EmptyFieldSet);
        }
        let mut seen = BTreeSet::new();
        // Every query publishes local vertical bounds and uses the canonical
        // column locator, even when all requested values are horizontal.
        let mut required = CapabilitySet::new().with(Capability::Transport);
        for field in &request.fields {
            if !seen.insert(field.clone()) {
                return Err(QueryPlanError::DuplicateField(field.clone()));
            }
            let descriptor = self
                .fields
                .get(field)
                .ok_or_else(|| QueryPlanError::UnknownField(field.clone()))?;
            if !request.allow_estimated && descriptor.quality == FieldQuality::Estimated {
                return Err(QueryPlanError::EstimatedFieldForbidden(field.clone()));
            }
            required.insert(descriptor.required_capability);
        }
        let surface_layer = if let Some(model) = &request.surface_layer_model {
            let Some(implementation) = self.surface_layers.get(model) else {
                return Err(QueryPlanError::SurfaceLayerUnavailable(model.clone()));
            };
            required.insert(Capability::NearSurfaceTransport);
            Some(SurfaceLayerSelection {
                id: model.clone(),
                model: implementation.clone(),
            })
        } else {
            None
        };
        if !self.available_capabilities.contains_all(required) {
            return Err(QueryPlanError::MissingCapabilities {
                required,
                available: self.available_capabilities,
            });
        }
        Ok(QueryPlan {
            fields: request.fields,
            capabilities: required,
            allow_estimated: request.allow_estimated,
            surface_layer,
            derivations: self.derivations.clone(),
            explain: request.explain,
        })
    }

    /// Compiles the fixed M4 transport contract.
    pub fn build_transport(
        &self,
        request: TransportPlanRequest,
    ) -> Result<TransportPlan, QueryPlanError> {
        let query = self.build(QueryPlanRequest {
            fields: transport_fields().to_vec(),
            allow_estimated: request.allow_estimated,
            surface_layer_model: Some(request.surface_layer_model),
            explain: request.explain,
        })?;
        let required = CapabilitySet::new()
            .with(Capability::Transport)
            .with(Capability::NearSurfaceTransport);
        if !query.capabilities.contains_all(required) {
            return Err(QueryPlanError::MissingCapabilities {
                required,
                available: query.capabilities,
            });
        }
        Ok(TransportPlan { query })
    }
}

/// Request for the fixed complete transport plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportPlanRequest {
    /// Whether explicitly profiled estimated inputs may be used.
    pub allow_estimated: bool,
    /// Selected near-surface implementation.
    pub surface_layer_model: ModelId,
    /// Per-point explanation policy.
    pub explain: ExplainMode,
}

impl Default for TransportPlanRequest {
    fn default() -> Self {
        Self {
            allow_estimated: false,
            surface_layer_model: ModelId(MoninObukhovBusingerDyer::MODEL_ID.into()),
            explain: ExplainMode::Disabled,
        }
    }
}

/// Strongly typed fixed field plan consumed by the M4 integrator.
#[derive(Clone, Debug, PartialEq)]
pub struct TransportPlan {
    query: QueryPlan,
}

impl TransportPlan {
    /// Returns the underlying immutable generic query plan.
    #[must_use]
    pub const fn query_plan(&self) -> &QueryPlan {
        &self.query
    }
}

fn transport_fields() -> [FieldKey; 8] {
    transport_canonical_fields().map(FieldKey::Canonical)
}

fn transport_canonical_fields() -> [crate::field::CanonicalField; 8] {
    use crate::field::CanonicalField;

    [
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
        CanonicalField::GeometricVerticalVelocity,
        CanonicalField::AirPressure,
        CanonicalField::AirTemperature,
        CanonicalField::SpecificHumidity,
        CanonicalField::AirDensity,
        CanonicalField::GeometricTerrainHeight,
    ]
}

/// Query-plan or batch validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryPlanError {
    /// No output fields were requested.
    EmptyFieldSet,
    /// SoA columns have inconsistent lengths.
    LengthMismatch,
    /// A coordinate is NaN or infinite.
    NonFiniteCoordinate(usize),
    /// Latitude lies outside [-90, 90] degrees.
    InvalidLatitude(usize),
    /// Pressure queries require a strictly positive pressure.
    NonPositivePressure(usize),
    /// A requested field occurs more than once.
    DuplicateField(FieldKey),
    /// A requested field is not registered.
    UnknownField(FieldKey),
    /// Required meteorological capabilities are missing.
    MissingCapabilities {
        /// Capabilities required by the plan.
        required: CapabilitySet,
        /// Capabilities guaranteed by the catalog.
        available: CapabilitySet,
    },
    /// An estimated field is forbidden by the caller.
    EstimatedFieldForbidden(FieldKey),
    /// The selected surface-layer model is unavailable.
    SurfaceLayerUnavailable(ModelId),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use trajecta_case::quantity::{Dimension, Unit};

    use super::*;
    use crate::field::FieldDescriptor;
    use crate::surface_layer::SurfaceLayerModel;

    fn transport_registry() -> FieldRegistry {
        FieldRegistry::canonical().unwrap()
    }

    #[test]
    fn transport_plan_requires_both_capabilities_and_surface_model() {
        let registry = transport_registry();
        let mut models = SurfaceLayerRegistry::new();
        let model = Arc::new(MoninObukhovBusingerDyer::default());
        models.register(model.clone()).unwrap();
        assert_eq!(model.model_id().0, MoninObukhovBusingerDyer::MODEL_ID);
        let execution = ExecutionPlan::default();

        let only_transport = CapabilitySet::new().with(Capability::Transport);
        let builder = QueryPlanBuilder::new(&registry, only_transport, &models, &execution);
        assert!(matches!(
            builder.build_transport(TransportPlanRequest::default()),
            Err(QueryPlanError::MissingCapabilities { .. })
        ));

        let available = only_transport.with(Capability::NearSurfaceTransport);
        let builder = QueryPlanBuilder::new(&registry, available, &models, &execution);
        let plan = builder
            .build_transport(TransportPlanRequest::default())
            .unwrap();
        assert_eq!(plan.query_plan().fields(), transport_fields());
        assert!(!plan.query_plan().allow_estimated());
    }

    #[test]
    fn batch_validation_keeps_exact_poles_but_rejects_bad_latitudes() {
        let valid = QueryBatch {
            vertical_coordinate: VerticalQuery::AboveSeaLevel,
            points: QueryPointArrays {
                longitude_degrees: vec![360.0, -720.0],
                latitude_degrees: vec![90.0, -90.0],
                vertical: vec![100.0, 100.0],
            },
        };
        assert_eq!(valid.validate(), Ok(2));

        let invalid = QueryBatch {
            vertical_coordinate: VerticalQuery::Pressure,
            points: QueryPointArrays {
                longitude_degrees: vec![0.0],
                latitude_degrees: vec![91.0],
                vertical: vec![100_000.0],
            },
        };
        assert_eq!(invalid.validate(), Err(QueryPlanError::InvalidLatitude(0)));
    }

    #[test]
    fn generic_surface_field_still_requires_transport_column_support() {
        let mut registry = FieldRegistry::new();
        let canonical = crate::field::CanonicalField::BoundaryLayerHeight;
        let semantics = canonical.semantics();
        registry
            .register(FieldDescriptor {
                key: FieldKey::Canonical(canonical),
                unit: Unit::new("m", Dimension::Length, 1.0, 0.0).unwrap(),
                shape: semantics.shape,
                vertical_stagger: semantics.vertical_stagger,
                quality: FieldQuality::Source,
                required_capability: semantics.required_capability,
                interpolation: semantics.interpolation,
            })
            .unwrap();
        let models = SurfaceLayerRegistry::new();
        let execution = ExecutionPlan::default();
        let builder = QueryPlanBuilder::new(
            &registry,
            CapabilitySet::new().with(Capability::NearSurfaceTransport),
            &models,
            &execution,
        );
        assert!(matches!(
            builder.build(QueryPlanRequest {
                fields: vec![FieldKey::Canonical(canonical)],
                allow_estimated: false,
                surface_layer_model: None,
                explain: ExplainMode::Disabled,
            }),
            Err(QueryPlanError::MissingCapabilities { .. })
        ));
    }
}
