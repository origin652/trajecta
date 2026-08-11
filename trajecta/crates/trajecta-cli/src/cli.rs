//! # Contract: top-level command parsing
//!
//! Parsing converts operating-system arguments into typed commands without
//! opening meteorology files or executing scientific work. Machine-readable
//! and human output modes share the same command result envelope.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

use trajecta_case::document::MeteorologyReaderBackend;
use trajecta_case::model::time::Timestamp;

use crate::command::case::CaseCommand;
use crate::command::config::ConfigCommand;
use crate::command::project::ProjectCommand;
use crate::command::staged::{
    JobCommand, ProcessSelection, ResultCommand, RunInput, StagedRunCommand, TrajectorySelection,
};
use crate::command::{Command, DataCommand, DoctorCommand, MetCommand};

/// Requested output representation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OutputMode {
    /// Human-readable terminal output.
    Human,
    /// Stable machine-readable JSON envelope.
    Json,
    /// Stable machine-readable JSONL stream items.
    Jsonl,
}

type ParsedGlobalOptions = (OutputMode, Option<PathBuf>, Option<PathBuf>, Vec<OsString>);

/// Best-effort output selection used when argument parsing itself fails.
pub(crate) fn requested_output(arguments: &[OsString]) -> OutputMode {
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].to_str() {
            Some("--json") => return OutputMode::Json,
            Some("--format") => match arguments.get(index + 1).and_then(|value| value.to_str()) {
                Some("json") => return OutputMode::Json,
                Some("jsonl") => return OutputMode::Jsonl,
                _ => {}
            },
            _ => {}
        }
        index += 1;
    }
    OutputMode::Human
}

/// Best-effort command path used only for a parse-error envelope.
pub(crate) fn parse_error_command(arguments: &[OsString]) -> String {
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].to_str() {
            Some("--json") => index += 1,
            Some("--format") | Some("--config") | Some("--project") => index += 2,
            Some(root) if !root.starts_with('-') => {
                if matches!(
                    root,
                    "config" | "project" | "case" | "data" | "met" | "job" | "result"
                ) {
                    if let Some(subcommand) =
                        arguments.get(index + 1).and_then(|item| item.to_str())
                    {
                        return format!("{root} {subcommand}");
                    }
                }
                return root.into();
            }
            _ => index += 1,
        }
    }
    "cli".into()
}

/// Fully parsed top-level command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cli {
    /// Requested rendering mode.
    pub output: OutputMode,
    /// Explicit configuration path, when supplied.
    pub config_path: Option<PathBuf>,
    /// Explicit project root or index path, when supplied.
    pub project_path: Option<PathBuf>,
    /// Typed subcommand.
    pub command: Command,
}

