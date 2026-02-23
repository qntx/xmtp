//! xmtp-cli — Interactive XMTP chat demo.
//!
//! Open two terminals and run:
//! ```text
//! Terminal 1: cargo run -p xmtp-cli -- alice
//! Terminal 2: cargo run -p xmtp-cli -- bob
//! ```
//! Each instance prints its **inbox ID**. Paste the other's inbox ID when
//! prompted, then type messages freely.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::needless_raw_string_hashes
)]

use std::io::{self, BufRead as _, Write as _};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};

use k256::ecdsa::signature::hazmat::PrehashSigner as _;
use k256::ecdsa::{RecoveryId, Signature, SigningKey};
use sha3::{Digest as _, Keccak256};

use xmtp::{
    AccountIdentifier, Client, Env, IdentifierKind, ListMessagesOptions, Signer, SortDirection,
};

fn main() {
    if let Err(e) = run() {
        eprintln!("fatal: {e}");
        std::process::exit(1);
    }
}

fn run() -> xmtp::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| {
        eprintln!("usage: xmtp-cli <name>");
        eprintln!("  e.g. xmtp-cli alice");
        std::process::exit(1);
    });

    let db_path = format!("{name}.db3");
    let key_path = format!("{name}.key");

    // Load or generate an ECDSA identity.
    let signer = load_or_create_signer(&key_path);
    println!("address : {}", signer.address);

    // Create (or reconnect) the XMTP client.
    // If the database contains a different inbox ID (stale from a previous key),
    // delete it and retry with a fresh database.
    let client = match Client::builder()
        .env(Env::Dev)
        .db_path(&db_path)
        .build(&signer)
    {
        Ok(c) => c,
        Err(e) if format!("{e}").contains("does not match the stored InboxId") => {
            eprintln!("stale db detected, recreating...");
            for ext in ["", "-shm", "-wal"] {
                let _ = fs::remove_file(format!("{db_path}{ext}"));
            }
            Client::builder()
                .env(Env::Dev)
                .db_path(&db_path)
                .build(&signer)?
        }
        Err(e) => return Err(e),
    };

    let my_inbox = client.inbox_id()?;
    println!("inbox   : {my_inbox}");
    println!();

    // Ask for the peer's inbox ID.
    print!("peer inbox ID> ");
    io::stdout().flush().ok();
    let mut peer = String::new();
    io::stdin()
        .read_line(&mut peer)
        .map_err(|e| xmtp::Error::InvalidArgument(format!("failed to read stdin: {e}")))?;
    let peer = peer.trim();
    if peer.is_empty() {
        return Err(xmtp::Error::InvalidArgument("empty peer ID".into()));
    }

    // Sync first to discover any existing DM the peer may have already created.
    client.sync_welcomes()?;

    // Try to find an existing DM; only create a new one if none exists.
    // find_dm_by_inbox_id returns Err (not Ok(None)) when no DM exists in the
    // FFI layer, so treat errors containing "not found" as None.
    let existing = client.find_dm_by_inbox_id(peer).unwrap_or(None);

    let conv = if let Some(c) = existing {
        println!("dm      : {} (found)", c.id()?);
        c
    } else {
        let c = client.create_dm_by_inbox_id(peer)?;
        println!("dm      : {} (created)", c.id()?);
        c
    };
    conv.sync()?;
    println!("--- type messages, press Enter to send (Ctrl-C to quit) ---");
    println!();

    // Spawn a background thread to read stdin lines.
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(text) if !text.is_empty() => {
                    if tx.send(text).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });

    // Main chat loop: send user input + poll for new messages.
    let mut last_ns: i64 = 0;

    loop {
        // Drain all pending user input and send it.
        while let Ok(text) = rx.try_recv() {
            conv.send(text.as_bytes())?;
            println!("  \x1b[90m[you]\x1b[0m {text}");
        }

        // Sync welcomes (discover new conversations) + sync this conversation.
        let _ = client.sync_welcomes();
        conv.sync()?;
        let msgs = conv.list_messages(&ListMessagesOptions {
            sent_after_ns: last_ns,
            direction: Some(SortDirection::Ascending),
            ..Default::default()
        })?;

        for msg in &msgs {
            last_ns = msg.sent_at_ns;
            // Skip our own messages (already printed above).
            if msg.sender_inbox_id == my_inbox {
                continue;
            }
            let text = msg.fallback.as_deref().unwrap_or("<no fallback>");
            println!("  \x1b[36m[peer]\x1b[0m {text}");
        }

        thread::sleep(Duration::from_secs(2));
    }
}

struct LocalSigner {
    key: SigningKey,
    address: String,
}

impl Signer for LocalSigner {
    fn identifier(&self) -> AccountIdentifier {
        AccountIdentifier {
            address: self.address.clone(),
            kind: IdentifierKind::Ethereum,
        }
    }

    fn sign(&self, text: &str) -> xmtp::Result<Vec<u8>> {
        // EIP-191 personal sign: keccak256("\x19Ethereum Signed Message:\n" + len + text)
        let prefixed = format!("\x19Ethereum Signed Message:\n{}{}", text.len(), text);
        let hash = Keccak256::digest(prefixed.as_bytes());

        let (sig, recid): (Signature, RecoveryId) = self
            .key
            .sign_prehash(&hash)
            .map_err(|e| xmtp::Error::Ffi(format!("ecdsa: {e}")))?;

        let mut bytes = sig.to_bytes().to_vec(); // r(32) + s(32)
        bytes.push(recid.to_byte()); // v(1)
        Ok(bytes)
    }
}

fn load_or_create_signer(key_path: &str) -> LocalSigner {
    let key = if Path::new(key_path).exists() {
        let bytes = fs::read(key_path).expect("read key file");
        SigningKey::from_bytes(bytes.as_slice().into()).expect("parse key")
    } else {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).expect("generate random key");
        let key = SigningKey::from_bytes(&bytes.into()).expect("create key");
        fs::write(key_path, key.to_bytes()).expect("save key file");
        key
    };
    let address = eth_address(&key);
    LocalSigner { key, address }
}

/// Derive an Ethereum address (0x-prefixed, lowercase hex) from a signing key.
fn eth_address(key: &SigningKey) -> String {
    let pubkey = key.verifying_key().to_encoded_point(false);
    let hash = Keccak256::digest(&pubkey.as_bytes()[1..]); // skip 0x04 prefix
    format!("0x{}", hex::encode(&hash[12..]))
}
