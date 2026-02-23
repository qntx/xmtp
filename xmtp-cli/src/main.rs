//! xmtp-cli — Interactive XMTP TUI chat client.
//!
//! ```text
//! cargo run -p xmtp-cli -- <name>
//! ```
//!
//! # Runtime architecture
//!
//! The FFI layer (libxmtp) owns a global `tokio::runtime::Runtime` and calls
//! `Runtime::block_on` during stream setup.  Nesting another `block_on` (e.g.
//! via `#[tokio::main]`) would panic, so we create a **separate** runtime and
//! only call `rt.enter()` to register a reactor handle for background tasks
//! that need `Handle::current()`.  The main event loop is fully synchronous.

#![allow(
    missing_docs,
    missing_debug_implementations,
    clippy::print_stderr,
    clippy::print_stdout
)]

mod app;
mod event;
mod signer;
mod tui;
mod ui;

use std::time::Duration;
use std::{fs, process};

use xmtp::{stream, Client, Env};

use crate::app::App;
use crate::event::{Event, XmtpEvent};

fn main() {
    // Create a Tokio runtime and enter it so that `Handle::current()` is
    // available on this thread.  We intentionally do NOT call `block_on` —
    // the FFI layer's own runtime calls `block_on` during stream setup and
    // nesting would panic.
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    if let Err(e) = run() {
        let _ = tui::restore();
        eprintln!("fatal: {e}");
        process::exit(1);
    }
}

fn run() -> xmtp::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| {
        eprintln!("usage: xmtp-cli <name>");
        process::exit(1);
    });

    let db_path = format!("{name}.db3");
    let key_path = format!("{name}.key");

    // Load or generate local identity.
    let signer = signer::load_or_create(&key_path);
    eprintln!("address: {}", signer.address);

    // Create XMTP client, recreating DB on inbox ID mismatch.
    let client = create_client(&signer, &db_path)?;

    let my_inbox = client.inbox_id()?;
    eprintln!("inbox: {my_inbox}");

    // Sync existing conversations before entering the TUI.
    let _ = client.sync_welcomes();

    // Initialise app state.
    let mut app = App::new(my_inbox, name);
    app.refresh_conversations(&client);

    // Start the unified event channel (terminal poller + tick).
    let (rx, tx) = event::create(Duration::from_millis(50));

    // Subscribe to XMTP real-time streams.
    let _streams = start_streams(&client, &tx)?;

    // Enter alternate screen.
    tui::install_panic_hook();
    let mut terminal = tui::init().map_err(|e| xmtp::Error::Ffi(format!("terminal: {e}")))?;

    // Main event loop (synchronous — blocking recv on std::sync::mpsc).
    while !app.should_quit {
        terminal
            .draw(|f| ui::render(&app, f))
            .map_err(|e| xmtp::Error::Ffi(format!("render: {e}")))?;

        match rx.recv() {
            Ok(Event::Key(key)) => app.handle_key(key, &client),
            Ok(Event::Tick) => app.tick(),
            Ok(Event::Xmtp(xmtp_ev)) => app.handle_xmtp_event(xmtp_ev, &client),
            Ok(Event::Resize) => {} // ratatui handles resize automatically
            Err(_) => break,        // channel closed
        }
    }

    // Cleanup.
    tui::restore().map_err(|e| xmtp::Error::Ffi(format!("restore: {e}")))?;
    Ok(())
}

/// Build the XMTP client, clearing stale DB files on inbox ID mismatch.
fn create_client(signer: &signer::LocalSigner, db_path: &str) -> xmtp::Result<Client> {
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

/// Start XMTP background streams and wire them to the event channel.
///
/// Returns the stream handles — dropping them stops the streams.
fn start_streams(
    client: &Client,
    tx: &event::EventTx,
) -> xmtp::Result<(xmtp::StreamHandle, xmtp::StreamHandle)> {
    let msg_tx = tx.clone();
    let msg_stream =
        stream::stream_all_messages(client, None, &[], move |msg_id, conv_id| {
            let _ = msg_tx.send(Event::Xmtp(XmtpEvent::NewMessage { msg_id, conv_id }));
        })?;

    let conv_tx = tx.clone();
    let conv_stream = stream::stream_conversations(client, None, move |_| {
        let _ = conv_tx.send(Event::Xmtp(XmtpEvent::NewConversation));
    })?;

    Ok((msg_stream, conv_stream))
}