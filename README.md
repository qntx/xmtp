# xmtp

[![CI][ci-badge]][ci-url]
[![License][license-badge]][license-url]
[![Rust][rust-badge]][rust-url]

[ci-badge]: https://github.com/qntx/xmtp/actions/workflows/rust.yml/badge.svg
[ci-url]: https://github.com/qntx/xmtp/actions/workflows/rust.yml
[license-badge]: https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg
[license-url]: LICENSE-MIT
[rust-badge]: https://img.shields.io/badge/rust-edition%202024-orange.svg
[rust-url]: https://doc.rust-lang.org/edition-guide/

**Safe, ergonomic Rust SDK for the [XMTP](https://xmtp.org) messaging protocol — E2E encrypted messaging via MLS (RFC 9750), with a batteries-included TUI chat client.**

xmtp wraps the official [`libxmtp`](https://github.com/xmtp/libxmtp) FFI layer with idiomatic Rust types, providing a high-level `Client` → `Conversation` → `Message` API for DMs, groups, content types, identity management, ENS resolution, and real-time streaming. The CLI crate ships a full-featured terminal chat interface with profile-based persistent configuration.

## Crates

| Crate | | Description |
| --- | --- | --- |
| **[`xmtp`](xmtp/)** | [![crates.io][xmtp-crate]][xmtp-crate-url] [![docs.rs][xmtp-doc]][xmtp-doc-url] | SDK — Client, Conversation, Message, content codecs, ENS, Ledger |
| **[`xmtp-sys`](xmtp-sys/)** | [![crates.io][sys-crate]][sys-crate-url] [![docs.rs][sys-doc]][sys-doc-url] | Raw FFI bindings to `libxmtp_ffi` static library |
| **[`xmtp-cli`](xmtp-cli/)** | [![crates.io][cli-crate]][cli-crate-url] | TUI chat client + profile management CLI |

[xmtp-crate]: https://img.shields.io/crates/v/xmtp.svg
[xmtp-crate-url]: https://crates.io/crates/xmtp
[sys-crate]: https://img.shields.io/crates/v/xmtp-sys.svg
[sys-crate-url]: https://crates.io/crates/xmtp-sys
[cli-crate]: https://img.shields.io/crates/v/xmtp-cli.svg
[cli-crate-url]: https://crates.io/crates/xmtp-cli
[xmtp-doc]: https://img.shields.io/docsrs/xmtp.svg
[xmtp-doc-url]: https://docs.rs/xmtp
[sys-doc]: https://img.shields.io/docsrs/xmtp-sys.svg
[sys-doc-url]: https://docs.rs/xmtp-sys

## Quick Start

### Install the CLI

**Shell** (macOS / Linux):

```sh
curl -fsSL https://sh.qntx.fun/xmtp | sh
```

**PowerShell** (Windows):

```powershell
irm https://sh.qntx.fun/xmtp/ps | iex
```

### CLI

```bash

# Create a profile (generates a new key, registers with XMTP)
xmtp new alice

# Create a profile with a Ledger hardware wallet
xmtp new bob --ledger

# Create a profile with an imported private key
xmtp new carol --import 0xdeadbeef...

# Launch the TUI chat interface
xmtp              # uses default profile
xmtp alice        # uses profile "alice"

# Profile management
xmtp list          # list all profiles (* = default)
xmtp info alice    # show profile details + installations
xmtp default alice # set default profile
xmtp remove alice  # delete a profile
xmtp clear         # delete ALL profiles

# Revoke all other installations (requires wallet signature)
xmtp revoke alice
```

### Library

```rust
use xmtp::{Client, Env, AlloySigner};

// Create a client and register identity.
let signer = AlloySigner::random()?;
let client = Client::builder()
    .env(Env::Dev)
    .db_path("./alice.db3")
    .build(&signer)?;

// Send a DM.
let conv = client.dm(&"0xBob...".into())?;
conv.send_text("hello from Rust")?;

// List conversations.
let convs = client.list_conversations(&Default::default())?;
for c in &convs {
    println!("{}: {}", c.id()?, c.name().unwrap_or_default());
}
```

## Architecture

- **xmtp** — High-level SDK. Owns all unsafe FFI calls behind safe types. `Client` is built via `ClientBuilder` with optional signer, ENS resolver, and environment selection. `Conversation` provides send/receive/sync/metadata/consent operations. Content codecs handle text, markdown, reactions, replies, attachments, and read receipts.
- **xmtp-sys** — Auto-generated bindings from `xmtp_ffi.h`. Downloads pre-built static libraries at build time. No `libclang` required for end users.
- **xmtp-cli** — Profile-based TUI chat client. Profiles persist configuration (environment, signer type, wallet address) in platform data directories. Signer is only required for identity-changing operations (`new`, `revoke`); TUI and `info` operate without it.

## Feature Flags

### `xmtp` crate

| Feature | Description |
| --- | --- |
| `content` | Content type codecs (text, reactions, replies, attachments, read receipts) — enabled by default |
| `alloy` | Local private key signer via `alloy-signer-local` |
| `ledger` | Ledger hardware wallet signer via `alloy-signer-ledger` |
| `ens` | ENS name resolution via `alloy-ens` + `alloy-provider` |

### `xmtp-sys` crate

| Feature | Description |
| --- | --- |
| `regenerate` | Re-generate bindings from `xmtp_ffi.h` at build time (requires `libclang`) |

## XMTP Protocol Overview

### Identity Model

XMTP V3 uses [MLS (RFC 9750)](https://www.rfc-editor.org/rfc/rfc9750.html) for E2E encryption. Each user is identified by an **Inbox ID**, which can own multiple blockchain identities, each with up to 10 installations (devices/apps).

```text
Inbox ID
 ├── Identity (EOA / Smart Contract Wallet / Passkey)
 │    └── Installation 1  ← unique key pair, stored in local DB
 │    └── Installation 2  ← independent key pair, separate DB
 │    └── ...  (up to 10 per identity)
 └── Identity 2
      └── ...
```

### Two-Tier Key Architecture

XMTP separates **identity-level operations** (rare, high-privilege) from **messaging operations** (frequent, automated) using two distinct key tiers:

**Wallet key** (EOA private key / hardware wallet / smart contract wallet) — used exclusively for identity mutations that alter the on-chain association graph:

| Operation | When | Signers Required |
| --- | --- | --- |
| Create inbox (register) | First launch | Wallet |
| Add identity to inbox | Linking a new wallet | Existing wallet + new wallet |
| Remove identity from inbox | Unlinking a wallet | Wallet |
| Revoke installations | Device lost/compromised | Wallet |
| Change recovery identifier | Security rotation | Wallet |

**Installation key** (ed25519 key pair, auto-generated, stored in the local encrypted database) — handles everything else with zero user interaction:

- Sending and receiving messages
- Creating / joining conversations
- Syncing conversations, messages, and preferences
- Streaming real-time updates
- Managing consent state
- Group admin operations (add/remove members, update metadata)

> After initial registration, the wallet key is never needed for day-to-day messaging. For CLI users with Ledger hardware wallets, this means the device only needs to be connected for `xmtp new` and `xmtp revoke`.

### Compromise & Recovery Model

Each installation holds **independent MLS epoch keys**. This architecture provides strong security guarantees through MLS forward secrecy and post-compromise security:

| Scenario | Messages before compromise | Messages during compromise | Messages after revoke + new installation |
| --- | --- | --- | --- |
| Single installation compromised | Safe (on other devices) | Exposed (on that device only) | Safe (new keys, new epoch) |
| Wallet key compromised | Safe (cannot decrypt) | N/A (wallet key ≠ message key) | Rotate wallet, revoke installations |
| All installations lost | Unrecoverable* | N/A | Fresh start, no history |

\* Unless another installation was online to complete [History Transfer](#history-transfer) beforehand.

**Key insight**: compromising an installation key exposes only messages decrypted by _that specific installation_ during the compromise window. The attacker cannot decrypt messages on other devices, nor future messages after the compromised installation is revoked — because MLS generates fresh epoch keys that exclude the revoked installation.

### Message Synchronization

Every installation maintains independent MLS state. Understanding what syncs automatically and what requires explicit action is critical:

| Scenario | Automatic? | Mechanism |
| --- | --- | --- |
| New messages across existing installations | Yes | MLS group — all member installations decrypt in real time |
| New conversations (invites/welcomes) | Yes | `conversations.sync()` or `syncAll()` fetches new welcomes |
| Consent & preference state | Yes | Preference sync via a hidden sync group between your devices |
| **Historical messages on a new installation** | **No** | **History Transfer — requires an existing installation online** |

Each installation tracks a **cursor** (bookmark) per conversation. `sync()` only fetches messages after the cursor, making incremental sync efficient. Streaming (`stream()`) delivers messages in real time but does not advance the cursor — only `sync()` does.

### History Transfer

A new installation cannot decrypt messages sent before it joined the MLS group. To access historical messages, XMTP provides [History Transfer](https://docs.xmtp.org/chat-apps/list-stream-sync/history-sync) (defined in [XIP-64](https://github.com/xmtp/XIPs/blob/main/XIPs/xip-64-history-transfer.md)):

1. **New device** requests history via the sync group (`initiateHistoryRequest`)
2. **Existing device** (must be online) prepares and uploads an encrypted archive to the history server
3. **New device** downloads and imports the archive into its local database

Key details:

- History transfer is **enabled by default** — the SDK sets `historySyncUrl` to an XMTP Labs-hosted server based on your `env` setting
- The encrypted payload is stored on the history server for **24 hours** only
- The archive includes conversations, messages, consent state, and HMAC keys
- **If all installations are lost**, message history is unrecoverable (no server-side plaintext storage)
- You can self-host the history server or disable it by setting `historySyncUrl` to an empty string

## Supported Platforms

| Target | Status |
| --- | --- |
| `x86_64-unknown-linux-gnu` | ✅ |
| `aarch64-unknown-linux-gnu` | ✅ |
| `aarch64-apple-darwin` | ✅ |
| `x86_64-pc-windows-msvc` | ✅ |
| `aarch64-pc-windows-msvc` | ✅ |

## Security

This library has **not** been independently audited. See [SECURITY.md](SECURITY.md) for full disclaimer, supported versions, and vulnerability reporting instructions.

- All FFI calls are bounds-checked and null-pointer guarded
- Private keys remain in memory only as long as needed; Ledger keys never leave the device
- No key material is logged or persisted by the SDK (key files are managed by the CLI layer)
- E2E encryption handled by libxmtp's MLS implementation (RFC 9750)

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project shall be dual-licensed as above, without any additional terms or conditions.
