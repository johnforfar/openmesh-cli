//! Session context: load the active session for any command that needs it.
//!
//! Supports named profiles via `--profile <name>`. Falls back to the
//! default profile, then to the legacy `~/.openmesh_session.cookie`.

use crate::cli::error::{CliError, CliResult};
use crate::sdk::utils::Session;
use std::sync::OnceLock;

/// Global profile override set by the CLI parser before dispatch.
static ACTIVE_PROFILE: OnceLock<Option<String>> = OnceLock::new();

/// Called by main.rs after parsing CLI args to set the active profile.
pub fn set_active_profile(profile: Option<String>) {
    let _ = ACTIVE_PROFILE.set(profile);
}

/// Load the session for the active profile.
/// Priority: explicit --profile > default profile > legacy session.
pub fn require_session() -> CliResult<Session> {
    let profile = ACTIVE_PROFILE.get().and_then(|p| p.as_ref());

    match profile {
        Some(name) => Session::load_profile(name).map_err(|_| CliError::not_logged_in()),
        None => Session::load().map_err(|_| CliError::not_logged_in()),
    }
}
