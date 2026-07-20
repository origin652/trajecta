//! # Contract: capability-bounded logical-frame loading
//!
//! A loader opens and indexes each unique locked source once, resolves only
//! source and derived fields reachable from requested capabilities, applies
//! explicit Profile fallback and unit rules, and publishes one immutable frame
//! without leaving hidden source I/O behind it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::lockfile::VerticalSignature;
use trajecta_case::model::time::Timestamp;

use crate::field::{CapabilitySet, FieldKey};
use crate::frame::{
    FrameComputationField, FrameError, FrameGraphExecutionError, FrameGraphExecutionRequest,
    FrameGraphExecutor, FrameMetadata, RawField, RawFieldStore, RawMetFrame, TemporalSupport,
};
use crate::grid::DomainGeometry;
use crate::io::counting_reader::CountingReader;
use crate::io::inventory::FrameDescriptor;
use crate::io::metrics::IoCallCounters;
use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, MetReader, ReaderFactory, SourceGridGeometry,
    SourceIndex, detect_source_format,
};
use crate::profile::document::{
    DatasetProfile, DerivedField, FieldMapping, FieldSource, SourceRole, TemporalKind,
    TemporalSemantics,
};
use crate::profile::graph::{ExecutionStage, GraphNodeId, GraphOp, GraphUnit};
use crate::provenance::{ProvenanceError, ProvenanceRecord, ProvenanceTable, TransformRecord};
use crate::science::{
    LATENT_HEAT_FROM_MOISTURE_FLUX_ALGORITHM_ID, SURFACE_PRESSURE_FROM_LOG_ALGORITHM_ID,
    TWO_METRE_SPECIFIC_HUMIDITY_FROM_DEWPOINT_ALGORITHM_ID,
    UPWARD_HEAT_FLUX_FROM_DOWNWARD_ALGORITHM_ID,
};
use crate::vertical::VerticalTopology;

/// Inputs required to publish one logical meteorological frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameLoadRequest<'a> {
    /// Verified logical frame and its local file-role mapping.
    pub descriptor: &'a FrameDescriptor,
    /// Exact active dataset Profile.
    pub profile: &'a DatasetProfile,
    /// Capabilities required by this run.
    pub required_capabilities: CapabilitySet,
    /// Explicit reader implementation; no fallback is attempted.
    pub backend: MeteorologyReaderBackend,
    /// Optional preceding frame for de-accumulation.
    pub previous_frame: Option<&'a RawMetFrame>,
}

/// Stateless loader for one immutable logical frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameLoader;

impl FrameLoader {
    /// Decodes, normalizes, derives, validates, and publishes one frame.
    ///
    /// Uses [`crate::io::metrics::active_io_counters`] when installed so ordinary
    /// production entry points share the same instrumentation context.
    pub fn load(request: FrameLoadRequest<'_>) -> Result<RawMetFrame, FrameLoadError> {
        Self::load_with_io(request, crate::io::metrics::active_io_counters())
    }

