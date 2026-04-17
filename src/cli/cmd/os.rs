//! `om os ...` — host-level OS configuration management.
//!
//! These commands let you manage the xnode's OS-level nix configuration
//! by calling xnode-manager's `/os/get` and `/os/set` endpoints — the same
//! endpoints the xnode studio web UI uses under the hood.
//!
//! The most common use case (and the one currently implemented) is
//! `om os github-auth set <token>` which injects a fine-grained GitHub
//! PAT into `nix.extraOptions` so the xnode can deploy flakes from
//! private github repositories.
//!
//! ## Why a whole subcommand for a PAT?
//!
//! Nix's github fetcher uses unauthenticated HTTPS by default, which returns
//! 404 for private repos. To authenticate, nix reads `access-tokens` from
//! `/etc/nix/nix.conf`, which on NixOS is generated from the system config.
//! The canonical way to get it there is to add:
//!
//! ```nix
//! nix.extraOptions = ''
//!   access-tokens = github.com=ghp_YOUR_TOKEN
//! '';
//! ```
//!
//! into the xnode's user-config block. Then the next OS rebuild bakes
//! the line into `/etc/nix/nix.conf` and subsequent `om app deploy` calls
//! against private repos "just work".
//!
//! ## Security notes
//!
//! - The token ends up in the nix store and in `/etc/nix/nix.conf` on the
//!   xnode — world-readable by anything with shell access to the host.
//! - Use **fine-grained** PATs scoped to the minimum set of repos with
//!   `Contents: Read-only` permission. Never use a classic PAT with full
//!   `repo` scope.
//! - Rotate when contributors change, and when the token expires.
//!
//! See `om os github-auth --help` for usage.

use crate::cli::context::require_session;
use crate::cli::error::{CliError, CliResult, ErrorCode};
use crate::cli::output::{OutputFormat, Renderable, render};
use crate::cli::wait::wait_for_request;
use crate::sdk;
use clap::Subcommand;
use serde::Serialize;
use std::io::Write;

// ---------------------------------------------------------------------------
// Grammar
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum OsAction {
    /// Manage the GitHub access token used for deploying flakes from private
    /// github repositories (subcommands: set / clear / status).
    #[command(subcommand)]
    GithubAuth(GithubAuthAction),

    /// Set or view the xnode's public domain name.
    #[command(subcommand)]
    Domain(DomainAction),
}

