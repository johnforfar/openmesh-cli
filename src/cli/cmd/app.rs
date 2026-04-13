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

/// Derive a sane nix flake input name from a flake URI.
///
/// Examples:
///   github:johnforfar/openmesh-support-agent          → "openmesh-support-agent"
///   github:Openmesh-Network/xnode-apps?dir=jellyfin   → "jellyfin"
///   github:user/my.repo                                → "my-repo"
///
/// Nix flake input names must be valid attribute names. We force lowercase,
/// allow [a-z0-9_-], and replace anything else with `-`.
fn derive_input_name(uri: &str) -> String {
    // Prefer the `?dir=...` segment if present (xnode-apps style)
    let raw = if let Some(idx) = uri.find("?dir=") {
        let after = &uri[idx + 5..];
        after.split('&').next().unwrap_or(after).rsplit('/').next().unwrap_or(after)
    } else {
        // Otherwise: the last `/`-separated segment
        let stripped = uri
            .rsplit('/')
            .next()
            .unwrap_or(uri);
        // Drop ?ref=... suffix if present
        stripped.split('?').next().unwrap_or(stripped)
    };
    let cleaned: String = raw
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() { "app".to_string() } else { trimmed }
}

/// Wrap a flake URI into a full flake.nix expression that the xnode-manager
/// can build directly.
///
/// The xnode-manager's `config.set` API takes the `flake` field as the
/// **full text** of `flake.nix`, not as a flake URI. The backend at
/// `xnode-manager/rust-app/src/config/handlers.rs:158` writes the string
/// verbatim into `/var/lib/xnode-manager/containers/<name>/flake.nix`.
///
/// The wrapper:
///   1. declares `xnode-manager` as a flake input (mandatory — provides
///      `nixosModules.container`)
///   2. declares the user's flake as an input under a generated name
///   3. **follows xnode-manager's pinned nixpkgs** — see Lesson #2.
///   4. **enables DHCP** so the container registers with host dnsmasq
///      and `<name>.container` DNS resolves — see Lesson #6. Without
///      this, every exposed URL 502s.
///   5. composes them into `nixosConfigurations.container`
///
/// The user's flake MUST export `nixosModules.default`, matching the
/// `xnode-apps/*` convention.
///
/// See `openxai-studio/lib/xnode.ts:75-143` for the canonical pattern.
/// See `openmesh-cli/ENGINEERING/PIPELINE-LESSONS.md` Lessons #1, #2, #6.
fn wrap_uri_into_flake_expr(uri: &str) -> String {
    let input_name = derive_input_name(uri);
    format!(
        r#"{{
  inputs = {{
    xnode-manager.url = "github:Openmesh-Network/xnode-manager";
    # We pull openclaw's flake purely to follow its nixpkgs pin, which is
    # the version that's known-working for nixos-containers (dhcpcd starts
    # automatically, mDNS publishes correctly). xnode-manager's pinned
    # nixpkgs and bleeding-edge unstable both fail to start dhcpcd.
    # See PIPELINE-LESSONS.md Lesson #6 + #8.
    openclaw.url = "github:openclaw/nix-openclaw";
    {input_name}.url = "{uri}";
    nixpkgs.follows = "openclaw/nixpkgs";
  }};

  outputs = inputs: {{
    nixosConfigurations.container = inputs.nixpkgs.lib.nixosSystem {{
      specialArgs = {{ inherit inputs; }};
      modules = [
        inputs.xnode-manager.nixosModules.container
        {{
          services.xnode-container.xnode-config = {{
            host-platform = ./xnode-config/host-platform;
            state-version = ./xnode-config/state-version;
            hostname = ./xnode-config/hostname;
          }};
          # CRITICAL: DHCP is required so the container registers its
          # hostname with the host's dnsmasq. Without this, the host
          # nginx cannot resolve the container by name. The systemd
          # override forces dhcpcd to start (the standard
          # `networking.dhcpcd.enable` option doesn't actually start
          # the service in nixos-containers; we have to force it).
          # See PIPELINE-LESSONS.md Lesson #6.
          networking.useDHCP = true;
          networking.dhcpcd.enable = true;
          systemd.services.dhcpcd.wantedBy = [ "multi-user.target" ];
          systemd.services.dhcpcd.enable = true;
        }}
        inputs.{input_name}.nixosModules.default
      ];
    }};
  }};
}}
"#,
        input_name = input_name,
        uri = uri,
    )
}

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

    // The `--flake` arg accepts either:
    //   (a) a flake URI like `github:owner/repo` or `github:owner/repo?dir=foo`
    //       — we wrap it into a full flake.nix expression
    //   (b) a literal flake.nix expression starting with `{` (advanced)
    //       — we send it as-is
    let flake_expr = if flake.trim_start().starts_with('{') {
        flake.clone()
    } else {
        wrap_uri_into_flake_expr(&flake)
    };

    // CRITICAL: `network: Some("containernet")` attaches the container to
    // the host's `vz-*` bridge via systemd-nspawn's `--network-zone` flag.
    // WITHOUT THIS, the container has no network interface bridged to the
    // host at all — DHCP can't run, mDNS can't publish, and the host
    // reverse proxy can't reach the container at any hostname or IP.
    //
    // openclaw on Johnny's xnode has `network: "containernet"` and is
    // reachable; my earlier deploys had `network: None` and were not.
    // See PIPELINE-LESSONS.md Lesson #8 for the bug story.
    let change = sdk::config::ContainerChange {
        settings: sdk::config::ContainerSettings {
            flake: flake_expr.clone(),
            network: Some("containernet".to_string()),
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
    //
    // The forward target uses the **bare container name** with no DNS
    // suffix. This is what the host's dnsmasq resolves to the container's
    // DHCP-allocated IP, after the container has registered its hostname
    // via DHCP from the vz-* bridge. The container needs DHCP enabled
    // (handled by the wrapper from Lesson #6) for this to resolve.
    //
    // Note: `.container` and `.local` were both tried and don't work
    // for host→container forwarding. The first is stale upstream
    // documentation; the second is the mDNS suffix that works for
    // container→container glibc lookups but NOT for nginx upstream
    // resolution which goes via dnsmasq.
    //
    // See ENGINEERING/PIPELINE-LESSONS.md Lessons #3, #6, and #7.
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

    #[test]
    fn derive_input_name_from_simple_github_uri() {
        assert_eq!(
            derive_input_name("github:johnforfar/openmesh-support-agent"),
            "openmesh-support-agent"
        );
    }

    #[test]
    fn derive_input_name_from_xnode_apps_dir_uri() {
        assert_eq!(
            derive_input_name("github:Openmesh-Network/xnode-apps?dir=jellyfin"),
            "jellyfin"
        );
    }

    #[test]
    fn derive_input_name_strips_ref_query() {
        assert_eq!(
            derive_input_name("github:user/myrepo?ref=v1.0.0"),
            "myrepo"
        );
    }

    #[test]
    fn derive_input_name_lowercases_and_sanitizes() {
        assert_eq!(
            derive_input_name("github:UPPERCASE/My.Weird-Name"),
            "my-weird-name"
        );
    }

    #[test]
    fn wrap_uri_produces_valid_nix_attrset_with_required_fields() {
        let wrapped = wrap_uri_into_flake_expr("github:johnforfar/openmesh-support-agent");
        // Top-level must be `{` ... `}`
        assert!(wrapped.trim_start().starts_with('{'));
        assert!(wrapped.trim_end().ends_with('}'));
        // Must declare xnode-manager input (mandatory per the manager)
        assert!(wrapped.contains("xnode-manager.url"));
        // Must reference the user's flake
        assert!(wrapped.contains("github:johnforfar/openmesh-support-agent"));
        // Must produce nixosConfigurations.container (what the manager builds)
        assert!(wrapped.contains("nixosConfigurations.container"));
        // Must reference the user's flake's nixosModules.default
        assert!(wrapped.contains(".nixosModules.default"));
        // Must include the xnode-config module wiring
        assert!(wrapped.contains("xnode-container.xnode-config"));
    }

    #[test]
    fn wrap_uri_uses_derived_input_name_consistently() {
        let wrapped = wrap_uri_into_flake_expr("github:Openmesh-Network/xnode-apps?dir=ollama");
        // The input name "ollama" should appear in: input declaration AND
        // the modules list reference. (No longer in nixpkgs.follows after
        // Lesson #2 fix — that now follows xnode-manager.)
        assert!(wrapped.contains("ollama.url"));
        assert!(wrapped.contains("inputs.ollama.nixosModules.default"));
    }

    #[test]
    fn forward_url_format_uses_bare_hostname_for_dnsmasq() {
        // After trying .container (Lesson #3) and .local (Lesson #7),
        // the actual working pattern is the bare container hostname.
        // The host's dnsmasq resolves it via DHCP registration once the
        // container DHCPs from the vz-* bridge.
        // See PIPELINE-LESSONS.md Lessons #3, #6, #7.
        let name = "support-agent";
        let port = 8080u16;
        let protocol = ForwardProto::Http;
        let forward = format!("{}://{}:{}", protocol.as_str(), name, port);
        assert_eq!(forward, "http://support-agent:8080");
    }

    #[test]
    fn wrap_uri_includes_dhcp_for_container_dns_resolution() {
        // PIPELINE-LESSONS.md Lesson #6: the wrapper MUST enable DHCP so the
        // container registers its hostname with the host's dnsmasq. Without
        // this, host nginx cannot resolve `<name>.container` and every
        // exposed URL returns 502.
        let wrapped = wrap_uri_into_flake_expr("github:johnforfar/openmesh-support-agent");
        assert!(
            wrapped.contains("networking.dhcpcd.enable = true"),
            "wrapper must enable dhcpcd (Lesson #6)"
        );
        assert!(
            wrapped.contains("networking.useDHCP = true"),
            "wrapper must enable networking.useDHCP (Lesson #6)"
        );
    }

    #[test]
    fn wrap_uri_follows_openclaw_nixpkgs() {
        // PIPELINE-LESSONS.md Lesson #9: the wrapper MUST follow
        // openclaw/nixpkgs because that is the version known to start
        // dhcpcd correctly inside an xnode-manager nixos-container.
        // xnode-manager/nixpkgs and bleeding-edge unstable both fail to
        // start dhcpcd, which means host dnsmasq never learns the
        // container hostname, which means the public URL returns 502.
        //
        // (This test was originally written for Lesson #2 which asserted
        // xnode-manager/nixpkgs. Lesson #9 inverted that finding after
        // the openmesh-hello-world deploy proved which baseline actually
        // works end-to-end against the live xnode.)
        let wrapped = wrap_uri_into_flake_expr("github:johnforfar/openmesh-hello-world");
        assert!(
            wrapped.contains(r#"nixpkgs.follows = "openclaw/nixpkgs""#),
            "wrapper must follow openclaw/nixpkgs (Lesson #9)"
        );
        assert!(
            wrapped.contains(r#"openclaw.url = "github:openclaw/nix-openclaw""#),
            "wrapper must declare the openclaw input so the follows resolves"
        );
        // Negative: must NOT follow the user's nixpkgs (newer nixpkgs
        // breaks xnode-manager's container module — Lesson #2 finding
        // is still valid).
        assert!(
            !wrapped.contains(r#"nixpkgs.follows = "openmesh-hello-world/nixpkgs""#),
            "wrapper must not follow the user's nixpkgs"
        );
    }
}
