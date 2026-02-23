//! Core application state, input handling, and XMTP integration.
//!
//! The [`App`] struct owns all mutable state. Every key press, stream event, or
//! tick is dispatched through a single method so that state transitions remain
//! deterministic and easy to test.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use xmtp::content::Content;
use xmtp::{
    Client, Conversation, ConversationOrderBy, ConversationType, ListConversationsOptions,
    ListMessagesOptions, Message, MessageKind, SortDirection,
};

use crate::event::XmtpEvent;

// ── Enums ────────────────────────────────────────────────────────────────────

/// Which panel currently holds keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Conversation list sidebar.
    Sidebar,
    /// Message compose area.
    Input,
}

/// Top-level interaction mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal chat browsing.
    Normal,
    /// Composing a new DM (input captures the peer inbox ID).
    NewDm,
    /// Showing the help overlay.
    Help,
}

// ── Conversation entry ───────────────────────────────────────────────────────

/// Lightweight representation of a conversation for the sidebar.
pub struct ConvEntry {
    /// Hex-encoded conversation ID.
    pub id: String,
    /// Display label (peer address, group name, or truncated inbox ID).
    pub label: String,
    /// Short preview of the last message.
    pub preview: String,
    /// Timestamp of last activity (nanoseconds since epoch).
    pub last_ns: i64,
    /// Whether the conversation type is a group.
    pub is_group: bool,
    /// Whether new unread messages exist.
    pub unread: bool,
}

// ── App ──────────────────────────────────────────────────────────────────────

/// Central application state.
pub struct App {
    // -- Lifecycle --
    /// Set to `true` when the application should exit.
    pub should_quit: bool,

    // -- Identity --
    /// Human-readable name passed via CLI.
    pub name: String,
    /// This client's XMTP inbox ID.
    pub my_inbox_id: String,

    // -- UI state --
    /// Active focus panel.
    pub focus: Focus,
    /// Current interaction mode.
    pub mode: Mode,
    /// Selected sidebar index.
    pub sidebar_idx: usize,
    /// Chat-area scroll offset from the bottom (0 = pinned to latest).
    pub scroll_offset: usize,
    /// Whether the scroll is locked to the bottom (auto-scroll on new msg).
    pub scroll_pinned: bool,

    // -- Conversation state --
    /// Sidebar entries, ordered by last activity.
    pub conversations: Vec<ConvEntry>,
    /// Hex ID of the active conversation, if any.
    pub active_conv_id: Option<String>,
    /// Handle to the active conversation (kept alive for sends / syncs).
    active_conv: Option<Conversation>,
    /// Messages loaded for the active conversation (ascending time order).
    pub messages: Vec<Message>,

    // -- Input state (Unicode-aware, operates on `char` indices) --
    /// Text input buffer.
    pub input: String,
    /// Cursor position in *characters* (not bytes).
    pub input_cursor: usize,

    // -- Status / error --
    /// Status line shown at the bottom.
    pub status: String,
    /// Remaining ticks before the status line reverts to the default hint.
    status_ttl: u16,
}

// ── Constants ────────────────────────────────────────────────────────────────

const STATUS_DEFAULT: &str =
    " Tab:panel  j/k:nav  Enter:open  n:new DM  ?:help  q:quit";
const STATUS_INPUT: &str =
    " Enter:send  Esc:cancel  Tab:sidebar  n:new DM  ?:help";
const STATUS_NEW_DM: &str =
    " Enter peer inbox-id  ·  Enter:create  Esc:cancel";
const STATUS_TTL: u16 = 60; // ~3 s at 50 ms tick

impl App {
    /// Construct a fresh application state.
    pub fn new(my_inbox_id: String, name: String) -> Self {
        Self {
            should_quit: false,
            name,
            my_inbox_id,
            focus: Focus::Sidebar,
            mode: Mode::Normal,
            sidebar_idx: 0,
            scroll_offset: 0,
            scroll_pinned: true,
            conversations: Vec::new(),
            active_conv_id: None,
            active_conv: None,
            messages: Vec::new(),
            input: String::new(),
            input_cursor: 0,
            status: STATUS_DEFAULT.into(),
            status_ttl: 0,
        }
    }

    // ── Tick ──────────────────────────────────────────────────────────────────

    /// Called on every tick event; drives timed state transitions.
    pub fn tick(&mut self) {
        if self.status_ttl > 0 {
            self.status_ttl -= 1;
            if self.status_ttl == 0 {
                self.restore_default_status();
            }
        }
    }

    // ── Conversation list ────────────────────────────────────────────────────

