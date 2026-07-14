//! Real CF-NetCDF anchors produced offline from real GRIB meteorology.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::{Capability, CapabilitySet};
use trajecta_met::io::frame_loader::{FrameLoadRequest, FrameLoader};
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::io::netcdf::{NetCdfAssembly, NetCdfReader};
use trajecta_met::io::reader::{DecodeRequest, MetReader, SourceFormat};
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};
use trajecta_met::vertical::VerticalTopology;

fn require_real_met() -> bool {
    matches!(
        std::env::var("TRAJECTA_REQUIRE_REAL_MET").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn skip_or_fail(missing: &str) {
    if require_real_met() {
        panic!("TRAJECTA_REQUIRE_REAL_MET set but missing {missing}");
    }
}

/// Large multi-hundred-MB year files run only under explicit switches so default
/// `cargo test` stays fast even if local anchors exist.
fn large_real_met_enabled() -> bool {
    require_real_met()
        || matches!(
            std::env::var("TRAJECTA_LARGE_REAL_MET").as_deref(),
            Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
        )
}

fn skip_large_unless_enabled() -> bool {
    !large_real_met_enabled()
}

fn cfsr_pressure_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_CFSR_PRESSURE_NETCDF_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data/cfsr-cf-pressure-netcdf3")
}

fn hybrid_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_ERA5_HYBRID_NETCDF4_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data/era5-cf-hybrid-netcdf4")
}

fn cfsr_derived_multifile_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_CFSR_DERIVED_MULTIFILE_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data/cfsr-derived-cf-multifile")
}

fn lock_and_frame(root: PathBuf, start: i64, end: i64, expected_profile: &str, min_frames: usize) {
    lock_and_frame_with_backend(
        root,
        start,
        end,
        expected_profile,
        min_frames,
        MeteorologyReaderBackend::Rust,
        Capability::Transport,
        7,
    );
}

#[allow(clippy::too_many_arguments)]
fn lock_and_frame_with_backend(
    root: PathBuf,
    start: i64,
    end: i64,
    expected_profile: &str,
    min_frames: usize,
    backend: MeteorologyReaderBackend,
    capability: Capability,
    min_fields: usize,
) {
    let profiles = ProfileCatalog::load(&[]).unwrap();
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(capability);
    let inspector = ReaderMetadataInspector::new(backend);
    let mut hash_cache = FileHashCache::new();
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(
        &DatasetLockRequest {
            identity: DatasetIdentity {
                id: DatasetRef("netcdf-pipeline".into()),
                source: "offline CF-NetCDF from real GRIB".into(),
                source_url: None,
                attribution: None,
            },
            generator: GeneratorInfo {
                tool: "real_netcdf_pipeline".into(),
                version: "0".into(),
            },
            data_roots: BTreeMap::from([(DataRootId("met".into()), root.clone())]),
            coverage: LockCoverageRequest {
                start: Timestamp::new(start, 0).unwrap(),
                end: Timestamp::new(end, 0).unwrap(),
                interpolation_before_frames: 0,
                interpolation_after_frames: 0,
            },
            required_capabilities: capabilities,
            force_rehash: false,
        },
    );
    assert!(
        outcome.is_success(),
        "lock failed: {:?}",
        outcome.diagnostics
    );
    let lock = outcome.lock.unwrap();
    assert_eq!(lock.profile.name, expected_profile);
    let domain = DomainId("met".into());
    let roots = BTreeMap::from([(DataRootId("met".into()), root)]);
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir: roots.get(&DataRootId("met".into())).unwrap(),
        data_roots: &roots,
        domain: &domain,
        required_capabilities: capabilities,
    });
    assert!(
        inventory.is_success(),
        "{:?}",
        inventory.diagnostics.sorted()
    );
    let catalog = inventory.catalog.unwrap();
    let frames = &catalog.domains.get(&domain).unwrap().frames;
    assert!(
        frames.len() >= min_frames,
        "expected >= {min_frames} frames, got {}",
        frames.len()
    );
    let profile = profiles
        .get(&ProfileName(lock.profile.name.clone()))
        .unwrap();
    for descriptor in frames.values() {
        let frame = FrameLoader::load(FrameLoadRequest {
            descriptor,
            profile,
            required_capabilities: capabilities,
            backend,
            previous_frame: None,
        })
        .expect("frame");
        assert!(
            frame.fields().len() >= min_fields,
            "expected >= {min_fields} fields, got {}",
            frame.fields().len()
        );
    }
}

