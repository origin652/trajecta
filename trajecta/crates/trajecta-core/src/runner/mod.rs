//! # Contract: complete simulation orchestration
//!
//! The runner owns the clock, particles, meteorology engine, population,
//! integrator, boundaries, and products. Its main loop respects every event
//! boundary and keeps all strategy calls in the declared lifecycle order.

mod builder;
pub use builder::{
    RunnerBuildKnobs, build_runner, build_runner_with_knobs, build_runner_with_manifest_store,
    build_runner_with_store_and_knobs,
};

use std::collections::BTreeSet;

use trajecta_case::document::{ResolvedCase, ResolvedRunProfile};
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::time::{Direction, Timestamp};
use trajecta_met::query::engine::{BatchWorkspace, ExecutionContext, MetEngine};
use trajecta_met::query::output::QueryOutput;
use trajecta_met::query::request::{
    QueryBatch, QueryPlan, QueryPointArrays, TransportPlan, VerticalQuery,
};

use crate::boundary::{
    BoundaryContext, BoundaryDecision, BoundaryError, BoundaryPathSampler, BoundaryPolicy,
};
use crate::clock::{
    ClockError, SignedDuration, SimulationClock, StepBoundary, StepPlanner, add_timestamp,
};
use crate::integrator::{IntegratorContext, IntegratorInput, IntegratorModel};
use crate::manifest::{RunFailure, RunLifecycleStatus, RunManifest, TerminationSummary};
use crate::output::{OutputProduct, OutputScheduler};
use crate::particle::{ParticleBatch, ParticleState, ParticleStatus, TerminationClass};
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

