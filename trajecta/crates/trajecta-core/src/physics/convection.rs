//! # Contract: conservative Emanuel–Živković-Rothman column transport
//!
//! A complete thermodynamic column produces one non-negative continuous-time
//! generator. Uniformization forms a column-stochastic transition matrix. The
//! same matrix drives forward sampling and the weighted transpose proposal.

use trajecta_met::vertical::ColumnGeometry;

use crate::physics::PhysicsError;
use crate::rng::{CounterRng, ProcessRandomKey, StableRandomId};

const CONVECTION_GRAVITY_M_S2: f64 = 9.8;
const DRY_AIR_GAS_CONSTANT_J_KG_K: f64 = 287.04;
const WATER_VAPOR_GAS_CONSTANT_J_KG_K: f64 = 461.5;
const DRY_AIR_HEAT_CAPACITY_J_KG_K: f64 = 1_005.7;
const WATER_VAPOR_HEAT_CAPACITY_J_KG_K: f64 = 1_870.0;
const LIQUID_WATER_HEAT_CAPACITY_J_KG_K: f64 = 2_500.0;
const LATENT_HEAT_AT_FREEZING_J_KG: f64 = 2.501e6;
const WATER_VAPOR_TO_DRY_AIR_MOLAR_MASS_RATIO: f64 =
    DRY_AIR_GAS_CONSTANT_J_KG_K / WATER_VAPOR_GAS_CONSTANT_J_KG_K;
const AUTOCONVERSION_THRESHOLD_KG_KG: f64 = 0.0011;
const AUTOCONVERSION_CRITICAL_TEMPERATURE_C: f64 = -55.0;
const ENTRAINMENT_COEFFICIENT: f64 = 1.5;
const UNSATURATED_DOWNDRAFT_AREA_FRACTION: f64 = 0.05;
const PRECIPITATION_OUTSIDE_CLOUD_FRACTION: f64 = 0.12;
const RAIN_FALL_SPEED_PA_S: f64 = 50.0;
const SNOW_FALL_SPEED_PA_S: f64 = 5.5;
const RAIN_EVAPORATION_COEFFICIENT: f64 = 1.0;
const SNOW_EVAPORATION_COEFFICIENT: f64 = 0.8;
const MAXIMUM_NEGATIVE_TEMPERATURE_PERTURBATION_K: f64 = 0.9;
const QUASI_EQUILIBRIUM_ALPHA: f64 = 0.2;
const QUASI_EQUILIBRIUM_DAMPING: f64 = 0.1;
const QUASI_EQUILIBRIUM_REFERENCE_INCREMENT: f64 = 0.1;
const NEGATIVE_PROBABILITY_TOLERANCE: f64 = 1.0e-14;
const COLUMN_SUM_TOLERANCE: f64 = 1.0e-12;
const POISSON_TAIL_TOLERANCE: f64 = 1.0e-15;
const MAX_UNIFORMIZATION_MEAN: f64 = 8.0;
const DESTINATION_SAMPLING_DIMENSION: u32 = 96;
const DEEP_CONVECTION_RANDOM_ID: StableRandomId = StableRandomId(0x7be2_949e_34ae_ca8d);

/// One conservative deep-convection transition matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct DeepConvectionKernel {
    /// Destination-major matrix: `transition[destination][source]`.
    transition: Vec<Vec<f64>>,
    height_asl_m: Vec<f64>,
    maximum_column_residual: f64,
    identity: bool,
    diagnostics: Option<DeepConvectionDiagnostics>,
}

/// Deterministic column diagnostics behind one deep-convection kernel.
#[derive(Clone, Debug, PartialEq)]
pub struct DeepConvectionDiagnostics {
    /// Fixed-point cloud-base mass flux from the subcloud quasi-equilibrium closure.
    pub cloud_base_mass_flux_kg_m2_s: f64,
    /// Parcel-origin layer in the input column's top-to-surface order.
    pub origin_layer: usize,
    /// First layer above the lifted condensation level, in top-to-surface order.
    pub cloud_base_layer: usize,
    /// Highest layer reached by positive integrated buoyancy, in top-to-surface order.
    pub cloud_top_layer: usize,
    /// Upward air-mass flux across top-to-surface layer interfaces.
    pub upward_interface_mass_flux_kg_m2_s: Vec<f64>,
    /// Downward air-mass flux across top-to-surface layer interfaces.
    pub downward_interface_mass_flux_kg_m2_s: Vec<f64>,
    /// Precipitation-driven component of the downward interface flux.
    pub precipitation_downdraft_mass_flux_kg_m2_s: Vec<f64>,
}

/// One sampled physical layer transfer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SampledConvectionTransfer {
    pub(crate) source_layer: usize,
    pub(crate) destination_layer: usize,
    pub(crate) transfer_probability: f64,
    pub(crate) importance_weight: f64,
    pub(crate) column_residual: f64,
}

