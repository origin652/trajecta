//! # Contract: deterministic dry-air seeding and mass accounting
//!
//! Converts an immutable meteorological dry-air snapshot into equal-carrier-
//! mass particles and adjudicates every domain-fill transition with the frozen
//! relative-plus-ULP conservation gate.

use std::collections::BTreeMap;

use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, PopulationId,
};
use trajecta_case::model::time::{Direction, Timestamp};
use trajecta_met::derive::domain_fill::{
    AirMassColumn, AirMassLayer, AirMassSnapshot, BoundaryFaceLayer, BoundarySide,
};

use crate::clock::{SignedDuration, add_timestamp};
use crate::manifest::MassLedgerRecord;
use crate::particle::{
    BoundaryFaceId, ParticleBatch, ParticleId, ParticleOrigin, ParticleState, ParticleStatus,
};
use crate::reference::{
    ReferenceError, dry_air_density_kg_m3, mass_balance_tolerance_kg, neumaier_sum,
};
use crate::rng::{
    CounterRng, DOMAIN_FILL_BOUNDARY_TANGENTIAL_DIMENSION, DOMAIN_FILL_BOUNDARY_VERTICAL_DIMENSION,
    DOMAIN_FILL_LATITUDE_DIMENSION, DOMAIN_FILL_LONGITUDE_DIMENSION,
    DOMAIN_FILL_MASS_STRATUM_DIMENSION, DOMAIN_FILL_PRESSURE_DIMENSION, RandomKey, StableRandomId,
};

use super::{OzoneAssignmentInput, OzoneAssignmentRule, PopulationError};

/// Stable lifecycle identity used to key initial domain-fill samples.
pub const DOMAIN_INITIAL_LIFECYCLE_EVENT_ID: &str = "domain-fill-initial/v1";
/// Stable lifecycle prefix used to key finite-domain boundary births.
pub const DOMAIN_BOUNDARY_LIFECYCLE_EVENT_ID: &str = "domain-fill-boundary/v1";

/// Result of deterministic initial dry-air population construction.
#[derive(Clone, Debug, PartialEq)]
pub struct InitialAirMassSeeding {
    /// Complete initial particle batch.
    pub particles: ParticleBatch,
    /// Equal dry-air carrier mass represented by every generated particle.
    pub carrier_mass_per_particle_kg: f64,
    /// Dry-air mass below one complete carrier retained in the initial ledger.
    pub residual_mass_kg: f64,
    /// Fixed-order total dry-air mass of the source snapshot.
    pub total_dry_air_mass_kg: f64,
}

/// Result of exact-count seeding inside the PV60-eligible dry-air mass only.
#[derive(Clone, Debug, PartialEq)]
pub struct InitialOzoneSeeding {
    /// Complete initial ozone-carrier particle batch.
    pub particles: ParticleBatch,
    /// Equal eligible dry-air carrier mass represented by every particle.
    pub carrier_mass_per_particle_kg: f64,
    /// Eligible dry-air mass below one complete carrier retained in the ledger.
    pub residual_mass_kg: f64,
    /// Fixed-order total dry-air mass inside the frozen ozone eligibility mask.
    pub total_eligible_dry_air_mass_kg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ResolvedMassTarget {
    particle_count_u64: u64,
    carrier_mass_per_particle_kg: f64,
    residual_mass_kg: f64,
}

/// Seeds an initial population by deterministic equal-mass stratification.
pub fn seed_initial_air_mass(
    specification: &DomainFillAirMassSpec,
    snapshot: &AirMassSnapshot,
    random_seed: u64,
) -> Result<InitialAirMassSeeding, AirMassPopulationError> {
    validate_identity(specification, snapshot)?;
    let total = snapshot.total_dry_air_mass_kg;
    if !total.is_finite() || total <= 0.0 {
        return Err(AirMassPopulationError::InvalidMassBudget);
    }
    let target = resolve_mass_target(specification, total)?;
    let particle_count_u64 = target.particle_count_u64;
    let carrier_mass_per_particle_kg = target.carrier_mass_per_particle_kg;
    let residual_mass_kg = target.residual_mass_kg;
    let particle_count =
        usize::try_from(particle_count_u64).map_err(|_| AirMassPopulationError::ResourceLimit)?;
    let strata = flatten_mass_strata(snapshot)?;
    let mut particles = ParticleBatch::default();
    reserve_batch(&mut particles, particle_count)?;
    let population_key = StableRandomId::from_text(&specification.id.0);
    let lifecycle_key = StableRandomId::from_text(DOMAIN_INITIAL_LIFECYCLE_EVENT_ID);

    for ordinal in 0..particle_count_u64 {
        let id =
            ParticleId::for_domain_initial(&specification.id, &specification.domain_id, ordinal);
        let random = |dimension| {
            CounterRng::sample_unit(RandomKey {
                seed: random_seed,
                population: population_key,
                lifecycle_event: lifecycle_key,
                particle: id,
                sampling_dimension: dimension,
                draw_index: 0,
            })
        };
        let stratum_width = if specification.target_particle_count.is_some() {
            total / particle_count_u64 as f64
        } else {
            carrier_mass_per_particle_kg
        };
        let mass_coordinate = (ordinal as f64).mul_add(
            stratum_width,
            random(DOMAIN_FILL_MASS_STRATUM_DIMENSION) * stratum_width,
        );
        let (column, layer) = locate_mass_coordinate(&strata, mass_coordinate)?;
        let longitude_degrees = normalize_longitude(
            (column.east_degrees - column.west_degrees)
                .mul_add(random(DOMAIN_FILL_LONGITUDE_DIMENSION), column.west_degrees),
        );
        let sin_south = column.south_degrees.to_radians().sin();
        let sin_north = column.north_degrees.to_radians().sin();
        let sin_latitude =
            (sin_north - sin_south).mul_add(random(DOMAIN_FILL_LATITUDE_DIMENSION), sin_south);
        let latitude_degrees = sin_latitude.clamp(-1.0, 1.0).asin().to_degrees();
        let pressure_fraction = random(DOMAIN_FILL_PRESSURE_DIMENSION);
        let source_height_asl_m = sample_height_from_pressure_fraction(layer, pressure_fraction)?;
        let local_terrain_height_asl_m = snapshot
            .terrain_height_asl_m_at(longitude_degrees, latitude_degrees)
            .map_err(|_| AirMassPopulationError::InvalidSample)?;
        let local_transport_floor_height_asl_m = snapshot
            .transport_floor_height_asl_m_at(longitude_degrees, latitude_degrees)
            .map_err(|_| AirMassPopulationError::InvalidSample)?;
        let transport_floor_displacement_m =
            local_transport_floor_height_asl_m - column.transport_floor_height_asl_m;
        let height_asl_m = source_height_asl_m + transport_floor_displacement_m;
        let local_lower_height_asl_m = layer.lower_height_asl_m + transport_floor_displacement_m;
        let local_upper_height_asl_m = layer.upper_height_asl_m + transport_floor_displacement_m;
        if !longitude_degrees.is_finite()
            || !latitude_degrees.is_finite()
            || !height_asl_m.is_finite()
            || !local_transport_floor_height_asl_m.is_finite()
            || !local_lower_height_asl_m.is_finite()
            || !local_upper_height_asl_m.is_finite()
            || local_transport_floor_height_asl_m <= local_terrain_height_asl_m
            || height_asl_m <= local_transport_floor_height_asl_m
            || height_asl_m < local_lower_height_asl_m
            || height_asl_m > local_upper_height_asl_m
        {
            return Err(AirMassPopulationError::InvalidSample);
        }
        push_particle(
            &mut particles,
            id,
            &specification.id,
            &specification.domain_id,
            snapshot.time,
            longitude_degrees,
            latitude_degrees,
            height_asl_m,
            carrier_mass_per_particle_kg,
        );
    }
    particles
        .validate()
        .map_err(|_| AirMassPopulationError::InvalidParticleBatch)?;
    let represented =
        neumaier_sum(&particles.dry_air_mass_kg).map_err(AirMassPopulationError::Reference)?;
    let reconstructed = neumaier_sum(&[represented, residual_mass_kg])
        .map_err(AirMassPopulationError::Reference)?;
    let tolerance = mass_balance_tolerance_kg(total.max(reconstructed), false)
        .map_err(AirMassPopulationError::Reference)?;
    if (total - reconstructed).abs() > tolerance {
        return Err(AirMassPopulationError::MassImbalance);
    }
    Ok(InitialAirMassSeeding {
        particles,
        carrier_mass_per_particle_kg,
        residual_mass_kg,
        total_dry_air_mass_kg: total,
    })
}

fn resolve_mass_target(
    specification: &DomainFillAirMassSpec,
    total_mass_kg: f64,
) -> Result<ResolvedMassTarget, AirMassPopulationError> {
    if !total_mass_kg.is_finite() || total_mass_kg <= 0.0 {
        return Err(AirMassPopulationError::InvalidMassBudget);
    }
    match (
        specification.target_particle_count,
        specification.target_dry_air_mass_per_particle.as_ref(),
    ) {
        (Some(count), None) if count > 0 => {
            let mass = total_mass_kg / count as f64;
            if !mass.is_finite() || mass <= 0.0 {
                return Err(AirMassPopulationError::InvalidMassBudget);
            }
            Ok(ResolvedMassTarget {
                particle_count_u64: count,
                carrier_mass_per_particle_kg: mass,
                residual_mass_kg: 0.0,
            })
        }
        (None, Some(target)) => {
            let mass = target.value_si();
            if !mass.is_finite() || mass <= 0.0 {
                return Err(AirMassPopulationError::InvalidMassBudget);
            }
            let count_f64 = (total_mass_kg / mass).floor();
            if !count_f64.is_finite() || count_f64 < 0.0 || count_f64 > u64::MAX as f64 {
                return Err(AirMassPopulationError::ResourceLimit);
            }
            let count = count_f64 as u64;
            let represented = mass * count as f64;
            let residual = (total_mass_kg - represented).max(0.0);
            if !represented.is_finite()
                || !residual.is_finite()
                || residual >= mass * (1.0 + 16.0 * f64::EPSILON)
            {
                return Err(AirMassPopulationError::InvalidMassBudget);
            }
            Ok(ResolvedMassTarget {
                particle_count_u64: count,
                carrier_mass_per_particle_kg: mass,
                residual_mass_kg: residual,
            })
        }
        _ => Err(AirMassPopulationError::InvalidConfiguration),
    }
}

#[derive(Clone, Copy)]
struct MassStratum<'a> {
    upper_cumulative_kg: f64,
    column: &'a AirMassColumn,
    layer: &'a AirMassLayer,
}

