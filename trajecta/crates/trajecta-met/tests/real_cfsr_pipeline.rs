//! Real CFSR pgbl pressure-level GRIB2 lock-to-frame coverage.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::lockfile::{DatasetIdentity, GeneratorInfo};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_case::model::time::Timestamp;
use trajecta_met::field::{CanonicalField, Capability, CapabilitySet, FieldKey};
use trajecta_met::io::frame_loader::{FrameLoadRequest, FrameLoader};
use trajecta_met::io::grib::GribReader;
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, FileHashCache, LockCoverageRequest,
    ReaderMetadataInspector,
};
use trajecta_met::io::reader::{DecodeRequest, MetReader};
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};
use trajecta_met::vertical::VerticalTopology;

fn require_real_met() -> bool {
    matches!(
        std::env::var("TRAJECTA_REQUIRE_REAL_MET").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn fixture_directory() -> PathBuf {
    if let Ok(path) = std::env::var("TRAJECTA_REAL_CFSR_DIR") {
        return PathBuf::from(path);
    }
    if let Ok(path) = std::env::var("FLEXCTL_REAL_CFSR_DIR") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace root")
        .join("tools/flexctl/target/test-data/cfsr/20090101/raw")
}

fn fixture_file(name: &str) -> PathBuf {
    fixture_directory().join(name)
}

fn skip_or_fail(missing: &str) {
    if require_real_met() {
        panic!("TRAJECTA_REQUIRE_REAL_MET=1 but missing real CFSR asset: {missing}");
    }
}

#[test]
fn cfsr_pgbl_inspect_indexes_pressure_topology() {
    let path = fixture_file("pgbl00.gdas.2009010100.grb2");
    if !path.is_file() {
        skip_or_fail(path.to_string_lossy().as_ref());
        return;
    }
    let reader = GribReader::new(MeteorologyReaderBackend::Rust);
    let meta = reader.inspect(&path).expect("inspect");
    assert_eq!(
        meta.attributes.get("dataset_family").map(String::as_str),
        Some("cfsr_pgbl_pressure")
    );
    assert_eq!(meta.dimensions.get("pressure_level"), Some(&37));
    assert_eq!(meta.valid_times.len(), 1);
    let index = reader.build_index(&path).expect("index");
    let VerticalTopology::PressureLevels(levels) =
        trajecta_met::io::reader::SourceIndex::vertical_topology(index.as_ref())
            .cloned()
            .expect("pressure topology")
    else {
        panic!("expected pressure levels");
    };
    assert_eq!(levels.pressure_pa.len(), 37);
    assert!(levels.pressure_pa[0] < levels.pressure_pa[levels.pressure_pa.len() - 1]);
    // CFSR low-resolution pressure stack is 1..1000 hPa encoded in pascals.
    assert!((levels.pressure_pa[0] - 100.0).abs() < 1.0e-6);
    assert!((*levels.pressure_pa.last().unwrap() - 100_000.0).abs() < 1.0e-6);

    let profiles = ProfileCatalog::load(&[]).expect("profiles");
    let matched = profiles.match_source(&meta).expect("exact profile match");
    assert_eq!(matched.name().0, "cfsr-pgbl-pressure-v0");
}

#[test]
fn cfsr_pgbl_decodes_transport_fields_with_matching_level_order() {
    let path = fixture_file("pgbl00.gdas.2009010100.grb2");
    if !path.is_file() {
        skip_or_fail(path.to_string_lossy().as_ref());
        return;
    }
    let reader = GribReader::new(MeteorologyReaderBackend::Rust);
    let index = reader.build_index(&path).expect("index");
    let topology = match trajecta_met::io::reader::SourceIndex::vertical_topology(index.as_ref()) {
        Some(VerticalTopology::PressureLevels(levels)) => levels.clone(),
        other => panic!("expected pressure topology, got {other:?}"),
    };
    let request = DecodeRequest {
        source_identity: vec![
            ("discipline".into(), "0".into()),
            ("parameter_category".into(), "2".into()),
            ("parameter_number".into(), "2".into()),
            ("type_of_level".into(), "isobaric".into()),
        ],
        valid_time: None,
    };
    let decoded = reader.decode(&path, index.as_ref(), &request).expect("U");
    assert_eq!(
        decoded.layout,
        trajecta_met::frame::ArrayLayout::Full3D {
            levels: topology.pressure_pa.len(),
            ny: 73,
            nx: 144,
        }
    );
    assert_eq!(decoded.values.len(), 37 * 73 * 144);
    assert_eq!(decoded.valid.len(), decoded.values.len());
    assert!(decoded.valid.iter().filter(|v| **v).count() > 0);
}

#[test]
fn cfsr_pgbl_lock_inventory_and_frame_loading_run_end_to_end() {
    let source_directory = fixture_directory();
    let file_00 = source_directory.join("pgbl00.gdas.2009010100.grb2");
    let file_06 = source_directory.join("pgbl00.gdas.2009010106.grb2");
    if !file_00.is_file() || !file_06.is_file() {
        skip_or_fail("pgbl00 00/06 UTC pair");
        return;
    }
    // Isolate only pgbl analysis files so flux/spectral sidecars are not scanned.
    let directory = tempfile::tempdir().expect("temp data root");
    for name in ["pgbl00.gdas.2009010100.grb2", "pgbl00.gdas.2009010106.grb2"] {
        std::fs::copy(source_directory.join(name), directory.path().join(name)).unwrap();
    }
    let profiles = ProfileCatalog::load(&[]).expect("profiles");
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(Capability::Transport);
    // 2009-01-01 00:00 and 06:00 UTC
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("cfsr-pgbl-test".into()),
            source: "NOAA CFSR pgbl".into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-met-test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), directory.path().to_path_buf())]),
        coverage: LockCoverageRequest {
            start: Timestamp::new(1_230_768_000, 0).expect("time"),
            end: Timestamp::new(1_230_789_600, 0).expect("time"),
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities,
        force_rehash: true,
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    assert!(outcome.is_success(), "{:?}", outcome.diagnostics);
    assert_eq!(outcome.summary.meteorology_files, 2);
    let lock = outcome.lock.expect("dataset lock");
    assert_eq!(lock.profile.name, "cfsr-pgbl-pressure-v0");
    assert_eq!(lock.files.len(), 2);

    let domain = DomainId("cfsr-test".into());
    let roots = BTreeMap::from([(DataRootId("met".into()), directory.path().to_path_buf())]);
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir: directory.path(),
        data_roots: &roots,
        domain: &domain,
        required_capabilities: capabilities,
    });
    assert!(
        inventory.is_success(),
        "{:?}",
        inventory.diagnostics.sorted()
    );
    let catalog = inventory.catalog.expect("catalog");
    let frames = &catalog.domains.get(&domain).expect("domain").frames;
    assert_eq!(frames.len(), 2);
    let profile = profiles
        .get(&ProfileName(lock.profile.name.clone()))
        .expect("locked Profile");

    for descriptor in frames.values() {
        let frame = FrameLoader::load(FrameLoadRequest {
            descriptor,
            profile,
            required_capabilities: capabilities,
            backend: MeteorologyReaderBackend::Rust,
            previous_frame: None,
        })
        .expect("RawMetFrame");
        // Seven transport canonical fields plus the intermediate orography height source.
        assert_eq!(
            frame.fields().len(),
            8,
            "transport fields + orography intermediate"
        );
        for field in [
            CanonicalField::EastwardWind,
            CanonicalField::NorthwardWind,
            CanonicalField::PressureVerticalVelocity,
            CanonicalField::AirTemperature,
            CanonicalField::SpecificHumidity,
        ] {
            let layout = frame
                .fields()
                .get(&FieldKey::Canonical(field))
                .unwrap_or_else(|| panic!("missing {field:?}"))
                .layout();
            assert_eq!(
                layout,
                trajecta_met::frame::ArrayLayout::Full3D {
                    levels: 37,
                    ny: 73,
                    nx: 144,
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
                trajecta_met::frame::ArrayLayout::Horizontal2D { ny: 73, nx: 144 }
            );
        }
        let VerticalTopology::PressureLevels(levels) = &frame.metadata().vertical else {
            panic!("expected pressure topology on frame");
        };
        assert_eq!(levels.pressure_pa.len(), 37);
    }
}

