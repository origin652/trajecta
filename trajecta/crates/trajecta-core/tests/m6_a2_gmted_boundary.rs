//! Real-terrain continuous-boundary gate for M6-A2.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use trajecta_case::model::population::{PopulationId, ReleaseEventId};
use trajecta_case::model::time::Timestamp;
use trajecta_core::boundary::MetBoundaryPathSamplerFactory;
use trajecta_core::particle::{ParticleId, ParticleOrigin, ParticleState, ParticleStatus};
use trajecta_core::runner::{BoundaryPathSamplerFactory, BoundarySamplerRequest};
use trajecta_core::synthetic::{SyntheticWind, constant_wind_stack};
use trajecta_met::auxiliary::gmted2010::{
    GMTED2010_MEAN_GRID_FILE, GMTED2010_STANDARD_DEVIATION_GRID_FILE, Gmted2010,
};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn prepared_directory() -> PathBuf {
    std::env::var_os("TRAJECTA_REAL_GMTED2010_DIR").map_or_else(
        || workspace_root().join("target/m6-a2-gmted/prepared"),
        PathBuf::from,
    )
}

fn required() -> bool {
    std::env::var("TRAJECTA_REQUIRE_M6_A2_GMTED")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn particle(longitude_degrees: f64) -> ParticleState {
    ParticleState {
        id: ParticleId(1),
        population_id: PopulationId("ridge".into()),
        origin: ParticleOrigin::Release {
            event_id: ReleaseEventId("west-of-everest".into()),
        },
        birth_time: Timestamp::UNIX_EPOCH,
        longitude_degrees,
        latitude_degrees: 27.99,
        height_asl_m: 4_500.0,
        integration_offset_ns: 0,
        elapsed_age_ns: 0,
        dry_air_mass_kg: 1.0,
        mass_kg: Default::default(),
        adjoint_weight: Default::default(),
        status: ParticleStatus::Alive,
        termination: None,
    }
}

fn clearance(sample: &trajecta_core::boundary::BoundarySample) -> f64 {
    sample.position.height_asl_m - sample.surface_height_asl_m.unwrap()
}

#[test]
fn corrected_everest_ridge_path_locates_the_first_downcrossing() {
    let directory = prepared_directory();
    let mean = directory.join(GMTED2010_MEAN_GRID_FILE);
    let deviation = directory.join(GMTED2010_STANDARD_DEVIATION_GRID_FILE);
    if !mean.is_file() || !deviation.is_file() {
        assert!(
            !required(),
            "required GMTED2010 runtime files are missing under {}",
            directory.display()
        );
        eprintln!(
            "skip: GMTED2010 runtime files missing under {}",
            directory.display()
        );
        return;
    }

    let terrain = Arc::new(Gmted2010::open(&mean, &deviation).unwrap());
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::UNIX_EPOCH,
        Timestamp::new(75, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    let stack = constant_wind_stack(
        "global",
        &times,
        SyntheticWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        0.0,
        20_000.0,
        true,
    )
    .unwrap();
    let mut engine = stack.engine;
    let execution = stack.execution;
    let query_plan = stack.transport_plan;
    let domain = stack.domain;
    let start = particle(86.84);
    let mut proposed = particle(87.10);
    proposed.integration_offset_ns = 75_000_000_000;
    proposed.elapsed_age_ns = 75_000_000_000;

    terrain.mean_interpolation_geometry().unwrap();
    let meteorology_grid = engine
        .prepare_for_domain(Timestamp::UNIX_EPOCH, &domain)
        .unwrap()
        .frames
        .before
        .metadata()
        .grid
        .clone();
    let start_terrain = terrain
        .sample_for_meteorology_cell(
            &meteorology_grid,
            start.longitude_degrees,
            start.latitude_degrees,
        )
        .unwrap();
    assert!(start_terrain.terrain_anomaly_m() < start.height_asl_m);

    let mut factory = MetBoundaryPathSamplerFactory::new(Some(terrain));
    factory.begin_batch();
    let mut path = factory
        .build_residual(
            BoundarySamplerRequest {
                start_time: Timestamp::UNIX_EPOCH,
                end_time: Timestamp::new(75, 0).unwrap(),
                start: &start,
                proposed: &proposed,
                domain: Some(&domain),
            },
            &mut engine,
            &query_plan,
            execution.as_ref(),
        )
        .unwrap();
    let segments = path.ordered_segments().unwrap();
    assert!(segments.len() > 10, "dual-grid cuts={segments:?}");
    assert!(clearance(&path.sample(0.0).unwrap()) > 100.0);
    assert!(clearance(&path.sample(0.35).unwrap()) < -1_000.0);
    assert!(clearance(&path.sample(1.0).unwrap()) > 100.0);

    let mut bracket = None;
    for segment in segments {
        let left = clearance(&path.sample(segment.start_fraction).unwrap());
        let right = clearance(&path.sample(segment.end_fraction).unwrap());
        if left >= 0.0 && right <= 0.0 {
            bracket = Some((segment.start_fraction, segment.end_fraction));
            break;
        }
    }
    let Some((mut left, mut right)) = bracket else {
        panic!("first ridge downcrossing is missing");
    };
    for _ in 0..64 {
        let midpoint = 0.5 * (left + right);
        if clearance(&path.sample(midpoint).unwrap()) > 0.0 {
            left = midpoint;
        } else {
            right = midpoint;
        }
    }
    let root = path.sample(0.5 * (left + right)).unwrap();
    assert!(
        (86.85..86.87).contains(&root.position.longitude_degrees),
        "first contact longitude={}",
        root.position.longitude_degrees
    );
    assert!(clearance(&root).abs() <= 1.0e-8);
    drop(path);
    factory.end_batch();
}