    /// Reload the sidebar conversation list from the XMTP client.
    pub fn refresh_conversations(&mut self, client: &Client) {
        let opts = ListConversationsOptions {
            order_by: ConversationOrderBy::LastActivity,
            ..Default::default()
        };
        let convs = client.list_conversations(&opts).unwrap_or_default();
        self.conversations.clear();
        for conv in &convs {
            let id = conv.id().unwrap_or_default();
            let is_group = conv.conversation_type() == Some(ConversationType::Group);
            let label = if is_group {
                conv.name().unwrap_or_else(|| format!("Group {}", truncate_id(&id, 8)))
            } else {
                conv.dm_peer_inbox_id()
                    .map_or_else(|| "unknown".into(), |s| truncate_id(&s, 16))
            };
            let last = conv.last_message().ok().flatten();
            let preview = last.as_ref().map_or(String::new(), decode_preview);
            let last_ns = last.as_ref().map_or(0, |m| m.sent_at_ns);
            self.conversations.push(ConvEntry {
                id,
                label,
                preview,
                last_ns,
                is_group,
                unread: false,
            });
        }
        // Restore sidebar selection to the active conversation.
        if let Some(ref active) = self.active_conv_id
            && let Some(pos) = self.conversations.iter().position(|c| c.id == *active)
        {
            self.sidebar_idx = pos;
        }
        self.clamp_sidebar();
    }

    /// Open the conversation at the current sidebar index.
    fn open_selected_conversation(&mut self, client: &Client) {
        let Some(entry) = self.conversations.get(self.sidebar_idx) else {
            return;
        };
        let conv_id = entry.id.clone();
        // Skip if already active.
        if self.active_conv_id.as_deref() == Some(&conv_id) {
            return;
        }
        self.active_conv_id = Some(conv_id.clone());
        if let Some(e) = self.conversations.get_mut(self.sidebar_idx) {
            e.unread = false;
        }
        if let Ok(Some(conv)) = client.conversation(&conv_id) {
            let _ = conv.sync();
            self.messages = conv
                .list_messages(&ListMessagesOptions {
                    direction: Some(SortDirection::Ascending),
                    ..Default::default()
                })
                .unwrap_or_default();
            self.active_conv = Some(conv);
        } else {
            self.messages.clear();
            self.active_conv = None;
        }
        self.scroll_offset = 0;
        self.scroll_pinned = true;
    }

    /// Reload messages for the currently active conversation.
    fn reload_messages(&mut self) {
        let msgs = self.active_conv.as_ref().and_then(|conv| {
            let _ = conv.sync();
            conv.list_messages(&ListMessagesOptions {
                direction: Some(SortDirection::Ascending),
                ..Default::default()
            })
            .ok()
        });
        if let Some(m) = msgs {
            self.messages = m;
        }
        if self.scroll_pinned {
            self.scroll_offset = 0;
        }
    }

    const fn clamp_sidebar(&mut self) {
        if self.conversations.is_empty() {
            self.sidebar_idx = 0;
        } else if self.sidebar_idx >= self.conversations.len() {
            self.sidebar_idx = self.conversations.len() - 1;
        }
    }

    // ── Key dispatch ─────────────────────────────────────────────────────────

