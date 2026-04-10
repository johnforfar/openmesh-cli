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
