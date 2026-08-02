//! # Contract: production RunnerBuilder registry and wiring
//!
//! Resolves integrator/boundary/population/output IDs, creates run directories,
//! UUID-v7 run IDs, seed/manifest bootstrap, and assembles SimulationRunner.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use trajecta_case::document::{MeteorologyReaderBackend, ResolvedCase, ResolvedRunProfile};
use trajecta_case::lockfile::{parse_dataset_lock_json, parse_dataset_lock_yaml};
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::output::{OutputSchedule, PARTICLE_STATE_PRODUCT_ID};
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::population::{
    GeoJsonSource, ParticlePopulationSpec, ReleaseDrivenSpec, ReleaseEventSpec,
};
use trajecta_case::model::time::{Direction, Timestamp};
use trajecta_met::field::{Capability, CapabilitySet, FieldRegistry};
use trajecta_met::io::frame_loader::{FrameLoadRequest, FrameLoader};
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder, MetCatalog};
use trajecta_met::profile::document::{
    DatasetProfileDocument, FieldReference, ProfileCatalog, ProfileName,
};
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::query::cache::MemoryBudget;
use trajecta_met::query::engine::{MetEngine, MetEngineConfig, RayonExecutionContext};
use trajecta_met::query::request::{ExplainMode, QueryPlanRequest, TransportPlanRequest};
use trajecta_met::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerRegistry};
use uuid::Uuid;

use crate::boundary::{
    GlobalPeriodicBoundary, LimitedDomainTerminate, MetBoundaryPathSamplerFactory,
    ModelTopTerminate, SurfaceReflect,
};
use crate::clock::SignedDuration;
use crate::integrator::Rk2Spherical;
use crate::lifecycle_clock::SystemLifecycleClock;
use crate::manifest::{
    ExecutionSummary, GeometryIdentity, InputIdentity, JobSeriesId, NumericalSummary, RunId,
    RunManifest, RunManifestStart, SoftwareIdentity,
};
use crate::manifest_store::AtomicRunManifestStore;
use crate::output::sqlite::ParticleStateSqliteSink;
use crate::output::{OutputScheduler, ParticleStateProduct};
use crate::population::{
    DirectAslReleaseResolver, DomainFillAirMass, DomainFillStratosphericOzone,
    MetReleaseVerticalResolver, OzoneAssignmentRuleRegistry, ReleaseDrivenPopulation,
};
use crate::release::geometry::{SphericalGeometrySampler, canonicalize_geometry};
use crate::release::vertical::SpecVerticalSampler;
use crate::release::{ReleaseEvent, ReleaseSchedule};
use crate::runner::{
    RunError, RunnerBuilder, ScheduledOutputProduct, SimulationComponents, SimulationRunner,
    SimulationState,
};
use crate::science::{
    GLOBAL_PERIODIC_ID, LIMITED_DOMAIN_TERMINATE_ID, M4_CONSTANTS, MODEL_TOP_TERMINATE_ID,
    RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};

impl RunnerBuilder {
    /// Constructs a runnable simulation from a resolved Case and RunProfile.
    pub fn build(self) -> Result<SimulationRunner, RunError> {
        build_runner(self.case, self.run_profile, None)
    }
}

/// Optional production-path knobs for determinism / fault harnesses.
#[derive(Clone, Debug, Default)]
pub struct RunnerBuildKnobs {
    /// Override provenance external-sort chunk lines.
    pub bundle_chunk_lines: Option<usize>,
    /// Override provenance merge fan-in.
    pub bundle_merge_fan_in: Option<usize>,
    /// Reverse particle index scan when writing samples.
    pub reverse_particle_scan: bool,
}

/// Explicit durable job identity supplied by the M5 worker control plane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerAttemptIdentity {
    /// Logical series shared by future rerun attempts.
    pub job_series_id: JobSeriesId,
    /// Unique UUID-v7 identity of this attempt.
    pub run_id: RunId,
    /// One-based attempt number.
    pub attempt: u32,
}

/// Builds a runner, optionally injecting a prebuilt meteorology stack used by
/// synthetic hard-gate fixtures. Production calls use [`RunnerBuilder::build`].
pub fn build_runner(
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
    synthetic: Option<crate::synthetic::SyntheticStack>,
) -> Result<SimulationRunner, RunError> {
    build_runner_inner(
        case,
        run_profile,
        synthetic,
        None,
        None,
        RunnerBuildKnobs::default(),
    )
}

/// Builds a production runner whose manifest and output path use a durable job attempt identity.
pub fn build_runner_for_attempt(
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
    identity: RunnerAttemptIdentity,
) -> Result<SimulationRunner, RunError> {
    validate_attempt_identity(&identity)?;
    build_runner_inner(
        case,
        run_profile,
        None,
        Some(identity),
        None,
        RunnerBuildKnobs::default(),
    )
}