impl Cli {
    /// Parses operating-system strings without assuming UTF-8 paths.
    pub fn parse_from(
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Result<Self, CliParseError> {
        let mut all = arguments.into_iter();
        // Skip program name when present.
        let _ = all.next();
        let (output, config_path, project_path, positional) = parse_global_options(all.collect())?;
        if positional.is_empty() {
            return Err(CliParseError::MissingArgument("command".into()));
        }
        let command_name = positional[0]
            .to_str()
            .ok_or_else(|| CliParseError::InvalidArgument("command is not valid UTF-8".into()))?;
        let command = match command_name {
            "config" => Command::Config(parse_config(&positional[1..])?),
            "project" => Command::Project(parse_project(&positional[1..])?),
            "case" => Command::Case(parse_case(&positional[1..])?),
            "data" => Command::Data(parse_data(&positional[1..])?),
            "met" => Command::Met(parse_met(&positional[1..])?),
            "doctor" => Command::Doctor(parse_doctor(&positional[1..])?),
            "run" if positional.get(1).and_then(|value| value.to_str()) == Some("report") => {
                Command::Result(parse_run_report(&positional[2..])?)
            }
            "run" => Command::StagedRun(parse_staged_run(&positional[1..], project_path.clone())?),
            "job" => Command::Job(parse_job(&positional[1..])?),
            "result" => Command::Result(parse_result(&positional[1..])?),
            other => {
                return Err(CliParseError::InvalidArgument(format!(
                    "unknown command `{other}`"
                )));
            }
        };
        if output == OutputMode::Json
            && matches!(
                &command,
                Command::Job(JobCommand::Events { follow: true, .. })
            )
        {
            return Err(CliParseError::InvalidArgument(
                "job events --follow requires --format human or jsonl".into(),
            ));
        }
        Ok(Self {
            output,
            config_path,
            project_path,
            command,
        })
    }
}

fn parse_met(args: &[OsString]) -> Result<MetCommand, CliParseError> {
    if args.is_empty() {
        return Err(CliParseError::MissingArgument(
            "met subcommand (probe|replay)".into(),
        ));
    }
    match args[0].to_string_lossy().as_ref() {
        "probe" => parse_met_probe(&args[1..]),
        "replay" => parse_met_replay(&args[1..]),
        other => Err(CliParseError::InvalidArgument(format!(
            "unknown met subcommand `{other}`"
        ))),
    }
}

fn parse_met_probe(args: &[OsString]) -> Result<MetCommand, CliParseError> {
    let mut data_root = None;
    let mut profile = None;
    let mut time_unix = None;
    let mut points = None;
    let mut backend = MeteorologyReaderBackend::Rust;
    let mut coverage_start_unix = None;
    let mut coverage_end_unix = None;
    let mut allow_estimated = false;
    let mut summary = true;
    let mut explain = false;
    let mut output = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--data-root" => {
                index += 1;
                data_root = Some(require_path(args, index, "--data-root")?);
            }
            "--profile" => {
                index += 1;
                profile = Some(require_utf8(args, index, "--profile")?);
            }
            "--time" => {
                index += 1;
                time_unix = Some(require_i64(args, index, "--time")?);
            }
            "--points" => {
                index += 1;
                points = Some(require_path(args, index, "--points")?);
            }
            "--backend" => {
                index += 1;
                backend = parse_backend(&require_utf8(args, index, "--backend")?)?;
            }
            "--coverage-start" => {
                index += 1;
                coverage_start_unix = Some(require_i64(args, index, "--coverage-start")?);
            }
            "--coverage-end" => {
                index += 1;
                coverage_end_unix = Some(require_i64(args, index, "--coverage-end")?);
            }
            "--allow-estimated" => allow_estimated = true,
            "--no-summary" => summary = false,
            "--summary" => summary = true,
            "--explain" => explain = true,
            "--output" => {
                index += 1;
                output = Some(require_path(args, index, "--output")?);
            }
            other => {
                return Err(CliParseError::InvalidArgument(format!(
                    "unknown met probe option `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(MetCommand::Probe {
        data_root: data_root.ok_or_else(|| CliParseError::MissingArgument("--data-root".into()))?,
        profile: profile.ok_or_else(|| CliParseError::MissingArgument("--profile".into()))?,
        time_unix: time_unix.ok_or_else(|| CliParseError::MissingArgument("--time".into()))?,
        points,
        backend,
        coverage_start_unix,
        coverage_end_unix,
        allow_estimated,
        summary,
        explain,
        output,
    })
}

fn parse_met_replay(args: &[OsString]) -> Result<MetCommand, CliParseError> {
    let mut data_root = None;
    let mut profile = None;
    let mut input = None;
    let mut output = Some(PathBuf::from("-"));
    let mut backend = MeteorologyReaderBackend::Rust;
    let mut coverage_start_unix = None;
    let mut coverage_end_unix = None;
    let mut allow_estimated = false;
    let mut explain = false;
    let mut summary = true;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--data-root" => {
                index += 1;
                data_root = Some(require_path(args, index, "--data-root")?);
            }
            "--profile" => {
                index += 1;
                profile = Some(require_utf8(args, index, "--profile")?);
            }
            "--input" => {
                index += 1;
                input = Some(require_path(args, index, "--input")?);
            }
            "--output" => {
                index += 1;
                output = Some(require_path(args, index, "--output")?);
            }
            "--backend" => {
                index += 1;
                backend = parse_backend(&require_utf8(args, index, "--backend")?)?;
            }
            "--coverage-start" => {
                index += 1;
                coverage_start_unix = Some(require_i64(args, index, "--coverage-start")?);
            }
            "--coverage-end" => {
                index += 1;
                coverage_end_unix = Some(require_i64(args, index, "--coverage-end")?);
            }
            "--allow-estimated" => allow_estimated = true,
            "--explain" => explain = true,
            "--no-summary" => summary = false,
            "--summary" => summary = true,
            other => {
                return Err(CliParseError::InvalidArgument(format!(
                    "unknown met replay option `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(MetCommand::Replay {
        data_root: data_root.ok_or_else(|| CliParseError::MissingArgument("--data-root".into()))?,
        profile: profile.ok_or_else(|| CliParseError::MissingArgument("--profile".into()))?,
        input: input.ok_or_else(|| CliParseError::MissingArgument("--input".into()))?,
        output: output.unwrap_or_else(|| PathBuf::from("-")),
        backend,
        coverage_start_unix,
        coverage_end_unix,
        allow_estimated,
        explain,
        summary,
    })
}

fn parse_global_options(args: Vec<OsString>) -> Result<ParsedGlobalOptions, CliParseError> {
    let mut output = None;
    let mut config_path = None;
    let mut project_path = None;
    let mut positional = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--json") => {
                if output.replace(OutputMode::Json).is_some() {
                    return Err(CliParseError::InvalidArgument(
                        "duplicate or conflicting format option".into(),
                    ));
                }
            }
            Some("--format") => {
                index += 1;
                let format = require_utf8(&args, index, "--format")?;
                let parsed = match format.as_str() {
                    "human" => OutputMode::Human,
                    "json" => OutputMode::Json,
                    "jsonl" => OutputMode::Jsonl,
                    _ => {
                        return Err(CliParseError::InvalidArgument(
                            "--format must be human, json, or jsonl".into(),
                        ));
                    }
                };
                if output.replace(parsed).is_some() {
                    return Err(CliParseError::InvalidArgument(
                        "duplicate or conflicting format option".into(),
                    ));
                }
            }
            Some("--config") => {
                index += 1;
                if config_path.is_some() {
                    return Err(CliParseError::InvalidArgument("duplicate --config".into()));
                }
                config_path = Some(require_path(&args, index, "--config")?);
            }
            Some("--project") => {
                index += 1;
                if project_path.is_some() {
                    return Err(CliParseError::InvalidArgument("duplicate --project".into()));
                }
                project_path = Some(require_path(&args, index, "--project")?);
            }
            Some("--help") | Some("-h") => return Err(CliParseError::Help),
            _ => positional.push(args[index].clone()),
        }
        index += 1;
    }
    Ok((
        output.unwrap_or(OutputMode::Human),
        config_path,
        project_path,
        positional,
    ))
}

