//! Pure-Rust NetCDF4/HDF5 subset reader (`netcdf-reader` / `hdf5-reader`).
//!
//! Does not call netCDF-C or the HDF5 C library. Unsupported filters/types fail
//! with structured metadata errors.
//!
//! The index owns one dedicated reader worker: the file is opened once on that
//! thread. Field decode issues a **time/level hyperslab** via
//! `read_variable_slice_as_f64` (single time index, full vertical extent) and
//! only keeps a bounded slab cache. Releasing the last `Arc` to the worker
//! closes the file.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;

use netcdf_reader::{NcFile, NcSliceInfo, NcSliceInfoElem};
use sha2::{Digest, Sha256};
use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::lockfile::{GridSignature, VerticalSignature};
use trajecta_case::model::time::Timestamp;

use super::netcdf::{
    CfCoordinateResolver, DimensionPlan, NetCdfVariable, NetCdfVariableIndex,
    apply_packing_and_mask, decode_cf_time_pub, decode_prepared_time_slab, dimension_plan,
    reject_unsupported_calendar_pub, resolve_cf_axes_pub, resolve_time_index,
};
use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, SourceFormat, SourceGridGeometry,
};
use crate::vertical::{
    HybridCoefficients, HybridPressureTopology, PressureLevels, VerticalTopology,
};

/// Bound on cached hyperslab results (variable + selection key → values).
const MAX_SLAB_CACHE_ENTRIES: usize = 16;

enum Nc4Command {
    ReadHyperslab {
        name: String,
        selection: NcSliceInfo,
        response: mpsc::SyncSender<Result<Vec<f64>, String>>,
    },
    Shutdown,
}

/// Pure-Rust NetCDF4 worker owned by [`NetCdfVariableIndex`] via `Arc`.
#[derive(Debug)]
pub(super) struct NetCdf4ReadWorker {
    commands: mpsc::Sender<Nc4Command>,
}

impl NetCdf4ReadWorker {
    fn start(path: PathBuf) -> Result<(Arc<Self>, IndexParts), DecodeError> {
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_path = path.clone();
        thread::Builder::new()
            .name("trajecta-netcdf4-rust".into())
            .spawn(move || nc4_worker(worker_path, receiver, ready_tx))
            .map_err(|error| DecodeError::Io {
                path: path.clone(),
                message: format!("failed to start pure-Rust NetCDF4 worker: {error}"),
            })?;
        match ready_rx.recv() {
            Ok(Ok(parts)) => Ok((Arc::new(Self { commands }), parts)),
            Ok(Err(message)) => Err(DecodeError::InvalidMetadata(message)),
            Err(error) => Err(DecodeError::Io {
                path,
                message: format!("NetCDF4 worker init channel failed: {error}"),
            }),
        }
    }

    fn read_hyperslab(&self, name: &str, selection: NcSliceInfo) -> Result<Vec<f64>, DecodeError> {
        let (response, receiver) = mpsc::sync_channel(1);
        self.commands
            .send(Nc4Command::ReadHyperslab {
                name: name.to_owned(),
                selection,
                response,
            })
            .map_err(|error| {
                DecodeError::InvalidMetadata(format!("NetCDF4 worker stopped: {error}"))
            })?;
        receiver
            .recv()
            .map_err(|error| {
                DecodeError::InvalidMetadata(format!("NetCDF4 response failed: {error}"))
            })?
            .map_err(DecodeError::InvalidMetadata)
    }
}

impl Drop for NetCdf4ReadWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(Nc4Command::Shutdown);
    }
}

pub(super) fn build_rust_netcdf4_index(file: &Path) -> Result<NetCdfVariableIndex, DecodeError> {
    let path = std::fs::canonicalize(file).map_err(|error| DecodeError::Io {
        path: file.to_path_buf(),
        message: error.to_string(),
    })?;
    let (worker, parts) = NetCdf4ReadWorker::start(path.clone())?;
    Ok(NetCdfVariableIndex {
        path,
        format: SourceFormat::NetCdf4,
        variables: parts.variables,
        dimensions: parts.dimensions,
        global_attributes: parts.global_attributes,
        valid_times: parts.valid_times,
        time_coordinates: parts.time_coordinates,
        axes: parts.axes,
        vertical_source_order: parts.vertical_source_order,
        source_grid: Some(parts.source_grid),
        source_vertical: parts.source_vertical,
        source_grid_signature: Some(parts.source_grid_signature),
        source_vertical_signature: parts.source_vertical_signature,
        backend: MeteorologyReaderBackend::Rust,
        rust_worker: None,
        nc4_worker: Some(worker),
        native_worker: None,
    })
}

