use serde::Serialize;
use serde_json::Value;
use trajecta_case::diagnostic::Diagnostic;

use crate::cli::OutputMode;
use crate::command::Command;
use crate::command_result::{CommandError, CommandOutcome};
use crate::configuration;

#[derive(Clone, Debug)]
pub(crate) struct AppOutcome {
    pub(crate) command: String,
    pub(crate) data: Value,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) run_success: Option<bool>,
    pub(crate) exit_code: i32,
}

impl AppOutcome {
    pub(crate) fn ok(command: impl Into<String>, data: Value) -> Self {
        Self {
            command: command.into(),
            data,
            diagnostics: Vec::new(),
            run_success: None,
            exit_code: 0,
        }
    }

    pub(crate) fn error(
        command: impl Into<String>,
        code: &str,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            command: command.into(),
            data: Value::Null,
            diagnostics: vec![Diagnostic::error(code, message).with_hint(hint)],
            run_success: None,
            exit_code: 1,
        }
    }

    pub(crate) fn with_exit_code(mut self, exit_code: i32) -> Self {
        self.exit_code = exit_code;
        self
    }

    pub(crate) fn with_run_success(mut self, run_success: bool) -> Self {
        self.run_success = Some(run_success);
        self
    }

    pub(crate) fn is_ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity() == trajecta_case::diagnostic::Severity::Error)
    }
}

pub(crate) fn execute(
    command: &Command,
    config_path: Option<&std::path::Path>,
    project_path: Option<&std::path::Path>,
) -> AppOutcome {
    match command {
        Command::Config(config) => command_outcome(
            command,
            configuration::execute(config, config_path).map(CommandOutcome::ok),
            "inspect configuration path and schema",
        ),
        Command::Project(project_command) => command_outcome(
            command,
            crate::project::execute(project_command, project_path),
            "inspect project index and document paths",
        ),
        Command::Case(case_command) => command_outcome(
            command,
            crate::case_data::execute_case(case_command),
            "inspect Case document and references",
        ),
        Command::Data(data_command) => command_outcome(
            command,
            crate::case_data::execute_data(data_command),
            "inspect data input and lock prerequisites",
        ),
        Command::Doctor(doctor) => command_outcome(
            command,
            crate::project::doctor(project_path, config_path, doctor.deep),
            "inspect local filesystem readiness",
        ),
        Command::Met(_) => AppOutcome::error(
            command_name(command),
            "command.stage_not_available",
            "met machine envelope is being connected in M5-A1",
            "requires M5-A1 met envelope implementation",
        ),
        Command::Run(_) | Command::StagedRun(_) | Command::Job(_) | Command::Result(_) => {
            AppOutcome::error(
                command_name(command),
                "command.stage_not_available",
                "command is not available in this product stage",
                "requires M5-A2 local daemon",
            )
        }
    }
}

fn command_outcome(
    command: &Command,
    result: Result<CommandOutcome, CommandError>,
    hint: &str,
) -> AppOutcome {
    match result {
        Ok(outcome) => AppOutcome {
            command: command_name(command).into(),
            data: outcome.data,
            diagnostics: outcome.diagnostics,
            run_success: None,
            exit_code: 0,
        },
        Err(error) => AppOutcome::error(command_name(command), error.code, error.message, hint),
    }
}

pub(crate) fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Config(crate::command::config::ConfigCommand::Init) => "config init",
        Command::Config(crate::command::config::ConfigCommand::Path) => "config path",
        Command::Config(crate::command::config::ConfigCommand::List) => "config list",
        Command::Config(crate::command::config::ConfigCommand::Get { .. }) => "config get",
        Command::Config(crate::command::config::ConfigCommand::Set { .. }) => "config set",
        Command::Config(crate::command::config::ConfigCommand::Unset { .. }) => "config unset",
        Command::Config(crate::command::config::ConfigCommand::Validate) => "config validate",
        Command::Project(crate::command::project::ProjectCommand::Init { .. }) => "project init",
        Command::Project(crate::command::project::ProjectCommand::Status) => "project status",
        Command::Project(crate::command::project::ProjectCommand::Show) => "project show",
        Command::Project(crate::command::project::ProjectCommand::Get { .. }) => "project get",
        Command::Project(crate::command::project::ProjectCommand::Set { .. }) => "project set",
        Command::Project(crate::command::project::ProjectCommand::Unset { .. }) => "project unset",
        Command::Project(crate::command::project::ProjectCommand::Validate) => "project validate",
        Command::Project(crate::command::project::ProjectCommand::DataPlan { .. }) => {
            "project data-plan"
        }
        Command::Project(crate::command::project::ProjectCommand::Finalize) => "project finalize",
        Command::Case(crate::command::case::CaseCommand::Validate { .. }) => "case validate",
        Command::Case(crate::command::case::CaseCommand::Resolve { .. }) => "case resolve",
        Command::Data(crate::command::data::DataCommand::Inspect { .. }) => "data inspect",
        Command::Data(crate::command::data::DataCommand::Lock { .. }) => "data lock",
        Command::Met(crate::command::met::MetCommand::Probe { .. }) => "met probe",
        Command::Met(crate::command::met::MetCommand::Replay { .. }) => "met replay",
        Command::Doctor(_) => "doctor",
        Command::Run(_) | Command::StagedRun(_) => "run",
        Command::Job(crate::command::staged::JobCommand::List) => "job list",
        Command::Job(crate::command::staged::JobCommand::Status { .. }) => "job status",
        Command::Job(crate::command::staged::JobCommand::Wait { .. }) => "job wait",
        Command::Job(crate::command::staged::JobCommand::Events { .. }) => "job events",
        Command::Job(crate::command::staged::JobCommand::Cancel { .. }) => "job cancel",
        Command::Job(crate::command::staged::JobCommand::Rerun(_)) => "job rerun",
        Command::Job(crate::command::staged::JobCommand::Forget(_)) => "job forget",
        Command::Job(crate::command::staged::JobCommand::Prune) => "job prune",
        Command::Result(_) => "result",
    }
}

