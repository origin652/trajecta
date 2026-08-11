//! # Contract: complete simulation orchestration
//!
//! The runner owns the clock, particles, meteorology engine, population,
//! integrator, boundaries, and products. Its main loop respects every event
//! boundary and keeps all strategy calls in the declared lifecycle order.

mod builder;
pub use builder::{
    RunnerAttemptIdentity, RunnerBuildKnobs, build_runner, build_runner_for_attempt,
    build_runner_with_knobs, build_runner_with_manifest_store, build_runner_with_store_and_knobs,
    required_capabilities_for_population, run_directory_path,
};

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use trajecta_case::document::{ResolvedCase, ResolvedRunProfile};
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::{Direction, Timestamp};
use trajecta_met::performance::{PerformanceBatchScope, PerformanceScope, PerformanceStage};
use trajecta_met::query::cache::CacheMetrics;
use trajecta_met::query::engine::{BatchWorkspace, EngineError, ExecutionContext, MetEngine};
use trajecta_met::query::metrics::{QueryOrigin, QueryOriginScope};
use trajecta_met::query::output::QueryOutput;
use trajecta_met::query::request::{
    QueryBatch, QueryPlan, QueryPointArrays, TransportPlan, VerticalQuery,
};

use crate::boundary::{
    BoundaryContext, BoundaryDecision, BoundaryError, BoundaryPathSampler, BoundaryPolicy,
};
use crate::clock::{
    ClockError, SignedDuration, SimulationClock, StepBoundary, StepPlanner, add_timestamp,
    scale_duration_by_fraction, signed_duration_between,
};
use crate::integrator::{IntegratorContext, IntegratorModel, TimedIntegratorInput};
use crate::manifest::{RunFailure, RunLifecycleStatus, RunManifest, TerminationSummary};
use crate::output::{OutputProduct, OutputScheduler};
use crate::particle::{
    ParticleBatch, ParticleState, ParticleStatus, ParticleTermination, TerminationClass,
};
use crate::physics::{
    PhysicsError, PhysicsPipeline, active_indices_for_substep, clamp_start_times,
};
use crate::population::{PopulationContext, PopulationError, PopulationState, PopulationStrategy};

/// Mutable in-memory simulation state.
///
/// M4 does not support checkpoint/resume; this state is exposed for audit and
/// deterministic testing only.
#[derive(Clone, Debug, PartialEq)]
pub struct SimulationState {
    /// Current physical simulation clock.
    pub clock: SimulationClock,
    /// Current particle SoA.
    pub particles: ParticleBatch,
    /// Persistent strategy state exposed for audit and deterministic tests.
    pub population_state: PopulationState,
    /// Next stable numerical-step index.
    pub numerical_step_index: u64,
    /// Next stable output event index.
    pub output_event_index: usize,
}

/// One output implementation plus its exact physical schedule.
pub struct ScheduledOutputProduct {
    /// Output implementation.
    pub product: Box<dyn OutputProduct>,
    /// Ordered unique interval sample times; endpoints are forced by runner
    /// lifecycle events independently of this list.
    pub scheduler: OutputScheduler,
    /// Optional exact-time meteorology contract required by this product.
    /// The runner executes it at the stored particle positions and never
    /// reuses RK2 start or midpoint values.
    pub meteorology_plan: Option<QueryPlan>,
}

/// Immutable request for constructing one physical boundary path sampler.
#[derive(Clone, Copy, Debug)]
pub struct BoundarySamplerRequest<'a> {
    /// Physical step start.
    pub start_time: Timestamp,
    /// Physical step end.
    pub end_time: Timestamp,
    /// State before integration.
    pub start: &'a ParticleState,
    /// Full integration proposal before boundaries.
    pub proposed: &'a ParticleState,
    /// Selected meteorological domain.
    pub domain: Option<&'a DomainId>,
}

/// Coordinate-only request used by the bulk boundary certificate.
#[derive(Clone, Copy, Debug)]
pub struct BoundaryCertificateRequest<'a> {
    /// Physical step start.
    pub start_time: Timestamp,
    /// Physical step end.
    pub end_time: Timestamp,
    /// Start longitude in degrees east.
    pub start_longitude_degrees: f64,
    /// Start latitude in degrees north.
    pub start_latitude_degrees: f64,
    /// Start geometric height above mean sea level in metres.
    pub start_height_asl_m: f64,
    /// Proposed longitude in degrees east.
    pub proposed_longitude_degrees: f64,
    /// Proposed latitude in degrees north.
    pub proposed_latitude_degrees: f64,
    /// Proposed geometric height above mean sea level in metres.
    pub proposed_height_asl_m: f64,
    /// Selected meteorological domain.
    pub domain: Option<&'a DomainId>,
}

/// Constructs an exact path sampler for one particle proposal.
pub trait BoundaryPathSamplerFactory: Send {
    /// Starts one runner boundary batch and discards batch-local scratch.
    fn begin_batch(&mut self);

    /// Releases pins and scratch retained across one completed boundary batch.
    fn end_batch(&mut self);

    /// Certifies a path that cannot reach any configured physical boundary.
    fn certify_clear(
        &mut self,
        request: BoundaryCertificateRequest<'_>,
        meteorology: &mut MetEngine,
        query_plan: &TransportPlan,
        execution: &dyn ExecutionContext,
    ) -> Result<bool, BoundaryError>;

    /// Builds the exact sampler for one unresolved near-boundary path.
    fn build_residual<'a>(
        &'a mut self,
        request: BoundarySamplerRequest<'_>,
        meteorology: &'a mut MetEngine,
        query_plan: &'a TransportPlan,
        execution: &'a dyn ExecutionContext,
    ) -> Result<Box<dyn BoundaryPathSampler + 'a>, BoundaryError>;
}

/// Persistent manifest backend.
///
/// Production implementations must use same-directory temporary files and
/// atomic replacement. The runner calls this before population initialization
/// and after every terminal state transition.
pub trait RunManifestStore: Send {
    /// Persists one complete validated manifest snapshot.
    fn persist(&mut self, manifest: &RunManifest) -> Result<(), String>;
}

/// Wall-clock timestamp source used only for manifest lifecycle timestamps.
pub trait LifecycleClock: Send {
    /// Returns the current UTC timestamp.
    fn now(&mut self) -> Result<Timestamp, String>;
}

/// Task-level progress snapshot observed only at a completed macro-step boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunnerProgress {
    /// Number of fully completed numerical macro steps.
    pub completed_macro_steps: u64,
    /// Exact physical simulation time at this boundary.
    pub simulation_time: Timestamp,
    /// Currently alive particle count.
    pub active_particles: u64,
    /// Cumulative normal termination count.
    pub normal_terminations: u64,
    /// Cumulative abnormal termination count.
    pub abnormal_terminations: u64,
}

/// Decision returned by a runner control implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerControlDecision {
    /// Continue with the next planned macro step.
    Continue,
    /// Finalize a legal cancelled run at this completed boundary.
    Cancel,
}

/// Low-overhead control-plane hook invoked only at macro-step boundaries.
pub trait RunnerControl: Send {
    /// Persists heartbeat/progress and returns any safe cancellation request.
    fn macro_step_boundary(
        &mut self,
        progress: RunnerProgress,
    ) -> Result<RunnerControlDecision, String>;
}

#[derive(Clone, Copy, Debug, Default)]
struct NoopRunnerControl;

impl RunnerControl for NoopRunnerControl {
    fn macro_step_boundary(
        &mut self,
        _progress: RunnerProgress,
    ) -> Result<RunnerControlDecision, String> {
        Ok(RunnerControlDecision::Continue)
    }
}

/// Explicit runtime components supplied by builder/plumbing code.
pub struct SimulationComponents {
    /// Initial in-memory state.
    pub state: SimulationState,
    /// Prepared meteorology engine whose frame cache is populated by the
    /// builder before the first numerical step.
    pub meteorology: MetEngine,
    /// Frozen complete transport plan.
    pub query_plan: TransportPlan,
    /// Caller-owned deterministic execution policy.
    pub execution: Box<dyn ExecutionContext>,
    /// Full population lifecycle.
    pub population: Box<dyn PopulationStrategy>,
    /// Optional M6 physical-process pipeline; absence preserves pure advection.
    pub physics: Option<PhysicsPipeline>,
    /// Particle integrator.
    pub integrator: Box<dyn IntegratorModel>,
    /// Ordered post-integration boundary policies.
    pub boundaries: Vec<Box<dyn BoundaryPolicy>>,
    /// Required factory when any boundary policy is active.
    pub boundary_sampler_factory: Option<Box<dyn BoundaryPathSamplerFactory>>,
    /// Output products and exact schedules.
    pub outputs: Vec<ScheduledOutputProduct>,
    /// Running manifest prepared by the builder.
    pub manifest: RunManifest,
    /// Atomic manifest store.
    pub manifest_store: Box<dyn RunManifestStore>,
    /// Wall-clock lifecycle source.
    pub lifecycle_clock: Box<dyn LifecycleClock>,
    /// Maximum signed outer step.
    pub time_step: SignedDuration,
    /// Exact simulation end instant.
    pub end_time: Timestamp,
    /// Stable seed copied into population contexts and manifest.
    pub random_seed: u64,
    /// Selected single domain, when applicable.
    pub domain: Option<DomainId>,
}

/// Complete single-run orchestrator.
pub struct SimulationRunner {
    state: SimulationState,
    meteorology: MetEngine,
    query_plan: TransportPlan,
    execution: Box<dyn ExecutionContext>,
    population: Box<dyn PopulationStrategy>,
    physics: Option<PhysicsPipeline>,
    integrator: Box<dyn IntegratorModel>,
    boundaries: Vec<Box<dyn BoundaryPolicy>>,
    boundary_sampler_factory: Option<Box<dyn BoundaryPathSamplerFactory>>,
    outputs: Vec<ScheduledOutputProduct>,
    manifest: RunManifest,
    manifest_store: Box<dyn RunManifestStore>,
    lifecycle_clock: Box<dyn LifecycleClock>,
    time_step: SignedDuration,
    end_time: Timestamp,
    random_seed: u64,
    domain: Option<DomainId>,
}

impl SimulationRunner {
    /// Constructs the high-risk runner core from already-resolved components.
    ///
    /// Dataset loading, registry selection, run-directory creation, and atomic
    /// file implementations remain builder responsibilities.
    pub fn from_components(components: SimulationComponents) -> Result<Self, RunError> {
        components
            .state
            .particles
            .validate()
            .map_err(|_| RunError::InvalidConfiguration("invalid initial particles".into()))?;
        validate_direction(
            components.state.clock,
            components.time_step,
            components.end_time,
        )?;
        if components.manifest.status != RunLifecycleStatus::Running
            || components.manifest.numerical.random_seed != components.random_seed
            || components.manifest.numerical.integrator != components.integrator.model_id()
            || components.manifest.numerical.population != components.population.model_id()
            || components
                .manifest
                .numerical
                .boundary_policies
                .iter()
                .map(String::as_str)
                .ne(components
                    .boundaries
                    .iter()
                    .map(|policy| policy.policy_id()))
            || components
                .manifest
                .effective_outputs
                .iter()
                .map(|output| output.product.0.as_str())
                .ne(components
                    .outputs
                    .iter()
                    .map(|output| output.product.product_id()))
            || (!components.boundaries.is_empty() && components.boundary_sampler_factory.is_none())
        {
            return Err(RunError::InvalidConfiguration(
                "runtime components do not match the running manifest".into(),
            ));
        }
        validate_output_schedules(&components.outputs, components.state.clock.direction)?;
        components
            .manifest
            .validate()
            .map_err(|error| RunError::Manifest(format!("{error:?}")))?;
        Ok(Self {
            state: components.state,
            meteorology: components.meteorology,
            query_plan: components.query_plan,
            execution: components.execution,
            population: components.population,
            physics: components.physics,
            integrator: components.integrator,
            boundaries: components.boundaries,
            boundary_sampler_factory: components.boundary_sampler_factory,
            outputs: components.outputs,
            manifest: components.manifest,
            manifest_store: components.manifest_store,
            lifecycle_clock: components.lifecycle_clock,
            time_step: components.time_step,
            end_time: components.end_time,
            random_seed: components.random_seed,
            domain: components.domain,
        })
    }

