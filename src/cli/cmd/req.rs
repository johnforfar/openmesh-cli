//! `om req ...` — async request inspection and waiting.
//!
//! Many state-changing operations (deploy, expose, rebuild) return a
//! `request_id` immediately and run the heavy work in the background. These
//! commands let humans and agents observe and block on those requests.

use crate::cli::context::require_session;
use crate::cli::error::CliResult;
use crate::cli::output::{render, OutputFormat, Renderable};
use crate::cli::wait::{wait_for_request, DEFAULT_TIMEOUT_SECS};
use crate::sdk;
use clap::Subcommand;
use serde::Serialize;
use std::io::Write;

#[derive(Subcommand, Debug)]
pub enum ReqAction {
    /// Show the current state of an async request.
    Show {
        /// The request id returned by a previous deploy/expose/rebuild call.
        id: u32,
    },
    /// Block until an async request finishes (success or error).
    Wait {
        /// The request id to wait on.
        id: u32,
        /// Maximum seconds to wait before giving up.
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS)]
        timeout: u64,
    },
    /// Show stdout/stderr from each command in a request (deploy build logs).
    Logs {
        /// The request id to fetch logs for.
        id: u32,
    },
}

pub async fn run(action: ReqAction, format: OutputFormat) -> CliResult<()> {
    let session = require_session()?;
    match action {
        ReqAction::Show { id } => {
            let input = sdk::request::RequestInfoInput::new_with_path(
                &session,
                sdk::request::RequestInfoPath { request_id: id },
            );
            let info = sdk::request::request_info(input).await?;
            let view = RequestView::from_info(id, info);
            render(&view, format)?;
        }
        ReqAction::Wait { id, timeout } => {
            let info = wait_for_request(&session, id, timeout).await?;
            let view = RequestView::from_info(id, info);
            render(&view, format)?;
        }
        ReqAction::Logs { id } => {
            // 1. Fetch the request to get command IDs
            let input = sdk::request::RequestInfoInput::new_with_path(
                &session,
                sdk::request::RequestInfoPath { request_id: id },
            );
            let info = sdk::request::request_info(input).await?;

            // 2. For each command, fetch stdout/stderr
            let mut views = Vec::new();
            for cmd in &info.commands {
                let cmd_input = sdk::request::CommandInfoInput::new_with_path(
                    &session,
                    sdk::request::CommandInfoPath {
                        request_id: id,
                        command: cmd.clone(),
                    },
                );
                match sdk::request::command_info(cmd_input).await {
                    Ok(cmd_info) => views.push(CommandLogView::from_info(cmd, &cmd_info)),
                    Err(e) => views.push(CommandLogView {
                        command_id: cmd.clone(),
                        command: String::new(),
                        stdout: String::new(),
                        stderr: format!("[error fetching logs: {}]", e),
                        exit_code: None,
                    }),
                }
            }
            let logs_view = RequestLogsView {
                request_id: id,
                commands: views,
            };
            render(&logs_view, format)?;
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct RequestView {
    pub request_id: u32,
    pub status: String,
    pub commands: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_body: Option<String>,
}

impl RequestView {
    pub fn from_info(id: u32, info: sdk::request::RequestInfo) -> Self {
        let (status, error, success_body) = match info.result {
            None => ("running".to_string(), None, None),
            Some(sdk::request::RequestIdResult::Success { body }) => {
                ("success".to_string(), None, body)
            }
            Some(sdk::request::RequestIdResult::Error { error }) => {
                ("error".to_string(), Some(error), None)
            }
        };
        Self {
            request_id: id,
            status,
            commands: info.commands,
            error,
            success_body,
        }
    }
}

impl Renderable for RequestView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Request #{}", self.request_id)?;
        writeln!(w, "  status:   {}", self.status)?;
        writeln!(w, "  commands: {}", self.commands.len())?;
        for c in &self.commands {
            writeln!(w, "    - {}", c)?;
        }
        if let Some(e) = &self.error {
            writeln!(w, "  error:    {}", e)?;
        }
        if let Some(b) = &self.success_body {
            writeln!(w, "  body:     {}", b)?;
        }
        Ok(())
    }
}

// ─── om req logs ───────────────────────────────────────────────────────────

fn output_to_string(output: &sdk::utils::Output) -> String {
    match output {
        sdk::utils::Output::UTF8 { output } => output.clone(),
        sdk::utils::Output::Bytes { output } => {
            String::from_utf8_lossy(output).to_string()
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CommandLogView {
    pub command_id: String,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<String>,
}

impl CommandLogView {
    pub fn from_info(cmd_id: &str, info: &sdk::request::CommandInfo) -> Self {
        Self {
            command_id: cmd_id.to_string(),
            command: info.command.clone(),
            stdout: output_to_string(&info.stdout),
            stderr: output_to_string(&info.stderr),
            exit_code: info.result.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RequestLogsView {
    pub request_id: u32,
    pub commands: Vec<CommandLogView>,
}

impl Renderable for RequestLogsView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "📋 Build logs for request #{}", self.request_id)?;
        writeln!(w)?;
        for (i, cmd) in self.commands.iter().enumerate() {
            writeln!(
                w,
                "─── Command {}/{} ───",
                i + 1,
                self.commands.len()
            )?;
            if !cmd.command.is_empty() {
                // Show a shortened version of the command
                let short = if cmd.command.len() > 120 {
                    format!("{}...", &cmd.command[..120])
                } else {
                    cmd.command.clone()
                };
                writeln!(w, "  cmd:  {}", short)?;
            }
            if let Some(code) = &cmd.exit_code {
                writeln!(w, "  exit: {}", code)?;
            }
            if !cmd.stdout.is_empty() {
                writeln!(w, "  stdout:")?;
                for line in cmd.stdout.lines() {
                    writeln!(w, "    {}", line)?;
                }
            }
            if !cmd.stderr.is_empty() {
                writeln!(w, "  stderr:")?;
                for line in cmd.stderr.lines() {
                    writeln!(w, "    {}", line)?;
                }
            }
            if cmd.stdout.is_empty() && cmd.stderr.is_empty() {
                writeln!(w, "  (no output)")?;
            }
            writeln!(w)?;
        }
        Ok(())
    }
}
