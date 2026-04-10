//! Rich error type for `om` commands.
//!
//! Every error has:
//!   - a **code** (like `E_NOT_LOGGED_IN`) — stable, machine-readable, never
//!     localized; AI agents can branch on these
//!   - a **message** — short human description
//!   - an optional **hint** — actionable suggestion ("Run `om login --url X`")
//!   - an optional **source** — the underlying error chain
//!
//! When `--format json` is set, errors render as:
//! ```json
//! { "error": { "code": "E_...", "message": "...", "hint": "..." } }
//! ```
//! ...so an AI agent never has to parse English to recover.

use crate::sdk::utils::Error as SdkError;
use serde::Serialize;
use std::fmt;

pub type CliResult<T> = std::result::Result<T, CliError>;

#[derive(Debug, Serialize, Clone)]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Stable error codes. Never rename or remove a variant — AI agents and
/// scripts depend on these strings. Add new ones at the end.
///
/// Custom serializer below ensures JSON renders the `E_*` form (`E_NOT_LOGGED_IN`)
/// rather than the bare variant name. Code wants the prefix; the prefix is
/// what scripts and agents are documented to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// No session file present, or session is malformed.
    NotLoggedIn,
    /// Session present but expired (manager returned 401).
    SessionExpired,
    /// Manager returned 4xx other than 401 (bad request, not found, etc).
    BadRequest,
    /// Manager returned 5xx, or transport error (DNS, connect, TLS).
    ManagerUnreachable,
    /// JSON parsing failed on a manager response.
    InvalidResponse,
    /// User-supplied input failed validation (bad domain, missing flag, etc).
    InvalidInput,
    /// The requested resource (container, request id, etc) does not exist.
    NotFound,
    /// A resource the operation needs already exists in a conflicting state.
    AlreadyExists,
    /// The flake editor refused to apply a change because it would corrupt
    /// the configuration (e.g. user-config markers missing).
    UnsafeFlakeEdit,
    /// An asynchronous request did not complete within the timeout.
    Timeout,
    /// Generic catch-all. Prefer a specific code when possible.
    Internal,
}

impl serde::Serialize for ErrorCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotLoggedIn => "E_NOT_LOGGED_IN",
            ErrorCode::SessionExpired => "E_SESSION_EXPIRED",
            ErrorCode::BadRequest => "E_BAD_REQUEST",
            ErrorCode::ManagerUnreachable => "E_MANAGER_UNREACHABLE",
            ErrorCode::InvalidResponse => "E_INVALID_RESPONSE",
            ErrorCode::InvalidInput => "E_INVALID_INPUT",
            ErrorCode::NotFound => "E_NOT_FOUND",
            ErrorCode::AlreadyExists => "E_ALREADY_EXISTS",
            ErrorCode::UnsafeFlakeEdit => "E_UNSAFE_FLAKE_EDIT",
            ErrorCode::Timeout => "E_TIMEOUT",
            ErrorCode::Internal => "E_INTERNAL",
        }
    }
}

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn not_logged_in() -> Self {
        Self::new(ErrorCode::NotLoggedIn, "No active session")
            .with_hint("Run `om login --url <your-xnode-manager-url>` first")
    }

    pub fn unsafe_flake_edit(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::UnsafeFlakeEdit, message)
            .with_hint("This is a safety check to avoid corrupting /etc/nixos/flake.nix. Inspect the flake manually with `om node info` if needed.")
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {}", hint)?;
        }
        Ok(())
    }
}

impl std::error::Error for CliError {}

/// Map an SDK error into a CLI error with the best matching code.
///
/// The SDK currently has a coarse error type (`OutputError(String)` for most
/// things), so we sniff the message text to assign codes. As the SDK improves
/// upstream, this mapping should narrow.
impl From<SdkError> for CliError {
    fn from(e: SdkError) -> Self {
        let raw = e.to_string();
        let lower = raw.to_lowercase();

        // The SDK's curl fallback returns this exact string on 401.
        if lower.contains("session expired") || lower.contains("unauthorized") {
            return CliError::new(ErrorCode::SessionExpired, raw)
                .with_hint("Run `om login --url <your-xnode-manager-url>` to re-authenticate");
        }
        if lower.contains("no session") {
            return CliError::not_logged_in();
        }
        if lower.contains("not found") || lower.contains("404") {
            return CliError::new(ErrorCode::NotFound, raw);
        }
        if lower.contains("400") || lower.contains("bad request") {
            return CliError::new(ErrorCode::BadRequest, raw);
        }
        if lower.contains("failed to parse json") {
            return CliError::new(ErrorCode::InvalidResponse, raw);
        }
        if lower.contains("connect") || lower.contains("dns") || lower.contains("tls") || lower.contains("certificate") {
            return CliError::new(ErrorCode::ManagerUnreachable, raw)
                .with_hint("Check your network connection and that the Xnode Manager URL is reachable");
        }
        CliError::new(ErrorCode::Internal, raw)
    }
}

/// Convert anyhow errors (used in some leaf utilities) into CliError.
impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> Self {
        // If the chain wraps an SdkError, prefer that mapping.
        if let Some(sdk) = e.downcast_ref::<SdkError>() {
            return CliError::from(SdkError::OutputError(sdk.to_string()));
        }
        CliError::new(ErrorCode::Internal, e.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::new(ErrorCode::Internal, format!("I/O error: {}", e))
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        CliError::new(ErrorCode::InvalidResponse, format!("JSON error: {}", e))
    }
}

/// Render a CliError to the appropriate output stream and exit code.
///
/// JSON format goes to stdout under an `error` key so machine-readers get
/// a consistent envelope; plain format goes to stderr (errors are not data).
pub fn report(error: &CliError, format: crate::cli::OutputFormat) -> i32 {
    match format {
        crate::cli::OutputFormat::Json => {
            #[derive(Serialize)]
            struct Envelope<'a> {
                error: &'a CliError,
            }
            let env = Envelope { error };
            // Errors STILL go to stdout in JSON mode so a single pipe captures
            // both success and failure as parseable JSON.
            if let Ok(s) = serde_json::to_string_pretty(&env) {
                println!("{}", s);
            } else {
                eprintln!("{}", error);
            }
        }
        crate::cli::OutputFormat::Plain => {
            eprintln!("error: {}", error);
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable_strings() {
        // If a future refactor renames a variant, this test will catch it.
        assert_eq!(ErrorCode::NotLoggedIn.as_str(), "E_NOT_LOGGED_IN");
        assert_eq!(ErrorCode::SessionExpired.as_str(), "E_SESSION_EXPIRED");
        assert_eq!(ErrorCode::Timeout.as_str(), "E_TIMEOUT");
    }

    #[test]
    fn cli_error_serializes_with_hint() {
        let e = CliError::not_logged_in();
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("E_NOT_LOGGED_IN"));
        assert!(json.contains("hint"));
        assert!(json.contains("om login"));
    }

    #[test]
    fn cli_error_omits_hint_when_none() {
        let e = CliError::new(ErrorCode::Internal, "boom");
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("hint"));
    }
}