fn parse_config(args: &[OsString]) -> Result<ConfigCommand, CliParseError> {
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(CliParseError::MissingArgument("config subcommand".into()));
    };
    match name {
        "init" if args.len() == 1 => Ok(ConfigCommand::Init),
        "path" if args.len() == 1 => Ok(ConfigCommand::Path),
        "list" if args.len() == 1 => Ok(ConfigCommand::List),
        "validate" if args.len() == 1 => Ok(ConfigCommand::Validate),
        "get" if args.len() == 2 => Ok(ConfigCommand::Get {
            key: require_utf8(args, 1, "KEY")?,
        }),
        "set" if args.len() == 3 => Ok(ConfigCommand::Set {
            key: require_utf8(args, 1, "KEY")?,
            value: require_utf8(args, 2, "VALUE")?,
        }),
        "unset" if args.len() == 2 => Ok(ConfigCommand::Unset {
            key: require_utf8(args, 1, "KEY")?,
        }),
        _ => Err(CliParseError::InvalidArgument(
            "invalid config command arguments".into(),
        )),
    }
}

fn parse_project(args: &[OsString]) -> Result<ProjectCommand, CliParseError> {
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(CliParseError::MissingArgument("project subcommand".into()));
    };
    match name {
        "init" => {
            let mut path = None;
            let mut project_name = None;
            let mut index = 1;
            while index < args.len() {
                match args[index].to_str() {
                    Some("--name") => {
                        index += 1;
                        if project_name.is_some() {
                            return Err(CliParseError::InvalidArgument("duplicate --name".into()));
                        }
                        project_name = Some(require_utf8(args, index, "--name")?);
                    }
                    Some(value) if !value.starts_with('-') && path.is_none() => {
                        path = Some(PathBuf::from(&args[index]))
                    }
                    _ => {
                        return Err(CliParseError::InvalidArgument(
                            "invalid project init argument".into(),
                        ));
                    }
                }
                index += 1;
            }
            Ok(ProjectCommand::Init {
                path,
                name: project_name
                    .ok_or_else(|| CliParseError::MissingArgument("--name".into()))?,
            })
        }
        "status" if args.len() == 1 => Ok(ProjectCommand::Status),
        "show" if args.len() == 1 => Ok(ProjectCommand::Show),
        "validate" if args.len() == 1 => Ok(ProjectCommand::Validate),
        "finalize" if args.len() == 1 => Ok(ProjectCommand::Finalize),
        "get" if args.len() == 2 => Ok(ProjectCommand::Get {
            selector: require_utf8(args, 1, "SELECTOR")?,
        }),
        "set" if args.len() == 3 => Ok(ProjectCommand::Set {
            selector: require_utf8(args, 1, "SELECTOR")?,
            value: require_utf8(args, 2, "VALUE")?,
        }),
        "unset" if args.len() == 2 => Ok(ProjectCommand::Unset {
            selector: require_utf8(args, 1, "SELECTOR")?,
        }),
        "data-plan" => match args.get(1).and_then(|value| value.to_str()) {
            None => Ok(ProjectCommand::DataPlan { output: None }),
            Some("--output") if args.len() == 3 => Ok(ProjectCommand::DataPlan {
                output: Some(require_path(args, 2, "--output")?),
            }),
            _ => Err(CliParseError::InvalidArgument(
                "invalid project data-plan arguments".into(),
            )),
        },
        _ => Err(CliParseError::InvalidArgument(
            "invalid project command arguments".into(),
        )),
    }
}

fn parse_case(args: &[OsString]) -> Result<CaseCommand, CliParseError> {
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(CliParseError::MissingArgument("case subcommand".into()));
    };
    match name {
        "resolve" if args.len() == 2 => Ok(CaseCommand::Resolve {
            path: require_path(args, 1, "PATH")?,
        }),
        "validate" => {
            if args.len() != 2 && args.len() != 4 {
                return Err(CliParseError::InvalidArgument(
                    "invalid case validate arguments".into(),
                ));
            }
            let intent = if args.len() == 2 {
                trajecta_case::intent::ValidationIntent::Simulation
            } else {
                if args[2].to_str() != Some("--intent") {
                    return Err(CliParseError::InvalidArgument("expected --intent".into()));
                }
                match require_utf8(args, 3, "--intent")?.as_str() {
                    "simulation" => trajecta_case::intent::ValidationIntent::Simulation,
                    "met-probe" => trajecta_case::intent::ValidationIntent::MetProbe,
                    "migration" => trajecta_case::intent::ValidationIntent::Migration,
                    _ => {
                        return Err(CliParseError::InvalidArgument(
                            "--intent must be simulation, met-probe, or migration".into(),
                        ));
                    }
                }
            };
            Ok(CaseCommand::Validate {
                path: require_path(args, 1, "PATH")?,
                intent,
            })
        }
        _ => Err(CliParseError::InvalidArgument(
            "invalid case command arguments".into(),
        )),
    }
}

