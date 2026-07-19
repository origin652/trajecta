//! # Contract: GRIB1/2 reader
//!
//! GRIB decoding preserves exact parameter-table identity, scan direction,
//! bitmap validity, and the complete ECMWF hybrid PV coefficient array. It
//! distinguishes data-layer subsets from the full vertical definition. Native
//! and Rust decoders correlate logical fields by `(physical byte offset,
//! field_index_in_message)`, never by decoder enumeration order.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "native-eccodes")]
use std::sync::{Mutex, mpsc};
#[cfg(feature = "native-eccodes")]
use std::thread::{self, JoinHandle};

use grib_reader::{GribFile, GridDefinition, ReferenceTime};
use sha2::{Digest, Sha256};
use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::lockfile::{GridSignature, VerticalSignature};
use trajecta_case::model::time::Timestamp;

use crate::frame::{ArrayLayout, TemporalSupport};
use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, MetReader, SourceFormat, SourceGridGeometry,
    SourceIndex, SourceMetadata,
};
use crate::profile::graph::GraphUnit;
use crate::vertical::{HybridCoefficients, HybridPressureTopology, VerticalTopology};

#[cfg(all(feature = "native-eccodes", windows))]
static NATIVE_ECCODES_CALL_LOCK: Mutex<()> = Mutex::new(());

/// Exact GRIB parameter-table identity plus the ecCodes-compatible parameter ID
/// where that ID is standardized for the supported ECMWF fields.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GribParameterIdentity {
    /// GRIB2 discipline, absent in GRIB1.
    pub discipline: Option<u8>,
    /// GRIB2 parameter category, absent in GRIB1.
    pub category: Option<u8>,
    /// Edition-native parameter number.
    pub number: u8,
    /// GRIB1 parameter-table version, absent in GRIB2.
    pub table_version: Option<u8>,
    /// Widely used ECMWF/ecCodes parameter ID when exactly known.
    pub param_id: Option<u32>,
}

/// Exact GRIB parameter, layer, and generating-process identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GribFieldIdentity {
    /// GRIB edition.
    pub edition: u8,
    /// Originating centre.
    pub centre: u16,
    /// Originating sub-centre.
    pub sub_centre: u16,
    /// Generating-process identifier when encoded by the product template.
    pub generating_process: Option<u16>,
    /// GRIB2 product-definition template number when present.
    pub product_definition_template: Option<u16>,
    /// Exact edition-native parameter identity.
    pub parameter: GribParameterIdentity,
    /// Numeric GRIB level-type code.
    pub level_type_code: u8,
    /// Stable normalized level-type name used by Profiles.
    pub level_type: String,
    /// Exact integral level value or one-based model-level index.
    pub level: i32,
}

/// One indexed GRIB message location and decode-relevant metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GribIndexEntry {
    /// Logical field index exposed by the decoder.
    pub message_index: usize,
    /// Zero-based logical-field index inside the containing physical message.
    pub field_index_in_message: usize,
    /// Exact field identity.
    pub identity: GribFieldIdentity,
    /// Physical validity time.
    pub valid_time: Timestamp,
    /// Byte offset of the containing message.
    pub offset: u64,
    /// Encoded containing-message length.
    pub length: u64,
    /// GRIB scan-mode octet.
    pub scanning_mode: u8,
    /// Whether an explicit or predefined source bitmap is active.
    pub has_bitmap: bool,
    /// Parsed physical source-unit symbol when known to the active registry.
    pub source_unit: Option<String>,
}

/// Immutable GRIB message index retaining one opened decoder handle.
pub struct GribFileIndex {
    /// Canonical source path used to construct the index.
    pub path: PathBuf,
    /// File-wide GRIB edition.
    pub format: SourceFormat,
    /// Indexed logical fields in source order.
    pub entries: Vec<GribIndexEntry>,
    /// Shared regular latitude-longitude geometry.
    pub grid: GribGridDecoder,
    /// Complete hybrid coefficients, when present.
    pub hybrid_pv: Option<HybridPvDecoder>,
    /// Sorted one-based hybrid full levels carried by data messages.
    pub active_full_levels: Vec<u16>,
    source_grid: SourceGridGeometry,
    source_vertical: Option<VerticalTopology>,
    source_grid_signature: GridSignature,
    source_vertical_signature: Option<VerticalSignature>,
    backend: MeteorologyReaderBackend,
    source: Arc<GribFile>,
    #[cfg(feature = "native-eccodes")]
    native_decoder: Option<NativeDecoder>,
}

impl fmt::Debug for GribFileIndex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GribFileIndex")
            .field("path", &self.path)
            .field("format", &self.format)
            .field("entries", &self.entries)
            .field("grid", &self.grid)
            .field("hybrid_pv", &self.hybrid_pv)
            .field("active_full_levels", &self.active_full_levels)
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl SourceIndex for GribFileIndex {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn grid_geometry(&self) -> Option<&SourceGridGeometry> {
        Some(&self.source_grid)
    }

    fn vertical_topology(&self) -> Option<&VerticalTopology> {
        self.source_vertical.as_ref()
    }

    fn grid_signature(&self) -> Option<&GridSignature> {
        Some(&self.source_grid_signature)
    }

    fn vertical_signature(&self) -> Option<&VerticalSignature> {
        self.source_vertical_signature.as_ref()
    }
}

/// Decoded regular latitude-longitude grid definition.
#[derive(Clone, Debug, PartialEq)]
pub struct GribGridDecoder {
    /// Number of x/longitude points.
    pub nx: usize,
    /// Number of y/latitude points.
    pub ny: usize,
    /// First longitude in degrees east.
    pub first_longitude_degrees: f64,
    /// First latitude in degrees north.
    pub first_latitude_degrees: f64,
    /// Last longitude in degrees east.
    pub last_longitude_degrees: f64,
    /// Last latitude in degrees north.
    pub last_latitude_degrees: f64,
    /// Signed longitude increment in canonical row order.
    pub longitude_increment_degrees: f64,
    /// Signed latitude increment in canonical row order.
    pub latitude_increment_degrees: f64,
    /// Original GRIB scanning-mode octet.
    pub scanning_mode: u8,
}

/// Complete hybrid-interface A/B coefficients decoded from GRIB PV metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct HybridPvDecoder {
    /// Interface A coefficients in pascals.
    pub a_half_pa: Vec<f64>,
    /// Dimensionless interface B coefficients.
    pub b_half: Vec<f64>,
}

/// Explicit GRIB missing-value bitmap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GribBitmap {
    /// True where the decoded source value is valid.
    pub valid: Vec<bool>,
}

/// GRIB1/2 implementation of the format-neutral reader contract.
#[derive(Clone, Copy, Debug)]
pub struct GribReader {
    backend: MeteorologyReaderBackend,
}

impl GribReader {
    /// Creates a GRIB reader for one explicitly selected implementation.
    #[must_use]
    pub const fn new(backend: MeteorologyReaderBackend) -> Self {
        Self { backend }
    }

    /// Returns the exact selected backend.
    #[must_use]
    pub const fn backend(self) -> MeteorologyReaderBackend {
        self.backend
    }

    fn backend_index(&self, file: &Path) -> Result<GribFileIndex, DecodeError> {
        let mut index = build_rust_index(file)?;
        match self.backend {
            MeteorologyReaderBackend::Rust => {}
            MeteorologyReaderBackend::Native => {
                #[cfg(feature = "native-eccodes")]
                {
                    // Identify logical fields by their physical-message byte offset plus
                    // their zero-based field index inside that message. CFSR contains
                    // multi-field messages (notably paired U/V fields), while either
                    // decoder may skip unrelated physical messages.
                    index.native_decoder = Some(NativeDecoder::start(&index.path, &index.entries)?);
                }
                #[cfg(not(feature = "native-eccodes"))]
                {
                    return Err(DecodeError::BackendUnavailable {
                        backend: MeteorologyReaderBackend::Native,
                        message: "compile trajecta-met with feature 'native-eccodes'".into(),
                    });
                }
            }
        }
        index.backend = self.backend;
        Ok(index)
    }
}