fn build_runner_inner(
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
    synthetic: Option<crate::synthetic::SyntheticStack>,
    attempt_identity: Option<RunnerAttemptIdentity>,
    manifest_store: Option<Box<dyn crate::runner::RunManifestStore>>,
    knobs: RunnerBuildKnobs,
) -> Result<SimulationRunner, RunError> {
    let time = case
        .time
        .ok_or_else(|| RunError::InvalidConfiguration("case.time is required".into()))?;
    let numerics = case
        .numerics
        .clone()
        .ok_or_else(|| RunError::InvalidConfiguration("case.numerics is required".into()))?;
    let population_spec = case.particle_population.clone().ok_or_else(|| {
        RunError::InvalidConfiguration("case.particle_population is required".into())
    })?;

    let integrator_id = numerics.integrator.model.0.as_str();
    if integrator_id != RK2_SPHERICAL_ID {
        return Err(RunError::InvalidConfiguration(format!(
            "unknown integrator '{integrator_id}'"
        )));
    }

    let mut boundaries = Vec::new();
    let mut seen = BTreeMap::new();
    for policy in &numerics.boundaries.policies {
        if seen.insert(policy.0.clone(), ()).is_some() {
            return Err(RunError::InvalidConfiguration(format!(
                "duplicate boundary policy '{}'",
                policy.0
            )));
        }
        match policy.0.as_str() {
            SURFACE_REFLECT_ID => boundaries.push(Box::new(SurfaceReflect) as _),
            MODEL_TOP_TERMINATE_ID => boundaries.push(Box::new(ModelTopTerminate) as _),
            LIMITED_DOMAIN_TERMINATE_ID => boundaries.push(Box::new(LimitedDomainTerminate) as _),
            GLOBAL_PERIODIC_ID => boundaries.push(Box::new(GlobalPeriodicBoundary) as _),
            other => {
                return Err(RunError::InvalidConfiguration(format!(
                    "unknown boundary policy '{other}'"
                )));
            }
        }
    }

    let seed = numerics.random_seed.unwrap_or_else(generate_seed);
    let run_id = attempt_identity.as_ref().map_or_else(
        || RunId(Uuid::now_v7().to_string()),
        |value| value.run_id.clone(),
    );
    let run_dir = unique_run_directory(&run_profile.output_root, &case.metadata.name, &run_id)?;
    fs::create_dir_all(&run_dir).map_err(|error| RunError::Manifest(error.to_string()))?;
    let manifest_path = run_dir.join("run-manifest.json");
    let sqlite_path = run_dir.join("particles.sqlite");

    let case_dir = run_profile
        .case_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let (schedule, geometries) = match &population_spec {
        ParticlePopulationSpec::ReleaseDriven(spec) => {
            build_release_schedule(spec, Some(case_dir.as_path()))?
        }
        ParticlePopulationSpec::DomainFillAirMass(_)
        | ParticlePopulationSpec::DomainFillStratosphericOzone(_) => {
            (ReleaseSchedule::default(), Vec::new())
        }
    };
    let required_capabilities = required_capabilities_for_population(&population_spec)?;
    let runtime_domain = resolve_runtime_domain(
        &case,
        &population_spec,
        synthetic.as_ref().map(|stack| &stack.domain),
    )?;
    let is_domain_fill = matches!(
        &population_spec,
        ParticlePopulationSpec::DomainFillAirMass(_)
            | ParticlePopulationSpec::DomainFillStratosphericOzone(_)
    );
    let ozone_rule_id = match &population_spec {
        ParticlePopulationSpec::DomainFillStratosphericOzone(specification) => {
            Some(specification.ozone_rule.clone())
        }
        ParticlePopulationSpec::ReleaseDriven(_) | ParticlePopulationSpec::DomainFillAirMass(_) => {
            None
        }
    };

    let population: Box<dyn crate::population::PopulationStrategy> = match population_spec {
        ParticlePopulationSpec::ReleaseDriven(release_spec) => {
            if release_spec.id.0.trim().is_empty() {
                return Err(RunError::InvalidConfiguration("population id empty".into()));
            }
            let geometry_sampler: Box<dyn crate::release::GeometrySampler> =
                Box::new(MultiEventGeometrySampler::from_schedule(&schedule)?);
            let vertical_sampler = Box::new(SpecVerticalSampler);
            let needs_met_vertical = schedule.events.iter().any(|event| {
                !matches!(
                    event.vertical,
                    trajecta_case::model::population::ReleaseVerticalSpec::AboveSeaLevel { .. }
                )
            });
            let vertical_resolver: Box<dyn crate::population::ReleaseVerticalResolver> =
                if needs_met_vertical {
                    Box::new(MetReleaseVerticalResolver)
                } else {
                    Box::new(DirectAslReleaseResolver)
                };
            Box::new(ReleaseDrivenPopulation::new(
                release_spec,
                schedule.clone(),
                geometry_sampler,
                vertical_sampler,
                vertical_resolver,
            ))
        }
        ParticlePopulationSpec::DomainFillAirMass(specification) => {
            if specification.id.0.trim().is_empty() || specification.domain_id != runtime_domain {
                return Err(RunError::InvalidConfiguration(
                    "domain-fill population identity or selected domain is invalid".into(),
                ));
            }
            Box::new(DomainFillAirMass::new(specification))
        }
        ParticlePopulationSpec::DomainFillStratosphericOzone(specification) => {
            if specification.air_mass.id.0.trim().is_empty()
                || specification.air_mass.domain_id != runtime_domain
            {
                return Err(RunError::InvalidConfiguration(
                    "ozone domain-fill population identity or selected domain is invalid".into(),
                ));
            }
            let registry = OzoneAssignmentRuleRegistry::builtins();
            let rule = registry.resolve(&specification.ozone_rule).ok_or_else(|| {
                RunError::InvalidConfiguration(format!(
                    "unknown ozone assignment rule '{}'",
                    specification.ozone_rule
                ))
            })?;
            Box::new(
                DomainFillStratosphericOzone::new(specification, rule)
                    .map_err(|error| RunError::Population(error.code().into()))?,
            )
        }
    };
    let population_model_id = population.model_id().to_string();

    let (
        meteorology,
        query_plan,
        execution,
        domain,
        dataset_lock_sha256,
        dataset_profile_sha256,
        dataset_content_sha256,
        reader_backends,
    ) = if let Some(stack) = synthetic {
        (
            stack.engine,
            stack.transport_plan,
            stack.execution,
            Some(stack.domain),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
    } else {
        let query_times = collect_meteorology_query_times(&case, &time, &schedule, &numerics)?;
        let loaded = load_production_meteorology(
            &case,
            &run_profile,
            &time,
            &query_times,
            &runtime_domain,
            required_capabilities,
        )?;
        (
            loaded.engine,
            loaded.transport_plan,
            loaded.execution,
            loaded.selected_domain,
            loaded.dataset_lock_sha256,
            loaded.dataset_profile_sha256,
            loaded.dataset_content_sha256,
            loaded.reader_backends,
        )
    };

    let canonical_case = case_with_canonical_geometries(&case, &schedule)?;
    write_resolved_documents(&run_dir, &canonical_case, &run_profile)?;

    let time_step_ns = (numerics.time_step.value_si() * 1.0e9).round() as i64;
    if time_step_ns <= 0 {
        return Err(RunError::InvalidConfiguration(
            "time_step must be positive".into(),
        ));
    }
    let signed_step = match time.direction {
        Direction::Forward => SignedDuration(time_step_ns),
        Direction::Backward => SignedDuration(-time_step_ns),
    };

    let outputs = build_outputs(&case, &time, &sqlite_path, &meteorology, &knobs)?;
    let mut numerical_tolerances = BTreeMap::from([
        (
            "rk2_minimum_convergence_order".into(),
            crate::science::RK2_MINIMUM_CONVERGENCE_ORDER,
        ),
        (
            "geometry_area_relative_tolerance".into(),
            crate::science::GEOMETRY_AREA_RELATIVE_TOLERANCE,
        ),
    ]);
    if is_domain_fill {
        numerical_tolerances.insert(
            "domain_fill_step_relative".into(),
            crate::science::MASS_BALANCE_STEP_RELATIVE_TOLERANCE,
        );
        numerical_tolerances.insert(
            "domain_fill_final_relative".into(),
            crate::science::MASS_BALANCE_FINAL_RELATIVE_TOLERANCE,
        );
        numerical_tolerances.insert(
            "domain_fill_ulp_floor".into(),
            f64::from(crate::science::MASS_BALANCE_ULP_FLOOR),
        );
    }

    let manifest_start = RunManifestStart {
        run_id: run_id.clone(),
        case_name: sanitize_case_name(&case.metadata.name)?,
        started_at: Timestamp::UNIX_EPOCH, // overwritten below after lifecycle clock
        software: SoftwareIdentity {
            crate_versions: BTreeMap::from([
                ("trajecta-core".into(), env!("CARGO_PKG_VERSION").into()),
                ("trajecta-case".into(), env!("CARGO_PKG_VERSION").into()),
                ("trajecta-met".into(), env!("CARGO_PKG_VERSION").into()),
            ]),
            git_commit: None,
        },
        inputs: InputIdentity {
            case_sha256: hash_json(&canonical_case)?,
            run_profile_sha256: hash_json(&run_profile)?,
            dataset_lock_sha256,
            dataset_profile_sha256,
            dataset_content_sha256,
        },
        execution: ExecutionSummary {
            worker_threads: run_profile.execution.worker_threads.max(1),
            memory_budget_bytes: run_profile.execution.memory_budget_bytes,
            executor: "rayon".into(),
            reader_backends,
            wall_time_ns: None,
            peak_rss_bytes: None,
            io_counters: BTreeMap::new(),
        },
        numerical: NumericalSummary {
            random_seed: seed,
            integrator: RK2_SPHERICAL_ID.into(),
            boundary_policies: numerics
                .boundaries
                .policies
                .iter()
                .map(|policy| policy.0.clone())
                .collect(),
            population: population_model_id,
            ozone_rule: ozone_rule_id,
            particle_state_sink: trajecta_case::model::output::PARTICLE_STATE_SQLITE_SINK_ID.into(),
            tolerance_registry: "trajecta.m4.numerical-contract/v1".into(),
            tolerances: numerical_tolerances,
            deterministic: true,
        },
        geometries,
        effective_outputs: case.outputs.clone(),
    };
    let manifest = match attempt_identity {
        Some(identity) => {
            RunManifest::running_in_series(manifest_start, identity.job_series_id, identity.attempt)
        }
        None => RunManifest::running(manifest_start),
    };

    let mut lifecycle_clock = SystemLifecycleClock;
    let started_at =
        <SystemLifecycleClock as crate::runner::LifecycleClock>::now(&mut lifecycle_clock)
            .map_err(RunError::Manifest)?;
    let mut manifest = manifest;
    manifest.started_at = started_at;

    let state = SimulationState {
        clock: crate::clock::SimulationClock {
            current: time.start,
            direction: time.direction,
        },
        particles: crate::particle::ParticleBatch::default(),
        population_state: crate::population::PopulationState::Uninitialized,
        numerical_step_index: 0,
        output_event_index: 0,
    };

    let components = SimulationComponents {
        state,
        meteorology,
        query_plan,
        execution,
        population,
        integrator: Box::new(Rk2Spherical),
        boundaries,
        boundary_sampler_factory: Some(Box::new(MetBoundaryPathSamplerFactory::default())),
        outputs,
        manifest,
        manifest_store: manifest_store
            .unwrap_or_else(|| Box::new(AtomicRunManifestStore::new(manifest_path))),
        lifecycle_clock: Box::new(lifecycle_clock),
        time_step: signed_step,
        end_time: time.end,
        random_seed: seed,
        domain,
    };
    SimulationRunner::from_components(components)
}

/// Test/harness entry: production builder with an injected manifest store.
pub fn build_runner_with_manifest_store(
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
    synthetic: Option<crate::synthetic::SyntheticStack>,
    manifest_store: Box<dyn crate::runner::RunManifestStore>,
) -> Result<SimulationRunner, RunError> {
    build_runner_inner(
        case,
        run_profile,
        synthetic,
        None,
        Some(manifest_store),
        RunnerBuildKnobs::default(),
    )
}

/// Test/harness entry: production builder with sort/order knobs.
pub fn build_runner_with_knobs(
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
    synthetic: Option<crate::synthetic::SyntheticStack>,
    knobs: RunnerBuildKnobs,
) -> Result<SimulationRunner, RunError> {
    build_runner_inner(case, run_profile, synthetic, None, None, knobs)
}

/// Test/harness entry: store + knobs.
pub fn build_runner_with_store_and_knobs(
    case: ResolvedCase,
    run_profile: ResolvedRunProfile,
    synthetic: Option<crate::synthetic::SyntheticStack>,
    manifest_store: Box<dyn crate::runner::RunManifestStore>,
    knobs: RunnerBuildKnobs,
) -> Result<SimulationRunner, RunError> {
    build_runner_inner(
        case,
        run_profile,
        synthetic,
        None,
        Some(manifest_store),
        knobs,
    )
}

/// Returns the exact meteorological capability set required by a population strategy.
///
/// Project data planning must call this helper rather than duplicating the
/// RunnerBuilder population-to-capability mapping.
pub fn required_capabilities_for_population(
    population: &ParticlePopulationSpec,
) -> Result<CapabilitySet, RunError> {
    let base = CapabilitySet::new()
        .with(Capability::Transport)
        .with(Capability::NearSurfaceTransport);
    match population {
        ParticlePopulationSpec::ReleaseDriven(_) => Ok(base),
        ParticlePopulationSpec::DomainFillAirMass(_) => Ok(base.with(Capability::DomainFill)),
        ParticlePopulationSpec::DomainFillStratosphericOzone(_) => Ok(base
            .with(Capability::DomainFill)
            .with(Capability::Diagnostics)),
    }
}

fn resolve_runtime_domain(
    case: &ResolvedCase,
    population: &ParticlePopulationSpec,
    synthetic_domain: Option<&DomainId>,
) -> Result<DomainId, RunError> {
    if let Some(domain) = synthetic_domain {
        let population_domain = match population {
            ParticlePopulationSpec::DomainFillAirMass(specification) => {
                Some(&specification.domain_id)
            }
            ParticlePopulationSpec::DomainFillStratosphericOzone(specification) => {
                Some(&specification.air_mass.domain_id)
            }
            ParticlePopulationSpec::ReleaseDriven(_) => None,
        };
        if let Some(population_domain) = population_domain.filter(|candidate| *candidate != domain)
        {
            return Err(RunError::InvalidConfiguration(format!(
                "domain-fill domain '{}' does not match synthetic domain '{}'",
                population_domain.0, domain.0
            )));
        }
        return Ok(domain.clone());
    }

    let meteorology = case.meteorology.as_ref().ok_or_else(|| {
        RunError::InvalidConfiguration(
            "case.meteorology is required for production RunnerBuilder::build".into(),
        )
    })?;
    match population {
        ParticlePopulationSpec::ReleaseDriven(_) => {
            let [domain] = meteorology.domains.as_slice() else {
                return Err(RunError::InvalidConfiguration(
                    "release-driven M4 runner currently requires exactly one explicit meteorology domain"
                        .into(),
                ));
            };
            Ok(domain.id.clone())
        }
        ParticlePopulationSpec::DomainFillAirMass(specification) => {
            if meteorology
                .domains
                .iter()
                .any(|domain| domain.id == specification.domain_id)
            {
                Ok(specification.domain_id.clone())
            } else {
                Err(RunError::InvalidConfiguration(format!(
                    "domain-fill domain '{}' is not declared by case.meteorology",
                    specification.domain_id.0
                )))
            }
        }
        ParticlePopulationSpec::DomainFillStratosphericOzone(specification) => {
            if meteorology
                .domains
                .iter()
                .any(|domain| domain.id == specification.air_mass.domain_id)
            {
                Ok(specification.air_mass.domain_id.clone())
            } else {
                Err(RunError::InvalidConfiguration(format!(
                    "ozone domain-fill domain '{}' is not declared by case.meteorology",
                    specification.air_mass.domain_id.0
                )))
            }
        }
    }
}

fn build_release_schedule(
    spec: &ReleaseDrivenSpec,
    case_dir: Option<&std::path::Path>,
) -> Result<(ReleaseSchedule, Vec<GeometryIdentity>), RunError> {
    let mut events = Vec::new();
    let mut geometries = Vec::new();
    for event in &spec.events {
        let (geometry, identity) = resolve_event_geometry(event, case_dir)?;
        geometries.push(identity);
        let mass_kg = event
            .mass
            .iter()
            .map(|(id, quantity)| (id.clone(), quantity.value_si()))
            .collect();
        events.push(ReleaseEvent {
            id: event.id.clone(),
            start: event.start,
            end: event.end,
            geometry,
            vertical: event.vertical.clone(),
            particle_count: event.particle_count,
            mass_kg,
        });
    }
    let schedule = ReleaseSchedule { events };
    schedule
        .validate()
        .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;
    Ok((schedule, geometries))
}

fn resolve_event_geometry(
    event: &ReleaseEventSpec,
    case_dir: Option<&std::path::Path>,
) -> Result<
    (
        trajecta_case::model::population::GeoJsonGeometry,
        GeometryIdentity,
    ),
    RunError,
> {
    match &event.geometry {
        GeoJsonSource::Inline { geometry } => {
            let canonical = canonicalize_geometry(&event.id.0, geometry, None)
                .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;
            Ok((canonical.geometry, canonical.identity))
        }
        GeoJsonSource::File { path } => {
            let resolved = if path.is_absolute() {
                path.clone()
            } else if let Some(base) = case_dir {
                base.join(path)
            } else {
                path.clone()
            };
            let text = fs::read_to_string(&resolved)
                .map_err(|error| RunError::InvalidConfiguration(error.to_string()))?;
            let geometry = parse_geojson_geometry(&text)?;
            let canonical = canonicalize_geometry(&event.id.0, &geometry, Some(resolved.as_path()))
                .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;
            Ok((canonical.geometry, canonical.identity))
        }
    }
}

fn parse_geojson_geometry(
    text: &str,
) -> Result<trajecta_case::model::population::GeoJsonGeometry, RunError> {
    serde_json::from_str(text).map_err(|error| RunError::InvalidConfiguration(error.to_string()))
}

fn build_outputs(
    case: &ResolvedCase,
    time: &trajecta_case::model::time::TimeSpec,
    sqlite_path: &std::path::Path,
    meteorology: &trajecta_met::query::engine::MetEngine,
    knobs: &RunnerBuildKnobs,
) -> Result<Vec<ScheduledOutputProduct>, RunError> {
    let make_sink = || {
        ParticleStateSqliteSink::with_time_bounds(time.start, time.end).with_bundle_sort_knobs(
            knobs.bundle_chunk_lines,
            knobs.bundle_merge_fan_in,
            knobs.reverse_particle_scan,
        )
    };
    let mut outputs = Vec::new();
    for product in &case.outputs {
        if product.product.0 != PARTICLE_STATE_PRODUCT_ID {
            return Err(RunError::InvalidConfiguration(format!(
                "unsupported output product {}",
                product.product.0
            )));
        }
        if product.sink.model.0 != trajecta_case::model::output::PARTICLE_STATE_SQLITE_SINK_ID {
            return Err(RunError::InvalidConfiguration(format!(
                "unsupported particle-state sink {}",
                product.sink.model.0
            )));
        }
        if !product.sink.parameters.is_empty() {
            return Err(RunError::InvalidConfiguration(
                "particle_state_sqlite/v1 accepts no parameters".into(),
            ));
        }
        let scheduler = match &product.schedule {
            OutputSchedule::Endpoints => OutputScheduler {
                sample_times: Vec::new(),
            },
            OutputSchedule::Interval { interval, origin } => {
                let origin = origin.unwrap_or(time.start);
                let step_ns = (interval.value_si() * 1.0e9).round() as i64;
                if step_ns <= 0 {
                    return Err(RunError::InvalidConfiguration("output interval".into()));
                }
                OutputScheduler {
                    sample_times: enumerate_interval(
                        origin,
                        time.start,
                        time.end,
                        time.direction,
                        step_ns,
                    )?,
                }
            }
        };
        let meteorology_plan = Some(
            meteorology
                .compile_plan(
                    QueryPlanRequest {
                        fields: vec![
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::EastwardWind,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::NorthwardWind,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::GeometricVerticalVelocity,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::AirPressure,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::AirTemperature,
                            ),
                        ],
                        allow_estimated: false,
                        surface_layer_model: Some(ModelId(
                            MoninObukhovBusingerDyer::MODEL_ID.into(),
                        )),
                        explain: ExplainMode::Disabled,
                    },
                    &Default::default(),
                )
                .map_err(|error| RunError::Meteorology(format!("{error:?}")))?,
        );
        outputs.push(ScheduledOutputProduct {
            product: Box::new(ParticleStateProduct::new(
                Box::new(make_sink()),
                sqlite_path.to_path_buf(),
            )),
            scheduler,
            meteorology_plan,
        });
    }
    if outputs.is_empty() {
        let meteorology_plan = Some(
            meteorology
                .compile_plan(
                    QueryPlanRequest {
                        fields: vec![
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::EastwardWind,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::NorthwardWind,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::GeometricVerticalVelocity,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::AirPressure,
                            ),
                            trajecta_met::field::FieldKey::Canonical(
                                trajecta_met::field::CanonicalField::AirTemperature,
                            ),
                        ],
                        allow_estimated: false,
                        surface_layer_model: Some(ModelId(
                            MoninObukhovBusingerDyer::MODEL_ID.into(),
                        )),
                        explain: ExplainMode::Disabled,
                    },
                    &Default::default(),
                )
                .map_err(|error| RunError::Meteorology(format!("{error:?}")))?,
        );
        outputs.push(ScheduledOutputProduct {
            product: Box::new(ParticleStateProduct::new(
                Box::new(make_sink()),
                sqlite_path.to_path_buf(),
            )),
            scheduler: OutputScheduler {
                sample_times: Vec::new(),
            },
            meteorology_plan,
        });
    }
    Ok(outputs)
}

