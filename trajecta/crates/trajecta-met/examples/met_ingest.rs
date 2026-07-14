//! Unstable meteorology ingestion summary CLI.
//!
//! Prints a stable JSON summary for a data root, optional Profile name, backend,
//! and capability set. Large field arrays are never printed.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
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
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") || args.is_empty() {
        eprintln!(
            "usage: met_ingest --data-root DIR [--start UNIX] [--end UNIX] [--backend rust|native] [--capability transport]"
        );
        return ExitCode::from(2);
    }
    let mut data_root = None;
    let mut start = 1_230_768_000_i64;
    let mut end = 1_230_789_600_i64;
    let mut backend = MeteorologyReaderBackend::Rust;
    let mut capability = Capability::Transport;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--data-root" => {
                i += 1;
                data_root = Some(PathBuf::from(&args[i]));
            }
            "--start" => {
                i += 1;
                start = args[i].parse().expect("start unix seconds");
            }
            "--end" => {
                i += 1;
                end = args[i].parse().expect("end unix seconds");
            }
            "--backend" => {
                i += 1;
                backend = match args[i].as_str() {
                    "rust" => MeteorologyReaderBackend::Rust,
                    "native" => MeteorologyReaderBackend::Native,
                    other => {
                        eprintln!("unknown backend {other}");
                        return ExitCode::from(2);
                    }
                };
            }
            "--capability" => {
                i += 1;
                capability = match args[i].as_str() {
                    "transport" => Capability::Transport,
                    other => {
                        eprintln!("unsupported capability {other}");
                        return ExitCode::from(2);
                    }
                };
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::from(2);
            }
        }
        i += 1;
    }
    let Some(data_root) = data_root else {
        eprintln!("--data-root is required");
        return ExitCode::from(2);
    };
    let profiles = match ProfileCatalog::load(&[]) {
        Ok(profiles) => profiles,
        Err(error) => {
            eprintln!("profile load failed: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    let inspector = ReaderMetadataInspector::new(backend);
    let mut hash_cache = FileHashCache::new();
    let mut capabilities = CapabilitySet::new();
    capabilities.insert(capability);
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: DatasetRef("met-ingest".into()),
            source: "met_ingest example".into(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "met_ingest".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: BTreeMap::from([(DataRootId("met".into()), data_root.clone())]),
        coverage: LockCoverageRequest {
            start: match Timestamp::new(start, 0) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("invalid start: {error}");
                    return ExitCode::from(2);
                }
            },
            end: match Timestamp::new(end, 0) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("invalid end: {error}");
                    return ExitCode::from(2);
                }
            },
            interpolation_before_frames: 0,
            interpolation_after_frames: 0,
        },
        required_capabilities: capabilities,
        force_rehash: false,
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    if !outcome.is_success() {
        println!(
            "{}",
            json!({
                "ok": false,
                "diagnostics": format!("{:?}", outcome.diagnostics),
            })
        );
        return ExitCode::FAILURE;
    }
    let lock = outcome.lock.expect("lock");
    let domain = DomainId("ingest".into());
    let roots = BTreeMap::from([(DataRootId("met".into()), data_root)]);
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir: roots.get(&DataRootId("met".into())).unwrap(),
        data_roots: &roots,
        domain: &domain,
        required_capabilities: capabilities,
    });
    if !inventory.is_success() {
        println!(
            "{}",
            json!({
                "ok": false,
                "diagnostics": format!("{:?}", inventory.diagnostics.sorted()),
            })
        );
        return ExitCode::FAILURE;
    }
    let catalog = inventory.catalog.expect("catalog");
    let frames = &catalog.domains.get(&domain).expect("domain").frames;
    let profile = profiles
        .get(&ProfileName(lock.profile.name.clone()))
        .expect("profile");
    let mut frame_summaries = Vec::new();
    for descriptor in frames.values() {
        let frame = match FrameLoader::load(FrameLoadRequest {
            descriptor,
            profile,
            required_capabilities: capabilities,
            backend,
            previous_frame: None,
        }) {
            Ok(frame) => frame,
            Err(error) => {
                println!(
                    "{}",
                    json!({
                        "ok": false,
                        "diagnostics": format!("{error:?}"),
                    })
                );
                return ExitCode::FAILURE;
            }
        };
        frame_summaries.push(json!({
            "valid_time_unix": descriptor.id.valid_time.seconds_since_unix_epoch(),
            "field_count": frame.fields().len(),
            "resident_bytes": frame.resident_bytes(),
        }));
    }
    println!(
        "{}",
        json!({
            "ok": true,
            "profile": lock.profile.name,
            "profile_sha256": lock.profile.sha256,
            "backend": format!("{backend:?}").to_ascii_lowercase(),
            "files": lock.files.len(),
            "frames": frame_summaries,
        })
    );
    ExitCode::SUCCESS
}
