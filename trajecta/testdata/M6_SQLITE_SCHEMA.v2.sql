PRAGMA foreign_keys = ON;
PRAGMA page_size = 32768;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA wal_autocheckpoint = 0;
PRAGMA user_version = 2;

CREATE TABLE run (
    run_id TEXT PRIMARY KEY NOT NULL,
    job_series_id TEXT NOT NULL,
    attempt INTEGER NOT NULL CHECK (attempt >= 1),
    parent_run_id TEXT REFERENCES run(run_id),
    resumed_from_checkpoint_id TEXT,
    adopted_output_event_sequence_exclusive INTEGER CHECK (
        adopted_output_event_sequence_exclusive IS NULL
        OR adopted_output_event_sequence_exclusive >= 0
    ),
    adopted_process_event_sequence_exclusive INTEGER CHECK (
        adopted_process_event_sequence_exclusive IS NULL
        OR adopted_process_event_sequence_exclusive >= 0
    ),
    manifest_schema TEXT NOT NULL,
    case_name TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('forward', 'backward')),
    status TEXT NOT NULL CHECK (status IN (
        'running', 'complete', 'completed_with_particle_errors',
        'failed', 'cancelled', 'interrupted'
    )),
    started_seconds INTEGER NOT NULL,
    started_nanosecond INTEGER NOT NULL CHECK (started_nanosecond BETWEEN 0 AND 999999999),
    finished_seconds INTEGER,
    finished_nanosecond INTEGER CHECK (
        finished_nanosecond IS NULL OR finished_nanosecond BETWEEN 0 AND 999999999
    ),
    CHECK (
        (parent_run_id IS NULL
            AND resumed_from_checkpoint_id IS NULL
            AND adopted_output_event_sequence_exclusive IS NULL
            AND adopted_process_event_sequence_exclusive IS NULL)
        OR
        (parent_run_id IS NOT NULL
            AND resumed_from_checkpoint_id IS NOT NULL
            AND adopted_output_event_sequence_exclusive IS NOT NULL
            AND adopted_process_event_sequence_exclusive IS NOT NULL)
    ),
    CHECK (
        (status = 'running' AND finished_seconds IS NULL AND finished_nanosecond IS NULL)
        OR
        (status <> 'running' AND finished_seconds IS NOT NULL AND finished_nanosecond IS NOT NULL)
    ),
    UNIQUE (job_series_id, attempt)
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

-- Forward state only. Full verification rejects rows from backward runs.
CREATE TABLE particle_mass (
    run_id TEXT NOT NULL,
    particle_id INTEGER NOT NULL,
    substance_id TEXT NOT NULL,
    initial_mass_kg REAL NOT NULL CHECK (initial_mass_kg >= 0),
    mass_kg REAL NOT NULL CHECK (mass_kg >= 0),
    PRIMARY KEY (run_id, particle_id, substance_id),
    FOREIGN KEY (run_id, particle_id) REFERENCES particle(run_id, particle_id)
) STRICT, WITHOUT ROWID;

-- Backward state only. These quantities are adjoint weights and sensitivities,
-- and are never reported as particle mass.
CREATE TABLE particle_adjoint (
    run_id TEXT NOT NULL,
    particle_id INTEGER NOT NULL,
    substance_id TEXT NOT NULL,
    initial_adjoint_weight REAL NOT NULL,
    adjoint_weight REAL NOT NULL,
    source_sensitivity REAL NOT NULL,
    PRIMARY KEY (run_id, particle_id, substance_id),
    FOREIGN KEY (run_id, particle_id) REFERENCES particle(run_id, particle_id)
) STRICT, WITHOUT ROWID;