#[test]
fn real_cfsr_pressure_netcdf3_rust_full_chain() {
    let root = cfsr_pressure_dir();
    let file00 = root.join("pgbl_2009010100.nc");
    let file06 = root.join("pgbl_2009010106.nc");
    if !file00.is_file() || !file06.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
    let meta = reader.inspect(&file00).expect("inspect");
    assert_eq!(meta.format, SourceFormat::NetCdf3);
    assert_eq!(
        meta.attributes.get("dataset_family").map(String::as_str),
        Some("cfsr_cf_pressure_netcdf")
    );
    let index = reader.build_index(&file00).expect("index");
    let VerticalTopology::PressureLevels(levels) = index.vertical_topology().unwrap() else {
        panic!("expected pressure levels");
    };
    assert_eq!(levels.pressure_pa.len(), 37);
    assert!(levels.pressure_pa.windows(2).all(|pair| pair[1] > pair[0]));

    for variable in ["t", "u", "v", "q", "w", "sp", "z"] {
        let decoded = reader
            .decode(
                &file00,
                index.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), variable.into())],
                    valid_time: meta.valid_times.first().copied(),
                },
            )
            .unwrap_or_else(|error| panic!("decode {variable}: {error:?}"));
        assert!(decoded.values.iter().any(|value| *value != 0.0));
    }

    lock_and_frame(
        root,
        1_230_768_000,
        1_230_811_200,
        "cfsr-cf-pressure-netcdf-v0",
        2,
    );
}

#[test]
fn real_era5_hybrid_netcdf4_rust_two_times_and_frame() {
    let root = hybrid_dir();
    let file00 = root.join("EA18120100.nc");
    let file06 = root.join("EA18120106.nc");
    if !file00.is_file() || !file06.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
    let meta00 = reader.inspect(&file00).expect("inspect 00");
    let meta06 = reader.inspect(&file06).expect("inspect 06");
    assert_eq!(meta00.format, SourceFormat::NetCdf4);
    assert_eq!(meta06.format, SourceFormat::NetCdf4);
    assert_ne!(meta00.valid_times, meta06.valid_times);

    let index00 = reader.build_index(&file00).expect("index 00");
    let index06 = reader.build_index(&file06).expect("index 06");
    let VerticalTopology::HybridPressure(topology) = index00.vertical_topology().unwrap() else {
        panic!("expected hybrid");
    };
    assert_eq!(topology.coefficients.a_half_pa.len(), 138);
    assert_eq!(topology.active_full_levels.len(), 8);

    let t00 = reader
        .decode(
            &file00,
            index00.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "t".into())],
                valid_time: meta00.valid_times.first().copied(),
            },
        )
        .expect("t00");
    let t06 = reader
        .decode(
            &file06,
            index06.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "t".into())],
                valid_time: meta06.valid_times.first().copied(),
            },
        )
        .expect("t06");
    assert_ne!(t00.values.as_ref(), t06.values.as_ref());

    for variable in ["t", "u", "v", "q", "etadot", "sp", "z"] {
        reader
            .decode(
                &file00,
                index00.as_ref(),
                &DecodeRequest {
                    source_identity: vec![("variable".into(), variable.into())],
                    valid_time: None,
                },
            )
            .unwrap_or_else(|error| panic!("decode {variable}: {error:?}"));
    }

    // Hybrid files are 3-hourly in the ERA5 set; 00 and 06 exist (gap at 03 is OK
    // if only two files are present — use coverage that only expects those times
    // by placing only these two files in the root).
    // 00/03/06 at 3-hourly interval when all three anchors are present.
    lock_and_frame(
        root,
        1_543_622_400,
        1_543_644_000 + 1,
        "era5-cf-hybrid-netcdf4-v0",
        3,
    );
}

