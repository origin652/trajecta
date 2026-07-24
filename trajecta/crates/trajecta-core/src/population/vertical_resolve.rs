//! # Contract: official AGL/pressure to ASL release resolution
//!
//! Converts sampled release vertical coordinates with exact-birth-time
//! meteorology queries. Failures are event-wide hard errors.

use trajecta_case::model::population::ReleaseVerticalSpec;
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::{CanonicalField, FieldKey};
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::query::engine::BatchWorkspace;
use trajecta_met::query::output::SampleStatus;
use trajecta_met::query::request::{
    ExplainMode, QueryBatch, QueryPlanRequest, QueryPointArrays, TransportPlanRequest,
    VerticalQuery,
};

use crate::population::{PopulationContext, PopulationError, ReleaseVerticalResolver};
use crate::release::ReleaseEvent;

/// Resolves release vertical coordinates with official meteorology queries.
#[derive(Clone, Copy, Debug, Default)]
pub struct MetReleaseVerticalResolver;

impl ReleaseVerticalResolver for MetReleaseVerticalResolver {
    fn resolve_asl(
        &self,
        event: &ReleaseEvent,
        birth_times: &[Timestamp],
        horizontal: &[(f64, f64)],
        sampled_vertical: &[f64],
        context: &mut PopulationContext<'_>,
    ) -> Result<Vec<f64>, PopulationError> {
        if birth_times.len() != horizontal.len() || horizontal.len() != sampled_vertical.len() {
            return Err(PopulationError::InvalidConfiguration);
        }
        if sampled_vertical.iter().any(|value| !value.is_finite())
            || horizontal.iter().any(|(lon, lat)| {
                !lon.is_finite() || !lat.is_finite() || !(-90.0..=90.0).contains(lat)
            })
        {
            return Err(PopulationError::InvalidConfiguration);
        }

        match &event.vertical {
            ReleaseVerticalSpec::AboveSeaLevel { .. } => {
                for index in 0..sampled_vertical.len() {
                    let (lon, lat) = horizontal[index];
                    let status = transport_status(
                        context,
                        birth_times[index],
                        lon,
                        lat,
                        sampled_vertical[index],
                        VerticalQuery::AboveSeaLevel,
                    )?;
                    ensure_ok(status)?;
                }
                Ok(sampled_vertical.to_vec())
            }
            ReleaseVerticalSpec::AboveGround { .. } => {
                let mut out = Vec::with_capacity(sampled_vertical.len());
                for index in 0..sampled_vertical.len() {
                    if sampled_vertical[index] < 0.0 {
                        return Err(PopulationError::InvalidConfiguration);
                    }
                    let (lon, lat) = horizontal[index];
                    let (status, terrain) = transport_terrain(
                        context,
                        birth_times[index],
                        lon,
                        lat,
                        sampled_vertical[index],
                        VerticalQuery::AboveGround,
                    )?;
                    ensure_ok(status)?;
                    let terrain = terrain.ok_or(PopulationError::InvalidConfiguration)?;
                    let asl = terrain + sampled_vertical[index];
                    if !asl.is_finite() {
                        return Err(PopulationError::InvalidConfiguration);
                    }
                    let status = transport_status(
                        context,
                        birth_times[index],
                        lon,
                        lat,
                        asl,
                        VerticalQuery::AboveSeaLevel,
                    )?;
                    ensure_ok(status)?;
                    out.push(asl);
                }
                Ok(out)
            }
            ReleaseVerticalSpec::Pressure { .. } => {
                let mut out = Vec::with_capacity(sampled_vertical.len());
                for index in 0..sampled_vertical.len() {
                    if sampled_vertical[index] <= 0.0 {
                        return Err(PopulationError::InvalidConfiguration);
                    }
                    let (lon, lat) = horizontal[index];
                    let (status, height) = geometric_height_at_pressure(
                        context,
                        birth_times[index],
                        lon,
                        lat,
                        sampled_vertical[index],
                    )?;
                    ensure_ok(status)?;
                    let height = height.ok_or(PopulationError::InvalidConfiguration)?;
                    if !height.is_finite() {
                        return Err(PopulationError::InvalidConfiguration);
                    }
                    out.push(height);
                }
                Ok(out)
            }
        }
    }
}

