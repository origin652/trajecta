//! # Contract: event-native vertical release sampling
//!
//! Samples fixed or closed-interval ASL/AGL/pressure values with the frozen
//! vertical Philox dimension.

use trajecta_case::model::population::ReleaseVerticalSpec;

use crate::particle::ParticleId;
use crate::release::{ReleaseError, ReleaseSamplingRequest, VerticalSampler};
use crate::rng::{CounterRng, RELEASE_VERTICAL_DIMENSION, RandomKey, StableRandomId};

/// Samples event-native vertical coordinates with the frozen RNG dimensions.
#[derive(Clone, Copy, Debug, Default)]
pub struct SpecVerticalSampler;

impl VerticalSampler for SpecVerticalSampler {
    fn sample_vertical(
        &self,
        request: ReleaseSamplingRequest<'_>,
    ) -> Result<Vec<f64>, ReleaseError> {
        request.validate()?;
        let (lower, upper) = vertical_bounds(&request.event.vertical)?;
        let population = StableRandomId::from_text(&request.population_id.0);
        let event = StableRandomId::from_text(&request.event.id.0);
        let mut out = Vec::with_capacity(request.count);
        for offset in 0..request.count {
            let ordinal = request
                .first_ordinal
                .checked_add(u64::try_from(offset).map_err(|_| ReleaseError::CountOverflow)?)
                .ok_or(ReleaseError::CountOverflow)?;
            let value = if let Some(upper) = upper {
                let u = CounterRng::sample_unit(RandomKey {
                    seed: request.seed,
                    population,
                    lifecycle_event: event,
                    particle: ParticleId(ordinal),
                    sampling_dimension: RELEASE_VERTICAL_DIMENSION,
                    draw_index: 0,
                });
                lower + (upper - lower) * u
            } else {
                lower
            };
            if !value.is_finite() {
                return Err(ReleaseError::InvalidGeometry);
            }
            if matches!(request.event.vertical, ReleaseVerticalSpec::Pressure { .. })
                && value <= 0.0
            {
                return Err(ReleaseError::InvalidGeometry);
            }
            out.push(value);
        }
        Ok(out)
    }
}

fn vertical_bounds(spec: &ReleaseVerticalSpec) -> Result<(f64, Option<f64>), ReleaseError> {
    match spec {
        ReleaseVerticalSpec::AboveSeaLevel { lower, upper }
        | ReleaseVerticalSpec::AboveGround { lower, upper } => {
            let lower = lower.value_si();
            let upper = upper
                .as_ref()
                .map(trajecta_case::quantity::Quantity::value_si);
            if !lower.is_finite() || upper.is_some_and(|value| !value.is_finite()) {
                return Err(ReleaseError::InvalidGeometry);
            }
            if let Some(upper) = upper {
                if upper < lower {
                    return Err(ReleaseError::InvalidGeometry);
                }
            }
            if matches!(spec, ReleaseVerticalSpec::AboveGround { .. }) && lower < 0.0 {
                return Err(ReleaseError::InvalidGeometry);
            }
            Ok((lower, upper))
        }
        ReleaseVerticalSpec::Pressure { lower, upper } => {
            let lower = lower.value_si();
            let upper = upper
                .as_ref()
                .map(trajecta_case::quantity::Quantity::value_si);
            if !lower.is_finite()
                || lower <= 0.0
                || upper.is_some_and(|value| !value.is_finite() || value <= 0.0)
            {
                return Err(ReleaseError::InvalidGeometry);
            }
            if let Some(upper) = upper {
                // Closed numeric interval; order may be high→low pressure.
                Ok((lower.min(upper), Some(lower.max(upper))))
            } else {
                Ok((lower, None))
            }
        }
    }
}