#[test]
fn real_cfsr_derived_multifile_assembly_and_roles() {
    let root = cfsr_derived_multifile_dir();
    if !root.is_dir() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let files_00 = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "nc")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains("2009010100"))
        })
        .collect::<Vec<_>>();
    if files_00.is_empty() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let assembly = NetCdfAssembly::assemble(&files_00).expect("assemble");
    for role in ["t", "u", "v", "q", "w", "sp", "z"] {
        assert!(
            assembly.files_by_role.contains_key(role),
            "missing role {role}"
        );
    }
    assert!(!assembly.coordinate_signature.is_empty());

    let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
    let mut roles = BTreeMap::new();
    for path in &files_00 {
        let meta = reader.inspect(path).expect("inspect member");
        assert_eq!(
            meta.attributes.get("dataset_family").map(String::as_str),
            Some("cfsr_derived_cf_multifile_netcdf")
        );
        assert_eq!(meta.roles.len(), 1, "{path:?} roles={:?}", meta.roles);
        let role = meta.roles[0].clone();
        assert!(
            roles.insert(role.clone(), path.clone()).is_none(),
            "duplicate role {role}"
        );
    }

    lock_and_frame(
        root,
        1_230_768_000,
        1_230_811_200,
        "cfsr-derived-cf-multifile-netcdf-v0",
        2,
    );
}

#[test]
fn real_cfsr_derived_multifile_assembly_rejects_inconsistencies() {
    let root = cfsr_derived_multifile_dir();
    let t00 = root.join("t_2009010100.nc");
    let u00 = root.join("u_2009010100.nc");
    if !t00.is_file() || !u00.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    // Duplicate role: same file twice.
    let err = NetCdfAssembly::assemble(&[t00.clone(), t00.clone()]).unwrap_err();
    assert!(
        format!("{err:?}").contains("duplicate"),
        "expected duplicate role error, got {err:?}"
    );
    // Empty set.
    let err = NetCdfAssembly::assemble(&[]).unwrap_err();
    assert!(
        format!("{err:?}").contains("at least one"),
        "expected empty assembly error, got {err:?}"
    );
    let t06 = root.join("t_2009010106.nc");
    if t06.is_file() {
        let err = NetCdfAssembly::assemble(&[t00.clone(), t06]).unwrap_err();
        assert!(
            format!("{err:?}").contains("validity times")
                || format!("{err:?}").contains("duplicate"),
            "expected time inconsistency, got {err:?}"
        );
    }
    // Happy path still unique roles for two distinct members.

    let ok = NetCdfAssembly::assemble(&[t00, u00]).expect("assemble two roles");
    assert_eq!(ok.files_by_role.len(), 2);
}

fn era5_cds_pressure_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_ERA5_PRESSURE_NETCDF_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data/era5-cds-pressure-classic")
}