    /// Same as [`Self::load`], recording production I/O into shared counters.
    pub fn load_with_io(
        request: FrameLoadRequest<'_>,
        io_counters: Option<Arc<IoCallCounters>>,
    ) -> Result<RawMetFrame, FrameLoadError> {
        if request.profile.sha256 != request.descriptor.id.profile_sha256 {
            return Err(FrameLoadError::ProfileIdentityMismatch);
        }
        let requested_outputs = requested_outputs(request.profile, request.required_capabilities)?;
        if requested_outputs.is_empty() {
            return Err(FrameLoadError::NoRequestedFields);
        }
        let direct_dependencies = direct_dependencies(request.profile, &requested_outputs)?;
        let opened = open_sources(request.descriptor, request.backend, io_counters.clone())?;
        let (grid, vertical) = stable_geometry(request.descriptor, &opened)?;

        let mut provenance = ProvenanceTable::new();
        let mut raw_fields = RawFieldStore::new();
        let mut computations = BTreeMap::new();
        let mut direct_source_records = BTreeMap::<FieldKey, Vec<String>>::new();
        for field in &direct_dependencies {
            let mapping = mapping_for_field(request.profile, field)?;
            let selected = decode_mapping(mapping, &opened, request.descriptor.id.valid_time)?;
            let target_unit = source_node_unit(request.profile, mapping)?;
            let computation = normalize_direct_field(
                selected.decoded,
                selected.source,
                target_unit,
                &mapping.temporal,
                request.descriptor.id.valid_time,
            )?;
            let source_record = source_record(
                selected.path,
                &selected.decoded_identity,
                field,
                selected.source,
                &computation.unit,
                &request.profile.sha256,
            );
            let source_strings = source_record.sources.clone();
            let provenance_id = provenance.intern(source_record)?;
            let raw = RawField::new(
                computation.values.clone(),
                computation.valid.clone(),
                computation.unit.clone(),
                computation.layout,
                computation.temporal,
                selected.source.quality,
                provenance_id,
            )?;
            raw_fields.insert(field.clone(), raw)?;
            computations.insert(field.clone(), computation);
            direct_source_records.insert(field.clone(), source_strings);
        }

        let previous_sources = request.previous_frame.map(frame_computation_sources);
        let computed_outputs = FrameGraphExecutor::execute(FrameGraphExecutionRequest {
            graph: &request.profile.compiled.graph,
            execution_plan: &request.profile.compiled.execution_plan,
            requested_outputs: &requested_outputs,
            sources: &computations,
            previous_sources: previous_sources.as_ref(),
            valid_time: request.descriptor.id.valid_time,
            interval_seconds: request
                .profile
                .document
                .frame_interval_seconds
                .map(|value| value as f64),
        })?;

        for field in &requested_outputs {
            if direct_dependencies.contains(field) {
                continue;
            }
            let derived = request
                .profile
                .document
                .derived_fields
                .iter()
                .find(|candidate| candidate.target.to_field_key() == *field)
                .ok_or_else(|| FrameLoadError::MissingDerivedField(field.clone()))?;
            if derived.stage != ExecutionStage::Frame {
                return Err(FrameLoadError::UnsupportedDerivedStage {
                    field: field.clone(),
                    stage: derived.stage,
                });
            }
            let computed = computed_outputs
                .get(field)
                .cloned()
                .ok_or_else(|| FrameLoadError::MissingComputedField(field.clone()))?;
            let computed = apply_temporal_semantics(
                computed,
                &derived.temporal,
                request.descriptor.id.valid_time,
            )?;
            let dependencies = source_dependencies(request.profile, field)?;
            let sources = dependencies
                .iter()
                .flat_map(|dependency| {
                    direct_source_records
                        .get(dependency)
                        .into_iter()
                        .flatten()
                        .cloned()
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let provenance_id = provenance.intern(ProvenanceRecord {
                field: field.clone(),
                quality: derived.quality,
                sources,
                transforms: vec![TransformRecord {
                    operation: "profile_frame_graph".into(),
                    parameters: derived_transform_parameters(derived),
                }],
                fallback_reason: None,
                profile_sha256: request.profile.sha256.clone(),
            })?;
            raw_fields.insert(
                field.clone(),
                RawField::new(
                    computed.values,
                    computed.valid,
                    computed.unit,
                    computed.layout,
                    computed.temporal,
                    derived.quality,
                    provenance_id,
                )?,
            )?;
        }

        let domain = request.descriptor.id.domain.clone();
        let frame = RawMetFrame::publish(
            FrameMetadata {
                id: request.descriptor.id.clone(),
                domain: domain.clone(),
                valid_time: request.descriptor.id.valid_time,
                grid: DomainGeometry {
                    domain,
                    longitude_origin_degrees: grid.longitude_origin_degrees,
                    latitude_origin_degrees: grid.latitude_origin_degrees,
                    longitude_spacing_degrees: grid.longitude_spacing_degrees,
                    latitude_spacing_degrees: grid.latitude_spacing_degrees,
                    nx: grid.nx,
                    ny: grid.ny,
                    periodic_longitude: grid.periodic_longitude,
                    halo_cells: 1,
                },
                vertical,
            },
            raw_fields,
            Arc::new(provenance),
        )
        .map_err(FrameLoadError::Frame);
        // Count attempt regardless of publish success/failure.
        if let Some(counters) = io_counters {
            counters.record_provider_frame_load();
        }
        frame
    }
}

struct OpenedSource {
    path: PathBuf,
    /// Logical member role from [`FrameDescriptor::files`] (lock/inventory key).
    role: String,
    reader: Box<dyn MetReader>,
    index: Box<dyn SourceIndex>,
}

fn open_sources(
    descriptor: &FrameDescriptor,
    backend: MeteorologyReaderBackend,
    io_counters: Option<Arc<IoCallCounters>>,
) -> Result<Vec<OpenedSource>, FrameLoadError> {
    let mut opened = Vec::with_capacity(descriptor.files.len());
    // Preserve logical roles: multi-file Profiles select by role + variable + layout.
    for (role, path) in &descriptor.files {
        if let Some(counters) = io_counters.as_ref() {
            // Format detection open attempt only — not a full OS open tally.
            counters.record_format_detection_open_attempt();
        }
        let format = detect_source_format(path)?
            .ok_or_else(|| FrameLoadError::UnknownSourceFormat(path.clone()))?;
        let reader = ReaderFactory::create(format, backend)?;
        let reader: Box<dyn MetReader> = if let Some(counters) = io_counters.clone() {
            Box::new(CountingReader::new(reader, counters))
        } else {
            reader
        };
        let index = reader.build_index(path)?;
        opened.push(OpenedSource {
            path: path.clone(),
            role: role.clone(),
            reader,
            index,
        });
    }
    Ok(opened)
}

fn stable_geometry(
    descriptor: &FrameDescriptor,
    sources: &[OpenedSource],
) -> Result<(SourceGridGeometry, VerticalTopology), FrameLoadError> {
    let mut grid = None;
    let mut vertical = None;
    for source in sources {
        if source.index.grid_signature() != Some(&descriptor.grid) {
            return Err(FrameLoadError::GridMismatch);
        }
        if let Some(signature) = source.index.vertical_signature() {
            if signature != &descriptor.vertical {
                return Err(FrameLoadError::VerticalMismatch);
            }
        }
        if let Some(candidate) = source.index.grid_geometry() {
            if grid
                .as_ref()
                .is_some_and(|current: &SourceGridGeometry| current != candidate)
            {
                return Err(FrameLoadError::GridMismatch);
            }
            grid = Some(candidate.clone());
        }
        if let Some(candidate) = source.index.vertical_topology() {
            if vertical
                .as_ref()
                .is_some_and(|current: &VerticalTopology| current != candidate)
            {
                return Err(FrameLoadError::VerticalMismatch);
            }
            vertical = Some(candidate.clone());
        }
    }
    let grid = grid.ok_or(FrameLoadError::MissingGridGeometry)?;
    if grid.nx != descriptor.grid.nx || grid.ny != descriptor.grid.ny {
        return Err(FrameLoadError::GridMismatch);
    }
    let vertical = vertical.ok_or(FrameLoadError::MissingVerticalTopology)?;
    match (&descriptor.vertical, &vertical) {
        (
            VerticalSignature::HybridPressure {
                full_level_count, ..
            },
            VerticalTopology::HybridPressure(topology),
        ) if topology.coefficients.a_half_pa.len() == full_level_count.saturating_add(1) => {}
        (
            VerticalSignature::PressureLevels { level_count, .. },
            VerticalTopology::PressureLevels(levels),
        ) if levels.pressure_pa.len() == *level_count => {}
        _ => return Err(FrameLoadError::VerticalMismatch),
    }
    Ok((grid, vertical))
}

fn requested_outputs(
    profile: &DatasetProfile,
    capabilities: CapabilitySet,
) -> Result<BTreeSet<FieldKey>, FrameLoadError> {
    let mut fields = BTreeSet::new();
    for capability in capabilities.iter() {
        let required = profile
            .document
            .capabilities
            .get(&capability)
            .ok_or(FrameLoadError::MissingCapability(capability))?;
        fields.extend(required.iter().map(|field| field.to_field_key()));
    }
    Ok(fields)
}

fn direct_dependencies(
    profile: &DatasetProfile,
    requested: &BTreeSet<FieldKey>,
) -> Result<BTreeSet<FieldKey>, FrameLoadError> {
    let mut fields = BTreeSet::new();
    for output in requested {
        fields.extend(source_dependencies(profile, output)?);
    }
    Ok(fields)
}

fn source_dependencies(
    profile: &DatasetProfile,
    output: &FieldKey,
) -> Result<BTreeSet<FieldKey>, FrameLoadError> {
    let nodes = profile
        .compiled
        .graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<BTreeMap<_, _>>();
    let output_id = profile
        .compiled
        .graph
        .outputs
        .iter()
        .find_map(|(field, id)| (field == output).then_some(id.clone()))
        .ok_or_else(|| FrameLoadError::MissingGraphOutput(output.clone()))?;
    let mut pending = vec![output_id];
    let mut visited = BTreeSet::new();
    let mut fields = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .get(&id)
            .ok_or_else(|| FrameLoadError::MissingGraphNode(id.clone()))?;
        if let GraphOp::Source(field) = &node.operation {
            fields.insert(field.clone());
        }
        pending.extend(node.inputs.iter().cloned());
    }
    Ok(fields)
}

fn mapping_for_field<'a>(
    profile: &'a DatasetProfile,
    field: &FieldKey,
) -> Result<&'a FieldMapping, FrameLoadError> {
    profile
        .document
        .fields
        .iter()
        .find(|mapping| mapping.target.to_field_key() == *field)
        .ok_or_else(|| FrameLoadError::MissingDirectMapping(field.clone()))
}

