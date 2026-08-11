//! Boundary-layer Langevin velocity update.

use trajecta_case::model::time::Direction;

use crate::particle::ParticleId;
use crate::rng::{CounterRng, ProcessRandomKey, StableRandomId};

const GRAVITY_M_S2: f64 = 9.806_65;
const DRY_AIR_HEAT_CAPACITY_J_KG_K: f64 = 1004.0;
const FRICTION_VELOCITY_FLOOR_M_S: f64 = 0.01;
const MIXING_HEIGHT_FLOOR_M: f64 = 10.0;
const SIGMA_VELOCITY_FLOOR_M_S: f64 = 0.01;
const CONVECTIVE_TRANSITION_MINUS_ZETA: f64 = -0.2;
const CBL_SKEWNESS_TRANSITION_START: f64 = 5.0;
const CBL_SKEWNESS_TRANSITION_END: f64 = 15.0;
const GAUSSIAN_MAXIMUM_TL_FRACTION: f64 = 0.5;
const SKEWED_MAXIMUM_TL_FRACTION: f64 = 0.005;
const MINIMUM_SKEWNESS: f64 = 1.0e-8;
const INV_SQRT_TWO_PI: f64 = 0.398_942_280_401_432_7;
const U_DIMENSION: u32 = 64;
const V_DIMENSION: u32 = 65;
const VERTICAL_BRANCH_DIMENSION: u32 = 66;
const VERTICAL_VALUE_DIMENSION: u32 = 67;
pub(super) const BIRTH_DRAW_INDEX: u32 = 0;
pub(super) const MIDPOINT_DRAW_INDEX: u32 = 2;
pub(super) const ENDPOINT_DRAW_INDEX: u32 = 4;
const BOUNDARY_LAYER_RANDOM_ID: StableRandomId = StableRandomId(0x1ffe_1e7c_6f67_0b0c);

#[derive(Clone, Copy, Debug)]
pub(super) struct BoundaryLayerEnvironment {
    pub(super) height_agl_m: f64,
    pub(super) boundary_layer_height_m: f64,
    pub(super) friction_velocity_m_s: f64,
    pub(super) monin_obukhov_length_m: f64,
    pub(super) sensible_heat_flux_w_m2: f64,
    pub(super) roughness_length_m: f64,
    pub(super) air_temperature_k: f64,
    pub(super) air_density_kg_m3: f64,
}

impl BoundaryLayerEnvironment {
    pub(super) fn validate(self) -> Result<(), &'static str> {
        let finite = [
            self.height_agl_m,
            self.boundary_layer_height_m,
            self.friction_velocity_m_s,
            self.monin_obukhov_length_m,
            self.sensible_heat_flux_w_m2,
            self.roughness_length_m,
            self.air_temperature_k,
            self.air_density_kg_m3,
        ]
        .into_iter()
        .all(f64::is_finite);
        if !finite {
            return Err("non-finite boundary-layer input");
        }
        if self.boundary_layer_height_m <= 0.0
            || self.friction_velocity_m_s < 0.0
            || self.roughness_length_m < 0.0
            || self.air_temperature_k <= 0.0
            || self.air_density_kg_m3 <= 0.0
        {
            return Err("boundary-layer input lies outside its physical domain");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct TurbulenceScales {
    sigma_m_s: [f64; 3],
    lagrangian_time_s: [f64; 3],
    vertical_variance_gradient_s_2: f64,
    density_log_gradient_m_1: f64,
    vertical_pdf: VerticalPdf,
    vertical_pdf_gradient: VerticalPdfGradient,
}

#[derive(Clone, Copy, Debug)]
struct LocalTurbulence {
    sigma_m_s: [f64; 3],
    lagrangian_time_s: [f64; 3],
    vertical_pdf: VerticalPdf,
}

#[derive(Clone, Copy, Debug)]
enum VerticalPdf {
    Gaussian { sigma_m_s: f64 },
    BiGaussian(BiGaussianPdf),
}

impl VerticalPdf {
    fn from_moments(sigma_m_s: f64, skewness: f64) -> Result<Self, &'static str> {
        if !sigma_m_s.is_finite() || sigma_m_s <= 0.0 || !skewness.is_finite() {
            return Err("invalid convective vertical-velocity moments");
        }
        if skewness <= MINIMUM_SKEWNESS {
            return Ok(Self::Gaussian { sigma_m_s });
        }
        Ok(Self::BiGaussian(BiGaussianPdf::from_moments(
            sigma_m_s, skewness,
        )?))
    }

    fn log_density(self, velocity_m_s: f64) -> f64 {
        match self {
            Self::Gaussian { sigma_m_s } => log_normal_density(velocity_m_s, 0.0, sigma_m_s),
            Self::BiGaussian(pdf) => pdf.log_density(velocity_m_s),
        }
    }

    fn log_gradient(self, velocity_m_s: f64) -> f64 {
        match self {
            Self::Gaussian { sigma_m_s } => -velocity_m_s / sigma_m_s.powi(2),
            Self::BiGaussian(pdf) => pdf.log_gradient(velocity_m_s),
        }
    }

    fn log_negative_truncated_first_moment(self, velocity_m_s: f64) -> Result<f64, &'static str> {
        match self {
            Self::Gaussian { sigma_m_s } => {
                Ok(2.0 * sigma_m_s.ln() + log_normal_density(velocity_m_s, 0.0, sigma_m_s))
            }
            Self::BiGaussian(pdf) => pdf.log_negative_truncated_first_moment(velocity_m_s),
        }
    }

    fn sample(self, branch: f64, gaussian: f64) -> f64 {
        match self {
            Self::Gaussian { sigma_m_s } => sigma_m_s * gaussian,
            Self::BiGaussian(pdf) => pdf.sample(branch, gaussian),
        }
    }

    const fn is_skewed(self) -> bool {
        matches!(self, Self::BiGaussian(_))
    }
}

#[derive(Clone, Copy, Debug)]
struct BiGaussianPdf {
    updraft_weight: f64,
    updraft_mean_m_s: f64,
    updraft_sigma_m_s: f64,
    downdraft_weight: f64,
    downdraft_mean_m_s: f64,
    downdraft_sigma_m_s: f64,
}

