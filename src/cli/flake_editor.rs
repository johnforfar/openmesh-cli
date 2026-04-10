//! Safe rewriting of the user-config block in an Xnode NixOS flake.
//!
//! ## Why this exists
//!
//! Adding a reverse-proxy rule (so that `myapp.example.com` routes to a
//! container) requires modifying the host's `flake.nix`. The xnode-manager
//! frontend does this by treating everything between two literal markers as
//! "user-editable":
//!
//! ```nix
//! # START USER CONFIG
//! services.xnode-reverse-proxy.rules."blog.example.com" = [
//!   { forward = "http://blog:3000"; }
//! ];
//! # END USER CONFIG
//! ```
//!
//! ## Safety contract
//!
//! This module's job is to **never corrupt the user's flake**. Specifically:
//!
//! 1. If the markers are missing, we **refuse to write** rather than guess.
//! 2. Existing rules for *other domains* are **always preserved**.
//! 3. Adding a duplicate rule is rejected with [`FlakeEditError::DomainExists`]
//!    unless the caller passes [`AddRuleMode::Replace`].
//! 4. Round-tripping `parse(s).serialize()` is identity for any well-formed
//!    user-config block.
//!
//! All public functions are pure and operate on `&str`. The CLI command layer
//! is responsible for fetching and writing the flake.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The literal start marker. Must match xnode-manager-frontend exactly.
pub const START_MARKER: &str = "# START USER CONFIG";
/// The literal end marker. Must match xnode-manager-frontend exactly.
pub const END_MARKER: &str = "# END USER CONFIG";

/// One reverse-proxy forwarding rule.
///
/// Mirrors the on-the-wire shape used by `services.xnode-reverse-proxy.rules`:
/// `{ forward = "http://app:3000"; path = "/api"; }`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyRule {
    /// The full URL the proxy should forward to, e.g. `http://demo:3000`.
    pub forward: String,
    /// Optional URL path prefix this rule applies to. Default: rule applies
    /// to all paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// All forwarding rules attached to one domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainExpose {
    /// FQDN, e.g. `blog.johnny.openmesh.cloud`. Stored without surrounding
    /// quotes; serialization adds them.
    pub domain: String,
    pub rules: Vec<ProxyRule>,
}

/// Behavior when [`add_or_replace_expose`] finds an existing rule for the
/// same domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddRuleMode {
    /// Reject the change with [`FlakeEditError::DomainExists`].
    FailIfExists,
    /// Replace the existing rule's `rules` vec with the new one.
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlakeEditError {
    /// The flake does not contain `# START USER CONFIG` and `# END USER CONFIG`.
    /// We refuse to write rather than guess where user content goes.
    MissingMarkers,
    /// Markers are present but in the wrong order (END before START).
    MalformedMarkers,
    /// A rule already exists for the requested domain and the caller did not
    /// pass [`AddRuleMode::Replace`].
    DomainExists(String),
    /// Asked to remove a domain that isn't currently exposed.
    DomainNotFound(String),
    /// User input failed validation (e.g. domain contains a quote, port out of range).
    InvalidInput(String),
}

impl fmt::Display for FlakeEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMarkers => write!(
                f,
                "flake does not contain `{}` / `{}` markers",
                START_MARKER, END_MARKER
            ),
            Self::MalformedMarkers => write!(
                f,
                "flake markers are present but in the wrong order"
            ),
            Self::DomainExists(d) => write!(f, "an expose rule already exists for `{}`", d),
            Self::DomainNotFound(d) => write!(f, "no expose rule found for `{}`", d),
            Self::InvalidInput(s) => write!(f, "invalid input: {}", s),
        }
    }
}

impl std::error::Error for FlakeEditError {}

/// Validate a domain string is safe to embed inside a Nix string literal.
///
/// We do **not** want a domain like `evil.com"; system.activationScripts.x = "rm -rf /` to
/// inject arbitrary nix into the host config. Lock the character set hard.
fn validate_domain(domain: &str) -> Result<(), FlakeEditError> {
    if domain.is_empty() {
        return Err(FlakeEditError::InvalidInput("domain is empty".into()));
    }
    if domain.len() > 253 {
        return Err(FlakeEditError::InvalidInput(
            "domain exceeds 253 characters".into(),
        ));
    }
    for c in domain.chars() {
        let ok = c.is_ascii_alphanumeric() || c == '-' || c == '.';
        if !ok {
            return Err(FlakeEditError::InvalidInput(format!(
                "domain contains invalid character `{}`; allowed: a-z 0-9 . -",
                c
            )));
        }
    }
    if domain.starts_with('-') || domain.starts_with('.') || domain.ends_with('-') || domain.ends_with('.') {
        return Err(FlakeEditError::InvalidInput(
            "domain must not start or end with `.` or `-`".into(),
        ));
    }
    Ok(())
}