fn enumerate_interval(
    origin: Timestamp,
    start: Timestamp,
    end: Timestamp,
    direction: Direction,
    step_ns: i64,
) -> Result<Vec<Timestamp>, RunError> {
    let mut times = Vec::new();
    let start_ns = ts_ns(start);
    let end_ns = ts_ns(end);
    let origin_ns = ts_ns(origin);
    let step = i128::from(step_ns);
    match direction {
        Direction::Forward => {
            let mut t = origin_ns;
            if t < start_ns {
                let delta = start_ns - t;
                let n = delta.div_euclid(step);
                t += n * step;
                if t < start_ns {
                    t += step;
                }
            }
            while t <= end_ns {
                if t >= start_ns {
                    times.push(ns_ts(t)?);
                }
                t = t.checked_add(step).ok_or(RunError::ResourceLimit)?;
            }
        }
        Direction::Backward => {
            let mut t = origin_ns;
            if t > start_ns {
                let delta = t - start_ns;
                let n = delta.div_euclid(step);
                t -= n * step;
                if t > start_ns {
                    t -= step;
                }
            }
            while t >= end_ns {
                if t <= start_ns {
                    times.push(ns_ts(t)?);
                }
                t = t.checked_sub(step).ok_or(RunError::ResourceLimit)?;
            }
        }
    }
    match direction {
        Direction::Forward => times.sort(),
        Direction::Backward => times.sort_by(|left, right| right.cmp(left)),
    }
    times.dedup();
    Ok(times)
}

