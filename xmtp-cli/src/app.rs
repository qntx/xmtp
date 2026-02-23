//! Application state, input handling, and XMTP integration.
//!
//! Key design decisions following XMTP best practices:
//! - Sidebar split into **Inbox** (consent: Allowed) and **Requests** (consent: Unknown).
//! - DMs created by **wallet address** (`0x…`), with `canMessage` pre-check.
//! - Group creation by wallet addresses, resolved to inbox IDs automatically.
//! - Accept/Reject actions on message requests.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use xmtp::content::Content;
use xmtp::{
    AccountIdentifier, Client, ConsentState, Conversation, ConversationOrderBy, ConversationType,
    CreateGroupOptions, DeliveryStatus, IdentifierKind, ListConversationsOptions,
    ListMessagesOptions, Message, MessageKind, SortDirection,
};

use crate::event::XmtpEvent;

// ── Enums ────────────────────────────────────────────────────────

/// Active sidebar tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Allowed conversations (main inbox).
    Inbox,
    /// Unknown conversations (message requests).
    Requests,
}

/// Active panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Input,
}

/// Interaction mode overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal browsing.
    Normal,
    /// Creating a new DM (input captures wallet address).
    NewDm,
    /// Creating a new group (input captures comma-separated addresses).
    NewGroup,
    /// Viewing group members.
    Members,
    /// Help overlay.
    Help,
}

// ── Sidebar entry ────────────────────────────────────────────────

/// Sidebar conversation entry with pre-resolved display fields.
pub struct ConvEntry {
    pub id: String,
    pub label: String,
    pub preview: String,
    pub last_ns: i64,
    pub is_group: bool,
    pub unread: bool,
}

// ── Status hints ─────────────────────────────────────────────────

const HINT_SIDEBAR: &str =
    " Tab:input  j/k:nav  1/2:tab  Enter:open  n:DM  g:group  r:sync  ?:help  q:quit";
const HINT_INPUT: &str = " Enter:send  Esc:sidebar  PgUp/Dn:scroll  m:members";
const HINT_NEW_DM: &str = " Enter wallet address (0x…)  Enter:create  Esc:cancel";
const HINT_NEW_GROUP: &str = " Addresses comma-separated (0x…,0x…)  Enter:create  Esc:cancel";
const HINT_REQUESTS: &str = " j/k:nav  a:accept  x:reject  Enter:preview  1/2:tab  q:quit";
const HINT_MEMBERS: &str = " Esc:close";
const FLASH_TTL: u16 = 60; // ~3 s at 50 ms tick

// ── App ──────────────────────────────────────────────────────────

/// Central application state.
pub struct App {
    pub quit: bool,
    pub address: String,
    pub inbox_id: String,

    pub tab: Tab,
    pub focus: Focus,
    pub mode: Mode,
    pub sidebar_idx: usize,
    pub scroll: usize,
    pub scroll_pinned: bool,

    /// Inbox conversations (consent: Allowed).
    pub inbox: Vec<ConvEntry>,
    /// Request conversations (consent: Unknown).
    pub requests: Vec<ConvEntry>,

    pub active_id: Option<String>,
    active_conv: Option<Conversation>,
    pub messages: Vec<Message>,

    /// Group members (populated in Members mode).
    pub members: Vec<MemberEntry>,

    pub input: String,
    pub cursor: usize,

    pub status: String,
    status_ttl: u16,
}

/// Simplified member entry for display.
pub struct MemberEntry {
    pub address: String,
    pub role: &'static str,
}

impl App {
    pub fn new(address: String, inbox_id: String) -> Self {
        Self {
            quit: false,
            address,
            inbox_id,
            tab: Tab::Inbox,
            focus: Focus::Sidebar,
            mode: Mode::Normal,
            sidebar_idx: 0,
            scroll: 0,
            scroll_pinned: true,
            inbox: Vec::new(),
            requests: Vec::new(),
            active_id: None,
            active_conv: None,
            messages: Vec::new(),
            members: Vec::new(),
            input: String::new(),
            cursor: 0,
            status: HINT_SIDEBAR.into(),
            status_ttl: 0,
        }
    }

    /// The active sidebar list for the current tab.
    pub fn sidebar(&self) -> &[ConvEntry] {
        match self.tab {
            Tab::Inbox => &self.inbox,
            Tab::Requests => &self.requests,
        }
    }

    const fn sidebar_mut(&mut self) -> &mut Vec<ConvEntry> {
        match self.tab {
            Tab::Inbox => &mut self.inbox,
            Tab::Requests => &mut self.requests,
        }
    }