impl Default for GribReader {
    fn default() -> Self {
        Self::new(MeteorologyReaderBackend::Native)
    }
}

impl MetReader for GribReader {
    fn inspect(&self, file: &Path) -> Result<SourceMetadata, DecodeError> {
        let index = self.backend_index(file)?;
        metadata_from_index(&index)
    }

    fn build_index(&self, file: &Path) -> Result<Box<dyn SourceIndex>, DecodeError> {
        Ok(Box::new(self.backend_index(file)?))
    }

    fn decode(
        &self,
        file: &Path,
        index: &dyn SourceIndex,
        request: &DecodeRequest,
    ) -> Result<DecodedField, DecodeError> {
        let index = index
            .as_any()
            .downcast_ref::<GribFileIndex>()
            .ok_or_else(|| {
                DecodeError::InvalidMetadata("GRIB reader received a foreign source index".into())
            })?;
        let requested_path = canonical_path(file)?;
        if requested_path != index.path {
            return Err(DecodeError::InvalidMetadata(
                "decode path differs from the indexed GRIB source".into(),
            ));
        }
        if index.backend != self.backend {
            return Err(DecodeError::InvalidMetadata(
                "GRIB reader backend differs from the source index backend".into(),
            ));
        }
        match self.backend {
            MeteorologyReaderBackend::Rust => decode_request_rust(index, request),
            MeteorologyReaderBackend::Native => decode_request_native(index, request),
        }
    }
}

fn build_rust_index(file: &Path) -> Result<GribFileIndex, DecodeError> {
    let path = canonical_path(file)?;
    let source = Arc::new(GribFile::open(&path).map_err(|error| map_grib_error(&path, error))?);
    let mut entries = Vec::with_capacity(source.message_count());
    let mut format = None;
    let mut grid = None;
    let mut hybrid_pv = None;
    let mut active_full_levels = BTreeSet::new();

    for message_index in 0..source.message_count() {
        let message = source
            .message(message_index)
            .map_err(|error| map_grib_error(&path, error))?;
        let message_format = match message.edition() {
            1 => SourceFormat::Grib1,
            2 => SourceFormat::Grib2,
            _ => return Err(DecodeError::UnsupportedFormat),
        };
        if format.is_none() {
            format = Some(message_format);
        }

        let message_grid = regular_grid(&message)?;
        if grid
            .as_ref()
            .is_some_and(|existing: &GribGridDecoder| existing != &message_grid)
        {
            return Err(DecodeError::InvalidMetadata(
                "GRIB fields do not share one regular latitude-longitude grid".into(),
            ));
        }
        grid = Some(message_grid.clone());

        if let Some(pv) = extract_hybrid_pv(&message)? {
            if hybrid_pv
                .as_ref()
                .is_some_and(|existing: &HybridPvDecoder| existing != &pv)
            {
                return Err(DecodeError::InvalidMetadata(
                    "GRIB messages contain inconsistent hybrid PV coefficients".into(),
                ));
            }
            hybrid_pv = Some(pv);
        }

        let Some(identity) = field_identity(&message)? else {
            // Skip sigma / fractional surfaces that are outside the v0 exact-level contract.
            continue;
        };
        // Hybrid model-level data without complete PV is not queryable in v0. Skip those
        // messages so pressure-level products (CFSR pgbl) can still be indexed.
        if identity.level_type == "hybrid" || identity.level_type == "hybrid_interface" {
            if hybrid_pv.is_none() {
                continue;
            }
            if identity.level_type == "hybrid" {
                let level = u16::try_from(identity.level).map_err(|_| {
                    DecodeError::InvalidMetadata("hybrid model level is outside u16".into())
                })?;
                active_full_levels.insert(level);
            }
        }
        let valid_time =
            reference_time_to_timestamp(message.valid_time().unwrap_or(*message.reference_time()))?;
        let source_unit = source_unit(&identity.parameter, identity.centre).map(str::to_owned);
        entries.push(GribIndexEntry {
            message_index,
            field_index_in_message: message.metadata().field_index_in_message,
            scanning_mode: message_grid.scanning_mode,
            has_bitmap: message_has_bitmap(&message)?,
            identity,
            valid_time,
            offset: message.metadata().message_offset,
            length: message.metadata().message_length,
            source_unit,
        });
    }

    validate_active_hybrid_levels(&active_full_levels, hybrid_pv.as_ref())?;
    let grid = grid.ok_or_else(|| DecodeError::InvalidMetadata("GRIB grid is absent".into()))?;
    let active_full_levels = active_full_levels.into_iter().collect::<Vec<_>>();
    let source_grid = SourceGridGeometry {
        longitude_origin_degrees: grid.first_longitude_degrees,
        latitude_origin_degrees: grid.first_latitude_degrees,
        longitude_spacing_degrees: grid.longitude_increment_degrees,
        latitude_spacing_degrees: grid.latitude_increment_degrees,
        nx: grid.nx,
        ny: grid.ny,
        periodic_longitude: is_periodic_longitude(&grid),
    };
    let pressure_levels = collect_pressure_levels_pa(&entries)?;
    let source_vertical = if let Some(pv) = hybrid_pv.as_ref() {
        Some(VerticalTopology::HybridPressure(HybridPressureTopology {
            coefficients: HybridCoefficients {
                a_half_pa: Arc::from(pv.a_half_pa.clone()),
                b_half: Arc::from(pv.b_half.clone()),
            },
            active_full_levels: Arc::from(active_full_levels.clone()),
        }))
    } else if let Some(levels) = pressure_levels.as_ref() {
        Some(VerticalTopology::PressureLevels(
            crate::vertical::PressureLevels::new(Arc::from(levels.clone()))
                .map_err(|error| DecodeError::InvalidMetadata(error.to_string()))?,
        ))
    } else {
        None
    };
    let source_grid_signature = GridSignature {
        nx: grid.nx,
        ny: grid.ny,
        periodic_longitude: source_grid.periodic_longitude,
        sha256: grid_signature(&grid),
    };
    let source_vertical_signature = if let Some(pv) = hybrid_pv.as_ref() {
        Some(VerticalSignature::HybridPressure {
            full_level_count: pv.a_half_pa.len() - 1,
            coefficients_sha256: hybrid_signature(pv),
        })
    } else {
        pressure_levels
            .as_ref()
            .map(|levels| VerticalSignature::PressureLevels {
                level_count: levels.len(),
                levels_sha256: pressure_levels_signature(levels),
            })
    };
    Ok(GribFileIndex {
        path,
        format: format.ok_or_else(|| DecodeError::InvalidMetadata("empty GRIB index".into()))?,
        entries,
        grid,
        hybrid_pv,
        active_full_levels,
        source_grid,
        source_vertical,
        source_grid_signature,
        source_vertical_signature,
        backend: MeteorologyReaderBackend::Rust,
        source,
        #[cfg(feature = "native-eccodes")]
        native_decoder: None,
    })
}