pub(super) fn decode_rust_netcdf4(
    index: &NetCdfVariableIndex,
    request: &DecodeRequest,
) -> Result<DecodedField, DecodeError> {
    let variable_name = request
        .source_identity
        .iter()
        .find_map(|(key, value)| (key == "variable" || key == "name").then_some(value.as_str()))
        .ok_or(DecodeError::MissingField)?;
    let variable = index
        .variables
        .get(variable_name)
        .ok_or(DecodeError::MissingField)?;
    let worker = index.nc4_worker.as_ref().ok_or_else(|| {
        DecodeError::InvalidMetadata("pure-Rust NetCDF4 index has no reader worker".into())
    })?;
    let plan = dimension_plan(index, variable)?;
    let valid_time = request
        .valid_time
        .or_else(|| index.time_coordinates.first().copied())
        .or_else(|| index.valid_times.first().copied())
        .ok_or_else(|| DecodeError::InvalidMetadata("NetCDF decode has no validity time".into()))?;
    let time_index = resolve_time_index(index, valid_time, plan.has_time)?;
    let selection = hyperslab_selection(index, variable, &plan, time_index)?;
    let raw = worker.read_hyperslab(variable_name, selection)?;
    let (values, valid) = apply_packing_and_mask(variable, &raw)?;
    // Hyperslab already pinned the time axis; finalize only reorders levels/mask.
    decode_prepared_time_slab(index, request, variable_name, variable, values, valid)
}

fn hyperslab_selection(
    index: &NetCdfVariableIndex,
    variable: &NetCdfVariable,
    plan: &DimensionPlan,
    time_index: Option<usize>,
) -> Result<NcSliceInfo, DecodeError> {
    let time_name = index.axes.axes.get("time").map(String::as_str);
    let vertical_name = index.axes.axes.get("vertical").map(String::as_str);
    let mut selections = Vec::with_capacity(variable.dimensions.len());
    for dim in &variable.dimensions {
        let len = index.dimensions.get(dim).copied().ok_or_else(|| {
            DecodeError::InvalidMetadata(format!(
                "pure-Rust NetCDF4 hyperslab dimension {dim} is absent from index"
            ))
        })?;
        let len_u64 = u64::try_from(len).map_err(|_| {
            DecodeError::InvalidMetadata(format!("dimension {dim} length overflows u64"))
        })?;
        if time_name == Some(dim.as_str()) {
            let t = time_index.ok_or_else(|| {
                DecodeError::InvalidMetadata(
                    "pure-Rust NetCDF4 hyperslab requires a resolved time index".into(),
                )
            })?;
            if t >= len {
                return Err(DecodeError::InvalidMetadata(
                    "pure-Rust NetCDF4 hyperslab time index is out of range".into(),
                ));
            }
            selections.push(NcSliceInfoElem::Index(t as u64));
        } else if vertical_name == Some(dim.as_str()) {
            if !plan.has_vertical {
                return Err(DecodeError::InvalidMetadata(
                    "vertical axis present on variable without 3D plan".into(),
                ));
            }
            // Full vertical extent for the selected time; reordering is applied after.
            selections.push(NcSliceInfoElem::Slice {
                start: 0,
                end: len_u64,
                step: 1,
            });
        } else {
            selections.push(NcSliceInfoElem::Slice {
                start: 0,
                end: len_u64,
                step: 1,
            });
        }
    }
    Ok(NcSliceInfo { selections })
}

fn selection_cache_key(name: &str, selection: &NcSliceInfo) -> String {
    let mut key = name.to_owned();
    for elem in &selection.selections {
        match elem {
            NcSliceInfoElem::Index(idx) => {
                key.push_str(&format!("|i{idx}"));
            }
            NcSliceInfoElem::Slice { start, end, step } => {
                key.push_str(&format!("|s{start}:{end}:{step}"));
            }
        }
    }
    key
}

