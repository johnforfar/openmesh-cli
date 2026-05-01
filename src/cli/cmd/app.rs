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

    /// Show journal logs from a container's processes.
    Logs {
        /// The container name.
        name: String,

        /// Maximum number of log entries to return.
        #[arg(long, default_value_t = 100)]
        max: u32,

        /// Filter by log level (error, warn, info).
        #[arg(long)]
        level: Option<String>,
    },

    /// Set the xnode role (primary or replica) for a container.
    ///
    /// Writes /xnode-config/role. The wrapper flake reads this at build time
    /// and passes xnodeRole as a specialArg. Used by apps to enable
    /// role-specific behavior: backup services on primary, backup-pull and
    /// read-only replica on secondary.
    ///
    /// Example:
    ///   om --profile xnode-2 app set-role xnode-v10-app primary
    ///   om --profile hermes  app set-role xnode-v10-app replica
    SetRole {
        /// The container name.
        name: String,

        /// Role: "primary" or "replica".
        role: String,
    },

    /// Set the public domain for a container (writes /xnode-config/domain).
    ///
    /// This value is available to the container's nix module at build time
    /// via the `xnodeDomain` specialArg. Used for nginx serverName matching
    /// so the SAME commit can deploy to multiple xnodes with different domains.
    ///
    /// Example:
    ///   om --profile xnode-2 app domain set xnode-v10-app v10.own.openmesh.cloud
    SetDomain {
        /// The container name.
        name: String,

        /// The public domain (must match the expose rule + DNS A record).
        domain: String,
    },

    /// Manage environment variables (secrets) for a container.
    ///
    /// Secrets are stored in the container's /xnode-config/env file and
    /// loaded by systemd at service start via EnvironmentFile. They never
    /// enter git or the nix store.
    ///
    /// Examples:
    ///   om app env list my-app
    ///   om app env set my-app DISCORD_TOKEN=abc123 SMTP_PASS=secret
    ///   om app env remove my-app DISCORD_TOKEN
    Env {
        /// The env subcommand to run.
        #[command(subcommand)]
        action: EnvAction,
    },

    /// Read a file from inside a container via the xnode-manager file API.
    ///
    /// Auth uses the operator's manager session cookie (out-of-band; not stored
    /// in any container env). This is the recommended path for pulling backups
    /// and other container-internal files — qualitatively safer than
    /// app-level token-gated endpoints, since the credential is never on the
    /// running app's process.
    ///
    /// Examples:
    ///   om app file read xnode-v10-app /var/backups/v10/v10-daily-2026-05-01.sql.gz \
    ///     --output ~/Backups/v10/v10-2026-05-01.sql.gz
    ///   om app file read xnode-v10-app /xnode-config/role
    File {
        /// The file subcommand to run.
        #[command(subcommand)]
        action: FileAction,
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

#[derive(Subcommand, Debug)]
pub enum EnvAction {
    /// List all environment variables for a container (values masked).
    List {
        /// The container name.
        name: String,

        /// Show full values (DANGER: prints secrets to terminal).
        #[arg(long)]
        show_values: bool,
    },

    /// Set one or more environment variables for a container.
    ///
    /// Format: KEY=VALUE (multiple pairs allowed).
    /// Existing variables are preserved; matching keys are overwritten.
    Set {
        /// The container name.
        name: String,

        /// KEY=VALUE pairs to set.
        #[arg(required = true)]
        pairs: Vec<String>,
    },

    /// Remove one or more environment variables from a container.
    Remove {
        /// The container name.
        name: String,

        /// Variable names to remove.
        #[arg(required = true)]
        keys: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum FileAction {
    /// Read a file from inside a container.
    ///
    /// Writes raw bytes to --output if given, otherwise to stdout. Pipe to
    /// other tools or redirect to a file. Suitable for binary content
    /// (gzip, etc.) and text alike.
    Read {
        /// The container name (e.g. xnode-v10-app).
        name: String,

        /// Absolute path inside the container (e.g. /var/backups/v10/latest.sql.gz).
        path: String,

        /// Write to this local path instead of stdout.
        #[arg(long, short = 'o')]
        output: Option<std::path::PathBuf>,
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
        AppAction::Logs { name, max, level } => {
            app_logs(&session, name, max, level, format).await
        }
        AppAction::Env { action: env_action } => {
            app_env(&session, env_action, format).await
        }
        AppAction::File { action: file_action } => {
            app_file(&session, file_action, format).await
        }
        AppAction::SetDomain { name, domain } => {
            app_set_domain(&session, name, domain, format).await
        }
        AppAction::SetRole { name, role } => {
            app_set_role(&session, name, role, format).await
        }
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

  outputs = inputs: let
    # Read xnode-config/domain at build time so per-xnode domain is injected
    # into app modules via specialArgs. Falls back to container hostname if
    # the file doesn't exist (first-deploy bootstrap).
    xnodeDomainFile = ./xnode-config/domain;
    xnodeDomain =
      if builtins.pathExists xnodeDomainFile
      then inputs.nixpkgs.lib.strings.removeSuffix "\n" (builtins.readFile xnodeDomainFile)
      else inputs.nixpkgs.lib.strings.removeSuffix "\n" (builtins.readFile ./xnode-config/hostname);
    # Read xnode-config/role for primary/replica role (defaults to "primary").
    xnodeRoleFile = ./xnode-config/role;
    xnodeRole =
      if builtins.pathExists xnodeRoleFile
      then inputs.nixpkgs.lib.strings.removeSuffix "\n" (builtins.readFile xnodeRoleFile)
      else "primary";
  in {{
    nixosConfigurations.container = inputs.nixpkgs.lib.nixosSystem {{
      specialArgs = {{ inherit inputs xnodeDomain xnodeRole; }};
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

// =============================================================================
// logs
// =============================================================================

async fn app_logs(
    session: &sdk::utils::Session,
    name: String,
    max: u32,
    level: Option<String>,
    _format: OutputFormat,
) -> CliResult<()> {
    let scope = format!("container:{}", name);

    let log_level = level.as_deref().map(|l| match l.to_lowercase().as_str() {
        "error" => sdk::process::LogLevel::Error,
        "warn" => sdk::process::LogLevel::Warn,
        "info" => sdk::process::LogLevel::Info,
        _ => sdk::process::LogLevel::Unknown,
    });

    let query = sdk::process::LogQuery {
        max: Some(max),
        level: log_level,
    };

    let list_input = sdk::process::ListInput::new_with_path(
        session,
        sdk::process::ListPath { scope: scope.clone() },
    );
    let processes = sdk::process::list(list_input).await;

    match processes {
        Ok(procs) => {
            let mut out = std::io::stdout();
            writeln!(out, "Logs for container `{}`", name)?;
            writeln!(out)?;

            if procs.is_empty() {
                writeln!(out, "  (no processes found)")?;
            }

            for proc in &procs {
                let logs_input = sdk::process::LogsInput {
                    session,
                    path: sdk::process::LogsPath {
                        scope: scope.clone(),
                        process: proc.name.clone(),
                    },
                    query: query.clone(),
                };
                writeln!(out, "--- {} ({}) ---", proc.name, if proc.running { "running" } else { "stopped" })?;
                match sdk::process::logs(logs_input).await {
                    Ok(logs) => {
                        if logs.is_empty() {
                            writeln!(out, "  (no logs)")?;
                        }
                        for log in &logs {
                            let msg = match &log.message {
                                sdk::utils::Output::UTF8 { output } => output.clone(),
                                sdk::utils::Output::Bytes { output } => {
                                    String::from_utf8_lossy(output).to_string()
                                }
                            };
                            let lvl = match &log.level {
                                sdk::process::LogLevel::Error => "ERR",
                                sdk::process::LogLevel::Warn => "WRN",
                                sdk::process::LogLevel::Info => "INF",
                                sdk::process::LogLevel::Unknown => "???",
                            };
                            writeln!(out, "  [{}] {}", lvl, msg)?;
                        }
                    }
                    Err(e) => {
                        writeln!(out, "  [error fetching logs: {}]", e)?;
                    }
                }
                writeln!(out)?;
            }
        }
        Err(e) => {
            let mut out = std::io::stderr();
            writeln!(out, "Could not list processes for container `{}`: {}", name, e)?;
            writeln!(out, "The container may not be running.")?;
        }
    }

    Ok(())
}

// =============================================================================
// set-domain — write /xnode-config/domain for per-xnode domain override
// =============================================================================

async fn app_set_domain(
    session: &sdk::utils::Session,
    name: String,
    domain: String,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;
    if domain.is_empty() || !domain.contains('.') {
        return Err(CliError::invalid_input(
            "domain must be a valid FQDN (e.g. app.example.com)",
        ));
    }
    for c in domain.chars() {
        if !(c.is_ascii_alphanumeric() || c == '.' || c == '-') {
            return Err(CliError::invalid_input(format!(
                "domain `{}` contains invalid character `{}`", domain, c
            )));
        }
    }

    // The xnode-config/domain file is read by the wrapper flake at build time
    // and passed to modules as the `xnodeDomain` specialArg. Writing to the
    // container settings dir (scope = container:<name>) means it lives at
    // /var/lib/xnode-manager/containers/<name>/xnode-config/domain, which is
    // exactly where the wrapper flake looks for it.
    let scope = format!("container:{}", name);
    let input = sdk::file::WriteFileInput {
        session,
        path: sdk::file::WriteFilePath { scope },
        data: sdk::file::WriteFile {
            path: "/xnode-config/domain".to_string(),
            content: domain.clone().into_bytes(),
        },
    };

    sdk::file::write_file(input).await.map_err(|e| {
        CliError::new(
            crate::cli::error::ErrorCode::ManagerUnreachable,
            format!("failed to write domain: {}", e),
        )
    })?;

    let view = SetDomainView { name, domain };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct SetDomainView {
    name: String,
    domain: String,
}

impl Renderable for SetDomainView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Domain set for `{}`:", self.name)?;
        writeln!(w, "  → {}", self.domain)?;
        writeln!(w)?;
        writeln!(w, "Next: redeploy so the nix build picks it up:")?;
        writeln!(w, "  om app deploy --flake <URI> {}", self.name)?;
        Ok(())
    }
}

// =============================================================================
// set-role — write /xnode-config/role for role-aware deploys
// =============================================================================

async fn app_set_role(
    session: &sdk::utils::Session,
    name: String,
    role: String,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;
    if role != "primary" && role != "replica" {
        return Err(CliError::invalid_input(
            "role must be 'primary' or 'replica'",
        ));
    }

    let scope = format!("container:{}", name);
    let input = sdk::file::WriteFileInput {
        session,
        path: sdk::file::WriteFilePath { scope },
        data: sdk::file::WriteFile {
            path: "/xnode-config/role".to_string(),
            content: role.clone().into_bytes(),
        },
    };

    sdk::file::write_file(input).await.map_err(|e| {
        CliError::new(
            crate::cli::error::ErrorCode::ManagerUnreachable,
            format!("failed to write role: {}", e),
        )
    })?;

    let view = SetRoleView { name, role };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct SetRoleView {
    name: String,
    role: String,
}

impl Renderable for SetRoleView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Role set for `{}`:", self.name)?;
        writeln!(w, "  → {}", self.role)?;
        writeln!(w)?;
        writeln!(w, "Next: redeploy so the nix build picks it up:")?;
        writeln!(w, "  om app deploy --flake <URI> {}", self.name)?;
        Ok(())
    }
}

// =============================================================================
// env — secure environment variable management
// =============================================================================

const ENV_FILE_PATH: &str = "/xnode-config/env";

async fn app_env(
    session: &sdk::utils::Session,
    action: EnvAction,
    format: OutputFormat,
) -> CliResult<()> {
    match action {
        EnvAction::List { name, show_values } => env_list(session, name, show_values, format).await,
        EnvAction::Set { name, pairs } => env_set(session, name, pairs, format).await,
        EnvAction::Remove { name, keys } => env_remove(session, name, keys, format).await,
    }
}

async fn app_file(
    session: &sdk::utils::Session,
    action: FileAction,
    format: OutputFormat,
) -> CliResult<()> {
    match action {
        FileAction::Read { name, path, output } => file_read(session, name, path, output, format).await,
    }
}

/// Pull a file out of a container via the manager's file API. Auth is the
/// operator's session cookie (out-of-band; not on the running app's process).
/// Writes raw bytes to --output if given, else stdout.
async fn file_read(
    session: &sdk::utils::Session,
    name: String,
    path: String,
    output: Option<std::path::PathBuf>,
    _format: OutputFormat,
) -> CliResult<()> {
    use std::io::Write;

    let scope = format!("container:{}", name);
    let input = sdk::file::ReadFileInput {
        session,
        path: sdk::file::ReadFilePath { scope },
        query: sdk::file::ReadFile { path: path.clone() },
    };

    let file = sdk::file::read_file(input).await.map_err(|e| {
        CliError::new(
            crate::cli::error::ErrorCode::ManagerUnreachable,
            format!("failed to read {}: {}", path, e),
        )
    })?;

    let bytes: Vec<u8> = match file.content {
        sdk::utils::Output::UTF8 { output } => output.into_bytes(),
        sdk::utils::Output::Bytes { output } => output,
    };

    match output {
        Some(dest) => {
            if let Some(parent) = dest.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        CliError::new(
                            crate::cli::error::ErrorCode::ManagerUnreachable,
                            format!("failed to create parent dir {}: {}", parent.display(), e),
                        )
                    })?;
                }
            }
            std::fs::write(&dest, &bytes).map_err(|e| {
                CliError::new(
                    crate::cli::error::ErrorCode::ManagerUnreachable,
                    format!("failed to write {}: {}", dest.display(), e),
                )
            })?;
            // Stderr so stdout stays usable when the user pipes the command.
            eprintln!("wrote {} bytes to {}", bytes.len(), dest.display());
        }
        None => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&bytes).map_err(|e| {
                CliError::new(
                    crate::cli::error::ErrorCode::ManagerUnreachable,
                    format!("failed to write to stdout: {}", e),
                )
            })?;
        }
    }

    Ok(())
}

/// Read the current env file from the container. Returns a Vec of (key, value) pairs.
async fn env_read(session: &sdk::utils::Session, name: &str) -> CliResult<Vec<(String, String)>> {
    let scope = format!("container:{}", name);
    let input = sdk::file::ReadFileInput {
        session,
        path: sdk::file::ReadFilePath { scope },
        query: sdk::file::ReadFile { path: ENV_FILE_PATH.to_string() },
    };

    match sdk::file::read_file(input).await {
        Ok(file) => {
            let content = match &file.content {
                sdk::utils::Output::UTF8 { output } => output.clone(),
                sdk::utils::Output::Bytes { output } => {
                    String::from_utf8_lossy(output).to_string()
                }
            };
            Ok(parse_env(&content))
        }
        Err(_) => Ok(Vec::new()), // File doesn't exist yet — empty env
    }
}

/// Write env pairs to the container's env file.
async fn env_write(
    session: &sdk::utils::Session,
    name: &str,
    pairs: &[(String, String)],
) -> CliResult<()> {
    let scope = format!("container:{}", name);

    // Build env file content: KEY=VALUE per line, no quoting (systemd EnvironmentFile format)
    let content: String = pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("\n");

    let input = sdk::file::WriteFileInput {
        session,
        path: sdk::file::WriteFilePath { scope },
        data: sdk::file::WriteFile {
            path: ENV_FILE_PATH.to_string(),
            content: content.into_bytes(),
        },
    };

    sdk::file::write_file(input).await.map_err(|e| {
        CliError::new(crate::cli::error::ErrorCode::ManagerUnreachable, format!("failed to write env file: {}", e))
    })?;

    Ok(())
}

fn parse_env(content: &str) -> Vec<(String, String)> {
    content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.splitn(2, '=');
            let key = parts.next()?.trim().to_string();
            let value = parts.next().unwrap_or("").to_string();
            if key.is_empty() { None } else { Some((key, value)) }
        })
        .collect()
}

