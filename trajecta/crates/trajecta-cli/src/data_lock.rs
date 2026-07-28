use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use trajecta_case::diagnostic::Diagnostic;
use trajecta_case::document::{DataRootId, MeteorologyReaderBackend, ProfileSource, ResolvedCase};
use trajecta_case::lockfile::{
    DatasetIdentity, DatasetLock, GeneratorInfo, parse_dataset_lock_json,
};
use trajecta_case::model::meteorology::{DatasetRef, DomainId};
use trajecta_met::field::CapabilitySet;
use trajecta_met::io::inventory::{InventoryBuildRequest, InventoryBuilder};
use trajecta_met::io::lock_builder::{
    DatasetLockBuilder, DatasetLockRequest, DatasetLockRequirements, FileHashCache,
    LockBuildDiagnostic, LockCoverageRequest, ReaderMetadataInspector,
    validate_dataset_lock_requirements,
};
use trajecta_met::profile::document::{ProfileCatalog, ProfileName};

use crate::command_result::CommandError as DataLockError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CaseLockRequirements {
    pub(crate) coverage: LockCoverageRequest,
    pub(crate) capabilities: CapabilitySet,
    pub(crate) domains: BTreeMap<DatasetRef, DomainId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LockSpec {
    pub(crate) output: PathBuf,
    pub(crate) dataset: DatasetRef,
    pub(crate) source: String,
    pub(crate) data_roots: BTreeMap<DataRootId, PathBuf>,
    pub(crate) profile_name: String,
    pub(crate) profile_sources: Vec<ProfileSource>,
    pub(crate) backend: MeteorologyReaderBackend,
    pub(crate) coverage: LockCoverageRequest,
    pub(crate) capabilities: CapabilitySet,
    pub(crate) domain: DomainId,
}

pub(crate) struct LockArtifact {
    pub(crate) lock: DatasetLock,
    pub(crate) bytes: Vec<u8>,
    pub(crate) sha256: String,
    pub(crate) notes: Vec<Diagnostic>,
}

pub(crate) fn requirements_from_case(
    case: &ResolvedCase,
) -> Result<CaseLockRequirements, DataLockError> {
    let time = case.time.as_ref().ok_or_else(|| {
        DataLockError::new("data.case_time_missing", "resolved Case time is missing")
    })?;
    let population = case.particle_population.as_ref().ok_or_else(|| {
        DataLockError::new(
            "data.case_population_missing",
            "resolved Case particle population is missing",
        )
    })?;
    let meteorology = case.meteorology.as_ref().ok_or_else(|| {
        DataLockError::new(
            "data.case_meteorology_missing",
            "resolved Case meteorology is missing",
        )
    })?;
    let capabilities = trajecta_core::runner::required_capabilities_for_population(population)
        .map_err(|error| {
            DataLockError::new(
                "data.case_capabilities_invalid",
                format!("resolve Case capabilities: {error:?}"),
            )
        })?;
    let mut domains = BTreeMap::new();
    for domain in &meteorology.domains {
        domains
            .entry(domain.dataset.clone())
            .or_insert_with(|| domain.id.clone());
    }
    if domains.is_empty() {
        return Err(DataLockError::new(
            "data.case_datasets_missing",
            "resolved Case contains no meteorology datasets",
        ));
    }
    Ok(CaseLockRequirements {
        coverage: LockCoverageRequest {
            start: time.start.min(time.end),
            end: time.start.max(time.end),
            interpolation_before_frames: 1,
            interpolation_after_frames: 1,
        },
        capabilities,
        domains,
    })
}

pub(crate) fn build_lock(spec: &LockSpec) -> Result<LockArtifact, DataLockError> {
    let profiles = load_profiles(spec)?;
    if spec.data_roots.is_empty() {
        return Err(DataLockError::new(
            "data.lock_roots_missing",
            "at least one named data root is required",
        ));
    }
    let inspector = ReaderMetadataInspector::new(spec.backend);
    let mut hash_cache = FileHashCache::new();
    let request = DatasetLockRequest {
        identity: DatasetIdentity {
            id: spec.dataset.clone(),
            source: spec.source.clone(),
            source_url: None,
            attribution: None,
        },
        generator: GeneratorInfo {
            tool: "trajecta-cli".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        data_roots: spec.data_roots.clone(),
        coverage: spec.coverage,
        required_capabilities: spec.capabilities,
        force_rehash: false,
        preferred_profile: Some(spec.profile_name.clone()),
    };
    let outcome = DatasetLockBuilder::new(&profiles, &inspector, &mut hash_cache).build(&request);
    if !outcome.is_success() {
        return Err(DataLockError::new(
            "data.lock_build_failed",
            format_lock_diagnostics(&outcome.diagnostics),
        ));
    }
    let lock = outcome.lock.ok_or_else(|| {
        DataLockError::new(
            "data.lock_build_failed",
            "lock builder reported success without a lock",
        )
    })?;
    let mut bytes = serde_json::to_vec_pretty(&lock)
        .map_err(|error| DataLockError::new("data.lock_serialize_failed", error.to_string()))?;
    bytes.push(b'\n');
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| DataLockError::new("data.lock_serialize_failed", error.to_string()))?;
    let reparsed = parse_dataset_lock_json(text)
        .map_err(|error| DataLockError::new("data.lock_serialize_failed", error.to_string()))?;
    if reparsed != lock {
        return Err(DataLockError::new(
            "data.lock_serialize_failed",
            "serialized lock did not round-trip exactly",
        ));
    }
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let notes = outcome
        .notes
        .into_iter()
        .map(|note| {
            let message = match note.path {
                Some(path) => format!("{} ({})", note.message, path.display()),
                None => note.message,
            };
            Diagnostic::info(note.code, message)
        })
        .collect();
    Ok(LockArtifact {
        lock,
        bytes,
        sha256,
        notes,
    })
}

pub(crate) fn validate_existing_lock(
    spec: &LockSpec,
) -> Result<(DatasetLock, Vec<Diagnostic>), DataLockError> {
    let bytes = fs::read(&spec.output).map_err(|error| {
        DataLockError::new(
            "data.lock_read_failed",
            format!("read {}: {error}", spec.output.display()),
        )
    })?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| DataLockError::new("data.lock_invalid", error.to_string()))?;
    let lock = parse_dataset_lock_json(text)
        .map_err(|error| DataLockError::new("data.lock_invalid", error.to_string()))?;
    let profiles = load_profiles(spec)?;
    let requirements = DatasetLockRequirements {
        dataset: spec.dataset.clone(),
        coverage: spec.coverage,
        required_capabilities: spec.capabilities,
        preferred_profile: spec.profile_name.clone(),
    };
    let requirement_diagnostics =
        validate_dataset_lock_requirements(&profiles, &lock, &requirements);
    if !requirement_diagnostics.is_empty() {
        return Err(DataLockError::new(
            "data.lock_requirements_mismatch",
            format_lock_diagnostics(&requirement_diagnostics),
        ));
    }
    let lockfile_dir = spec.output.parent().unwrap_or_else(|| Path::new("."));
    let inventory = InventoryBuilder::new(&profiles).build(InventoryBuildRequest {
        lock: &lock,
        lockfile_dir,
        data_roots: &spec.data_roots,
        domain: &spec.domain,
        required_capabilities: spec.capabilities,
    });
    if !inventory.is_success() {
        let diagnostics = inventory.diagnostics.into_sorted();
        return Err(DataLockError::new(
            "data.lock_invalid",
            format_diagnostics(&diagnostics),
        ));
    }
    Ok((lock, inventory.diagnostics.into_sorted()))
}