    /// Executes the complete event-driven main loop.
    pub fn run(&mut self) -> Result<RunOutcome, RunError> {
        self.run_with_control(&mut NoopRunnerControl)
    }

    /// Executes with a control hook polled only at safe macro-step boundaries.
    pub fn run_with_control(
        &mut self,
        control: &mut dyn RunnerControl,
    ) -> Result<RunOutcome, RunError> {
        let _performance = PerformanceScope::enter(PerformanceStage::RunnerTotal);
        if self.manifest.status != RunLifecycleStatus::Running {
            return Err(RunError::InvalidConfiguration(
                "a runner can execute only once from running state".into(),
            ));
        }
        self.persist_manifest()?;
        let result = self.run_inner(control);
        match result {
            Ok(RunLoopCompletion::Natural) => self.finalize_terminal(TerminalDisposition::Natural),
            Ok(RunLoopCompletion::Cancelled) => {
                self.finalize_terminal(TerminalDisposition::Cancelled)
            }
            Err(error) => {
                self.finalize_failure(&error)?;
                Err(error)
            }
        }
    }

    /// Returns the current running or finalized manifest.
    #[must_use]
    pub const fn manifest(&self) -> &RunManifest {
        &self.manifest
    }

    /// Returns current in-memory state for deterministic audit tests.
    #[must_use]
    pub const fn state(&self) -> &SimulationState {
        &self.state
    }

    /// Returns observable meteorology column-cache state for performance audit.
    pub fn meteorology_column_cache_metrics(&self) -> Result<CacheMetrics, EngineError> {
        self.meteorology.column_cache_metrics()
    }

    fn run_inner(
        &mut self,
        control: &mut dyn RunnerControl,
    ) -> Result<RunLoopCompletion, RunError> {
        {
            let _output_performance = PerformanceScope::enter(PerformanceStage::RunnerOutput);
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutputBegin);
            for output in &mut self.outputs {
                output
                    .product
                    .begin(&self.manifest)
                    .map_err(|error| RunError::Output(format!("{error:?}")))?;
            }
        }

