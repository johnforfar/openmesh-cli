//! `om` library crate.
//!
//! The `om` binary is implemented as a thin wrapper over this library so
//! that integration tests and (in the future) other Rust consumers can
//! reuse the same modules.
//!
//! Public surface:
//!   - [`cli`] — output formatting, errors, async waits, flake editing,
//!     and the per-subcommand handlers
//!   - [`sdk`] — generated-style HTTP client for the Xnode Manager API,
//!     vendored from `Openmesh-Network/xnode-manager-sdk` with a curl
//!     fallback that works around the manager's nginx fingerprint check

pub mod cli;
pub mod sdk;
