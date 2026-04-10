//! `om app ...` — application lifecycle management.
//!
//! "App" in this CLI means a NixOS container managed via the Xnode Manager
//! `/config/container/...` API surface, plus reverse-proxy rules added to
//! the host flake's user-config region for subdomain exposure.
//!
//! Subcommands:
//!   - `om app list`                — show deployed containers
//!   - `om app info <name>`         — show one container's flake config
//!   - `om app deploy <name> --flake <uri>` — create or update a container
//!   - `om app remove <name>`       — delete a container
//!   - `om app expose <name> --domain <fqdn> --port <n>` — add subdomain rule
//!   - `om app unexpose --domain <fqdn>` — remove subdomain rule

use crate::cli::context::require_session;
use crate::cli::error::{CliError, CliResult};
use crate::cli::flake_editor::{
    add_or_replace_expose, remove_expose, AddRuleMode, DomainExpose, ProxyRule,
};
use crate::cli::output::{render, OutputFormat, Renderable};
use crate::cli::wait::{wait_for_request, DEFAULT_TIMEOUT_SECS};
use crate::sdk;
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use std::io::Write;

#[derive(Subcommand, Debug)]
pub enum AppAction {
    /// List all containers deployed on the xnode.
    List,

    /// Show the flake configuration of one container.
    Info {
        /// The container name.
        name: String,
    },

    /// Deploy or update an application container.
    ///
    /// This is "apply" semantics: if a container with the same name exists,
    /// its configuration is updated; otherwise it is created.
    Deploy {
        /// Container name (alphanumeric + hyphens, used as the systemd unit
        /// name and the reverse-proxy forward target).
        name: String,

        /// Flake URI to build the container from.
        ///
        /// Examples:
        ///   github:Openmesh-Network/xnode-apps?dir=jellyfin
        ///   github:Openmesh-Network/xnode-nextjs-template
        #[arg(long)]
        flake: String,

        /// Optional flake inputs to update before building (passes through
        /// to `nix flake update --update-input`).
        #[arg(long)]
        update_input: Vec<String>,

        /// Block until the deploy finishes (default: true).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        wait: bool,

        /// Maximum seconds to wait when --wait is set.
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS)]
        timeout: u64,

        /// Show what would be sent without applying.
        #[arg(long)]
        dry_run: bool,
    },

    /// Delete a container from the xnode.
    Remove {
        /// The container name.
        name: String,

        /// Block until the removal finishes (default: true).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        wait: bool,

        /// Maximum seconds to wait when --wait is set.
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS)]
        timeout: u64,
    },

    /// Expose a container on a public subdomain via the host reverse proxy.
    ///
    /// This adds a `services.xnode-reverse-proxy.rules.<domain>` block to
    /// the host flake's user-config region and triggers a host rebuild.
    /// Other rules (other domains) are preserved.
    Expose {
        /// Container name to forward to (becomes the host portion of the
        /// `forward = "..."` URL).
        name: String,

        /// FQDN to expose, e.g. `demo.build.openmesh.cloud`.
        #[arg(long)]
        domain: String,

        /// Port the container is listening on.
        #[arg(long)]
        port: u16,

        /// Forward protocol scheme.
        #[arg(long, value_enum, default_value_t = ForwardProto::Http)]
        protocol: ForwardProto,

        /// Optional URL path prefix this rule applies to.
        #[arg(long)]
        path: Option<String>,

        /// If a rule already exists for the domain, replace it instead of
        /// erroring out.
        #[arg(long)]
        replace: bool,

        /// Block until the host rebuild finishes (default: true).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        wait: bool,

        /// Maximum seconds to wait when --wait is set.
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS)]
        timeout: u64,

        /// Show the flake diff without applying.
        #[arg(long)]
        dry_run: bool,
    },

    /// Remove a reverse-proxy rule for a subdomain.
    Unexpose {
        /// FQDN to remove, e.g. `demo.build.openmesh.cloud`.
        #[arg(long)]
        domain: String,

        /// Block until the host rebuild finishes (default: true).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        wait: bool,

        /// Maximum seconds to wait when --wait is set.
        #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS)]
        timeout: u64,

        /// Show the flake diff without applying.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardProto {
    Http,
    Https,
    Tcp,
    Udp,
}