fn parse_data(args: &[OsString]) -> Result<DataCommand, CliParseError> {
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(CliParseError::MissingArgument("data subcommand".into()));
    };
    match name {
        "inspect" if args.len() == 2 => Ok(DataCommand::Inspect {
            file: require_path(args, 1, "FILE")?,
        }),
        "lock" => {
            let mut root = None;
            let mut profile = None;
            let mut case = None;
            let mut output = None;
            let mut replace = false;
            let mut index = 1;
            while index < args.len() {
                match args[index].to_str() {
                    Some("--root") => {
                        index += 1;
                        if root.is_some() {
                            return Err(CliParseError::InvalidArgument("duplicate --root".into()));
                        }
                        root = Some(require_path(args, index, "--root")?);
                    }
                    Some("--profile") => {
                        index += 1;
                        if profile.is_some() {
                            return Err(CliParseError::InvalidArgument(
                                "duplicate --profile".into(),
                            ));
                        }
                        profile = Some(require_utf8(args, index, "--profile")?);
                    }
                    Some("--case") => {
                        index += 1;
                        if case.is_some() {
                            return Err(CliParseError::InvalidArgument("duplicate --case".into()));
                        }
                        case = Some(require_path(args, index, "--case")?);
                    }
                    Some("--output") => {
                        index += 1;
                        if output.is_some() {
                            return Err(CliParseError::InvalidArgument(
                                "duplicate --output".into(),
                            ));
                        }
                        output = Some(require_path(args, index, "--output")?);
                    }
                    Some("--replace") if !replace => replace = true,
                    Some("--replace") => {
                        return Err(CliParseError::InvalidArgument("duplicate --replace".into()));
                    }
                    _ => {
                        return Err(CliParseError::InvalidArgument(
                            "invalid data lock argument".into(),
                        ));
                    }
                }
                index += 1;
            }
            Ok(DataCommand::Lock {
                root: root.ok_or_else(|| CliParseError::MissingArgument("--root".into()))?,
                profile: profile
                    .ok_or_else(|| CliParseError::MissingArgument("--profile".into()))?,
                case: case.ok_or_else(|| CliParseError::MissingArgument("--case".into()))?,
                output: output.ok_or_else(|| CliParseError::MissingArgument("--output".into()))?,
                replace,
            })
        }
        _ => Err(CliParseError::InvalidArgument(
            "invalid data command arguments".into(),
        )),
    }
}

fn parse_doctor(args: &[OsString]) -> Result<DoctorCommand, CliParseError> {
    match args {
        [] => Ok(DoctorCommand { deep: false }),
        [value] if value.to_str() == Some("--deep") => Ok(DoctorCommand { deep: true }),
        _ => Err(CliParseError::InvalidArgument(
            "doctor accepts only --deep".into(),
        )),
    }
}

fn parse_run_report(args: &[OsString]) -> Result<ResultCommand, CliParseError> {
    let mut result = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_string_lossy().as_ref() {
            "--result" => {
                index += 1;
                result = Some(require_utf8(args, index, "--result")?);
            }
            "--output" => {
                return Err(CliParseError::InvalidArgument(
                    "run report writes fixed run-report.md inside the resolved run directory"
                        .into(),
                ));
            }
            other => {
                return Err(CliParseError::InvalidArgument(format!(
                    "unknown run report option `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(ResultCommand::Report {
        result: result.ok_or_else(|| CliParseError::MissingArgument("--result".into()))?,
    })
}

fn parse_staged_run(
    args: &[OsString],
    project_path: Option<PathBuf>,
) -> Result<StagedRunCommand, CliParseError> {
    let mut project = project_path;
    let mut profile = None;
    let mut case = None;
    let mut run_profile = None;
    let mut detach = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--project") => {
                index += 1;
                if project.is_some() {
                    return Err(CliParseError::InvalidArgument("duplicate --project".into()));
                }
                project = Some(require_path(args, index, "--project")?);
            }
            Some("--profile") => {
                index += 1;
                if profile.is_some() {
                    return Err(CliParseError::InvalidArgument("duplicate --profile".into()));
                }
                profile = Some(require_utf8(args, index, "--profile")?);
            }
            Some("--case") => {
                index += 1;
                if case.is_some() {
                    return Err(CliParseError::InvalidArgument("duplicate --case".into()));
                }
                case = Some(require_path(args, index, "--case")?);
            }
            Some("--run-profile") => {
                index += 1;
                if run_profile.is_some() {
                    return Err(CliParseError::InvalidArgument(
                        "duplicate --run-profile".into(),
                    ));
                }
                run_profile = Some(require_path(args, index, "--run-profile")?);
            }
            Some("--detach") if !detach => detach = true,
            Some("--detach") => {
                return Err(CliParseError::InvalidArgument("duplicate --detach".into()));
            }
            _ => {
                return Err(CliParseError::InvalidArgument(
                    "invalid run argument".into(),
                ));
            }
        }
        index += 1;
    }
    let input = match (project, profile, case, run_profile) {
        (Some(project), Some(profile), None, None) => RunInput::Project { project, profile },
        (None, None, Some(case), Some(run_profile)) => RunInput::Direct { case, run_profile },
        _ => return Err(CliParseError::InvalidArgument("run requires exactly (--project PATH --profile NAME) or (--case FILE --run-profile FILE)".into())),
    };
    Ok(StagedRunCommand { input, detach })
}

