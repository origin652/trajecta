//! Three-dimensional mesoscale Ornstein-Uhlenbeck velocity.

use crate::particle::ParticleId;
use crate::rng::{CounterRng, ProcessRandomKey, StableRandomId};

const EASTWARD_DIMENSION: u32 = 128;
const NORTHWARD_DIMENSION: u32 = 129;
const VERTICAL_DIMENSION: u32 = 130;
const MESOSCALE_RANDOM_ID: StableRandomId = StableRandomId(0x1043_7cbc_619a_de5a);

#[derive(Clone, Copy, Debug)]
pub(super) struct MesoscaleKey {
    pub(super) seed: u64,
    pub(super) particle: ParticleId,
    pub(super) macro_step: u64,
    pub(super) substep: u32,
    pub(super) draw_index: u32,
}

pub(super) fn sample_stationary_velocity(
    variance_m2_s2: [f64; 3],
    key: MesoscaleKey,
) -> Result<[f64; 3], &'static str> {
    validate_variance(variance_m2_s2)?;
    Ok(std::array::from_fn(|component| {
        if variance_m2_s2[component] == 0.0 {
            0.0
        } else {
            variance_m2_s2[component].sqrt() * innovation(key, component)
        }
    }))
}

pub(super) fn update_velocity(
    previous_m_s: [f64; 3],
    variance_m2_s2: [f64; 3],
    elapsed_seconds: f64,
    native_interval_seconds: f64,
    correlation_interval_fraction: f64,
    key: MesoscaleKey,
) -> Result<[f64; 3], &'static str> {
    validate_variance(variance_m2_s2)?;
    if previous_m_s.into_iter().any(|value| !value.is_finite()) {
        return Err("non-finite mesoscale velocity state");
    }
    if !elapsed_seconds.is_finite()
        || elapsed_seconds <= 0.0
        || !native_interval_seconds.is_finite()
        || native_interval_seconds <= 0.0
        || !correlation_interval_fraction.is_finite()
        || !(0.05..=1.0).contains(&correlation_interval_fraction)
    {
        return Err("invalid mesoscale correlation interval");
    }
    let correlation_seconds = native_interval_seconds * correlation_interval_fraction;
    let decay = (-elapsed_seconds / correlation_seconds).exp();
    let innovation_scale = (1.0 - decay * decay).max(0.0).sqrt();
    let next = std::array::from_fn(|component| {
        if variance_m2_s2[component] == 0.0 {
            0.0
        } else {
            let noise =
                innovation_scale * variance_m2_s2[component].sqrt() * innovation(key, component);
            decay.mul_add(previous_m_s[component], noise)
        }
    });
    if next.into_iter().all(f64::is_finite) {
        Ok(next)
    } else {
        Err("mesoscale OU update produced a non-finite velocity")
    }
}

fn innovation(key: MesoscaleKey, component: usize) -> f64 {
    let dimension = [EASTWARD_DIMENSION, NORTHWARD_DIMENSION, VERTICAL_DIMENSION][component];
    CounterRng::sample_process_normal_pair(ProcessRandomKey {
        seed: key.seed,
        particle: key.particle,
        module: MESOSCALE_RANDOM_ID,
        macro_step: key.macro_step,
        substep: key.substep,
        sampling_dimension: dimension,
        draw_index: key.draw_index,
    })[0]
}

fn validate_variance(variance_m2_s2: [f64; 3]) -> Result<(), &'static str> {
    if variance_m2_s2
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
    {
        Ok(())
    } else {
        Err("invalid mesoscale wind variance")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> MesoscaleKey {
        MesoscaleKey {
            seed: 7,
            particle: ParticleId(11),
            macro_step: 3,
            substep: 2,
            draw_index: 4,
        }
    }

    #[test]
    fn zero_variance_resets_every_component_exactly() {
        assert_eq!(
            update_velocity([3.0, -2.0, 1.0], [0.0; 3], 30.0, 3_600.0, 0.5, key()),
            Ok([0.0; 3])
        );
        assert_eq!(sample_stationary_velocity([0.0; 3], key()), Ok([0.0; 3]));
    }

    #[test]
    fn exact_transition_uses_frozen_dimensions() {
        let variance: [f64; 3] = [4.0, 9.0, 16.0];
        let elapsed: f64 = 300.0;
        let interval: f64 = 3_600.0;
        let fraction: f64 = 0.5;
        let decay = (-elapsed / (interval * fraction)).exp();
        let scale = (1.0_f64 - decay * decay).sqrt();
        let expected = std::array::from_fn(|component| {
            decay.mul_add(
                1.0,
                scale * variance[component].sqrt() * innovation(key(), component),
            )
        });
        assert_eq!(
            update_velocity([1.0; 3], variance, elapsed, interval, fraction, key()),
            Ok(expected)
        );
    }

    #[test]
    fn stationary_ensemble_meets_the_frozen_ou_moment_gates() -> Result<(), &'static str> {
        const SAMPLES: u64 = 100_000;
        let variance: [f64; 3] = [4.0, 9.0, 16.0];
        let elapsed_seconds: f64 = 300.0;
        let native_interval_seconds: f64 = 3_600.0;
        let interval_fraction: f64 = 0.5;
        let expected_correlation =
            (-elapsed_seconds / (native_interval_seconds * interval_fraction)).exp();
        let mut x_sum = [0.0; 3];
        let mut y_sum = [0.0; 3];
        let mut xx_sum = [0.0; 3];
        let mut yy_sum = [0.0; 3];
        let mut xy_sum = [0.0; 3];

        for particle in 0..SAMPLES {
            let initial = sample_stationary_velocity(
                variance,
                MesoscaleKey {
                    seed: 0x6a20_2026,
                    particle: ParticleId(particle),
                    macro_step: 0,
                    substep: 0,
                    draw_index: 0,
                },
            )?;
            let next = update_velocity(
                initial,
                variance,
                elapsed_seconds,
                native_interval_seconds,
                interval_fraction,
                MesoscaleKey {
                    seed: 0x6a20_2026,
                    particle: ParticleId(particle),
                    macro_step: 1,
                    substep: 0,
                    draw_index: 0,
                },
            )?;
            for component in 0..3 {
                x_sum[component] += initial[component];
                y_sum[component] += next[component];
                xx_sum[component] += initial[component] * initial[component];
                yy_sum[component] += next[component] * next[component];
                xy_sum[component] += initial[component] * next[component];
            }
        }

        let count = SAMPLES as f64;
        for component in 0..3 {
            let x_mean = x_sum[component] / count;
            let y_mean = y_sum[component] / count;
            let x_variance = xx_sum[component] / count - x_mean * x_mean;
            let y_variance = yy_sum[component] / count - y_mean * y_mean;
            let covariance = xy_sum[component] / count - x_mean * y_mean;
            let correlation = covariance / (x_variance * y_variance).sqrt();
            let sigma = variance[component].sqrt();
            assert!(
                x_mean.abs() <= 0.02 * sigma,
                "component={component} x_mean={x_mean}"
            );
            assert!(
                y_mean.abs() <= 0.02 * sigma,
                "component={component} y_mean={y_mean}"
            );
            assert!(
                (x_variance / variance[component] - 1.0).abs() <= 0.03,
                "component={component} x_variance={x_variance}"
            );
            assert!(
                (y_variance / variance[component] - 1.0).abs() <= 0.03,
                "component={component} y_variance={y_variance}"
            );
            assert!(
                (correlation - expected_correlation).abs() <= 0.02,
                "component={component} correlation={correlation} expected={expected_correlation}"
            );
        }
        Ok(())
    }
}