#[test]
fn real_era5_cds_pressure_classic_rust_full_chain() {
    let root = era5_cds_pressure_dir();
    let pressure = root.join("era5_pressure_20181201.nc");
    let surface = root.join("era5_surface_20181201.nc");
    if !pressure.is_file() || !surface.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }

    let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
    let meta_p = reader.inspect(&pressure).expect("inspect pressure");
    let meta_s = reader.inspect(&surface).expect("inspect surface");
    assert_eq!(
        meta_p.attributes.get("dataset_family").map(String::as_str),
        Some("era5_cf_pressure_netcdf")
    );
    assert_eq!(
        meta_s.attributes.get("dataset_family").map(String::as_str),
        Some("era5_cf_pressure_netcdf")
    );
    assert_eq!(meta_p.roles, vec!["pressure".to_owned()]);
    assert_eq!(meta_s.roles, vec!["surface".to_owned()]);
    assert_eq!(meta_p.valid_times.len(), 2);
    assert_eq!(meta_s.valid_times.len(), 2);
    assert_ne!(meta_p.valid_times[0], meta_p.valid_times[1]);

    let index_p = reader.build_index(&pressure).expect("index pressure");
    let index_s = reader.build_index(&surface).expect("index surface");
    let t0 = meta_p.valid_times[0];
    let t1 = meta_p.valid_times[1];

    let temp0 = reader
        .decode(
            &pressure,
            index_p.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "t".into())],
                valid_time: Some(t0),
            },
        )
        .expect("t0");
    let temp1 = reader
        .decode(
            &pressure,
            index_p.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "t".into())],
                valid_time: Some(t1),
            },
        )
        .expect("t1");
    assert_ne!(temp0.values.as_ref(), temp1.values.as_ref());
    assert!(matches!(
        temp0.layout,
        trajecta_met::frame::ArrayLayout::Full3D { levels, ny, nx }
            if levels == 37 && ny > 1 && nx > 1
    ));

    // Pressure topology must be ascending Pa (model top -> near surface).
    let vertical = index_p
        .vertical_topology()
        .expect("pressure vertical topology");
    match vertical {
        VerticalTopology::PressureLevels(levels) => {
            assert_eq!(levels.pressure_pa.len(), 37);
            for window in levels.pressure_pa.windows(2) {
                assert!(
                    window[0] < window[1],
                    "pressure levels must strictly increase in Pa: {} !< {}",
                    window[0],
                    window[1]
                );
            }
            assert!(levels.pressure_pa[0] < 200.0);
            assert!(*levels.pressure_pa.last().unwrap() > 90_000.0);
        }
        other => panic!("expected pressure levels, got {other:?}"),
    }

    let omega = reader
        .decode(
            &pressure,
            index_p.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "w".into())],
                valid_time: Some(t0),
            },
        )
        .expect("omega");
    assert_eq!(omega.source_unit.symbol(), "Pa s-1");
    assert!(matches!(
        omega.layout,
        trajecta_met::frame::ArrayLayout::Full3D { .. }
    ));

    let sp = reader
        .decode(
            &surface,
            index_s.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "sp".into())],
                valid_time: Some(t0),
            },
        )
        .expect("sp");
    assert!(matches!(
        sp.layout,
        trajecta_met::frame::ArrayLayout::Horizontal2D { .. }
    ));
    let z = reader
        .decode(
            &surface,
            index_s.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "z".into())],
                valid_time: Some(t0),
            },
        )
        .expect("z");
    assert!(matches!(
        z.layout,
        trajecta_met::frame::ArrayLayout::Horizontal2D { .. }
    ));

    // 2018-12-01 00/06 UTC
    lock_and_frame(
        root,
        1_543_622_400,
        1_543_644_000 + 1,
        "era5-cf-pressure-netcdf-v0",
        2,
    );
}

#[cfg(feature = "native-netcdf")]
fn era5_cds_pressure_official_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_ERA5_PRESSURE_OFFICIAL_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-data/era5-cds-pressure-official")
}

/// Official CDS NetCDF4 originals via native netCDF-C (not the classic conversion).
#[cfg(feature = "native-netcdf")]
#[test]
fn real_era5_cds_pressure_official_native_full_chain() {
    let root = era5_cds_pressure_official_dir();
    let pressure = root.join("era5_pressure_20181201.nc");
    let surface = root.join("era5_surface_20181201.nc");
    if !pressure.is_file() || !surface.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let reader = NetCdfReader::new(MeteorologyReaderBackend::Native);
    let meta_p = reader
        .inspect(&pressure)
        .expect("inspect official pressure");
    let meta_s = reader.inspect(&surface).expect("inspect official surface");
    assert_eq!(meta_p.valid_times.len(), 2);
    assert_eq!(meta_s.valid_times.len(), 2);
    assert_eq!(
        meta_p.attributes.get("dataset_family").map(String::as_str),
        Some("era5_cf_pressure_netcdf")
    );
    let index_p = reader.build_index(&pressure).expect("index pressure");
    let index_s = reader.build_index(&surface).expect("index surface");
    for valid_time in &meta_p.valid_times {
        for variable in ["t", "u", "v", "q", "w"] {
            reader
                .decode(
                    &pressure,
                    index_p.as_ref(),
                    &DecodeRequest {
                        source_identity: vec![("variable".into(), variable.into())],
                        valid_time: Some(*valid_time),
                    },
                )
                .unwrap_or_else(|error| panic!("official {variable}: {error:?}"));
        }
        for variable in ["sp", "z"] {
            reader
                .decode(
                    &surface,
                    index_s.as_ref(),
                    &DecodeRequest {
                        source_identity: vec![("variable".into(), variable.into())],
                        valid_time: Some(*valid_time),
                    },
                )
                .unwrap_or_else(|error| panic!("official {variable}: {error:?}"));
        }
    }
    lock_and_frame_with_backend(
        root,
        1_543_622_400,
        1_543_644_000 + 1,
        "era5-cf-pressure-netcdf-v0",
        2,
        MeteorologyReaderBackend::Native,
        Capability::Transport,
        7,
    );
}

