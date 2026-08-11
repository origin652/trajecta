//! # Contract: GMTED2010 mean elevation and elevation variability
//!
//! The production reader consumes two deterministic, tile-compressed grids
//! derived from the USGS 30 arc-second mean and standard-deviation products.
//! The immutable global DatasetLock contains the two prepared grids and their
//! source manifest. The manifest records the original USGS archive identities,
//! so users do not need the archives, GDAL, or Rasterio at runtime. Runtime
//! sampling performs no format conversion.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use flate2::read::ZlibDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use trajecta_case::document::DataRootId;
use trajecta_case::lockfile::{
    DatasetIdentity, DatasetLock, GeneratorInfo, GridSignature, LockedFile, ProfileIdentity,
    VerticalSignature,
};
use trajecta_case::model::meteorology::DatasetRef;
use trajecta_case::resolver::sha256_file_streaming;
use trajecta_case::schema::CURRENT_SCHEMA_VERSION;

use crate::grid::{DomainGeometry, RegularLatLonGrid};

/// Stable logical identifier used by RunProfile dataset bindings.
pub const GMTED2010_DATASET_ID: &str = "gmted2010-30arcsec-mean-std/v1";
/// Fixed interpretation profile for the prepared GMTED2010 pair.
pub const GMTED2010_PROFILE_NAME: &str = "gmted2010_30arcsec_mean_std";
/// Stable prepared-grid format identity.
pub const GMTED2010_GRID_FORMAT_ID: &str = "trajecta.gmted2010-grid/v1";
/// Required prepared mean-grid basename.
pub const GMTED2010_MEAN_GRID_FILE: &str = "gmted2010-30arcsec-mean.tgrid";
/// Required prepared standard-deviation-grid basename.
pub const GMTED2010_STANDARD_DEVIATION_GRID_FILE: &str =
    "gmted2010-30arcsec-standard-deviation.tgrid";
/// Required conversion/source manifest basename.
pub const GMTED2010_SOURCE_MANIFEST_FILE: &str = "gmted2010-source-manifest.json";
/// Required original USGS mean archive basename.
pub const GMTED2010_MEAN_SOURCE_ARCHIVE: &str = "mn30_grd.zip";
/// Required original USGS standard-deviation archive basename.
pub const GMTED2010_STANDARD_DEVIATION_SOURCE_ARCHIVE: &str = "sd30_grd.zip";

/// Frozen byte count of the official mean-elevation source archive.
pub const GMTED2010_MEAN_SOURCE_BYTES: u64 = 277_229_566;
/// Frozen SHA-256 of the official mean-elevation source archive.
pub const GMTED2010_MEAN_SOURCE_SHA256: &str =
    "dfd0d6c6486f4da22109be6107c93a70ef9917a5b6f954cd82d87cf8b920149d";
/// Frozen byte count of the official elevation-standard-deviation archive.
pub const GMTED2010_STANDARD_DEVIATION_SOURCE_BYTES: u64 = 126_192_743;
/// Frozen SHA-256 of the official elevation-standard-deviation archive.
pub const GMTED2010_STANDARD_DEVIATION_SOURCE_SHA256: &str =
    "953b05060a44a35adc2e7834a2498f5c1283b9243cdbbce6aa5b2e8f20375bd4";

const SOURCE_MANIFEST_SCHEMA: &str = "trajecta.gmted2010-source/v1";
const SOURCE_PROVIDER: &str = "U.S. Geological Survey";
const SOURCE_ATTRIBUTION: &str = "U.S. Geological Survey Global Multi-resolution Terrain Elevation Data 2010 (GMTED2010), DOI 10.3133/ofr20111073";