fn metadata_from_index(index: &GribFileIndex) -> Result<SourceMetadata, DecodeError> {
    let centres = index
        .entries
        .iter()
        .map(|entry| entry.identity.centre)
        .collect::<BTreeSet<_>>();
    let sub_centres = index
        .entries
        .iter()
        .map(|entry| entry.identity.sub_centre)
        .collect::<BTreeSet<_>>();
    if centres.len() != 1 || sub_centres.len() != 1 {
        return Err(DecodeError::InvalidMetadata(
            "GRIB source contains multiple originating centres".into(),
        ));
    }
    let mut attributes = BTreeMap::from([
        (
            "centre".into(),
            centres.first().copied().unwrap_or_default().to_string(),
        ),
        (
            "sub_centre".into(),
            sub_centres.first().copied().unwrap_or_default().to_string(),
        ),
        (
            "edition".into(),
            match index.format {
                SourceFormat::Grib1 => "1",
                SourceFormat::Grib2 => "2",
                SourceFormat::NetCdf3 | SourceFormat::NetCdf4 => unreachable!(),
            }
            .into(),
        ),
    ]);
    let editions = index
        .entries
        .iter()
        .map(|entry| entry.identity.edition.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    attributes.insert("message_editions".into(), editions);
    let processes = index
        .entries
        .iter()
        .filter_map(|entry| entry.identity.generating_process)
        .collect::<BTreeSet<_>>();
    if processes.len() == 1 {
        attributes.insert(
            "generating_process".into(),
            processes.first().copied().unwrap_or_default().to_string(),
        );
    }
    if is_era5_flex_extract_hybrid(index) {
        attributes.insert("dataset_family".into(), "era5_flex_extract_hybrid".into());
    } else if is_cfsr_pgbl_pressure(index) {
        attributes.insert("dataset_family".into(), "cfsr_pgbl_pressure".into());
    }

    let mut dimensions = BTreeMap::from([("x".into(), index.grid.nx), ("y".into(), index.grid.ny)]);
    if let Some(pv) = index.hybrid_pv.as_ref() {
        dimensions.insert("hybrid_interface".into(), pv.a_half_pa.len());
        dimensions.insert(
            "active_hybrid_full_level".into(),
            index.active_full_levels.len(),
        );
    }
    if let Some(VerticalTopology::PressureLevels(levels)) = index.source_vertical.as_ref() {
        dimensions.insert("pressure_level".into(), levels.pressure_pa.len());
    }
    let all_valid_times = index
        .entries
        .iter()
        .map(|entry| entry.valid_time)
        .collect::<BTreeSet<_>>();
    let layered_valid_times = index
        .entries
        .iter()
        .filter(|entry| is_layered_level_type(&entry.identity.level_type))
        .map(|entry| entry.valid_time)
        .collect::<BTreeSet<_>>();
    let valid_times = if layered_valid_times.is_empty() {
        all_valid_times
    } else {
        layered_valid_times
    }
    .into_iter()
    .collect();
    Ok(SourceMetadata {
        path: index.path.clone(),
        format: index.format,
        attributes,
        dimensions,
        valid_times,
        roles: vec!["analysis".into()],
        grid: Some(index.source_grid_signature.clone()),
        vertical: index.source_vertical_signature.clone(),
    })
}

fn decode_request_rust(
    index: &GribFileIndex,
    request: &DecodeRequest,
) -> Result<DecodedField, DecodeError> {
    decode_request_with(index, request, |entry| {
        let message = index
            .source
            .message(entry.message_index)
            .map_err(|error| map_grib_error(&index.path, error))?;
        message
            .read_flat_data_as_f64()
            .map_err(|error| map_grib_error(&index.path, error))
    })
}

#[cfg(feature = "native-eccodes")]
fn decode_request_native(
    index: &GribFileIndex,
    request: &DecodeRequest,
) -> Result<DecodedField, DecodeError> {
    let decoder = index
        .native_decoder
        .as_ref()
        .ok_or_else(|| DecodeError::BackendUnavailable {
            backend: MeteorologyReaderBackend::Native,
            message: "native ecCodes worker is absent from the source index".into(),
        })?;
    decode_request_with(index, request, |entry| {
        let mut decoded = decoder.decode(NativeFieldKey::from(entry))?;
        let message = index
            .source
            .message(entry.message_index)
            .map_err(|error| map_grib_error(&index.path, error))?;
        message
            .grid_definition()
            .reorder_for_ndarray_in_place(&mut decoded)
            .map_err(|error| map_grib_error(&index.path, error))?;
        Ok(decoded)
    })
}

#[cfg(not(feature = "native-eccodes"))]
fn decode_request_native(
    _index: &GribFileIndex,
    _request: &DecodeRequest,
) -> Result<DecodedField, DecodeError> {
    Err(DecodeError::BackendUnavailable {
        backend: MeteorologyReaderBackend::Native,
        message: "compile trajecta-met with feature 'native-eccodes'".into(),
    })
}

fn decode_request_with(
    index: &GribFileIndex,
    request: &DecodeRequest,
    mut decode_message: impl FnMut(&GribIndexEntry) -> Result<Vec<f64>, DecodeError>,
) -> Result<DecodedField, DecodeError> {
    let selected = index
        .entries
        .iter()
        .filter(|entry| {
            request
                .valid_time
                .is_none_or(|time| entry.valid_time == time)
                && request
                    .source_identity
                    .iter()
                    .all(|(key, value)| identity_value(entry, key).as_deref() == Some(value))
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(DecodeError::MissingField);
    }
    let first = selected[0];
    if selected.iter().any(|entry| {
        entry.identity.parameter != first.identity.parameter
            || entry.identity.level_type != first.identity.level_type
            || entry.valid_time != first.valid_time
            || entry.source_unit != first.source_unit
    }) {
        return Err(DecodeError::AmbiguousField);
    }

    let mut by_level = BTreeMap::new();
    for entry in selected {
        if by_level.insert(entry.identity.level, entry).is_some() {
            return Err(DecodeError::AmbiguousField);
        }
    }
    if matches!(
        first.identity.level_type.as_str(),
        "hybrid" | "hybrid_interface"
    ) && by_level
        .keys()
        .copied()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|levels| levels[1] != levels[0].saturating_add(1))
    {
        return Err(DecodeError::InvalidMetadata(
            "selected GRIB model levels are not contiguous".into(),
        ));
    }

    // Order layered values to match the indexed vertical topology exactly.
    let ordered_entries = ordered_layered_entries(index, first, &by_level)?;

    let horizontal_count = index
        .grid
        .nx
        .checked_mul(index.grid.ny)
        .ok_or_else(|| DecodeError::InvalidMetadata("GRIB grid size overflow".into()))?;
    let capacity = horizontal_count
        .checked_mul(ordered_entries.len())
        .ok_or_else(|| DecodeError::InvalidMetadata("decoded field size overflow".into()))?;
    let mut values = Vec::with_capacity(capacity);
    let mut valid = Vec::with_capacity(capacity);
    let level_count = ordered_entries.len();
    for entry in ordered_entries {
        let decoded = decode_message(entry)?;
        if decoded.len() != horizontal_count {
            return Err(DecodeError::InvalidMetadata(
                "decoded GRIB field length disagrees with indexed grid".into(),
            ));
        }
        for value in decoded {
            if value.is_nan() {
                values.push(0.0);
                valid.push(false);
            } else if value.is_finite() {
                values.push(value);
                valid.push(true);
            } else {
                return Err(DecodeError::InvalidMetadata(
                    "decoded GRIB field contains infinity".into(),
                ));
            }
        }
    }
    let layout = if is_layered_level_type(&first.identity.level_type) {
        ArrayLayout::Full3D {
            levels: level_count,
            ny: index.grid.ny,
            nx: index.grid.nx,
        }
    } else if level_count == 1 {
        ArrayLayout::Horizontal2D {
            ny: index.grid.ny,
            nx: index.grid.nx,
        }
    } else {
        return Err(DecodeError::AmbiguousField);
    };
    let mut source_identity = request.source_identity.clone();
    source_identity.sort();
    let source_unit = first.source_unit.as_deref().ok_or_else(|| {
        DecodeError::InvalidMetadata(format!(
            "unknown physical unit for requested GRIB parameter {:?}",
            first.identity.parameter
        ))
    })?;
    Ok(DecodedField {
        values: Arc::from(values),
        valid: Arc::from(valid),
        source_unit: GraphUnit::parse(source_unit).map_err(|error| {
            DecodeError::InvalidMetadata(format!("invalid indexed source unit: {error}"))
        })?,
        layout,
        temporal: TemporalSupport::Instantaneous {
            valid_time: first.valid_time,
        },
        source_identity,
    })
}

fn regular_grid(message: &grib_reader::Message<'_>) -> Result<GribGridDecoder, DecodeError> {
    let GridDefinition::LatLon(grid) = message.grid_definition() else {
        return Err(DecodeError::InvalidMetadata(
            "v0 GRIB reader requires a regular latitude-longitude grid".into(),
        ));
    };
    let nx = usize::try_from(grid.ni)
        .map_err(|_| DecodeError::InvalidMetadata("GRIB longitude count overflow".into()))?;
    let ny = usize::try_from(grid.nj)
        .map_err(|_| DecodeError::InvalidMetadata("GRIB latitude count overflow".into()))?;
    if nx == 0 || ny == 0 {
        return Err(DecodeError::InvalidMetadata(
            "GRIB grid dimensions must be positive".into(),
        ));
    }
    let first_longitude_degrees = f64::from(grid.lon_first) / 1_000_000.0;
    let first_latitude_degrees = f64::from(grid.lat_first) / 1_000_000.0;
    let last_longitude_degrees = f64::from(grid.lon_last) / 1_000_000.0;
    let last_latitude_degrees = f64::from(grid.lat_last) / 1_000_000.0;
    let longitude_increment_degrees = signed_increment(
        first_longitude_degrees,
        last_longitude_degrees,
        nx,
        f64::from(grid.di) / 1_000_000.0,
    );
    let latitude_increment_degrees = signed_increment(
        first_latitude_degrees,
        last_latitude_degrees,
        ny,
        f64::from(grid.dj) / 1_000_000.0,
    );
    Ok(GribGridDecoder {
        nx,
        ny,
        first_longitude_degrees,
        first_latitude_degrees,
        last_longitude_degrees,
        last_latitude_degrees,
        longitude_increment_degrees,
        latitude_increment_degrees,
        scanning_mode: grid.scanning_mode,
    })
}

fn signed_increment(first: f64, last: f64, count: usize, encoded_magnitude: f64) -> f64 {
    if count <= 1 {
        return encoded_magnitude;
    }
    let direct = (last - first) / (count - 1) as f64;
    if direct.abs() > 0.0 {
        direct
    } else {
        encoded_magnitude
    }
}

fn field_identity(
    message: &grib_reader::Message<'_>,
) -> Result<Option<GribFieldIdentity>, DecodeError> {
    let parameter = message.parameter();
    let (generating_process, product_definition_template, level_type_code, level) =
        if let Some(product) = message.product_definition() {
            let surface = product.first_surface().ok_or_else(|| {
                DecodeError::InvalidMetadata("GRIB2 field has no first fixed surface".into())
            })?;
            let level_type = normalized_level_type(surface.surface_type);
            let level = match exact_integral_level(surface.scaled_value_f64()) {
                Ok(level) => level,
                // Fractional sigma/eta/height surfaces are outside the v0 exact-level
                // contract. Only hybrid and isobaric layers must remain integral.
                Err(error) if matches!(level_type, "hybrid" | "hybrid_interface" | "isobaric") => {
                    return Err(error);
                }
                Err(_) => return Ok(None),
            };
            (
                product.generating_process().map(u16::from),
                Some(product.template_number()),
                surface.surface_type,
                level,
            )
        } else if let Some(product) = message.grib1_product_definition() {
            (
                Some(u16::from(product.generating_process_id)),
                None,
                product.level_type,
                i32::from(product.level_value),
            )
        } else {
            return Err(DecodeError::InvalidMetadata(
                "GRIB product definition is absent".into(),
            ));
        };
    let parameter = GribParameterIdentity {
        discipline: parameter.discipline,
        category: parameter.category,
        number: parameter.number,
        table_version: parameter.table_version,
        param_id: normalized_param_id(
            message.edition(),
            message.center_id(),
            parameter.discipline,
            parameter.category,
            parameter.table_version,
            parameter.number,
        ),
    };
    Ok(Some(GribFieldIdentity {
        edition: message.edition(),
        centre: message.center_id(),
        sub_centre: message.subcenter_id(),
        generating_process,
        product_definition_template,
        parameter,
        level_type_code,
        level_type: normalized_level_type(level_type_code).into(),
        level,
    }))
}

fn exact_integral_level(value: f64) -> Result<i32, DecodeError> {
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(DecodeError::InvalidMetadata(format!(
            "non-finite or out-of-range GRIB level {value} is outside the v0 exact-level contract"
        )));
    }
    let rounded = value.round();
    // Tolerate scale-factor float residue while still rejecting true fractional surfaces.
    if (value - rounded).abs() > 1.0e-4 {
        return Err(DecodeError::InvalidMetadata(format!(
            "non-integral GRIB level {value} is outside the v0 exact-level contract"
        )));
    }
    Ok(rounded as i32)
}

