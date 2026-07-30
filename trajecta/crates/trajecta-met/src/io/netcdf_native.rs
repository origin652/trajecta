//! Native netCDF-C/HDF5 backend (`native-netcdf` feature).
//!
//! Never falls back to the pure-Rust readers. The netCDF-C handle lives on a
//! dedicated worker thread; the index holds the only ownership handle and
//! shuts the worker down exactly once when the last `Arc` is dropped.

#![cfg(feature = "native-netcdf")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;

use sha2::{Digest, Sha256};
use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::lockfile::{GridSignature, VerticalSignature};
use trajecta_case::model::time::Timestamp;

use super::netcdf::{
    CfCoordinateResolver, DimensionPlan, NetCdfIndexPartsPublic, NetCdfVariable,
    NetCdfVariableIndex, apply_packing_and_mask, decode_cf_time_pub, decode_prepared_time_slab,
    dimension_plan, reject_unsupported_calendar_pub, resolve_cf_axes_pub, resolve_time_index,
};
use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, SourceFormat, SourceGridGeometry,
};
use crate::vertical::{
    HybridCoefficients, HybridPressureTopology, PressureLevels, VerticalTopology,
};

enum NativeCommand {
    /// netCDF-C hyperslab read: one `(start,count)` pair per variable dimension.
    ReadHyperslab {
        name: String,
        starts: Vec<usize>,
        counts: Vec<usize>,
        response: mpsc::SyncSender<Result<Vec<f64>, String>>,
    },
    Shutdown,
}

/// Shared native worker handle. `Drop` sends shutdown only for the last owner.
#[derive(Debug)]
pub(super) struct NativeNetCdfWorker {
    commands: mpsc::Sender<NativeCommand>,
}

impl NativeNetCdfWorker {
    fn start(path: PathBuf) -> Result<(Arc<Self>, NetCdfIndexPartsPublic), DecodeError> {
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_path = path.clone();
        thread::Builder::new()
            .name("trajecta-netcdf-c".into())
            .spawn(move || native_worker(worker_path, receiver, ready_tx))
            .map_err(|error| DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message: format!("cannot start native netCDF worker: {error}"),
            })?;
        match ready_rx.recv() {
            Ok(Ok(parts)) => Ok((Arc::new(Self { commands }), parts)),
            Ok(Err(message)) => Err(DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message,
            }),
            Err(error) => Err(DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message: format!("native netCDF worker init channel failed: {error}"),
            }),
        }
    }

    fn read_hyperslab(
        &self,
        name: &str,
        starts: &[usize],
        counts: &[usize],
    ) -> Result<Vec<f64>, DecodeError> {
        let (response, receiver) = mpsc::sync_channel(1);
        self.commands
            .send(NativeCommand::ReadHyperslab {
                name: name.to_owned(),
                starts: starts.to_vec(),
                counts: counts.to_vec(),
                response,
            })
            .map_err(|error| DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message: format!("native netCDF worker stopped: {error}"),
            })?;
        receiver
            .recv()
            .map_err(|error| DecodeError::BackendUnavailable {
                backend: MeteorologyReaderBackend::Native,
                message: format!("native netCDF response failed: {error}"),
            })?
            .map_err(DecodeError::InvalidMetadata)
    }
}

impl Drop for NativeNetCdfWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(NativeCommand::Shutdown);
    }
}

pub(super) fn build_native_index(
    file: &Path,
    format: SourceFormat,
) -> Result<NetCdfVariableIndex, DecodeError> {
    let path = std::fs::canonicalize(file).map_err(|error| DecodeError::Io {
        path: file.to_path_buf(),
        message: error.to_string(),
    })?;
    let resolved_format = match format {
        SourceFormat::NetCdf3 => SourceFormat::NetCdf3,
        SourceFormat::NetCdf4 => SourceFormat::NetCdf4,
        SourceFormat::Grib1 | SourceFormat::Grib2 => {
            return Err(DecodeError::UnsupportedFormat);
        }
    };
    let (worker, parts) = NativeNetCdfWorker::start(path.clone())?;
    Ok(NetCdfVariableIndex {
        path,
        format: resolved_format,
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
        backend: MeteorologyReaderBackend::Native,
        rust_worker: None,
        nc4_worker: None,
        native_worker: Some(worker),
    })
}

