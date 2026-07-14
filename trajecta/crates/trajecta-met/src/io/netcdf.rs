//! # Contract: NetCDF3/4 and HDF5-backed reader
//!
//! CF coordinates, calendars, missing masks, formula terms, and multi-file
//! assembly are compared exactly. Inconsistent logical-frame components fail;
//! this module never silently resamples or guesses coordinate roles.
//!
//! The default Rust backend uses pure-Rust classic NetCDF3 (`netcdf3`). NetCDF4
//! HDF5 containers are recognized and rejected with a structured error until the
//! pure-Rust HDF5 path is selected for a supported Profile feature set. The
//! optional `native-netcdf` feature uses netCDF-C/HDF5 without silent fallback.
//! A classic index owns one dedicated reader thread: the file is opened once,
//! full variables are cached on first use, and time/level slices are selected in
//! memory because `netcdf3` exposes no general public hyperslab API.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use netcdf3::{DataVector, FileReader};
use sha2::{Digest, Sha256};
use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::lockfile::{GridSignature, VerticalSignature};
use trajecta_case::model::time::Timestamp;

use crate::frame::{ArrayLayout, TemporalSupport};
use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, MetReader, SourceFormat, SourceGridGeometry,
    SourceIndex, SourceMetadata, detect_source_format,
};
use crate::profile::graph::GraphUnit;
use crate::vertical::{
    HybridCoefficients, HybridPressureTopology, PressureLevels, VerticalTopology,
};

/// Indexed NetCDF variable definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfVariable {
    /// Exact variable name.
    pub name: String,
    /// Ordered dimension names.
    pub dimensions: Vec<String>,
    /// Exact normalized attributes.
    pub attributes: BTreeMap<String, String>,
}

/// Immutable NetCDF variable and dimension index.
#[derive(Clone, Debug)]
pub struct NetCdfVariableIndex {
    /// Canonical source path.
    pub path: PathBuf,
    /// Container format.
    pub format: SourceFormat,
    /// Indexed variables by exact name.
    pub variables: BTreeMap<String, NetCdfVariable>,
    /// Dimension lengths by exact name.
    pub dimensions: BTreeMap<String, usize>,
    /// Exact global attributes.
    pub global_attributes: BTreeMap<String, String>,
    /// Sorted unique physical validity times.
    pub valid_times: Vec<Timestamp>,
    /// Time coordinate values in file order (for hyperslab / slice selection).
    pub time_coordinates: Vec<Timestamp>,
    /// Resolved CF axes.
    pub axes: CfCoordinateResolver,
    /// Source-level indices ordered to match the increasing-Pa topology.
    /// Empty when no pressure vertical axis is present.
    pub vertical_source_order: Vec<usize>,
    pub(super) source_grid: Option<SourceGridGeometry>,
    pub(super) source_vertical: Option<VerticalTopology>,
    pub(super) source_grid_signature: Option<GridSignature>,
    pub(super) source_vertical_signature: Option<VerticalSignature>,
    #[allow(dead_code)]
    pub(super) backend: MeteorologyReaderBackend,
    pub(super) rust_worker: Option<NetCdfReadWorker>,
    /// Pure-Rust NetCDF4 worker owned by this index (opened once, cached variables).
    pub(super) nc4_worker: Option<std::sync::Arc<super::netcdf4_rust::NetCdf4ReadWorker>>,
    /// Native netCDF-C worker owned by this index (feature `native-netcdf` only).
    #[cfg(feature = "native-netcdf")]
    pub(super) native_worker: Option<std::sync::Arc<super::netcdf_native::NativeNetCdfWorker>>,
    #[cfg(not(feature = "native-netcdf"))]
    #[allow(dead_code)]
    pub(super) native_worker: Option<()>,
}

#[derive(Clone, Debug)]
pub(super) struct NetCdfReadWorker {
    path: PathBuf,
    requests: mpsc::Sender<NetCdfWorkerRequest>,
    #[allow(dead_code)]
    metrics: Arc<NetCdfWorkerMetrics>,
}

#[derive(Debug, Default)]
struct NetCdfWorkerMetrics {
    file_opens: AtomicUsize,
    variable_reads: AtomicUsize,
    cache_hits: AtomicUsize,
}

#[derive(Debug)]
enum NetCdfWorkerRequest {
    ReadVariable {
        name: String,
        response: mpsc::SyncSender<Result<Arc<[f64]>, DecodeError>>,
    },
}

#[derive(Debug)]
pub(super) struct NetCdfIndexPartsPublic {
    pub(super) variables: BTreeMap<String, NetCdfVariable>,
    pub(super) dimensions: BTreeMap<String, usize>,
    pub(super) global_attributes: BTreeMap<String, String>,
    pub(super) valid_times: Vec<Timestamp>,
    pub(super) time_coordinates: Vec<Timestamp>,
    pub(super) axes: CfCoordinateResolver,
    pub(super) vertical_source_order: Vec<usize>,
    pub(super) source_grid: SourceGridGeometry,
    pub(super) source_vertical: Option<VerticalTopology>,
    pub(super) source_grid_signature: GridSignature,
    pub(super) source_vertical_signature: Option<VerticalSignature>,
}

impl NetCdfReadWorker {
    fn spawn(path: PathBuf) -> Result<(Self, NetCdfIndexPartsPublic), DecodeError> {
        let (request_sender, request_receiver) = mpsc::channel();
        let (initialization_sender, initialization_receiver) = mpsc::sync_channel(1);
        let metrics = Arc::new(NetCdfWorkerMetrics::default());
        let worker_metrics = Arc::clone(&metrics);
        let worker_path = path.clone();
        thread::Builder::new()
            .name("trajecta-netcdf3-reader".into())
            .spawn(move || {
                run_netcdf3_worker(
                    worker_path,
                    request_receiver,
                    initialization_sender,
                    worker_metrics,
                );
            })
            .map_err(|error| DecodeError::Io {
                path: path.clone(),
                message: format!("failed to start NetCDF3 reader worker: {error}"),
            })?;

        let parts = initialization_receiver
            .recv()
            .map_err(|_| DecodeError::Io {
                path: path.clone(),
                message: "NetCDF3 reader worker stopped during initialization".into(),
            })??;
        Ok((
            Self {
                path,
                requests: request_sender,
                metrics,
            },
            parts,
        ))
    }

    fn read_variable(&self, name: &str) -> Result<Arc<[f64]>, DecodeError> {
        let (response_sender, response_receiver) = mpsc::sync_channel(1);
        self.requests
            .send(NetCdfWorkerRequest::ReadVariable {
                name: name.to_owned(),
                response: response_sender,
            })
            .map_err(|_| DecodeError::Io {
                path: self.path.clone(),
                message: "NetCDF3 reader worker is unavailable".into(),
            })?;
        response_receiver.recv().map_err(|_| DecodeError::Io {
            path: self.path.clone(),
            message: "NetCDF3 reader worker stopped before returning a variable".into(),
        })?
    }

    #[cfg(test)]
    fn metric_snapshot(&self) -> (usize, usize, usize) {
        (
            self.metrics.file_opens.load(Ordering::Relaxed),
            self.metrics.variable_reads.load(Ordering::Relaxed),
            self.metrics.cache_hits.load(Ordering::Relaxed),
        )
    }
}

impl SourceIndex for NetCdfVariableIndex {
    fn len(&self) -> usize {
        self.variables.len()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn grid_geometry(&self) -> Option<&SourceGridGeometry> {
        self.source_grid.as_ref()
    }

    fn vertical_topology(&self) -> Option<&VerticalTopology> {
        self.source_vertical.as_ref()
    }

    fn grid_signature(&self) -> Option<&GridSignature> {
        self.source_grid_signature.as_ref()
    }

    fn vertical_signature(&self) -> Option<&VerticalSignature> {
        self.source_vertical_signature.as_ref()
    }
}

/// One logical meteorological frame assembled from exact file roles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfAssembly {
    /// Deterministic role-to-file mapping.
    pub files_by_role: BTreeMap<String, PathBuf>,
    /// Digest of all compared coordinate definitions.
    pub coordinate_signature: String,
}