/// Constructs an exact path sampler for one particle proposal.
pub trait BoundaryPathSamplerFactory: Send {
    /// Returns an owned or runtime-borrowing sampler for one full step.
    fn build<'a>(
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
        if self.manifest.status != RunLifecycleStatus::Running {
            return Err(RunError::InvalidConfiguration(
                "a runner can execute only once from running state".into(),
            ));
        }
        self.persist_manifest()?;
        let result = self.run_inner();
        match result {
            Ok(()) => self.finalize_success(),
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

    fn run_inner(&mut self) -> Result<(), RunError> {
        for output in &mut self.outputs {
            output
                .product
                .begin(&self.manifest)
                .map_err(|error| RunError::Output(format!("{error:?}")))?;
        }

        {
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
        }
        self.sync_population_state();
        self.emit_current_particles()?;
        self.sample_outputs(self.state.clock.current, true)?;

        while self.state.clock.current != self.end_time {
            let boundaries = self.collect_step_boundaries()?;
            let planned = StepPlanner::plan(self.state.clock, self.time_step, &boundaries)
                .map_err(RunError::Clock)?;
            let step = *planned.first().ok_or_else(|| {
                RunError::InvalidConfiguration("step planner made no progress before end".into())
            })?;
            let end_time =
                add_timestamp(self.state.clock.current, step).map_err(RunError::Clock)?;

            {
                let mut context = PopulationContext {
                    time: self.state.clock.current,
                    direction: self.state.clock.direction,
                    step: Some(step),
                    step_index: Some(self.state.numerical_step_index),
                    random_seed: self.random_seed,
                    meteorology: &mut self.meteorology,
                    execution: self.execution.as_ref(),
                    domain: self.domain.clone(),
                };
                self.population
                    .before_step(&mut context, &self.state.particles)
                    .map_err(map_population_error)?;
            }

            let start_particles = self.state.particles.clone();
            let step_result = self
                .integrator
                .advance(
                    IntegratorInput {
                        particles: &start_particles,
                        time: self.state.clock.current,
                        step,
                    },
                    &mut IntegratorContext {
                        meteorology: &mut self.meteorology,
                        query_plan: &self.query_plan,
                        execution: self.execution.as_ref(),
                        domain: self.domain.as_ref(),
                    },
                )
                .map_err(|error| RunError::Integration(format!("{error:?}")))?;
            let previously_terminated = count_terminated(&start_particles);
            let bounded = self.apply_boundaries(
                self.state.clock.current,
                end_time,
                &start_particles,
                step_result.particles,
            )?;
            self.state.particles = bounded;
            self.state.clock.current = end_time;

            {
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
                    .after_advection(&mut context, &self.state.particles)
                    .map_err(map_population_error)?;
                self.population
                    .apply_boundary_maintenance(&mut context, &mut self.state.particles)
                    .map_err(map_population_error)?;
            }
            self.state.numerical_step_index = self
                .state
                .numerical_step_index
                .checked_add(1)
                .ok_or(RunError::ResourceLimit)?;
            self.sync_population_state();
            let emitted = self.emit_current_particles()?;
            let newly_terminated =
                count_terminated(&self.state.particles).saturating_sub(previously_terminated);
            self.sample_outputs(
                end_time,
                emitted > 0 || newly_terminated > 0 || end_time == self.end_time,
            )?;
        }

        {
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
        Ok(())
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
        let emitted = {
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
        let mut boundaries = self
            .population
            .step_boundaries(self.state.clock, self.time_step)
            .map_err(map_population_error)?;
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
        let statically_capped = *StepPlanner::plan(self.state.clock, self.time_step, &boundaries)
            .map_err(RunError::Clock)?
            .first()
            .ok_or_else(|| {
                RunError::InvalidConfiguration(
                    "static step-boundary planning made no progress before end".into(),
                )
            })?;
        let dynamic = {
            let mut context = PopulationContext {
                time: self.state.clock.current,
                direction: self.state.clock.direction,
                step: Some(statically_capped),
                step_index: Some(self.state.numerical_step_index),
                random_seed: self.random_seed,
                meteorology: &mut self.meteorology,
                execution: self.execution.as_ref(),
                domain: self.domain.clone(),
            };
            self.population
                .dynamic_step_boundaries(&mut context, statically_capped)
                .map_err(map_population_error)?
        };
        boundaries.extend(dynamic);
        order_step_boundaries(&mut boundaries, self.state.clock.direction);
        Ok(boundaries)
    }

    fn apply_boundaries(
        &mut self,
        start_time: Timestamp,
        end_time: Timestamp,
        start_particles: &ParticleBatch,
        mut proposed_particles: ParticleBatch,
    ) -> Result<ParticleBatch, RunError> {
        proposed_particles
            .validate()
            .map_err(|_| RunError::Integration("invalid proposal batch".into()))?;
        if self.boundaries.is_empty() {
            return Ok(proposed_particles);
        }
        let factory = self
            .boundary_sampler_factory
            .as_mut()
            .ok_or_else(|| RunError::InvalidConfiguration("missing boundary sampler".into()))?;
        let len = proposed_particles
            .len()
            .map_err(|_| RunError::Integration("invalid proposal batch".into()))?;
        for index in 0..len {
            let start = start_particles
                .state(index)
                .map_err(|_| RunError::Integration("invalid start batch".into()))?;
            let mut proposed = proposed_particles
                .state(index)
                .map_err(|_| RunError::Integration("invalid proposal batch".into()))?;
            if start.status != ParticleStatus::Alive || proposed.status != ParticleStatus::Alive {
                continue;
            }
            {
                let mut path = factory
                    .build(
                        BoundarySamplerRequest {
                            start_time,
                            end_time,
                            start: &start,
                            proposed: &proposed,
                            domain: self.domain.as_ref(),
                        },
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
                    if let BoundaryDecision::Terminated { reason, .. } = decision {
                        if proposed.status == ParticleStatus::Alive {
                            proposed.status = ParticleStatus::Terminated { reason };
                        }
                        break;
                    }
                }
            }
            proposed_particles
                .set_state(index, proposed)
                .map_err(|_| RunError::Integration("invalid bounded particle".into()))?;
        }
        Ok(proposed_particles)
    }

    fn sample_outputs(&mut self, time: Timestamp, force_endpoints: bool) -> Result<(), RunError> {
        let mut sampled_any = false;
        for index in 0..self.outputs.len() {
            let scheduled = self.outputs[index].scheduler.sample_times.contains(&time);
            if force_endpoints || scheduled {
                let plan = self.outputs[index].meteorology_plan.clone();
                let meteorology = plan
                    .as_ref()
                    .map(|plan| self.query_output_meteorology(time, plan))
                    .transpose()?;
                self.outputs[index]
                    .product
                    .sample(time, &self.state.particles, meteorology.as_ref())
                    .map_err(|error| RunError::Output(format!("{error:?}")))?;
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
        plan: &QueryPlan,
    ) -> Result<QueryOutput, RunError> {
        let domain = self.domain.as_ref().ok_or_else(|| {
            RunError::Meteorology("output query requires an explicit meteorology domain".into())
        })?;
        let window = self
            .meteorology
            .prepare_for_domain(time, domain)
            .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;
        let mut workspace = BatchWorkspace::default();
        window
            .prepare_batch(
                plan,
                QueryBatch {
                    vertical_coordinate: VerticalQuery::AboveSeaLevel,
                    points: QueryPointArrays {
                        longitude_degrees: self.state.particles.longitude_degrees.clone(),
                        latitude_degrees: self.state.particles.latitude_degrees.clone(),
                        vertical: self.state.particles.height_asl_m.clone(),
                    },
                },
                &mut workspace,
            )
            .map_err(|error| RunError::Meteorology(format!("{error:?}")))?
            .execute(self.execution.as_ref(), &mut workspace)
            .map_err(|error| RunError::Meteorology(format!("{error:?}")))
    }

    fn sync_population_state(&mut self) {
        self.state.population_state = self.population.snapshot_state();
    }

    fn finalize_success(&mut self) -> Result<RunOutcome, RunError> {
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
            let outcome = if self.manifest.terminations.abnormal_count == 0 {
                self.manifest.status = RunLifecycleStatus::Complete;
                RunOutcome::Complete
            } else {
                self.manifest.status = RunLifecycleStatus::CompletedWithParticleErrors;
                RunOutcome::CompletedWithParticleErrors
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
        self.manifest
            .validate()
            .map_err(|error| RunError::Manifest(format!("{error:?}")))?;
        self.manifest_store
            .persist(&self.manifest)
            .map_err(RunError::Manifest)
    }
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

fn count_terminated(particles: &ParticleBatch) -> usize {
    particles
        .status
        .iter()
        .filter(|status| matches!(status, ParticleStatus::Terminated { .. }))
        .count()
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

/// Successful run-level outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// All run-level work completed and no particle terminated abnormally.
    Complete,
    /// Run-level work completed but one or more particles terminated abnormally.
    CompletedWithParticleErrors,
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
    /// A boundary sampler or policy failed structurally.
    Boundary(BoundaryError),
    /// Clock planning or timestamp arithmetic failed.
    Clock(ClockError),
    /// Output product or encoder failed.
    Output(String),
    /// Running/final manifest validation or persistence failed.
    Manifest(String),
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
            Self::Boundary(_) => "run.boundary",
            Self::Clock(_) => "run.clock",
            Self::Output(_) => "run.output",
            Self::Manifest(_) => "run.manifest",
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
    use crate::integrator::{IntegratorError, StepResult};
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

    fn one_particle() -> ParticleBatch {
        ParticleBatch {
            id: vec![ParticleId(1)],
            population_id: vec![PopulationId("p".into())],
            origin: vec![ParticleOrigin::Release {
                event_id: ReleaseEventId("e".into()),
            }],
            birth_time: vec![Timestamp::UNIX_EPOCH],
            longitude_degrees: vec![0.0],
            latitude_degrees: vec![0.0],
            height_asl_m: vec![1_000.0],
            integration_offset_ns: vec![0],
            elapsed_age_ns: vec![0],
            dry_air_mass_kg: vec![0.0],
            sensitivity_weight: vec![None],
            status: vec![ParticleStatus::Alive],
            mass: SubstanceMassStore::default(),
        }
    }

    struct LoggingPopulation {
        log: Arc<Mutex<Vec<String>>>,
        state: PopulationState,
        initialized_particle: bool,
    }

    impl PopulationStrategy for LoggingPopulation {
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
                format!("integrator:{}", input.time.seconds_since_unix_epoch()),
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
            _particles: &ParticleBatch,
            _meteorology: Option<&trajecta_met::query::output::QueryOutput>,
        ) -> Result<(), OutputError> {
            log(
                &self.log,
                format!("output.sample:{}", time.seconds_since_unix_epoch()),
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
            }),
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