fn parse_job(args: &[OsString]) -> Result<JobCommand, CliParseError> {
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(CliParseError::MissingArgument("job subcommand".into()));
    };
    match name {
        "list" if args.len() == 1 => Ok(JobCommand::List),
        "prune" if args.len() == 1 => Ok(JobCommand::Prune),
        "status" if args.len() == 2 => Ok(JobCommand::Status {
            job_id: require_utf8(args, 1, "JOB_ID")?,
        }),
        "wait" if args.len() == 2 => Ok(JobCommand::Wait {
            job_id: require_utf8(args, 1, "JOB_ID")?,
        }),
        "events" => parse_job_events(&args[1..]),
        "cancel" if args.len() == 2 || args.len() == 3 => {
            let force = args.len() == 3;
            if force && args[2].to_str() != Some("--force") {
                return Err(CliParseError::InvalidArgument(
                    "job cancel accepts only --force after JOB_ID".into(),
                ));
            }
            Ok(JobCommand::Cancel {
                job_id: require_utf8(args, 1, "JOB_ID")?,
                force,
            })
        }
        "rerun" if args.len() == 2 => Ok(JobCommand::Rerun(require_utf8(args, 1, "JOB_ID")?)),
        "forget" if args.len() == 2 => Ok(JobCommand::Forget(require_utf8(args, 1, "JOB_ID")?)),
        _ => Err(CliParseError::InvalidArgument(
            "invalid job command arguments".into(),
        )),
    }
}

fn parse_job_events(args: &[OsString]) -> Result<JobCommand, CliParseError> {
    let mut job_id = None;
    let mut since = None;
    let mut follow = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].to_str() {
            Some("--since") => {
                index += 1;
                if since.is_some() {
                    return Err(CliParseError::InvalidArgument("duplicate --since".into()));
                }
                let value = require_utf8(args, index, "--since")?;
                since = Some(value.parse::<u64>().map_err(|_| {
                    CliParseError::InvalidArgument("--since must be a non-negative integer".into())
                })?);
            }
            Some("--follow") if !follow => follow = true,
            Some("--follow") => {
                return Err(CliParseError::InvalidArgument("duplicate --follow".into()));
            }
            Some(value) if !value.starts_with('-') && job_id.is_none() => {
                job_id = Some(value.to_owned());
            }
            _ => {
                return Err(CliParseError::InvalidArgument(
                    "invalid job events argument".into(),
                ));
            }
        }
        index += 1;
    }
    Ok(JobCommand::Events {
        job_id,
        since,
        follow,
    })
}

fn parse_result(args: &[OsString]) -> Result<ResultCommand, CliParseError> {
    let Some(name) = args.first().and_then(|value| value.to_str()) else {
        return Err(CliParseError::MissingArgument("result subcommand".into()));
    };
    match name {
        "inspect" if args.len() == 2 => {
            Ok(ResultCommand::Inspect(require_utf8(args, 1, "RESULT")?))
        }
        "verify" if matches!(args.len(), 2 | 3) => {
            let full = args.len() == 3 && args[2] == "--full";
            if args.len() == 3 && !full {
                return Err(CliParseError::InvalidArgument(
                    "result verify accepts only --full after RESULT".into(),
                ));
            }
            Ok(ResultCommand::Verify {
                result: require_utf8(args, 1, "RESULT")?,
                full,
            })
        }
        "trajectory" => parse_result_trajectory(&args[1..]),
        "processes" => parse_result_processes(&args[1..]),
        _ => Err(CliParseError::InvalidArgument(
            "invalid result command arguments".into(),
        )),
    }
}

fn parse_result_processes(args: &[OsString]) -> Result<ResultCommand, CliParseError> {
    let result = require_utf8(args, 0, "RESULT")?;
    let mut particle_ids = BTreeSet::new();
    let mut module_ids = BTreeSet::new();
    let mut substance_ids = BTreeSet::new();
    let mut start = None;
    let mut end = None;
    let mut events = false;
    let mut max_records = None;
    let mut index = 1;
    while index < args.len() {
        let option = require_utf8(args, index, "result processes option")?;
        match option.as_str() {
            "--particle" => {
                index += 1;
                let value = require_utf8(args, index, "--particle")?;
                let particle_id = value.parse::<u64>().map_err(|_| {
                    CliParseError::InvalidArgument(
                        "--particle must be a non-negative integer".into(),
                    )
                })?;
                if particle_id > i64::MAX as u64 {
                    return Err(CliParseError::InvalidArgument(
                        "--particle exceeds the SQLite identity range".into(),
                    ));
                }
                if !particle_ids.insert(particle_id) {
                    return Err(CliParseError::InvalidArgument(format!(
                        "duplicate --particle {particle_id}"
                    )));
                }
            }
            "--module" => {
                index += 1;
                let value = require_utf8(args, index, "--module")?;
                insert_nonempty_filter(&mut module_ids, value, "--module")?;
            }
            "--substance" => {
                index += 1;
                let value = require_utf8(args, index, "--substance")?;
                insert_nonempty_filter(&mut substance_ids, value, "--substance")?;
            }
            "--start" if start.is_none() => {
                index += 1;
                start = Some(parse_utc_filter(&require_utf8(args, index, "--start")?)?);
            }
            "--end" if end.is_none() => {
                index += 1;
                end = Some(parse_utc_filter(&require_utf8(args, index, "--end")?)?);
            }
            "--events" if !events => events = true,
            "--max-records" if max_records.is_none() => {
                index += 1;
                let value = require_utf8(args, index, "--max-records")?;
                let limit = value.parse::<u64>().map_err(|_| {
                    CliParseError::InvalidArgument(
                        "--max-records must be a positive integer".into(),
                    )
                })?;
                if limit == 0 || limit > i64::MAX as u64 {
                    return Err(CliParseError::InvalidArgument(
                        "--max-records must be within 1..=9223372036854775807".into(),
                    ));
                }
                max_records = Some(limit);
            }
            _ => {
                return Err(CliParseError::InvalidArgument(format!(
                    "invalid result processes option `{option}`"
                )));
            }
        }
        index += 1;
    }
    if start.zip(end).is_some_and(|(start, end)| start > end) {
        return Err(CliParseError::InvalidArgument(
            "--start must not be later than --end".into(),
        ));
    }
    if max_records.is_some() && !events {
        return Err(CliParseError::InvalidArgument(
            "--max-records requires --events".into(),
        ));
    }
    Ok(ResultCommand::Processes {
        result,
        selection: ProcessSelection {
            particle_ids: particle_ids.into_iter().collect(),
            module_ids: module_ids.into_iter().collect(),
            substance_ids: substance_ids.into_iter().collect(),
            start,
            end,
            events,
            max_records,
        },
    })
}

