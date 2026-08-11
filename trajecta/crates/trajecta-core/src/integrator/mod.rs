//! # Contract: particle integration
//!
//! The v0 integrator is a spherical midpoint RK2 scheme. It queries wind at
//! the start and midpoint, advances longitude/latitude on a sphere, uses
//! geometric vertical velocity, supports signed time, and applies boundary
//! policies after proposing the full step.

use std::collections::BTreeMap;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::Timestamp;
use trajecta_met::performance::{PerformanceScope, PerformanceStage};
use trajecta_met::query::engine::{BatchWorkspace, ExecutionContext, MetEngine};
use trajecta_met::query::metrics::{QueryOrigin, QueryOriginScope};
use trajecta_met::query::output::SampleStatus;
use trajecta_met::query::request::{QueryBatch, QueryPointArrays, TransportPlan, VerticalQuery};
use trajecta_met::vertical::VerticalBounds;

use crate::clock::{SignedDuration, add_timestamp, signed_duration_between};
#[cfg(test)]
use crate::particle::ParticleState;
use crate::particle::{ParticleBatch, ParticleStatus, ParticleTermination, TerminationReason};
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

/// Particle batch whose rows may start at different exact physical instants
/// but share one macro-step endpoint.
#[derive(Clone, Copy, Debug)]
pub struct TimedIntegratorInput<'a> {
    /// Current particles in stable runner order.
    pub particles: &'a ParticleBatch,
    /// One exact integration start per particle row.
    pub start_times: &'a [Timestamp],
    /// Common physical endpoint of the runner macro step.
    pub end_time: Timestamp,
}