impl NetCdfAssembly {
    /// Assemble one logical frame from role-tagged NetCDF files.
    ///
    /// Roles come from the exact global attribute `role`, or the sole
    /// non-coordinate data variable when unambiguous. Multiple data variables
    /// without `role` are rejected. Members must agree on grid, times,
    /// vertical topology (when present), data-variable shape summary, and a
    /// coordinate attribute digest.
    pub fn assemble(files: &[PathBuf]) -> Result<Self, DecodeError> {
        if files.is_empty() {
            return Err(DecodeError::InvalidMetadata(
                "NetCDF assembly requires at least one file".into(),
            ));
        }
        let mut files_by_role = BTreeMap::new();
        let mut signatures = BTreeMap::new();
        for file in files {
            let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
            let index = reader.build_typed_index(file)?;
            let role = assembly_role(&index, file)?;
            if files_by_role.insert(role.clone(), file.clone()).is_some() {
                return Err(DecodeError::InvalidMetadata(format!(
                    "duplicate NetCDF assembly role {role}"
                )));
            }
            let grid = index.grid_signature().ok_or_else(|| {
                DecodeError::InvalidMetadata("assembly member missing grid".into())
            })?;
            let vertical = index.vertical_signature().cloned();
            let times = index.valid_times.clone();
            let (data_shape, attr_digest) = assembly_member_payload(&index, &role)?;
            signatures.insert(
                role,
                AssemblyMemberSignature {
                    grid_sha: grid.sha256.clone(),
                    vertical,
                    times,
                    data_shape,
                    attr_digest,
                },
            );
        }
        let mut coord_hasher = Sha256::new();
        let mut reference_grid: Option<String> = None;
        let mut reference_times: Option<Vec<Timestamp>> = None;
        let mut reference_vertical: Option<VerticalSignature> = None;
        let mut reference_horizontal: Option<(usize, usize)> = None;
        let mut reference_levels: Option<usize> = None;
        let mut reference_attr: Option<String> = None;
        for (role, signature) in &signatures {
            coord_hasher.update(role.as_bytes());
            coord_hasher.update(signature.grid_sha.as_bytes());
            coord_hasher.update(signature.attr_digest.as_bytes());
            for size in &signature.data_shape {
                coord_hasher.update((*size as u64).to_be_bytes());
            }
            for time in &signature.times {
                coord_hasher.update(time.seconds_since_unix_epoch().to_be_bytes());
            }
            match &reference_grid {
                None => reference_grid = Some(signature.grid_sha.clone()),
                Some(existing) if existing != &signature.grid_sha => {
                    return Err(DecodeError::InvalidMetadata(
                        "NetCDF assembly members disagree on horizontal grid".into(),
                    ));
                }
                Some(_) => {}
            }
            match &reference_times {
                None => reference_times = Some(signature.times.clone()),
                Some(existing) if existing != &signature.times => {
                    return Err(DecodeError::InvalidMetadata(
                        "NetCDF assembly members disagree on validity times".into(),
                    ));
                }
                Some(_) => {}
            }
            if let Some(vertical) = &signature.vertical {
                coord_hasher.update(format!("{vertical:?}").as_bytes());
                match &reference_vertical {
                    None => reference_vertical = Some(vertical.clone()),
                    Some(existing) if existing != vertical => {
                        return Err(DecodeError::InvalidMetadata(
                            "NetCDF assembly members disagree on vertical topology".into(),
                        ));
                    }
                    Some(_) => {}
                }
            }
            let horizontal = assembly_horizontal_shape(&signature.data_shape)?;
            match reference_horizontal {
                None => reference_horizontal = Some(horizontal),
                Some(existing) if existing != horizontal => {
                    return Err(DecodeError::InvalidMetadata(
                        "NetCDF assembly members disagree on data variable horizontal shape".into(),
                    ));
                }
                Some(_) => {}
            }
            // Only members that carry a vertical topology contribute a level count.
            // Surface [time,y,x] files must not treat the time axis as levels.
            if signature.vertical.is_some() {
                if let Some(levels) = assembly_level_count(&signature.data_shape) {
                    match reference_levels {
                        None => reference_levels = Some(levels),
                        Some(existing) if existing != levels => {
                            return Err(DecodeError::InvalidMetadata(
                                "NetCDF assembly members disagree on vertical level count".into(),
                            ));
                        }
                        Some(_) => {}
                    }
                }
            }
            match &reference_attr {
                None => reference_attr = Some(signature.attr_digest.clone()),
                Some(existing) if existing != &signature.attr_digest => {
                    return Err(DecodeError::InvalidMetadata(
                        "NetCDF assembly members disagree on coordinate attribute summary".into(),
                    ));
                }
                Some(_) => {}
            }
        }
        Ok(Self {
            files_by_role,
            coordinate_signature: hex::encode(coord_hasher.finalize()),
        })
    }
}

#[derive(Clone, Debug)]
struct AssemblyMemberSignature {
    grid_sha: String,
    vertical: Option<VerticalSignature>,
    times: Vec<Timestamp>,
    data_shape: Vec<usize>,
    attr_digest: String,
}

fn is_coordinate_or_aux_variable(name: &str) -> bool {
    matches!(
        name,
        "time"
            | "valid_time"
            | "time_bnds"
            | "lat"
            | "latitude"
            | "lon"
            | "longitude"
            | "level"
            | "pressure_level"
            | "hybrid"
            | "ap"
            | "b"
            | "a"
            | "nhyi"
            | "hyai"
            | "hybi"
            | "number"
            | "expver"
            | "nbnds"
    )
}

fn assembly_role(index: &NetCdfVariableIndex, file: &Path) -> Result<String, DecodeError> {
    if let Some(role) = index.global_attributes.get("role") {
        let role = role.trim();
        if role.is_empty() {
            return Err(DecodeError::InvalidMetadata(format!(
                "NetCDF assembly file {} has empty role attribute",
                file.display()
            )));
        }
        return Ok(role.to_owned());
    }
    let data_vars = index
        .variables
        .keys()
        .filter(|name| !is_coordinate_or_aux_variable(name))
        .cloned()
        .collect::<Vec<_>>();
    match data_vars.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(DecodeError::InvalidMetadata(format!(
            "NetCDF assembly file {} has no role and no data variable",
            file.display()
        ))),
        many => Err(DecodeError::InvalidMetadata(format!(
            "NetCDF assembly file {} has no role and ambiguous data variables {:?}",
            file.display(),
            many
        ))),
    }
}

fn assembly_member_payload(
    index: &NetCdfVariableIndex,
    role: &str,
) -> Result<(Vec<usize>, String), DecodeError> {
    let data_name = if index.variables.contains_key(role) {
        role.to_owned()
    } else {
        index
            .variables
            .keys()
            .find(|name| !is_coordinate_or_aux_variable(name))
            .cloned()
            .ok_or_else(|| {
                DecodeError::InvalidMetadata(
                    "assembly member has no primary data variable for shape summary".into(),
                )
            })?
    };
    let variable = index
        .variables
        .get(&data_name)
        .ok_or(DecodeError::MissingField)?;
    let mut data_shape = Vec::with_capacity(variable.dimensions.len());
    for dim in &variable.dimensions {
        let len = index.dimensions.get(dim).copied().ok_or_else(|| {
            DecodeError::InvalidMetadata(format!(
                "assembly data variable {data_name} references unknown dimension {dim}"
            ))
        })?;
        data_shape.push(len);
    }
    // Digest only shared time/horizontal coordinate *units* so surface-only
    // members remain comparable to 3D pressure members. Optional CF attributes
    // (standard_name/long_name) differ across NOAA PSL pressure vs surface files
    // even when units and grid are identical; those must not break assembly.
    let mut hasher = Sha256::new();
    for axis in ["time", "latitude", "longitude"] {
        if let Some(name) = index.axes.axes.get(axis) {
            hasher.update(axis.as_bytes());
            hasher.update(name.as_bytes());
            if let Some(variable) = index.variables.get(name) {
                if let Some(units) = variable.attributes.get("units") {
                    hasher.update(b"units");
                    hasher.update(units.as_bytes());
                }
            }
        }
    }
    Ok((data_shape, hex::encode(hasher.finalize())))
}

fn assembly_horizontal_shape(shape: &[usize]) -> Result<(usize, usize), DecodeError> {
    if shape.len() < 2 {
        return Err(DecodeError::InvalidMetadata(
            "assembly data variable shape is too short for horizontal axes".into(),
        ));
    }
    Ok((shape[shape.len() - 2], shape[shape.len() - 1]))
}

fn assembly_level_count(shape: &[usize]) -> Option<usize> {
    match shape.len() {
        // [level, y, x] — only valid when the member also has vertical topology.
        3 => Some(shape[0]),
        // [time, level, y, x]
        4 => Some(shape[1]),
        _ => None,
    }
}

/// Resolved CF axis and formula-term roles.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CfCoordinateResolver {
    /// Canonical role to exact variable name.
    pub axes: BTreeMap<String, String>,
    /// Formula-term role to exact variable name.
    pub formula_terms: BTreeMap<String, String>,
}

/// Explicit NetCDF missing-value mask.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfMissingMask {
    /// True where a decoded value is valid.
    pub valid: Vec<bool>,
}

/// Decoded CF time-axis definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetCdfTimeAxis {
    /// Exact CF units string.
    pub units: String,
    /// Exact calendar name.
    pub calendar: String,
    /// Number of coordinate values.
    pub length: usize,
    /// Coordinate values in file order.
    pub values: Vec<Timestamp>,
}

/// NetCDF3/4 implementation of the format-neutral reader contract.
#[derive(Clone, Copy, Debug)]
pub struct NetCdfReader {
    backend: MeteorologyReaderBackend,
}

impl NetCdfReader {
    /// Creates a NetCDF reader for one explicitly selected implementation.
    #[must_use]
    pub const fn new(backend: MeteorologyReaderBackend) -> Self {
        Self { backend }
    }

    /// Returns the exact selected backend.
    #[must_use]
    pub const fn backend(self) -> MeteorologyReaderBackend {
        self.backend
    }
}

impl Default for NetCdfReader {
    fn default() -> Self {
        Self::new(MeteorologyReaderBackend::Native)
    }
}

impl MetReader for NetCdfReader {
    fn inspect(&self, file: &Path) -> Result<SourceMetadata, DecodeError> {
        let index = self.build_typed_index(file)?;
        Ok(metadata_from_index(&index))
    }

    fn build_index(&self, file: &Path) -> Result<Box<dyn SourceIndex>, DecodeError> {
        Ok(Box::new(self.build_typed_index(file)?))
    }

    fn decode(
        &self,
        file: &Path,
        index: &dyn SourceIndex,
        request: &DecodeRequest,
    ) -> Result<DecodedField, DecodeError> {
        let Some(index) = index.as_any().downcast_ref::<NetCdfVariableIndex>() else {
            return Err(DecodeError::InvalidMetadata(
                "NetCDF decode received a non-NetCDF source index".into(),
            ));
        };
        if index.path != canonical_path(file)? {
            return Err(DecodeError::InvalidMetadata(
                "NetCDF decode path does not match the immutable index path".into(),
            ));
        }
        match self.backend {
            MeteorologyReaderBackend::Rust => decode_rust(index, request),
            MeteorologyReaderBackend::Native => decode_native(index, request),
        }
    }
}