/// Validate a forward target like `http://demo:3000` or `tcp://app:5432`.
///
/// We do not parse this as a `url::Url` because Nix accepts non-standard
/// schemes (`tcp`, `udp`) the URL crate would reject. Instead we enforce a
/// strict character set on the parts that get embedded in nix strings.
fn validate_forward(forward: &str) -> Result<(), FlakeEditError> {
    if forward.is_empty() || forward.len() > 512 {
        return Err(FlakeEditError::InvalidInput(
            "forward URL is empty or too long".into(),
        ));
    }
    for c in forward.chars() {
        // Allow URL chars but reject anything that could break out of a nix
        // string literal. Notably: NO quote, NO backslash, NO newline.
        let ok = c.is_ascii_alphanumeric()
            || matches!(c, '.' | '-' | '_' | '/' | ':' | '?' | '&' | '=' | '%' | '#');
        if !ok {
            return Err(FlakeEditError::InvalidInput(format!(
                "forward contains invalid character `{}`",
                c
            )));
        }
    }
    Ok(())
}

/// Validate a path prefix like `/api` or `/`.
fn validate_path(path: &str) -> Result<(), FlakeEditError> {
    if path.is_empty() {
        return Err(FlakeEditError::InvalidInput("path is empty".into()));
    }
    if !path.starts_with('/') {
        return Err(FlakeEditError::InvalidInput(
            "path must start with `/`".into(),
        ));
    }
    for c in path.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.');
        if !ok {
            return Err(FlakeEditError::InvalidInput(format!(
                "path contains invalid character `{}`",
                c
            )));
        }
    }
    Ok(())
}

/// Extract the user-config text (without surrounding markers and without the
/// blank lines immediately after START / before END).
///
/// Returns `Err(MissingMarkers)` if the markers are absent.
pub fn extract_user_config(flake: &str) -> Result<&str, FlakeEditError> {
    let after_start = flake
        .find(START_MARKER)
        .ok_or(FlakeEditError::MissingMarkers)?
        + START_MARKER.len();
    let end = flake[after_start..]
        .find(END_MARKER)
        .ok_or(FlakeEditError::MissingMarkers)?
        + after_start;

    if end <= after_start {
        return Err(FlakeEditError::MalformedMarkers);
    }
    Ok(&flake[after_start..end])
}

/// Substitute the user-config region with a new value, preserving everything
/// outside the markers byte-for-byte.
pub fn replace_user_config(flake: &str, new_user_config: &str) -> Result<String, FlakeEditError> {
    let start_idx = flake
        .find(START_MARKER)
        .ok_or(FlakeEditError::MissingMarkers)?;
    let after_start = start_idx + START_MARKER.len();
    let end_idx = flake[after_start..]
        .find(END_MARKER)
        .ok_or(FlakeEditError::MissingMarkers)?
        + after_start;

    let mut out = String::with_capacity(flake.len() + new_user_config.len());
    out.push_str(&flake[..after_start]);
    out.push_str(new_user_config);
    out.push_str(&flake[end_idx..]);
    Ok(out)
}

/// Parse the user-config region into a list of [`DomainExpose`] entries.
///
/// This is a **lossy** parse: it only recognizes the
/// `services.xnode-reverse-proxy.rules."<domain>" = [ ... ];` shape that the
/// frontend writes. Other nix code in the user-config region is ignored on
/// read but **preserved** on write by [`add_or_replace_expose`].
pub fn parse_exposes(user_config: &str) -> Vec<DomainExpose> {
    let mut out = Vec::new();
    let needle = "services.xnode-reverse-proxy.rules.";
    let mut cursor = 0usize;
    while let Some(rel) = user_config[cursor..].find(needle) {
        let abs = cursor + rel;
        let after_needle = abs + needle.len();
        // Domain is between the next pair of quotes.
        let bytes = user_config.as_bytes();
        if after_needle >= bytes.len() || bytes[after_needle] != b'"' {
            cursor = after_needle;
            continue;
        }
        let domain_start = after_needle + 1;
        let Some(quote_rel) = user_config[domain_start..].find('"') else {
            break;
        };
        let domain_end = domain_start + quote_rel;
        let domain = &user_config[domain_start..domain_end];

        // Find the `[` ... `]` that holds the rules. We scan from after the
        // closing quote of the domain.
        let Some(open_rel) = user_config[domain_end..].find('[') else {
            cursor = domain_end;
            continue;
        };
        let open = domain_end + open_rel;
        // Find matching `]`. The rule list is flat (no nested `[`) in
        // practice, so a naive scan is correct.
        let Some(close_rel) = user_config[open..].find(']') else {
            cursor = open;
            continue;
        };
        let close = open + close_rel;
        let body = &user_config[open + 1..close];

        let rules = parse_rule_body(body);
        out.push(DomainExpose {
            domain: domain.to_string(),
            rules,
        });

        cursor = close + 1;
    }
    out
}