impl BiGaussianPdf {
    fn from_moments(sigma_m_s: f64, skewness: f64) -> Result<Self, &'static str> {
        let separation = (2.0 / 3.0) * skewness.cbrt();
        let separation_squared = separation * separation;
        let ratio = ((1.0 + separation_squared).powi(3) * skewness.powi(2))
            / ((3.0 + separation_squared).powi(2) * separation_squared);
        let updraft_weight = 0.5 * (1.0 - (ratio / (4.0 + ratio)).sqrt());
        let downdraft_weight = 1.0 - updraft_weight;
        let common = 1.0 + separation_squared;
        let updraft_sigma_m_s = sigma_m_s * (downdraft_weight / (updraft_weight * common)).sqrt();
        let downdraft_sigma_m_s = sigma_m_s * (updraft_weight / (downdraft_weight * common)).sqrt();
        let pdf = Self {
            updraft_weight,
            updraft_mean_m_s: separation * updraft_sigma_m_s,
            updraft_sigma_m_s,
            downdraft_weight,
            downdraft_mean_m_s: -separation * downdraft_sigma_m_s,
            downdraft_sigma_m_s,
        };
        if [
            pdf.updraft_weight,
            pdf.updraft_mean_m_s,
            pdf.updraft_sigma_m_s,
            pdf.downdraft_weight,
            pdf.downdraft_mean_m_s,
            pdf.downdraft_sigma_m_s,
        ]
        .into_iter()
        .all(f64::is_finite)
            && pdf.updraft_weight > 0.0
            && pdf.downdraft_weight > 0.0
            && pdf.updraft_sigma_m_s > 0.0
            && pdf.downdraft_sigma_m_s > 0.0
        {
            Ok(pdf)
        } else {
            Err("bi-Gaussian closure produced invalid parameters")
        }
    }

    fn log_density(self, velocity_m_s: f64) -> f64 {
        let updraft = self.updraft_weight.ln()
            + log_normal_density(velocity_m_s, self.updraft_mean_m_s, self.updraft_sigma_m_s);
        let downdraft = self.downdraft_weight.ln()
            + log_normal_density(
                velocity_m_s,
                self.downdraft_mean_m_s,
                self.downdraft_sigma_m_s,
            );
        log_sum_exp(updraft, downdraft)
    }

    fn log_gradient(self, velocity_m_s: f64) -> f64 {
        let updraft_log = self.updraft_weight.ln()
            - self.updraft_sigma_m_s.ln()
            - 0.5 * ((velocity_m_s - self.updraft_mean_m_s) / self.updraft_sigma_m_s).powi(2);
        let downdraft_log = self.downdraft_weight.ln()
            - self.downdraft_sigma_m_s.ln()
            - 0.5 * ((velocity_m_s - self.downdraft_mean_m_s) / self.downdraft_sigma_m_s).powi(2);
        let common = updraft_log.max(downdraft_log);
        let updraft = (updraft_log - common).exp();
        let downdraft = (downdraft_log - common).exp();
        let total = updraft + downdraft;
        let updraft_gradient =
            -(velocity_m_s - self.updraft_mean_m_s) / self.updraft_sigma_m_s.powi(2);
        let downdraft_gradient =
            -(velocity_m_s - self.downdraft_mean_m_s) / self.downdraft_sigma_m_s.powi(2);
        (updraft * updraft_gradient + downdraft * downdraft_gradient) / total
    }

    fn log_negative_truncated_first_moment(self, velocity_m_s: f64) -> Result<f64, &'static str> {
        let components = [
            (
                self.updraft_weight,
                self.updraft_mean_m_s,
                self.updraft_sigma_m_s,
            ),
            (
                self.downdraft_weight,
                self.downdraft_mean_m_s,
                self.downdraft_sigma_m_s,
            ),
        ];
        let lower_tail = velocity_m_s <= 0.0;
        let mut terms = [(0.0, f64::NEG_INFINITY); 4];
        for (component, (weight, mean, sigma)) in components.into_iter().enumerate() {
            let standardized = (velocity_m_s - mean) / sigma;
            let tail_log = if lower_tail {
                log_standard_normal_lower(standardized)
            } else {
                log_standard_normal_upper(standardized)
            };
            let outer_sign = if lower_tail { 1.0 } else { -1.0 };
            terms[2 * component] = (
                outer_sign * mean.signum(),
                weight.ln() + mean.abs().ln() + tail_log,
            );
            terms[2 * component + 1] = (
                -1.0,
                weight.ln() + sigma.ln() + log_standard_normal_density(standardized),
            );
        }
        log_negative_signed_sum(terms)
    }

    fn sample(self, branch: f64, gaussian: f64) -> f64 {
        if branch < self.updraft_weight {
            self.updraft_sigma_m_s
                .mul_add(gaussian, self.updraft_mean_m_s)
        } else {
            self.downdraft_sigma_m_s
                .mul_add(gaussian, self.downdraft_mean_m_s)
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct VerticalPdfGradient {
    lower: VerticalPdf,
    upper: VerticalPdf,
    lower_distance_m: f64,
    upper_distance_m: f64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LangevinKey {
    pub(super) seed: u64,
    pub(super) particle: ParticleId,
    pub(super) macro_step: u64,
    pub(super) substep: u32,
    pub(super) draw_index: u32,
}

pub(super) fn update_velocity(
    previous_m_s: [f64; 3],
    environment: BoundaryLayerEnvironment,
    elapsed_seconds: f64,
    direction: Direction,
    key: LangevinKey,
) -> Result<[f64; 3], &'static str> {
    environment.validate()?;
    if !elapsed_seconds.is_finite() || elapsed_seconds <= 0.0 {
        return Err("boundary-layer substep must be finite and positive");
    }
    if environment.height_agl_m <= 0.0
        || environment.height_agl_m >= environment.boundary_layer_height_m
    {
        return Ok([0.0; 3]);
    }
    let scales = hanna_scales(environment)?;
    let module = BOUNDARY_LAYER_RANDOM_ID;
    let normal = |dimension| {
        CounterRng::sample_process_normal_pair(ProcessRandomKey {
            seed: key.seed,
            particle: key.particle,
            module,
            macro_step: key.macro_step,
            substep: key.substep,
            sampling_dimension: dimension,
            draw_index: key.draw_index,
        })[0]
    };
    let innovations = [
        normal(U_DIMENSION),
        normal(V_DIMENSION),
        normal(VERTICAL_VALUE_DIMENSION),
    ];
    let mut next = [0.0; 3];
    for component in 0..2 {
        let decay = (-elapsed_seconds / scales.lagrangian_time_s[component]).exp();
        let noise = (1.0 - decay * decay).max(0.0).sqrt()
            * scales.sigma_m_s[component]
            * innovations[component];
        next[component] = decay.mul_add(previous_m_s[component], noise);
    }

    next[2] = advance_vertical_velocity(
        previous_m_s[2],
        &scales,
        elapsed_seconds,
        direction,
        innovations[2],
    )?;
    if next.into_iter().all(f64::is_finite) {
        Ok(next)
    } else {
        Err("boundary-layer Langevin update produced a non-finite velocity")
    }
}

fn advance_vertical_velocity(
    previous_m_s: f64,
    scales: &TurbulenceScales,
    elapsed_seconds: f64,
    direction: Direction,
    innovation: f64,
) -> Result<f64, &'static str> {
    let spatial_drift = vertical_spatial_drift(scales, previous_m_s)?;
    let directed_spatial_drift = match direction {
        Direction::Forward => spatial_drift,
        Direction::Backward => -spatial_drift,
    };
    let next = match scales.vertical_pdf {
        VerticalPdf::Gaussian { .. } => {
            let decay = (-elapsed_seconds / scales.lagrangian_time_s[2]).exp();
            let noise = (1.0 - decay * decay).max(0.0).sqrt() * scales.sigma_m_s[2] * innovation;
            directed_spatial_drift.mul_add(elapsed_seconds, decay.mul_add(previous_m_s, noise))
        }
        VerticalPdf::BiGaussian(_) => {
            let variance = scales.sigma_m_s[2].powi(2);
            let local_drift = variance / scales.lagrangian_time_s[2]
                * scales.vertical_pdf.log_gradient(previous_m_s);
            let diffusion = (2.0 * variance / scales.lagrangian_time_s[2]).sqrt();
            diffusion.mul_add(
                elapsed_seconds.sqrt() * innovation,
                (local_drift + directed_spatial_drift).mul_add(elapsed_seconds, previous_m_s),
            )
        }
    };
    if next.is_finite() {
        Ok(next)
    } else {
        Err("boundary-layer vertical update produced a non-finite velocity")
    }
}

pub(super) fn sample_stationary_velocity(
    environment: BoundaryLayerEnvironment,
    key: LangevinKey,
) -> Result<[f64; 3], &'static str> {
    environment.validate()?;
    if environment.height_agl_m <= 0.0
        || environment.height_agl_m >= environment.boundary_layer_height_m
    {
        return Ok([0.0; 3]);
    }
    let scales = hanna_scales(environment)?;
    let random_key = |sampling_dimension| ProcessRandomKey {
        seed: key.seed,
        particle: key.particle,
        module: BOUNDARY_LAYER_RANDOM_ID,
        macro_step: key.macro_step,
        substep: key.substep,
        sampling_dimension,
        draw_index: key.draw_index,
    };
    Ok([
        scales.sigma_m_s[0] * CounterRng::sample_process_normal_pair(random_key(U_DIMENSION))[0],
        scales.sigma_m_s[1] * CounterRng::sample_process_normal_pair(random_key(V_DIMENSION))[0],
        scales.vertical_pdf.sample(
            CounterRng::sample_process_unit(random_key(VERTICAL_BRANCH_DIMENSION)),
            CounterRng::sample_process_normal_pair(random_key(VERTICAL_VALUE_DIMENSION))[0],
        ),
    ])
}