impl NetCdfReader {
    pub(super) fn build_typed_index(
        &self,
        file: &Path,
    ) -> Result<NetCdfVariableIndex, DecodeError> {
        let format = detect_source_format(file)?.ok_or(DecodeError::UnsupportedFormat)?;
        match self.backend {
            MeteorologyReaderBackend::Rust => match format {
                SourceFormat::NetCdf3 => build_rust_classic_index(file),
                SourceFormat::NetCdf4 => super::netcdf4_rust::build_rust_netcdf4_index(file),
                SourceFormat::Grib1 | SourceFormat::Grib2 => Err(DecodeError::UnsupportedFormat),
            },
            MeteorologyReaderBackend::Native => {
                #[cfg(feature = "native-netcdf")]
                {
                    build_native_index(file, format)
                }
                #[cfg(not(feature = "native-netcdf"))]
                {
                    let _ = format;
                    Err(DecodeError::BackendUnavailable {
                        backend: MeteorologyReaderBackend::Native,
                        message: "compile trajecta-met with feature 'native-netcdf'".into(),
                    })
                }
            }
        }
    }
}

fn build_rust_classic_index(file: &Path) -> Result<NetCdfVariableIndex, DecodeError> {
    let path = canonical_path(file)?;
    let (worker, parts) = NetCdfReadWorker::spawn(path.clone())?;

    Ok(NetCdfVariableIndex {
        path,
        format: SourceFormat::NetCdf3,
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
        rust_worker: Some(worker),
        nc4_worker: None,
        native_worker: None,
    })
}

fn run_netcdf3_worker(
    path: PathBuf,
    requests: mpsc::Receiver<NetCdfWorkerRequest>,
    initialization: mpsc::SyncSender<Result<NetCdfIndexPartsPublic, DecodeError>>,
    metrics: Arc<NetCdfWorkerMetrics>,
) {
    let mut reader = match FileReader::open(&path) {
        Ok(reader) => {
            metrics.file_opens.fetch_add(1, Ordering::Relaxed);
            reader
        }
        Err(error) => {
            let _ = initialization.send(Err(DecodeError::Io {
                path,
                message: error.to_string(),
            }));
            return;
        }
    };

    let parts = build_rust_classic_index_parts(&mut reader);
    if parts.is_err() {
        let _ = initialization.send(parts);
        return;
    }
    if initialization.send(parts).is_err() {
        return;
    }

    let mut cache = BTreeMap::<String, Arc<[f64]>>::new();
    while let Ok(request) = requests.recv() {
        match request {
            NetCdfWorkerRequest::ReadVariable { name, response } => {
                let result = if let Some(values) = cache.get(&name) {
                    metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                    Ok(Arc::clone(values))
                } else {
                    metrics.variable_reads.fetch_add(1, Ordering::Relaxed);
                    read_numeric_variable(&mut reader, &name).map(|values| {
                        let values = Arc::<[f64]>::from(values);
                        cache.insert(name, Arc::clone(&values));
                        values
                    })
                };
                let _ = response.send(result);
            }
        }
    }
}

fn build_rust_classic_index_parts(
    reader: &mut FileReader,
) -> Result<NetCdfIndexPartsPublic, DecodeError> {
    let dataset = reader.data_set();

    let mut dimensions = BTreeMap::new();
    for dim in dataset.get_dims() {
        dimensions.insert(dim.name().to_owned(), dim.size());
    }

    let mut global_attributes = BTreeMap::new();
    for attr in dataset.get_global_attrs() {
        global_attributes.insert(attr.name().to_owned(), attribute_to_string(attr));
    }

    let mut variables = BTreeMap::new();
    for var in dataset.get_vars() {
        let mut attributes = BTreeMap::new();
        for attr in var.get_attrs() {
            attributes.insert(attr.name().to_owned(), attribute_to_string(attr));
        }
        variables.insert(
            var.name().to_owned(),
            NetCdfVariable {
                name: var.name().to_owned(),
                dimensions: var
                    .get_dims()
                    .into_iter()
                    .map(|dim| dim.name().to_owned())
                    .collect(),
                attributes,
            },
        );
    }

    let axes = resolve_cf_axes_pub(&variables)?;
    let (valid_times, time_axis) = decode_time_axis(reader, &variables, &axes)?;
    let time_coordinates = time_axis
        .as_ref()
        .map(|axis| axis.values.clone())
        .unwrap_or_default();
    let (source_grid, source_grid_signature) =
        decode_horizontal_grid(reader, &variables, &axes, &dimensions)?;
    let (source_vertical, source_vertical_signature, vertical_source_order) =
        decode_vertical_topology(reader, &variables, &axes)?;

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

fn metadata_from_index(index: &NetCdfVariableIndex) -> SourceMetadata {
    let mut attributes = index.global_attributes.clone();
    attributes.insert(
        "edition".into(),
        match index.format {
            SourceFormat::NetCdf3 => "3",
            SourceFormat::NetCdf4 => "4",
            SourceFormat::Grib1 | SourceFormat::Grib2 => unreachable!(),
        }
        .into(),
    );
    if let Some(conventions) = index.global_attributes.get("Conventions") {
        attributes.insert("Conventions".into(), conventions.clone());
    }
    if index
        .global_attributes
        .get("dataset_family")
        .is_some_and(|value| value == "era5_cf_pressure_netcdf")
        || looks_like_era5_cds_pressure(index)
    {
        attributes
            .entry("dataset_family".into())
            .or_insert_with(|| "era5_cf_pressure_netcdf".into());
    }
    if index
        .global_attributes
        .values()
        .any(|value| value.to_ascii_lowercase().contains("era5"))
        && !attributes.contains_key("dataset_family")
        && index.variables.contains_key("t")
        && index.variables.contains_key("sp")
        && !index.variables.contains_key("hyai")
    {
        // Offline converted single-file pressure products may only stamp ERA5 in text attrs.
        attributes.insert("dataset_family".into(), "era5_cf_pressure_netcdf".into());
    }
    if index
        .global_attributes
        .get("dataset_family")
        .is_some_and(|value| value == "cfsr_derived_cf_multifile_netcdf")
    {
        attributes.insert(
            "dataset_family".into(),
            "cfsr_derived_cf_multifile_netcdf".into(),
        );
    }
    if index
        .global_attributes
        .get("dataset_family")
        .is_some_and(|value| value == "era5_cf_hybrid_netcdf4")
    {
        attributes.insert("dataset_family".into(), "era5_cf_hybrid_netcdf4".into());
    }
    // Official NOAA PSL NCEP R1 files are matched by exact `dataset_title` in the
    // Profile document. Do not invent a synthetic dataset_family for them here.

    let mut roles = Vec::new();
    if let Some(role) = index.global_attributes.get("role") {
        let role = role.trim();
        if !role.is_empty() {
            roles.push(role.to_owned());
        }
    } else if attributes
        .get("dataset_family")
        .is_some_and(|value| value == "era5_cf_pressure_netcdf")
    {
        roles.push(era5_pressure_role(index).into());
    } else {
        // Official multi-file products usually ship one data variable per file and
        // no role attribute; use that unique variable name as the lock role.
        let data_vars = index
            .variables
            .keys()
            .filter(|name| !is_coordinate_or_aux_variable(name))
            .cloned()
            .collect::<Vec<_>>();
        match data_vars.as_slice() {
            [only] => roles.push(only.clone()),
            _ if index.variables.values().any(|var| {
                var.attributes
                    .get("standard_name")
                    .is_some_and(|name| name.contains("eastward") || name.contains("northward"))
                    || var.name == "u"
                    || var.name == "v"
                    || var.name == "uwnd"
                    || var.name == "vwnd"
            }) =>
            {
                roles.push("analysis".into());
            }
            _ => {}
        }
    }
    roles.sort();
    roles.dedup();

    SourceMetadata {
        path: index.path.clone(),
        format: index.format,
        attributes,
        dimensions: index.dimensions.clone(),
        valid_times: index.valid_times.clone(),
        roles,
        grid: index.source_grid_signature.clone(),
        vertical: index.source_vertical_signature.clone(),
    }
}

fn looks_like_era5_cds_pressure(index: &NetCdfVariableIndex) -> bool {
    let institution = index
        .global_attributes
        .get("institution")
        .map(String::as_str)
        .unwrap_or("");
    let centre = index
        .global_attributes
        .get("GRIB_centre")
        .map(String::as_str)
        .unwrap_or("");
    let ecmwf = institution.contains("European Centre for Medium-Range Weather Forecasts")
        || centre == "ecmf";
    if !ecmwf {
        return false;
    }
    // Hybrid products carry formula coefficients; exclude them from pressure family.
    if index.variables.contains_key("hyai")
        || index.variables.contains_key("a")
        || index.axes.formula_terms.contains_key("a")
        || index.axes.formula_terms.contains_key("ap")
    {
        return false;
    }
    let has_pressure_axis = index.variables.contains_key("pressure_level")
        || index.dimensions.contains_key("pressure_level")
        || index.axes.axes.contains_key("vertical");
    let has_pressure_fields = ["t", "u", "v", "q", "w"]
        .iter()
        .any(|name| index.variables.contains_key(*name));
    let has_surface_fields =
        index.variables.contains_key("sp") || index.variables.contains_key("z");
    (has_pressure_axis && has_pressure_fields) || (has_surface_fields && !has_pressure_fields)
}

fn era5_pressure_role(index: &NetCdfVariableIndex) -> &'static str {
    let has_pressure_fields = ["t", "u", "v", "q", "w"]
        .iter()
        .any(|name| index.variables.contains_key(*name));
    let has_surface_fields =
        index.variables.contains_key("sp") || index.variables.contains_key("z");
    match (has_pressure_fields, has_surface_fields) {
        (true, true) => "analysis",
        (true, false) => "pressure",
        (false, true) => "surface",
        (false, false) => "analysis",
    }
}

