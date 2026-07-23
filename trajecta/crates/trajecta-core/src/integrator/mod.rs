//! # Contract: particle integration
//!
//! The v0 integrator is a spherical midpoint RK2 scheme. It queries wind at
//! the start and midpoint, advances longitude/latitude on a sphere, uses
//! geometric vertical velocity, supports signed time, and applies boundary
//! policies after proposing the full step.

use trajecta_case::model::time::Timestamp;
use trajecta_met::query::engine::{BatchWorkspace, ExecutionContext, MetEngine};
use trajecta_met::query::output::SampleStatus;
use trajecta_met::query::request::{QueryBatch, QueryPointArrays, TransportPlan, VerticalQuery};

use crate::clock::SignedDuration;
use crate::particle::{ParticleBatch, ParticleState, ParticleStatus, TerminationReason};
use crate::science::M4_CONSTANTS;

/// Immutable particle state and signed step presented to an integrator.
#[derive(Clone, Copy, Debug)]
pub struct IntegratorInput<'a> {
    /// Current particle batch.
    pub particles: &'a ParticleBatch,
    /// Physical start time.
    pub time: Timestamp,
    /// Signed step duration.
    pub step: SignedDuration,
}

/// Mutable meteorology access and immutable execution policy for one step.
pub struct IntegratorContext<'a> {
    /// Meteorology engine used only through explicit prepare/query phases.
    pub meteorology: &'a mut MetEngine,
    /// Precompiled transport query plan.
    pub query_plan: &'a TransportPlan,
    /// Caller-owned execution context.
    pub execution: &'a dyn ExecutionContext,
}

/// Proposed next particle batch and integration-only diagnostics.
///
/// Boundary policies are deliberately applied by the runner after this result
/// is produced, so custom integrators cannot alter boundary lifecycle order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepResult {
    /// Particle state after integration and before boundary policies.
    pub particles: ParticleBatch,
    /// Number of particles terminated abnormally by integration itself.
    pub abnormal_terminated_count: usize,
}

/// Deterministic particle-advance interface.
pub trait IntegratorModel: Send + Sync {
    /// Returns the stable implementation identifier.
    fn model_id(&self) -> &'static str;

    /// Advances one batch by one signed step.
    fn advance(
        &self,
        input: IntegratorInput<'_>,
        context: &mut IntegratorContext<'_>,
    ) -> Result<StepResult, IntegratorError>;
}

/// v0 spherical midpoint integrator without random turbulence.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rk2Spherical;

