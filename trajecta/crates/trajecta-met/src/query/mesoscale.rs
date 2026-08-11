//! # Contract: local space-time wind variance
//!
//! The mesoscale Markov process consumes a three-component weighted variance
//! sampled from two time planes, four horizontal corners, and two native
//! vertical levels. Invalid query points carry a typed status and finite zero
//! storage; callers may read scientific values only for `Ok` rows.

use crate::query::output::{SampleStatus, StatusColumn};

/// Stable identity of the M6-A2 local-variance calculation.
pub const MESOSCALE_LOCAL_VARIANCE_ALGORITHM_ID: &str = "three_dimensional_local_variance_ou/v1";

/// I/O-free local wind statistics returned in caller order.
#[derive(Clone, Debug, PartialEq)]
pub struct MesoscaleStatisticsOutput {
    variance_m2_s2: Vec<[f64; 3]>,
    status: StatusColumn,
    native_interval_seconds: f64,
}

impl MesoscaleStatisticsOutput {
    pub(crate) fn new(
        variance_m2_s2: Vec<[f64; 3]>,
        status: StatusColumn,
        native_interval_seconds: f64,
    ) -> Result<Self, MesoscaleStatisticsError> {
        if variance_m2_s2.len() != status.len() {
            return Err(MesoscaleStatisticsError::LengthMismatch);
        }
        if !native_interval_seconds.is_finite() || native_interval_seconds <= 0.0 {
            return Err(MesoscaleStatisticsError::InvalidNativeInterval);
        }
        if variance_m2_s2
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(MesoscaleStatisticsError::InvalidVariance);
        }
        Ok(Self {
            variance_m2_s2,
            status,
            native_interval_seconds,
        })
    }

    /// Returns the number of input points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.status.len()
    }

    /// Returns whether the output contains no points.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.status.is_empty()
    }

    /// Returns typed sample status in caller order.
    #[must_use]
    pub const fn status(&self) -> &StatusColumn {
        &self.status
    }

    /// Returns the native meteorology interval used by the OU process.
    #[must_use]
    pub const fn native_interval_seconds(&self) -> f64 {
        self.native_interval_seconds
    }

    /// Returns three component variances only for a valid point.
    #[must_use]
    pub fn variance_m2_s2(&self, index: usize) -> Option<[f64; 3]> {
        (self.status.get(index) == Some(SampleStatus::Ok))
            .then(|| self.variance_m2_s2.get(index).copied())
            .flatten()
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WeightedVelocity {
    pub(crate) weight: f64,
    pub(crate) velocity_m_s: [f64; 3],
}

pub(crate) fn weighted_variance(
    support: &[WeightedVelocity],
) -> Result<[f64; 3], MesoscaleStatisticsError> {
    if support.is_empty()
        || support.iter().any(|sample| {
            !sample.weight.is_finite()
                || sample.weight < 0.0
                || sample
                    .velocity_m_s
                    .into_iter()
                    .any(|value| !value.is_finite())
        })
    {
        return Err(MesoscaleStatisticsError::InvalidSupport);
    }
    let weight_sum = support.iter().map(|sample| sample.weight).sum::<f64>();
    if (weight_sum - 1.0).abs() > 1.0e-12 {
        return Err(MesoscaleStatisticsError::InvalidSupport);
    }
    let mean: [f64; 3] = std::array::from_fn(|component| {
        support.iter().fold(0.0, |sum, sample| {
            sample.weight.mul_add(sample.velocity_m_s[component], sum)
        })
    });
    let variance = std::array::from_fn(|component| {
        support.iter().fold(0.0, |sum, sample| {
            let difference = sample.velocity_m_s[component] - mean[component];
            sample.weight.mul_add(difference * difference, sum)
        })
    });
    if variance
        .into_iter()
        .any(|value| !value.is_finite() || value < 0.0)
    {
        Err(MesoscaleStatisticsError::InvalidVariance)
    } else {
        Ok(variance)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MesoscaleStatisticsError {
    LengthMismatch,
    InvalidNativeInterval,
    InvalidSupport,
    InvalidVariance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_support_has_exact_zero_variance() {
        let support = [WeightedVelocity {
            weight: 1.0,
            velocity_m_s: [4.0, -2.0, 0.5],
        }];
        assert_eq!(weighted_variance(&support), Ok([0.0; 3]));
    }

    #[test]
    fn weighted_variance_uses_all_three_components() {
        let support = [
            WeightedVelocity {
                weight: 0.25,
                velocity_m_s: [0.0, 2.0, -2.0],
            },
            WeightedVelocity {
                weight: 0.75,
                velocity_m_s: [4.0, 6.0, 2.0],
            },
        ];
        assert_eq!(weighted_variance(&support), Ok([3.0, 3.0, 3.0]));
    }
}
