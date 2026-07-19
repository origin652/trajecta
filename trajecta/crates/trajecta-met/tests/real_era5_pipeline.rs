//! Real ERA5/flex_extract lock-to-frame integration coverage.

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
use trajecta_met::profile::document::ProfileCatalog;

fn fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("workspace root")
        .join("tools/flexctl/target/test-data/ecmwf-era5/flex_extract-7.1")
}

#[test]
fn real_era5_lock_inventory_and_frame_loading_run_end_to_end() {
    let directory = fixture_directory();
    if !directory.join("EA18120100").is_file() {
        return;
    }
    let profiles = ProfileCatalog::load(&[]).expect("built-in Profiles");
    let inspector = ReaderMetadataInspector::new(MeteorologyReaderBackend::Rust);
    let mut hash_cache = FileHashCache::new();
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(Capability::Transport);
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("era5-flex-extract-test".into()),
            source: "ECMWF ERA5 flex_extract".into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-met-test".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), directory.clone())]),
        coverage: LockCoverageRequest {
            start: Timestamp::new(1_543_622_400, 0).expect("time"),
            end: Timestamp::new(1_543_698_000, 0).expect("time"),
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities,
        force_rehash: true,
        preferred_profile: None,
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    assert!(outcome.is_success(), "{:?}", outcome.diagnostics);
    assert_eq!(outcome.summary.meteorology_files, 8);
    assert_eq!(outcome.summary.files_hashed, 8);
    let lock = outcome.lock.expect("dataset lock");
    assert_eq!(lock.files.len(), 8);

    let domain = DomainId("era5-test".into());
    let roots = BTreeMap::from([(DataRootId("met".into()), directory.clone())]);
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir: &directory,
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
    assert_eq!(frames.len(), 8);
    let profile = profiles
        .get(&trajecta_met::profile::document::ProfileName(
            lock.profile.name.clone(),
        ))
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
        assert_eq!(frame.fields().len(), 7);
        assert_eq!(frame.metadata().valid_time, descriptor.id.valid_time);
        assert_eq!(frame.resident_bytes(), 13_608);
    }
}