#[cfg(feature = "native-eccodes")]
#[test]
fn native_and_rust_cfsr_pressure_fields_match() {
    let path = fixture_file("pgbl00.gdas.2009010100.grb2");
    if !path.is_file() {
        skip_or_fail(path.display().to_string().as_str());
        return;
    }
    let rust = GribReader::new(MeteorologyReaderBackend::Rust);
    let native = GribReader::new(MeteorologyReaderBackend::Native);
    let rust_index = rust.build_index(&path).expect("rust index");
    let native_index = native.build_index(&path).expect("native index");
    let valid_time = rust.inspect(&path).expect("inspect").valid_times[0];
    // Stable-offset mapping must allow native index construction even when
    // ecCodes and grib-reader message counts differ.
    for (discipline, category, number, level_type) in [
        ("0", "2", "2", "isobaric"),
        ("0", "2", "3", "isobaric"),
        ("0", "0", "0", "isobaric"),
        ("0", "1", "0", "isobaric"),
        ("0", "2", "8", "isobaric"),
        ("0", "3", "0", "surface"),
        ("0", "3", "5", "surface"),
    ] {
        let request = DecodeRequest {
            source_identity: vec![
                ("discipline".into(), discipline.into()),
                ("parameter_category".into(), category.into()),
                ("parameter_number".into(), number.into()),
                ("type_of_level".into(), level_type.into()),
            ],
            valid_time: Some(valid_time),
        };
        let rust_field = rust
            .decode(&path, rust_index.as_ref(), &request)
            .unwrap_or_else(|error| {
                panic!("rust decode {discipline}.{category}.{number}: {error:?}")
            });
        let native_field = native
            .decode(&path, native_index.as_ref(), &request)
            .unwrap_or_else(|error| {
                panic!("native decode {discipline}.{category}.{number}: {error:?}")
            });
        assert_eq!(native_field.layout, rust_field.layout);
        assert_eq!(native_field.valid, rust_field.valid);
        assert_eq!(native_field.source_unit, rust_field.source_unit);
        let max_difference = native_field
            .values
            .iter()
            .zip(rust_field.values.iter())
            .map(|(left, right)| (left - right).abs())
            .fold(0.0_f64, f64::max);
        assert!(
            max_difference <= 1.0e-6,
            "CFSR field {discipline}.{category}.{number} max_difference={max_difference}"
        );
    }
}