#[cfg(feature = "native-netcdf")]
fn assert_rust_native_field_diff(file: &Path, variables: &[&str], abs_tol: f64, rel_tol: f64) {
    let rust = NetCdfReader::new(MeteorologyReaderBackend::Rust);
    let native = NetCdfReader::new(MeteorologyReaderBackend::Native);
    let rust_index = rust.build_index(file).expect("rust index");
    let native_index = native.build_index(file).expect("native index");
    let rust_meta = rust.inspect(file).expect("rust inspect");
    let native_meta = native.inspect(file).expect("native inspect");

    assert_eq!(
        rust_meta.valid_times, native_meta.valid_times,
        "valid_times"
    );
    assert_eq!(rust_meta.grid, native_meta.grid, "grid signature");
    assert_eq!(
        rust_meta.vertical, native_meta.vertical,
        "vertical signature"
    );
    assert_eq!(rust_meta.format, native_meta.format, "format");
    assert_eq!(
        rust_meta.attributes.get("dataset_family"),
        native_meta.attributes.get("dataset_family"),
        "dataset_family"
    );

    assert_eq!(
        rust_index.grid_signature(),
        native_index.grid_signature(),
        "index grid signature"
    );
    assert_eq!(
        rust_index.vertical_signature(),
        native_index.vertical_signature(),
        "index vertical signature"
    );
    assert_eq!(
        rust_index.grid_geometry(),
        native_index.grid_geometry(),
        "index grid geometry"
    );
    assert_eq!(
        format!("{:?}", rust_index.vertical_topology()),
        format!("{:?}", native_index.vertical_topology()),
        "index vertical topology"
    );

    // Decode every valid time and every field; keep indexes alive across passes.
    for valid_time in &rust_meta.valid_times {
        for variable in variables {
            let request = DecodeRequest {
                source_identity: vec![("variable".into(), (*variable).into())],
                valid_time: Some(*valid_time),
            };
            let rust_field = rust
                .decode(file, rust_index.as_ref(), &request)
                .unwrap_or_else(|error| panic!("rust {variable}@{valid_time:?}: {error:?}"));
            let native_field = native
                .decode(file, native_index.as_ref(), &request)
                .unwrap_or_else(|error| panic!("native {variable}@{valid_time:?}: {error:?}"));
            assert_eq!(
                rust_field.layout, native_field.layout,
                "{variable}@{valid_time:?} layout"
            );
            assert_eq!(
                rust_field.valid, native_field.valid,
                "{variable}@{valid_time:?} mask"
            );
            assert_eq!(
                rust_field.source_unit.symbol(),
                native_field.source_unit.symbol(),
                "{variable}@{valid_time:?} unit"
            );
            assert_eq!(
                rust_field.temporal, native_field.temporal,
                "{variable}@{valid_time:?} temporal"
            );
            assert_eq!(
                rust_field.values.len(),
                native_field.values.len(),
                "{variable}@{valid_time:?} len"
            );
            let mut max_abs = 0.0_f64;
            let mut max_rel = 0.0_f64;
            for (a, b) in rust_field.values.iter().zip(native_field.values.iter()) {
                let abs = (a - b).abs();
                max_abs = max_abs.max(abs);
                let denom = a.abs().max(b.abs()).max(1.0);
                max_rel = max_rel.max(abs / denom);
            }
            assert!(
                max_abs <= abs_tol,
                "{variable}@{valid_time:?} max_abs={max_abs} > {abs_tol}"
            );
            assert!(
                max_rel <= rel_tol,
                "{variable}@{valid_time:?} max_rel={max_rel} > {rel_tol}"
            );
        }
    }

    // Second multi-field pass on the same workers/indexes.
    if let Some(valid_time) = rust_meta.valid_times.first().copied() {
        for variable in variables {
            let request = DecodeRequest {
                source_identity: vec![("variable".into(), (*variable).into())],
                valid_time: Some(valid_time),
            };
            let _ = rust
                .decode(file, rust_index.as_ref(), &request)
                .expect("rust re-decode");
            let _ = native
                .decode(file, native_index.as_ref(), &request)
                .expect("native re-decode");
        }
    }
}