impl DeepConvectionKernel {
    /// Diagnoses a column and integrates its conservative generator for `seconds`.
    pub fn diagnose(column: &ColumnGeometry, seconds: f64) -> Result<Self, PhysicsError> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(science("deep-convection duration is invalid"));
        }
        let first = column
            .available_top_index()
            .map_err(|_| science("deep-convection column has no valid top"))?;
        let last = column
            .lowest_valid_index()
            .map_err(|_| science("deep-convection column has no valid bottom"))?;
        if last <= first
            || column.validity().valid[first..=last]
                .iter()
                .any(|valid| !valid)
        {
            return Err(science("deep-convection column is incomplete"));
        }
        let pressure = &column.pressure_pa()[first..=last];
        let height = &column.height_asl_m()[first..=last];
        let temperature = &column.temperature_k()[first..=last];
        let humidity = &column.specific_humidity()[first..=last];
        validate_column(
            pressure,
            height,
            temperature,
            humidity,
            column.surface_pressure_pa(),
        )?;

        let layer_count = pressure.len();
        let heights = height.to_vec();
        if seconds == 0.0 {
            return Ok(Self::identity(layer_count, heights));
        }
        let layer_mass = layer_air_mass_kg_m2(pressure, column.surface_pressure_pa())?;
        let Some(diagnosis) = diagnose_column(
            pressure,
            height,
            temperature,
            humidity,
            column.surface_pressure_pa(),
        )?
        else {
            return Ok(Self::identity(layer_count, heights));
        };
        let generator = conservative_generator(layer_count, &layer_mass, &diagnosis.transfers)?;
        let (transition, maximum_column_residual) = exponentiate_generator(&generator, seconds)?;
        Ok(Self {
            transition,
            height_asl_m: heights,
            maximum_column_residual,
            identity: false,
            diagnostics: Some(diagnosis.diagnostics),
        })
    }

    fn identity(layer_count: usize, height_asl_m: Vec<f64>) -> Self {
        let mut transition = vec![vec![0.0; layer_count]; layer_count];
        for (index, row) in transition.iter_mut().enumerate() {
            row[index] = 1.0;
        }
        Self {
            transition,
            height_asl_m,
            maximum_column_residual: 0.0,
            identity: true,
            diagnostics: None,
        }
    }

    /// Returns the number of valid top-to-surface layers.
    #[must_use]
    pub fn layer_count(&self) -> usize {
        self.transition.len()
    }

    /// Returns whether zero diagnosed mass flux produced the exact identity.
    #[must_use]
    pub const fn is_identity(&self) -> bool {
        self.identity
    }

    /// Returns the largest pre-closure column residual encountered.
    #[must_use]
    pub const fn maximum_column_residual(&self) -> f64 {
        self.maximum_column_residual
    }

    /// Returns the physical diagnosis, or `None` for an exact zero-flux kernel.
    #[must_use]
    pub const fn diagnostics(&self) -> Option<&DeepConvectionDiagnostics> {
        self.diagnostics.as_ref()
    }

    /// Returns `K[destination, source]`.
    #[must_use]
    pub fn transition_probability(&self, destination: usize, source: usize) -> Option<f64> {
        self.transition
            .get(destination)
            .and_then(|row| row.get(source))
            .copied()
    }

    /// Applies `m_next = K m` to a layer vector.
    pub fn apply_forward(&self, values: &[f64]) -> Result<Vec<f64>, PhysicsError> {
        multiply_matrix_vector(&self.transition, values)
    }

    /// Applies `lambda_prev = K^T lambda_next` to a layer vector.
    pub fn apply_adjoint(&self, values: &[f64]) -> Result<Vec<f64>, PhysicsError> {
        validate_vector(values, self.layer_count())?;
        let mut result = vec![0.0; self.layer_count()];
        for (source, output) in result.iter_mut().enumerate() {
            for (destination, value) in values.iter().copied().enumerate() {
                *output += self.transition[destination][source] * value;
            }
        }
        validate_finite_vector(result)
    }

    pub(crate) fn layer_for_height(&self, height_asl_m: f64) -> Result<usize, PhysicsError> {
        if !height_asl_m.is_finite() || self.height_asl_m.is_empty() {
            return Err(science(
                "particle height cannot be mapped to a convection layer",
            ));
        }
        self.height_asl_m
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                (*left - height_asl_m)
                    .abs()
                    .total_cmp(&(*right - height_asl_m).abs())
            })
            .map(|(index, _)| index)
            .ok_or_else(|| science("deep-convection column is empty"))
    }

    pub(crate) fn layer_height_asl_m(&self, layer: usize) -> Result<f64, PhysicsError> {
        self.height_asl_m
            .get(layer)
            .copied()
            .ok_or_else(|| science("sampled convection layer is outside the column"))
    }

    pub(crate) fn sample_forward(
        &self,
        source: usize,
        random_key: ProcessRandomKey,
    ) -> Result<SampledConvectionTransfer, PhysicsError> {
        if source >= self.layer_count() {
            return Err(science(
                "forward convection source layer is outside the column",
            ));
        }
        let random = CounterRng::sample_process_unit(ProcessRandomKey {
            module: DEEP_CONVECTION_RANDOM_ID,
            sampling_dimension: DESTINATION_SAMPLING_DIMENSION,
            ..random_key
        });
        let destination = sample_categorical(
            (0..self.layer_count()).map(|index| self.transition[index][source]),
            random,
        )?;
        Ok(SampledConvectionTransfer {
            source_layer: source,
            destination_layer: destination,
            transfer_probability: self.transition[destination][source],
            importance_weight: 1.0,
            column_residual: self.maximum_column_residual,
        })
    }

    pub(crate) fn sample_adjoint(
        &self,
        destination: usize,
        random_key: ProcessRandomKey,
    ) -> Result<SampledConvectionTransfer, PhysicsError> {
        if destination >= self.layer_count() {
            return Err(science(
                "backward convection destination layer is outside the column",
            ));
        }
        let row_sum = self.transition[destination].iter().sum::<f64>();
        if !row_sum.is_finite() || row_sum <= 0.0 {
            return Err(science("transpose convection proposal has zero support"));
        }
        let random = CounterRng::sample_process_unit(ProcessRandomKey {
            module: DEEP_CONVECTION_RANDOM_ID,
            sampling_dimension: DESTINATION_SAMPLING_DIMENSION,
            ..random_key
        });
        let source = sample_categorical(
            self.transition[destination]
                .iter()
                .map(|probability| probability / row_sum),
            random,
        )?;
        Ok(SampledConvectionTransfer {
            source_layer: source,
            destination_layer: destination,
            transfer_probability: self.transition[destination][source],
            importance_weight: row_sum,
            column_residual: self.maximum_column_residual,
        })
    }
}

struct DiagnosedColumn {
    diagnostics: DeepConvectionDiagnostics,
    transfers: Vec<MassTransfer>,
}

#[derive(Clone, Copy)]
struct MassTransfer {
    source_bottom_up: usize,
    destination_bottom_up: usize,
    flux_kg_m2_s: f64,
}

#[derive(Clone, Copy)]
struct SortedMixture {
    destination: usize,
    environmental_fraction: f64,
    condensed_water_kg_kg: f64,
}