fn ts_ns(timestamp: Timestamp) -> i128 {
    i128::from(timestamp.seconds_since_unix_epoch()) * 1_000_000_000
        + i128::from(timestamp.nanosecond())
}

fn ns_ts(value: i128) -> Result<Timestamp, RunError> {
    let seconds =
        i64::try_from(value.div_euclid(1_000_000_000)).map_err(|_| RunError::ResourceLimit)?;
    let nanos =
        u32::try_from(value.rem_euclid(1_000_000_000)).map_err(|_| RunError::ResourceLimit)?;
    Timestamp::new(seconds, nanos)
        .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))
}

fn unique_run_directory(
    output_root: &std::path::Path,
    case_name: &str,
    run_id: &RunId,
) -> Result<PathBuf, RunError> {
    let dir = run_directory_path(output_root, case_name, run_id)?;
    if dir.exists() {
        return Err(RunError::InvalidConfiguration(format!(
            "run directory already exists: {}",
            dir.display()
        )));
    }
    Ok(dir)
}

/// Returns the frozen `<output>/<sanitized-case>/<run-id>` attempt path.
pub fn run_directory_path(
    output_root: &Path,
    case_name: &str,
    run_id: &RunId,
) -> Result<PathBuf, RunError> {
    let safe = sanitize_case_name(case_name)?;
    Ok(output_root.join(safe).join(&run_id.0))
}

fn validate_attempt_identity(identity: &RunnerAttemptIdentity) -> Result<(), RunError> {
    let series = Uuid::parse_str(&identity.job_series_id.0)
        .map_err(|_| RunError::InvalidConfiguration("job series id is not a UUID".into()))?;
    let run = Uuid::parse_str(&identity.run_id.0)
        .map_err(|_| RunError::InvalidConfiguration("run id is not a UUID".into()))?;
    if series.get_version_num() != 7 || run.get_version_num() != 7 || identity.attempt == 0 {
        return Err(RunError::InvalidConfiguration(
            "attempt identity requires UUID-v7 ids and a positive attempt".into(),
        ));
    }
    Ok(())
}

fn case_with_canonical_geometries(
    case: &ResolvedCase,
    schedule: &ReleaseSchedule,
) -> Result<ResolvedCase, RunError> {
    let mut out = case.clone();
    let Some(ParticlePopulationSpec::ReleaseDriven(spec)) = out.particle_population.as_mut() else {
        return Ok(out);
    };
    if spec.events.len() != schedule.events.len() {
        return Err(RunError::InvalidConfiguration(
            "canonical geometry count mismatch".into(),
        ));
    }
    for (event_spec, runtime) in spec.events.iter_mut().zip(schedule.events.iter()) {
        event_spec.geometry = GeoJsonSource::Inline {
            geometry: runtime.geometry.clone(),
        };
    }
    Ok(out)
}

fn write_resolved_documents(
    run_dir: &std::path::Path,
    case: &ResolvedCase,
    run_profile: &ResolvedRunProfile,
) -> Result<(), RunError> {
    let case_path = run_dir.join("resolved-case.json");
    let profile_path = run_dir.join("resolved-run-profile.json");
    let case_json =
        serde_json::to_vec_pretty(case).map_err(|error| RunError::Manifest(error.to_string()))?;
    let profile_json = serde_json::to_vec_pretty(run_profile)
        .map_err(|error| RunError::Manifest(error.to_string()))?;
    fs::write(&case_path, case_json).map_err(|error| RunError::Manifest(error.to_string()))?;
    fs::write(&profile_path, profile_json)
        .map_err(|error| RunError::Manifest(error.to_string()))?;
    Ok(())
}

fn sanitize_case_name(name: &str) -> Result<String, RunError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(RunError::InvalidConfiguration("case name empty".into()));
    }
    let safe: String = trimmed
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    Ok(safe)
}

fn generate_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(1);
    nanos ^ 0xA5A5_A5A5_A5A5_A5A5
}

