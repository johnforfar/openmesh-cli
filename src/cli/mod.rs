//! CLI infrastructure for the `om` binary.
//!
//! This module is the bridge between user-facing commands and the SDK. It
//! provides:
//!   - [`output`] — format-agnostic rendering (`--format json|plain|table`)
//!   - [`error`] — rich errors with codes + hints, mappable to SDK errors
//!   - [`wait`]  — polling helpers for async `request_id`-style operations
//!   - [`flake_editor`] — safe rewriting of the `# START USER CONFIG` block
//!     in a NixOS flake
//!   - [`context`] — session loading and (future) multi-xnode profiles
//!   - [`cmd`]  — one module per top-level subcommand group
//!
//! Design rules followed throughout:
//!   1. **stdout is data, stderr is status.** Never `println!` progress info
//!      when `--format json` is selected — it would corrupt machine-readable
//!      output. Use `eprintln!` for human status.
//!   2. **Every state-changing op returns a `RequestId`** and supports `--wait`
//!      (default true) to block until the Xnode finishes the operation.
//!   3. **Errors carry codes** so AI agents can branch on `E_NOT_LOGGED_IN`
//!      etc. without parsing English messages.
//!   4. **No panics in command code.** Every error path is a `CliError`.

pub mod cmd;
pub mod context;
pub mod error;
pub mod flake_editor;
pub mod output;
pub mod wait;

pub use error::{CliError, CliResult};
pub use output::{OutputFormat, Renderable};