#[allow(clippy::too_many_arguments)]
fn diagnose_column(
    pressure_top_down_pa: &[f64],
    height_top_down_m: &[f64],
    temperature_top_down_k: &[f64],
    humidity_top_down: &[f64],
    surface_pressure_pa: f64,
) -> Result<Option<DiagnosedColumn>, PhysicsError> {
    let pressure = reversed(pressure_top_down_pa);
    let height = reversed(height_top_down_m);
    let temperature = reversed(temperature_top_down_k);
    let humidity = reversed(humidity_top_down);
    let layer_count = pressure.len();
    let interfaces = pressure_interfaces_bottom_up(&pressure, surface_pressure_pa)?;
    let saturation = pressure
        .iter()
        .copied()
        .zip(temperature.iter().copied())
        .map(|(pressure, temperature)| saturation_specific_humidity(pressure, temperature))
        .collect::<Result<Vec<_>, _>>()?;
    let virtual_temperature = temperature
        .iter()
        .copied()
        .zip(humidity.iter().copied())
        .map(|(temperature, humidity)| {
            temperature
                * (1.0
                    + humidity
                        * (WATER_VAPOR_GAS_CONSTANT_J_KG_K / DRY_AIR_GAS_CONSTANT_J_KG_K - 1.0))
        })
        .collect::<Vec<_>>();
    let base_height = height[0];
    let geopotential = height
        .iter()
        .map(|value| CONVECTION_GRAVITY_M_S2 * (*value - base_height))
        .collect::<Vec<_>>();
    let heat_capacity = humidity
        .iter()
        .map(|q| DRY_AIR_HEAT_CAPACITY_J_KG_K * (1.0 - q) + WATER_VAPOR_HEAT_CAPACITY_J_KG_K * q)
        .collect::<Vec<_>>();
    let latent_heat = temperature
        .iter()
        .map(|value| latent_heat_vaporization(*value))
        .collect::<Vec<_>>();
    let moist_static_energy = (0..layer_count)
        .map(|index| {
            (DRY_AIR_HEAT_CAPACITY_J_KG_K * (1.0 - humidity[index])
                + LIQUID_WATER_HEAT_CAPACITY_J_KG_K * humidity[index])
                * (temperature[index] - temperature[0])
                + latent_heat[index] * humidity[index]
                + geopotential[index]
        })
        .collect::<Vec<_>>();

    let Some(minimum_energy_layer) = (1..layer_count).fold(None, |minimum, index| {
        if moist_static_energy[index] < moist_static_energy[index - 1]
            && minimum
                .is_none_or(|current| moist_static_energy[index] < moist_static_energy[current])
        {
            Some(index)
        } else {
            minimum
        }
    }) else {
        return Ok(None);
    };
    if minimum_energy_layer >= layer_count - 2 {
        return Ok(None);
    }
    let origin = (0..=minimum_energy_layer)
        .max_by(|left, right| moist_static_energy[*left].total_cmp(&moist_static_energy[*right]))
        .ok_or_else(|| science("deep-convection parcel origin is absent"))?;
    if temperature[origin] < 250.0 || humidity[origin] <= 0.0 {
        return Ok(None);
    }
    let relative_humidity = (humidity[origin] / saturation[origin]).clamp(1.0e-12, 1.0);
    let chi_denominator = 1_669.0 - 122.0 * relative_humidity - temperature[origin];
    if !chi_denominator.is_finite() || chi_denominator <= 0.0 {
        return Err(science("lifted condensation level diagnosis failed"));
    }
    let lcl_pressure =
        pressure[origin] * relative_humidity.powf(temperature[origin] / chi_denominator);
    if !lcl_pressure.is_finite() || !(20_000.0..200_000.0).contains(&lcl_pressure) {
        return Ok(None);
    }
    let Some(cloud_base) =
        ((origin + 1)..layer_count).find(|index| pressure[*index] < lcl_pressure)
    else {
        return Ok(None);
    };
    if cloud_base >= layer_count - 1 {
        return Ok(None);
    }

    let source_total_water = humidity[origin];
    let source_static_energy = (DRY_AIR_HEAT_CAPACITY_J_KG_K * (1.0 - source_total_water)
        + LIQUID_WATER_HEAT_CAPACITY_J_KG_K * source_total_water)
        * temperature[origin]
        + source_total_water * latent_heat[origin]
        + geopotential[origin];
    let source_heat_capacity = DRY_AIR_HEAT_CAPACITY_J_KG_K * (1.0 - source_total_water)
        + WATER_VAPOR_HEAT_CAPACITY_J_KG_K * source_total_water;
    let mut parcel_temperature = temperature.clone();
    let mut parcel_density_temperature = vec![0.0; layer_count];
    let mut parcel_condensed_water = vec![0.0; layer_count];
    for index in origin..cloud_base {
        parcel_temperature[index] = temperature[origin]
            - (geopotential[index] - geopotential[origin]) / source_heat_capacity;
        parcel_density_temperature[index] = parcel_temperature[index]
            * (1.0
                + source_total_water
                    * (WATER_VAPOR_GAS_CONSTANT_J_KG_K / DRY_AIR_GAS_CONSTANT_J_KG_K - 1.0));
    }
    for index in cloud_base..layer_count {
        let mut trial_temperature = temperature[index];
        let mut saturated_humidity = saturation[index];
        let parcel_latent_heat = latent_heat[index];
        for _ in 0..2 {
            let inverse_derivative = 1.0
                / (DRY_AIR_HEAT_CAPACITY_J_KG_K
                    + parcel_latent_heat.powi(2) * saturated_humidity
                        / (WATER_VAPOR_GAS_CONSTANT_J_KG_K * temperature[index].powi(2)));
            let trial_static_energy = DRY_AIR_HEAT_CAPACITY_J_KG_K * trial_temperature
                + (LIQUID_WATER_HEAT_CAPACITY_J_KG_K - DRY_AIR_HEAT_CAPACITY_J_KG_K)
                    * source_total_water
                    * temperature[index]
                + parcel_latent_heat * saturated_humidity
                + geopotential[index];
            trial_temperature = (trial_temperature
                + inverse_derivative * (source_static_energy - trial_static_energy))
                .max(35.0);
            saturated_humidity = saturation_specific_humidity(pressure[index], trial_temperature)?;
        }
        parcel_temperature[index] = (source_static_energy
            - (LIQUID_WATER_HEAT_CAPACITY_J_KG_K - DRY_AIR_HEAT_CAPACITY_J_KG_K)
                * source_total_water
                * temperature[index]
            - geopotential[index]
            - parcel_latent_heat * saturated_humidity)
            / DRY_AIR_HEAT_CAPACITY_J_KG_K;
        parcel_condensed_water[index] = (source_total_water - saturated_humidity).max(0.0);
        let parcel_mixing_ratio = saturated_humidity / (1.0 - source_total_water);
        parcel_density_temperature[index] = parcel_temperature[index]
            * (1.0
                + parcel_mixing_ratio * WATER_VAPOR_GAS_CONSTANT_J_KG_K
                    / DRY_AIR_GAS_CONSTANT_J_KG_K
                - source_total_water);
    }
    if parcel_density_temperature[cloud_base]
        <= virtual_temperature[cloud_base] - MAXIMUM_NEGATIVE_TEMPERATURE_PERTURBATION_K
    {
        return Ok(None);
    }

    let mut cumulative_buoyancy = 0.0;
    let mut highest_positive_layer = cloud_base + 1;
    let mut highest_neutral_layer = cloud_base + 1;
    for index in (cloud_base + 1)..(layer_count - 1) {
        let contribution = (parcel_density_temperature[index] - virtual_temperature[index])
            * (interfaces[index] - interfaces[index + 1])
            / pressure[index];
        cumulative_buoyancy += contribution;
        if contribution >= 0.0 {
            highest_neutral_layer = index + 1;
        }
        if cumulative_buoyancy > 0.0 {
            highest_positive_layer = index + 1;
        }
    }
    let cloud_top = highest_positive_layer.max(highest_neutral_layer);

    let below_cloud_base = cloud_base - 1;
    let parcel_at_lcl = parcel_density_temperature[below_cloud_base]
        - DRY_AIR_GAS_CONSTANT_J_KG_K
            * parcel_density_temperature[below_cloud_base]
            * (pressure[below_cloud_base] - lcl_pressure)
            / (heat_capacity[below_cloud_base] * pressure[below_cloud_base]);
    let environment_at_lcl = virtual_temperature[cloud_base]
        + (parcel_density_temperature[cloud_base] - parcel_density_temperature[cloud_base + 1])
            * (lcl_pressure - pressure[cloud_base])
            / (pressure[cloud_base] - pressure[cloud_base + 1]);
    let pressure_depth = interfaces[origin] - interfaces[cloud_base];
    if pressure_depth <= 0.0 {
        return Err(science("subcloud pressure depth is invalid"));
    }
    let subcloud_perturbation = (origin..cloud_base)
        .map(|index| {
            (parcel_density_temperature[index] - virtual_temperature[index])
                * (interfaces[index] - interfaces[index + 1])
        })
        .sum::<f64>()
        / pressure_depth;
    let closure_perturbation = parcel_at_lcl - environment_at_lcl
        + MAXIMUM_NEGATIVE_TEMPERATURE_PERTURBATION_K
        + subcloud_perturbation;
    let cloud_base_mass_flux = (QUASI_EQUILIBRIUM_REFERENCE_INCREMENT * QUASI_EQUILIBRIUM_ALPHA
        / QUASI_EQUILIBRIUM_DAMPING
        * closure_perturbation)
        .max(0.0);
    if !cloud_base_mass_flux.is_finite() {
        return Err(science("diagnosed cloud-base mass flux is invalid"));
    }
    if cloud_base_mass_flux == 0.0 {
        return Ok(None);
    }

    let mut mixing_mass_flux = vec![0.0; layer_count];
    let mixing_weights = ((cloud_base + 1)..=cloud_top)
        .map(|index| {
            let buoyancy_index = index.min(highest_neutral_layer);
            (virtual_temperature[buoyancy_index] - parcel_density_temperature[buoyancy_index]).abs()
                + ENTRAINMENT_COEFFICIENT
                    * 2.0e-4
                    * (interfaces[buoyancy_index] - interfaces[buoyancy_index + 1])
        })
        .collect::<Vec<_>>();
    let mixing_weight_sum = mixing_weights.iter().sum::<f64>();
    if !mixing_weight_sum.is_finite() || mixing_weight_sum <= 0.0 {
        return Err(science("deep-convection mixing-rate closure failed"));
    }
    for (index, weight) in ((cloud_base + 1)..=cloud_top).zip(mixing_weights) {
        mixing_mass_flux[index] = cloud_base_mass_flux * weight / mixing_weight_sum;
    }

    let precipitation_efficiency = (0..layer_count)
        .map(|index| {
            let celsius = parcel_temperature[index] - 273.15;
            let critical_water = if celsius >= 0.0 {
                AUTOCONVERSION_THRESHOLD_KG_KG
            } else {
                (AUTOCONVERSION_THRESHOLD_KG_KG
                    * (1.0 - celsius / AUTOCONVERSION_CRITICAL_TEMPERATURE_C))
                    .max(0.0)
            };
            (0.999 * (1.0 - critical_water / parcel_condensed_water[index].max(1.0e-8)))
                .clamp(0.0, 0.999)
        })
        .collect::<Vec<_>>();
    let static_energy = (0..layer_count)
        .map(|index| heat_capacity[index] * temperature[index] + geopotential[index])
        .collect::<Vec<_>>();
    let mut parcel_static_energy = static_energy.clone();
    for index in cloud_base..=cloud_top {
        parcel_static_energy[index] = static_energy[origin]
            + (latent_heat[index]
                + (DRY_AIR_HEAT_CAPACITY_J_KG_K - WATER_VAPOR_HEAT_CAPACITY_J_KG_K)
                    * temperature[index])
                * precipitation_efficiency[index]
                * parcel_condensed_water[index];
    }

    let mut entrained_mass_flux = vec![vec![0.0; layer_count]; layer_count];
    let mut mixture_condensed_water = vec![vec![0.0; layer_count]; layer_count];
    let mut transfers = Vec::new();
    for mixing_layer in (cloud_base + 1)..=cloud_top {
        let plume_total_water = source_total_water
            - precipitation_efficiency[mixing_layer] * parcel_condensed_water[mixing_layer];
        let mut mixtures = Vec::new();
        for destination in cloud_base..=cloud_top {
            let saturation_derivative = 1.0
                + latent_heat[destination].powi(2) * saturation[destination]
                    / (WATER_VAPOR_GAS_CONSTANT_J_KG_K
                        * temperature[destination].powi(2)
                        * DRY_AIR_HEAT_CAPACITY_J_KG_K);
            let numerator = static_energy[destination] - parcel_static_energy[mixing_layer]
                + (WATER_VAPOR_HEAT_CAPACITY_J_KG_K - DRY_AIR_HEAT_CAPACITY_J_KG_K)
                    * temperature[destination]
                    * (plume_total_water - humidity[destination]);
            let mut denominator = static_energy[mixing_layer] - parcel_static_energy[mixing_layer]
                + (DRY_AIR_HEAT_CAPACITY_J_KG_K - WATER_VAPOR_HEAT_CAPACITY_J_KG_K)
                    * (humidity[mixing_layer] - plume_total_water)
                    * temperature[destination];
            if denominator.abs() < 0.01 {
                denominator = 0.01;
            }
            let mut fraction = numerator / denominator;
            let mut condensed = (fraction * humidity[mixing_layer]
                + (1.0 - fraction) * plume_total_water
                - saturation[destination])
                / saturation_derivative;
            let retained_cloud_water =
                parcel_condensed_water[destination] * (1.0 - precipitation_efficiency[destination]);
            if (!(0.0..=1.0).contains(&fraction) || condensed > retained_cloud_water)
                && destination > mixing_layer
            {
                let adjusted_numerator = numerator
                    - latent_heat[destination]
                        * (plume_total_water
                            - saturation[destination]
                            - retained_cloud_water * saturation_derivative);
                let adjusted_denominator = denominator
                    + latent_heat[destination] * (humidity[mixing_layer] - plume_total_water);
                fraction = adjusted_numerator
                    / if adjusted_denominator.abs() < 0.01 {
                        0.01
                    } else {
                        adjusted_denominator
                    };
                condensed = fraction * humidity[mixing_layer]
                    + (1.0 - fraction) * plume_total_water
                    - saturation[destination]
                    - (saturation_derivative - 1.0) * retained_cloud_water;
            }
            if fraction > 0.0 && fraction < 0.9 {
                mixtures.push(SortedMixture {
                    destination,
                    environmental_fraction: fraction,
                    condensed_water_kg_kg: condensed.max(0.0),
                });
            }
        }
        if mixtures.is_empty() {
            entrained_mass_flux[mixing_layer][mixing_layer] = mixing_mass_flux[mixing_layer];
            mixture_condensed_water[mixing_layer][mixing_layer] =
                parcel_condensed_water[mixing_layer];
            add_mass_transfer(
                &mut transfers,
                origin,
                mixing_layer,
                mixing_mass_flux[mixing_layer],
            )?;
            continue;
        }
        mixtures.sort_by(|left, right| {
            left.environmental_fraction
                .total_cmp(&right.environmental_fraction)
        });
        let raw_weights = mixtures
            .iter()
            .enumerate()
            .map(|(index, mixture)| {
                let lower = if index == 0 {
                    0.0
                } else {
                    0.5 * (mixtures[index - 1].environmental_fraction
                        + mixture.environmental_fraction)
                };
                let upper = if index + 1 == mixtures.len() {
                    1.0
                } else {
                    0.5 * (mixture.environmental_fraction
                        + mixtures[index + 1].environmental_fraction)
                };
                (upper - lower).max(0.0)
                    * (interfaces[mixture.destination] - interfaces[mixture.destination + 1])
                    / (1.0 - mixture.environmental_fraction)
            })
            .collect::<Vec<_>>();
        let normalization = raw_weights
            .iter()
            .zip(&mixtures)
            .map(|(weight, mixture)| weight * (1.0 - mixture.environmental_fraction))
            .sum::<f64>();
        if !normalization.is_finite() || normalization <= 0.0 {
            return Err(science("buoyancy-sorting distribution does not close"));
        }
        for (mixture, raw_weight) in mixtures.into_iter().zip(raw_weights) {
            let mass_flux = mixing_mass_flux[mixing_layer] * raw_weight / normalization;
            entrained_mass_flux[mixing_layer][mixture.destination] = mass_flux;
            mixture_condensed_water[mixing_layer][mixture.destination] =
                mixture.condensed_water_kg_kg;
            add_mass_transfer(
                &mut transfers,
                mixing_layer,
                mixture.destination,
                mass_flux * mixture.environmental_fraction,
            )?;
            add_mass_transfer(
                &mut transfers,
                origin,
                mixture.destination,
                mass_flux * (1.0 - mixture.environmental_fraction),
            )?;
        }
    }

    let precipitation_downdraft = precipitation_downdraft_flux(
        &pressure,
        &interfaces,
        &temperature,
        &humidity,
        &saturation,
        &geopotential,
        &latent_heat,
        &static_energy,
        &parcel_condensed_water,
        &precipitation_efficiency,
        &mixing_mass_flux,
        &entrained_mass_flux,
        &mixture_condensed_water,
        cloud_top,
    )?;
    for (interface, mass_flux) in precipitation_downdraft
        .iter()
        .copied()
        .enumerate()
        .take(cloud_top)
    {
        add_mass_transfer(&mut transfers, interface + 1, interface, mass_flux)?;
    }
    close_interface_mass_fluxes(layer_count, &mut transfers)?;
    let (upward_bottom_up, downward_bottom_up) = interface_mass_fluxes(layer_count, &transfers);
    let diagnostics = DeepConvectionDiagnostics {
        cloud_base_mass_flux_kg_m2_s: cloud_base_mass_flux,
        origin_layer: layer_count - 1 - origin,
        cloud_base_layer: layer_count - 1 - cloud_base,
        cloud_top_layer: layer_count - 1 - cloud_top,
        upward_interface_mass_flux_kg_m2_s: reversed(&upward_bottom_up),
        downward_interface_mass_flux_kg_m2_s: reversed(&downward_bottom_up),
        precipitation_downdraft_mass_flux_kg_m2_s: reversed(&precipitation_downdraft),
    };
    Ok(Some(DiagnosedColumn {
        diagnostics,
        transfers,
    }))
}