impl ForwardProto {
    fn as_str(&self) -> &'static str {
        match self {
            ForwardProto::Http => "http",
            ForwardProto::Https => "https",
            ForwardProto::Tcp => "tcp",
            ForwardProto::Udp => "udp",
        }
    }
}

pub async fn run(action: AppAction, format: OutputFormat) -> CliResult<()> {
    let session = require_session()?;
    match action {
        AppAction::List => list(&session, format).await,
        AppAction::Info { name } => info(&session, name, format).await,
        AppAction::Deploy {
            name,
            flake,
            update_input,
            wait,
            timeout,
            dry_run,
        } => deploy(&session, name, flake, update_input, wait, timeout, dry_run, format).await,
        AppAction::Remove { name, wait, timeout } => {
            remove(&session, name, wait, timeout, format).await
        }
        AppAction::Expose {
            name,
            domain,
            port,
            protocol,
            path,
            replace,
            wait,
            timeout,
            dry_run,
        } => {
            expose(
                &session, name, domain, port, protocol, path, replace, wait, timeout, dry_run, format,
            )
            .await
        }
        AppAction::Unexpose {
            domain,
            wait,
            timeout,
            dry_run,
        } => unexpose(&session, domain, wait, timeout, dry_run, format).await,
    }
}

// =============================================================================
// list / info
// =============================================================================

async fn list(session: &sdk::utils::Session, format: OutputFormat) -> CliResult<()> {
    let input = sdk::config::ContainersInput::new(session);
    let containers = sdk::config::containers(input).await?;
    let view = AppListView { containers };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct AppListView {
    containers: Vec<String>,
}

impl Renderable for AppListView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.containers.is_empty() {
            writeln!(w, "(no apps deployed)")?;
        } else {
            writeln!(w, "{} app(s) deployed:", self.containers.len())?;
            for c in &self.containers {
                writeln!(w, "  - {}", c)?;
            }
        }
        Ok(())
    }
}

async fn info(
    session: &sdk::utils::Session,
    name: String,
    format: OutputFormat,
) -> CliResult<()> {
    let input = sdk::config::GetInput::new_with_path(
        session,
        sdk::config::GetPath { container: name.clone() },
    );
    let cfg = sdk::config::get(input).await?;
    let view = AppInfoView { name, config: cfg };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct AppInfoView {
    name: String,
    config: sdk::config::ContainerConfiguration,
}

impl Renderable for AppInfoView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "App: {}", self.name)?;
        writeln!(w, "  flake:   {}", self.config.flake)?;
        if let Some(network) = &self.config.network {
            writeln!(w, "  network: {}", network)?;
        }
        if let Some(gpus) = &self.config.nvidia_gpus {
            writeln!(w, "  gpus:    {:?}", gpus)?;
        }
        Ok(())
    }
}

// =============================================================================
// deploy / remove
// =============================================================================

#[allow(clippy::too_many_arguments)]
async fn deploy(
    session: &sdk::utils::Session,
    name: String,
    flake: String,
    update_input: Vec<String>,
    wait: bool,
    timeout: u64,
    dry_run: bool,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;

    let change = sdk::config::ContainerChange {
        settings: sdk::config::ContainerSettings {
            flake: flake.clone(),
            network: None,
            nvidia_gpus: None,
        },
        update_inputs: if update_input.is_empty() { None } else { Some(update_input) },
    };

    if dry_run {
        let view = DryRunView {
            action: "deploy".into(),
            target: name,
            payload: serde_json::to_value(&change)?,
        };
        render(&view, format)?;
        return Ok(());
    }

    crate::status!("deploying app `{}` from {}", name, flake);

    let input = sdk::config::SetInput {
        session,
        path: sdk::config::SetPath { container: name.clone() },
        data: change,
    };
    let resp = sdk::config::set(input).await?;
    let request_id = resp.request_id;
    crate::status!("submitted (request_id={})", request_id);

    if wait {
        let info = wait_for_request(session, request_id, timeout).await?;
        let view = DeployResultView {
            app: name,
            request_id,
            status: status_of(&info),
        };
        render(&view, format)?;
    } else {
        let view = DeployResultView {
            app: name,
            request_id,
            status: "submitted".into(),
        };
        render(&view, format)?;
    }
    Ok(())
}

