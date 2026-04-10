# Openmesh Skills — Deploy Apps to a Sovereign Xnode

> **For:** university hackathon students, Claude Code vibe-coders, and AI agents
> driving `om` programmatically.
>
> **What you'll learn:** how to deploy a containerized app to your own
> sovereign Xnode and put it on a public subdomain — in two commands.
>
> **Time:** ~5 minutes once you have a wallet and an Xnode.

---

## Why this exists

You have a sovereign Xnode. Maybe you spun it up at
[xnode.openmesh.network](https://xnode.openmesh.network), maybe a friend gave
you access. You want to put an app on it. Today the options are:

1. Open the manager web UI, click through six dialogs, hope nothing breaks
2. SSH into the box and learn NixOS
3. **Use `om`, the Openmesh CLI, and run two commands**

This file teaches option 3. It is the source of truth for how a Claude Code
agent (or a human reading along) should drive `om`.

---

## Prerequisites (one-time setup)

```bash
# 1. Install om
git clone https://github.com/johnforfar/openmesh-cli
cd openmesh-cli
cargo install --path .

# 2. Import your wallet (stored in your OS keychain — never written to disk)
om wallet import

# 3. Log in to your xnode manager
om login --url https://manager.<your-subdomain>.openmesh.cloud
```

**Verify it worked:**
```bash
om node info
```

You should see your xnode's domain, owner, and flake config. If you get
`E_NOT_LOGGED_IN`, run `om login` again.

> **Tip for AI agents:** add `--format json` to any command for machine-readable
> output. Errors come back as `{"error": {"code": "E_...", "message": "...", "hint": "..."}}`.

---

## The deploy pattern (4 steps)

Every app deploy on Openmesh follows the same shape:

```
deploy   →   wait   →   expose   →   verify
```

1. **deploy** — build the app as a NixOS container on the xnode
2. **wait** — block until `nixos-rebuild` finishes (the manager runs it
   asynchronously and returns a `request_id`)
3. **expose** — add a reverse-proxy rule so the public can reach it
4. **verify** — curl the URL or open it in a browser

`om app deploy` and `om app expose` both default to `--wait true`, so steps
1+2 and 3+4 collapse into single commands. Use `--wait false` if you want to
fire and forget.

---

## Worked example: Openmesh Support Agent

A RAG-powered docs assistant. Users ask questions about the `om` CLI,
deployment patterns, and Openmesh concepts. The agent retrieves the most
relevant chunks from a multi-source doc corpus (openxai-docs + the openmesh-cli
canonical docs + this app's own docs) and feeds them to a local `llama3.2:1b`
running on the same xnode. Returns answers with source citations.

Runs entirely on your xnode — no OpenAI key, no SaaS, no telemetry.

### Source

[github.com/johnforfar/openmesh-support-agent](https://github.com/johnforfar/openmesh-support-agent)

A single nixos-container that bundles:
- `services.ollama` with `llama3.2:1b` (chat) + `nomic-embed-text` (embeddings)
- `services.postgresql` with the `pgvector` extension
- A ~330-line Python Flask backend that ingests `.md`/`.mdx` from configurable
  paths, generates embeddings, stores them in pgvector, and serves
  `/api/chat` for retrieval-augmented generation
- `services.nginx` serving a vanilla HTML frontend on `/` and proxying
  `/api/*` to the Python backend

### Deploy

```bash
om app deploy support-agent \
  --flake github:johnforfar/openmesh-support-agent
```

What this does behind the scenes:
- POSTs to `/config/container/support-agent/set` with the flake URI
- The manager runs `nixos-rebuild switch` to materialize the container
  (first deploy is slow: ~5-15 min for nix build + downloading two ollama
  models, ~1.6 GB total)
- `om` polls `/request/<id>/info` until done (default timeout: 10 minutes)
- Output:

```
App:        support-agent
Request:    #42
Status:     success
```

### Expose on a subdomain

```bash
om app expose support-agent \
  --domain chat.<your-subdomain>.openmesh.cloud \
  --port 80
```

What this does behind the scenes:
- Fetches the current host flake from `/os/get`
- **Additively** adds a `services.xnode-reverse-proxy.rules` block — never
  touches rules for other domains
- POSTs the modified flake to `/os/set`
- The manager runs `nixos-rebuild switch` to provision nginx + a Let's Encrypt
  cert via ACME
- Output:

```
Exposed:    chat.<your-subdomain>.openmesh.cloud
  → http://support-agent:80
  app:      support-agent
  request:  #43
  status:   success
Try: curl https://chat.<your-subdomain>.openmesh.cloud
```

### Verify

For a production-grade end-to-end check, run the `verify_deployment.py`
suite that ships with the support agent repo:

```bash
git clone https://github.com/johnforfar/openmesh-support-agent
python openmesh-support-agent/tests/verify_deployment.py \
  https://chat.<your-subdomain>.openmesh.cloud
```

This runs 16 checks: TLS validity, latency budgets, knowledge tests
(questions whose answers are in the docs), and negative tests (questions
the agent should refuse rather than hallucinate). Use `--json` for CI.

For a quick smoke test, just open the URL in your browser and ask it a
question about the `om` CLI.

---

## Command reference

### `om app deploy <name> --flake <uri>`

Create or update a container.

| Flag | Default | Effect |
|---|---|---|
| `--flake <uri>` | required | Flake URI to build from. Examples: `github:Openmesh-Network/xnode-apps?dir=jellyfin`, `github:you/your-repo` |
| `--update-input <name>` | none | Update specific flake inputs before building. Repeatable. |
| `--wait` | `true` | Block until rebuild finishes |
| `--timeout <sec>` | `600` | How long to wait |
| `--dry-run` | off | Show payload without applying |

### `om app expose <name> --domain <fqdn> --port <n>`

Add a public subdomain that forwards to a container.

| Flag | Default | Effect |
|---|---|---|
| `--domain <fqdn>` | required | The subdomain to expose |
| `--port <n>` | required | The port the container listens on |
| `--protocol` | `http` | `http`, `https`, `tcp`, or `udp` |
| `--path <prefix>` | none | Only forward requests under a path |
| `--replace` | off | Overwrite an existing rule for the same domain |
| `--wait` | `true` | Block until rebuild finishes |
| `--dry-run` | off | Show flake diff without applying |

### `om app list`

Show all deployed containers.

### `om app info <name>`

Show one container's flake configuration.

### `om app remove <name>`

Delete a container. Use `--wait false` if you don't want to block.

### `om app unexpose --domain <fqdn>`

Remove the reverse-proxy rule for a subdomain (does not delete the container).

### `om req show <id>` / `om req wait <id>`

Inspect or block on an async request id (returned by every state-changing
command).

---

## JSON mode (for AI agents)

Every command supports `--format json`. Output goes to stdout, status messages
go to stderr, so you can pipe stdout into `jq` and it stays parseable.

```bash
om --format json app list
# {
#   "containers": ["vibecheck"]
# }

om --format json app deploy vibecheck --flake github:johnforfar/openmesh-vibe-check
# {
#   "app": "vibecheck",
#   "request_id": 42,
#   "status": "success"
# }
```

### Errors are structured

If a command fails in `--format json` mode, it prints to **stdout** (not
stderr) so a single pipe captures both success and failure:

```json
{
  "error": {
    "code": "E_SESSION_EXPIRED",
    "message": "Session expired or unauthorized. Please run 'om login' again.",
    "hint": "Run `om login --url <your-xnode-manager-url>` to re-authenticate"
  }
}
```

### Stable error codes

Branch on these, never on the English message:

| Code | When |
|---|---|
| `E_NOT_LOGGED_IN` | No session file. Run `om login`. |
| `E_SESSION_EXPIRED` | Manager returned 401. Run `om login` again. |
| `E_BAD_REQUEST` | Manager returned 4xx. Check your input. |
| `E_MANAGER_UNREACHABLE` | Network/TLS/5xx. Check connectivity. |
| `E_INVALID_RESPONSE` | Manager returned non-JSON. |
| `E_INVALID_INPUT` | A flag failed validation. The message tells you which. |
| `E_NOT_FOUND` | Container or request id doesn't exist. |
| `E_ALREADY_EXISTS` | A rule for that domain already exists; pass `--replace` to overwrite. |
| `E_UNSAFE_FLAKE_EDIT` | The flake editor refused to modify the host config (markers missing). |
| `E_TIMEOUT` | Async op didn't finish within `--timeout` seconds. Check `om req show <id>`. |
| `E_INTERNAL` | Catch-all. Check stderr for details. |

---

## Common patterns

### Deploy from the official xnode-apps catalog

```bash
om app deploy ollama   --flake github:Openmesh-Network/xnode-apps?dir=ollama
om app deploy jellyfin --flake github:Openmesh-Network/xnode-apps?dir=jellyfin
om app deploy vscode   --flake github:Openmesh-Network/xnode-apps?dir=vscode-server
om app deploy vault    --flake github:Openmesh-Network/xnode-apps?dir=vaultwarden
```

Available templates: `ollama`, `jellyfin`, `nextcloud`, `immich`,
`vaultwarden`, `minecraft-server`, `vscode-server`, `near-validator`.

### Deploy your own app

Any flake that exports `nixosConfigurations.container` works:

```bash
om app deploy myapp --flake github:youruser/your-app-repo
```

Check `examples/` in this repo for a minimal template.

### Expose multiple paths to the same container

Run `om app expose` once per path:

```bash
om app expose myapi --domain api.example.com --port 8080
om app expose myapi --domain api.example.com --port 8080 --path /v2 --replace
```

Or combine paths in a single rule by editing the flake directly (advanced).

### Idempotent re-deploy (CI / GitHub Actions)

`om app deploy` is `apply`-style: it creates if missing, updates if exists.
Run it from CI safely:

```yaml
- name: Deploy to xnode
  run: |
    om login --url ${{ secrets.XNODE_MANAGER_URL }}
    om --format json app deploy ${{ github.event.repository.name }} \
      --flake github:${{ github.repository }}
```

---

## Troubleshooting

### `om app deploy` succeeds but the URL returns 502

The container is built but nginx hasn't reloaded yet, or the container
doesn't actually listen on the port you exposed. Check:

```bash
om req show <request_id>          # was the rebuild actually successful?
om app info <name>                 # is the container configured?
```

### Subdomain doesn't resolve at all

Two possibilities:

1. **DNS isn't pointed at your xnode.** If you're using
   `*.<your-subdomain>.openmesh.cloud`, the manager handles DNS via the
   `claim.dns.openmesh.network` service when you first claimed your domain
   at xnode.openmesh.network. New subdomains under your claimed root should
   resolve automatically. If they don't, claim the specific subdomain via
   the web UI.

2. **You used a domain you don't control.** The xnode can't ACME-provision
   a cert for a domain whose DNS doesn't point at it.

### `E_UNSAFE_FLAKE_EDIT`

`om` refused to modify the host flake because it couldn't find the
`# START USER CONFIG` / `# END USER CONFIG` markers. This usually means
your xnode is running an old image. Run `om node info` and check the flake
manually.

### `E_TIMEOUT` during deploy

NixOS rebuilds can be slow on first build of a new flake (substituting
binaries, evaluating). Bump the timeout:

```bash
om app deploy <name> --flake <uri> --timeout 1800
```

Or fire-and-forget and check later:

```bash
om app deploy <name> --flake <uri> --wait false
# ... go get coffee ...
om req show <request_id>
```

---

## What this skill does NOT cover

- **Multiple xnodes / fleet management** — single xnode for now. Multi-node
  config (`om context use ...`) is on the roadmap.
- **Secrets injection** — agenix / sops-nix integration not yet wired.
  For now, secrets live in the flake (use a private repo).
- **GPU containers** — `nvidia_gpus` field exists in the SDK but isn't
  surfaced as a CLI flag yet. Edit the flake directly.
- **Host-level NixOS edits** — `om host edit` is not yet implemented.
  Use the manager web UI for advanced configuration.

---

## For AI agents: the contract

If you are an AI agent reading this file to help a user deploy apps on an
Openmesh Xnode, follow this contract:

1. **Always pass `--format json`** when invoking `om` so output is parseable.
2. **Branch on error codes**, never on English messages.
3. **Every state-changing op returns a `request_id`.** With default
   `--wait true`, the command blocks until the request finishes and the
   final status is in the JSON output. With `--wait false`, you must call
   `om req wait <id>` yourself.
4. **`om app deploy` is `apply`-style** — always idempotent, safe to retry.
5. **`om app expose` is additive** — it never removes rules for other
   domains. You can call it many times for many subdomains.
6. **Never bypass `om` with curl** unless `om` is missing the operation
   you need. If you find a missing operation, that's a bug to file
   against `johnforfar/openmesh-cli`, not a workaround to merge.
7. **Stop and ask the user** before any operation that:
   - Removes a deployed app or expose rule
   - Changes the host's `domain` or `xnode_owner`
   - Reboots the xnode
8. **Status messages go to stderr** in `--format json` mode. You can
   ignore them or surface them to the user as progress info.

### Minimum capability check

Before recommending a deploy, verify the agent's environment is ready:

```bash
om node info --format json    # E_NOT_LOGGED_IN means: tell user to run `om login`
om app list --format json     # confirms manager is reachable + session valid
```

If both succeed, you're cleared to deploy.

---

## Footer (the test)

A successful deploy of the Sovereign Vibe Check app should display, at the
bottom of every page:

> Deployed with Openmesh CLI v2.0 via Claude Code — [John Forfar](https://github.com/johnforfar)

If you see that on a `chat.<something>.openmesh.cloud` URL, the whole pipeline
worked end-to-end: SDK → CLI → manager API → nixos-rebuild → reverse proxy →
ACME → DNS → your browser. That's a lot of moving parts to get right in two
commands.

---

*This file is the source of truth for how to drive the Openmesh CLI. If a
behavior here disagrees with the actual `om` binary, the binary is wrong —
file an issue.*
