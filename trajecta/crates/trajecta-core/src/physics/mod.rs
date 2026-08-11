//! # Contract: one deterministic M6 physical-process pipeline
//!
//! The pipeline owns the common substep partition and process state. M6 stages
//! add modules to this sequence; there is no alternate fast or compatibility
//! executor.

mod boundary_layer;
mod mesoscale;

use std::collections::BTreeMap;
use std::sync::Arc;

use trajecta_case::model::physics::{
    ModelId, PhysicsModuleId, ResolvedPhysicsModule, ResolvedPhysicsSpec,
};
use trajecta_case::model::time::{Direction, Timestamp};
use trajecta_met::auxiliary::gmted2010::Gmted2010;
use trajecta_met::field::{CanonicalField, FieldKey};
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::query::engine::{BatchWorkspace, ExecutionContext, MetEngine};
use trajecta_met::query::output::SampleStatus;
use trajecta_met::query::request::{
    ExplainMode, QueryBatch, QueryPlan, QueryPlanRequest, QueryPointArrays, VerticalQuery,
};
use trajecta_met::surface_layer::MoninObukhovBusingerDyer;

use crate::clock::{SignedDuration, add_timestamp, signed_duration_between};
use crate::particle::{ParticleBatch, ParticleStatus};

use self::boundary_layer::{
    BIRTH_DRAW_INDEX, BoundaryLayerEnvironment, ENDPOINT_DRAW_INDEX, LangevinKey,
    MIDPOINT_DRAW_INDEX, maximum_stable_step_ns, sample_stationary_velocity, update_velocity,
};
use self::mesoscale::{
    MesoscaleKey, sample_stationary_velocity as sample_stationary_mesoscale_velocity,
    update_velocity as update_mesoscale_velocity,
};

/// Stable implementation identity for the A1 boundary-layer module.
pub const BOUNDARY_LAYER_LANGEVIN_ALGORITHM_ID: &str = "thomson_hanna_skewed_cbl_langevin/v1";
/// Stable implementation identity for the A2 terrain correction.
pub const SUBGRID_OROGRAPHY_ALGORITHM_ID: &str = "gmted2010_anomaly_stability_limited_mixing/v1";

const BUOYANCY_FREQUENCY_FLOOR_S_INV: f64 = 1.0e-4;
const STABLE_KINETIC_CAP_MULTIPLIER: f64 = 2.0;

/// Concrete M6 physical-process pipeline.
pub struct PhysicsPipeline {
    subgrid_orography: Option<SubgridOrography>,
    boundary_layer: Option<BoundaryLayerLangevin>,
    mesoscale: Option<MesoscaleMarkov>,
    common_maximum_substep_ns: i64,
}

/// Process state prepared for one common substep.
///
/// The particle batch carries midpoint velocities while the transport
/// integrator runs. End-point velocities are committed only for particles
/// that complete the substep.
pub(crate) struct PreparedMotion {
    boundary_layer_end: Vec<(usize, [f64; 3])>,
    mesoscale_end: Vec<(usize, [f64; 3])>,
}

impl PreparedMotion {
    /// Commits persistent process state after transport and boundary handling.
    pub(crate) fn finish(
        self,
        particles: &mut ParticleBatch,
        surface_reflections: &[usize],
    ) -> Result<(), PhysicsError> {
        let particle_count = particles
            .len()
            .map_err(|_| PhysicsError::InvalidParticleBatch)?;
        if surface_reflections
            .iter()
            .any(|index| *index >= particle_count)
            || surface_reflections
                .windows(2)
                .any(|window| window[0] >= window[1])
        {
            return Err(PhysicsError::InvalidParticleBatch);
        }
        for (particle_index, mut velocity) in self.boundary_layer_end {
            if particle_index >= particle_count {
                return Err(PhysicsError::InvalidParticleBatch);
            }
            if particles.status[particle_index] == ParticleStatus::Alive {
                if surface_reflections.binary_search(&particle_index).is_ok() {
                    velocity[2] = -velocity[2];
                }
                particles
                    .set_boundary_layer_velocity(particle_index, velocity)
                    .map_err(|_| PhysicsError::InvalidParticleBatch)?;
            }
        }
        for (particle_index, mut velocity) in self.mesoscale_end {
            if particle_index >= particle_count {
                return Err(PhysicsError::InvalidParticleBatch);
            }
            if particles.status[particle_index] == ParticleStatus::Alive {
                if surface_reflections.binary_search(&particle_index).is_ok() {
                    velocity[2] = -velocity[2];
                }
                particles
                    .set_mesoscale_velocity(particle_index, velocity)
                    .map_err(|_| PhysicsError::InvalidParticleBatch)?;
            }
        }
        Ok(())
    }
}

