PRAGMA foreign_keys = ON;
PRAGMA page_size = 32768;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA wal_autocheckpoint = 0;
PRAGMA user_version = 1;

CREATE TABLE run (
    run_id TEXT PRIMARY KEY NOT NULL,
    manifest_schema TEXT NOT NULL,
    case_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'running', 'complete', 'completed_with_particle_errors', 'failed'
    )),
    started_seconds INTEGER NOT NULL,
    started_nanosecond INTEGER NOT NULL CHECK (started_nanosecond BETWEEN 0 AND 999999999),
    finished_seconds INTEGER,
    finished_nanosecond INTEGER CHECK (
        finished_nanosecond IS NULL OR finished_nanosecond BETWEEN 0 AND 999999999
    ),
    CHECK (
        (status = 'running' AND finished_seconds IS NULL AND finished_nanosecond IS NULL)
        OR
        (status <> 'running' AND finished_seconds IS NOT NULL AND finished_nanosecond IS NOT NULL)
    )
) STRICT;

CREATE TABLE particle (
    run_id TEXT NOT NULL REFERENCES run(run_id),
    particle_id INTEGER NOT NULL CHECK (particle_id >= 0),
    population_id TEXT NOT NULL,
    origin_kind TEXT NOT NULL CHECK (origin_kind IN (
        'release', 'domain_initial', 'domain_boundary'
    )),
    origin_event_id TEXT,
    origin_domain_id TEXT,
    origin_boundary_face_id INTEGER CHECK (
        origin_boundary_face_id IS NULL OR origin_boundary_face_id >= 0
    ),
    birth_seconds INTEGER NOT NULL,
    birth_nanosecond INTEGER NOT NULL CHECK (birth_nanosecond BETWEEN 0 AND 999999999),
    dry_air_mass_kg REAL NOT NULL CHECK (dry_air_mass_kg >= 0),
    sensitivity_weight REAL,
    CHECK (
        (origin_kind = 'release'
            AND origin_event_id IS NOT NULL
            AND origin_domain_id IS NULL
            AND origin_boundary_face_id IS NULL)
        OR
        (origin_kind = 'domain_initial'
            AND origin_event_id IS NULL
            AND origin_domain_id IS NOT NULL
            AND origin_boundary_face_id IS NULL)
        OR
        (origin_kind = 'domain_boundary'
            AND origin_event_id IS NULL
            AND origin_domain_id IS NOT NULL
            AND origin_boundary_face_id IS NOT NULL)
    ),
    PRIMARY KEY (run_id, particle_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE particle_mass (
    run_id TEXT NOT NULL,
    particle_id INTEGER NOT NULL,
    substance_id TEXT NOT NULL,
    mass_kg REAL NOT NULL CHECK (mass_kg >= 0),
    PRIMARY KEY (run_id, particle_id, substance_id),
    FOREIGN KEY (run_id, particle_id) REFERENCES particle(run_id, particle_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE output_event (
    run_id TEXT NOT NULL REFERENCES run(run_id),
    event_sequence INTEGER NOT NULL CHECK (event_sequence >= 0),
    physical_seconds INTEGER NOT NULL,
    physical_nanosecond INTEGER NOT NULL CHECK (physical_nanosecond BETWEEN 0 AND 999999999),
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'birth', 'start', 'interval', 'end', 'termination'
    )),
    PRIMARY KEY (run_id, event_sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE particle_state (
    run_id TEXT NOT NULL,
    particle_id INTEGER NOT NULL,
    sample_sequence INTEGER NOT NULL CHECK (sample_sequence >= 0),
    event_sequence INTEGER NOT NULL,
    physical_seconds INTEGER NOT NULL,
    physical_nanosecond INTEGER NOT NULL CHECK (physical_nanosecond BETWEEN 0 AND 999999999),
    integration_offset_ns INTEGER NOT NULL,
    elapsed_age_ns INTEGER NOT NULL CHECK (elapsed_age_ns >= 0),
    longitude_degrees REAL NOT NULL,
    latitude_degrees REAL NOT NULL CHECK (latitude_degrees BETWEEN -90 AND 90),
    height_asl_m REAL NOT NULL,
    particle_status TEXT NOT NULL CHECK (particle_status IN ('alive', 'terminated')),
    termination_reason TEXT,
    eastward_wind_m_s REAL,
    northward_wind_m_s REAL,
    geometric_vertical_velocity_m_s REAL,
    air_pressure_pa REAL,
    air_temperature_k REAL,
    wind_validity TEXT NOT NULL,
    wind_quality TEXT,
    pressure_validity TEXT NOT NULL,
    pressure_quality TEXT,
    temperature_validity TEXT NOT NULL,
    temperature_quality TEXT,
    provenance_id INTEGER,
    CHECK (
        (particle_status = 'alive' AND termination_reason IS NULL)
        OR
        (particle_status = 'terminated' AND termination_reason IS NOT NULL)
    ),
    PRIMARY KEY (run_id, particle_id, sample_sequence),
    FOREIGN KEY (run_id, particle_id) REFERENCES particle(run_id, particle_id),
    FOREIGN KEY (run_id, event_sequence) REFERENCES output_event(run_id, event_sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE termination (
    run_id TEXT NOT NULL,
    particle_id INTEGER NOT NULL,
    reason TEXT NOT NULL,
    classification TEXT NOT NULL CHECK (classification IN ('normal', 'abnormal')),
    physical_seconds INTEGER NOT NULL,
    physical_nanosecond INTEGER NOT NULL CHECK (physical_nanosecond BETWEEN 0 AND 999999999),
    intersection_fraction REAL CHECK (
        intersection_fraction IS NULL OR intersection_fraction BETWEEN 0 AND 1
    ),
    PRIMARY KEY (run_id, particle_id),
    FOREIGN KEY (run_id, particle_id) REFERENCES particle(run_id, particle_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX particle_state_by_time
    ON particle_state (run_id, physical_seconds, physical_nanosecond, particle_id);

CREATE INDEX output_event_by_time
    ON output_event (run_id, physical_seconds, physical_nanosecond, event_sequence);