fn mask_value(value: &str) -> String {
    if value.len() <= 4 {
        "****".to_string()
    } else {
        format!("{}****", &value[..4])
    }
}

/// Validate an env key: must be ASCII, alphanumeric + underscore, start with letter.
fn validate_env_key(key: &str) -> CliResult<()> {
    if key.is_empty() {
        return Err(CliError::invalid_input("env key must not be empty"));
    }
    if !key.chars().next().unwrap().is_ascii_alphabetic() && key.chars().next().unwrap() != '_' {
        return Err(CliError::invalid_input(format!(
            "env key `{}` must start with a letter or underscore", key
        )));
    }
    for c in key.chars() {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return Err(CliError::invalid_input(format!(
                "env key `{}` contains invalid character `{}`; allowed: A-Z a-z 0-9 _", key, c
            )));
        }
    }
    Ok(())
}

async fn env_list(
    session: &sdk::utils::Session,
    name: String,
    show_values: bool,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;
    let pairs = env_read(session, &name).await?;
    let view = EnvListView { name, pairs, show_values };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct EnvListView {
    name: String,
    pairs: Vec<(String, String)>,
    show_values: bool,
}

impl Renderable for EnvListView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.pairs.is_empty() {
            writeln!(w, "No env vars set for `{}`.", self.name)?;
            writeln!(w, "Use `om app env set {} KEY=VALUE` to add secrets.", self.name)?;
        } else {
            writeln!(w, "Env vars for `{}` ({} total):", self.name, self.pairs.len())?;
            for (key, value) in &self.pairs {
                if self.show_values {
                    writeln!(w, "  {}={}", key, value)?;
                } else {
                    writeln!(w, "  {}={}", key, mask_value(value))?;
                }
            }
            if !self.show_values {
                writeln!(w)?;
                writeln!(w, "Use --show-values to reveal full values (prints secrets to terminal).")?;
            }
        }
        Ok(())
    }
}

