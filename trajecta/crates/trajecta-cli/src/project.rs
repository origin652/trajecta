use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use trajecta_case::diagnostic::Diagnostic;
use trajecta_case::document::{
    CaseDocument, DatasetBinding, MeteorologyReaderBackend, ProfileSource, ResolvedCase,
    ResolvedRunProfile, RunProfileDocument,
};
use trajecta_case::expand::{expand_case_file, expand_run_profile_file};
use trajecta_case::intent::{IntentValidator, ValidationIntent};
use trajecta_case::lockfile::parse_dataset_lock_json;
use trajecta_case::resolver::LocalRefResolver;
use trajecta_case::schema::{CURRENT_SCHEMA_VERSION, SchemaDocument, SchemaError};
use trajecta_met::io::lock_builder::{ReaderMetadataInspector, SourceMetadataInspector};
use trajecta_met::io::reader::{SourceFormat, detect_source_format};

use crate::command::project::ProjectCommand;
use crate::data_lock::{
    DataLockError, LockArtifact, LockSpec, build_lock, persist_lock, requirements_from_case,
    validate_existing_lock,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectIndex {
    schema_version: String,
    name: String,
    cases: BTreeMap<String, String>,
    profiles: BTreeMap<String, ProjectProfile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_profile: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectProfile {
    path: String,
    dataset_profiles: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    template_sha256: Option<String>,
}

struct Project {
    root: PathBuf,
    index_path: PathBuf,
    index: ProjectIndex,
}

/// Validated project-local fields needed by A1 planning without pretending a
/// missing local lock is a resolved Case-library RunProfile.
#[derive(Clone)]
struct ProjectProfileSnapshot {
    case_path: PathBuf,
    output_root: PathBuf,
    datasets: Vec<DatasetBinding>,
    profile_sources: Vec<ProfileSource>,
    reader_backend: MeteorologyReaderBackend,
}

/// Fully resolved project selection handed to the M5 worker path.
pub(crate) struct ProjectRunResolution {
    pub(crate) project_root: PathBuf,
    pub(crate) profile_name: String,
    pub(crate) case: ResolvedCase,
    pub(crate) run_profile: ResolvedRunProfile,
}

/// Resolves one concrete project profile through the existing project jail and validators.
pub(crate) fn resolve_runtime_run(
    requested: &Path,
    profile_name: &str,
) -> Result<ProjectRunResolution, ProjectError> {
    if profile_name.trim().is_empty() {
        return Err(ProjectError::new(
            "project.profile_not_found",
            "profile name must not be empty",
        ));
    }
    let mut project = discover(Some(requested))?;
    let mut status = validate_project(&project)?;
    if status.state == "configured" && !status.has_errors {
        finalize_project(Some(&project.root))?;
        project = discover(Some(&project.root))?;
        status = validate_project(&project)?;
    }
    if status.state != "finalized" || status.has_errors {
        return Err(ProjectError::new(
            "project.not_finalized",
            "run requires a finalized project; inspect project validate and data-plan",
        ));
    }
    let entry = project.index.profiles.get(profile_name).ok_or_else(|| {
        ProjectError::new(
            "project.profile_not_found",
            format!("project profile `{profile_name}` is not declared"),
        )
    })?;
    let profile_path = indexed_path(&project, &entry.path)?;
    let resolver = LocalRefResolver::new(&project.root);
    let run_profile = expand_run_profile_file(&profile_path, &resolver)
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    let indexed_case_matches = project.index.cases.values().try_fold(
        false,
        |matched, relative| -> Result<bool, ProjectError> {
            Ok(matched || same_path(&indexed_path(&project, relative)?, &run_profile.case_path))
        },
    )?;
    if !indexed_case_matches {
        return Err(ProjectError::new(
            "project.profile_case_mismatch",
            "resolved RunProfile case_path does not select an indexed Case",
        ));
    }
    let case = expand_case_file(&run_profile.case_path, &resolver)
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    let intent = IntentValidator::validate(&case, ValidationIntent::Simulation);
    if intent.has_errors() {
        return Err(ProjectError::new(
            "project.document_invalid",
            "resolved Case does not satisfy simulation intent",
        ));
    }
    Ok(ProjectRunResolution {
        project_root: project.root,
        profile_name: profile_name.into(),
        case,
        run_profile,
    })
}

pub(crate) fn execute(
    command: &ProjectCommand,
    requested: Option<&Path>,
) -> Result<ProjectOutcome, ProjectError> {
    match command {
        ProjectCommand::Init { path, name } => init(path.as_deref().or(requested), name),
        ProjectCommand::Status => {
            let project = discover(requested)?;
            let status = validate_project(&project)?;
            Ok(ProjectOutcome::with_diagnostics(
                serde_json::json!({"root": project.root, "state": status.state}),
                status.diagnostics,
            ))
        }
        ProjectCommand::Show => {
            let project = discover(requested)?;
            let status = validate_project(&project)?;
            Ok(ProjectOutcome::with_diagnostics(
                serde_json::json!({"root": project.root, "index": project.index, "state": status.state}),
                status.diagnostics,
            ))
        }
        ProjectCommand::Validate => {
            let project = discover(requested)?;
            let status = validate_project(&project)?;
            Ok(ProjectOutcome::with_diagnostics(
                serde_json::json!({"valid": !status.has_errors, "state": status.state}),
                status.diagnostics,
            ))
        }
        ProjectCommand::Get { selector } => {
            let project = discover(requested)?;
            Ok(ProjectOutcome::ok(read_selector(&project, selector)?))
        }
        ProjectCommand::Set { selector, value } => {
            let project = discover(requested)?;
            set_selector(&project, selector, parse_input_value(value))?;
            Ok(ProjectOutcome::ok(serde_json::json!({"updated": selector})))
        }
        ProjectCommand::Unset { selector } => {
            let project = discover(requested)?;
            unset_selector(&project, selector)?;
            Ok(ProjectOutcome::ok(serde_json::json!({"unset": selector})))
        }
        ProjectCommand::DataPlan { output } => {
            let project = discover(requested)?;
            let status = validate_project(&project)?;
            if !matches!(status.state, "configured" | "finalized") || status.has_errors {
                return Err(ProjectError::new(
                    "project.not_configured",
                    "data-plan requires a configured project",
                ));
            }
            let plan = data_plan(&project)?;
            let bytes = canonical_json(&plan)?;
            if let Some(path) = output.as_deref() {
                if path != Path::new("-") {
                    write_file_atomic(path, &bytes)?;
                }
            }
            Ok(ProjectOutcome::with_diagnostics(plan, status.diagnostics))
        }
        ProjectCommand::Finalize => finalize_project(requested),
    }
}

pub(crate) fn doctor(
    requested: Option<&Path>,
    config_path: Option<&Path>,
    deep: bool,
) -> Result<ProjectOutcome, ProjectError> {
    let config = crate::configuration::validate_selected(config_path)
        .map_err(|error| ProjectError::new("doctor.config_invalid", error.message))?;
    let project = match discover(requested) {
        Ok(project) => project,
        Err(error) if error.code == "project.not_found" => {
            return Ok(ProjectOutcome::with_diagnostics(
                serde_json::json!({"project": null, "config": config, "deep": deep}),
                vec![diagnostic(
                    "warning",
                    "project.not_found",
                    "no project was discovered",
                )],
            ));
        }
        Err(error) => return Err(error),
    };
    let status = validate_project(&project)?;
    let mut diagnostics = status.diagnostics;
    if deep {
        let probe_root = create_doctor_probe_root(&project.root)?;
        let result = (|| -> Result<(), ProjectError> {
            doctor_filesystem_probe(&probe_root)?;
            doctor_existing_data_probe(&project)?;
            doctor_sqlite_probe(&probe_root)
        })();
        let cleanup = cleanup_doctor_probe_root(&probe_root);
        match (result, cleanup) {
            (Ok(()), Ok(())) => {}
            (Err(error), Ok(())) => return Err(error),
            (Ok(()), Err(error)) => return Err(error),
            (Err(primary), Err(cleanup)) => {
                return Err(ProjectError::new(
                    primary.code,
                    format!(
                        "{}; cleanup also failed: {}",
                        primary.message, cleanup.message
                    ),
                ));
            }
        }
        diagnostics.push(diagnostic(
            "info",
            "doctor.deep_sqlite_ok",
            "SQLite create, integrity_check, WAL checkpoint and cleanup succeeded",
        ));
    }
    diagnostics.push(diagnostic(
        "info",
        "doctor.daemon_pending_m5_a2",
        "daemon and IPC checks begin in M5-A2",
    ));
    Ok(ProjectOutcome::with_diagnostics(
        serde_json::json!({"root": project.root, "config": config, "state": status.state, "deep": deep}),
        diagnostics,
    ))
}

fn create_doctor_probe_root(root: &Path) -> Result<PathBuf, ProjectError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    for attempt in 0..32_u8 {
        let path = root.join(format!(
            ".trajecta-doctor-{}-{nonce}-{attempt}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ProjectError::new(
                    "doctor.filesystem_unwritable",
                    format!("create {}: {error}", path.display()),
                ));
            }
        }
    }
    Err(ProjectError::new(
        "doctor.filesystem_unwritable",
        "could not allocate a unique doctor probe directory",
    ))
}

