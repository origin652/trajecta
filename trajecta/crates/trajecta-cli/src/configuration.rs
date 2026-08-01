use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use trajecta_case::document::MeteorologyReaderBackend as ReaderBackend;

use crate::command::config::ConfigCommand;
use crate::command_result::CommandError as ConfigError;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    schema_version: String,
    default_reader_backend: ReaderBackend,
    daemon: Daemon,
    resources: Resources,
    monitoring: Monitoring,
    profile_templates: BTreeMap<String, ProfileTemplate>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Daemon {
    idle_shutdown_seconds: u64,
    local_ipc_only: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Resources {
    cpu_slots: usize,
    memory_pool_mib: u64,
    memory_reserve_mib: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Monitoring {
    sample_interval_ms: u64,
    maximum_median_overhead_percent: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileTemplate {
    execution: TemplateExecution,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TemplateExecution {
    worker_threads: usize,
    memory_budget_bytes: u64,
    executor: String,
    meteorology_reader: ReaderBackend,
}

pub(crate) fn selected_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Some(value) = env::var_os("TRAJECTA_CONFIG") {
        return PathBuf::from(value);
    }
    if cfg!(windows) {
        let base = env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        return base.join("Trajecta").join("config.toml");
    }
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("trajecta").join("config.toml")
}

/// Loads and semantically validates the selected configuration for another local command.
pub(crate) fn validate_selected(explicit_path: Option<&Path>) -> Result<PathBuf, ConfigError> {
    let path = selected_path(explicit_path);
    load_valid(&path)?;
    Ok(path)
}

/// Validated local-daemon settings derived only from the persisted machine configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeSettings {
    pub(crate) config_path: PathBuf,
    pub(crate) catalog_path: PathBuf,
    pub(crate) endpoint: String,
    pub(crate) cpu_slots: u32,
    pub(crate) schedulable_memory_mib: u64,
    pub(crate) memory_reserve_mib: u64,
    pub(crate) sample_interval_ms: u64,
    pub(crate) idle_shutdown_seconds: u64,
}

/// Loads the selected configuration and derives stable local runtime paths.
pub(crate) fn runtime_settings(
    explicit_path: Option<&Path>,
) -> Result<RuntimeSettings, ConfigError> {
    let selected = selected_path(explicit_path);
    let config = load_valid(&selected)?;
    let config_path = fs::canonicalize(&selected).map_err(|error| {
        ConfigError::new(
            "config.not_found",
            format!("canonicalize {}: {error}", selected.display()),
        )
    })?;
    let parent = config_path.parent().ok_or_else(|| {
        ConfigError::new("config.invalid_schema", "configuration path has no parent")
    })?;
    let cpu_slots = u32::try_from(config.resources.cpu_slots).map_err(|_| {
        ConfigError::new(
            "config.invalid_schema",
            "resources.cpu_slots exceeds the local scheduler representation",
        )
    })?;
    let schedulable_memory_mib = config
        .resources
        .memory_pool_mib
        .checked_sub(config.resources.memory_reserve_mib)
        .ok_or_else(|| {
            ConfigError::new(
                "config.invalid_schema",
                "resources.memory_reserve_mib exceeds resources.memory_pool_mib",
            )
        })?;
    let runtime_root = parent.join("runtime");
    Ok(RuntimeSettings {
        endpoint: daemon_endpoint(&config_path)?,
        config_path,
        catalog_path: runtime_root.join("jobs.sqlite3"),
        cpu_slots,
        schedulable_memory_mib,
        memory_reserve_mib: config.resources.memory_reserve_mib,
        sample_interval_ms: config.monitoring.sample_interval_ms,
        idle_shutdown_seconds: config.daemon.idle_shutdown_seconds,
    })
}

fn daemon_endpoint(config_path: &Path) -> Result<String, ConfigError> {
    let mut hasher = Sha256::new();
    hash_native_path(&mut hasher, config_path);
    let digest = hex::encode(hasher.finalize());
    #[cfg(windows)]
    {
        Ok(format!(r"\\.\pipe\trajecta-{}", &digest[..32]))
    }
    #[cfg(unix)]
    {
        let path = env::temp_dir().join(format!("trajecta-{}.sock", &digest[..24]));
        path.to_str().map(str::to_owned).ok_or_else(|| {
            ConfigError::new(
                "config.invalid_path",
                "local socket path cannot be represented as UTF-8",
            )
        })
    }
}

#[cfg(windows)]
fn hash_native_path(hasher: &mut Sha256, path: &Path) {
    use std::os::windows::ffi::OsStrExt;

    for unit in path.as_os_str().encode_wide() {
        hasher.update(unit.to_le_bytes());
    }
}

#[cfg(unix)]
fn hash_native_path(hasher: &mut Sha256, path: &Path) {
    use std::os::unix::ffi::OsStrExt;

    hasher.update(path.as_os_str().as_bytes());
}

pub(crate) fn execute(
    command: &ConfigCommand,
    explicit_path: Option<&Path>,
) -> Result<Value, ConfigError> {
    let path = selected_path(explicit_path);
    match command {
        ConfigCommand::Path => Ok(serde_json::json!({"path": path})),
        ConfigCommand::Init => {
            if path.exists() {
                return Err(ConfigError::new(
                    "config.already_exists",
                    "configuration already exists",
                ));
            }
            let config = default_config();
            validate(&config)?;
            write_atomic(&path, &config)?;
            Ok(serde_json::json!({"path": path, "created": true}))
        }
        ConfigCommand::Validate => {
            let config = load(&path)?;
            validate(&config)?;
            Ok(serde_json::json!({"path": path, "valid": true}))
        }
        ConfigCommand::List => {
            let config = load_valid(&path)?;
            let mut leaves = BTreeMap::new();
            flatten(
                "",
                &serde_json::to_value(config).map_err(serialize_error)?,
                &mut leaves,
            );
            Ok(serde_json::to_value(leaves).map_err(serialize_error)?)
        }
        ConfigCommand::Get { key } => {
            let config = load_valid(&path)?;
            let value = serde_json::to_value(config).map_err(serialize_error)?;
            let selected = lookup(&value, key).cloned().ok_or_else(|| {
                ConfigError::new(
                    "config.invalid_key",
                    format!("unknown configuration key `{key}`"),
                )
            })?;
            Ok(selected)
        }
        ConfigCommand::Set { key, value } => {
            let config = load_valid(&path)?;
            let mut document = serde_json::to_value(config).map_err(serialize_error)?;
            let replacement =
                serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone()));
            replace_existing(&mut document, key, replacement)?;
            let updated: ConfigFile = serde_json::from_value(document)
                .map_err(|error| ConfigError::new("config.invalid_value", error.to_string()))?;
            validate(&updated)?;
            write_atomic(&path, &updated)?;
            Ok(serde_json::json!({"path": path, "updated": key}))
        }
        ConfigCommand::Unset { key } => {
            let config = load_valid(&path)?;
            let mut document = serde_json::to_value(config).map_err(serialize_error)?;
            unset_template(&mut document, key)?;
            let updated: ConfigFile = serde_json::from_value(document)
                .map_err(|error| ConfigError::new("config.invalid_value", error.to_string()))?;
            validate(&updated)?;
            write_atomic(&path, &updated)?;
            Ok(serde_json::json!({"path": path, "unset": key}))
        }
    }
}