    /// Top-level key handler. Routes to mode-specific handlers.
    pub fn handle_key(&mut self, key: KeyEvent, client: &Client) {
        // Ctrl-C: unconditional quit.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match self.mode {
            Mode::Help => self.handle_key_help(key),
            Mode::NewDm => self.handle_key_new_dm(key, client),
            Mode::Normal => match self.focus {
                Focus::Sidebar => self.handle_key_sidebar(key, client),
                Focus::Input => self.handle_key_input(key),
            },
        }
    }

    fn handle_key_help(&mut self, key: KeyEvent) {
        // Any key dismisses help.
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | '?') | KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.restore_default_status();
            }
            _ => {}
        }
    }

    fn handle_key_sidebar(&mut self, key: KeyEvent, client: &Client) {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
            }
            // Navigation
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.conversations.is_empty() {
                    self.sidebar_idx = (self.sidebar_idx + 1) % self.conversations.len();
                    self.open_selected_conversation(client);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if !self.conversations.is_empty() {
                    self.sidebar_idx = self
                        .sidebar_idx
                        .checked_sub(1)
                        .unwrap_or(self.conversations.len() - 1);
                    self.open_selected_conversation(client);
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if !self.conversations.is_empty() {
                    self.sidebar_idx = 0;
                    self.open_selected_conversation(client);
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if !self.conversations.is_empty() {
                    self.sidebar_idx = self.conversations.len() - 1;
                    self.open_selected_conversation(client);
                }
            }
            // Focus switch
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                if self.active_conv.is_some() {
                    self.focus = Focus::Input;
                    self.restore_default_status();
                }
            }
            // New DM
            KeyCode::Char('n') => {
                self.mode = Mode::NewDm;
                self.input.clear();
                self.input_cursor = 0;
                self.status = STATUS_NEW_DM.into();
            }
            // Sync
            KeyCode::Char('r') => {
                let _ = client.sync_welcomes();
                self.refresh_conversations(client);
                if self.active_conv.is_some() {
                    self.reload_messages();
                }
                self.flash_status("Synced");
            }
            _ => {}
        }
    }

    fn handle_key_input(&mut self, key: KeyEvent) {
        match key.code {
            // Focus / mode
            KeyCode::Tab | KeyCode::Esc => {
                self.focus = Focus::Sidebar;
                self.restore_default_status();
            }
            KeyCode::Char('?') if self.input.is_empty() => {
                self.mode = Mode::Help;
            }
            KeyCode::Char('n') if self.input.is_empty() => {
                self.mode = Mode::NewDm;
                self.input.clear();
                self.input_cursor = 0;
                self.status = STATUS_NEW_DM.into();
            }
            // Send
            KeyCode::Enter => self.action_send(),
            // Scroll chat
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            // Text editing (Unicode-aware)
            KeyCode::Backspace => self.input_backspace(),
            KeyCode::Delete => self.input_delete(),
            KeyCode::Left => self.input_cursor = self.input_cursor.saturating_sub(1),
            KeyCode::Right => {
                let max = self.input.chars().count();
                if self.input_cursor < max {
                    self.input_cursor += 1;
                }
            }
            KeyCode::Home => self.input_cursor = 0,
            KeyCode::End => self.input_cursor = self.input.chars().count(),
            KeyCode::Char(c) => self.input_insert(c),
            _ => {}
        }
    }

    fn handle_key_new_dm(&mut self, key: KeyEvent, client: &Client) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.input.clear();
                self.input_cursor = 0;
                self.restore_default_status();
            }
            KeyCode::Enter => {
                let peer = self.input.trim().to_owned();
                if !peer.is_empty() {
                    self.action_create_dm(client, &peer);
                }
                self.mode = Mode::Normal;
                self.input.clear();
                self.input_cursor = 0;
                self.restore_default_status();
            }
            KeyCode::Backspace => self.input_backspace(),
            KeyCode::Delete => self.input_delete(),
            KeyCode::Left => self.input_cursor = self.input_cursor.saturating_sub(1),
            KeyCode::Right => {
                let max = self.input.chars().count();
                if self.input_cursor < max {
                    self.input_cursor += 1;
                }
            }
            KeyCode::Home => self.input_cursor = 0,
            KeyCode::End => self.input_cursor = self.input.chars().count(),
            KeyCode::Char(c) => self.input_insert(c),
            _ => {}
        }
    }

    // ── Unicode-aware input helpers ──────────────────────────────────────────

    /// Insert a character at the cursor (char index).
    fn input_insert(&mut self, ch: char) {
        let byte_idx = char_to_byte(&self.input, self.input_cursor);
        self.input.insert(byte_idx, ch);
        self.input_cursor += 1;
    }

    /// Delete the character before the cursor.
    fn input_backspace(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
            let byte_idx = char_to_byte(&self.input, self.input_cursor);
            self.input.remove(byte_idx);
        }
    }

    /// Delete the character at the cursor.
    fn input_delete(&mut self) {
        let len = self.input.chars().count();
        if self.input_cursor < len {
            let byte_idx = char_to_byte(&self.input, self.input_cursor);
            self.input.remove(byte_idx);
        }
    }

    // ── Scroll ───────────────────────────────────────────────────────────────

    /// Scroll the chat view up by `n` lines.
    pub const fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
        self.scroll_pinned = false;
    }

    /// Scroll the chat view down by `n` lines (towards latest).
    pub const fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        if self.scroll_offset == 0 {
            self.scroll_pinned = true;
        }
    }

    // ── Actions ──────────────────────────────────────────────────────────────

    fn action_send(&mut self) {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Some(ref conv) = self.active_conv {
            match conv.send_text(&text) {
                Ok(_) => {
                    self.input.clear();
                    self.input_cursor = 0;
                    self.reload_messages();
                }
                Err(e) => self.flash_status(&format!("Send failed: {e}")),
            }
        }
    }

    fn action_create_dm(&mut self, client: &Client, peer: &str) {
        match client.create_dm_by_inbox_id(peer) {
            Ok(conv) => {
                let id = conv.id().unwrap_or_default();
                self.active_conv_id = Some(id);
                self.active_conv = Some(conv);
                self.reload_messages();
                self.refresh_conversations(client);
                self.focus = Focus::Input;
                self.flash_status("DM created");
            }
            Err(e) => self.flash_status(&format!("DM failed: {e}")),
        }
    }

    // ── XMTP stream events ──────────────────────────────────────────────────

    /// Process an incoming XMTP stream event.
    pub fn handle_xmtp_event(&mut self, event: XmtpEvent, client: &Client) {
        match event {
            XmtpEvent::NewMessage { conv_id, msg_id } => {
                let is_active = self.active_conv_id.as_deref() == Some(&conv_id);
                if is_active {
                    self.reload_messages();
                }
                // Update sidebar entry.
                if let Ok(Some(msg)) = client.message_by_id(&msg_id) {
                    for entry in &mut self.conversations {
                        if entry.id == conv_id {
                            entry.preview = decode_preview(&msg);
                            entry.last_ns = msg.sent_at_ns;
                            if !is_active {
                                entry.unread = true;
                            }
                        }
                    }
                }
            }
            XmtpEvent::NewConversation => {
                let _ = client.sync_welcomes();
                self.refresh_conversations(client);
            }
        }
    }

    // ── Status helpers ───────────────────────────────────────────────────────

    /// Show a temporary status message that auto-reverts.
    fn flash_status(&mut self, msg: &str) {
        self.status = format!(" {msg}");
        self.status_ttl = STATUS_TTL;
    }

    fn restore_default_status(&mut self) {
        self.status = match self.focus {
            Focus::Sidebar => STATUS_DEFAULT,
            Focus::Input => STATUS_INPUT,
        }
        .into();
        self.status_ttl = 0;
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a char-index to a byte-index in a UTF-8 string.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or(s.len(), |(i, _)| i)
}