fn doctor_filesystem_probe(root: &Path) -> Result<(), ProjectError> {
    let probe = root.join("write-probe");
    let renamed = root.join("write-probe-renamed");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|error| ProjectError::new("doctor.filesystem_unwritable", error.to_string()))?;
    file.write_all(b"probe")
        .and_then(|()| file.sync_all())
        .map_err(|error| ProjectError::new("doctor.filesystem_unwritable", error.to_string()))?;
    drop(file);
    fs::rename(&probe, &renamed)
        .map_err(|error| ProjectError::new("doctor.filesystem_unwritable", error.to_string()))?;
    fs::remove_file(&renamed)
        .map_err(|error| ProjectError::new("doctor.filesystem_unwritable", error.to_string()))
}

fn cleanup_doctor_probe_root(root: &Path) -> Result<(), ProjectError> {
    for name in [
        "write-probe",
        "write-probe-renamed",
        "probe.sqlite",
        "probe.sqlite-wal",
        "probe.sqlite-shm",
    ] {
        let path = root.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ProjectError::new(
                    "doctor.cleanup_failed",
                    format!("remove {}: {error}", path.display()),
                ));
            }
        }
    }
    fs::remove_dir(root).map_err(|error| {
        ProjectError::new(
            "doctor.cleanup_failed",
            format!("remove {}: {error}", root.display()),
        )
    })
}

fn doctor_existing_data_probe(project: &Project) -> Result<(), ProjectError> {
    for entry in project.index.profiles.values() {
        let profile_path = indexed_path(project, &entry.path)?;
        let Some(profile) = resolve_profile(&profile_path, &project.root)? else {
            continue;
        };
        for binding in profile.datasets {
            if binding.lockfile.is_file() {
                let text = fs::read_to_string(&binding.lockfile).map_err(io_error)?;
                let lock = parse_dataset_lock_json(&text)
                    .map_err(|error| ProjectError::new("doctor.lock_invalid", error.to_string()))?;
                let base = binding.lockfile.parent().unwrap_or_else(|| Path::new("."));
                let verification = lock
                    .verify_local_files_with_roots(base, &binding.data_roots)
                    .map_err(|error| ProjectError::new("doctor.lock_invalid", error.to_string()))?;
                if verification.has_errors() {
                    return Err(ProjectError::new(
                        "doctor.lock_invalid",
                        "existing lock verification failed",
                    ));
                }
            }
            let inspector = ReaderMetadataInspector::new(
                binding.reader_backend.unwrap_or(profile.reader_backend),
            );
            for root in binding.data_roots.values().filter(|root| root.is_dir()) {
                if let Some((source, format)) = first_supported_source(root)? {
                    inspector.inspect(&source, format).map_err(|error| {
                        ProjectError::new("doctor.data_inspect_failed", format!("{error:?}"))
                    })?;
                }
            }
        }
    }
    Ok(())
}

fn first_supported_source(root: &Path) -> Result<Option<(PathBuf, SourceFormat)>, ProjectError> {
    const MAX_SCAN_ENTRIES: usize = 100_000;

    let scan_root = fs::canonicalize(root).map_err(io_error)?;
    let mut pending = VecDeque::from([scan_root.clone()]);
    let mut visited = BTreeSet::new();
    let mut scanned = 0_usize;
    while let Some(directory) = pending.pop_front() {
        if !visited.insert(directory.clone()) {
            continue;
        }
        let mut entries = fs::read_dir(&directory)
            .map_err(io_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(io_error)?;
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            scanned = scanned.saturating_add(1);
            if scanned > MAX_SCAN_ENTRIES {
                return Err(ProjectError::new(
                    "doctor.data_scan_limit",
                    format!("data scan exceeded {MAX_SCAN_ENTRIES} entries"),
                ));
            }
            let canonical = fs::canonicalize(entry.path()).map_err(io_error)?;
            if !canonical.starts_with(&scan_root) {
                return Err(ProjectError::new(
                    "doctor.data_path_escape",
                    "configured data root contains a link outside its root",
                ));
            }
            let metadata = fs::metadata(&canonical).map_err(io_error)?;
            if metadata.is_dir() {
                pending.push_back(canonical);
            } else if metadata.is_file() {
                let format = detect_source_format(&canonical).map_err(|error| {
                    ProjectError::new("doctor.data_inspect_failed", format!("{error:?}"))
                })?;
                if let Some(format) = format {
                    return Ok(Some((canonical, format)));
                }
            }
        }
    }
    Ok(None)
}

fn doctor_sqlite_probe(root: &Path) -> Result<(), ProjectError> {
    let path = root.join("probe.sqlite");
    let result = (|| -> Result<(), ProjectError> {
        let connection = rusqlite::Connection::open(&path)
            .map_err(|error| ProjectError::new("doctor.sqlite_create_failed", error.to_string()))?;
        connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE probe(value INTEGER); INSERT INTO probe VALUES (1);")
            .map_err(|error| ProjectError::new("doctor.sqlite_create_failed", error.to_string()))?;
        let integrity: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|error| {
                ProjectError::new("doctor.sqlite_integrity_failed", error.to_string())
            })?;
        if integrity != "ok" {
            return Err(ProjectError::new(
                "doctor.sqlite_integrity_failed",
                integrity,
            ));
        }
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|error| {
                ProjectError::new("doctor.sqlite_checkpoint_failed", error.to_string())
            })?;
        Ok(())
    })();
    let cleanup = [
        path.clone(),
        path.with_extension("sqlite-wal"),
        path.with_extension("sqlite-shm"),
    ]
    .into_iter()
    .try_for_each(|candidate| match fs::remove_file(&candidate) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ProjectError::new(
            "doctor.sqlite_cleanup_failed",
            format!("remove {}: {error}", candidate.display()),
        )),
    });
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(primary), Err(cleanup)) => Err(ProjectError::new(
            primary.code,
            format!(
                "{}; cleanup also failed: {}",
                primary.message, cleanup.message
            ),
        )),
    }
}

fn init(requested: Option<&Path>, name: &str) -> Result<ProjectOutcome, ProjectError> {
    if name.trim().is_empty() {
        return Err(ProjectError::new(
            "project.invalid_index",
            "project name must not be empty",
        ));
    }
    let root = requested
        .map(Path::to_path_buf)
        .unwrap_or(env::current_dir().map_err(io_error)?);
    let index_path = root.join("trajecta-project.yaml");
    if index_path.exists() {
        return Err(ProjectError::new(
            "project.already_exists",
            "project index already exists",
        ));
    }
    for directory in ["cases", "profiles", "locks", "runs"] {
        fs::create_dir_all(root.join(directory)).map_err(io_error)?;
    }
    let index = ProjectIndex {
        schema_version: "trajecta.project-index/v1".into(),
        name: name.into(),
        cases: BTreeMap::new(),
        profiles: BTreeMap::new(),
        default_profile: None,
    };
    write_yaml_atomic(&index_path, &index)?;
    Ok(ProjectOutcome::ok(
        serde_json::json!({"root": root, "state": "draft"}),
    ))
}

fn discover(requested: Option<&Path>) -> Result<Project, ProjectError> {
    let requested_index_path = if let Some(path) = requested {
        if path
            .file_name()
            .is_some_and(|name| name == "trajecta-project.yaml")
        {
            path.to_path_buf()
        } else {
            path.join("trajecta-project.yaml")
        }
    } else {
        let mut current = env::current_dir().map_err(io_error)?;
        loop {
            let candidate = current.join("trajecta-project.yaml");
            if candidate.is_file() {
                break candidate;
            }
            if !current.pop() {
                return Err(ProjectError::new(
                    "project.not_found",
                    "no trajecta-project.yaml found from current directory",
                ));
            }
        }
    };
    let root = fs::canonicalize(
        requested_index_path
            .parent()
            .ok_or_else(|| ProjectError::new("project.invalid_index", "index has no parent"))?,
    )
    .map_err(io_error)?;
    let index_path = fs::canonicalize(&requested_index_path).map_err(io_error)?;
    if index_path.parent() != Some(root.as_path()) {
        return Err(ProjectError::new(
            "project.path_escape",
            "project index resolves outside the selected project root",
        ));
    }
    let text = fs::read_to_string(&index_path).map_err(io_error)?;
    // Parse through serde_yml's own mapping value first. This preserves YAML
    // mapping semantics, including duplicate-key rejection, before conversion
    // into the typed index (where a map-like Rust field could otherwise lose
    // the duplicate-key evidence).
    let raw_index: serde_yml::Value = serde_yml::from_str(&text)
        .map_err(|error| ProjectError::new("project.invalid_index", error.to_string()))?;
    let index: ProjectIndex = serde_yml::from_value(raw_index)
        .map_err(|error| ProjectError::new("project.invalid_index", error.to_string()))?;
    validate_index(&index)?;
    validate_index_paths(&root, &index)?;
    Ok(Project {
        root,
        index_path,
        index,
    })
}