fn default_config() -> ConfigFile {
    let cpu_slots = std::thread::available_parallelism().map_or(1, |value| value.get());
    let memory_pool_mib = physical_memory_mib().unwrap_or(4_096).max(1);
    let memory_reserve_mib = (memory_pool_mib / 4).min(memory_pool_mib.saturating_sub(1));
    ConfigFile {
        schema_version: "trajecta.config/v1".into(),
        default_reader_backend: ReaderBackend::Rust,
        daemon: Daemon {
            idle_shutdown_seconds: 300,
            local_ipc_only: true,
        },
        resources: Resources {
            cpu_slots,
            memory_pool_mib,
            memory_reserve_mib,
        },
        monitoring: Monitoring {
            sample_interval_ms: 1_000,
            maximum_median_overhead_percent: 1.0,
        },
        profile_templates: BTreeMap::new(),
    }
}

fn physical_memory_mib() -> Option<u64> {
    #[cfg(not(windows))]
    {
        let body = fs::read_to_string("/proc/meminfo").ok()?;
        let kib = body.lines().find_map(|line| {
            let mut parts = line.split_whitespace();
            (parts.next() == Some("MemTotal:"))
                .then(|| parts.next()?.parse::<u64>().ok())
                .flatten()
        })?;
        Some(kib / 1024)
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "[int64](Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let bytes = std::str::from_utf8(&output.stdout)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?;
        Some(bytes / 1024 / 1024)
    }
}

fn load(path: &Path) -> Result<ConfigFile, ConfigError> {
    let text = fs::read_to_string(path).map_err(|error| {
        ConfigError::new(
            "config.not_found",
            format!("read {}: {error}", path.display()),
        )
    })?;
    toml::from_str(&text)
        .map_err(|error| ConfigError::new("config.invalid_schema", error.to_string()))
}