impl IntegratorModel for Rk2Spherical {
    fn model_id(&self) -> &'static str {
        crate::science::RK2_SPHERICAL_ID
    }

    fn advance(
        &self,
        input: IntegratorInput<'_>,
        context: &mut IntegratorContext<'_>,
    ) -> Result<StepResult, IntegratorError> {
        advance_with_transport_query(input, |time, particles| {
            query_transport(context, time, particles)
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct VelocitySample {
    status: SampleStatus,
    eastward_m_s: Option<f64>,
    northward_m_s: Option<f64>,
    vertical_m_s: Option<f64>,
}

impl VelocitySample {
    fn velocity(self) -> Option<[f64; 3]> {
        if self.status != SampleStatus::Ok {
            return None;
        }
        Some([self.eastward_m_s?, self.northward_m_s?, self.vertical_m_s?])
    }
}

fn query_transport(
    context: &mut IntegratorContext<'_>,
    time: Timestamp,
    particles: &[ParticleState],
) -> Result<Vec<VelocitySample>, IntegratorError> {
    if particles.is_empty() {
        return Ok(Vec::new());
    }
    let window = context
        .meteorology
        .prepare(time)
        .map_err(|error| IntegratorError::Meteorology(format!("{error:?}")))?;
    let mut workspace = BatchWorkspace::default();
    let prepared = window
        .prepare_transport_batch(
            context.query_plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points: QueryPointArrays {
                    longitude_degrees: particles
                        .iter()
                        .map(|particle| particle.longitude_degrees)
                        .collect(),
                    latitude_degrees: particles
                        .iter()
                        .map(|particle| particle.latitude_degrees)
                        .collect(),
                    vertical: particles
                        .iter()
                        .map(|particle| particle.height_asl_m)
                        .collect(),
                },
            },
            &mut workspace,
        )
        .map_err(|error| IntegratorError::Meteorology(format!("{error:?}")))?;
    let output = prepared
        .execute(context.execution, &mut workspace)
        .map_err(|error| IntegratorError::Meteorology(format!("{error:?}")))?;
    (0..particles.len())
        .map(|index| {
            let row = output.row(index).ok_or_else(|| {
                IntegratorError::NumericalInvariant(
                    "transport output returned the wrong row count".into(),
                )
            })?;
            Ok(VelocitySample {
                status: row.status(),
                eastward_m_s: row.eastward_wind_m_s(),
                northward_m_s: row.northward_wind_m_s(),
                vertical_m_s: row.geometric_vertical_velocity_m_s(),
            })
        })
        .collect()
}

fn advance_with_transport_query<F>(
    input: IntegratorInput<'_>,
    mut query: F,
) -> Result<StepResult, IntegratorError>
where
    F: FnMut(Timestamp, &[ParticleState]) -> Result<Vec<VelocitySample>, IntegratorError>,
{
    input
        .particles
        .validate()
        .map_err(|_| IntegratorError::InvalidParticleBatch)?;
    if input.step == SignedDuration::ZERO {
        return Ok(StepResult {
            particles: input.particles.clone(),
            abnormal_terminated_count: 0,
        });
    }

    let midpoint_time = checked_add_timestamp(input.time, input.step.0 / 2)?;
    checked_add_timestamp(input.time, input.step.0)?;
    let seconds = input.step.0 as f64 * 1.0e-9;
    if !seconds.is_finite() {
        return Err(IntegratorError::NumericalInvariant(
            "signed step cannot be represented in seconds".into(),
        ));
    }

    let mut proposal = input.particles.clone();
    let mut active_indices = Vec::new();
    let mut active_states = Vec::new();
    for index in 0..input
        .particles
        .len()
        .map_err(|_| IntegratorError::InvalidParticleBatch)?
    {
        let state = input
            .particles
            .state(index)
            .map_err(|_| IntegratorError::InvalidParticleBatch)?;
        if state.status == ParticleStatus::Alive {
            active_indices.push(index);
            active_states.push(state);
        }
    }

    let start_samples = query(input.time, &active_states)?;
    if start_samples.len() != active_states.len() {
        return Err(IntegratorError::NumericalInvariant(
            "start transport query returned the wrong row count".into(),
        ));
    }

    let mut midpoint_states = Vec::new();
    let mut midpoint_indices = Vec::new();
    let mut abnormal_terminated_count = 0;
    for ((batch_index, start), sample) in active_indices
        .iter()
        .copied()
        .zip(&active_states)
        .zip(start_samples)
    {
        let Some([eastward, northward, vertical]) = sample.velocity() else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::InvalidMeteorology,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        let Some((longitude, latitude)) = spherical_displacement(
            start.longitude_degrees,
            start.latitude_degrees,
            eastward,
            northward,
            0.5 * seconds,
        ) else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::NumericalFailure,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        let height = start.height_asl_m + 0.5 * seconds * vertical;
        if !height.is_finite() {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::NumericalFailure,
            )?;
            abnormal_terminated_count += 1;
            continue;
        }
        let mut midpoint = start.clone();
        midpoint.longitude_degrees = longitude;
        midpoint.latitude_degrees = latitude;
        midpoint.height_asl_m = height;
        midpoint_states.push(midpoint);
        midpoint_indices.push(batch_index);
    }

    let midpoint_samples = query(midpoint_time, &midpoint_states)?;
    if midpoint_samples.len() != midpoint_states.len() {
        return Err(IntegratorError::NumericalInvariant(
            "midpoint transport query returned the wrong row count".into(),
        ));
    }
    for ((batch_index, midpoint), sample) in midpoint_indices
        .into_iter()
        .zip(midpoint_states)
        .zip(midpoint_samples)
    {
        let Some([eastward, northward, vertical]) = sample.velocity() else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::InvalidMeteorology,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        let start = input
            .particles
            .state(batch_index)
            .map_err(|_| IntegratorError::InvalidParticleBatch)?;
        let Some((longitude, latitude)) = spherical_displacement_from_midpoint_velocity(
            &start, &midpoint, eastward, northward, seconds,
        ) else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::NumericalFailure,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        let height = start.height_asl_m + seconds * vertical;
        let Some(integration_offset_ns) = start
            .integration_offset_ns
            .checked_add(input.step.as_nanoseconds())
        else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::NumericalFailure,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        let Some(elapsed_age_ns) = start
            .elapsed_age_ns
            .checked_add(input.step.as_nanoseconds().unsigned_abs())
        else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::NumericalFailure,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        if !height.is_finite() {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::NumericalFailure,
            )?;
            abnormal_terminated_count += 1;
            continue;
        }
        let mut advanced = start;
        advanced.longitude_degrees = longitude;
        advanced.latitude_degrees = latitude;
        advanced.height_asl_m = height;
        advanced.integration_offset_ns = integration_offset_ns;
        advanced.elapsed_age_ns = elapsed_age_ns;
        proposal
            .set_state(batch_index, advanced)
            .map_err(|_| IntegratorError::InvalidParticleBatch)?;
    }

    Ok(StepResult {
        particles: proposal,
        abnormal_terminated_count,
    })
}