async fn remove(
    session: &sdk::utils::Session,
    name: String,
    wait: bool,
    timeout: u64,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;
    crate::status!("removing app `{}`", name);

    let input = sdk::config::RemoveInput::new_with_path(
        session,
        sdk::config::RemovePath { container: name.clone() },
    );
    let resp = sdk::config::remove(input).await?;
    let request_id = resp.request_id;
    crate::status!("submitted (request_id={})", request_id);

    if wait {
        let info = wait_for_request(session, request_id, timeout).await?;
        let view = DeployResultView {
            app: name,
            request_id,
            status: status_of(&info),
        };
        render(&view, format)?;
    } else {
        let view = DeployResultView {
            app: name,
            request_id,
            status: "submitted".into(),
        };
        render(&view, format)?;
    }
    Ok(())
}

#[derive(Serialize)]
struct DeployResultView {
    app: String,
    request_id: u32,
    status: String,
}

impl Renderable for DeployResultView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "App:        {}", self.app)?;
        writeln!(w, "Request:    #{}", self.request_id)?;
        writeln!(w, "Status:     {}", self.status)?;
        Ok(())
    }
}

#[derive(Serialize)]
struct DryRunView {
    action: String,
    target: String,
    payload: serde_json::Value,
}

impl Renderable for DryRunView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "DRY RUN: would {} `{}`", self.action, self.target)?;
        writeln!(w, "Payload:")?;
        writeln!(w, "{}", serde_json::to_string_pretty(&self.payload).unwrap_or_default())?;
        Ok(())
    }
}

// =============================================================================
// expose / unexpose
// =============================================================================

#[allow(clippy::too_many_arguments)]
async fn expose(
    session: &sdk::utils::Session,
    name: String,
    domain: String,
    port: u16,
    protocol: ForwardProto,
    path: Option<String>,
    replace: bool,
    wait: bool,
    timeout: u64,
    dry_run: bool,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;

    // Fetch the current host flake.
    let os = sdk::os::get(sdk::os::GetInput::new(session)).await?;
    let current_flake = os.flake.clone();

    // Build the new expose rule.
    let forward = format!("{}://{}:{}", protocol.as_str(), name, port);
    let expose_rule = DomainExpose {
        domain: domain.clone(),
        rules: vec![ProxyRule { forward: forward.clone(), path: path.clone() }],
    };

    let mode = if replace { AddRuleMode::Replace } else { AddRuleMode::FailIfExists };
    let new_flake = add_or_replace_expose(&current_flake, expose_rule, mode).map_err(|e| {
        CliError::unsafe_flake_edit(format!("flake edit refused: {}", e))
    })?;

    if dry_run {
        let view = ExposeDryRunView {
            domain,
            forward,
            path,
            current_flake_size: current_flake.len(),
            new_flake_size: new_flake.len(),
            diff_summary: format!(
                "+1 reverse-proxy rule for `{}` ({} bytes)",
                name,
                new_flake.len() as i64 - current_flake.len() as i64
            ),
        };
        render(&view, format)?;
        return Ok(());
    }

    crate::status!("exposing `{}` at {} → {}", name, domain, forward);

    let change = sdk::os::OSChange {
        flake: Some(new_flake),
        update_inputs: None,
        xnode_owner: None,
        domain: None,
        acme_email: None,
        user_passwd: None,
    };
    let input = sdk::os::SetInput {
        session,
        path: sdk::utils::Empty::default(),
        data: change,
    };
    let resp = sdk::os::set(input).await?;
    let request_id = resp.request_id;
    crate::status!("submitted (request_id={})", request_id);

    let status = if wait {
        let info = wait_for_request(session, request_id, timeout).await?;
        status_of(&info)
    } else {
        "submitted".into()
    };

    let view = ExposeResultView {
        domain,
        app: name,
        forward,
        request_id,
        status,
    };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct ExposeResultView {
    domain: String,
    app: String,
    forward: String,
    request_id: u32,
    status: String,
}

impl Renderable for ExposeResultView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Exposed:    {}", self.domain)?;
        writeln!(w, "  → {}", self.forward)?;
        writeln!(w, "  app:      {}", self.app)?;
        writeln!(w, "  request:  #{}", self.request_id)?;
        writeln!(w, "  status:   {}", self.status)?;
        writeln!(w, "Try: curl https://{}", self.domain)?;
        Ok(())
    }
}

#[derive(Serialize)]
struct ExposeDryRunView {
    domain: String,
    forward: String,
    path: Option<String>,
    current_flake_size: usize,
    new_flake_size: usize,
    diff_summary: String,
}