fn source_node_unit<'a>(
    profile: &'a DatasetProfile,
    mapping: &FieldMapping,
) -> Result<&'a GraphUnit, FrameLoadError> {
    let id = GraphNodeId(format!("source:{}", mapping.id));
    profile
        .compiled
        .graph
        .nodes
        .iter()
        .find(|node| node.id == id)
        .map(|node| &node.output_type.unit)
        .ok_or(FrameLoadError::MissingGraphNode(id))
}

struct SelectedDecode<'a> {
    path: &'a Path,
    source: &'a FieldSource,
    decoded: DecodedField,
    decoded_identity: Vec<(String, String)>,
}

fn decode_mapping<'a>(
    mapping: &'a FieldMapping,
    opened: &'a [OpenedSource],
    valid_time: Timestamp,
) -> Result<SelectedDecode<'a>, FrameLoadError> {
    for alternative in &mapping.sources {
        let (reader_identity, required_role, expected_layout) =
            split_source_identity(&alternative.identity)?;
        let request = DecodeRequest {
            source_identity: reader_identity,
            valid_time: Some(valid_time),
        };
        let candidates: Vec<&OpenedSource> = match required_role {
            Some(role) => {
                let matched = opened
                    .iter()
                    .filter(|source| source.role == role)
                    .collect::<Vec<_>>();
                match matched.len() {
                    0 => {
                        return Err(FrameLoadError::MissingRole {
                            field: mapping.target.to_field_key(),
                            role: role.to_owned(),
                        });
                    }
                    1 => matched,
                    _ => {
                        return Err(FrameLoadError::DuplicateRole {
                            field: mapping.target.to_field_key(),
                            role: role.to_owned(),
                        });
                    }
                }
            }
            None => opened.iter().collect(),
        };
        let mut matches = Vec::new();
        for source in candidates {
            match source
                .reader
                .decode(&source.path, source.index.as_ref(), &request)
            {
                Ok(decoded) => matches.push((source, decoded)),
                Err(DecodeError::MissingField) => {}
                Err(error) => return Err(FrameLoadError::Decode(error)),
            }
        }
        match matches.len() {
            0 => {}
            1 => {
                let (source, decoded) = matches.pop().ok_or(FrameLoadError::MissingField)?;
                if let Some(expected) = expected_layout {
                    if !layout_matches_expected(&decoded.layout, expected) {
                        return Err(FrameLoadError::LayoutMismatch {
                            field: mapping.target.to_field_key(),
                            expected: expected.to_owned(),
                            actual: layout_label(&decoded.layout).into(),
                        });
                    }
                }
                let decoded_identity = decoded.source_identity.clone();
                return Ok(SelectedDecode {
                    path: &source.path,
                    source: alternative,
                    decoded,
                    decoded_identity,
                });
            }
            _ => {
                return Err(FrameLoadError::AmbiguousSource(
                    mapping.target.to_field_key(),
                ));
            }
        }
    }
    Err(FrameLoadError::MissingField)
}