struct BoundaryLayerLangevin {
    query_plan: QueryPlan,
}

struct SubgridOrography {
    dataset: Arc<Gmted2010>,
}

struct MesoscaleMarkov {
    correlation_interval_fraction: f64,
}

#[derive(Clone, Copy, Debug)]
struct BoundaryLayerMotionInput {
    particle_index: usize,
    first_half_ns: i64,
    second_half_ns: i64,
    environment: BoundaryLayerEnvironment,
}

#[derive(Clone, Copy, Debug)]
struct MesoscaleMotionInput {
    particle_index: usize,
    first_half_ns: i64,
    second_half_ns: i64,
    variance_m2_s2: [f64; 3],
    native_interval_seconds: f64,
}

impl PhysicsPipeline {
    /// Builds the available M6 motion pipeline and rejects later-stage modules.
    pub fn build(
        physics: Option<&ResolvedPhysicsSpec>,
        meteorology: &MetEngine,
        gmted2010: Option<Arc<Gmted2010>>,
    ) -> Result<Option<Self>, PhysicsError> {
        let Some(physics) = physics else {
            return Ok(None);
        };
        for module in &physics.modules {
            if !matches!(
                module.model,
                PhysicsModuleId::SubgridOrography
                    | PhysicsModuleId::BoundaryLayerLangevin
                    | PhysicsModuleId::MesoscaleMarkov
            ) {
                return Err(PhysicsError::ModuleStageNotAvailable {
                    module: module.model,
                    stage: module.model.implementation_stage(),
                });
            }
        }
        let subgrid_module = physics
            .modules
            .iter()
            .find(|module| module.model == PhysicsModuleId::SubgridOrography);
        let boundary_layer_module = physics
            .modules
            .iter()
            .find(|module| module.model == PhysicsModuleId::BoundaryLayerLangevin);
        let mesoscale_module = physics
            .modules
            .iter()
            .find(|module| module.model == PhysicsModuleId::MesoscaleMarkov);
        if subgrid_module.is_none() && boundary_layer_module.is_none() && mesoscale_module.is_none()
        {
            return Ok(None);
        }
        let common_maximum_substep_ns = physics
            .modules
            .iter()
            .filter(|module| {
                matches!(
                    module.model,
                    PhysicsModuleId::SubgridOrography
                        | PhysicsModuleId::BoundaryLayerLangevin
                        | PhysicsModuleId::MesoscaleMarkov
                )
            })
            .map(duration_ns)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .min()
            .ok_or(PhysicsError::SubstepUnderflow)?;
        let subgrid_orography = subgrid_module
            .map(|_| {
                gmted2010
                    .map(|dataset| SubgridOrography { dataset })
                    .ok_or_else(|| {
                        PhysicsError::Scientific(
                            "subgrid_orography requires the locked GMTED2010 dataset".into(),
                        )
                    })
            })
            .transpose()?;
        let boundary_layer = boundary_layer_module
            .map(|_| {
                meteorology
                    .compile_plan(
                        QueryPlanRequest {
                            fields: boundary_layer_fields()
                                .into_iter()
                                .map(FieldKey::Canonical)
                                .collect(),
                            allow_estimated: false,
                            surface_layer_model: Some(ModelId(
                                MoninObukhovBusingerDyer::MODEL_ID.into(),
                            )),
                            explain: ExplainMode::Disabled,
                        },
                        &ExecutionPlan::default(),
                    )
                    .map(|query_plan| BoundaryLayerLangevin { query_plan })
                    .map_err(|error| PhysicsError::Meteorology(format!("{error:?}")))
            })
            .transpose()?;
        let mesoscale = mesoscale_module
            .map(|module| {
                module
                    .correlation_interval_fraction
                    .filter(|value| value.is_finite() && (0.05..=1.0).contains(value))
                    .map(|correlation_interval_fraction| MesoscaleMarkov {
                        correlation_interval_fraction,
                    })
                    .ok_or(PhysicsError::Scientific(
                        "mesoscale correlation interval is invalid".into(),
                    ))
            })
            .transpose()?;
        Ok(Some(Self {
            subgrid_orography,
            boundary_layer,
            mesoscale,
            common_maximum_substep_ns,
        }))
    }