impl Renderable for ExposeDryRunView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "DRY RUN: would expose")?;
        writeln!(w, "  domain:  {}", self.domain)?;
        writeln!(w, "  forward: {}", self.forward)?;
        if let Some(p) = &self.path {
            writeln!(w, "  path:    {}", p)?;
        }
        writeln!(w, "  flake:   {} bytes -> {} bytes", self.current_flake_size, self.new_flake_size)?;
        writeln!(w, "  diff:    {}", self.diff_summary)?;
        Ok(())
    }
}

async fn unexpose(
    session: &sdk::utils::Session,
    domain: String,
    wait: bool,
    timeout: u64,
    dry_run: bool,
    format: OutputFormat,
) -> CliResult<()> {
    let os = sdk::os::get(sdk::os::GetInput::new(session)).await?;
    let new_flake = remove_expose(&os.flake, &domain).map_err(|e| {
        CliError::unsafe_flake_edit(format!("flake edit refused: {}", e))
    })?;

    if dry_run {
        let view = ExposeDryRunView {
            domain: domain.clone(),
            forward: "(removed)".into(),
            path: None,
            current_flake_size: os.flake.len(),
            new_flake_size: new_flake.len(),
            diff_summary: format!("-1 reverse-proxy rule for `{}`", domain),
        };
        render(&view, format)?;
        return Ok(());
    }

    crate::status!("removing expose rule for `{}`", domain);

    let change = sdk::os::OSChange {
        flake: Some(new_flake),
        update_inputs: None,
        xnode_owner: None,
        domain: None,
        acme_email: None,
        user_passwd: None,
    };
    let input = sdk::os::SetInput {
        session,
        path: sdk::utils::Empty::default(),
        data: change,
    };
    let resp = sdk::os::set(input).await?;
    let request_id = resp.request_id;
    crate::status!("submitted (request_id={})", request_id);

    let status = if wait {
        let info = wait_for_request(session, request_id, timeout).await?;
        status_of(&info)
    } else {
        "submitted".into()
    };

    let view = UnexposeResultView { domain, request_id, status };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct UnexposeResultView {
    domain: String,
    request_id: u32,
    status: String,
}

impl Renderable for UnexposeResultView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Unexposed:  {}", self.domain)?;
        writeln!(w, "  request:  #{}", self.request_id)?;
        writeln!(w, "  status:   {}", self.status)?;
        Ok(())
    }
}

// =============================================================================
// helpers
// =============================================================================

fn validate_container_name(name: &str) -> CliResult<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(CliError::invalid_input(
            "container name must be 1-64 characters",
        ));
    }
    for c in name.chars() {
        if !(c.is_ascii_alphanumeric() || c == '-') {
            return Err(CliError::invalid_input(format!(
                "container name `{}` contains invalid character `{}`; allowed: a-z 0-9 -",
                name, c
            )));
        }
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(CliError::invalid_input(
            "container name must not start or end with `-`",
        ));
    }
    Ok(())
}

fn status_of(info: &sdk::request::RequestInfo) -> String {
    match &info.result {
        None => "running".into(),
        Some(sdk::request::RequestIdResult::Success { .. }) => "success".into(),
        Some(sdk::request::RequestIdResult::Error { error }) => format!("error: {}", error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_container_name_accepts_simple() {
        assert!(validate_container_name("jellyfin").is_ok());
        assert!(validate_container_name("my-app").is_ok());
        assert!(validate_container_name("a").is_ok());
    }

    #[test]
    fn validate_container_name_rejects_unsafe() {
        assert!(validate_container_name("").is_err());
        assert!(validate_container_name("my app").is_err()); // space
        assert!(validate_container_name("my_app").is_err()); // underscore not allowed
        assert!(validate_container_name("-leading").is_err());
        assert!(validate_container_name("trailing-").is_err());
        assert!(validate_container_name("evil; rm -rf /").is_err());
    }

    #[test]
    fn forward_proto_strings() {
        assert_eq!(ForwardProto::Http.as_str(), "http");
        assert_eq!(ForwardProto::Https.as_str(), "https");
        assert_eq!(ForwardProto::Tcp.as_str(), "tcp");
        assert_eq!(ForwardProto::Udp.as_str(), "udp");
    }
}