/// Mutable meteorology access and immutable execution policy for one step.
pub struct IntegratorContext<'a> {
    /// Meteorology engine used only through explicit prepare/query phases.
    pub meteorology: &'a mut MetEngine,
    /// Precompiled transport query plan.
    pub query_plan: &'a TransportPlan,
    /// Caller-owned execution context.
    pub execution: &'a dyn ExecutionContext,
    /// Explicit meteorology domain selected by the runner.
    pub domain: Option<&'a DomainId>,
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

    /// Advances rows with distinct exact start times to one common endpoint.
    ///
    /// The default implementation groups equal `(start, step)` rows and never
    /// re-advances a row because another cohort was born. Integrators may
    /// override this to execute true heterogeneous-time vector batches.
    fn advance_timed(
        &self,
        input: TimedIntegratorInput<'_>,
        context: &mut IntegratorContext<'_>,
    ) -> Result<StepResult, IntegratorError> {
        let count = input
            .particles
            .validate()
            .map_err(|_| IntegratorError::InvalidParticleBatch)?;
        if input.start_times.len() != count {
            return Err(IntegratorError::InvalidParticleBatch);
        }
        let mut groups = BTreeMap::<(Timestamp, i64), Vec<usize>>::new();
        for (index, start) in input.start_times.iter().copied().enumerate() {
            let step = signed_duration_between(start, input.end_time)
                .map_err(|_| IntegratorError::NumericalInvariant("timestamp overflow".into()))?;
            groups.entry((start, step.0)).or_default().push(index);
        }
        let mut particles = input.particles.clone();
        let mut abnormal_terminated_count = 0_usize;
        for ((time, step_ns), indices) in groups {
            let subset = input
                .particles
                .select_indices(&indices)
                .map_err(|_| IntegratorError::InvalidParticleBatch)?;
            let result = self.advance(
                IntegratorInput {
                    particles: &subset,
                    time,
                    step: SignedDuration(step_ns),
                },
                context,
            )?;
            abnormal_terminated_count = abnormal_terminated_count
                .checked_add(result.abnormal_terminated_count)
                .ok_or_else(|| IntegratorError::NumericalInvariant("termination count".into()))?;
            for (local, global) in indices.into_iter().enumerate() {
                let state = result
                    .particles
                    .state(local)
                    .map_err(|_| IntegratorError::InvalidParticleBatch)?;
                particles
                    .set_state(global, state)
                    .map_err(|_| IntegratorError::InvalidParticleBatch)?;
            }
        }
        Ok(StepResult {
            particles,
            abnormal_terminated_count,
        })
    }
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
        let count = input
            .particles
            .len()
            .map_err(|_| IntegratorError::InvalidParticleBatch)?;
        let end_time = add_timestamp(input.time, input.step)
            .map_err(|_| IntegratorError::NumericalInvariant("timestamp overflow".into()))?;
        let start_times = vec![input.time; count];
        self.advance_timed(
            TimedIntegratorInput {
                particles: input.particles,
                start_times: &start_times,
                end_time,
            },
            context,
        )
    }

    fn advance_timed(
        &self,
        input: TimedIntegratorInput<'_>,
        context: &mut IntegratorContext<'_>,
    ) -> Result<StepResult, IntegratorError> {
        let end_time = input.end_time;
        advance_with_timed_transport_query(input, |times, particles| {
            query_transport_at_times(context, times, particles, end_time)
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct VelocitySample {
    status: SampleStatus,
    eastward_m_s: Option<f64>,
    northward_m_s: Option<f64>,
    vertical_m_s: Option<f64>,
    boundary_bridge: bool,
}

#[derive(Clone, Copy, Debug)]
struct MotionState {
    longitude_degrees: f64,
    latitude_degrees: f64,
    height_asl_m: f64,
    integration_offset_ns: i64,
    elapsed_age_ns: u64,
}

impl MotionState {
    fn at(particles: &ParticleBatch, index: usize) -> Self {
        Self {
            longitude_degrees: particles.longitude_degrees[index],
            latitude_degrees: particles.latitude_degrees[index],
            height_asl_m: particles.height_asl_m[index],
            integration_offset_ns: particles.integration_offset_ns[index],
            elapsed_age_ns: particles.elapsed_age_ns[index],
        }
    }

    fn advanced(
        self,
        longitude_degrees: f64,
        latitude_degrees: f64,
        height_asl_m: f64,
        step: SignedDuration,
    ) -> Option<Self> {
        if !longitude_degrees.is_finite()
            || !latitude_degrees.is_finite()
            || !height_asl_m.is_finite()
        {
            return None;
        }
        Some(Self {
            longitude_degrees,
            latitude_degrees,
            height_asl_m,
            integration_offset_ns: self
                .integration_offset_ns
                .checked_add(step.as_nanoseconds())?,
            elapsed_age_ns: self
                .elapsed_age_ns
                .checked_add(step.as_nanoseconds().unsigned_abs())?,
        })
    }
}

impl VelocitySample {
    fn velocity(self) -> Option<[f64; 3]> {
        if self.status != SampleStatus::Ok {
            return None;
        }
        Some([self.eastward_m_s?, self.northward_m_s?, self.vertical_m_s?])
    }

    fn with_perturbation(mut self, perturbation_m_s: [f64; 3]) -> Self {
        if perturbation_m_s == [0.0; 3] {
            return self;
        }
        if self.status == SampleStatus::Ok {
            self.eastward_m_s = self.eastward_m_s.map(|value| value + perturbation_m_s[0]);
            self.northward_m_s = self.northward_m_s.map(|value| value + perturbation_m_s[1]);
            self.vertical_m_s = self.vertical_m_s.map(|value| value + perturbation_m_s[2]);
        }
        self
    }
}

fn query_transport(
    context: &mut IntegratorContext<'_>,
    time: Timestamp,
    particles: &[MotionState],
) -> Result<Vec<VelocitySample>, IntegratorError> {
    let _query_origin = QueryOriginScope::enter(QueryOrigin::Integrator);
    if particles.is_empty() {
        return Ok(Vec::new());
    }
    let domain = context.domain.ok_or_else(|| {
        IntegratorError::Meteorology("integrator requires an explicit meteorology domain".into())
    })?;
    let window = context
        .meteorology
        .prepare_for_domain(time, domain)
        .map_err(|error| IntegratorError::Meteorology(format!("{error:?}")))?;
    let points = QueryPointArrays {
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
    };
    let mut workspace = BatchWorkspace::default();
    let prepared = window
        .prepare_transport_batch(
            context.query_plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points,
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
                boundary_bridge: is_boundary_bridge_status(
                    row.status(),
                    row.bounds(),
                    particles[index].height_asl_m,
                ),
            })
        })
        .collect()
}

fn is_boundary_bridge_status(
    status: SampleStatus,
    bounds: Option<VerticalBounds>,
    height_asl_m: f64,
) -> bool {
    match status {
        SampleStatus::OutOfDomain => true,
        SampleStatus::BelowGround => {
            bounds.is_some_and(|bounds| height_asl_m <= bounds.terrain_asl_m())
        }
        SampleStatus::SurfaceLayerUndefined => {
            bounds.is_some_and(|bounds| height_asl_m < bounds.minimum_transport_asl_m())
        }
        // Between-frame sampling can report a bracketing endpoint top exit
        // even while the target-time blended top still contains the RK2
        // predictor. The typed status plus bounds is sufficient to defer the
        // proposal to the continuous boundary path; rechecking against
        // different target-time bounds incorrectly converts it to invalid met.
        SampleStatus::AboveAvailableTop | SampleStatus::AboveModelTop => bounds.is_some(),
        _ => false,
    }
}