#[derive(Subcommand, Debug)]
pub enum DomainAction {
    /// Claim a *.openmesh.cloud subdomain and configure the xnode.
    ///
    /// Reserves the DNS record via claim.dns.openmesh.network, then sets
    /// the domain on the xnode and triggers an OS rebuild for ACME/TLS.
    ///
    /// Example:
    ///   om os domain claim xnode
    ///   om --profile v10 os domain claim xnode --email john@openxai.org
    Claim {
        /// Subdomain to claim (e.g., "xnode" → manager.xnode.openmesh.cloud).
        subdomain: String,

        /// ACME email for TLS certificates.
        #[arg(long, default_value = "john@openxai.org")]
        email: String,

        /// Don't wait for the OS rebuild to finish.
        #[arg(long)]
        no_wait: bool,

        /// Maximum seconds to wait for the rebuild.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Set the domain directly (skip DNS claim — use if DNS is already configured).
    Set {
        /// The fully qualified domain name.
        domain: String,

        /// ACME email for TLS certificates.
        #[arg(long)]
        email: Option<String>,

        /// Don't wait for the OS rebuild to finish.
        #[arg(long)]
        no_wait: bool,

        /// Maximum seconds to wait for the rebuild.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },
    /// Check if a *.openmesh.cloud subdomain is available.
    Check {
        /// Subdomain to check (e.g., "xnode").
        subdomain: String,
    },
    /// Show the xnode's current domain configuration.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum GithubAuthAction {
    /// Inject or replace a GitHub PAT in the xnode's nix.extraOptions
    /// user-config block, then trigger an OS rebuild so the new token lands
    /// in /etc/nix/nix.conf.
    ///
    /// Example:
    ///   om os github-auth set ghp_YourFineGrainedReadOnlyToken
    ///
    /// After this command returns success, subsequent `om app deploy` calls
    /// against private github repos will authenticate automatically.
    ///
    /// Security: The token is stored in the xnode's OS flake and baked into
    /// /etc/nix/nix.conf on the xnode's filesystem. Use fine-grained PATs
    /// scoped to the minimum set of repos with read-only Contents permission.
    Set {
        /// The GitHub PAT. Accepts classic `ghp_...` and fine-grained
        /// `github_pat_...` tokens.
        token: String,

        /// Don't wait for the OS rebuild to finish; return the request id
        /// immediately after submitting the config change.
        #[arg(long)]
        no_wait: bool,

        /// Maximum seconds to wait for the rebuild to complete.
        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },

    /// Remove the github.com access-tokens entry from the xnode's
    /// nix.extraOptions, trigger a rebuild.
    ///
    /// Use this when you've made all your previously-private repos public,
    /// or when rotating a token (you'd typically `set` the new one directly
    /// which replaces the old — this command is for full removal).
    Clear {
        #[arg(long)]
        no_wait: bool,

        #[arg(long, default_value_t = 600)]
        timeout: u64,
    },

    /// Report whether a github.com access-tokens entry is currently
    /// configured in the xnode's OS flake. Does NOT print the token itself.
    Status,
}

// ---------------------------------------------------------------------------
// run()
// ---------------------------------------------------------------------------

pub async fn run(action: OsAction, format: OutputFormat) -> CliResult<()> {
    let session = require_session()?;
    match action {
        OsAction::GithubAuth(sub) => match sub {
            GithubAuthAction::Set {
                token,
                no_wait,
                timeout,
            } => {
                set_github_token(&session, &token, no_wait, timeout, format).await
            }
            GithubAuthAction::Clear { no_wait, timeout } => {
                clear_github_token(&session, no_wait, timeout, format).await
            }
            GithubAuthAction::Status => status_github_token(&session, format).await,
        },
        OsAction::Domain(sub) => match sub {
            DomainAction::Claim { subdomain, email, no_wait, timeout } => {
                claim_domain(&session, &subdomain, &email, no_wait, timeout, format).await
            }
            DomainAction::Set { domain, email, no_wait, timeout } => {
                set_domain(&session, &domain, email.as_deref(), no_wait, timeout, format).await
            }
            DomainAction::Check { subdomain } => check_domain(&subdomain, format).await,
            DomainAction::Status => domain_status(&session, format).await,
        },
    }
}

// ---------------------------------------------------------------------------
// domain claim / check / set / status
// ---------------------------------------------------------------------------

const DNS_CLAIM_BASE: &str = "https://claim.dns.openmesh.network";

async fn check_domain(subdomain: &str, _format: OutputFormat) -> CliResult<()> {
    let url = format!("{}/{}/available", DNS_CLAIM_BASE, subdomain);
    let output = std::process::Command::new("curl")
        .arg("-s").arg("-k")
        .arg(&url)
        .output()
        .map_err(|e| CliError::new(ErrorCode::Internal, format!("curl failed: {}", e)))?;
    let body = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let domain = format!("manager.{}.openmesh.cloud", subdomain);
    match body.as_str() {
        "true" => println!("{} is available (→ {})", subdomain, domain),
        "false" => println!("{} is taken (→ {})", subdomain, domain),
        _ => println!("Unexpected response: {}", body),
    }
    Ok(())
}

async fn claim_domain(
    session: &sdk::utils::Session,
    subdomain: &str,
    email: &str,
    no_wait: bool,
    timeout: u64,
    format: OutputFormat,
) -> CliResult<()> {
    let domain = format!("manager.{}.openmesh.cloud", subdomain);

    // Step 1: Check availability
    println!("Checking availability of '{}'...", subdomain);
    let check_url = format!("{}/{}/available", DNS_CLAIM_BASE, subdomain);
    let output = std::process::Command::new("curl")
        .arg("-s").arg("-k")
        .arg(&check_url)
        .output()
        .map_err(|e| CliError::new(ErrorCode::Internal, format!("curl failed: {}", e)))?;
    let available = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if available == "false" {
        // Already claimed — check if it points to our IP
        println!("  '{}' already claimed. Checking if it's ours...", subdomain);
        let resolve = std::process::Command::new("dig")
            .arg("+short").arg(&domain).arg("A")
            .output()
            .ok()
            .and_then(|o| Some(String::from_utf8_lossy(&o.stdout).trim().to_string()));
        if let Some(ip) = resolve {
            if !ip.is_empty() {
                println!("  {} → {} (already configured)", domain, ip);
            }
        }
    } else {
        // Step 2: Reserve the subdomain via DNS claim service
        println!("Reserving DNS record for '{}'...", domain);

        // Get the xnode's IP from the session URL
        let url_parsed = url::Url::parse(&session.base_url)
            .map_err(|e| CliError::new(ErrorCode::Internal, format!("Invalid session URL: {}", e)))?;
        let ip = url_parsed.host_str()
            .ok_or_else(|| CliError::new(ErrorCode::Internal, "No host in session URL".to_string()))?;

        // The wallet address from the session cookies
        let user = session.cookies.iter()
            .find(|c| c.starts_with("xnode_auth_user="))
            .map(|c| c.trim_start_matches("xnode_auth_user=").to_string())
            .map(|u| u.replace("%3A", ":").replace("%2F", "/"))
            .unwrap_or_default();

        let reserve_url = format!("{}/{}/reserve", DNS_CLAIM_BASE, subdomain);
        let payload = serde_json::json!({
            "user": user,
            "ipv4": ip
        });

        let reserve_output = std::process::Command::new("curl")
            .arg("-s").arg("-k")
            .arg("-X").arg("POST")
            .arg("-H").arg("Content-Type: application/json")
            .arg("-d").arg(serde_json::to_string(&payload).unwrap_or_default())
            .arg(&reserve_url)
            .output()
            .map_err(|e| CliError::new(ErrorCode::Internal, format!("curl failed: {}", e)))?;

        let reserve_body = String::from_utf8_lossy(&reserve_output.stdout);
        if !reserve_output.status.success() {
            return Err(CliError::new(ErrorCode::Internal,
                format!("DNS claim failed: {}", reserve_body)));
        }
        println!("  DNS record created: {} → {}", domain, ip);
    }

    // Step 3: Re-login with the new domain so cookies are signed for it.
    // xnode-auth validates the cookie signature against the Host header,
    // so we need cookies signed for the target domain, not the IP.
    println!("Re-authenticating with new domain '{}'...", domain);
    let new_url = format!("https://{}", domain);

    // Shell out to `om profile login` with the new URL
    let login_output = std::process::Command::new("om")
        .arg("profile").arg("login")
        .arg(subdomain)  // temporary profile name
        .arg("-u").arg(&new_url)
        .output()
        .map_err(|e| CliError::new(ErrorCode::Internal, format!("login failed: {}", e)))?;

    if !login_output.status.success() {
        let stderr = String::from_utf8_lossy(&login_output.stderr);
        return Err(CliError::new(ErrorCode::Internal,
            format!("Re-login with new domain failed: {}", stderr)));
    }
    println!("  Authenticated with {}", domain);

    // Load the freshly created session for the new domain
    let new_session = sdk::utils::Session::load_profile(subdomain)
        .map_err(|e| CliError::new(ErrorCode::Internal, format!("Failed to load new session: {:?}", e)))?;

    // Step 4: Set the domain on the xnode using the domain-signed session
    println!("Configuring xnode with domain '{}'...", domain);
    set_domain(&new_session, &domain, Some(email), no_wait, timeout, format).await
}

async fn set_domain(
    session: &sdk::utils::Session,
    domain: &str,
    email: Option<&str>,
    no_wait: bool,
    timeout: u64,
    _format: OutputFormat,
) -> CliResult<()> {
    println!("Setting domain to '{}' ...", domain);

    let change = sdk::os::OSChange {
        flake: None,
        update_inputs: None,
        xnode_owner: None,
        domain: Some(domain.to_string()),
        acme_email: email.map(|e| e.to_string()),
        user_passwd: None,
    };

    let input = sdk::os::SetInput::new_with_data(session, change);
    let resp = sdk::os::set(input).await?;

    println!("submitted (request_id={})", resp.request_id);

    if no_wait {
        println!("Domain change submitted. Use `om req wait {}` to track.", resp.request_id);
        return Ok(());
    }

    println!("waiting for OS rebuild...");
    let info = wait_for_request(session, resp.request_id, timeout).await?;

    match info.result {
        Some(sdk::request::RequestIdResult::Success { .. }) => {
            println!("Domain set to '{}'", domain);
            if let Some(e) = email {
                println!("  ACME email: {}", e);
            }
            println!("  The xnode will now serve TLS on this domain.");
        }
        Some(sdk::request::RequestIdResult::Error { error }) => {
            return Err(CliError::new(ErrorCode::Internal, format!("Domain set failed: {}", error)));
        }
        None => {
            return Err(CliError::new(ErrorCode::Internal, "Domain set timed out".to_string()));
        }
    }

    Ok(())
}

async fn domain_status(
    session: &sdk::utils::Session,
    _format: OutputFormat,
) -> CliResult<()> {
    let input = sdk::os::GetInput::new(session);
    let config = sdk::os::get(input).await?;

    println!("Domain: {}", config.domain.unwrap_or_else(|| "(not set)".to_string()));
    if let Some(email) = config.acme_email {
        println!("  ACME email: {}", email);
    }
    if let Some(owner) = config.xnode_owner {
        println!("  Owner: {}", owner);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// set
// ---------------------------------------------------------------------------

async fn set_github_token(
    session: &sdk::utils::Session,
    token: &str,
    no_wait: bool,
    timeout: u64,
    format: OutputFormat,
) -> CliResult<()> {
    if !is_plausible_github_pat(token) {
        return Err(CliError::new(ErrorCode::InvalidInput,
            "Token does not look like a GitHub PAT. Expected `ghp_...` (classic) \
             or `github_pat_...` (fine-grained). Refusing to inject."
                .to_string(),
        ));
    }

    // 1. Fetch the current OS config
    let current = fetch_os(session).await?;

    // 2. Build the new flake string with the token injected/replaced
    let new_flake = inject_github_token(&current.flake, token);
    if new_flake == current.flake {
        // No change means the token is already present with the same value.
        let view = GithubAuthView {
            status: "unchanged".to_string(),
            message: "Token already configured — no rebuild needed.".to_string(),
            request_id: None,
        };
        render(&view, format)?;
        return Ok(());
    }

    // 3. POST the new flake to /os/set
    let request_id = push_os_flake(session, new_flake).await?;

    // 4. Wait for the rebuild if asked
    let view = if no_wait {
        GithubAuthView {
            status: "submitted".to_string(),
            message: format!(
                "OS rebuild submitted. Use `om req wait {request_id}` to block."
            ),
            request_id: Some(request_id),
        }
    } else {
        let info = wait_for_request(session, request_id, timeout).await?;
        match info.result {
            Some(sdk::request::RequestIdResult::Success { .. }) => GithubAuthView {
                status: "success".to_string(),
                message: "Token installed; xnode rebuilt. Private deploys now work."
                    .to_string(),
                request_id: Some(request_id),
            },
            Some(sdk::request::RequestIdResult::Error { error }) => {
                return Err(CliError::new(
                    ErrorCode::Internal,
                    format!(
                        "OS rebuild failed: {error}. The xnode may be in a partially-configured \
                         state — check `om req show {request_id}` for details."
                    ),
                ));
            }
            None => GithubAuthView {
                status: "timeout".to_string(),
                message: format!(
                    "Rebuild still running after {timeout}s. Use `om req wait {request_id}` \
                     to keep watching."
                ),
                request_id: Some(request_id),
            },
        }
    };

    render(&view, format)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// clear
// ---------------------------------------------------------------------------

async fn clear_github_token(
    session: &sdk::utils::Session,
    no_wait: bool,
    timeout: u64,
    format: OutputFormat,
) -> CliResult<()> {
    let current = fetch_os(session).await?;
    let new_flake = remove_github_token(&current.flake);
    if new_flake == current.flake {
        let view = GithubAuthView {
            status: "unchanged".to_string(),
            message: "No github.com access-tokens entry to remove.".to_string(),
            request_id: None,
        };
        render(&view, format)?;
        return Ok(());
    }
    let request_id = push_os_flake(session, new_flake).await?;
    let view = if no_wait {
        GithubAuthView {
            status: "submitted".to_string(),
            message: format!("OS rebuild submitted. `om req wait {request_id}` to block."),
            request_id: Some(request_id),
        }
    } else {
        let info = wait_for_request(session, request_id, timeout).await?;
        match info.result {
            Some(sdk::request::RequestIdResult::Success { .. }) => GithubAuthView {
                status: "success".to_string(),
                message: "Token removed; xnode rebuilt.".to_string(),
                request_id: Some(request_id),
            },
            Some(sdk::request::RequestIdResult::Error { error }) => {
                return Err(CliError::new(ErrorCode::Internal, format!("OS rebuild failed: {error}")));
            }
            None => GithubAuthView {
                status: "timeout".to_string(),
                message: format!("Rebuild still running after {timeout}s."),
                request_id: Some(request_id),
            },
        }
    };
    render(&view, format)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

async fn status_github_token(
    session: &sdk::utils::Session,
    format: OutputFormat,
) -> CliResult<()> {
    let current = fetch_os(session).await?;
    let configured = current.flake.contains("access-tokens = github.com=");
    // Try to extract a fingerprint of the token (last 4 chars) without
    // printing the whole thing, so users can eyeball whether rotation worked.
    let fingerprint = extract_token_fingerprint(&current.flake);
    let view = GithubAuthStatusView {
        configured,
        fingerprint,
    };
    render(&view, format)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers — OS flake GET/SET
// ---------------------------------------------------------------------------

async fn fetch_os(session: &sdk::utils::Session) -> CliResult<sdk::os::OSConfiguration> {
    let input: sdk::os::GetInput = sdk::os::GetInput::new(session);
    let config = sdk::os::get(input).await?;
    Ok(config)
}

async fn push_os_flake(session: &sdk::utils::Session, new_flake: String) -> CliResult<u32> {
    let change = sdk::os::OSChange {
        flake: Some(new_flake),
        update_inputs: None,
        xnode_owner: None,
        domain: None,
        acme_email: None,
        user_passwd: None,
    };
    let input = sdk::os::SetInput::new_with_data(session, change);
    let resp = sdk::os::set(input).await?;
    Ok(resp.request_id)
}

// ---------------------------------------------------------------------------
// Helpers — flake-string mutation (idempotent + regex-free)
// ---------------------------------------------------------------------------

/// Inject (or replace) a `nix.extraOptions` block containing the github
/// access token inside the user-config fenced region of the xnode's OS flake.
///
/// The fenced region looks like this in the current xnode-manager OS flake:
///
/// ```text
/// # START USER CONFIG
/// services.xnode-reverse-proxy.rules."hello.build.openmesh.cloud" = ...;
/// # END USER CONFIG
/// ```
///
/// We insert (or replace) our `nix.extraOptions` block at the top of that
/// region, preserving everything else the user has configured there.
///
/// Idempotent: calling set with the same token twice is a no-op; calling set
/// with a different token replaces the old one.
fn inject_github_token(flake: &str, token: &str) -> String {
    let new_block = format!(
        "nix.extraOptions = ''\n  access-tokens = github.com={token}\n'';\n"
    );

    // If an existing nix.extraOptions line containing our access-tokens
    // directive is present, replace its whole block (between the opening
    // `''` and the matching `''`).
    if let Some(start) = flake.find("nix.extraOptions = ''") {
        // Find the closing `''` after `start`.
        let after_open = start + "nix.extraOptions = ''".len();
        if let Some(rel_close) = flake[after_open..].find("''") {
            let absolute_close = after_open + rel_close + 2; // include the closing ''
            // Also swallow an immediate trailing `;` and newline so we don't
            // leave dangling punctuation.
            let mut tail_end = absolute_close;
            for b in flake[absolute_close..].bytes() {
                match b {
                    b';' | b'\n' | b' ' | b'\t' => tail_end += 1,
                    _ => break,
                }
            }
            let mut out = String::with_capacity(flake.len() + new_block.len());
            out.push_str(&flake[..start]);
            out.push_str(&new_block);
            out.push_str(&flake[tail_end..]);
            return out;
        }
    }

    // Otherwise, insert the block right after `# START USER CONFIG\n`
    if let Some(start) = flake.find("# START USER CONFIG") {
        // Skip to the newline after the marker
        if let Some(nl) = flake[start..].find('\n') {
            let insert_at = start + nl + 1;
            let mut out = String::with_capacity(flake.len() + new_block.len());
            out.push_str(&flake[..insert_at]);
            out.push_str(&new_block);
            out.push_str(&flake[insert_at..]);
            return out;
        }
    }

    // If we can't find the fenced block, give up without mutating.
    // (The flake format may have changed; failing safe is better than
    // writing a malformed flake.)
    flake.to_string()
}

/// Remove any existing `nix.extraOptions` block from the flake.
///
/// This is intentionally simple: we only look for `nix.extraOptions = ''`
/// and strip from that line through the matching closing `''`. If there
/// are multiple matches we strip them all.
fn remove_github_token(flake: &str) -> String {
    let mut out = flake.to_string();
    while let Some(start) = out.find("nix.extraOptions = ''") {
        let after_open = start + "nix.extraOptions = ''".len();
        let Some(rel_close) = out[after_open..].find("''") else {
            break;
        };
        let absolute_close = after_open + rel_close + 2;
        let mut tail_end = absolute_close;
        for b in out[absolute_close..].bytes() {
            match b {
                b';' | b'\n' | b' ' | b'\t' => tail_end += 1,
                _ => break,
            }
        }
        // Also trim any leading whitespace on the line we're about to cut
        // so we don't leave a blank line behind.
        let mut head_start = start;
        while head_start > 0 {
            let b = out.as_bytes()[head_start - 1];
            if b == b' ' || b == b'\t' {
                head_start -= 1;
            } else {
                break;
            }
        }
        let mut new_out = String::with_capacity(out.len());
        new_out.push_str(&out[..head_start]);
        new_out.push_str(&out[tail_end..]);
        out = new_out;
    }
    out
}

/// Pull the last ~4 chars of the currently-configured github token out of
/// the flake so `om os github-auth status` can show a fingerprint without
/// leaking the full token. Returns None if no token is configured or if
/// the format is unrecognised.
fn extract_token_fingerprint(flake: &str) -> Option<String> {
    let needle = "access-tokens = github.com=";
    let idx = flake.find(needle)?;
    let after = &flake[idx + needle.len()..];
    let end = after
        .find(|c: char| c.is_whitespace() || c == '\'' || c == ';')
        .unwrap_or(after.len());
    let tok = &after[..end];
    if tok.len() < 8 {
        Some(format!("…{tok}"))
    } else {
        Some(format!("{}…{}", &tok[..4], &tok[tok.len().saturating_sub(4)..]))
    }
}

/// Cheap sanity check on the token format — catches pastes that are
/// obviously not a PAT (whitespace, URLs, empty, etc).
fn is_plausible_github_pat(token: &str) -> bool {
    let t = token.trim();
    if t.is_empty() {
        return false;
    }
    (t.starts_with("ghp_") || t.starts_with("github_pat_")) && t.len() >= 20
}

// ---------------------------------------------------------------------------
// Output views
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct GithubAuthView {
    pub status: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u32>,
}

impl Renderable for GithubAuthView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "github-auth: {}", self.status)?;
        writeln!(w, "  {}", self.message)?;
        if let Some(id) = self.request_id {
            writeln!(w, "  request_id: {}", id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct GithubAuthStatusView {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

impl Renderable for GithubAuthStatusView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            w,
            "github-auth: {}",
            if self.configured { "configured" } else { "not set" }
        )?;
        if let Some(fp) = &self.fingerprint {
            writeln!(w, "  fingerprint: {fp}")?;
        }
        Ok(())
    }
}