#[allow(clippy::too_many_arguments)]
fn precipitation_downdraft_flux(
    pressure_pa: &[f64],
    interface_pressure_pa: &[f64],
    temperature_k: &[f64],
    specific_humidity: &[f64],
    saturation_humidity: &[f64],
    geopotential_m2_s2: &[f64],
    latent_heat_j_kg: &[f64],
    static_energy_j_kg: &[f64],
    parcel_condensed_water: &[f64],
    precipitation_efficiency: &[f64],
    mixing_mass_flux: &[f64],
    entrained_mass_flux: &[Vec<f64>],
    mixture_condensed_water: &[Vec<f64>],
    cloud_top: usize,
) -> Result<Vec<f64>, PhysicsError> {
    let layer_count = pressure_pa.len();
    let mut precipitation_water = vec![0.0; layer_count + 1];
    let mut fall_speed = vec![SNOW_FALL_SPEED_PA_S; layer_count + 1];
    let mut downdraft = vec![0.0; layer_count + 1];
    let mut downdraft_humidity = vec![specific_humidity[0]; layer_count + 1];
    downdraft_humidity[1..].copy_from_slice(specific_humidity);
    let pressure_hpa = pressure_pa
        .iter()
        .map(|value| value * 0.01)
        .collect::<Vec<_>>();
    let interface_hpa = interface_pressure_pa
        .iter()
        .map(|value| value * 0.01)
        .collect::<Vec<_>>();
    let mut taper_origin = 1_usize.min(cloud_top);
    for index in (0..=cloud_top).rev() {
        let mut detrained_water = CONVECTION_GRAVITY_M_S2
            * precipitation_efficiency[index]
            * mixing_mass_flux[index]
            * parcel_condensed_water[index];
        for mixing_layer in 0..index {
            let excess = (mixture_condensed_water[mixing_layer][index]
                - (1.0 - precipitation_efficiency[index]) * parcel_condensed_water[index])
                .max(0.0);
            detrained_water +=
                CONVECTION_GRAVITY_M_S2 * excess * entrained_mass_flux[mixing_layer][index];
        }
        let (coefficient, speed) = if temperature_k[index] > 273.0 {
            (RAIN_EVAPORATION_COEFFICIENT, RAIN_FALL_SPEED_PA_S)
        } else {
            (SNOW_EVAPORATION_COEFFICIENT, SNOW_FALL_SPEED_PA_S)
        };
        fall_speed[index] = speed;
        let provisional_humidity = 0.5 * (specific_humidity[index] + downdraft_humidity[index + 1]);
        let evaporation_factor = (coefficient
            * interface_hpa[index]
            * (saturation_humidity[index] - provisional_humidity)
            / (1.0e4 + 2.0e3 * interface_hpa[index] * saturation_humidity[index]))
            .max(0.0);
        let b = 100.0
            * (interface_hpa[index] - interface_hpa[index + 1])
            * PRECIPITATION_OUTSIDE_CLOUD_FRACTION
            * evaporation_factor
            / fall_speed[index];
        let c = (precipitation_water[index + 1] * fall_speed[index + 1]
            + detrained_water / UNSATURATED_DOWNDRAFT_AREA_FRACTION)
            / fall_speed[index];
        let falling_water = 0.5 * (-b + (b * b + 4.0 * c).sqrt());
        precipitation_water[index] = falling_water * falling_water;
        let evaporation = PRECIPITATION_OUTSIDE_CLOUD_FRACTION * evaporation_factor * falling_water;
        if index == 0 {
            continue;
        }
        let enthalpy_gradient = ((static_energy_j_kg[index] - static_energy_j_kg[index - 1])
            / (pressure_hpa[index - 1] - pressure_hpa[index]))
            .max(10.0);
        downdraft[index] = (100.0 / CONVECTION_GRAVITY_M_S2
            * latent_heat_j_kg[index]
            * UNSATURATED_DOWNDRAFT_AREA_FRACTION
            * evaporation
            / enthalpy_gradient)
            .max(0.0);
        let inertia = 20.0 / (interface_hpa[index - 1] - interface_hpa[index]);
        downdraft[index] = (inertia * downdraft[index + 1] + downdraft[index]) / (1.0 + inertia);
        if pressure_pa[index] > 0.949 * pressure_pa[0] {
            taper_origin = taper_origin.max(index);
            let denominator = pressure_pa[0] - pressure_pa[taper_origin];
            if denominator > 0.0 {
                downdraft[index] =
                    downdraft[taper_origin] * (pressure_pa[0] - pressure_pa[index]) / denominator;
            }
        }
        if index == cloud_top {
            continue;
        }
        let maximum_humidity = if index == 0 {
            saturation_humidity[0]
        } else {
            saturation_humidity[index - 1]
        };
        if downdraft[index] > downdraft[index + 1] {
            let retained = downdraft[index + 1] / downdraft[index];
            downdraft_humidity[index] = downdraft_humidity[index + 1] * retained
                + specific_humidity[index] * (1.0 - retained)
                + 100.0 / CONVECTION_GRAVITY_M_S2
                    * UNSATURATED_DOWNDRAFT_AREA_FRACTION
                    * (interface_hpa[index] - interface_hpa[index + 1])
                    * evaporation
                    / downdraft[index];
        } else if downdraft[index + 1] > 0.0 {
            downdraft_humidity[index] = (geopotential_m2_s2[index + 1] - geopotential_m2_s2[index]
                + downdraft_humidity[index + 1]
                    * (latent_heat_j_kg[index + 1]
                        + temperature_k[index + 1]
                            * (LIQUID_WATER_HEAT_CAPACITY_J_KG_K - DRY_AIR_HEAT_CAPACITY_J_KG_K))
                + DRY_AIR_HEAT_CAPACITY_J_KG_K * (temperature_k[index + 1] - temperature_k[index]))
                / (latent_heat_j_kg[index]
                    + temperature_k[index]
                        * (LIQUID_WATER_HEAT_CAPACITY_J_KG_K - DRY_AIR_HEAT_CAPACITY_J_KG_K));
        }
        downdraft_humidity[index] = downdraft_humidity[index].clamp(0.0, maximum_humidity);
    }
    let interface_flux = (0..(layer_count - 1))
        .map(|interface| downdraft[interface + 1])
        .collect::<Vec<_>>();
    if interface_flux
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0)
    {
        Ok(interface_flux)
    } else {
        Err(science("precipitation-driven downdraft is invalid"))
    }
}