fn query_transport_at_times(
    context: &mut IntegratorContext<'_>,
    times: &[Timestamp],
    particles: &[MotionState],
    end_time: Timestamp,
) -> Result<Vec<VelocitySample>, IntegratorError> {
    if times.len() != particles.len() {
        return Err(IntegratorError::InvalidParticleBatch);
    }
    if let Some(time) = times.first().copied() {
        if times.iter().all(|candidate| *candidate == time) {
            return query_transport(context, time, particles);
        }
    }
    let groups = ordered_transport_time_groups(times, end_time)?;
    let mut output = vec![None; particles.len()];
    for (time, indices) in groups {
        let states = indices
            .iter()
            .map(|index| particles[*index])
            .collect::<Vec<_>>();
        let samples = query_transport(context, time, &states)?;
        if samples.len() != indices.len() {
            return Err(IntegratorError::NumericalInvariant(
                "transport output returned the wrong row count".into(),
            ));
        }
        for (index, sample) in indices.into_iter().zip(samples) {
            output[index] = Some(sample);
        }
    }
    output
        .into_iter()
        .map(|sample| {
            sample.ok_or_else(|| {
                IntegratorError::NumericalInvariant("missing timed transport row".into())
            })
        })
        .collect()
}

fn ordered_transport_time_groups(
    times: &[Timestamp],
    end_time: Timestamp,
) -> Result<Vec<(Timestamp, Vec<usize>)>, IntegratorError> {
    let mut direction_sign = 0_i8;
    let mut groups = BTreeMap::<Timestamp, Vec<usize>>::new();
    for (index, time) in times.iter().copied().enumerate() {
        let sign = if time < end_time {
            1
        } else if time > end_time {
            -1
        } else {
            0
        };
        if sign != 0 {
            if direction_sign != 0 && direction_sign != sign {
                return Err(IntegratorError::NumericalInvariant(
                    "timed transport groups mix integration directions".into(),
                ));
            }
            direction_sign = sign;
        }
        groups.entry(time).or_default().push(index);
    }
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    if direction_sign < 0 {
        groups.reverse();
    }
    Ok(groups)
}