fn terminate_particle(
    particles: &mut ParticleBatch,
    index: usize,
    reason: TerminationReason,
) -> Result<(), IntegratorError> {
    let mut state = particles
        .state(index)
        .map_err(|_| IntegratorError::InvalidParticleBatch)?;
    state.status = ParticleStatus::Terminated { reason };
    particles
        .set_state(index, state)
        .map_err(|_| IntegratorError::InvalidParticleBatch)
}

fn spherical_displacement(
    longitude_degrees: f64,
    latitude_degrees: f64,
    eastward_m_s: f64,
    northward_m_s: f64,
    seconds: f64,
) -> Option<(f64, f64)> {
    let position = unit_position(longitude_degrees, latitude_degrees)?;
    let velocity = tangent_velocity(
        longitude_degrees,
        latitude_degrees,
        eastward_m_s,
        northward_m_s,
    )?;
    let scale = seconds / M4_CONSTANTS.earth_radius_m;
    unit_to_lon_lat(normalize([
        position[0] + scale * velocity[0],
        position[1] + scale * velocity[1],
        position[2] + scale * velocity[2],
    ])?)
}

fn spherical_displacement_from_midpoint_velocity(
    start: &ParticleState,
    midpoint: &ParticleState,
    eastward_m_s: f64,
    northward_m_s: f64,
    seconds: f64,
) -> Option<(f64, f64)> {
    let position = unit_position(start.longitude_degrees, start.latitude_degrees)?;
    let velocity = tangent_velocity(
        midpoint.longitude_degrees,
        midpoint.latitude_degrees,
        eastward_m_s,
        northward_m_s,
    )?;
    let scale = seconds / M4_CONSTANTS.earth_radius_m;
    unit_to_lon_lat(normalize([
        position[0] + scale * velocity[0],
        position[1] + scale * velocity[1],
        position[2] + scale * velocity[2],
    ])?)
}

fn unit_position(longitude_degrees: f64, latitude_degrees: f64) -> Option<[f64; 3]> {
    if !longitude_degrees.is_finite()
        || !latitude_degrees.is_finite()
        || !(-90.0..=90.0).contains(&latitude_degrees)
    {
        return None;
    }
    let longitude = longitude_degrees.to_radians();
    let latitude = latitude_degrees.to_radians();
    let cos_latitude = latitude.cos();
    Some([
        cos_latitude * longitude.cos(),
        cos_latitude * longitude.sin(),
        latitude.sin(),
    ])
}

fn tangent_velocity(
    longitude_degrees: f64,
    latitude_degrees: f64,
    eastward_m_s: f64,
    northward_m_s: f64,
) -> Option<[f64; 3]> {
    if !eastward_m_s.is_finite() || !northward_m_s.is_finite() {
        return None;
    }
    let longitude = longitude_degrees.to_radians();
    let latitude = latitude_degrees.to_radians();
    let east = [-longitude.sin(), longitude.cos(), 0.0];
    let north = [
        -latitude.sin() * longitude.cos(),
        -latitude.sin() * longitude.sin(),
        latitude.cos(),
    ];
    Some([
        eastward_m_s * east[0] + northward_m_s * north[0],
        eastward_m_s * east[1] + northward_m_s * north[1],
        eastward_m_s * east[2] + northward_m_s * north[2],
    ])
}