#[derive(Clone, Copy)]
struct EligibleOzoneMassStratum<'a> {
    upper_cumulative_kg: f64,
    column: &'a AirMassColumn,
    layer: &'a AirMassLayer,
    south_degrees: f64,
    north_degrees: f64,
    eligible_lower_pressure_pa: f64,
    potential_vorticity_pvu: f64,
}

/// Seeds exactly the requested number of particles from PV60-eligible dry-air
/// mass instead of generating a full-domain candidate population and dropping
/// particles outside the mask.
pub fn seed_initial_stratospheric_ozone(
    specification: &DomainFillStratosphericOzoneSpec,
    snapshot: &AirMassSnapshot,
    rule: &dyn OzoneAssignmentRule,
    random_seed: u64,
) -> Result<InitialOzoneSeeding, PopulationError> {
    validate_identity(&specification.air_mass, snapshot).map_err(PopulationError::AirMass)?;
    if specification.ozone_rule != rule.model_id()
        || specification.ozone_substance.0.trim().is_empty()
    {
        return Err(PopulationError::InvalidConfiguration);
    }
    let strata = flatten_eligible_ozone_mass_strata(snapshot)?;
    let total = strata
        .last()
        .map(|stratum| stratum.upper_cumulative_kg)
        .ok_or(PopulationError::NoEligibleOzoneMass)?;
    let target =
        resolve_mass_target(&specification.air_mass, total).map_err(PopulationError::AirMass)?;
    let particle_count =
        usize::try_from(target.particle_count_u64).map_err(|_| PopulationError::ResourceLimit)?;
    let mut particles = ParticleBatch::default();
    reserve_batch(&mut particles, particle_count).map_err(PopulationError::AirMass)?;
    let mut ozone_mass_kg = Vec::new();
    ozone_mass_kg
        .try_reserve_exact(particle_count)
        .map_err(|_| PopulationError::ResourceLimit)?;
    let population_key = StableRandomId::from_text(&specification.air_mass.id.0);
    let lifecycle_key = StableRandomId::from_text(DOMAIN_INITIAL_LIFECYCLE_EVENT_ID);

    for ordinal in 0..target.particle_count_u64 {
        let id = ParticleId::for_domain_initial(
            &specification.air_mass.id,
            &specification.air_mass.domain_id,
            ordinal,
        );
        let random = |dimension| {
            CounterRng::sample_unit(RandomKey {
                seed: random_seed,
                population: population_key,
                lifecycle_event: lifecycle_key,
                particle: id,
                sampling_dimension: dimension,
                draw_index: 0,
            })
        };
        let stratum_width = if specification.air_mass.target_particle_count.is_some() {
            total / target.particle_count_u64 as f64
        } else {
            target.carrier_mass_per_particle_kg
        };
        let mass_coordinate = (ordinal as f64).mul_add(
            stratum_width,
            random(DOMAIN_FILL_MASS_STRATUM_DIMENSION) * stratum_width,
        );
        let stratum = locate_eligible_ozone_mass_coordinate(&strata, mass_coordinate)?;
        let longitude_degrees = normalize_longitude(
            (stratum.column.east_degrees - stratum.column.west_degrees).mul_add(
                random(DOMAIN_FILL_LONGITUDE_DIMENSION),
                stratum.column.west_degrees,
            ),
        );
        let sin_south = stratum.south_degrees.to_radians().sin();
        let sin_north = stratum.north_degrees.to_radians().sin();
        let sin_latitude =
            (sin_north - sin_south).mul_add(random(DOMAIN_FILL_LATITUDE_DIMENSION), sin_south);
        let latitude_degrees = sin_latitude.clamp(-1.0, 1.0).asin().to_degrees();
        let height_asl_m = sample_height_from_pressure_bounds(
            stratum.layer,
            stratum.layer.upper_pressure_pa,
            stratum.eligible_lower_pressure_pa,
            random(DOMAIN_FILL_PRESSURE_DIMENSION),
        )
        .map_err(PopulationError::AirMass)?;
        let local_transport_floor_height_asl_m = snapshot
            .transport_floor_height_asl_m_at(longitude_degrees, latitude_degrees)
            .map_err(|_| PopulationError::AirMass(AirMassPopulationError::InvalidSample))?;
        if !longitude_degrees.is_finite()
            || !latitude_degrees.is_finite()
            || !height_asl_m.is_finite()
            || height_asl_m <= local_transport_floor_height_asl_m
        {
            return Err(PopulationError::AirMass(
                AirMassPopulationError::InvalidSample,
            ));
        }
        let assignment = rule.assign(OzoneAssignmentInput {
            carrier_dry_air_mass_kg: target.carrier_mass_per_particle_kg,
            height_asl_m,
            latitude_degrees,
            potential_vorticity_pvu: stratum.potential_vorticity_pvu,
        })?;
        if !assignment.ozone_mass_kg.is_finite() || assignment.ozone_mass_kg <= 0.0 {
            return Err(PopulationError::OzoneEligibilityMismatch);
        }
        push_particle(
            &mut particles,
            id,
            &specification.air_mass.id,
            &specification.air_mass.domain_id,
            snapshot.time,
            longitude_degrees,
            latitude_degrees,
            height_asl_m,
            target.carrier_mass_per_particle_kg,
        );
        ozone_mass_kg.push(assignment.ozone_mass_kg);
    }
    particles
        .mass
        .mass_kg
        .insert(specification.ozone_substance.clone(), ozone_mass_kg);
    particles
        .validate()
        .map_err(|_| PopulationError::AirMass(AirMassPopulationError::InvalidParticleBatch))?;
    let represented =
        neumaier_sum(&particles.dry_air_mass_kg).map_err(PopulationError::OzoneReference)?;
    let reconstructed = neumaier_sum(&[represented, target.residual_mass_kg])
        .map_err(PopulationError::OzoneReference)?;
    let tolerance = mass_balance_tolerance_kg(total.max(reconstructed), false)
        .map_err(PopulationError::OzoneReference)?;
    if (total - reconstructed).abs() > tolerance {
        return Err(PopulationError::MassImbalance);
    }
    Ok(InitialOzoneSeeding {
        particles,
        carrier_mass_per_particle_kg: target.carrier_mass_per_particle_kg,
        residual_mass_kg: target.residual_mass_kg,
        total_eligible_dry_air_mass_kg: total,
    })
}

fn flatten_eligible_ozone_mass_strata(
    snapshot: &AirMassSnapshot,
) -> Result<Vec<EligibleOzoneMassStratum<'_>>, PopulationError> {
    let mut strata = Vec::new();
    let mut sum = 0.0_f64;
    let mut correction = 0.0_f64;
    for column in &snapshot.columns {
        let full_latitude_measure =
            column.north_degrees.to_radians().sin() - column.south_degrees.to_radians().sin();
        if !full_latitude_measure.is_finite() || full_latitude_measure <= 0.0 {
            return Err(PopulationError::InvalidConfiguration);
        }
        for layer in &column.layers {
            let Some(eligible_lower_pressure_pa) =
                ozone_eligible_lower_pressure(layer).map_err(PopulationError::AirMass)?
            else {
                continue;
            };
            let pv = layer
                .potential_vorticity_pvu
                .ok_or(PopulationError::MissingOzoneDiagnostic)?;
            if !pv.is_finite() {
                return Err(PopulationError::MissingOzoneDiagnostic);
            }
            let latitude_bounds = if pv > crate::science::OZONE_RULE_MINIMUM_PV_PVU {
                (column.south_degrees.max(0.0), column.north_degrees)
            } else if pv < -crate::science::OZONE_RULE_MINIMUM_PV_PVU {
                (column.south_degrees, column.north_degrees.min(0.0))
            } else {
                continue;
            };
            if latitude_bounds.1 <= latitude_bounds.0 {
                continue;
            }
            let eligible_latitude_measure =
                latitude_bounds.1.to_radians().sin() - latitude_bounds.0.to_radians().sin();
            let horizontal_fraction = eligible_latitude_measure / full_latitude_measure;
            if !horizontal_fraction.is_finite()
                || horizontal_fraction <= 0.0
                || horizontal_fraction > 1.0 + 16.0 * f64::EPSILON
            {
                return Err(PopulationError::InvalidConfiguration);
            }
            let pressure_fraction = (eligible_lower_pressure_pa - layer.upper_pressure_pa)
                / layer.pressure_thickness_pa();
            let mass = layer.dry_air_mass_kg * horizontal_fraction * pressure_fraction;
            if !mass.is_finite() || mass <= 0.0 {
                return Err(PopulationError::InvalidConfiguration);
            }
            let next = sum + mass;
            if sum.abs() >= mass.abs() {
                correction += (sum - next) + mass;
            } else {
                correction += (mass - next) + sum;
            }
            sum = next;
            let upper = sum + correction;
            if !upper.is_finite()
                || strata
                    .last()
                    .is_some_and(|previous: &EligibleOzoneMassStratum<'_>| {
                        upper <= previous.upper_cumulative_kg
                    })
            {
                return Err(PopulationError::InvalidConfiguration);
            }
            strata.push(EligibleOzoneMassStratum {
                upper_cumulative_kg: upper,
                column,
                layer,
                south_degrees: latitude_bounds.0,
                north_degrees: latitude_bounds.1,
                eligible_lower_pressure_pa,
                potential_vorticity_pvu: pv,
            });
        }
    }
    if strata.is_empty() {
        Err(PopulationError::NoEligibleOzoneMass)
    } else {
        Ok(strata)
    }
}