#[cfg(test)]
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
    let mut midpoint_start_velocities = Vec::new();
    let mut abnormal_terminated_count = 0;
    for ((batch_index, start), sample) in active_indices
        .iter()
        .copied()
        .zip(&active_states)
        .zip(start_samples)
    {
        if sample.boundary_bridge {
            let Some(advanced) = complete_advanced_state(
                start.clone(),
                start.longitude_degrees,
                start.latitude_degrees,
                start.height_asl_m,
                input.step,
            ) else {
                terminate_particle(
                    &mut proposal,
                    batch_index,
                    TerminationReason::NumericalFailure,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            proposal
                .set_state(batch_index, advanced)
                .map_err(|_| IntegratorError::InvalidParticleBatch)?;
            continue;
        }
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
        midpoint_start_velocities.push([eastward, northward, vertical]);
    }

    let midpoint_samples = query(midpoint_time, &midpoint_states)?;
    if midpoint_samples.len() != midpoint_states.len() {
        return Err(IntegratorError::NumericalInvariant(
            "midpoint transport query returned the wrong row count".into(),
        ));
    }
    for (((batch_index, midpoint), start_velocity), sample) in midpoint_indices
        .into_iter()
        .zip(midpoint_states)
        .zip(midpoint_start_velocities)
        .zip(midpoint_samples)
    {
        let start = input
            .particles
            .state(batch_index)
            .map_err(|_| IntegratorError::InvalidParticleBatch)?;
        if sample.boundary_bridge {
            let Some((longitude, latitude)) = spherical_displacement(
                start.longitude_degrees,
                start.latitude_degrees,
                start_velocity[0],
                start_velocity[1],
                seconds,
            ) else {
                terminate_particle(
                    &mut proposal,
                    batch_index,
                    TerminationReason::NumericalFailure,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            let height = start.height_asl_m + seconds * start_velocity[2];
            let Some(advanced) =
                complete_advanced_state(start, longitude, latitude, height, input.step)
            else {
                terminate_particle(
                    &mut proposal,
                    batch_index,
                    TerminationReason::NumericalFailure,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            proposal
                .set_state(batch_index, advanced)
                .map_err(|_| IntegratorError::InvalidParticleBatch)?;
            continue;
        }
        let Some([eastward, northward, vertical]) = sample.velocity() else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::InvalidMeteorology,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        let Some((longitude, latitude)) = spherical_displacement_from_midpoint_velocity(
            start.longitude_degrees,
            start.latitude_degrees,
            midpoint.longitude_degrees,
            midpoint.latitude_degrees,
            eastward,
            northward,
            seconds,
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
        let Some(advanced) =
            complete_advanced_state(start, longitude, latitude, height, input.step)
        else {
            terminate_particle(
                &mut proposal,
                batch_index,
                TerminationReason::NumericalFailure,
            )?;
            abnormal_terminated_count += 1;
            continue;
        };
        proposal
            .set_state(batch_index, advanced)
            .map_err(|_| IntegratorError::InvalidParticleBatch)?;
    }

    Ok(StepResult {
        particles: proposal,
        abnormal_terminated_count,
    })
}

fn advance_with_timed_transport_query<F>(
    input: TimedIntegratorInput<'_>,
    mut query: F,
) -> Result<StepResult, IntegratorError>
where
    F: FnMut(&[Timestamp], &[MotionState]) -> Result<Vec<VelocitySample>, IntegratorError>,
{
    let count = {
        let _performance = PerformanceScope::enter(PerformanceStage::IntegratorInputValidate);
        input
            .particles
            .validate()
            .map_err(|_| IntegratorError::InvalidParticleBatch)?
    };
    if input.start_times.len() != count {
        return Err(IntegratorError::InvalidParticleBatch);
    }

    let mut proposal = {
        let _performance = PerformanceScope::enter(PerformanceStage::IntegratorProposalClone);
        input.particles.clone()
    };
    let mut active_indices = Vec::new();
    let mut active_states = Vec::new();
    let mut active_start_times = Vec::new();
    let mut active_steps = Vec::new();
    let mut active_seconds = Vec::new();
    {
        let _performance = PerformanceScope::enter(PerformanceStage::IntegratorActiveSetup);
        let mut direction_sign = 0_i8;
        for (index, start_time) in input.start_times.iter().copied().enumerate() {
            if input.particles.status[index] != ParticleStatus::Alive {
                continue;
            }
            let step = signed_duration_between(start_time, input.end_time)
                .map_err(|_| IntegratorError::NumericalInvariant("timestamp overflow".into()))?;
            if step == SignedDuration::ZERO {
                continue;
            }
            let sign = if step.0 < 0 { -1 } else { 1 };
            if direction_sign != 0 && direction_sign != sign {
                return Err(IntegratorError::NumericalInvariant(
                    "timed batch mixes integration directions".into(),
                ));
            }
            direction_sign = sign;
            let seconds = step.0 as f64 * 1.0e-9;
            if !seconds.is_finite() {
                return Err(IntegratorError::NumericalInvariant(
                    "signed step cannot be represented in seconds".into(),
                ));
            }
            active_indices.push(index);
            active_states.push(MotionState::at(input.particles, index));
            active_start_times.push(start_time);
            active_steps.push(step);
            active_seconds.push(seconds);
        }
    }
    if active_indices.is_empty() {
        return Ok(StepResult {
            particles: proposal,
            abnormal_terminated_count: 0,
        });
    }

    let start_samples = {
        let _performance = PerformanceScope::enter(PerformanceStage::IntegratorStartQuery);
        query(&active_start_times, &active_states)?
    };
    if start_samples.len() != active_states.len() {
        return Err(IntegratorError::NumericalInvariant(
            "start transport query returned the wrong row count".into(),
        ));
    }

    let mut midpoint_states = Vec::new();
    let mut midpoint_indices = Vec::new();
    let mut midpoint_times = Vec::new();
    let mut midpoint_steps = Vec::new();
    let mut midpoint_seconds = Vec::new();
    let mut midpoint_start_velocities = Vec::new();
    let mut abnormal_terminated_count = 0;
    {
        let _performance = PerformanceScope::enter(PerformanceStage::IntegratorMidpointBuild);
        for (((((batch_index, start), start_time), step), seconds), sample) in active_indices
            .iter()
            .copied()
            .zip(&active_states)
            .zip(active_start_times.iter().copied())
            .zip(active_steps.iter().copied())
            .zip(active_seconds.iter().copied())
            .zip(start_samples)
        {
            let sample = sample.with_perturbation(
                input
                    .particles
                    .random_motion_velocity(batch_index)
                    .map_err(|_| IntegratorError::InvalidParticleBatch)?,
            );
            if sample.boundary_bridge {
                let Some(advanced) = start.advanced(
                    start.longitude_degrees,
                    start.latitude_degrees,
                    start.height_asl_m,
                    step,
                ) else {
                    terminate_particle_at(
                        &mut proposal,
                        batch_index,
                        TerminationReason::NumericalFailure,
                        start_time,
                    )?;
                    abnormal_terminated_count += 1;
                    continue;
                };
                apply_motion(&mut proposal, batch_index, advanced);
                continue;
            }
            let Some([eastward, northward, vertical]) = sample.velocity() else {
                terminate_particle_at(
                    &mut proposal,
                    batch_index,
                    TerminationReason::InvalidMeteorology,
                    start_time,
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
                terminate_particle_at(
                    &mut proposal,
                    batch_index,
                    TerminationReason::NumericalFailure,
                    start_time,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            let height = start.height_asl_m + 0.5 * seconds * vertical;
            if !height.is_finite() {
                terminate_particle_at(
                    &mut proposal,
                    batch_index,
                    TerminationReason::NumericalFailure,
                    start_time,
                )?;
                abnormal_terminated_count += 1;
                continue;
            }
            let midpoint_step = SignedDuration(step.0 / 2);
            let midpoint_time = checked_add_timestamp(start_time, midpoint_step.0)?;
            let Some(midpoint) = start.advanced(longitude, latitude, height, midpoint_step) else {
                terminate_particle_at(
                    &mut proposal,
                    batch_index,
                    TerminationReason::NumericalFailure,
                    start_time,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            midpoint_states.push(midpoint);
            midpoint_indices.push(batch_index);
            midpoint_times.push(midpoint_time);
            midpoint_steps.push(step);
            midpoint_seconds.push(seconds);
            midpoint_start_velocities.push([eastward, northward, vertical]);
        }
    }

    let midpoint_samples = {
        let _performance = PerformanceScope::enter(PerformanceStage::IntegratorMidpointQuery);
        query(&midpoint_times, &midpoint_states)?
    };
    if midpoint_samples.len() != midpoint_states.len() {
        return Err(IntegratorError::NumericalInvariant(
            "midpoint transport query returned the wrong row count".into(),
        ));
    }
    {
        let _performance = PerformanceScope::enter(PerformanceStage::IntegratorApply);
        for (
            (((((batch_index, midpoint), midpoint_time), step), seconds), start_velocity),
            sample,
        ) in midpoint_indices
            .into_iter()
            .zip(midpoint_states)
            .zip(midpoint_times)
            .zip(midpoint_steps)
            .zip(midpoint_seconds)
            .zip(midpoint_start_velocities)
            .zip(midpoint_samples)
        {
            let sample = sample.with_perturbation(
                input
                    .particles
                    .random_motion_velocity(batch_index)
                    .map_err(|_| IntegratorError::InvalidParticleBatch)?,
            );
            let start = MotionState::at(input.particles, batch_index);
            if sample.boundary_bridge {
                let Some((longitude, latitude)) = spherical_displacement(
                    start.longitude_degrees,
                    start.latitude_degrees,
                    start_velocity[0],
                    start_velocity[1],
                    seconds,
                ) else {
                    terminate_particle_motion_at(
                        &mut proposal,
                        batch_index,
                        midpoint,
                        TerminationReason::NumericalFailure,
                        midpoint_time,
                    )?;
                    abnormal_terminated_count += 1;
                    continue;
                };
                let height = start.height_asl_m + seconds * start_velocity[2];
                let Some(advanced) = start.advanced(longitude, latitude, height, step) else {
                    terminate_particle_motion_at(
                        &mut proposal,
                        batch_index,
                        midpoint,
                        TerminationReason::NumericalFailure,
                        midpoint_time,
                    )?;
                    abnormal_terminated_count += 1;
                    continue;
                };
                apply_motion(&mut proposal, batch_index, advanced);
                continue;
            }
            let Some([eastward, northward, vertical]) = sample.velocity() else {
                terminate_particle_motion_at(
                    &mut proposal,
                    batch_index,
                    midpoint,
                    TerminationReason::InvalidMeteorology,
                    midpoint_time,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            let Some((longitude, latitude)) = spherical_displacement_from_midpoint_velocity(
                start.longitude_degrees,
                start.latitude_degrees,
                midpoint.longitude_degrees,
                midpoint.latitude_degrees,
                eastward,
                northward,
                seconds,
            ) else {
                terminate_particle_motion_at(
                    &mut proposal,
                    batch_index,
                    midpoint,
                    TerminationReason::NumericalFailure,
                    midpoint_time,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            let height = start.height_asl_m + seconds * vertical;
            let Some(advanced) = start.advanced(longitude, latitude, height, step) else {
                terminate_particle_motion_at(
                    &mut proposal,
                    batch_index,
                    midpoint,
                    TerminationReason::NumericalFailure,
                    midpoint_time,
                )?;
                abnormal_terminated_count += 1;
                continue;
            };
            apply_motion(&mut proposal, batch_index, advanced);
        }
    }

    Ok(StepResult {
        particles: proposal,
        abnormal_terminated_count,
    })
}

#[cfg(test)]
fn complete_advanced_state(
    mut start: ParticleState,
    longitude_degrees: f64,
    latitude_degrees: f64,
    height_asl_m: f64,
    step: SignedDuration,
) -> Option<ParticleState> {
    if !longitude_degrees.is_finite() || !latitude_degrees.is_finite() || !height_asl_m.is_finite()
    {
        return None;
    }
    start.integration_offset_ns = start
        .integration_offset_ns
        .checked_add(step.as_nanoseconds())?;
    start.elapsed_age_ns = start
        .elapsed_age_ns
        .checked_add(step.as_nanoseconds().unsigned_abs())?;
    start.longitude_degrees = longitude_degrees;
    start.latitude_degrees = latitude_degrees;
    start.height_asl_m = height_asl_m;
    Some(start)
}

#[cfg(test)]
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

fn terminate_particle_at(
    particles: &mut ParticleBatch,
    index: usize,
    reason: TerminationReason,
    time: Timestamp,
) -> Result<(), IntegratorError> {
    let len = particles
        .len()
        .map_err(|_| IntegratorError::InvalidParticleBatch)?;
    if index >= len {
        return Err(IntegratorError::InvalidParticleBatch);
    }
    particles.status[index] = ParticleStatus::Terminated { reason };
    particles.termination[index] = Some(ParticleTermination {
        time,
        intersection_fraction: None,
    });
    Ok(())
}

fn terminate_particle_motion_at(
    particles: &mut ParticleBatch,
    index: usize,
    motion: MotionState,
    reason: TerminationReason,
    time: Timestamp,
) -> Result<(), IntegratorError> {
    apply_motion(particles, index, motion);
    terminate_particle_at(particles, index, reason, time)
}

fn apply_motion(particles: &mut ParticleBatch, index: usize, motion: MotionState) {
    particles.longitude_degrees[index] = motion.longitude_degrees;
    particles.latitude_degrees[index] = motion.latitude_degrees;
    particles.height_asl_m[index] = motion.height_asl_m;
    particles.integration_offset_ns[index] = motion.integration_offset_ns;
    particles.elapsed_age_ns[index] = motion.elapsed_age_ns;
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
    start_longitude_degrees: f64,
    start_latitude_degrees: f64,
    midpoint_longitude_degrees: f64,
    midpoint_latitude_degrees: f64,
    eastward_m_s: f64,
    northward_m_s: f64,
    seconds: f64,
) -> Option<(f64, f64)> {
    let position = unit_position(start_longitude_degrees, start_latitude_degrees)?;
    let velocity = tangent_velocity(
        midpoint_longitude_degrees,
        midpoint_latitude_degrees,
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
            adjoint_weight: BTreeMap::new(),
            status: ParticleStatus::Alive,
            termination: None,
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
            status: states.iter().map(|state| state.status.clone()).collect(),
            termination: states
                .iter()
                .map(|state| state.termination.clone())
                .collect(),
            mass: SubstanceMassStore::default(),
            adjoint: Default::default(),
            motion: Default::default(),
        }
    }

    fn ok(eastward: f64, northward: f64, vertical: f64) -> VelocitySample {
        VelocitySample {
            status: SampleStatus::Ok,
            eastward_m_s: Some(eastward),
            northward_m_s: Some(northward),
            vertical_m_s: Some(vertical),
            boundary_bridge: false,
        }
    }

    #[test]
    fn transport_time_groups_follow_forward_direction_and_preserve_row_order() {
        let groups = ordered_transport_time_groups(
            &[timestamp(7), timestamp(5), timestamp(7), timestamp(6)],
            timestamp(10),
        )
        .unwrap();

        assert_eq!(
            groups,
            vec![
                (timestamp(5), vec![1]),
                (timestamp(6), vec![3]),
                (timestamp(7), vec![0, 2]),
            ]
        );
    }

    #[test]
    fn transport_time_groups_follow_backward_direction_and_preserve_row_order() {
        let groups = ordered_transport_time_groups(
            &[timestamp(13), timestamp(15), timestamp(13), timestamp(14)],
            timestamp(10),
        )
        .unwrap();

        assert_eq!(
            groups,
            vec![
                (timestamp(15), vec![1]),
                (timestamp(14), vec![3]),
                (timestamp(13), vec![0, 2]),
            ]
        );
    }

    #[test]
    fn transport_time_groups_reject_mixed_directions() {
        assert!(matches!(
            ordered_transport_time_groups(&[timestamp(9), timestamp(11)], timestamp(10)),
            Err(IntegratorError::NumericalInvariant(message))
                if message == "timed transport groups mix integration directions"
        ));
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
    fn timed_cohorts_match_independent_exact_time_integrations_in_both_directions() {
        for (starts, end) in [
            (
                [
                    Timestamp::UNIX_EPOCH,
                    Timestamp::new(0, 250_000_000).unwrap(),
                ],
                Timestamp::new(2, 0).unwrap(),
            ),
            (
                [
                    Timestamp::new(2, 0).unwrap(),
                    Timestamp::new(1, 750_000_000).unwrap(),
                ],
                Timestamp::UNIX_EPOCH,
            ),
        ] {
            let mut states = [
                particle(1, -20.0, 30.0, 1_000.0),
                particle(2, 40.0, -10.0, 2_000.0),
            ];
            states[0].birth_time = starts[0];
            states[1].birth_time = starts[1];
            let input_batch = batch(&states);
            let velocity = |time: Timestamp| {
                let seconds =
                    time.seconds_since_unix_epoch() as f64 + f64::from(time.nanosecond()) * 1.0e-9;
                ok(3.0 + seconds, -2.0 + 0.5 * seconds, 0.25 + seconds)
            };
            let timed = advance_with_timed_transport_query(
                TimedIntegratorInput {
                    particles: &input_batch,
                    start_times: &starts,
                    end_time: end,
                },
                |times, points| {
                    assert_eq!(times.len(), points.len());
                    Ok(times.iter().copied().map(velocity).collect())
                },
            )
            .unwrap();

            for index in 0..states.len() {
                let step = signed_duration_between(starts[index], end).unwrap();
                let independent = advance_with_transport_query(
                    IntegratorInput {
                        particles: &batch(&[states[index].clone()]),
                        time: starts[index],
                        step,
                    },
                    |time, points| Ok(vec![velocity(time); points.len()]),
                )
                .unwrap();
                assert_eq!(
                    timed.particles.state(index).unwrap(),
                    independent.particles.state(0).unwrap()
                );
            }
        }
    }

    #[test]
    fn midpoint_failure_terminates_at_the_matching_position_time_age_and_offset() {
        for (start, end, expected_offset, expected_height) in [
            (
                Timestamp::new(10, 0).unwrap(),
                Timestamp::new(20, 0).unwrap(),
                5_000_000_000,
                1_010.0,
            ),
            (
                Timestamp::new(20, 0).unwrap(),
                Timestamp::new(10, 0).unwrap(),
                -5_000_000_000,
                990.0,
            ),
        ] {
            let mut initial = particle(1, 30.0, 45.0, 1_000.0);
            initial.birth_time = start;
            let input_batch = batch(&[initial]);
            let mut call = 0_u8;
            let result = advance_with_timed_transport_query(
                TimedIntegratorInput {
                    particles: &input_batch,
                    start_times: &[start],
                    end_time: end,
                },
                |_times, points| {
                    call += 1;
                    if call == 1 {
                        Ok(vec![ok(0.0, 0.0, 2.0); points.len()])
                    } else {
                        Ok(vec![
                            VelocitySample {
                                status: SampleStatus::NumericalFailure,
                                eastward_m_s: None,
                                northward_m_s: None,
                                vertical_m_s: None,
                                boundary_bridge: false,
                            };
                            points.len()
                        ])
                    }
                },
            )
            .unwrap();
            assert_eq!(result.abnormal_terminated_count, 1);
            let terminated = result.particles.state(0).unwrap();
            assert_eq!(terminated.integration_offset_ns, expected_offset);
            assert_eq!(terminated.elapsed_age_ns, 5_000_000_000);
            assert_eq!(terminated.height_asl_m, expected_height);
            assert_eq!(
                terminated.status,
                ParticleStatus::Terminated {
                    reason: TerminationReason::InvalidMeteorology
                }
            );
            assert_eq!(
                terminated.termination,
                Some(ParticleTermination {
                    time: Timestamp::new(15, 0).unwrap(),
                    intersection_fraction: None,
                })
            );
        }
    }

    #[test]
    fn typed_start_boundary_bridge_preserves_an_alive_stationary_proposal() {
        let start_time = Timestamp::new(10, 0).unwrap();
        let end_time = Timestamp::new(20, 0).unwrap();
        let mut initial = particle(1, 30.0, 45.0, 1_000.0);
        initial.birth_time = start_time;
        let input_batch = batch(&[initial.clone()]);
        let mut call = 0_u8;
        let result = advance_with_timed_transport_query(
            TimedIntegratorInput {
                particles: &input_batch,
                start_times: &[start_time],
                end_time,
            },
            |_times, points| {
                call += 1;
                if call == 1 {
                    Ok(vec![VelocitySample {
                        status: SampleStatus::AboveAvailableTop,
                        eastward_m_s: None,
                        northward_m_s: None,
                        vertical_m_s: None,
                        boundary_bridge: true,
                    }])
                } else {
                    assert!(points.is_empty());
                    Ok(Vec::new())
                }
            },
        )
        .unwrap();
        assert_eq!(result.abnormal_terminated_count, 0);
        let advanced = result.particles.state(0).unwrap();
        assert_eq!(advanced.status, ParticleStatus::Alive);
        assert_eq!(advanced.longitude_degrees, initial.longitude_degrees);
        assert_eq!(advanced.latitude_degrees, initial.latitude_degrees);
        assert_eq!(advanced.height_asl_m, initial.height_asl_m);
        assert_eq!(advanced.integration_offset_ns, 10_000_000_000);
        assert_eq!(advanced.elapsed_age_ns, 10_000_000_000);
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
                                    boundary_bridge: false,
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

    #[test]
    fn midpoint_boundary_exit_is_deferred_to_continuous_boundary_policy() {
        for status in [
            SampleStatus::OutOfDomain,
            SampleStatus::BelowGround,
            SampleStatus::SurfaceLayerUndefined,
            SampleStatus::AboveAvailableTop,
            SampleStatus::AboveModelTop,
        ] {
            let input_batch = batch(&[particle(1, 0.0, 0.0, 1_000.0)]);
            let mut call = 0_u8;
            let result = advance_with_transport_query(
                IntegratorInput {
                    particles: &input_batch,
                    time: Timestamp::UNIX_EPOCH,
                    step: SignedDuration(10_000_000_000),
                },
                |_time, points| {
                    call += 1;
                    if call == 1 {
                        Ok(vec![ok(100.0, 0.0, 0.0); points.len()])
                    } else {
                        Ok(vec![
                            VelocitySample {
                                status,
                                eastward_m_s: None,
                                northward_m_s: None,
                                vertical_m_s: None,
                                boundary_bridge: true,
                            };
                            points.len()
                        ])
                    }
                },
            )
            .unwrap();
            assert_eq!(result.abnormal_terminated_count, 0, "status={status:?}");
            let proposed = result.particles.state(0).unwrap();
            assert_eq!(proposed.status, ParticleStatus::Alive, "status={status:?}");
            assert!(proposed.longitude_degrees > 0.0, "status={status:?}");
            assert_eq!(proposed.integration_offset_ns, 10_000_000_000);
            assert_eq!(proposed.elapsed_age_ns, 10_000_000_000);
        }
    }

    #[test]
    fn bracketing_endpoint_top_exit_is_a_boundary_bridge_inside_target_top() {
        let bounds = VerticalBounds::new(0.0, 1.0, 1.0, 100.0, Some(100.0), 10.0, 1_000.0).unwrap();
        for status in [SampleStatus::AboveAvailableTop, SampleStatus::AboveModelTop] {
            assert!(is_boundary_bridge_status(status, Some(bounds), 99.0));
        }
        assert!(!is_boundary_bridge_status(
            SampleStatus::NumericalFailure,
            Some(bounds),
            99.0
        ));
    }

    fn angular_distance_degrees(left: f64, right: f64) -> f64 {
        ((left - right + 180.0).rem_euclid(360.0) - 180.0).abs()
    }
}
