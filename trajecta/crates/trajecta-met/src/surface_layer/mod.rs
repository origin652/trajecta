//! # Contract: near-surface similarity models
//!
//! Surface-layer models are pure deterministic batch functions with explicit
//! input quality and fallback status. Results cannot depend on thread count,
//! chunk boundaries, shared mutable state, or statistics of the entire batch.

use std::collections::BTreeMap;
use std::sync::Arc;

use trajecta_case::model::physics::ModelId;

use crate::field::FieldQuality;

/// Structure-of-arrays inputs for one near-surface query batch.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceLayerInput<'a> {
    /// Query height above local ground in metres.
    pub query_height_agl_m: &'a [f64],
    /// Reference eastward wind in metres per second.
    pub reference_eastward_wind_m_s: &'a [f64],
    /// Reference northward wind in metres per second.
    pub reference_northward_wind_m_s: &'a [f64],
    /// Reference measurement height above ground in metres.
    pub reference_height_agl_m: &'a [f64],
    /// Reference air temperature in kelvin.
    pub air_temperature_k: &'a [f64],
    /// Reference specific humidity.
    pub specific_humidity: &'a [f64],
    /// Aerodynamic roughness length in metres.
    pub roughness_length_m: &'a [f64],
    /// Monin-Obukhov length in metres.
    pub monin_obukhov_length_m: &'a [f64],
    /// Friction velocity in metres per second.
    pub friction_velocity_m_s: &'a [f64],
}

/// Structure-of-arrays output from a surface-layer model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceLayerOutput {
    /// Eastward wind at query height.
    pub eastward_wind_m_s: Vec<f64>,
    /// Northward wind at query height.
    pub northward_wind_m_s: Vec<f64>,
    /// Air temperature at query height.
    pub air_temperature_k: Vec<f64>,
    /// Specific humidity at query height.
    pub specific_humidity: Vec<f64>,
    /// Per-sample source, derived, or estimated quality.
    pub quality: Vec<FieldQuality>,
}

/// Pure batch interface for a named near-surface model.
pub trait SurfaceLayerModel: Send + Sync {
    /// Stable implementation identifier.
    fn model_id(&self) -> &ModelId;

    /// Evaluates one independent structure-of-arrays batch.
    fn evaluate(
        &self,
        input: SurfaceLayerInput<'_>,
    ) -> Result<SurfaceLayerOutput, SurfaceLayerError>;
}

/// Modern Monin-Obukhov model using Businger-Dyer stability functions.
#[derive(Clone, Debug)]
pub struct MoninObukhovBusingerDyer {
    model_id: ModelId,
}

impl Default for MoninObukhovBusingerDyer {
    fn default() -> Self {
        Self {
            model_id: ModelId("surface_layer/monin_obukhov_businger_dyer/v0".to_owned()),
        }
    }
}

impl SurfaceLayerModel for MoninObukhovBusingerDyer {
    fn model_id(&self) -> &ModelId {
        &self.model_id
    }

    fn evaluate(
        &self,
        _input: SurfaceLayerInput<'_>,
    ) -> Result<SurfaceLayerOutput, SurfaceLayerError> {
        Err(SurfaceLayerError::NotImplemented)
    }
}

/// Independently written compatibility model calibrated against FLEXPART.
#[derive(Clone, Debug)]
pub struct FlexpartCompatibleSurfaceLayer {
    model_id: ModelId,
}

impl Default for FlexpartCompatibleSurfaceLayer {
    fn default() -> Self {
        Self {
            model_id: ModelId("surface_layer/flexpart_compatible/v0".to_owned()),
        }
    }
}

impl SurfaceLayerModel for FlexpartCompatibleSurfaceLayer {
    fn model_id(&self) -> &ModelId {
        &self.model_id
    }

    fn evaluate(
        &self,
        _input: SurfaceLayerInput<'_>,
    ) -> Result<SurfaceLayerOutput, SurfaceLayerError> {
        Err(SurfaceLayerError::NotImplemented)
    }
}

/// Registry of named pure surface-layer implementations.
#[derive(Default)]
pub struct SurfaceLayerRegistry {
    models: BTreeMap<ModelId, Arc<dyn SurfaceLayerModel>>,
}

impl SurfaceLayerRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            models: BTreeMap::new(),
        }
    }

    /// Registers one unique model identifier.
    pub fn register(&mut self, model: Arc<dyn SurfaceLayerModel>) -> Result<(), SurfaceLayerError> {
        let id = model.model_id().clone();
        if self.models.contains_key(&id) {
            return Err(SurfaceLayerError::DuplicateModel(id));
        }
        self.models.insert(id, model);
        Ok(())
    }

    /// Returns one exact named implementation.
    #[must_use]
    pub fn get(&self, id: &ModelId) -> Option<&Arc<dyn SurfaceLayerModel>> {
        self.models.get(id)
    }
}

/// Surface-layer validation or evaluation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SurfaceLayerError {
    /// Scientific model has not been implemented yet.
    NotImplemented,
    /// Structure-of-arrays inputs have inconsistent lengths.
    LengthMismatch,
    /// Input is outside the declared physical domain of the model.
    InvalidInput(usize),
    /// A model identifier is already registered.
    DuplicateModel(ModelId),
}
