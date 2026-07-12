//! Workspace-level source-contract audit.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[test]
fn every_public_module_has_an_authoritative_contract_file() -> Result<(), Box<dyn Error>> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crate_roots = [
        workspace.join("crates/trajecta-case/src/lib.rs"),
        workspace.join("crates/trajecta-met/src/lib.rs"),
        workspace.join("crates/trajecta-core/src/lib.rs"),
        workspace.join("crates/trajecta-cli/src/lib.rs"),
    ];

    let mut visited = BTreeSet::new();
    for root in crate_roots {
        audit_module_declarations(&root, &mut visited)?;
    }
    assert_eq!(
        visited.len(),
        72,
        "the v0 public module inventory changed; review and update its contracts"
    );
    Ok(())
}

fn audit_module_declarations(
    source_file: &Path,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let source_file = source_file.canonicalize()?;
    if !visited.insert(source_file.clone()) {
        return Ok(());
    }

    let source = fs::read_to_string(&source_file)?;
    assert!(
        source.starts_with("//! # Contract:"),
        "public module contract must start with `//! # Contract:`: {}",
        source_file.display()
    );
    let source_directory = source_file.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("source file has no parent: {}", source_file.display()),
        )
    })?;

    for line in source.lines().map(str::trim) {
        let Some(module_name) = line
            .strip_prefix("pub mod ")
            .and_then(|value| value.strip_suffix(';'))
        else {
            continue;
        };

        let flat_file = source_directory.join(format!("{module_name}.rs"));
        let directory_file = source_directory.join(module_name).join("mod.rs");
        let contract_file = if flat_file.is_file() {
            flat_file
        } else if directory_file.is_file() {
            directory_file
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "public module {module_name} declared by {} has no contract file",
                    source_file.display()
                ),
            )
            .into());
        };

        audit_module_declarations(&contract_file, visited)?;
    }
    Ok(())
}
