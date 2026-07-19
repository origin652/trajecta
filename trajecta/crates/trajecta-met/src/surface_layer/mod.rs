//! # Contract: near-surface similarity models
//!
//! Surface-layer models are pure deterministic batch functions with explicit
//! local status. M3 exposes one modern Monin-Obukhov/Businger-Dyer model. It
//! uses 10 m wind, 2 m thermodynamic anchors, surface scales, PBL height, and
//! the lowest three-dimensional level; it never silently clamps a query to an
//! anchor or substitutes a FLEXPART-compatible empirical profile.

use std::collections::BTreeMap;
use std::sync::Arc;

use trajecta_case::model::physics::ModelId;

use crate::field::FieldQuality;
use crate::science::M3_CONSTANTS;

/// Structure-of-arrays inputs for one near-surface query batch.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceLayerInput<'a> {
    /// Query height above local ground in metres.
    pub query_height_agl_m: &'a [f64],
    /// Lowest height accepted by the complete transport contract.
    pub minimum_height_agl_m: &'a [f64],
    /// Eastward 10 m wind in metres per second.
    pub ten_metre_eastward_wind_m_s: &'a [f64],
    /// Northward 10 m wind in metres per second.
    pub ten_metre_northward_wind_m_s: &'a [f64],
    /// Two-metre air temperature in kelvin.
    pub two_metre_air_temperature_k: &'a [f64],
    /// Two-metre specific humidity.
    pub two_metre_specific_humidity: &'a [f64],
    /// Aerodynamic roughness length in metres.
    pub roughness_length_m: &'a [f64],
    /// Monin-Obukhov length in metres for non-neutral points.
    pub monin_obukhov_length_m: &'a [f64],
    /// True when the exact neutral branch is required.
    pub neutral_stability: &'a [bool],
    /// Friction velocity in metres per second.
    pub friction_velocity_m_s: &'a [f64],
    /// Temperature scale in kelvin.
    pub temperature_scale_k: &'a [f64],
    /// Specific-humidity scale.
    pub humidity_scale: &'a [f64],
    /// Planetary boundary-layer height in metres.
    pub boundary_layer_height_m: &'a [f64],
    /// Lowest valid three-dimensional level height above ground.
    pub lowest_model_height_agl_m: &'a [f64],
    /// Eastward wind at the lowest three-dimensional level.
    pub lowest_model_eastward_wind_m_s: &'a [f64],
    /// Northward wind at the lowest three-dimensional level.
    pub lowest_model_northward_wind_m_s: &'a [f64],
    /// Air temperature at the lowest three-dimensional level.
    pub lowest_model_air_temperature_k: &'a [f64],
    /// Specific humidity at the lowest three-dimensional level.
    pub lowest_model_specific_humidity: &'a [f64],
    /// Terrain-following no-penetration vertical velocity at the ground.
    pub terrain_vertical_velocity_m_s: &'a [f64],
    /// Geometric vertical velocity at the lowest three-dimensional level.
    pub lowest_model_geometric_vertical_velocity_m_s: &'a [f64],
}

impl SurfaceLayerInput<'_> {
    fn len(self) -> Result<usize, SurfaceLayerError> {
        let len = self.query_height_agl_m.len();
        let lengths = [
            self.minimum_height_agl_m.len(),
            self.ten_metre_eastward_wind_m_s.len(),
            self.ten_metre_northward_wind_m_s.len(),
            self.two_metre_air_temperature_k.len(),
            self.two_metre_specific_humidity.len(),
            self.roughness_length_m.len(),
            self.monin_obukhov_length_m.len(),
            self.neutral_stability.len(),
            self.friction_velocity_m_s.len(),
            self.temperature_scale_k.len(),
            self.humidity_scale.len(),
            self.boundary_layer_height_m.len(),
            self.lowest_model_height_agl_m.len(),
            self.lowest_model_eastward_wind_m_s.len(),
            self.lowest_model_northward_wind_m_s.len(),
            self.lowest_model_air_temperature_k.len(),
            self.lowest_model_specific_humidity.len(),
            self.terrain_vertical_velocity_m_s.len(),
            self.lowest_model_geometric_vertical_velocity_m_s.len(),
        ];
        if lengths.into_iter().any(|value| value != len) {
            return Err(SurfaceLayerError::LengthMismatch);
        }
        Ok(len)
    }
}

