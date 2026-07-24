//! M4-A3 production-builder hard gates for PV60 stratospheric ozone filling.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use tempfile::tempdir;
use trajecta_case::document::{
    ExecutionSpec, MeteorologyReaderBackend, ResolvedCase, ResolvedRunProfile,
};
use trajecta_case::model::metadata::Metadata;
use trajecta_case::model::meteorology::DomainId;
use trajecta_case::model::numerics::{BoundarySpec, IntegratorSpec, NumericsSpec};
use trajecta_case::model::output::default_particle_state_output;
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, DomainFillStratosphericOzoneSpec, ParticlePopulationSpec, PopulationId,
};
use trajecta_case::model::substance::SubstanceId;
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Quantity, Time as TimeDimension, Unit};
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::runner::{RunError, RunOutcome, build_runner};
use trajecta_core::science::{
    FLEXPART_PV60_OZONE_ID, LIMITED_DOMAIN_TERMINATE_ID, MODEL_TOP_TERMINATE_ID,
    OZONE_DOMAIN_FILL_ID, RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};
use trajecta_core::synthetic::{SyntheticWind, constant_wind_stack};

const DOMAIN_ID: &str = "finite";
const OZONE_SUBSTANCE_ID: &str = "ozone";

fn seconds(value: f64) -> Quantity<TimeDimension> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn ozone_case(rule: &str, target_particle_count: u64) -> ResolvedCase {
    ResolvedCase {
        metadata: Metadata {
            name: "m4-a3-ozone".into(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start: Timestamp::new(0, 0).unwrap(),
            end: Timestamp::new(10, 0).unwrap(),
            direction: Direction::Forward,
        }),
        meteorology: None,
        particle_population: Some(ParticlePopulationSpec::DomainFillStratosphericOzone(
            DomainFillStratosphericOzoneSpec {
                air_mass: DomainFillAirMassSpec {
                    id: PopulationId("ozone".into()),
                    domain_id: DomainId(DOMAIN_ID.into()),
                    target_dry_air_mass_per_particle: None,
                    target_particle_count: Some(target_particle_count),
                },
                ozone_rule: rule.into(),
                ozone_substance: SubstanceId(OZONE_SUBSTANCE_ID.into()),
            },
        )),
        substances: Vec::new(),
        numerics: Some(NumericsSpec {
            time_step: seconds(10.0),
            integrator: IntegratorSpec {
                model: ModelId(RK2_SPHERICAL_ID.into()),
                parameters: BTreeMap::new(),
            },
            boundaries: BoundarySpec {
                policies: vec![
                    ModelId(SURFACE_REFLECT_ID.into()),
                    ModelId(MODEL_TOP_TERMINATE_ID.into()),
                    ModelId(LIMITED_DOMAIN_TERMINATE_ID.into()),
                ],
            },
            random_seed: Some(83),
        }),
        physics: Vec::new(),
        outputs: vec![default_particle_state_output()],
        sources: Vec::new(),
    }
}

fn profile(output_root: PathBuf) -> ResolvedRunProfile {
    ResolvedRunProfile {
        metadata: Metadata {
            name: "m4-a3-ozone-profile".into(),
            ..Metadata::default()
        },
        case_path: PathBuf::from("case.yaml"),
        output_root,
        datasets: Vec::new(),
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: 1,
            memory_budget_bytes: 64 * 1024 * 1024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
        },
        sources: Vec::new(),
    }
}

fn synthetic_stack() -> trajecta_core::synthetic::SyntheticStack {
    let times = [
        Timestamp::new(-3_600, 0).unwrap(),
        Timestamp::new(0, 0).unwrap(),
        Timestamp::new(10, 0).unwrap(),
        Timestamp::new(3_600, 0).unwrap(),
    ];
    constant_wind_stack(
        DOMAIN_ID,
        &times,
        SyntheticWind {
            eastward_m_s: 0.0,
            northward_m_s: 0.0,
            vertical_m_s: 0.0,
        },
        0.0,
        20_000.0,
        false,
    )
    .unwrap()
}

fn find_named_file(root: &Path, name: &str) -> Option<PathBuf> {
    if root.is_file() && root.file_name().and_then(|value| value.to_str()) == Some(name) {
        return Some(root.to_path_buf());
    }
    if root.is_dir() {
        for entry in std::fs::read_dir(root).ok()? {
            if let Some(found) = find_named_file(&entry.ok()?.path(), name) {
                return Some(found);
            }
        }
    }
    None
}

#[test]
fn production_builder_ozone_is_complete_and_persists_rule_and_mass() {
    const PARTICLE_COUNT: u64 = 16;

    let output = tempdir().unwrap();
    let mut runner = build_runner(
        ozone_case(FLEXPART_PV60_OZONE_ID, PARTICLE_COUNT),
        profile(output.path().into()),
        Some(synthetic_stack()),
    )
    .expect("ozone production builder");

    assert_eq!(runner.run(), Ok(RunOutcome::Complete));
    assert_eq!(
        runner.state().particles.len().unwrap(),
        PARTICLE_COUNT as usize
    );
    assert_eq!(runner.manifest().terminations.abnormal_count, 0);
    assert_eq!(runner.manifest().numerical.population, OZONE_DOMAIN_FILL_ID);
    assert_eq!(
        runner.manifest().numerical.ozone_rule.as_deref(),
        Some(FLEXPART_PV60_OZONE_ID)
    );

    let sqlite = find_named_file(output.path(), "particles.sqlite").expect("particles.sqlite");
    let inspection = ParticleStateSqliteSink::inspect(&sqlite).unwrap();
    assert_eq!(inspection.integrity, "ok");

    let connection = Connection::open_with_flags(&sqlite, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("read-only particle database");
    let (count, distinct_particles, total_mass, minimum_mass): (i64, i64, f64, f64) = connection
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT particle_id), SUM(mass_kg), MIN(mass_kg)\
             FROM particle_mass WHERE substance_id = ?1",
            [OZONE_SUBSTANCE_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(count, PARTICLE_COUNT as i64);
    assert_eq!(distinct_particles, PARTICLE_COUNT as i64);
    assert!(total_mass.is_finite() && total_mass > 0.0);
    assert!(minimum_mass.is_finite() && minimum_mass > 0.0);
}

#[test]
fn production_builder_rejects_unknown_ozone_rule() {
    let output = tempdir().unwrap();
    let result = build_runner(
        ozone_case("unknown_ozone_rule/v1", 4),
        profile(output.path().into()),
        Some(synthetic_stack()),
    );
    match result {
        Err(RunError::InvalidConfiguration(message)) => {
            assert!(message.contains("unknown ozone assignment rule"));
        }
        Err(other) => panic!("unexpected error: {other:?}"),
        Ok(_) => panic!("unknown ozone rule must hard fail"),
    }
}