fn validate_index(index: &ProjectIndex) -> Result<(), ProjectError> {
    if index.schema_version != "trajecta.project-index/v1" || index.name.trim().is_empty() {
        return Err(ProjectError::new(
            "project.invalid_index",
            "invalid project index schema or name",
        ));
    }
    if index.template_pair_invalid() {
        return Err(ProjectError::new(
            "project.invalid_index",
            "profile template and template_sha256 must occur together",
        ));
    }
    if index.cases.keys().any(|name| name.trim().is_empty())
        || index.profiles.keys().any(|name| name.trim().is_empty())
        || index.profiles.values().any(|profile| {
            profile
                .dataset_profiles
                .iter()
                .any(|(dataset, profile_name)| {
                    dataset.trim().is_empty() || profile_name.trim().is_empty()
                })
                || profile
                    .template
                    .as_deref()
                    .is_some_and(|value| value.trim().is_empty())
                || profile.template_sha256.as_deref().is_some_and(|value| {
                    value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        })
    {
        return Err(ProjectError::new(
            "project.invalid_index",
            "project index contains an empty name or invalid template digest",
        ));
    }
    if let Some(default_profile) = &index.default_profile {
        if default_profile.trim().is_empty() || !index.profiles.contains_key(default_profile) {
            return Err(ProjectError::new(
                "project.invalid_index",
                "default_profile is not declared",
            ));
        }
    }
    let mut paths = BTreeSet::new();
    for path in index
        .cases
        .values()
        .chain(index.profiles.values().map(|profile| &profile.path))
    {
        validate_index_relative_path(path)?;
        if !paths.insert(path) {
            return Err(ProjectError::new(
                "project.invalid_index",
                "project index paths must be unique",
            ));
        }
    }
    Ok(())
}

fn validate_index_relative_path(value: &str) -> Result<(), ProjectError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ProjectError::new(
            "project.path_escape",
            "index paths must use normalized '/'-separated relative components",
        ));
    }
    Ok(())
}

fn indexed_path(project: &Project, relative: &str) -> Result<PathBuf, ProjectError> {
    validate_index_relative_path(relative)?;
    resolve_contained_path(&project.root, project.root.join(relative), false)
}

fn validate_index_paths(root: &Path, index: &ProjectIndex) -> Result<(), ProjectError> {
    for relative in index
        .cases
        .values()
        .chain(index.profiles.values().map(|profile| &profile.path))
    {
        validate_index_relative_path(relative)?;
        resolve_contained_path(root, root.join(relative), false)?;
    }
    Ok(())
}

impl ProjectIndex {
    fn template_pair_invalid(&self) -> bool {
        self.profiles
            .values()
            .any(|profile| profile.template.is_some() != profile.template_sha256.is_some())
    }
}

struct ProjectStatus {
    state: &'static str,
    diagnostics: Vec<Diagnostic>,
    has_errors: bool,
}

#[derive(Clone, Debug)]
struct ProjectLockUse {
    project_profile: String,
    case_name: String,
    output_root: PathBuf,
    spec: LockSpec,
}

#[derive(Clone, Debug)]
struct ProjectLockGroup {
    spec: LockSpec,
    consumers: Vec<(String, String)>,
}

fn project_lock_uses(project: &Project) -> Result<Vec<ProjectLockUse>, ProjectError> {
    let mut cases = Vec::new();
    for (case_name, relative) in &project.index.cases {
        let path = indexed_path(project, relative)?;
        let case = resolve_case(&path, &project.root)?.ok_or_else(|| {
            ProjectError::new(
                "project.not_configured",
                format!("Case `{case_name}` is still a draft"),
            )
        })?;
        cases.push((case_name.clone(), path, case));
    }

    let mut uses = Vec::new();
    for (project_profile, entry) in &project.index.profiles {
        let path = indexed_path(project, &entry.path)?;
        let profile = resolve_profile(&path, &project.root)?.ok_or_else(|| {
            ProjectError::new(
                "project.not_configured",
                format!("RunProfile `{project_profile}` is still a draft"),
            )
        })?;
        let (case_name, _, case) = cases
            .iter()
            .find(|(_, case_path, _)| same_path(case_path, &profile.case_path))
            .ok_or_else(|| {
                ProjectError::new(
                    "project.profile_case_mismatch",
                    format!(
                        "RunProfile `{project_profile}` does not select an indexed resolved Case"
                    ),
                )
            })?;
        let requirements = requirements_from_case(case).map_err(project_data_lock_error)?;
        for binding in &profile.datasets {
            let dataset_profile = entry
                .dataset_profiles
                .get(&binding.dataset.0)
                .ok_or_else(|| {
                    ProjectError::new(
                        "project.dataset_profiles_mismatch",
                        format!(
                            "RunProfile `{project_profile}` has no Profile mapping for dataset `{}`",
                            binding.dataset.0
                        ),
                    )
                })?;
            let domain = requirements.domains.get(&binding.dataset).ok_or_else(|| {
                ProjectError::new(
                    "project.dataset_profiles_mismatch",
                    format!(
                        "dataset `{}` is not used by Case `{case_name}`",
                        binding.dataset.0
                    ),
                )
            })?;
            uses.push(ProjectLockUse {
                project_profile: project_profile.clone(),
                case_name: case_name.clone(),
                output_root: profile.output_root.clone(),
                spec: LockSpec {
                    output: binding.lockfile.clone(),
                    dataset: binding.dataset.clone(),
                    source: format!("local data for {}", binding.dataset.0),
                    data_roots: binding.data_roots.clone(),
                    profile_name: dataset_profile.clone(),
                    profile_sources: profile.profile_sources.clone(),
                    backend: binding.reader_backend.unwrap_or(profile.reader_backend),
                    coverage: requirements.coverage,
                    capabilities: requirements.capabilities,
                    domain: domain.clone(),
                },
            });
        }
    }
    uses.sort_by(|left, right| {
        (
            &left.spec.output,
            &left.project_profile,
            &left.case_name,
            &left.spec.dataset,
        )
            .cmp(&(
                &right.spec.output,
                &right.project_profile,
                &right.case_name,
                &right.spec.dataset,
            ))
    });
    Ok(uses)
}

fn group_lock_uses(uses: &[ProjectLockUse]) -> Result<Vec<ProjectLockGroup>, ProjectError> {
    let mut groups = BTreeMap::<PathBuf, ProjectLockGroup>::new();
    for item in uses {
        let consumer = (item.project_profile.clone(), item.case_name.clone());
        match groups.get_mut(&item.spec.output) {
            None => {
                groups.insert(
                    item.spec.output.clone(),
                    ProjectLockGroup {
                        spec: item.spec.clone(),
                        consumers: vec![consumer],
                    },
                );
            }
            Some(group) => {
                let conflict = if group.spec.dataset != item.spec.dataset {
                    Some("dataset")
                } else if group.spec.profile_name != item.spec.profile_name {
                    Some("dataset Profile")
                } else if group.spec.data_roots != item.spec.data_roots {
                    Some("data roots")
                } else if group.spec.profile_sources != item.spec.profile_sources {
                    Some("Profile sources")
                } else if group.spec.backend != item.spec.backend {
                    Some("reader backend")
                } else {
                    None
                };
                if let Some(field) = conflict {
                    return Err(ProjectError::new(
                        "project.lock_binding_conflict",
                        format!(
                            "shared lockfile {} has conflicting {field}",
                            item.spec.output.display()
                        ),
                    ));
                }
                group.spec.coverage.start = group.spec.coverage.start.min(item.spec.coverage.start);
                group.spec.coverage.end = group.spec.coverage.end.max(item.spec.coverage.end);
                group.spec.coverage.interpolation_before_frames = group
                    .spec
                    .coverage
                    .interpolation_before_frames
                    .max(item.spec.coverage.interpolation_before_frames);
                group.spec.coverage.interpolation_after_frames = group
                    .spec
                    .coverage
                    .interpolation_after_frames
                    .max(item.spec.coverage.interpolation_after_frames);
                for capability in item.spec.capabilities.iter() {
                    group.spec.capabilities.insert(capability);
                }
                if !group.consumers.contains(&consumer) {
                    group.consumers.push(consumer);
                }
            }
        }
    }
    Ok(groups.into_values().collect())
}

fn project_data_lock_error(error: DataLockError) -> ProjectError {
    ProjectError::new(error.code, error.message)
}

