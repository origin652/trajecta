//! # Contract: frozen M4 particle-loop science
//!
//! Algorithm identifiers, constants, and tolerances used by the particle loop.
//!
//! Any change in this module is a scientific-contract change and requires an
//! A-level audit plus regenerated M4 acceptance evidence.

use trajecta_met::science::{
    EARTH_ROTATION_RATE_RAD_S, ERTEL_PV_SPHERICAL_ALGORITHM_ID, M3_CONSTANTS,
    POTENTIAL_TEMPERATURE_REFERENCE_PRESSURE_PA,
};

/// Complete M4 particle-loop contract identity.
pub const M4_PARTICLE_LOOP_CONTRACT_ID: &str = "trajecta/particle_loop/m4/v1";
/// Spherical midpoint integrator identity.
pub const RK2_SPHERICAL_ID: &str = "rk2_spherical/v0";
/// Macro-step scheduler that advances existing particles once while starting
/// each new cohort at its own exact birth time.
pub const COHORT_LOCAL_SCHEDULER_ID: &str = "cohort_local_macro_step/v1";
/// One-sided proposal bridge used only to expose a horizontal RK2 stage exit
/// to the continuous limited-domain boundary solver.
pub const RK2_DOMAIN_EXIT_BRIDGE_ID: &str = "rk2_domain_exit_euler_bridge/v1";
/// Stable short-arc angle and oriented tangent construction.
pub const GREAT_CIRCLE_PATH_BASIS_ID: &str = "great_circle_atan2_oriented_basis/v1";
/// Continuous lower-surface reflection identity.
pub const SURFACE_REFLECT_ID: &str = "surface_reflect/v0";
/// Native model-top termination identity.
pub const MODEL_TOP_TERMINATE_ID: &str = "model_top_terminate/v0";
/// Limited-domain termination identity.
pub const LIMITED_DOMAIN_TERMINATE_ID: &str = "limited_domain_terminate/v0";
/// Periodic global-longitude identity.
pub const GLOBAL_PERIODIC_ID: &str = "global_periodic/v0";
/// Dry-air domain-fill identity.
pub const DRY_AIR_DOMAIN_FILL_ID: &str = "dry_air_domain_fill/v1";
/// Ordinary explicit-release population identity.
pub const RELEASE_DRIVEN_POPULATION_ID: &str = "release_driven/v1";
/// Integer-stratified release-birth time mapping identity.
pub const RELEASE_BIRTH_STRATA_ID: &str = "integer_stratified_birth/v1";
/// Stratospheric-ozone domain-fill population identity.
pub const OZONE_DOMAIN_FILL_ID: &str = "stratospheric_ozone_domain_fill/v1";
/// Latitude/longitude spherical cell-area identity.
pub const SPHERICAL_CELL_AREA_ID: &str = "spherical_lat_lon_cell_area/v1";
/// Pressure-level logarithmic-interface construction identity.
pub const PRESSURE_LOG_INTERFACE_ID: &str = "pressure_log_midpoint_interfaces/v1";
/// Domain-fill mass-ledger identity.
pub const DOMAIN_FILL_MASS_LEDGER_ID: &str = "domain_fill_mass/v1";
/// Equal-mass initial dry-air population sampling identity.
pub const DRY_AIR_INITIAL_SAMPLING_ID: &str = "dry_air_equal_mass_stratified/v1";
/// Complete-transport-floor-following relocation after pressure-stratified sampling.
pub const DRY_AIR_TRANSPORT_FLOOR_FOLLOWING_RELOCATION_ID: &str =
    "dry_air_transport_floor_following_horizontal_relocation/v1";
/// Log-pressure interpolation at the complete-transport lower boundary.
pub const DRY_AIR_TRANSPORT_FLOOR_PRESSURE_ID: &str = "dry_air_transport_floor_log_pressure/v1";
/// Static-interval midpoint boundary-flux time approximation identity.
pub const DRY_AIR_FLUX_TIME_ID: &str = "dry_air_static_midpoint_flux/v1";
/// Exact accumulated-mass-threshold boundary birth identity.
pub const DRY_AIR_BOUNDARY_BIRTH_ID: &str = "dry_air_mass_threshold_birth/v1";
/// Spherical Ertel-potential-vorticity identity.
pub const ERTEL_PV_SPHERICAL_ID: &str = ERTEL_PV_SPHERICAL_ALGORITHM_ID;
/// FLEXPART-derived empirical stratospheric ozone rule identity.
pub const FLEXPART_PV60_OZONE_ID: &str = "flexpart_stratospheric_ozone_pv60/v1";
/// Run-manifest schema identity.
pub const RUN_MANIFEST_SCHEMA_ID: &str = "trajecta.run-manifest/v1";
/// Particle-state provenance bundle schema identity.
pub const PROVENANCE_BUNDLE_SCHEMA_ID: &str = "trajecta.provenance-bundle/v1";
/// Canonical provenance artifact basename inside a run directory.
pub const PROVENANCE_BUNDLE_FILE_NAME: &str = "provenance-bundle.json";
/// Content-address algorithm for provenance records and five-field sets.
pub const PROVENANCE_RECORD_HASH_ALGORITHM: &str = "sha256-rfc8785";
/// Public SQLite schema version.
pub const SQLITE_SCHEMA_VERSION: u32 = 2;