fn extract_hybrid_pv(
    message: &grib_reader::Message<'_>,
) -> Result<Option<HybridPvDecoder>, DecodeError> {
    if message.edition() != 2 {
        return Ok(None);
    }
    let fields = grib_reader::sections::index_fields(message.raw_bytes())
        .map_err(|error| DecodeError::InvalidMetadata(error.to_string()))?;
    let field = fields
        .get(message.metadata().field_index_in_message)
        .ok_or_else(|| {
            DecodeError::InvalidMetadata("GRIB2 field-section index is absent".into())
        })?;
    let section = section_bytes(message.raw_bytes(), field.product)?;
    if section.len() < 9 {
        return Err(DecodeError::InvalidMetadata(
            "GRIB2 product section is truncated".into(),
        ));
    }
    let coordinate_count = usize::from(u16::from_be_bytes([section[5], section[6]]));
    if coordinate_count == 0 {
        return Ok(None);
    }
    if coordinate_count % 2 != 0 {
        return Err(DecodeError::InvalidMetadata(
            "hybrid PV coordinate count must be even".into(),
        ));
    }
    let coordinate_bytes = coordinate_count
        .checked_mul(4)
        .ok_or_else(|| DecodeError::InvalidMetadata("hybrid PV byte size overflow".into()))?;
    let start = section.len().checked_sub(coordinate_bytes).ok_or_else(|| {
        DecodeError::InvalidMetadata("hybrid PV exceeds product-section length".into())
    })?;
    if start < 9 {
        return Err(DecodeError::InvalidMetadata(
            "hybrid PV overlaps the product template".into(),
        ));
    }
    let coefficients = section[start..]
        .chunks_exact(4)
        .map(|bytes| f64::from(f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
        .collect::<Vec<_>>();
    if coefficients.iter().any(|value| !value.is_finite()) {
        return Err(DecodeError::InvalidMetadata(
            "hybrid PV contains a non-finite coefficient".into(),
        ));
    }
    let split = coefficients.len() / 2;
    Ok(Some(HybridPvDecoder {
        a_half_pa: coefficients[..split].to_vec(),
        b_half: coefficients[split..].to_vec(),
    }))
}

fn section_bytes(
    message: &[u8],
    section: grib_reader::sections::SectionRef,
) -> Result<&[u8], DecodeError> {
    let end = section
        .offset
        .checked_add(section.length)
        .ok_or_else(|| DecodeError::InvalidMetadata("GRIB section byte range overflow".into()))?;
    message
        .get(section.offset..end)
        .ok_or_else(|| DecodeError::InvalidMetadata("GRIB section byte range is truncated".into()))
}

fn message_has_bitmap(message: &grib_reader::Message<'_>) -> Result<bool, DecodeError> {
    if let Some(product) = message.grib1_product_definition() {
        return Ok(product.has_bitmap);
    }
    let fields = grib_reader::sections::index_fields(message.raw_bytes())
        .map_err(|error| DecodeError::InvalidMetadata(error.to_string()))?;
    let field = fields
        .get(message.metadata().field_index_in_message)
        .ok_or_else(|| {
            DecodeError::InvalidMetadata("GRIB2 field-section index is absent".into())
        })?;
    let Some(bitmap) = field.bitmap else {
        return Ok(false);
    };
    let section = section_bytes(message.raw_bytes(), bitmap)?;
    Ok(section.get(5).is_some_and(|indicator| *indicator != 255))
}

fn validate_active_hybrid_levels(
    levels: &BTreeSet<u16>,
    pv: Option<&HybridPvDecoder>,
) -> Result<(), DecodeError> {
    if levels.is_empty() {
        return Ok(());
    }
    let pv = pv.ok_or_else(|| {
        DecodeError::InvalidMetadata("hybrid fields are present but complete PV is absent".into())
    })?;
    if pv.a_half_pa.len() < 2 || pv.a_half_pa.len() != pv.b_half.len() {
        return Err(DecodeError::InvalidMetadata(
            "hybrid PV A/B array lengths are invalid".into(),
        ));
    }
    let full_level_count = pv.a_half_pa.len() - 1;
    if levels
        .iter()
        .any(|level| *level == 0 || usize::from(*level) > full_level_count)
        || levels
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair[1] != pair[0].saturating_add(1))
    {
        return Err(DecodeError::InvalidMetadata(
            "active hybrid levels must be a contiguous subset of complete PV".into(),
        ));
    }
    Ok(())
}

fn identity_value(entry: &GribIndexEntry, key: &str) -> Option<String> {
    let identity = &entry.identity;
    match key {
        "edition" => Some(identity.edition.to_string()),
        "centre" => Some(identity.centre.to_string()),
        "sub_centre" => Some(identity.sub_centre.to_string()),
        "generating_process" => identity.generating_process.map(|value| value.to_string()),
        "product_definition_template" => identity
            .product_definition_template
            .map(|value| value.to_string()),
        "discipline" => identity.parameter.discipline.map(|value| value.to_string()),
        "parameter_category" => identity.parameter.category.map(|value| value.to_string()),
        "parameter_number" => Some(identity.parameter.number.to_string()),
        "table_version" => identity
            .parameter
            .table_version
            .map(|value| value.to_string()),
        "param_id" => identity.parameter.param_id.map(|value| value.to_string()),
        "type_of_level" => Some(identity.level_type.clone()),
        "level_type_code" => Some(identity.level_type_code.to_string()),
        "level" => Some(identity.level.to_string()),
        _ => None,
    }
}

fn normalized_level_type(code: u8) -> &'static str {
    match code {
        1 => "surface",
        100 => "isobaric",
        101 => "mean_sea_level",
        103 => "height_above_ground",
        104 => "sigma",
        105 => "hybrid",
        107 => "eta",
        109 => "hybrid_interface",
        _ => "unknown",
    }
}