pub(crate) fn persist_lock(
    output: &Path,
    artifact: &LockArtifact,
    replace: bool,
) -> Result<(), DataLockError> {
    if output.exists() && !replace {
        return Err(DataLockError::new(
            "data.lock_exists",
            "destination lock exists; pass --replace to permit replacement",
        ));
    }
    let parent = output
        .parent()
        .ok_or_else(|| DataLockError::new("data.lock_write_failed", "lock path has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| {
        DataLockError::new(
            "data.lock_write_failed",
            format!("create {}: {error}", parent.display()),
        )
    })?;
    let (temporary, mut file) = create_temporary(parent, output)?;
    let write_result = file
        .write_all(&artifact.bytes)
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(DataLockError::new(
            "data.lock_write_failed",
            format!("write {}: {error}", temporary.display()),
        ));
    }
    if replace {
        if let Err(error) = fs::rename(&temporary, output) {
            let _ = fs::remove_file(&temporary);
            return Err(DataLockError::new(
                "data.lock_write_failed",
                format!("atomic replace {}: {error}", output.display()),
            ));
        }
        return Ok(());
    }
    match fs::hard_link(&temporary, output) {
        Ok(()) => {
            fs::remove_file(&temporary).map_err(|error| {
                DataLockError::new(
                    "data.lock_cleanup_failed",
                    format!("remove {}: {error}", temporary.display()),
                )
            })?;
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            let (code, message) = if error.kind() == std::io::ErrorKind::AlreadyExists {
                (
                    "data.lock_exists",
                    "destination lock appeared before atomic install".into(),
                )
            } else {
                (
                    "data.lock_write_failed",
                    format!("atomic install {}: {error}", output.display()),
                )
            };
            Err(DataLockError::new(code, message))
        }
    }
}

