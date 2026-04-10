# Open Source Fixes Needed Upstream

This file is a numbered, append-only log of issues that need to be fixed in
upstream Openmesh repositories (`Openmesh-Network/*`, `OpenxAI-Network/*`).
Each fix gets a number that never changes, even after the fix lands. The log
exists so future contributors can read the history of *why* `om` works around
something rather than fixing it cleanly.

**Editing rules:**
- New fixes get the next number and appended at the bottom
- Closed fixes get a `**Status: closed**` line but stay in the file
- Don't renumber, don't reorder, don't delete

---

## Fix #1 — `xnode-manager` nginx rejects valid Rust HTTP clients

**Status:** open
**Affects:** `Openmesh-Network/xnode-manager`, `Openmesh-Network/xnode-manager-sdk`
**Discovered:** 2026-02 (initial CLI development)
**Reproduces against:** every Xnode running the current xnode-manager image

### Symptom

`reqwest::Client` requests against the manager always return
`400 Bad Request` from nginx. System curl with the exact same cookie jar and
headers succeeds.

Reproduction lives in `tests/test_suite.rs` of this repo:

| Test | Path | Result |
|---|---|---|
| `live_usage_memory` | reqwest path | ❌ 400 |
| `curl_live_memory`  | curl path    | ✅ 200 |

Both call the same endpoint with the same session cookies and the same
proxy headers. The only difference is the HTTP client.

### Impact

Every non-browser HTTP client in the ecosystem is blocked. TypeScript with
`node-fetch`/`undici`, Python with `requests`/`httpx`, Go with `net/http`,
Rust with `reqwest` — all likely affected. This forces every SDK and tool
to shell out to system `curl`, which is fragile and platform-specific.

### Likely root cause

We have not isolated the trigger. Hypotheses worth investigating, in order
of decreasing plausibility:

1. **Duplicate `Host` header.** `reqwest` sets one automatically from the
   URL; the SDK also explicitly sets one in `Session::load`. The duplicate
   may be confusing nginx's `if` rules.
2. **TLS fingerprint.** `rustls` and `node-tls` both produce different
   ClientHello extensions/cipher orderings than libcurl. nginx may be
   running a TLS fingerprint check (or sitting behind something that does).
3. **HTTP/2 frame normalization.** `reqwest` defaults to HTTP/2; if the
   nginx config has HTTP/2 disabled and the redirect is brittle, this
   could 400.
4. **Header ordering / casing.** Some nginx configs reject requests with
   non-canonical header order.

### Workaround currently in place (in this fork)

`src/sdk/utils/session.rs` has a `force_curl()` env-var helper and both
`session_get` and `session_post` will fall through to a `std::process::Command::new("curl")`
shell-out when reqwest fails. Setting `OM_FORCE_CURL=1` skips the doomed
reqwest attempt entirely and saves an RTT per call.

This works but is **not a fix** — it just hides the upstream problem under
a `Command::new("curl")` call. A proper fix would let real HTTP clients
talk to the manager.

### Suggested fix (upstream)

Three options for the xnode-manager maintainers:

1. **Strip the duplicate `Host` header in nginx** (or document the
   requirement clearly so SDK authors know not to set it explicitly)
2. **Disable any TLS fingerprint check** if one is in place — these
   inevitably break legitimate clients
3. **Publish a minimal known-good request example** (curl invocation
   with full header set) so SDK authors have a target to match

### Related upstream work needed

- `Openmesh-Network/xnode-manager-sdk` — the SDK in this repo presumably
  has the same broken `session_post` curl fallback as my fork did before
  fix 2 below. The fix from my fork can be ported as a PR (with John's
  approval per the no-remote-changes rule).

---

## Fix #2 — `xnode-manager-sdk` `session_post` curl fallback was unimplemented

**Status:** worked around in this fork — upstream fix not yet sent
**Affects:** `Openmesh-Network/xnode-manager-sdk`
**Discovered:** 2026-04-10
**Fixed-here-locally:** yes (commit on `johnforfar/openmesh-cli` main)

### Symptom

In the upstream `xnode-manager-sdk` Rust crate, `session_post` returned a
hardcoded `"Post fallback not fully implemented for generic data"` error
after the reqwest path hit Fix #1's nginx 400 wall. This made every
state-changing endpoint (deploy, file write, OS rebuild, process execute)
completely dead through the SDK.

### Status in this fork

Fixed in `src/sdk/utils/session.rs` of this fork. The fix mirrors
`session_get`'s curl path and pipes the JSON body to curl via
`--data-binary @-` (stdin) to avoid argv length limits and shell-escaping
issues. Empty success bodies are handled.

### What still needs to happen upstream

The same fix should be ported to `Openmesh-Network/xnode-manager-sdk` so
non-fork users get a working POST path. **This requires John's approval
before any PR or issue is filed.**

---

## Fix #3 — `xnode-auth` undocumented EIP-55 case-sensitivity gotcha

**Status:** open (documentation issue, no code change needed)
**Affects:** `Openmesh-Network/xnode-auth`
**Discovered:** 2026-02 (during initial CLI auth implementation)

### Symptom

A login attempt with a lowercased identity cookie passes the signature
check but fails the string comparison `recoveredAddress === user`,
returning 401. The cookie identity must be the **EIP-55 checksummed**
address.

### Impact

Six failed implementation attempts (per the master plan failure audit)
before this was tracked down. Anyone implementing a new SDK/client will
hit this exact wall.

### Suggested fix (upstream)

Either:

1. **Accept lowercased identities** and normalize internally. Cheapest fix.
2. **Document the requirement loudly** in the README and return a clear
   `400 Identity must be EIP-55 checksummed` error instead of the generic
   401.

---

## Fix #4 — `xnode-apps/openclaw` hardcodes a gateway auth token

**Status:** notified plopmenz upstream (per John, 2026-04-10) — awaiting upstream action
**Affects:** `Openmesh-Network/xnode-apps`
**Discovered:** 2026-04-10 (during inspection of currently-deployed openclaw container)

### Symptom

`xnode-apps/openclaw/flake.nix` line 18 hardcodes
`services.openclaw-gateway.config.gateway.auth.token = "<token>"` directly
in the source tree. The token is therefore public on github.com. Any xnode
that deploys this template inherits the same token.

### Impact

Low-to-moderate. The token is for `openclaw-gateway`'s internal auth, not
for any wallet or production credential. But:
- It is deterministic across all openclaw deployments
- It is static (no rotation)
- It cannot be easily overridden without forking the template

### Suggested fix (upstream)

Read the token from a runtime file, env var, or systemd `EnvironmentFile`,
not from the source tree. The standard NixOS pattern is
`pkgs.writeText "token" (builtins.readFile /run/secrets/...)` or similar.

---

*This file is local-only documentation for John's reference. Filing any of
these fixes as upstream PRs or issues requires explicit per-fix approval.*
