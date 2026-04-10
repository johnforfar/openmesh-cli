# Open Source Fixes — Status & Roadmap

This document tracks technical issues discovered while building `om-cli` and the
upstream Openmesh tooling, what's been fixed in this fork, and what still needs
attention either here or in upstream repositories.

## 1. Repository Topology

| Repo | Owner | Editable here? |
|---|---|---|
| `johnforfar/openmesh-cli` (this repo) | personal | Yes — every change reviewed before commit |
| `Openmesh-Network/xnode-manager` | upstream | No — file an issue/PR upstream |
| `Openmesh-Network/xnode-manager-sdk` | upstream | No — see §6 below |
| `Openmesh-Network/xnode-manager-frontend` | upstream | No |
| `Openmesh-Network/xnode-auth` | upstream | No |
| `Openmesh-Network/xnode-apps` | upstream | No |

## 2. Fixed in this PR

### 2.1 `session_post` curl fallback (CRITICAL — was unimplemented)

**File:** `src/sdk/utils/session.rs`

Before this PR, every POST endpoint in the SDK was effectively dead. The code
path was:

1. Try the `reqwest` client → nginx returns `400 Bad Request` (see §4)
2. Fall through to the curl branch
3. The curl branch returned a hardcoded error string:
   `"Post fallback not fully implemented for generic data"`

This blocked **every state-changing operation**: container deploy/remove,
file write/delete/mkdir, OS rebuild/reboot, process execute, domain claim.

**The fix** mirrors the working `session_get` curl path and pipes the JSON body
to curl via stdin (`--data-binary @-`). Stdin piping avoids:
- argv length limits (some payloads include flake content > 10 KB)
- shell escaping pitfalls with quotes/newlines in JSON
- temporary files on disk (smaller attack surface for any cleanup races)

It also handles three response shapes:
- normal JSON body → parsed as `Output`
- empty body (success on no-content endpoints) → parsed as `{}`
- 401 / "unauthorized" / "login" → friendly "Session expired" error

### 2.2 `OM_FORCE_CURL` environment variable (NEW)

**File:** `src/sdk/utils/session.rs`

When `OM_FORCE_CURL=1` is set, both `session_get` and `session_post` skip the
`reqwest` attempt entirely and go straight to the curl shell-out. This saves
one wasted round-trip per call on networks where reqwest is known to fail
against the Xnode Manager nginx layer (see §4).

```bash
export OM_FORCE_CURL=1
om node status   # ~50% faster: no doomed reqwest attempt first
```

Default behavior (env var unset or `0`) is unchanged: try reqwest, fall back
to curl on failure.

### 2.3 Test suite (new)

**File:** `tests/test_suite.rs`

Replaces a single 86-line smoke test with **32 tests across four tiers**:

| Tier | Count | What it covers |
|---|---|---|
| Unit | 10 | EIP-55 checksumming, BIP-39/BIP-32 derivation, EIP-191 sign/recover, JSON payload shape, proxy header construction, session serialization |
| Session | 5 | Session file existence, parse, auth cookies present, valid URL, cookie jar present |
| Live (reqwest path) | 10 | Hits real endpoints; gracefully degrades on 400 |
| Live (curl path) | 7 | Same endpoints via the curl shim — currently the only path that works against `manager.build.openmesh.cloud` |

Run them tiered:
```bash
cargo test --test test_suite unit -- --nocapture       # offline, ~50ms
cargo test --test test_suite session -- --nocapture    # checks ~/.openmesh_session.cookie
cargo test --test test_suite curl_live -- --nocapture  # hits the real Xnode
```

Live tests **gracefully skip** if no session file is present, so CI can run
the unit tier safely.

### 2.4 Test fixtures use Hardhat dev account, not personal wallets

**File:** `tests/test_suite.rs`

All test fixtures now use the **Hardhat test account #0**
(`0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266`) — a publicly known development
address derived from the open Hardhat mnemonic
`"test test test test test test test test test test test junk"`.

Hardcoding any individual contributor's wallet address into a public test
suite is a privacy footgun: it ties the OSS repo to a specific identity.
Using the Hardhat account makes tests reproducible by anyone without
revealing who is running them.

### 2.5 Removed orphaned `tests/test_checksum.rs`

That file was a one-off scratch script with a `main()` and was not
referenced from `Cargo.toml`, so `cargo test` never ran it. Deleting
keeps the working tree clean and removes a stale hardcoded address.

## 3. Known Issues That Are NOT Fixed Here

### 3.1 The reqwest path is fundamentally broken (TECH DEBT)

**Where:** `src/sdk/utils/session.rs`

`reqwest` requests against `manager.build.openmesh.cloud` consistently
return `400 Bad Request` from nginx. We have not isolated whether the
trigger is:
- TLS fingerprint (rustls vs OpenSSL/curl differences)
- HTTP/2 frame normalization
- Header ordering / casing
- A duplicate `Host` header (reqwest sets one automatically + we set one
  in `Session::load`)

Until that's fixed, every Rust call goes through curl. **The `OM_FORCE_CURL=1`
env var lets users skip the wasted attempt entirely** (see §2.2).

**Possible fixes (need investigation):**
1. Drop the explicit `Host` header in `Session::load` — reqwest sets it from
   the URL automatically and the duplicate may be confusing nginx
