# xmtp

Ergonomic Rust SDK for the [XMTP](https://xmtp.org) messaging protocol.

Wraps the `xmtp-sys` FFI bindings with safe, idiomatic Rust types.

## Quick Start

```rust,ignore
use xmtp::{Client, Env};

let client = Client::builder()
    .env(Env::Dev)
    .db_path("./my.db3")
    .build(&my_signer)?;

// Send a message
let conv = client.create_dm("0xRecipient", xmtp::IdentifierKind::Ethereum)?;
conv.send(b"hello")?;
```