fn normalize(vector: [f64; 3]) -> Option<[f64; 3]> {
    let norm = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    Some([vector[0] / norm, vector[1] / norm, vector[2] / norm])
}

fn unit_to_lon_lat(position: [f64; 3]) -> Option<(f64, f64)> {
    if position.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let longitude = position[1].atan2(position[0]).to_degrees();
    let latitude = position[2].clamp(-1.0, 1.0).asin().to_degrees();
    (longitude.is_finite() && latitude.is_finite()).then_some((longitude, latitude))
}

fn checked_add_timestamp(
    timestamp: Timestamp,
    nanoseconds: i64,
) -> Result<Timestamp, IntegratorError> {
    let start = i128::from(timestamp.seconds_since_unix_epoch()) * 1_000_000_000
        + i128::from(timestamp.nanosecond());
    let value = start
        .checked_add(i128::from(nanoseconds))
        .ok_or_else(|| IntegratorError::NumericalInvariant("timestamp overflow".into()))?;
    let seconds = i64::try_from(value.div_euclid(1_000_000_000))
        .map_err(|_| IntegratorError::NumericalInvariant("timestamp overflow".into()))?;
    let nanos = u32::try_from(value.rem_euclid(1_000_000_000))
        .map_err(|_| IntegratorError::NumericalInvariant("timestamp overflow".into()))?;
    Timestamp::new(seconds, nanos)
        .map_err(|_| IntegratorError::NumericalInvariant("timestamp overflow".into()))
}

/// Particle-integration failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntegratorError {
    /// Integrator algorithm has not been implemented yet.
    NotImplemented,
    /// Required meteorology could not be prepared or queried.
    Meteorology(String),
    /// Input particle SoA is inconsistent.
    InvalidParticleBatch,
    /// A batch-wide numerical invariant failed outside normal per-particle handling.
    NumericalInvariant(String),
}