/// Local surface-layer outcome for one point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceLayerStatus {
    /// All four output fields are valid.
    Ok,
    /// Query is at/below the declared minimum, at/below roughness, or above the lowest 3-D level.
    Undefined,
    /// Source anchors are physically inconsistent.
    InvalidPhysicalState,
    /// Finite deterministic arithmetic failed.
    NumericalFailure,
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
    /// Geometric vertical velocity at query height.
    pub geometric_vertical_velocity_m_s: Vec<f64>,
    /// True where all four numeric outputs are scientifically valid.
    pub valid: Vec<bool>,
    /// Per-point local status.
    pub status: Vec<SurfaceLayerStatus>,
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

impl MoninObukhovBusingerDyer {
    /// Stable public algorithm identifier frozen for M3.
    pub const MODEL_ID: &'static str = "surface_layer/monin_obukhov_businger_dyer/v0";
}

impl Default for MoninObukhovBusingerDyer {
    fn default() -> Self {
        Self {
            model_id: ModelId(Self::MODEL_ID.to_owned()),
        }
    }
}

impl SurfaceLayerModel for MoninObukhovBusingerDyer {
    fn model_id(&self) -> &ModelId {
        &self.model_id
    }

    fn evaluate(
        &self,
        input: SurfaceLayerInput<'_>,
    ) -> Result<SurfaceLayerOutput, SurfaceLayerError> {
        let len = input.len()?;
        let mut output = SurfaceLayerOutput {
            eastward_wind_m_s: vec![0.0; len],
            northward_wind_m_s: vec![0.0; len],
            air_temperature_k: vec![0.0; len],
            specific_humidity: vec![0.0; len],
            geometric_vertical_velocity_m_s: vec![0.0; len],
            valid: vec![false; len],
            status: vec![SurfaceLayerStatus::InvalidPhysicalState; len],
            quality: vec![FieldQuality::Derived; len],
        };
        for index in 0..len {
            match evaluate_point(input, index) {
                Ok(point) => {
                    output.eastward_wind_m_s[index] = point.eastward_wind_m_s;
                    output.northward_wind_m_s[index] = point.northward_wind_m_s;
                    output.air_temperature_k[index] = point.air_temperature_k;
                    output.specific_humidity[index] = point.specific_humidity;
                    output.geometric_vertical_velocity_m_s[index] =
                        point.geometric_vertical_velocity_m_s;
                    output.valid[index] = true;
                    output.status[index] = SurfaceLayerStatus::Ok;
                }
                Err(status) => output.status[index] = status,
            }
        }
        Ok(output)
    }
}

#[derive(Clone, Copy, Debug)]
struct SurfacePoint {
    eastward_wind_m_s: f64,
    northward_wind_m_s: f64,
    air_temperature_k: f64,
    specific_humidity: f64,
    geometric_vertical_velocity_m_s: f64,
}