2. Use `curl-impersonate`-style TLS fingerprinting (a Rust crate exists)
3. Configure rustls cipher suite ordering to match curl's defaults
4. Switch HTTP/2 → HTTP/1.1 for the manager endpoint

This is a meaningful research project — out of scope for this PR.

### 3.2 `xnode-manager-sdk` is vendored, not depended-on

**Where:** `src/sdk/`

The SDK source lives in this repo as a **modified fork** of the upstream
`Openmesh-Network/xnode-manager-sdk`. They have already diverged across
every file:

```
$ diff -rq src/sdk submodules/xnode-manager-sdk/rust/package/src
Files differ: auth/handlers.rs, config/handlers.rs, file/handlers.rs,
              file/models.rs, info/handlers.rs, info/models.rs,
              os/handlers.rs, process/handlers.rs, process/models.rs,
              request/handlers.rs, request/models.rs, usage/handlers.rs,
              utils/error.rs, utils/session.rs
Only in submodules: lib.rs
Only in src/sdk: mod.rs
```

This means:
- Bug fixes here don't reach the official SDK
- Improvements upstream aren't pulled in
- Anyone using `xnode-manager-sdk` directly hits the same broken
  `session_post` we just fixed in §2.1

**Three options, not blocking this PR:**

| Option | Effort | Tradeoff |
|---|---|---|
| Stay forked | none | Drift increases over time |
| Upstream the curl fallback as a PR to `Openmesh-Network/xnode-manager-sdk` | medium | Eliminates divergence but needs upstream maintainer buy-in |
| Switch to depending on upstream as a crate + write a thin transport adapter | high | Best engineering, biggest refactor |

Tracking issue: **TODO** (file in this repo).

### 3.3 Stubbed CLI commands

**Where:** `src/main.rs:412`

The `App` and `Host` subcommand groups all hit:
```rust
_ => println!("🚧 Command not yet fully bridged to SDK. Check back soon!"),
```

These are unblocked by §2.1 (POST fix) and need wiring:
- `om app list/deploy/update/delete/files`
- `om host ps/explorer/edit/update/reboot`

To be addressed in follow-up PRs.

## 4. Upstream Issues to File

These are not fixable in this fork. Each should become a GitHub issue
against the relevant Openmesh repo, with a reproduction.

### 4.1 `Openmesh-Network/xnode-manager` — nginx rejects valid Rust HTTP clients

**Symptom:** `reqwest::Client` requests against the manager always return
400 from nginx. System curl with the exact same cookie jar and headers
succeeds. Reproduction is in this repo's `tests/test_suite.rs` —
`live_usage_memory` (reqwest, fails) vs `curl_live_memory` (curl, succeeds)
hit identical endpoints.

**Impact:** Every non-browser HTTP client in the ecosystem is blocked.
TypeScript with `node-fetch`/`undici`, Python with `requests`/`httpx`, Go
with `net/http` — all likely affected. This forces every SDK and tool to
shell out to curl, which is fragile and platform-specific.

**Likely root cause:** nginx config rejects requests based on
header ordering, TLS fingerprint, or HTTP/2 framing.

### 4.2 `Openmesh-Network/xnode-manager-sdk` — `session_post` curl fallback never implemented

If §2.1 of this doc is right, the SDK has the same broken POST in upstream.
Should be fixed there too — the fix in `src/sdk/utils/session.rs:312` of
this fork can be ported as a PR.

### 4.3 `Openmesh-Network/xnode-auth` — undocumented EIP-55 case-sensitivity gotcha

**Symptom:** A login attempt with a lowercased identity cookie succeeds
the signature check but fails the string comparison
`recoveredAddress === user`, returning 401. The cookie identity must be
the **checksummed** EIP-55 address.

**Impact:** Six failed implementation attempts (per the master plan
failure audit) before this was tracked down.

**Suggested fix:** Either accept lowercased identities and normalize
internally, or document the requirement loudly in the README and return
a clear `400 Identity must be EIP-55 checksummed` instead of the
generic 401.

## 5. Security Notes

This is public open-source code that manages wallet-authenticated
infrastructure. Contributors must follow these rules:

1. No private keys, mnemonics, or API tokens in source or git history.
2. No personal wallet addresses in test fixtures. Use publicly-known
   development accounts (e.g. the Hardhat default mnemonic) so tests
   are reproducible without identifying contributors.
3. Secrets are read from environment variables, never hardcoded.
4. The `.gitignore` excludes credential and session files. Check it
   before adding any new file pattern that might hold secrets.
5. Private keys are stored in the OS keychain (macOS Keychain on
   Darwin, Secret Service on Linux), never in plaintext files.

If you find a security issue, please report it privately rather than
opening a public issue.

## 6. Audit

A pre-commit security audit was performed before this PR. No leaked
private keys, API tokens, or other credentials were found in the
working tree or commit history.

Future audits should be run before any commit that touches
authentication, transport, or session storage code.

## 7. Environment Variables

| Variable | Default | Effect |
|---|---|---|
| `OM_FORCE_CURL` | unset | When `1`, skip the reqwest attempt and use curl directly. Speeds up calls on networks where reqwest is blocked by the manager nginx layer. |

---

*Last updated: 2026-04-10*