fn insert_nonempty_filter(
    values: &mut BTreeSet<String>,
    value: String,
    option: &str,
) -> Result<(), CliParseError> {
    if value.trim().is_empty() || value != value.trim() {
        return Err(CliParseError::InvalidArgument(format!(
            "{option} must be a non-empty identifier"
        )));
    }
    if !values.insert(value.clone()) {
        return Err(CliParseError::InvalidArgument(format!(
            "duplicate {option} {value}"
        )));
    }
    Ok(())
}

fn parse_utc_filter(value: &str) -> Result<Timestamp, CliParseError> {
    if let Ok(seconds) = value.parse::<i64>() {
        return Timestamp::new(seconds, 0).map_err(|error| {
            CliParseError::InvalidArgument(format!("invalid UTC timestamp: {error}"))
        });
    }
    let text = value.strip_suffix('Z').ok_or_else(|| {
        CliParseError::InvalidArgument(
            "UTC time must be Unix seconds or YYYY-MM-DDTHH:MM:SS[.nnnnnnnnn]Z".into(),
        )
    })?;
    let (date, time) = text
        .split_once('T')
        .ok_or_else(|| CliParseError::InvalidArgument("UTC time must contain `T`".into()))?;
    let mut date_parts = date.split('-');
    let year = parse_time_part(date_parts.next(), "year")?;
    let month = parse_time_part(date_parts.next(), "month")?;
    let day = parse_time_part(date_parts.next(), "day")?;
    if date_parts.next().is_some() || year < 0 {
        return Err(CliParseError::InvalidArgument("invalid UTC date".into()));
    }
    let (whole_time, fraction) = time
        .split_once('.')
        .map_or((time, None), |(whole, fraction)| (whole, Some(fraction)));
    let mut time_parts = whole_time.split(':');
    let hour = parse_time_part(time_parts.next(), "hour")?;
    let minute = parse_time_part(time_parts.next(), "minute")?;
    let second = parse_time_part(time_parts.next(), "second")?;
    if time_parts.next().is_some()
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return Err(CliParseError::InvalidArgument(
            "invalid UTC timestamp".into(),
        ));
    }
    let nanosecond = match fraction {
        None => 0,
        Some(value)
            if !value.is_empty()
                && value.len() <= 9
                && value.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            let parsed = value.parse::<u32>().map_err(|_| {
                CliParseError::InvalidArgument("invalid UTC fractional second".into())
            })?;
            parsed * 10_u32.pow(u32::try_from(9 - value.len()).unwrap_or(0))
        }
        Some(_) => {
            return Err(CliParseError::InvalidArgument(
                "UTC fractional second must contain 1 to 9 digits".into(),
            ));
        }
    };
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(hour * 3_600 + minute * 60 + second))
        .ok_or_else(|| CliParseError::InvalidArgument("UTC timestamp is out of range".into()))?;
    Timestamp::new(seconds, nanosecond)
        .map_err(|error| CliParseError::InvalidArgument(format!("invalid UTC timestamp: {error}")))
}

fn parse_time_part(value: Option<&str>, name: &str) -> Result<i64, CliParseError> {
    value
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| CliParseError::InvalidArgument(format!("invalid UTC {name}")))?
        .parse::<i64>()
        .map_err(|_| CliParseError::InvalidArgument(format!("invalid UTC {name}")))
}

const fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - if month <= 2 { 1 } else { 0 };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn parse_result_trajectory(args: &[OsString]) -> Result<ResultCommand, CliParseError> {
    let result = require_utf8(args, 0, "RESULT")?;
    let mut all = false;
    let mut particle_ids = BTreeSet::new();
    let mut index = 1;
    while index < args.len() {
        match args[index].to_str() {
            Some("--all") if !all => all = true,
            Some("--all") => {
                return Err(CliParseError::InvalidArgument("duplicate --all".into()));
            }
            Some("--particle-id") => {
                index += 1;
                let value = require_utf8(args, index, "--particle-id")?;
                let particle_id = value.parse::<u64>().map_err(|_| {
                    CliParseError::InvalidArgument(
                        "--particle-id must be a non-negative integer".into(),
                    )
                })?;
                if particle_id > i64::MAX as u64 {
                    return Err(CliParseError::InvalidArgument(
                        "--particle-id exceeds the SQLite identity range".into(),
                    ));
                }
                if !particle_ids.insert(particle_id) {
                    return Err(CliParseError::InvalidArgument(format!(
                        "duplicate --particle-id {particle_id}"
                    )));
                }
            }
            _ => {
                return Err(CliParseError::InvalidArgument(
                    "result trajectory accepts RESULT plus repeated --particle-id ID or --all"
                        .into(),
                ));
            }
        }
        index += 1;
    }
    let selection = match (all, particle_ids.is_empty()) {
        (true, true) => TrajectorySelection::All,
        (false, false) => TrajectorySelection::ParticleIds(particle_ids.into_iter().collect()),
        (true, false) => {
            return Err(CliParseError::InvalidArgument(
                "--all cannot be combined with --particle-id".into(),
            ));
        }
        (false, true) => {
            return Err(CliParseError::InvalidArgument(
                "result trajectory requires --particle-id ID or --all".into(),
            ));
        }
    };
    Ok(ResultCommand::Trajectory { result, selection })
}