fn validate_project(project: &Project) -> Result<ProjectStatus, ProjectError> {
    let mut diagnostics = Vec::new();
    let mut draft = project.index.cases.is_empty() || project.index.profiles.is_empty();
    let mut has_errors = false;
    let mut resolved_cases = BTreeMap::new();
    for (name, relative) in &project.index.cases {
        let path = indexed_path(project, relative)?;
        match resolve_case(&path, &project.root) {
            Ok(Some(case)) => {
                resolved_cases.insert(name.clone(), (path, case));
            }
            Ok(None) => draft = true,
            Err(error) => {
                has_errors = true;
                diagnostics.push(diagnostic("error", error.code, error.message));
            }
        }
    }
    for (name, entry) in &project.index.profiles {
        let path = indexed_path(project, &entry.path)?;
        match resolve_profile(&path, &project.root) {
            Ok(Some(profile)) => {
                let matching_case = resolved_cases
                    .iter()
                    .find(|(_, (case_path, _))| same_path(case_path, &profile.case_path));
                let Some((case_name, (_, case))) = matching_case else {
                    has_errors = true;
                    diagnostics.push(diagnostic(
                        "error",
                        "project.profile_case_mismatch",
                        "profile case_path does not select an indexed resolved Case",
                    ));
                    continue;
                };
                let expected = entry
                    .dataset_profiles
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                let actual = profile
                    .datasets
                    .iter()
                    .map(|binding| binding.dataset.0.clone())
                    .collect::<BTreeSet<_>>();
                let case_datasets = case
                    .meteorology
                    .as_ref()
                    .map(|meteorology| {
                        meteorology
                            .domains
                            .iter()
                            .map(|domain| domain.dataset.0.clone())
                            .collect::<BTreeSet<_>>()
                    })
                    .unwrap_or_default();
                if expected != actual || actual != case_datasets {
                    has_errors = true;
                    diagnostics.push(diagnostic("error", "project.dataset_profiles_mismatch", format!("profile `{name}` dataset_profiles and RunProfile bindings must exactly match Case `{case_name}` datasets")));
                }
                if case.time.is_none() || case.particle_population.is_none() {
                    draft = true;
                }
            }
            Ok(None) => draft = true,
            Err(error) => {
                has_errors = true;
                diagnostics.push(diagnostic("error", error.code, error.message));
            }
        }
    }
    if has_errors || draft {
        return Ok(ProjectStatus {
            state: "draft",
            diagnostics,
            has_errors,
        });
    }

    let uses = project_lock_uses(project)?;
    let groups = group_lock_uses(&uses)?;
    let mut ready = true;
    let output_roots = uses
        .iter()
        .map(|item| item.output_root.clone())
        .collect::<BTreeSet<_>>();
    for output_root in output_roots {
        let display = relative_string(&project.root, &output_root)?;
        match fs::metadata(&output_root) {
            Ok(metadata) if metadata.is_dir() => {
                if let Err(error) = probe_writable_directory(&output_root) {
                    has_errors = true;
                    diagnostics.push(diagnostic(
                        "error",
                        error.code,
                        format!("output root is not writable: {display}"),
                    ));
                }
            }
            Ok(_) => {
                has_errors = true;
                diagnostics.push(diagnostic(
                    "error",
                    "project.output_root_invalid",
                    format!("output root is not a directory: {display}"),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ready = false;
                diagnostics.push(diagnostic(
                    "warning",
                    "project.output_root_pending",
                    format!("output root will be created by finalize: {display}"),
                ));
            }
            Err(error) => {
                has_errors = true;
                diagnostics.push(diagnostic(
                    "error",
                    "project.output_root_invalid",
                    format!("inspect {display}: {error}"),
                ));
            }
        }
    }
    for group in groups {
        let display = relative_string(&project.root, &group.spec.output)?;
        if !group.spec.output.exists() {
            ready = false;
            diagnostics.push(diagnostic(
                "warning",
                "project.lock_missing",
                format!("dataset lock is missing: {display}"),
            ));
            continue;
        }
        if !group.spec.output.is_file() {
            has_errors = true;
            diagnostics.push(diagnostic(
                "error",
                "project.lock_invalid",
                format!("dataset lock is not a file: {display}"),
            ));
            continue;
        }
        match validate_existing_lock(&group.spec) {
            Ok((_lock, notes)) => diagnostics.extend(notes),
            Err(error) => {
                has_errors = true;
                diagnostics.push(diagnostic(
                    "error",
                    "project.lock_invalid",
                    format!("{display}: {}", error.message),
                ));
            }
        }
    }
    let state = if ready && !has_errors {
        "finalized"
    } else {
        "configured"
    };
    if state == "configured" && !has_errors {
        diagnostics.push(diagnostic(
            "info",
            "project.finalize_pending",
            "run project finalize after local data has been prepared",
        ));
    }
    Ok(ProjectStatus {
        state,
        diagnostics,
        has_errors,
    })
}

fn finalize_project(requested: Option<&Path>) -> Result<ProjectOutcome, ProjectError> {
    let project = discover(requested)?;
    let preflight = validate_project(&project)?;
    if preflight.has_errors {
        let details = preflight
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.severity() == trajecta_case::diagnostic::Severity::Error
            })
            .map(|diagnostic| format!("{}: {}", diagnostic.code(), diagnostic.message()))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ProjectError::new(
            "project.finalize_preflight_failed",
            if details.is_empty() {
                "project validation contains errors; no lockfiles were changed".into()
            } else {
                format!("{details}; no lockfiles were changed")
            },
        ));
    }
    if preflight.state == "draft" {
        return Err(ProjectError::new(
            "project.not_configured",
            "project finalize requires complete Case and RunProfile documents",
        ));
    }

    let uses = project_lock_uses(&project)?;
    let groups = group_lock_uses(&uses)?;
    let output_roots = uses
        .iter()
        .map(|item| item.output_root.clone())
        .collect::<BTreeSet<_>>();
    let mut diagnostics = Vec::new();
    let mut reused = Vec::new();
    let mut missing = Vec::new();
    for group in groups {
        if group.spec.output.exists() {
            if !group.spec.output.is_file() {
                return Err(ProjectError::new(
                    "project.lock_invalid",
                    format!(
                        "dataset lock is not a file: {}",
                        group.spec.output.display()
                    ),
                ));
            }
            let (_, notes) =
                validate_existing_lock(&group.spec).map_err(project_data_lock_error)?;
            diagnostics.extend(notes);
            reused.push(relative_string(&project.root, &group.spec.output)?);
        } else {
            missing.push(group);
        }
    }
    let mut pending = Vec::<(ProjectLockGroup, LockArtifact)>::new();
    for group in missing {
        let artifact = build_lock(&group.spec).map_err(project_data_lock_error)?;
        diagnostics.extend(artifact.notes.clone());
        pending.push((group, artifact));
    }

    let created_output_roots = prepare_output_roots(&output_roots)?;
    let mut installed = Vec::<(PathBuf, Vec<u8>)>::new();
    for (group, artifact) in &pending {
        if let Err(error) = persist_lock(&group.spec.output, artifact, false) {
            let rollback = rollback_created_locks(&installed);
            cleanup_created_output_roots(&created_output_roots);
            return Err(combine_finalize_error(
                project_data_lock_error(error),
                rollback,
            ));
        }
        installed.push((group.spec.output.clone(), artifact.bytes.clone()));
    }

    let final_status = match validate_project(&project) {
        Ok(status) if status.state == "finalized" && !status.has_errors => status,
        Ok(status) => {
            let rollback = rollback_created_locks(&installed);
            cleanup_created_output_roots(&created_output_roots);
            let primary = ProjectError::new(
                "project.finalize_incomplete",
                format!(
                    "finalize postflight state was `{}`; newly created lockfiles were rolled back",
                    status.state
                ),
            );
            return Err(combine_finalize_error(primary, rollback));
        }
        Err(error) => {
            let rollback = rollback_created_locks(&installed);
            cleanup_created_output_roots(&created_output_roots);
            return Err(combine_finalize_error(error, rollback));
        }
    };
    diagnostics.extend(final_status.diagnostics);

    let created = pending
        .iter()
        .map(|(group, artifact)| {
            Ok(serde_json::json!({
                "path": relative_string(&project.root, &group.spec.output)?,
                "sha256": artifact.sha256,
                "file_count": artifact.lock.files.len(),
                "consumers": group.consumers.iter().map(|(profile, case_name)| {
                    serde_json::json!({"profile": profile, "case": case_name})
                }).collect::<Vec<_>>()
            }))
        })
        .collect::<Result<Vec<_>, ProjectError>>()?;
    let output_roots = output_roots
        .iter()
        .map(|path| relative_string(&project.root, path))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ProjectOutcome::with_diagnostics(
        serde_json::json!({
            "root": project.root,
            "state": "finalized",
            "created_locks": created,
            "reused_locks": reused,
            "output_roots": output_roots,
        }),
        diagnostics,
    ))
}

