//! Evaluate Trajecta at a frozen FLEXPART oracle matrix and adjudicate it.
//!
//! This MIT-side tool only reads the JSON emitted by the separate GPL harness.
//! It never links FLEXPART or imports its implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_case::model::physics::ModelId;
use trajecta_case::model::time::Timestamp;
use trajecta_met::derive::height::{geometric_height_to_geopotential_m2_s2, geopotential_height_m};
use trajecta_met::field::{CanonicalField, Capability, CapabilitySet, FieldKey, FieldRegistry};
use trajecta_met::frame::{ArrayLayout, RawMetFrame};
use trajecta_met::grid::{GridBackend, RegularLatLonGrid};
use trajecta_met::io::frame_loader::{FrameLoadRequest, FrameLoader};
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};
use trajecta_met::profile::graph::ExecutionPlan;
use trajecta_met::query::cache::MemoryBudget;
use trajecta_met::query::engine::{
    BatchWorkspace, MetEngine, MetEngineConfig, RayonExecutionContext,
};
use trajecta_met::query::output::SampleStatus;
use trajecta_met::query::request::{
    ExplainMode, QueryBatch, QueryPlanRequest, QueryPointArrays, VerticalQuery,
};
use trajecta_met::surface_layer::{MoninObukhovBusingerDyer, SurfaceLayerRegistry};
use trajecta_met::validation::report::{
    ComparisonIdentity, ComparisonStatus, FieldSlab, ResultStatus, SampleContextOwned,
    SideMetadata, VectorElement, adjudicate, report_to_json,
};
use trajecta_met::validation::tolerance::load_registry_from_path;
use trajecta_met::vertical::{ColumnBuilder, ColumnRequest, HybridColumnBuilder, VerticalTopology};

const ALGORITHM_ID: &str = "trajecta/met_query/m3/v0";
const COMMON_TARGET: &str = "flexpart_common_semantics";
const DIFFERENCE_TARGET: &str = "flexpart_difference_report";
const COMPARISON_VARIANT: &str = "trajecta-vs-flexpart-v11.1";
/// A-registered pressure-coordinate adapter variant. It calls FLEXPART's
/// `verttransform_gfs` and only aliases the resulting `pplev` pressure field
/// into the meter-mode `prs` consumer slot.
const PRESSURE_ADAPTER_VARIANT: &str = "trajecta-vs-flexpart-pressure-adapter-v1";

fn comparison_variant_for(dataset_family: &str, comparison_target: &str) -> &'static str {
    match (dataset_family, comparison_target) {
        // The adapter is a distinct identity for common-semantics gates. The
        // deliberately non-certifying difference target remains the shared
        // FLEXPART-v11.1 diagnostic variant.
        ("era5_pressure" | "cfsr_pressure", COMMON_TARGET) => PRESSURE_ADAPTER_VARIANT,
        _ => COMPARISON_VARIANT,
    }
}