fn normalized_param_id(
    edition: u8,
    centre: u16,
    discipline: Option<u8>,
    category: Option<u8>,
    table_version: Option<u8>,
    number: u8,
) -> Option<u32> {
    if edition == 1 && centre == 98 {
        return Some(u32::from(number));
    }
    if edition != 2 {
        return None;
    }
    match (discipline?, category?, number) {
        (0, 3, 4) => Some(129), // geopotential
        (0, 3, 5) => Some(156), // geopotential height
        (0, 0, 0) => Some(130), // temperature
        (0, 2, 2) => Some(131), // u-wind
        (0, 2, 3) => Some(132), // v-wind
        (0, 2, 32) => Some(77), // hybrid vertical velocity (ECMWF)
        (0, 1, 0) => Some(133), // specific humidity
        (0, 3, 0) => Some(134), // surface pressure
        (0, 2, 8) => Some(135), // pressure vertical velocity
        (192, 201, 31) => Some(201_031),
        _ => table_version.map(|_| u32::from(number)),
    }
}

fn source_unit(parameter: &GribParameterIdentity, centre: u16) -> Option<&'static str> {
    match parameter.param_id {
        Some(129) => Some("m2/s2"),
        Some(130) => Some("K"),
        Some(131 | 132) => Some("m/s"),
        Some(77) => Some("s-1"),
        Some(133) => Some("1"),
        Some(134) => Some("Pa"),
        Some(135) => Some("Pa/s"),
        Some(156) => Some("m"), // geopotential height
        Some(201_031) => Some("1"),
        _ => match (parameter.discipline, parameter.category, parameter.number) {
            (Some(0), Some(0), 0) => Some("K"),
            // Sensible / latent heat net flux (W m-2 == kg s-3).
            // Sign convention is source-native; conversion to a global "upward
            // positive" convention is reserved for A adjudication.
            (Some(0), Some(0), 10 | 11) => Some("kg/s3"),
            (Some(0), Some(1), 0) => Some("1"),
            (Some(0), Some(2), 2 | 3) => Some("m/s"),
            (Some(0), Some(2), 8) => Some("Pa/s"),
            // UFLX/VFLX momentum fluxes (Pa / N m-2).
            (Some(0), Some(2), 17 | 18) => Some("Pa"),
            // WMO friction velocity.
            (Some(0), Some(2), 30) => Some("m/s"),
            (Some(0), Some(3), 0 | 1) => Some("Pa"),
            (Some(0), Some(3), 4) => Some("m2/s2"),
            (Some(0), Some(3), 5) => Some("m"),
            // Land-surface aerodynamic roughness length (SFCR / fsr).
            (Some(2), Some(0), 1) => Some("m"),
            // NCEP local-table parameters are centre-scoped (kwbc / centre 7).
            (Some(0), Some(2), 197) if centre == 7 => Some("m/s"), // FRICV
            (Some(0), Some(3), 196) if centre == 7 => Some("m"),   // HPBL
            _ => None,
        },
    }
}

fn is_layered_level_type(level_type: &str) -> bool {
    matches!(level_type, "hybrid" | "hybrid_interface" | "isobaric")
}

fn is_era5_flex_extract_hybrid(index: &GribFileIndex) -> bool {
    let params = index
        .entries
        .iter()
        .filter_map(|entry| entry.identity.parameter.param_id)
        .collect::<BTreeSet<_>>();
    index.format == SourceFormat::Grib2
        && index
            .entries
            .first()
            .is_some_and(|entry| entry.identity.centre == 98)
        && index.hybrid_pv.is_some()
        && !index.active_full_levels.is_empty()
        && [77, 129, 130, 131, 132, 133, 134]
            .into_iter()
            .all(|param| params.contains(&param))
}