pub(super) fn decode_native_impl(
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
    let worker = index
        .native_worker
        .as_ref()
        .ok_or_else(|| DecodeError::BackendUnavailable {
            backend: MeteorologyReaderBackend::Native,
            message: "native netCDF index has no worker handle".into(),
        })?;
    let plan = dimension_plan(index, variable)?;
    let valid_time = request
        .valid_time
        .or_else(|| index.time_coordinates.first().copied())
        .or_else(|| index.valid_times.first().copied())
        .ok_or_else(|| DecodeError::InvalidMetadata("NetCDF decode has no validity time".into()))?;
    let time_index = resolve_time_index(index, valid_time, plan.has_time)?;
    let (starts, counts) = hyperslab_for_plan(index, variable, &plan, time_index)?;
    let raw = worker.read_hyperslab(variable_name, &starts, &counts)?;
    let (values, valid) = apply_packing_and_mask(variable, &raw)?;
    // Hyperslab already pinned the time axis; finalize only reorders levels/mask.
    decode_prepared_time_slab(index, request, variable_name, variable, values, valid)
}

fn hyperslab_for_plan(
    index: &NetCdfVariableIndex,
    variable: &NetCdfVariable,
    plan: &DimensionPlan,
    time_index: Option<usize>,
) -> Result<(Vec<usize>, Vec<usize>), DecodeError> {
    let time_name = index.axes.axes.get("time").map(String::as_str);
    let vertical_name = index.axes.axes.get("vertical").map(String::as_str);
    let mut starts = Vec::with_capacity(variable.dimensions.len());
    let mut counts = Vec::with_capacity(variable.dimensions.len());
    for dim in &variable.dimensions {
        let len = index.dimensions.get(dim).copied().ok_or_else(|| {
            DecodeError::InvalidMetadata(format!(
                "native hyperslab dimension {dim} is absent from index"
            ))
        })?;
        if time_name == Some(dim.as_str()) {
            let t = time_index.ok_or_else(|| {
                DecodeError::InvalidMetadata(
                    "native hyperslab requires a resolved time index".into(),
                )
            })?;
            if t >= len {
                return Err(DecodeError::InvalidMetadata(
                    "native hyperslab time index is out of range".into(),
                ));
            }
            starts.push(t);
            counts.push(1);
        } else if vertical_name == Some(dim.as_str()) {
            // Full vertical extent for the selected time; reordering is applied after.
            if !plan.has_vertical {
                return Err(DecodeError::InvalidMetadata(
                    "vertical axis present on variable without 3D plan".into(),
                ));
            }
            starts.push(0);
            counts.push(len);
        } else {
            starts.push(0);
            counts.push(len);
        }
    }
    Ok((starts, counts))
}

fn native_worker(
    path: PathBuf,
    receiver: mpsc::Receiver<NativeCommand>,
    ready: mpsc::SyncSender<Result<NetCdfIndexPartsPublic, String>>,
) {
    if let Err(error) = disable_hdf5_auto_error_printing() {
        let _ = ready.send(Err(error));
        return;
    }
    let file = match netcdf::open(&path) {
        Ok(file) => file,
        Err(error) => {
            let _ = ready.send(Err(format!("netCDF-C open failed: {error}")));
            return;
        }
    };
    let parts = match index_parts_from_native(&file) {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(parts)).is_err() {
        return;
    }
    // Cache keyed by hyperslab identity so multi-field decode reuses the same
    // time/level window without re-reading the whole variable.
    let mut cache = BTreeMap::<(String, Vec<usize>, Vec<usize>), Vec<f64>>::new();
    while let Ok(command) = receiver.recv() {
        match command {
            NativeCommand::ReadHyperslab {
                name,
                starts,
                counts,
                response,
            } => {
                let key = (name.clone(), starts.clone(), counts.clone());
                let result = if let Some(values) = cache.get(&key) {
                    Ok(values.clone())
                } else {
                    match file
                        .variable(&name)
                        .ok_or_else(|| format!("variable {name} missing"))
                    {
                        Ok(variable) => {
                            if starts.len() != counts.len() {
                                Err(format!(
                                    "hyperslab start/count rank mismatch for {name}: {} vs {}",
                                    starts.len(),
                                    counts.len()
                                ))
                            } else {
                                let extents = starts
                                    .iter()
                                    .zip(counts.iter())
                                    .map(|(&start, &count)| netcdf::Extent::SliceCount {
                                        start,
                                        count,
                                        stride: 1,
                                    })
                                    .collect::<Vec<_>>();
                                match variable.get_values::<f64, _>(extents) {
                                    Ok(values) => {
                                        cache.insert(key, values.clone());
                                        Ok(values)
                                    }
                                    Err(error) => Err(format!("hyperslab read {name}: {error}")),
                                }
                            }
                        }
                        Err(error) => Err(error),
                    }
                };
                let _ = response.send(result);
            }
            NativeCommand::Shutdown => break,
        }
    }
}