        {
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerPopulation);
            let mut context = PopulationContext {
                time: self.state.clock.current,
                direction: self.state.clock.direction,
                step: None,
                step_index: None,
                random_seed: self.random_seed,
                meteorology: &mut self.meteorology,
                execution: self.execution.as_ref(),
                domain: self.domain.clone(),
            };
            self.population
                .initialize(&mut context, &mut self.state.particles)
                .map_err(map_population_error)?;
            self.state
                .particles
                .select_directional_substance_state(self.state.clock.direction)
                .map_err(|error| RunError::Population(format!("{error:?}")))?;
        }
        self.sync_population_state();
        {
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerPopulation);
            self.emit_current_particles()?;
        }
        {
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutput);
            self.sample_outputs(self.state.clock.current, true, &[])?;
        }

        let initial_terminations = termination_summary(&self.state.particles)?;
        let mut progress_counts = ProgressCounts {
            normal: initial_terminations.normal_count,
            abnormal: initial_terminations.abnormal_count,
        };
        if poll_runner_control(control, self.progress_snapshot(progress_counts)?)? {
            self.finish_run_inner()?;
            return Ok(RunLoopCompletion::Cancelled);
        }

        while self.state.clock.current != self.end_time {
            let (step, end_time) = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerStepPlan);
                let boundaries = self.collect_step_boundaries()?;
                let planned = StepPlanner::plan(self.state.clock, self.time_step, &boundaries)
                    .map_err(RunError::Clock)?;
                let step = *planned.first().ok_or_else(|| {
                    RunError::InvalidConfiguration(
                        "step planner made no progress before end".into(),
                    )
                })?;
                let end_time =
                    add_timestamp(self.state.clock.current, step).map_err(RunError::Clock)?;
                (step, end_time)
            };

            let macro_start = self.state.clock.current;
            let existing_particles = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerParticleClone);
                self.state.particles.clone()
            };
            let existing_particle_count = existing_particles
                .len()
                .map_err(|_| RunError::Population("invalid starting particle batch".into()))?;
            let mut births = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerPopulation);
                let mut context = PopulationContext {
                    time: macro_start,
                    direction: self.state.clock.direction,
                    step: Some(step),
                    step_index: Some(self.state.numerical_step_index),
                    random_seed: self.random_seed,
                    meteorology: &mut self.meteorology,
                    execution: self.execution.as_ref(),
                    domain: self.domain.clone(),
                };
                self.population
                    .prepare_cohort_step(&mut context, &existing_particles)
                    .map_err(map_population_error)?
            };
            births
                .select_directional_substance_state(self.state.clock.direction)
                .map_err(|error| RunError::Population(format!("{error:?}")))?;
            let birth_count = births
                .len()
                .map_err(|_| RunError::Population("invalid birth cohort".into()))?;
            if births.birth_time.iter().any(|birth_time| {
                !time_inside_macro_step(
                    macro_start,
                    end_time,
                    *birth_time,
                    self.state.clock.direction,
                )
            }) {
                return Err(RunError::Population(
                    "birth cohort lies outside its macro step".into(),
                ));
            }

            let (start_particles, start_particle_count, start_times) = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerStepAssembly);
                self.sample_interior_births(&births, end_time)?;
                let mut start_particles = existing_particles;
                if birth_count != 0 {
                    start_particles.append(births).map_err(|_| {
                        RunError::Population("invalid appended birth cohort".into())
                    })?;
                }
                let start_particle_count = start_particles
                    .len()
                    .map_err(|_| RunError::Population("invalid starting cohort batch".into()))?;
                if start_particle_count != existing_particle_count.saturating_add(birth_count) {
                    return Err(RunError::Population(
                        "birth cohort count does not match appended batch".into(),
                    ));
                }
                let mut start_times = vec![macro_start; existing_particle_count];
                start_times.extend(
                    start_particles.birth_time[existing_particle_count..]
                        .iter()
                        .copied(),
                );
                (start_particles, start_particle_count, start_times)
            };
            let bounded = self.advance_macro_step(
                macro_start,
                step,
                end_time,
                &start_times,
                &start_particles,
            )?;
            self.state.particles = bounded;
            self.state.clock.current = end_time;

            {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerPopulation);
                let mut context = PopulationContext {
                    time: end_time,
                    direction: self.state.clock.direction,
                    step: Some(step),
                    step_index: Some(self.state.numerical_step_index),
                    random_seed: self.random_seed,
                    meteorology: &mut self.meteorology,
                    execution: self.execution.as_ref(),
                    domain: self.domain.clone(),
                };
                self.population
                    .complete_cohort_step(&mut context, &mut self.state.particles)
                    .map_err(map_population_error)?;
            }
            let (lifecycle_indices, cancelled) = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerLifecycle);
                self.state.numerical_step_index = self
                    .state
                    .numerical_step_index
                    .checked_add(1)
                    .ok_or(RunError::ResourceLimit)?;
                self.sync_population_state();
                let current_particle_count =
                    self.state.particles.len().map_err(|_| {
                        RunError::Population("invalid current particle batch".into())
                    })?;
                if current_particle_count != start_particle_count {
                    return Err(RunError::Population(
                        "cohort integration changed particle row count".into(),
                    ));
                }
                let newly_terminated = (0..start_particle_count)
                    .filter(|index| {
                        matches!(start_particles.status[*index], ParticleStatus::Alive)
                            && matches!(
                                self.state.particles.status[*index],
                                ParticleStatus::Terminated { .. }
                            )
                    })
                    .collect::<Vec<_>>();
                for index in &newly_terminated {
                    let ParticleStatus::Terminated { reason } =
                        &self.state.particles.status[*index]
                    else {
                        return Err(RunError::InvalidConfiguration(
                            "new termination index is not terminated".into(),
                        ));
                    };
                    match reason.class() {
                        TerminationClass::Normal => {
                            progress_counts.normal = progress_counts
                                .normal
                                .checked_add(1)
                                .ok_or(RunError::ResourceLimit)?;
                        }
                        TerminationClass::Abnormal => {
                            progress_counts.abnormal = progress_counts
                                .abnormal
                                .checked_add(1)
                                .ok_or(RunError::ResourceLimit)?;
                        }
                    }
                }
                self.sample_interior_terminations(&newly_terminated, end_time)?;
                let mut lifecycle_indices = newly_terminated
                    .into_iter()
                    .filter(|index| {
                        self.state.particles.termination[*index]
                            .as_ref()
                            .is_none_or(|termination| termination.time == end_time)
                    })
                    .collect::<Vec<_>>();
                lifecycle_indices.extend(
                    (existing_particle_count..start_particle_count)
                        .filter(|index| self.state.particles.birth_time[*index] == end_time),
                );
                lifecycle_indices.sort_unstable();
                lifecycle_indices.dedup();
                let cancelled =
                    poll_runner_control(control, self.progress_snapshot(progress_counts)?)?;
                (lifecycle_indices, cancelled)
            };
            {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutput);
                self.sample_outputs(
                    end_time,
                    cancelled || end_time == self.end_time,
                    &lifecycle_indices,
                )?;
            }
            if cancelled {
                self.finish_run_inner()?;
                return Ok(RunLoopCompletion::Cancelled);
            }
        }

        self.finish_run_inner()?;
        Ok(RunLoopCompletion::Natural)
    }

    fn finish_run_inner(&mut self) -> Result<(), RunError> {
        {
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerPopulation);
            let mut context = PopulationContext {
                time: self.state.clock.current,
                direction: self.state.clock.direction,
                step: None,
                step_index: None,
                random_seed: self.random_seed,
                meteorology: &mut self.meteorology,
                execution: self.execution.as_ref(),
                domain: self.domain.clone(),
            };
            self.population
                .finalize(&mut context, &self.state.particles)
                .map_err(map_population_error)?;
        }
        self.sync_population_state();
        {
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutput);
            let _finish_performance = PerformanceScope::enter(PerformanceStage::RunnerOutputFinish);
            for output in &mut self.outputs {
                if let Err(error) = output.product.finish() {
                    // Immediate abort — do not wait for runner Drop; never swallow abort errors.
                    let abort_err = self.abort_outputs();
                    return Err(match abort_err {
                        Ok(()) => RunError::Output(format!("{error:?}")),
                        Err(ae) => RunError::Output(format!(
                            "finish failed: {error:?}; abort also failed: {ae:?}"
                        )),
                    });
                }
            }
        }
        Ok(())
    }

    fn progress_snapshot(&self, counts: ProgressCounts) -> Result<RunnerProgress, RunError> {
        let total = u64::try_from(
            self.state
                .particles
                .len()
                .map_err(|_| RunError::Population("invalid progress particle batch".into()))?,
        )
        .map_err(|_| RunError::ResourceLimit)?;
        let terminated = counts
            .normal
            .checked_add(counts.abnormal)
            .ok_or(RunError::ResourceLimit)?;
        let active_particles = total.checked_sub(terminated).ok_or_else(|| {
            RunError::InvalidConfiguration("progress termination count exceeds population".into())
        })?;
        Ok(RunnerProgress {
            completed_macro_steps: self.state.numerical_step_index,
            simulation_time: self.state.clock.current,
            active_particles,
            normal_terminations: counts.normal,
            abnormal_terminations: counts.abnormal,
        })
    }

    fn abort_outputs(&mut self) -> Result<(), RunError> {
        let mut errors = Vec::new();
        for output in &mut self.outputs {
            if let Err(e) = output.product.abort() {
                errors.push(format!("{e:?}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(RunError::Output(format!(
                "abort_outputs: {}",
                errors.join("; ")
            )))
        }
    }

    fn quarantine_outputs(&mut self) -> Result<(), RunError> {
        let mut errors = Vec::new();
        for output in &mut self.outputs {
            if let Err(e) = output.product.quarantine_forensic() {
                errors.push(format!("quarantine:{e:?}"));
            }
            if let Err(e) = output.product.abort() {
                errors.push(format!("abort:{e:?}"));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(RunError::Output(format!(
                "quarantine_outputs: {}",
                errors.join("; ")
            )))
        }
    }

    fn emit_current_particles(&mut self) -> Result<usize, RunError> {
        let mut emitted = {
            let mut context = PopulationContext {
                time: self.state.clock.current,
                direction: self.state.clock.direction,
                step: None,
                step_index: Some(self.state.numerical_step_index),
                random_seed: self.random_seed,
                meteorology: &mut self.meteorology,
                execution: self.execution.as_ref(),
                domain: self.domain.clone(),
            };
            self.population
                .emit_particles(&mut context)
                .map_err(map_population_error)?
        };
        emitted
            .select_directional_substance_state(self.state.clock.direction)
            .map_err(|error| RunError::Population(format!("{error:?}")))?;
        let count = emitted
            .len()
            .map_err(|_| RunError::Population("invalid emitted batch".into()))?;
        self.state
            .particles
            .append(emitted)
            .map_err(|error| RunError::Population(format!("{error:?}")))?;
        self.sync_population_state();
        Ok(count)
    }

    fn collect_step_boundaries(&mut self) -> Result<Vec<StepBoundary>, RunError> {
        let target =
            add_timestamp(self.state.clock.current, self.time_step).map_err(RunError::Clock)?;
        let mut boundaries = Vec::new();
        let inside = |time: Timestamp| match self.state.clock.direction {
            Direction::Forward => time > self.state.clock.current && time <= target,
            Direction::Backward => time < self.state.clock.current && time >= target,
        };
        let mut meteorology_times = BTreeSet::new();
        for domain in self.meteorology.catalog().domains.values() {
            meteorology_times.extend(domain.frames.keys().copied().filter(|time| inside(*time)));
        }
        boundaries.extend(
            meteorology_times
                .into_iter()
                .map(StepBoundary::MeteorologyFrame),
        );
        for output in &self.outputs {
            boundaries.extend(
                output
                    .scheduler
                    .sample_times
                    .iter()
                    .copied()
                    .filter(|time| inside(*time))
                    .map(|time| StepBoundary::Output {
                        time,
                        product_id: output.product.product_id().into(),
                    }),
            );
        }
        boundaries.push(StepBoundary::End(self.end_time));
        order_step_boundaries(&mut boundaries, self.state.clock.direction);
        Ok(boundaries)
    }

    fn apply_boundaries(
        &mut self,
        start_times: &[Timestamp],
        end_time: Timestamp,
        start_particles: &ParticleBatch,
        mut proposed_particles: ParticleBatch,
    ) -> Result<(ParticleBatch, Vec<usize>), RunError> {
        proposed_particles
            .validate()
            .map_err(|_| RunError::Integration("invalid proposal batch".into()))?;
        if self.boundaries.is_empty() {
            return Ok((proposed_particles, Vec::new()));
        }
        let _performance_batch = PerformanceBatchScope::enter();
        let factory = self
            .boundary_sampler_factory
            .as_mut()
            .ok_or_else(|| RunError::InvalidConfiguration("missing boundary sampler".into()))?;
        factory.begin_batch();
        let skip_certified_clear = self
            .boundaries
            .iter()
            .all(|policy| policy.skips_certified_clear_path());
        let len = proposed_particles
            .len()
            .map_err(|_| RunError::Integration("invalid proposal batch".into()))?;
        if start_times.len() != len {
            return Err(RunError::Integration(
                "boundary start-time column length mismatch".into(),
            ));
        }
        let mut surface_reflections = Vec::new();
        for (index, start_time) in start_times.iter().copied().enumerate() {
            if start_time == end_time
                || start_particles.status[index] != ParticleStatus::Alive
                || proposed_particles.status[index] != ParticleStatus::Alive
            {
                continue;
            }
            let certificate = BoundaryCertificateRequest {
                start_time,
                end_time,
                start_longitude_degrees: start_particles.longitude_degrees[index],
                start_latitude_degrees: start_particles.latitude_degrees[index],
                start_height_asl_m: start_particles.height_asl_m[index],
                proposed_longitude_degrees: proposed_particles.longitude_degrees[index],
                proposed_latitude_degrees: proposed_particles.latitude_degrees[index],
                proposed_height_asl_m: proposed_particles.height_asl_m[index],
                domain: self.domain.as_ref(),
            };
            if skip_certified_clear
                && factory
                    .certify_clear(
                        certificate,
                        &mut self.meteorology,
                        &self.query_plan,
                        self.execution.as_ref(),
                    )
                    .map_err(RunError::Boundary)?
            {
                continue;
            }
            let start = start_particles
                .state(index)
                .map_err(|_| RunError::Integration("invalid start batch".into()))?;
            let mut proposed = proposed_particles
                .state(index)
                .map_err(|_| RunError::Integration("invalid proposal batch".into()))?;
            let request = BoundarySamplerRequest {
                start_time,
                end_time,
                start: &start,
                proposed: &proposed,
                domain: self.domain.as_ref(),
            };
            let mut surface_reflected = false;
            {
                let mut path = factory
                    .build_residual(
                        request,
                        &mut self.meteorology,
                        &self.query_plan,
                        self.execution.as_ref(),
                    )
                    .map_err(RunError::Boundary)?;
                let mut context = BoundaryContext {
                    start_time,
                    end_time,
                    domain: self.domain.clone(),
                    path: path.as_mut(),
                };
                for policy in &self.boundaries {
                    let decision = policy
                        .apply(&start, &mut proposed, &mut context)
                        .map_err(RunError::Boundary)?;
                    if reverses_vertical_process_velocity(policy.policy_id(), &decision) {
                        surface_reflected = !surface_reflected;
                    }
                    if let BoundaryDecision::Terminated {
                        reason,
                        intersection_fraction,
                    } = decision
                    {
                        if proposed.status == ParticleStatus::Alive {
                            proposed.status = ParticleStatus::Terminated { reason };
                        }
                        let full_step = signed_duration_between(start_time, end_time)
                            .map_err(RunError::Clock)?;
                        let partial_step =
                            scale_duration_by_fraction(full_step, intersection_fraction)
                                .map_err(RunError::Clock)?;
                        let termination_time =
                            add_timestamp(start_time, partial_step).map_err(RunError::Clock)?;
                        proposed.integration_offset_ns = start
                            .integration_offset_ns
                            .checked_add(partial_step.0)
                            .ok_or(RunError::ResourceLimit)?;
                        proposed.elapsed_age_ns = start
                            .elapsed_age_ns
                            .checked_add(partial_step.0.unsigned_abs())
                            .ok_or(RunError::ResourceLimit)?;
                        proposed.termination = Some(ParticleTermination {
                            time: termination_time,
                            intersection_fraction: Some(intersection_fraction),
                        });
                        break;
                    }
                }
            }
            proposed_particles
                .set_state(index, proposed)
                .map_err(|_| RunError::Integration("invalid bounded particle".into()))?;
            if surface_reflected {
                surface_reflections.push(index);
            }
        }
        factory.end_batch();
        Ok((proposed_particles, surface_reflections))
    }

    fn advance_macro_step(
        &mut self,
        macro_start: Timestamp,
        macro_step: SignedDuration,
        macro_end: Timestamp,
        start_times: &[Timestamp],
        start_particles: &ParticleBatch,
    ) -> Result<ParticleBatch, RunError> {
        if self.physics.is_none() {
            let step_result = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerIntegrator);
                self.integrator
                    .advance_timed(
                        TimedIntegratorInput {
                            particles: start_particles,
                            start_times,
                            end_time: macro_end,
                        },
                        &mut IntegratorContext {
                            meteorology: &mut self.meteorology,
                            query_plan: &self.query_plan,
                            execution: self.execution.as_ref(),
                            domain: self.domain.as_ref(),
                        },
                    )
                    .map_err(|error| RunError::Integration(format!("{error:?}")))?
            };
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerBoundary);
            return self
                .apply_boundaries(
                    start_times,
                    macro_end,
                    start_particles,
                    step_result.particles,
                )
                .map(|(particles, _)| particles);
        }

        let substeps = self
            .physics
            .as_ref()
            .ok_or_else(|| RunError::InvalidConfiguration("missing physics pipeline".into()))?
            .partition(macro_step)
            .map_err(RunError::Physics)?;
        let direction = self.state.clock.direction;
        let domain = self.domain.clone().ok_or_else(|| {
            RunError::InvalidConfiguration("physics requires an explicit meteorology domain".into())
        })?;
        let mut particles = start_particles.clone();
        let mut substep_start = macro_start;
        let mut pending = VecDeque::from(substeps);
        let mut substep_index = 0_u32;
        while let Some(substep) = pending.pop_front() {
            let substep_end = add_timestamp(substep_start, substep).map_err(RunError::Clock)?;
            let clamped_starts =
                clamp_start_times(start_times, substep_start, substep_end, direction);
            let active = active_indices_for_substep(
                &particles,
                start_times,
                substep_start,
                substep_end,
                direction,
            )
            .map_err(RunError::Physics)?;
            let preparation = self
                .physics
                .as_mut()
                .ok_or_else(|| RunError::InvalidConfiguration("missing physics pipeline".into()))?
                .prepare_motion(
                    substep_start,
                    substep_end,
                    &clamped_starts,
                    direction,
                    self.state.numerical_step_index,
                    substep_index,
                    &active,
                    &mut particles,
                    &mut self.meteorology,
                    self.execution.as_ref(),
                    &domain,
                    self.random_seed,
                );
            let prepared_motion = match preparation {
                Ok(prepared) => prepared,
                Err(PhysicsError::SubstepTooLarge { maximum_ns }) => {
                    let magnitude = substep
                        .0
                        .checked_abs()
                        .ok_or(RunError::Physics(PhysicsError::SubstepUnderflow))?;
                    if maximum_ns <= 0 || maximum_ns >= magnitude {
                        return Err(RunError::Physics(PhysicsError::SubstepUnderflow));
                    }
                    let sign = substep.0.signum();
                    let first = SignedDuration(sign * maximum_ns);
                    let remainder = SignedDuration(
                        substep
                            .0
                            .checked_sub(first.0)
                            .ok_or(RunError::Physics(PhysicsError::SubstepUnderflow))?,
                    );
                    pending.push_front(remainder);
                    pending.push_front(first);
                    continue;
                }
                Err(error) => return Err(RunError::Physics(error)),
            };
            let proposal = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerIntegrator);
                self.integrator
                    .advance_timed(
                        TimedIntegratorInput {
                            particles: &particles,
                            start_times: &clamped_starts,
                            end_time: substep_end,
                        },
                        &mut IntegratorContext {
                            meteorology: &mut self.meteorology,
                            query_plan: &self.query_plan,
                            execution: self.execution.as_ref(),
                            domain: self.domain.as_ref(),
                        },
                    )
                    .map_err(|error| RunError::Integration(format!("{error:?}")))?
                    .particles
            };
            let surface_reflections;
            (particles, surface_reflections) = {
                let _performance = PerformanceScope::enter(PerformanceStage::RunnerBoundary);
                self.apply_boundaries(&clamped_starts, substep_end, &particles, proposal)?
            };
            prepared_motion
                .finish(&mut particles, &surface_reflections)
                .map_err(RunError::Physics)?;
            substep_start = substep_end;
            substep_index = substep_index
                .checked_add(1)
                .ok_or(RunError::ResourceLimit)?;
        }
        if substep_start != macro_end {
            return Err(RunError::Physics(PhysicsError::SubstepUnderflow));
        }
        Ok(particles)
    }

    fn sample_interior_births(
        &mut self,
        births: &ParticleBatch,
        macro_end: Timestamp,
    ) -> Result<(), RunError> {
        let mut groups = BTreeMap::<Timestamp, Vec<usize>>::new();
        for (index, time) in births.birth_time.iter().copied().enumerate() {
            if time != macro_end {
                groups.entry(time).or_default().push(index);
            }
        }
        for (time, indices) in ordered_event_groups(groups, self.state.clock.direction) {
            let particles = births
                .select_indices(&indices)
                .map_err(|_| RunError::Output("invalid birth lifecycle subset".into()))?;
            self.sample_lifecycle_batch(time, &particles)?;
        }
        Ok(())
    }

    fn sample_interior_terminations(
        &mut self,
        indices: &[usize],
        macro_end: Timestamp,
    ) -> Result<(), RunError> {
        let mut groups = BTreeMap::<Timestamp, Vec<usize>>::new();
        for index in indices.iter().copied() {
            if let Some(termination) = self.state.particles.termination[index]
                .as_ref()
                .filter(|termination| termination.time != macro_end)
            {
                groups.entry(termination.time).or_default().push(index);
            }
        }
        for (time, indices) in ordered_event_groups(groups, self.state.clock.direction) {
            let particles = self
                .state
                .particles
                .select_indices(&indices)
                .map_err(|_| RunError::Output("invalid termination lifecycle subset".into()))?;
            self.sample_lifecycle_batch(time, &particles)?;
        }
        Ok(())
    }

    fn sample_lifecycle_batch(
        &mut self,
        time: Timestamp,
        particles: &ParticleBatch,
    ) -> Result<(), RunError> {
        if particles.is_empty() {
            return Ok(());
        }
        let mut sampled_any = false;
        for index in 0..self.outputs.len() {
            let meteorology = if self.outputs[index].meteorology_plan.is_some() {
                let _performance =
                    PerformanceScope::enter(PerformanceStage::RunnerOutputMeteorology);
                Some(self.query_output_meteorology(
                    time,
                    QueryPointArrays {
                        longitude_degrees: particles.longitude_degrees.clone(),
                        latitude_degrees: particles.latitude_degrees.clone(),
                        vertical: particles.height_asl_m.clone(),
                    },
                    QueryOrigin::OutputLifecycle,
                )?)
            } else {
                None
            };
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutputSink);
            self.outputs[index]
                .product
                .sample(time, particles, meteorology.as_ref())
                .map_err(|error| RunError::Output(format!("{error:?}")))?;
            sampled_any = true;
        }
        if sampled_any {
            self.state.output_event_index = self
                .state
                .output_event_index
                .checked_add(1)
                .ok_or(RunError::ResourceLimit)?;
        }
        Ok(())
    }

    fn sample_outputs(
        &mut self,
        time: Timestamp,
        force_full_snapshot: bool,
        lifecycle_indices: &[usize],
    ) -> Result<(), RunError> {
        let mut sampled_any = false;
        let lifecycle_particles = if lifecycle_indices.is_empty() {
            None
        } else {
            Some(
                self.state
                    .particles
                    .select_indices(lifecycle_indices)
                    .map_err(|_| RunError::Output("invalid lifecycle output subset".into()))?,
            )
        };
        for index in 0..self.outputs.len() {
            let scheduled = self.outputs[index].scheduler.sample_times.contains(&time);
            let full_snapshot = force_full_snapshot || scheduled;
            if full_snapshot || lifecycle_particles.is_some() {
                let requires_meteorology = self.outputs[index].meteorology_plan.is_some();
                let meteorology = if requires_meteorology {
                    let _performance =
                        PerformanceScope::enter(PerformanceStage::RunnerOutputMeteorology);
                    let particles = if full_snapshot {
                        &self.state.particles
                    } else {
                        lifecycle_particles
                            .as_ref()
                            .ok_or_else(|| RunError::Output("missing lifecycle subset".into()))?
                    };
                    let points = QueryPointArrays {
                        longitude_degrees: particles.longitude_degrees.clone(),
                        latitude_degrees: particles.latitude_degrees.clone(),
                        vertical: particles.height_asl_m.clone(),
                    };
                    let origin = if full_snapshot {
                        QueryOrigin::Output
                    } else {
                        QueryOrigin::OutputLifecycle
                    };
                    Some(self.query_output_meteorology(time, points, origin)?)
                } else {
                    None
                };
                {
                    let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutputSink);
                    let particles = if full_snapshot {
                        &self.state.particles
                    } else {
                        lifecycle_particles
                            .as_ref()
                            .ok_or_else(|| RunError::Output("missing lifecycle subset".into()))?
                    };
                    self.outputs[index]
                        .product
                        .sample(time, particles, meteorology.as_ref())
                        .map_err(|error| RunError::Output(format!("{error:?}")))?;
                }
                sampled_any = true;
            }
        }
        if sampled_any {
            self.state.output_event_index = self
                .state
                .output_event_index
                .checked_add(1)
                .ok_or(RunError::ResourceLimit)?;
        }
        Ok(())
    }

    fn query_output_meteorology(
        &mut self,
        time: Timestamp,
        points: QueryPointArrays,
        origin: QueryOrigin,
    ) -> Result<QueryOutput, RunError> {
        let _query_origin = QueryOriginScope::enter(origin);
        let domain = self.domain.as_ref().ok_or_else(|| {
            RunError::Meteorology("output query requires an explicit meteorology domain".into())
        })?;
        let window = self
            .meteorology
            .prepare_for_domain(time, domain)
            .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;
        let mut workspace = BatchWorkspace::default();
        let prepared = {
            let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutputQueryPrepare);
            window
                .prepare_transport_batch(
                    &self.query_plan,
                    QueryBatch {
                        vertical_coordinate: VerticalQuery::AboveSeaLevel,
                        points,
                    },
                    &mut workspace,
                )
                .map_err(|error| RunError::Meteorology(format!("{error:?}")))?
        };
        let _performance = PerformanceScope::enter(PerformanceStage::RunnerOutputQueryExecute);
        prepared
            .execute(self.execution.as_ref(), &mut workspace)
            .map_err(|error| RunError::Meteorology(format!("{error:?}")))?
            .clone_as_query_output()
            .map_err(|error| RunError::Meteorology(format!("{error:?}")))
    }

    fn sync_population_state(&mut self) {
        self.state.population_state = self.population.snapshot_state();
    }

    fn finalize_terminal(
        &mut self,
        disposition: TerminalDisposition,
    ) -> Result<RunOutcome, RunError> {
        let _performance = PerformanceScope::enter(PerformanceStage::RunnerTerminalFinalize);
        // Freeze terminal timestamp first so any subsequent Failed path is lifecycle-legal.
        // Clock failure is retained and aggregated — never silently dropped.
        let clock_result = self
            .lifecycle_clock
            .now()
            .map_err(|e| RunError::Manifest(format!("lifecycle_clock: {e}")));
        let finished_fallback = match &clock_result {
            Ok(t) => *t,
            Err(_) => {
                // Lifecycle-valid fallback: never earlier than started_at.
                self.manifest.started_at
            }
        };

        let terminal = (|| -> Result<RunOutcome, RunError> {
            self.manifest.mass_ledger = self.population.mass_ledger_records();
            for output in &mut self.outputs {
                output
                    .product
                    .contribute_manifest(&mut self.manifest)
                    .map_err(|error| RunError::Output(format!("{error:?}")))?;
            }
            self.manifest.terminations = termination_summary(&self.state.particles)?;
            let finished_at = clock_result.clone()?;
            let outcome = match disposition {
                TerminalDisposition::Cancelled => {
                    self.manifest.status = RunLifecycleStatus::Cancelled;
                    RunOutcome::Cancelled
                }
                TerminalDisposition::Natural if self.manifest.terminations.abnormal_count == 0 => {
                    self.manifest.status = RunLifecycleStatus::Complete;
                    RunOutcome::Complete
                }
                TerminalDisposition::Natural => {
                    self.manifest.status = RunLifecycleStatus::CompletedWithParticleErrors;
                    RunOutcome::CompletedWithParticleErrors
                }
            };
            self.manifest.finished_at = Some(finished_at);
            self.manifest.failure = None;
            self.persist_manifest()?;
            Ok(outcome)
        })();

        match terminal {
            Ok(outcome) => Ok(outcome),
            Err(primary) => {
                // Quarantine/abort formal artifacts; failures are aggregated, not swallowed.
                let q = self.quarantine_outputs();
                self.manifest.provenance = None;
                self.manifest.status = RunLifecycleStatus::Failed;
                // Always set a lifecycle-legal finished_at before failed-manifest persist.
                if self.manifest.finished_at.is_none() {
                    self.manifest.finished_at = Some(finished_fallback);
                }
                if self
                    .manifest
                    .finished_at
                    .is_some_and(|t| t < self.manifest.started_at)
                {
                    self.manifest.finished_at = Some(self.manifest.started_at);
                }
                let mut message = format!("primary={primary:?}");
                if let Err(clock_err) = &clock_result {
                    message = format!("{message}; clock={clock_err:?}");
                }
                if let Err(qe) = &q {
                    message = format!("{message}; quarantine_or_abort_failed={qe:?}");
                }
                self.manifest.failure = Some(RunFailure {
                    code: "terminal_finalize".into(),
                    message: message.clone(),
                });
                // Failed-manifest second persist: do not swallow.
                let second = self.persist_manifest();
                match (q, second) {
                    (Ok(()), Ok(())) => Err(primary),
                    (Err(qe), Ok(())) => Err(RunError::Output(format!(
                        "terminal failed: {primary:?}; then quarantine failed: {qe:?}"
                    ))),
                    (Ok(()), Err(se)) => Err(RunError::Output(format!(
                        "terminal failed: {primary:?}; failed-manifest persist also failed: {se:?}"
                    ))),
                    (Err(qe), Err(se)) => Err(RunError::Output(format!(
                        "terminal failed: {primary:?}; quarantine failed: {qe:?}; failed-manifest persist failed: {se:?}"
                    ))),
                }
            }
        }
    }

    fn finalize_failure(&mut self, error: &RunError) -> Result<(), RunError> {
        let abort_result = self.abort_outputs();
        // Best-effort termination summary; if it fails, keep previous and fold into message.
        let term_err = match termination_summary(&self.state.particles) {
            Ok(summary) => {
                self.manifest.terminations = summary;
                None
            }
            Err(e) => Some(e),
        };
        let clock_result = self
            .lifecycle_clock
            .now()
            .map_err(|e| RunError::Manifest(format!("lifecycle_clock: {e}")));
        let finished_at = match &clock_result {
            Ok(t) if *t >= self.manifest.started_at => *t,
            Ok(_) | Err(_) => self.manifest.started_at,
        };
        self.manifest.status = RunLifecycleStatus::Failed;
        self.manifest.finished_at = Some(finished_at);
        self.manifest.mass_ledger = self.population.mass_ledger_records();
        let mut message = format!("{error:?}");
        if let Some(te) = &term_err {
            message = format!("{message}; termination_summary={te:?}");
        }
        if let Err(ce) = &clock_result {
            message = format!("{message}; clock={ce:?}");
        }
        if let Err(ae) = &abort_result {
            message = format!("{message}; abort_outputs failed: {ae:?}");
        }
        self.manifest.failure = Some(RunFailure {
            code: error.code().into(),
            message,
        });
        // Never cite provenance on failed terminal status.
        self.manifest.provenance = None;
        let persist = self.persist_manifest();
        match (abort_result, persist, term_err, clock_result) {
            (Ok(()), Ok(()), None, Ok(_)) => Ok(()),
            (abort_r, persist_r, term_r, clock_r) => {
                let mut parts = vec![format!("primary={error:?}")];
                if let Err(ae) = abort_r {
                    parts.push(format!("abort={ae:?}"));
                }
                if let Err(pe) = persist_r {
                    parts.push(format!("persist={pe:?}"));
                }
                if let Some(te) = term_r {
                    parts.push(format!("termination={te:?}"));
                }
                if let Err(ce) = clock_r {
                    parts.push(format!("clock={ce:?}"));
                }
                Err(RunError::Output(parts.join("; ")))
            }
        }
    }

    fn persist_manifest(&mut self) -> Result<(), RunError> {
        let _performance = PerformanceScope::enter(PerformanceStage::RunnerManifestPersist);
        self.manifest
            .validate()
            .map_err(|error| RunError::Manifest(format!("{error:?}")))?;
        self.manifest_store
            .persist(&self.manifest)
            .map_err(RunError::Manifest)
    }
}