async fn env_set(
    session: &sdk::utils::Session,
    name: String,
    pairs: Vec<String>,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;

    // Parse and validate KEY=VALUE pairs
    let mut new_pairs: Vec<(String, String)> = Vec::new();
    for pair in &pairs {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").to_string();
        let value = parts.next().ok_or_else(|| {
            CliError::invalid_input(format!(
                "invalid format `{}`; expected KEY=VALUE", pair
            ))
        })?.to_string();
        validate_env_key(&key)?;
        new_pairs.push((key, value));
    }

    // Read existing, merge, write
    let mut existing = env_read(session, &name).await?;
    for (key, value) in &new_pairs {
        if let Some(pos) = existing.iter().position(|(k, _)| k == key) {
            existing[pos].1 = value.clone();
        } else {
            existing.push((key.clone(), value.clone()));
        }
    }

    env_write(session, &name, &existing).await?;

    let keys_set: Vec<String> = new_pairs.iter().map(|(k, _)| k.clone()).collect();
    let view = EnvSetView {
        name,
        keys_set,
        total: existing.len(),
    };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct EnvSetView {
    name: String,
    keys_set: Vec<String>,
    total: usize,
}

impl Renderable for EnvSetView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Set {} var(s) for `{}`:", self.keys_set.len(), self.name)?;
        for key in &self.keys_set {
            writeln!(w, "  {} ✓", key)?;
        }
        writeln!(w, "Total env vars: {}", self.total)?;
        writeln!(w)?;
        writeln!(w, "Restart the app to pick up changes:")?;
        writeln!(w, "  om app deploy --flake <URI> {}", self.name)?;
        Ok(())
    }
}

async fn env_remove(
    session: &sdk::utils::Session,
    name: String,
    keys: Vec<String>,
    format: OutputFormat,
) -> CliResult<()> {
    validate_container_name(&name)?;

    let mut existing = env_read(session, &name).await?;
    let before = existing.len();
    existing.retain(|(k, _)| !keys.contains(k));
    let removed = before - existing.len();

    env_write(session, &name, &existing).await?;

    let view = EnvRemoveView {
        name,
        keys_removed: keys,
        actually_removed: removed,
        total: existing.len(),
    };
    render(&view, format)?;
    Ok(())
}

#[derive(Serialize)]
struct EnvRemoveView {
    name: String,
    keys_removed: Vec<String>,
    actually_removed: usize,
    total: usize,
}

impl Renderable for EnvRemoveView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Removed {} var(s) from `{}`.", self.actually_removed, self.name)?;
        for key in &self.keys_removed {
            writeln!(w, "  {} ✗", key)?;
        }
        writeln!(w, "Remaining env vars: {}", self.total)?;
        Ok(())
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
