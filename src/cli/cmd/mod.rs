//! Per-subcommand-group command handlers.
//!
//! Each module here corresponds to one top-level `om` subcommand
//! (`om app ...`, `om req ...`, etc). The module exposes:
//!   - a clap-derived `Args` enum for the subcommand grammar
//!   - a `run(args, format)` async fn that returns `CliResult<()>`
//!
//! `main.rs` is responsible only for parsing the top-level Cli struct and
//! dispatching to the right `run` function.

pub mod app;
pub mod os;
pub mod req;