fn add_mass_transfer(
    transfers: &mut Vec<MassTransfer>,
    source: usize,
    destination: usize,
    flux_kg_m2_s: f64,
) -> Result<(), PhysicsError> {
    if source == destination || flux_kg_m2_s == 0.0 {
        return Ok(());
    }
    if !flux_kg_m2_s.is_finite() || flux_kg_m2_s < 0.0 {
        return Err(science("deep-convection mass transfer is invalid"));
    }
    transfers.push(MassTransfer {
        source_bottom_up: source,
        destination_bottom_up: destination,
        flux_kg_m2_s,
    });
    Ok(())
}

fn close_interface_mass_fluxes(
    layer_count: usize,
    transfers: &mut Vec<MassTransfer>,
) -> Result<(), PhysicsError> {
    for interface in 0..(layer_count - 1) {
        let (upward, downward) = interface_mass_flux(layer_count, transfers, interface);
        if upward > downward {
            add_mass_transfer(transfers, interface + 1, interface, upward - downward)?;
        } else if downward > upward {
            add_mass_transfer(transfers, interface, interface + 1, downward - upward)?;
        }
    }
    Ok(())
}

fn interface_mass_fluxes(layer_count: usize, transfers: &[MassTransfer]) -> (Vec<f64>, Vec<f64>) {
    let mut upward = Vec::with_capacity(layer_count - 1);
    let mut downward = Vec::with_capacity(layer_count - 1);
    for interface in 0..(layer_count - 1) {
        let values = interface_mass_flux(layer_count, transfers, interface);
        upward.push(values.0);
        downward.push(values.1);
    }
    (upward, downward)
}