/// Decode a message to a short preview string for the sidebar.
fn decode_preview(msg: &Message) -> String {
    if msg.kind != MessageKind::Application {
        return String::new();
    }
    match msg.decode() {
        Ok(Content::Text(s) | Content::Markdown(s)) => truncate_str(&s, 28),
        Ok(Content::Reaction(r)) => truncate_str(&r.content, 28),
        Ok(Content::ReadReceipt) => String::new(),
        Ok(Content::Reply(r)) => reply_text_preview(&r.content, 28),
        Ok(Content::Attachment(a)) => {
            let name = a.filename.as_deref().unwrap_or("file");
            format!("[file: {}]", truncate_str(name, 20))
        }
        Ok(Content::RemoteAttachment(_)) => "[attachment]".into(),
        Ok(Content::Unknown { .. }) | Err(_) => {
            msg.fallback.clone().unwrap_or_default()
        }
    }
}

/// Decode full message content for the chat view.
pub fn message_body(msg: &Message) -> String {
    match msg.decode() {
        Ok(Content::Text(s) | Content::Markdown(s)) => s,
        Ok(Content::Reaction(r)) => format!("[{}]", r.content),
        Ok(Content::ReadReceipt) => "[read]".into(),
        Ok(Content::Reply(r)) => reply_text(&r.content),
        Ok(Content::Attachment(a)) => {
            format!("[file: {}]", a.filename.as_deref().unwrap_or("file"))
        }
        Ok(Content::RemoteAttachment(_)) => "[remote attachment]".into(),
        Ok(Content::Unknown { content_type, .. }) => format!("[unknown: {content_type}]"),
        Err(_) => msg.fallback.clone().unwrap_or_default(),
    }
}

/// Extract display text from a reply's inner `EncodedContent`.
fn reply_text(ec: &xmtp::content::EncodedContent) -> String {
    let is_text = ec
        .r#type
        .as_ref()
        .is_some_and(|t| t.type_id == "text" || t.type_id == "markdown");
    if is_text {
        String::from_utf8(ec.content.clone()).unwrap_or_else(|_| "[reply]".into())
    } else {
        "[reply]".into()
    }
}

/// Extract a truncated preview from a reply's inner `EncodedContent`.
fn reply_text_preview(ec: &xmtp::content::EncodedContent, max: usize) -> String {
    truncate_str(&reply_text(ec), max)
}

/// Truncate a string to `max` chars, appending `…` if needed.
fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// Truncate an identifier for display (e.g. `0x1a2b…c3d4`).
pub fn truncate_id(id: &str, max: usize) -> String {
    if id.len() <= max {
        id.to_owned()
    } else {
        let half = max.saturating_sub(1) / 2;
        format!("{}…{}", &id[..half], &id[id.len() - half..])
    }
}