fn parse_backend(value: &str) -> Result<MeteorologyReaderBackend, CliParseError> {
    match value {
        "rust" => Ok(MeteorologyReaderBackend::Rust),
        "native" => Ok(MeteorologyReaderBackend::Native),
        other => Err(CliParseError::InvalidArgument(format!(
            "unknown backend `{other}` (rust|native)"
        ))),
    }
}

fn require_path(args: &[OsString], index: usize, flag: &str) -> Result<PathBuf, CliParseError> {
    args.get(index)
        .map(PathBuf::from)
        .ok_or_else(|| CliParseError::MissingArgument(flag.into()))
}

fn require_utf8(args: &[OsString], index: usize, flag: &str) -> Result<String, CliParseError> {
    let value = args
        .get(index)
        .ok_or_else(|| CliParseError::MissingArgument(flag.into()))?;
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| CliParseError::InvalidArgument(format!("{flag} is not valid UTF-8")))
}

fn require_i64(args: &[OsString], index: usize, flag: &str) -> Result<i64, CliParseError> {
    let text = require_utf8(args, index, flag)?;
    text.parse::<i64>()
        .map_err(|_| CliParseError::InvalidArgument(format!("{flag} must be an integer")))
}

/// Top-level argument parsing failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CliParseError {
    /// Help was requested or no command was provided.
    Help,
    /// A required subcommand or option is absent.
    MissingArgument(String),
    /// An option value is invalid.
    InvalidArgument(String),
}

impl CliParseError {
    /// Stable process exit code.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Help => 0,
            Self::MissingArgument(_) | Self::InvalidArgument(_) => 2,
        }
    }
}

impl std::fmt::Display for CliParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Help => formatter.write_str(HELP_TEXT),
            Self::MissingArgument(name) => write!(formatter, "missing argument: {name}"),
            Self::InvalidArgument(message) => write!(formatter, "invalid argument: {message}"),
        }
    }
}

const HELP_TEXT: &str = "\
trajecta — Trajecta command-line interface

Usage:
  trajecta run (--project PATH --profile NAME | --case FILE --run-profile FILE) [--detach]
  trajecta COMMAND [arguments]

Commands:
  config init
  config path
  config list
  config get KEY
  config set KEY VALUE
  config unset KEY
  config validate
  project init [PATH] --name NAME
  project status
  project show
  project get SELECTOR
  project set SELECTOR VALUE
  project unset SELECTOR
  project validate
  project data-plan [--output PATH]
  project finalize
  case validate PATH [--intent simulation|met-probe|migration]
  case resolve PATH
  data inspect FILE
  data lock --root DIR --profile NAME --case FILE --output PATH [--replace]
  met probe --data-root DIR --profile NAME --time UNIX [options]
  met replay --data-root DIR --profile NAME --input JSONL [options]
  doctor [--deep]
  run (--project PATH --profile NAME | --case FILE --run-profile FILE) [--detach]
  job list
  job status JOB_ID
  job wait JOB_ID
  job events [JOB_ID] [--since SEQUENCE] [--follow]
  job cancel JOB_ID [--force]
  job rerun JOB_ID
  job forget JOB_ID
  job prune
  result inspect RESULT
  result verify RESULT [--full]
  result trajectory RESULT (--particle-id ID ... | --all)
  result processes RESULT [--particle ID ...] [--module ID ...] [--substance ID ...]
                    [--start UTC] [--end UTC] [--events [--max-records N]]
  run report --result RESULT

met probe options:
  --points PATH       JSONL points (`-` = stdin); default single Europe sample
  --backend rust|native
  --coverage-start UNIX --coverage-end UNIX
  --allow-estimated   --explain  --summary|--no-summary
  --output PATH       JSONL output (`-` = stdout)

met replay options:
  --input PATH        JSONL stream (`-` = stdin)
  --output PATH       JSONL stream (`-` = stdout)
  --backend rust|native
  --coverage-start UNIX --coverage-end UNIX
  --allow-estimated   --explain  --summary|--no-summary

Global:
  --format human|json|jsonl
  --json              alias for --format json
  --config PATH       explicit machine configuration
  --project PATH      explicit project root or index
  -h, --help          show this help