fn evaluate_point(
    input: SurfaceLayerInput<'_>,
    index: usize,
) -> Result<SurfacePoint, SurfaceLayerStatus> {
    let query = input.query_height_agl_m[index];
    let minimum = input.minimum_height_agl_m[index];
    let z0 = input.roughness_length_m[index];
    let lowest = input.lowest_model_height_agl_m[index];
    let pbl = input.boundary_layer_height_m[index];
    let monin_obukhov = input.monin_obukhov_length_m[index];
    let neutral = input.neutral_stability[index];
    let ustar = input.friction_velocity_m_s[index];
    let temperature_scale = input.temperature_scale_k[index];
    let humidity_scale = input.humidity_scale[index];
    let scalar_inputs = [
        query,
        minimum,
        z0,
        lowest,
        pbl,
        monin_obukhov,
        ustar,
        temperature_scale,
        humidity_scale,
        input.ten_metre_eastward_wind_m_s[index],
        input.ten_metre_northward_wind_m_s[index],
        input.two_metre_air_temperature_k[index],
        input.two_metre_specific_humidity[index],
        input.lowest_model_eastward_wind_m_s[index],
        input.lowest_model_northward_wind_m_s[index],
        input.lowest_model_air_temperature_k[index],
        input.lowest_model_specific_humidity[index],
        input.terrain_vertical_velocity_m_s[index],
        input.lowest_model_geometric_vertical_velocity_m_s[index],
    ];
    if scalar_inputs.iter().any(|value| !value.is_finite()) {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    if minimum <= z0
        || z0 <= 0.0
        || lowest <= minimum
        || pbl <= 0.0
        || ustar < 0.0
        || (!neutral && monin_obukhov == 0.0)
        || input.two_metre_air_temperature_k[index] <= 0.0
        || input.lowest_model_air_temperature_k[index] <= 0.0
        || !(0.0..1.0).contains(&input.two_metre_specific_humidity[index])
        || !(0.0..1.0).contains(&input.lowest_model_specific_humidity[index])
    {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    if query < minimum || query <= z0 || query > lowest {
        return Err(SurfaceLayerStatus::Undefined);
    }

    let similarity_top = lowest.min(0.1 * pbl);
    if similarity_top <= z0 {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    let stability = Stability {
        neutral,
        monin_obukhov_length_m: monin_obukhov,
    };
    let wind = similarity_wind(
        query.min(similarity_top),
        z0,
        stability,
        ustar,
        input.ten_metre_eastward_wind_m_s[index],
        input.ten_metre_northward_wind_m_s[index],
    )?;
    let temperature = similarity_scalar(
        query.min(similarity_top),
        2.0,
        input.two_metre_air_temperature_k[index],
        temperature_scale,
        z0,
        stability,
    )?;
    let humidity = similarity_scalar(
        query.min(similarity_top),
        2.0,
        input.two_metre_specific_humidity[index],
        humidity_scale,
        z0,
        stability,
    )?;

    let point = if query <= similarity_top || similarity_top == lowest {
        SurfacePoint {
            eastward_wind_m_s: wind.0,
            northward_wind_m_s: wind.1,
            air_temperature_k: temperature,
            specific_humidity: humidity,
            geometric_vertical_velocity_m_s: monotone_bridge(
                query,
                0.0,
                lowest,
                input.terrain_vertical_velocity_m_s[index],
                input.lowest_model_geometric_vertical_velocity_m_s[index],
                0.0,
            )?,
        }
    } else {
        let wind_derivative = similarity_wind_derivative(
            similarity_top,
            stability,
            ustar,
            input.ten_metre_eastward_wind_m_s[index],
            input.ten_metre_northward_wind_m_s[index],
        )?;
        let scalar_shape_derivative =
            stability_function_derivative(similarity_top, stability, StabilityFunction::Heat)?;
        SurfacePoint {
            eastward_wind_m_s: monotone_bridge(
                query,
                similarity_top,
                lowest,
                wind.0,
                input.lowest_model_eastward_wind_m_s[index],
                wind_derivative.0,
            )?,
            northward_wind_m_s: monotone_bridge(
                query,
                similarity_top,
                lowest,
                wind.1,
                input.lowest_model_northward_wind_m_s[index],
                wind_derivative.1,
            )?,
            air_temperature_k: monotone_bridge(
                query,
                similarity_top,
                lowest,
                temperature,
                input.lowest_model_air_temperature_k[index],
                temperature_scale / M3_CONSTANTS.von_karman * scalar_shape_derivative,
            )?,
            specific_humidity: monotone_bridge(
                query,
                similarity_top,
                lowest,
                humidity,
                input.lowest_model_specific_humidity[index],
                humidity_scale / M3_CONSTANTS.von_karman * scalar_shape_derivative,
            )?,
            geometric_vertical_velocity_m_s: monotone_bridge(
                query,
                0.0,
                lowest,
                input.terrain_vertical_velocity_m_s[index],
                input.lowest_model_geometric_vertical_velocity_m_s[index],
                0.0,
            )?,
        }
    };
    if [
        point.eastward_wind_m_s,
        point.northward_wind_m_s,
        point.air_temperature_k,
        point.specific_humidity,
        point.geometric_vertical_velocity_m_s,
    ]
    .iter()
    .any(|value| !value.is_finite())
        || point.air_temperature_k <= 0.0
        || !(0.0..1.0).contains(&point.specific_humidity)
    {
        return Err(SurfaceLayerStatus::NumericalFailure);
    }
    Ok(point)
}

/// Returns the frozen local lower bound for complete transport queries.
///
/// The half-metre floor avoids pretending similarity theory is defined at the
/// ground, while `2*z0` keeps the first accepted point strictly above the
/// aerodynamic roughness sublayer.
pub fn minimum_transport_height_agl_m(roughness_length_m: f64) -> Result<f64, SurfaceLayerStatus> {
    if !roughness_length_m.is_finite() || roughness_length_m <= 0.0 {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    let minimum = 0.5_f64.max(2.0 * roughness_length_m);
    minimum
        .is_finite()
        .then_some(minimum)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

#[derive(Clone, Copy)]
struct Stability {
    neutral: bool,
    monin_obukhov_length_m: f64,
}

#[derive(Clone, Copy)]
enum StabilityFunction {
    Momentum,
    Heat,
}

fn similarity_wind(
    height_m: f64,
    z0_m: f64,
    stability: Stability,
    friction_velocity_m_s: f64,
    reference_eastward_m_s: f64,
    reference_northward_m_s: f64,
) -> Result<(f64, f64), SurfaceLayerStatus> {
    let reference_speed = reference_eastward_m_s.hypot(reference_northward_m_s);
    if reference_speed == 0.0 {
        if friction_velocity_m_s == 0.0 {
            return Ok((0.0, 0.0));
        }
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    let shape = stability_profile(height_m, z0_m, stability, StabilityFunction::Momentum)?;
    let reference_shape = stability_profile(10.0, z0_m, stability, StabilityFunction::Momentum)?;
    let speed = reference_speed
        + friction_velocity_m_s / M3_CONSTANTS.von_karman * (shape - reference_shape);
    if !speed.is_finite() || speed < 0.0 {
        return Err(SurfaceLayerStatus::NumericalFailure);
    }
    let scale = speed / reference_speed;
    Ok((
        reference_eastward_m_s * scale,
        reference_northward_m_s * scale,
    ))
}

fn similarity_wind_derivative(
    height_m: f64,
    stability: Stability,
    friction_velocity_m_s: f64,
    reference_eastward_m_s: f64,
    reference_northward_m_s: f64,
) -> Result<(f64, f64), SurfaceLayerStatus> {
    let speed = reference_eastward_m_s.hypot(reference_northward_m_s);
    if speed == 0.0 {
        return Ok((0.0, 0.0));
    }
    let speed_derivative = friction_velocity_m_s / M3_CONSTANTS.von_karman
        * stability_function_derivative(height_m, stability, StabilityFunction::Momentum)?;
    Ok((
        speed_derivative * reference_eastward_m_s / speed,
        speed_derivative * reference_northward_m_s / speed,
    ))
}

fn similarity_scalar(
    height_m: f64,
    reference_height_m: f64,
    reference_value: f64,
    scale: f64,
    z0_m: f64,
    stability: Stability,
) -> Result<f64, SurfaceLayerStatus> {
    if reference_height_m <= z0_m {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    let shape = stability_profile(height_m, z0_m, stability, StabilityFunction::Heat)?;
    let reference_shape =
        stability_profile(reference_height_m, z0_m, stability, StabilityFunction::Heat)?;
    let value = reference_value + scale / M3_CONSTANTS.von_karman * (shape - reference_shape);
    value
        .is_finite()
        .then_some(value)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

fn stability_profile(
    height_m: f64,
    z0_m: f64,
    stability: Stability,
    function: StabilityFunction,
) -> Result<f64, SurfaceLayerStatus> {
    if !height_m.is_finite() || !z0_m.is_finite() || z0_m <= 0.0 || height_m <= z0_m {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    if stability.neutral {
        return Ok((height_m / z0_m).ln());
    }
    let length = stability.monin_obukhov_length_m;
    if !length.is_finite() || length == 0.0 {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    let zeta = height_m / length;
    let zeta0 = z0_m / length;
    let psi = match function {
        StabilityFunction::Momentum => businger_dyer_psi_m(zeta),
        StabilityFunction::Heat => businger_dyer_psi_h(zeta),
    }?;
    let psi0 = match function {
        StabilityFunction::Momentum => businger_dyer_psi_m(zeta0),
        StabilityFunction::Heat => businger_dyer_psi_h(zeta0),
    }?;
    let value = (height_m / z0_m).ln() - psi + psi0;
    value
        .is_finite()
        .then_some(value)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

fn stability_function_derivative(
    height_m: f64,
    stability: Stability,
    function: StabilityFunction,
) -> Result<f64, SurfaceLayerStatus> {
    if !height_m.is_finite() || height_m <= 0.0 {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    if stability.neutral {
        return Ok(1.0 / height_m);
    }
    let length = stability.monin_obukhov_length_m;
    if !length.is_finite() || length == 0.0 {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    let zeta = height_m / length;
    let psi_prime = match function {
        StabilityFunction::Momentum => businger_dyer_psi_m_prime(zeta),
        StabilityFunction::Heat => businger_dyer_psi_h_prime(zeta),
    }?;
    let derivative = 1.0 / height_m - psi_prime / length;
    derivative
        .is_finite()
        .then_some(derivative)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

/// Businger-Dyer momentum stability correction.
pub fn businger_dyer_psi_m(zeta: f64) -> Result<f64, SurfaceLayerStatus> {
    if !zeta.is_finite() {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    if zeta >= 0.0 {
        return Ok(-5.0 * zeta);
    }
    let x = (1.0 - 16.0 * zeta).powf(0.25);
    let value = 2.0 * ((1.0 + x) / 2.0).ln() + ((1.0 + x * x) / 2.0).ln() - 2.0 * x.atan()
        + std::f64::consts::FRAC_PI_2;
    value
        .is_finite()
        .then_some(value)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

/// Businger-Dyer heat and moisture stability correction.
pub fn businger_dyer_psi_h(zeta: f64) -> Result<f64, SurfaceLayerStatus> {
    if !zeta.is_finite() {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    if zeta >= 0.0 {
        return Ok(-5.0 * zeta);
    }
    let x = (1.0 - 16.0 * zeta).powf(0.25);
    let value = 2.0 * ((1.0 + x * x) / 2.0).ln();
    value
        .is_finite()
        .then_some(value)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

fn businger_dyer_psi_m_prime(zeta: f64) -> Result<f64, SurfaceLayerStatus> {
    if zeta >= 0.0 {
        return Ok(-5.0);
    }
    let x = (1.0 - 16.0 * zeta).powf(0.25);
    let derivative_x = 2.0 / (1.0 + x) + (2.0 * x - 2.0) / (1.0 + x * x);
    let value = derivative_x * (-4.0 / (x * x * x));
    value
        .is_finite()
        .then_some(value)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

fn businger_dyer_psi_h_prime(zeta: f64) -> Result<f64, SurfaceLayerStatus> {
    if zeta >= 0.0 {
        return Ok(-5.0);
    }
    let x = (1.0 - 16.0 * zeta).powf(0.25);
    let value = -16.0 / (x * x * (1.0 + x * x));
    value
        .is_finite()
        .then_some(value)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
}

fn monotone_bridge(
    query: f64,
    start: f64,
    end: f64,
    start_value: f64,
    end_value: f64,
    start_derivative: f64,
) -> Result<f64, SurfaceLayerStatus> {
    let width = end - start;
    if !width.is_finite() || width <= 0.0 || query < start || query > end {
        return Err(SurfaceLayerStatus::InvalidPhysicalState);
    }
    let secant = (end_value - start_value) / width;
    let derivative = if secant == 0.0 || start_derivative * secant <= 0.0 {
        0.0
    } else {
        start_derivative.signum() * start_derivative.abs().min(3.0 * secant.abs())
    };
    let t = (query - start) / width;
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let value = h00 * start_value + h10 * width * derivative + h01 * end_value;
    value
        .is_finite()
        .then_some(value)
        .ok_or(SurfaceLayerStatus::NumericalFailure)
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

/// Surface-layer structural validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SurfaceLayerError {
    /// Structure-of-arrays inputs have inconsistent lengths.
    LengthMismatch,
    /// A model identifier is already registered.
    DuplicateModel(ModelId),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn repeated(value: f64, len: usize) -> Vec<f64> {
        vec![value; len]
    }

    #[test]
    fn businger_dyer_stable_neutral_and_unstable_branches_are_frozen() {
        assert_eq!(businger_dyer_psi_m(0.0), Ok(0.0));
        assert_eq!(businger_dyer_psi_h(0.2), Ok(-1.0));
        assert!(businger_dyer_psi_m(-0.5).unwrap() > 0.0);
        assert!(businger_dyer_psi_h(-0.5).unwrap() > 0.0);
    }

    #[test]
    fn batch_handles_similarity_bridge_and_local_undefined_status() {
        let len = 3;
        let query = vec![5.0, 30.0, 0.0];
        let neutral = vec![true; len];
        let model = MoninObukhovBusingerDyer::default();
        let output = model
            .evaluate(SurfaceLayerInput {
                query_height_agl_m: &query,
                minimum_height_agl_m: &repeated(0.5, len),
                ten_metre_eastward_wind_m_s: &repeated(5.0, len),
                ten_metre_northward_wind_m_s: &repeated(0.0, len),
                two_metre_air_temperature_k: &repeated(290.0, len),
                two_metre_specific_humidity: &repeated(0.005, len),
                roughness_length_m: &repeated(0.1, len),
                monin_obukhov_length_m: &repeated(100.0, len),
                neutral_stability: &neutral,
                friction_velocity_m_s: &repeated(0.3, len),
                temperature_scale_k: &repeated(0.1, len),
                humidity_scale: &repeated(0.0001, len),
                boundary_layer_height_m: &repeated(100.0, len),
                lowest_model_height_agl_m: &repeated(50.0, len),
                lowest_model_eastward_wind_m_s: &repeated(8.0, len),
                lowest_model_northward_wind_m_s: &repeated(1.0, len),
                lowest_model_air_temperature_k: &repeated(292.0, len),
                lowest_model_specific_humidity: &repeated(0.004, len),
                terrain_vertical_velocity_m_s: &repeated(0.2, len),
                lowest_model_geometric_vertical_velocity_m_s: &repeated(1.0, len),
            })
            .unwrap();
        assert_eq!(output.status[0], SurfaceLayerStatus::Ok);
        assert_eq!(output.status[1], SurfaceLayerStatus::Ok);
        assert_eq!(output.status[2], SurfaceLayerStatus::Undefined);
        assert!(output.valid[0]);
        assert!(output.valid[1]);
        assert!(!output.valid[2]);
        assert!(output.eastward_wind_m_s[1] > output.eastward_wind_m_s[0]);
        assert!(output.geometric_vertical_velocity_m_s[0] > 0.2);
        assert!(output.geometric_vertical_velocity_m_s[1] < 1.0);
    }

    #[test]
    fn bridge_is_monotone_and_hits_lowest_model_anchor() {
        let start = monotone_bridge(10.0, 10.0, 50.0, 3.0, 7.0, 0.2).unwrap();
        let middle = monotone_bridge(30.0, 10.0, 50.0, 3.0, 7.0, 0.2).unwrap();
        let end = monotone_bridge(50.0, 10.0, 50.0, 3.0, 7.0, 0.2).unwrap();
        assert_eq!(start, 3.0);
        assert!(middle > start && middle < end);
        assert_eq!(end, 7.0);
    }
}
