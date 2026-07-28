//! # Contract: top-level command parsing
//!
//! Parsing converts operating-system arguments into typed commands without
//! opening meteorology files or executing scientific work. Machine-readable
//! and human output modes share the same command result envelope.

use std::ffi::OsString;
use std::path::PathBuf;

use trajecta_case::document::MeteorologyReaderBackend;

use crate::command::case::CaseCommand;
use crate::command::config::ConfigCommand;
use crate::command::project::ProjectCommand;
use crate::command::staged::{JobCommand, ResultCommand, RunInput, StagedRunCommand};
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
            return Err(CliParseError::Help);
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
    if args.len() != 2 {
        return Err(CliParseError::InvalidArgument(
            "result command requires RESULT".into(),
        ));
    }
    let value = require_utf8(args, 1, "RESULT")?;
    match name {
        "inspect" => Ok(ResultCommand::Inspect(value)),
        "verify" => Ok(ResultCommand::Verify(value)),
        "trajectory" => Ok(ResultCommand::Trajectory(value)),
        _ => Err(CliParseError::InvalidArgument(
            "invalid result command".into(),
        )),
    }
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
            Self::Help | Self::MissingArgument(_) | Self::InvalidArgument(_) => 2,
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
  trajecta job list|status|wait|events|cancel ...
  trajecta met probe --data-root DIR --profile NAME --time UNIX [options]
  trajecta met replay --data-root DIR --profile NAME --input JSONL [options]

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
}
