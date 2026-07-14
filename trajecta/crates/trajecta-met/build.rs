//! Validates and embeds repository-owned meteorology Profile documents.

use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "CARGO_MANIFEST_DIR is not set")
        })?);
    let profiles = manifest.join("profiles");
    println!("cargo:rerun-if-changed={}", profiles.display());

    let mut files = fs::read_dir(&profiles)?
        .map(|entry| entry.map(|value| value.path()))
        .collect::<Result<Vec<_>, _>>()?;
    files.retain(|path| is_profile_file(path));
    files.sort();

    let mut names = BTreeSet::new();
    for path in &files {
        let bytes = fs::read(path)?;
        let value = if extension(path) == Some("json") {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| {
                invalid_data(format!(
                    "invalid built-in JSON Profile {}: {error}",
                    path.display()
                ))
            })?
        } else {
            let yaml = serde_yml::from_slice::<serde_yml::Value>(&bytes).map_err(|error| {
                invalid_data(format!(
                    "invalid built-in YAML Profile {}: {error}",
                    path.display()
                ))
            })?;
            serde_json::to_value(yaml).map_err(|error| {
                invalid_data(format!(
                    "cannot normalize built-in Profile {}: {error}",
                    path.display()
                ))
            })?
        };
        let object = value.as_object().ok_or_else(|| {
            invalid_data(format!(
                "built-in Profile {} must be an object",
                path.display()
            ))
        })?;
        if object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(0)
        {
            return Err(invalid_data(format!(
                "built-in Profile {} must use schema_version 0",
                path.display()
            ))
            .into());
        }
        if object.get("kind").and_then(serde_json::Value::as_str) != Some("dataset_profile") {
            return Err(invalid_data(format!(
                "built-in Profile {} must have kind dataset_profile",
                path.display()
            ))
            .into());
        }
        let name = object
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                invalid_data(format!(
                    "built-in Profile {} has no string name",
                    path.display()
                ))
            })?;
        if !names.insert(name.to_owned()) {
            return Err(invalid_data(format!("duplicate built-in Profile name '{name}'")).into());
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }

    let generated = files
        .iter()
        .map(|path| {
            let relative = path.strip_prefix(&manifest).map_err(|error| {
                invalid_data(format!(
                    "built-in Profile {} is outside {}: {error}",
                    path.display(),
                    manifest.display()
                ))
            })?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            Ok(format!(
                "    ({relative:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{relative}\"))),\n"
            ))
        })
        .collect::<Result<String, io::Error>>()?;
    let output = format!(
        "pub(crate) const BUILT_IN_PROFILE_DOCUMENTS: &[(&str, &str)] = &[\n{generated}];\n"
    );
    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "OUT_DIR is not set"))?,
    );
    fs::write(out_dir.join("built_in_profiles.rs"), output)?;
    Ok(())
}

fn invalid_data(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn is_profile_file(path: &Path) -> bool {
    path.is_file() && matches!(extension(path), Some("yaml" | "yml" | "json"))
}

fn extension(path: &Path) -> Option<&str> {
    path.extension()?.to_str()
}
