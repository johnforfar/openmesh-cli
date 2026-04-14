//! `om status` — one-page dashboard banner with xnode specs, resource
//! utilisation, and container list.
//!
//! Renders a boxed ANSI-coloured banner that gives an at-a-glance view
//! of the xnode you're currently logged into. Fetches:
//!
//! - host CPU/memory/disk usage via `/usage/host/*`
//! - container list via `/config/containers`
//! - per-container running state + primary service description via
//!   `/process/container:<name>/list`
//!
//! All per-container fetches run in parallel via `futures::join_all`,
//! so the full render is bounded by the slowest single container query.
//! On a healthy xnode with 14 containers this is ~300ms end-to-end.
//!
//! Flags:
//!   --watch            re-render every N seconds until Ctrl-C
//!   --interval <secs>  watch interval (default 3)
//!   --no-color         disable ANSI colour (auto-off if stdout isn't a tty)

use crate::cli::context::require_session;
use crate::cli::error::CliResult;
use crate::cli::output::{OutputFormat, Renderable, render};
use crate::sdk;
use futures::future::join_all;
use serde::Serialize;
use std::io::{IsTerminal, Write};

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

pub async fn run(
    watch: bool,
    interval: u64,
    no_color: bool,
    format: OutputFormat,
) -> CliResult<()> {
    // Color is opt-out, and we also auto-disable if stdout isn't a tty
    // (piped to a file or another command) to keep ANSI junk out of logs.
    let color = !no_color && std::io::stdout().is_terminal() && matches!(format, OutputFormat::Plain);

    if !watch {
        let view = collect(color).await?;
        render(&view, format)?;
        return Ok(());
    }

    // Watch loop. Only meaningful in plain mode — json + watch is silly so
    // we still honour it, just by re-emitting the JSON object every tick.
    let interval = interval.max(1);
    loop {
        if matches!(format, OutputFormat::Plain) {
            // Clear screen + move cursor home — standard ANSI sequence.
            print!("\x1b[2J\x1b[H");
            std::io::stdout().flush().ok();
        }
        let view = collect(color).await?;
        render(&view, format)?;
        if matches!(format, OutputFormat::Plain) {
            println!();
            println!(
                "  {}refreshing every {}s — press Ctrl-C to exit{}",
                if color { "\x1b[2m" } else { "" },
                interval,
                if color { "\x1b[0m" } else { "" }
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
    }
}

// ---------------------------------------------------------------------------
// data collection
// ---------------------------------------------------------------------------

async fn collect(color: bool) -> CliResult<StatusView> {
    let session = require_session()?;

    // Fire all top-level fetches in parallel.
    let mem_fut = sdk::usage::memory(sdk::usage::MemoryInput::new_with_path(
        &session,
        sdk::usage::MemoryPath { scope: "host".to_string() },
    ));
    let cpu_fut = sdk::usage::cpu(sdk::usage::CpuInput::new_with_path(
        &session,
        sdk::usage::CpuPath { scope: "host".to_string() },
    ));
    let disk_fut = sdk::usage::disk(sdk::usage::DiskInput::new_with_path(
        &session,
        sdk::usage::DiskPath { scope: "host".to_string() },
    ));
    let containers_fut = sdk::config::containers(sdk::config::ContainersInput::new(&session));

    let (mem_res, cpus_res, disks_res, containers_res) =
        futures::join!(mem_fut, cpu_fut, disk_fut, containers_fut);

    let mem = mem_res.ok();
    let cpus = cpus_res.ok().unwrap_or_default();
    let disks = disks_res.ok().unwrap_or_default();
    let containers = containers_res.unwrap_or_default();

    // For every container, fetch its process list in parallel — bounded
    // wait is the slowest container query rather than N × slowest.
    // We borrow `session` into each future so we only build one client.
    let list_futs = containers.iter().map(|name| {
        let scope = format!("container:{name}");
        let session_ref = &session;
        async move {
            let input = sdk::process::ListInput::new_with_path(
                session_ref,
                sdk::process::ListPath { scope },
            );
            (name.clone(), sdk::process::list(input).await)
        }
    });
    let per_container = join_all(list_futs).await;

    let mut container_states = Vec::with_capacity(per_container.len());
    for (name, res) in per_container {
        match res {
            Ok(units) => {
                let running = units.iter().any(|p| p.running);
                let description = primary_description(&name, &units);
                container_states.push(ContainerState { name, running, description });
            }
            Err(_) => container_states.push(ContainerState {
                name,
                running: false,
                description: None,
            }),
        }
    }

    // Largest disk is usually the root / or main data mount. Xnodes
    // typically have a single backing disk so this is a no-op most of
    // the time.
    let disk = disks.iter().max_by_key(|d| d.total).cloned();

    let cpu_avg = if cpus.is_empty() {
        0.0
    } else {
        cpus.iter().map(|c| c.used).sum::<f32>() / cpus.len() as f32
    };

    // Per-CPU bars are only shown when the distribution is interesting
    // (at least one core >50% usage, which implies an unbalanced load
    // that the aggregate would hide).
    let show_per_cpu = cpus.iter().any(|c| c.used > 50.0);

    let identity = extract_identity(&session.cookies);

    Ok(StatusView {
        manager_url: session.base_url.clone(),
        domain: session.domain.clone(),
        identity,
        cpu_percent: cpu_avg,
        cpu_count: cpus.len(),
        cpus: cpus
            .iter()
            .map(|c| CpuCore { used: c.used, frequency_mhz: c.frequency / 1_000_000 })
            .collect(),
        show_per_cpu,
        mem_used: mem.as_ref().map(|m| m.used).unwrap_or(0),
        mem_total: mem.as_ref().map(|m| m.total).unwrap_or(0),
        disk_used: disk.as_ref().map(|d| d.used).unwrap_or(0),
        disk_total: disk.as_ref().map(|d| d.total).unwrap_or(0),
        disk_mount: disk.as_ref().map(|d| d.mount_point.clone()).unwrap_or_default(),
        containers: container_states,
        color,
    })
}

/// Given the systemd unit list from inside a container, pick the
/// "primary" description to show next to the container name.
///
/// Heuristic, in preference order:
/// 1. Exact match on `<container-name>.service` — this hits for bots
///    and dashboards where we named the unit after the container.
/// 2. First non-system unit we find (skips systemd-*, dbus, dhcpcd, etc.)
/// 3. None — leave it blank, better than lying.
fn primary_description(
    container: &str,
    units: &[sdk::process::Process],
) -> Option<String> {
    let exact = format!("{container}.service");
    if let Some(u) = units.iter().find(|u| u.name == exact) {
        return u.description.clone();
    }
    // Fall back to the first non-system user service we find.
    let skip_prefixes = [
        "systemd-",
        "dbus",
        "dhcpcd",
        "nscd",
        "avahi",
        "user@",
        "init.scope",
    ];
    units
        .iter()
        .filter(|u| u.running)
        .filter(|u| {
            !skip_prefixes
                .iter()
                .any(|p| u.name.starts_with(p) || u.name == *p)
        })
        .find_map(|u| u.description.clone())
}

/// Pull `eth:0xabc…` out of the xnode_auth_user cookie so we can show
/// it on the banner. Returns None if the cookie isn't found or doesn't
/// parse the way we expect.
///
/// The cookie value is typically URL-encoded (`eth%3A6d9b...`), so we
/// decode the `%3A` → `:` before matching.
fn extract_identity(cookies: &[String]) -> Option<String> {
    for c in cookies {
        if let Some(raw) = c.strip_prefix("xnode_auth_user=") {
            // URL-decode the colon, which is the only special character
            // we expect in a valid eth:<addr> identity.
            let decoded = raw.replace("%3A", ":").replace("%3a", ":");
            if let Some(rest) = decoded.strip_prefix("eth:") {
                if rest.len() > 12 {
                    return Some(format!("eth:{}…{}", &rest[..6], &rest[rest.len() - 4..]));
                }
                return Some(format!("eth:{rest}"));
            }
            return Some(decoded);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// view
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct ContainerState {
    name: String,
    running: bool,
    description: Option<String>,
}

#[derive(Serialize, Clone)]
struct CpuCore {
    used: f32,
    frequency_mhz: u64,
}

#[derive(Serialize)]
struct StatusView {
    manager_url: String,
    domain: String,
    identity: Option<String>,
    cpu_percent: f32,
    cpu_count: usize,
    cpus: Vec<CpuCore>,
    show_per_cpu: bool,
    mem_used: u64,
    mem_total: u64,
    disk_used: u64,
    disk_total: u64,
    disk_mount: String,
    containers: Vec<ContainerState>,
    #[serde(skip)]
    color: bool,
}

// ---------------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------------

const INNER: usize = 64;
const BAR: usize = 22;

// ANSI escape helpers. Everything routes through `c(s, code)` so when
// `color` is false we emit plain text with zero escape bytes.
fn c(s: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
fn bold(s: &str, e: bool) -> String { c(s, "1", e) }
fn dim(s: &str, e: bool) -> String { c(s, "2", e) }
fn green(s: &str, e: bool) -> String { c(s, "32", e) }
fn yellow(s: &str, e: bool) -> String { c(s, "33", e) }
fn red(s: &str, e: bool) -> String { c(s, "31", e) }
fn cyan(s: &str, e: bool) -> String { c(s, "36", e) }

fn color_for_pct(pct: f32) -> &'static str {
    if pct < 50.0 {
        "32" // green
    } else if pct < 80.0 {
        "33" // yellow
    } else {
        "31" // red
    }
}

fn bar(pct: f32, enabled: bool) -> String {
    let filled = ((pct / 100.0) * BAR as f32).round() as usize;
    let filled = filled.min(BAR);
    let empty = BAR - filled;
    let filled_str = "█".repeat(filled);
    let empty_str = "░".repeat(empty);
    if enabled {
        format!(
            "\x1b[{}m{}\x1b[0m\x1b[2m{}\x1b[0m",
            color_for_pct(pct),
            filled_str,
            empty_str
        )
    } else {
        format!("{filled_str}{empty_str}")
    }
}

fn fmt_gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1024.0 / 1024.0 / 1024.0)
}

/// Pad a line to exactly `INNER` *display* columns. Strips ANSI escapes
/// before counting so coloured lines still line up with the box border.
fn pad(s: &str) -> String {
    let stripped = strip_ansi(s);
    let width = stripped.chars().count();
    if width >= INNER {
        // If the line is too wide, trim the stripped form and hope it
        // didn't cut inside an escape sequence. Our callers always stay
        // under 64 chars so this branch is defensive.
        stripped.chars().take(INNER).collect()
    } else {
        format!("{}{}", s, " ".repeat(INNER - width))
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume [
            // consume until an ASCII letter (end of CSI)
            while let Some(&c) = chars.peek() {
                chars.next();
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

impl Renderable for StatusView {
    fn render_plain(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let e = self.color;
        let top = format!("╔{}╗", "═".repeat(INNER));
        let sep = format!("╠{}╣", "═".repeat(INNER));
        let bot = format!("╚{}╝", "═".repeat(INNER));
        let border = |s: &str| cyan(s, e);

        writeln!(w, "{}", border(&top))?;

        // ── Om CLI word-mark + tagline ─────────────────────────────────
        let om_art = [
            "  ██████  ███    ███ ",
            " ██    ██ ████  ████ ",
            " ██    ██ ██ ████ ██ ",
            " ██    ██ ██  ██  ██ ",
            "  ██████  ██      ██ ",
        ];
        let tagline = [
            "",
            "   openmesh cli     ",
            "   ─────────────    ",
            "   xnode operator   ",
            "",
        ];
        for i in 0..5 {
            let art_colored = cyan(om_art[i], e);
            let tag_colored = dim(tagline[i], e);
            let line = format!("  {art_colored}{tag_colored}");
            writeln!(w, "{}{}{}", border("║"), pad(&line), border("║"))?;
        }
        writeln!(w, "{}", border(&sep))?;

        // ── Header row: xnode domain + identity ────────────────────────
        let header_label = dim("Xnode ──", e);
        let header_val = bold(&self.domain, e);
        let header_line = format!("  {header_label} {header_val}");
        writeln!(w, "{}{}{}", border("║"), pad(&header_line), border("║"))?;

        if let Some(id) = &self.identity {
            let id_label = dim("User  ──", e);
            let id_val = dim(id, e);
            let id_line = format!("  {id_label} {id_val}");
            writeln!(w, "{}{}{}", border("║"), pad(&id_line), border("║"))?;
        }
        writeln!(w, "{}", border(&sep))?;

        // ── Resources ──────────────────────────────────────────────────
        let cpu_pct_txt = format!("{:>4.1}%", self.cpu_percent);
        let cpu_pct_col = match color_for_pct(self.cpu_percent) {
            "31" => red(&cpu_pct_txt, e),
            "33" => yellow(&cpu_pct_txt, e),
            _ => green(&cpu_pct_txt, e),
        };
        let cpu_line = format!(
            "  {:<7} {}  {}    ({} vCPU)",
            bold("CPU", e),
            bar(self.cpu_percent, e),
            cpu_pct_col,
            self.cpu_count
        );
        writeln!(w, "{}{}{}", border("║"), pad(&cpu_line), border("║"))?;

        // Optional per-core breakdown when load is interesting
        if self.show_per_cpu && !self.cpus.is_empty() {
            for (i, core) in self.cpus.iter().enumerate() {
                let bar_small_w = 10usize;
                let filled = ((core.used / 100.0) * bar_small_w as f32).round() as usize;
                let filled = filled.min(bar_small_w);
                let b_filled = "█".repeat(filled);
                let b_empty = "░".repeat(bar_small_w - filled);
                let b = if e {
                    format!(
                        "\x1b[{}m{}\x1b[0m\x1b[2m{}\x1b[0m",
                        color_for_pct(core.used),
                        b_filled,
                        b_empty
                    )
                } else {
                    format!("{b_filled}{b_empty}")
                };
                let core_line = format!(
                    "    {} cpu{:<2} {}  {:>4.1}%",
                    dim("·", e),
                    i,
                    b,
                    core.used
                );
                writeln!(w, "{}{}{}", border("║"), pad(&core_line), border("║"))?;
            }
        }

        let mem_pct = if self.mem_total == 0 {
            0.0
        } else {
            (self.mem_used as f32 / self.mem_total as f32) * 100.0
        };
        let mem_pct_txt = format!("{:>4.1}%", mem_pct);
        let mem_pct_col = match color_for_pct(mem_pct) {
            "31" => red(&mem_pct_txt, e),
            "33" => yellow(&mem_pct_txt, e),
            _ => green(&mem_pct_txt, e),
        };
        let mem_line = format!(
            "  {:<7} {}  {}    ({} / {} GB)",
            bold("Memory", e),
            bar(mem_pct, e),
            mem_pct_col,
            fmt_gb(self.mem_used),
            fmt_gb(self.mem_total)
        );
        writeln!(w, "{}{}{}", border("║"), pad(&mem_line), border("║"))?;

        let disk_pct = if self.disk_total == 0 {
            0.0
        } else {
            (self.disk_used as f32 / self.disk_total as f32) * 100.0
        };
        let disk_pct_txt = format!("{:>4.1}%", disk_pct);
        let disk_pct_col = match color_for_pct(disk_pct) {
            "31" => red(&disk_pct_txt, e),
            "33" => yellow(&disk_pct_txt, e),
            _ => green(&disk_pct_txt, e),
        };
        let disk_line = format!(
            "  {:<7} {}  {}    ({} / {} GB)",
            bold("Disk", e),
            bar(disk_pct, e),
            disk_pct_col,
            fmt_gb(self.disk_used),
            fmt_gb(self.disk_total)
        );
        writeln!(w, "{}{}{}", border("║"), pad(&disk_line), border("║"))?;

        writeln!(w, "{}", border(&sep))?;

        // ── Containers ─────────────────────────────────────────────────
        let running = self.containers.iter().filter(|c| c.running).count();
        let stopped = self.containers.len() - running;
        let stopped_part = if stopped > 0 {
            format!(" / {} stopped", red(&stopped.to_string(), e))
        } else {
            String::new()
        };
        let container_header = format!(
            "  {} ({} running{stopped_part} / {} total)",
            bold("CONTAINERS", e),
            green(&running.to_string(), e),
            self.containers.len()
        );
        writeln!(w, "{}{}{}", border("║"), pad(&container_header), border("║"))?;

        if self.containers.is_empty() {
            let line = format!("  {}", dim("(none deployed)", e));
            writeln!(w, "{}{}{}", border("║"), pad(&line), border("║"))?;
        } else {
            // Sort by running-first, then alphabetical — makes the running
            // block visually cohesive at the top.
            let mut sorted: Vec<&ContainerState> = self.containers.iter().collect();
            sorted.sort_by(|a, b| {
                b.running
                    .cmp(&a.running)
                    .then_with(|| a.name.cmp(&b.name))
            });

            // Compute name column width so descriptions align.
            let name_col = sorted
                .iter()
                .map(|c| c.name.chars().count())
                .max()
                .unwrap_or(0)
                .min(22); // clamp so long names don't blow out the layout

            for c_state in sorted {
                let glyph = if c_state.running {
                    green("●", e)
                } else {
                    red("○", e)
                };
                let name_str = &c_state.name;
                let desc_opt = c_state
                    .description
                    .as_deref()
                    .filter(|s| !s.is_empty());

                let name_part = if name_str.chars().count() < name_col {
                    format!("{}{}", name_str, " ".repeat(name_col - name_str.chars().count()))
                } else {
                    name_str.to_string()
                };

                let line = if let Some(desc) = desc_opt {
                    // Budget for description = INNER − 2 (left pad) − 2 (glyph) − name_col − 2 (gap)
                    let budget = INNER.saturating_sub(2 + 2 + name_col + 2);
                    let desc_chars = desc.chars().count();
                    let desc_trimmed: String = if desc_chars > budget {
                        // Reserve 1 char for an ellipsis so it reads as truncated.
                        let take = budget.saturating_sub(1);
                        let mut s: String = desc.chars().take(take).collect();
                        s.push('…');
                        s
                    } else {
                        desc.to_string()
                    };
                    format!("  {} {}  {}", glyph, name_part, dim(&desc_trimmed, e))
                } else {
                    format!("  {} {}", glyph, name_part)
                };
                writeln!(w, "{}{}{}", border("║"), pad(&line), border("║"))?;
            }
        }

        writeln!(w, "{}", border(&bot))?;
        Ok(())
    }
}