fn reverses_vertical_process_velocity(policy_id: &str, decision: &BoundaryDecision) -> bool {
    policy_id == crate::science::SURFACE_REFLECT_ID
        && matches!(
            decision,
            BoundaryDecision::Reflected { collision_count } if collision_count % 2 == 1
        )
}

fn map_population_error(error: PopulationError) -> RunError {
    if matches!(error, PopulationError::MassImbalance) {
        RunError::MassConservation
    } else {
        RunError::Population(format!("{error:?}"))
    }
}

fn order_step_boundaries(boundaries: &mut [StepBoundary], direction: Direction) {
    boundaries.sort_by_key(StepBoundary::time);
    if direction == Direction::Backward {
        boundaries.reverse();
    }
}

fn validate_direction(
    clock: SimulationClock,
    time_step: SignedDuration,
    end_time: Timestamp,
) -> Result<(), RunError> {
    let valid = match clock.direction {
        Direction::Forward => time_step.0 > 0 && end_time >= clock.current,
        Direction::Backward => time_step.0 < 0 && end_time <= clock.current,
    };
    if !valid {
        return Err(RunError::InvalidConfiguration(
            "time step or end time conflicts with direction".into(),
        ));
    }
    Ok(())
}

fn validate_output_schedules(
    outputs: &[ScheduledOutputProduct],
    direction: Direction,
) -> Result<(), RunError> {
    let mut products = BTreeSet::new();
    for output in outputs {
        if !products.insert(output.product.product_id())
            || output
                .scheduler
                .sample_times
                .windows(2)
                .any(|pair| match direction {
                    Direction::Forward => pair[0] >= pair[1],
                    Direction::Backward => pair[0] <= pair[1],
                })
        {
            return Err(RunError::InvalidConfiguration(
                "duplicate output product or unordered output schedule".into(),
            ));
        }
    }
    Ok(())
}

