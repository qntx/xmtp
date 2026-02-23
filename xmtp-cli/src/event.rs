//! Unified event system that multiplexes terminal input, XMTP stream
//! callbacks, and periodic ticks into a single `std::sync::mpsc` channel.
//!
//! Uses `std::sync::mpsc` (not `tokio::sync`) so the main loop stays fully
//! synchronous.  This avoids conflicts with the FFI layer's internal Tokio
//! runtime which calls `Runtime::block_on` in stream setup.

use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as CtEvent, KeyEvent, KeyEventKind};

/// Unified application event.
#[derive(Debug)]
pub enum Event {
    /// A key was pressed.
    Key(KeyEvent),
    /// The terminal was resized (handled automatically by ratatui).
    Resize,
    /// Periodic tick for background work.
    Tick,
    /// An XMTP stream event.
    Xmtp(XmtpEvent),
}

/// Events originating from XMTP background streams.
#[derive(Debug)]
pub enum XmtpEvent {
    /// A new message arrived: `(message_id_hex, conversation_id_hex)`.
    NewMessage { msg_id: String, conv_id: String },
    /// A new conversation was received (e.g. group invitation).
    NewConversation,
}

/// Sender half — cloneable, passed to stream callbacks.
pub type EventTx = mpsc::Sender<Event>;

/// Receiver half — consumed by the main loop.
pub type EventRx = mpsc::Receiver<Event>;

/// Create the event channel and spawn the terminal-polling thread.
///
/// Returns `(receiver, sender)`.  The sender is cloned for XMTP stream
/// callbacks; the receiver is consumed by the main event loop.
pub fn create(tick_rate: Duration) -> (EventRx, EventTx) {
    let (tx, rx) = mpsc::channel();

    let term_tx = tx.clone();
    std::thread::spawn(move || poll_terminal(term_tx, tick_rate));

    (rx, tx)
}

/// Poll crossterm for terminal events on a dedicated thread.
/// Sends `Tick` when no event arrives within `tick_rate`.
#[allow(clippy::needless_pass_by_value)] // ownership required — runs in spawned thread
fn poll_terminal(tx: EventTx, tick_rate: Duration) {
    loop {
        match event::poll(tick_rate) {
            Ok(true) => {
                if let Ok(ev) = event::read() {
                    let sent = match ev {
                        CtEvent::Key(key) if key.kind == KeyEventKind::Press => {
                            tx.send(Event::Key(key))
                        }
                        CtEvent::Resize(_, _) => tx.send(Event::Resize),
                        _ => Ok(()),
                    };
                    if sent.is_err() {
                        break;
                    }
                }
            }
            Ok(false) => {
                if tx.send(Event::Tick).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