";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn parse(arguments: &[&str]) -> Result<Cli, CliParseError> {
        Cli::parse_from(arguments.iter().map(OsString::from))
    }

    #[test]
    fn job_events_cursor_follow_and_force_cancel_parse_to_typed_commands() {
        let events = parse(&[
            "trajecta",
            "--format",
            "jsonl",
            "job",
            "events",
            "018f0000-0000-7000-8000-000000000001",
            "--follow",
            "--since",
            "42",
        ])
        .unwrap();
        assert_eq!(events.output, OutputMode::Jsonl);
        assert_eq!(
            events.command,
            Command::Job(JobCommand::Events {
                job_id: Some("018f0000-0000-7000-8000-000000000001".into()),
                since: Some(42),
                follow: true,
            })
        );

        let cancel = parse(&[
            "trajecta",
            "job",
            "cancel",
            "018f0000-0000-7000-8000-000000000001",
            "--force",
        ])
        .unwrap();
        assert_eq!(
            cancel.command,
            Command::Job(JobCommand::Cancel {
                job_id: "018f0000-0000-7000-8000-000000000001".into(),
                force: true,
            })
        );
    }

    #[test]
    fn job_event_stream_options_reject_ambiguous_or_non_streaming_forms() {
        assert_eq!(
            parse(&["trajecta", "--format", "json", "job", "events", "--follow",]),
            Err(CliParseError::InvalidArgument(
                "job events --follow requires --format human or jsonl".into()
            ))
        );
        assert_eq!(
            parse(&["trajecta", "job", "events", "--since", "1", "--since", "2",]),
            Err(CliParseError::InvalidArgument("duplicate --since".into()))
        );
        assert_eq!(
            parse(&[
                "trajecta",
                "job",
                "cancel",
                "018f0000-0000-7000-8000-000000000001",
                "--now",
            ]),
            Err(CliParseError::InvalidArgument(
                "job cancel accepts only --force after JOB_ID".into()
            ))
        );
    }

    #[test]
    fn trajectory_selection_is_explicit_sorted_and_bounded() {
        let selected = parse(&[
            "trajecta",
            "result",
            "trajectory",
            "run",
            "--particle-id",
            "9",
            "--particle-id",
            "2",
        ])
        .unwrap();
        assert_eq!(
            selected.command,
            Command::Result(ResultCommand::Trajectory {
                result: "run".into(),
                selection: TrajectorySelection::ParticleIds(vec![2, 9]),
            })
        );
        assert_eq!(
            parse(&["trajecta", "result", "trajectory", "run", "--all"])
                .unwrap()
                .command,
            Command::Result(ResultCommand::Trajectory {
                result: "run".into(),
                selection: TrajectorySelection::All,
            })
        );

        for arguments in [
            &["trajecta", "result", "trajectory", "run"][..],
            &[
                "trajecta",
                "result",
                "trajectory",
                "run",
                "--all",
                "--particle-id",
                "1",
            ],
            &[
                "trajecta",
                "result",
                "trajectory",
                "run",
                "--particle-id",
                "1",
                "--particle-id",
                "1",
            ],
            &[
                "trajecta",
                "result",
                "trajectory",
                "run",
                "--particle-id",
                "9223372036854775808",
            ],
        ] {
            assert!(parse(arguments).is_err(), "{arguments:?}");
        }
    }

    #[test]
    fn process_selection_is_typed_sorted_and_directionally_bounded() {
        let parsed = parse(&[
            "trajecta",
            "--format",
            "jsonl",
            "result",
            "processes",
            "run",
            "--particle",
            "9",
            "--particle",
            "2",
            "--module",
            "water_vapor_exchange",
            "--module",
            "boundary_layer_langevin",
            "--substance",
            "water",
            "--start",
            "2009-01-01T00:00:00.125Z",
            "--end",
            "1230768001",
            "--events",
            "--max-records",
            "17",
        ])
        .unwrap();
        assert_eq!(parsed.output, OutputMode::Jsonl);
        assert_eq!(
            parsed.command,
            Command::Result(ResultCommand::Processes {
                result: "run".into(),
                selection: ProcessSelection {
                    particle_ids: vec![2, 9],
                    module_ids: vec![
                        "boundary_layer_langevin".into(),
                        "water_vapor_exchange".into(),
                    ],
                    substance_ids: vec!["water".into()],
                    start: Some(Timestamp::new(1_230_768_000, 125_000_000).unwrap()),
                    end: Some(Timestamp::new(1_230_768_001, 0).unwrap()),
                    events: true,
                    max_records: Some(17),
                },
            })
        );
    }

    #[test]
    fn process_selection_rejects_ambiguous_or_invalid_filters() {
        for arguments in [
            &["trajecta", "result", "processes"][..],
            &[
                "trajecta",
                "result",
                "processes",
                "run",
                "--particle",
                "1",
                "--particle",
                "1",
            ],
            &[
                "trajecta",
                "result",
                "processes",
                "run",
                "--module",
                "boundary_layer_langevin",
                "--module",
                "boundary_layer_langevin",
            ],
            &[
                "trajecta",
                "result",
                "processes",
                "run",
                "--start",
                "2009-01-01T00:00:01Z",
                "--end",
                "2009-01-01T00:00:00Z",
            ],
            &[
                "trajecta",
                "result",
                "processes",
                "run",
                "--start",
                "2009-02-29T00:00:00Z",
            ],
            &[
                "trajecta",
                "result",
                "processes",
                "run",
                "--max-records",
                "1",
            ],
            &[
                "trajecta",
                "result",
                "processes",
                "run",
                "--events",
                "--max-records",
                "0",
            ],
        ] {
            assert!(parse(arguments).is_err(), "{arguments:?}");
        }
    }
}