fn is_cfsr_pgbl_pressure(index: &GribFileIndex) -> bool {
    if index.format != SourceFormat::Grib2 {
        return false;
    }
    if index
        .entries
        .first()
        .is_none_or(|entry| entry.identity.centre != 7)
    {
        return false;
    }
    let has_isobaric = index
        .entries
        .iter()
        .any(|entry| entry.identity.level_type == "isobaric");
    let params = index
        .entries
        .iter()
        .map(|entry| {
            (
                entry.identity.parameter.discipline,
                entry.identity.parameter.category,
                entry.identity.parameter.number,
            )
        })
        .collect::<BTreeSet<_>>();
    has_isobaric
        && params.contains(&(Some(0), Some(2), 2))
        && params.contains(&(Some(0), Some(2), 3))
        && params.contains(&(Some(0), Some(2), 8))
        && params.contains(&(Some(0), Some(0), 0))
        && params.contains(&(Some(0), Some(1), 0))
}

fn collect_pressure_levels_pa(entries: &[GribIndexEntry]) -> Result<Option<Vec<f64>>, DecodeError> {
    // Prefer the eastward-wind isobaric stack as the transport vertical axis. Mixed
    // archives can encode unrelated isobaric diagnostics with different scale factors.
    let mut levels = isobaric_levels_for_parameter(entries, Some(0), Some(2), 2);
    if levels.is_empty() {
        levels = isobaric_levels_for_parameter(entries, Some(0), Some(0), 0);
    }
    if levels.is_empty() {
        let mut all = BTreeSet::new();
        for entry in entries {
            if entry.identity.level_type == "isobaric" {
                all.insert(entry.identity.level);
            }
        }
        levels = all;
    }
    if levels.is_empty() {
        return Ok(None);
    }
    let mut pressure_pa = levels
        .iter()
        .copied()
        .map(|level| isobaric_raw_to_pa(level, &levels))
        .collect::<Vec<_>>();
    // Canonical order: strictly increasing pressure in pascals.
    pressure_pa.sort_by(|left, right| left.total_cmp(right));
    if pressure_pa.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err(DecodeError::InvalidMetadata(
            "isobaric pressure levels are not strictly increasing after normalization".into(),
        ));
    }
    Ok(Some(pressure_pa))
}

fn isobaric_levels_for_parameter(
    entries: &[GribIndexEntry],
    discipline: Option<u8>,
    category: Option<u8>,
    number: u8,
) -> BTreeSet<i32> {
    entries
        .iter()
        .filter(|entry| {
            entry.identity.level_type == "isobaric"
                && entry.identity.parameter.discipline == discipline
                && entry.identity.parameter.category == category
                && entry.identity.parameter.number == number
        })
        .map(|entry| entry.identity.level)
        .collect()
}

fn isobaric_levels_use_hpa_encoding(levels: &BTreeSet<i32>) -> bool {
    // GRIB2 type 100 is Pa. Compact millibar archives keep every level <= 2000.
    !levels.is_empty() && levels.iter().all(|level| level.abs() <= 2000)
}

fn isobaric_raw_to_pa(level: i32, levels: &BTreeSet<i32>) -> f64 {
    if isobaric_levels_use_hpa_encoding(levels) {
        f64::from(level) * 100.0
    } else {
        f64::from(level)
    }
}

fn pressure_levels_signature(levels: &[f64]) -> String {
    let mut hasher = Sha256::new();
    hasher.update((levels.len() as u64).to_be_bytes());
    for value in levels {
        hasher.update(value.to_bits().to_be_bytes());
    }
    hex::encode(hasher.finalize())
}

fn ordered_layered_entries<'a>(
    index: &GribFileIndex,
    first: &GribIndexEntry,
    by_level: &BTreeMap<i32, &'a GribIndexEntry>,
) -> Result<Vec<&'a GribIndexEntry>, DecodeError> {
    if first.identity.level_type == "isobaric" {
        let Some(VerticalTopology::PressureLevels(levels)) = index.source_vertical.as_ref() else {
            return Err(DecodeError::InvalidMetadata(
                "isobaric field selected without indexed pressure topology".into(),
            ));
        };
        let raw_levels = by_level.keys().copied().collect::<BTreeSet<_>>();
        let mut ordered = Vec::with_capacity(levels.pressure_pa.len());
        for pressure in levels.pressure_pa.iter() {
            let entry = by_level
                .iter()
                .find_map(|(raw, entry)| {
                    let converted = isobaric_raw_to_pa(*raw, &raw_levels);
                    ((converted - *pressure).abs() <= 1.0e-6).then_some(*entry)
                })
                .ok_or_else(|| {
                    DecodeError::InvalidMetadata(format!(
                        "missing isobaric level for {pressure} Pa while decoding stacked field"
                    ))
                })?;
            ordered.push(entry);
        }
        if ordered.len() != by_level.len() {
            return Err(DecodeError::InvalidMetadata(
                "selected isobaric levels do not match the complete pressure topology".into(),
            ));
        }
        return Ok(ordered);
    }
    Ok(by_level.values().copied().collect())
}

fn is_periodic_longitude(grid: &GribGridDecoder) -> bool {
    ((grid.nx as f64) * grid.longitude_increment_degrees.abs() - 360.0).abs() < 1.0e-8
}

fn grid_signature(grid: &GribGridDecoder) -> String {
    let mut hasher = Sha256::new();
    for value in [
        grid.nx as u64,
        grid.ny as u64,
        u64::from(grid.scanning_mode),
    ] {
        hasher.update(value.to_be_bytes());
    }
    for value in [
        grid.first_longitude_degrees,
        grid.first_latitude_degrees,
        grid.last_longitude_degrees,
        grid.last_latitude_degrees,
        grid.longitude_increment_degrees,
        grid.latitude_increment_degrees,
    ] {
        hasher.update(value.to_bits().to_be_bytes());
    }
    hex::encode(hasher.finalize())
}

fn hybrid_signature(pv: &HybridPvDecoder) -> String {
    let mut hasher = Sha256::new();
    hasher.update((pv.a_half_pa.len() as u64).to_be_bytes());
    for value in pv.a_half_pa.iter().chain(&pv.b_half) {
        hasher.update(value.to_bits().to_be_bytes());
    }
    hex::encode(hasher.finalize())
}

fn reference_time_to_timestamp(time: ReferenceTime) -> Result<Timestamp, DecodeError> {
    let days = days_from_civil(time.year, time.month, time.day).ok_or_else(|| {
        DecodeError::InvalidMetadata("GRIB validity date is outside Timestamp range".into())
    })?;
    let seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(i64::from(time.hour) * 3_600))
        .and_then(|value| value.checked_add(i64::from(time.minute) * 60))
        .and_then(|value| value.checked_add(i64::from(time.second)))
        .ok_or_else(|| DecodeError::InvalidMetadata("GRIB validity time overflow".into()))?;
    Timestamp::new(seconds, 0).map_err(|error| DecodeError::InvalidMetadata(error.to_string()))
}