pub(crate) fn render(
    outcome: &AppOutcome,
    output: OutputMode,
) -> Result<String, serde_json::Error> {
    match output {
        OutputMode::Human => Ok(render_human(outcome)),
        OutputMode::Json => serde_json::to_string(&OutputEnvelope::from(outcome)),
        OutputMode::Jsonl => render_jsonl(outcome),
    }
}

fn render_human(outcome: &AppOutcome) -> String {
    let mut lines = Vec::new();
    if !outcome.data.is_null() {
        lines.push(serde_json::to_string_pretty(&outcome.data).unwrap_or_else(|_| "null".into()));
    }
    for diagnostic in &outcome.diagnostics {
        lines.push(format!("{}: {}", diagnostic.code(), diagnostic.message()));
        if let Some(hint) = diagnostic.hint() {
            lines.push(format!("hint: {hint}"));
        }
    }
    if lines.is_empty() {
        "ok".into()
    } else {
        lines.join("\n")
    }
}

fn render_jsonl(outcome: &AppOutcome) -> Result<String, serde_json::Error> {
    let mut sequence = 1_u64;
    let mut lines = Vec::new();
    if !outcome.data.is_null() {
        lines.push(serde_json::to_string(&StreamItem::data(
            &outcome.command,
            sequence,
            outcome.data.clone(),
        ))?);
        sequence += 1;
    }
    for diagnostic in &outcome.diagnostics {
        lines.push(serde_json::to_string(&StreamItem::diagnostic(
            &outcome.command,
            sequence,
            diagnostic,
        ))?);
        sequence += 1;
    }
    lines.push(serde_json::to_string(&StreamItem::summary(
        &outcome.command,
        sequence,
        outcome.is_ok(),
        outcome.run_success,
    ))?);
    Ok(lines.join("\n"))
}

pub(crate) fn render_stream_data(
    command: &str,
    sequence: u64,
    data: Value,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&StreamItem::data(command, sequence, data))
}

pub(crate) fn render_stream_diagnostic(
    command: &str,
    sequence: u64,
    diagnostic: &Diagnostic,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&StreamItem::diagnostic(command, sequence, diagnostic))
}

pub(crate) fn render_stream_summary(
    command: &str,
    sequence: u64,
    ok: bool,
    run_success: Option<bool>,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&StreamItem::summary(command, sequence, ok, run_success))
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct OutputEnvelope<'a> {
    schema_version: &'static str,
    command: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_success: Option<bool>,
    data: &'a Value,
    diagnostics: &'a [Diagnostic],
}

impl<'a> From<&'a AppOutcome> for OutputEnvelope<'a> {
    fn from(value: &'a AppOutcome) -> Self {
        Self {
            schema_version: "trajecta.cli-output/v1",
            command: &value.command,
            ok: value.is_ok(),
            run_success: value.run_success,
            data: &value.data,
            diagnostics: &value.diagnostics,
        }
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct StreamItem<'a> {
    schema_version: &'static str,
    command: &'a str,
    sequence: u64,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic: Option<&'a Diagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<StreamSummary>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct StreamSummary {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_success: Option<bool>,
}

impl<'a> StreamItem<'a> {
    fn data(command: &'a str, sequence: u64, data: Value) -> Self {
        Self {
            schema_version: "trajecta.cli-stream-item/v1",
            command,
            sequence,
            kind: "data",
            data: Some(data),
            diagnostic: None,
            summary: None,
        }
    }
    fn diagnostic(command: &'a str, sequence: u64, diagnostic: &'a Diagnostic) -> Self {
        Self {
            schema_version: "trajecta.cli-stream-item/v1",
            command,
            sequence,
            kind: "diagnostic",
            data: None,
            diagnostic: Some(diagnostic),
            summary: None,
        }
    }
    fn summary(command: &'a str, sequence: u64, ok: bool, run_success: Option<bool>) -> Self {
        Self {
            schema_version: "trajecta.cli-stream-item/v1",
            command,
            sequence,
            kind: "summary",
            data: None,
            diagnostic: None,
            summary: Some(StreamSummary { ok, run_success }),
        }
    }
}
