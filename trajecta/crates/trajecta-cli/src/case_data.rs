use std::fs;
use std::path::Path;

use serde_json::Value;
use trajecta_case::diagnostic::Diagnostic;
use trajecta_case::document::{DataRootId, MeteorologyReaderBackend};
use trajecta_case::expand::expand_case_file;
use trajecta_case::intent::{IntentValidator, ValidationIntent};
use trajecta_case::resolver::LocalRefResolver;
use trajecta_case::schema::{SchemaDocument, parse_case_json, parse_case_yaml};
use trajecta_met::io::lock_builder::{ReaderMetadataInspector, SourceMetadataInspector};
use trajecta_met::io::reader::detect_source_format;

use crate::command::case::CaseCommand;
use crate::command::data::DataCommand;
use crate::data_lock::{LockSpec, build_lock, persist_lock, requirements_from_case};

pub(crate) struct CaseDataOutcome {
    pub(crate) data: Value,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

pub(crate) struct CaseDataError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl CaseDataError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub(crate) fn execute_case(command: &CaseCommand) -> Result<CaseDataOutcome, CaseDataError> {
    match command {
        CaseCommand::Validate { path, intent } => {
            let document = parse_case(path)?;
            let mut diagnostics = document
                .validate_shape()
                .map_err(schema_error)?
                .into_sorted();
            let root = path.parent().unwrap_or_else(|| Path::new("."));
            match expand_case_file(path, &LocalRefResolver::new(root)) {
                Ok(resolved) => {
                    diagnostics.extend(IntentValidator::validate(&resolved, *intent).into_sorted())
                }
                Err(error) => {
                    diagnostics.push(Diagnostic::error("case.expand_failed", error.to_string()))
                }
            }
            diagnostics.sort();
            Ok(CaseDataOutcome {
                data: serde_json::json!({"path": path, "intent": intent}),
                diagnostics,
            })
        }
        CaseCommand::Resolve { path } => {
            let root = path.parent().unwrap_or_else(|| Path::new("."));
            let resolved = expand_case_file(path, &LocalRefResolver::new(root))
                .map_err(|error| CaseDataError::new("case.expand_failed", error.to_string()))?;
            let data = serde_json::to_value(resolved).map_err(json_error)?;
            Ok(CaseDataOutcome {
                data,
                diagnostics: Vec::new(),
            })
        }
    }
}

pub(crate) fn execute_data(command: &DataCommand) -> Result<CaseDataOutcome, CaseDataError> {
    match command {
        DataCommand::Inspect { file } => {
            let format = detect_source_format(file)
                .map_err(|error| CaseDataError::new("data.inspect_failed", format!("{error:?}")))?;
            let Some(format) = format else {
                return Err(CaseDataError::new(
                    "data.unknown_format",
                    "file is not a supported GRIB or NetCDF container",
                ));
            };
            let inspector = ReaderMetadataInspector::new(
                trajecta_case::document::MeteorologyReaderBackend::Rust,
            );
            let metadata = inspector
                .inspect(file, format)
                .map_err(|error| CaseDataError::new("data.inspect_failed", format!("{error:?}")))?;
            Ok(CaseDataOutcome {
                data: serde_json::json!({
                    "path": metadata.path,
                    "format": format,
                    "attributes": metadata.attributes,
                    "dimensions": metadata.dimensions,
                    "valid_times": metadata.valid_times,
                    "roles": metadata.roles,
                    "grid": metadata.grid,
                    "vertical": metadata.vertical,
                }),
                diagnostics: Vec::new(),
            })
        }
        DataCommand::Lock {
            root,
            profile,
            case,
            output,
            replace,
        } => {
            if output.exists() && !replace {
                return Err(CaseDataError::new(
                    "data.lock_exists",
                    "destination lock exists; pass --replace to permit replacement",
                ));
            }
            let root = fs::canonicalize(root).map_err(|error| {
                CaseDataError::new(
                    "data.root_invalid",
                    format!("resolve {}: {error}", root.display()),
                )
            })?;
            if !root.is_dir() {
                return Err(CaseDataError::new(
                    "data.root_invalid",
                    format!("data root is not a directory: {}", root.display()),
                ));
            }
            let case_root = case.parent().unwrap_or_else(|| Path::new("."));
            let resolved = expand_case_file(case, &LocalRefResolver::new(case_root))
                .map_err(|error| CaseDataError::new("data.case_invalid", error.to_string()))?;
            let intent = IntentValidator::validate(&resolved, ValidationIntent::Simulation);
            if intent.has_errors() {
                return Err(CaseDataError::new(
                    "data.case_invalid",
                    intent
                        .into_sorted()
                        .into_iter()
                        .map(|diagnostic| {
                            format!("{}: {}", diagnostic.code(), diagnostic.message())
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                ));
            }
            let requirements = requirements_from_case(&resolved)
                .map_err(|error| CaseDataError::new(error.code, error.message))?;
            let mut domains = requirements.domains.into_iter();
            let Some((dataset, domain)) = domains.next() else {
                return Err(CaseDataError::new(
                    "data.case_dataset_ambiguous",
                    "standalone data lock requires a Case with exactly one logical dataset",
                ));
            };
            if domains.next().is_some() {
                return Err(CaseDataError::new(
                    "data.case_dataset_ambiguous",
                    "standalone data lock requires a Case with exactly one logical dataset",
                ));
            }
            let spec = LockSpec {
                output: output.clone(),
                dataset: dataset.clone(),
                source: format!("local data for {}", dataset.0),
                data_roots: std::collections::BTreeMap::from([(DataRootId("met".into()), root)]),
                profile_name: profile.clone(),
                profile_sources: Vec::new(),
                backend: MeteorologyReaderBackend::Rust,
                coverage: requirements.coverage,
                capabilities: requirements.capabilities,
                domain,
            };
            let artifact =
                build_lock(&spec).map_err(|error| CaseDataError::new(error.code, error.message))?;
            persist_lock(output, &artifact, *replace)
                .map_err(|error| CaseDataError::new(error.code, error.message))?;
            Ok(CaseDataOutcome {
                data: serde_json::json!({
                    "output": output,
                    "dataset": dataset,
                    "profile": artifact.lock.profile,
                    "file_count": artifact.lock.files.len(),
                    "sha256": artifact.sha256,
                }),
                diagnostics: artifact.notes,
            })
        }
    }
}

fn parse_case(path: &Path) -> Result<trajecta_case::document::CaseDocument, CaseDataError> {
    let text = fs::read_to_string(path)
        .map_err(|error| CaseDataError::new("case.read_failed", error.to_string()))?;
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        parse_case_json(&text).map_err(schema_error)
    } else {
        parse_case_yaml(&text).map_err(schema_error)
    }
}

fn schema_error(error: impl std::fmt::Display) -> CaseDataError {
    CaseDataError::new("case.invalid_document", error.to_string())
}
fn json_error(error: serde_json::Error) -> CaseDataError {
    CaseDataError::new("case.serialize_failed", error.to_string())
}