pub(super) fn maximum_stable_step_ns(
    environment: BoundaryLayerEnvironment,
) -> Result<i64, &'static str> {
    environment.validate()?;
    if environment.height_agl_m <= 0.0
        || environment.height_agl_m >= environment.boundary_layer_height_m
    {
        return Ok(i64::MAX);
    }
    let scales = hanna_scales(environment)?;
    let fraction = if scales.vertical_pdf.is_skewed() {
        SKEWED_MAXIMUM_TL_FRACTION
    } else {
        GAUSSIAN_MAXIMUM_TL_FRACTION
    };
    let seconds = scales
        .lagrangian_time_s
        .into_iter()
        .fold(f64::INFINITY, f64::min)
        * fraction;
    let nanoseconds = (seconds * 1.0e9).floor();
    if nanoseconds < 1.0 || nanoseconds > i64::MAX as f64 || !nanoseconds.is_finite() {
        Err("boundary-layer correlation-time limit is below one nanosecond")
    } else {
        Ok(nanoseconds as i64)
    }
}

fn hanna_scales(environment: BoundaryLayerEnvironment) -> Result<TurbulenceScales, &'static str> {
    let height = environment
        .boundary_layer_height_m
        .max(MIXING_HEIGHT_FLOOR_M);
    let dz = (0.01 * height).clamp(0.1, 10.0);
    let lower_bound = environment.roughness_length_m.max(0.01);
    let upper_bound = height * (1.0 - 1.0e-6);
    let margin = (0.5 * dz).min(0.25 * (upper_bound - lower_bound));
    let z = environment
        .height_agl_m
        .clamp(lower_bound + margin, upper_bound - margin);
    let lower = (z - dz).max(lower_bound);
    let upper = (z + dz).min(upper_bound);
    let local = local_turbulence(environment, z)?;
    let lower_profile = local_turbulence(environment, lower)?;
    let upper_profile = local_turbulence(environment, upper)?;
    let variance_gradient = quadratic_derivative(
        lower_profile.sigma_m_s[2].powi(2),
        local.sigma_m_s[2].powi(2),
        upper_profile.sigma_m_s[2].powi(2),
        z - lower,
        upper - z,
    );
    let density_log_gradient = -GRAVITY_M_S2
        / (crate::science::M4_CONSTANTS.dry_air_gas_constant_j_kg_k
            * environment.air_temperature_k);
    let scales = TurbulenceScales {
        sigma_m_s: local.sigma_m_s,
        lagrangian_time_s: local.lagrangian_time_s,
        vertical_variance_gradient_s_2: variance_gradient,
        density_log_gradient_m_1: density_log_gradient,
        vertical_pdf: local.vertical_pdf,
        vertical_pdf_gradient: VerticalPdfGradient {
            lower: lower_profile.vertical_pdf,
            upper: upper_profile.vertical_pdf,
            lower_distance_m: z - lower,
            upper_distance_m: upper - z,
        },
    };
    if scales
        .sigma_m_s
        .into_iter()
        .chain(scales.lagrangian_time_s)
        .chain([
            scales.vertical_variance_gradient_s_2,
            scales.density_log_gradient_m_1,
        ])
        .all(f64::is_finite)
    {
        Ok(scales)
    } else {
        Err("Hanna boundary-layer scales are non-finite")
    }
}

fn local_turbulence(
    environment: BoundaryLayerEnvironment,
    height_agl_m: f64,
) -> Result<LocalTurbulence, &'static str> {
    let height = environment
        .boundary_layer_height_m
        .max(MIXING_HEIGHT_FLOOR_M);
    let normalized = (height_agl_m / height).clamp(1.0e-6, 1.0 - 1.0e-6);
    let upper_taper = (1.0 - normalized).max(0.02);
    let friction_velocity = environment
        .friction_velocity_m_s
        .max(FRICTION_VELOCITY_FLOOR_M_S);
    let zeta = if environment.monin_obukhov_length_m == 0.0 {
        0.0
    } else {
        height_agl_m / environment.monin_obukhov_length_m
    };
    let neutral = [
        2.4 * friction_velocity * upper_taper.sqrt(),
        1.9 * friction_velocity * upper_taper.sqrt(),
        1.25 * friction_velocity * upper_taper,
    ];
    let sigma = if zeta > 0.0 {
        let stability = (1.0 + 3.0 * zeta).sqrt();
        [
            2.0 * friction_velocity * upper_taper / stability,
            1.7 * friction_velocity * upper_taper / stability,
            1.3 * friction_velocity * upper_taper / stability,
        ]
    } else {
        let convective_velocity = convective_velocity(environment, height);
        let unstable_vertical = (1.2
            * convective_velocity.powi(2)
            * (1.0 - 0.9 * normalized).max(0.0)
            * normalized.powf(2.0 / 3.0)
            + (1.8 - 1.4 * normalized).max(0.0) * friction_velocity.powi(2))
        .max(0.0)
        .sqrt()
            + SIGMA_VELOCITY_FLOOR_M_S;
        let unstable = [
            (2.4_f64.mul_add(
                friction_velocity.powi(2),
                0.35 * convective_velocity.powi(2),
            ))
            .sqrt(),
            (1.9_f64.mul_add(
                friction_velocity.powi(2),
                0.35 * convective_velocity.powi(2),
            ))
            .sqrt(),
            unstable_vertical,
        ];
        let transition = if environment.sensible_heat_flux_w_m2 > 0.0 {
            smoothstep((-zeta / -CONVECTIVE_TRANSITION_MINUS_ZETA).clamp(0.0, 1.0))
        } else {
            0.0
        };
        std::array::from_fn(|index| {
            ((1.0 - transition) * neutral[index].powi(2) + transition * unstable[index].powi(2))
                .sqrt()
        })
    }
    .map(|value| value.max(SIGMA_VELOCITY_FLOOR_M_S));

    let horizontal_time = (0.15 * height / sigma[0]).clamp(1.0, 10_000.0);
    let vertical_time =
        (0.15 * height * normalized.sqrt() * upper_taper.sqrt() / sigma[2]).clamp(1.0, 10_000.0);
    let minus_height_over_obukhov = if environment.monin_obukhov_length_m < 0.0 {
        -height / environment.monin_obukhov_length_m
    } else {
        0.0
    };
    let skewness_transition = cosine_transition(
        minus_height_over_obukhov,
        CBL_SKEWNESS_TRANSITION_START,
        CBL_SKEWNESS_TRANSITION_END,
    );
    let local_transition = if environment.sensible_heat_flux_w_m2 > 0.0 {
        smoothstep((-zeta / -CONVECTIVE_TRANSITION_MINUS_ZETA).clamp(0.0, 1.0))
    } else {
        0.0
    };
    let third_moment = local_transition
        * skewness_transition
        * convective_velocity(environment, height).powi(3)
        * 1.2
        * normalized
        * upper_taper.powf(1.5);
    let skewness = third_moment / sigma[2].powi(3);
    let local = LocalTurbulence {
        sigma_m_s: sigma,
        lagrangian_time_s: [horizontal_time, horizontal_time, vertical_time],
        vertical_pdf: VerticalPdf::from_moments(sigma[2], skewness)?,
    };
    if local
        .sigma_m_s
        .into_iter()
        .chain(local.lagrangian_time_s)
        .all(f64::is_finite)
    {
        Ok(local)
    } else {
        Err("Hanna local turbulence profile is non-finite")
    }
}