fn load_profiles(spec: &LockSpec) -> Result<ProfileCatalog, DataLockError> {
    let profiles = ProfileCatalog::load(&spec.profile_sources).map_err(|error| {
        DataLockError::new(
            "data.profile_load_failed",
            format!("load Profile catalog: {error:?}"),
        )
    })?;
    if profiles
        .get(&ProfileName(spec.profile_name.clone()))
        .is_none()
    {
        return Err(DataLockError::new(
            "data.profile_unknown",
            format!("unknown Profile '{}'", spec.profile_name),
        ));
    }
    Ok(profiles)
}

fn create_temporary(parent: &Path, output: &Path) -> Result<(PathBuf, fs::File), DataLockError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    let name = output
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("dataset-lock.json");
    for attempt in 0..32_u8 {
        let path = parent.join(format!(
            ".{name}.{}-{nonce}-{attempt}.tmp",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(DataLockError::new(
                    "data.lock_write_failed",
                    format!("create {}: {error}", path.display()),
                ));
            }
        }
    }
    Err(DataLockError::new(
        "data.lock_write_failed",
        "could not allocate a unique lock temporary file",
    ))
}

fn format_lock_diagnostics(diagnostics: &[LockBuildDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| match &diagnostic.path {
            Some(path) => format!(
                "{:?}/{}: {} ({})",
                diagnostic.stage,
                diagnostic.code,
                diagnostic.message,
                path.display()
            ),
            None => format!(
                "{:?}/{}: {}",
                diagnostic.stage, diagnostic.code, diagnostic.message
            ),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code(), diagnostic.message()))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use trajecta_case::document::ResolvedCase;
    use trajecta_case::model::time::Timestamp;

    use super::requirements_from_case;

    #[test]
    fn backward_case_requirements_use_physical_coverage_bounds() {
        let case: ResolvedCase = serde_yml::from_str(
            r#"
metadata: { name: backward }
time:
  start: { seconds_since_unix_epoch: 20, nanosecond: 0 }
  end: { seconds_since_unix_epoch: 10, nanosecond: 0 }
  direction: backward
meteorology:
  domains: [{ id: global, dataset: met, priority: 1, horizontal_halo_cells: 1 }]
particle_population:
  strategy: domain_fill_air_mass
  id: fill
  domain_id: global
  target_particle_count: 1
"#,
        )
        .unwrap();
        let requirements = requirements_from_case(&case).unwrap();
        assert_eq!(requirements.coverage.start, Timestamp::new(10, 0).unwrap());
        assert_eq!(requirements.coverage.end, Timestamp::new(20, 0).unwrap());
        assert_eq!(requirements.coverage.interpolation_before_frames, 1);
        assert_eq!(requirements.coverage.interpolation_after_frames, 1);
    }
}