struct IndexParts {
    variables: BTreeMap<String, NetCdfVariable>,
    dimensions: BTreeMap<String, usize>,
    global_attributes: BTreeMap<String, String>,
    valid_times: Vec<Timestamp>,
    time_coordinates: Vec<Timestamp>,
    axes: CfCoordinateResolver,
    vertical_source_order: Vec<usize>,
    source_grid: SourceGridGeometry,
    source_vertical: Option<VerticalTopology>,
    source_grid_signature: GridSignature,
    source_vertical_signature: Option<VerticalSignature>,
}

fn nc4_worker(
    path: PathBuf,
    receiver: mpsc::Receiver<Nc4Command>,
    ready: mpsc::SyncSender<Result<IndexParts, String>>,
) {
    let nc = match NcFile::open(&path) {
        Ok(nc) => nc,
        Err(error) => {
            let _ = ready.send(Err(format!("pure-Rust NetCDF4 open failed: {error}")));
            return;
        }
    };
    let parts = match index_parts_from_nc_file(&nc) {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(format!("{error:?}")));
            return;
        }
    };
    if ready.send(Ok(parts)).is_err() {
        return;
    }
    // Bounded slab cache: never stores whole multi-time variables.
    let mut cache = BTreeMap::<String, Vec<f64>>::new();
    let mut cache_order = VecDeque::<String>::new();
    while let Ok(command) = receiver.recv() {
        match command {
            Nc4Command::ReadHyperslab {
                name,
                selection,
                response,
            } => {
                let key = selection_cache_key(&name, &selection);
                let result = if let Some(values) = cache.get(&key) {
                    Ok(values.clone())
                } else {
                    match nc.read_variable_slice_as_f64(&name, &selection) {
                        Ok(array) => {
                            let values = array.iter().copied().collect::<Vec<_>>();
                            if cache.len() >= MAX_SLAB_CACHE_ENTRIES {
                                if let Some(old) = cache_order.pop_front() {
                                    cache.remove(&old);
                                }
                            }
                            cache.insert(key.clone(), values.clone());
                            cache_order.push_back(key);
                            Ok(values)
                        }
                        Err(error) => Err(format!("hyperslab read {name}: {error}")),
                    }
                };
                let _ = response.send(result);
            }
            Nc4Command::Shutdown => break,
        }
    }
}

fn index_parts_from_nc_file(nc: &NcFile) -> Result<IndexParts, DecodeError> {
    let mut dimensions = BTreeMap::new();
    for dim in nc.dimensions().map_err(map_nc4)? {
        let size = usize::try_from(dim.size).map_err(|_| {
            DecodeError::InvalidMetadata(format!("dimension {} size overflows usize", dim.name))
        })?;
        dimensions.insert(dim.name.clone(), size);
    }
    let mut global_attributes = BTreeMap::new();
    for attr in nc.global_attributes().map_err(map_nc4)? {
        global_attributes.insert(attr.name.clone(), attr_to_string(attr));
    }
    let mut variables = BTreeMap::new();
    for var in nc.variables().map_err(map_nc4)? {
        let mut attributes = BTreeMap::new();
        for attr in var.attributes() {
            attributes.insert(attr.name.clone(), attr_to_string(attr));
        }
        variables.insert(
            var.name().to_owned(),
            NetCdfVariable {
                name: var.name().to_owned(),
                dimensions: var
                    .dimensions()
                    .iter()
                    .map(|dimension| dimension.name.clone())
                    .collect(),
                attributes,
            },
        );
    }
    let axes = resolve_cf_axes_pub(&variables)?;
    let (valid_times, time_coordinates) = decode_time_from_nc(nc, &variables, &axes)?;
    let (source_grid, source_grid_signature) = decode_grid_from_nc(nc, &axes)?;
    let (source_vertical, source_vertical_signature, vertical_source_order) =
        decode_vertical_from_nc(nc, &variables, &axes)?;
    Ok(IndexParts {
        variables,
        dimensions,
        global_attributes,
        valid_times,
        time_coordinates,
        axes,
        vertical_source_order,
        source_grid,
        source_vertical,
        source_grid_signature,
        source_vertical_signature,
    })
}