fn interface_mass_flux(
    layer_count: usize,
    transfers: &[MassTransfer],
    interface: usize,
) -> (f64, f64) {
    debug_assert!(interface + 1 < layer_count);
    transfers.iter().fold((0.0, 0.0), |mut flux, transfer| {
        if transfer.source_bottom_up <= interface && transfer.destination_bottom_up > interface {
            flux.0 += transfer.flux_kg_m2_s;
        } else if transfer.destination_bottom_up <= interface
            && transfer.source_bottom_up > interface
        {
            flux.1 += transfer.flux_kg_m2_s;
        }
        flux
    })
}

fn reversed(values: &[f64]) -> Vec<f64> {
    values.iter().rev().copied().collect()
}

fn pressure_interfaces_bottom_up(
    pressure_pa: &[f64],
    surface_pressure_pa: f64,
) -> Result<Vec<f64>, PhysicsError> {
    let mut interfaces = Vec::with_capacity(pressure_pa.len() + 1);
    interfaces.push(surface_pressure_pa);
    interfaces.extend(pressure_pa.windows(2).map(|pair| 0.5 * (pair[0] + pair[1])));
    let top_pressure = pressure_pa[pressure_pa.len() - 1];
    let extrapolated_top = top_pressure - 0.5 * (pressure_pa[pressure_pa.len() - 2] - top_pressure);
    interfaces.push(extrapolated_top.clamp(0.01 * top_pressure, 0.999_999 * top_pressure));
    if interfaces.windows(2).all(|pair| pair[0] > pair[1]) {
        Ok(interfaces)
    } else {
        Err(science("deep-convection pressure interfaces are invalid"))
    }
}

fn latent_heat_vaporization(temperature_k: f64) -> f64 {
    LATENT_HEAT_AT_FREEZING_J_KG
        - (LIQUID_WATER_HEAT_CAPACITY_J_KG_K - WATER_VAPOR_HEAT_CAPACITY_J_KG_K)
            * (temperature_k - 273.15)
}

fn saturation_vapor_pressure_pa(temperature_k: f64) -> Result<f64, PhysicsError> {
    let celsius = temperature_k - 273.15;
    let vapor_pressure = if celsius >= 0.0 {
        611.2 * (17.67 * celsius / (celsius + 243.5)).exp()
    } else {
        100.0 * (23.330_86 - 6_111.727_84 / temperature_k + 0.152_15 * temperature_k.ln()).exp()
    };
    if vapor_pressure.is_finite() && vapor_pressure > 0.0 {
        Ok(vapor_pressure)
    } else {
        Err(science("saturation vapor pressure failed"))
    }
}

fn saturation_specific_humidity(pressure_pa: f64, temperature_k: f64) -> Result<f64, PhysicsError> {
    let vapor_pressure = saturation_vapor_pressure_pa(temperature_k)?.min(0.999 * pressure_pa);
    let denominator =
        pressure_pa - (1.0 - WATER_VAPOR_TO_DRY_AIR_MOLAR_MASS_RATIO) * vapor_pressure;
    let value = WATER_VAPOR_TO_DRY_AIR_MOLAR_MASS_RATIO * vapor_pressure / denominator;
    if value.is_finite() && value >= 0.0 {
        Ok(value.min(0.999_999))
    } else {
        Err(science("saturation specific humidity is invalid"))
    }
}

fn layer_air_mass_kg_m2(
    pressure_pa: &[f64],
    surface_pressure_pa: f64,
) -> Result<Vec<f64>, PhysicsError> {
    let mut interfaces = Vec::with_capacity(pressure_pa.len() + 1);
    let extrapolated_top = pressure_pa[0] - 0.5 * (pressure_pa[1] - pressure_pa[0]);
    interfaces.push(extrapolated_top.clamp(0.01 * pressure_pa[0], 0.999_999 * pressure_pa[0]));
    interfaces.extend(pressure_pa.windows(2).map(|pair| 0.5 * (pair[0] + pair[1])));
    interfaces.push(surface_pressure_pa);
    let mass = interfaces
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) / CONVECTION_GRAVITY_M_S2)
        .collect::<Vec<_>>();
    if mass.iter().all(|value| value.is_finite() && *value > 0.0) {
        Ok(mass)
    } else {
        Err(science("deep-convection pressure-layer mass is invalid"))
    }
}

fn conservative_generator(
    layer_count: usize,
    layer_mass_kg_m2: &[f64],
    transfers: &[MassTransfer],
) -> Result<Vec<Vec<f64>>, PhysicsError> {
    let mut generator = vec![vec![0.0; layer_count]; layer_count];
    for transfer in transfers {
        let source = layer_count - 1 - transfer.source_bottom_up;
        let destination = layer_count - 1 - transfer.destination_bottom_up;
        add_transition_rate(
            &mut generator,
            source,
            destination,
            transfer.flux_kg_m2_s / layer_mass_kg_m2[source],
        )?;
    }
    Ok(generator)
}