fn prepare_output_roots(roots: &BTreeSet<PathBuf>) -> Result<Vec<PathBuf>, ProjectError> {
    let mut created = Vec::new();
    for root in roots {
        match fs::metadata(root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                cleanup_created_output_roots(&created);
                return Err(ProjectError::new(
                    "project.output_root_invalid",
                    format!("output root is not a directory: {}", root.display()),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = fs::create_dir_all(root) {
                    cleanup_created_output_roots(&created);
                    return Err(ProjectError::new(
                        "project.output_root_unwritable",
                        format!("create {}: {error}", root.display()),
                    ));
                }
                created.push(root.clone());
            }
            Err(error) => {
                cleanup_created_output_roots(&created);
                return Err(ProjectError::new(
                    "project.output_root_unwritable",
                    format!("inspect {}: {error}", root.display()),
                ));
            }
        }
        if let Err(error) = probe_writable_directory(root) {
            cleanup_created_output_roots(&created);
            return Err(error);
        }
    }
    Ok(created)
}

fn probe_writable_directory(root: &Path) -> Result<(), ProjectError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    for attempt in 0..32_u8 {
        let probe = root.join(format!(
            ".trajecta-write-probe-{}-{nonce}-{attempt}",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(mut file) => {
                let result = file.write_all(b"probe").and_then(|()| file.sync_all());
                drop(file);
                let cleanup = fs::remove_file(&probe);
                return match (result, cleanup) {
                    (Ok(()), Ok(())) => Ok(()),
                    (Err(error), _) | (Ok(()), Err(error)) => Err(ProjectError::new(
                        "project.output_root_unwritable",
                        format!("probe {}: {error}", root.display()),
                    )),
                };
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ProjectError::new(
                    "project.output_root_unwritable",
                    format!("probe {}: {error}", root.display()),
                ));
            }
        }
    }
    Err(ProjectError::new(
        "project.output_root_unwritable",
        format!("could not allocate a write probe in {}", root.display()),
    ))
}

fn rollback_created_locks(installed: &[(PathBuf, Vec<u8>)]) -> Result<(), ProjectError> {
    let mut failures = Vec::new();
    for (path, expected) in installed.iter().rev() {
        match fs::read(path) {
            Ok(actual) if actual == *expected => {
                if let Err(error) = fs::remove_file(path) {
                    failures.push(format!("remove {}: {error}", path.display()));
                }
            }
            Ok(_) => failures.push(format!(
                "refused to remove concurrently changed lock {}",
                path.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("inspect {}: {error}", path.display())),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ProjectError::new(
            "project.finalize_rollback_failed",
            failures.join("; "),
        ))
    }
}

fn cleanup_created_output_roots(roots: &[PathBuf]) {
    for root in roots.iter().rev() {
        let _ = fs::remove_dir(root);
    }
}

fn combine_finalize_error(
    primary: ProjectError,
    rollback: Result<(), ProjectError>,
) -> ProjectError {
    match rollback {
        Ok(()) => primary,
        Err(rollback) => ProjectError::new(
            primary.code,
            format!(
                "{}; rollback also failed: {}",
                primary.message, rollback.message
            ),
        ),
    }
}

fn resolve_case(path: &Path, root: &Path) -> Result<Option<ResolvedCase>, ProjectError> {
    let text = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let raw: Value = serde_yml::from_str(&text)
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    validate_partial_identity(&raw, "case")?;
    let parsed: CaseDocument = match trajecta_case::schema::parse_case_yaml(&text) {
        Ok(parsed) => parsed,
        Err(error) if schema_error_is_missing_field(&error) => return Ok(None),
        Err(error) => {
            return Err(ProjectError::new(
                "project.document_invalid",
                error.to_string(),
            ));
        }
    };
    let bag = parsed
        .validate_shape()
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    if bag.has_errors() {
        return Err(ProjectError::new(
            "project.document_invalid",
            format!("Case shape validation failed at {}", path.display()),
        ));
    }
    let resolved = expand_case_file(path, &LocalRefResolver::new(root))
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    let intent = IntentValidator::validate(&resolved, ValidationIntent::Simulation);
    if intent.has_errors() {
        return Ok(None);
    }
    Ok(Some(resolved))
}

fn resolve_profile(
    path: &Path,
    root: &Path,
) -> Result<Option<ProjectProfileSnapshot>, ProjectError> {
    let text = match fs::read_to_string(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let raw: Value = serde_yml::from_str(&text)
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    validate_partial_identity(&raw, "run_profile")?;
    let parsed: RunProfileDocument = match trajecta_case::schema::parse_run_profile_yaml(&text) {
        Ok(parsed) => parsed,
        Err(error) if schema_error_is_missing_field(&error) => return Ok(None),
        Err(error) => {
            return Err(ProjectError::new(
                "project.document_invalid",
                error.to_string(),
            ));
        }
    };
    let bag = parsed
        .validate_shape()
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    if bag.has_errors() {
        return Err(ProjectError::new(
            "project.document_invalid",
            format!("RunProfile shape validation failed at {}", path.display()),
        ));
    }
    let base = path
        .parent()
        .ok_or_else(|| ProjectError::new("project.document_invalid", "profile has no parent"))?;
    let case_path = resolve_project_relative(root, base, &parsed.case_path, true)?;
    let output_root = resolve_project_relative(root, base, &parsed.output_root, false)?;
    let datasets = parsed
        .datasets
        .into_iter()
        .map(|mut binding| {
            binding.lockfile = resolve_project_relative(root, base, &binding.lockfile, false)?;
            binding.cache_root = binding
                .cache_root
                .map(|value| resolve_project_relative(root, base, &value, false))
                .transpose()?;
            binding.data_roots = binding
                .data_roots
                .into_iter()
                .map(|(id, value)| Ok((id, resolve_project_relative(root, base, &value, false)?)))
                .collect::<Result<_, ProjectError>>()?;
            Ok(binding)
        })
        .collect::<Result<Vec<_>, ProjectError>>()?;
    let profile_sources = parsed
        .profile_sources
        .into_iter()
        .map(|source| match source {
            ProfileSource::File { path } => Ok(ProfileSource::File {
                path: resolve_project_relative(root, base, &path, true)?,
            }),
            ProfileSource::Directory { path } => Ok(ProfileSource::Directory {
                path: resolve_project_relative(root, base, &path, true)?,
            }),
        })
        .collect::<Result<Vec<_>, ProjectError>>()?;
    Ok(Some(ProjectProfileSnapshot {
        case_path,
        datasets,
        output_root,
        profile_sources,
        reader_backend: parsed.execution.meteorology_reader,
    }))
}

fn resolve_project_relative(
    root: &Path,
    base: &Path,
    value: &Path,
    must_exist: bool,
) -> Result<PathBuf, ProjectError> {
    if value.is_absolute() {
        return Err(ProjectError::new(
            "project.path_escape",
            "project document paths must be relative",
        ));
    }
    let candidate = lexical_normalize(base.join(value));
    resolve_contained_path(root, candidate, must_exist)
}

fn resolve_contained_path(
    root: &Path,
    candidate: PathBuf,
    must_exist: bool,
) -> Result<PathBuf, ProjectError> {
    if !candidate.starts_with(root) {
        return Err(ProjectError::new(
            "project.path_escape",
            "project path escapes project root",
        ));
    }
    match fs::canonicalize(&candidate) {
        Ok(canonical) => {
            if !canonical.starts_with(root) {
                return Err(ProjectError::new(
                    "project.path_escape",
                    "project path resolves outside project root",
                ));
            }
            Ok(canonical)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !must_exist => {
            let ancestor = nearest_existing_ancestor(&candidate)?;
            let canonical = fs::canonicalize(ancestor).map_err(io_error)?;
            if !canonical.starts_with(root) {
                return Err(ProjectError::new(
                    "project.path_escape",
                    "project path has an ancestor outside project root",
                ));
            }
            Ok(candidate)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn nearest_existing_ancestor(path: &Path) -> Result<&Path, ProjectError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = candidate.parent();
            }
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(ProjectError::new(
        "project.path_escape",
        "project path has no existing contained ancestor",
    ))
}

fn validate_partial_identity(value: &Value, expected_kind: &str) -> Result<(), ProjectError> {
    let object = value.as_object().ok_or_else(|| {
        ProjectError::new(
            "project.document_invalid",
            "project document must be a mapping",
        )
    })?;
    if let Some(version) = object.get("schema_version") {
        if version.as_u64() != Some(u64::from(CURRENT_SCHEMA_VERSION)) {
            return Err(ProjectError::new(
                "project.document_invalid",
                format!("schema_version must be {CURRENT_SCHEMA_VERSION}"),
            ));
        }
    }
    if let Some(kind) = object.get("kind") {
        if kind.as_str() != Some(expected_kind) {
            return Err(ProjectError::new(
                "project.document_invalid",
                format!("kind must be {expected_kind}"),
            ));
        }
    }
    Ok(())
}

fn schema_error_is_missing_field(error: &SchemaError) -> bool {
    matches!(error, SchemaError::Parse(message) if message.contains("missing field"))
}

fn lexical_normalize(path: PathBuf) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => result.push(prefix.as_os_str()),
            Component::RootDir => result.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            Component::Normal(value) => result.push(value),
        }
    }
    result
}

fn same_path(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left).ok() == fs::canonicalize(right).ok()
}