fn days_from_civil(year: u16, month: u8, day: u8) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let month = i64::from(month);
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn canonical_path(path: &Path) -> Result<PathBuf, DecodeError> {
    std::fs::canonicalize(path).map_err(|error| DecodeError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn map_grib_error(path: &Path, error: grib_reader::Error) -> DecodeError {
    match error {
        grib_reader::Error::Io(source, _) => DecodeError::Io {
            path: path.to_path_buf(),
            message: source.to_string(),
        },
        other => DecodeError::InvalidMetadata(other.to_string()),
    }
}

#[cfg(feature = "native-eccodes")]
enum NativeCommand {
    Decode {
        /// Stable physical-message and logical-subfield identity.
        field: NativeFieldKey,
        response: mpsc::SyncSender<Result<Vec<f64>, String>>,
    },
    Shutdown,
}

#[cfg(feature = "native-eccodes")]
struct NativeDecoder {
    commands: mpsc::SyncSender<NativeCommand>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[cfg(feature = "native-eccodes")]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct NativeFieldKey {
    message_offset: u64,
    field_index_in_message: usize,
}

#[cfg(feature = "native-eccodes")]
impl From<&GribIndexEntry> for NativeFieldKey {
    fn from(entry: &GribIndexEntry) -> Self {
        Self {
            message_offset: entry.offset,
            field_index_in_message: entry.field_index_in_message,
        }
    }
}

#[cfg(feature = "native-eccodes")]
impl fmt::Debug for NativeDecoder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeDecoder { ecCodes worker }")
    }
}

#[cfg(feature = "native-eccodes")]
impl NativeDecoder {
    fn start(path: &Path, required_entries: &[GribIndexEntry]) -> Result<Self, DecodeError> {
        let (commands, receiver) = mpsc::sync_channel(1);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker_path = path.to_path_buf();
        let required_entries = required_entries.to_vec();
        let worker = thread::Builder::new()
            .name("trajecta-eccodes".into())
            .spawn(move || {
                native_worker(&worker_path, &required_entries, receiver, ready_sender);
            })
            .map_err(|error| DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message: format!("cannot start ecCodes worker: {error}"),
            })?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                commands,
                worker: Mutex::new(Some(worker)),
            }),
            Ok(Err(message)) => {
                let _ = worker.join();
                Err(DecodeError::BackendUnavailable {
                    backend: MeteorologyReaderBackend::Native,
                    message,
                })
            }
            Err(error) => {
                let _ = worker.join();
                Err(DecodeError::BackendUnavailable {
                    backend: MeteorologyReaderBackend::Native,
                    message: format!("ecCodes worker initialization channel failed: {error}"),
                })
            }
        }
    }

    fn decode(&self, field: NativeFieldKey) -> Result<Vec<f64>, DecodeError> {
        let (response, receiver) = mpsc::sync_channel(1);
        self.commands
            .send(NativeCommand::Decode { field, response })
            .map_err(|error| DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message: format!("ecCodes worker stopped: {error}"),
            })?;
        receiver
            .recv()
            .map_err(|error| DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message: format!("ecCodes response channel failed: {error}"),
            })?
            .map_err(DecodeError::InvalidMetadata)
    }
}