fn load_valid(path: &Path) -> Result<ConfigFile, ConfigError> {
    let config = load(path)?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &ConfigFile) -> Result<(), ConfigError> {
    if config.schema_version != "trajecta.config/v1" {
        return Err(ConfigError::new(
            "config.invalid_schema",
            "schema_version must be trajecta.config/v1",
        ));
    }
    if !config.daemon.local_ipc_only {
        return Err(ConfigError::new(
            "config.invalid_schema",
            "daemon.local_ipc_only must be true",
        ));
    }
    if config.resources.cpu_slots == 0 || config.resources.memory_pool_mib == 0 {
        return Err(ConfigError::new(
            "config.invalid_schema",
            "resource slots and pool must be positive",
        ));
    }
    if config.resources.memory_reserve_mib >= config.resources.memory_pool_mib {
        return Err(ConfigError::new(
            "config.invalid_schema",
            "resources.memory_reserve_mib must be smaller than resources.memory_pool_mib",
        ));
    }
    if config.monitoring.sample_interval_ms < 100
        || !config
            .monitoring
            .maximum_median_overhead_percent
            .is_finite()
        || !(0.0..=1.0).contains(&config.monitoring.maximum_median_overhead_percent)
    {
        return Err(ConfigError::new(
            "config.invalid_schema",
            "invalid monitoring values",
        ));
    }
    for (name, template) in &config.profile_templates {
        if name.trim().is_empty()
            || template.execution.worker_threads == 0
            || template.execution.worker_threads > config.resources.cpu_slots
            || template.execution.memory_budget_bytes == 0
            || template.execution.executor.trim().is_empty()
        {
            return Err(ConfigError::new(
                "config.invalid_schema",
                "invalid profile template",
            ));
        }
    }
    Ok(())
}

fn flatten(prefix: &str, value: &Value, leaves: &mut BTreeMap<String, Value>) {
    match value {
        Value::Object(values) => {
            for (key, child) in values {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&next, child, leaves);
            }
        }
        _ => {
            leaves.insert(prefix.into(), value.clone());
        }
    }
}

fn lookup<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(value, |current, segment| current.get(segment))
}

fn replace_existing(
    document: &mut Value,
    key: &str,
    replacement: Value,
) -> Result<(), ConfigError> {
    let parts = key.split('.').collect::<Vec<_>>();
    let Some((last, parents)) = parts.split_last() else {
        return Err(ConfigError::new(
            "config.invalid_key",
            "empty configuration key",
        ));
    };
    let mut current = document;
    for parent in parents {
        current = current.get_mut(*parent).ok_or_else(|| {
            ConfigError::new(
                "config.invalid_key",
                format!("unknown configuration key `{key}`"),
            )
        })?;
    }
    let object = current.as_object_mut().ok_or_else(|| {
        ConfigError::new(
            "config.invalid_key",
            format!("unknown configuration key `{key}`"),
        )
    })?;
    if let Some(slot) = object.get_mut(*last) {
        *slot = replacement;
        return Ok(());
    }
    if parents == ["profile_templates"] && !last.is_empty() {
        object.insert((*last).to_owned(), replacement);
        return Ok(());
    }
    Err(ConfigError::new(
        "config.invalid_key",
        format!("unknown configuration key `{key}`"),
    ))
}

fn unset_template(document: &mut Value, key: &str) -> Result<(), ConfigError> {
    let parts = key.split('.').collect::<Vec<_>>();
    if parts.len() != 2 || parts[0] != "profile_templates" || parts[1].is_empty() {
        return Err(ConfigError::new(
            "config.invalid_key",
            "only profile_templates.<name> may be unset",
        ));
    }
    let templates = document
        .get_mut("profile_templates")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ConfigError::new("config.invalid_schema", "profile_templates is invalid"))?;
    if templates.remove(parts[1]).is_none() {
        return Err(ConfigError::new(
            "config.invalid_key",
            format!("unknown configuration key `{key}`"),
        ));
    }
    Ok(())
}

fn write_atomic(path: &Path, config: &ConfigFile) -> Result<(), ConfigError> {
    let parent = path.parent().ok_or_else(|| {
        ConfigError::new("config.write_failed", "configuration path has no parent")
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| ConfigError::new("config.write_failed", error.to_string()))?;
    let text = toml::to_string_pretty(config).map_err(serialize_error)?;
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    let temporary = parent.join(format!(
        ".trajecta-config-{}-{token}.tmp",
        std::process::id()
    ));
    let mut file = fs::File::create(&temporary)
        .map_err(|error| ConfigError::new("config.write_failed", error.to_string()))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| ConfigError::new("config.write_failed", error.to_string()))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        ConfigError::new(
            "config.write_failed",
            format!("atomic replace {}: {error}", path.display()),
        )
    })
}

fn serialize_error(error: impl std::fmt::Display) -> ConfigError {
    ConfigError::new("config.invalid_schema", error.to_string())
}