#[cfg(feature = "native-netcdf")]
#[test]
fn real_era5_hybrid_netcdf4_rust_native_field_diff() {
    let root = hybrid_dir();
    let file = root.join("EA18120100.nc");
    if !file.is_file() {
        skip_or_fail(file.display().to_string().as_str());
        return;
    }
    // float64 classic->netcdf4 path: fixed absolute tolerance 1e-10.
    assert_rust_native_field_diff(
        &file,
        &["t", "u", "v", "q", "etadot", "sp", "z"],
        1.0e-10,
        1.0e-10,
    );
}

#[cfg(feature = "native-netcdf")]
#[test]
fn real_era5_cds_pressure_rust_native_field_diff() {
    let root = era5_cds_pressure_dir();
    let pressure = root.join("era5_pressure_20181201.nc");
    let surface = root.join("era5_surface_20181201.nc");
    if !pressure.is_file() || !surface.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    // CDS float32 packing: fixed abs 1e-4, rel 1e-5 of magnitude.
    assert_rust_native_field_diff(&pressure, &["t", "u", "v", "q", "w"], 1.0e-4, 1.0e-5);
    assert_rust_native_field_diff(&surface, &["sp", "z"], 1.0e-4, 1.0e-5);
}

#[cfg(feature = "native-netcdf")]
#[test]
fn real_cfsr_derived_multifile_rust_native_field_diff() {
    let root = cfsr_derived_multifile_dir();
    let file = root.join("t_2009010100.nc");
    if !file.is_file() {
        skip_or_fail(file.display().to_string().as_str());
        return;
    }
    assert_rust_native_field_diff(&file, &["t"], 1.0e-10, 1.0e-10);
    for role in ["u", "v", "q", "w", "sp", "z"] {
        let path = root.join(format!("{role}_2009010100.nc"));
        if path.is_file() {
            assert_rust_native_field_diff(&path, &[role], 1.0e-10, 1.0e-10);
        }
    }
}

#[cfg(feature = "native-netcdf")]
#[test]
fn real_cfsr_pressure_rust_native_field_diff() {
    let root = cfsr_pressure_dir();
    let file = root.join("pgbl_2009010100.nc");
    if !file.is_file() {
        skip_or_fail(file.display().to_string().as_str());
        return;
    }
    // Classic NetCDF3 float64 offline conversion: fixed abs/rel 1e-10.
    assert_rust_native_field_diff(
        &file,
        &["t", "u", "v", "q", "w", "sp", "z"],
        1.0e-10,
        1.0e-10,
    );
}

fn noaa_psl_ncep_r1_dir() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_NOAA_PSL_NCEP_R1_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-data/noaa-psl-ncep-reanalysis1-official")
}

fn noaa_psl_ncep_r1_compatible_dir() -> PathBuf {
    noaa_psl_ncep_r1_dir().join("compatible-17level-with-pres")
}