fn vertical_spatial_drift(
    scales: &TurbulenceScales,
    velocity_m_s: f64,
) -> Result<f64, &'static str> {
    if matches!(scales.vertical_pdf, VerticalPdf::Gaussian { .. }) {
        let sigma = scales.sigma_m_s[2];
        let normalized_velocity = velocity_m_s / sigma;
        return Ok(0.5
            * (1.0 + normalized_velocity.powi(2))
            * scales.vertical_variance_gradient_s_2
            + sigma.powi(2) * scales.density_log_gradient_m_1);
    }

    let local_log_density = scales.vertical_pdf.log_density(velocity_m_s);
    if !local_log_density.is_finite() {
        return Err("convective vertical-velocity log PDF is non-finite");
    }
    let local_log_moment = scales
        .vertical_pdf
        .log_negative_truncated_first_moment(velocity_m_s)?;
    let local_moment_ratio = (local_log_moment - local_log_density).exp();
    let gradient = scales.vertical_pdf_gradient;
    let log_moment_gradient = quadratic_derivative(
        gradient
            .lower
            .log_negative_truncated_first_moment(velocity_m_s)?,
        local_log_moment,
        gradient
            .upper
            .log_negative_truncated_first_moment(velocity_m_s)?,
        gradient.lower_distance_m,
        gradient.upper_distance_m,
    );
    let drift = local_moment_ratio * (scales.density_log_gradient_m_1 + log_moment_gradient);
    if drift.is_finite() {
        Ok(drift)
    } else {
        Err("convective well-mixed drift is non-finite")
    }
}

fn convective_velocity(environment: BoundaryLayerEnvironment, height_m: f64) -> f64 {
    (GRAVITY_M_S2 * environment.sensible_heat_flux_w_m2.max(0.0) * height_m
        / (environment.air_temperature_k
            * environment.air_density_kg_m3
            * DRY_AIR_HEAT_CAPACITY_J_KG_K))
        .max(0.0)
        .cbrt()
}

fn smoothstep(value: f64) -> f64 {
    value * value * (3.0 - 2.0 * value)
}

fn cosine_transition(value: f64, start: f64, end: f64) -> f64 {
    if value <= start {
        0.0
    } else if value >= end {
        1.0
    } else {
        0.5 - 0.5 * (std::f64::consts::PI * (value - start) / (end - start)).cos()
    }
}

fn quadratic_derivative(
    lower: f64,
    center: f64,
    upper: f64,
    lower_distance: f64,
    upper_distance: f64,
) -> f64 {
    let span = lower_distance + upper_distance;
    -upper_distance / (lower_distance * span) * lower
        + (upper_distance - lower_distance) / (lower_distance * upper_distance) * center
        + lower_distance / (upper_distance * span) * upper
}

fn log_normal_density(value: f64, mean: f64, sigma: f64) -> f64 {
    log_standard_normal_density((value - mean) / sigma) - sigma.ln()
}

fn standard_normal_density(value: f64) -> f64 {
    INV_SQRT_TWO_PI * (-0.5 * value * value).exp()
}

fn log_standard_normal_density(value: f64) -> f64 {
    INV_SQRT_TWO_PI.ln() - 0.5 * value * value
}

fn log_standard_normal_lower(value: f64) -> f64 {
    log_standard_normal_upper(-value)
}

fn log_standard_normal_upper(value: f64) -> f64 {
    if value < 0.0 {
        return (-standard_normal_upper(-value)).ln_1p();
    }
    if value < 8.0 {
        return standard_normal_upper(value).ln();
    }
    let inverse_squared = value.powi(-2);
    let correction = 1.0
        + inverse_squared
            * (-1.0
                + inverse_squared * (3.0 + inverse_squared * (-15.0 + inverse_squared * 105.0)));
    log_standard_normal_density(value) - value.ln() + correction.ln()
}

fn log_sum_exp(left: f64, right: f64) -> f64 {
    let maximum = left.max(right);
    maximum + ((left.min(right) - maximum).exp()).ln_1p()
}

fn log_negative_signed_sum(terms: [(f64, f64); 4]) -> Result<f64, &'static str> {
    let maximum = terms
        .iter()
        .map(|(_, log_magnitude)| *log_magnitude)
        .fold(f64::NEG_INFINITY, f64::max);
    if !maximum.is_finite() {
        return Err("convective vertical-velocity tail moment vanished");
    }
    let scaled = terms
        .into_iter()
        .map(|(sign, log_magnitude)| sign * (log_magnitude - maximum).exp())
        .sum::<f64>();
    if scaled < 0.0 && scaled.is_finite() {
        Ok(maximum + (-scaled).ln())
    } else {
        Err("convective vertical-velocity tail moment is not negative")
    }
}

