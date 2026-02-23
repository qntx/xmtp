//! Unified event channel: terminal input, XMTP stream callbacks, and ticks.

use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as CtEvent, KeyEvent, KeyEventKind};

/// Application event.
#[derive(Debug)]
pub enum Event {
    /// A key was pressed.
    Key(KeyEvent),
    /// Terminal was resized.
    Resize,
    /// Periodic tick (~50 ms).
    Tick,
    /// XMTP stream event.
    Xmtp(XmtpEvent),
}

/// Events from XMTP background streams.
#[derive(Debug)]
pub enum XmtpEvent {
    /// New message arrived.
    Message { msg_id: String, conv_id: String },
    /// New conversation received.
    Conversation,
}

/// Sender half — cloneable, passed to stream callbacks.
pub type Tx = mpsc::Sender<Event>;

/// Receiver half — consumed by the main event loop.
pub type Rx = mpsc::Receiver<Event>;

/// Create the event channel and spawn the terminal-polling thread.
pub fn channel(tick_rate: Duration) -> (Rx, Tx) {
    let (tx, rx) = mpsc::channel();
    let poll_tx = tx.clone();
    std::thread::spawn(move || poll(poll_tx, tick_rate));
    (rx, tx)
}

/// Poll crossterm for terminal events on a dedicated thread.
#[allow(clippy::needless_pass_by_value)]
fn poll(tx: Tx, tick: Duration) {
    loop {
        let ok = match event::poll(tick) {
            Ok(true) => event::read().map_or(Ok(()), |ev| match ev {
                CtEvent::Key(k) if k.kind == KeyEventKind::Press => tx.send(Event::Key(k)),
                CtEvent::Resize(_, _) => tx.send(Event::Resize),
                _ => Ok(()),
            }),
            Ok(false) => tx.send(Event::Tick),
            Err(_) => break,
        };
        if ok.is_err() {
            break;
        }
    }
}