/// Official PSL NCEP R1 originals: inspect/index/decode without restamping attributes.
#[test]
fn real_noaa_psl_ncep_r1_official_inspect_index_decode() {
    if skip_large_unless_enabled() {
        return;
    }
    let root = noaa_psl_ncep_r1_dir();
    let air = root.join("air.2009.nc");
    let pres = root.join("pres.sfc.2009.nc");
    if !air.is_file() || !pres.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let reader = NetCdfReader::new(MeteorologyReaderBackend::Rust);
    let meta = reader.inspect(&air).expect("inspect air");
    assert_eq!(
        meta.attributes.get("dataset_title").map(String::as_str),
        Some("NCEP-NCAR Reanalysis 1")
    );
    assert_eq!(meta.roles, vec!["air".to_owned()]);
    assert!(
        meta.valid_times.len() >= 2,
        "expected full year 6-hourly times"
    );
    // 2009-01-01 00Z and 06Z
    assert_eq!(
        meta.valid_times[0].seconds_since_unix_epoch(),
        1_230_768_000
    );
    assert_eq!(
        meta.valid_times[1].seconds_since_unix_epoch(),
        1_230_768_000 + 21_600
    );

    let index = reader.build_index(&air).expect("index air");
    match index.vertical_topology().expect("vertical") {
        VerticalTopology::PressureLevels(levels) => {
            assert_eq!(levels.pressure_pa.len(), 17);
            for window in levels.pressure_pa.windows(2) {
                assert!(window[0] < window[1], "Pa must increase top->surface");
            }
            // millibar 10 -> 1000 becomes 1000..100000 Pa ascending
            assert!((levels.pressure_pa[0] - 1000.0).abs() < 1.0e-6);
            assert!((levels.pressure_pa[16] - 100_000.0).abs() < 1.0e-3);
        }
        other => panic!("expected pressure levels, got {other:?}"),
    }

    let t0 = meta.valid_times[0];
    let t1 = meta.valid_times[1];
    let field0 = reader
        .decode(
            &air,
            index.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "air".into())],
                valid_time: Some(t0),
            },
        )
        .expect("decode air t0");
    let field1 = reader
        .decode(
            &air,
            index.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "air".into())],
                valid_time: Some(t1),
            },
        )
        .expect("decode air t1");
    assert_ne!(field0.values.as_ref(), field1.values.as_ref());
    assert_eq!(field0.source_unit.symbol(), "K");
    assert!(matches!(
        field0.layout,
        trajecta_met::frame::ArrayLayout::Full3D {
            levels: 17,
            ny: 73,
            nx: 144
        }
    ));

    let meta_p = reader.inspect(&pres).expect("inspect pres");
    assert_eq!(meta_p.roles, vec!["pres".to_owned()]);
    let index_p = reader.build_index(&pres).expect("index pres");
    let sp = reader
        .decode(
            &pres,
            index_p.as_ref(),
            &DecodeRequest {
                source_identity: vec![("variable".into(), "pres".into())],
                valid_time: Some(t0),
            },
        )
        .expect("decode pres");
    assert_eq!(sp.source_unit.symbol(), "Pa");
    assert!(matches!(
        sp.layout,
        trajecta_met::frame::ArrayLayout::Horizontal2D { ny: 73, nx: 144 }
    ));
}

/// Compatible 17-level pressure members + surface pressure assemble without custom role attrs.
#[test]
fn real_noaa_psl_ncep_r1_compatible_assembly_and_diagnostics_chain() {
    if skip_large_unless_enabled() {
        return;
    }
    let root = noaa_psl_ncep_r1_compatible_dir();
    if !root.is_dir() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let files = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "nc"))
        .collect::<Vec<_>>();
    if files.len() < 5 {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let assembly = NetCdfAssembly::assemble(&files).expect("assemble compatible set");
    for role in ["air", "uwnd", "vwnd", "hgt", "pres"] {
        assert!(
            assembly.files_by_role.contains_key(role),
            "missing role {role}"
        );
    }

    // Two 6-hourly frames on 2009-01-01
    lock_and_frame_with_backend(
        root,
        1_230_768_000,
        1_230_768_000 + 21_600 + 1,
        "noaa-psl-ncep-reanalysis1-multifile-netcdf-v0",
        2,
        MeteorologyReaderBackend::Rust,
        Capability::Diagnostics,
        5,
    );
}