-- Diameter is sampled once at birth and is immutable for the particle lifetime.
CREATE TABLE particle_aerosol_property (
    run_id TEXT NOT NULL,
    particle_id INTEGER NOT NULL,
    substance_id TEXT NOT NULL,
    diameter_m REAL NOT NULL CHECK (diameter_m BETWEEN 1e-8 AND 1e-4),
    material_density_kg_m3 REAL NOT NULL CHECK (material_density_kg_m3 > 0),
    shape TEXT NOT NULL CHECK (shape = 'sphere'),
    PRIMARY KEY (run_id, particle_id, substance_id),
    FOREIGN KEY (run_id, particle_id) REFERENCES particle(run_id, particle_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE output_event (
    run_id TEXT NOT NULL REFERENCES run(run_id),
    event_sequence INTEGER NOT NULL CHECK (event_sequence >= 0),
    physical_seconds INTEGER NOT NULL,
    physical_nanosecond INTEGER NOT NULL CHECK (physical_nanosecond BETWEEN 0 AND 999999999),
    event_kind TEXT NOT NULL CHECK (event_kind IN (
        'birth', 'start', 'interval', 'end', 'termination', 'checkpoint'
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
    boundary_layer_random_eastward_m_s REAL,
    boundary_layer_random_northward_m_s REAL,
    boundary_layer_random_vertical_m_s REAL,
    mesoscale_random_eastward_m_s REAL,
    mesoscale_random_northward_m_s REAL,
    mesoscale_random_vertical_m_s REAL,
    gravitational_settling_m_s REAL,
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

CREATE TABLE process_summary (
    run_id TEXT NOT NULL REFERENCES run(run_id),
    module_id TEXT NOT NULL,
    substance_id TEXT NOT NULL,
    direction TEXT NOT NULL CHECK (direction IN ('forward', 'backward')),
    event_count INTEGER NOT NULL CHECK (event_count >= 0),
    initial_mass_kg REAL,
    positive_mass_delta_kg REAL,
    negative_mass_delta_kg REAL,
    final_mass_kg REAL,
    initial_adjoint_weight REAL,
    survival_multiplier_product REAL,
    source_sensitivity REAL,
    convection_importance_product REAL,
    final_adjoint_weight REAL,
    closure_residual REAL NOT NULL,
    CHECK (
        (direction = 'forward'
            AND initial_mass_kg IS NOT NULL AND initial_mass_kg >= 0
            AND positive_mass_delta_kg IS NOT NULL AND positive_mass_delta_kg >= 0
            AND negative_mass_delta_kg IS NOT NULL AND negative_mass_delta_kg <= 0
            AND final_mass_kg IS NOT NULL AND final_mass_kg >= 0
            AND initial_adjoint_weight IS NULL
            AND survival_multiplier_product IS NULL
            AND source_sensitivity IS NULL
            AND convection_importance_product IS NULL
            AND final_adjoint_weight IS NULL)
        OR
        (direction = 'backward'
            AND initial_mass_kg IS NULL
            AND positive_mass_delta_kg IS NULL
            AND negative_mass_delta_kg IS NULL
            AND final_mass_kg IS NULL
            AND initial_adjoint_weight IS NOT NULL
            AND survival_multiplier_product IS NOT NULL AND survival_multiplier_product >= 0
            AND source_sensitivity IS NOT NULL
            AND convection_importance_product IS NOT NULL AND convection_importance_product >= 0
            AND final_adjoint_weight IS NOT NULL)
    ),
    PRIMARY KEY (run_id, module_id, substance_id)
) STRICT, WITHOUT ROWID;

-- One row is emitted for each particle/module/substance/macro-step tuple.
CREATE TABLE process_event (
    run_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL CHECK (event_sequence >= 0),
    particle_id INTEGER NOT NULL,
    macro_step INTEGER NOT NULL CHECK (macro_step >= 0),
    module_id TEXT NOT NULL,
    substance_id TEXT NOT NULL,
    physical_seconds INTEGER NOT NULL,
    physical_nanosecond INTEGER NOT NULL CHECK (physical_nanosecond BETWEEN 0 AND 999999999),
    direction TEXT NOT NULL CHECK (direction IN ('forward', 'backward')),
    detail_kind TEXT NOT NULL CHECK (detail_kind IN (
        'water_vapor', 'deposition', 'chemistry', 'emission', 'convection'
    )),
    mass_delta_kg REAL,
    survival_multiplier REAL,
    source_sensitivity REAL,
    importance_weight REAL,
    CHECK (
        (direction = 'forward'
            AND mass_delta_kg IS NOT NULL
            AND survival_multiplier IS NULL
            AND source_sensitivity IS NULL
            AND importance_weight IS NULL)
        OR
        (direction = 'backward'
            AND mass_delta_kg IS NULL
            AND survival_multiplier IS NOT NULL AND survival_multiplier >= 0
            AND source_sensitivity IS NOT NULL
            AND importance_weight IS NOT NULL AND importance_weight >= 0)
    ),
    PRIMARY KEY (run_id, event_sequence),
    UNIQUE (run_id, particle_id, module_id, substance_id, macro_step),
    FOREIGN KEY (run_id, particle_id) REFERENCES particle(run_id, particle_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE water_vapor_event (
    run_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL,
    evaporation_mass_kg REAL,
    precipitation_mass_kg REAL,
    unresolved_tendency_mass_kg REAL,
    evaporation_source_sensitivity REAL,
    precipitation_survival_multiplier REAL,
    unresolved_tendency_sensitivity REAL,
    CHECK (
        (evaporation_mass_kg IS NOT NULL
            AND precipitation_mass_kg IS NOT NULL
            AND unresolved_tendency_mass_kg IS NOT NULL
            AND evaporation_source_sensitivity IS NULL
            AND precipitation_survival_multiplier IS NULL
            AND unresolved_tendency_sensitivity IS NULL)
        OR
        (evaporation_mass_kg IS NULL
            AND precipitation_mass_kg IS NULL
            AND unresolved_tendency_mass_kg IS NULL
            AND evaporation_source_sensitivity IS NOT NULL
            AND precipitation_survival_multiplier IS NOT NULL
            AND precipitation_survival_multiplier >= 0
            AND unresolved_tendency_sensitivity IS NOT NULL)
    ),
    PRIMARY KEY (run_id, event_sequence),
    FOREIGN KEY (run_id, event_sequence) REFERENCES process_event(run_id, event_sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE deposition_event (
    run_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL,
    pathway TEXT NOT NULL CHECK (pathway IN (
        'dry_gas', 'dry_aerosol', 'wet_in_cloud_rain', 'wet_in_cloud_snow',
        'wet_below_cloud_rain', 'wet_below_cloud_snow'
    )),
    deposition_velocity_m_s REAL CHECK (
        deposition_velocity_m_s IS NULL OR deposition_velocity_m_s >= 0
    ),
    scavenging_coefficient_s_1 REAL CHECK (
        scavenging_coefficient_s_1 IS NULL OR scavenging_coefficient_s_1 >= 0
    ),
    removed_mass_kg REAL CHECK (removed_mass_kg IS NULL OR removed_mass_kg >= 0),
    survival_multiplier REAL CHECK (
        survival_multiplier IS NULL OR survival_multiplier BETWEEN 0 AND 1
    ),
    CHECK (
        (removed_mass_kg IS NOT NULL AND survival_multiplier IS NULL)
        OR
        (removed_mass_kg IS NULL AND survival_multiplier IS NOT NULL)
    ),
    PRIMARY KEY (run_id, event_sequence),
    FOREIGN KEY (run_id, event_sequence) REFERENCES process_event(run_id, event_sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE chemistry_event (
    run_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL,
    pathway TEXT NOT NULL CHECK (pathway IN ('half_life', 'oh_oxidation')),
    loss_rate_s_1 REAL NOT NULL CHECK (loss_rate_s_1 >= 0),
    survival_multiplier REAL NOT NULL CHECK (survival_multiplier BETWEEN 0 AND 1),
    removed_mass_kg REAL CHECK (removed_mass_kg IS NULL OR removed_mass_kg >= 0),
    PRIMARY KEY (run_id, event_sequence),
    FOREIGN KEY (run_id, event_sequence) REFERENCES process_event(run_id, event_sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE emission_event (
    run_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL,
    source_id TEXT NOT NULL,
    calendar_factor REAL NOT NULL CHECK (calendar_factor >= 0),
    scheduled_mass_kg REAL CHECK (scheduled_mass_kg IS NULL OR scheduled_mass_kg >= 0),
    source_sensitivity REAL,
    injection_height_asl_m REAL,
    plume_top_height_asl_m REAL,
    CHECK (
        (scheduled_mass_kg IS NOT NULL AND source_sensitivity IS NULL)
        OR
        (scheduled_mass_kg IS NULL AND source_sensitivity IS NOT NULL)
    ),
    PRIMARY KEY (run_id, event_sequence),
    FOREIGN KEY (run_id, event_sequence) REFERENCES process_event(run_id, event_sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE convection_event (
    run_id TEXT NOT NULL,
    event_sequence INTEGER NOT NULL,
    source_layer INTEGER NOT NULL CHECK (source_layer >= 0),
    destination_layer INTEGER NOT NULL CHECK (destination_layer >= 0),
    transfer_probability REAL NOT NULL CHECK (transfer_probability BETWEEN 0 AND 1),
    importance_weight REAL NOT NULL CHECK (importance_weight >= 0),
    column_residual REAL NOT NULL,
    PRIMARY KEY (run_id, event_sequence),
    FOREIGN KEY (run_id, event_sequence) REFERENCES process_event(run_id, event_sequence)
) STRICT, WITHOUT ROWID;

CREATE INDEX particle_state_by_time
    ON particle_state (run_id, physical_seconds, physical_nanosecond, particle_id);

CREATE INDEX output_event_by_time
    ON output_event (run_id, physical_seconds, physical_nanosecond, event_sequence);

CREATE INDEX particle_mass_by_substance
    ON particle_mass (run_id, substance_id, particle_id);

CREATE INDEX particle_adjoint_by_substance
    ON particle_adjoint (run_id, substance_id, particle_id);

CREATE INDEX process_event_by_filter
    ON process_event (
        run_id, module_id, substance_id,
        physical_seconds, physical_nanosecond,
        particle_id, event_sequence
    );

CREATE INDEX process_event_by_particle
    ON process_event (
        run_id, particle_id,
        physical_seconds, physical_nanosecond,
        module_id, substance_id, event_sequence
    );