    /// Partitions one signed macro step into shared integer-nanosecond substeps.
    pub fn partition(
        &self,
        macro_step: SignedDuration,
    ) -> Result<Vec<SignedDuration>, PhysicsError> {
        let sign = match macro_step.0.cmp(&0) {
            std::cmp::Ordering::Less => -1_i64,
            std::cmp::Ordering::Equal => return Err(PhysicsError::SubstepUnderflow),
            std::cmp::Ordering::Greater => 1_i64,
        };
        let mut remaining = macro_step
            .0
            .checked_abs()
            .ok_or(PhysicsError::SubstepUnderflow)?;
        if self.common_maximum_substep_ns <= 0 {
            return Err(PhysicsError::SubstepUnderflow);
        }
        let count = (remaining - 1) / self.common_maximum_substep_ns + 1;
        let capacity = usize::try_from(count).map_err(|_| PhysicsError::ResourceLimit)?;
        let mut steps = Vec::with_capacity(capacity);
        while remaining > 0 {
            let magnitude = remaining.min(self.common_maximum_substep_ns);
            steps.push(SignedDuration(sign * magnitude));
            remaining -= magnitude;
        }
        Ok(steps)
    }

    /// Prepares midpoint motion velocity and the persistent end-point state.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_motion(
        &mut self,
        substep_start: Timestamp,
        substep_end: Timestamp,
        active_start_times: &[Timestamp],
        direction: Direction,
        macro_step: u64,
        substep: u32,
        active_indices: &[usize],
        particles: &mut ParticleBatch,
        meteorology: &mut MetEngine,
        execution: &dyn ExecutionContext,
        domain: &trajecta_case::model::meteorology::DomainId,
        seed: u64,
    ) -> Result<PreparedMotion, PhysicsError> {
        if self.subgrid_orography.is_none()
            && self.boundary_layer.is_none()
            && self.mesoscale.is_none()
        {
            return Ok(PreparedMotion {
                boundary_layer_end: Vec::new(),
                mesoscale_end: Vec::new(),
            });
        }
        if self.boundary_layer.is_some() {
            particles
                .initialize_boundary_layer_state()
                .map_err(|_| PhysicsError::InvalidParticleBatch)?;
        }
        if self.mesoscale.is_some() {
            particles
                .initialize_mesoscale_state()
                .map_err(|_| PhysicsError::InvalidParticleBatch)?;
        }
        if active_indices.is_empty() {
            return Ok(PreparedMotion {
                boundary_layer_end: Vec::new(),
                mesoscale_end: Vec::new(),
            });
        }
        if active_start_times.len()
            != particles
                .len()
                .map_err(|_| PhysicsError::InvalidParticleBatch)?
        {
            return Err(PhysicsError::InvalidParticleBatch);
        }

        // Ordinary particles share one midpoint. A particle born inside this
        // substep forms a smaller cohort with its exact active interval.
        let mut midpoint_groups = BTreeMap::<Timestamp, Vec<(usize, i64, i64)>>::new();
        for particle_index in active_indices.iter().copied() {
            let active_step =
                signed_duration_between(active_start_times[particle_index], substep_end)
                    .map_err(|_| PhysicsError::SubstepUnderflow)?;
            if active_step == SignedDuration::ZERO {
                continue;
            }
            let midpoint = add_timestamp(
                active_start_times[particle_index],
                SignedDuration(active_step.0 / 2),
            )
            .map_err(|_| PhysicsError::SubstepUnderflow)?;
            let first_half_ns = (active_step.0 / 2).unsigned_abs();
            let second_half_ns = active_step
                .0
                .checked_sub(active_step.0 / 2)
                .ok_or(PhysicsError::SubstepUnderflow)?
                .unsigned_abs();
            midpoint_groups.entry(midpoint).or_default().push((
                particle_index,
                i64::try_from(first_half_ns).map_err(|_| PhysicsError::SubstepUnderflow)?,
                i64::try_from(second_half_ns).map_err(|_| PhysicsError::SubstepUnderflow)?,
            ));
        }

        let mut workspace = BatchWorkspace::default();
        let mut boundary_layer_inputs = Vec::with_capacity(active_indices.len());
        let mut mesoscale_inputs = Vec::with_capacity(active_indices.len());
        for (midpoint, group) in midpoint_groups {
            let window = meteorology
                .prepare_for_domain(midpoint, domain)
                .map_err(|error| PhysicsError::Meteorology(format!("{error:?}")))?;
            let points = QueryPointArrays {
                longitude_degrees: group
                    .iter()
                    .map(|(index, _, _)| particles.longitude_degrees[*index])
                    .collect(),
                latitude_degrees: group
                    .iter()
                    .map(|(index, _, _)| particles.latitude_degrees[*index])
                    .collect(),
                vertical: group
                    .iter()
                    .map(|(index, _, _)| particles.height_asl_m[*index])
                    .collect(),
            };
            let batch = QueryBatch {
                vertical_coordinate: VerticalQuery::AboveSeaLevel,
                points,
            };
            let boundary_layer_output = self
                .boundary_layer
                .as_ref()
                .map(|module| {
                    window
                        .prepare_batch(&module.query_plan, batch.clone(), &mut workspace)?
                        .execute(execution, &mut workspace)
                })
                .transpose()
                .map_err(|error| PhysicsError::Meteorology(format!("{error:?}")))?;
            let stability_output = (self.subgrid_orography.is_some()
                && self.boundary_layer.is_some())
            .then(|| {
                window
                    .prepare_stability_batch(batch.clone(), &mut workspace)?
                    .execute(execution, &mut workspace)
            })
            .transpose()
            .map_err(|error| PhysicsError::Meteorology(format!("{error:?}")))?;
            let mesoscale_output = self
                .mesoscale
                .as_ref()
                .map(|_| {
                    window
                        .prepare_mesoscale_batch(batch, &mut workspace)?
                        .execute(execution, &mut workspace)
                })
                .transpose()
                .map_err(|error| PhysicsError::Meteorology(format!("{error:?}")))?;
            let terrain_samples = self
                .subgrid_orography
                .as_ref()
                .map(|module| {
                    group
                        .iter()
                        .map(|(particle_index, _, _)| {
                            module.dataset.sample_for_meteorology_cell(
                                &window.frames.before.metadata().grid,
                                particles.longitude_degrees[*particle_index],
                                particles.latitude_degrees[*particle_index],
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()
                .map_err(|error| PhysicsError::Meteorology(error.to_string()))?;
            for (row_index, (particle_index, first_half_ns, second_half_ns)) in
                group.into_iter().enumerate()
            {
                if let Some(output) = &boundary_layer_output {
                    let row = output.row(row_index).ok_or_else(|| {
                        PhysicsError::Meteorology("physics query row missing".into())
                    })?;
                    if row.status() != SampleStatus::Ok {
                        return Err(PhysicsError::Meteorology(format!(
                            "boundary-layer meteorology is unavailable for particle {}: {:?}",
                            particles.id[particle_index].0,
                            row.status(),
                        )));
                    }
                    let field = |canonical| {
                        row.value(&FieldKey::Canonical(canonical)).ok_or_else(|| {
                            PhysicsError::Meteorology(format!(
                                "boundary-layer field {canonical:?} is unavailable"
                            ))
                        })
                    };
                    let raw_terrain = field(CanonicalField::GeometricTerrainHeight)?;
                    let (terrain, boundary_layer_height_m) = match (
                        terrain_samples
                            .as_ref()
                            .and_then(|samples| samples.get(row_index)),
                        stability_output.as_ref(),
                    ) {
                        (Some(terrain_sample), Some(stability)) => {
                            if stability.status().get(row_index) != Some(SampleStatus::Ok) {
                                return Err(PhysicsError::Meteorology(format!(
                                    "terrain stability is unavailable for particle {}: {:?}",
                                    particles.id[particle_index].0,
                                    stability.status().get(row_index),
                                )));
                            }
                            let frequency_squared = stability
                                .brunt_vaisala_frequency_squared_s2(row_index)
                                .ok_or_else(|| {
                                    PhysicsError::Meteorology(
                                        "terrain stability row is unavailable".into(),
                                    )
                                })?;
                            let horizontal_speed = field(CanonicalField::EastwardWind)?
                                .hypot(field(CanonicalField::NorthwardWind)?);
                            let increment = terrain_mixing_increment_m(
                                terrain_sample.elevation_standard_deviation_m,
                                frequency_squared,
                                horizontal_speed,
                            )?;
                            (
                                raw_terrain + terrain_sample.terrain_anomaly_m(),
                                field(CanonicalField::BoundaryLayerHeight)? + increment,
                            )
                        }
                        (None, None) => (raw_terrain, field(CanonicalField::BoundaryLayerHeight)?),
                        _ => {
                            return Err(PhysicsError::Scientific(
                                "sub-grid terrain preparation is inconsistent".into(),
                            ));
                        }
                    };
                    boundary_layer_inputs.push(BoundaryLayerMotionInput {
                        particle_index,
                        first_half_ns,
                        second_half_ns,
                        environment: BoundaryLayerEnvironment {
                            height_agl_m: particles.height_asl_m[particle_index] - terrain,
                            boundary_layer_height_m,
                            friction_velocity_m_s: field(CanonicalField::FrictionVelocity)?,
                            monin_obukhov_length_m: field(CanonicalField::MoninObukhovLength)?,
                            sensible_heat_flux_w_m2: field(CanonicalField::SensibleHeatFlux)?,
                            roughness_length_m: field(CanonicalField::AerodynamicRoughnessLength)?,
                            air_temperature_k: field(CanonicalField::AirTemperature)?,
                            air_density_kg_m3: field(CanonicalField::AirDensity)?,
                        },
                    });
                }
                if let Some(output) = &mesoscale_output {
                    if output.status().get(row_index) != Some(SampleStatus::Ok) {
                        return Err(PhysicsError::Meteorology(format!(
                            "mesoscale meteorology is unavailable for particle {}: {:?}",
                            particles.id[particle_index].0,
                            output.status().get(row_index),
                        )));
                    }
                    mesoscale_inputs.push(MesoscaleMotionInput {
                        particle_index,
                        first_half_ns,
                        second_half_ns,
                        variance_m2_s2: output.variance_m2_s2(row_index).ok_or_else(|| {
                            PhysicsError::Meteorology(
                                "mesoscale variance row is unavailable".into(),
                            )
                        })?,
                        native_interval_seconds: output.native_interval_seconds(),
                    });
                }
            }
        }

        let mut candidate_limit_ns = None::<i64>;
        for input in &boundary_layer_inputs {
            let active_ns = input
                .first_half_ns
                .checked_add(input.second_half_ns)
                .ok_or(PhysicsError::SubstepUnderflow)?;
            let stable_ns = maximum_stable_step_ns(input.environment)
                .map_err(|message| PhysicsError::Scientific(message.into()))?;
            tighten_dynamic_limit(
                &mut candidate_limit_ns,
                input.particle_index,
                active_ns,
                stable_ns,
                substep_start,
                active_start_times,
            )?;
        }
        let correlation_interval_fraction = self
            .mesoscale
            .as_ref()
            .map(|module| module.correlation_interval_fraction);
        for input in &mesoscale_inputs {
            let active_ns = input
                .first_half_ns
                .checked_add(input.second_half_ns)
                .ok_or(PhysicsError::SubstepUnderflow)?;
            let stable_ns = mesoscale_stable_step_ns(
                input.native_interval_seconds,
                correlation_interval_fraction.ok_or(PhysicsError::InvalidParticleBatch)?,
            )?;
            tighten_dynamic_limit(
                &mut candidate_limit_ns,
                input.particle_index,
                active_ns,
                stable_ns,
                substep_start,
                active_start_times,
            )?;
        }
        if let Some(maximum_ns) = candidate_limit_ns {
            return Err(PhysicsError::SubstepTooLarge { maximum_ns });
        }

        let mut boundary_layer_end = Vec::with_capacity(boundary_layer_inputs.len());
        for input in boundary_layer_inputs {
            let particle_index = input.particle_index;
            let previous = if particles.elapsed_age_ns[particle_index] == 0 {
                sample_stationary_velocity(
                    input.environment,
                    LangevinKey {
                        seed,
                        particle: particles.id[particle_index],
                        macro_step: 0,
                        substep: 0,
                        draw_index: BIRTH_DRAW_INDEX,
                    },
                )
                .map_err(|message| PhysicsError::Scientific(message.into()))?
            } else {
                particles
                    .boundary_layer_velocity(particle_index)
                    .map_err(|_| PhysicsError::InvalidParticleBatch)?
            };
            let key = LangevinKey {
                seed,
                particle: particles.id[particle_index],
                macro_step,
                substep,
                draw_index: MIDPOINT_DRAW_INDEX,
            };
            let midpoint_velocity = if input.first_half_ns == 0 {
                previous
            } else {
                update_velocity(
                    previous,
                    input.environment,
                    input.first_half_ns as f64 * 1.0e-9,
                    direction,
                    key,
                )
                .map_err(|message| PhysicsError::Scientific(message.into()))?
            };
            let end_velocity = update_velocity(
                midpoint_velocity,
                input.environment,
                input.second_half_ns as f64 * 1.0e-9,
                direction,
                LangevinKey {
                    draw_index: ENDPOINT_DRAW_INDEX,
                    ..key
                },
            )
            .map_err(|message| PhysicsError::Scientific(message.into()))?;
            particles
                .set_boundary_layer_velocity(particle_index, midpoint_velocity)
                .map_err(|_| PhysicsError::InvalidParticleBatch)?;
            boundary_layer_end.push((particle_index, end_velocity));
        }
        let mut mesoscale_end = Vec::with_capacity(mesoscale_inputs.len());
        for input in mesoscale_inputs {
            let particle_index = input.particle_index;
            let previous = if particles.elapsed_age_ns[particle_index] == 0 {
                sample_stationary_mesoscale_velocity(
                    input.variance_m2_s2,
                    MesoscaleKey {
                        seed,
                        particle: particles.id[particle_index],
                        macro_step: 0,
                        substep: 0,
                        draw_index: BIRTH_DRAW_INDEX,
                    },
                )
                .map_err(|message| PhysicsError::Scientific(message.into()))?
            } else {
                particles
                    .mesoscale_velocity(particle_index)
                    .map_err(|_| PhysicsError::InvalidParticleBatch)?
            };
            let key = MesoscaleKey {
                seed,
                particle: particles.id[particle_index],
                macro_step,
                substep,
                draw_index: MIDPOINT_DRAW_INDEX,
            };
            let fraction =
                correlation_interval_fraction.ok_or(PhysicsError::InvalidParticleBatch)?;
            let midpoint_velocity = if input.first_half_ns == 0 {
                previous
            } else {
                update_mesoscale_velocity(
                    previous,
                    input.variance_m2_s2,
                    input.first_half_ns as f64 * 1.0e-9,
                    input.native_interval_seconds,
                    fraction,
                    key,
                )
                .map_err(|message| PhysicsError::Scientific(message.into()))?
            };
            let end_velocity = update_mesoscale_velocity(
                midpoint_velocity,
                input.variance_m2_s2,
                input.second_half_ns as f64 * 1.0e-9,
                input.native_interval_seconds,
                fraction,
                MesoscaleKey {
                    draw_index: ENDPOINT_DRAW_INDEX,
                    ..key
                },
            )
            .map_err(|message| PhysicsError::Scientific(message.into()))?;
            particles
                .set_mesoscale_velocity(particle_index, midpoint_velocity)
                .map_err(|_| PhysicsError::InvalidParticleBatch)?;
            mesoscale_end.push((particle_index, end_velocity));
        }
        Ok(PreparedMotion {
            boundary_layer_end,
            mesoscale_end,
        })
    }
}

fn mesoscale_stable_step_ns(
    native_interval_seconds: f64,
    correlation_interval_fraction: f64,
) -> Result<i64, PhysicsError> {
    let nanoseconds = (native_interval_seconds * correlation_interval_fraction * 1.0e9).floor();
    if !nanoseconds.is_finite() || nanoseconds < 1.0 || nanoseconds > i64::MAX as f64 {
        Err(PhysicsError::SubstepUnderflow)
    } else {
        Ok(nanoseconds as i64)
    }
}

fn tighten_dynamic_limit(
    candidate: &mut Option<i64>,
    particle_index: usize,
    active_ns: i64,
    stable_ns: i64,
    substep_start: Timestamp,
    active_start_times: &[Timestamp],
) -> Result<(), PhysicsError> {
    if active_ns <= stable_ns {
        return Ok(());
    }
    let inactive_prefix_ns = signed_duration_between(
        substep_start,
        *active_start_times
            .get(particle_index)
            .ok_or(PhysicsError::InvalidParticleBatch)?,
    )
    .map_err(|_| PhysicsError::SubstepUnderflow)?
    .0
    .unsigned_abs();
    let limit = i64::try_from(inactive_prefix_ns)
        .map_err(|_| PhysicsError::SubstepUnderflow)?
        .checked_add(stable_ns)
        .ok_or(PhysicsError::SubstepUnderflow)?;
    *candidate = Some(candidate.map_or(limit, |old| old.min(limit)));
    Ok(())
}

fn duration_ns(module: &ResolvedPhysicsModule) -> Result<i64, PhysicsError> {
    let seconds = module
        .maximum_substep
        .as_ref()
        .ok_or(PhysicsError::SubstepUnderflow)?
        .value_si();
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(PhysicsError::SubstepUnderflow);
    }
    let nanoseconds = (seconds * 1.0e9).round();
    if nanoseconds < 1.0 || nanoseconds > i64::MAX as f64 {
        return Err(PhysicsError::SubstepUnderflow);
    }
    Ok(nanoseconds as i64)
}

fn boundary_layer_fields() -> [CanonicalField; 10] {
    [
        CanonicalField::EastwardWind,
        CanonicalField::NorthwardWind,
        CanonicalField::BoundaryLayerHeight,
        CanonicalField::FrictionVelocity,
        CanonicalField::MoninObukhovLength,
        CanonicalField::SensibleHeatFlux,
        CanonicalField::AerodynamicRoughnessLength,
        CanonicalField::AirTemperature,
        CanonicalField::AirDensity,
        CanonicalField::GeometricTerrainHeight,
    ]
}

fn terrain_mixing_increment_m(
    elevation_standard_deviation_m: f64,
    brunt_vaisala_frequency_squared_s2: f64,
    horizontal_speed_m_s: f64,
) -> Result<f64, PhysicsError> {
    if !elevation_standard_deviation_m.is_finite()
        || elevation_standard_deviation_m < 0.0
        || !brunt_vaisala_frequency_squared_s2.is_finite()
        || !horizontal_speed_m_s.is_finite()
        || horizontal_speed_m_s < 0.0
    {
        return Err(PhysicsError::Scientific(
            "sub-grid terrain input is invalid".into(),
        ));
    }
    if brunt_vaisala_frequency_squared_s2 <= 0.0 {
        return Ok(elevation_standard_deviation_m);
    }
    let frequency = brunt_vaisala_frequency_squared_s2
        .sqrt()
        .max(BUOYANCY_FREQUENCY_FLOOR_S_INV);
    Ok(elevation_standard_deviation_m
        .min(STABLE_KINETIC_CAP_MULTIPLIER * horizontal_speed_m_s / frequency))
}

/// Physical-process configuration or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhysicsError {
    /// A selected module belongs to a later M6 stage.
    ModuleStageNotAvailable {
        /// Requested module.
        module: PhysicsModuleId,
        /// Stage that owns its executor.
        stage: &'static str,
    },
    /// A stability limit cannot form a positive integer-nanosecond substep.
    SubstepUnderflow,
    /// A sampled correlation-time limit requires retrying with a shorter step.
    SubstepTooLarge {
        /// Largest allowed candidate duration in nanoseconds.
        maximum_ns: i64,
    },
    /// The particle SoA is inconsistent.
    InvalidParticleBatch,
    /// Meteorological planning or sampling failed.
    Meteorology(String),
    /// A scientific invariant failed during process evaluation.
    Scientific(String),
    /// A substep count cannot fit in memory.
    ResourceLimit,
}

impl PhysicsError {
    /// Returns the stable machine-readable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ModuleStageNotAvailable { .. } => "physics.module_stage_not_available",
            Self::SubstepUnderflow => "physics.substep_underflow",
            Self::SubstepTooLarge { .. } => "physics.substep_too_large",
            Self::InvalidParticleBatch => "physics.invalid_particle_batch",
            Self::Meteorology(_) => "physics.meteorology",
            Self::Scientific(_) => "physics.scientific_error",
            Self::ResourceLimit => "physics.resource_limit",
        }
    }
}