fn read_selector(project: &Project, selector: &str) -> Result<Value, ProjectError> {
    let (kind, name, tail) = split_selector(selector)?;
    match kind.as_str() {
        "index" => lookup(
            &serde_json::to_value(&project.index).map_err(json_error)?,
            &tail,
        )
        .cloned(),
        "case" => read_document_selector(project, &project.index.cases, name.as_deref(), &tail),
        "profile" => {
            let paths = project
                .index
                .profiles
                .iter()
                .map(|(key, value)| (key.clone(), value.path.clone()))
                .collect::<BTreeMap<_, _>>();
            read_document_selector(project, &paths, name.as_deref(), &tail)
        }
        _ => None,
    }
    .ok_or_else(|| {
        ProjectError::new(
            "project.invalid_selector",
            format!("unknown selector `{selector}`"),
        )
    })
}

fn read_document_selector(
    project: &Project,
    paths: &BTreeMap<String, String>,
    name: Option<&str>,
    tail: &[String],
) -> Option<Value> {
    let path = indexed_path(project, paths.get(name?)?).ok()?;
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_yml::from_str(&text).ok()?;
    lookup(&value, tail).cloned()
}

fn set_selector(project: &Project, selector: &str, value: Value) -> Result<(), ProjectError> {
    let (kind, name, tail) = split_selector(selector)?;
    match kind.as_str() {
        "index" => {
            let mut index = serde_json::to_value(&project.index).map_err(json_error)?;
            replace_value(&mut index, &tail, value, true)?;
            let decoded: ProjectIndex = serde_json::from_value(index)
                .map_err(|error| ProjectError::new("project.invalid_index", error.to_string()))?;
            validate_index(&decoded)?;
            validate_index_paths(&project.root, &decoded)?;
            write_yaml_atomic(&project.index_path, &decoded)
        }
        "case" => mutate_document(
            project,
            &project.index.cases,
            name.as_deref(),
            &tail,
            value,
            true,
            false,
        ),
        "profile" => {
            let paths = project
                .index
                .profiles
                .iter()
                .map(|(key, item)| (key.clone(), item.path.clone()))
                .collect::<BTreeMap<_, _>>();
            mutate_document(project, &paths, name.as_deref(), &tail, value, true, true)
        }
        _ => Err(ProjectError::new(
            "project.invalid_selector",
            "unknown selector root",
        )),
    }
}

fn unset_selector(project: &Project, selector: &str) -> Result<(), ProjectError> {
    let (kind, name, tail) = split_selector(selector)?;
    match kind.as_str() {
        "index" => Err(ProjectError::new(
            "project.invalid_selector",
            "index required fields may not be unset",
        )),
        "case" => mutate_document(
            project,
            &project.index.cases,
            name.as_deref(),
            &tail,
            Value::Null,
            false,
            false,
        ),
        "profile" => {
            let paths = project
                .index
                .profiles
                .iter()
                .map(|(key, item)| (key.clone(), item.path.clone()))
                .collect::<BTreeMap<_, _>>();
            mutate_document(
                project,
                &paths,
                name.as_deref(),
                &tail,
                Value::Null,
                false,
                true,
            )
        }
        _ => Err(ProjectError::new(
            "project.invalid_selector",
            "unknown selector root",
        )),
    }
}

fn split_selector(selector: &str) -> Result<(String, Option<String>, Vec<String>), ProjectError> {
    let parts = selector.split('.').collect::<Vec<_>>();
    match parts.as_slice() {
        ["index", tail @ ..] if !tail.is_empty() => Ok((
            "index".into(),
            None,
            tail.iter().map(|value| (*value).into()).collect(),
        )),
        ["case", name, tail @ ..] if !name.is_empty() && !tail.is_empty() => Ok((
            "case".into(),
            Some((*name).into()),
            tail.iter().map(|value| (*value).into()).collect(),
        )),
        ["profile", name, tail @ ..] if !name.is_empty() && !tail.is_empty() => Ok((
            "profile".into(),
            Some((*name).into()),
            tail.iter().map(|value| (*value).into()).collect(),
        )),
        _ => Err(ProjectError::new(
            "project.invalid_selector",
            "selector must address index.<field>, case.<name>.<field>, or profile.<name>.<field>",
        )),
    }
}

fn mutate_document(
    project: &Project,
    paths: &BTreeMap<String, String>,
    name: Option<&str>,
    tail: &[String],
    value: Value,
    set: bool,
    is_profile: bool,
) -> Result<(), ProjectError> {
    let name =
        name.ok_or_else(|| ProjectError::new("project.invalid_selector", "missing document name"))?;
    let relative = paths
        .get(name)
        .ok_or_else(|| ProjectError::new("project.invalid_selector", "unknown indexed document"))?;
    let path = indexed_path(project, relative)?;
    let mut document = if path.exists() {
        let text = fs::read_to_string(&path).map_err(io_error)?;
        serde_yml::from_str(&text)
            .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?
    } else {
        Value::Object(Default::default())
    };
    if set {
        replace_value(&mut document, tail, value, true)?;
    } else {
        remove_value(&mut document, tail)?;
    }
    validate_mutated_document(&document, is_profile)?;
    let path = indexed_path(project, relative)?;
    write_yaml_value_atomic(&path, &document)
}

fn validate_mutated_document(value: &Value, is_profile: bool) -> Result<(), ProjectError> {
    validate_partial_identity(value, if is_profile { "run_profile" } else { "case" })?;
    let text = serde_yml::to_string(value)
        .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
    if is_profile {
        let document = match trajecta_case::schema::parse_run_profile_yaml(&text) {
            Ok(document) => document,
            Err(error) if schema_error_is_missing_field(&error) => return Ok(()),
            Err(error) => {
                return Err(ProjectError::new(
                    "project.document_invalid",
                    error.to_string(),
                ));
            }
        };
        let diagnostics = document
            .validate_shape()
            .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
        if diagnostics.has_errors() {
            return Err(ProjectError::new(
                "project.document_invalid",
                "profile mutation violates shape contract",
            ));
        }
    } else {
        let document = match trajecta_case::schema::parse_case_yaml(&text) {
            Ok(document) => document,
            Err(error) if schema_error_is_missing_field(&error) => return Ok(()),
            Err(error) => {
                return Err(ProjectError::new(
                    "project.document_invalid",
                    error.to_string(),
                ));
            }
        };
        let diagnostics = document
            .validate_shape()
            .map_err(|error| ProjectError::new("project.document_invalid", error.to_string()))?;
        if diagnostics.has_errors() {
            return Err(ProjectError::new(
                "project.document_invalid",
                "case mutation violates shape contract",
            ));
        }
    }
    Ok(())
}

