//! Output formatting for `om` commands.
//!
//! All commands take a `--format` flag (`plain`, `json`, `yaml` future) and
//! render through the [`Renderable`] trait. Plain output is for humans; JSON
//! is for scripts and AI agents.
//!
//! Rule: **stdout is data**. Status messages (`Deploying...`, `Waiting for
//! request 42...`) must go to stderr via [`status!`] so JSON output remains
//! parseable when piped.

use clap::ValueEnum;
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable plain text (default).
    Plain,
    /// Machine-readable JSON. Use this from scripts and AI agents.
    Json,
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Plain
    }
}

/// Trait implemented by command output types.
///
/// Each command defines a small struct that holds the data it wants to show,
/// then implements `Renderable` to control how it formats in plain mode. JSON
/// rendering is automatic via `Serialize`.
pub trait Renderable: Serialize {
    /// Render this value as human-readable plain text to the given writer.
    /// JSON output is handled centrally by [`render`] using `serde_json`.
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()>;
}

/// Render a value to stdout in the requested format.
pub fn render<T: Renderable>(value: &T, format: OutputFormat) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    match format {
        OutputFormat::Plain => {
            value.render_plain(&mut out)?;
        }
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut out, value)?;
            writeln!(&mut out)?;
        }
    }
    Ok(())
}

/// Print a status message to **stderr** so it never pollutes stdout output.
///
/// Use this for progress indicators, "Deploying foo...", etc. Suppressed
/// automatically when `--quiet` is in effect (future).
#[macro_export]
macro_rules! status {
    ($($arg:tt)*) => {{
        eprintln!($($arg)*);
    }};
}