const HEADER_BYTES: usize = 128;
const INDEX_ENTRY_BYTES: usize = 16;
const MAGIC: &[u8; 16] = b"TRAJECTA-TGRID1\0";
const FORMAT_VERSION: u32 = 1;
const COMPRESSION_ZLIB: u8 = 1;
const DEFAULT_TILE_CACHE_CAPACITY: usize = 64;
const DEFAULT_CELL_MEAN_CACHE_CAPACITY: usize = 4096;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceManifest {
    schema_version: String,
    dataset_id: String,
    prepared_format: String,
    source: SourceProvenance,
    sources: Vec<SourceArchiveIdentity>,
    prepared_files: Vec<PreparedFileIdentity>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceProvenance {
    provider: String,
    product: String,
    product_url: String,
    catalog_url: String,
    transfer_source: Option<String>,
    attribution: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceArchiveIdentity {
    role: String,
    file_name: String,
    size_bytes: u64,
    sha256: String,
    member_manifest_sha256: String,
    members: Vec<SourceArchiveMember>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceArchiveMember {
    path: String,
    size_bytes: u64,
    compressed_size_bytes: u64,
    crc32: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedFileIdentity {
    role: String,
    relative_path: String,
    size_bytes: u64,
    sha256: String,
    grid: PreparedGridIdentity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedGridIdentity {
    width: usize,
    height: usize,
    tile_width: usize,
    tile_height: usize,
    longitude_origin_degrees: f64,
    latitude_origin_degrees: f64,
    longitude_spacing_degrees: f64,
    latitude_spacing_degrees: f64,
    nodata: i16,
}

#[derive(Clone, Debug)]
struct LocatedFile {
    root_id: DataRootId,
    relative_path: PathBuf,
    absolute_path: PathBuf,
}

/// Builds the immutable global lock for the prepared GMTED2010 pair.
///
/// Each required basename must occur at the top level of exactly one configured
/// data root. The three runtime files may use separate named roots.
pub fn build_global_dataset_lock(
    data_roots: &BTreeMap<DataRootId, PathBuf>,
    generator: GeneratorInfo,
) -> Result<DatasetLock, GmtedError> {
    if data_roots.is_empty() {
        return Err(GmtedError::InvalidLock(
            "GMTED2010 requires at least one named data root".into(),
        ));
    }
    let mean_grid = locate_required_file(data_roots, GMTED2010_MEAN_GRID_FILE)?;
    let deviation_grid = locate_required_file(data_roots, GMTED2010_STANDARD_DEVIATION_GRID_FILE)?;
    let manifest_file = locate_required_file(data_roots, GMTED2010_SOURCE_MANIFEST_FILE)?;

    let manifest = read_source_manifest(&manifest_file.absolute_path)?;
    validate_source_manifest(&manifest)?;
    validate_prepared_manifest(&manifest, &mean_grid, &deviation_grid)?;
    let dataset = Gmted2010::open(&mean_grid.absolute_path, &deviation_grid.absolute_path)?;

    let mut files = vec![
        locked_file(
            mean_grid,
            vec!["mean_elevation".into(), "prepared_grid".into()],
        )?,
        locked_file(
            deviation_grid,
            vec!["prepared_grid".into(), "standard_deviation".into()],
        )?,
        locked_file(manifest_file, vec!["source_manifest".into()])?,
    ];
    files.sort_by(|left, right| {
        (&left.root_id, &left.relative_path).cmp(&(&right.root_id, &right.relative_path))
    });
    let grid = GridSignature {
        nx: dataset.mean_descriptor().width,
        ny: dataset.mean_descriptor().height,
        periodic_longitude: true,
        sha256: paired_grid_sha256(
            dataset.mean_descriptor(),
            dataset.standard_deviation_descriptor(),
        ),
    };
    let lock = DatasetLock {
        schema_version: CURRENT_SCHEMA_VERSION,
        identity: DatasetIdentity {
            id: DatasetRef(GMTED2010_DATASET_ID.into()),
            source: SOURCE_PROVIDER.into(),
            source_url: Some("https://pubs.usgs.gov/of/2011/1073/".into()),
            attribution: Some(SOURCE_ATTRIBUTION.into()),
        },
        profile: ProfileIdentity {
            name: GMTED2010_PROFILE_NAME.into(),
            sha256: interpretation_profile_sha256(),
        },
        generator,
        files,
        grid,
        vertical: VerticalSignature::Surface,
    };
    let diagnostics = lock.validate_shape();
    if diagnostics.has_errors() {
        return Err(GmtedError::InvalidLock(format!(
            "generated GMTED2010 lock is invalid: {:?}",
            diagnostics.sorted()
        )));
    }
    Ok(lock)
}

/// Verifies a global GMTED2010 lock and opens its prepared grids.
pub fn open_from_dataset_lock(
    lock: &DatasetLock,
    lockfile_dir: &Path,
    data_roots: &BTreeMap<DataRootId, PathBuf>,
) -> Result<Gmted2010, GmtedError> {
    let diagnostics = lock
        .verify_local_files_with_roots(lockfile_dir, data_roots)
        .map_err(|error| GmtedError::InvalidLock(error.to_string()))?;
    if diagnostics.has_errors() {
        return Err(GmtedError::InvalidLock(format!(
            "GMTED2010 locked files failed verification: {:?}",
            diagnostics.sorted()
        )));
    }
    if lock.identity.id.0 != GMTED2010_DATASET_ID
        || lock.identity.source != SOURCE_PROVIDER
        || lock.identity.attribution.as_deref() != Some(SOURCE_ATTRIBUTION)
        || lock.profile.name != GMTED2010_PROFILE_NAME
        || lock.profile.sha256 != interpretation_profile_sha256()
        || lock.vertical != VerticalSignature::Surface
        || lock.files.len() != 3
    {
        return Err(GmtedError::InvalidLock(
            "GMTED2010 lock identity is inconsistent".into(),
        ));
    }
    let mean_grid = locked_path(lock, GMTED2010_MEAN_GRID_FILE, lockfile_dir, data_roots)?;
    let deviation_grid = locked_path(
        lock,
        GMTED2010_STANDARD_DEVIATION_GRID_FILE,
        lockfile_dir,
        data_roots,
    )?;
    let manifest_path = locked_path(
        lock,
        GMTED2010_SOURCE_MANIFEST_FILE,
        lockfile_dir,
        data_roots,
    )?;
    let manifest = read_source_manifest(&manifest_path)?;
    validate_source_manifest(&manifest)?;
    validate_prepared_manifest(
        &manifest,
        &located_from_locked(lock, GMTED2010_MEAN_GRID_FILE, mean_grid.clone())?,
        &located_from_locked(
            lock,
            GMTED2010_STANDARD_DEVIATION_GRID_FILE,
            deviation_grid.clone(),
        )?,
    )?;
    let dataset = Gmted2010::open(&mean_grid, &deviation_grid)?;
    let expected_grid = GridSignature {
        nx: dataset.mean_descriptor().width,
        ny: dataset.mean_descriptor().height,
        periodic_longitude: true,
        sha256: paired_grid_sha256(
            dataset.mean_descriptor(),
            dataset.standard_deviation_descriptor(),
        ),
    };
    if lock.grid != expected_grid {
        return Err(GmtedError::InvalidLock(
            "GMTED2010 grid identity is inconsistent".into(),
        ));
    }
    Ok(dataset)
}

fn locate_required_file(
    data_roots: &BTreeMap<DataRootId, PathBuf>,
    basename: &str,
) -> Result<LocatedFile, GmtedError> {
    let mut matches = Vec::new();
    for (root_id, root) in data_roots {
        let path = root.join(basename);
        if path.is_file() {
            matches.push(LocatedFile {
                root_id: root_id.clone(),
                relative_path: PathBuf::from(basename),
                absolute_path: path,
            });
        }
    }
    match matches.as_slice() {
        [located] => Ok(located.clone()),
        [] => Err(GmtedError::RequiredFileMissing(basename.into())),
        _ => Err(GmtedError::RequiredFileAmbiguous(basename.into())),
    }
}

fn locked_file(mut located: LocatedFile, mut roles: Vec<String>) -> Result<LockedFile, GmtedError> {
    roles.sort();
    roles.dedup();
    let (size_bytes, sha256) =
        sha256_file_streaming(&located.absolute_path).map_err(|error| GmtedError::Io {
            path: located.absolute_path.clone(),
            message: error.to_string(),
        })?;
    Ok(LockedFile {
        roles,
        root_id: located.root_id,
        relative_path: std::mem::take(&mut located.relative_path),
        valid_times: Vec::new(),
        size_bytes,
        sha256,
    })
}

fn locked_path(
    lock: &DatasetLock,
    basename: &str,
    lockfile_dir: &Path,
    data_roots: &BTreeMap<DataRootId, PathBuf>,
) -> Result<PathBuf, GmtedError> {
    let mut matches = lock
        .files
        .iter()
        .filter(|file| file.relative_path == Path::new(basename));
    let file = matches
        .next()
        .ok_or_else(|| GmtedError::RequiredFileMissing(basename.into()))?;
    if matches.next().is_some() {
        return Err(GmtedError::RequiredFileAmbiguous(basename.into()));
    }
    let root = if file.root_id.0 == DataRootId::LOCKFILE {
        lockfile_dir
    } else {
        data_roots
            .get(&file.root_id)
            .map(PathBuf::as_path)
            .ok_or_else(|| GmtedError::InvalidLock("unknown GMTED2010 data root".into()))?
    };
    Ok(root.join(&file.relative_path))
}

fn located_from_locked(
    lock: &DatasetLock,
    basename: &str,
    absolute_path: PathBuf,
) -> Result<LocatedFile, GmtedError> {
    let file = lock
        .files
        .iter()
        .find(|file| file.relative_path == Path::new(basename))
        .ok_or_else(|| GmtedError::RequiredFileMissing(basename.into()))?;
    Ok(LocatedFile {
        root_id: file.root_id.clone(),
        relative_path: file.relative_path.clone(),
        absolute_path,
    })
}

fn read_source_manifest(path: &Path) -> Result<SourceManifest, GmtedError> {
    let bytes = fs::read(path).map_err(|error| GmtedError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|error| GmtedError::InvalidFormat(format!("GMTED2010 manifest: {error}")))
}

fn validate_source_manifest(manifest: &SourceManifest) -> Result<(), GmtedError> {
    validate_manifest_header(manifest)?;
    let expected = [
        (
            "mean_source_archive",
            GMTED2010_MEAN_SOURCE_ARCHIVE,
            GMTED2010_MEAN_SOURCE_BYTES,
            GMTED2010_MEAN_SOURCE_SHA256,
        ),
        (
            "standard_deviation_source_archive",
            GMTED2010_STANDARD_DEVIATION_SOURCE_ARCHIVE,
            GMTED2010_STANDARD_DEVIATION_SOURCE_BYTES,
            GMTED2010_STANDARD_DEVIATION_SOURCE_SHA256,
        ),
    ];
    if manifest.sources.len() != expected.len() {
        return Err(GmtedError::InvalidFormat(
            "GMTED2010 manifest source count is invalid".into(),
        ));
    }
    for (role, file_name, size_bytes, sha256) in expected {
        let source = manifest
            .sources
            .iter()
            .find(|source| source.role == role)
            .ok_or_else(|| GmtedError::InvalidFormat(format!("GMTED2010 manifest lacks {role}")))?;
        if source.file_name != file_name
            || source.size_bytes != size_bytes
            || source.sha256 != sha256
            || source.member_manifest_sha256.len() != 64
            || source.members.is_empty()
            || source.members.iter().any(|member| {
                member.path.is_empty()
                    || member.crc32.len() != 8
                    || member.size_bytes == 0
                    || member.compressed_size_bytes == 0
            })
        {
            return Err(GmtedError::SourceIdentityMismatch(file_name.into()));
        }
    }
    Ok(())
}

fn validate_prepared_manifest(
    manifest: &SourceManifest,
    mean: &LocatedFile,
    deviation: &LocatedFile,
) -> Result<(), GmtedError> {
    validate_manifest_header(manifest)?;
    if manifest.prepared_files.len() != 2 {
        return Err(GmtedError::InvalidFormat(
            "GMTED2010 prepared file count is invalid".into(),
        ));
    }
    for (role, name, located, field) in [
        (
            "mean_elevation",
            GMTED2010_MEAN_GRID_FILE,
            mean,
            TerrainField::MeanElevation,
        ),
        (
            "standard_deviation",
            GMTED2010_STANDARD_DEVIATION_GRID_FILE,
            deviation,
            TerrainField::StandardDeviation,
        ),
    ] {
        let prepared = manifest
            .prepared_files
            .iter()
            .find(|prepared| prepared.role == role)
            .ok_or_else(|| GmtedError::InvalidFormat(format!("GMTED2010 manifest lacks {role}")))?;
        let (actual_size, actual_sha256) =
            sha256_file_streaming(&located.absolute_path).map_err(|error| GmtedError::Io {
                path: located.absolute_path.clone(),
                message: error.to_string(),
            })?;
        let grid = TerrainGrid::open(&located.absolute_path, field)?;
        if prepared.relative_path != name
            || prepared.size_bytes != actual_size
            || prepared.sha256 != actual_sha256
            || !prepared.grid.matches(grid.descriptor())
        {
            return Err(GmtedError::SourceIdentityMismatch(name.into()));
        }
    }
    Ok(())
}

fn validate_manifest_header(manifest: &SourceManifest) -> Result<(), GmtedError> {
    if manifest.schema_version != SOURCE_MANIFEST_SCHEMA
        || manifest.dataset_id != GMTED2010_DATASET_ID
        || manifest.prepared_format != GMTED2010_GRID_FORMAT_ID
        || manifest.source.provider != SOURCE_PROVIDER
        || manifest.source.product != "Global Multi-resolution Terrain Elevation Data 2010"
        || manifest.source.product_url != "https://pubs.usgs.gov/of/2011/1073/"
        || manifest.source.catalog_url.is_empty()
        || manifest.source.attribution != SOURCE_ATTRIBUTION
        || manifest
            .source
            .transfer_source
            .as_deref()
            .is_some_and(str::is_empty)
    {
        return Err(GmtedError::InvalidFormat(
            "GMTED2010 manifest identity is invalid".into(),
        ));
    }
    Ok(())
}

impl PreparedGridIdentity {
    fn matches(&self, descriptor: &TerrainGridDescriptor) -> bool {
        self.width == descriptor.width
            && self.height == descriptor.height
            && self.tile_width == descriptor.tile_width
            && self.tile_height == descriptor.tile_height
            && self.longitude_origin_degrees.to_bits()
                == descriptor.longitude_origin_degrees.to_bits()
            && self.latitude_origin_degrees.to_bits()
                == descriptor.latitude_origin_degrees.to_bits()
            && self.longitude_spacing_degrees.to_bits()
                == descriptor.longitude_spacing_degrees.to_bits()
            && self.latitude_spacing_degrees.to_bits()
                == descriptor.latitude_spacing_degrees.to_bits()
            && self.nodata == descriptor.nodata
    }
}

fn interpretation_profile_sha256() -> String {
    let mut digest = Sha256::new();
    digest.update(b"trajecta.gmted2010-profile/v1\n");
    digest.update(GMTED2010_DATASET_ID.as_bytes());
    digest.update(
        b"\nint16-metres\nperiodic-longitude-bilinear\nspherical-area-cell-mean\nhard-nodata\n",
    );
    hex::encode(digest.finalize())
}

fn paired_grid_sha256(mean: &TerrainGridDescriptor, deviation: &TerrainGridDescriptor) -> String {
    let mut digest = Sha256::new();
    digest.update(GMTED2010_GRID_FORMAT_ID.as_bytes());
    for descriptor in [mean, deviation] {
        digest.update([descriptor.field as u8]);
        digest.update(descriptor.width.to_le_bytes());
        digest.update(descriptor.height.to_le_bytes());
        digest.update(descriptor.tile_width.to_le_bytes());
        digest.update(descriptor.tile_height.to_le_bytes());
        digest.update(descriptor.longitude_origin_degrees.to_bits().to_le_bytes());
        digest.update(descriptor.latitude_origin_degrees.to_bits().to_le_bytes());
        digest.update(descriptor.longitude_spacing_degrees.to_bits().to_le_bytes());
        digest.update(descriptor.latitude_spacing_degrees.to_bits().to_le_bytes());
        digest.update(descriptor.nodata.to_le_bytes());
    }
    hex::encode(digest.finalize())
}

/// Physical field stored in one prepared grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TerrainField {
    /// GMTED2010 mean elevation in metres.
    MeanElevation = 1,
    /// GMTED2010 elevation standard deviation in metres.
    StandardDeviation = 2,
}

impl TerrainField {
    fn parse(value: u8) -> Result<Self, GmtedError> {
        match value {
            1 => Ok(Self::MeanElevation),
            2 => Ok(Self::StandardDeviation),
            _ => Err(GmtedError::InvalidFormat("unknown terrain field".into())),
        }
    }
}

/// Immutable prepared-grid geometry and storage identity.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainGridDescriptor {
    /// Stored physical field.
    pub field: TerrainField,
    /// Number of raster columns.
    pub width: usize,
    /// Number of raster rows.
    pub height: usize,
    /// Tile width in samples.
    pub tile_width: usize,
    /// Tile height in samples.
    pub tile_height: usize,
    /// Longitude of the first pixel's western edge.
    pub longitude_origin_degrees: f64,
    /// Latitude of the first pixel's northern edge.
    pub latitude_origin_degrees: f64,
    /// Positive pixel width in degrees.
    pub longitude_spacing_degrees: f64,
    /// Negative pixel height in degrees.
    pub latitude_spacing_degrees: f64,
    /// Signed integer missing-value marker.
    pub nodata: i16,
}

impl TerrainGridDescriptor {
    fn validate(&self) -> Result<(), GmtedError> {
        if self.width == 0
            || self.height == 0
            || self.tile_width == 0
            || self.tile_height == 0
            || !self.longitude_origin_degrees.is_finite()
            || !self.latitude_origin_degrees.is_finite()
            || !self.longitude_spacing_degrees.is_finite()
            || self.longitude_spacing_degrees <= 0.0
            || !self.latitude_spacing_degrees.is_finite()
            || self.latitude_spacing_degrees >= 0.0
        {
            return Err(GmtedError::InvalidFormat(
                "invalid prepared-grid geometry".into(),
            ));
        }
        let longitude_span = self.longitude_spacing_degrees * self.width as f64;
        let southern_edge =
            self.latitude_origin_degrees + self.latitude_spacing_degrees * self.height as f64;
        // The official Arc/Info grids register their outer bounds a small
        // fraction of a source pixel beyond the geographic pole. Accept at
        // most half a pixel of edge-registration offset.
        let latitude_edge_slack = 0.5 * self.latitude_spacing_degrees.abs() + 1.0e-6;
        if (longitude_span - 360.0).abs() > 5.0e-6
            || !southern_edge.is_finite()
            || self.latitude_origin_degrees > 90.0 + latitude_edge_slack
            || southern_edge < -90.0 - latitude_edge_slack
        {
            return Err(GmtedError::InvalidFormat(
                "prepared grid has invalid geographic coverage".into(),
            ));
        }
        Ok(())
    }

    fn tile_columns(&self) -> usize {
        self.width.div_ceil(self.tile_width)
    }

    fn tile_rows(&self) -> usize {
        self.height.div_ceil(self.tile_height)
    }

    fn tile_sample_count(&self) -> Result<usize, GmtedError> {
        self.tile_width
            .checked_mul(self.tile_height)
            .ok_or(GmtedError::ResourceLimit)
    }

    fn southern_edge(&self) -> f64 {
        self.latitude_origin_degrees + self.latitude_spacing_degrees * self.height as f64
    }
}

#[derive(Clone, Copy, Debug)]
struct TileIndexEntry {
    offset: u64,
    compressed_bytes: u32,
    raw_bytes: u32,
}

#[derive(Debug)]
struct TileCache {
    capacity: usize,
    order: VecDeque<usize>,
    tiles: BTreeMap<usize, Arc<[i16]>>,
}

impl TileCache {
    fn new(capacity: usize) -> Result<Self, GmtedError> {
        if capacity == 0 {
            return Err(GmtedError::InvalidFormat(
                "tile cache capacity must be positive".into(),
            ));
        }
        Ok(Self {
            capacity,
            order: VecDeque::new(),
            tiles: BTreeMap::new(),
        })
    }

    fn get(&mut self, index: usize) -> Option<Arc<[i16]>> {
        let value = self.tiles.get(&index).cloned()?;
        if let Some(position) = self.order.iter().position(|candidate| *candidate == index) {
            self.order.remove(position);
        }
        self.order.push_back(index);
        Some(value)
    }

    fn insert(&mut self, index: usize, tile: Arc<[i16]>) {
        if self.tiles.contains_key(&index) {
            return;
        }
        while self.tiles.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.tiles.remove(&evicted);
            }
        }
        self.order.push_back(index);
        self.tiles.insert(index, tile);
    }
}

#[derive(Debug)]
struct GridState {
    file: File,
    cache: TileCache,
}

/// Read-only deterministic tile reader for one prepared GMTED2010 field.
#[derive(Debug)]
pub struct TerrainGrid {
    path: PathBuf,
    descriptor: TerrainGridDescriptor,
    index: Vec<TileIndexEntry>,
    state: Mutex<GridState>,
}

impl TerrainGrid {
    /// Opens and structurally validates one prepared grid.
    pub fn open(path: &Path, expected_field: TerrainField) -> Result<Self, GmtedError> {
        Self::open_with_cache(path, expected_field, DEFAULT_TILE_CACHE_CAPACITY)
    }

    fn open_with_cache(
        path: &Path,
        expected_field: TerrainField,
        cache_capacity: usize,
    ) -> Result<Self, GmtedError> {
        let mut file = File::open(path).map_err(|error| GmtedError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let file_bytes = file
            .metadata()
            .map_err(|error| GmtedError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?
            .len();
        let mut header = [0_u8; HEADER_BYTES];
        file.read_exact(&mut header)
            .map_err(|error| GmtedError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            })?;
        let parsed = parse_header(&header)?;
        if parsed.descriptor.field != expected_field {
            return Err(GmtedError::FieldMismatch {
                expected: expected_field,
                actual: parsed.descriptor.field,
            });
        }
        let expected_tiles = parsed
            .descriptor
            .tile_columns()
            .checked_mul(parsed.descriptor.tile_rows())
            .ok_or(GmtedError::ResourceLimit)?;
        if parsed.tile_count != expected_tiles
            || parsed.index_offset != HEADER_BYTES as u64
            || parsed.data_offset
                != parsed
                    .index_offset
                    .checked_add(
                        u64::try_from(expected_tiles)
                            .map_err(|_| GmtedError::ResourceLimit)?
                            .checked_mul(INDEX_ENTRY_BYTES as u64)
                            .ok_or(GmtedError::ResourceLimit)?,
                    )
                    .ok_or(GmtedError::ResourceLimit)?
        {
            return Err(GmtedError::InvalidFormat(
                "prepared-grid index geometry is inconsistent".into(),
            ));
        }
        let expected_raw_bytes = parsed
            .descriptor
            .tile_sample_count()?
            .checked_mul(std::mem::size_of::<i16>())
            .ok_or(GmtedError::ResourceLimit)?;
        let mut index = Vec::with_capacity(expected_tiles);
        let mut expected_offset = parsed.data_offset;
        for _ in 0..expected_tiles {
            let mut bytes = [0_u8; INDEX_ENTRY_BYTES];
            file.read_exact(&mut bytes)
                .map_err(|error| GmtedError::Io {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;
            let entry = TileIndexEntry {
                offset: le_u64(&bytes[0..8])?,
                compressed_bytes: le_u32(&bytes[8..12])?,
                raw_bytes: le_u32(&bytes[12..16])?,
            };
            if entry.offset != expected_offset
                || entry.compressed_bytes == 0
                || usize::try_from(entry.raw_bytes).ok() != Some(expected_raw_bytes)
            {
                return Err(GmtedError::InvalidFormat(
                    "prepared-grid tile index is invalid".into(),
                ));
            }
            expected_offset = expected_offset
                .checked_add(u64::from(entry.compressed_bytes))
                .ok_or(GmtedError::ResourceLimit)?;
            index.push(entry);
        }
        if expected_offset != file_bytes {
            return Err(GmtedError::InvalidFormat(
                "prepared-grid payload length does not match its index".into(),
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            descriptor: parsed.descriptor,
            index,
            state: Mutex::new(GridState {
                file,
                cache: TileCache::new(cache_capacity)?,
            }),
        })
    }

    /// Returns immutable grid geometry.
    #[must_use]
    pub const fn descriptor(&self) -> &TerrainGridDescriptor {
        &self.descriptor
    }

    /// Samples the pixel-centred field with periodic-longitude bilinear interpolation.
    pub fn sample_bilinear(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<f64, GmtedError> {
        if !longitude_degrees.is_finite() || !latitude_degrees.is_finite() {
            return Err(GmtedError::InvalidCoordinate);
        }
        let north = self.descriptor.latitude_origin_degrees;
        let south = self.descriptor.southern_edge();
        if latitude_degrees < south || latitude_degrees > north {
            return Err(GmtedError::OutsideCoverage {
                latitude_degrees,
                southern_edge: south,
                northern_edge: north,
            });
        }
        let longitude_span =
            self.descriptor.longitude_spacing_degrees * self.descriptor.width as f64;
        let normalized_longitude = self.descriptor.longitude_origin_degrees
            + (longitude_degrees - self.descriptor.longitude_origin_degrees)
                .rem_euclid(longitude_span);
        let x = (normalized_longitude
            - (self.descriptor.longitude_origin_degrees
                + 0.5 * self.descriptor.longitude_spacing_degrees))
            / self.descriptor.longitude_spacing_degrees;
        let y = (latitude_degrees
            - (self.descriptor.latitude_origin_degrees
                + 0.5 * self.descriptor.latitude_spacing_degrees))
            / self.descriptor.latitude_spacing_degrees;
        let x_floor = x.floor();
        let x0 = (x_floor as i64).rem_euclid(self.descriptor.width as i64) as usize;
        let x1 = (x0 + 1) % self.descriptor.width;
        let fx = x - x_floor;
        let clamped_y = y.clamp(0.0, self.descriptor.height.saturating_sub(1) as f64);
        let y0 = clamped_y.floor() as usize;
        let y1 = (y0 + 1).min(self.descriptor.height - 1);
        let fy = clamped_y - y0 as f64;
        let values = [
            self.value_at(x0, y0)?,
            self.value_at(x1, y0)?,
            self.value_at(x0, y1)?,
            self.value_at(x1, y1)?,
        ];
        let top = (values[1] - values[0]).mul_add(fx, values[0]);
        let bottom = (values[3] - values[2]).mul_add(fx, values[2]);
        let value = (bottom - top).mul_add(fy, top);
        value
            .is_finite()
            .then_some(value)
            .ok_or(GmtedError::InvalidValue)
    }

    /// Computes a source-pixel spherical-area-weighted mean over one geographic cell.
    pub fn spherical_mean(
        &self,
        west_degrees: f64,
        east_degrees: f64,
        south_degrees: f64,
        north_degrees: f64,
    ) -> Result<f64, GmtedError> {
        if [west_degrees, east_degrees, south_degrees, north_degrees]
            .into_iter()
            .any(|value| !value.is_finite())
            || east_degrees <= west_degrees
            || north_degrees <= south_degrees
            || east_degrees - west_degrees > 360.0 + 1.0e-9
        {
            return Err(GmtedError::InvalidCoordinate);
        }
        let grid_south = self.descriptor.southern_edge();
        let grid_north = self.descriptor.latitude_origin_degrees;
        if south_degrees < grid_south - 1.0e-9 || north_degrees > grid_north + 1.0e-9 {
            return Err(GmtedError::OutsideCoverage {
                latitude_degrees: if south_degrees < grid_south {
                    south_degrees
                } else {
                    north_degrees
                },
                southern_edge: grid_south,
                northern_edge: grid_north,
            });
        }
        let y_step = -self.descriptor.latitude_spacing_degrees;
        let first_row = ((grid_north - north_degrees) / y_step).floor().max(0.0) as usize;
        let final_row = (((grid_north - south_degrees) / y_step).ceil() as usize)
            .min(self.descriptor.height)
            .saturating_sub(1);
        let span = self.descriptor.longitude_spacing_degrees * self.descriptor.width as f64;
        let mut relative_west =
            (west_degrees - self.descriptor.longitude_origin_degrees).rem_euclid(span);
        let mut remaining = east_degrees - west_degrees;
        let mut weighted_sum = 0.0_f64;
        let mut area_sum = 0.0_f64;
        while remaining > 1.0e-12 {
            let segment_width = remaining.min(span - relative_west);
            let segment_east = relative_west + segment_width;
            let first_column = (relative_west / self.descriptor.longitude_spacing_degrees)
                .floor()
                .max(0.0) as usize;
            let final_column = ((segment_east / self.descriptor.longitude_spacing_degrees).ceil()
                as usize)
                .min(self.descriptor.width)
                .saturating_sub(1);
            for row in first_row..=final_row {
                let pixel_north = grid_north - row as f64 * y_step;
                let pixel_south = pixel_north - y_step;
                let overlap_north = north_degrees.min(pixel_north);
                let overlap_south = south_degrees.max(pixel_south);
                if overlap_north <= overlap_south {
                    continue;
                }
                let latitude_area =
                    overlap_north.to_radians().sin() - overlap_south.to_radians().sin();
                for column in first_column..=final_column {
                    let pixel_west = column as f64 * self.descriptor.longitude_spacing_degrees;
                    let pixel_east = pixel_west + self.descriptor.longitude_spacing_degrees;
                    let overlap_west = relative_west.max(pixel_west);
                    let overlap_east = segment_east.min(pixel_east);
                    if overlap_east <= overlap_west {
                        continue;
                    }
                    let area = (overlap_east - overlap_west).to_radians() * latitude_area;
                    let value = self.value_at(column, row)?;
                    weighted_sum = value.mul_add(area, weighted_sum);
                    area_sum += area;
                }
            }
            remaining -= segment_width;
            relative_west = 0.0;
        }
        let mean = weighted_sum / area_sum;
        if area_sum <= 0.0 || !mean.is_finite() {
            Err(GmtedError::InvalidValue)
        } else {
            Ok(mean)
        }
    }

    fn value_at(&self, column: usize, row: usize) -> Result<f64, GmtedError> {
        if column >= self.descriptor.width || row >= self.descriptor.height {
            return Err(GmtedError::InvalidCoordinate);
        }
        let tile_column = column / self.descriptor.tile_width;
        let tile_row = row / self.descriptor.tile_height;
        let tile_index = tile_row
            .checked_mul(self.descriptor.tile_columns())
            .and_then(|value| value.checked_add(tile_column))
            .ok_or(GmtedError::ResourceLimit)?;
        let local_column = column % self.descriptor.tile_width;
        let local_row = row % self.descriptor.tile_height;
        let local_index = local_row
            .checked_mul(self.descriptor.tile_width)
            .and_then(|value| value.checked_add(local_column))
            .ok_or(GmtedError::ResourceLimit)?;
        let tile = self.tile(tile_index)?;
        let value = *tile
            .get(local_index)
            .ok_or_else(|| GmtedError::InvalidFormat("tile sample is missing".into()))?;
        if value == self.descriptor.nodata {
            Err(GmtedError::NoData { column, row })
        } else {
            Ok(f64::from(value))
        }
    }

    fn tile(&self, index: usize) -> Result<Arc<[i16]>, GmtedError> {
        let entry = *self
            .index
            .get(index)
            .ok_or_else(|| GmtedError::InvalidFormat("tile index is out of range".into()))?;
        let mut state = self.state.lock().map_err(|_| GmtedError::CachePoisoned)?;
        if let Some(tile) = state.cache.get(index) {
            return Ok(tile);
        }
        state
            .file
            .seek(SeekFrom::Start(entry.offset))
            .map_err(|error| GmtedError::Io {
                path: self.path.clone(),
                message: error.to_string(),
            })?;
        let compressed_len =
            usize::try_from(entry.compressed_bytes).map_err(|_| GmtedError::ResourceLimit)?;
        let mut compressed = vec![0_u8; compressed_len];
        state
            .file
            .read_exact(&mut compressed)
            .map_err(|error| GmtedError::Io {
                path: self.path.clone(),
                message: error.to_string(),
            })?;
        let raw_len = usize::try_from(entry.raw_bytes).map_err(|_| GmtedError::ResourceLimit)?;
        let mut raw = vec![0_u8; raw_len];
        let mut decoder = ZlibDecoder::new(compressed.as_slice());
        decoder
            .read_exact(&mut raw)
            .map_err(|error| GmtedError::InvalidFormat(format!("zlib tile: {error}")))?;
        let mut trailing = [0_u8; 1];
        if decoder
            .read(&mut trailing)
            .map_err(|error| GmtedError::InvalidFormat(format!("zlib tile: {error}")))?
            != 0
        {
            return Err(GmtedError::InvalidFormat(
                "zlib tile expands beyond its declared length".into(),
            ));
        }
        let tile = raw
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>();
        if !raw.chunks_exact(2).remainder().is_empty()
            || tile.len() != self.descriptor.tile_sample_count()?
        {
            return Err(GmtedError::InvalidFormat(
                "tile has an invalid sample count".into(),
            ));
        }
        let tile: Arc<[i16]> = Arc::from(tile);
        state.cache.insert(index, Arc::clone(&tile));
        Ok(tile)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MeteorologyCellKey {
    longitude_origin_bits: u64,
    latitude_origin_bits: u64,
    longitude_spacing_bits: u64,
    latitude_spacing_bits: u64,
    nx: usize,
    ny: usize,
    x: usize,
    y: usize,
}

#[derive(Debug)]
struct CellMeanCache {
    capacity: usize,
    order: VecDeque<MeteorologyCellKey>,
    values: BTreeMap<MeteorologyCellKey, f64>,
}

impl CellMeanCache {
    fn get(&mut self, key: MeteorologyCellKey) -> Option<f64> {
        let value = self.values.get(&key).copied()?;
        if let Some(position) = self.order.iter().position(|candidate| *candidate == key) {
            self.order.remove(position);
        }
        self.order.push_back(key);
        Some(value)
    }

    fn insert(&mut self, key: MeteorologyCellKey, value: f64) {
        while self.values.len() >= self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.values.remove(&evicted);
            }
        }
        self.order.push_back(key);
        self.values.insert(key, value);
    }
}

/// GMTED values evaluated for one point and its containing meteorology cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrographySample {
    /// Bilinear mean elevation at the particle point.
    pub point_elevation_m: f64,
    /// Spherical-area mean over the containing meteorology interpolation cell.
    pub meteorology_cell_mean_elevation_m: f64,
    /// Bilinear source elevation standard deviation at the particle point.
    pub elevation_standard_deviation_m: f64,
}

impl OrographySample {
    /// Additive displacement applied to the meteorological terrain.
    #[must_use]
    pub fn terrain_anomaly_m(self) -> f64 {
        self.point_elevation_m - self.meteorology_cell_mean_elevation_m
    }
}

/// Thread-safe paired GMTED2010 dataset shared by physics and boundary paths.
#[derive(Debug)]
pub struct Gmted2010 {
    mean: TerrainGrid,
    standard_deviation: TerrainGrid,
    cell_means: Mutex<CellMeanCache>,
}

impl Gmted2010 {
    /// Opens the prepared mean and standard-deviation pair.
    pub fn open(mean_path: &Path, standard_deviation_path: &Path) -> Result<Self, GmtedError> {
        let mean = TerrainGrid::open(mean_path, TerrainField::MeanElevation)?;
        let standard_deviation =
            TerrainGrid::open(standard_deviation_path, TerrainField::StandardDeviation)?;
        validate_pair(mean.descriptor(), standard_deviation.descriptor())?;
        Ok(Self {
            mean,
            standard_deviation,
            cell_means: Mutex::new(CellMeanCache {
                capacity: DEFAULT_CELL_MEAN_CACHE_CAPACITY,
                order: VecDeque::new(),
                values: BTreeMap::new(),
            }),
        })
    }

    /// Returns the mean-elevation grid descriptor.
    #[must_use]
    pub const fn mean_descriptor(&self) -> &TerrainGridDescriptor {
        self.mean.descriptor()
    }

    /// Returns the standard-deviation grid descriptor.
    #[must_use]
    pub const fn standard_deviation_descriptor(&self) -> &TerrainGridDescriptor {
        self.standard_deviation.descriptor()
    }

    /// Returns the pixel-centre grid on which mean elevation is bilinear.
    pub fn mean_interpolation_geometry(&self) -> Result<DomainGeometry, GmtedError> {
        let descriptor = self.mean.descriptor();
        let geometry = DomainGeometry {
            domain: trajecta_case::model::meteorology::DomainId("gmted2010-30arcsec-mean".into()),
            longitude_origin_degrees: descriptor.longitude_origin_degrees
                + 0.5 * descriptor.longitude_spacing_degrees,
            latitude_origin_degrees: descriptor.latitude_origin_degrees
                + 0.5 * descriptor.latitude_spacing_degrees,
            longitude_spacing_degrees: descriptor.longitude_spacing_degrees,
            latitude_spacing_degrees: descriptor.latitude_spacing_degrees,
            nx: descriptor.width,
            ny: descriptor.height,
            periodic_longitude: true,
            halo_cells: 0,
        };
        geometry
            .validate()
            .map_err(|error| GmtedError::MeteorologyGrid(format!("{error:?}")))?;
        Ok(geometry)
    }

    /// Samples prepared mean elevation at one geographic point.
    pub fn mean_elevation_m(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<f64, GmtedError> {
        self.mean
            .sample_bilinear(longitude_degrees, latitude_degrees)
    }

    /// Samples prepared elevation standard deviation at one geographic point.
    pub fn elevation_standard_deviation_m(
        &self,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<f64, GmtedError> {
        self.standard_deviation
            .sample_bilinear(longitude_degrees, latitude_degrees)
    }

    /// Samples GMTED point statistics and the exact containing meteorology-cell mean.
    pub fn sample_for_meteorology_cell(
        &self,
        geometry: &DomainGeometry,
        longitude_degrees: f64,
        latitude_degrees: f64,
    ) -> Result<OrographySample, GmtedError> {
        let grid = RegularLatLonGrid::new(geometry.clone())
            .map_err(|error| GmtedError::MeteorologyGrid(format!("{error:?}")))?;
        let (_, weights) = grid
            .locate_cell_and_weights(longitude_degrees, latitude_degrees)
            .map_err(|error| GmtedError::MeteorologyGrid(format!("{error:?}")))?;
        let x = weights.points[0].x;
        let y = weights.points[0].y;
        let key = MeteorologyCellKey {
            longitude_origin_bits: geometry.longitude_origin_degrees.to_bits(),
            latitude_origin_bits: geometry.latitude_origin_degrees.to_bits(),
            longitude_spacing_bits: geometry.longitude_spacing_degrees.to_bits(),
            latitude_spacing_bits: geometry.latitude_spacing_degrees.to_bits(),
            nx: geometry.nx,
            ny: geometry.ny,
            x,
            y,
        };
        let cached = self
            .cell_means
            .lock()
            .map_err(|_| GmtedError::CachePoisoned)?
            .get(key);
        let cell_mean = match cached {
            Some(value) => value,
            None => {
                let west = geometry.longitude_origin_degrees
                    + geometry.longitude_spacing_degrees * x as f64;
                let east = west + geometry.longitude_spacing_degrees;
                let first_latitude =
                    geometry.latitude_origin_degrees + geometry.latitude_spacing_degrees * y as f64;
                let second_latitude = first_latitude + geometry.latitude_spacing_degrees;
                let south = first_latitude.min(second_latitude);
                let north = first_latitude.max(second_latitude);
                let value = self.mean.spherical_mean(west, east, south, north)?;
                self.cell_means
                    .lock()
                    .map_err(|_| GmtedError::CachePoisoned)?
                    .insert(key, value);
                value
            }
        };
        Ok(OrographySample {
            point_elevation_m: self
                .mean
                .sample_bilinear(longitude_degrees, latitude_degrees)?,
            meteorology_cell_mean_elevation_m: cell_mean,
            elevation_standard_deviation_m: self
                .standard_deviation
                .sample_bilinear(longitude_degrees, latitude_degrees)?,
        })
    }
}

fn validate_pair(
    mean: &TerrainGridDescriptor,
    standard_deviation: &TerrainGridDescriptor,
) -> Result<(), GmtedError> {
    if mean.field != TerrainField::MeanElevation
        || standard_deviation.field != TerrainField::StandardDeviation
        || mean.width != standard_deviation.width
        || mean.longitude_origin_degrees.to_bits()
            != standard_deviation.longitude_origin_degrees.to_bits()
        || (mean.longitude_spacing_degrees - standard_deviation.longitude_spacing_degrees).abs()
            > 1.0e-9
    {
        return Err(GmtedError::PairMismatch);
    }
    Ok(())
}

struct ParsedHeader {
    descriptor: TerrainGridDescriptor,
    tile_count: usize,
    index_offset: u64,
    data_offset: u64,
}

fn parse_header(bytes: &[u8; HEADER_BYTES]) -> Result<ParsedHeader, GmtedError> {
    if &bytes[0..16] != MAGIC
        || le_u32(&bytes[16..20])? != FORMAT_VERSION
        || le_u32(&bytes[20..24])? != HEADER_BYTES as u32
        || bytes[43] != COMPRESSION_ZLIB
        || bytes[44..48] != [0; 4]
        || bytes[112..128] != [0; 16]
    {
        return Err(GmtedError::InvalidFormat(
            "prepared-grid header identity is invalid".into(),
        ));
    }
    let descriptor = TerrainGridDescriptor {
        field: TerrainField::parse(bytes[42])?,
        width: usize::try_from(le_u32(&bytes[24..28])?).map_err(|_| GmtedError::ResourceLimit)?,
        height: usize::try_from(le_u32(&bytes[28..32])?).map_err(|_| GmtedError::ResourceLimit)?,
        tile_width: usize::try_from(le_u32(&bytes[32..36])?)
            .map_err(|_| GmtedError::ResourceLimit)?,
        tile_height: usize::try_from(le_u32(&bytes[36..40])?)
            .map_err(|_| GmtedError::ResourceLimit)?,
        nodata: i16::from_le_bytes([bytes[40], bytes[41]]),
        longitude_origin_degrees: le_f64(&bytes[48..56])?,
        latitude_origin_degrees: le_f64(&bytes[56..64])?,
        longitude_spacing_degrees: le_f64(&bytes[64..72])?,
        latitude_spacing_degrees: le_f64(&bytes[72..80])?,
    };
    descriptor.validate()?;
    let tile_columns =
        usize::try_from(le_u32(&bytes[80..84])?).map_err(|_| GmtedError::ResourceLimit)?;
    let tile_rows =
        usize::try_from(le_u32(&bytes[84..88])?).map_err(|_| GmtedError::ResourceLimit)?;
    if tile_columns != descriptor.tile_columns() || tile_rows != descriptor.tile_rows() {
        return Err(GmtedError::InvalidFormat(
            "prepared-grid tile geometry is invalid".into(),
        ));
    }
    let tile_count =
        usize::try_from(le_u64(&bytes[88..96])?).map_err(|_| GmtedError::ResourceLimit)?;
    Ok(ParsedHeader {
        descriptor,
        tile_count,
        index_offset: le_u64(&bytes[96..104])?,
        data_offset: le_u64(&bytes[104..112])?,
    })
}

fn le_u32(bytes: &[u8]) -> Result<u32, GmtedError> {
    let bytes: [u8; 4] = bytes
        .try_into()
        .map_err(|_| GmtedError::InvalidFormat("invalid u32 field".into()))?;
    Ok(u32::from_le_bytes(bytes))
}

fn le_u64(bytes: &[u8]) -> Result<u64, GmtedError> {
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| GmtedError::InvalidFormat("invalid u64 field".into()))?;
    Ok(u64::from_le_bytes(bytes))
}

fn le_f64(bytes: &[u8]) -> Result<f64, GmtedError> {
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| GmtedError::InvalidFormat("invalid f64 field".into()))?;
    Ok(f64::from_le_bytes(bytes))
}

/// GMTED preparation, lock, or sampling failure.
#[derive(Clone, Debug, PartialEq)]
pub enum GmtedError {
    /// Local file I/O failed.
    Io {
        /// Failing path.
        path: PathBuf,
        /// Sanitized operating-system message.
        message: String,
    },
    /// Prepared bytes violate the frozen format.
    InvalidFormat(String),
    /// DatasetLock shape, identity, or local-file verification failed.
    InvalidLock(String),
    /// One required source or prepared basename was not found.
    RequiredFileMissing(String),
    /// A required basename occurred under more than one configured root.
    RequiredFileAmbiguous(String),
    /// A source or derived file differs from its frozen identity.
    SourceIdentityMismatch(String),
    /// Opened field does not match the role selected by the lock.
    FieldMismatch {
        /// Required field.
        expected: TerrainField,
        /// Stored field.
        actual: TerrainField,
    },
    /// Mean and standard-deviation grids cannot form one dataset.
    PairMismatch,
    /// Geographic coordinate is non-finite or malformed.
    InvalidCoordinate,
    /// Requested latitude falls outside the source coverage.
    OutsideCoverage {
        /// Requested latitude.
        latitude_degrees: f64,
        /// Southern grid edge.
        southern_edge: f64,
        /// Northern grid edge.
        northern_edge: f64,
    },
    /// A source pixel is explicitly missing.
    NoData {
        /// Source column.
        column: usize,
        /// Source row.
        row: usize,
    },
    /// Decoded value is not finite.
    InvalidValue,
    /// Integer or allocation bound overflowed.
    ResourceLimit,
    /// Internal bounded cache mutex was poisoned.
    CachePoisoned,
    /// Meteorology grid cannot identify a containing interpolation cell.
    MeteorologyGrid(String),
}

impl std::fmt::Display for GmtedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::InvalidFormat(message) => formatter.write_str(message),
            Self::InvalidLock(message) => formatter.write_str(message),
            Self::RequiredFileMissing(name) => {
                write!(formatter, "required GMTED2010 file is missing: {name}")
            }
            Self::RequiredFileAmbiguous(name) => {
                write!(formatter, "required GMTED2010 file is ambiguous: {name}")
            }
            Self::SourceIdentityMismatch(name) => {
                write!(formatter, "GMTED2010 source identity mismatch: {name}")
            }
            Self::FieldMismatch { expected, actual } => {
                write!(
                    formatter,
                    "terrain field mismatch: expected {expected:?}, got {actual:?}"
                )
            }
            Self::PairMismatch => {
                formatter.write_str("GMTED mean and standard-deviation grids differ")
            }
            Self::InvalidCoordinate => formatter.write_str("invalid terrain coordinate"),
            Self::OutsideCoverage {
                latitude_degrees,
                southern_edge,
                northern_edge,
            } => write!(
                formatter,
                "latitude {latitude_degrees} is outside GMTED coverage [{southern_edge}, {northern_edge}]"
            ),
            Self::NoData { column, row } => {
                write!(formatter, "GMTED source pixel ({column}, {row}) is missing")
            }
            Self::InvalidValue => formatter.write_str("invalid GMTED value"),
            Self::ResourceLimit => formatter.write_str("GMTED resource limit exceeded"),
            Self::CachePoisoned => formatter.write_str("GMTED tile cache is unavailable"),
            Self::MeteorologyGrid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for GmtedError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use tempfile::tempdir;

    use super::*;
    use trajecta_case::model::meteorology::DomainId;

    fn write_source_manifest(mean_path: &Path, deviation_path: &Path, manifest_path: &Path) {
        let prepared = [
            (
                "mean_elevation",
                GMTED2010_MEAN_GRID_FILE,
                TerrainField::MeanElevation,
                mean_path,
            ),
            (
                "standard_deviation",
                GMTED2010_STANDARD_DEVIATION_GRID_FILE,
                TerrainField::StandardDeviation,
                deviation_path,
            ),
        ]
        .into_iter()
        .map(|(role, name, field, path)| {
            let descriptor = TerrainGrid::open(path, field).unwrap().descriptor().clone();
            let (size_bytes, sha256) = sha256_file_streaming(path).unwrap();
            serde_json::json!({
                "role": role,
                "relative_path": name,
                "size_bytes": size_bytes,
                "sha256": sha256,
                "grid": {
                    "width": descriptor.width,
                    "height": descriptor.height,
                    "tile_width": descriptor.tile_width,
                    "tile_height": descriptor.tile_height,
                    "longitude_origin_degrees": descriptor.longitude_origin_degrees,
                    "latitude_origin_degrees": descriptor.latitude_origin_degrees,
                    "longitude_spacing_degrees": descriptor.longitude_spacing_degrees,
                    "latitude_spacing_degrees": descriptor.latitude_spacing_degrees,
                    "nodata": descriptor.nodata,
                }
            })
        })
        .collect::<Vec<_>>();
        let source_member = |path: &str| {
            serde_json::json!({
                "path": path,
                "size_bytes": 1,
                "compressed_size_bytes": 1,
                "crc32": "00000000"
            })
        };
        let manifest = serde_json::json!({
            "schema_version": SOURCE_MANIFEST_SCHEMA,
            "dataset_id": GMTED2010_DATASET_ID,
            "prepared_format": GMTED2010_GRID_FORMAT_ID,
            "source": {
                "provider": SOURCE_PROVIDER,
                "product": "Global Multi-resolution Terrain Elevation Data 2010",
                "product_url": "https://pubs.usgs.gov/of/2011/1073/",
                "catalog_url": "https://www.usgs.gov/coastal-changes-and-impacts/gmted2010",
                "transfer_source": null,
                "attribution": SOURCE_ATTRIBUTION
            },
            "sources": [
                {
                    "role": "mean_source_archive",
                    "file_name": GMTED2010_MEAN_SOURCE_ARCHIVE,
                    "size_bytes": GMTED2010_MEAN_SOURCE_BYTES,
                    "sha256": GMTED2010_MEAN_SOURCE_SHA256,
                    "member_manifest_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                    "members": [source_member("mn30_grd/hdr.adf")]
                },
                {
                    "role": "standard_deviation_source_archive",
                    "file_name": GMTED2010_STANDARD_DEVIATION_SOURCE_ARCHIVE,
                    "size_bytes": GMTED2010_STANDARD_DEVIATION_SOURCE_BYTES,
                    "sha256": GMTED2010_STANDARD_DEVIATION_SOURCE_SHA256,
                    "member_manifest_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                    "members": [source_member("sd30_grd/hdr.adf")]
                }
            ],
            "prepared_files": prepared
        });
        let mut bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        bytes.push(b'\n');
        fs::write(manifest_path, bytes).unwrap();
    }

    fn write_grid(path: &Path, field: TerrainField, width: usize, height: usize, values: &[i16]) {
        let descriptor = TerrainGridDescriptor {
            field,
            width,
            height,
            tile_width: 2,
            tile_height: 2,
            longitude_origin_degrees: -180.0,
            latitude_origin_degrees: 90.0,
            longitude_spacing_degrees: 360.0 / width as f64,
            latitude_spacing_degrees: -180.0 / height as f64,
            nodata: -32768,
        };
        descriptor.validate().unwrap();
        assert_eq!(values.len(), width * height);
        let tile_count = descriptor.tile_columns() * descriptor.tile_rows();
        let data_offset = HEADER_BYTES + tile_count * INDEX_ENTRY_BYTES;
        let mut compressed = Vec::new();
        for tile_row in 0..descriptor.tile_rows() {
            for tile_column in 0..descriptor.tile_columns() {
                let mut raw = Vec::new();
                for local_row in 0..descriptor.tile_height {
                    for local_column in 0..descriptor.tile_width {
                        let row = tile_row * descriptor.tile_height + local_row;
                        let column = tile_column * descriptor.tile_width + local_column;
                        let value = if row < height && column < width {
                            values[row * width + column]
                        } else {
                            descriptor.nodata
                        };
                        raw.extend_from_slice(&value.to_le_bytes());
                    }
                }
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(&raw).unwrap();
                compressed.push((encoder.finish().unwrap(), raw.len()));
            }
        }
        let mut header = [0_u8; HEADER_BYTES];
        header[0..16].copy_from_slice(MAGIC);
        header[16..20].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        header[20..24].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        header[24..28].copy_from_slice(&(width as u32).to_le_bytes());
        header[28..32].copy_from_slice(&(height as u32).to_le_bytes());
        header[32..36].copy_from_slice(&(descriptor.tile_width as u32).to_le_bytes());
        header[36..40].copy_from_slice(&(descriptor.tile_height as u32).to_le_bytes());
        header[40..42].copy_from_slice(&descriptor.nodata.to_le_bytes());
        header[42] = field as u8;
        header[43] = COMPRESSION_ZLIB;
        header[48..56].copy_from_slice(&descriptor.longitude_origin_degrees.to_le_bytes());
        header[56..64].copy_from_slice(&descriptor.latitude_origin_degrees.to_le_bytes());
        header[64..72].copy_from_slice(&descriptor.longitude_spacing_degrees.to_le_bytes());
        header[72..80].copy_from_slice(&descriptor.latitude_spacing_degrees.to_le_bytes());
        header[80..84].copy_from_slice(&(descriptor.tile_columns() as u32).to_le_bytes());
        header[84..88].copy_from_slice(&(descriptor.tile_rows() as u32).to_le_bytes());
        header[88..96].copy_from_slice(&(tile_count as u64).to_le_bytes());
        header[96..104].copy_from_slice(&(HEADER_BYTES as u64).to_le_bytes());
        header[104..112].copy_from_slice(&(data_offset as u64).to_le_bytes());

        let mut output = File::create(path).unwrap();
        output.write_all(&header).unwrap();
        let mut offset = data_offset as u64;
        for (bytes, raw_len) in &compressed {
            output.write_all(&offset.to_le_bytes()).unwrap();
            output
                .write_all(&(bytes.len() as u32).to_le_bytes())
                .unwrap();
            output.write_all(&(*raw_len as u32).to_le_bytes()).unwrap();
            offset += bytes.len() as u64;
        }
        for (bytes, _) in compressed {
            output.write_all(&bytes).unwrap();
        }
        output.sync_all().unwrap();
    }

    #[test]
    fn prepared_grid_interpolates_and_area_weights_exact_source_values() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("mean.tgrid");
        write_grid(
            &path,
            TerrainField::MeanElevation,
            4,
            2,
            &[0, 10, 20, 30, 40, 50, 60, 70],
        );
        let grid = TerrainGrid::open(&path, TerrainField::MeanElevation).unwrap();
        assert_eq!(grid.sample_bilinear(-135.0, 45.0).unwrap(), 0.0);
        assert_eq!(grid.sample_bilinear(-90.0, 0.0).unwrap(), 25.0);
        assert_eq!(grid.sample_bilinear(270.0, 0.0).unwrap(), 25.0);
        assert!((grid.spherical_mean(-180.0, 0.0, -90.0, 90.0).unwrap() - 25.0).abs() < 1.0e-12);
    }

    #[test]
    fn paired_dataset_uses_meteorology_cell_and_rejects_missing_source_pixels() {
        let directory = tempdir().unwrap();
        let mean_path = directory.path().join("mean.tgrid");
        let deviation_path = directory.path().join("deviation.tgrid");
        write_grid(
            &mean_path,
            TerrainField::MeanElevation,
            4,
            2,
            &[0, 10, 20, 30, 40, 50, 60, 70],
        );
        write_grid(
            &deviation_path,
            TerrainField::StandardDeviation,
            4,
            2,
            &[1, 2, 3, 4, 5, 6, 7, 8],
        );
        let dataset = Gmted2010::open(&mean_path, &deviation_path).unwrap();
        let geometry = DomainGeometry {
            domain: DomainId("global".into()),
            longitude_origin_degrees: -180.0,
            latitude_origin_degrees: 45.0,
            longitude_spacing_degrees: 90.0,
            latitude_spacing_degrees: -90.0,
            nx: 4,
            ny: 2,
            periodic_longitude: true,
            halo_cells: 0,
        };
        let sample = dataset
            .sample_for_meteorology_cell(&geometry, -90.0, 0.0)
            .unwrap();
        assert_eq!(sample.point_elevation_m, 25.0);
        assert_eq!(sample.elevation_standard_deviation_m, 3.5);
        assert!(sample.meteorology_cell_mean_elevation_m.is_finite());

        write_grid(
            &deviation_path,
            TerrainField::StandardDeviation,
            4,
            2,
            &[1, -32768, 3, 4, 5, 6, 7, 8],
        );
        let dataset = Gmted2010::open(&mean_path, &deviation_path).unwrap();
        assert!(matches!(
            dataset.sample_for_meteorology_cell(&geometry, -90.0, 0.0),
            Err(GmtedError::NoData { .. })
        ));
    }

    #[test]
    fn three_runtime_files_build_and_reopen_the_global_lock() {
        let directory = tempdir().unwrap();
        let mean_path = directory.path().join(GMTED2010_MEAN_GRID_FILE);
        let deviation_path = directory
            .path()
            .join(GMTED2010_STANDARD_DEVIATION_GRID_FILE);
        let manifest_path = directory.path().join(GMTED2010_SOURCE_MANIFEST_FILE);
        write_grid(
            &mean_path,
            TerrainField::MeanElevation,
            4,
            2,
            &[0, 10, 20, 30, 40, 50, 60, 70],
        );
        write_grid(
            &deviation_path,
            TerrainField::StandardDeviation,
            4,
            2,
            &[1, 2, 3, 4, 5, 6, 7, 8],
        );
        write_source_manifest(&mean_path, &deviation_path, &manifest_path);

        let roots = BTreeMap::from([(
            DataRootId("auxiliary".into()),
            directory.path().to_path_buf(),
        )]);
        let lock = build_global_dataset_lock(
            &roots,
            GeneratorInfo {
                tool: "gmted2010-contract-test".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
        )
        .unwrap();
        assert_eq!(lock.files.len(), 3);
        assert!(lock.files.iter().all(|file| {
            !matches!(
                file.relative_path.to_str(),
                Some(GMTED2010_MEAN_SOURCE_ARCHIVE | GMTED2010_STANDARD_DEVIATION_SOURCE_ARCHIVE)
            )
        }));

        let reopened = open_from_dataset_lock(&lock, directory.path(), &roots).unwrap();
        assert_eq!(reopened.mean_elevation_m(-90.0, 0.0).unwrap(), 25.0);
    }
}