fn disable_hdf5_auto_error_printing() -> Result<(), String> {
    trajecta_hdf5_control::disable_automatic_error_printing().map_err(|error| error.to_string())
}

fn index_parts_from_native(file: &netcdf::File) -> Result<NetCdfIndexPartsPublic, String> {
    let mut dimensions = BTreeMap::new();
    for dim in file.dimensions() {
        dimensions.insert(dim.name().to_owned(), dim.len());
    }
    let mut global_attributes = BTreeMap::new();
    for attr in file.attributes() {
        global_attributes.insert(attr.name().to_owned(), attr_as_string(&attr));
    }
    let mut variables = BTreeMap::new();
    for var in file.variables() {
        let mut attributes = BTreeMap::new();
        for attr in var.attributes() {
            attributes.insert(attr.name().to_owned(), attr_as_string(&attr));
        }
        variables.insert(
            var.name().to_owned(),
            NetCdfVariable {
                name: var.name().to_owned(),
                dimensions: var
                    .dimensions()
                    .iter()
                    .map(|dimension| dimension.name().to_owned())
                    .collect(),
                attributes,
            },
        );
    }
    let axes = resolve_cf_axes_pub(&variables).map_err(|e| format!("{e:?}"))?;
    let (valid_times, time_coordinates) = decode_time_native(file, &variables, &axes)?;
    let (source_grid, source_grid_signature) = decode_grid_native(file, &axes)?;
    let (source_vertical, source_vertical_signature, vertical_source_order) =
        decode_vertical_native(file, &variables, &axes)?;
    Ok(NetCdfIndexPartsPublic {
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

fn decode_time_native(
    file: &netcdf::File,
    variables: &BTreeMap<String, NetCdfVariable>,
    axes: &CfCoordinateResolver,
) -> Result<(Vec<Timestamp>, Vec<Timestamp>), String> {
    let Some(time_name) = axes.axes.get("time") else {
        return Ok((Vec::new(), Vec::new()));
    };
    let variable = variables
        .get(time_name)
        .ok_or_else(|| format!("time axis {time_name} missing"))?;
    let units = variable
        .attributes
        .get("units")
        .cloned()
        .ok_or_else(|| "time units missing".to_owned())?;
    let calendar = variable
        .attributes
        .get("calendar")
        .cloned()
        .unwrap_or_else(|| "standard".into());
    reject_unsupported_calendar_pub(&calendar).map_err(|e| format!("{e:?}"))?;
    let values = read_var_f64(file, time_name)?;
    let mut ordered = Vec::new();
    let mut unique = BTreeSet::new();
    for value in values {
        let ts = decode_cf_time_pub(value, &units).map_err(|e| format!("{e:?}"))?;
        ordered.push(ts);
        unique.insert(ts);
    }
    Ok((unique.into_iter().collect(), ordered))
}

fn decode_grid_native(
    file: &netcdf::File,
    axes: &CfCoordinateResolver,
) -> Result<(SourceGridGeometry, GridSignature), String> {
    let lat_name = axes
        .axes
        .get("latitude")
        .ok_or_else(|| "latitude missing".to_owned())?;
    let lon_name = axes
        .axes
        .get("longitude")
        .ok_or_else(|| "longitude missing".to_owned())?;
    let latitudes = read_var_f64(file, lat_name)?;
    let longitudes = read_var_f64(file, lon_name)?;
    if latitudes.len() < 2 || longitudes.len() < 2 {
        return Err("lat/lon too short".into());
    }
    let lat_spacing = latitudes[1] - latitudes[0];
    let lon_spacing = longitudes[1] - longitudes[0];
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

type NativeVerticalParts = (
    Option<VerticalTopology>,
    Option<VerticalSignature>,
    Vec<usize>,
);

fn decode_vertical_native(
    file: &netcdf::File,
    variables: &BTreeMap<String, NetCdfVariable>,
    axes: &CfCoordinateResolver,
) -> Result<NativeVerticalParts, String> {
    if let (Some(a_name), Some(b_name)) = (
        axes.formula_terms
            .get("a")
            .or_else(|| axes.formula_terms.get("ap")),
        axes.formula_terms.get("b"),
    ) {
        let a = read_var_f64(file, a_name)?;
        let b = read_var_f64(file, b_name)?;
        let a_half_pa = if a.iter().all(|value| value.abs() <= 2_000.0) {
            a.iter().map(|v| v * 100.0).collect::<Vec<_>>()
        } else {
            a
        };
        let full_level_count = a_half_pa.len() - 1;
        let active = if let Some(vertical_name) = axes.axes.get("vertical") {
            read_var_f64(file, vertical_name)?
                .iter()
                .map(|v| *v as u16)
                .collect::<Vec<_>>()
        } else {
            (1..=full_level_count as u16).collect()
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
    let variable = variables
        .get(vertical_name)
        .ok_or_else(|| format!("vertical {vertical_name} missing"))?;
    let units = variable
        .attributes
        .get("units")
        .map(String::as_str)
        .unwrap_or("Pa");
    let mut levels = read_var_f64(file, vertical_name)?;
    if matches!(
        units,
        "hPa" | "hpa" | "mb" | "mbar" | "millibar" | "millibars"
    ) {
        levels.iter_mut().for_each(|v| *v *= 100.0);
    }
    let mut order = (0..levels.len()).collect::<Vec<_>>();
    order.sort_by(|a, b| levels[*a].total_cmp(&levels[*b]));
    let ordered = order.iter().map(|i| levels[*i]).collect::<Vec<_>>();
    let pressure = PressureLevels::new(Arc::from(ordered)).map_err(|error| format!("{error:?}"))?;
    let mut hasher = Sha256::new();
    hasher.update((pressure.pressure_pa.len() as u64).to_be_bytes());
    for value in pressure.pressure_pa.iter() {
        hasher.update(value.to_bits().to_be_bytes());
    }
    Ok((
        Some(VerticalTopology::PressureLevels(pressure)),
        Some(VerticalSignature::PressureLevels {
            level_count: order.len(),
            levels_sha256: hex::encode(hasher.finalize()),
        }),
        order,
    ))
}

fn read_var_f64(file: &netcdf::File, name: &str) -> Result<Vec<f64>, String> {
    let variable = file
        .variable(name)
        .ok_or_else(|| format!("variable {name} missing"))?;
    variable
        .get_values::<f64, _>(..)
        .map_err(|error| error.to_string())
}

fn attr_as_string(attr: &netcdf::Attribute<'_>) -> String {
    if let Ok(value) = attr.value() {
        return match value {
            netcdf::AttributeValue::Str(text) => text,
            netcdf::AttributeValue::Strs(values) => values.join(" "),
            netcdf::AttributeValue::Double(value) => value.to_string(),
            netcdf::AttributeValue::Doubles(values) => values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            netcdf::AttributeValue::Float(value) => value.to_string(),
            netcdf::AttributeValue::Floats(values) => values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            netcdf::AttributeValue::Int(value) => value.to_string(),
            netcdf::AttributeValue::Ints(values) => values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
            other => format!("{other:?}"),
        };
    }
    String::new()
}