fn add_transition_rate(
    generator: &mut [Vec<f64>],
    source: usize,
    destination: usize,
    rate_s_1: f64,
) -> Result<(), PhysicsError> {
    if source == destination {
        return Ok(());
    }
    if !rate_s_1.is_finite() || rate_s_1 < 0.0 {
        return Err(science("deep-convection transition rate is invalid"));
    }
    generator[destination][source] += rate_s_1;
    generator[source][source] -= rate_s_1;
    Ok(())
}

fn exponentiate_generator(
    generator: &[Vec<f64>],
    seconds: f64,
) -> Result<(Vec<Vec<f64>>, f64), PhysicsError> {
    let layer_count = generator.len();
    validate_square_matrix(generator)?;
    let uniformization_rate = (0..layer_count)
        .map(|index| -generator[index][index])
        .fold(0.0_f64, f64::max);
    if uniformization_rate == 0.0 {
        return Ok((identity_matrix(layer_count), 0.0));
    }
    if !uniformization_rate.is_finite() || uniformization_rate < 0.0 {
        return Err(science(
            "deep-convection generator has an invalid exit rate",
        ));
    }
    let mean = uniformization_rate * seconds;
    let squarings = if mean <= MAX_UNIFORMIZATION_MEAN {
        0
    } else {
        (mean / MAX_UNIFORMIZATION_MEAN).log2().ceil() as u32
    };
    if squarings > 60 {
        return Err(PhysicsError::ResourceLimit);
    }
    let scale = 2_f64.powi(i32::try_from(squarings).map_err(|_| PhysicsError::ResourceLimit)?);
    let scaled_mean = mean / scale;
    let mut poisson = (-scaled_mean).exp();
    let mut cumulative = poisson;
    let mut transition = vec![vec![0.0; layer_count]; layer_count];
    let mut term = identity_matrix(layer_count);
    for (index, row) in transition.iter_mut().enumerate() {
        row[index] = poisson;
    }
    let mut embedded = identity_matrix(layer_count);
    for destination in 0..layer_count {
        for source in 0..layer_count {
            embedded[destination][source] += generator[destination][source] / uniformization_rate;
        }
    }
    let mut maximum_residual = validate_and_close_columns(&mut embedded)?;
    for order in 1_u32..=512 {
        term = multiply_matrices(&embedded, &term)?;
        poisson *= scaled_mean / f64::from(order);
        cumulative += poisson;
        for destination in 0..layer_count {
            for source in 0..layer_count {
                transition[destination][source] += poisson * term[destination][source];
            }
        }
        if order as f64 > scaled_mean && (1.0 - cumulative).abs() <= POISSON_TAIL_TOLERANCE {
            break;
        }
        if order == 512 {
            return Err(PhysicsError::ResourceLimit);
        }
    }
    maximum_residual = maximum_residual.max(validate_and_close_columns(&mut transition)?);
    for _ in 0..squarings {
        transition = multiply_matrices(&transition, &transition)?;
        maximum_residual = maximum_residual.max(validate_and_close_columns(&mut transition)?);
    }
    Ok((transition, maximum_residual))
}

fn multiply_matrices(left: &[Vec<f64>], right: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, PhysicsError> {
    validate_square_matrix(left)?;
    validate_square_matrix(right)?;
    if left.len() != right.len() {
        return Err(science("deep-convection matrix dimensions differ"));
    }
    let count = left.len();
    let mut product = vec![vec![0.0; count]; count];
    for destination in 0..count {
        for intermediate in 0..count {
            let coefficient = left[destination][intermediate];
            if coefficient == 0.0 {
                continue;
            }
            for source in 0..count {
                product[destination][source] += coefficient * right[intermediate][source];
            }
        }
    }
    if product.iter().flatten().all(|value| value.is_finite()) {
        Ok(product)
    } else {
        Err(science(
            "deep-convection matrix multiplication is non-finite",
        ))
    }
}

fn multiply_matrix_vector(matrix: &[Vec<f64>], values: &[f64]) -> Result<Vec<f64>, PhysicsError> {
    validate_square_matrix(matrix)?;
    validate_vector(values, matrix.len())?;
    let mut result = vec![0.0; matrix.len()];
    for (destination, output) in result.iter_mut().enumerate() {
        for (source, value) in values.iter().copied().enumerate() {
            *output += matrix[destination][source] * value;
        }
    }
    validate_finite_vector(result)
}

fn validate_and_close_columns(matrix: &mut [Vec<f64>]) -> Result<f64, PhysicsError> {
    validate_square_matrix(matrix)?;
    let count = matrix.len();
    let mut maximum_residual = 0.0_f64;
    for source in 0..count {
        for row in matrix.iter_mut() {
            let probability = &mut row[source];
            if *probability < -NEGATIVE_PROBABILITY_TOLERANCE {
                return Err(science(
                    "deep-convection matrix contains a negative probability",
                ));
            }
            if *probability < 0.0 {
                *probability = 0.0;
            }
        }
        let sum = matrix.iter().map(|row| row[source]).sum::<f64>();
        let residual = 1.0 - sum;
        maximum_residual = maximum_residual.max(residual.abs());
        if !residual.is_finite() || residual.abs() > COLUMN_SUM_TOLERANCE {
            return Err(science("deep-convection matrix column does not close"));
        }
        let closed_diagonal = matrix[source][source] + residual;
        if closed_diagonal >= 0.0 {
            matrix[source][source] = closed_diagonal;
        } else if closed_diagonal >= -NEGATIVE_PROBABILITY_TOLERANCE {
            matrix[source][source] = 0.0;
            let correction = -closed_diagonal;
            let (row, largest) = matrix
                .iter()
                .enumerate()
                .filter(|(row, _)| *row != source)
                .max_by(|(_, left), (_, right)| left[source].total_cmp(&right[source]))
                .map(|(row, values)| (row, values[source]))
                .ok_or_else(|| science("deep-convection column has no closure support"))?;
            if largest < correction {
                return Err(science(
                    "deep-convection column closure exhausted its support",
                ));
            }
            matrix[row][source] -= correction;
        } else {
            return Err(science(
                "deep-convection column closure made the diagonal negative",
            ));
        }
    }
    Ok(maximum_residual)
}

fn validate_square_matrix(matrix: &[Vec<f64>]) -> Result<(), PhysicsError> {
    let count = matrix.len();
    if count == 0
        || matrix.iter().any(|row| row.len() != count)
        || matrix.iter().flatten().any(|value| !value.is_finite())
    {
        Err(science("deep-convection matrix is invalid"))
    } else {
        Ok(())
    }
}

fn validate_column(
    pressure_pa: &[f64],
    height_asl_m: &[f64],
    temperature_k: &[f64],
    specific_humidity: &[f64],
    surface_pressure_pa: f64,
) -> Result<(), PhysicsError> {
    let count = pressure_pa.len();
    if count < 2
        || height_asl_m.len() != count
        || temperature_k.len() != count
        || specific_humidity.len() != count
        || pressure_pa
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        || pressure_pa.windows(2).any(|pair| pair[1] <= pair[0])
        || height_asl_m.iter().any(|value| !value.is_finite())
        || height_asl_m.windows(2).any(|pair| pair[1] >= pair[0])
        || temperature_k
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        || specific_humidity
            .iter()
            .any(|value| !value.is_finite() || !(0.0..1.0).contains(value))
        || !surface_pressure_pa.is_finite()
        || surface_pressure_pa < pressure_pa[count - 1]
    {
        Err(science("deep-convection thermodynamic column is invalid"))
    } else {
        Ok(())
    }
}

fn validate_vector(values: &[f64], expected: usize) -> Result<(), PhysicsError> {
    if values.len() != expected || values.iter().any(|value| !value.is_finite()) {
        Err(science("deep-convection layer vector is invalid"))
    } else {
        Ok(())
    }
}

fn validate_finite_vector(values: Vec<f64>) -> Result<Vec<f64>, PhysicsError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(values)
    } else {
        Err(science("deep-convection result vector is non-finite"))
    }
}