    // ── Tick ──────────────────────────────────────────────────────

    pub fn tick(&mut self) {
        if self.status_ttl > 0 {
            self.status_ttl -= 1;
            if self.status_ttl == 0 {
                self.set_default_status();
            }
        }
    }

    // ── Conversations ────────────────────────────────────────────

    /// Refresh both Inbox and Requests from the network.
    pub fn refresh_conversations(&mut self, client: &Client) {
        self.inbox = load_conversations(client, &[ConsentState::Allowed], &self.inbox_id);
        self.requests = load_conversations(client, &[ConsentState::Unknown], &self.inbox_id);
        self.clamp_sidebar();
    }

    fn open_selected(&mut self, client: &Client) {
        let list = self.sidebar();
        let Some(entry) = list.get(self.sidebar_idx) else {
            return;
        };
        let id = entry.id.clone();
        if self.active_id.as_deref() == Some(&id) {
            return;
        }
        self.active_id = Some(id.clone());
        // Mark read in sidebar.
        let idx = self.sidebar_idx;
        if let Some(e) = self.sidebar_mut().get_mut(idx) {
            e.unread = false;
        }
        if let Ok(Some(conv)) = client.conversation(&id) {
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
        self.scroll = 0;
        self.scroll_pinned = true;
    }

    fn reload_messages(&mut self) {
        if let Some(ref conv) = self.active_conv {
            let _ = conv.sync();
            if let Ok(msgs) = conv.list_messages(&ListMessagesOptions {
                direction: Some(SortDirection::Ascending),
                ..Default::default()
            }) {
                self.messages = msgs;
            }
        }
        if self.scroll_pinned {
            self.scroll = 0;
        }
    }

    fn clamp_sidebar(&mut self) {
        let len = self.sidebar().len();
        if len == 0 {
            self.sidebar_idx = 0;
        } else if self.sidebar_idx >= len {
            self.sidebar_idx = len - 1;
        }
    }

    // ── Key dispatch ─────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent, client: &Client) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Help => self.key_help(key),
            Mode::NewDm => self.key_new_dm(key, client),
            Mode::NewGroup => self.key_new_group(key, client),
            Mode::Members => self.key_members(key),
            Mode::Normal => match self.focus {
                Focus::Sidebar => self.key_sidebar(key, client),
                Focus::Input => self.key_input(key),
            },
        }
    }

    fn key_help(&mut self, key: KeyEvent) {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q' | '?') | KeyCode::Enter
        ) {
            self.mode = Mode::Normal;
            self.set_default_status();
        }
    }

    fn key_sidebar(&mut self, key: KeyEvent, client: &Client) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
            }
            // Tab switching: 1=Inbox, 2=Requests
            KeyCode::Char('1') => self.switch_tab(Tab::Inbox),
            KeyCode::Char('2') => self.switch_tab(Tab::Requests),
            // Navigation
            KeyCode::Char('j') | KeyCode::Down => self.nav_down(client),
            KeyCode::Char('k') | KeyCode::Up => self.nav_up(client),
            KeyCode::Char('h') | KeyCode::Home => {
                if !self.sidebar().is_empty() {
                    self.sidebar_idx = 0;
                    self.open_selected(client);
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                let len = self.sidebar().len();
                if len > 0 {
                    self.sidebar_idx = len - 1;
                    self.open_selected(client);
                }
            }
            // Enter input mode
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                if self.active_conv.is_some() {
                    self.focus = Focus::Input;
                    self.set_default_status();
                }
            }
            // Accept/Reject (only in Requests tab)
            KeyCode::Char('a') if self.tab == Tab::Requests => {
                self.accept_request(client);
            }
            KeyCode::Char('x') if self.tab == Tab::Requests => {
                self.reject_request(client);
            }
            // New DM by wallet address
            KeyCode::Char('n') => {
                self.mode = Mode::NewDm;
                self.input.clear();
                self.cursor = 0;
                self.status = HINT_NEW_DM.into();
            }
            // New group
            KeyCode::Char('g') => {
                self.mode = Mode::NewGroup;
                self.input.clear();
                self.cursor = 0;
                self.status = HINT_NEW_GROUP.into();
            }
            // Sync
            KeyCode::Char('r') => {
                let _ = client.sync_welcomes();
                self.refresh_conversations(client);
                if self.active_conv.is_some() {
                    self.reload_messages();
                }
                self.flash("Synced");
            }
            _ => {}
        }
    }

    fn key_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab | KeyCode::Esc => {
                self.focus = Focus::Sidebar;
                self.set_default_status();
            }
            KeyCode::Char('m') if self.input.is_empty() => self.show_members(),
            KeyCode::Enter => self.send_message(),
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::Backspace => self.input_backspace(),
            KeyCode::Delete => self.input_delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                let max = self.input.chars().count();
                if self.cursor < max {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Char(c) => self.input_insert(c),
            _ => {}
        }
    }

    fn key_new_dm(&mut self, key: KeyEvent, client: &Client) {
        match key.code {
            KeyCode::Esc => self.cancel_overlay(),
            KeyCode::Enter => {
                let addr = self.input.trim().to_owned();
                if !addr.is_empty() {
                    self.create_dm_by_address(client, &addr);
                }
                self.cancel_overlay();
            }
            _ => self.overlay_edit(key),
        }
    }

    fn key_new_group(&mut self, key: KeyEvent, client: &Client) {
        match key.code {
            KeyCode::Esc => self.cancel_overlay(),
            KeyCode::Enter => {
                let raw = self.input.trim().to_owned();
                if !raw.is_empty() {
                    self.create_group_by_addresses(client, &raw);
                }
                self.cancel_overlay();
            }
            _ => self.overlay_edit(key),
        }
    }

    fn key_members(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            self.members.clear();
            self.set_default_status();
        }
    }

    /// Shared overlay text editing for NewDm/NewGroup modes.
    fn overlay_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => self.input_backspace(),
            KeyCode::Delete => self.input_delete(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                let max = self.input.chars().count();
                if self.cursor < max {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Char(c) => self.input_insert(c),
            _ => {}
        }
    }

    fn cancel_overlay(&mut self) {
        self.mode = Mode::Normal;
        self.input.clear();
        self.cursor = 0;
        self.set_default_status();
    }

    // ── Navigation ───────────────────────────────────────────────

    fn nav_down(&mut self, client: &Client) {
        let len = self.sidebar().len();
        if len > 0 {
            self.sidebar_idx = (self.sidebar_idx + 1) % len;
            self.open_selected(client);
        }
    }

    fn nav_up(&mut self, client: &Client) {
        let len = self.sidebar().len();
        if len > 0 {
            self.sidebar_idx = self.sidebar_idx.checked_sub(1).unwrap_or(len - 1);
            self.open_selected(client);
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.sidebar_idx = 0;
            self.active_id = None;
            self.active_conv = None;
            self.messages.clear();
            self.set_default_status();
        }
    }

    // ── Unicode-aware input helpers ──────────────────────────────

    fn input_insert(&mut self, ch: char) {
        let idx = char_byte_offset(&self.input, self.cursor);
        self.input.insert(idx, ch);
        self.cursor += 1;
    }

    fn input_backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let idx = char_byte_offset(&self.input, self.cursor);
            self.input.remove(idx);
        }
    }

    fn input_delete(&mut self) {
        if self.cursor < self.input.chars().count() {
            let idx = char_byte_offset(&self.input, self.cursor);
            self.input.remove(idx);
        }
    }

    // ── Scroll ───────────────────────────────────────────────────

    pub const fn scroll_up(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
        self.scroll_pinned = false;
    }

    pub const fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
        if self.scroll == 0 {
            self.scroll_pinned = true;
        }
    }

    // ── Actions: DM by wallet address ────────────────────────────

    fn create_dm_by_address(&mut self, client: &Client, address: &str) {
        // Step 1: canMessage pre-check (XMTP best practice).
        let id = AccountIdentifier {
            address: address.to_owned(),
            kind: IdentifierKind::Ethereum,
        };
        match client.can_message(&[id]) {
            Ok(results) if results.first() == Some(&true) => {}
            Ok(_) => {
                self.flash("Address not registered on XMTP");
                return;
            }
            Err(e) => {
                self.flash(&format!("canMessage failed: {e}"));
                return;
            }
        }
        // Step 2: Create DM by address (SDK resolves address → inbox ID).
        match client.create_dm(address, IdentifierKind::Ethereum) {
            Ok(conv) => {
                let id = conv.id().unwrap_or_default();
                // Auto-accept: set consent to Allowed (user initiated).
                let _ = conv.set_consent(ConsentState::Allowed);
                self.active_id = Some(id);
                self.active_conv = Some(conv);
                self.reload_messages();
                self.refresh_conversations(client);
                self.tab = Tab::Inbox;
                self.focus = Focus::Input;
                self.flash("DM created");
            }
            Err(e) => self.flash(&format!("DM failed: {e}")),
        }
    }

    // ── Actions: Group by wallet addresses ───────────────────────

    fn create_group_by_addresses(&mut self, client: &Client, raw: &str) {
        let addrs: Vec<AccountIdentifier> = raw
            .split(',')
            .map(|s| AccountIdentifier {
                address: s.trim().to_owned(),
                kind: IdentifierKind::Ethereum,
            })
            .filter(|a| !a.address.is_empty())
            .collect();

        if addrs.is_empty() {
            self.flash("No addresses provided");
            return;
        }

        // canMessage pre-check for all addresses.
        match client.can_message(&addrs) {
            Ok(results) => {
                let unreachable: Vec<_> = addrs
                    .iter()
                    .zip(&results)
                    .filter(|&(_, ok)| !*ok)
                    .map(|(a, _)| truncate_id(&a.address, 12))
                    .collect();
                if !unreachable.is_empty() {
                    self.flash(&format!("Not on XMTP: {}", unreachable.join(", ")));
                    return;
                }
            }
            Err(e) => {
                self.flash(&format!("canMessage failed: {e}"));
                return;
            }
        }

        match client.create_group_by_identifiers(&addrs, &CreateGroupOptions::default()) {
            Ok(conv) => {
                let id = conv.id().unwrap_or_default();
                let _ = conv.set_consent(ConsentState::Allowed);
                self.active_id = Some(id);
                self.active_conv = Some(conv);
                self.reload_messages();
                self.refresh_conversations(client);
                self.tab = Tab::Inbox;
                self.focus = Focus::Input;
                self.flash("Group created");
            }
            Err(e) => self.flash(&format!("Group failed: {e}")),
        }
    }

    // ── Actions: Accept / Reject message request ─────────────────

    fn accept_request(&mut self, client: &Client) {
        if self.tab != Tab::Requests {
            return;
        }
        let Some(entry) = self.requests.get(self.sidebar_idx) else {
            return;
        };
        let id = entry.id.clone();
        if let Ok(Some(conv)) = client.conversation(&id) {
            let _ = conv.set_consent(ConsentState::Allowed);
            self.refresh_conversations(client);
            self.flash("Accepted");
        }
    }

    fn reject_request(&mut self, client: &Client) {
        if self.tab != Tab::Requests {
            return;
        }
        let Some(entry) = self.requests.get(self.sidebar_idx) else {
            return;
        };
        let id = entry.id.clone();
        if let Ok(Some(conv)) = client.conversation(&id) {
            let _ = conv.set_consent(ConsentState::Denied);
            self.refresh_conversations(client);
            self.flash("Rejected");
        }
    }

    // ── Actions: Send message ────────────────────────────────────

    fn send_message(&mut self) {
        let text = self.input.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Some(ref conv) = self.active_conv {
            match conv.send_text(&text) {
                Ok(_) => {
                    self.input.clear();
                    self.cursor = 0;
                    self.reload_messages();
                }
                Err(e) => self.flash(&format!("Send failed: {e}")),
            }
        }
    }

    // ── Actions: Members ─────────────────────────────────────────

    fn show_members(&mut self) {
        let Some(ref conv) = self.active_conv else {
            return;
        };
        match conv.members() {
            Ok(members) => {
                self.members = members
                    .into_iter()
                    .map(|m| {
                        let addr = m
                            .account_identifiers
                            .first()
                            .cloned()
                            .unwrap_or_else(|| m.inbox_id.clone());
                        let role = match m.permission_level {
                            xmtp::PermissionLevel::SuperAdmin => "super_admin",
                            xmtp::PermissionLevel::Admin => "admin",
                            xmtp::PermissionLevel::Member => "member",
                        };
                        MemberEntry {
                            address: addr,
                            role,
                        }
                    })
                    .collect();
                self.mode = Mode::Members;
                self.status = HINT_MEMBERS.into();
            }
            Err(e) => self.flash(&format!("Members failed: {e}")),
        }
    }

    // ── XMTP stream events ──────────────────────────────────────

    pub fn handle_xmtp(&mut self, event: XmtpEvent, client: &Client) {
        match event {
            XmtpEvent::Message { conv_id, msg_id } => {
                let is_active = self.active_id.as_deref() == Some(&conv_id);
                if is_active {
                    self.reload_messages();
                }
                // Update preview in whichever list contains this conversation.
                if let Ok(Some(msg)) = client.message_by_id(&msg_id) {
                    for list in [&mut self.inbox, &mut self.requests] {
                        for entry in list.iter_mut() {
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
            }
            XmtpEvent::Conversation => {
                let _ = client.sync_welcomes();
                self.refresh_conversations(client);
            }
        }
    }

    // ── Status ───────────────────────────────────────────────────

    fn flash(&mut self, msg: &str) {
        self.status = format!(" {msg}");
        self.status_ttl = FLASH_TTL;
    }

    fn set_default_status(&mut self) {
        self.status = match self.mode {
            Mode::Help | Mode::Normal => match self.tab {
                Tab::Inbox => match self.focus {
                    Focus::Sidebar => HINT_SIDEBAR,
                    Focus::Input => HINT_INPUT,
                },
                Tab::Requests => HINT_REQUESTS,
            },
            Mode::NewDm => HINT_NEW_DM,
            Mode::NewGroup => HINT_NEW_GROUP,
            Mode::Members => HINT_MEMBERS,
        }
        .into();
        self.status_ttl = 0;
    }
}

// ── Free functions ───────────────────────────────────────────────

/// Load conversations filtered by consent state.
fn load_conversations(
    client: &Client,
    consent: &[ConsentState],
    my_inbox_id: &str,
) -> Vec<ConvEntry> {
    let opts = ListConversationsOptions {
        consent_states: consent.to_vec(),
        order_by: ConversationOrderBy::LastActivity,
        ..Default::default()
    };
    let convs = client.list_conversations(&opts).unwrap_or_default();
    convs
        .iter()
        .map(|conv| {
            let id = conv.id().unwrap_or_default();
            let is_group = conv.conversation_type() == Some(ConversationType::Group);

            // Display: use wallet address for DMs, group name for groups.
            let label = if is_group {
                conv.name()
                    .unwrap_or_else(|| format!("Group {}", truncate_id(&id, 8)))
            } else {
                peer_display(conv, my_inbox_id)
            };

            let last = conv.last_message().ok().flatten();
            let preview = last.as_ref().map_or(String::new(), decode_preview);
            let last_ns = last.as_ref().map_or(0, |m| m.sent_at_ns);

            ConvEntry {
                id,
                label,
                preview,
                last_ns,
                is_group,
                unread: false,
            }
        })
        .collect()
}

/// Resolve peer display name: prefer wallet address over inbox ID.
fn peer_display(conv: &Conversation, my_inbox_id: &str) -> String {
    // Try to get members to find peer's wallet address.
    if let Ok(members) = conv.members() {
        for m in &members {
            if m.inbox_id != my_inbox_id {
                if let Some(addr) = m.account_identifiers.first() {
                    return truncate_id(addr, 16);
                }
                return truncate_id(&m.inbox_id, 16);
            }
        }
    }
    // Fallback to inbox ID.
    conv.dm_peer_inbox_id()
        .map_or_else(|| "unknown".into(), |s| truncate_id(&s, 16))
}

/// Convert a char-index to a byte-index in a UTF-8 string.
fn char_byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(i, _)| i)
}