#[derive(Clone, Debug)]
struct Args {
    oracle: PathBuf,
    query: PathBuf,
    manifest: PathBuf,
    profile_path: PathBuf,
    profile_name: String,
    data_root: PathBuf,
    registry: PathBuf,
    subject_out: PathBuf,
    report_out: PathBuf,
    difference_report_out: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct OracleDocument {
    status: String,
    input: OracleInput,
    records: Vec<OracleRecord>,
}

#[derive(Clone, Debug, Deserialize)]
struct OracleInput {
    dataset_family: String,
    dataset_manifest_sha256: String,
    profile_sha256: String,
    query_sha256: String,
    files: Vec<OracleInputFile>,
}

#[derive(Clone, Debug, Deserialize)]
struct OracleInputFile {
    name: String,
    size: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
struct OracleRecord {
    point_id: String,
    sample_scope: String,
    time_unix_seconds: i64,
    time_nanosecond: u32,
    longitude_degrees: f64,
    latitude_degrees: f64,
    vertical_selector: VerticalSelector,
    status: String,
    fields: BTreeMap<String, OracleSample>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind")]
enum VerticalSelector {
    #[serde(rename = "physical")]
    Physical { coordinate: String, value: f64 },
    #[serde(rename = "native_full_level")]
    NativeFullLevel {
        level_index_from_top: usize,
        resolved_coordinate: String,
        resolved_value: f64,
    },
}

#[derive(Clone, Debug, Deserialize)]
struct OracleSample {
    valid: bool,
    value: Option<f64>,
    unit: String,
}

#[derive(Clone, Debug, Serialize)]
struct SubjectDocument {
    schema_version: &'static str,
    algorithm_id: &'static str,
    dataset_family: String,
    reference_sha256: String,
    registry_sha256: String,
    query_sha256: String,
    manifest_sha256: String,
    profile_name: String,
    profile_sha256: String,
    records: Vec<SubjectRecord>,
}

#[derive(Clone, Debug, Serialize)]
struct SubjectRecord {
    point_id: String,
    sample_scope: String,
    time_unix_seconds: i64,
    time_nanosecond: u32,
    longitude_degrees: f64,
    latitude_degrees: f64,
    vertical_selector: VerticalSelector,
    /// Point-level SampleStatus (engine / structural). Independent of field validity.
    sample_status: String,
    /// Aggregate of field validity only: "ok" if every present field is valid.
    status: String,
    fields: BTreeMap<String, SubjectSample>,
}

#[derive(Clone, Debug, Serialize)]
struct SubjectSample {
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<f64>,
    unit: &'static str,
    /// Field-level status token used by comparator Status exact-metadata.
    field_status: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum GroupCoordinate {
    NativeLevel,
    Asl,
    Agl,
    PressurePa,
}

impl GroupCoordinate {
    const fn label(self) -> &'static str {
        match self {
            Self::NativeLevel => "native_level",
            Self::Asl => "asl",
            Self::Agl => "agl",
            Self::PressurePa => "pressure_pa",
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("adjudicate_flexpart_oracle: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u8, String> {
    let args = parse_args()?;
    let oracle_bytes = fs::read(&args.oracle)
        .map_err(|error| format!("read oracle {}: {error}", args.oracle.display()))?;
    let oracle: OracleDocument = serde_json::from_slice(&oracle_bytes)
        .map_err(|error| format!("parse oracle {}: {error}", args.oracle.display()))?;
    if oracle.status == "failed" {
        return Err("reference oracle status is failed".into());
    }
    match oracle.input.dataset_family.as_str() {
        "era5_pressure" | "era5_hybrid" | "cfsr_pressure" => {}
        other => {
            return Err(format!(
                "unsupported dataset_family `{other}` (expected era5_pressure|era5_hybrid|cfsr_pressure)"
            ));
        }
    }

    verify_identity_file(&args.query, &oracle.input.query_sha256, "frozen query")?;
    verify_identity_file(
        &args.manifest,
        &oracle.input.dataset_manifest_sha256,
        "real-met manifest",
    )?;
    verify_identity_file(&args.profile_path, &oracle.input.profile_sha256, "Profile")?;
    verify_input_files(&args.data_root, &oracle.input.files)?;

    let registry = load_registry_from_path(&args.registry)?;
    let reference_sha256 = sha256_bytes(&oracle_bytes);
    let supported = oracle
        .records
        .iter()
        .filter(|record| record.status == "ok")
        .collect::<Vec<_>>();
    if supported.is_empty() {
        return Err("reference oracle contains no ok records".into());
    }
    for record in &supported {
        if !matches!(
            record.sample_scope.as_str(),
            "native_anchor" | "interpolated_common" | "surface_layer" | "modern_difference"
        ) {
            return Err(format!(
                "unsupported sample_scope `{}` on {}",
                record.sample_scope, record.point_id
            ));
        }
    }

    let coverage_start = supported
        .iter()
        .map(|record| record.time_unix_seconds)
        .min()
        .ok_or_else(|| "missing minimum valid time".to_string())?;
    let coverage_end = supported
        .iter()
        .map(|record| record.time_unix_seconds)
        .max()
        .ok_or_else(|| "missing maximum valid time".to_string())?;
    let (mut engine, frames) = build_engine(
        &args.data_root,
        &args.profile_name,
        coverage_start,
        coverage_end,
    )?;
    let free_fields = free_atmosphere_fields();
    let surface_fields = surface_and_modern_fields();
    let free_plan = engine
        .compile_plan(
            QueryPlanRequest {
                fields: free_fields
                    .iter()
                    .map(|(_, field, _)| field.clone())
                    .collect(),
                allow_estimated: false,
                surface_layer_model: None,
                explain: ExplainMode::Disabled,
            },
            &ExecutionPlan::default(),
        )
        .map_err(|error| format!("compile free-atmosphere plan: {error:?}"))?;
    let modern_plan = engine
        .compile_plan(
            QueryPlanRequest {
                fields: surface_fields
                    .iter()
                    .map(|(_, field, _)| field.clone())
                    .collect(),
                allow_estimated: false,
                surface_layer_model: None,
                explain: ExplainMode::Disabled,
            },
            &ExecutionPlan::default(),
        )
        .map_err(|error| format!("compile modern-difference plan: {error:?}"))?;
    let surface_plan = engine
        .compile_plan(
            QueryPlanRequest {
                fields: surface_fields
                    .iter()
                    .map(|(_, field, _)| field.clone())
                    .collect(),
                allow_estimated: false,
                surface_layer_model: Some(ModelId(MoninObukhovBusingerDyer::MODEL_ID.into())),
                explain: ExplainMode::Disabled,
            },
            &ExecutionPlan::default(),
        )
        .map_err(|error| format!("compile surface-layer plan: {error:?}"))?;

    let mut workspace = BatchWorkspace::default();
    let mut subject_records = Vec::with_capacity(supported.len());
    for reference in supported {
        if reference.sample_scope == "native_anchor" {
            subject_records.push(evaluate_native_record(reference, &frames)?);
            continue;
        }
        // Only surface_layer uses the Monin-Obukhov plan. modern_difference is
        // free-atmosphere density/W/terrain and must not require surface inputs.
        let use_surface = reference.sample_scope == "surface_layer";
        let use_modern = reference.sample_scope == "modern_difference";
        let plan = if use_surface {
            &surface_plan
        } else if use_modern {
            &modern_plan
        } else {
            &free_plan
        };
        let fields = if use_surface || use_modern {
            surface_fields.as_slice()
        } else {
            free_fields.as_slice()
        };
        let (vertical_coordinate, vertical) = query_vertical(&reference.vertical_selector)?;
        let time = Timestamp::new(reference.time_unix_seconds, reference.time_nanosecond)
            .map_err(|error| format!("{} time: {error:?}", reference.point_id))?;
        let window = engine
            .prepare(time)
            .map_err(|error| format!("{} prepare: {error:?}", reference.point_id))?;
        let output = window
            .prepare_batch(
                plan,
                QueryBatch {
                    vertical_coordinate,
                    points: QueryPointArrays {
                        longitude_degrees: vec![reference.longitude_degrees],
                        latitude_degrees: vec![reference.latitude_degrees],
                        vertical: vec![vertical],
                    },
                },
                &mut workspace,
            )
            .and_then(|prepared| {
                prepared.execute(&RayonExecutionContext { worker_threads: 1 }, &mut workspace)
            })
            .map_err(|error| format!("{} query: {error:?}", reference.point_id))?;
        let row = output
            .row(0)
            .ok_or_else(|| format!("{} missing output row", reference.point_id))?;
        let wanted: BTreeSet<&str> = reference.fields.keys().map(String::as_str).collect();
        let mut output_fields = BTreeMap::new();
        for (name, field, unit) in fields {
            if !wanted.is_empty() && !wanted.contains(name) {
                continue;
            }
            let value = row
                .value(field)
                .map(|number| {
                    if *name == "geopotential_height" {
                        geometric_height_to_geopotential_m2_s2(number)
                            .and_then(geopotential_height_m)
                            .map_err(|error| {
                                format!(
                                    "{} convert geometric to geopotential height: {error:?}",
                                    reference.point_id
                                )
                            })
                    } else {
                        Ok(number)
                    }
                })
                .transpose()?;
            if value.is_some_and(|number| !number.is_finite()) {
                return Err(format!("{} {name} is non-finite", reference.point_id));
            }
            output_fields.insert(
                (*name).to_string(),
                SubjectSample {
                    valid: value.is_some(),
                    value,
                    unit,
                    field_status: field_status_token(value.is_some()),
                },
            );
        }
        // Ensure every reference field has a subject slot (even if query missed).
        for name in &wanted {
            output_fields
                .entry((*name).to_string())
                .or_insert(SubjectSample {
                    valid: false,
                    value: None,
                    unit: static_unit_for(name),
                    field_status: field_status_token(false),
                });
        }
        subject_records.push(SubjectRecord {
            point_id: reference.point_id.clone(),
            sample_scope: reference.sample_scope.clone(),
            time_unix_seconds: reference.time_unix_seconds,
            time_nanosecond: reference.time_nanosecond,
            longitude_degrees: reference.longitude_degrees,
            latitude_degrees: reference.latitude_degrees,
            vertical_selector: reference.vertical_selector.clone(),
            sample_status: sample_status_label(row.status()).into(),
            status: record_field_aggregate_status(&output_fields),
            fields: output_fields,
        });
    }

    let subject = SubjectDocument {
        schema_version: "trajecta.m3.oracle_subject/v1",
        algorithm_id: ALGORITHM_ID,
        dataset_family: oracle.input.dataset_family.clone(),
        reference_sha256: reference_sha256.clone(),
        registry_sha256: registry.sha256.clone(),
        query_sha256: oracle.input.query_sha256.clone(),
        manifest_sha256: oracle.input.dataset_manifest_sha256.clone(),
        profile_name: args.profile_name.clone(),
        profile_sha256: oracle.input.profile_sha256.clone(),
        records: subject_records,
    };
    let subject_json = json_with_lf(&subject)?;
    let subject_sha256 = sha256_bytes(subject_json.as_bytes());
    write_parented(&args.subject_out, subject_json.as_bytes())?;

    let mut blockers = Vec::new();
    if oracle.status != "complete" {
        blockers.push(format!("reference_oracle_status_{}", oracle.status));
    }
    let unavailable = oracle.records.len().saturating_sub(subject.records.len());
    if unavailable > 0 {
        blockers.push(format!(
            "reference_oracle_coverage_{}_of_{}",
            subject.records.len(),
            oracle.records.len()
        ));
    }

    let common_slabs = build_slabs(
        &oracle,
        &subject,
        COMMON_TARGET,
        &["native_anchor", "interpolated_common"],
    )?;
    let difference_slabs = build_slabs(
        &oracle,
        &subject,
        DIFFERENCE_TARGET,
        &["surface_layer", "modern_difference"],
    )?;

    let common_ids = common_slabs
        .iter()
        .map(|slab| slab.slab_id.clone())
        .collect::<Vec<_>>();
    let difference_ids = difference_slabs
        .iter()
        .map(|slab| slab.slab_id.clone())
        .collect::<Vec<_>>();

    let common_report = adjudicate(
        &format!("m3/oracle/{}/common_semantics", oracle.input.dataset_family),
        &registry,
        ComparisonIdentity {
            dataset_family: oracle.input.dataset_family.clone(),
            comparison_target: COMMON_TARGET.into(),
            comparison_variant: comparison_variant_for(&oracle.input.dataset_family, COMMON_TARGET)
                .into(),
            subject_sha256: subject_sha256.clone(),
            reference_sha256: reference_sha256.clone(),
        },
        &common_ids,
        &common_slabs,
        &blockers,
    );
    write_parented(&args.report_out, report_to_json(&common_report)?.as_bytes())?;

    // Difference report is diagnostic: report_only rules must not auto-widen registry.
    // Blockers from incomplete common coverage still apply so partial is honest.
    let difference_report = adjudicate(
        &format!(
            "m3/oracle/{}/difference_report",
            oracle.input.dataset_family
        ),
        &registry,
        ComparisonIdentity {
            dataset_family: oracle.input.dataset_family.clone(),
            comparison_target: DIFFERENCE_TARGET.into(),
            comparison_variant: comparison_variant_for(
                &oracle.input.dataset_family,
                DIFFERENCE_TARGET,
            )
            .into(),
            subject_sha256: subject_sha256.clone(),
            reference_sha256: reference_sha256.clone(),
        },
        &difference_ids,
        &difference_slabs,
        &blockers,
    );
    write_parented(
        &args.difference_report_out,
        report_to_json(&difference_report)?.as_bytes(),
    )?;

    let hard_failures = common_report
        .results
        .iter()
        .filter(|result| result.status == ResultStatus::Fail)
        .count();
    let reported_results = common_report
        .results
        .iter()
        .filter(|result| result.status == ResultStatus::Reported)
        .count();
    let difference_failures = difference_report
        .results
        .iter()
        .filter(|result| result.status == ResultStatus::Fail)
        .count();
    // report_only failures stay report_only: they do not flip hard_gate status by themselves.
    let hard_status = comparison_status_label(common_report.status);
    println!(
        "{}",
        serde_json::json!({
            "status": hard_status,
            "hard_gate_status": hard_status,
            "difference_status": comparison_status_label(difference_report.status),
            "reference_status": oracle.status,
            "covered_records": subject.records.len(),
            "total_records": oracle.records.len(),
            "hard_result_rows": common_report.results.len(),
            "hard_failed_result_rows": hard_failures,
            "common_reported_result_rows": reported_results,
            "difference_result_rows": difference_report.results.len(),
            "difference_failed_result_rows": difference_failures,
            "subject": args.subject_out,
            "report": args.report_out,
            "difference_report": args.difference_report_out,
        })
    );

    if hard_failures > 0 {
        Ok(1)
    } else {
        match common_report.status {
            ComparisonStatus::Passed => Ok(0),
            ComparisonStatus::Incomplete => Ok(3),
            ComparisonStatus::Failed => Ok(1),
            ComparisonStatus::Unvalidated => Ok(2),
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut values = BTreeMap::new();
    let mut iter = std::env::args().skip(1);
    while let Some(flag) = iter.next() {
        if flag == "--help" || flag == "-h" {
            return Err(usage());
        }
        if !flag.starts_with("--") {
            return Err(format!("unexpected argument `{flag}`\n{}", usage()));
        }
        let value = iter
            .next()
            .ok_or_else(|| format!("missing value for `{flag}`\n{}", usage()))?;
        if values.insert(flag.clone(), value).is_some() {
            return Err(format!("duplicate argument `{flag}`"));
        }
    }
    let required = |name: &str| {
        values
            .get(name)
            .cloned()
            .ok_or_else(|| format!("missing `{name}`\n{}", usage()))
    };
    Ok(Args {
        oracle: PathBuf::from(required("--oracle")?),
        query: PathBuf::from(required("--query")?),
        manifest: PathBuf::from(required("--manifest")?),
        profile_path: PathBuf::from(required("--profile-path")?),
        profile_name: required("--profile-name")?,
        data_root: PathBuf::from(required("--data-root")?),
        registry: PathBuf::from(required("--registry")?),
        subject_out: PathBuf::from(required("--subject-out")?),
        report_out: PathBuf::from(required("--report-out")?),
        difference_report_out: PathBuf::from(required("--difference-report-out")?),
    })
}

fn usage() -> String {
    "usage: adjudicate_flexpart_oracle \\\n  --oracle PATH --query PATH --manifest PATH \\\n  --profile-path PATH --profile-name NAME --data-root DIR \\\n  --registry PATH --subject-out PATH --report-out PATH \\\n  --difference-report-out PATH"
        .into()
}

fn free_atmosphere_fields() -> Vec<(&'static str, FieldKey, &'static str)> {
    vec![
        (
            "eastward_wind",
            FieldKey::Canonical(CanonicalField::EastwardWind),
            "m s-1",
        ),
        (
            "northward_wind",
            FieldKey::Canonical(CanonicalField::NorthwardWind),
            "m s-1",
        ),
        (
            "air_pressure",
            FieldKey::Canonical(CanonicalField::AirPressure),
            "Pa",
        ),
        (
            "air_temperature",
            FieldKey::Canonical(CanonicalField::AirTemperature),
            "K",
        ),
        (
            "specific_humidity",
            FieldKey::Canonical(CanonicalField::SpecificHumidity),
            "kg kg-1",
        ),
        (
            "geopotential_height",
            FieldKey::Canonical(CanonicalField::GeometricHeight),
            "m",
        ),
    ]
}

fn surface_and_modern_fields() -> Vec<(&'static str, FieldKey, &'static str)> {
    vec![
        (
            "eastward_wind",
            FieldKey::Canonical(CanonicalField::EastwardWind),
            "m s-1",
        ),
        (
            "northward_wind",
            FieldKey::Canonical(CanonicalField::NorthwardWind),
            "m s-1",
        ),
        (
            "air_pressure",
            FieldKey::Canonical(CanonicalField::AirPressure),
            "Pa",
        ),
        (
            "air_temperature",
            FieldKey::Canonical(CanonicalField::AirTemperature),
            "K",
        ),
        (
            "specific_humidity",
            FieldKey::Canonical(CanonicalField::SpecificHumidity),
            "kg kg-1",
        ),
        (
            "geometric_vertical_velocity",
            FieldKey::Canonical(CanonicalField::GeometricVerticalVelocity),
            "m s-1",
        ),
        (
            "air_density",
            FieldKey::Canonical(CanonicalField::AirDensity),
            "kg m-3",
        ),
        (
            "geometric_terrain_height",
            FieldKey::Canonical(CanonicalField::GeometricTerrainHeight),
            "m",
        ),
    ]
}

fn static_unit_for(name: &str) -> &'static str {
    match name {
        "eastward_wind" | "northward_wind" | "geometric_vertical_velocity" => "m s-1",
        "air_temperature" => "K",
        "specific_humidity" => "kg kg-1",
        "air_pressure" => "Pa",
        "geopotential_height" | "geometric_terrain_height" => "m",
        "air_density" => "kg m-3",
        _ => "",
    }
}

fn query_vertical(selector: &VerticalSelector) -> Result<(VerticalQuery, f64), String> {
    match selector {
        VerticalSelector::Physical { coordinate, value } => {
            Ok((physical_coordinate(coordinate)?, *value))
        }
        VerticalSelector::NativeFullLevel {
            resolved_coordinate,
            resolved_value,
            ..
        } => Ok((physical_coordinate(resolved_coordinate)?, *resolved_value)),
    }
}

fn physical_coordinate(value: &str) -> Result<VerticalQuery, String> {
    match value {
        "asl" => Ok(VerticalQuery::AboveSeaLevel),
        "agl" => Ok(VerticalQuery::AboveGround),
        "pressure_pa" => Ok(VerticalQuery::Pressure),
        other => Err(format!("unsupported oracle vertical coordinate `{other}`")),
    }
}

fn group_coordinate(record: &OracleRecord) -> Result<GroupCoordinate, String> {
    if record.sample_scope == "native_anchor" {
        return Ok(GroupCoordinate::NativeLevel);
    }
    match &record.vertical_selector {
        VerticalSelector::Physical { coordinate, .. } => match coordinate.as_str() {
            "asl" => Ok(GroupCoordinate::Asl),
            "agl" => Ok(GroupCoordinate::Agl),
            "pressure_pa" => Ok(GroupCoordinate::PressurePa),
            other => Err(format!("unsupported group coordinate `{other}`")),
        },
        VerticalSelector::NativeFullLevel { .. } => Err(format!(
            "non-native scope {} uses native selector",
            record.point_id
        )),
    }
}

fn build_engine(
    data_root: &Path,
    profile_name: &str,
    coverage_start_unix: i64,
    coverage_end_unix: i64,
) -> Result<(MetEngine, Vec<Arc<RawMetFrame>>), String> {
    if !data_root.is_dir() {
        return Err(format!(
            "data root is not a directory: {}",
            data_root.display()
        ));
    }
    let profiles =
        ProfileCatalog::load(&[]).map_err(|error| format!("load Profile catalog: {error:?}"))?;
    if profiles.get(&ProfileName(profile_name.into())).is_none() {
        return Err(format!("unknown profile `{profile_name}`"));
    }
    // Near-surface fields are required for surface_layer Monin-Obukhov queries.
    let capabilities = CapabilitySet::new()
        .with(Capability::Transport)
        .with(Capability::NearSurfaceTransport);
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let start = Timestamp::new(coverage_start_unix, 0)
        .map_err(|error| format!("coverage start: {error:?}"))?;
    let end =
        Timestamp::new(coverage_end_unix, 0).map_err(|error| format!("coverage end: {error:?}"))?;
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("m3-flexpart-oracle-subject".into()),
            source: profile_name.into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "adjudicate_flexpart_oracle".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), data_root.to_path_buf())]),
        coverage: LockCoverageRequest {
            start,
            end,
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities,
        force_rehash: false,
        preferred_profile: Some(profile_name.into()),
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    if !outcome.is_success() {
        return Err(format!("dataset lock failed: {:?}", outcome.diagnostics));
    }
    let lock = outcome
        .lock
        .ok_or_else(|| "dataset lock missing".to_string())?;
    if lock.profile.name != profile_name {
        return Err(format!(
            "locked profile `{}` differs from requested `{profile_name}`",
            lock.profile.name
        ));
    }
    let domain = DomainId("oracle".into());
    let roots = BTreeMap::from([(DataRootId("met".into()), data_root.to_path_buf())]);
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir: data_root,
        data_roots: &roots,
        domain: &domain,
        required_capabilities: capabilities,
    });
    if !inventory.is_success() {
        return Err(format!(
            "inventory failed: {:?}",
            inventory.diagnostics.sorted()
        ));
    }
    let catalog = inventory
        .catalog
        .ok_or_else(|| "inventory catalog missing".to_string())?;
    let profile = profiles
        .get(&ProfileName(profile_name.into()))
        .ok_or_else(|| format!("unknown profile `{profile_name}`"))?;
    let descriptors = catalog
        .domains
        .get(&domain)
        .ok_or_else(|| "inventory domain missing".to_string())?;
    let mut frames = Vec::new();
    for descriptor in descriptors.frames.values() {
        let frame = FrameLoader::load(FrameLoadRequest {
            descriptor,
            profile,
            required_capabilities: capabilities,
            backend: MeteorologyReaderBackend::Rust,
            previous_frame: frames.last().map(Arc::as_ref),
        })
        .map_err(|error| format!("frame load: {error:?}"))?;
        frames.push(Arc::new(frame));
    }
    if frames.is_empty() {
        return Err("no frames loaded for oracle coverage".into());
    }
    let budget = MemoryBudget::new(512 * 1024 * 1024, 96 * 1024 * 1024)
        .map_err(|error| format!("memory budget: {error:?}"))?;
    let mut surface_layers = SurfaceLayerRegistry::new();
    surface_layers
        .register(Arc::new(MoninObukhovBusingerDyer::default()))
        .map_err(|error| format!("register surface layer model: {error:?}"))?;
    let mut engine = MetEngine::new(MetEngineConfig {
        catalog,
        profiles,
        fields: FieldRegistry::canonical()
            .map_err(|error| format!("canonical field registry: {error:?}"))?,
        surface_layers,
        memory_budget: budget,
    });
    for frame in &frames {
        engine
            .cache_frame(Arc::clone(frame))
            .map_err(|error| format!("cache frame: {error:?}"))?;
    }
    Ok((engine, frames))
}

fn field_status_token(valid: bool) -> String {
    if valid {
        "ok".into()
    } else {
        "invalid_field".into()
    }
}

fn record_field_aggregate_status(fields: &BTreeMap<String, SubjectSample>) -> String {
    if fields.values().all(|sample| sample.valid) {
        "ok".into()
    } else {
        "partial_fields".into()
    }
}

fn evaluate_native_record(
    reference: &OracleRecord,
    frames: &[Arc<RawMetFrame>],
) -> Result<SubjectRecord, String> {
    let frame = frames
        .iter()
        .find(|frame| {
            frame.metadata().valid_time.seconds_since_unix_epoch() == reference.time_unix_seconds
                && frame.metadata().valid_time.nanosecond() == reference.time_nanosecond
        })
        .ok_or_else(|| format!("{} exact frame missing", reference.point_id))?;
    let VerticalSelector::NativeFullLevel {
        level_index_from_top,
        resolved_coordinate,
        resolved_value,
    } = &reference.vertical_selector
    else {
        return Err(format!(
            "{} native anchor lacks native selector",
            reference.point_id
        ));
    };
    if resolved_coordinate != "pressure_pa" {
        return Err(format!(
            "{} native resolved_coordinate must be pressure_pa, got {resolved_coordinate}",
            reference.point_id
        ));
    }
    let grid = &frame.metadata().grid;
    let x = exact_grid_index(
        reference.longitude_degrees,
        grid.longitude_origin_degrees,
        grid.longitude_spacing_degrees,
        grid.nx,
        "longitude",
        &reference.point_id,
    )?;
    let y = exact_grid_index(
        reference.latitude_degrees,
        grid.latitude_origin_degrees,
        grid.latitude_spacing_degrees,
        grid.ny,
        "latitude",
        &reference.point_id,
    )?;
    let level_count = match &frame.metadata().vertical {
        VerticalTopology::PressureLevels(pressure) => pressure.pressure_pa.len(),
        VerticalTopology::HybridPressure(hybrid) => hybrid.active_full_levels.len(),
    };
    if *level_index_from_top >= level_count {
        return Err(format!(
            "{} native level_index_from_top {} out of range 0..{level_count}",
            reference.point_id, level_index_from_top
        ));
    }
    if let VerticalTopology::PressureLevels(pressure) = &frame.metadata().vertical {
        let pressure_pa = pressure.pressure_pa[*level_index_from_top];
        if (pressure_pa - *resolved_value).abs() > 1.0e-9 * pressure_pa.max(1.0) {
            return Err(format!(
                "{} resolved pressure mismatch selector={} topology={pressure_pa}",
                reference.point_id, resolved_value
            ));
        }
    }

    let mut fields = BTreeMap::new();
    for (name, canonical, unit) in [
        ("eastward_wind", CanonicalField::EastwardWind, "m s-1"),
        ("northward_wind", CanonicalField::NorthwardWind, "m s-1"),
        ("air_temperature", CanonicalField::AirTemperature, "K"),
        (
            "specific_humidity",
            CanonicalField::SpecificHumidity,
            "kg kg-1",
        ),
        ("air_pressure", CanonicalField::AirPressure, "Pa"),
    ] {
        match sample_native_field(
            frame,
            canonical,
            *level_index_from_top,
            y,
            x,
            unit,
            &reference.point_id,
        ) {
            Ok(sample) => {
                fields.insert(name.into(), sample);
            }
            Err(error) if name == "air_pressure" => {
                fields.insert(
                    name.into(),
                    SubjectSample {
                        valid: true,
                        value: Some(*resolved_value),
                        unit,
                        field_status: field_status_token(true),
                    },
                );
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
    if let VerticalTopology::PressureLevels(pressure) = &frame.metadata().vertical {
        fields.insert(
            "air_pressure".into(),
            SubjectSample {
                valid: true,
                value: Some(pressure.pressure_pa[*level_index_from_top]),
                unit: "Pa",
                field_status: field_status_token(true),
            },
        );
    }

    // Geopotential height:
    // - pressure topology: stored Full3D field
    // - hybrid topology: ColumnGeometry hydrostatic full-level geometric height
    //   converted to conventional geopotential height (never surface_z).
    let geo_sample = match &frame.metadata().vertical {
        VerticalTopology::PressureLevels(_) => sample_native_field(
            frame,
            CanonicalField::GeopotentialHeight,
            *level_index_from_top,
            y,
            x,
            "m",
            &reference.point_id,
        )?,
        VerticalTopology::HybridPressure(_) => {
            let latlon = RegularLatLonGrid::new(grid.clone()).map_err(|error| {
                format!(
                    "{} hybrid native grid geometry: {error:?}",
                    reference.point_id
                )
            })?;
            let cell = latlon
                .locate_cell(reference.longitude_degrees, reference.latitude_degrees)
                .map_err(|error| {
                    format!(
                        "{} hybrid native locate_cell: {error:?}",
                        reference.point_id
                    )
                })?;
            let request = ColumnRequest {
                frame,
                cell,
                longitude_degrees: reference.longitude_degrees,
                latitude_degrees: reference.latitude_degrees,
            };
            match HybridColumnBuilder.build(request) {
                Ok(column) => {
                    let geometric = column.height_asl_m().get(*level_index_from_top).copied();
                    match geometric {
                        Some(height) if height.is_finite() => {
                            let gph = geometric_height_to_geopotential_m2_s2(height)
                                .and_then(geopotential_height_m)
                                .map_err(|error| {
                                    format!(
                                        "{} hybrid ColumnGeometry height convert: {error:?}",
                                        reference.point_id
                                    )
                                })?;
                            SubjectSample {
                                valid: true,
                                value: Some(gph),
                                unit: "m",
                                field_status: field_status_token(true),
                            }
                        }
                        _ => SubjectSample {
                            valid: false,
                            value: None,
                            unit: "m",
                            field_status: field_status_token(false),
                        },
                    }
                }
                Err(error) => {
                    let _ = error;
                    SubjectSample {
                        valid: false,
                        value: None,
                        unit: "m",
                        field_status: field_status_token(false),
                    }
                }
            }
        }
    };
    fields.insert("geopotential_height".into(), geo_sample);

    let sample_status = if fields.values().all(|sample| sample.valid) {
        "ok"
    } else {
        "partial_fields"
    };
    Ok(SubjectRecord {
        point_id: reference.point_id.clone(),
        sample_scope: reference.sample_scope.clone(),
        time_unix_seconds: reference.time_unix_seconds,
        time_nanosecond: reference.time_nanosecond,
        longitude_degrees: reference.longitude_degrees,
        latitude_degrees: reference.latitude_degrees,
        vertical_selector: reference.vertical_selector.clone(),
        sample_status: sample_status.into(),
        status: record_field_aggregate_status(&fields),
        fields,
    })
}

fn exact_grid_index(
    coordinate: f64,
    origin: f64,
    spacing: f64,
    count: usize,
    label: &str,
    point_id: &str,
) -> Result<usize, String> {
    if !coordinate.is_finite() || !origin.is_finite() || !spacing.is_finite() || spacing == 0.0 {
        return Err(format!("{point_id} invalid {label} grid coordinate"));
    }
    let fractional = (coordinate - origin) / spacing;
    let rounded = fractional.round();
    if (fractional - rounded).abs() > 1.0e-8 {
        return Err(format!(
            "{point_id} {label}={coordinate} is not a native grid node ({fractional})"
        ));
    }
    let index = usize::try_from(rounded as i64)
        .map_err(|_| format!("{point_id} {label} index is negative"))?;
    if index >= count {
        return Err(format!(
            "{point_id} {label} index {index} outside 0..{count}"
        ));
    }
    Ok(index)
}

fn sample_native_field(
    frame: &RawMetFrame,
    canonical: CanonicalField,
    level: usize,
    y: usize,
    x: usize,
    unit: &'static str,
    point_id: &str,
) -> Result<SubjectSample, String> {
    let key = FieldKey::Canonical(canonical);
    let field = frame
        .fields()
        .get(&key)
        .ok_or_else(|| format!("{point_id} missing native field {canonical:?}"))?;
    let ArrayLayout::Full3D { levels, ny, nx } = field.layout() else {
        return Err(format!("{point_id} native {canonical:?} is not Full3D"));
    };
    if level >= levels || y >= ny || x >= nx {
        return Err(format!(
            "{point_id} native index ({level},{y},{x}) outside ({levels},{ny},{nx})"
        ));
    }
    let index = level
        .checked_mul(ny)
        .and_then(|value| value.checked_mul(nx))
        .and_then(|value| value.checked_add(y.checked_mul(nx)?))
        .and_then(|value| value.checked_add(x))
        .ok_or_else(|| format!("{point_id} native flat index overflow"))?;
    let valid = field
        .validity()
        .get(index)
        .ok_or_else(|| format!("{point_id} native validity index missing"))?;
    let value = field
        .values()
        .get(index)
        .copied()
        .ok_or_else(|| format!("{point_id} native value index missing"))?;
    Ok(SubjectSample {
        valid,
        value: valid.then_some(value),
        unit,
        field_status: field_status_token(valid),
    })
}

fn build_slabs(
    oracle: &OracleDocument,
    subject: &SubjectDocument,
    comparison_target: &str,
    scopes: &[&str],
) -> Result<Vec<FieldSlab>, String> {
    let subject_by_id = subject
        .records
        .iter()
        .map(|record| (record.point_id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut grouped: BTreeMap<(String, GroupCoordinate), Vec<&OracleRecord>> = BTreeMap::new();
    for record in oracle.records.iter().filter(|record| record.status == "ok") {
        if !scopes.iter().any(|scope| *scope == record.sample_scope) {
            continue;
        }
        grouped
            .entry((record.sample_scope.clone(), group_coordinate(record)?))
            .or_default()
            .push(record);
    }
    let mut slabs = Vec::new();
    for ((scope, coordinate), records) in grouped {
        let field_set = records
            .iter()
            .flat_map(|record| record.fields.keys().cloned())
            .collect::<BTreeSet<_>>();
        if field_set.contains("eastward_wind") && field_set.contains("northward_wind") {
            slabs.push(vector_slab(
                &oracle.input.dataset_family,
                comparison_target,
                &scope,
                coordinate,
                &records,
                &subject_by_id,
            )?);
        }
        for field in field_set {
            if field == "eastward_wind" || field == "northward_wind" {
                continue;
            }
            let namespace = if field == "geopotential_height" {
                "canonical_field"
            } else {
                "transport_output"
            };
            // Only include records that actually carry this field.
            let field_records = records
                .iter()
                .copied()
                .filter(|record| record.fields.contains_key(&field))
                .collect::<Vec<_>>();
            if field_records.is_empty() {
                continue;
            }
            slabs.push(scalar_slab(
                &oracle.input.dataset_family,
                comparison_target,
                &scope,
                coordinate,
                &field,
                namespace,
                &field_records,
                &subject_by_id,
            )?);
        }
    }
    Ok(slabs)
}

fn vector_slab(
    dataset_family: &str,
    comparison_target: &str,
    scope: &str,
    coordinate: GroupCoordinate,
    records: &[&OracleRecord],
    subject_by_id: &BTreeMap<&str, &SubjectRecord>,
) -> Result<FieldSlab, String> {
    let mut elements = Vec::with_capacity(records.len());
    for reference in records {
        let subject = subject_by_id
            .get(reference.point_id.as_str())
            .ok_or_else(|| format!("missing subject record {}", reference.point_id))?;
        let su = subject_sample(subject, "eastward_wind")?;
        let sv = subject_sample(subject, "northward_wind")?;
        let ru = oracle_sample(reference, "eastward_wind")?;
        let rv = oracle_sample(reference, "northward_wind")?;
        // Field-level status only — point sample_status must not pollute other fields.
        elements.push(VectorElement {
            case_id: reference.point_id.clone(),
            subject_u: su.value.unwrap_or(0.0),
            subject_v: sv.value.unwrap_or(0.0),
            reference_u: ru.value.unwrap_or(0.0),
            reference_v: rv.value.unwrap_or(0.0),
            subject_u_valid: su.valid,
            subject_v_valid: sv.valid,
            reference_u_valid: ru.valid,
            reference_v_valid: rv.valid,
            subject_u_unit: su.unit.into(),
            subject_v_unit: sv.unit.into(),
            reference_u_unit: ru.unit.clone(),
            reference_v_unit: rv.unit.clone(),
            subject_u_status: Some(su.field_status.clone()),
            subject_v_status: Some(sv.field_status.clone()),
            reference_u_status: Some(if ru.valid {
                "ok".into()
            } else {
                "invalid_field".into()
            }),
            reference_v_status: Some(if rv.valid {
                "ok".into()
            } else {
                "invalid_field".into()
            }),
        });
    }
    let slab_id = slab_id(dataset_family, scope, coordinate, "vector_wind");
    Ok(FieldSlab {
        slab_id: slab_id.clone(),
        context: context(
            dataset_family,
            comparison_target,
            scope,
            coordinate,
            "eastward_wind",
            "transport_output",
        ),
        // Status exact-metadata is field-level: both sides "ok" when the field channel exists.
        // Per-element validity/mask carries invalid fields; point sample_status is separate.
        subject_meta: side_meta("m s-1", Some("ok".into()), &slab_id, records.len()),
        reference_meta: side_meta("m s-1", Some("ok".into()), &slab_id, records.len()),
        expected_element_count: records.len(),
        subject_values: Vec::new(),
        reference_values: Vec::new(),
        subject_mask: Vec::new(),
        reference_mask: Vec::new(),
        element_case_ids: Vec::new(),
        vector_elements: Some(elements),
    })
}

#[allow(clippy::too_many_arguments)]
fn scalar_slab(
    dataset_family: &str,
    comparison_target: &str,
    scope: &str,
    coordinate: GroupCoordinate,
    field: &str,
    namespace: &str,
    records: &[&OracleRecord],
    subject_by_id: &BTreeMap<&str, &SubjectRecord>,
) -> Result<FieldSlab, String> {
    let mut subject_values = Vec::with_capacity(records.len());
    let mut reference_values = Vec::with_capacity(records.len());
    let mut subject_mask = Vec::with_capacity(records.len());
    let mut reference_mask = Vec::with_capacity(records.len());
    let mut element_case_ids = Vec::with_capacity(records.len());
    let mut subject_units = BTreeSet::new();
    let mut reference_units = BTreeSet::new();
    for reference in records {
        let subject = subject_by_id
            .get(reference.point_id.as_str())
            .ok_or_else(|| format!("missing subject record {}", reference.point_id))?;
        let subject_field = subject_sample(subject, field)?;
        let reference_field = oracle_sample(reference, field)?;
        subject_values.push(subject_field.value.unwrap_or(0.0));
        reference_values.push(reference_field.value.unwrap_or(0.0));
        subject_mask.push(subject_field.valid);
        reference_mask.push(reference_field.valid);
        element_case_ids.push(reference.point_id.clone());
        // Field-level only; point sample_status is recorded on SubjectRecord separately.
        subject_units.insert(subject_field.unit);
        reference_units.insert(reference_field.unit.as_str());
    }
    let subject_unit = one_unit(&subject_units, field, "subject")?;
    let reference_unit = one_unit(&reference_units, field, "reference")?;
    let slab_id = slab_id(dataset_family, scope, coordinate, field);
    Ok(FieldSlab {
        slab_id: slab_id.clone(),
        context: context(
            dataset_family,
            comparison_target,
            scope,
            coordinate,
            field,
            namespace,
        ),
        subject_meta: side_meta(subject_unit, Some("ok".into()), &slab_id, records.len()),
        reference_meta: side_meta(reference_unit, Some("ok".into()), &slab_id, records.len()),
        expected_element_count: records.len(),
        subject_values,
        reference_values,
        subject_mask,
        reference_mask,
        element_case_ids,
        vector_elements: None,
    })
}

fn context(
    dataset_family: &str,
    comparison_target: &str,
    scope: &str,
    coordinate: GroupCoordinate,
    field: &str,
    namespace: &str,
) -> SampleContextOwned {
    let vertical_region = match scope {
        "surface_layer" => "surface_layer",
        "modern_difference" if field == "geometric_terrain_height" => "all",
        _ => "upper_air",
    };
    SampleContextOwned {
        dataset_family: dataset_family.into(),
        comparison_target: comparison_target.into(),
        comparison_variant: comparison_variant_for(dataset_family, comparison_target).into(),
        sample_scope: scope.into(),
        field_namespace: namespace.into(),
        field: field.into(),
        coordinate: coordinate.label().into(),
        vertical_region: vertical_region.into(),
    }
}

fn side_meta(unit: &str, status: Option<String>, slab_id: &str, count: usize) -> SideMetadata {
    SideMetadata {
        valid_time_unix_seconds: 0,
        valid_time_nanosecond: 0,
        grid_signature: format!("oracle_query_points:{slab_id}"),
        vertical_signature: format!("oracle_vertical_selector:{slab_id}"),
        layout_signature: format!("record_vector:{count}"),
        unit: unit.into(),
        temporal_support: "query_time".into(),
        status,
    }
}

fn slab_id(dataset_family: &str, scope: &str, coordinate: GroupCoordinate, field: &str) -> String {
    format!("{dataset_family}/{scope}/{}/{field}", coordinate.label())
}

fn one_unit<'a>(units: &'a BTreeSet<&'a str>, field: &str, side: &str) -> Result<&'a str, String> {
    if units.len() != 1 {
        return Err(format!("{side} {field} has inconsistent units: {units:?}"));
    }
    units
        .first()
        .copied()
        .ok_or_else(|| format!("{side} {field} has no unit"))
}

fn subject_sample<'a>(record: &'a SubjectRecord, field: &str) -> Result<&'a SubjectSample, String> {
    record
        .fields
        .get(field)
        .ok_or_else(|| format!("{} missing subject field {field}", record.point_id))
}

fn oracle_sample<'a>(record: &'a OracleRecord, field: &str) -> Result<&'a OracleSample, String> {
    record
        .fields
        .get(field)
        .ok_or_else(|| format!("{} missing reference field {field}", record.point_id))
}

fn verify_identity_file(path: &Path, expected_sha256: &str, label: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{label} missing: {}", path.display()));
    }
    let actual = sha256_file(path)?;
    if actual != expected_sha256 {
        return Err(format!(
            "{label} SHA mismatch: {} expected={expected_sha256} actual={actual}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_input_files(data_root: &Path, expected: &[OracleInputFile]) -> Result<(), String> {
    for entry in expected {
        let path = data_root.join(&entry.name);
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("input file {}: {error}", path.display()))?;
        if metadata.len() != entry.size {
            return Err(format!(
                "input size mismatch {} expected={} actual={}",
                path.display(),
                entry.size,
                metadata.len()
            ));
        }
        verify_identity_file(&path, &entry.sha256, "oracle input")?;
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn json_with_lf<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map(|mut text| {
            text.push('\n');
            text
        })
        .map_err(|error| error.to_string())
}

fn write_parented(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

const fn sample_status_label(status: SampleStatus) -> &'static str {
    match status {
        SampleStatus::Ok => "ok",
        SampleStatus::OutOfDomain => "out_of_domain",
        SampleStatus::PolarSingularity => "polar_singularity",
        SampleStatus::BelowGround => "below_ground",
        SampleStatus::SurfaceLayerUndefined => "surface_layer_undefined",
        SampleStatus::AboveAvailableTop => "above_available_top",
        SampleStatus::AboveModelTop => "above_model_top",
        SampleStatus::InvalidVerticalColumn => "invalid_vertical_column",
        SampleStatus::NumericalFailure => "numerical_failure",
    }
}

const fn comparison_status_label(status: ComparisonStatus) -> &'static str {
    match status {
        ComparisonStatus::Passed => "passed",
        ComparisonStatus::Failed => "failed",
        ComparisonStatus::Incomplete => "incomplete",
        ComparisonStatus::Unvalidated => "unvalidated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_selector_maps_to_query_contract() {
        let pressure = VerticalSelector::Physical {
            coordinate: "pressure_pa".into(),
            value: 50_000.0,
        };
        assert_eq!(
            query_vertical(&pressure),
            Ok((VerticalQuery::Pressure, 50_000.0))
        );
        let native = VerticalSelector::NativeFullLevel {
            level_index_from_top: 3,
            resolved_coordinate: "asl".into(),
            resolved_value: 4_000.0,
        };
        assert_eq!(
            query_vertical(&native),
            Ok((VerticalQuery::AboveSeaLevel, 4_000.0))
        );
    }

    #[test]
    fn partial_reference_is_never_a_pass_status() {
        assert_eq!(
            comparison_status_label(ComparisonStatus::Incomplete),
            "incomplete"
        );
    }

    #[test]
    fn pressure_adapter_variant_is_scoped_to_common_semantics() {
        assert_eq!(
            comparison_variant_for("era5_pressure", COMMON_TARGET),
            PRESSURE_ADAPTER_VARIANT
        );
        assert_eq!(
            comparison_variant_for("cfsr_pressure", COMMON_TARGET),
            PRESSURE_ADAPTER_VARIANT
        );
        assert_eq!(
            comparison_variant_for("era5_pressure", DIFFERENCE_TARGET),
            COMPARISON_VARIANT
        );
        assert_eq!(
            comparison_variant_for("era5_hybrid", COMMON_TARGET),
            COMPARISON_VARIANT
        );
    }
}