fn ordered_event_groups(
    groups: BTreeMap<Timestamp, Vec<usize>>,
    direction: Direction,
) -> Vec<(Timestamp, Vec<usize>)> {
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    if direction == Direction::Backward {
        groups.reverse();
    }
    groups
}

fn time_inside_macro_step(
    start: Timestamp,
    end: Timestamp,
    time: Timestamp,
    direction: Direction,
) -> bool {
    match direction {
        Direction::Forward => time > start && time <= end,
        Direction::Backward => time < start && time >= end,
    }
}

fn termination_summary(particles: &ParticleBatch) -> Result<TerminationSummary, RunError> {
    particles
        .validate()
        .map_err(|_| RunError::InvalidConfiguration("invalid final particles".into()))?;
    let mut summary = TerminationSummary::default();
    for status in &particles.status {
        let ParticleStatus::Terminated { reason } = status else {
            continue;
        };
        match reason.class() {
            TerminationClass::Normal => {
                summary.normal_count = summary
                    .normal_count
                    .checked_add(1)
                    .ok_or(RunError::ResourceLimit)?;
            }
            TerminationClass::Abnormal => {
                summary.abnormal_count = summary
                    .abnormal_count
                    .checked_add(1)
                    .ok_or(RunError::ResourceLimit)?;
            }
        }
        let count = summary.by_reason.entry(reason.code()).or_insert(0);
        *count = count.checked_add(1).ok_or(RunError::ResourceLimit)?;
    }
    Ok(summary)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProgressCounts {
    normal: u64,
    abnormal: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunLoopCompletion {
    Natural,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalDisposition {
    Natural,
    Cancelled,
}

fn poll_runner_control(
    control: &mut dyn RunnerControl,
    progress: RunnerProgress,
) -> Result<bool, RunError> {
    control
        .macro_step_boundary(progress)
        .map(|decision| decision == RunnerControlDecision::Cancel)
        .map_err(RunError::Control)
}

/// Successful run-level outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// All run-level work completed and no particle terminated abnormally.
    Complete,
    /// Run-level work completed but one or more particles terminated abnormally.
    CompletedWithParticleErrors,
    /// A safe macro-step-boundary cancellation finalized auditable outputs.
    Cancelled,
}

/// Builder that resolves model identifiers and validates runtime capabilities.
pub struct RunnerBuilder {
    /// Portable resolved scientific intent.
    pub case: ResolvedCase,
    /// Resolved machine resources and dataset locations.
    pub run_profile: ResolvedRunProfile,
}

/// Fatal run-construction or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunError {
    /// Builder-side engineering plumbing has not been implemented yet.
    NotImplemented,
    /// Runtime components conflict with the frozen scientific contract.
    InvalidConfiguration(String),
    /// Meteorology could not prepare the required time window.
    Meteorology(String),
    /// Population lifecycle failed.
    Population(String),
    /// Particle integration failed.
    Integration(String),
    /// M6 physical-process planning or execution failed.
    Physics(PhysicsError),
    /// A boundary sampler or policy failed structurally.
    Boundary(BoundaryError),
    /// Clock planning or timestamp arithmetic failed.
    Clock(ClockError),
    /// Output product or encoder failed.
    Output(String),
    /// Running/final manifest validation or persistence failed.
    Manifest(String),
    /// Job-control heartbeat or cancellation polling failed.
    Control(String),
    /// Domain-fill mass conservation exceeded tolerance.
    MassConservation,
    /// Requested memory, counter, or particle capacity cannot be satisfied.
    ResourceLimit,
}

impl RunError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotImplemented => "run.not_implemented",
            Self::InvalidConfiguration(_) => "run.invalid_configuration",
            Self::Meteorology(_) => "run.meteorology",
            Self::Population(_) => "run.population",
            Self::Integration(_) => "run.integration",
            Self::Physics(error) => error.code(),
            Self::Boundary(_) => "run.boundary",
            Self::Clock(_) => "run.clock",
            Self::Output(_) => "run.output",
            Self::Manifest(_) => "run.manifest",
            Self::Control(_) => "run.control",
            Self::MassConservation => "run.mass_conservation",
            Self::ResourceLimit => "run.resource_limit",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use trajecta_case::document::MeteorologyReaderBackend;
    use trajecta_case::model::output::default_particle_state_output;
    use trajecta_case::model::population::{PopulationId, ReleaseEventId};
    use trajecta_met::field::{Capability, CapabilitySet, FieldRegistry};
    use trajecta_met::io::inventory::MetCatalog;
    use trajecta_met::profile::document::ProfileCatalog;
    use trajecta_met::profile::graph::ExecutionPlan;
    use trajecta_met::query::cache::MemoryBudget;
    use trajecta_met::query::engine::{MetEngineConfig, RayonExecutionContext};
    use trajecta_met::query::request::TransportPlanRequest;
    use trajecta_met::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerRegistry};

    use super::*;
    use crate::integrator::{IntegratorError, IntegratorInput, StepResult};
    use crate::manifest::{
        ExecutionSummary, InputIdentity, NumericalSummary, RunId, RunManifestStart,
        SoftwareIdentity,
    };
    use crate::output::OutputError;
    use crate::particle::{ParticleId, ParticleOrigin, SubstanceMassStore, TerminationReason};
    use crate::population::PopulationError;

    const TEST_INTEGRATOR_ID: &str = "test_integrator/v1";
    const TEST_POPULATION_ID: &str = "test_population/v1";

    fn log(log: &Arc<Mutex<Vec<String>>>, value: impl Into<String>) {
        log.lock().unwrap().push(value.into());
    }

    fn particle_at(id: u64, birth_time: Timestamp) -> ParticleBatch {
        ParticleBatch {
            id: vec![ParticleId(id)],
            population_id: vec![PopulationId("p".into())],
            origin: vec![ParticleOrigin::Release {
                event_id: ReleaseEventId("e".into()),
            }],
            birth_time: vec![birth_time],
            longitude_degrees: vec![0.0],
            latitude_degrees: vec![0.0],
            height_asl_m: vec![1_000.0],
            integration_offset_ns: vec![0],
            elapsed_age_ns: vec![0],
            dry_air_mass_kg: vec![0.0],
            status: vec![ParticleStatus::Alive],
            termination: vec![None],
            mass: SubstanceMassStore::default(),
            adjoint: Default::default(),
            motion: Default::default(),
        }
    }

    fn one_particle() -> ParticleBatch {
        particle_at(1, Timestamp::UNIX_EPOCH)
    }

    #[test]
    fn surface_process_velocity_uses_collision_parity() {
        for (collision_count, expected) in [(0, false), (1, true), (2, false), (3, true)] {
            assert_eq!(
                reverses_vertical_process_velocity(
                    crate::science::SURFACE_REFLECT_ID,
                    &BoundaryDecision::Reflected { collision_count },
                ),
                expected
            );
        }
        assert!(!reverses_vertical_process_velocity(
            "another_boundary",
            &BoundaryDecision::Reflected { collision_count: 1 },
        ));
        assert!(!reverses_vertical_process_velocity(
            crate::science::SURFACE_REFLECT_ID,
            &BoundaryDecision::Continue,
        ));
    }

    struct LoggingPopulation {
        log: Arc<Mutex<Vec<String>>>,
        state: PopulationState,
        initialized_particle: bool,
        dynamic_birth_at: Option<Timestamp>,
        dynamic_birth_emitted: bool,
    }

    impl PopulationStrategy for LoggingPopulation {
        fn model_id(&self) -> &'static str {
            TEST_POPULATION_ID
        }

        fn snapshot_state(&self) -> PopulationState {
            self.state.clone()
        }

        fn dynamic_step_boundaries(
            &mut self,
            context: &mut PopulationContext<'_>,
            requested: SignedDuration,
        ) -> Result<Vec<StepBoundary>, PopulationError> {
            let Some(time) = self.dynamic_birth_at else {
                return Ok(Vec::new());
            };
            if self.dynamic_birth_emitted {
                return Ok(Vec::new());
            }
            let target = add_timestamp(context.time, requested)
                .map_err(|_| PopulationError::TimeOverflow)?;
            let inside = match context.direction {
                Direction::Forward => time > context.time && time <= target,
                Direction::Backward => time < context.time && time >= target,
            };
            Ok(inside
                .then(|| StepBoundary::Population {
                    time,
                    event_id: "test-dynamic-birth".into(),
                })
                .into_iter()
                .collect())
        }

        fn initialize(
            &mut self,
            _context: &mut PopulationContext<'_>,
            particles: &mut ParticleBatch,
        ) -> Result<(), PopulationError> {
            log(&self.log, "population.initialize");
            self.state = PopulationState::ReleaseDriven {
                emitted_birth_count: 0,
            };
            if self.initialized_particle {
                particles
                    .append(one_particle())
                    .map_err(|_| PopulationError::InvalidParticleBatch)?;
            }
            Ok(())
        }

        fn before_step(
            &mut self,
            context: &mut PopulationContext<'_>,
            _particles: &ParticleBatch,
        ) -> Result<(), PopulationError> {
            log(
                &self.log,
                format!(
                    "population.before:{}",
                    context.time.seconds_since_unix_epoch()
                ),
            );
            Ok(())
        }

        fn emit_particles(
            &mut self,
            context: &mut PopulationContext<'_>,
        ) -> Result<ParticleBatch, PopulationError> {
            log(
                &self.log,
                format!(
                    "population.emit:{}",
                    context.time.seconds_since_unix_epoch()
                ),
            );
            if self.dynamic_birth_at == Some(context.time) && !self.dynamic_birth_emitted {
                self.dynamic_birth_emitted = true;
                self.state = PopulationState::ReleaseDriven {
                    emitted_birth_count: 1,
                };
                return Ok(particle_at(2, context.time));
            }
            Ok(ParticleBatch::default())
        }

        fn after_advection(
            &mut self,
            context: &mut PopulationContext<'_>,
            _particles: &ParticleBatch,
        ) -> Result<(), PopulationError> {
            log(
                &self.log,
                format!(
                    "population.after:{}",
                    context.time.seconds_since_unix_epoch()
                ),
            );
            Ok(())
        }

        fn apply_boundary_maintenance(
            &mut self,
            context: &mut PopulationContext<'_>,
            _particles: &mut ParticleBatch,
        ) -> Result<(), PopulationError> {
            log(
                &self.log,
                format!(
                    "population.maintain:{}",
                    context.time.seconds_since_unix_epoch()
                ),
            );
            Ok(())
        }

        fn finalize(
            &mut self,
            _context: &mut PopulationContext<'_>,
            _particles: &ParticleBatch,
        ) -> Result<(), PopulationError> {
            log(&self.log, "population.finalize");
            Ok(())
        }
    }

    struct LoggingIntegrator {
        log: Arc<Mutex<Vec<String>>>,
        terminate_abnormally: bool,
    }

    impl IntegratorModel for LoggingIntegrator {
        fn model_id(&self) -> &'static str {
            TEST_INTEGRATOR_ID
        }

        fn advance(
            &self,
            input: IntegratorInput<'_>,
            _context: &mut IntegratorContext<'_>,
        ) -> Result<StepResult, IntegratorError> {
            log(
                &self.log,
                format!(
                    "integrator:{}:{}:{}:{:?}",
                    input.time.seconds_since_unix_epoch(),
                    input.time.nanosecond(),
                    input.step.0,
                    input.particles.id.iter().map(|id| id.0).collect::<Vec<_>>()
                ),
            );
            let mut particles = input.particles.clone();
            for index in 0..particles
                .len()
                .map_err(|_| IntegratorError::InvalidParticleBatch)?
            {
                let mut state = particles
                    .state(index)
                    .map_err(|_| IntegratorError::InvalidParticleBatch)?;
                if state.status != ParticleStatus::Alive {
                    continue;
                }
                state.integration_offset_ns = state
                    .integration_offset_ns
                    .checked_add(input.step.0)
                    .ok_or_else(|| IntegratorError::NumericalInvariant("offset".into()))?;
                state.elapsed_age_ns = state
                    .elapsed_age_ns
                    .checked_add(input.step.0.unsigned_abs())
                    .ok_or_else(|| IntegratorError::NumericalInvariant("age".into()))?;
                if self.terminate_abnormally {
                    state.status = ParticleStatus::Terminated {
                        reason: TerminationReason::NumericalFailure,
                    };
                }
                particles
                    .set_state(index, state)
                    .map_err(|_| IntegratorError::InvalidParticleBatch)?;
            }
            Ok(StepResult {
                particles,
                abnormal_terminated_count: usize::from(self.terminate_abnormally),
            })
        }
    }

    struct NoopBoundaryPathSampler;

    impl BoundaryPathSampler for NoopBoundaryPathSampler {
        fn ordered_segments(
            &self,
        ) -> Result<Vec<crate::boundary::BoundaryPathSegment>, BoundaryError> {
            Ok(vec![crate::boundary::BoundaryPathSegment {
                start_fraction: 0.0,
                end_fraction: 1.0,
            }])
        }

        fn sample(
            &mut self,
            _fraction: f64,
        ) -> Result<crate::boundary::BoundarySample, BoundaryError> {
            Err(BoundaryError::MissingContext)
        }

        fn retarget(
            &mut self,
            _start_fraction: f64,
            _start: &ParticleState,
            _proposed: &ParticleState,
        ) -> Result<(), BoundaryError> {
            Ok(())
        }
    }

    struct NoopBoundaryPathSamplerFactory;

    impl BoundaryPathSamplerFactory for NoopBoundaryPathSamplerFactory {
        fn begin_batch(&mut self) {}

        fn end_batch(&mut self) {}

        fn certify_clear(
            &mut self,
            _request: BoundaryCertificateRequest<'_>,
            _meteorology: &mut MetEngine,
            _query_plan: &TransportPlan,
            _execution: &dyn ExecutionContext,
        ) -> Result<bool, BoundaryError> {
            Ok(false)
        }

        fn build_residual<'a>(
            &'a mut self,
            _request: BoundarySamplerRequest<'_>,
            _meteorology: &'a mut MetEngine,
            _query_plan: &'a TransportPlan,
            _execution: &'a dyn ExecutionContext,
        ) -> Result<Box<dyn BoundaryPathSampler + 'a>, BoundaryError> {
            Ok(Box::new(NoopBoundaryPathSampler))
        }
    }

    struct FixedFractionTermination {
        fraction: f64,
    }

    impl BoundaryPolicy for FixedFractionTermination {
        fn policy_id(&self) -> &'static str {
            "test_fraction_termination"
        }

        fn skips_certified_clear_path(&self) -> bool {
            false
        }

        fn apply(
            &self,
            start: &ParticleState,
            proposed: &mut ParticleState,
            _context: &mut BoundaryContext<'_>,
        ) -> Result<BoundaryDecision, BoundaryError> {
            proposed.longitude_degrees = start.longitude_degrees
                + self.fraction * (proposed.longitude_degrees - start.longitude_degrees);
            proposed.latitude_degrees = start.latitude_degrees
                + self.fraction * (proposed.latitude_degrees - start.latitude_degrees);
            proposed.height_asl_m =
                start.height_asl_m + self.fraction * (proposed.height_asl_m - start.height_asl_m);
            Ok(BoundaryDecision::Terminated {
                reason: TerminationReason::OutsideDomain,
                intersection_fraction: self.fraction,
            })
        }
    }

    struct MassCheckingPopulation {
        state: PopulationState,
        expected_carrier_mass_kg: f64,
    }

    impl PopulationStrategy for MassCheckingPopulation {
        fn model_id(&self) -> &'static str {
            TEST_POPULATION_ID
        }

        fn snapshot_state(&self) -> PopulationState {
            self.state.clone()
        }

        fn initialize(
            &mut self,
            _context: &mut PopulationContext<'_>,
            particles: &mut ParticleBatch,
        ) -> Result<(), PopulationError> {
            let mut particle = one_particle();
            particle.dry_air_mass_kg[0] = self.expected_carrier_mass_kg;
            particles
                .append(particle)
                .map_err(|_| PopulationError::InvalidParticleBatch)?;
            self.state = PopulationState::ReleaseDriven {
                emitted_birth_count: 0,
            };
            Ok(())
        }

        fn before_step(
            &mut self,
            _context: &mut PopulationContext<'_>,
            _particles: &ParticleBatch,
        ) -> Result<(), PopulationError> {
            Ok(())
        }

        fn emit_particles(
            &mut self,
            _context: &mut PopulationContext<'_>,
        ) -> Result<ParticleBatch, PopulationError> {
            Ok(ParticleBatch::default())
        }

        fn after_advection(
            &mut self,
            _context: &mut PopulationContext<'_>,
            particles: &ParticleBatch,
        ) -> Result<(), PopulationError> {
            particles
                .validate()
                .map_err(|_| PopulationError::InvalidParticleBatch)?;
            if particles
                .dry_air_mass_kg
                .iter()
                .any(|mass| *mass != self.expected_carrier_mass_kg)
            {
                return Err(PopulationError::MassImbalance);
            }
            Ok(())
        }

        fn apply_boundary_maintenance(
            &mut self,
            _context: &mut PopulationContext<'_>,
            _particles: &mut ParticleBatch,
        ) -> Result<(), PopulationError> {
            Ok(())
        }

        fn finalize(
            &mut self,
            _context: &mut PopulationContext<'_>,
            _particles: &ParticleBatch,
        ) -> Result<(), PopulationError> {
            Ok(())
        }
    }

    struct DryAirMassTamperingIntegrator;

    impl IntegratorModel for DryAirMassTamperingIntegrator {
        fn model_id(&self) -> &'static str {
            TEST_INTEGRATOR_ID
        }

        fn advance(
            &self,
            input: IntegratorInput<'_>,
            _context: &mut IntegratorContext<'_>,
        ) -> Result<StepResult, IntegratorError> {
            let mut particles = input.particles.clone();
            particles
                .validate()
                .map_err(|_| IntegratorError::InvalidParticleBatch)?;
            for mass in &mut particles.dry_air_mass_kg {
                *mass *= 2.0;
            }
            Ok(StepResult {
                particles,
                abnormal_terminated_count: 0,
            })
        }
    }

    struct LoggingOutput {
        log: Arc<Mutex<Vec<String>>>,
        fail_begin: bool,
        fail_contribute: bool,
    }

    impl OutputProduct for LoggingOutput {
        fn product_id(&self) -> &'static str {
            trajecta_case::model::output::PARTICLE_STATE_PRODUCT_ID
        }

        fn begin(&mut self, _manifest: &RunManifest) -> Result<(), OutputError> {
            log(&self.log, "output.begin");
            if self.fail_begin {
                Err(OutputError::Io("forced".into()))
            } else {
                Ok(())
            }
        }

        fn sample(
            &mut self,
            time: Timestamp,
            particles: &ParticleBatch,
            _meteorology: Option<&trajecta_met::query::output::QueryOutput>,
        ) -> Result<(), OutputError> {
            log(
                &self.log,
                format!("output.sample:{}", time.seconds_since_unix_epoch()),
            );
            log(
                &self.log,
                format!(
                    "output.sample_count:{}:{}:{}",
                    time.seconds_since_unix_epoch(),
                    time.nanosecond(),
                    particles.len().map_err(|_| OutputError::InvalidInput)?
                ),
            );
            Ok(())
        }

        fn finish(&mut self) -> Result<(), OutputError> {
            log(&self.log, "output.finish");
            Ok(())
        }

        fn contribute_manifest(&mut self, manifest: &mut RunManifest) -> Result<(), OutputError> {
            if self.fail_contribute {
                return Err(OutputError::Encoding("forced contribute_manifest".into()));
            }
            // Unit-test stub identity so terminal Complete validates under
            // provenance-bundle/v1 contract without a real SQLite sink.
            let content = "33".repeat(32);
            let sql = "44".repeat(32);
            let canonical =
                crate::output::provenance_bundle::canonical_output_digest(&sql, &content)
                    .unwrap_or_else(|_| "00".repeat(32));
            manifest.provenance = Some(crate::manifest::ProvenanceBundleIdentity {
                schema_version: crate::science::PROVENANCE_BUNDLE_SCHEMA_ID.into(),
                relative_path: crate::science::PROVENANCE_BUNDLE_FILE_NAME.into(),
                sha256: "11".repeat(32),
                sqlite_sha256: "22".repeat(32),
                content_sha256: content,
                sqlite_sql_sha256: sql,
                canonical_output_sha256: canonical,
                record_count: 0,
                field_set_count: 0,
                sample_count: 0,
            });
            Ok(())
        }
    }

    struct LoggingManifestStore {
        log: Arc<Mutex<Vec<String>>>,
    }

    impl RunManifestStore for LoggingManifestStore {
        fn persist(&mut self, manifest: &RunManifest) -> Result<(), String> {
            log(&self.log, format!("manifest:{:?}", manifest.status));
            Ok(())
        }
    }

    /// Fails on the N-th persist call (1-based). Used for terminal-persist injection.
    struct FailOnNthManifestStore {
        log: Arc<Mutex<Vec<String>>>,
        n: usize,
        count: usize,
        last: Option<RunLifecycleStatus>,
    }

    impl RunManifestStore for FailOnNthManifestStore {
        fn persist(&mut self, manifest: &RunManifest) -> Result<(), String> {
            self.count += 1;
            self.last = Some(manifest.status);
            log(
                &self.log,
                format!("manifest:{:?}:call{}", manifest.status, self.count),
            );
            if self.count == self.n {
                return Err("forced terminal persist failure".into());
            }
            Ok(())
        }
    }

    struct FixedLifecycleClock;

    impl LifecycleClock for FixedLifecycleClock {
        fn now(&mut self) -> Result<Timestamp, String> {
            Timestamp::new(100, 0).map_err(|error| format!("{error:?}"))
        }
    }

    #[derive(Default)]
    struct CancelAfterOneStep {
        observed: Vec<RunnerProgress>,
    }

    impl RunnerControl for CancelAfterOneStep {
        fn macro_step_boundary(
            &mut self,
            progress: RunnerProgress,
        ) -> Result<RunnerControlDecision, String> {
            self.observed.push(progress);
            if progress.completed_macro_steps >= 1 {
                Ok(RunnerControlDecision::Cancel)
            } else {
                Ok(RunnerControlDecision::Continue)
            }
        }
    }

    fn engine_and_plan() -> (MetEngine, TransportPlan) {
        let capabilities = CapabilitySet::new()
            .with(Capability::Transport)
            .with(Capability::NearSurfaceTransport);
        let catalog = MetCatalog {
            domains: BTreeMap::new(),
            capabilities,
        };
        let mut surface_layers = SurfaceLayerRegistry::new();
        surface_layers
            .register(Arc::new(MoninObukhovBusingerDyer::default()))
            .unwrap();
        let engine = MetEngine::new(MetEngineConfig {
            catalog,
            profiles: ProfileCatalog::default(),
            fields: FieldRegistry::canonical().unwrap(),
            surface_layers,
            memory_budget: MemoryBudget::new(1024 * 1024, 0).unwrap(),
        });
        let plan = engine
            .compile_transport_plan(TransportPlanRequest::default(), &ExecutionPlan::default())
            .unwrap();
        (engine, plan)
    }

    fn manifest(direction: Direction) -> RunManifest {
        let _ = direction;
        RunManifest::running(RunManifestStart {
            run_id: RunId("018f0000-0000-7000-8000-000000000000".into()),
            case_name: "case".into(),
            started_at: Timestamp::UNIX_EPOCH,
            software: SoftwareIdentity {
                crate_versions: BTreeMap::from([("trajecta-core".into(), "0.0.0".into())]),
                git_commit: None,
            },
            inputs: InputIdentity {
                case_sha256: "0".repeat(64),
                run_profile_sha256: "1".repeat(64),
                dataset_lock_sha256: BTreeMap::new(),
                dataset_profile_sha256: BTreeMap::new(),
                dataset_content_sha256: BTreeMap::new(),
            },
            execution: ExecutionSummary {
                worker_threads: 1,
                memory_budget_bytes: 1024 * 1024,
                executor: "test".into(),
                reader_backends: BTreeMap::<String, MeteorologyReaderBackend>::new(),
                wall_time_ns: None,
                peak_rss_bytes: None,
                io_counters: BTreeMap::new(),
            },
            numerical: NumericalSummary {
                random_seed: 7,
                integrator: TEST_INTEGRATOR_ID.into(),
                boundary_policies: Vec::new(),
                population: TEST_POPULATION_ID.into(),
                ozone_rule: None,
                particle_state_sink: trajecta_case::model::output::PARTICLE_STATE_SQLITE_SINK_ID
                    .into(),
                tolerance_registry: "trajecta.m4.numerical-contract/v1".into(),
                tolerances: BTreeMap::from([("test".into(), 0.0)]),
                deterministic: true,
            },
            geometries: Vec::new(),
            effective_outputs: vec![default_particle_state_output()],
        })
    }

    fn runner(
        direction: Direction,
        terminate_abnormally: bool,
        fail_begin: bool,
    ) -> (SimulationRunner, Arc<Mutex<Vec<String>>>) {
        runner_with_flags(direction, terminate_abnormally, fail_begin, false, None)
    }

    fn runner_with_flags(
        direction: Direction,
        terminate_abnormally: bool,
        fail_begin: bool,
        fail_contribute: bool,
        store: Option<Box<dyn crate::runner::RunManifestStore>>,
    ) -> (SimulationRunner, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (start, end, step, schedule) = match direction {
            Direction::Forward => (
                Timestamp::UNIX_EPOCH,
                Timestamp::new(3, 0).unwrap(),
                SignedDuration(1_000_000_000),
                vec![Timestamp::new(1, 0).unwrap(), Timestamp::new(2, 0).unwrap()],
            ),
            Direction::Backward => (
                Timestamp::new(3, 0).unwrap(),
                Timestamp::UNIX_EPOCH,
                SignedDuration(-1_000_000_000),
                vec![Timestamp::new(2, 0).unwrap(), Timestamp::new(1, 0).unwrap()],
            ),
        };
        let (meteorology, query_plan) = engine_and_plan();
        let runner = SimulationRunner::from_components(SimulationComponents {
            state: SimulationState {
                clock: SimulationClock {
                    current: start,
                    direction,
                },
                particles: ParticleBatch::default(),
                population_state: PopulationState::Uninitialized,
                numerical_step_index: 0,
                output_event_index: 0,
            },
            meteorology,
            query_plan,
            execution: Box::new(RayonExecutionContext { worker_threads: 1 }),
            population: Box::new(LoggingPopulation {
                log: log.clone(),
                state: PopulationState::Uninitialized,
                initialized_particle: true,
                dynamic_birth_at: None,
                dynamic_birth_emitted: false,
            }),
            physics: None,
            integrator: Box::new(LoggingIntegrator {
                log: log.clone(),
                terminate_abnormally,
            }),
            boundaries: Vec::new(),
            boundary_sampler_factory: None,
            outputs: vec![ScheduledOutputProduct {
                product: Box::new(LoggingOutput {
                    log: log.clone(),
                    fail_begin,
                    fail_contribute,
                }),
                scheduler: OutputScheduler {
                    sample_times: schedule,
                },
                meteorology_plan: None,
            }],
            manifest: manifest(direction),
            manifest_store: store
                .unwrap_or_else(|| Box::new(LoggingManifestStore { log: log.clone() })),
            lifecycle_clock: Box::new(FixedLifecycleClock),
            time_step: step,
            end_time: end,
            random_seed: 7,
            domain: None,
        })
        .unwrap();
        (runner, log)
    }

    #[test]
    fn running_manifest_precedes_first_particle_and_lifecycle_order_is_fixed() {
        let (mut runner, log) = runner(Direction::Forward, false, false);
        assert_eq!(runner.run(), Ok(RunOutcome::Complete));
        assert_eq!(runner.manifest.status, RunLifecycleStatus::Complete);
        assert_eq!(runner.state.clock.current, Timestamp::new(3, 0).unwrap());
        assert_eq!(runner.state.numerical_step_index, 3);
        assert_eq!(
            runner.state.particles.integration_offset_ns,
            vec![3_000_000_000]
        );
        assert_eq!(runner.state.particles.elapsed_age_ns, vec![3_000_000_000]);
        let values = log.lock().unwrap();
        assert_eq!(values[0], "manifest:Running");
        assert!(
            values
                .iter()
                .position(|value| value == "population.initialize")
                .unwrap()
                > 0
        );
        assert!(
            values
                .iter()
                .position(|value| value == "population.finalize")
                .unwrap()
                < values
                    .iter()
                    .position(|value| value == "output.finish")
                    .unwrap()
        );
        assert_eq!(values.last().unwrap(), "manifest:Complete");
        assert!(values.iter().any(|value| value == "output.sample:1"));
        assert!(values.iter().any(|value| value == "output.sample:2"));
    }

    #[test]
    fn safe_control_cancel_stops_at_macro_boundary_and_finalizes_cancelled() {
        let (mut runner, log) = runner(Direction::Forward, false, false);
        let mut control = CancelAfterOneStep::default();
        assert_eq!(
            runner.run_with_control(&mut control),
            Ok(RunOutcome::Cancelled)
        );
        assert_eq!(runner.manifest.status, RunLifecycleStatus::Cancelled);
        assert_eq!(runner.manifest.failure, None);
        assert!(runner.manifest.provenance.is_some());
        assert_eq!(runner.state.clock.current, Timestamp::new(1, 0).unwrap());
        assert_eq!(runner.state.numerical_step_index, 1);
        assert_eq!(control.observed.len(), 2);
        assert_eq!(control.observed[0].completed_macro_steps, 0);
        assert_eq!(control.observed[1].completed_macro_steps, 1);
        assert_eq!(
            control.observed[1].simulation_time,
            Timestamp::new(1, 0).unwrap()
        );
        assert_eq!(control.observed[1].active_particles, 1);
        let values = log.lock().unwrap();
        assert!(values.iter().any(|value| value == "output.sample:1"));
        assert!(
            values
                .iter()
                .position(|value| value == "population.finalize")
                .unwrap()
                < values
                    .iter()
                    .position(|value| value == "output.finish")
                    .unwrap()
        );
        assert_eq!(values.last().unwrap(), "manifest:Cancelled");
        assert!(
            !values
                .iter()
                .any(|value| value == "integrator:1:0:1000000000:[1]")
        );
    }

    #[test]
    fn unscheduled_dynamic_birth_writes_only_the_new_particle() {
        let (mut runner, log) = runner(Direction::Forward, false, false);
        runner.population = Box::new(LoggingPopulation {
            log: log.clone(),
            state: PopulationState::Uninitialized,
            initialized_particle: true,
            dynamic_birth_at: Some(Timestamp::new(0, 500_000_000).unwrap()),
            dynamic_birth_emitted: false,
        });

        assert_eq!(runner.run(), Ok(RunOutcome::Complete));
        let values = log.lock().unwrap();
        assert!(
            values
                .iter()
                .any(|value| value == "output.sample_count:0:0:1")
        );
        assert!(
            values
                .iter()
                .any(|value| value == "output.sample_count:0:500000000:1"),
            "dynamic birth must not duplicate a full two-particle snapshot: {values:?}"
        );
        assert!(
            values
                .iter()
                .any(|value| value == "output.sample_count:1:0:2"),
            "the next scheduled output must contain both particles: {values:?}"
        );
    }

    #[test]
    fn cohort_birth_never_splits_existing_particle_advection() {
        for (direction, birth_time, old_step, newborn_step) in [
            (
                Direction::Forward,
                Timestamp::new(0, 500_000_000).unwrap(),
                1_000_000_000,
                500_000_000,
            ),
            (
                Direction::Backward,
                Timestamp::new(2, 500_000_000).unwrap(),
                -1_000_000_000,
                -500_000_000,
            ),
        ] {
            let (mut runner, log) = runner(direction, false, false);
            runner.population = Box::new(LoggingPopulation {
                log: log.clone(),
                state: PopulationState::Uninitialized,
                initialized_particle: true,
                dynamic_birth_at: Some(birth_time),
                dynamic_birth_emitted: false,
            });

            assert_eq!(runner.run(), Ok(RunOutcome::Complete));
            let by_id = (0..runner.state.particles.len().unwrap())
                .map(|index| {
                    let state = runner.state.particles.state(index).unwrap();
                    (state.id, state)
                })
                .collect::<BTreeMap<_, _>>();
            assert_eq!(by_id[&ParticleId(1)].integration_offset_ns, 3 * old_step);
            assert_eq!(by_id[&ParticleId(1)].elapsed_age_ns, 3_000_000_000);
            assert_eq!(
                by_id[&ParticleId(2)].integration_offset_ns,
                2 * old_step + newborn_step
            );
            assert_eq!(by_id[&ParticleId(2)].elapsed_age_ns, 2_500_000_000);

            let values = log.lock().unwrap();
            let integration_calls = values
                .iter()
                .filter(|value| value.starts_with("integrator:"))
                .collect::<Vec<_>>();
            assert_eq!(integration_calls.len(), 4, "{integration_calls:?}");
            let birth_prefix = format!(
                "integrator:{}:{}:{newborn_step}:",
                birth_time.seconds_since_unix_epoch(),
                birth_time.nanosecond()
            );
            assert!(
                integration_calls
                    .iter()
                    .any(|value| value.starts_with(&birth_prefix) && value.ends_with("[2]")),
                "newborn must advance alone from its exact birth time: {integration_calls:?}"
            );
            assert!(
                integration_calls
                    .iter()
                    .all(|value| !(value.starts_with(&birth_prefix) && value.contains('1'))),
                "an interior birth must not re-advance the existing particle: {integration_calls:?}"
            );
        }
    }

    #[test]
    fn boundary_fraction_sets_matching_time_age_and_signed_offset() {
        for (direction, expected_time, expected_offset) in [
            (
                Direction::Forward,
                Timestamp::new(0, 250_000_000).unwrap(),
                250_000_000,
            ),
            (
                Direction::Backward,
                Timestamp::new(2, 750_000_000).unwrap(),
                -250_000_000,
            ),
        ] {
            let (mut runner, log) = runner(direction, false, false);
            runner.boundaries = vec![Box::new(FixedFractionTermination { fraction: 0.25 })];
            runner.boundary_sampler_factory = Some(Box::new(NoopBoundaryPathSamplerFactory));

            assert_eq!(runner.run(), Ok(RunOutcome::Complete));
            let state = runner.state.particles.state(0).unwrap();
            assert_eq!(state.integration_offset_ns, expected_offset);
            assert_eq!(state.elapsed_age_ns, 250_000_000);
            assert_eq!(
                state.termination,
                Some(ParticleTermination {
                    time: expected_time,
                    intersection_fraction: Some(0.25),
                })
            );
            assert_eq!(
                state.status,
                ParticleStatus::Terminated {
                    reason: TerminationReason::OutsideDomain
                }
            );
            let event = format!(
                "output.sample_count:{}:{}:1",
                expected_time.seconds_since_unix_epoch(),
                expected_time.nanosecond()
            );
            assert!(log.lock().unwrap().contains(&event));
        }
    }

    #[test]
    fn backward_run_keeps_negative_offset_and_nonnegative_age() {
        let (mut runner, log) = runner(Direction::Backward, false, false);
        assert_eq!(runner.run(), Ok(RunOutcome::Complete));
        assert_eq!(
            runner.state.particles.integration_offset_ns,
            vec![-3_000_000_000]
        );
        assert_eq!(runner.state.particles.elapsed_age_ns, vec![3_000_000_000]);
        let values = log.lock().unwrap();
        assert!(values.iter().any(|value| value == "output.sample:2"));
        assert!(values.iter().any(|value| value == "output.sample:1"));
    }

    #[test]
    fn abnormal_particle_does_not_abort_run_but_changes_outcome_and_manifest() {
        let (mut runner, _) = runner(Direction::Forward, true, false);
        assert_eq!(runner.run(), Ok(RunOutcome::CompletedWithParticleErrors));
        assert_eq!(
            runner.manifest.status,
            RunLifecycleStatus::CompletedWithParticleErrors
        );
        assert_eq!(runner.manifest.terminations.abnormal_count, 1);
        assert_eq!(
            runner
                .manifest
                .terminations
                .by_reason
                .get("numerical_failure"),
            Some(&1)
        );
    }

    #[test]
    fn fatal_output_error_persists_failed_manifest() {
        let (mut runner, log) = runner(Direction::Forward, false, true);
        assert!(matches!(runner.run(), Err(RunError::Output(_))));
        assert_eq!(runner.manifest.status, RunLifecycleStatus::Failed);
        assert_eq!(log.lock().unwrap().last().unwrap(), "manifest:Failed");
    }

    #[test]
    fn carrier_mass_tampering_fails_runner_and_persists_mass_conservation_code() {
        let output = tempfile::tempdir().unwrap();
        let manifest_path = output.path().join("run-manifest.json");
        let log = Arc::new(Mutex::new(Vec::new()));
        let (meteorology, query_plan) = engine_and_plan();
        let mut runner = SimulationRunner::from_components(SimulationComponents {
            state: SimulationState {
                clock: SimulationClock {
                    current: Timestamp::UNIX_EPOCH,
                    direction: Direction::Forward,
                },
                particles: ParticleBatch::default(),
                population_state: PopulationState::Uninitialized,
                numerical_step_index: 0,
                output_event_index: 0,
            },
            meteorology,
            query_plan,
            execution: Box::new(RayonExecutionContext { worker_threads: 1 }),
            population: Box::new(MassCheckingPopulation {
                state: PopulationState::Uninitialized,
                expected_carrier_mass_kg: 1.0,
            }),
            physics: None,
            integrator: Box::new(DryAirMassTamperingIntegrator),
            boundaries: Vec::new(),
            boundary_sampler_factory: None,
            outputs: vec![ScheduledOutputProduct {
                product: Box::new(LoggingOutput {
                    log,
                    fail_begin: false,
                    fail_contribute: false,
                }),
                scheduler: OutputScheduler::default(),
                meteorology_plan: None,
            }],
            manifest: manifest(Direction::Forward),
            manifest_store: Box::new(crate::manifest_store::AtomicRunManifestStore::new(
                manifest_path.clone(),
            )),
            lifecycle_clock: Box::new(FixedLifecycleClock),
            time_step: SignedDuration(1_000_000_000),
            end_time: Timestamp::new(1, 0).unwrap(),
            random_seed: 7,
            domain: None,
        })
        .unwrap();

        assert_eq!(runner.run(), Err(RunError::MassConservation));
        assert_eq!(runner.manifest.status, RunLifecycleStatus::Failed);
        assert_eq!(
            runner
                .manifest
                .failure
                .as_ref()
                .map(|failure| failure.code.as_str()),
            Some("run.mass_conservation")
        );
        runner.manifest.validate().unwrap();

        let persisted: RunManifest =
            serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
        assert_eq!(persisted.status, RunLifecycleStatus::Failed);
        assert_eq!(
            persisted
                .failure
                .as_ref()
                .map(|failure| failure.code.as_str()),
            Some("run.mass_conservation")
        );
        persisted.validate().unwrap();
    }

    #[test]
    fn contribute_manifest_failure_yields_legal_failed_manifest() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let store = FailOnNthManifestStore {
            log: log.clone(),
            n: usize::MAX, // never fail store
            count: 0,
            last: None,
        };
        let (mut runner, _) = runner_with_flags(
            Direction::Forward,
            false,
            false,
            true, // fail contribute
            Some(Box::new(store)),
        );
        let err = runner.run().expect_err("contribute must fail run");
        assert!(format!("{err:?}").contains("contribute") || format!("{err:?}").contains("forced"));
        assert_eq!(runner.manifest.status, RunLifecycleStatus::Failed);
        assert!(runner.manifest.finished_at.is_some());
        assert!(runner.manifest.failure.is_some());
        assert!(runner.manifest.provenance.is_none());
        // Failed manifest must validate.
        runner.manifest.validate().expect("Failed lifecycle legal");
        let values = log.lock().unwrap();
        assert!(
            values.iter().any(|v| v.contains("manifest:Failed")),
            "store must receive Failed: {values:?}"
        );
    }

    #[test]
    fn terminal_persist_failure_quarantines_and_keeps_failed_legal() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let store = FailOnNthManifestStore {
            log: log.clone(),
            n: 2, // running ok, terminal fail
            count: 0,
            last: None,
        };
        let (mut runner, _) = runner_with_flags(
            Direction::Forward,
            false,
            false,
            false,
            Some(Box::new(store)),
        );
        let err = runner.run().expect_err("terminal persist must fail");
        assert!(
            format!("{err:?}").contains("persist") || format!("{err:?}").contains("forced"),
            "{err:?}"
        );
        assert_eq!(runner.manifest.status, RunLifecycleStatus::Failed);
        assert!(runner.manifest.finished_at.is_some());
        assert!(runner.manifest.failure.is_some());
        assert!(runner.manifest.provenance.is_none());
        runner
            .manifest
            .validate()
            .expect("Failed legal after terminal persist fail");
    }
}
