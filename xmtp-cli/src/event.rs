//! Event types, command types, shared transport structs, and channel helpers.
//!
//! Architecture: the main thread only handles UI rendering and key input.
//! All FFI / network operations run on a dedicated **worker thread**.
//!
//! - Stream callbacks → `CmdTx` → Worker (via [`Cmd::NewMessage`] / [`Cmd::NewConversation`])
//! - App key handling → `CmdTx` → Worker (via [`Cmd::Send`], [`Cmd::Refresh`], etc.)
//! - Worker results  → `Tx`    → Main thread (via [`Event::Conversations`], [`Event::Messages`], etc.)

use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event as CtEvent, KeyEvent, KeyEventKind};
use xmtp::{ConsentState, Message, PermissionLevel};

/// Event sender (terminal poller + worker → main thread).
pub type Tx = mpsc::Sender<Event>;

/// Command sender (App + stream callbacks → worker thread).
pub type CmdTx = mpsc::Sender<Cmd>;

/// Sidebar conversation entry (display-only, no FFI handles).
#[derive(Debug, Clone)]
pub struct ConvEntry {
    pub id: String,
    pub label: String,
    pub preview: String,
    pub last_ns: i64,
    pub unread: bool,
}

/// Group member entry for display.
#[derive(Debug, Clone)]
pub struct MemberEntry {
    pub inbox_id: String,
    pub address: String,
    pub permission: PermissionLevel,
}

/// Events consumed by the main loop. Worker results are non-blocking.
#[derive(Debug)]
pub enum Event {
    /// Terminal key press.
    Key(KeyEvent),
    /// Terminal resize.
    Resize,
    /// Periodic tick (~50 ms).
    Tick,
    /// Worker: sidebar lists refreshed.
    Conversations {
        inbox: Vec<ConvEntry>,
        requests: Vec<ConvEntry>,
    },
    /// Worker: messages loaded (includes `conv_id` to prevent stale updates).
    Messages { conv_id: String, msgs: Vec<Message> },
    /// Worker: single sidebar preview updated.
    Preview {
        conv_id: String,
        text: String,
        time_ns: i64,
        unread: bool,
    },
    /// Worker: group members loaded.
    Members(Vec<MemberEntry>),
    /// Worker: DM/Group created — UI should switch to it.
    Created { conv_id: String },
    /// Worker: flash status message.
    Flash(String),
}

/// Commands sent from UI thread (or stream callbacks) to the worker thread.
#[derive(Debug)]
pub enum Cmd {
    /// Open conversation by ID, load its messages.
    Open(String),
    /// Send text in the active conversation.
    Send(String),
    /// Create DM with a recipient (address, ENS name, or inbox ID).
    CreateDm(String),
    /// Create group with optional name and member recipients.
    CreateGroup {
        name: Option<String>,
        addrs: Vec<String>,
    },
    /// Update consent state for a conversation.
    SetConsent { id: String, state: ConsentState },
    /// Remove a member from the active group by inbox ID.
    RemoveMember(String),
    /// Toggle admin status for a member in the active group.
    ToggleAdmin(String),
    /// Full network sync (welcomes + refresh + active reload).
    Sync,
    /// Load members for the active conversation.
    LoadMembers,
    /// Stream callback: new message arrived.
    NewMessage { msg_id: String, conv_id: String },
    /// Stream callback: new conversation received.
    NewConversation,
}

/// Spawn the terminal-polling thread. Sends [`Event::Key`], [`Event::Resize`], [`Event::Tick`].
pub fn spawn_poller(tx: Tx, tick: Duration) {
    std::thread::spawn(move || {
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
    });
}