fn transport_status(
    context: &mut PopulationContext<'_>,
    time: Timestamp,
    lon: f64,
    lat: f64,
    vertical: f64,
    coordinate: VerticalQuery,
) -> Result<SampleStatus, PopulationError> {
    let (status, _) = transport_terrain(context, time, lon, lat, vertical, coordinate)?;
    Ok(status)
}

fn transport_terrain(
    context: &mut PopulationContext<'_>,
    time: Timestamp,
    lon: f64,
    lat: f64,
    vertical: f64,
    coordinate: VerticalQuery,
) -> Result<(SampleStatus, Option<f64>), PopulationError> {
    let domain = context
        .domain
        .as_ref()
        .ok_or(PopulationError::MissingMeteorology)?;
    let window = context
        .meteorology
        .prepare_for_domain(time, domain)
        .map_err(|_| PopulationError::MissingMeteorology)?;
    let plan = context
        .meteorology
        .compile_transport_plan(TransportPlanRequest::default(), &ExecutionPlan::default())
        .map_err(|_| PopulationError::MissingMeteorology)?;
    let mut workspace = BatchWorkspace::default();
    let output = window
        .prepare_transport_batch(
            &plan,
            QueryBatch {
                vertical_coordinate: coordinate,
                points: QueryPointArrays {
                    longitude_degrees: vec![lon],
                    latitude_degrees: vec![lat],
                    vertical: vec![vertical],
                },
            },
            &mut workspace,
        )
        .map_err(|_| PopulationError::MissingMeteorology)?
        .execute(context.execution, &mut workspace)
        .map_err(|_| PopulationError::MissingMeteorology)?;
    let status = *output
        .status()
        .values()
        .first()
        .ok_or(PopulationError::InvalidConfiguration)?;
    let terrain = output.row(0).and_then(|row| row.terrain_height_asl_m());
    Ok((status, terrain))
}

fn geometric_height_at_pressure(
    context: &mut PopulationContext<'_>,
    time: Timestamp,
    lon: f64,
    lat: f64,
    pressure_pa: f64,
) -> Result<(SampleStatus, Option<f64>), PopulationError> {
    let domain = context
        .domain
        .as_ref()
        .ok_or(PopulationError::MissingMeteorology)?;
    let window = context
        .meteorology
        .prepare_for_domain(time, domain)
        .map_err(|_| PopulationError::MissingMeteorology)?;
    let plan = context
        .meteorology
        .compile_plan(
            QueryPlanRequest {
                fields: vec![FieldKey::Canonical(CanonicalField::GeometricHeight)],
                allow_estimated: false,
                surface_layer_model: None,
                explain: ExplainMode::Disabled,
            },
            &ExecutionPlan::default(),
        )
        .map_err(|_| PopulationError::MissingMeteorology)?;
    let mut workspace = BatchWorkspace::default();
    let output = window
        .prepare_batch(
            &plan,
            QueryBatch {
                vertical_coordinate: VerticalQuery::Pressure,
                points: QueryPointArrays {
                    longitude_degrees: vec![lon],
                    latitude_degrees: vec![lat],
                    vertical: vec![pressure_pa],
                },
            },
            &mut workspace,
        )
        .map_err(|_| PopulationError::MissingMeteorology)?
        .execute(context.execution, &mut workspace)
        .map_err(|_| PopulationError::MissingMeteorology)?;
    let status = *output
        .status()
        .values()
        .first()
        .ok_or(PopulationError::InvalidConfiguration)?;
    let height = output
        .fields()
        .first()
        .and_then(|field| field.samples().value(0));
    Ok((status, height))
}

fn ensure_ok(status: SampleStatus) -> Result<(), PopulationError> {
    match status {
        SampleStatus::Ok => Ok(()),
        _ => Err(PopulationError::InvalidConfiguration),
    }
}