/// Frozen physical and numerical constants for M4.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M4ScienceConstants {
    /// Conventional standard gravity in metres per second squared.
    pub standard_gravity_m_s2: f64,
    /// Spherical Earth radius inherited from the M3 query geometry.
    pub earth_radius_m: f64,
    /// Dry-air gas constant in joules per kilogram kelvin.
    pub dry_air_gas_constant_j_kg_k: f64,
    /// Water-vapour gas constant in joules per kilogram kelvin.
    pub water_vapour_gas_constant_j_kg_k: f64,
    /// Dry-air heat capacity at constant pressure in joules per kilogram kelvin.
    pub dry_air_heat_capacity_j_kg_k: f64,
    /// Earth rotation rate used by spherical Ertel PV in radians per second.
    pub earth_rotation_rate_rad_s: f64,
    /// Potential-temperature reference pressure in pascals.
    pub potential_temperature_reference_pressure_pa: f64,
    /// Fixed minimum post-reflection clearance in metres.
    pub surface_clearance_min_m: f64,
    /// ULP multiplier used for post-reflection clearance.
    pub surface_clearance_ulps: u32,
    /// Maximum lower-surface reflections allowed in one numerical step.
    pub maximum_surface_reflections_per_step: u32,
    /// Deterministic bisection iterations after a boundary crossing is bracketed.
    pub boundary_root_bisection_iterations: u32,
}

/// Frozen M4 constants.
pub const M4_CONSTANTS: M4ScienceConstants = M4ScienceConstants {
    standard_gravity_m_s2: M3_CONSTANTS.standard_gravity_m_s2,
    earth_radius_m: M3_CONSTANTS.earth_radius_m,
    dry_air_gas_constant_j_kg_k: M3_CONSTANTS.dry_air_gas_constant_j_kg_k,
    water_vapour_gas_constant_j_kg_k: M3_CONSTANTS.water_vapour_gas_constant_j_kg_k,
    dry_air_heat_capacity_j_kg_k: M3_CONSTANTS.dry_air_heat_capacity_j_kg_k,
    earth_rotation_rate_rad_s: EARTH_ROTATION_RATE_RAD_S,
    potential_temperature_reference_pressure_pa: POTENTIAL_TEMPERATURE_REFERENCE_PRESSURE_PA,
    surface_clearance_min_m: 1.0e-6,
    surface_clearance_ulps: 64,
    maximum_surface_reflections_per_step: 4,
    boundary_root_bisection_iterations: 64,
};

/// Per-step relative domain-fill mass tolerance.
pub const MASS_BALANCE_STEP_RELATIVE_TOLERANCE: f64 = 1.0e-12;
/// Final accumulated relative domain-fill mass tolerance.
pub const MASS_BALANCE_FINAL_RELATIVE_TOLERANCE: f64 = 1.0e-11;
/// Floating-point representation floor for mass gates.
pub const MASS_BALANCE_ULP_FLOOR: u32 = 64;
/// Relative area tolerance for canonical antimeridian splitting.
pub const GEOMETRY_AREA_RELATIVE_TOLERANCE: f64 = 1.0e-12;
/// Minimum accepted observed convergence order for analytic RK2 fixtures.
pub const RK2_MINIMUM_CONVERGENCE_ORDER: f64 = 1.9;

/// Strict minimum geometric height for the empirical PV60 ozone rule.
pub const OZONE_RULE_MINIMUM_HEIGHT_ASL_M: f64 = 3_000.0;
/// Strict minimum hemisphere-normalized potential vorticity in PVU.
pub const OZONE_RULE_MINIMUM_PV_PVU: f64 = 2.0;
/// Ozone mole-fraction slope in ppbv per PVU.
pub const OZONE_RULE_PPBV_PER_PVU: f64 = 60.0;
/// Ozone molar mass used by the empirical rule in grams per mole.
pub const OZONE_MOLAR_MASS_G_MOL: f64 = 48.0;
/// Dry-air molar mass used by the empirical rule in grams per mole.
pub const DRY_AIR_MOLAR_MASS_G_MOL: f64 = 29.0;