/// Split Profile identity into loader selectors (`role`, `layout`) and reader keys.
///
/// Unknown selectors hard-fail; readers never see `role`/`layout`.
type SplitIdentity<'a> = (Vec<(String, String)>, Option<&'a str>, Option<&'a str>);

fn split_source_identity(
    identity: &BTreeMap<String, String>,
) -> Result<SplitIdentity<'_>, FrameLoadError> {
    let mut reader_identity = Vec::new();
    let mut required_role = None;
    let mut expected_layout = None;
    for (key, value) in identity {
        match key.as_str() {
            "role" => required_role = Some(value.as_str()),
            "layout" => expected_layout = Some(value.as_str()),
            // Format-specific reader keys (NetCDF variable/name/unit; GRIB discipline…).
            "variable"
            | "name"
            | "unit"
            | "param_id"
            | "discipline"
            | "parameter_category"
            | "parameter_number"
            | "type_of_level"
            | "product_definition_template"
            | "parameter"
            | "level_type"
            | "level"
            | "shortName"
            | "typeOfLevel" => reader_identity.push((key.clone(), value.clone())),
            other => {
                return Err(FrameLoadError::UnknownIdentitySelector(other.to_owned()));
            }
        }
    }
    Ok((reader_identity, required_role, expected_layout))
}