impl IntegratorError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented => "integrator.not_implemented",
            Self::Meteorology(_) => "integrator.meteorology",
            Self::InvalidParticleBatch => "integrator.invalid_particle_batch",
            Self::NumericalInvariant(_) => "integrator.numerical_invariant",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use trajecta_case::model::population::{PopulationId, ReleaseEventId};

    use super::*;
    use crate::particle::{ParticleId, ParticleOrigin, SubstanceMassStore};
    use crate::science::RK2_MINIMUM_CONVERGENCE_ORDER;

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::new(seconds, 0).unwrap()
    }

    fn particle(id: u64, longitude: f64, latitude: f64, height: f64) -> ParticleState {
        ParticleState {
            id: ParticleId(id),
            population_id: PopulationId("release".into()),
            origin: ParticleOrigin::Release {
                event_id: ReleaseEventId("event".into()),
            },
            birth_time: Timestamp::UNIX_EPOCH,
            longitude_degrees: longitude,
            latitude_degrees: latitude,
            height_asl_m: height,
            integration_offset_ns: 0,
            elapsed_age_ns: 0,
            dry_air_mass_kg: 0.0,
            mass_kg: BTreeMap::new(),
            sensitivity_weight: None,
            status: ParticleStatus::Alive,
        }
    }

    fn batch(states: &[ParticleState]) -> ParticleBatch {
        ParticleBatch {
            id: states.iter().map(|state| state.id).collect(),
            population_id: states
                .iter()
                .map(|state| state.population_id.clone())
                .collect(),
            origin: states.iter().map(|state| state.origin.clone()).collect(),
            birth_time: states.iter().map(|state| state.birth_time).collect(),
            longitude_degrees: states.iter().map(|state| state.longitude_degrees).collect(),
            latitude_degrees: states.iter().map(|state| state.latitude_degrees).collect(),
            height_asl_m: states.iter().map(|state| state.height_asl_m).collect(),
            integration_offset_ns: states
                .iter()
                .map(|state| state.integration_offset_ns)
                .collect(),
            elapsed_age_ns: states.iter().map(|state| state.elapsed_age_ns).collect(),
            dry_air_mass_kg: states.iter().map(|state| state.dry_air_mass_kg).collect(),
            sensitivity_weight: states
                .iter()
                .map(|state| state.sensitivity_weight)
                .collect(),
            status: states.iter().map(|state| state.status.clone()).collect(),
            mass: SubstanceMassStore::default(),
        }
    }

    fn ok(eastward: f64, northward: f64, vertical: f64) -> VelocitySample {
        VelocitySample {
            status: SampleStatus::Ok,
            eastward_m_s: Some(eastward),
            northward_m_s: Some(northward),
            vertical_m_s: Some(vertical),
        }
    }

    #[test]
    fn zero_wind_and_signed_vertical_motion_use_two_exact_query_times() {
        for (step_ns, expected_height, expected_offset) in [
            (10_000_000_000, 1_020.0, 10_000_000_000),
            (-10_000_000_000, 980.0, -10_000_000_000),
        ] {
            let input_batch = batch(&[particle(1, 170.0, 80.0, 1_000.0)]);
            let mut calls = Vec::new();
            let result = advance_with_transport_query(
                IntegratorInput {
                    particles: &input_batch,
                    time: timestamp(100),
                    step: SignedDuration(step_ns),
                },
                |time, points| {
                    calls.push((time, points.to_vec()));
                    Ok(vec![ok(0.0, 0.0, 2.0); points.len()])
                },
            )
            .unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].0, timestamp(100));
            assert_eq!(calls[1].0, timestamp(100 + step_ns / 2_000_000_000));
            assert_eq!(
                calls[1].1[0].height_asl_m,
                0.5 * (1_000.0 + expected_height)
            );
            let state = result.particles.state(0).unwrap();
            assert!((state.longitude_degrees - 170.0).abs() < 1.0e-12);
            assert!((state.latitude_degrees - 80.0).abs() < 1.0e-12);
            assert!((state.height_asl_m - expected_height).abs() < 1.0e-12);
            assert_eq!(state.integration_offset_ns, expected_offset);
            assert_eq!(state.elapsed_age_ns, 10_000_000_000);
        }
    }

    #[test]
    fn solid_body_rotation_observes_second_order_convergence() {
        const ANGULAR_RATE: f64 = 1.0e-4;
        const TOTAL_SECONDS: i64 = 3_600;

        let integrate = |step_seconds: i64| {
            let mut particles = batch(&[particle(1, -20.0, 63.0, 2_000.0)]);
            let mut time = Timestamp::UNIX_EPOCH;
            for _ in 0..TOTAL_SECONDS / step_seconds {
                let result = advance_with_transport_query(
                    IntegratorInput {
                        particles: &particles,
                        time,
                        step: SignedDuration(step_seconds * 1_000_000_000),
                    },
                    |_query_time, points| {
                        Ok(points
                            .iter()
                            .map(|point| {
                                ok(
                                    ANGULAR_RATE
                                        * M4_CONSTANTS.earth_radius_m
                                        * point.latitude_degrees.to_radians().cos(),
                                    0.0,
                                    0.0,
                                )
                            })
                            .collect())
                    },
                )
                .unwrap();
                particles = result.particles;
                time = checked_add_timestamp(time, step_seconds * 1_000_000_000).unwrap();
            }
            let expected = -20.0 + (ANGULAR_RATE * TOTAL_SECONDS as f64).to_degrees();
            angular_distance_degrees(particles.longitude_degrees[0], expected)
        };

        let coarse = integrate(600);
        let fine = integrate(300);
        let order = (coarse / fine).ln() / 2.0_f64.ln();
        assert!(order >= RK2_MINIMUM_CONVERGENCE_ORDER, "order={order}");
    }

    #[test]
    fn constant_east_and_north_fields_follow_spherical_axes() {
        let run = |initial: ParticleState, eastward: f64, northward: f64| {
            let input_batch = batch(&[initial]);
            advance_with_transport_query(
                IntegratorInput {
                    particles: &input_batch,
                    time: Timestamp::UNIX_EPOCH,
                    step: SignedDuration(100_000_000_000),
                },
                |_time, points| Ok(vec![ok(eastward, northward, 0.0); points.len()]),
            )
            .unwrap()
            .particles
            .state(0)
            .unwrap()
        };

        let east = run(particle(1, 5.0, 0.0, 1_000.0), 100.0, 0.0);
        let expected_longitude = 5.0 + (100.0 * 100.0 / M4_CONSTANTS.earth_radius_m).to_degrees();
        assert!(angular_distance_degrees(east.longitude_degrees, expected_longitude) < 1.0e-6);
        assert!(east.latitude_degrees.abs() < 1.0e-10);

        let north = run(particle(1, 30.0, 10.0, 1_000.0), 0.0, 100.0);
        let expected_latitude = 10.0 + (100.0 * 100.0 / M4_CONSTANTS.earth_radius_m).to_degrees();
        assert!((north.latitude_degrees - expected_latitude).abs() < 1.0e-6);
        assert!(angular_distance_degrees(north.longitude_degrees, 30.0) < 1.0e-10);
    }

    #[test]
    fn forward_then_backward_rotation_is_consistent_across_dateline_and_high_latitude() {
        const ANGULAR_RATE: f64 = 8.0e-5;
        let initial = particle(1, 179.8, 82.0, 3_000.0);
        let mut particles = batch(std::slice::from_ref(&initial));
        let mut time = timestamp(10_000);
        for step in [300_i64, -300_i64] {
            particles = advance_with_transport_query(
                IntegratorInput {
                    particles: &particles,
                    time,
                    step: SignedDuration(step * 1_000_000_000),
                },
                |_query_time, points| {
                    Ok(points
                        .iter()
                        .map(|point| {
                            ok(
                                ANGULAR_RATE
                                    * M4_CONSTANTS.earth_radius_m
                                    * point.latitude_degrees.to_radians().cos(),
                                0.0,
                                0.0,
                            )
                        })
                        .collect())
                },
            )
            .unwrap()
            .particles;
            time = checked_add_timestamp(time, step * 1_000_000_000).unwrap();
        }
        let restored = particles.state(0).unwrap();
        assert!(
            angular_distance_degrees(restored.longitude_degrees, initial.longitude_degrees)
                < 1.0e-4
        );
        assert!((restored.latitude_degrees - initial.latitude_degrees).abs() < 1.0e-4);
        assert_eq!(restored.integration_offset_ns, 0);
        assert_eq!(restored.elapsed_age_ns, 600_000_000_000);
    }

    #[test]
    fn invalid_meteorology_is_particle_local_and_storage_order_independent() {
        let states = [
            particle(1, -10.0, 10.0, 1_000.0),
            particle(2, 10.0, -10.0, 1_000.0),
            particle(3, 20.0, 30.0, 1_000.0),
        ];
        let run = |states: &[ParticleState]| {
            let input_batch = batch(states);
            advance_with_transport_query(
                IntegratorInput {
                    particles: &input_batch,
                    time: Timestamp::UNIX_EPOCH,
                    step: SignedDuration(60_000_000_000),
                },
                |_time, points| {
                    Ok(points
                        .iter()
                        .map(|point| {
                            if point.id == ParticleId(2) {
                                VelocitySample {
                                    status: SampleStatus::NumericalFailure,
                                    eastward_m_s: None,
                                    northward_m_s: None,
                                    vertical_m_s: None,
                                }
                            } else {
                                ok(5.0, -2.0, 0.5)
                            }
                        })
                        .collect())
                },
            )
            .unwrap()
        };
        let ordered = run(&states);
        let permuted = run(&[states[2].clone(), states[0].clone(), states[1].clone()]);
        assert_eq!(ordered.abnormal_terminated_count, 1);
        assert_eq!(permuted.abnormal_terminated_count, 1);
        let by_id = |batch: &ParticleBatch| {
            (0..batch.len().unwrap())
                .map(|index| {
                    let state = batch.state(index).unwrap();
                    (state.id, state)
                })
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(by_id(&ordered.particles), by_id(&permuted.particles));
        assert!(matches!(
            by_id(&ordered.particles)[&ParticleId(2)].status,
            ParticleStatus::Terminated {
                reason: TerminationReason::InvalidMeteorology
            }
        ));
    }

    fn angular_distance_degrees(left: f64, right: f64) -> f64 {
        ((left - right + 180.0).rem_euclid(360.0) - 180.0).abs()
    }
}
