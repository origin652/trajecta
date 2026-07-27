//! Explicit M4-A4 real-data performance matrix for dry-air domain filling.
//!
//! This gate is ignored by default because the same entry point scales from a
//! 1,000-particle preflight to the frozen 50,000/100,000-particle A4 matrix. Run
//! it explicitly with `--ignored`; missing frozen fixtures are hard failures.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use tempfile::tempdir;
use trajecta_case::document::{
    DataRootId, DatasetBinding, ExecutionSpec, MeteorologyReaderBackend, ResolvedCase,
    ResolvedRunProfile,
};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::metadata::Metadata;
use trajecta_case::model::meteorology::{DatasetRef, DomainId, DomainSpec, MeteorologySpec};
use trajecta_case::model::numerics::{BoundarySpec, IntegratorSpec, NumericsSpec};
use trajecta_case::model::output::OutputSchedule;
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::population::{
    DomainFillAirMassSpec, ParticlePopulationSpec, PopulationId,
};
use trajecta_case::model::time::{Direction, TimeSpec, Timestamp};
use trajecta_case::quantity::{Dimension, Quantity, Time as TimeDimension, Unit};
use trajecta_core::manifest::ProvenanceBundleIdentity;
use trajecta_core::output::provenance_bundle::{
    canonical_output_digest, file_sha256, validate_bundle_file_semantics_loose,
};
use trajecta_core::output::sqlite::ParticleStateSqliteSink;
use trajecta_core::runner::{RunOutcome, build_runner};
use trajecta_core::science::{
    DRY_AIR_DOMAIN_FILL_ID, GLOBAL_PERIODIC_ID, LIMITED_DOMAIN_TERMINATE_ID,
    MODEL_TOP_TERMINATE_ID, RK2_SPHERICAL_ID, SURFACE_REFLECT_ID,
};
use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::io::metrics::{
    IoCallCounters, IoCallSnapshot, clear_io_counters, install_io_counters,
};
use trajecta_met::performance::{
    PerformanceCounters, PerformanceObservationSnapshot, PerformanceSnapshot,
    clear_performance_counters, install_performance_counters,
};
use trajecta_met::profile::document::ProfileCatalog;
use trajecta_met::query::metrics::{
    QueryCallCounters, QueryCallSnapshot, QueryOrigin, clear_query_counters, install_query_counters,
};

const PRESSURE_00: i64 = 1_543_622_400;
const CFSR_00: i64 = 1_230_768_000;

#[derive(Clone, Copy)]
struct Family {
    id: &'static str,
    environment_variable: &'static str,
    relative_fixture: &'static str,
    profile: &'static str,
    source: &'static str,
    coverage_start: i64,
    coverage_end: i64,
    run_start: i64,
    periodic_longitude: bool,
}

const FAMILIES: [Family; 3] = [
    Family {
        id: "era5-pressure",
        environment_variable: "TRAJECTA_REAL_ERA5_PRESSURE_OFFICIAL_DIR",
        relative_fixture: "../../target/test-data/era5-cds-pressure-official/ready",
        profile: "era5-cf-pressure-netcdf-v0",
        source: "Copernicus CDS ERA5 pressure levels",
        coverage_start: PRESSURE_00,
        coverage_end: PRESSURE_00 + 12 * 3_600,
        run_start: PRESSURE_00 + 3 * 3_600,
        periodic_longitude: false,
    },
    Family {
        id: "era5-hybrid",
        environment_variable: "TRAJECTA_REAL_ERA5_HYBRID137_DIR",
        relative_fixture: "../../target/test-data/era5-cds-hybrid137-official/ready",
        profile: "era5-cds-hybrid137-v0",
        source: "Copernicus CDS ERA5 hybrid 137 levels",
        coverage_start: PRESSURE_00,
        coverage_end: PRESSURE_00 + 6 * 3_600,
        run_start: PRESSURE_00 + 3 * 3_600,
        periodic_longitude: false,
    },
    Family {
        id: "cfsr-pressure",
        environment_variable: "TRAJECTA_REAL_CFSR_DIR",
        relative_fixture: "../../target/test-data/cfsr-ncei-pgbl-official",
        profile: "cfsr-pgbl-pressure-v0",
        source: "NOAA NCEI CFSR pgbl",
        coverage_start: CFSR_00,
        coverage_end: CFSR_00 + 12 * 3_600,
        run_start: CFSR_00 + 3 * 3_600,
        periodic_longitude: true,
    },
];

#[derive(Serialize)]
struct MatrixEvidence {
    schema_version: &'static str,
    mode: &'static str,
    target_particle_count: u64,
    worker_threads: usize,
    duration_seconds: i64,
    time_step_seconds: i64,
    output_interval_seconds: i64,
    runs: Vec<RunEvidence>,
}

#[derive(Serialize)]
struct IoEvidence {
    after_preload: IoCallSnapshotEvidence,
    after_run: IoCallSnapshotEvidence,
    execute_delta: IoCallSnapshotEvidence,
}

#[derive(Serialize)]
struct IoCallSnapshotEvidence {
    inspect: u64,
    build_index: u64,
    decode: u64,
    provider_frame_load: u64,
    format_detection_open_attempt: u64,
}

impl From<IoCallSnapshot> for IoCallSnapshotEvidence {
    fn from(snapshot: IoCallSnapshot) -> Self {
        Self {
            inspect: snapshot.inspect,
            build_index: snapshot.build_index,
            decode: snapshot.decode,
            provider_frame_load: snapshot.provider_frame_load,
            format_detection_open_attempt: snapshot.format_detection_open_attempt,
        }
    }
}

#[derive(Serialize)]
struct QueryEvidence {
    after_preload: QueryCallSnapshotEvidence,
    after_run: QueryCallSnapshotEvidence,
}

#[derive(Serialize)]
struct QueryCallSnapshotEvidence {
    logical_requests: u64,
    executed_batches: u64,
    exact_key_reuses: u64,
    particle_loop_exact: ParticleLoopExactEvidence,
    by_origin: BTreeMap<String, QueryOriginEvidence>,
}

#[derive(Serialize)]
struct ParticleLoopExactEvidence {
    unique_exact_keys: u64,
    repeated_exact_executions: u64,
    first_repeated_executed_key: Option<String>,
}

#[derive(Serialize)]
struct QueryOriginEvidence {
    logical_requests: u64,
    executed_batches: u64,
    exact_key_reuses: u64,
}

