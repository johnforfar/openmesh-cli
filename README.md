# Openmesh CLI (om)

The `om` CLI is a high-performance Rust-based interface for direct, cryptographic management of Sovereign Xnodes. It leverages OS-native keychains for secure identity storage and bitwise-accurate EIP-191 signing for decentralized authentication.

> [!IMPORTANT]
> **This is a Beta release.** More features are coming soon, including domain mapping, application deployment to your Xnode, custom app flake management, and enhanced status monitoring.

## 1. Installation

### From Source
```bash
git clone https://github.com/johnforfar/openmesh-cli.git
cd openmesh-cli
cargo install --path .
```

## 2. Quick Start

### Secure Identity Setup
Import your EVM private key or 12/24-word mnemonic. Your secret is stored securely in the **macOS Keychain** or **Linux Secret Service (D-Bus/Libsecret)**.
```bash
om wallet import
```

Verify your active address:
```bash
om wallet status
```

### Setup your Xnode
Before using the CLI, ensure your Xnode is initialized and has a domain set.
1. Visit [https://xnode.openmesh.network](https://xnode.openmesh.network)
2. Complete the setup and assign your domain.

### Authenticate
Log in to your specific Xnode Manager URL. This establishes a cryptographically signed session.
```bash
om login --url https://your-xnode-manager-url
```

### Monitor Resources
Fetch real-time CPU, RAM, and container metrics directly from the node:
```bash
om node status
om ps
```

## 3. Engineering Excellence
- **Hybrid Staging**: Automatically falls back to system `curl` with standard browser fingerprints to bypass strict infrastructure blocks.
- **Identity Precision**: Fully compatible with EIP-55 checksumming and EIP-191 signature recovery.
- **Dynamic Proxying**: Injects mandatory `Path` and `Referer` headers required for Nginx/Next.js proxy routing.

## 4. Capability Audit
Run the logic verification suite:
```bash
cargo test --test test_suite -- --nocapture
```

---
*Created by Kilo Code for the Openmesh Network*