/// Normalize CF/UDUNITS style unit strings into GraphUnit-parseable form.
///
/// CDS/cfgrib often emits `m s**-1`, `Pa s**-1`, `m**2 s**-2`, `kg kg**-1`.
pub(super) fn normalize_cf_unit_string(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "1".into();
    }
    // Collapse UDUNITS `**` power markers: m**2 -> m2, s**-1 -> s-1.
    let without_stars = trimmed.replace("**", "");
    let collapsed = without_stars
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed == "kg kg-1" || collapsed == "kg/kg" {
        return "1".into();
    }
    // NOAA PSL / COARDS common spellings.
    if collapsed == "degK" || collapsed == "degK." {
        return "K".into();
    }
    if collapsed.eq_ignore_ascii_case("pascals") || collapsed == "Pascal" {
        return "Pa".into();
    }
    if collapsed == "Pascal/s" || collapsed == "Pascals/s" || collapsed == "Pa/s" {
        return "Pa/s".into();
    }
    if collapsed == "m/s" || collapsed == "m s-1" || collapsed == "meters/second" {
        return if collapsed == "m/s" {
            collapsed
        } else {
            "m/s".into()
        };
    }
    collapsed
}

pub(super) fn resolve_cf_axes_pub(
    variables: &BTreeMap<String, NetCdfVariable>,
) -> Result<CfCoordinateResolver, DecodeError> {
    let mut axes = BTreeMap::new();
    let mut formula_terms = BTreeMap::new();
    for var in variables.values() {
        if let Some(axis) = var.attributes.get("axis") {
            let role = match axis.as_str() {
                "T" | "t" => "time",
                "Z" | "z" => "vertical",
                "Y" | "y" => "latitude",
                "X" | "x" => "longitude",
                other => {
                    return Err(DecodeError::InvalidMetadata(format!(
                        "unsupported CF axis attribute {other}"
                    )));
                }
            };
            if axes.insert(role.to_owned(), var.name.clone()).is_some() {
                return Err(DecodeError::InvalidMetadata(format!(
                    "duplicate CF axis role {role}"
                )));
            }
        }
        if let Some(standard_name) = var.attributes.get("standard_name") {
            let role = match standard_name.as_str() {
                "time" => Some("time"),
                "latitude" | "grid_latitude" => Some("latitude"),
                "longitude" | "grid_longitude" => Some("longitude"),
                "air_pressure" | "atmosphere_hybrid_sigma_pressure_coordinate" => Some("vertical"),
                "surface_air_pressure" => Some("surface_pressure"),
                _ => None,
            };
            if let Some(role) = role {
                axes.entry(role.to_owned())
                    .or_insert_with(|| var.name.clone());
            }
        }
        if let Some(terms) = var.attributes.get("formula_terms") {
            // CF formula_terms is whitespace-separated `key: value` pairs. Values may
            // follow the colon immediately (`a:ap`) or as the next token (`a: ap`).
            let tokens = terms.split_whitespace().collect::<Vec<_>>();
            let mut token_index = 0;
            while token_index < tokens.len() {
                let token = tokens[token_index];
                if let Some((key, value)) = token.split_once(':') {
                    let value = if value.is_empty() {
                        token_index += 1;
                        tokens.get(token_index).copied().unwrap_or("")
                    } else {
                        value
                    };
                    if key.is_empty() || value.is_empty() {
                        return Err(DecodeError::InvalidMetadata(format!(
                            "invalid CF formula_terms token near {token:?} in {terms:?}"
                        )));
                    }
                    formula_terms.insert(key.to_owned(), value.to_owned());
                    token_index += 1;
                } else {
                    return Err(DecodeError::InvalidMetadata(format!(
                        "invalid CF formula_terms segment {token:?} in {terms:?}"
                    )));
                }
            }
        }
    }
    // Fallback coordinate names used by many CF pressure products.
    for (role, candidates) in [
        ("time", &["time", "Time", "TIME"][..]),
        ("latitude", &["latitude", "lat", "Lat", "LAT"][..]),
        ("longitude", &["longitude", "lon", "Lon", "LON"][..]),
        ("vertical", &["level", "plev", "pressure", "lev"][..]),
    ] {
        if !axes.contains_key(role) {
            if let Some(name) = candidates
                .iter()
                .find(|name| variables.contains_key(**name))
            {
                axes.insert(role.to_owned(), (*name).to_owned());
            }
        }
    }
    Ok(CfCoordinateResolver {
        axes,
        formula_terms,
    })
}

fn decode_time_axis(
    reader: &mut FileReader,
    variables: &BTreeMap<String, NetCdfVariable>,
    axes: &CfCoordinateResolver,
) -> Result<(Vec<Timestamp>, Option<NetCdfTimeAxis>), DecodeError> {
    let Some(time_name) = axes.axes.get("time") else {
        return Ok((Vec::new(), None));
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
    let values = read_numeric_variable(reader, time_name)?;
    let mut ordered = Vec::with_capacity(values.len());
    let mut unique = BTreeSet::new();
    for value in &values {
        let timestamp = decode_cf_time_pub(*value, &units)?;
        ordered.push(timestamp);
        unique.insert(timestamp);
    }
    Ok((
        unique.into_iter().collect(),
        Some(NetCdfTimeAxis {
            units,
            calendar,
            length: ordered.len(),
            values: ordered,
        }),
    ))
}

pub(super) fn reject_unsupported_calendar_pub(calendar: &str) -> Result<(), DecodeError> {
    match calendar.to_ascii_lowercase().as_str() {
        "standard" | "gregorian" | "proleptic_gregorian" | "utc" => Ok(()),
        "360_day" | "noleap" | "365_day" | "all_leap" | "366_day" => {
            Err(DecodeError::InvalidMetadata(format!(
                "CF calendar {calendar} is not supported in v0 (only exact UTC-mappable calendars)"
            )))
        }
        other => Err(DecodeError::InvalidMetadata(format!(
            "unsupported CF calendar {other}"
        ))),
    }
}

pub(super) fn decode_cf_time_pub(value: f64, units: &str) -> Result<Timestamp, DecodeError> {
    // CF: "<unit> since <date-time>"
    let lower = units.to_ascii_lowercase();
    let (unit, since) = lower
        .split_once(" since ")
        .ok_or_else(|| DecodeError::InvalidMetadata(format!("invalid CF time units {units}")))?;
    let origin = parse_cf_origin(since.trim())?;
    let seconds = match unit.trim() {
        "second" | "seconds" | "sec" | "secs" | "s" => value,
        "minute" | "minutes" | "min" | "mins" => value * 60.0,
        "hour" | "hours" | "hr" | "hrs" | "h" => value * 3_600.0,
        "day" | "days" | "d" => value * 86_400.0,
        other => {
            return Err(DecodeError::InvalidMetadata(format!(
                "unsupported CF time unit {other}"
            )));
        }
    };
    if !seconds.is_finite() {
        return Err(DecodeError::InvalidMetadata(
            "CF time coordinate is not finite".into(),
        ));
    }
    let whole = seconds.trunc() as i64;
    let nanos = ((seconds.fract().abs()) * 1_000_000_000.0).round() as u32;
    let total = origin
        .checked_add(whole)
        .ok_or_else(|| DecodeError::InvalidMetadata("CF time overflow".into()))?;
    Timestamp::new(total, nanos).map_err(|error| DecodeError::InvalidMetadata(error.to_string()))
}

fn parse_cf_origin(origin: &str) -> Result<i64, DecodeError> {
    // Accept YYYY-MM-DD[ HH:MM:SS]
    let mut parts = origin.split_whitespace();
    let date = parts.next().ok_or_else(|| {
        DecodeError::InvalidMetadata(format!("CF time origin is empty: {origin}"))
    })?;
    let mut date_parts = date.split('-');
    let year: i32 = parse_component(date_parts.next(), "year")?;
    let month: u32 = parse_component(date_parts.next(), "month")?;
    let day: u32 = parse_component(date_parts.next(), "day")?;
    let mut hour = 0_u32;
    let mut minute = 0_u32;
    let mut second = 0_u32;
    if let Some(time) = parts.next() {
        let mut time_parts = time.split(':');
        hour = parse_component(time_parts.next(), "hour")?;
        minute = parse_component(time_parts.next(), "minute").unwrap_or(0);
        second = parse_component(time_parts.next(), "second").unwrap_or(0);
    }
    let days = days_from_civil(year, month, day).ok_or_else(|| {
        DecodeError::InvalidMetadata(format!("CF time origin date is invalid: {origin}"))
    })?;
    Ok(days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))
}

fn parse_component<T: std::str::FromStr>(
    value: Option<&str>,
    label: &str,
) -> Result<T, DecodeError> {
    value
        .ok_or_else(|| DecodeError::InvalidMetadata(format!("CF time origin missing {label}")))?
        .parse()
        .map_err(|_| DecodeError::InvalidMetadata(format!("CF time origin has invalid {label}")))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400) as u32;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(i64::from(era) * 146_097 + i64::from(doe) - 719_468)
}