fn hash_json<T: serde::Serialize>(value: &T) -> Result<String, RunError> {
    let bytes = serde_json::to_vec(value).map_err(|error| RunError::Manifest(error.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(&bytes);
    Ok(hex::encode(digest.finalize()))
}

/// Geometry sampler that dispatches by event geometry identity.
struct MultiEventGeometrySampler {
    by_event: BTreeMap<String, SphericalGeometrySampler>,
}

impl MultiEventGeometrySampler {
    fn from_schedule(schedule: &ReleaseSchedule) -> Result<Self, RunError> {
        let mut by_event = BTreeMap::new();
        for event in &schedule.events {
            by_event.insert(
                event.id.0.clone(),
                SphericalGeometrySampler::new(event.geometry.clone())
                    .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?,
            );
        }
        Ok(Self { by_event })
    }
}

impl crate::release::GeometrySampler for MultiEventGeometrySampler {
    fn sample_horizontal(
        &self,
        request: crate::release::ReleaseSamplingRequest<'_>,
    ) -> Result<Vec<(f64, f64)>, crate::release::ReleaseError> {
        self.by_event
            .get(&request.event.id.0)
            .ok_or(crate::release::ReleaseError::InvalidEvent(
                "missing event geometry sampler".into(),
            ))?
            .sample_horizontal(request)
    }
}

struct ProductionMeteorology {
    engine: MetEngine,
    transport_plan: trajecta_met::query::request::TransportPlan,
    execution: Box<dyn trajecta_met::query::engine::ExecutionContext>,
    selected_domain: Option<DomainId>,
    dataset_lock_sha256: BTreeMap<String, String>,
    dataset_profile_sha256: BTreeMap<String, String>,
    dataset_content_sha256: BTreeMap<String, String>,
    reader_backends: BTreeMap<String, MeteorologyReaderBackend>,
}

fn collect_meteorology_query_times(
    case: &ResolvedCase,
    time: &trajecta_case::model::time::TimeSpec,
    schedule: &ReleaseSchedule,
    numerics: &trajecta_case::model::numerics::NumericsSpec,
) -> Result<Vec<Timestamp>, RunError> {
    let mut times = vec![time.start, time.end];
    // RK2 midpoint of the whole window (and step midpoints via half step).
    let start_ns = ts_ns(time.start);
    let end_ns = ts_ns(time.end);
    times.push(ns_ts((start_ns + end_ns) / 2)?);
    let step_ns = (numerics.time_step.value_si() * 1.0e9).round() as i128;
    if step_ns > 0 {
        let (lo, hi) = if end_ns >= start_ns {
            (start_ns, end_ns)
        } else {
            (end_ns, start_ns)
        };
        let mut t = lo;
        while t <= hi {
            times.push(ns_ts(t)?);
            // RK2 stage midpoint between t and t+step
            let mid = t + step_ns / 2;
            if mid >= lo && mid <= hi {
                times.push(ns_ts(mid)?);
            }
            t += step_ns;
            if times.len() > 100_000 {
                return Err(RunError::InvalidConfiguration(
                    "meteorology query time enumeration exceeded limit".into(),
                ));
            }
        }
    }
    for event in &schedule.events {
        // Continuous releases query met across the full event window; include ends
        // and midpoint (individual particle births are dense strata of this span).
        times.push(event.start);
        times.push(event.end);
        times.push(ns_ts((ts_ns(event.start) + ts_ns(event.end)) / 2)?);
    }
    // Output exact times from product schedules.
    for product in &case.outputs {
        match &product.schedule {
            trajecta_case::model::output::OutputSchedule::Interval { interval, origin } => {
                let step = (interval.value_si() * 1.0e9).round() as i64;
                if step > 0 {
                    let origin_ts = origin.unwrap_or(time.start);
                    times.extend(enumerate_interval(
                        origin_ts,
                        time.start,
                        time.end,
                        time.direction,
                        step,
                    )?);
                }
            }
            trajecta_case::model::output::OutputSchedule::Endpoints => {}
        }
    }
    times.sort_by_key(|t| (t.seconds_since_unix_epoch(), t.nanosecond()));
    times.dedup();
    Ok(times)
}

type FrameJob = (
    trajecta_met::io::inventory::FrameDescriptor,
    ProfileName,
    MeteorologyReaderBackend,
    DomainId,
);

/// Max `warmup_frames` over the full dependency closure of capability outputs.
///
/// 1. Roots = FieldReferences listed under required capabilities.
/// 2. Map each root target onto unique direct/derived field id (duplicate target → fail).
/// 3. For every derived field, parse its expression with the official Profile parser and
///    collect identifier dependencies (no string splitting).
/// 4. Recursively close over direct/derived ids; unknown identifiers hard-fail.
/// 5. Return max temporal.warmup_frames in the closed set.
fn max_warmup_frames_for_capabilities(
    document: &DatasetProfileDocument,
    capabilities: CapabilitySet,
) -> Result<usize, RunError> {
    let mut target_to_id: BTreeMap<FieldReference, String> = BTreeMap::new();
    let mut id_warmup: BTreeMap<String, u16> = BTreeMap::new();
    let mut derived_deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for field in &document.fields {
        if target_to_id
            .insert(field.target.clone(), field.id.clone())
            .is_some()
        {
            return Err(RunError::InvalidConfiguration(format!(
                "profile {}: duplicate field target for direct id {}",
                document.name.0, field.id
            )));
        }
        if id_warmup
            .insert(field.id.clone(), field.temporal.warmup_frames)
            .is_some()
        {
            return Err(RunError::InvalidConfiguration(format!(
                "profile {}: duplicate direct field id {}",
                document.name.0, field.id
            )));
        }
    }
    for field in &document.derived_fields {
        if target_to_id
            .insert(field.target.clone(), field.id.clone())
            .is_some()
        {
            return Err(RunError::InvalidConfiguration(format!(
                "profile {}: duplicate field target for derived id {}",
                document.name.0, field.id
            )));
        }
        if id_warmup
            .insert(field.id.clone(), field.temporal.warmup_frames)
            .is_some()
        {
            return Err(RunError::InvalidConfiguration(format!(
                "profile {}: duplicate derived field id {}",
                document.name.0, field.id
            )));
        }
        let expression = trajecta_met::profile::expression::parse_expression(&field.expression)
            .map_err(|error| {
                RunError::InvalidConfiguration(format!(
                    "profile {}: derived field {} expression: {error}",
                    document.name.0, field.id
                ))
            })?;
        let mut identifiers = BTreeSet::new();
        collect_expression_identifiers(&expression, &mut identifiers);
        for identifier in &identifiers {
            if !id_warmup.contains_key(identifier) && identifier != &field.id {
                // Forward reference allowed only to known direct/derived ids declared
                // in the same profile; unknown names hard-fail (may resolve after full
                // insert — re-check after maps complete).
                let _ = identifier;
            }
        }
        derived_deps.insert(field.id.clone(), identifiers);
    }
    // Re-validate derived deps against the complete id set.
    for (id, deps) in &derived_deps {
        for dep in deps {
            if dep == id {
                continue;
            }
            if !id_warmup.contains_key(dep) {
                return Err(RunError::InvalidConfiguration(format!(
                    "profile {}: derived field {id} references unknown identifier {dep}",
                    document.name.0
                )));
            }
        }
    }

    let mut roots = BTreeSet::<String>::new();
    for capability in capabilities.iter() {
        let Some(fields) = document.capabilities.get(&capability) else {
            continue;
        };
        for field_ref in fields {
            let Some(id) = target_to_id.get(field_ref) else {
                return Err(RunError::InvalidConfiguration(format!(
                    "profile {}: capability {capability:?} lists unmapped field target {field_ref:?}",
                    document.name.0
                )));
            };
            roots.insert(id.clone());
        }
    }
    if roots.is_empty() {
        return Ok(0);
    }

    let mut stack: Vec<String> = roots.into_iter().collect();
    let mut closed = BTreeSet::<String>::new();
    while let Some(id) = stack.pop() {
        if !closed.insert(id.clone()) {
            continue;
        }
        if let Some(deps) = derived_deps.get(&id) {
            for dep in deps {
                if dep != &id {
                    stack.push(dep.clone());
                }
            }
        }
    }

    let mut max_warmup = 0_usize;
    for id in closed {
        let warmup = *id_warmup.get(&id).ok_or_else(|| {
            RunError::InvalidConfiguration(format!(
                "profile {}: closed field id {id} missing warmup",
                document.name.0
            ))
        })?;
        max_warmup = max_warmup.max(usize::from(warmup));
    }
    Ok(max_warmup)
}

fn collect_expression_identifiers(
    expression: &trajecta_met::profile::expression::Expression,
    out: &mut BTreeSet<String>,
) {
    use trajecta_met::profile::expression::Expression;
    match expression {
        Expression::Identifier(name) => {
            out.insert(name.clone());
        }
        Expression::Constant(_) => {}
        Expression::Unary { operand, .. } => collect_expression_identifiers(operand, out),
        Expression::Binary { left, right, .. } => {
            collect_expression_identifiers(left, out);
            collect_expression_identifiers(right, out);
        }
        Expression::Call { arguments, .. } => {
            for arg in arguments {
                collect_expression_identifiers(arg, out);
            }
        }
    }
}

/// Select inventory frames covering the continuous query interval with formal support.
///
/// Rules (formal Transport + Profile warmup):
/// - Cover the closed interval [min query, max query] with the contiguous inventory
///   subsequence whose valid times span that interval.
/// - Exact-on-frame queries require distinct previous and next inventory frames
///   (MissingSymmetricTimeSupport).
/// - Expand **left** by per-domain `warmup_frames` = max over the full capability
///   dependency closure for that domain's Profile (not a global max, not a fixed pad).
/// - Never use the same frame as both before and after without neighbors.
/// - Forward and backward use the same selection.
fn select_bracketing_frames(
    frame_jobs: Vec<FrameJob>,
    query_times: &[Timestamp],
    warmup_by_domain: &BTreeMap<DomainId, usize>,
) -> Result<Vec<FrameJob>, RunError> {
    if query_times.is_empty() {
        return Err(RunError::Meteorology(
            "no meteorology query times to select frames".into(),
        ));
    }
    if frame_jobs.is_empty() {
        return Err(RunError::Meteorology("inventory produced no frames".into()));
    }
    let mut by_domain: BTreeMap<DomainId, Vec<usize>> = BTreeMap::new();
    for (idx, job) in frame_jobs.iter().enumerate() {
        by_domain.entry(job.3.clone()).or_default().push(idx);
    }
    let mut keep = vec![false; frame_jobs.len()];
    for (domain, mut idxs) in by_domain {
        idxs.sort_by(|&a, &b| {
            frame_jobs[a]
                .0
                .id
                .valid_time
                .cmp(&frame_jobs[b].0.id.valid_time)
                .then(
                    frame_jobs[a]
                        .0
                        .id
                        .content_sha256
                        .cmp(&frame_jobs[b].0.id.content_sha256),
                )
        });
        let times: Vec<Timestamp> = idxs
            .iter()
            .map(|&i| frame_jobs[i].0.id.valid_time)
            .collect();
        let left_warmup_frames = *warmup_by_domain.get(&domain).ok_or_else(|| {
            RunError::Meteorology(format!("missing per-domain warmup for domain {}", domain.0))
        })?;
        let selected_local =
            select_bracketing_frame_indices(&times, query_times, left_warmup_frames)?;
        for local in selected_local {
            keep[idxs[local]] = true;
        }
    }
    let selected: Vec<_> = frame_jobs
        .into_iter()
        .enumerate()
        .filter_map(|(i, job)| keep[i].then_some(job))
        .collect();
    if selected.is_empty() {
        return Err(RunError::Meteorology(
            "frame coverage selection retained no frames".into(),
        ));
    }
    Ok(selected)
}

/// Pure inventory-index selection used by production path and unit tests.
fn select_bracketing_frame_indices(
    times: &[Timestamp],
    query_times: &[Timestamp],
    left_warmup_frames: usize,
) -> Result<Vec<usize>, RunError> {
    if query_times.is_empty() {
        return Err(RunError::Meteorology(
            "no meteorology query times to select frames".into(),
        ));
    }
    if times.is_empty() {
        return Err(RunError::Meteorology("inventory produced no frames".into()));
    }
    let t_lo = query_times
        .iter()
        .min()
        .copied()
        .ok_or_else(|| RunError::Meteorology("empty query times".into()))?;
    let t_hi = query_times
        .iter()
        .max()
        .copied()
        .ok_or_else(|| RunError::Meteorology("empty query times".into()))?;
    let n = times.len();

    // Rightmost frame with vt <= t_lo (left bracket of interval start).
    let mut left = None;
    for (i, &vt) in times.iter().enumerate() {
        if vt <= t_lo {
            left = Some(i);
        }
    }
    // Leftmost frame with vt >= t_hi (right bracket of interval end).
    let mut right = None;
    for (i, &vt) in times.iter().enumerate() {
        if vt >= t_hi {
            right = Some(i);
            break;
        }
    }
    let (Some(mut lo_i), Some(mut hi_i)) = (left, right) else {
        return Err(RunError::Meteorology(format!(
            "missing continuous frame coverage for query interval [{}, {}]s",
            t_lo.seconds_since_unix_epoch(),
            t_hi.seconds_since_unix_epoch()
        )));
    };
    // Expand for exact-on-frame symmetric support (previous/next).
    if times[lo_i] == t_lo {
        if lo_i == 0 {
            return Err(RunError::Meteorology(
                "missing left guard frame before exact start valid_time".into(),
            ));
        }
        lo_i = lo_i.saturating_sub(1);
    }
    if times[hi_i] == t_hi {
        if hi_i + 1 >= n {
            return Err(RunError::Meteorology(
                "missing right guard frame after exact end valid_time".into(),
            ));
        }
        hi_i += 1;
    }
    // Profile warmup: capability-reachable max, not a fixed one-frame pad.
    if left_warmup_frames > 0 {
        if lo_i < left_warmup_frames {
            return Err(RunError::Meteorology(format!(
                "missing {left_warmup_frames} warmup frame(s) before query interval start; only {lo_i} preceding frame(s) available"
            )));
        }
        lo_i -= left_warmup_frames;
    }
    // Sanity: every query time is bracketed inside the kept range.
    for &query in query_times {
        let mut before_i = None;
        let mut after_i = None;
        for (i, vt) in times.iter().enumerate().take(hi_i + 1).skip(lo_i) {
            if *vt <= query {
                before_i = Some(i);
            }
            if *vt >= query && after_i.is_none() {
                after_i = Some(i);
            }
        }
        let (Some(b), Some(a)) = (before_i, after_i) else {
            return Err(RunError::Meteorology(format!(
                "selected frames do not bracket query {}s",
                query.seconds_since_unix_epoch()
            )));
        };
        if b == a {
            // Exact hit: need previous and next inside the kept range.
            if b == lo_i || a == hi_i {
                return Err(RunError::Meteorology(format!(
                    "exact frame query {}s lacks symmetric previous/next support",
                    query.seconds_since_unix_epoch()
                )));
            }
        }
    }
    Ok((lo_i..=hi_i).collect())
}

fn load_production_meteorology(
    case: &ResolvedCase,
    run_profile: &ResolvedRunProfile,
    _time: &trajecta_case::model::time::TimeSpec,
    query_times: &[Timestamp],
    runtime_domain: &DomainId,
    capabilities: CapabilitySet,
) -> Result<ProductionMeteorology, RunError> {
    let meteorology = case.meteorology.as_ref().ok_or_else(|| {
        RunError::InvalidConfiguration(
            "case.meteorology is required for production RunnerBuilder::build".into(),
        )
    })?;
    if meteorology.domains.is_empty() {
        return Err(RunError::InvalidConfiguration(
            "case.meteorology.domains must not be empty".into(),
        ));
    }
    if run_profile.datasets.is_empty() {
        return Err(RunError::InvalidConfiguration(
            "run_profile.datasets must bind every case meteorology domain".into(),
        ));
    }

    let profiles = ProfileCatalog::load(&run_profile.profile_sources)
        .map_err(|error| RunError::InvalidConfiguration(format!("profile catalog: {error:?}")))?;
    let profiles_for_loader = profiles.clone();

    let mut catalog = MetCatalog {
        capabilities,
        ..MetCatalog::default()
    };
    let selected_domain = Some(runtime_domain.clone());
    let mut dataset_lock_sha256 = BTreeMap::new();
    let mut dataset_profile_sha256 = BTreeMap::new();
    let mut dataset_content_sha256 = BTreeMap::new();
    let mut reader_backends = BTreeMap::new();
    let mut frame_jobs = Vec::new();

    for domain_spec in meteorology
        .domains
        .iter()
        .filter(|domain| domain.id == *runtime_domain)
    {
        let binding = run_profile
            .datasets
            .iter()
            .find(|binding| binding.dataset == domain_spec.dataset)
            .ok_or_else(|| {
                RunError::InvalidConfiguration(format!(
                    "missing dataset binding for {}",
                    domain_spec.dataset.0
                ))
            })?;
        let lock_path = if binding.lockfile.is_absolute() {
            binding.lockfile.clone()
        } else {
            run_profile
                .case_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&binding.lockfile)
        };
        let lock_text = fs::read_to_string(&lock_path)
            .map_err(|error| RunError::InvalidConfiguration(error.to_string()))?;
        let lock = if lock_path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            parse_dataset_lock_json(&lock_text)
                .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?
        } else {
            parse_dataset_lock_yaml(&lock_text)
                .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?
        };
        let lockfile_dir = lock_path
            .parent()
            .ok_or_else(|| RunError::InvalidConfiguration("lockfile has no parent".into()))?;
        let dataset_key = binding.dataset.0.clone();
        let mut lock_digest = Sha256::new();
        lock_digest.update(lock_text.as_bytes());
        dataset_lock_sha256.insert(dataset_key.clone(), hex::encode(lock_digest.finalize()));
        dataset_profile_sha256.insert(dataset_key.clone(), lock.profile.sha256.clone());
        for file in &lock.files {
            dataset_content_sha256.insert(
                format!("{}:{}", dataset_key, file.relative_path.display()),
                file.sha256.clone(),
            );
        }
        let backend = binding
            .reader_backend
            .unwrap_or(run_profile.execution.meteorology_reader);
        reader_backends.insert(dataset_key, backend);

        let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
            lock: &lock,
            lockfile_dir,
            data_roots: &binding.data_roots,
            domain: &domain_spec.id,
            required_capabilities: capabilities,
        });
        if !inventory.is_success() {
            return Err(RunError::InvalidConfiguration(format!(
                "inventory failed for domain {}: {:?}",
                domain_spec.id.0,
                inventory.diagnostics.sorted()
            )));
        }
        let domain_catalog = inventory
            .catalog
            .ok_or_else(|| RunError::InvalidConfiguration("inventory catalog missing".into()))?;
        let profile_name = ProfileName(lock.profile.name.clone());
        for (domain_id, domain) in domain_catalog.domains {
            if domain_id != *runtime_domain {
                return Err(RunError::InvalidConfiguration(format!(
                    "inventory returned unexpected domain '{}' while loading '{}'",
                    domain_id.0, runtime_domain.0
                )));
            }
            for descriptor in domain.frames.values() {
                frame_jobs.push((
                    descriptor.clone(),
                    profile_name.clone(),
                    backend,
                    domain_id.clone(),
                ));
            }
            catalog.domains.insert(domain_id, domain);
        }
    }

    if catalog.domains.is_empty() {
        return Err(RunError::InvalidConfiguration(
            "production meteorology catalog is empty".into(),
        ));
    }

    let fields = FieldRegistry::canonical()
        .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;
    let mut surface_layers = SurfaceLayerRegistry::new();
    surface_layers
        .register(Arc::new(MoninObukhovBusingerDyer::default()))
        .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;
    let memory_budget = MemoryBudget::new(
        run_profile
            .execution
            .memory_budget_bytes
            .max(64 * 1024 * 1024),
        (run_profile.execution.memory_budget_bytes / 4).max(16 * 1024 * 1024),
    )
    .map_err(|error| RunError::InvalidConfiguration(format!("{error:?}")))?;

    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles,
        fields,
        surface_layers,
        memory_budget,
    });

    // Keep only frames that formally bracket at least one meteorology query time
    // (start/end, RK2 midpoints, release window, output exact times). Left expand
    // is per (domain, profile): full capability dependency-closure warmup — not a
    // global max, not a fixed one-frame pad, not pad=max(span,3600).
    let mut warmup_by_domain = BTreeMap::<DomainId, usize>::new();
    let mut domain_profile = BTreeMap::<DomainId, ProfileName>::new();
    for (_descriptor, profile_name, _backend, domain) in &frame_jobs {
        match domain_profile.entry(domain.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(profile_name.clone());
            }
            std::collections::btree_map::Entry::Occupied(slot) => {
                if slot.get() != profile_name {
                    return Err(RunError::InvalidConfiguration(format!(
                        "domain {} bound to inconsistent profiles {} vs {}",
                        domain.0,
                        slot.get().0,
                        profile_name.0
                    )));
                }
            }
        }
    }
    for (domain, profile_name) in &domain_profile {
        let profile = profiles_for_loader.get(profile_name).ok_or_else(|| {
            RunError::InvalidConfiguration(format!(
                "missing profile {} for domain {}",
                profile_name.0, domain.0
            ))
        })?;
        let warmup = max_warmup_frames_for_capabilities(&profile.document, capabilities)?;
        warmup_by_domain.insert(domain.clone(), warmup);
    }
    frame_jobs = select_bracketing_frames(frame_jobs, query_times, &warmup_by_domain)?;

    frame_jobs.sort_by(|left, right| {
        left.3
            .cmp(&right.3)
            .then(left.0.id.valid_time.cmp(&right.0.id.valid_time))
            .then(left.0.id.content_sha256.cmp(&right.0.id.content_sha256))
    });
    let mut previous_by_domain: BTreeMap<DomainId, Arc<trajecta_met::frame::RawMetFrame>> =
        BTreeMap::new();
    for (descriptor, profile_name, backend, domain_id) in frame_jobs {
        let profile = profiles_for_loader.get(&profile_name).ok_or_else(|| {
            RunError::InvalidConfiguration(format!("missing profile {}", profile_name.0))
        })?;
        let previous = previous_by_domain.get(&domain_id).map(Arc::as_ref);
        let frame = FrameLoader::load(FrameLoadRequest {
            descriptor: &descriptor,
            profile,
            required_capabilities: capabilities,
            backend,
            previous_frame: previous,
        })
        .map_err(|error| RunError::Meteorology(format!("frame load: {error:?}")))?;
        let frame = Arc::new(frame);
        engine
            .cache_frame(Arc::clone(&frame))
            .map_err(|error| RunError::Meteorology(format!("cache_frame: {error:?}")))?;
        previous_by_domain.insert(domain_id, frame);
    }

    let transport_plan = engine
        .compile_transport_plan(
            TransportPlanRequest {
                allow_estimated: false,
                explain: ExplainMode::Disabled,
                surface_layer_model: ModelId(MoninObukhovBusingerDyer::MODEL_ID.into()),
            },
            &ExecutionPlan::default(),
        )
        .map_err(|error| RunError::Meteorology(format!("{error:?}")))?;

    let workers = run_profile.execution.worker_threads.max(1);
    let execution: Box<dyn trajecta_met::query::engine::ExecutionContext> =
        Box::new(RayonExecutionContext {
            worker_threads: workers,
        });
    Ok(ProductionMeteorology {
        engine,
        transport_plan,
        execution,
        selected_domain,
        dataset_lock_sha256,
        dataset_profile_sha256,
        dataset_content_sha256,
        reader_backends,
    })
}

