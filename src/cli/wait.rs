//! Async request polling.
//!
//! Many Xnode Manager operations are asynchronous: the manager responds with
//! a `request_id` immediately, then runs the heavy work (typically
//! `nixos-rebuild switch`) in the background. The manager exposes
//! `GET /request/{id}/info` so callers can poll for completion.
//!
//! This module provides one helper, [`wait_for_request`], that:
//!   - polls with exponential backoff (capped at 5s)
//!   - prints status to stderr (so JSON output stays clean)
//!   - times out after a configurable duration
//!   - returns the final `RequestInfo` so the caller can render the result

use crate::cli::error::{CliError, CliResult, ErrorCode};
use crate::sdk::{self, utils::Session};
use std::time::{Duration, Instant};

/// Default timeout for `om req wait` and the `--wait` flag on state-changing
/// commands. NixOS rebuilds can be slow on first boot of a flake, so we allow
/// 10 minutes by default.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Poll a request until it terminates (success or error) or the timeout
/// expires. Returns the final `RequestInfo`.
pub async fn wait_for_request(
    session: &Session,
    request_id: u32,
    timeout_secs: u64,
) -> CliResult<sdk::request::RequestInfo> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut delay_ms: u64 = 250;
    let max_delay_ms: u64 = 5_000;

    crate::status!("waiting for request {}...", request_id);

    loop {
        let input = sdk::request::RequestInfoInput::new_with_path(
            session,
            sdk::request::RequestInfoPath { request_id },
        );

        match sdk::request::request_info(input).await {
            Ok(info) => {
                if info.result.is_some() {
                    // Terminal state.
                    match &info.result {
                        Some(sdk::request::RequestIdResult::Success { .. }) => {
                            crate::status!("request {} completed successfully", request_id);
                        }
                        Some(sdk::request::RequestIdResult::Error { error }) => {
                            return Err(CliError::new(
                                ErrorCode::Internal,
                                format!("request {} failed: {}", request_id, error),
                            ));
                        }
                        None => unreachable!(),
                    }
                    return Ok(info);
                }
                // Still running.
                crate::status!(
                    "  ...still running ({} commands)",
                    info.commands.len()
                );
            }
            Err(e) => {
                // Transient errors are common during a rebuild (the manager
                // briefly returns 502 while nginx restarts). We tolerate them
                // until the timeout fires.
                crate::status!("  poll error: {} (will retry)", e);
            }
        }

        if Instant::now() >= deadline {
            return Err(CliError::new(
                ErrorCode::Timeout,
                format!(
                    "request {} did not complete within {}s",
                    request_id, timeout_secs
                ),
            )
            .with_hint("Run `om req show <id>` to check status manually, or pass --timeout <seconds>"));
        }

        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        delay_ms = (delay_ms * 2).min(max_delay_ms);
    }
}