impl From<QueryCallSnapshot> for QueryCallSnapshotEvidence {
    fn from(snapshot: QueryCallSnapshot) -> Self {
        Self {
            logical_requests: snapshot.logical_requests,
            executed_batches: snapshot.executed_batches,
            exact_key_reuses: snapshot.exact_key_reuses,
            particle_loop_exact: ParticleLoopExactEvidence {
                unique_exact_keys: snapshot.particle_loop_exact.unique_exact_keys,
                repeated_exact_executions: snapshot.particle_loop_exact.repeated_exact_executions,
                first_repeated_executed_key: snapshot
                    .particle_loop_exact
                    .first_repeated_executed_key,
            },
            by_origin: snapshot
                .by_origin
                .into_iter()
                .map(|(origin, counts)| {
                    (
                        origin.code().into(),
                        QueryOriginEvidence {
                            logical_requests: counts.logical_requests,
                            executed_batches: counts.executed_batches,
                            exact_key_reuses: counts.exact_key_reuses,
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct LifecycleCoverageEvidence {
    scheduled_output_times_expected: i64,
    scheduled_output_times_present: i64,
    missing_scheduled_output_times: i64,
    duplicate_scheduled_output_times: i64,
    extra_lifecycle_output_events: i64,
    invalid_extra_event_kinds: i64,
    expected_scheduled_state_rows: i64,
    expected_lifecycle_only_state_rows: i64,
    expected_rows_from_lifecycle: i64,
    actual_particle_state_rows: i64,
    invalid_sample_sequence_particles: i64,
    nonmonotonic_sample_event_particles: i64,
    birth_state_mismatches: i64,
    termination_state_mismatches: i64,
    unterminated_missing_final_state: i64,
    scheduled_state_mismatches: i64,
    unrelated_lifecycle_state_rows: i64,
    state_event_time_mismatches: i64,
    empty_output_events: i64,
    invalid_output_event_sequence: i64,
    valid: bool,
}

#[derive(Serialize)]
struct RunEvidence {
    family: &'static str,
    direction: &'static str,
    run_id: String,
    status: &'static str,
    numerical_steps: u64,
    seeded_particles: u64,
    final_particle_rows: usize,
    output_event_rows: i64,
    particle_state_rows: i64,
    lifecycle_coverage: LifecycleCoverageEvidence,
    normal_terminations: u64,
    mass_ledger_records: usize,
    maximum_ledger_fraction: f64,
    abnormal_terminations: u64,
    lock_build_preload_milliseconds: u128,
    run_milliseconds: u128,
    process_elapsed_milliseconds: u128,
    execute_io_gate_valid: bool,
    query_accounting_valid: bool,
    particle_loop_query_gate: ParticleLoopQueryGateEvidence,
    column_cache: ColumnCacheEvidence,
    output_identity: OutputIdentityEvidence,
    io: IoEvidence,
    queries: QueryEvidence,
    performance_attribution: Option<PerformanceAttributionEvidence>,
    performance_artifact_relative_path: Option<PathBuf>,
    manifest_relative_path: PathBuf,
}

#[derive(Serialize)]
struct OutputIdentityEvidence {
    manifest_bundle_sha256: Option<String>,
    manifest_sqlite_sha256: Option<String>,
    manifest_content_sha256: Option<String>,
    manifest_sqlite_sql_sha256: Option<String>,
    manifest_canonical_output_sha256: Option<String>,
    disk_bundle_sha256: Option<String>,
    disk_sqlite_sha256: Option<String>,
    recomputed_content_sha256: Option<String>,
    recomputed_sqlite_sql_sha256: Option<String>,
    recomputed_canonical_output_sha256: Option<String>,
    recomputed_record_count: Option<u64>,
    recomputed_field_set_count: Option<u64>,
    recomputed_sample_count: Option<u64>,
    diagnostics: Vec<String>,
    valid: bool,
}

#[derive(Clone, Serialize)]
struct PerformanceAttributionEvidence {
    schema_version: &'static str,
    stages: BTreeMap<String, PerformanceObservationEvidence>,
    distributions: BTreeMap<String, PerformanceObservationEvidence>,
}

#[derive(Clone, Serialize)]
struct PerformanceObservationEvidence {
    observations: u64,
    total: u64,
    maximum: u64,
    log2_histogram: Vec<u64>,
}

impl From<PerformanceObservationSnapshot> for PerformanceObservationEvidence {
    fn from(snapshot: PerformanceObservationSnapshot) -> Self {
        Self {
            observations: snapshot.observations,
            total: snapshot.total,
            maximum: snapshot.maximum,
            log2_histogram: snapshot.log2_histogram,
        }
    }
}

impl From<PerformanceSnapshot> for PerformanceAttributionEvidence {
    fn from(snapshot: PerformanceSnapshot) -> Self {
        Self {
            schema_version: "trajecta.m4-a4.5-performance-attribution/v1",
            stages: snapshot
                .stages
                .into_iter()
                .map(|(stage, observation)| (stage.code().into(), observation.into()))
                .collect(),
            distributions: snapshot
                .distributions
                .into_iter()
                .map(|(distribution, observation)| (distribution.code().into(), observation.into()))
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct ColumnCacheEvidence {
    hits: u64,
    misses: u64,
    evictions: u64,
    resident_bytes: u64,
}

#[derive(Serialize)]
struct ParticleLoopQueryGateEvidence {
    logical_requests: u64,
    executed_batches: u64,
    exact_key_reuses: u64,
    unique_exact_keys: u64,
    repeated_exact_executions: u64,
    interior_birth_time_count: u64,
    expected_integrator_logical_requests: u64,
    observed_integrator_logical_requests: u64,
    expected_output_logical_requests: u64,
    observed_output_logical_requests: u64,
    valid: bool,
}

struct CounterScope;

impl CounterScope {
    fn install(
        io_counters: std::sync::Arc<IoCallCounters>,
        query_counters: std::sync::Arc<QueryCallCounters>,
    ) -> Self {
        install_io_counters(io_counters);
        install_query_counters(query_counters);
        Self
    }
}

struct PerformanceCounterScope;

impl PerformanceCounterScope {
    fn install(counters: std::sync::Arc<PerformanceCounters>) -> Self {
        install_performance_counters(counters);
        Self
    }
}

impl Drop for PerformanceCounterScope {
    fn drop(&mut self) {
        clear_performance_counters();
    }
}

impl Drop for CounterScope {
    fn drop(&mut self) {
        clear_io_counters();
        clear_query_counters();
    }
}

fn audit_lifecycle_coverage(
    connection: &Connection,
    expected_schedule: &[Timestamp],
    direction: Direction,
    actual_rows: i64,
) -> LifecycleCoverageEvidence {
    assert!(!expected_schedule.is_empty());
    let schedule_values = expected_schedule
        .iter()
        .map(|time| {
            format!(
                "({}, {})",
                time.seconds_since_unix_epoch(),
                time.nanosecond()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let schedule_cte =
        format!("schedule(physical_seconds, physical_nanosecond) AS (VALUES {schedule_values})");
    let after_birth = match direction {
        Direction::Forward => {
            "(sc.physical_seconds > p.birth_seconds OR \
             (sc.physical_seconds = p.birth_seconds AND \
              sc.physical_nanosecond >= p.birth_nanosecond))"
        }
        Direction::Backward => {
            "(sc.physical_seconds < p.birth_seconds OR \
             (sc.physical_seconds = p.birth_seconds AND \
              sc.physical_nanosecond <= p.birth_nanosecond))"
        }
    };
    let before_termination = match direction {
        Direction::Forward => {
            "(t.particle_id IS NULL OR \
             sc.physical_seconds < t.physical_seconds OR \
             (sc.physical_seconds = t.physical_seconds AND \
              sc.physical_nanosecond <= t.physical_nanosecond))"
        }
        Direction::Backward => {
            "(t.particle_id IS NULL OR \
             sc.physical_seconds > t.physical_seconds OR \
             (sc.physical_seconds = t.physical_seconds AND \
              sc.physical_nanosecond >= t.physical_nanosecond))"
        }
    };
    let expected_schedule_cte = format!(
        "{schedule_cte}, expected_schedule AS (
             SELECT p.run_id, p.particle_id,
                    sc.physical_seconds, sc.physical_nanosecond
             FROM particle p
             CROSS JOIN schedule sc
             LEFT JOIN termination t
               ON t.run_id = p.run_id AND t.particle_id = p.particle_id
             WHERE {after_birth} AND {before_termination}
         )"
    );

    let (missing_scheduled_output_times, duplicate_scheduled_output_times): (i64, i64) = connection
        .query_row(
            &format!(
                "WITH {schedule_cte}, event_counts AS (
                     SELECT sc.physical_seconds, sc.physical_nanosecond,
                            COUNT(e.event_sequence) AS event_count
                     FROM schedule sc
                     LEFT JOIN output_event e
                       ON e.physical_seconds = sc.physical_seconds
                      AND e.physical_nanosecond = sc.physical_nanosecond
                     GROUP BY sc.physical_seconds, sc.physical_nanosecond
                 )
                 SELECT COALESCE(SUM(event_count = 0), 0),
                        COALESCE(SUM(event_count > 1), 0)
                 FROM event_counts"
            ),
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("scheduled output-event coverage");
    let scheduled_output_times_expected = i64::try_from(expected_schedule.len()).unwrap();
    let scheduled_output_times_present =
        scheduled_output_times_expected - missing_scheduled_output_times;
    let extra_lifecycle_output_events: i64 = connection
        .query_row(
            &format!(
                "WITH {schedule_cte}
                 SELECT COUNT(*)
                 FROM output_event e
                 WHERE NOT EXISTS (
                     SELECT 1 FROM schedule sc
                     WHERE sc.physical_seconds = e.physical_seconds
                       AND sc.physical_nanosecond = e.physical_nanosecond
                 )"
            ),
            [],
            |row| row.get(0),
        )
        .expect("extra lifecycle output events");
    let invalid_extra_event_kinds: i64 = connection
        .query_row(
            &format!(
                "WITH {schedule_cte}
                 SELECT COUNT(*)
                 FROM output_event e
                 WHERE NOT EXISTS (
                     SELECT 1 FROM schedule sc
                     WHERE sc.physical_seconds = e.physical_seconds
                       AND sc.physical_nanosecond = e.physical_nanosecond
                 )
                   AND e.event_kind NOT IN ('birth', 'termination')"
            ),
            [],
            |row| row.get(0),
        )
        .expect("extra lifecycle event kinds");
    let expected_scheduled_state_rows: i64 = connection
        .query_row(
            &format!("WITH {expected_schedule_cte} SELECT COUNT(*) FROM expected_schedule"),
            [],
            |row| row.get(0),
        )
        .expect("expected scheduled state rows");
    let expected_rows_from_lifecycle: i64 = connection
        .query_row(
            &format!(
                "WITH {expected_schedule_cte}, expected_state AS (
                     SELECT run_id, particle_id,
                            physical_seconds, physical_nanosecond
                     FROM expected_schedule
                     UNION
                     SELECT run_id, particle_id, birth_seconds, birth_nanosecond
                     FROM particle
                     UNION
                     SELECT run_id, particle_id,
                            physical_seconds, physical_nanosecond
                     FROM termination
                 )
                 SELECT COUNT(*) FROM expected_state"
            ),
            [],
            |row| row.get(0),
        )
        .expect("lifecycle-derived state row count");
    let expected_lifecycle_only_state_rows =
        expected_rows_from_lifecycle - expected_scheduled_state_rows;
    let invalid_sample_sequence_particles: i64 = connection
        .query_row(
            "WITH per_particle AS (
                 SELECT run_id, particle_id,
                        MIN(sample_sequence) AS first_sample,
                        MAX(sample_sequence) AS last_sample,
                        COUNT(*) AS state_count
                 FROM particle_state
                 GROUP BY run_id, particle_id
             )
             SELECT COUNT(*)
             FROM particle p
             LEFT JOIN per_particle s
               ON s.run_id = p.run_id AND s.particle_id = p.particle_id
             WHERE s.particle_id IS NULL
                OR s.first_sample != 0
                OR s.state_count != s.last_sample + 1
                OR s.last_sample < 0",
            [],
            |row| row.get(0),
        )
        .expect("contiguous per-particle sample sequence");
    let nonmonotonic_sample_event_particles: i64 = connection
        .query_row(
            "SELECT COUNT(DISTINCT current.particle_id)
             FROM particle_state current
             JOIN particle_state previous
               ON previous.run_id = current.run_id
              AND previous.particle_id = current.particle_id
              AND previous.sample_sequence = current.sample_sequence - 1
             WHERE current.event_sequence <= previous.event_sequence",
            [],
            |row| row.get(0),
        )
        .expect("monotonic particle event sequence");
    let birth_state_mismatches: i64 = connection
        .query_row(
            "SELECT COUNT(*)
             FROM particle p
             WHERE (
                 SELECT COUNT(*) FROM particle_state s
                 WHERE s.run_id = p.run_id
                   AND s.particle_id = p.particle_id
                   AND s.sample_sequence = 0
                   AND s.physical_seconds = p.birth_seconds
                   AND s.physical_nanosecond = p.birth_nanosecond
             ) != 1",
            [],
            |row| row.get(0),
        )
        .expect("birth-state coverage");
    let termination_state_mismatches: i64 = connection
        .query_row(
            "WITH last_sample AS (
                 SELECT run_id, particle_id, MAX(sample_sequence) AS sample_sequence
                 FROM particle_state
                 GROUP BY run_id, particle_id
             )
             SELECT COUNT(*)
             FROM termination t
             LEFT JOIN last_sample last
               ON last.run_id = t.run_id AND last.particle_id = t.particle_id
             LEFT JOIN particle_state s
               ON s.run_id = t.run_id
              AND s.particle_id = t.particle_id
              AND s.sample_sequence = last.sample_sequence
             WHERE s.particle_id IS NULL
                OR s.physical_seconds != t.physical_seconds
                OR s.physical_nanosecond != t.physical_nanosecond
                OR s.particle_status != 'terminated'
                OR s.termination_reason != t.reason",
            [],
            |row| row.get(0),
        )
        .expect("termination-state coverage");
    let run_end = expected_schedule.last().unwrap();
    let unterminated_missing_final_state: i64 = connection
        .query_row(
            "WITH last_sample AS (
                 SELECT run_id, particle_id, MAX(sample_sequence) AS sample_sequence
                 FROM particle_state
                 GROUP BY run_id, particle_id
             )
             SELECT COUNT(*)
             FROM particle p
             LEFT JOIN termination t
               ON t.run_id = p.run_id AND t.particle_id = p.particle_id
             LEFT JOIN last_sample last
               ON last.run_id = p.run_id AND last.particle_id = p.particle_id
             LEFT JOIN particle_state s
               ON s.run_id = p.run_id
              AND s.particle_id = p.particle_id
              AND s.sample_sequence = last.sample_sequence
             WHERE t.particle_id IS NULL
               AND (s.particle_id IS NULL
                    OR s.physical_seconds != ?1
                    OR s.physical_nanosecond != ?2
                    OR s.particle_status != 'alive')",
            params![run_end.seconds_since_unix_epoch(), run_end.nanosecond()],
            |row| row.get(0),
        )
        .expect("unterminated final-state coverage");
    let scheduled_state_mismatches: i64 = connection
        .query_row(
            &format!(
                "WITH {expected_schedule_cte}
                 SELECT COUNT(*) FROM (
                     SELECT expected.run_id, expected.particle_id,
                            expected.physical_seconds, expected.physical_nanosecond
                     FROM expected_schedule expected
                     LEFT JOIN particle_state actual
                       ON actual.run_id = expected.run_id
                      AND actual.particle_id = expected.particle_id
                      AND actual.physical_seconds = expected.physical_seconds
                      AND actual.physical_nanosecond = expected.physical_nanosecond
                     GROUP BY expected.run_id, expected.particle_id,
                              expected.physical_seconds, expected.physical_nanosecond
                     HAVING COUNT(actual.sample_sequence) != 1
                 )"
            ),
            [],
            |row| row.get(0),
        )
        .expect("scheduled particle-state coverage");
    let unrelated_lifecycle_state_rows: i64 = connection
        .query_row(
            &format!(
                "WITH {schedule_cte}
                 SELECT COUNT(*)
                 FROM particle_state s
                 JOIN particle p
                   ON p.run_id = s.run_id AND p.particle_id = s.particle_id
                 LEFT JOIN termination t
                   ON t.run_id = s.run_id AND t.particle_id = s.particle_id
                 WHERE NOT EXISTS (
                     SELECT 1 FROM schedule sc
                     WHERE sc.physical_seconds = s.physical_seconds
                       AND sc.physical_nanosecond = s.physical_nanosecond
                 )
                   AND NOT (
                       s.physical_seconds = p.birth_seconds
                       AND s.physical_nanosecond = p.birth_nanosecond
                   )
                   AND NOT (
                       t.particle_id IS NOT NULL
                       AND s.physical_seconds = t.physical_seconds
                       AND s.physical_nanosecond = t.physical_nanosecond
                   )"
            ),
            [],
            |row| row.get(0),
        )
        .expect("unrelated lifecycle state rows");
    let state_event_time_mismatches: i64 = connection
        .query_row(
            "SELECT COUNT(*)
             FROM particle_state s
             JOIN output_event e
               ON e.run_id = s.run_id AND e.event_sequence = s.event_sequence
             WHERE s.physical_seconds != e.physical_seconds
                OR s.physical_nanosecond != e.physical_nanosecond",
            [],
            |row| row.get(0),
        )
        .expect("state/output-event time identity");
    let empty_output_events: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM (
                 SELECT run_id, event_sequence FROM output_event
                 EXCEPT
                 SELECT run_id, event_sequence FROM particle_state
             )",
            [],
            |row| row.get(0),
        )
        .expect("nonempty output events");
    let invalid_output_event_sequence: i64 = connection
        .query_row(
            "SELECT CASE
                 WHEN COUNT(*) > 0
                  AND MIN(event_sequence) = 0
                  AND COUNT(*) = MAX(event_sequence) + 1
                 THEN 0 ELSE 1 END
             FROM output_event",
            [],
            |row| row.get(0),
        )
        .expect("global output-event sequence");
    let valid = actual_rows == expected_rows_from_lifecycle
        && missing_scheduled_output_times == 0
        && duplicate_scheduled_output_times == 0
        && invalid_extra_event_kinds == 0
        && invalid_sample_sequence_particles == 0
        && nonmonotonic_sample_event_particles == 0
        && birth_state_mismatches == 0
        && termination_state_mismatches == 0
        && unterminated_missing_final_state == 0
        && scheduled_state_mismatches == 0
        && unrelated_lifecycle_state_rows == 0
        && state_event_time_mismatches == 0
        && empty_output_events == 0
        && invalid_output_event_sequence == 0;
    LifecycleCoverageEvidence {
        scheduled_output_times_expected,
        scheduled_output_times_present,
        missing_scheduled_output_times,
        duplicate_scheduled_output_times,
        extra_lifecycle_output_events,
        invalid_extra_event_kinds,
        expected_scheduled_state_rows,
        expected_lifecycle_only_state_rows,
        expected_rows_from_lifecycle,
        actual_particle_state_rows: actual_rows,
        invalid_sample_sequence_particles,
        nonmonotonic_sample_event_particles,
        birth_state_mismatches,
        termination_state_mismatches,
        unterminated_missing_final_state,
        scheduled_state_mismatches,
        unrelated_lifecycle_state_rows,
        state_event_time_mismatches,
        empty_output_events,
        invalid_output_event_sequence,
        valid,
    }
}

fn frozen_output_schedule(
    start: Timestamp,
    direction: Direction,
    duration_seconds: i64,
    interval_seconds: i64,
) -> Vec<Timestamp> {
    assert!(duration_seconds >= 0);
    assert!(interval_seconds > 0);
    assert_eq!(duration_seconds % interval_seconds, 0);
    let direction_sign = match direction {
        Direction::Forward => 1,
        Direction::Backward => -1,
    };
    (0..=duration_seconds / interval_seconds)
        .map(|step| {
            Timestamp::new(
                start.seconds_since_unix_epoch() + direction_sign * step * interval_seconds,
                start.nanosecond(),
            )
            .unwrap()
        })
        .collect()
}

fn audit_output_identity(
    run_id: &str,
    manifest_identity: Option<&ProvenanceBundleIdentity>,
    run_dir: &Path,
) -> OutputIdentityEvidence {
    let mut diagnostics = Vec::new();
    let bundle_path = run_dir.join("provenance-bundle.json");
    let sqlite_path = run_dir.join("particles.sqlite");
    let disk_bundle_sha256 = match file_sha256(&bundle_path) {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.push(format!("bundle exact SHA: {error:?}"));
            None
        }
    };
    let disk_sqlite_sha256 = match file_sha256(&sqlite_path) {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.push(format!("SQLite exact SHA: {error:?}"));
            None
        }
    };
    let inspection = match ParticleStateSqliteSink::inspect(&sqlite_path) {
        Ok(value) => Some(value),
        Err(error) => {
            diagnostics.push(format!("SQLite canonical inspection: {error:?}"));
            None
        }
    };
    let bundle_validation = match disk_sqlite_sha256.as_deref() {
        Some(sqlite_sha) => {
            match validate_bundle_file_semantics_loose(&bundle_path, run_id, sqlite_sha) {
                Ok(value) => Some(value),
                Err(error) => {
                    diagnostics.push(format!("bundle streaming semantic validation: {error:?}"));
                    None
                }
            }
        }
        None => None,
    };
    let recomputed_canonical_output_sha256 = match (inspection.as_ref(), bundle_validation.as_ref())
    {
        (Some(sqlite), Some(bundle)) => {
            match canonical_output_digest(&sqlite.canonical_sql_sha256, &bundle.content_sha256) {
                Ok(value) => Some(value),
                Err(error) => {
                    diagnostics.push(format!("canonical output digest: {error:?}"));
                    None
                }
            }
        }
        _ => None,
    };

    let manifest_bundle_sha256 = manifest_identity.map(|value| value.sha256.clone());
    let manifest_sqlite_sha256 = manifest_identity.map(|value| value.sqlite_sha256.clone());
    let manifest_content_sha256 = manifest_identity.map(|value| value.content_sha256.clone());
    let manifest_sqlite_sql_sha256 = manifest_identity.map(|value| value.sqlite_sql_sha256.clone());
    let manifest_canonical_output_sha256 =
        manifest_identity.map(|value| value.canonical_output_sha256.clone());
    if manifest_identity.is_none() {
        diagnostics.push("terminal manifest has no provenance identity".into());
    }

    let recomputed_content_sha256 = bundle_validation
        .as_ref()
        .map(|value| value.content_sha256.clone());
    let recomputed_sqlite_sql_sha256 = inspection
        .as_ref()
        .map(|value| value.canonical_sql_sha256.clone());
    let recomputed_record_count = bundle_validation.as_ref().map(|value| value.record_count);
    let recomputed_field_set_count = bundle_validation
        .as_ref()
        .map(|value| value.field_set_count);
    let recomputed_sample_count = bundle_validation.as_ref().map(|value| value.sample_count);
    let valid = manifest_identity.is_some_and(|identity| {
        disk_bundle_sha256.as_deref() == Some(identity.sha256.as_str())
            && disk_sqlite_sha256.as_deref() == Some(identity.sqlite_sha256.as_str())
            && inspection
                .as_ref()
                .is_some_and(|value| value.sha256 == identity.sqlite_sha256)
            && recomputed_content_sha256.as_deref() == Some(identity.content_sha256.as_str())
            && recomputed_sqlite_sql_sha256.as_deref() == Some(identity.sqlite_sql_sha256.as_str())
            && recomputed_canonical_output_sha256.as_deref()
                == Some(identity.canonical_output_sha256.as_str())
            && recomputed_record_count == Some(identity.record_count)
            && recomputed_field_set_count == Some(identity.field_set_count)
            && recomputed_sample_count == Some(identity.sample_count)
    });
    if !valid && diagnostics.is_empty() {
        diagnostics.push("manifest/disk normalized output identity mismatch".into());
    }

    OutputIdentityEvidence {
        manifest_bundle_sha256,
        manifest_sqlite_sha256,
        manifest_content_sha256,
        manifest_sqlite_sql_sha256,
        manifest_canonical_output_sha256,
        disk_bundle_sha256,
        disk_sqlite_sha256,
        recomputed_content_sha256,
        recomputed_sqlite_sql_sha256,
        recomputed_canonical_output_sha256,
        recomputed_record_count,
        recomputed_field_set_count,
        recomputed_sample_count,
        diagnostics,
        valid,
    }
}

fn lifecycle_fixture(with_hole: bool) -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE particle (
                 run_id TEXT NOT NULL,
                 particle_id INTEGER NOT NULL,
                 birth_seconds INTEGER NOT NULL,
                 birth_nanosecond INTEGER NOT NULL
             );
             CREATE TABLE output_event (
                 run_id TEXT NOT NULL,
                 event_sequence INTEGER NOT NULL,
                 physical_seconds INTEGER NOT NULL,
                 physical_nanosecond INTEGER NOT NULL,
                 event_kind TEXT NOT NULL
             );
             CREATE TABLE termination (
                 run_id TEXT NOT NULL,
                 particle_id INTEGER NOT NULL,
                 physical_seconds INTEGER NOT NULL,
                 physical_nanosecond INTEGER NOT NULL,
                 reason TEXT NOT NULL
             );
             CREATE TABLE particle_state (
                 run_id TEXT NOT NULL,
                 particle_id INTEGER NOT NULL,
                 sample_sequence INTEGER NOT NULL,
                 event_sequence INTEGER NOT NULL,
                 physical_seconds INTEGER NOT NULL,
                 physical_nanosecond INTEGER NOT NULL,
                 particle_status TEXT NOT NULL,
                 termination_reason TEXT
             );",
        )
        .unwrap();
    for particle_id in [1_i64, 2] {
        connection
            .execute(
                "INSERT INTO particle
                 (run_id, particle_id, birth_seconds, birth_nanosecond)
                 VALUES ('run', ?1, 0, 0)",
                [particle_id],
            )
            .unwrap();
    }
    for event_sequence in 0_i64..7 {
        connection
            .execute(
                "INSERT INTO output_event
                 (run_id, event_sequence, physical_seconds, physical_nanosecond, event_kind)
                 VALUES ('run', ?1, ?2, 0, ?3)",
                params![
                    event_sequence,
                    event_sequence * 600,
                    match event_sequence {
                        0 => "birth",
                        2 => "termination",
                        6 => "end",
                        _ => "interval",
                    }
                ],
            )
            .unwrap();
        if !(with_hole && event_sequence == 3) {
            connection
                .execute(
                    "INSERT INTO particle_state
                     (run_id, particle_id, sample_sequence, event_sequence,
                      physical_seconds, physical_nanosecond,
                      particle_status, termination_reason)
                     VALUES ('run', 1, ?1, ?1, ?2, 0, 'alive', NULL)",
                    params![event_sequence, event_sequence * 600],
                )
                .unwrap();
        }
        if event_sequence <= 2 {
            let (status, reason) = if event_sequence == 2 {
                ("terminated", Some("population_outflow"))
            } else {
                ("alive", None)
            };
            connection
                .execute(
                    "INSERT INTO particle_state
                     (run_id, particle_id, sample_sequence, event_sequence,
                      physical_seconds, physical_nanosecond,
                      particle_status, termination_reason)
                     VALUES ('run', 2, ?1, ?1, ?2, 0, ?3, ?4)",
                    params![event_sequence, event_sequence * 600, status, reason],
                )
                .unwrap();
        }
    }
    connection
        .execute(
            "INSERT INTO termination
             (run_id, particle_id, physical_seconds, physical_nanosecond, reason)
             VALUES ('run', 2, 1200, 0, 'population_outflow')",
            [],
        )
        .unwrap();
    connection
}

fn lifecycle_fixture_with_dynamic_birth(write_unrelated_snapshot: bool) -> Connection {
    let connection = lifecycle_fixture(false);
    connection
        .execute(
            "INSERT INTO particle
             (run_id, particle_id, birth_seconds, birth_nanosecond)
             VALUES ('run', 3, 750, 0)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE output_event
             SET event_sequence = event_sequence + 1
             WHERE event_sequence >= 2",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE particle_state
             SET event_sequence = event_sequence + 1
             WHERE event_sequence >= 2",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO output_event
             (run_id, event_sequence, physical_seconds, physical_nanosecond, event_kind)
             VALUES ('run', 2, 750, 0, 'birth')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO particle_state
             (run_id, particle_id, sample_sequence, event_sequence,
              physical_seconds, physical_nanosecond, particle_status, termination_reason)
             VALUES ('run', 3, 0, 2, 750, 0, 'alive', NULL)",
            [],
        )
        .unwrap();
    for (sample_sequence, event_sequence) in (1_i64..=5).zip(3_i64..=7) {
        connection
            .execute(
                "INSERT INTO particle_state
                 (run_id, particle_id, sample_sequence, event_sequence,
                  physical_seconds, physical_nanosecond,
                  particle_status, termination_reason)
                 VALUES ('run', 3, ?1, ?2, ?3, 0, 'alive', NULL)",
                params![sample_sequence, event_sequence, (event_sequence - 1) * 600],
            )
            .unwrap();
    }
    if write_unrelated_snapshot {
        for particle_id in [1_i64, 2] {
            connection
                .execute(
                    "UPDATE particle_state
                     SET sample_sequence = sample_sequence + 1
                     WHERE particle_id = ?1 AND sample_sequence >= 2",
                    [particle_id],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO particle_state
                     (run_id, particle_id, sample_sequence, event_sequence,
                      physical_seconds, physical_nanosecond,
                      particle_status, termination_reason)
                     VALUES ('run', ?1, 2, 2, 750, 0, 'alive', NULL)",
                    [particle_id],
                )
                .unwrap();
        }
    }
    connection
}

#[test]
fn lifecycle_coverage_accepts_normal_termination_at_its_last_event() {
    let connection = lifecycle_fixture(false);
    let rows = connection
        .query_row("SELECT COUNT(*) FROM particle_state", [], |row| row.get(0))
        .unwrap();
    let schedule = frozen_output_schedule(Timestamp::UNIX_EPOCH, Direction::Forward, 3_600, 600);
    let audit = audit_lifecycle_coverage(&connection, &schedule, Direction::Forward, rows);
    assert_eq!(audit.scheduled_output_times_expected, 7);
    assert_eq!(audit.expected_scheduled_state_rows, 10);
    assert_eq!(audit.expected_lifecycle_only_state_rows, 0);
    assert_eq!(audit.expected_rows_from_lifecycle, 10);
    assert_eq!(audit.actual_particle_state_rows, 10);
    assert!(audit.valid);
}

#[test]
fn lifecycle_coverage_rejects_an_interior_state_hole() {
    let connection = lifecycle_fixture(true);
    let rows = connection
        .query_row("SELECT COUNT(*) FROM particle_state", [], |row| row.get(0))
        .unwrap();
    let schedule = frozen_output_schedule(Timestamp::UNIX_EPOCH, Direction::Forward, 3_600, 600);
    let audit = audit_lifecycle_coverage(&connection, &schedule, Direction::Forward, rows);
    assert_eq!(audit.actual_particle_state_rows, 9);
    assert_eq!(audit.invalid_sample_sequence_particles, 1);
    assert_eq!(audit.scheduled_state_mismatches, 1);
    assert!(!audit.valid);
}

#[test]
fn lifecycle_coverage_accepts_dynamic_birth_without_full_snapshot() {
    let connection = lifecycle_fixture_with_dynamic_birth(false);
    let rows = connection
        .query_row("SELECT COUNT(*) FROM particle_state", [], |row| row.get(0))
        .unwrap();
    let schedule = frozen_output_schedule(Timestamp::UNIX_EPOCH, Direction::Forward, 3_600, 600);
    let audit = audit_lifecycle_coverage(&connection, &schedule, Direction::Forward, rows);
    assert_eq!(audit.extra_lifecycle_output_events, 1);
    assert_eq!(audit.expected_scheduled_state_rows, 15);
    assert_eq!(audit.expected_lifecycle_only_state_rows, 1);
    assert_eq!(audit.actual_particle_state_rows, 16);
    assert!(audit.valid);
}

#[test]
fn lifecycle_coverage_rejects_unrelated_states_at_dynamic_birth() {
    let connection = lifecycle_fixture_with_dynamic_birth(true);
    let rows = connection
        .query_row("SELECT COUNT(*) FROM particle_state", [], |row| row.get(0))
        .unwrap();
    let schedule = frozen_output_schedule(Timestamp::UNIX_EPOCH, Direction::Forward, 3_600, 600);
    let audit = audit_lifecycle_coverage(&connection, &schedule, Direction::Forward, rows);
    assert_eq!(audit.expected_rows_from_lifecycle, 16);
    assert_eq!(audit.actual_particle_state_rows, 18);
    assert_eq!(audit.unrelated_lifecycle_state_rows, 2);
    assert!(!audit.valid);
}

#[test]
fn query_expectation_counts_only_distinct_interior_birth_times() {
    let start = Timestamp::new(10_000, 0).unwrap();
    let births = [
        start,
        Timestamp::new(10_300, 0).unwrap(),
        Timestamp::new(10_300, 0).unwrap(),
        Timestamp::new(10_600, 0).unwrap(),
        Timestamp::new(9_700, 0).unwrap(),
        Timestamp::new(9_400, 0).unwrap(),
    ];
    assert_eq!(interior_birth_time_count(&births, start, 600), 2);
}

#[test]
fn lifecycle_coverage_accepts_backward_schedule() {
    let connection = lifecycle_fixture(false);
    for table in ["output_event", "particle_state", "termination"] {
        connection
            .execute(
                &format!("UPDATE {table} SET physical_seconds = -physical_seconds"),
                [],
            )
            .unwrap();
    }
    let rows = connection
        .query_row("SELECT COUNT(*) FROM particle_state", [], |row| row.get(0))
        .unwrap();
    let schedule = frozen_output_schedule(Timestamp::UNIX_EPOCH, Direction::Backward, 3_600, 600);
    let audit = audit_lifecycle_coverage(&connection, &schedule, Direction::Backward, rows);
    assert!(audit.valid);
}

fn seconds(value: f64) -> Quantity<TimeDimension> {
    Quantity::from_si(value, Unit::new("s", Dimension::Time, 1.0, 0.0).unwrap()).unwrap()
}

fn capabilities() -> CapabilitySet {
    CapabilitySet::new()
        .with(Capability::Transport)
        .with(Capability::NearSurfaceTransport)
        .with(Capability::DomainFill)
}

fn fixture_directory(family: Family) -> PathBuf {
    std::env::var_os(family.environment_variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(family.relative_fixture))
}

fn required_env<T: std::str::FromStr>(name: &str) -> T
where
    T::Err: std::fmt::Debug,
{
    std::env::var(name)
        .unwrap_or_else(|_| panic!("required A4 environment variable {name} is unset"))
        .parse::<T>()
        .unwrap_or_else(|error| panic!("invalid {name}: {error:?}"))
}

fn target_particle_count() -> u64 {
    required_env("TRAJECTA_M4_A4_PARTICLES")
}

fn run_duration_seconds() -> i64 {
    required_env("TRAJECTA_M4_A4_DURATION_SECONDS")
}

fn time_step_seconds() -> i64 {
    required_env("TRAJECTA_M4_A4_TIME_STEP_SECONDS")
}

fn output_interval_seconds() -> i64 {
    required_env("TRAJECTA_M4_A4_OUTPUT_INTERVAL_SECONDS")
}

fn worker_threads() -> usize {
    required_env("TRAJECTA_M4_A4_WORKERS")
}

fn selected_mode() -> &'static str {
    match std::env::var("TRAJECTA_M4_A4_MODE").as_deref() {
        Ok("preflight") => "preflight",
        Ok("diagnostic") => "diagnostic",
        Ok("formal") => "formal",
        Ok("attribution") => "attribution",
        other => {
            panic!(
                "TRAJECTA_M4_A4_MODE must be preflight|diagnostic|formal|attribution, got {other:?}"
            )
        }
    }
}

fn performance_attribution_enabled() -> bool {
    match std::env::var("TRAJECTA_M4_A4_5_OBSERVE").as_deref() {
        Ok("1") => true,
        Err(std::env::VarError::NotPresent) | Ok("0") => false,
        other => panic!("TRAJECTA_M4_A4_5_OBSERVE must be 0|1, got {other:?}"),
    }
}

fn validate_frozen_cell() {
    assert_eq!(
        std::env::var("TRAJECTA_M4_A4_FAMILY").as_deref(),
        Ok("era5-hybrid"),
        "A4 frozen family must be era5-hybrid"
    );
    assert!(matches!(
        std::env::var("TRAJECTA_M4_A4_DIRECTION").as_deref(),
        Ok("forward") | Ok("backward")
    ));
    assert_eq!(run_duration_seconds(), 3_600, "A4 duration is frozen");
    assert_eq!(time_step_seconds(), 600, "A4 maximum step is frozen");
    assert_eq!(
        output_interval_seconds(),
        600,
        "A4 output interval is frozen"
    );
    let workers = worker_threads();
    assert!(matches!(workers, 1 | 4), "A4 workers must be 1 or 4");
    match selected_mode() {
        "preflight" => {
            assert_eq!(target_particle_count(), 1_000);
            assert_eq!(workers, 4);
            assert_eq!(
                std::env::var("TRAJECTA_M4_A4_DIRECTION").as_deref(),
                Ok("forward")
            );
        }
        "diagnostic" => {
            assert_eq!(target_particle_count(), 10_000);
            assert_eq!(workers, 4);
            assert_eq!(
                std::env::var("TRAJECTA_M4_A4_DIRECTION").as_deref(),
                Ok("forward")
            );
        }
        "formal" => assert!(matches!(target_particle_count(), 50_000 | 100_000)),
        "attribution" => {
            assert_eq!(target_particle_count(), 1_000);
            assert!(matches!(workers, 1 | 4));
            assert_eq!(
                std::env::var("TRAJECTA_M4_A4_DIRECTION").as_deref(),
                Ok("forward")
            );
        }
        _ => unreachable!(),
    }
}

fn selected_family(family: Family) -> bool {
    std::env::var("TRAJECTA_M4_A4_FAMILY")
        .ok()
        .is_none_or(|selected| selected == family.id)
}

fn selected_direction(direction: Direction) -> bool {
    std::env::var("TRAJECTA_M4_A4_DIRECTION")
        .ok()
        .is_none_or(|selected| {
            selected
                == match direction {
                    Direction::Forward => "forward",
                    Direction::Backward => "backward",
                }
        })
}

fn timestamp_delta_nanoseconds(start: Timestamp, end: Timestamp) -> i128 {
    let start_ns = i128::from(start.seconds_since_unix_epoch()) * 1_000_000_000
        + i128::from(start.nanosecond());
    let end_ns =
        i128::from(end.seconds_since_unix_epoch()) * 1_000_000_000 + i128::from(end.nanosecond());
    end_ns - start_ns
}

fn interior_birth_time_count(
    birth_times: &[Timestamp],
    run_start: Timestamp,
    macro_step_seconds: i64,
) -> u64 {
    let macro_step_ns = i128::from(macro_step_seconds) * 1_000_000_000;
    assert!(macro_step_ns > 0, "macro step must be positive");
    u64::try_from(
        birth_times
            .iter()
            .copied()
            .filter(|birth_time| {
                timestamp_delta_nanoseconds(run_start, *birth_time) % macro_step_ns != 0
            })
            .collect::<BTreeSet<_>>()
            .len(),
    )
    .expect("interior birth-time count")
}

fn build_lock(family: Family, fixture: &Path, control_root: &Path) -> (DatasetRef, PathBuf) {
    assert!(
        fixture.is_dir(),
        "missing {} fixture: {fixture:?}",
        family.id
    );
    let dataset = DatasetRef(format!("m4-a4-{}", family.id));
    let profiles = ProfileCatalog::load(&[]).expect("built-in profiles");
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: dataset.clone(),
            source: family.source.into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-core-m4-a4-real-matrix".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), fixture.to_path_buf())]),
        coverage: LockCoverageRequest {
            start: Timestamp::new(family.coverage_start, 0).unwrap(),
            end: Timestamp::new(family.coverage_end, 0).unwrap(),
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities(),
        force_rehash: true,
        preferred_profile: Some(family.profile.into()),
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    assert!(
        outcome.is_success(),
        "{} lock failed: {:?}",
        family.id,
        outcome.diagnostics
    );
    let lock = outcome.lock.expect("successful lock");
    assert_eq!(lock.profile.name, family.profile);
    let lock_path = control_root.join(format!("{}-dataset-lock.json", family.id));
    fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();
    (dataset, lock_path)
}

fn case(family: Family, dataset: DatasetRef, direction: Direction, particles: u64) -> ResolvedCase {
    let start = Timestamp::new(family.run_start, 0).unwrap();
    let duration_seconds = run_duration_seconds();
    assert_eq!(duration_seconds, 3_600, "A4 duration is frozen");
    let end_seconds = match direction {
        Direction::Forward => family.run_start + duration_seconds,
        Direction::Backward => family.run_start - duration_seconds,
    };
    let domain = DomainId(family.id.into());
    let horizontal_policy = if family.periodic_longitude {
        GLOBAL_PERIODIC_ID
    } else {
        LIMITED_DOMAIN_TERMINATE_ID
    };
    ResolvedCase {
        metadata: Metadata {
            name: format!("m4-a4-real-{}-{direction:?}", family.id).to_ascii_lowercase(),
            ..Metadata::default()
        },
        time: Some(TimeSpec {
            start,
            end: Timestamp::new(end_seconds, 0).unwrap(),
            direction,
        }),
        meteorology: Some(MeteorologySpec {
            domains: vec![DomainSpec {
                id: domain.clone(),
                dataset,
                priority: 0,
                parent: None,
                horizontal_halo_cells: 1,
            }],
        }),
        particle_population: Some(ParticlePopulationSpec::DomainFillAirMass(
            DomainFillAirMassSpec {
                id: PopulationId("air".into()),
                domain_id: domain,
                target_dry_air_mass_per_particle: None,
                target_particle_count: Some(particles),
            },
        )),
        substances: Vec::new(),
        numerics: Some(NumericsSpec {
            time_step: seconds(time_step_seconds() as f64),
            integrator: IntegratorSpec {
                model: ModelId(RK2_SPHERICAL_ID.into()),
                parameters: BTreeMap::new(),
            },
            boundaries: BoundarySpec {
                policies: vec![
                    ModelId(SURFACE_REFLECT_ID.into()),
                    ModelId(MODEL_TOP_TERMINATE_ID.into()),
                    ModelId(horizontal_policy.into()),
                ],
            },
            random_seed: Some(4_202),
        }),
        physics: Vec::new(),
        outputs: vec![trajecta_case::model::output::OutputProductSpec {
            product: ModelId(trajecta_case::model::output::PARTICLE_STATE_PRODUCT_ID.into()),
            schedule: OutputSchedule::Interval {
                interval: seconds(output_interval_seconds() as f64),
                origin: None,
            },
            sink: trajecta_case::model::output::OutputSinkSpec {
                model: ModelId(trajecta_case::model::output::PARTICLE_STATE_SQLITE_SINK_ID.into()),
                parameters: BTreeMap::new(),
            },
        }],
        sources: Vec::new(),
    }
}

fn run_family(
    family: Family,
    direction: Direction,
    particles: u64,
    output_root: &Path,
    control_root: &Path,
) -> RunEvidence {
    let process_started = Instant::now();
    let fixture = fixture_directory(family);
    let (dataset, lock_path) = build_lock(family, &fixture, control_root);
    let case = case(family, dataset.clone(), direction, particles);
    let case_path = control_root.join(format!("{}-{direction:?}-case.json", family.id));
    fs::write(&case_path, serde_json::to_vec_pretty(&case).unwrap()).unwrap();
    let profile = ResolvedRunProfile {
        metadata: Metadata {
            name: format!("m4-a4-real-{}-profile", family.id),
            ..Metadata::default()
        },
        case_path,
        output_root: output_root.to_path_buf(),
        datasets: vec![DatasetBinding {
            dataset,
            lockfile: lock_path,
            cache_root: None,
            data_roots: BTreeMap::from([(DataRootId("met".into()), fixture)]),
            reader_backend: Some(MeteorologyReaderBackend::Rust),
        }],
        profile_sources: Vec::new(),
        execution: ExecutionSpec {
            worker_threads: worker_threads(),
            memory_budget_bytes: 2 * 1_024 * 1_024 * 1_024,
            executor: "rayon".into(),
            meteorology_reader: MeteorologyReaderBackend::Rust,
        },
        sources: Vec::new(),
    };

    let counters = IoCallCounters::new();
    let query_counters = QueryCallCounters::new();
    let _counter_scope = CounterScope::install(
        std::sync::Arc::clone(&counters),
        std::sync::Arc::clone(&query_counters),
    );
    let mut runner = build_runner(case, profile, None).expect("production RunnerBuilder");
    let after_preload = counters.snapshot();
    let queries_after_preload = query_counters.snapshot();
    assert!(
        after_preload.total() > 0,
        "A4 production preload must observe real reader/provider I/O"
    );
    let performance_counters = performance_attribution_enabled().then(PerformanceCounters::new);
    let performance_scope = performance_counters
        .as_ref()
        .map(|counters| PerformanceCounterScope::install(std::sync::Arc::clone(counters)));
    let run_started = Instant::now();
    assert_eq!(
        runner.run(),
        Ok(RunOutcome::Complete),
        "{:?}",
        runner.manifest()
    );
    let run_elapsed = run_started.elapsed();
    let performance_attribution = performance_counters
        .as_ref()
        .map(|counters| counters.snapshot().into());
    drop(performance_scope);
    let after_run = counters.snapshot();
    let queries_after_run = query_counters.snapshot();
    let numerical_steps = runner.state().numerical_step_index;
    let interior_birth_time_count = interior_birth_time_count(
        &runner.state().particles.birth_time,
        Timestamp::new(family.run_start, 0).unwrap(),
        time_step_seconds(),
    );
    let expected_integrator_logical_requests = numerical_steps
        .checked_add(interior_birth_time_count)
        .and_then(|groups| groups.checked_mul(2))
        .expect("integrator query expectation overflow");
    let expected_output_logical_requests = numerical_steps
        .checked_add(1)
        .expect("output query expectation overflow");
    let execute_delta = after_run.saturating_sub(after_preload);
    let execute_io_gate_valid = execute_delta.total() == 0;
    let query_accounting_valid = queries_after_run.logical_requests > 0
        && queries_after_run.executed_batches > 0
        && queries_after_run.logical_requests
            == queries_after_run
                .executed_batches
                .saturating_add(queries_after_run.exact_key_reuses);
    let integrator_queries = queries_after_run
        .by_origin
        .get(&QueryOrigin::Integrator)
        .copied()
        .unwrap_or_default();
    let output_queries = queries_after_run
        .by_origin
        .get(&QueryOrigin::Output)
        .copied()
        .unwrap_or_default();
    let particle_loop_logical = integrator_queries
        .logical_requests
        .saturating_add(output_queries.logical_requests);
    let particle_loop_executed = integrator_queries
        .executed_batches
        .saturating_add(output_queries.executed_batches);
    let particle_loop_reused = integrator_queries
        .exact_key_reuses
        .saturating_add(output_queries.exact_key_reuses);
    let particle_loop_query_gate = ParticleLoopQueryGateEvidence {
        logical_requests: particle_loop_logical,
        executed_batches: particle_loop_executed,
        exact_key_reuses: particle_loop_reused,
        unique_exact_keys: queries_after_run.particle_loop_exact.unique_exact_keys,
        repeated_exact_executions: queries_after_run
            .particle_loop_exact
            .repeated_exact_executions,
        interior_birth_time_count,
        expected_integrator_logical_requests,
        observed_integrator_logical_requests: integrator_queries.logical_requests,
        expected_output_logical_requests,
        observed_output_logical_requests: output_queries.logical_requests,
        valid: integrator_queries.logical_requests == expected_integrator_logical_requests
            && output_queries.logical_requests == expected_output_logical_requests
            && integrator_queries.logical_requests
                == integrator_queries
                    .executed_batches
                    .saturating_add(integrator_queries.exact_key_reuses)
            && output_queries.logical_requests
                == output_queries
                    .executed_batches
                    .saturating_add(output_queries.exact_key_reuses)
            && particle_loop_logical
                == expected_integrator_logical_requests
                    .saturating_add(expected_output_logical_requests)
            && particle_loop_logical == particle_loop_executed.saturating_add(particle_loop_reused)
            && queries_after_run.particle_loop_exact.unique_exact_keys == particle_loop_executed
            && queries_after_run
                .particle_loop_exact
                .repeated_exact_executions
                == 0,
    };
    let process_elapsed = process_started.elapsed();
    let column_cache = runner
        .meteorology_column_cache_metrics()
        .expect("A4 column-cache metrics");
    let manifest = runner.manifest();
    assert_eq!(manifest.numerical.population, DRY_AIR_DOMAIN_FILL_ID);
    assert_eq!(manifest.terminations.abnormal_count, 0);
    assert!(!manifest.inputs.dataset_lock_sha256.is_empty());
    assert!(!manifest.inputs.dataset_profile_sha256.is_empty());
    assert!(!manifest.inputs.dataset_content_sha256.is_empty());
    assert_eq!(
        manifest.mass_ledger.len(),
        usize::try_from(runner.state().numerical_step_index).unwrap()
    );
    assert!(!manifest.mass_ledger.is_empty());
    let maximum_ledger_fraction = manifest
        .mass_ledger
        .iter()
        .map(|record| {
            assert!(record.imbalance_kg.abs() <= record.tolerance_kg);
            if record.tolerance_kg == 0.0 {
                0.0
            } else {
                record.imbalance_kg.abs() / record.tolerance_kg
            }
        })
        .fold(0.0_f64, f64::max);
    let seeded_particles = match &runner.state().population_state {
        trajecta_core::population::PopulationState::DomainFill {
            seeded_particles, ..
        } => *seeded_particles,
        other => panic!("unexpected population state: {other:?}"),
    };
    let final_particle_rows = runner.state().particles.len().unwrap();
    runner.state().particles.validate().unwrap();
    let case_dir = output_root.join(&manifest.case_name);
    let run_dir = case_dir.join(&manifest.run_id.0);
    let manifest_path = run_dir.join("run-manifest.json");
    let sqlite_path = run_dir.join("particles.sqlite");
    assert!(manifest_path.is_file(), "missing {manifest_path:?}");
    assert!(sqlite_path.is_file(), "missing {sqlite_path:?}");
    let connection = Connection::open_with_flags(&sqlite_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("read-only A4 particle database");
    let output_event_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM output_event", [], |row| row.get(0))
        .expect("output_event count");
    let particle_state_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM particle_state", [], |row| row.get(0))
        .expect("particle_state count");
    let expected_schedule = frozen_output_schedule(
        Timestamp::new(family.run_start, 0).unwrap(),
        direction,
        run_duration_seconds(),
        output_interval_seconds(),
    );
    let lifecycle_coverage = audit_lifecycle_coverage(
        &connection,
        &expected_schedule,
        direction,
        particle_state_rows,
    );
    drop(connection);
    let output_identity =
        audit_output_identity(&manifest.run_id.0, manifest.provenance.as_ref(), &run_dir);
    let performance_artifact_relative_path = performance_attribution.as_ref().map(|attribution| {
        let path = run_dir.join("A4_5_STAGE_TIMINGS.json");
        fs::write(&path, serde_json::to_vec_pretty(attribution).unwrap()).unwrap();
        path.strip_prefix(output_root)
            .expect("A4.5 performance artifact under output root")
            .to_path_buf()
    });
    RunEvidence {
        family: family.id,
        direction: match direction {
            Direction::Forward => "forward",
            Direction::Backward => "backward",
        },
        run_id: manifest.run_id.0.clone(),
        status: "complete",
        numerical_steps: runner.state().numerical_step_index,
        seeded_particles,
        final_particle_rows,
        output_event_rows,
        particle_state_rows,
        lifecycle_coverage,
        normal_terminations: manifest.terminations.normal_count,
        mass_ledger_records: manifest.mass_ledger.len(),
        maximum_ledger_fraction,
        abnormal_terminations: manifest.terminations.abnormal_count,
        lock_build_preload_milliseconds: run_started.duration_since(process_started).as_millis(),
        run_milliseconds: run_elapsed.as_millis(),
        process_elapsed_milliseconds: process_elapsed.as_millis(),
        execute_io_gate_valid,
        query_accounting_valid,
        particle_loop_query_gate,
        column_cache: ColumnCacheEvidence {
            hits: column_cache.hits,
            misses: column_cache.misses,
            evictions: column_cache.evictions,
            resident_bytes: column_cache.resident_bytes,
        },
        output_identity,
        io: IoEvidence {
            after_preload: after_preload.into(),
            after_run: after_run.into(),
            execute_delta: execute_delta.into(),
        },
        queries: QueryEvidence {
            after_preload: queries_after_preload.into(),
            after_run: queries_after_run.into(),
        },
        performance_attribution,
        performance_artifact_relative_path,
        manifest_relative_path: manifest_path
            .strip_prefix(output_root)
            .expect("manifest under output root")
            .to_path_buf(),
    }
}

#[test]
#[ignore = "explicit M4-A4 frozen real-data performance cell; run with --ignored"]
fn real_air_mass_domain_fill_hybrid_performance_cell() {
    validate_frozen_cell();
    let particles = target_particle_count();
    let temporary_output = tempdir().unwrap();
    let output_root = std::env::var_os("TRAJECTA_M4_A4_ARTIFACT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| temporary_output.path().to_path_buf());
    fs::create_dir_all(&output_root).unwrap();
    let control = tempdir().unwrap();
    let mut runs = Vec::new();
    for family in FAMILIES
        .into_iter()
        .filter(|family| selected_family(*family))
    {
        for direction in [Direction::Forward, Direction::Backward] {
            if !selected_direction(direction) {
                continue;
            }
            runs.push(run_family(
                family,
                direction,
                particles,
                &output_root,
                control.path(),
            ));
        }
    }
    assert!(!runs.is_empty(), "M4-A4 frozen matrix selection is empty");
    let all_lifecycle_coverage_valid = runs.iter().all(|run| run.lifecycle_coverage.valid);
    let all_execute_io_gates_valid = runs.iter().all(|run| run.execute_io_gate_valid);
    let all_query_accounting_valid = runs.iter().all(|run| run.query_accounting_valid);
    let all_particle_loop_query_gates_valid =
        runs.iter().all(|run| run.particle_loop_query_gate.valid);
    let all_output_identities_valid = runs.iter().all(|run| run.output_identity.valid);
    let performance_attribution_valid = if performance_attribution_enabled() {
        runs.iter().all(|run| {
            run.performance_artifact_relative_path.is_some()
                && run
                    .performance_attribution
                    .as_ref()
                    .is_some_and(|attribution| {
                        attribution
                            .stages
                            .get("runner_total")
                            .is_some_and(|stage| stage.observations == 1 && stage.total > 0)
                            && attribution
                                .stages
                                .get("runner_boundary")
                                .is_some_and(|stage| stage.observations > 0)
                            && attribution
                                .stages
                                .get("boundary_query_total")
                                .is_some_and(|stage| stage.observations > 0)
                            && attribution
                                .distributions
                                .get("boundary_segments_per_path")
                                .is_some_and(|distribution| distribution.observations > 0)
                    })
        })
    } else {
        runs.iter().all(|run| {
            run.performance_attribution.is_none()
                && run.performance_artifact_relative_path.is_none()
        })
    };
    let evidence = MatrixEvidence {
        schema_version: "trajecta.m4-a4-real-matrix/v1",
        mode: selected_mode(),
        target_particle_count: particles,
        worker_threads: worker_threads(),
        duration_seconds: run_duration_seconds(),
        time_step_seconds: time_step_seconds(),
        output_interval_seconds: output_interval_seconds(),
        runs,
    };
    fs::write(
        output_root.join("M4_A4_REAL_MATRIX_SUMMARY.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
    assert!(
        all_execute_io_gates_valid,
        "A4 execute phase must not perform reader/provider I/O; inspect the written summary"
    );
    assert!(
        all_query_accounting_valid,
        "every logical A4 query must execute or reuse one exact cached result; inspect the written summary"
    );
    assert!(
        all_particle_loop_query_gates_valid,
        "A4 particle-loop queries must match macro-step plus distinct interior-cohort expectations, account for every execute/reuse, and never execute one exact key twice; inspect the written summary"
    );
    assert!(
        all_lifecycle_coverage_valid,
        "A4 particle-state lifecycle coverage must be contiguous through normal termination or end"
    );
    assert!(
        all_output_identities_valid,
        "A4 manifest/bundle/SQLite normalized output identity must recompute from disk"
    );
    assert!(
        performance_attribution_valid,
        "A4.5 attribution mode must write non-empty bounded timing/path evidence"
    );
}
