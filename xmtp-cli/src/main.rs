//! xmtp-cli — Interactive XMTP TUI chat client.
//!
//! Uses a separate tokio runtime (`rt.enter()`) to provide a reactor handle
//! without nesting `block_on` calls (the FFI layer owns its own runtime).

#![allow(
    missing_docs,
    missing_debug_implementations,
    clippy::print_stderr,
    clippy::print_stdout
)]

mod app;
mod event;
mod tui;
mod ui;

use std::path::Path;
use std::time::Duration;
use std::{fs, process};

use xmtp::{AlloySigner, Client, Env, stream};

use crate::app::App;
use crate::event::{Event, XmtpEvent};

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    if let Err(e) = run() {
        let _ = tui::restore();
        eprintln!("fatal: {e}");
        process::exit(1);
    }
}

fn run() -> xmtp::Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: xmtp-cli <name>");
        process::exit(1);
    });

    let signer = load_or_create_signer(&format!("{name}.key"))?;
    let address = signer.address();
    eprintln!("address: {address}");

    let client = create_client(&signer, &format!("{name}.db3"))?;
    let inbox_id = client.inbox_id()?;
    eprintln!("inbox: {inbox_id}");

    let _ = client.sync_welcomes();

    let mut app = App::new(address, inbox_id);
    app.refresh_conversations(&client);

    let (rx, tx) = event::channel(Duration::from_millis(50));
    let _streams = start_streams(&client, &tx)?;

    tui::install_panic_hook();
    let mut terminal = tui::init().map_err(|e| xmtp::Error::Ffi(format!("terminal: {e}")))?;

    while !app.quit {
        terminal
            .draw(|f| ui::render(&app, f))
            .map_err(|e| xmtp::Error::Ffi(format!("render: {e}")))?;

        match rx.recv() {
            Ok(Event::Key(key)) => app.handle_key(key, &client),
            Ok(Event::Tick) => app.tick(),
            Ok(Event::Xmtp(ev)) => app.handle_xmtp(ev, &client),
            Ok(Event::Resize) => {}
            Err(_) => break,
        }
    }

    tui::restore().map_err(|e| xmtp::Error::Ffi(format!("restore: {e}")))
}

/// Load a persisted private key or generate a new one (32-byte raw).
fn load_or_create_signer(key_path: &str) -> xmtp::Result<AlloySigner> {
    let key: [u8; 32] = if Path::new(key_path).exists() {
        let bytes = fs::read(key_path).map_err(|e| xmtp::Error::Ffi(format!("read key: {e}")))?;
        bytes
            .try_into()
            .map_err(|_| xmtp::Error::InvalidArgument("key file must be 32 bytes".into()))?
    } else {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| xmtp::Error::Ffi(format!("rng: {e}")))?;
        fs::write(key_path, key).map_err(|e| xmtp::Error::Ffi(format!("write key: {e}")))?;
        key
    };
    AlloySigner::from_bytes(&key)
}

/// Build the XMTP client, clearing stale DB files on inbox ID mismatch.
fn create_client(signer: &AlloySigner, db_path: &str) -> xmtp::Result<Client> {
    match Client::builder()
        .env(Env::Dev)
        .db_path(db_path)
        .build(signer)
    {
        Ok(c) => Ok(c),
        Err(e) if format!("{e}").contains("does not match the stored InboxId") => {
            for ext in ["", "-shm", "-wal"] {
                let _ = fs::remove_file(format!("{db_path}{ext}"));
            }
            Client::builder()
                .env(Env::Dev)
                .db_path(db_path)
                .build(signer)
        }
        Err(e) => Err(e),
    }
}

/// Start XMTP background streams wired to the event channel.
fn start_streams(
    client: &Client,
    tx: &event::Tx,
) -> xmtp::Result<(xmtp::StreamHandle, xmtp::StreamHandle)> {
    let msg_tx = tx.clone();
    let msg_stream = stream::stream_all_messages(client, None, &[], move |msg_id, conv_id| {
        let _ = msg_tx.send(Event::Xmtp(XmtpEvent::Message { msg_id, conv_id }));
    })?;

    let conv_tx = tx.clone();
    let conv_stream = stream::stream_conversations(client, None, move |_| {
        let _ = conv_tx.send(Event::Xmtp(XmtpEvent::Conversation));
    })?;

    Ok((msg_stream, conv_stream))
}