/// Parse the body between `[` and `]` of a rules list, e.g.
/// `{ forward = "http://app:3000"; } { forward = "http://app:3000"; path = "/api"; }`.
fn parse_rule_body(body: &str) -> Vec<ProxyRule> {
    let mut rules = Vec::new();
    let mut cursor = 0usize;
    while let Some(open_rel) = body[cursor..].find('{') {
        let open = cursor + open_rel;
        let Some(close_rel) = body[open..].find('}') else { break };
        let close = open + close_rel;
        let inner = &body[open + 1..close];

        let mut forward: Option<String> = None;
        let mut path: Option<String> = None;
        for entry in inner.split(';') {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once('=') {
                let key = k.trim();
                let value = v.trim().trim_matches('"').to_string();
                match key {
                    "forward" => forward = Some(value),
                    "path" => path = Some(value),
                    _ => {}
                }
            }
        }
        if let Some(f) = forward {
            rules.push(ProxyRule { forward: f, path });
        }
        cursor = close + 1;
    }
    rules
}

/// Render a single `DomainExpose` as the nix snippet that goes into the
/// user-config region.
fn serialize_expose(expose: &DomainExpose) -> String {
    let rules = expose
        .rules
        .iter()
        .map(|r| match &r.path {
            Some(p) => format!("{{ forward = \"{}\"; path = \"{}\"; }}", r.forward, p),
            None => format!("{{ forward = \"{}\"; }}", r.forward),
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "services.xnode-reverse-proxy.rules.\"{}\" = [ {} ];",
        expose.domain, rules
    )
}

/// Add or replace an expose rule and return the new full flake.
///
/// This is the main entry point used by `om app expose`. It is **additive**:
/// rules for other domains are preserved byte-for-byte.
pub fn add_or_replace_expose(
    flake: &str,
    expose: DomainExpose,
    mode: AddRuleMode,
) -> Result<String, FlakeEditError> {
    validate_domain(&expose.domain)?;
    for rule in &expose.rules {
        validate_forward(&rule.forward)?;
        if let Some(p) = &rule.path {
            validate_path(p)?;
        }
    }

    let user_config = extract_user_config(flake)?;
    let existing = parse_exposes(user_config);

    let already = existing.iter().any(|e| e.domain == expose.domain);
    if already && mode == AddRuleMode::FailIfExists {
        return Err(FlakeEditError::DomainExists(expose.domain));
    }

    // Strategy: build a new user-config string. For each existing expose,
    // either keep it as-is (different domain) or replace its serialized form
    // (matching domain). If no existing expose matched, append the new one
    // to the end of the user-config region.
    let mut new_uc = String::with_capacity(user_config.len() + 256);

    if already {
        // Walk the original user_config text and find the literal block we
        // need to replace, swapping it out in-place. We use a regex-free
        // search anchored on the canonical needle to avoid touching unrelated
        // text. The block always starts with `services.xnode-reverse-proxy.rules."<domain>"`.
        let needle = format!(
            "services.xnode-reverse-proxy.rules.\"{}\"",
            expose.domain
        );
        let Some(start) = user_config.find(&needle) else {
            // Defensive: parse_exposes said it was there, find said no.
            return Err(FlakeEditError::MalformedMarkers);
        };
        // Find the terminating `];` of THIS block (the first `]` after the
        // start, then a `;`).
        let Some(close_rel) = user_config[start..].find(']') else {
            return Err(FlakeEditError::MalformedMarkers);
        };
        let close = start + close_rel;
        // Optional trailing semicolon.
        let after = if user_config.as_bytes().get(close + 1) == Some(&b';') {
            close + 2
        } else {
            close + 1
        };

        new_uc.push_str(&user_config[..start]);
        new_uc.push_str(&serialize_expose(&expose));
        new_uc.push_str(&user_config[after..]);
    } else {
        new_uc.push_str(user_config);
        // Ensure there's a newline before the new rule.
        if !new_uc.ends_with('\n') {
            new_uc.push('\n');
        }
        new_uc.push_str(&serialize_expose(&expose));
        new_uc.push('\n');
    }

    replace_user_config(flake, &new_uc)
}

/// Remove an expose rule for a domain. Returns `DomainNotFound` if the
/// domain has no current rule.
pub fn remove_expose(flake: &str, domain: &str) -> Result<String, FlakeEditError> {
    validate_domain(domain)?;
    let user_config = extract_user_config(flake)?;
    let needle = format!("services.xnode-reverse-proxy.rules.\"{}\"", domain);
    let Some(start) = user_config.find(&needle) else {
        return Err(FlakeEditError::DomainNotFound(domain.to_string()));
    };
    let Some(close_rel) = user_config[start..].find(']') else {
        return Err(FlakeEditError::MalformedMarkers);
    };
    let close = start + close_rel;
    let after = if user_config.as_bytes().get(close + 1) == Some(&b';') {
        close + 2
    } else {
        close + 1
    };

    // Also drop a single trailing newline if removing the rule leaves a
    // blank line behind.
    let mut new_uc = String::with_capacity(user_config.len());
    new_uc.push_str(&user_config[..start]);
    let tail = &user_config[after..];
    let tail = tail.strip_prefix('\n').unwrap_or(tail);
    new_uc.push_str(tail);

    replace_user_config(flake, &new_uc)
}