#[cfg(feature = "native-eccodes")]
impl Drop for NativeDecoder {
    fn drop(&mut self) {
        let _ = self.commands.send(NativeCommand::Shutdown);
        if let Ok(mut worker) = self.worker.lock()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

#[cfg(feature = "native-eccodes")]
fn native_worker(
    path: &Path,
    required_entries: &[GribIndexEntry],
    receiver: mpsc::Receiver<NativeCommand>,
    ready: mpsc::SyncSender<Result<(), String>>,
) {
    use std::collections::HashMap;

    use eccodes::{
        CodesHandle, FallibleStreamingIterator, KeyRead, KeyedMessage, ProductKind,
        enable_grib_multi_support,
    };

    // The Windows ecCodes build can race while lazily parsing its definition
    // files. On Windows the guard keeps ecCodes FFI calls process-serial while
    // retaining one worker per opened source; it is a no-op on other platforms.
    let messages = {
        let _call_guard = native_eccodes_call_guard();
        (|| -> Result<HashMap<NativeFieldKey, KeyedMessage>, String> {
            enable_grib_multi_support();
            let mut handle = CodesHandle::new_from_file(path, ProductKind::GRIB)
                .map_err(|error| error.to_string())?;
            let required = required_entries
                .iter()
                .map(|entry| (NativeFieldKey::from(entry), entry))
                .collect::<HashMap<_, _>>();
            let mut next_field_by_offset = HashMap::<u64, usize>::new();
            let mut fields = HashMap::new();
            while let Some(message) = handle.next().map_err(|error| error.to_string())? {
                let offset = read_eccodes_message_offset(message)?;
                let field_index = next_field_by_offset.entry(offset).or_default();
                let key = NativeFieldKey {
                    message_offset: offset,
                    field_index_in_message: *field_index,
                };
                *field_index += 1;
                if let Some(expected) = required.get(&key) {
                    validate_eccodes_field_identity(expected, message)?;
                    let message = message.try_clone().map_err(|error| error.to_string())?;
                    if fields.insert(key, message).is_some() {
                        return Err(format!(
                            "ecCodes produced duplicate logical GRIB field at offset {} subfield {}",
                            key.message_offset, key.field_index_in_message
                        ));
                    }
                }
            }
            // Require every structurally indexed logical field to map exactly.
            // Extra messages or fields seen only by ecCodes are allowed.
            for key in required.keys() {
                if !fields.contains_key(key) {
                    return Err(format!(
                        "ecCodes is missing logical GRIB field at offset {} subfield {} required by the structural index (matched {}, required {})",
                        key.message_offset,
                        key.field_index_in_message,
                        fields.len(),
                        required.len()
                    ));
                }
            }
            Ok(fields)
        })()
    };
    let messages = match messages {
        Ok(messages) => messages,
        Err(message) => {
            let _ = ready.send(Err(message));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        let _call_guard = native_eccodes_call_guard();
        drop(messages);
        return;
    }

    while let Ok(command) = receiver.recv() {
        match command {
            NativeCommand::Decode { field, response } => {
                let result = {
                    let _call_guard = native_eccodes_call_guard();
                    messages
                        .get(&field)
                        .ok_or_else(|| {
                            format!(
                                "ecCodes logical field at offset {} subfield {} is absent",
                                field.message_offset, field.field_index_in_message
                            )
                        })
                        .and_then(|message| {
                            let mut values: Vec<f64> = message
                                .read_key("values")
                                .map_err(|error| error.to_string())?;
                            let missing_count: i64 = message
                                .read_key("numberOfMissing")
                                .map_err(|error| error.to_string())?;
                            if missing_count > 0 {
                                let missing_value: f64 = message
                                    .read_key("missingValue")
                                    .map_err(|error| error.to_string())?;
                                for value in &mut values {
                                    if *value == missing_value {
                                        *value = f64::NAN;
                                    }
                                }
                            }
                            Ok(values)
                        })
                };
                let _ = response.send(result);
            }
            NativeCommand::Shutdown => break,
        }
    }

    let _call_guard = native_eccodes_call_guard();
    drop(messages);
}

#[cfg(feature = "native-eccodes")]
fn read_eccodes_message_offset(message: &eccodes::KeyedMessage) -> Result<u64, String> {
    use eccodes::KeyRead;

    // ecCodes exposes the byte offset of the containing physical message. Its
    // reported native key type is platform/version dependent, so use the safe
    // unchecked conversion path while retaining range validation here.
    let offset: i64 = message
        .read_key_unchecked("offset")
        .map_err(|error| format!("ecCodes message offset key is unavailable: {error}"))?;
    if offset < 0 {
        return Err(format!("ecCodes message offset is negative: {offset}"));
    }
    Ok(offset as u64)
}

#[cfg(feature = "native-eccodes")]
fn validate_eccodes_field_identity(
    expected: &GribIndexEntry,
    message: &eccodes::KeyedMessage,
) -> Result<(), String> {
    use eccodes::KeyRead;

    let read_long = |key: &str| -> Result<i64, String> {
        message
            .read_key_unchecked(key)
            .map_err(|error| format!("ecCodes key {key} is unavailable: {error}"))
    };
    let edition = u8::try_from(read_long("edition")?)
        .map_err(|_| "ecCodes edition is outside u8".to_owned())?;
    if edition != expected.identity.edition {
        return Err(format!(
            "ecCodes field identity mismatch at offset {} subfield {}: edition {} != {}",
            expected.offset, expected.field_index_in_message, edition, expected.identity.edition
        ));
    }

    let (number, discipline, category, table_version, level_type_code, level) = if edition == 2 {
        let discipline = u8::try_from(read_long("discipline")?)
            .map_err(|_| "ecCodes discipline is outside u8".to_owned())?;
        let category = u8::try_from(read_long("parameterCategory")?)
            .map_err(|_| "ecCodes parameterCategory is outside u8".to_owned())?;
        let number = u8::try_from(read_long("parameterNumber")?)
            .map_err(|_| "ecCodes parameterNumber is outside u8".to_owned())?;
        let level_type_code = u8::try_from(read_long("typeOfFirstFixedSurface")?)
            .map_err(|_| "ecCodes typeOfFirstFixedSurface is outside u8".to_owned())?;
        let scale_factor = i32::try_from(read_long("scaleFactorOfFirstFixedSurface")?)
            .map_err(|_| "ecCodes first-surface scale factor is outside i32".to_owned())?;
        let scaled_value = read_long("scaledValueOfFirstFixedSurface")? as f64;
        let level = scaled_value * 10.0_f64.powi(-scale_factor);
        let level = exact_integral_level(level).map_err(|error| format!("{error:?}"))?;
        (
            number,
            Some(discipline),
            Some(category),
            None,
            level_type_code,
            Some(level),
        )
    } else {
        let number = u8::try_from(read_long("indicatorOfParameter")?)
            .map_err(|_| "ecCodes indicatorOfParameter is outside u8".to_owned())?;
        let table_version = u8::try_from(read_long("table2Version")?)
            .map_err(|_| "ecCodes table2Version is outside u8".to_owned())?;
        let level_type_code = u8::try_from(read_long("indicatorOfTypeOfLevel")?)
            .map_err(|_| "ecCodes indicatorOfTypeOfLevel is outside u8".to_owned())?;
        let level = if is_layered_level_type(&expected.identity.level_type) {
            Some(
                i32::try_from(read_long("level")?)
                    .map_err(|_| "ecCodes GRIB1 level is outside i32".to_owned())?,
            )
        } else {
            None
        };
        (
            number,
            None,
            None,
            Some(table_version),
            level_type_code,
            level,
        )
    };

    let actual = (
        discipline,
        category,
        number,
        table_version,
        level_type_code,
        level,
    );
    let wanted = (
        expected.identity.parameter.discipline,
        expected.identity.parameter.category,
        expected.identity.parameter.number,
        expected.identity.parameter.table_version,
        expected.identity.level_type_code,
        (edition == 2 || is_layered_level_type(&expected.identity.level_type))
            .then_some(expected.identity.level),
    );
    if actual != wanted {
        return Err(format!(
            "ecCodes field identity mismatch at offset {} subfield {}: {:?} != {:?}",
            expected.offset, expected.field_index_in_message, actual, wanted
        ));
    }
    Ok(())
}

#[cfg(all(feature = "native-eccodes", windows))]
fn native_eccodes_call_guard() -> std::sync::MutexGuard<'static, ()> {
    NATIVE_ECCODES_CALL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(all(feature = "native-eccodes", not(windows)))]
fn native_eccodes_call_guard() {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn real_era5_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap()
            .join("tools/flexctl/target/test-data/ecmwf-era5/flex_extract-7.1/EA18120100")
    }

    #[test]
    fn real_era5_hybrid_grib_indexes_complete_pv_and_decodes_active_layers() {
        let fixture = real_era5_fixture();
        if !fixture.is_file() {
            return;
        }
        let reader = GribReader::new(MeteorologyReaderBackend::Rust);
        let metadata = reader.inspect(&fixture).unwrap();
        let index = reader.build_index(&fixture).unwrap();
        let concrete = index.as_any().downcast_ref::<GribFileIndex>().unwrap();
        assert_eq!(metadata.format, SourceFormat::Grib2);
        assert_eq!(
            metadata.attributes.get("centre").map(String::as_str),
            Some("98")
        );
        assert_eq!(
            metadata
                .attributes
                .get("dataset_family")
                .map(String::as_str),
            Some("era5_flex_extract_hybrid")
        );
        assert_eq!(
            metadata
                .attributes
                .get("message_editions")
                .map(String::as_str),
            Some("1,2")
        );
        assert_eq!(metadata.dimensions.get("x"), Some(&6));
        assert_eq!(metadata.dimensions.get("y"), Some(&6));
        assert_eq!(
            metadata.valid_times,
            vec![Timestamp::new(1_543_622_400, 0).unwrap()]
        );
        assert!(matches!(
            metadata.vertical,
            Some(VerticalSignature::HybridPressure {
                full_level_count: 137,
                ..
            })
        ));

        assert_eq!(
            concrete.active_full_levels,
            (130_u16..=137).collect::<Vec<_>>()
        );
        let pv = concrete.hybrid_pv.as_ref().unwrap();
        assert_eq!(pv.a_half_pa.len(), 138);
        assert_eq!(pv.b_half.len(), 138);
        assert_eq!(pv.a_half_pa[0], 0.0);
        assert_eq!(pv.b_half[137], 1.0);

        let decoded = reader
            .decode(
                &fixture,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![
                        ("param_id".into(), "130".into()),
                        ("type_of_level".into(), "hybrid".into()),
                    ],
                    valid_time: metadata.valid_times.first().copied(),
                },
            )
            .unwrap();
        assert_eq!(
            decoded.layout,
            ArrayLayout::Full3D {
                levels: 8,
                ny: 6,
                nx: 6,
            }
        );
        assert_eq!(decoded.values.len(), 288);
        assert!(decoded.valid.iter().all(|value| *value));
        assert!(decoded.values.iter().all(|value| value.is_finite()));
        assert_eq!(decoded.source_unit.symbol(), "K");
    }

    #[cfg(feature = "native-eccodes")]
    #[test]
    fn native_eccodes_and_rust_match_all_era5_transport_fields() {
        let fixture = real_era5_fixture();
        if !fixture.is_file() {
            return;
        }
        let rust = GribReader::new(MeteorologyReaderBackend::Rust);
        let native = GribReader::new(MeteorologyReaderBackend::Native);
        let rust_index = rust.build_index(&fixture).unwrap();
        let native_index = native.build_index(&fixture).unwrap();
        let valid_time = rust.inspect(&fixture).unwrap().valid_times[0];
        for (param_id, level_type) in [
            (131, "hybrid"),
            (132, "hybrid"),
            (77, "hybrid"),
            (130, "hybrid"),
            (133, "hybrid"),
            (134, "surface"),
            (129, "surface"),
        ] {
            let request = DecodeRequest {
                source_identity: vec![
                    ("param_id".into(), param_id.to_string()),
                    ("type_of_level".into(), level_type.into()),
                ],
                valid_time: Some(valid_time),
            };
            let rust_field = rust
                .decode(&fixture, rust_index.as_ref(), &request)
                .unwrap();
            let native_field = native
                .decode(&fixture, native_index.as_ref(), &request)
                .unwrap();
            assert_eq!(native_field.layout, rust_field.layout);
            assert_eq!(native_field.valid, rust_field.valid);
            assert_eq!(native_field.source_unit, rust_field.source_unit);
            let max_difference = native_field
                .values
                .iter()
                .zip(rust_field.values.iter())
                .map(|(native, rust)| (native - rust).abs())
                .fold(0.0_f64, f64::max);
            assert!(
                max_difference <= 1.0e-12,
                "param_id={param_id} max_difference={max_difference}"
            );
        }
    }
}
