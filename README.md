# Openmesh CLI (`om`)

A fast, scriptable command-line tool for deploying and operating applications
on your own **Sovereign Xnode** — a NixOS-based server you fully control.

`om` authenticates with your EVM wallet, speaks directly to the Xnode Manager
API, and turns the full lifecycle (deploy, expose, observe, update, remove)
into a handful of predictable commands that work identically from a laptop, a
CI pipeline, or an AI agent.

```bash
om wallet import
om login --url https://manager.your-subdomain.openmesh.cloud
om app deploy my-app --flake github:you/your-app
om app expose my-app --domain my-app.your-subdomain.openmesh.cloud --port 8080
```

That's the whole happy path. Everything below is how to get fluent.

---

## Install

```bash
git clone https://github.com/johnforfar/openmesh-cli
cd openmesh-cli
cargo install --path .
```

Requires a recent stable Rust toolchain. The binary installs as `om`.

---

## Prerequisites

1. **A wallet.** Any EVM private key or 12/24-word mnemonic. `om` stores it
   in the OS-native secure store (macOS Keychain or Linux Secret Service) —
   never on disk, never in the binary, never in logs.
2. **An Xnode with a domain.** Spin one up at
   [xnode.openmesh.network](https://xnode.openmesh.network) and complete the
   initial setup so the manager is reachable at `https://manager.<your-subdomain>.openmesh.cloud`.

That's it. No account signup, no API keys to manage, no cloud project to
configure.

---

## Quick Start

```bash
# 1. Import your wallet (stored in your OS keychain)
om wallet import

# 2. Confirm the active address
om wallet status

# 3. Log in to your xnode manager
om login --url https://manager.<your-subdomain>.openmesh.cloud

# 4. Verify you're connected
om node info
om status
```

`om status` shows a live dashboard of the xnode's specs, CPU/memory/disk, and
every running container — a good place to land after logging in.

---

## Deploy an app

A deploy on Openmesh is just: *build the app as a NixOS container, then attach
a public subdomain.*

```bash
# Build and start the container
om app deploy my-app --flake github:you/your-app

# Put it on the public internet (adds a reverse-proxy rule + TLS)
om app expose my-app \
  --domain my-app.<your-subdomain>.openmesh.cloud \
  --port 8080
```

Both commands block until the xnode has finished rebuilding and return cleanly
on success. Pass `--wait false` to fire-and-forget and poll with
`om req wait <id>` later.

Every app deploy follows the same four-step rhythm — `deploy → wait → expose → verify` — and `om` collapses it into two commands by waiting by default.

---

## Command reference

| Area | What it does |
|---|---|
| `om wallet import / status / clear` | Manage the wallet used to sign auth challenges |
| `om login --url <URL>` | Authenticate with an Xnode Manager and cache the session |
| `om profile list / use / login / remove` | Switch between multiple xnodes by name |
| `om node info / status` | Read host identity and live resource utilisation |
| `om status [--watch]` | One-page dashboard: specs + CPU/mem/disk + all containers |
| `om ps` | List containers and their running state |
| `om app list / info / logs` | Inspect deployed apps |
| `om app deploy / remove` | Create or tear down an app container |
| `om app expose / unexpose` | Attach or detach a public subdomain |
| `om app env list / set / remove` | Manage per-app environment variables (secrets) |
| `om app set-domain / set-role` | Per-xnode config for multi-node deploys |
| `om os domain claim / set / status` | Manage the xnode's public domain and TLS |
| `om os github-auth set / clear / status` | Register a GitHub PAT so the xnode can fetch private flakes |
| `om req show / wait / logs` | Inspect or block on async operations |

All commands accept `-p, --profile <name>` to target a specific xnode and
`-f, --format json` for machine-readable output.

---

## Secrets: `om app env`

Environment variables for your app live inside the container and are loaded
by systemd at startup — they never enter git, the Nix store, or the host
flake.

```bash
om app env set my-app DATABASE_URL=postgres://... SMTP_PASS=...
om app env list my-app              # values masked by default
om app env list my-app --show-values # reveal (use carefully)
om app env remove my-app OLD_KEY
```

Restart the container after a change:

```bash
om app deploy my-app --flake <same-or-newer-uri>
```

---

## Running private repos

`om app deploy` can pull from private GitHub repositories once the xnode has
a Personal Access Token configured:

```bash
om os github-auth set github_pat_<fine-grained-read-only-token>
```

Use a **fine-grained** PAT scoped to `Contents: Read-only` on the specific
repos you need. Rotate it on a cadence and remove with `om os github-auth clear`
when no longer needed.

---

## Multi-xnode deployments (primary + replica)

A single git commit can drive two or more xnodes, each with its own role and
domain, by combining named profiles with per-xnode config:

```bash
# Once per xnode
om profile login prod --url https://manager.prod.openmesh.cloud
om profile login replica --url https://manager.replica.openmesh.cloud

# Pin each xnode's role and domain
om --profile prod    app set-role   my-app primary
om --profile replica app set-role   my-app replica
om --profile prod    app set-domain my-app app.example.com
om --profile replica app set-domain my-app app-replica.example.com

# Deploy the same commit to both
om --profile prod    app deploy my-app --flake github:you/your-app
om --profile replica app deploy my-app --flake github:you/your-app
```

Your app's flake receives `xnodeRole` and `xnodeDomain` as specialArgs at
build time, so one codebase can enable role-specific services (backups on
primary, read-only on replica) and render the right URLs on each host.

---

## AI-agent mode

Every command supports `--format json` and returns stable, non-localised
error codes (`E_NOT_LOGGED_IN`, `E_SESSION_EXPIRED`, `E_UNSAFE_FLAKE_EDIT`,
`E_TIMEOUT`, …) so an agent can branch on codes rather than parsing English.

For a full agent-facing guide — worked examples, the 7-rule agent contract,
CI patterns, and troubleshooting recipes — see
[OPENMESH-SKILLS.md](./OPENMESH-SKILLS.md).

---

## Know before you deploy

A short list of things that trip up first-time deployers. Each has a clear
error code so you'll see the problem fast, but knowing these up front saves
an iteration.

1. **Your xnode needs the `# START USER CONFIG` / `# END USER CONFIG`
   markers** in its host flake. Modern xnodes have them by default; very old
   images may not. If `om app expose` returns `E_UNSAFE_FLAKE_EDIT`, update
   the xnode's base flake.
2. **Names and domains are strictly validated.** Container names are
   lowercase ASCII, alphanumeric plus hyphen. Domains are standard FQDN
   characters only. This is deliberate — it keeps user input from ever
   landing inside a Nix string unchecked.
3. **Private GitHub repos require `om os github-auth set` first.** Without
   it, `nix` fetches return 404 and the deploy fails before it starts.
4. **Your app flake's `nixpkgs` input is pinned by the wrapper.** The xnode
   composes your `nixosModules.default` onto a known-good nixpkgs for
   runtime stability. If you need a newer nixpkgs to build a specific
   package, add it as a separate input (e.g. `nixpkgs-app`) and use it only
   for that package.
5. **Redeploying a name that existed before?** Run `om app remove <name>`
   first, then `om app deploy`. This clears any stale container state from
   earlier failed rebuilds.

---

## Development

```bash
cargo build              # build
cargo test               # full suite (64 offline tests)
cargo test --lib         # unit tests only
cargo test --test test_flake_editor  # flake-edit safety tests (24)
```

The flake-editor test suite is the most security-critical part of the
codebase — it covers round-trip identity, additive-preservation guarantees,
and rejection of Nix-injection attempts on every validated input.

---

## License & contributing

Issues and pull requests welcome. File them against
[johnforfar/openmesh-cli](https://github.com/johnforfar/openmesh-cli).

Built for the Openmesh Network.