fn decode_horizontal_grid(
    reader: &mut FileReader,
    _variables: &BTreeMap<String, NetCdfVariable>,
    axes: &CfCoordinateResolver,
    _dimensions: &BTreeMap<String, usize>,
) -> Result<(SourceGridGeometry, GridSignature), DecodeError> {
    let lat_name = axes.axes.get("latitude").ok_or_else(|| {
        DecodeError::InvalidMetadata("NetCDF source has no CF latitude axis".into())
    })?;
    let lon_name = axes.axes.get("longitude").ok_or_else(|| {
        DecodeError::InvalidMetadata("NetCDF source has no CF longitude axis".into())
    })?;
    let latitudes = read_numeric_variable(reader, lat_name)?;
    let longitudes = read_numeric_variable(reader, lon_name)?;
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

fn regular_spacing(values: &[f64]) -> Result<f64, DecodeError> {
    let spacing = values[1] - values[0];
    if !spacing.is_finite() || spacing == 0.0 {
        return Err(DecodeError::InvalidMetadata(
            "coordinate axis spacing is not a non-zero finite step".into(),
        ));
    }
    for window in values.windows(2) {
        if (window[1] - window[0] - spacing).abs() > 1.0e-6 * spacing.abs().max(1.0) {
            return Err(DecodeError::InvalidMetadata(
                "v0 NetCDF reader requires a regular latitude-longitude grid".into(),
            ));
        }
    }
    Ok(spacing)
}

type VerticalDecodeOutcome = (
    Option<VerticalTopology>,
    Option<VerticalSignature>,
    Vec<usize>,
);

fn decode_vertical_topology(
    reader: &mut FileReader,
    variables: &BTreeMap<String, NetCdfVariable>,
    axes: &CfCoordinateResolver,
) -> Result<VerticalDecodeOutcome, DecodeError> {
    if let (Some(a_name), Some(b_name)) = (
        axes.formula_terms
            .get("a")
            .or_else(|| axes.formula_terms.get("ap")),
        axes.formula_terms.get("b"),
    ) {
        let a = read_numeric_variable(reader, a_name)?;
        let b = read_numeric_variable(reader, b_name)?;
        if a.len() != b.len() || a.len() < 2 {
            return Err(DecodeError::InvalidMetadata(
                "hybrid formula_terms A/B coefficient lengths are inconsistent".into(),
            ));
        }
        // Prefer Pa; if coefficients look like hPa interfaces, scale.
        let a_half_pa = if a.iter().all(|value| value.abs() <= 2_000.0) {
            a.iter().map(|value| value * 100.0).collect::<Vec<_>>()
        } else {
            a
        };
        let full_level_count = a_half_pa.len() - 1;
        let active = (1_u16..=u16::try_from(full_level_count).map_err(|_| {
            DecodeError::InvalidMetadata("hybrid full level count exceeds u16".into())
        })?)
            .collect::<Vec<_>>();
        let topology = VerticalTopology::HybridPressure(HybridPressureTopology {
            coefficients: HybridCoefficients {
                a_half_pa: Arc::from(a_half_pa.clone()),
                b_half: Arc::from(b),
            },
            active_full_levels: Arc::from(active),
        });
        let mut hasher = Sha256::new();
        hasher.update((a_half_pa.len() as u64).to_be_bytes());
        for value in &a_half_pa {
            hasher.update(value.to_bits().to_be_bytes());
        }
        let signature = VerticalSignature::HybridPressure {
            full_level_count,
            coefficients_sha256: hex::encode(hasher.finalize()),
        };
        return Ok((Some(topology), Some(signature), Vec::new()));
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
    let mut levels = read_numeric_variable(reader, vertical_name)?;
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
    // Build a stable source->topology permutation so values and masks reorder together.
    let mut order = (0..pressure_pa.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| pressure_pa[*left].total_cmp(&pressure_pa[*right]));
    let ordered = order
        .iter()
        .map(|source| pressure_pa[*source])
        .collect::<Vec<_>>();
    if ordered.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err(DecodeError::InvalidMetadata(
            "pressure levels are not strictly monotonic after ordering".into(),
        ));
    }
    finalize_pressure_topology(ordered, order)
}

fn finalize_pressure_topology(
    pressure_pa: Vec<f64>,
    vertical_source_order: Vec<usize>,
) -> Result<VerticalDecodeOutcome, DecodeError> {
    let mut hasher = Sha256::new();
    hasher.update((pressure_pa.len() as u64).to_be_bytes());
    for value in &pressure_pa {
        hasher.update(value.to_bits().to_be_bytes());
    }
    let signature = VerticalSignature::PressureLevels {
        level_count: pressure_pa.len(),
        levels_sha256: hex::encode(hasher.finalize()),
    };
    let levels = PressureLevels::new(Arc::from(pressure_pa))
        .map_err(|error| DecodeError::InvalidMetadata(error.to_string()))?;
    Ok((
        Some(VerticalTopology::PressureLevels(levels)),
        Some(signature),
        vertical_source_order,
    ))
}

fn decode_rust(
    index: &NetCdfVariableIndex,
    request: &DecodeRequest,
) -> Result<DecodedField, DecodeError> {
    if index.format == SourceFormat::NetCdf4 {
        return super::netcdf4_rust::decode_rust_netcdf4(index, request);
    }
    let variable_name = request
        .source_identity
        .iter()
        .find_map(|(key, value)| (key == "variable" || key == "name").then_some(value.as_str()))
        .ok_or(DecodeError::MissingField)?;
    let variable = index
        .variables
        .get(variable_name)
        .ok_or(DecodeError::MissingField)?;
    let plan = dimension_plan(index, variable)?;
    let valid_time = request
        .valid_time
        .or_else(|| index.time_coordinates.first().copied())
        .or_else(|| index.valid_times.first().copied())
        .ok_or_else(|| DecodeError::InvalidMetadata("NetCDF decode has no validity time".into()))?;
    let time_index = resolve_time_index(index, valid_time, plan.has_time)?;

    // The index owns one dedicated worker because netcdf3::FileReader is not Send.
    // Classic NetCDF3 has no public general hyperslab API, so each variable is
    // read in full only on first use, cached, then sliced by time/level in memory.
    let worker = index.rust_worker.as_ref().ok_or_else(|| {
        DecodeError::InvalidMetadata("Rust NetCDF3 index has no reader worker".into())
    })?;
    let raw = worker.read_variable(variable_name)?;
    let (packed, packed_valid) = apply_packing_and_mask(variable, raw.as_ref())?;
    let (values, valid, layout) =
        extract_field_slab(index, &plan, time_index, &packed, &packed_valid, false)?;

    let unit = normalize_cf_unit_string(
        &variable
            .attributes
            .get("units")
            .cloned()
            .or_else(|| {
                request
                    .source_identity
                    .iter()
                    .find_map(|(key, value)| (key == "unit").then(|| value.clone()))
            })
            .unwrap_or_else(|| "1".into()),
    );
    let mut source_identity = request.source_identity.clone();
    source_identity.sort();
    Ok(DecodedField {
        values: Arc::from(values),
        valid: Arc::from(valid),
        source_unit: GraphUnit::parse(&unit)
            .map_err(|error| DecodeError::InvalidMetadata(error.to_string()))?,
        layout,
        temporal: TemporalSupport::Instantaneous { valid_time },
        source_identity,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum VariableKind {
    Horizontal2D,
    Full3D,
}

#[derive(Clone, Debug)]
pub(super) struct DimensionPlan {
    pub kind: VariableKind,
    pub has_time: bool,
    pub has_vertical: bool,
    pub ny: usize,
    pub nx: usize,
    pub levels: usize,
}

/// Shared finalize path for full-variable (pre-hyperslab) buffers.
/// Kept for classic NetCDF3 unit tests / future codecs; NetCDF4 pure-Rust and
/// native paths use [`decode_prepared_time_slab`] after time hyperslabs.
#[allow(dead_code)]
pub(super) fn decode_prepared_field(
    index: &NetCdfVariableIndex,
    request: &DecodeRequest,
    variable_name: &str,
    variable: &NetCdfVariable,
    packed: Vec<f64>,
    packed_valid: Vec<bool>,
) -> Result<DecodedField, DecodeError> {
    let _ = variable_name;
    let plan = dimension_plan(index, variable)?;
    let valid_time = request
        .valid_time
        .or_else(|| index.time_coordinates.first().copied())
        .or_else(|| index.valid_times.first().copied())
        .ok_or_else(|| DecodeError::InvalidMetadata("NetCDF decode has no validity time".into()))?;
    let time_index = resolve_time_index(index, valid_time, plan.has_time)?;
    let (values, valid, layout) =
        extract_field_slab(index, &plan, time_index, &packed, &packed_valid, false)?;
    let unit = normalize_cf_unit_string(
        &variable
            .attributes
            .get("units")
            .cloned()
            .or_else(|| {
                request
                    .source_identity
                    .iter()
                    .find_map(|(key, value)| (key == "unit").then(|| value.clone()))
            })
            .unwrap_or_else(|| "1".into()),
    );
    let mut source_identity = request.source_identity.clone();
    source_identity.sort();
    Ok(DecodedField {
        values: Arc::from(values),
        valid: Arc::from(valid),
        source_unit: GraphUnit::parse(&unit)
            .map_err(|error| DecodeError::InvalidMetadata(error.to_string()))?,
        layout,
        temporal: TemporalSupport::Instantaneous { valid_time },
        source_identity,
    })
}

/// Finalize a decoded field from a slab that already selected the requested time
/// via native hyperslab (so the buffer contains a single time).
#[cfg_attr(not(feature = "native-netcdf"), allow(dead_code))]
pub(super) fn decode_prepared_time_slab(
    index: &NetCdfVariableIndex,
    request: &DecodeRequest,
    variable_name: &str,
    variable: &NetCdfVariable,
    packed: Vec<f64>,
    packed_valid: Vec<bool>,
) -> Result<DecodedField, DecodeError> {
    let _ = variable_name;
    let plan = dimension_plan(index, variable)?;
    let valid_time = request
        .valid_time
        .or_else(|| index.time_coordinates.first().copied())
        .or_else(|| index.valid_times.first().copied())
        .ok_or_else(|| DecodeError::InvalidMetadata("NetCDF decode has no validity time".into()))?;
    let time_index = resolve_time_index(index, valid_time, plan.has_time)?;
    let (values, valid, layout) =
        extract_field_slab(index, &plan, time_index, &packed, &packed_valid, true)?;
    let unit = normalize_cf_unit_string(
        &variable
            .attributes
            .get("units")
            .cloned()
            .or_else(|| {
                request
                    .source_identity
                    .iter()
                    .find_map(|(key, value)| (key == "unit").then(|| value.clone()))
            })
            .unwrap_or_else(|| "1".into()),
    );
    let mut source_identity = request.source_identity.clone();
    source_identity.sort();
    Ok(DecodedField {
        values: Arc::from(values),
        valid: Arc::from(valid),
        source_unit: GraphUnit::parse(&unit)
            .map_err(|error| DecodeError::InvalidMetadata(error.to_string()))?,
        layout,
        temporal: TemporalSupport::Instantaneous { valid_time },
        source_identity,
    })
}

#[cfg_attr(not(feature = "native-netcdf"), allow(dead_code))]
pub(super) fn dimension_plan(
    index: &NetCdfVariableIndex,
    variable: &NetCdfVariable,
) -> Result<DimensionPlan, DecodeError> {
    let grid = index.source_grid.as_ref().ok_or_else(|| {
        DecodeError::InvalidMetadata("NetCDF index has no horizontal grid geometry".into())
    })?;
    let time_name = index.axes.axes.get("time").map(String::as_str);
    let vertical_name = index.axes.axes.get("vertical").map(String::as_str);
    let y_name = index
        .axes
        .axes
        .get("y")
        .or_else(|| index.axes.axes.get("latitude"))
        .map(String::as_str);
    let x_name = index
        .axes
        .axes
        .get("x")
        .or_else(|| index.axes.axes.get("longitude"))
        .map(String::as_str);
    let dims = variable
        .dimensions
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    let matches = |expected: &[&str]| -> bool {
        dims.len() == expected.len()
            && dims
                .iter()
                .zip(expected.iter())
                .all(|(got, want)| Some(*got) == Some(*want))
    };

    let y = y_name.ok_or_else(|| {
        DecodeError::InvalidMetadata("NetCDF horizontal Y axis is unresolved".into())
    })?;
    let x = x_name.ok_or_else(|| {
        DecodeError::InvalidMetadata("NetCDF horizontal X axis is unresolved".into())
    })?;

    // Exact CF dimension-order contracts accepted in v0.
    if let Some(time) = time_name {
        if let Some(vertical) = vertical_name {
            if matches(&[time, vertical, y, x]) {
                return Ok(DimensionPlan {
                    kind: VariableKind::Full3D,
                    has_time: true,
                    has_vertical: true,
                    ny: grid.ny,
                    nx: grid.nx,
                    levels: vertical_level_count(index)?,
                });
            }
        }
        if matches(&[time, y, x]) {
            return Ok(DimensionPlan {
                kind: VariableKind::Horizontal2D,
                has_time: true,
                has_vertical: false,
                ny: grid.ny,
                nx: grid.nx,
                levels: 1,
            });
        }
    }
    if let Some(vertical) = vertical_name {
        if matches(&[vertical, y, x]) {
            return Ok(DimensionPlan {
                kind: VariableKind::Full3D,
                has_time: false,
                has_vertical: true,
                ny: grid.ny,
                nx: grid.nx,
                levels: vertical_level_count(index)?,
            });
        }
    }
    if matches(&[y, x]) {
        return Ok(DimensionPlan {
            kind: VariableKind::Horizontal2D,
            has_time: false,
            has_vertical: false,
            ny: grid.ny,
            nx: grid.nx,
            levels: 1,
        });
    }

    Err(DecodeError::InvalidMetadata(format!(
        "unsupported NetCDF dimension order {:?} for variable {}; expected CF (time?, level?, y, x)",
        variable.dimensions, variable.name
    )))
}

fn vertical_level_count(index: &NetCdfVariableIndex) -> Result<usize, DecodeError> {
    match index.source_vertical.as_ref() {
        Some(VerticalTopology::PressureLevels(levels)) => Ok(levels.pressure_pa.len()),
        Some(VerticalTopology::HybridPressure(topology)) => Ok(topology.active_full_levels.len()),
        None => Err(DecodeError::InvalidMetadata(
            "vertical dimension present without vertical topology".into(),
        )),
    }
}

#[cfg_attr(not(feature = "native-netcdf"), allow(dead_code))]
pub(super) fn resolve_time_index(
    index: &NetCdfVariableIndex,
    valid_time: Timestamp,
    has_time: bool,
) -> Result<Option<usize>, DecodeError> {
    if !has_time {
        return Ok(None);
    }
    let matches = index
        .time_coordinates
        .iter()
        .enumerate()
        .filter(|(_, time)| **time == valid_time)
        .map(|(idx, _)| idx)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [only] => Ok(Some(*only)),
        [] => Err(DecodeError::InvalidMetadata(
            "requested valid_time is absent from the NetCDF time axis".into(),
        )),
        _ => Err(DecodeError::AmbiguousField),
    }
}

fn extract_field_slab(
    index: &NetCdfVariableIndex,
    plan: &DimensionPlan,
    time_index: Option<usize>,
    values: &[f64],
    valid: &[bool],
    time_already_selected: bool,
) -> Result<(Vec<f64>, Vec<bool>, ArrayLayout), DecodeError> {
    let horizontal = plan
        .ny
        .checked_mul(plan.nx)
        .ok_or_else(|| DecodeError::InvalidMetadata("horizontal size overflow".into()))?;
    let source_levels = if plan.has_vertical { plan.levels } else { 1 };
    let times = if plan.has_time && !time_already_selected {
        index.time_coordinates.len().max(1)
    } else {
        1
    };
    let expected = times
        .checked_mul(source_levels)
        .and_then(|count| count.checked_mul(horizontal))
        .ok_or_else(|| DecodeError::InvalidMetadata("variable size overflow".into()))?;
    if values.len() != expected || valid.len() != expected {
        return Err(DecodeError::InvalidMetadata(format!(
            "NetCDF variable size {} disagrees with dimension plan {expected}",
            values.len()
        )));
    }

    let t = if time_already_selected {
        0
    } else {
        time_index.unwrap_or(0)
    };
    if plan.has_time && !time_already_selected && t >= times {
        return Err(DecodeError::InvalidMetadata(
            "time index is outside the NetCDF time axis".into(),
        ));
    }

    let mut out_values = Vec::new();
    let mut out_valid = Vec::new();
    match plan.kind {
        VariableKind::Horizontal2D => {
            let start = t * horizontal;
            let end = start + horizontal;
            out_values.extend_from_slice(&values[start..end]);
            out_valid.extend_from_slice(&valid[start..end]);
            Ok((
                out_values,
                out_valid,
                ArrayLayout::Horizontal2D {
                    ny: plan.ny,
                    nx: plan.nx,
                },
            ))
        }
        VariableKind::Full3D => {
            let order = if index.vertical_source_order.is_empty() {
                (0..source_levels).collect::<Vec<_>>()
            } else if index.vertical_source_order.len() == source_levels {
                index.vertical_source_order.clone()
            } else {
                return Err(DecodeError::InvalidMetadata(
                    "vertical source order length disagrees with level count".into(),
                ));
            };
            out_values.reserve(source_levels * horizontal);
            out_valid.reserve(source_levels * horizontal);
            for source_level in order {
                let start = ((t * source_levels) + source_level) * horizontal;
                let end = start + horizontal;
                out_values.extend_from_slice(&values[start..end]);
                out_valid.extend_from_slice(&valid[start..end]);
            }
            Ok((
                out_values,
                out_valid,
                ArrayLayout::Full3D {
                    levels: source_levels,
                    ny: plan.ny,
                    nx: plan.nx,
                },
            ))
        }
    }
}

pub(super) fn apply_packing_and_mask(
    variable: &NetCdfVariable,
    raw: &[f64],
) -> Result<(Vec<f64>, Vec<bool>), DecodeError> {
    let fill = attribute_f64(&variable.attributes, "_FillValue")
        .or_else(|| attribute_f64(&variable.attributes, "missing_value"));
    let valid_min = attribute_f64(&variable.attributes, "valid_min");
    let valid_max = attribute_f64(&variable.attributes, "valid_max");
    let (range_min, range_max) = attribute_range(&variable.attributes, "valid_range");
    let scale = attribute_f64(&variable.attributes, "scale_factor").unwrap_or(1.0);
    let offset = attribute_f64(&variable.attributes, "add_offset").unwrap_or(0.0);
    let mut values = Vec::with_capacity(raw.len());
    let mut valid = Vec::with_capacity(raw.len());
    for &source in raw {
        let mut is_valid = source.is_finite();
        if let Some(fill) = fill {
            if (source - fill).abs() <= 0.0 || source == fill {
                is_valid = false;
            }
        }
        if let Some(min) = valid_min.or(range_min) {
            if source < min {
                is_valid = false;
            }
        }
        if let Some(max) = valid_max.or(range_max) {
            if source > max {
                is_valid = false;
            }
        }
        if is_valid {
            let unpacked = source * scale + offset;
            if !unpacked.is_finite() {
                return Err(DecodeError::InvalidMetadata(
                    "NetCDF unpacked value is not finite".into(),
                ));
            }
            values.push(unpacked);
            valid.push(true);
        } else {
            values.push(0.0);
            valid.push(false);
        }
    }
    Ok((values, valid))
}

pub(super) fn attribute_f64(attributes: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    attributes.get(key)?.parse().ok()
}

pub(super) fn attribute_range(
    attributes: &BTreeMap<String, String>,
    key: &str,
) -> (Option<f64>, Option<f64>) {
    let Some(raw) = attributes.get(key) else {
        return (None, None);
    };
    let mut parts = raw
        .split([',', ' '])
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<f64>().ok());
    (parts.next(), parts.next())
}

fn read_numeric_variable(reader: &mut FileReader, name: &str) -> Result<Vec<f64>, DecodeError> {
    let data = reader.read_var(name).map_err(|error| DecodeError::Io {
        path: PathBuf::from(name),
        message: error.to_string(),
    })?;
    data_vector_to_f64(data)
}

fn data_vector_to_f64(data: DataVector) -> Result<Vec<f64>, DecodeError> {
    match data {
        DataVector::F64(values) => Ok(values),
        DataVector::F32(values) => Ok(values.into_iter().map(f64::from).collect()),
        DataVector::I32(values) => Ok(values.into_iter().map(f64::from).collect()),
        DataVector::I16(values) => Ok(values.into_iter().map(f64::from).collect()),
        DataVector::I8(values) => Ok(values.into_iter().map(f64::from).collect()),
        DataVector::U8(values) => Ok(values.into_iter().map(f64::from).collect()),
    }
}

fn attribute_to_string(attr: &netcdf3::Attribute) -> String {
    if let Some(text) = attr.get_as_string() {
        return text;
    }
    if let Some(values) = attr.get_f64() {
        return values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(values) = attr.get_f32() {
        return values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(values) = attr.get_i32() {
        return values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(values) = attr.get_i16() {
        return values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(values) = attr.get_i8() {
        return values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(values) = attr.get_u8() {
        return String::from_utf8_lossy(values).into_owned();
    }
    String::new()
}

fn canonical_path(path: &Path) -> Result<PathBuf, DecodeError> {
    std::fs::canonicalize(path).map_err(|error| DecodeError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn decode_native(
    index: &NetCdfVariableIndex,
    request: &DecodeRequest,
) -> Result<DecodedField, DecodeError> {
    #[cfg(feature = "native-netcdf")]
    {
        decode_native_impl(index, request)
    }
    #[cfg(not(feature = "native-netcdf"))]
    {
        let _ = (index, request);
        Err(DecodeError::BackendUnavailable {
            backend: MeteorologyReaderBackend::Native,
            message: "compile trajecta-met with feature 'native-netcdf'".into(),
        })
    }
}

#[cfg(feature = "native-netcdf")]
fn build_native_index(
    file: &Path,
    format: SourceFormat,
) -> Result<NetCdfVariableIndex, DecodeError> {
    super::netcdf_native::build_native_index(file, format)
}

#[cfg(feature = "native-netcdf")]
fn decode_native_impl(
    index: &NetCdfVariableIndex,
    request: &DecodeRequest,
) -> Result<DecodedField, DecodeError> {
    super::netcdf_native::decode_native_impl(index, request)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use netcdf3::{DataSet, DataType, FileWriter, Version};

    #[test]
    fn classic_netcdf3_roundtrip_inspect_and_decode() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pressure.nc");
        write_pressure_fixture(&path, /*decreasing_levels=*/ false, /*times=*/ 1);

        let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let meta = reader.inspect(&path).unwrap();
        assert_eq!(meta.format, SourceFormat::NetCdf3);
        assert_eq!(meta.valid_times.len(), 1);
        assert_eq!(meta.dimensions.get("lat"), Some(&2));
        assert_eq!(meta.dimensions.get("lon"), Some(&3));

        let index = reader.build_index(&path).unwrap();
        let decoded = reader
            .decode(
                &path,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), "t".into())],
                    valid_time: None,
                },
            )
            .unwrap();
        assert_eq!(
            decoded.layout,
            ArrayLayout::Full3D {
                levels: 2,
                ny: 2,
                nx: 3
            }
        );
        assert_eq!(decoded.values.len(), 12);
        assert!(decoded.valid.iter().all(|value| *value));
        // Source levels already increasing: first slab is 500 hPa values.
        assert_eq!(decoded.values[0], 250.0);
    }

    #[test]
    fn decreasing_pressure_levels_reorder_values_and_masks_together() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pressure_desc.nc");
        write_pressure_fixture(&path, /*decreasing_levels=*/ true, /*times=*/ 1);

        let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let index = reader.build_index(&path).unwrap();
        let concrete = index
            .as_any()
            .downcast_ref::<NetCdfVariableIndex>()
            .unwrap();
        let VerticalTopology::PressureLevels(levels) = concrete.vertical_topology().unwrap() else {
            panic!("expected pressure topology");
        };
        assert_eq!(levels.pressure_pa.as_ref(), &[50_000.0, 100_000.0]);
        assert_eq!(concrete.vertical_source_order, vec![1, 0]);

        let decoded = reader
            .decode(
                &path,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), "t".into())],
                    valid_time: None,
                },
            )
            .unwrap();
        // Source order was [1000 hPa slab, 500 hPa slab]; topology is increasing Pa,
        // so 500 hPa slab (source index 1) comes first after reorder.
        assert_eq!(decoded.values[0], 270.0);
        assert_eq!(decoded.values[6], 250.0);
    }

    #[test]
    fn fill_mask_follows_the_same_pressure_level_permutation_as_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pressure_desc_fill.nc");
        write_pressure_fixture_with_fill(&path, true, 1, true);

        let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let index = reader.build_index(&path).unwrap();
        let decoded = reader
            .decode(
                &path,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), "t".into())],
                    valid_time: None,
                },
            )
            .unwrap();

        // The fill was source level index 1 / element 0. That 500 hPa level is
        // moved to canonical output level index 0 together with its mask.
        assert_eq!(decoded.values[0], 0.0);
        assert!(!decoded.valid[0]);
        assert_eq!(decoded.values[6], 250.0);
        assert!(decoded.valid[6]);
    }

    #[test]
    fn surface_variable_is_horizontal2d_not_pressure3d() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("surface.nc");
        write_pressure_fixture(&path, false, 1);

        let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let index = reader.build_index(&path).unwrap();
        let decoded = reader
            .decode(
                &path,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), "sp".into())],
                    valid_time: None,
                },
            )
            .unwrap();
        assert_eq!(decoded.layout, ArrayLayout::Horizontal2D { ny: 2, nx: 3 });
        assert_eq!(decoded.values.len(), 6);
        assert_eq!(decoded.values[0], 100_000.0);
    }

    #[test]
    fn multi_time_selects_requested_slice() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("multi_time.nc");
        write_pressure_fixture(&path, false, 2);

        let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let index = reader.build_index(&path).unwrap();
        let concrete = index
            .as_any()
            .downcast_ref::<NetCdfVariableIndex>()
            .unwrap();
        assert_eq!(concrete.time_coordinates.len(), 2);
        let t1 = concrete.time_coordinates[1];

        let decoded = reader
            .decode(
                &path,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), "sp".into())],
                    valid_time: Some(t1),
                },
            )
            .unwrap();
        assert_eq!(decoded.values[0], 101_000.0);
        assert_eq!(
            decoded.temporal,
            TemporalSupport::Instantaneous { valid_time: t1 }
        );
    }

    #[test]
    fn missing_requested_time_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing_time.nc");
        write_pressure_fixture(&path, false, 2);

        let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let index = reader.build_index(&path).unwrap();
        let concrete = index
            .as_any()
            .downcast_ref::<NetCdfVariableIndex>()
            .unwrap();
        let requested = Timestamp::new(
            concrete.time_coordinates[1].seconds_since_unix_epoch() + 1,
            0,
        )
        .unwrap();
        let error = reader
            .decode(
                &path,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), "sp".into())],
                    valid_time: Some(requested),
                },
            )
            .unwrap_err();
        assert!(
            matches!(error, DecodeError::InvalidMetadata(message) if message.contains("absent"))
        );
    }

    #[test]
    fn netcdf3_index_keeps_one_reader_and_caches_full_variables() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker_cache.nc");
        write_pressure_fixture(&path, false, 1);

        let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let index = reader.build_index(&path).unwrap();
        let concrete = index
            .as_any()
            .downcast_ref::<NetCdfVariableIndex>()
            .unwrap();
        let worker = concrete.rust_worker.as_ref().unwrap();
        assert_eq!(worker.metric_snapshot(), (1, 0, 0));

        let request = DecodeRequest {
            source_identity: vec![("variable".into(), "sp".into())],
            valid_time: None,
        };
        reader.decode(&path, index.as_ref(), &request).unwrap();
        reader.decode(&path, index.as_ref(), &request).unwrap();
        assert_eq!(worker.metric_snapshot(), (1, 1, 1));
    }

    #[test]
    fn formula_terms_parses_spaced_key_value_pairs() {
        let terms = "a: ap b: b ps: ps";
        let mut variables = BTreeMap::new();
        variables.insert(
            "lev".into(),
            NetCdfVariable {
                name: "lev".into(),
                dimensions: vec!["lev".into()],
                attributes: BTreeMap::from([("formula_terms".into(), terms.into())]),
            },
        );
        let axes = resolve_cf_axes_pub(&variables).unwrap();
        assert_eq!(axes.formula_terms.get("a").map(String::as_str), Some("ap"));
        assert_eq!(axes.formula_terms.get("b").map(String::as_str), Some("b"));
        assert_eq!(axes.formula_terms.get("ps").map(String::as_str), Some("ps"));
    }

    /// Without the `native-netcdf` feature the Native backend must hard-fail
    /// instead of borrowing the pure-Rust path.
    #[cfg(not(feature = "native-netcdf"))]
    #[test]
    fn native_backend_does_not_silently_use_rust_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pressure.nc");
        write_pressure_fixture(&path, false, 1);
        let reader = NetCdfReader::new(MeteorologyReaderBackend::Native);
        let error = match reader.build_index(&path) {
            Ok(_) => panic!("native backend must not succeed without a linked worker"),
            Err(error) => error,
        };
        match error {
            DecodeError::BackendUnavailable { backend, message } => {
                assert_eq!(backend, MeteorologyReaderBackend::Native);
                assert!(
                    message.contains("native-netcdf") || message.contains("silent"),
                    "{message}"
                );
            }
            other => panic!("expected BackendUnavailable, got {other:?}"),
        }
    }

    /// With the feature linked, Native must open and decode through the C worker.
    #[cfg(feature = "native-netcdf")]
    #[test]
    fn native_backend_uses_linked_worker_not_rust_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pressure.nc");
        write_pressure_fixture(&path, false, 1);
        let reader = NetCdfReader::new(MeteorologyReaderBackend::Native);
        let index = reader
            .build_index(&path)
            .expect("native backend should open with linked netCDF-C worker");
        let field = reader
            .decode(
                &path,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), "t".into())],
                    valid_time: None,
                },
            )
            .expect("native decode");
        assert!(!field.values.is_empty());
    }

    #[test]
    fn assembly_rejects_ambiguous_multi_variable_without_role() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("multi_var.nc");
        write_pressure_fixture(&path, false, 1);
        let error = NetCdfAssembly::assemble(&[path]).unwrap_err();
        assert!(
            format!("{error:?}").contains("ambiguous"),
            "expected ambiguous multi-variable error, got {error:?}"
        );
    }

    #[test]
    fn assembly_rejects_grid_time_and_level_drift() {
        let directory = tempfile::tempdir().unwrap();
        let a = directory.path().join("a.nc");
        let grid_b = directory.path().join("grid_b.nc");
        let time_b = directory.path().join("time_b.nc");
        let level_b = directory.path().join("level_b.nc");
        write_role_member_fixture(&a, "t", 10.0, 2.5, 1, &[500.0, 1000.0]);
        write_role_member_fixture(&grid_b, "u", 10.0, 5.0, 1, &[500.0, 1000.0]);
        write_role_member_fixture(&time_b, "u", 10.0, 2.5, 2, &[500.0, 1000.0]);
        write_role_member_fixture(&level_b, "u", 10.0, 2.5, 1, &[250.0, 500.0, 1000.0]);

        let grid_err = NetCdfAssembly::assemble(&[a.clone(), grid_b]).unwrap_err();
        assert!(
            format!("{grid_err:?}").contains("horizontal grid")
                || format!("{grid_err:?}").contains("horizontal shape"),
            "{grid_err:?}"
        );

        let time_err = NetCdfAssembly::assemble(&[a.clone(), time_b]).unwrap_err();
        assert!(
            format!("{time_err:?}").contains("validity times"),
            "{time_err:?}"
        );

        let level_err = NetCdfAssembly::assemble(&[a, level_b]).unwrap_err();
        assert!(
            format!("{level_err:?}").contains("vertical")
                || format!("{level_err:?}").contains("level"),
            "{level_err:?}"
        );
    }

    #[test]
    fn assembly_accepts_unique_role_members() {
        let directory = tempfile::tempdir().unwrap();
        let t = directory.path().join("t.nc");
        let u = directory.path().join("u.nc");
        write_role_member_fixture(&t, "t", 10.0, 2.5, 1, &[500.0, 1000.0]);
        write_role_member_fixture(&u, "u", 10.0, 2.5, 1, &[500.0, 1000.0]);
        let assembly = NetCdfAssembly::assemble(&[t, u]).unwrap();
        assert_eq!(assembly.files_by_role.len(), 2);
        assert!(assembly.files_by_role.contains_key("t"));
        assert!(assembly.files_by_role.contains_key("u"));
    }

    fn write_pressure_fixture(path: &Path, decreasing_levels: bool, times: usize) {
        write_pressure_fixture_with_fill(path, decreasing_levels, times, false);
    }

    fn write_role_member_fixture(
        path: &Path,
        role: &str,
        lat0: f64,
        lon_spacing: f64,
        times: usize,
        levels_hpa: &[f64],
    ) {
        assert!(times >= 1);
        assert!(levels_hpa.len() >= 2);
        let mut dataset = DataSet::new();
        dataset.add_fixed_dim("time", times).unwrap();
        dataset.add_fixed_dim("level", levels_hpa.len()).unwrap();
        dataset.add_fixed_dim("lat", 2).unwrap();
        dataset.add_fixed_dim("lon", 3).unwrap();
        dataset.add_var_f64("time", &["time"]).unwrap();
        dataset
            .add_var_attr_string("time", "units", "hours since 2009-01-01 00:00:00")
            .unwrap();
        dataset
            .add_var_attr_string("time", "calendar", "standard")
            .unwrap();
        dataset
            .add_var_attr_string("time", "standard_name", "time")
            .unwrap();
        dataset.add_var_f64("level", &["level"]).unwrap();
        dataset
            .add_var_attr_string("level", "units", "hPa")
            .unwrap();
        dataset
            .add_var_attr_string("level", "standard_name", "air_pressure")
            .unwrap();
        dataset.add_var_f64("lat", &["lat"]).unwrap();
        dataset
            .add_var_attr_string("lat", "units", "degrees_north")
            .unwrap();
        dataset
            .add_var_attr_string("lat", "standard_name", "latitude")
            .unwrap();
        dataset.add_var_f64("lon", &["lon"]).unwrap();
        dataset
            .add_var_attr_string("lon", "units", "degrees_east")
            .unwrap();
        dataset
            .add_var_attr_string("lon", "standard_name", "longitude")
            .unwrap();
        dataset
            .add_var_f64(role, &["time", "level", "lat", "lon"])
            .unwrap();
        dataset.add_var_attr_string(role, "units", "K").unwrap();
        dataset
            .add_global_attr_string("Conventions", "CF-1.7")
            .unwrap();
        dataset
            .add_global_attr_string("dataset_family", "cfsr_derived_cf_multifile_netcdf")
            .unwrap();
        dataset.add_global_attr_string("role", role).unwrap();

        let mut writer = FileWriter::create_new(path).unwrap();
        writer.set_def(&dataset, Version::Classic, 0).unwrap();
        let time_values = (0..times).map(|hour| hour as f64 * 6.0).collect::<Vec<_>>();
        writer.write_var_f64("time", &time_values).unwrap();
        writer.write_var_f64("level", levels_hpa).unwrap();
        writer.write_var_f64("lat", &[lat0, lat0 + 2.5]).unwrap();
        writer
            .write_var_f64("lon", &[0.0, lon_spacing, lon_spacing * 2.0])
            .unwrap();
        let values = vec![250.0_f64; times * levels_hpa.len() * 6];
        writer.write_var_f64(role, &values).unwrap();
        writer.close().unwrap();
    }

    fn write_pressure_fixture_with_fill(
        path: &Path,
        decreasing_levels: bool,
        times: usize,
        fill_temperature: bool,
    ) {
        assert!(times >= 1);
        let mut dataset = DataSet::new();
        dataset.add_fixed_dim("time", times).unwrap();
        dataset.add_fixed_dim("level", 2).unwrap();
        dataset.add_fixed_dim("lat", 2).unwrap();
        dataset.add_fixed_dim("lon", 3).unwrap();
        dataset.add_var_f64("time", &["time"]).unwrap();
        dataset
            .add_var_attr_string("time", "units", "hours since 2009-01-01 00:00:00")
            .unwrap();
        dataset
            .add_var_attr_string("time", "calendar", "standard")
            .unwrap();
        dataset.add_var_attr_string("time", "axis", "T").unwrap();
        dataset.add_var_f64("level", &["level"]).unwrap();
        dataset
            .add_var_attr_string("level", "units", "hPa")
            .unwrap();
        dataset.add_var_attr_string("level", "axis", "Z").unwrap();
        dataset.add_var_f64("lat", &["lat"]).unwrap();
        dataset
            .add_var_attr_string("lat", "units", "degrees_north")
            .unwrap();
        dataset.add_var_attr_string("lat", "axis", "Y").unwrap();
        dataset
            .add_var_attr_string("lat", "standard_name", "latitude")
            .unwrap();
        dataset.add_var_f64("lon", &["lon"]).unwrap();
        dataset
            .add_var_attr_string("lon", "units", "degrees_east")
            .unwrap();
        dataset.add_var_attr_string("lon", "axis", "X").unwrap();
        dataset
            .add_var_attr_string("lon", "standard_name", "longitude")
            .unwrap();
        dataset
            .add_var_f64("t", &["time", "level", "lat", "lon"])
            .unwrap();
        dataset.add_var_attr_string("t", "units", "K").unwrap();
        if fill_temperature {
            dataset
                .add_var_attr_f64("t", "_FillValue", vec![-9_999.0])
                .unwrap();
        }
        dataset
            .add_var_attr_string("t", "standard_name", "air_temperature")
            .unwrap();
        dataset.add_var_f64("sp", &["time", "lat", "lon"]).unwrap();
        dataset.add_var_attr_string("sp", "units", "Pa").unwrap();
        dataset
            .add_global_attr_string("Conventions", "CF-1.7")
            .unwrap();
        dataset
            .add_global_attr_string("dataset_family", "era5_cf_pressure_netcdf")
            .unwrap();

        let mut writer = FileWriter::create_new(path).unwrap();
        writer.set_def(&dataset, Version::Classic, 0).unwrap();
        let time_values = (0..times).map(|hour| hour as f64 * 6.0).collect::<Vec<_>>();
        writer.write_var_f64("time", &time_values).unwrap();
        if decreasing_levels {
            // source: 1000 hPa then 500 hPa
            writer.write_var_f64("level", &[1000.0, 500.0]).unwrap();
        } else {
            writer.write_var_f64("level", &[500.0, 1000.0]).unwrap();
        }
        writer.write_var_f64("lat", &[10.0, 12.5]).unwrap();
        writer.write_var_f64("lon", &[0.0, 2.5, 5.0]).unwrap();

        // Per-time temperature: level0 slab 250..255, level1 slab 270..275, then +10 per time.
        let mut t_values = Vec::with_capacity(times * 12);
        for time in 0..times {
            let shift = time as f64 * 10.0;
            if decreasing_levels {
                t_values.extend(
                    [250.0, 251.0, 252.0, 253.0, 254.0, 255.0]
                        .into_iter()
                        .map(|value| value + shift),
                );
                t_values.extend(
                    [270.0, 271.0, 272.0, 273.0, 274.0, 275.0]
                        .into_iter()
                        .map(|value| value + shift),
                );
            } else {
                t_values.extend(
                    [250.0, 251.0, 252.0, 253.0, 254.0, 255.0]
                        .into_iter()
                        .map(|value| value + shift),
                );
                t_values.extend(
                    [270.0, 271.0, 272.0, 273.0, 274.0, 275.0]
                        .into_iter()
                        .map(|value| value + shift),
                );
            }
        }
        if fill_temperature {
            for time in 0..times {
                t_values[time * 12 + 6] = -9_999.0;
            }
        }
        writer.write_var_f64("t", &t_values).unwrap();

        let mut sp_values = Vec::with_capacity(times * 6);
        for time in 0..times {
            let base = 100_000.0 + time as f64 * 1_000.0;
            sp_values.extend([base; 6]);
        }
        writer.write_var_f64("sp", &sp_values).unwrap();
        writer.close().unwrap();
        let _ = DataType::F64;
    }
}