fn layout_matches_expected(layout: &crate::frame::ArrayLayout, expected: &str) -> bool {
    match expected {
        "scalar" => matches!(layout, crate::frame::ArrayLayout::Scalar),
        "horizontal2_d" | "horizontal2d" | "Horizontal2D" => {
            matches!(layout, crate::frame::ArrayLayout::Horizontal2D { .. })
        }
        "full3_d" | "full3d" | "Full3D" => {
            matches!(layout, crate::frame::ArrayLayout::Full3D { .. })
        }
        "interface3_d" | "interface3d" | "Interface3D" => {
            matches!(layout, crate::frame::ArrayLayout::Interface3D { .. })
        }
        _ => false,
    }
}

fn layout_label(layout: &crate::frame::ArrayLayout) -> &'static str {
    match layout {
        crate::frame::ArrayLayout::Scalar => "scalar",
        crate::frame::ArrayLayout::Horizontal2D { .. } => "horizontal2_d",
        crate::frame::ArrayLayout::Full3D { .. } => "full3_d",
        crate::frame::ArrayLayout::Interface3D { .. } => "interface3_d",
    }
}

fn normalize_direct_field(
    decoded: DecodedField,
    source: &FieldSource,
    target_unit: &GraphUnit,
    temporal: &TemporalSemantics,
    valid_time: Timestamp,
) -> Result<FrameComputationField, FrameLoadError> {
    let declared_source = GraphUnit::parse(&source.unit)
        .map_err(|error| FrameLoadError::InvalidUnit(error.to_string()))?;
    if !same_unit_conversion(&decoded.source_unit, &declared_source) {
        return Err(FrameLoadError::SourceUnitMismatch {
            expected: source.unit.clone(),
            actual: decoded.source_unit.symbol().into(),
        });
    }
    let values = decoded
        .values
        .iter()
        .zip(decoded.valid.iter())
        .map(|(value, valid)| {
            if *valid {
                decoded
                    .source_unit
                    .convert_value_to(*value, target_unit)
                    .map_err(|error| FrameLoadError::InvalidUnit(error.to_string()))
            } else {
                Ok(0.0)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let field = FrameComputationField::new(
        Arc::from(values),
        decoded.valid,
        target_unit.clone(),
        decoded.layout,
        decoded.temporal,
    )?;
    apply_temporal_semantics(field, temporal, valid_time)
}

fn apply_temporal_semantics(
    mut field: FrameComputationField,
    semantics: &TemporalSemantics,
    valid_time: Timestamp,
) -> Result<FrameComputationField, FrameLoadError> {
    if field.temporal.valid_time() != valid_time {
        return Err(FrameLoadError::FieldTimeMismatch);
    }
    field.temporal = match semantics.kind {
        TemporalKind::Instantaneous => match field.temporal {
            TemporalSupport::Instantaneous { .. } => field.temporal,
            _ => return Err(FrameLoadError::TemporalSemanticsMismatch),
        },
        TemporalKind::Interval => match field.temporal {
            TemporalSupport::Interval { .. } => field.temporal,
            TemporalSupport::Instantaneous { .. } => {
                let seconds = semantics
                    .interval_seconds
                    .ok_or(FrameLoadError::MissingIntervalSemantics)?;
                TemporalSupport::Interval {
                    start: subtract_seconds(valid_time, seconds)?,
                    end: valid_time,
                }
            }
            TemporalSupport::Accumulation { .. } => {
                return Err(FrameLoadError::TemporalSemanticsMismatch);
            }
        },
        TemporalKind::Accumulation => match field.temporal {
            TemporalSupport::Accumulation { .. } => field.temporal,
            _ => return Err(FrameLoadError::TemporalSemanticsMismatch),
        },
    };
    Ok(field)
}

fn subtract_seconds(time: Timestamp, seconds: u64) -> Result<Timestamp, FrameLoadError> {
    let seconds = i64::try_from(seconds).map_err(|_| FrameLoadError::TimeOverflow)?;
    let value = time
        .seconds_since_unix_epoch()
        .checked_sub(seconds)
        .ok_or(FrameLoadError::TimeOverflow)?;
    Timestamp::new(value, time.nanosecond()).map_err(|_| FrameLoadError::TimeOverflow)
}

/// Attach frozen algorithm IDs for M3 NearSurface / lnsp derivations so provenance
/// is not only a generic `profile_frame_graph` label.
fn derived_transform_parameters(derived: &DerivedField) -> Vec<(String, String)> {
    let mut parameters = vec![
        ("node".into(), format!("derived:{}", derived.id)),
        ("expression".into(), derived.expression.clone()),
    ];
    let algorithm = match derived.id.as_str() {
        "surface_pressure" if derived.expression.contains("surface_pressure_from_log") => {
            Some(SURFACE_PRESSURE_FROM_LOG_ALGORITHM_ID)
        }
        "two_metre_specific_humidity"
            if derived
                .expression
                .contains("two_metre_specific_humidity_from_dewpoint") =>
        {
            Some(TWO_METRE_SPECIFIC_HUMIDITY_FROM_DEWPOINT_ALGORITHM_ID)
        }
        "latent_heat_flux"
            if derived
                .expression
                .contains("latent_heat_from_moisture_flux") =>
        {
            Some(LATENT_HEAT_FROM_MOISTURE_FLUX_ALGORITHM_ID)
        }
        "sensible_heat_flux" if derived.expression.trim_start().starts_with('-') => {
            Some(UPWARD_HEAT_FLUX_FROM_DOWNWARD_ALGORITHM_ID)
        }
        _ => None,
    };
    if let Some(algorithm) = algorithm {
        parameters.push(("algorithm".into(), algorithm.into()));
    }
    // Trace logarithmic surface-pressure source variable when present in the expression.
    if derived.expression.contains("log_surface_pressure")
        || derived.expression.contains("surface_pressure_from_log")
    {
        parameters.push(("source_variable".into(), "lnsp".into()));
        parameters.push(("source_field_id".into(), "log_surface_pressure".into()));
    }
    parameters
}

fn source_record(
    path: &Path,
    identity: &[(String, String)],
    field: &FieldKey,
    source: &FieldSource,
    target_unit: &GraphUnit,
    profile_sha256: &str,
) -> ProvenanceRecord {
    let identity = identity
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut transforms = Vec::new();
    if source.unit != target_unit.symbol() {
        transforms.push(TransformRecord {
            operation: "unit_conversion".into(),
            parameters: vec![
                ("from".into(), source.unit.clone()),
                ("to".into(), target_unit.symbol().into()),
            ],
        });
    }
    ProvenanceRecord {
        field: field.clone(),
        quality: source.quality,
        sources: vec![format!("{}#{identity}", path.display())],
        transforms,
        fallback_reason: (source.role == SourceRole::Fallback)
            .then(|| "Profile primary source was absent".into()),
        profile_sha256: profile_sha256.into(),
    }
}

fn frame_computation_sources(frame: &RawMetFrame) -> BTreeMap<FieldKey, FrameComputationField> {
    frame
        .fields()
        .iter()
        .map(|(key, field)| {
            (
                key.clone(),
                FrameComputationField {
                    values: field.values().clone(),
                    valid: field.validity().as_arc().clone(),
                    unit: field.unit().clone(),
                    layout: field.layout(),
                    temporal: field.temporal(),
                },
            )
        })
        .collect()
}

fn same_unit_conversion(left: &GraphUnit, right: &GraphUnit) -> bool {
    left.dimension() == right.dimension()
        && left.scale_to_si() == right.scale_to_si()
        && left.offset_to_si() == right.offset_to_si()
}

/// Logical-frame opening, normalization, derivation, or publication failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameLoadError {
    /// Active Profile hash differs from the logical-frame identity.
    ProfileIdentityMismatch,
    /// No capabilities requested any fields.
    NoRequestedFields,
    /// Profile does not provide one requested capability.
    MissingCapability(crate::field::Capability),
    /// A requested output has no graph producer.
    MissingGraphOutput(FieldKey),
    /// A graph node is absent.
    MissingGraphNode(GraphNodeId),
    /// A direct graph dependency has no source mapping.
    MissingDirectMapping(FieldKey),
    /// A requested derived output has no declaration.
    MissingDerivedField(FieldKey),
    /// A derived output belongs to a later execution stage.
    UnsupportedDerivedStage {
        /// Requested field.
        field: FieldKey,
        /// Required stage.
        stage: ExecutionStage,
    },
    /// Frame executor omitted a requested output.
    MissingComputedField(FieldKey),
    /// Local file magic is not a supported meteorological container.
    UnknownSourceFormat(PathBuf),
    /// No source alternative contained the requested field.
    MissingField,
    /// More than one file matched one exact source alternative.
    AmbiguousSource(FieldKey),
    /// Profile required a logical role that is absent from the frame descriptor.
    MissingRole {
        /// Target field.
        field: FieldKey,
        /// Required role.
        role: String,
    },
    /// Frame descriptor listed the same logical role more than once.
    DuplicateRole {
        /// Target field.
        field: FieldKey,
        /// Duplicate role.
        role: String,
    },
    /// Decoded array layout disagrees with the Profile identity contract.
    LayoutMismatch {
        /// Target field.
        field: FieldKey,
        /// Expected layout selector.
        expected: String,
        /// Actual decoded layout label.
        actual: String,
    },
    /// Profile identity contained a selector neither the loader nor reader understands.
    UnknownIdentitySelector(String),
    /// Indexed source files disagree on normalized grid geometry.
    GridMismatch,
    /// Indexed source files disagree on vertical topology.
    VerticalMismatch,
    /// No source index exposed queryable grid geometry.
    MissingGridGeometry,
    /// No source index exposed native vertical topology.
    MissingVerticalTopology,
    /// Profile source-unit declaration disagrees with decoded metadata.
    SourceUnitMismatch {
        /// Profile-declared unit.
        expected: String,
        /// Reader-reported unit.
        actual: String,
    },
    /// A unit expression or conversion failed.
    InvalidUnit(String),
    /// Decoded physical time differs from the logical frame.
    FieldTimeMismatch,
    /// Decoded support category disagrees with the Profile.
    TemporalSemanticsMismatch,
    /// Interval semantics omitted a fixed or encoded interval.
    MissingIntervalSemantics,
    /// Timestamp arithmetic overflowed.
    TimeOverflow,
    /// Reader inspection, indexing, or decoding failed.
    Decode(DecodeError),
    /// Frame-stage graph execution failed.
    Graph(FrameGraphExecutionError),
    /// Provenance table construction failed.
    Provenance(ProvenanceError),
    /// Raw-field or frame publication failed.
    Frame(FrameError),
}

impl From<DecodeError> for FrameLoadError {
    fn from(value: DecodeError) -> Self {
        Self::Decode(value)
    }
}

impl From<FrameGraphExecutionError> for FrameLoadError {
    fn from(value: FrameGraphExecutionError) -> Self {
        Self::Graph(value)
    }
}

impl From<ProvenanceError> for FrameLoadError {
    fn from(value: ProvenanceError) -> Self {
        Self::Provenance(value)
    }
}

impl From<FrameError> for FrameLoadError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use trajecta_case::model::meteorology::DomainId;
    use trajecta_case::resolver::sha256_hex;

    use super::*;
    use crate::field::{CanonicalField, Capability};
    use crate::io::grib::GribReader;
    use crate::io::inventory::LogicalFrameId;
    use crate::profile::document::{ProfileCatalog, ProfileName};

    fn real_era5_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap()
            .join("tools/flexctl/target/test-data/ecmwf-era5/flex_extract-7.1/EA18120100")
    }

    #[test]
    fn real_era5_transport_capability_publishes_one_complete_frame() {
        let fixture = real_era5_fixture();
        if !fixture.is_file() {
            return;
        }
        let profiles = ProfileCatalog::load(&[]).unwrap();
        let profile = profiles
            .get(&ProfileName("era5-flex-extract-hybrid-v0".into()))
            .unwrap();
        let reader = GribReader::new(MeteorologyReaderBackend::Rust);
        let source = reader.inspect(&fixture).unwrap();
        let valid_time = source.valid_times[0];
        let domain = DomainId("era5-test".into());
        let descriptor = FrameDescriptor {
            id: LogicalFrameId {
                domain: domain.clone(),
                valid_time,
                profile_sha256: profile.sha256.clone(),
                content_sha256: sha256_hex(b"EA18120100"),
            },
            files: BTreeMap::from([("analysis".into(), fixture)]),
            grid: source.grid.unwrap(),
            vertical: source.vertical.unwrap(),
        };
        let mut capabilities = CapabilitySet::new();
        capabilities.insert(Capability::Transport);

        let frame = FrameLoader::load(FrameLoadRequest {
            descriptor: &descriptor,
            profile,
            required_capabilities: capabilities,
            backend: MeteorologyReaderBackend::Rust,
            previous_frame: None,
        })
        .unwrap();

        assert_eq!(frame.fields().len(), 7);
        for field in [
            CanonicalField::EastwardWind,
            CanonicalField::NorthwardWind,
            CanonicalField::HybridVerticalVelocity,
            CanonicalField::AirTemperature,
            CanonicalField::SpecificHumidity,
        ] {
            assert_eq!(
                frame
                    .fields()
                    .get(&FieldKey::Canonical(field))
                    .unwrap()
                    .layout(),
                crate::frame::ArrayLayout::Full3D {
                    levels: 8,
                    ny: 6,
                    nx: 6,
                }
            );
        }
        for field in [
            CanonicalField::SurfacePressure,
            CanonicalField::SurfaceGeopotential,
        ] {
            assert_eq!(
                frame
                    .fields()
                    .get(&FieldKey::Canonical(field))
                    .unwrap()
                    .layout(),
                crate::frame::ArrayLayout::Horizontal2D { ny: 6, nx: 6 }
            );
        }
        let VerticalTopology::HybridPressure(topology) = &frame.metadata().vertical else {
            panic!("expected hybrid topology");
        };
        assert_eq!(topology.coefficients.a_half_pa.len(), 138);
        assert_eq!(
            topology.active_full_levels.as_ref(),
            &[130, 131, 132, 133, 134, 135, 136, 137]
        );
    }

    #[test]
    fn real_era5_day_loads_all_eight_three_hourly_frames() {
        let directory = real_era5_fixture().parent().unwrap().to_path_buf();
        if !directory.is_dir() {
            return;
        }
        let profiles = ProfileCatalog::load(&[]).unwrap();
        let profile = profiles
            .get(&ProfileName("era5-flex-extract-hybrid-v0".into()))
            .unwrap();
        let reader = GribReader::new(MeteorologyReaderBackend::Rust);
        let domain = DomainId("era5-day".into());
        let mut capabilities = CapabilitySet::new();
        capabilities.insert(Capability::Transport);
        let mut expected_grid = None;
        let mut expected_vertical = None;

        for (index, hour) in (0_u8..=21).step_by(3).enumerate() {
            let fixture = directory.join(format!("EA181201{hour:02}"));
            let source = reader.inspect(&fixture).unwrap();
            assert_eq!(
                source.valid_times,
                vec![Timestamp::new(1_543_622_400 + (index as i64) * 10_800, 0).unwrap()]
            );
            if let Some(grid) = &expected_grid {
                assert_eq!(source.grid.as_ref(), Some(grid));
            } else {
                expected_grid = source.grid.clone();
            }
            if let Some(vertical) = &expected_vertical {
                assert_eq!(source.vertical.as_ref(), Some(vertical));
            } else {
                expected_vertical = source.vertical.clone();
            }
            let descriptor = FrameDescriptor {
                id: LogicalFrameId {
                    domain: domain.clone(),
                    valid_time: source.valid_times[0],
                    profile_sha256: profile.sha256.clone(),
                    content_sha256: sha256_hex(fixture.to_string_lossy().as_bytes()),
                },
                files: BTreeMap::from([("analysis".into(), fixture)]),
                grid: source.grid.unwrap(),
                vertical: source.vertical.unwrap(),
            };
            let frame = FrameLoader::load(FrameLoadRequest {
                descriptor: &descriptor,
                profile,
                required_capabilities: capabilities,
                backend: MeteorologyReaderBackend::Rust,
                previous_frame: None,
            })
            .unwrap();
            assert_eq!(frame.metadata().valid_time, descriptor.id.valid_time);
            assert_eq!(frame.fields().len(), 7);
            assert_eq!(frame.resident_bytes(), 13_608);
        }
    }

    #[cfg(feature = "native-eccodes")]
    #[test]
    fn real_era5_native_frame_matches_rust_frame() {
        let fixture = real_era5_fixture();
        if !fixture.is_file() {
            return;
        }
        let profiles = ProfileCatalog::load(&[]).unwrap();
        let profile = profiles
            .get(&ProfileName("era5-flex-extract-hybrid-v0".into()))
            .unwrap();
        let reader = GribReader::new(MeteorologyReaderBackend::Rust);
        let source = reader.inspect(&fixture).unwrap();
        let domain = DomainId("era5-native-diff".into());
        let descriptor = FrameDescriptor {
            id: LogicalFrameId {
                domain: domain.clone(),
                valid_time: source.valid_times[0],
                profile_sha256: profile.sha256.clone(),
                content_sha256: sha256_hex(b"EA18120100-native-diff"),
            },
            files: BTreeMap::from([("analysis".into(), fixture)]),
            grid: source.grid.unwrap(),
            vertical: source.vertical.unwrap(),
        };
        let mut capabilities = CapabilitySet::new();
        capabilities.insert(Capability::Transport);
        let load = |backend| {
            FrameLoader::load(FrameLoadRequest {
                descriptor: &descriptor,
                profile,
                required_capabilities: capabilities,
                backend,
                previous_frame: None,
            })
            .unwrap()
        };
        let rust = load(MeteorologyReaderBackend::Rust);
        let native = load(MeteorologyReaderBackend::Native);
        assert_eq!(native.metadata(), rust.metadata());
        for (field, rust_value) in rust.fields().iter() {
            let native_value = native.fields().get(field).unwrap();
            assert_eq!(native_value.layout(), rust_value.layout());
            assert_eq!(native_value.validity(), rust_value.validity());
            let max_difference = native_value
                .values()
                .iter()
                .zip(rust_value.values().iter())
                .map(|(native, rust)| (native - rust).abs())
                .fold(0.0_f64, f64::max);
            assert!(max_difference <= 1.0e-12, "{field:?}: {max_difference}");
        }
    }
}
