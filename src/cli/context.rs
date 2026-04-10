//! Session context: load the active session for any command that needs it.
//!
//! Today this is just a thin wrapper over `Session::load()`. The reason it
//! exists as its own module is to make multi-xnode support easy later: a
//! future `~/.openmesh/config.yaml` with named profiles will plug in here
//! without touching every command.

use crate::cli::error::{CliError, CliResult};
use crate::sdk::utils::Session;

/// Load the active session, mapping any error into a CliError with a
/// helpful "run om login" hint.
pub fn require_session() -> CliResult<Session> {
    Session::load().map_err(|_| CliError::not_logged_in())
}