fn locate_eligible_ozone_mass_coordinate<'a>(
    strata: &'a [EligibleOzoneMassStratum<'a>],
    coordinate_kg: f64,
) -> Result<&'a EligibleOzoneMassStratum<'a>, PopulationError> {
    if !coordinate_kg.is_finite() || coordinate_kg < 0.0 {
        return Err(PopulationError::AirMass(
            AirMassPopulationError::InvalidSample,
        ));
    }
    let index = strata.partition_point(|stratum| stratum.upper_cumulative_kg <= coordinate_kg);
    strata
        .get(index)
        .or_else(|| strata.last())
        .ok_or(PopulationError::NoEligibleOzoneMass)
}

fn ozone_eligible_lower_pressure(
    layer: &AirMassLayer,
) -> Result<Option<f64>, AirMassPopulationError> {
    let threshold = crate::science::OZONE_RULE_MINIMUM_HEIGHT_ASL_M;
    if layer.upper_height_asl_m <= threshold {
        return Ok(None);
    }
    if layer.lower_height_asl_m >= threshold {
        return Ok(Some(layer.lower_pressure_pa));
    }
    pressure_at_height(layer, threshold).map(Some)
}

fn flatten_mass_strata(
    snapshot: &AirMassSnapshot,
) -> Result<Vec<MassStratum<'_>>, AirMassPopulationError> {
    let layer_count = snapshot
        .columns
        .iter()
        .try_fold(0_usize, |total, column| {
            total.checked_add(column.layers.len())
        })
        .ok_or(AirMassPopulationError::ResourceLimit)?;
    let mut strata = Vec::with_capacity(layer_count);
    let mut sum = 0.0_f64;
    let mut correction = 0.0_f64;
    for column in &snapshot.columns {
        for layer in &column.layers {
            let value = layer.dry_air_mass_kg;
            if !value.is_finite() || value <= 0.0 {
                return Err(AirMassPopulationError::InvalidMassBudget);
            }
            let next = sum + value;
            if sum.abs() >= value.abs() {
                correction += (sum - next) + value;
            } else {
                correction += (value - next) + sum;
            }
            sum = next;
            let upper = sum + correction;
            if !upper.is_finite()
                || strata
                    .last()
                    .is_some_and(|previous: &MassStratum<'_>| upper <= previous.upper_cumulative_kg)
            {
                return Err(AirMassPopulationError::InvalidMassBudget);
            }
            strata.push(MassStratum {
                upper_cumulative_kg: upper,
                column,
                layer,
            });
        }
    }
    if strata.is_empty() {
        return Err(AirMassPopulationError::InvalidMassBudget);
    }
    Ok(strata)
}

fn locate_mass_coordinate<'a>(
    strata: &'a [MassStratum<'a>],
    coordinate_kg: f64,
) -> Result<(&'a AirMassColumn, &'a AirMassLayer), AirMassPopulationError> {
    if !coordinate_kg.is_finite() || coordinate_kg < 0.0 {
        return Err(AirMassPopulationError::InvalidSample);
    }
    let index = strata.partition_point(|stratum| stratum.upper_cumulative_kg <= coordinate_kg);
    let selected = strata
        .get(index)
        .or_else(|| strata.last())
        .ok_or(AirMassPopulationError::InvalidMassBudget)?;
    Ok((selected.column, selected.layer))
}

fn reserve_batch(
    particles: &mut ParticleBatch,
    count: usize,
) -> Result<(), AirMassPopulationError> {
    particles
        .id
        .try_reserve_exact(count)
        .and_then(|_| particles.population_id.try_reserve_exact(count))
        .and_then(|_| particles.origin.try_reserve_exact(count))
        .and_then(|_| particles.birth_time.try_reserve_exact(count))
        .and_then(|_| particles.longitude_degrees.try_reserve_exact(count))
        .and_then(|_| particles.latitude_degrees.try_reserve_exact(count))
        .and_then(|_| particles.height_asl_m.try_reserve_exact(count))
        .and_then(|_| particles.integration_offset_ns.try_reserve_exact(count))
        .and_then(|_| particles.elapsed_age_ns.try_reserve_exact(count))
        .and_then(|_| particles.dry_air_mass_kg.try_reserve_exact(count))
        .and_then(|_| particles.sensitivity_weight.try_reserve_exact(count))
        .and_then(|_| particles.status.try_reserve_exact(count))
        .map_err(|_| AirMassPopulationError::ResourceLimit)
}

#[allow(clippy::too_many_arguments)]
fn push_particle(
    particles: &mut ParticleBatch,
    id: ParticleId,
    population_id: &PopulationId,
    domain_id: &DomainId,
    birth_time: Timestamp,
    longitude_degrees: f64,
    latitude_degrees: f64,
    height_asl_m: f64,
    dry_air_mass_kg: f64,
) {
    particles.id.push(id);
    particles.population_id.push(population_id.clone());
    particles.origin.push(ParticleOrigin::DomainInitial {
        domain_id: domain_id.clone(),
    });
    particles.birth_time.push(birth_time);
    particles.longitude_degrees.push(longitude_degrees);
    particles.latitude_degrees.push(latitude_degrees);
    particles.height_asl_m.push(height_asl_m);
    particles.integration_offset_ns.push(0);
    particles.elapsed_age_ns.push(0);
    particles.dry_air_mass_kg.push(dry_air_mass_kg);
    particles.sensitivity_weight.push(None);
    particles.status.push(ParticleStatus::Alive);
}

fn validate_identity(
    specification: &DomainFillAirMassSpec,
    snapshot: &AirMassSnapshot,
) -> Result<(), AirMassPopulationError> {
    if specification.id.0.trim().is_empty()
        || specification.domain_id.0.trim().is_empty()
        || specification.domain_id != snapshot.domain
    {
        return Err(AirMassPopulationError::InvalidConfiguration);
    }
    Ok(())
}

fn normalize_longitude(longitude_degrees: f64) -> f64 {
    (longitude_degrees + 180.0).rem_euclid(360.0) - 180.0
}

fn sample_height_from_pressure_fraction(
    layer: &AirMassLayer,
    pressure_fraction: f64,
) -> Result<f64, AirMassPopulationError> {
    sample_height_from_pressure_bounds(
        layer,
        layer.upper_pressure_pa,
        layer.lower_pressure_pa,
        pressure_fraction,
    )
}

fn sample_height_from_pressure_bounds(
    layer: &AirMassLayer,
    upper_pressure_pa: f64,
    lower_pressure_pa: f64,
    pressure_fraction: f64,
) -> Result<f64, AirMassPopulationError> {
    validate_sampling_layer(layer)?;
    if !pressure_fraction.is_finite()
        || !(0.0..1.0).contains(&pressure_fraction)
        || !upper_pressure_pa.is_finite()
        || !lower_pressure_pa.is_finite()
        || upper_pressure_pa < layer.upper_pressure_pa
        || lower_pressure_pa > layer.lower_pressure_pa
        || lower_pressure_pa <= upper_pressure_pa
    {
        return Err(AirMassPopulationError::InvalidSample);
    }
    // Hydrostatic layer mass is uniform in pressure when the full-level q is
    // held fixed. Draw pressure linearly, then place the particle in geometric
    // height using the frozen log-pressure interpolation between interfaces.
    // The native ECMWF top interface is exactly 0 Pa while the queryable model
    // top is the first full level. Under the ECMWF alpha=ln(2) convention that
    // full level corresponds to half the lower-interface pressure. Pressure
    // mass above it is retained but cannot be placed above the physical/data
    // model top, so its geometric support saturates at that top height.
    let sampled_pressure_pa =
        (lower_pressure_pa - upper_pressure_pa).mul_add(pressure_fraction, upper_pressure_pa);
    height_from_pressure(layer, sampled_pressure_pa)
}

fn height_from_pressure(
    layer: &AirMassLayer,
    pressure_pa: f64,
) -> Result<f64, AirMassPopulationError> {
    validate_sampling_layer(layer)?;
    if !pressure_pa.is_finite()
        || pressure_pa < layer.upper_pressure_pa
        || pressure_pa > layer.lower_pressure_pa
    {
        return Err(AirMassPopulationError::InvalidSample);
    }
    let geometric_upper_pressure_pa = if layer.upper_pressure_pa == 0.0 {
        0.5 * layer.lower_pressure_pa
    } else {
        layer.upper_pressure_pa
    };
    let geometric_pressure_pa = pressure_pa.max(geometric_upper_pressure_pa);
    let log_span = (layer.lower_pressure_pa / geometric_upper_pressure_pa).ln();
    let log_fraction = (geometric_pressure_pa / geometric_upper_pressure_pa).ln() / log_span;
    let height_asl_m = (layer.lower_height_asl_m - layer.upper_height_asl_m)
        .mul_add(log_fraction, layer.upper_height_asl_m);
    if height_asl_m.is_finite()
        && height_asl_m >= layer.lower_height_asl_m
        && height_asl_m <= layer.upper_height_asl_m
    {
        Ok(height_asl_m)
    } else {
        Err(AirMassPopulationError::InvalidSample)
    }
}

fn pressure_at_height(
    layer: &AirMassLayer,
    height_asl_m: f64,
) -> Result<f64, AirMassPopulationError> {
    validate_sampling_layer(layer)?;
    pressure_at_height_interfaces(
        layer.upper_pressure_pa,
        layer.lower_pressure_pa,
        layer.upper_height_asl_m,
        layer.lower_height_asl_m,
        height_asl_m,
    )
}

fn pressure_at_height_interfaces(
    upper_pressure_pa: f64,
    lower_pressure_pa: f64,
    upper_height_asl_m: f64,
    lower_height_asl_m: f64,
    height_asl_m: f64,
) -> Result<f64, AirMassPopulationError> {
    if !upper_pressure_pa.is_finite()
        || !lower_pressure_pa.is_finite()
        || upper_pressure_pa < 0.0
        || lower_pressure_pa <= upper_pressure_pa
        || !upper_height_asl_m.is_finite()
        || !lower_height_asl_m.is_finite()
        || upper_height_asl_m <= lower_height_asl_m
        || !height_asl_m.is_finite()
        || height_asl_m < lower_height_asl_m
        || height_asl_m > upper_height_asl_m
    {
        return Err(AirMassPopulationError::InvalidSample);
    }
    if height_asl_m == upper_height_asl_m {
        return Ok(upper_pressure_pa);
    }
    if height_asl_m == lower_height_asl_m {
        return Ok(lower_pressure_pa);
    }
    let geometric_upper_pressure_pa = if upper_pressure_pa == 0.0 {
        0.5 * lower_pressure_pa
    } else {
        upper_pressure_pa
    };
    let height_fraction =
        (height_asl_m - upper_height_asl_m) / (lower_height_asl_m - upper_height_asl_m);
    let pressure_pa = geometric_upper_pressure_pa
        * (height_fraction * (lower_pressure_pa / geometric_upper_pressure_pa).ln()).exp();
    if pressure_pa.is_finite() && pressure_pa > upper_pressure_pa && pressure_pa < lower_pressure_pa
    {
        Ok(pressure_pa)
    } else {
        Err(AirMassPopulationError::InvalidSample)
    }
}