fn decode_time_from_nc(
    nc: &NcFile,
    variables: &BTreeMap<String, NetCdfVariable>,
    axes: &CfCoordinateResolver,
) -> Result<(Vec<Timestamp>, Vec<Timestamp>), DecodeError> {
    let Some(time_name) = axes.axes.get("time") else {
        return Ok((Vec::new(), Vec::new()));
    };
    let variable = variables.get(time_name).ok_or_else(|| {
        DecodeError::InvalidMetadata(format!("CF time axis variable {time_name} is absent"))
    })?;
    let units = variable
        .attributes
        .get("units")
        .cloned()
        .ok_or_else(|| DecodeError::InvalidMetadata("CF time axis has no units".into()))?;
    let calendar = variable
        .attributes
        .get("calendar")
        .cloned()
        .unwrap_or_else(|| "standard".into());
    reject_unsupported_calendar_pub(&calendar)?;
    let values = read_f64_var(nc, time_name)?;
    let mut ordered = Vec::with_capacity(values.len());
    let mut unique = BTreeSet::new();
    for value in values {
        let timestamp = decode_cf_time_pub(value, &units)?;
        ordered.push(timestamp);
        unique.insert(timestamp);
    }
    Ok((unique.into_iter().collect(), ordered))
}

fn decode_grid_from_nc(
    nc: &NcFile,
    axes: &CfCoordinateResolver,
) -> Result<(SourceGridGeometry, GridSignature), DecodeError> {
    let lat_name = axes.axes.get("latitude").ok_or_else(|| {
        DecodeError::InvalidMetadata("NetCDF source has no CF latitude axis".into())
    })?;
    let lon_name = axes.axes.get("longitude").ok_or_else(|| {
        DecodeError::InvalidMetadata("NetCDF source has no CF longitude axis".into())
    })?;
    let latitudes = read_f64_var(nc, lat_name)?;
    let longitudes = read_f64_var(nc, lon_name)?;
    if latitudes.len() < 2 || longitudes.len() < 2 {
        return Err(DecodeError::InvalidMetadata(
            "latitude/longitude axes must contain at least two points".into(),
        ));
    }
    let lat_spacing = regular_spacing(&latitudes)?;
    let lon_spacing = regular_spacing(&longitudes)?;
    let ny = latitudes.len();
    let nx = longitudes.len();
    let geometry = SourceGridGeometry {
        longitude_origin_degrees: longitudes[0],
        latitude_origin_degrees: latitudes[0],
        longitude_spacing_degrees: lon_spacing,
        latitude_spacing_degrees: lat_spacing,
        nx,
        ny,
        periodic_longitude: ((nx as f64) * lon_spacing.abs() - 360.0).abs() < 1.0e-6,
    };
    let mut hasher = Sha256::new();
    hasher.update((nx as u64).to_be_bytes());
    hasher.update((ny as u64).to_be_bytes());
    for value in longitudes.iter().chain(latitudes.iter()) {
        hasher.update(value.to_bits().to_be_bytes());
    }
    let signature = GridSignature {
        nx,
        ny,
        periodic_longitude: geometry.periodic_longitude,
        sha256: hex::encode(hasher.finalize()),
    };
    Ok((geometry, signature))
}

type VerticalParts = (
    Option<VerticalTopology>,
    Option<VerticalSignature>,
    Vec<usize>,
);