fn data_plan(project: &Project) -> Result<Value, ProjectError> {
    let lock_uses = project_lock_uses(project)?;
    let mut cases = BTreeMap::new();
    let mut case_values = BTreeMap::new();
    for (name, path) in &project.index.cases {
        let full = indexed_path(project, path)?;
        let case = resolve_case(&full, &project.root)?
            .ok_or_else(|| ProjectError::new("project.not_configured", "case is draft"))?;
        cases.insert(
            name.clone(),
            serde_yml::from_str::<Value>(&fs::read_to_string(&full).map_err(io_error)?).map_err(
                |error| ProjectError::new("project.document_invalid", error.to_string()),
            )?,
        );
        case_values.insert(name.clone(), (full, case));
    }
    let mut profiles_value = BTreeMap::new();
    let mut requirements = Vec::new();
    for (profile_name, entry) in &project.index.profiles {
        let profile_path = indexed_path(project, &entry.path)?;
        let profile = resolve_profile(&profile_path, &project.root)?
            .ok_or_else(|| ProjectError::new("project.not_configured", "profile is draft"))?;
        profiles_value.insert(
            profile_name.clone(),
            serde_yml::from_str::<Value>(&fs::read_to_string(&profile_path).map_err(io_error)?)
                .map_err(|error| {
                    ProjectError::new("project.document_invalid", error.to_string())
                })?,
        );
        let (case_name, (_, _case)) = case_values
            .iter()
            .find(|(_, (path, _))| same_path(path, &profile.case_path))
            .ok_or_else(|| {
                ProjectError::new(
                    "project.profile_case_mismatch",
                    "profile Case is not indexed",
                )
            })?;
        for binding in &profile.datasets {
            let dataset_id = binding.dataset.0.clone();
            let lock_use = lock_uses
                .iter()
                .find(|item| {
                    item.project_profile == *profile_name
                        && item.case_name == *case_name
                        && item.spec.dataset == binding.dataset
                })
                .ok_or_else(|| {
                    ProjectError::new(
                        "project.dataset_profiles_mismatch",
                        "profile dataset mapping is missing",
                    )
                })?;
            let lockfile = relative_string(&project.root, &lock_use.spec.output)?;
            let data_roots = lock_use
                .spec
                .data_roots
                .iter()
                .map(|(key, value)| Ok((key.0.clone(), relative_string(&project.root, value)?)))
                .collect::<Result<BTreeMap<_, _>, ProjectError>>()?;
            let status = lock_status(&lock_use.spec)?;
            let mut required_capabilities = lock_use
                .spec
                .capabilities
                .iter()
                .map(|value| serde_json::to_value(value).map_err(json_error))
                .collect::<Result<Vec<_>, _>>()?;
            required_capabilities
                .sort_by_key(|value| value.as_str().unwrap_or_default().to_owned());
            let mut requirement = serde_json::Map::new();
            requirement.insert("profile_name".into(), Value::String(profile_name.clone()));
            requirement.insert("case_name".into(), Value::String(case_name.clone()));
            requirement.insert("dataset_id".into(), Value::String(dataset_id));
            requirement.insert(
                "dataset_profile".into(),
                Value::String(lock_use.spec.profile_name.clone()),
            );
            requirement.insert(
                "coverage_start".into(),
                serde_json::to_value(lock_use.spec.coverage.start).map_err(json_error)?,
            );
            requirement.insert(
                "coverage_end".into(),
                serde_json::to_value(lock_use.spec.coverage.end).map_err(json_error)?,
            );
            requirement.insert("lockfile".into(), Value::String(lockfile));
            if let Some(cache_root) = &binding.cache_root {
                requirement.insert(
                    "cache_root".into(),
                    Value::String(relative_string(&project.root, cache_root)?),
                );
            }
            requirement.insert(
                "data_roots".into(),
                serde_json::to_value(data_roots).map_err(json_error)?,
            );
            requirement.insert(
                "reader_backend".into(),
                serde_json::to_value(lock_use.spec.backend).map_err(json_error)?,
            );
            requirement.insert(
                "required_capabilities".into(),
                Value::Array(required_capabilities.clone()),
            );
            requirement.insert("status".into(), Value::String(status.into()));
            requirements.push(Value::Object(requirement));
        }
    }
    requirements.sort_by_key(|value| {
        (
            value["profile_name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            value["case_name"].as_str().unwrap_or_default().to_owned(),
            value["dataset_id"].as_str().unwrap_or_default().to_owned(),
        )
    });
    let canonical =
        serde_json::json!({"index": project.index, "cases": cases, "profiles": profiles_value});
    let project_sha256 = hex::encode(Sha256::digest(canonical_json(&canonical)?));
    let plan = serde_json::json!({"schema_version":"trajecta.data-plan/v1", "project_sha256": project_sha256, "requirements": requirements});
    validate_data_plan_shape(&plan)?;
    Ok(plan)
}

fn validate_data_plan_shape(plan: &Value) -> Result<(), ProjectError> {
    let object = plan.as_object().ok_or_else(|| {
        ProjectError::new("project.data_plan_schema", "data plan must be an object")
    })?;
    if object.len() != 3
        || object.get("schema_version").and_then(Value::as_str) != Some("trajecta.data-plan/v1")
    {
        return Err(ProjectError::new(
            "project.data_plan_schema",
            "invalid data-plan envelope",
        ));
    }
    let digest = object
        .get("project_sha256")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProjectError::new(
            "project.data_plan_schema",
            "project_sha256 must be hexadecimal SHA-256",
        ));
    }
    let requirements = object
        .get("requirements")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ProjectError::new("project.data_plan_schema", "requirements must be an array")
        })?;
    let mut previous_key = None;
    for requirement in requirements {
        let item = requirement.as_object().ok_or_else(|| {
            ProjectError::new("project.data_plan_schema", "requirement must be an object")
        })?;
        let required = [
            "profile_name",
            "case_name",
            "dataset_id",
            "dataset_profile",
            "coverage_start",
            "coverage_end",
            "lockfile",
            "data_roots",
            "reader_backend",
            "required_capabilities",
            "status",
        ];
        if !required.iter().all(|key| item.contains_key(*key))
            || item
                .keys()
                .any(|key| !required.contains(&key.as_str()) && key != "cache_root")
        {
            return Err(ProjectError::new(
                "project.data_plan_schema",
                "invalid requirement keys",
            ));
        }
        let mut names = Vec::new();
        for key in [
            "profile_name",
            "case_name",
            "dataset_id",
            "dataset_profile",
            "lockfile",
        ] {
            let value = item
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProjectError::new("project.data_plan_schema", "required path/name is empty")
                })?;
            names.push(value);
        }
        let key = (names[0], names[1], names[2]);
        if previous_key.is_some_and(|previous| previous >= key) {
            return Err(ProjectError::new(
                "project.data_plan_schema",
                "requirements must be strictly sorted and unique by profile, case, dataset",
            ));
        }
        previous_key = Some(key);
        validate_index_relative_path(names[4]).map_err(|_| {
            ProjectError::new(
                "project.data_plan_schema",
                "lockfile must be project-relative",
            )
        })?;
        if let Some(cache_root) = item.get("cache_root") {
            let cache_root = cache_root
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    ProjectError::new(
                        "project.data_plan_schema",
                        "cache_root must be omitted or non-empty string",
                    )
                })?;
            validate_index_relative_path(cache_root).map_err(|_| {
                ProjectError::new(
                    "project.data_plan_schema",
                    "cache_root must be project-relative",
                )
            })?;
        }
        let mut coverage = Vec::new();
        for key in ["coverage_start", "coverage_end"] {
            let timestamp = item.get(key).and_then(Value::as_object).ok_or_else(|| {
                ProjectError::new(
                    "project.data_plan_schema",
                    "coverage timestamp must be object",
                )
            })?;
            let seconds = timestamp
                .get("seconds_since_unix_epoch")
                .and_then(Value::as_i64);
            let nanosecond = timestamp.get("nanosecond").and_then(Value::as_u64);
            if timestamp.len() != 2
                || seconds.is_none()
                || nanosecond.is_none_or(|value| value > 999_999_999)
            {
                return Err(ProjectError::new(
                    "project.data_plan_schema",
                    "invalid coverage timestamp",
                ));
            }
            coverage.push((seconds.unwrap_or_default(), nanosecond.unwrap_or_default()));
        }
        if coverage[0] > coverage[1] {
            return Err(ProjectError::new(
                "project.data_plan_schema",
                "coverage_start must not exceed coverage_end",
            ));
        }
        if !matches!(
            item.get("reader_backend").and_then(Value::as_str),
            Some("rust" | "native")
        ) || !matches!(
            item.get("status").and_then(Value::as_str),
            Some("missing" | "partial" | "ready")
        ) {
            return Err(ProjectError::new(
                "project.data_plan_schema",
                "invalid reader backend or status",
            ));
        }
        let roots = item
            .get("data_roots")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                ProjectError::new("project.data_plan_schema", "data_roots must be an object")
            })?;
        for (key, value) in roots {
            let path = value
                .as_str()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| {
                    ProjectError::new("project.data_plan_schema", "invalid data root")
                })?;
            if key.is_empty() {
                return Err(ProjectError::new(
                    "project.data_plan_schema",
                    "invalid data root",
                ));
            }
            validate_index_relative_path(path).map_err(|_| {
                ProjectError::new(
                    "project.data_plan_schema",
                    "data root must be project-relative",
                )
            })?;
        }
        let capabilities = item
            .get("required_capabilities")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProjectError::new(
                    "project.data_plan_schema",
                    "required_capabilities must be an array",
                )
            })?;
        let names = capabilities
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .collect::<Option<BTreeSet<_>>>()
            .ok_or_else(|| ProjectError::new("project.data_plan_schema", "invalid capability"))?;
        if names.len() != capabilities.len()
            || !capabilities
                .windows(2)
                .all(|pair| pair[0].as_str() < pair[1].as_str())
        {
            return Err(ProjectError::new(
                "project.data_plan_schema",
                "capabilities must be sorted and unique",
            ));
        }
    }
    Ok(())
}

fn lock_status(spec: &LockSpec) -> Result<&'static str, ProjectError> {
    if !spec.output.is_file() {
        return Ok(if spec.data_roots.values().any(|path| path.exists()) {
            "partial"
        } else {
            "missing"
        });
    }
    validate_existing_lock(spec).map_err(project_data_lock_error)?;
    Ok("ready")
}