fn standard_normal_upper(value: f64) -> f64 {
    if value < 0.0 {
        return 1.0 - standard_normal_upper(-value);
    }
    if value >= 38.0 {
        return 0.0;
    }
    let t = 1.0 / (1.0 + 0.231_641_9 * value);
    let polynomial = t
        * (0.319_381_530
            + t * (-0.356_563_782
                + t * (1.781_477_937 + t * (-1.821_255_978 + t * 1.330_274_429))));
    standard_normal_density(value) * polynomial
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde::Deserialize;

    use super::*;

    fn neutral_environment() -> BoundaryLayerEnvironment {
        BoundaryLayerEnvironment {
            height_agl_m: 100.0,
            boundary_layer_height_m: 1000.0,
            friction_velocity_m_s: 0.3,
            monin_obukhov_length_m: 1.0e9,
            sensible_heat_flux_w_m2: 0.0,
            roughness_length_m: 0.1,
            air_temperature_k: 290.0,
            air_density_kg_m3: 1.2,
        }
    }

    #[test]
    fn exact_key_repeats_and_substep_changes_velocity() {
        assert_eq!(
            BOUNDARY_LAYER_RANDOM_ID,
            StableRandomId::from_text("boundary_layer_langevin")
        );
        let key = LangevinKey {
            seed: 9,
            particle: ParticleId(7),
            macro_step: 2,
            substep: 1,
            draw_index: 0,
        };
        let first = update_velocity(
            [0.0; 3],
            neutral_environment(),
            30.0,
            Direction::Forward,
            key,
        )
        .unwrap();
        assert_eq!(
            first,
            update_velocity(
                [0.0; 3],
                neutral_environment(),
                30.0,
                Direction::Forward,
                key,
            )
            .unwrap()
        );
        assert_ne!(
            first,
            update_velocity(
                [0.0; 3],
                neutral_environment(),
                30.0,
                Direction::Forward,
                LangevinKey { substep: 2, ..key }
            )
            .unwrap()
        );
        assert_ne!(
            first,
            update_velocity(
                [0.0; 3],
                neutral_environment(),
                30.0,
                Direction::Forward,
                LangevinKey {
                    draw_index: ENDPOINT_DRAW_INDEX,
                    ..key
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn velocity_is_zero_outside_the_mixing_layer() {
        let mut environment = neutral_environment();
        environment.height_agl_m = 1200.0;
        let value = update_velocity(
            [1.0, 1.0, 1.0],
            environment,
            30.0,
            Direction::Forward,
            LangevinKey {
                seed: 1,
                particle: ParticleId(1),
                macro_step: 0,
                substep: 0,
                draw_index: 0,
            },
        )
        .unwrap();
        assert_eq!(value, [0.0; 3]);
    }

    #[test]
    fn boundary_adjacent_particles_use_a_finite_interior_gradient_stencil() {
        for height_agl_m in [1.0e-6, 999.999_9] {
            let mut environment = neutral_environment();
            environment.height_agl_m = height_agl_m;
            let sampled = sample_stationary_velocity(
                environment,
                LangevinKey {
                    seed: 5,
                    particle: ParticleId(3),
                    macro_step: 0,
                    substep: 0,
                    draw_index: BIRTH_DRAW_INDEX,
                },
            )
            .unwrap();
            assert!(sampled.into_iter().all(f64::is_finite));
            assert_ne!(sampled, [0.0; 3]);
            assert!(maximum_stable_step_ns(environment).unwrap() > 0);
        }
    }

    #[test]
    fn neutral_ou_stationary_statistics_meet_the_frozen_m6_gate() {
        const SAMPLE_COUNT: u64 = 50_000;
        let environment = neutral_environment();
        let scales = hanna_scales(environment).unwrap();
        let module = StableRandomId::from_text("boundary_layer_langevin.stationary-test");
        let mut previous = Vec::with_capacity(SAMPLE_COUNT as usize);
        let mut next = Vec::with_capacity(SAMPLE_COUNT as usize);
        for ordinal in 0..SAMPLE_COUNT {
            let particle = ParticleId(ordinal);
            let prior = scales.sigma_m_s[0]
                * CounterRng::sample_process_normal_pair(ProcessRandomKey {
                    seed: 41,
                    particle,
                    module,
                    macro_step: 0,
                    substep: 0,
                    sampling_dimension: 192,
                    draw_index: 0,
                })[0];
            let updated = update_velocity(
                [prior, 0.0, 0.0],
                environment,
                30.0,
                Direction::Forward,
                LangevinKey {
                    seed: 17,
                    particle,
                    macro_step: 3,
                    substep: 2,
                    draw_index: 0,
                },
            )
            .unwrap()[0];
            previous.push(prior);
            next.push(updated);
        }
        let (previous_mean, previous_variance) = mean_variance(&previous);
        let (next_mean, next_variance) = mean_variance(&next);
        let covariance = previous
            .iter()
            .zip(&next)
            .map(|(left, right)| (left - previous_mean) * (right - next_mean))
            .sum::<f64>()
            / SAMPLE_COUNT as f64;
        let correlation = covariance / (previous_variance * next_variance).sqrt();
        let expected_correlation = (-30.0 / scales.lagrangian_time_s[0]).exp();
        let expected_variance = scales.sigma_m_s[0].powi(2);

        assert!(next_mean.abs() <= 0.02 * scales.sigma_m_s[0]);
        assert!((next_variance / expected_variance - 1.0).abs() <= 0.03);
        assert!((correlation - expected_correlation).abs() <= 0.02);
    }

    #[test]
    #[ignore = "formal M6 well-mixed joint-distribution gate"]
    fn nonuniform_gaussian_turbulence_preserves_the_well_mixed_joint_distribution() {
        const PARTICLES: u64 = 8_192;
        const STEPS: u32 = 400;
        const DT_SECONDS: f64 = 0.125;
        const LOWER_M: f64 = 50.0;
        const UPPER_M: f64 = 800.0;
        const BINS: usize = 8;

        let base = neutral_environment();
        let density_gradient = -GRAVITY_M_S2
            / (crate::science::M4_CONSTANTS.dry_air_gas_constant_j_kg_k * base.air_temperature_k);
        let height_module = StableRandomId::from_text("boundary_layer_langevin.height-test");
        let lower_density = (density_gradient * LOWER_M).exp();
        let upper_density = (density_gradient * UPPER_M).exp();
        let mut particles = (0..PARTICLES)
            .map(|ordinal| {
                let id = ParticleId(ordinal);
                let unit = CounterRng::sample_process_unit(ProcessRandomKey {
                    seed: 83,
                    particle: id,
                    module: height_module,
                    macro_step: 0,
                    substep: 0,
                    sampling_dimension: 193,
                    draw_index: 0,
                });
                let height = (lower_density + unit * (upper_density - lower_density)).ln()
                    / density_gradient;
                let mut environment = base;
                environment.height_agl_m = height;
                let velocity = sample_stationary_velocity(
                    environment,
                    LangevinKey {
                        seed: 83,
                        particle: id,
                        macro_step: 0,
                        substep: 0,
                        draw_index: BIRTH_DRAW_INDEX,
                    },
                )
                .unwrap()[2];
                (id, height, velocity)
            })
            .collect::<Vec<_>>();

        for substep in 0..STEPS {
            for (id, height, velocity) in &mut particles {
                let mut environment = base;
                environment.height_agl_m = *height;
                let key = LangevinKey {
                    seed: 83,
                    particle: *id,
                    macro_step: 1,
                    substep,
                    draw_index: MIDPOINT_DRAW_INDEX,
                };
                let midpoint = update_velocity(
                    [0.0, 0.0, *velocity],
                    environment,
                    0.5 * DT_SECONDS,
                    Direction::Forward,
                    key,
                )
                .unwrap()[2];
                let mut endpoint = update_velocity(
                    [0.0, 0.0, midpoint],
                    environment,
                    0.5 * DT_SECONDS,
                    Direction::Forward,
                    LangevinKey {
                        draw_index: ENDPOINT_DRAW_INDEX,
                        ..key
                    },
                )
                .unwrap()[2];
                let proposed = midpoint.mul_add(DT_SECONDS, *height);
                if proposed < LOWER_M {
                    *height = 2.0 * LOWER_M - proposed;
                    endpoint = -endpoint;
                } else if proposed > UPPER_M {
                    *height = 2.0 * UPPER_M - proposed;
                    endpoint = -endpoint;
                } else {
                    *height = proposed;
                }
                *velocity = endpoint;
            }
        }

        let mut counts = [0_u64; BINS];
        let mut normalized_sum = [0.0; BINS];
        let mut normalized_square_sum = [0.0; BINS];
        for (_, height, velocity) in particles {
            let bin = (((height - LOWER_M) / (UPPER_M - LOWER_M) * BINS as f64).floor() as usize)
                .min(BINS - 1);
            let mut environment = base;
            environment.height_agl_m = height;
            let normalized_velocity = velocity / hanna_scales(environment).unwrap().sigma_m_s[2];
            counts[bin] += 1;
            normalized_sum[bin] += normalized_velocity;
            normalized_square_sum[bin] += normalized_velocity * normalized_velocity;
        }

        for bin in 0..BINS {
            let bin_lower = LOWER_M + (UPPER_M - LOWER_M) * bin as f64 / BINS as f64;
            let bin_upper = LOWER_M + (UPPER_M - LOWER_M) * (bin + 1) as f64 / BINS as f64;
            let probability = ((density_gradient * bin_upper).exp()
                - (density_gradient * bin_lower).exp())
                / (upper_density - lower_density);
            let expected = PARTICLES as f64 * probability;
            let count_z = (counts[bin] as f64 - expected) / expected.sqrt();
            let mean = normalized_sum[bin] / counts[bin] as f64;
            let variance = normalized_square_sum[bin] / counts[bin] as f64 - mean * mean;
            assert!(count_z.abs() <= 5.0, "bin={bin}, count z={count_z}");
            assert!(mean.abs() <= 0.15, "bin={bin}, normalized mean={mean}");
            assert!(
                (variance - 1.0).abs() <= 0.20,
                "bin={bin}, normalized variance={variance}"
            );
        }
    }

    #[test]
    #[ignore = "formal M6 gate: eight seeds with one million samples each"]
    fn formal_neutral_ou_statistics_report_eight_million_samples() {
        const SAMPLE_COUNT: u64 = 1_000_000;
        const SEEDS: [u64; 8] = [17, 41, 73, 109, 151, 197, 251, 307];
        const NORMAL_95: f64 = 1.959_963_984_540_054;

        let handles = SEEDS.map(|seed| {
            std::thread::spawn(move || {
                let environment = neutral_environment();
                let scales = hanna_scales(environment).unwrap();
                let prior_module =
                    StableRandomId::from_text("boundary_layer_langevin.stationary-test");
                let mut moments = PairMoments::default();
                for ordinal in 0..SAMPLE_COUNT {
                    let particle = ParticleId(ordinal);
                    let prior = scales.sigma_m_s[0]
                        * CounterRng::sample_process_normal_pair(ProcessRandomKey {
                            seed,
                            particle,
                            module: prior_module,
                            macro_step: 0,
                            substep: 0,
                            sampling_dimension: 192,
                            draw_index: 0,
                        })[0];
                    let updated = update_velocity(
                        [prior, 0.0, 0.0],
                        environment,
                        30.0,
                        Direction::Forward,
                        LangevinKey {
                            seed,
                            particle,
                            macro_step: 3,
                            substep: 2,
                            draw_index: 0,
                        },
                    )
                    .unwrap()[0];
                    moments.push(prior, updated);
                }
                (seed, scales, moments.finish())
            })
        });

        for handle in handles {
            let (seed, scales, moments) = handle.join().unwrap();
            let expected_variance = scales.sigma_m_s[0].powi(2);
            let expected_correlation = (-30.0 / scales.lagrangian_time_s[0]).exp();
            let mean_half_width = NORMAL_95 * (moments.next_variance / SAMPLE_COUNT as f64).sqrt();
            let variance_half_width =
                NORMAL_95 * moments.next_variance * (2.0 / (SAMPLE_COUNT - 1) as f64).sqrt();
            let fisher_half_width = NORMAL_95 / ((SAMPLE_COUNT - 3) as f64).sqrt();
            let fisher = moments.correlation.atanh();
            let correlation_interval = [
                (fisher - fisher_half_width).tanh(),
                (fisher + fisher_half_width).tanh(),
            ];

            assert!(moments.next_mean.abs() <= 0.02 * scales.sigma_m_s[0]);
            assert!((moments.next_variance / expected_variance - 1.0).abs() <= 0.03);
            assert!((moments.correlation - expected_correlation).abs() <= 0.02);
            println!(
                "{}",
                serde_json::json!({
                    "seed": seed,
                    "samples": SAMPLE_COUNT,
                    "raw_moments": {
                        "previous_mean": moments.previous_mean,
                        "next_mean": moments.next_mean,
                        "previous_second": moments.previous_second,
                        "next_second": moments.next_second,
                        "cross": moments.cross,
                    },
                    "next_variance": moments.next_variance,
                    "lag_one_correlation": moments.correlation,
                    "expected": {
                        "mean": 0.0,
                        "variance": expected_variance,
                        "lag_one_correlation": expected_correlation,
                    },
                    "two_sided_95_percent": {
                        "next_mean": [moments.next_mean - mean_half_width, moments.next_mean + mean_half_width],
                        "next_variance_large_sample": [
                            moments.next_variance - variance_half_width,
                            moments.next_variance + variance_half_width,
                        ],
                        "lag_one_correlation_fisher": correlation_interval,
                    },
                })
            );
        }
    }

    #[test]
    fn convective_stationary_sample_matches_the_bi_gaussian_moments() {
        const SAMPLE_COUNT: u64 = 50_000;
        let mut environment = neutral_environment();
        environment.monin_obukhov_length_m = -50.0;
        environment.sensible_heat_flux_w_m2 = 250.0;
        let scales = hanna_scales(environment).unwrap();
        assert!(scales.vertical_pdf.is_skewed());
        let mut values = Vec::with_capacity(SAMPLE_COUNT as usize);
        for ordinal in 0..SAMPLE_COUNT {
            values.push(
                sample_stationary_velocity(
                    environment,
                    LangevinKey {
                        seed: 23,
                        particle: ParticleId(ordinal),
                        macro_step: 0,
                        substep: 0,
                        draw_index: BIRTH_DRAW_INDEX,
                    },
                )
                .unwrap()[2],
            );
        }
        let (mean, variance) = mean_variance(&values);
        let skewness = values
            .iter()
            .map(|value| (value - mean).powi(3))
            .sum::<f64>()
            / (SAMPLE_COUNT as f64 * variance.powf(1.5));
        assert!(
            mean.abs() <= 0.02 * scales.sigma_m_s[2],
            "vertical mean={mean}"
        );
        assert!(
            (variance / scales.sigma_m_s[2].powi(2) - 1.0).abs() <= 0.03,
            "vertical variance={variance}"
        );
        assert!(skewness > 0.10, "vertical skewness={skewness}");
    }

    #[test]
    fn skewed_drift_retains_the_target_moments_at_the_frozen_step_fraction() {
        const SAMPLE_COUNT: u64 = 50_000;
        let mut environment = neutral_environment();
        environment.monin_obukhov_length_m = -50.0;
        environment.sensible_heat_flux_w_m2 = 250.0;
        let scales = hanna_scales(environment).unwrap();
        let VerticalPdf::BiGaussian(pdf) = scales.vertical_pdf else {
            panic!("expected a skewed CBL PDF");
        };
        let elapsed = SKEWED_MAXIMUM_TL_FRACTION * scales.lagrangian_time_s[2];
        let sample_module = StableRandomId::from_text("boundary_layer_langevin.skewed-test");
        let mut previous = Vec::with_capacity(SAMPLE_COUNT as usize);
        let mut next = Vec::with_capacity(SAMPLE_COUNT as usize);
        for ordinal in 0..SAMPLE_COUNT {
            let particle = ParticleId(ordinal);
            let random_key = |sampling_dimension, draw_index| ProcessRandomKey {
                seed: 61,
                particle,
                module: sample_module,
                macro_step: 0,
                substep: 0,
                sampling_dimension,
                draw_index,
            };
            let prior = pdf.sample(
                CounterRng::sample_process_unit(random_key(VERTICAL_BRANCH_DIMENSION, 0)),
                CounterRng::sample_process_normal_pair(random_key(VERTICAL_VALUE_DIMENSION, 0))[0],
            );
            let innovation =
                CounterRng::sample_process_normal_pair(random_key(VERTICAL_VALUE_DIMENSION, 2))[0];
            let forward =
                advance_vertical_velocity(prior, &scales, elapsed, Direction::Forward, innovation)
                    .unwrap();
            let backward =
                advance_vertical_velocity(prior, &scales, elapsed, Direction::Backward, innovation)
                    .unwrap();
            previous.push(prior);
            next.push(0.5 * (forward + backward));
        }
        let (previous_mean, previous_variance) = mean_variance(&previous);
        let (next_mean, next_variance) = mean_variance(&next);
        let previous_skewness = standardized_third(&previous, previous_mean, previous_variance);
        let next_skewness = standardized_third(&next, next_mean, next_variance);
        assert!(next_mean.abs() <= 0.02 * scales.sigma_m_s[2]);
        assert!((next_variance / scales.sigma_m_s[2].powi(2) - 1.0).abs() <= 0.03);
        assert!((next_skewness - previous_skewness).abs() <= 0.03);
    }

    #[test]
    fn bi_gaussian_closure_reproduces_mean_variance_and_third_moment() {
        let sigma = 1.7;
        let requested_skewness = 0.8;
        let pdf = BiGaussianPdf::from_moments(sigma, requested_skewness).unwrap();
        let components = [
            (
                pdf.updraft_weight,
                pdf.updraft_mean_m_s,
                pdf.updraft_sigma_m_s,
            ),
            (
                pdf.downdraft_weight,
                pdf.downdraft_mean_m_s,
                pdf.downdraft_sigma_m_s,
            ),
        ];
        let mean = components
            .into_iter()
            .map(|(weight, component_mean, _)| weight * component_mean)
            .sum::<f64>();
        let second = components
            .into_iter()
            .map(|(weight, component_mean, component_sigma)| {
                weight * (component_sigma.powi(2) + component_mean.powi(2))
            })
            .sum::<f64>();
        let third = components
            .into_iter()
            .map(|(weight, component_mean, component_sigma)| {
                weight * (component_mean.powi(3) + 3.0 * component_mean * component_sigma.powi(2))
            })
            .sum::<f64>();
        assert!(mean.abs() <= 1.0e-12);
        assert!((second - sigma.powi(2)).abs() <= 1.0e-12);
        assert!((third - requested_skewness * sigma.powi(3)).abs() <= 1.0e-12);
    }

    #[test]
    fn bi_gaussian_tail_moments_remain_finite_beyond_density_underflow() {
        let sigma = 1.7;
        let pdf = VerticalPdf::BiGaussian(BiGaussianPdf::from_moments(sigma, 0.8).unwrap());
        for velocity in [-100.0 * sigma, 100.0 * sigma] {
            assert!(pdf.log_density(velocity).is_finite());
            assert!(
                pdf.log_negative_truncated_first_moment(velocity)
                    .unwrap()
                    .is_finite()
            );
        }
    }

    #[test]
    fn convective_tail_uses_a_local_spatial_gradient() {
        let scales = hanna_scales(wd76_environment(11.586_775_660_098_576)).unwrap();
        let drift = vertical_spatial_drift(&scales, 25.849_613_138_273_295).unwrap();
        assert!(drift.is_finite());
        assert!(drift.abs() <= 100.0, "vertical drift={drift}");
    }

    #[test]
    fn gaussian_density_gradient_has_the_full_thomson_coefficient() {
        let sigma = 2.0;
        let density_gradient = -1.2e-4;
        let pdf = VerticalPdf::Gaussian { sigma_m_s: sigma };
        let scales = TurbulenceScales {
            sigma_m_s: [1.0, 1.0, sigma],
            lagrangian_time_s: [10.0; 3],
            vertical_variance_gradient_s_2: 0.0,
            density_log_gradient_m_1: density_gradient,
            vertical_pdf: pdf,
            vertical_pdf_gradient: VerticalPdfGradient {
                lower: pdf,
                upper: pdf,
                lower_distance_m: 1.0,
                upper_distance_m: 1.0,
            },
        };
        for velocity in [-3.0, 0.0, 4.0] {
            assert_eq!(
                vertical_spatial_drift(&scales, velocity).unwrap(),
                sigma.powi(2) * density_gradient
            );
        }
    }

    #[test]
    fn gaussian_variance_gradient_has_the_full_thomson_coefficient() {
        let sigma = 2.0;
        let variance_gradient = 3.5e-3;
        let density_gradient = -1.2e-4;
        let pdf = VerticalPdf::Gaussian { sigma_m_s: sigma };
        let scales = TurbulenceScales {
            sigma_m_s: [1.0, 1.0, sigma],
            lagrangian_time_s: [10.0; 3],
            vertical_variance_gradient_s_2: variance_gradient,
            density_log_gradient_m_1: density_gradient,
            vertical_pdf: pdf,
            vertical_pdf_gradient: VerticalPdfGradient {
                lower: pdf,
                upper: pdf,
                lower_distance_m: 1.0,
                upper_distance_m: 1.0,
            },
        };
        for velocity in [-6.0, 0.0, 5.0] {
            let expected = 0.5 * (1.0 + (velocity / sigma).powi(2)) * variance_gradient
                + sigma.powi(2) * density_gradient;
            assert_eq!(vertical_spatial_drift(&scales, velocity).unwrap(), expected);
        }
    }

    #[test]
    fn backward_reverses_only_the_spatial_well_mixed_drift() {
        let environment = neutral_environment();
        let scales = hanna_scales(environment).unwrap();
        let previous = [0.2, -0.3, 0.4];
        let key = LangevinKey {
            seed: 7,
            particle: ParticleId(11),
            macro_step: 3,
            substep: 1,
            draw_index: MIDPOINT_DRAW_INDEX,
        };
        let elapsed = 0.25;
        let forward =
            update_velocity(previous, environment, elapsed, Direction::Forward, key).unwrap();
        let backward =
            update_velocity(previous, environment, elapsed, Direction::Backward, key).unwrap();
        let spatial = vertical_spatial_drift(&scales, previous[2]).unwrap();
        assert!((forward[2] - backward[2] - 2.0 * elapsed * spatial).abs() <= 1.0e-12);
        assert_eq!(forward[..2], backward[..2]);
    }

    #[test]
    fn skewed_cbl_uses_the_published_correlation_time_fraction() {
        let neutral = neutral_environment();
        let mut convective = neutral;
        convective.monin_obukhov_length_m = -50.0;
        convective.sensible_heat_flux_w_m2 = 250.0;
        let neutral_limit = maximum_stable_step_ns(neutral).unwrap();
        let convective_scales = hanna_scales(convective).unwrap();
        let expected = (convective_scales
            .lagrangian_time_s
            .into_iter()
            .fold(f64::INFINITY, f64::min)
            * SKEWED_MAXIMUM_TL_FRACTION
            * 1.0e9)
            .floor() as i64;
        assert_eq!(maximum_stable_step_ns(convective).unwrap(), expected);
        assert!(expected < neutral_limit);
    }

    #[test]
    #[ignore = "formal M6 external gate: WD76 concentration profiles"]
    fn formal_willis_deardorff_concentration_profiles() {
        const PARTICLE_COUNT: usize = 8_192;
        const THREAD_COUNT: usize = 8;
        const STEP_SECONDS: f64 = 0.25;
        const KERNEL_BANDWIDTH: f64 = 0.04;

        let asset: Wd76Asset = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/M6_WILLIS_DEARDORFF_CBL.v1.json"
        )))
        .unwrap();
        assert_eq!(asset.schema_version, "trajecta.m6.wd76-cbl/v1");
        assert_eq!(asset.asset_id, "willis-deardorff-cbl-1976/v1");
        assert_eq!(asset.digitization.point_count, 71);
        assert_eq!(asset.profiles.len(), 6);

        let snapshots = std::thread::scope(|scope| {
            let handles = (0..THREAD_COUNT)
                .map(|thread_index| {
                    let profiles = &asset.profiles;
                    scope.spawn(move || {
                        simulate_wd76_chunk(
                            thread_index * PARTICLE_COUNT / THREAD_COUNT,
                            PARTICLE_COUNT / THREAD_COUNT,
                            profiles,
                            STEP_SECONDS,
                        )
                    })
                })
                .collect::<Vec<_>>();
            let mut snapshots = vec![Vec::with_capacity(PARTICLE_COUNT); asset.profiles.len()];
            for handle in handles {
                for (combined, chunk) in snapshots.iter_mut().zip(handle.join().unwrap()) {
                    combined.extend(chunk);
                }
            }
            snapshots
        });

        let mut residuals = Vec::with_capacity(asset.digitization.point_count);
        let mut profile_reports = Vec::with_capacity(asset.profiles.len());
        for (profile, heights) in asset.profiles.iter().zip(&snapshots) {
            let mut profile_residuals = Vec::with_capacity(profile.points.len());
            let mut point_reports = Vec::with_capacity(profile.points.len());
            for point in &profile.points {
                let predicted =
                    reflected_kernel_density(heights, point.height_fraction, KERNEL_BANDWIDTH);
                let residual =
                    (predicted - point.dimensionless_concentration) / profile.normalization_scale;
                residuals.push(residual);
                profile_residuals.push(residual);
                point_reports.push(serde_json::json!({
                    "height_fraction": point.height_fraction,
                    "observed": point.dimensionless_concentration,
                    "predicted": predicted,
                    "normalized_residual": residual,
                }));
            }
            profile_reports.push(serde_json::json!({
                "dimensionless_time": profile.dimensionless_time,
                "observations": profile.points.len(),
                "normalized_bias": mean(&profile_residuals),
                "normalized_rmse": root_mean_square(&profile_residuals),
                "points": point_reports,
            }));
        }

        let normalized_bias = mean(&residuals);
        let normalized_rmse = root_mean_square(&residuals);
        println!(
            "{}",
            serde_json::json!({
                "asset_id": asset.asset_id,
                "particles": PARTICLE_COUNT,
                "step_seconds": STEP_SECONDS,
                "kernel_bandwidth_height_fraction": KERNEL_BANDWIDTH,
                "observations": residuals.len(),
                "normalized_bias": normalized_bias,
                "normalized_rmse": normalized_rmse,
                "profiles": profile_reports,
            })
        );
        assert!(residuals.len() >= 20);
        assert!(normalized_bias.abs() <= 0.20);
        assert!(normalized_rmse <= 0.30);
    }

    #[test]
    fn normal_tail_approximation_covers_the_drift_range() {
        let cases = [
            (-3.0, 0.998_650_101_968_369_9),
            (0.0, 0.5),
            (1.0, 0.158_655_253_931_457_07),
            (5.0, 2.866_515_718_791_933e-7),
        ];
        for (value, expected) in cases {
            assert!((standard_normal_upper(value) - expected).abs() <= 8.0e-8);
        }
    }

    #[derive(Deserialize)]
    struct Wd76Asset {
        schema_version: String,
        asset_id: String,
        digitization: Wd76Digitization,
        profiles: Vec<Wd76Profile>,
    }

    #[derive(Deserialize)]
    struct Wd76Digitization {
        point_count: usize,
    }

    #[derive(Deserialize)]
    struct Wd76Profile {
        dimensionless_time: f64,
        normalization_scale: f64,
        points: Vec<Wd76Point>,
    }

    #[derive(Deserialize)]
    struct Wd76Point {
        height_fraction: f64,
        dimensionless_concentration: f64,
    }

    #[derive(Clone, Copy)]
    struct Wd76Particle {
        id: ParticleId,
        height_m: f64,
        velocity_m_s: [f64; 3],
    }

    fn wd76_environment(height_m: f64) -> BoundaryLayerEnvironment {
        const BOUNDARY_LAYER_HEIGHT_M: f64 = 1_000.0;
        const AIR_TEMPERATURE_K: f64 = 290.0;
        const AIR_DENSITY_KG_M3: f64 = 1.2;
        const CONVECTIVE_VELOCITY_M_S: f64 = 2.0;
        let sensible_heat_flux_w_m2 = CONVECTIVE_VELOCITY_M_S.powi(3)
            * AIR_TEMPERATURE_K
            * AIR_DENSITY_KG_M3
            * DRY_AIR_HEAT_CAPACITY_J_KG_K
            / (GRAVITY_M_S2 * BOUNDARY_LAYER_HEIGHT_M);
        BoundaryLayerEnvironment {
            height_agl_m: height_m,
            boundary_layer_height_m: BOUNDARY_LAYER_HEIGHT_M,
            friction_velocity_m_s: 0.31,
            monin_obukhov_length_m: -9.4,
            sensible_heat_flux_w_m2,
            roughness_length_m: 0.16,
            air_temperature_k: AIR_TEMPERATURE_K,
            air_density_kg_m3: AIR_DENSITY_KG_M3,
        }
    }

    fn simulate_wd76_chunk(
        first_particle: usize,
        particle_count: usize,
        profiles: &[Wd76Profile],
        step_seconds: f64,
    ) -> Vec<Vec<f64>> {
        const BOUNDARY_LAYER_HEIGHT_M: f64 = 1_000.0;
        let mut particles = (first_particle..first_particle + particle_count)
            .map(|ordinal| {
                let id = ParticleId(ordinal as u64);
                let height_m = 70.0;
                let velocity_m_s = sample_stationary_velocity(
                    wd76_environment(height_m),
                    LangevinKey {
                        seed: 0x5744_3736,
                        particle: id,
                        macro_step: 0,
                        substep: 0,
                        draw_index: BIRTH_DRAW_INDEX,
                    },
                )
                .unwrap();
                Wd76Particle {
                    id,
                    height_m,
                    velocity_m_s,
                }
            })
            .collect::<Vec<_>>();
        let mut elapsed = 0.0;
        let mut substep = 0_u32;
        let mut snapshots = Vec::with_capacity(profiles.len());
        for profile in profiles {
            let target = profile.dimensionless_time * BOUNDARY_LAYER_HEIGHT_M / 2.0;
            while elapsed < target {
                let dt = step_seconds.min(target - elapsed);
                for particle in &mut particles {
                    let environment = wd76_environment(particle.height_m);
                    let key = LangevinKey {
                        seed: 0x5744_3736,
                        particle: particle.id,
                        macro_step: 0,
                        substep,
                        draw_index: MIDPOINT_DRAW_INDEX,
                    };
                    let midpoint = update_velocity(
                        particle.velocity_m_s,
                        environment,
                        0.5 * dt,
                        Direction::Forward,
                        key,
                    )
                    .unwrap_or_else(|error| {
                        panic!(
                            "WD76 midpoint failed: error={error}, particle={}, height_m={}, velocity_m_s={:?}, elapsed_s={elapsed}, substep={substep}",
                            particle.id.0, particle.height_m, particle.velocity_m_s
                        )
                    });
                    let mut endpoint = update_velocity(
                        midpoint,
                        environment,
                        0.5 * dt,
                        Direction::Forward,
                        LangevinKey {
                            draw_index: ENDPOINT_DRAW_INDEX,
                            ..key
                        },
                    )
                    .unwrap_or_else(|error| {
                        panic!(
                            "WD76 endpoint failed: error={error}, particle={}, height_m={}, prior_velocity_m_s={:?}, midpoint_velocity_m_s={midpoint:?}, elapsed_s={elapsed}, substep={substep}",
                            particle.id.0, particle.height_m, particle.velocity_m_s
                        )
                    });
                    let proposed_height = particle.height_m + dt * midpoint[2];
                    if proposed_height < 0.0 {
                        particle.height_m = -proposed_height;
                        endpoint[2] = -endpoint[2];
                    } else if proposed_height > BOUNDARY_LAYER_HEIGHT_M {
                        particle.height_m = 2.0 * BOUNDARY_LAYER_HEIGHT_M - proposed_height;
                        endpoint[2] = -endpoint[2];
                    } else {
                        particle.height_m = proposed_height;
                    }
                    particle.velocity_m_s = endpoint;
                }
                elapsed += dt;
                substep = substep.checked_add(1).unwrap();
            }
            snapshots.push(
                particles
                    .iter()
                    .map(|particle| particle.height_m / BOUNDARY_LAYER_HEIGHT_M)
                    .collect(),
            );
        }
        snapshots
    }

    fn reflected_kernel_density(samples: &[f64], value: f64, bandwidth: f64) -> f64 {
        let inverse = 1.0 / (samples.len() as f64 * bandwidth);
        samples
            .iter()
            .map(|sample| {
                standard_normal_density((value - sample) / bandwidth)
                    + standard_normal_density((value + sample) / bandwidth)
            })
            .sum::<f64>()
            * inverse
    }

    fn mean(values: &[f64]) -> f64 {
        values.iter().sum::<f64>() / values.len() as f64
    }

    fn root_mean_square(values: &[f64]) -> f64 {
        (values.iter().map(|value| value * value).sum::<f64>() / values.len() as f64).sqrt()
    }

    fn mean_variance(values: &[f64]) -> (f64, f64) {
        let count = values.len() as f64;
        let mean = values.iter().sum::<f64>() / count;
        let variance = values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / count;
        (mean, variance)
    }

    fn standardized_third(values: &[f64], mean: f64, variance: f64) -> f64 {
        values
            .iter()
            .map(|value| (value - mean).powi(3))
            .sum::<f64>()
            / (values.len() as f64 * variance.powf(1.5))
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct PairMoments {
        count: u64,
        previous_mean: f64,
        next_mean: f64,
        previous_m2: f64,
        next_m2: f64,
        co_moment: f64,
    }

    impl PairMoments {
        fn push(&mut self, previous: f64, next: f64) {
            self.count += 1;
            let count = self.count as f64;
            let previous_delta = previous - self.previous_mean;
            self.previous_mean += previous_delta / count;
            let next_delta = next - self.next_mean;
            self.next_mean += next_delta / count;
            self.previous_m2 += previous_delta * (previous - self.previous_mean);
            self.next_m2 += next_delta * (next - self.next_mean);
            self.co_moment += previous_delta * (next - self.next_mean);
        }

        fn finish(self) -> FinishedPairMoments {
            let count = self.count as f64;
            let previous_variance = self.previous_m2 / count;
            let next_variance = self.next_m2 / count;
            let covariance = self.co_moment / count;
            FinishedPairMoments {
                previous_mean: self.previous_mean,
                next_mean: self.next_mean,
                previous_second: previous_variance + self.previous_mean.powi(2),
                next_second: next_variance + self.next_mean.powi(2),
                cross: covariance + self.previous_mean * self.next_mean,
                next_variance,
                correlation: covariance / (previous_variance * next_variance).sqrt(),
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct FinishedPairMoments {
        previous_mean: f64,
        next_mean: f64,
        previous_second: f64,
        next_second: f64,
        cross: f64,
        next_variance: f64,
        correlation: f64,
    }
}