fn identity_matrix(count: usize) -> Vec<Vec<f64>> {
    let mut matrix = vec![vec![0.0; count]; count];
    for (index, row) in matrix.iter_mut().enumerate() {
        row[index] = 1.0;
    }
    matrix
}

fn sample_categorical(
    probabilities: impl Iterator<Item = f64>,
    random: f64,
) -> Result<usize, PhysicsError> {
    if !random.is_finite() || !(0.0..1.0).contains(&random) {
        return Err(science("deep-convection random sample is invalid"));
    }
    let mut cumulative = 0.0;
    let mut last = None;
    for (index, probability) in probabilities.enumerate() {
        if !probability.is_finite() || probability < 0.0 {
            return Err(science(
                "deep-convection categorical probability is invalid",
            ));
        }
        cumulative += probability;
        last = Some(index);
        if random < cumulative {
            return Ok(index);
        }
    }
    last.filter(|_| (1.0 - cumulative).abs() <= COLUMN_SUM_TOLERANCE)
        .ok_or_else(|| science("deep-convection categorical distribution does not close"))
}

fn science(message: &str) -> PhysicsError {
    PhysicsError::Scientific(message.to_owned())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::Arc;

    use crate::particle::ParticleId;
    use trajecta_met::vertical::VerticalValidity;

    use super::*;

    fn column(unstable: bool) -> ColumnGeometry {
        let temperature = if unstable {
            vec![245.0, 258.0, 270.0, 294.0]
        } else {
            vec![229.0, 263.0, 289.0, 305.0]
        };
        let humidity = if unstable {
            vec![0.0002, 0.001, 0.004, 0.016]
        } else {
            vec![0.0001; 4]
        };
        ColumnGeometry::new(
            Arc::from([25_000.0, 45_000.0, 70_000.0, 95_000.0]),
            Arc::from([10_000.0, 6_000.0, 3_000.0, 500.0]),
            Arc::from(temperature),
            Arc::from(humidity),
            Arc::from([[0.25; 4]; 4]),
            VerticalValidity {
                valid: Arc::from([true; 4]),
            },
            0.0,
            100_000.0,
            Some(12_000.0),
        )
        .unwrap()
    }

    #[test]
    fn zero_flux_is_exact_identity() {
        let kernel = DeepConvectionKernel::diagnose(&column(false), 300.0).unwrap();
        assert!(kernel.is_identity());
        assert_eq!(
            kernel.apply_forward(&[1.0, 2.0, 3.0, 4.0]).unwrap(),
            vec![1.0, 2.0, 3.0, 4.0]
        );
        assert_eq!(kernel.maximum_column_residual(), 0.0);
    }

    #[test]
    fn unstable_column_is_nonnegative_and_column_stochastic() {
        let kernel = DeepConvectionKernel::diagnose(&column(true), 300.0).unwrap();
        assert!(!kernel.is_identity());
        for source in 0..kernel.layer_count() {
            let sum = (0..kernel.layer_count())
                .map(|destination| kernel.transition_probability(destination, source).unwrap())
                .sum::<f64>();
            assert!((sum - 1.0).abs() <= 1.0e-14);
            assert!((0..kernel.layer_count()).all(|destination| {
                kernel.transition_probability(destination, source).unwrap() >= 0.0
            }));
        }
        assert!(kernel.maximum_column_residual() <= COLUMN_SUM_TOLERANCE);
    }

    #[test]
    fn forward_and_transpose_satisfy_dot_product_identity() {
        let kernel = DeepConvectionKernel::diagnose(&column(true), 300.0).unwrap();
        let mass = [0.2, 0.3, 0.4, 0.1];
        let adjoint = [1.5, -0.25, 0.75, 2.0];
        let forward = kernel.apply_forward(&mass).unwrap();
        let transpose = kernel.apply_adjoint(&adjoint).unwrap();
        let left = forward
            .iter()
            .zip(adjoint)
            .map(|(value, weight)| value * weight)
            .sum::<f64>();
        let right = mass
            .iter()
            .zip(transpose)
            .map(|(value, weight)| value * weight)
            .sum::<f64>();
        assert!((left - right).abs() <= 1.0e-12);
    }

    #[test]
    fn transpose_proposal_is_an_unbiased_million_sample_adjoint_estimator() {
        assert_eq!(
            DEEP_CONVECTION_RANDOM_ID,
            StableRandomId::from_text("deep_convection_column")
        );
        const INDEPENDENT_SEEDS: u64 = 8;
        const SAMPLES_PER_SEED: u64 = 125_000;
        let kernel = DeepConvectionKernel::diagnose(&column(true), 300.0).unwrap();
        let values = [0.25, 1.5, 0.75, 2.0];
        let destination = 1;
        let expected = values
            .iter()
            .enumerate()
            .map(|(source, value)| {
                kernel.transition_probability(destination, source).unwrap() * value
            })
            .sum::<f64>();
        let mut seed_means = Vec::new();
        let mut raw_sum = 0.0;
        let mut raw_square_sum = 0.0;
        for seed in 0..INDEPENDENT_SEEDS {
            let mut seed_sum = 0.0;
            for sample in 0..SAMPLES_PER_SEED {
                let transfer = kernel
                    .sample_adjoint(
                        destination,
                        ProcessRandomKey {
                            seed: 6_003 + seed,
                            particle: ParticleId(sample),
                            module: StableRandomId(0),
                            macro_step: 0,
                            substep: 0,
                            sampling_dimension: 0,
                            draw_index: 0,
                        },
                    )
                    .unwrap();
                let estimate = transfer.importance_weight * values[transfer.source_layer];
                seed_sum += estimate;
                raw_sum += estimate;
                raw_square_sum += estimate * estimate;
            }
            seed_means.push(seed_sum / SAMPLES_PER_SEED as f64);
        }
        let samples = (INDEPENDENT_SEEDS * SAMPLES_PER_SEED) as f64;
        let mean = raw_sum / samples;
        let variance = (raw_square_sum / samples - mean * mean).max(0.0);
        let standard_error = (variance / samples).sqrt();
        assert_eq!(seed_means.len(), INDEPENDENT_SEEDS as usize);
        assert!((mean - expected).abs() <= 4.0 * standard_error + 1.0e-5);
    }
}