/// Decode a message to a short preview string for the sidebar.
fn decode_preview(msg: &Message) -> String {
    if msg.kind != MessageKind::Application {
        return String::new();
    }
    match msg.decode() {
        Ok(Content::Text(s) | Content::Markdown(s)) => truncate(&s, 28),
        Ok(Content::Reaction(r)) => truncate(&r.content, 28),
        Ok(Content::ReadReceipt) => String::new(),
        Ok(Content::Reply(r)) => truncate(&reply_text(&r.content), 28),
        Ok(Content::Attachment(a)) => {
            format!(
                "[file: {}]",
                truncate(a.filename.as_deref().unwrap_or("file"), 20)
            )
        }
        Ok(Content::RemoteAttachment(_)) => "[attachment]".into(),
        Ok(Content::Unknown { .. }) | Err(_) => msg.fallback.clone().unwrap_or_default(),
    }
}

/// Decode full message body for the chat view.
pub fn decode_body(msg: &Message) -> String {
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

/// Delivery status indicator.
pub const fn delivery_icon(status: DeliveryStatus) -> &'static str {
    match status {
        DeliveryStatus::Published => "✓",
        DeliveryStatus::Unpublished => "⏳",
        DeliveryStatus::Failed => "✗",
    }
}

/// Extract display text from a reply's inner `EncodedContent`.
fn reply_text(ec: &xmtp::content::EncodedContent) -> String {
    ec.r#type
        .as_ref()
        .filter(|t| t.type_id == "text" || t.type_id == "markdown")
        .map_or_else(
            || "[reply]".into(),
            |_| String::from_utf8(ec.content.clone()).unwrap_or_else(|_| "[reply]".into()),
        )
}

/// Truncate a string to `max` chars, appending `…` if needed.
fn truncate(s: &str, max: usize) -> String {
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