fn relative_string(root: &Path, path: &Path) -> Result<String, ProjectError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        ProjectError::new("project.path_escape", "data-plan path escapes project root")
    })?;
    relative
        .to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            ProjectError::new(
                "project.path_not_utf8",
                "data-plan paths must be valid UTF-8",
            )
        })
}
fn parse_input_value(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into()))
}
fn lookup<'a>(value: &'a Value, tail: &[String]) -> Option<&'a Value> {
    tail.iter().try_fold(value, |current, key| current.get(key))
}
fn replace_value(
    value: &mut Value,
    tail: &[String],
    replacement: Value,
    create: bool,
) -> Result<(), ProjectError> {
    let Some((last, parents)) = tail.split_last() else {
        return Err(ProjectError::new(
            "project.invalid_selector",
            "empty selector",
        ));
    };
    let mut current = value;
    for key in parents {
        let object = current.as_object_mut().ok_or_else(|| {
            ProjectError::new("project.invalid_selector", "selector crosses a scalar")
        })?;
        if !object.contains_key(key) && create {
            object.insert(key.clone(), Value::Object(Default::default()));
        }
        current = object
            .get_mut(key)
            .ok_or_else(|| ProjectError::new("project.invalid_selector", "unknown selector"))?;
    }
    let object = current.as_object_mut().ok_or_else(|| {
        ProjectError::new(
            "project.invalid_selector",
            "selector parent is not a mapping",
        )
    })?;
    if !create && !object.contains_key(last) {
        return Err(ProjectError::new(
            "project.invalid_selector",
            "unknown selector",
        ));
    }
    object.insert(last.clone(), replacement);
    Ok(())
}
fn remove_value(value: &mut Value, tail: &[String]) -> Result<(), ProjectError> {
    let Some((last, parents)) = tail.split_last() else {
        return Err(ProjectError::new(
            "project.invalid_selector",
            "empty selector",
        ));
    };
    let mut current = value;
    for key in parents {
        current = current
            .get_mut(key)
            .ok_or_else(|| ProjectError::new("project.invalid_selector", "unknown selector"))?;
    }
    current
        .as_object_mut()
        .and_then(|object| object.remove(last))
        .ok_or_else(|| ProjectError::new("project.invalid_selector", "unknown selector"))?;
    Ok(())
}
fn canonical_json(value: &Value) -> Result<Vec<u8>, ProjectError> {
    serde_json::to_vec(value).map_err(json_error)
}
fn write_yaml_atomic(path: &Path, value: &impl Serialize) -> Result<(), ProjectError> {
    write_file_atomic(
        path,
        serde_yml::to_string(value)
            .map_err(|error| ProjectError::new("project.write_failed", error.to_string()))?
            .as_bytes(),
    )
}
fn write_yaml_value_atomic(path: &Path, value: &Value) -> Result<(), ProjectError> {
    write_file_atomic(
        path,
        serde_yml::to_string(value)
            .map_err(|error| ProjectError::new("project.write_failed", error.to_string()))?
            .as_bytes(),
    )
}
fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), ProjectError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProjectError::new("project.write_failed", "path has no parent"))?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    let temporary = parent.join(format!(".trajecta-{}-{nonce}.tmp", std::process::id()));
    fs::write(&temporary, bytes).map_err(io_error)?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ProjectError::new("project.write_failed", error.to_string())
    })
}
fn diagnostic(severity: &str, code: &str, message: impl Into<String>) -> Diagnostic {
    let message = message.into();
    match severity {
        "error" => Diagnostic::error(code, message),
        "warning" => Diagnostic::warning(code, message),
        _ => Diagnostic::info(code, message),
    }
}
fn json_error(error: impl std::fmt::Display) -> ProjectError {
    ProjectError::new("project.document_invalid", error.to_string())
}
fn io_error(error: std::io::Error) -> ProjectError {
    ProjectError::new("project.write_failed", error.to_string())
}

pub(crate) struct ProjectOutcome {
    pub(crate) data: Value,
    pub(crate) diagnostics: Vec<Diagnostic>,
}
impl ProjectOutcome {
    fn ok(data: Value) -> Self {
        Self {
            data,
            diagnostics: Vec::new(),
        }
    }
    fn with_diagnostics(data: Value, diagnostics: Vec<Diagnostic>) -> Self {
        Self { data, diagnostics }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct ProjectError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}
impl ProjectError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde_json::{Value, json};
    use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
    use trajecta_case::model::meteorology::{DatasetRef, DomainId};
    use trajecta_case::model::time::Timestamp;
    use trajecta_met::field::{Capability, CapabilitySet};
    use trajecta_met::io::lock_builder::LockCoverageRequest;

    use super::{LockSpec, ProjectLockUse, group_lock_uses, validate_data_plan_shape};

    fn lock_use(
        project_profile: &str,
        case_name: &str,
        start: i64,
        end: i64,
        capability: Capability,
    ) -> ProjectLockUse {
        let mut capabilities = CapabilitySet::new();
        capabilities.insert(capability);
        ProjectLockUse {
            project_profile: project_profile.into(),
            case_name: case_name.into(),
            output_root: PathBuf::from("runs"),
            spec: LockSpec {
                output: PathBuf::from("locks/shared.lock.json"),
                dataset: DatasetRef("met".into()),
                source: "local data for met".into(),
                data_roots: BTreeMap::from([(DataRootId("raw".into()), PathBuf::from("data"))]),
                profile_name: "era5-flex-extract-hybrid-v0".into(),
                profile_sources: Vec::new(),
                backend: MeteorologyReaderBackend::Rust,
                coverage: LockCoverageRequest {
                    start: Timestamp::new(start, 0).unwrap(),
                    end: Timestamp::new(end, 0).unwrap(),
                    interpolation_before_frames: 1,
                    interpolation_after_frames: 1,
                },
                capabilities,
                domain: DomainId("global".into()),
            },
        }
    }

    #[test]
    fn shared_lock_group_unions_coverage_and_capabilities_but_rejects_conflicts() {
        let first = lock_use("one", "forward", 10, 20, Capability::Transport);
        let second = lock_use("two", "backward", 5, 30, Capability::WetDeposition);
        let groups = group_lock_uses(&[first.clone(), second.clone()]).unwrap();
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert_eq!(group.spec.coverage.start, Timestamp::new(5, 0).unwrap());
        assert_eq!(group.spec.coverage.end, Timestamp::new(30, 0).unwrap());
        assert!(group.spec.capabilities.contains(Capability::Transport));
        assert!(group.spec.capabilities.contains(Capability::WetDeposition));
        assert_eq!(group.consumers.len(), 2);

        let mut conflicting = second;
        conflicting.spec.backend = MeteorologyReaderBackend::Native;
        let error = group_lock_uses(&[first, conflicting]).unwrap_err();
        assert_eq!(error.code, "project.lock_binding_conflict");
    }

    fn plan() -> Value {
        let requirement = |profile: &str, case_name: &str, dataset: &str| {
            json!({
                "profile_name": profile,
                "case_name": case_name,
                "dataset_id": dataset,
                "dataset_profile": "hybrid",
                "coverage_start": {"seconds_since_unix_epoch": 1, "nanosecond": 0},
                "coverage_end": {"seconds_since_unix_epoch": 2, "nanosecond": 0},
                "lockfile": "locks/input.lock.json",
                "data_roots": {"data": "data/era5"},
                "reader_backend": "rust",
                "required_capabilities": ["alpha", "beta"],
                "status": "ready"
            })
        };
        json!({
            "schema_version": "trajecta.data-plan/v1",
            "project_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            "requirements": [
                requirement("a", "case", "one"),
                requirement("b", "case", "two")
            ]
        })
    }

    #[test]
    fn data_plan_guard_rejects_unsorted_duplicate_and_escaping_shapes() {
        let mut unsorted = plan();
        let Some(requirements) = unsorted["requirements"].as_array_mut() else {
            panic!("fixture requirements must be an array");
        };
        requirements.reverse();
        let mut duplicate = plan();
        duplicate["requirements"][1]["profile_name"] = json!("a");
        duplicate["requirements"][1]["dataset_id"] = json!("one");
        let mut unsorted_capabilities = plan();
        unsorted_capabilities["requirements"][0]["required_capabilities"] =
            json!(["beta", "alpha"]);
        let mut inverted_coverage = plan();
        inverted_coverage["requirements"][0]["coverage_start"]["seconds_since_unix_epoch"] =
            json!(3);
        let mut escaping_lockfile = plan();
        escaping_lockfile["requirements"][0]["lockfile"] = json!("../input.lock.json");
        let mut absolute_root = plan();
        absolute_root["requirements"][0]["data_roots"]["data"] = json!("/outside");
        let mut null_cache_root = plan();
        null_cache_root["requirements"][0]["cache_root"] = Value::Null;
        for invalid in [
            unsorted,
            duplicate,
            unsorted_capabilities,
            inverted_coverage,
            escaping_lockfile,
            absolute_root,
            null_cache_root,
        ] {
            let error = match validate_data_plan_shape(&invalid) {
                Ok(()) => panic!("malformed data-plan fixture must fail validation"),
                Err(error) => error,
            };
            assert_eq!(error.code, "project.data_plan_schema");
        }
    }
}