fn validate_sampling_layer(layer: &AirMassLayer) -> Result<(), AirMassPopulationError> {
    if !layer.upper_pressure_pa.is_finite()
        || !layer.lower_pressure_pa.is_finite()
        || layer.upper_pressure_pa < 0.0
        || layer.lower_pressure_pa <= layer.upper_pressure_pa
        || !layer.upper_height_asl_m.is_finite()
        || !layer.lower_height_asl_m.is_finite()
        || layer.upper_height_asl_m <= layer.lower_height_asl_m
    {
        Err(AirMassPopulationError::InvalidSample)
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(super) struct BoundaryInflowPlan {
    pub(super) start_time: Timestamp,
    pub(super) end_time: Timestamp,
    pub(super) direction: Direction,
    pub(super) lifecycle_event_index: u64,
    pub(super) rates: Vec<BoundaryFaceRate>,
    pub(super) births: Vec<PlannedBoundaryBirth>,
}

#[derive(Clone, Debug)]
pub(super) struct BoundaryFaceRate {
    pub(super) face_id: u64,
    pub(super) rate_kg_s: f64,
}

#[derive(Clone, Debug)]
pub(super) struct PlannedBoundaryBirth {
    pub(super) time: Timestamp,
    pub(super) face_id: u64,
    pub(super) particle: ParticleState,
}

#[derive(Clone, Copy, Debug)]
struct EligibleBoundaryFace<'a> {
    face: &'a BoundaryFaceLayer,
    tangential_lower_degrees: f64,
    tangential_upper_degrees: f64,
    eligible_lower_height_asl_m: f64,
    potential_vorticity_pvu: f64,
    rate_kg_s: f64,
}

impl BoundaryInflowPlan {
    pub(super) fn contains_interval(
        &self,
        start: Timestamp,
        step: SignedDuration,
    ) -> Result<bool, AirMassPopulationError> {
        let end = add_timestamp(start, step).map_err(|_| AirMassPopulationError::TimeOverflow)?;
        Ok(self.direction
            == if step.0 > 0 {
                Direction::Forward
            } else {
                Direction::Backward
            }
            && match self.direction {
                Direction::Forward => start >= self.start_time && end <= self.end_time,
                Direction::Backward => start <= self.start_time && end >= self.end_time,
            })
    }

    pub(super) fn remaining_birth_times(&self, current: Timestamp) -> Vec<Timestamp> {
        self.births
            .iter()
            .filter_map(|birth| {
                let ahead = match self.direction {
                    Direction::Forward => birth.time > current,
                    Direction::Backward => birth.time < current,
                };
                ahead.then_some(birth.time)
            })
            .collect()
    }

    pub(super) fn incoming_by_face(
        &self,
        step: SignedDuration,
    ) -> Result<BTreeMap<u64, f64>, AirMassPopulationError> {
        let seconds = duration_seconds(step)?;
        self.rates
            .iter()
            .map(|rate| {
                let mass = rate.rate_kg_s * seconds;
                if mass.is_finite() && mass >= 0.0 {
                    Ok((rate.face_id, mass))
                } else {
                    Err(AirMassPopulationError::InvalidMassBudget)
                }
            })
            .collect()
    }

    pub(super) fn births_at(&self, time: Timestamp) -> Vec<PlannedBoundaryBirth> {
        self.births
            .iter()
            .filter(|birth| birth.time == time)
            .cloned()
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_boundary_inflow(
    specification: &DomainFillAirMassSpec,
    snapshot: &AirMassSnapshot,
    carrier_mass_per_particle_kg: f64,
    residual_mass_kg: &BTreeMap<u64, f64>,
    direction: Direction,
    start_time: Timestamp,
    step: SignedDuration,
    lifecycle_event_index: u64,
    random_seed: u64,
) -> Result<BoundaryInflowPlan, AirMassPopulationError> {
    validate_identity(specification, snapshot)?;
    if snapshot.boundary_faces.is_empty()
        || !carrier_mass_per_particle_kg.is_finite()
        || carrier_mass_per_particle_kg <= 0.0
    {
        return Err(AirMassPopulationError::InvalidConfiguration);
    }
    let end_time =
        add_timestamp(start_time, step).map_err(|_| AirMassPopulationError::TimeOverflow)?;
    let duration_seconds = duration_seconds(step)?;
    let mut rates = Vec::with_capacity(snapshot.boundary_faces.len());
    let mut births = Vec::new();
    for face in &snapshot.boundary_faces {
        let rate_kg_s = face
            .inflow_rate_kg_s(direction)
            .map_err(|_| AirMassPopulationError::InvalidMassBudget)?;
        rates.push(BoundaryFaceRate {
            face_id: face.face_id,
            rate_kg_s,
        });
        if rate_kg_s == 0.0 {
            continue;
        }
        let opening_residual = residual_mass_kg.get(&face.face_id).copied().unwrap_or(0.0);
        if !opening_residual.is_finite()
            || opening_residual < 0.0
            || opening_residual >= carrier_mass_per_particle_kg
        {
            return Err(AirMassPopulationError::InvalidMassBudget);
        }
        let available = rate_kg_s.mul_add(duration_seconds, opening_residual);
        if !available.is_finite() || available < opening_residual {
            return Err(AirMassPopulationError::InvalidMassBudget);
        }
        let count_f64 = (available / carrier_mass_per_particle_kg).floor();
        if count_f64 > u64::MAX as f64 {
            return Err(AirMassPopulationError::ResourceLimit);
        }
        let count = count_f64 as u64;
        for ordinal in 0..count {
            let threshold = carrier_mass_per_particle_kg * (ordinal + 1) as f64;
            let required = threshold - opening_residual;
            let exact_elapsed_ns = required / rate_kg_s * 1.0e9;
            if !exact_elapsed_ns.is_finite() || exact_elapsed_ns <= 0.0 {
                return Err(AirMassPopulationError::InvalidMassBudget);
            }
            let elapsed_ns = exact_elapsed_ns.ceil();
            if elapsed_ns > i64::MAX as f64 {
                return Err(AirMassPopulationError::TimeOverflow);
            }
            let magnitude = elapsed_ns as i64;
            let signed = match direction {
                Direction::Forward => SignedDuration(magnitude),
                Direction::Backward => SignedDuration(-magnitude),
            };
            let birth_time = add_timestamp(start_time, signed)
                .map_err(|_| AirMassPopulationError::TimeOverflow)?;
            let inside = match direction {
                Direction::Forward => birth_time <= end_time,
                Direction::Backward => birth_time >= end_time,
            };
            if !inside {
                return Err(AirMassPopulationError::TimeOverflow);
            }
            births.push(PlannedBoundaryBirth {
                time: birth_time,
                face_id: face.face_id,
                particle: boundary_particle(
                    specification,
                    face,
                    carrier_mass_per_particle_kg,
                    birth_time,
                    lifecycle_event_index,
                    ordinal,
                    random_seed,
                )?,
            });
        }
    }
    births.sort_by(|left, right| {
        let time_order = match direction {
            Direction::Forward => left.time.cmp(&right.time),
            Direction::Backward => right.time.cmp(&left.time),
        };
        time_order
            .then(left.face_id.cmp(&right.face_id))
            .then(left.particle.id.cmp(&right.particle.id))
    });
    Ok(BoundaryInflowPlan {
        start_time,
        end_time,
        direction,
        lifecycle_event_index,
        rates,
        births,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_ozone_boundary_inflow(
    specification: &DomainFillStratosphericOzoneSpec,
    snapshot: &AirMassSnapshot,
    rule: &dyn OzoneAssignmentRule,
    carrier_mass_per_particle_kg: f64,
    residual_mass_kg: &BTreeMap<u64, f64>,
    direction: Direction,
    start_time: Timestamp,
    step: SignedDuration,
    lifecycle_event_index: u64,
    random_seed: u64,
) -> Result<BoundaryInflowPlan, PopulationError> {
    validate_identity(&specification.air_mass, snapshot).map_err(PopulationError::AirMass)?;
    if specification.ozone_rule != rule.model_id()
        || snapshot.boundary_faces.is_empty()
        || !carrier_mass_per_particle_kg.is_finite()
        || carrier_mass_per_particle_kg <= 0.0
    {
        return Err(PopulationError::InvalidConfiguration);
    }
    let end_time = add_timestamp(start_time, step).map_err(|_| PopulationError::TimeOverflow)?;
    let duration_seconds = duration_seconds(step).map_err(super::map_air_mass_error)?;
    let mut rates = Vec::with_capacity(snapshot.boundary_faces.len());
    let mut births = Vec::new();
    for face in &snapshot.boundary_faces {
        let eligible = eligible_boundary_face(face, direction)?;
        let rate_kg_s = eligible.as_ref().map_or(0.0, |value| value.rate_kg_s);
        rates.push(BoundaryFaceRate {
            face_id: face.face_id,
            rate_kg_s,
        });
        let Some(eligible) = eligible else {
            continue;
        };
        let opening_residual = residual_mass_kg.get(&face.face_id).copied().unwrap_or(0.0);
        if !opening_residual.is_finite()
            || opening_residual < 0.0
            || opening_residual >= carrier_mass_per_particle_kg
        {
            return Err(PopulationError::AirMass(
                AirMassPopulationError::InvalidMassBudget,
            ));
        }
        let available = rate_kg_s.mul_add(duration_seconds, opening_residual);
        if !available.is_finite() || available < opening_residual {
            return Err(PopulationError::AirMass(
                AirMassPopulationError::InvalidMassBudget,
            ));
        }
        let count_f64 = (available / carrier_mass_per_particle_kg).floor();
        if count_f64 > u64::MAX as f64 {
            return Err(PopulationError::ResourceLimit);
        }
        let count = count_f64 as u64;
        for ordinal in 0..count {
            let threshold = carrier_mass_per_particle_kg * (ordinal + 1) as f64;
            let required = threshold - opening_residual;
            let exact_elapsed_ns = required / rate_kg_s * 1.0e9;
            if !exact_elapsed_ns.is_finite() || exact_elapsed_ns <= 0.0 {
                return Err(PopulationError::AirMass(
                    AirMassPopulationError::InvalidMassBudget,
                ));
            }
            let elapsed_ns = exact_elapsed_ns.ceil();
            if elapsed_ns > i64::MAX as f64 {
                return Err(PopulationError::TimeOverflow);
            }
            let magnitude = elapsed_ns as i64;
            let signed = match direction {
                Direction::Forward => SignedDuration(magnitude),
                Direction::Backward => SignedDuration(-magnitude),
            };
            let birth_time =
                add_timestamp(start_time, signed).map_err(|_| PopulationError::TimeOverflow)?;
            let inside = match direction {
                Direction::Forward => birth_time <= end_time,
                Direction::Backward => birth_time >= end_time,
            };
            if !inside {
                return Err(PopulationError::TimeOverflow);
            }
            births.push(PlannedBoundaryBirth {
                time: birth_time,
                face_id: face.face_id,
                particle: ozone_boundary_particle(
                    specification,
                    &eligible,
                    rule,
                    carrier_mass_per_particle_kg,
                    birth_time,
                    lifecycle_event_index,
                    ordinal,
                    random_seed,
                )?,
            });
        }
    }
    births.sort_by(|left, right| {
        let time_order = match direction {
            Direction::Forward => left.time.cmp(&right.time),
            Direction::Backward => right.time.cmp(&left.time),
        };
        time_order
            .then(left.face_id.cmp(&right.face_id))
            .then(left.particle.id.cmp(&right.particle.id))
    });
    Ok(BoundaryInflowPlan {
        start_time,
        end_time,
        direction,
        lifecycle_event_index,
        rates,
        births,
    })
}

fn eligible_boundary_face(
    face: &BoundaryFaceLayer,
    direction: Direction,
) -> Result<Option<EligibleBoundaryFace<'_>>, PopulationError> {
    let threshold = crate::science::OZONE_RULE_MINIMUM_HEIGHT_ASL_M;
    if face.upper_height_asl_m <= threshold {
        return Ok(None);
    }
    let inward = face.inward_normal_wind_m_s(direction).max(0.0);
    if inward == 0.0 {
        return Ok(None);
    }
    let pv = face
        .potential_vorticity_pvu
        .ok_or(PopulationError::MissingOzoneDiagnostic)?;
    if !pv.is_finite() {
        return Err(PopulationError::MissingOzoneDiagnostic);
    }
    let (tangential_lower_degrees, tangential_upper_degrees, horizontal_fraction) = match face.side
    {
        BoundarySide::West | BoundarySide::East => {
            let bounds = if pv > crate::science::OZONE_RULE_MINIMUM_PV_PVU {
                (
                    face.tangential_lower_degrees.max(0.0),
                    face.tangential_upper_degrees,
                )
            } else if pv < -crate::science::OZONE_RULE_MINIMUM_PV_PVU {
                (
                    face.tangential_lower_degrees,
                    face.tangential_upper_degrees.min(0.0),
                )
            } else {
                return Ok(None);
            };
            if bounds.1 <= bounds.0 {
                return Ok(None);
            }
            let full_span = face.tangential_upper_degrees - face.tangential_lower_degrees;
            let fraction = (bounds.1 - bounds.0) / full_span;
            (bounds.0, bounds.1, fraction)
        }
        BoundarySide::South | BoundarySide::North => {
            let hemisphere_pv = if face.latitude_degrees < 0.0 { -pv } else { pv };
            if hemisphere_pv <= crate::science::OZONE_RULE_MINIMUM_PV_PVU {
                return Ok(None);
            }
            (
                face.tangential_lower_degrees,
                face.tangential_upper_degrees,
                1.0,
            )
        }
    };
    if !horizontal_fraction.is_finite()
        || horizontal_fraction <= 0.0
        || horizontal_fraction > 1.0 + 16.0 * f64::EPSILON
    {
        return Err(PopulationError::InvalidConfiguration);
    }
    let eligible_lower_height_asl_m = face.lower_height_asl_m.max(threshold);
    let eligible_lower_pressure_pa = if face.lower_height_asl_m >= threshold {
        face.lower_pressure_pa
    } else {
        pressure_at_height_interfaces(
            face.upper_pressure_pa,
            face.lower_pressure_pa,
            face.upper_height_asl_m,
            face.lower_height_asl_m,
            threshold,
        )
        .map_err(PopulationError::AirMass)?
    };
    let density = dry_air_density_kg_m3(
        0.5 * (face.upper_pressure_pa + eligible_lower_pressure_pa),
        face.air_temperature_k,
        face.specific_humidity,
    )
    .map_err(PopulationError::OzoneReference)?;
    let eligible_face_area_m2 = face.horizontal_edge_length_m
        * horizontal_fraction
        * (face.upper_height_asl_m - eligible_lower_height_asl_m);
    let rate_kg_s = density * inward * eligible_face_area_m2;
    if !rate_kg_s.is_finite() || rate_kg_s < 0.0 {
        return Err(PopulationError::InvalidConfiguration);
    }
    if rate_kg_s == 0.0 {
        return Ok(None);
    }
    Ok(Some(EligibleBoundaryFace {
        face,
        tangential_lower_degrees,
        tangential_upper_degrees,
        eligible_lower_height_asl_m,
        potential_vorticity_pvu: pv,
        rate_kg_s,
    }))
}

#[allow(clippy::too_many_arguments)]
fn ozone_boundary_particle(
    specification: &DomainFillStratosphericOzoneSpec,
    eligible: &EligibleBoundaryFace<'_>,
    rule: &dyn OzoneAssignmentRule,
    carrier_mass_per_particle_kg: f64,
    birth_time: Timestamp,
    lifecycle_event_index: u64,
    ordinal: u64,
    random_seed: u64,
) -> Result<ParticleState, PopulationError> {
    let face = eligible.face;
    let boundary_face_id = BoundaryFaceId(face.face_id);
    let id = ParticleId::for_domain_boundary(
        &specification.air_mass.id,
        &specification.air_mass.domain_id,
        boundary_face_id,
        lifecycle_event_index,
        ordinal,
    );
    let lifecycle = StableRandomId::from_text(&format!(
        "{DOMAIN_BOUNDARY_LIFECYCLE_EVENT_ID}/{lifecycle_event_index}"
    ));
    let key = |dimension| RandomKey {
        seed: random_seed,
        population: StableRandomId::from_text(&specification.air_mass.id.0),
        lifecycle_event: lifecycle,
        particle: id,
        sampling_dimension: dimension,
        draw_index: 0,
    };
    let tangential = CounterRng::sample_unit(key(DOMAIN_FILL_BOUNDARY_TANGENTIAL_DIMENSION));
    let vertical = CounterRng::sample_unit(key(DOMAIN_FILL_BOUNDARY_VERTICAL_DIMENSION));
    let tangential_coordinate = (eligible.tangential_upper_degrees
        - eligible.tangential_lower_degrees)
        .mul_add(tangential, eligible.tangential_lower_degrees);
    let (longitude_degrees, latitude_degrees) = match face.side {
        BoundarySide::West | BoundarySide::East => (
            normalize_longitude(face.longitude_degrees),
            tangential_coordinate,
        ),
        BoundarySide::South | BoundarySide::North => (
            normalize_longitude(tangential_coordinate),
            face.latitude_degrees,
        ),
    };
    let height_asl_m = face.upper_height_asl_m
        - vertical * (face.upper_height_asl_m - eligible.eligible_lower_height_asl_m);
    let assignment = rule.assign(OzoneAssignmentInput {
        carrier_dry_air_mass_kg: carrier_mass_per_particle_kg,
        height_asl_m,
        latitude_degrees,
        potential_vorticity_pvu: eligible.potential_vorticity_pvu,
    })?;
    if !assignment.ozone_mass_kg.is_finite() || assignment.ozone_mass_kg <= 0.0 {
        return Err(PopulationError::OzoneEligibilityMismatch);
    }
    let state = ParticleState {
        id,
        population_id: specification.air_mass.id.clone(),
        origin: ParticleOrigin::DomainBoundary {
            domain_id: specification.air_mass.domain_id.clone(),
            boundary_face_id,
        },
        birth_time,
        longitude_degrees,
        latitude_degrees,
        height_asl_m,
        integration_offset_ns: 0,
        elapsed_age_ns: 0,
        dry_air_mass_kg: carrier_mass_per_particle_kg,
        mass_kg: BTreeMap::from([(
            specification.ozone_substance.clone(),
            assignment.ozone_mass_kg,
        )]),
        sensitivity_weight: None,
        status: ParticleStatus::Alive,
    };
    state
        .validate()
        .map_err(|_| PopulationError::AirMass(AirMassPopulationError::InvalidSample))?;
    Ok(state)
}

fn boundary_particle(
    specification: &DomainFillAirMassSpec,
    face: &BoundaryFaceLayer,
    carrier_mass_per_particle_kg: f64,
    birth_time: Timestamp,
    lifecycle_event_index: u64,
    ordinal: u64,
    random_seed: u64,
) -> Result<ParticleState, AirMassPopulationError> {
    let boundary_face_id = BoundaryFaceId(face.face_id);
    let id = ParticleId::for_domain_boundary(
        &specification.id,
        &specification.domain_id,
        boundary_face_id,
        lifecycle_event_index,
        ordinal,
    );
    let lifecycle = StableRandomId::from_text(&format!(
        "{DOMAIN_BOUNDARY_LIFECYCLE_EVENT_ID}/{lifecycle_event_index}"
    ));
    let key = |dimension| RandomKey {
        seed: random_seed,
        population: StableRandomId::from_text(&specification.id.0),
        lifecycle_event: lifecycle,
        particle: id,
        sampling_dimension: dimension,
        draw_index: 0,
    };
    let tangential = CounterRng::sample_unit(key(DOMAIN_FILL_BOUNDARY_TANGENTIAL_DIMENSION));
    let vertical = CounterRng::sample_unit(key(DOMAIN_FILL_BOUNDARY_VERTICAL_DIMENSION));
    let (longitude_degrees, latitude_degrees) = match face.side {
        BoundarySide::West | BoundarySide::East => (
            normalize_longitude(face.longitude_degrees),
            (face.tangential_upper_degrees - face.tangential_lower_degrees)
                .mul_add(tangential, face.tangential_lower_degrees),
        ),
        BoundarySide::South | BoundarySide::North => (
            normalize_longitude(
                (face.tangential_upper_degrees - face.tangential_lower_degrees)
                    .mul_add(tangential, face.tangential_lower_degrees),
            ),
            face.latitude_degrees,
        ),
    };
    let height_asl_m = (face.upper_height_asl_m - face.lower_height_asl_m)
        .mul_add(vertical, face.lower_height_asl_m);
    let state = ParticleState {
        id,
        population_id: specification.id.clone(),
        origin: ParticleOrigin::DomainBoundary {
            domain_id: specification.domain_id.clone(),
            boundary_face_id,
        },
        birth_time,
        longitude_degrees,
        latitude_degrees,
        height_asl_m,
        integration_offset_ns: 0,
        elapsed_age_ns: 0,
        dry_air_mass_kg: carrier_mass_per_particle_kg,
        mass_kg: BTreeMap::new(),
        sensitivity_weight: None,
        status: ParticleStatus::Alive,
    };
    state
        .validate()
        .map_err(|_| AirMassPopulationError::InvalidSample)?;
    Ok(state)
}

pub(super) fn particle_batch_from_states(
    states: Vec<ParticleState>,
) -> Result<ParticleBatch, AirMassPopulationError> {
    let mut batch = ParticleBatch::default();
    reserve_batch(&mut batch, states.len())?;
    for state in states {
        batch.id.push(state.id);
        batch.population_id.push(state.population_id);
        batch.origin.push(state.origin);
        batch.birth_time.push(state.birth_time);
        batch.longitude_degrees.push(state.longitude_degrees);
        batch.latitude_degrees.push(state.latitude_degrees);
        batch.height_asl_m.push(state.height_asl_m);
        batch
            .integration_offset_ns
            .push(state.integration_offset_ns);
        batch.elapsed_age_ns.push(state.elapsed_age_ns);
        batch.dry_air_mass_kg.push(state.dry_air_mass_kg);
        batch.sensitivity_weight.push(state.sensitivity_weight);
        batch.status.push(state.status);
        for (substance, mass) in state.mass_kg {
            batch
                .mass
                .mass_kg
                .entry(substance)
                .or_insert_with(|| vec![0.0; batch.id.len() - 1])
                .push(mass);
        }
        for values in batch.mass.mass_kg.values_mut() {
            if values.len() < batch.id.len() {
                values.push(0.0);
            }
        }
    }
    batch
        .validate()
        .map_err(|_| AirMassPopulationError::InvalidParticleBatch)?;
    Ok(batch)
}

fn duration_seconds(step: SignedDuration) -> Result<f64, AirMassPopulationError> {
    if step.0 == 0 {
        return Err(AirMassPopulationError::TimeOverflow);
    }
    let seconds = (step.0.unsigned_abs() as f64) * 1.0e-9;
    if seconds.is_finite() && seconds > 0.0 {
        Ok(seconds)
    } else {
        Err(AirMassPopulationError::TimeOverflow)
    }
}

/// Inputs to one per-step domain-fill mass conservation adjudication.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassLedgerInput {
    /// Stable numerical-step index.
    pub step_index: u64,
    /// Physical time after the transition.
    pub time: Timestamp,
    /// Opening active carrier mass.
    pub opening_active_kg: f64,
    /// Opening initial plus boundary residual mass.
    pub opening_residual_kg: f64,
    /// Integrated positive boundary inflow mass.
    pub incoming_kg: f64,
    /// Carrier mass removed as finite-domain population outflow.
    pub outgoing_kg: f64,
    /// Carrier mass removed by other normal terminations.
    pub normal_terminated_kg: f64,
    /// Carrier mass removed by abnormal terminations.
    pub abnormal_terminated_kg: f64,
    /// Closing active carrier mass, including exact-time births.
    pub closing_active_kg: f64,
    /// Closing initial plus boundary residual mass.
    pub closing_residual_kg: f64,
}

/// Ordered per-step and cumulative mass-conservation ledger.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DomainFillMassLedger {
    records: Vec<MassLedgerRecord>,
    accumulated_imbalance_kg: Vec<f64>,
    accumulated_scale_kg: Vec<f64>,
}

impl DomainFillMassLedger {
    /// Adjudicates and appends one step in strictly increasing step order.
    pub fn record_step(
        &mut self,
        input: MassLedgerInput,
    ) -> Result<&MassLedgerRecord, AirMassPopulationError> {
        if self
            .records
            .last()
            .is_some_and(|record| input.step_index <= record.step_index)
        {
            return Err(AirMassPopulationError::InvalidLedgerOrder);
        }
        let values = [
            input.opening_active_kg,
            input.opening_residual_kg,
            input.incoming_kg,
            input.outgoing_kg,
            input.normal_terminated_kg,
            input.abnormal_terminated_kg,
            input.closing_active_kg,
            input.closing_residual_kg,
        ];
        if values
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(AirMassPopulationError::InvalidMassBudget);
        }
        let opening = neumaier_sum(&[
            input.opening_active_kg,
            input.opening_residual_kg,
            input.incoming_kg,
        ])
        .map_err(AirMassPopulationError::Reference)?;
        let closing = neumaier_sum(&[
            input.outgoing_kg,
            input.normal_terminated_kg,
            input.abnormal_terminated_kg,
            input.closing_active_kg,
            input.closing_residual_kg,
        ])
        .map_err(AirMassPopulationError::Reference)?;
        let imbalance_kg = opening - closing;
        let scale_kg = opening.max(closing);
        let tolerance_kg = mass_balance_tolerance_kg(scale_kg, false)
            .map_err(AirMassPopulationError::Reference)?;
        if imbalance_kg.abs() > tolerance_kg {
            return Err(AirMassPopulationError::MassImbalance);
        }
        self.accumulated_imbalance_kg.push(imbalance_kg);
        self.accumulated_scale_kg.push(scale_kg);
        self.records.push(MassLedgerRecord {
            step_index: input.step_index,
            time: input.time,
            opening_active_kg: input.opening_active_kg,
            opening_residual_kg: input.opening_residual_kg,
            incoming_kg: input.incoming_kg,
            outgoing_kg: input.outgoing_kg,
            normal_terminated_kg: input.normal_terminated_kg,
            abnormal_terminated_kg: input.abnormal_terminated_kg,
            closing_active_kg: input.closing_active_kg,
            closing_residual_kg: input.closing_residual_kg,
            imbalance_kg,
            tolerance_kg,
        });
        self.records
            .last()
            .ok_or(AirMassPopulationError::InvalidLedgerOrder)
    }

    /// Applies the stricter final accumulated gate.
    pub fn finalize(&self) -> Result<(), AirMassPopulationError> {
        let imbalance = neumaier_sum(&self.accumulated_imbalance_kg)
            .map_err(AirMassPopulationError::Reference)?;
        let scale =
            neumaier_sum(&self.accumulated_scale_kg).map_err(AirMassPopulationError::Reference)?;
        let tolerance =
            mass_balance_tolerance_kg(scale, true).map_err(AirMassPopulationError::Reference)?;
        if imbalance.abs() <= tolerance {
            Ok(())
        } else {
            Err(AirMassPopulationError::MassImbalance)
        }
    }

    /// Returns immutable manifest-ready records.
    #[must_use]
    pub fn records(&self) -> &[MassLedgerRecord] {
        &self.records
    }
}

/// Fixed-order active carrier mass for one population.
pub fn active_carrier_mass_kg(
    particles: &ParticleBatch,
    population_id: &PopulationId,
) -> Result<f64, AirMassPopulationError> {
    let len = particles
        .validate()
        .map_err(|_| AirMassPopulationError::InvalidParticleBatch)?;
    let values = (0..len)
        .filter(|index| {
            particles.population_id[*index] == *population_id
                && particles.status[*index] == ParticleStatus::Alive
        })
        .map(|index| particles.dry_air_mass_kg[index])
        .collect::<Vec<_>>();
    neumaier_sum(&values).map_err(AirMassPopulationError::Reference)
}

/// Fixed-order sum of residual masses stored by stable face identity.
pub fn residual_mass_kg(
    initial_residual_kg: f64,
    boundary_residual_kg: &BTreeMap<u64, f64>,
) -> Result<f64, AirMassPopulationError> {
    if !initial_residual_kg.is_finite() || initial_residual_kg < 0.0 {
        return Err(AirMassPopulationError::InvalidMassBudget);
    }
    let mut values = Vec::with_capacity(boundary_residual_kg.len() + 1);
    values.push(initial_residual_kg);
    for value in boundary_residual_kg.values() {
        if !value.is_finite() || *value < 0.0 {
            return Err(AirMassPopulationError::InvalidMassBudget);
        }
        values.push(*value);
    }
    neumaier_sum(&values).map_err(AirMassPopulationError::Reference)
}

/// Dry-air seeding or conservation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AirMassPopulationError {
    /// Case target or domain identity is inconsistent.
    InvalidConfiguration,
    /// Total, carrier, or residual mass is non-finite or non-physical.
    InvalidMassBudget,
    /// Deterministic spatial sample is non-finite or outside its stratum.
    InvalidSample,
    /// Generated particle storage is inconsistent.
    InvalidParticleBatch,
    /// Count, allocation, or stable integer conversion exceeds resources.
    ResourceLimit,
    /// Per-step or final conservation exceeds the frozen tolerance.
    MassImbalance,
    /// Step records are duplicated or out of order.
    InvalidLedgerOrder,
    /// Signed physical-time arithmetic overflowed or could not resolve a birth instant.
    TimeOverflow,
    /// Independent scalar reference formula rejected an input.
    Reference(ReferenceError),
}

impl AirMassPopulationError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "population.air_mass.invalid_configuration",
            Self::InvalidMassBudget => "population.air_mass.invalid_mass_budget",
            Self::InvalidSample => "population.air_mass.invalid_sample",
            Self::InvalidParticleBatch => "population.air_mass.invalid_particle_batch",
            Self::ResourceLimit => "population.air_mass.resource_limit",
            Self::MassImbalance => "run.mass_conservation",
            Self::InvalidLedgerOrder => "population.air_mass.invalid_ledger_order",
            Self::TimeOverflow => "population.air_mass.time_overflow",
            Self::Reference(_) => "population.air_mass.reference",
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use trajecta_case::model::population::{DomainFillStratosphericOzoneSpec, PopulationId};
    use trajecta_case::model::substance::SubstanceId;
    use trajecta_case::quantity::{Mass, Quantity, Unit};
    use trajecta_met::derive::domain_fill::{AirMassLayer, BoundaryFaceLayer, BoundarySide};
    use trajecta_met::grid::DomainGeometry;

    use super::*;

    fn timestamp(seconds: i64) -> Timestamp {
        Timestamp::new(seconds, 0).unwrap()
    }

    fn snapshot() -> AirMassSnapshot {
        let domain = DomainId("d".into());
        let layer = |level_index, mass, upper, lower, top, bottom| AirMassLayer {
            level_index,
            native_full_level: u16::try_from(level_index + 1).unwrap(),
            upper_pressure_pa: upper,
            lower_pressure_pa: lower,
            upper_height_asl_m: top,
            lower_height_asl_m: bottom,
            centre_height_asl_m: 0.5 * (top + bottom),
            potential_vorticity_pvu: None,
            air_temperature_k: 280.0,
            specific_humidity: 0.0,
            eastward_wind_m_s: 0.0,
            northward_wind_m_s: 0.0,
            dry_air_mass_kg: mass,
        };
        AirMassSnapshot {
            domain: domain.clone(),
            time: timestamp(0),
            grid: DomainGeometry {
                domain,
                longitude_origin_degrees: 0.0,
                latitude_origin_degrees: 0.0,
                longitude_spacing_degrees: 1.0,
                latitude_spacing_degrees: 1.0,
                nx: 2,
                ny: 2,
                periodic_longitude: false,
                halo_cells: 0,
            },
            terrain_height_grid_asl_m: vec![0.0; 4],
            aerodynamic_roughness_length_grid_m: vec![0.1; 4],
            columns: vec![AirMassColumn {
                cell_id: 0,
                x: 0,
                y: 0,
                longitude_degrees: 0.0,
                latitude_degrees: 0.0,
                west_degrees: 0.0,
                east_degrees: 1.0,
                south_degrees: 0.0,
                north_degrees: 1.0,
                area_m2: 1.0,
                surface_pressure_pa: 1_000.0,
                terrain_height_asl_m: 0.0,
                transport_floor_height_asl_m: 0.5,
                transport_floor_pressure_pa: 999.0,
                layers: vec![
                    layer(0, 3.0, 100.0, 400.0, 3_000.0, 2_000.0),
                    layer(1, 7.0, 400.0, 999.0, 2_000.0, 0.5),
                ],
                dry_air_mass_kg: 10.0,
            }],
            boundary_faces: Vec::new(),
            total_dry_air_mass_kg: 10.0,
        }
    }

    fn count_spec(count: u64) -> DomainFillAirMassSpec {
        DomainFillAirMassSpec {
            id: PopulationId("air".into()),
            domain_id: DomainId("d".into()),
            target_dry_air_mass_per_particle: None,
            target_particle_count: Some(count),
        }
    }

    fn ozone_snapshot(potential_vorticity_pvu: Option<f64>) -> AirMassSnapshot {
        let mut snapshot = snapshot();
        snapshot.grid.latitude_origin_degrees = -1.0;
        snapshot.grid.latitude_spacing_degrees = 2.0;
        let column = &mut snapshot.columns[0];
        column.latitude_degrees = 0.0;
        column.south_degrees = -1.0;
        column.north_degrees = 1.0;
        column.layers = vec![AirMassLayer {
            level_index: 0,
            native_full_level: 1,
            upper_pressure_pa: 100.0,
            lower_pressure_pa: 400.0,
            upper_height_asl_m: 4_000.0,
            lower_height_asl_m: 2_000.0,
            centre_height_asl_m: 3_000.0,
            potential_vorticity_pvu,
            air_temperature_k: 250.0,
            specific_humidity: 0.0,
            eastward_wind_m_s: 0.0,
            northward_wind_m_s: 0.0,
            dry_air_mass_kg: 9.0,
        }];
        column.dry_air_mass_kg = 9.0;
        snapshot.total_dry_air_mass_kg = 9.0;
        snapshot
    }

    fn ozone_spec(count: u64) -> DomainFillStratosphericOzoneSpec {
        DomainFillStratosphericOzoneSpec {
            air_mass: count_spec(count),
            ozone_rule: crate::science::FLEXPART_PV60_OZONE_ID.into(),
            ozone_substance: SubstanceId("ozone".into()),
        }
    }

    fn ozone_boundary_snapshot(potential_vorticity_pvu: Option<f64>) -> AirMassSnapshot {
        let mut snapshot = ozone_snapshot(potential_vorticity_pvu);
        snapshot.boundary_faces = vec![BoundaryFaceLayer {
            face_id: 7,
            side: BoundarySide::West,
            segment_index: 0,
            level_index: 0,
            native_full_level: 1,
            longitude_degrees: 0.0,
            latitude_degrees: 0.0,
            tangential_lower_degrees: -1.0,
            tangential_upper_degrees: 1.0,
            upper_pressure_pa: 100.0,
            lower_pressure_pa: 400.0,
            upper_height_asl_m: 4_000.0,
            lower_height_asl_m: 2_000.0,
            potential_vorticity_pvu,
            air_temperature_k: 250.0,
            specific_humidity: 0.0,
            forward_inward_normal_wind_m_s: 10.0,
            horizontal_edge_length_m: 1_000.0,
            geometric_face_area_m2: 2_000_000.0,
        }];
        snapshot
    }

    #[test]
    fn target_count_is_exact_equal_mass_and_order_independent() {
        let seeded = seed_initial_air_mass(&count_spec(5), &snapshot(), 7).unwrap();
        assert_eq!(seeded.particles.len().unwrap(), 5);
        assert_eq!(seeded.carrier_mass_per_particle_kg, 2.0);
        assert_eq!(seeded.residual_mass_kg, 0.0);
        assert!(
            seeded
                .particles
                .dry_air_mass_kg
                .iter()
                .all(|mass| *mass == 2.0)
        );
        let again = seed_initial_air_mass(&count_spec(5), &snapshot(), 7).unwrap();
        assert_eq!(seeded, again);
        let mut reversed = seeded.particles.clone();
        reversed.id.reverse();
        assert_ne!(reversed.id, seeded.particles.id);
        assert_eq!(
            active_carrier_mass_kg(&reversed, &PopulationId("air".into())).unwrap(),
            10.0
        );
    }

    #[test]
    fn fixed_mass_retains_only_subparticle_initial_residual() {
        let unit = Unit::new("kg", trajecta_case::quantity::Dimension::MASS, 1.0, 0.0).unwrap();
        let specification = DomainFillAirMassSpec {
            id: PopulationId("air".into()),
            domain_id: DomainId("d".into()),
            target_dry_air_mass_per_particle: Some(Quantity::<Mass>::new(3.0, unit).unwrap()),
            target_particle_count: None,
        };
        let seeded = seed_initial_air_mass(&specification, &snapshot(), 11).unwrap();
        assert_eq!(seeded.particles.len().unwrap(), 3);
        assert_eq!(seeded.carrier_mass_per_particle_kg, 3.0);
        assert_eq!(seeded.residual_mass_kg, 1.0);
    }

    #[test]
    fn ozone_target_count_is_exact_inside_clipped_eligible_mass() {
        let snapshot = ozone_snapshot(Some(3.0));
        let seeded = seed_initial_stratospheric_ozone(
            &ozone_spec(3),
            &snapshot,
            &super::super::FlexpartPv60OzoneRule,
            17,
        )
        .unwrap();
        assert_eq!(seeded.particles.len().unwrap(), 3);
        // Half the spherical latitude band is in the PV-compatible northern
        // hemisphere and one third of the layer pressure mass is above 3 km.
        assert!((seeded.total_eligible_dry_air_mass_kg - 1.5).abs() < 1.0e-12);
        assert!((seeded.carrier_mass_per_particle_kg - 0.5).abs() < 1.0e-12);
        assert_eq!(seeded.residual_mass_kg, 0.0);
        assert!(
            seeded
                .particles
                .latitude_degrees
                .iter()
                .all(|latitude| *latitude >= 0.0)
        );
        assert!(
            seeded
                .particles
                .height_asl_m
                .iter()
                .all(|height| *height > crate::science::OZONE_RULE_MINIMUM_HEIGHT_ASL_M)
        );
        let ozone = seeded
            .particles
            .mass
            .mass_kg
            .get(&SubstanceId("ozone".into()))
            .unwrap();
        let expected = 0.5 * 3.0 * 60.0e-9 * 48.0 / 29.0;
        assert_eq!(ozone.len(), 3);
        assert!(ozone.iter().all(|mass| (*mass - expected).abs() < 1.0e-20));
    }

    #[test]
    fn ozone_negative_pv_selects_only_the_southern_hemisphere() {
        let snapshot = ozone_snapshot(Some(-3.0));
        let seeded = seed_initial_stratospheric_ozone(
            &ozone_spec(32),
            &snapshot,
            &super::super::FlexpartPv60OzoneRule,
            19,
        )
        .unwrap();
        assert!(
            seeded
                .particles
                .latitude_degrees
                .iter()
                .all(|latitude| *latitude < 0.0)
        );
    }

    #[test]
    fn ozone_seeding_rejects_missing_or_empty_eligible_pv_support() {
        assert_eq!(
            seed_initial_stratospheric_ozone(
                &ozone_spec(3),
                &ozone_snapshot(None),
                &super::super::FlexpartPv60OzoneRule,
                23,
            ),
            Err(PopulationError::MissingOzoneDiagnostic)
        );
        assert_eq!(
            seed_initial_stratospheric_ozone(
                &ozone_spec(3),
                &ozone_snapshot(Some(2.0)),
                &super::super::FlexpartPv60OzoneRule,
                23,
            ),
            Err(PopulationError::NoEligibleOzoneMass)
        );
    }

    #[test]
    fn ozone_seeding_does_not_require_pv_below_strict_height_mask() {
        let mut snapshot = ozone_snapshot(Some(3.0));
        let mut low_layer = snapshot.columns[0].layers[0].clone();
        low_layer.level_index = 1;
        low_layer.native_full_level = 2;
        low_layer.upper_pressure_pa = 400.0;
        low_layer.lower_pressure_pa = 900.0;
        low_layer.upper_height_asl_m = 2_500.0;
        low_layer.lower_height_asl_m = 100.0;
        low_layer.centre_height_asl_m = 1_000.0;
        low_layer.potential_vorticity_pvu = None;
        low_layer.dry_air_mass_kg = 5.0;
        snapshot.columns[0].layers.push(low_layer);
        snapshot.columns[0].dry_air_mass_kg += 5.0;
        snapshot.total_dry_air_mass_kg += 5.0;

        let seeded = seed_initial_stratospheric_ozone(
            &ozone_spec(3),
            &snapshot,
            &super::super::FlexpartPv60OzoneRule,
            29,
        )
        .unwrap();
        assert_eq!(seeded.particles.len().unwrap(), 3);
        assert!((seeded.total_eligible_dry_air_mass_kg - 1.5).abs() < 1.0e-12);
    }

    #[test]
    fn ozone_boundary_inflow_accumulates_only_eligible_face_mass() {
        let specification = ozone_spec(3);
        let snapshot = ozone_boundary_snapshot(Some(3.0));
        let plan = plan_ozone_boundary_inflow(
            &specification,
            &snapshot,
            &super::super::FlexpartPv60OzoneRule,
            1_000.0,
            &BTreeMap::new(),
            Direction::Forward,
            timestamp(0),
            SignedDuration(1_000_000_000),
            0,
            31,
        )
        .unwrap();
        assert_eq!(plan.rates.len(), 1);
        assert!(plan.rates[0].rate_kg_s > 0.0);
        assert!(!plan.births.is_empty());
        for birth in plan.births {
            assert!(birth.particle.latitude_degrees >= 0.0);
            assert!(birth.particle.height_asl_m > 3_000.0);
            assert!(
                birth
                    .particle
                    .mass_kg
                    .get(&SubstanceId("ozone".into()))
                    .copied()
                    .unwrap()
                    > 0.0
            );
        }

        let no_mask = ozone_boundary_snapshot(Some(2.0));
        let empty = plan_ozone_boundary_inflow(
            &specification,
            &no_mask,
            &super::super::FlexpartPv60OzoneRule,
            1_000.0,
            &BTreeMap::new(),
            Direction::Forward,
            timestamp(0),
            SignedDuration(1_000_000_000),
            1,
            31,
        )
        .unwrap();
        assert_eq!(empty.rates[0].rate_kg_s, 0.0);
        assert!(empty.births.is_empty());
    }

    #[test]
    fn ozone_boundary_does_not_require_pv_for_geometrically_ineligible_faces() {
        let specification = ozone_spec(3);
        let mut below_height = ozone_boundary_snapshot(None);
        below_height.boundary_faces[0].upper_height_asl_m = 2_500.0;
        below_height.boundary_faces[0].lower_height_asl_m = 100.0;
        let below_plan = plan_ozone_boundary_inflow(
            &specification,
            &below_height,
            &super::super::FlexpartPv60OzoneRule,
            1_000.0,
            &BTreeMap::new(),
            Direction::Forward,
            timestamp(0),
            SignedDuration(1_000_000_000),
            0,
            31,
        )
        .unwrap();
        assert_eq!(below_plan.rates[0].rate_kg_s, 0.0);
        assert!(below_plan.births.is_empty());

        let mut no_inflow = ozone_boundary_snapshot(None);
        no_inflow.boundary_faces[0].forward_inward_normal_wind_m_s = -1.0;
        let no_inflow_plan = plan_ozone_boundary_inflow(
            &specification,
            &no_inflow,
            &super::super::FlexpartPv60OzoneRule,
            1_000.0,
            &BTreeMap::new(),
            Direction::Forward,
            timestamp(0),
            SignedDuration(1_000_000_000),
            1,
            31,
        )
        .unwrap();
        assert_eq!(no_inflow_plan.rates[0].rate_kg_s, 0.0);
        assert!(no_inflow_plan.births.is_empty());
    }

    #[test]
    fn step_and_final_ledgers_enforce_frozen_gate() {
        let mut ledger = DomainFillMassLedger::default();
        ledger
            .record_step(MassLedgerInput {
                step_index: 0,
                time: timestamp(1),
                opening_active_kg: 9.0,
                opening_residual_kg: 1.0,
                incoming_kg: 2.0,
                outgoing_kg: 3.0,
                normal_terminated_kg: 0.0,
                abnormal_terminated_kg: 0.0,
                closing_active_kg: 8.0,
                closing_residual_kg: 1.0,
            })
            .unwrap();
        assert_eq!(ledger.records().len(), 1);
        ledger.finalize().unwrap();

        let mut tampered = DomainFillMassLedger::default();
        assert_eq!(
            tampered.record_step(MassLedgerInput {
                step_index: 0,
                time: timestamp(1),
                opening_active_kg: 10.0,
                opening_residual_kg: 0.0,
                incoming_kg: 0.0,
                outgoing_kg: 0.0,
                normal_terminated_kg: 0.0,
                abnormal_terminated_kg: 0.0,
                closing_active_kg: 9.0,
                closing_residual_kg: 0.0,
            }),
            Err(AirMassPopulationError::MassImbalance)
        );
    }

    #[test]
    fn backward_and_forward_seed_the_same_initial_mass_distribution() {
        let forward = seed_initial_air_mass(&count_spec(8), &snapshot(), 42).unwrap();
        let backward = seed_initial_air_mass(&count_spec(8), &snapshot(), 42).unwrap();
        assert_eq!(forward, backward);
    }

    #[test]
    fn within_layer_sampling_is_uniform_in_pressure_then_log_mapped_to_height() {
        let layer = &snapshot().columns[0].layers[0];
        let height = sample_height_from_pressure_fraction(layer, 0.5).unwrap();
        let geometric_fraction =
            (layer.upper_height_asl_m - height) / layer.geometric_thickness_m();
        let reconstructed_pressure = layer.upper_pressure_pa
            * (layer.lower_pressure_pa / layer.upper_pressure_pa).powf(geometric_fraction);
        let expected_pressure = 0.5 * (layer.upper_pressure_pa + layer.lower_pressure_pa);
        assert!((reconstructed_pressure - expected_pressure).abs() < 1.0e-12);
        assert_ne!(
            height,
            0.5 * (layer.upper_height_asl_m + layer.lower_height_asl_m)
        );
    }

    #[test]
    fn native_zero_pressure_top_retains_mass_without_sampling_above_model_top() {
        let mut layer = snapshot().columns[0].layers[0].clone();
        layer.upper_pressure_pa = 0.0;
        layer.lower_pressure_pa = 2.0;
        layer.upper_height_asl_m = 40_000.0;
        layer.lower_height_asl_m = 39_000.0;

        assert_eq!(
            sample_height_from_pressure_fraction(&layer, 0.25),
            Ok(layer.upper_height_asl_m)
        );
        assert_eq!(
            sample_height_from_pressure_fraction(&layer, 0.5),
            Ok(layer.upper_height_asl_m)
        );
        let below_top = sample_height_from_pressure_fraction(&layer, 0.75).unwrap();
        assert!(below_top < layer.upper_height_asl_m);
        assert!(below_top > layer.lower_height_asl_m);
    }

    #[test]
    fn horizontal_relocation_preserves_transport_floor_offset() {
        let flat_snapshot = snapshot();
        let flat = seed_initial_air_mass(&count_spec(64), &flat_snapshot, 4_202).unwrap();
        let mut sloped_snapshot = flat_snapshot.clone();
        sloped_snapshot.terrain_height_grid_asl_m = vec![0.0, 100.0, 200.0, 300.0];
        sloped_snapshot.aerodynamic_roughness_length_grid_m = vec![0.1, 0.4, 0.8, 1.2];
        let sloped = seed_initial_air_mass(&count_spec(64), &sloped_snapshot, 4_202).unwrap();

        assert_eq!(flat.particles.id, sloped.particles.id);
        assert_eq!(
            flat.particles.longitude_degrees,
            sloped.particles.longitude_degrees
        );
        assert_eq!(
            flat.particles.latitude_degrees,
            sloped.particles.latitude_degrees
        );
        let mut saw_roughness_floor_above_half_metre = false;
        for index in 0..sloped.particles.id.len() {
            let longitude = sloped.particles.longitude_degrees[index];
            let latitude = sloped.particles.latitude_degrees[index];
            let terrain = sloped_snapshot
                .terrain_height_asl_m_at(longitude, latitude)
                .unwrap();
            let flat_floor = flat_snapshot
                .transport_floor_height_asl_m_at(longitude, latitude)
                .unwrap();
            let sloped_floor = sloped_snapshot
                .transport_floor_height_asl_m_at(longitude, latitude)
                .unwrap();
            saw_roughness_floor_above_half_metre |= sloped_floor - terrain > 0.5;
            let flat_offset = flat.particles.height_asl_m[index] - flat_floor;
            let sloped_offset = sloped.particles.height_asl_m[index] - sloped_floor;
            assert!(sloped_offset > 0.0);
            assert!(
                (sloped_offset - flat_offset).abs()
                    <= 4.0 * f64::EPSILON * flat_offset.abs().max(1.0)
            );
        }
        assert!(saw_roughness_floor_above_half_metre);
    }
}