// Silence unused import in some builds.
#[allow(dead_code)]
fn _arc_unused(_: Arc<()>) {}
#[allow(dead_code)]
fn _rayon_unused(_: RayonExecutionContext) {}
#[allow(dead_code)]
fn _constants() {
    let _ = M4_CONSTANTS.earth_radius_m;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{
        max_warmup_frames_for_capabilities, required_capabilities_for_population,
        select_bracketing_frame_indices,
    };
    use trajecta_case::model::meteorology::DomainId;
    use trajecta_case::model::population::{
        DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, ParticlePopulationSpec,
        PopulationId,
    };
    use trajecta_case::model::substance::SubstanceId;
    use trajecta_case::model::time::Timestamp;
    use trajecta_met::field::{
        CanonicalField, Capability, CapabilitySet, FieldQuality, FieldShape,
    };
    use trajecta_met::profile::document::{
        DatasetProfileDocument, DerivedField, FieldMapping, FieldReference, FieldSource,
        ProfileDocumentKind, ProfileName, TemporalKind, TemporalSemantics,
    };
    use trajecta_met::profile::graph::ExecutionStage;

    #[test]
    fn air_mass_builder_requests_domain_fill_capability_in_addition_to_transport() {
        let capabilities = required_capabilities_for_population(
            &ParticlePopulationSpec::DomainFillAirMass(DomainFillAirMassSpec {
                id: PopulationId("air".into()),
                domain_id: DomainId("d".into()),
                target_dry_air_mass_per_particle: None,
                target_particle_count: Some(1),
            }),
        )
        .unwrap();
        assert!(capabilities.contains(Capability::Transport));
        assert!(capabilities.contains(Capability::NearSurfaceTransport));
        assert!(capabilities.contains(Capability::DomainFill));
    }

    #[test]
    fn ozone_builder_requests_domain_fill_and_diagnostics_capabilities() {
        let capabilities = required_capabilities_for_population(
            &ParticlePopulationSpec::DomainFillStratosphericOzone(
                DomainFillStratosphericOzoneSpec {
                    air_mass: DomainFillAirMassSpec {
                        id: PopulationId("ozone".into()),
                        domain_id: DomainId("d".into()),
                        target_dry_air_mass_per_particle: None,
                        target_particle_count: Some(1),
                    },
                    ozone_rule: crate::science::FLEXPART_PV60_OZONE_ID.into(),
                    ozone_substance: SubstanceId("ozone".into()),
                },
            ),
        )
        .unwrap();
        assert!(capabilities.contains(Capability::Transport));
        assert!(capabilities.contains(Capability::NearSurfaceTransport));
        assert!(capabilities.contains(Capability::DomainFill));
        assert!(capabilities.contains(Capability::Diagnostics));
    }

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::new(seconds, 0).unwrap()
    }

    #[test]
    fn continuous_interval_retains_00_and_06_for_03utc_query() {
        // Inventory times 00/06/12; query only 03 UTC; warmup=0 → no fixed outer pad.
        let frames = [ts(0), ts(6 * 3600), ts(12 * 3600)];
        let query = [ts(3 * 3600)];
        let kept = select_bracketing_frame_indices(&frames, &query, 0).unwrap();
        assert_eq!(kept, vec![0, 1]);
    }

    #[test]
    fn exact_frame_query_requires_distinct_neighbors() {
        let frames = [ts(0), ts(6 * 3600), ts(12 * 3600)];
        let query = [ts(6 * 3600)];
        let kept = select_bracketing_frame_indices(&frames, &query, 0).unwrap();
        // exact 06 needs previous 00 and next 12.
        assert_eq!(kept, vec![0, 1, 2]);
    }

    #[test]
    fn left_warmup_expands_by_capability_max_not_fixed_one() {
        let frames = [
            ts(0),
            ts(6 * 3600),
            ts(12 * 3600),
            ts(18 * 3600),
            ts(24 * 3600),
        ];
        // Query exact 18 → previous/next then +warmup left of lo.
        let query = [ts(18 * 3600)];
        let kept = select_bracketing_frame_indices(&frames, &query, 2).unwrap();
        // exact: lo=12(idx2), hi=24(idx4); warmup 2 → lo=0.
        assert_eq!(kept, vec![0, 1, 2, 3, 4]);
        let kept1 = select_bracketing_frame_indices(&frames, &query, 1).unwrap();
        assert_eq!(kept1, vec![1, 2, 3, 4]);
    }

    #[test]
    fn missing_warmup_is_hard_fail() {
        // 00/06/12 present so exact-06 symmetric support succeeds; warmup still fails.
        let frames = [ts(0), ts(6 * 3600), ts(12 * 3600)];
        let query = [ts(6 * 3600)];
        // exact → lo=0 (00), hi=2 (12); warmup 1 needs one more left of lo → fail.
        let err = select_bracketing_frame_indices(&frames, &query, 1).unwrap_err();
        assert!(format!("{err:?}").contains("warmup"), "{err:?}");
    }

    fn direct_field(id: &str, target: FieldReference, warmup: u16, unit: &str) -> FieldMapping {
        FieldMapping {
            id: id.into(),
            target,
            sources: vec![FieldSource {
                identity: Default::default(),
                unit: unit.into(),
                role: Default::default(),
                quality: FieldQuality::Source,
            }],
            temporal: TemporalSemantics {
                kind: if warmup == 0 {
                    TemporalKind::Instantaneous
                } else {
                    TemporalKind::Accumulation
                },
                interval_seconds: if warmup == 0 { None } else { Some(21600) },
                reset: None,
                warmup_frames: warmup,
            },
        }
    }

    fn derived_field(
        id: &str,
        target: FieldReference,
        expr: &str,
        warmup: u16,
        unit: &str,
        shape: FieldShape,
    ) -> DerivedField {
        DerivedField {
            id: id.into(),
            target,
            expression: expr.into(),
            unit: unit.into(),
            shape,
            stage: ExecutionStage::Frame,
            quality: FieldQuality::Derived,
            temporal: TemporalSemantics {
                kind: if warmup == 0 {
                    TemporalKind::Instantaneous
                } else {
                    TemporalKind::Accumulation
                },
                interval_seconds: if warmup == 0 { None } else { Some(21600) },
                reset: None,
                warmup_frames: warmup,
            },
        }
    }

    fn ext(ns: &str, name: &str) -> FieldReference {
        FieldReference::Extension {
            namespace: ns.into(),
            name: name.into(),
        }
    }

    #[test]
    fn max_warmup_uses_only_capability_reachable_fields() {
        let precip = FieldReference::Canonical(CanonicalField::PrecipitationRate);
        let u = FieldReference::Canonical(CanonicalField::EastwardWind);
        let mut capabilities = std::collections::BTreeMap::new();
        capabilities.insert(Capability::Transport, vec![u.clone()]);
        capabilities.insert(Capability::WetDeposition, vec![precip.clone()]);
        let document = DatasetProfileDocument {
            schema_version: 0,
            kind: ProfileDocumentKind::DatasetProfile,
            name: ProfileName("t".into()),
            fingerprint: Default::default(),
            source_matchers: Vec::new(),
            candidate_path_globs: Vec::new(),
            frame_interval_seconds: None,
            extension_fields: Vec::new(),
            fields: vec![
                direct_field("u", u, 0, "m s-1"),
                // intermediate accumulated source (unique target)
                direct_field("pr_src", ext("tmp", "pr_acc"), 4, "kg m-2 s-1"),
            ],
            derived_fields: vec![derived_field(
                "pr",
                precip,
                "pr_src",
                0,
                "kg m-2 s-1",
                FieldShape::Horizontal2D,
            )],
            capabilities,
        };
        let transport_only = CapabilitySet::new().with(Capability::Transport);
        assert_eq!(
            max_warmup_frames_for_capabilities(&document, transport_only).unwrap(),
            0
        );
        let with_wet = transport_only.with(Capability::WetDeposition);
        // capability lists derived pr → closes over pr_src warmup 4
        assert_eq!(
            max_warmup_frames_for_capabilities(&document, with_wet).unwrap(),
            4
        );
    }

    #[test]
    fn max_warmup_follows_derived_dependency_chain() {
        // capability output -> derived A -> derived B -> accumulated direct source
        let out = FieldReference::Canonical(CanonicalField::EastwardWind);
        let mut capabilities = std::collections::BTreeMap::new();
        capabilities.insert(Capability::Transport, vec![out.clone()]);
        let junk = FieldReference::Canonical(CanonicalField::PrecipitationRate);
        let document = DatasetProfileDocument {
            schema_version: 0,
            kind: ProfileDocumentKind::DatasetProfile,
            name: ProfileName("chain".into()),
            fingerprint: Default::default(),
            source_matchers: Vec::new(),
            candidate_path_globs: Vec::new(),
            frame_interval_seconds: None,
            extension_fields: Vec::new(),
            fields: vec![
                direct_field("acc_src", ext("tmp", "acc"), 3, "m s-1"),
                // unrelated high warmup must not count
                direct_field("junk_src", ext("tmp", "junk_acc"), 9, "kg m-2 s-1"),
            ],
            derived_fields: vec![
                derived_field("b", ext("tmp", "b"), "acc_src", 0, "K", FieldShape::Full3D),
                derived_field("a", out, "b", 1, "m s-1", FieldShape::Full3D),
                derived_field(
                    "junk",
                    junk,
                    "junk_src",
                    0,
                    "kg m-2 s-1",
                    FieldShape::Horizontal2D,
                ),
            ],
            capabilities,
        };
        let caps = CapabilitySet::new().with(Capability::Transport);
        assert_eq!(
            max_warmup_frames_for_capabilities(&document, caps).unwrap(),
            3
        );
    }

    #[test]
    fn unknown_identifier_in_derived_hard_fails() {
        let out = FieldReference::Canonical(CanonicalField::EastwardWind);
        let mut capabilities = std::collections::BTreeMap::new();
        capabilities.insert(Capability::Transport, vec![out.clone()]);
        let document = DatasetProfileDocument {
            schema_version: 0,
            kind: ProfileDocumentKind::DatasetProfile,
            name: ProfileName("bad".into()),
            fingerprint: Default::default(),
            source_matchers: Vec::new(),
            candidate_path_globs: Vec::new(),
            frame_interval_seconds: None,
            extension_fields: Vec::new(),
            fields: Vec::new(),
            derived_fields: vec![derived_field(
                "a",
                out,
                "missing_id",
                0,
                "m s-1",
                FieldShape::Full3D,
            )],
            capabilities,
        };
        let err = max_warmup_frames_for_capabilities(
            &document,
            CapabilitySet::new().with(Capability::Transport),
        )
        .unwrap_err();
        assert!(format!("{err:?}").contains("unknown identifier"), "{err:?}");
    }

    #[test]
    fn per_domain_warmup_does_not_cross_contaminate() {
        // domain A warmup=0 keeps [0,1]; domain B warmup=2 needs more history.
        let frames = [ts(0), ts(6 * 3600), ts(12 * 3600), ts(18 * 3600)];
        let query = [ts(12 * 3600)];
        // exact 12 → lo=6h(idx1), hi=18h(idx3)
        let a = select_bracketing_frame_indices(&frames, &query, 0).unwrap();
        assert_eq!(a, vec![1, 2, 3]);
        let b = select_bracketing_frame_indices(&frames, &query, 1).unwrap();
        assert_eq!(b, vec![0, 1, 2, 3]);
        // domain with warmup=0 succeeds even when another domain would need more
        // (selection is independent — B missing history fails only for B).
        let short = [ts(6 * 3600), ts(12 * 3600), ts(18 * 3600)];
        assert!(select_bracketing_frame_indices(&short, &query, 0).is_ok());
        assert!(select_bracketing_frame_indices(&short, &query, 1).is_err());
    }

    #[test]
    fn forward_and_backward_select_same_time_set() {
        let frames = [ts(0), ts(6 * 3600), ts(12 * 3600), ts(18 * 3600)];
        let forward = [ts(3 * 3600), ts(9 * 3600)];
        let backward = [ts(9 * 3600), ts(3 * 3600)];
        let a = select_bracketing_frame_indices(&frames, &forward, 0).unwrap();
        let b = select_bracketing_frame_indices(&frames, &backward, 0).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, vec![0, 1, 2]);
    }
}