fn decode_vertical_from_nc(
    nc: &NcFile,
    variables: &BTreeMap<String, NetCdfVariable>,
    axes: &CfCoordinateResolver,
) -> Result<VerticalParts, DecodeError> {
    if let (Some(a_name), Some(b_name)) = (
        axes.formula_terms
            .get("a")
            .or_else(|| axes.formula_terms.get("ap")),
        axes.formula_terms.get("b"),
    ) {
        let a = read_f64_var(nc, a_name)?;
        let b = read_f64_var(nc, b_name)?;
        if a.len() != b.len() || a.len() < 2 {
            return Err(DecodeError::InvalidMetadata(
                "hybrid formula_terms A/B coefficient lengths are inconsistent".into(),
            ));
        }
        let a_half_pa = if a.iter().all(|value| value.abs() <= 2_000.0) {
            a.iter().map(|value| value * 100.0).collect::<Vec<_>>()
        } else {
            a
        };
        let full_level_count = a_half_pa.len() - 1;
        let active = if let Some(vertical_name) = axes.axes.get("vertical") {
            read_f64_var(nc, vertical_name)?
                .iter()
                .map(|value| *value as u16)
                .collect::<Vec<_>>()
        } else {
            (1_u16..=u16::try_from(full_level_count).map_err(|_| {
                DecodeError::InvalidMetadata("hybrid full level count exceeds u16".into())
            })?)
                .collect::<Vec<_>>()
        };
        let mut hasher = Sha256::new();
        hasher.update((a_half_pa.len() as u64).to_be_bytes());
        for value in &a_half_pa {
            hasher.update(value.to_bits().to_be_bytes());
        }
        return Ok((
            Some(VerticalTopology::HybridPressure(HybridPressureTopology {
                coefficients: HybridCoefficients {
                    a_half_pa: Arc::from(a_half_pa),
                    b_half: Arc::from(b),
                },
                active_full_levels: Arc::from(active),
            })),
            Some(VerticalSignature::HybridPressure {
                full_level_count,
                coefficients_sha256: hex::encode(hasher.finalize()),
            }),
            Vec::new(),
        ));
    }

    let Some(vertical_name) = axes.axes.get("vertical") else {
        return Ok((None, None, Vec::new()));
    };
    let variable = variables.get(vertical_name).ok_or_else(|| {
        DecodeError::InvalidMetadata(format!("vertical axis {vertical_name} is absent"))
    })?;
    let units = variable
        .attributes
        .get("units")
        .map(String::as_str)
        .unwrap_or("Pa");
    let mut levels = read_f64_var(nc, vertical_name)?;
    let pressure_pa = match units {
        "Pa" | "pa" => levels,
        "hPa" | "hpa" | "mb" | "mbar" | "millibar" | "millibars" => {
            levels.iter_mut().for_each(|value| *value *= 100.0);
            levels
        }
        other => {
            return Err(DecodeError::InvalidMetadata(format!(
                "unsupported vertical units {other}; expected Pa or hPa"
            )));
        }
    };
    let mut order = (0..pressure_pa.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| pressure_pa[*left].total_cmp(&pressure_pa[*right]));
    let ordered = order
        .iter()
        .map(|source| pressure_pa[*source])
        .collect::<Vec<_>>();
    let levels = PressureLevels::new(Arc::from(ordered)).map_err(|error| {
        DecodeError::InvalidMetadata(format!("invalid pressure topology: {error:?}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update((levels.pressure_pa.len() as u64).to_be_bytes());
    for value in levels.pressure_pa.iter() {
        hasher.update(value.to_bits().to_be_bytes());
    }
    Ok((
        Some(VerticalTopology::PressureLevels(levels)),
        Some(VerticalSignature::PressureLevels {
            level_count: order.len(),
            levels_sha256: hex::encode(hasher.finalize()),
        }),
        order,
    ))
}

fn read_f64_var(nc: &NcFile, name: &str) -> Result<Vec<f64>, DecodeError> {
    let array = nc
        .read_variable_as_f64(name)
        .map_err(|error| DecodeError::InvalidMetadata(format!("read {name}: {error}")))?;
    Ok(array.iter().copied().collect())
}

fn regular_spacing(values: &[f64]) -> Result<f64, DecodeError> {
    if values.len() < 2 {
        return Err(DecodeError::InvalidMetadata(
            "coordinate axis is too short for spacing".into(),
        ));
    }
    let spacing = values[1] - values[0];
    if !spacing.is_finite() || spacing == 0.0 {
        return Err(DecodeError::InvalidMetadata(
            "coordinate spacing is not a finite non-zero value".into(),
        ));
    }
    for window in values.windows(2) {
        let step = window[1] - window[0];
        if (step - spacing).abs() > 1.0e-6 * spacing.abs().max(1.0) {
            return Err(DecodeError::InvalidMetadata(
                "v0 NetCDF reader requires a regular latitude-longitude grid".into(),
            ));
        }
    }
    Ok(spacing)
}

fn attr_to_string(attr: &netcdf_reader::NcAttribute) -> String {
    if let Some(text) = attr.value.as_string() {
        return text;
    }
    if let Some(values) = attr.value.as_f64_vec() {
        return values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(value) = attr.value.as_f64() {
        return value.to_string();
    }
    String::new()
}

fn map_nc4(error: netcdf_reader::Error) -> DecodeError {
    DecodeError::InvalidMetadata(format!("pure-Rust NetCDF4: {error}"))
}