/// Honest negatives: shum/omega vertical ladder mismatch; static hgt.sfc time mismatch.
#[test]
fn real_noaa_psl_ncep_r1_rejects_heterogeneous_vertical_and_static_surface() {
    if skip_large_unless_enabled() {
        return;
    }
    let root = noaa_psl_ncep_r1_dir();
    let air = root.join("air.2009.nc");
    let shum = root.join("shum.2009.nc");
    let omega = root.join("omega.2009.nc");
    let hgt_sfc = root.join("hgt.sfc.nc");
    if !air.is_file() || !shum.is_file() || !omega.is_file() || !hgt_sfc.is_file() {
        skip_or_fail(root.display().to_string().as_str());
        return;
    }
    let err = NetCdfAssembly::assemble(&[air.clone(), shum]).unwrap_err();
    assert!(
        format!("{err:?}").contains("vertical") || format!("{err:?}").contains("level"),
        "expected vertical/level mismatch for shum, got {err:?}"
    );
    let err = NetCdfAssembly::assemble(&[air.clone(), omega]).unwrap_err();
    assert!(
        format!("{err:?}").contains("vertical") || format!("{err:?}").contains("level"),
        "expected vertical/level mismatch for omega, got {err:?}"
    );
    let err = NetCdfAssembly::assemble(&[air, hgt_sfc]).unwrap_err();
    assert!(
        format!("{err:?}").contains("validity times")
            || format!("{err:?}").contains("time")
            || format!("{err:?}").contains("duplicate"),
        "expected time inconsistency for static hgt.sfc, got {err:?}"
    );
}

#[cfg(feature = "native-netcdf")]
#[test]
fn real_noaa_psl_ncep_r1_rust_native_field_diff() {
    if skip_large_unless_enabled() {
        return;
    }
    let root = noaa_psl_ncep_r1_dir();
    let members = [
        ("air.2009.nc", "air", 1.0e-3_f64),
        ("uwnd.2009.nc", "uwnd", 1.0e-3_f64),
        ("vwnd.2009.nc", "vwnd", 1.0e-3_f64),
        ("hgt.2009.nc", "hgt", 1.0e-2_f64),
        ("pres.sfc.2009.nc", "pres", 1.0e-2_f64),
    ];
    for (file_name, variable, abs_tol) in members {
        let path = root.join(file_name);
        if !path.is_file() {
            skip_or_fail(path.display().to_string().as_str());
            return;
        }
        let rust = NetCdfReader::new(MeteorologyReaderBackend::Rust);
        let native = NetCdfReader::new(MeteorologyReaderBackend::Native);
        let rust_index = rust.build_index(&path).expect("rust index");
        let native_index = native.build_index(&path).expect("native index");
        let rust_meta = rust.inspect(&path).expect("rust inspect");
        let native_meta = native.inspect(&path).expect("native inspect");
        assert_eq!(
            rust_meta.valid_times, native_meta.valid_times,
            "{variable} times"
        );
        assert_eq!(rust_meta.grid, native_meta.grid, "{variable} grid");
        assert_eq!(
            rust_meta.vertical, native_meta.vertical,
            "{variable} vertical meta"
        );
        assert_eq!(
            rust_index.grid_signature(),
            native_index.grid_signature(),
            "{variable} grid signature"
        );
        assert_eq!(
            rust_index.vertical_signature(),
            native_index.vertical_signature(),
            "{variable} vertical signature"
        );
        for valid_time in rust_meta.valid_times.iter().take(2) {
            let request = DecodeRequest {
                source_identity: vec![("variable".into(), variable.into())],
                valid_time: Some(*valid_time),
            };
            let a = rust
                .decode(&path, rust_index.as_ref(), &request)
                .unwrap_or_else(|e| panic!("rust {variable}: {e:?}"));
            let b = native
                .decode(&path, native_index.as_ref(), &request)
                .unwrap_or_else(|e| panic!("native {variable}: {e:?}"));
            assert_eq!(a.layout, b.layout, "{variable} layout");
            assert_eq!(a.valid, b.valid, "{variable} mask");
            assert_eq!(
                a.source_unit.symbol(),
                b.source_unit.symbol(),
                "{variable} unit"
            );
            assert_eq!(a.temporal, b.temporal, "{variable} temporal");
            let max_abs = a
                .values
                .iter()
                .zip(b.values.iter())
                .map(|(x, y)| (x - y).abs())
                .fold(0.0_f64, f64::max);
            assert!(
                max_abs <= abs_tol,
                "{variable}@{valid_time:?} max_abs={max_abs} > {abs_tol}"
            );
            let _ = rust
                .decode(&path, rust_index.as_ref(), &request)
                .expect("rust re");
            let _ = native
                .decode(&path, native_index.as_ref(), &request)
                .expect("native re");
        }
    }
}