impl std::fmt::Display for PhysicsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModuleStageNotAvailable { module, stage } => {
                write!(formatter, "physics module '{module}' is staged for {stage}")
            }
            Self::SubstepUnderflow => {
                formatter.write_str("physics substep is below one nanosecond")
            }
            Self::SubstepTooLarge { maximum_ns } => {
                write!(
                    formatter,
                    "physics substep exceeds dynamic limit {maximum_ns} ns"
                )
            }
            Self::InvalidParticleBatch => formatter.write_str("invalid particle batch"),
            Self::Meteorology(message) | Self::Scientific(message) => formatter.write_str(message),
            Self::ResourceLimit => formatter.write_str("physics substep resource limit exceeded"),
        }
    }
}

impl std::error::Error for PhysicsError {}

/// Returns indices active during one direction-aware common substep.
pub(crate) fn active_indices_for_substep(
    particles: &ParticleBatch,
    start_times: &[Timestamp],
    substep_start: Timestamp,
    substep_end: Timestamp,
    direction: Direction,
) -> Result<Vec<usize>, PhysicsError> {
    if start_times.len()
        != particles
            .len()
            .map_err(|_| PhysicsError::InvalidParticleBatch)?
    {
        return Err(PhysicsError::InvalidParticleBatch);
    }
    Ok(start_times
        .iter()
        .enumerate()
        .filter(|(index, birth)| {
            particles.status[*index] == ParticleStatus::Alive
                && match direction {
                    Direction::Forward => **birth < substep_end,
                    Direction::Backward => **birth > substep_end,
                }
                && substep_start != substep_end
        })
        .map(|(index, _)| index)
        .collect())
}

/// Clamps particle start times to one direction-aware common substep.
pub(crate) fn clamp_start_times(
    start_times: &[Timestamp],
    substep_start: Timestamp,
    substep_end: Timestamp,
    direction: Direction,
) -> Vec<Timestamp> {
    start_times
        .iter()
        .map(|time| match direction {
            Direction::Forward if *time <= substep_start => substep_start,
            Direction::Forward if *time >= substep_end => substep_end,
            Direction::Backward if *time >= substep_start => substep_start,
            Direction::Backward if *time <= substep_end => substep_end,
            _ => *time,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use trajecta_case::model::population::{PopulationId, ReleaseEventId};

    use crate::particle::{
        MotionProcessStore, ParticleId, ParticleOrigin, SubstanceAdjointStore, SubstanceMassStore,
        TerminationReason,
    };

    #[test]
    fn partition_preserves_exact_signed_nanoseconds() {
        let pipeline = PhysicsPipeline {
            subgrid_orography: None,
            boundary_layer: None,
            mesoscale: None,
            common_maximum_substep_ns: 30_000_000_000,
        };
        assert_eq!(
            pipeline.partition(SignedDuration(75_000_000_000)).unwrap(),
            vec![
                SignedDuration(30_000_000_000),
                SignedDuration(30_000_000_000),
                SignedDuration(15_000_000_000),
            ]
        );
        assert_eq!(
            pipeline.partition(SignedDuration(-61_000_000_001)).unwrap(),
            vec![
                SignedDuration(-30_000_000_000),
                SignedDuration(-30_000_000_000),
                SignedDuration(-1_000_000_001),
            ]
        );
    }

    #[test]
    fn terminated_particle_keeps_midpoint_state_instead_of_future_endpoint_state() {
        let mut particles = one_particle_batch();
        particles
            .set_boundary_layer_velocity(0, [1.0, 2.0, 3.0])
            .unwrap();
        particles.status[0] = ParticleStatus::Terminated {
            reason: TerminationReason::ModelTop,
        };
        PreparedMotion {
            boundary_layer_end: vec![(0, [4.0, 5.0, 6.0])],
            mesoscale_end: Vec::new(),
        }
        .finish(&mut particles, &[])
        .unwrap();
        assert_eq!(
            particles.boundary_layer_velocity(0).unwrap(),
            [1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn reflected_surface_motion_reverses_only_the_vertical_process_velocity() {
        let mut particles = one_particle_batch();
        PreparedMotion {
            boundary_layer_end: vec![(0, [4.0, 5.0, -6.0])],
            mesoscale_end: Vec::new(),
        }
        .finish(&mut particles, &[0])
        .unwrap();
        assert_eq!(
            particles.boundary_layer_velocity(0).unwrap(),
            [4.0, 5.0, 6.0]
        );
    }

    fn one_particle_batch() -> ParticleBatch {
        let population = PopulationId("p".into());
        let event = ReleaseEventId("e".into());
        ParticleBatch {
            id: vec![ParticleId::for_release(&population, &event, 0)],
            population_id: vec![population],
            origin: vec![ParticleOrigin::Release { event_id: event }],
            birth_time: vec![Timestamp::UNIX_EPOCH],
            longitude_degrees: vec![0.0],
            latitude_degrees: vec![0.0],
            height_asl_m: vec![100.0],
            integration_offset_ns: vec![0],
            elapsed_age_ns: vec![0],
            dry_air_mass_kg: vec![1.0],
            status: vec![ParticleStatus::Alive],
            termination: vec![None],
            mass: SubstanceMassStore::default(),
            adjoint: SubstanceAdjointStore::default(),
            motion: MotionProcessStore::default(),
        }
    }
}
