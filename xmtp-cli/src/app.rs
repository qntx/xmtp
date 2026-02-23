//! Pure state machine — **zero FFI calls**, **zero blocking**.
//!
//! All network / FFI operations are delegated to the worker thread via
//! [`Cmd`] messages sent through [`CmdTx`]. Worker results arrive as
//! [`Event`] variants and are applied via [`App::apply`].

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use xmtp::content::Content;
use xmtp::{DeliveryStatus, Message, MessageKind};

use crate::event::{Cmd, CmdTx, ConvEntry, Event, MemberEntry};

/// Active sidebar tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Inbox,
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
    Normal,
    NewDm,
    NewGroup,
    Members,
    Help,
}

const HINT_SIDEBAR: &str =
    " Tab:input  j/k:nav  1/2:tab  Enter:open  n:DM  g:group  r:sync  ?:help  q:quit";
const HINT_INPUT: &str = " Enter:send  Esc:sidebar  PgUp/Dn:scroll  m:members";
const HINT_NEW_DM: &str = " Enter wallet address (0x…)  Enter:create  Esc:cancel";
const HINT_NEW_GROUP: &str = " [name:]addr1,addr2  (name optional)  Enter:create  Esc:cancel";
const HINT_REQUESTS: &str = " j/k:nav  a:accept  x:reject  Enter:preview  1/2:tab  q:quit";
const HINT_MEMBERS: &str = " Esc:close";
const FLASH_TTL: u16 = 60;

/// Central application state. Holds **no FFI handles**.
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

    pub inbox: Vec<ConvEntry>,
    pub requests: Vec<ConvEntry>,

    pub active_id: Option<String>,
    pub messages: Vec<Message>,
    pub members: Vec<MemberEntry>,

    pub input: String,
    pub cursor: usize,

    pub status: String,
    status_ttl: u16,

    /// Command channel to the worker thread (never blocks).
    cmd: CmdTx,
}

impl App {
    pub fn new(address: String, inbox_id: String, cmd: CmdTx) -> Self {
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
            messages: Vec::new(),
            members: Vec::new(),
            input: String::new(),
            cursor: 0,
            status: " Loading…".into(),
            status_ttl: 0,
            cmd,
        }
    }

    /// Current sidebar list.
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

    fn cmd(&self, c: Cmd) {
        let _ = self.cmd.send(c);
    }

    /// Apply a worker result event. Called from the main loop.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Conversations { inbox, requests } => {
                self.inbox = inbox;
                self.requests = requests;
                self.clamp_sidebar();
            }
            Event::Messages { conv_id, msgs } => {
                if self.active_id.as_deref() == Some(&conv_id) {
                    self.messages = msgs;
                    if self.scroll_pinned {
                        self.scroll = 0;
                    }
                }
            }
            Event::Preview {
                conv_id,
                text,
                time_ns,
                unread,
            } => {
                for list in [&mut self.inbox, &mut self.requests] {
                    for e in list.iter_mut() {
                        if e.id == conv_id {
                            e.preview.clone_from(&text);
                            e.last_ns = time_ns;
                            if unread {
                                e.unread = true;
                            }
                        }
                    }
                }
            }
            Event::Members(m) => {
                self.members = m;
            }
            Event::Created { conv_id } => {
                self.active_id = Some(conv_id);
                self.messages.clear();
                self.tab = Tab::Inbox;
                self.focus = Focus::Input;
                self.scroll = 0;
                self.scroll_pinned = true;
                self.set_default_status();
            }
            Event::Flash(msg) => self.flash(&msg),
            Event::Key(_) | Event::Resize | Event::Tick => {}
        }
    }

    pub fn tick(&mut self) {
        if self.status_ttl > 0 {
            self.status_ttl -= 1;
            if self.status_ttl == 0 {
                self.set_default_status();
            }
        }
    }

    /// Key dispatch — **never blocks**.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match self.mode {
            Mode::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('q' | '?') | KeyCode::Enter
                ) {
                    self.mode = Mode::Normal;
                    self.set_default_status();
                }
            }
            Mode::Members => {
                if key.code == KeyCode::Esc {
                    self.mode = Mode::Normal;
                    self.members.clear();
                    self.set_default_status();
                }
            }
            Mode::NewDm | Mode::NewGroup => self.key_overlay(key),
            Mode::Normal => match self.focus {
                Focus::Sidebar => self.key_sidebar(key),
                Focus::Input => self.key_input(key),
            },
        }
    }

    fn key_sidebar(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('1') => self.switch_tab(Tab::Inbox),
            KeyCode::Char('2') => self.switch_tab(Tab::Requests),
            KeyCode::Char('j') | KeyCode::Down => self.nav(1),
            KeyCode::Char('k') | KeyCode::Up => self.nav(-1),
            KeyCode::Char('h') | KeyCode::Home => {
                if !self.sidebar().is_empty() {
                    self.sidebar_idx = 0;
                    self.open_selected();
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                let len = self.sidebar().len();
                if len > 0 {
                    self.sidebar_idx = len - 1;
                    self.open_selected();
                }
            }
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
                if self.active_id.is_some() {
                    self.focus = Focus::Input;
                    self.set_default_status();
                } else if !self.sidebar().is_empty() {
                    // First launch: open the selected (first) conversation.
                    self.open_selected();
                }
            }
            KeyCode::Char('a') if self.tab == Tab::Requests => {
                if let Some(e) = self.requests.get(self.sidebar_idx) {
                    self.cmd(Cmd::Accept(e.id.clone()));
                }
            }
            KeyCode::Char('x') if self.tab == Tab::Requests => {
                if let Some(e) = self.requests.get(self.sidebar_idx) {
                    self.cmd(Cmd::Reject(e.id.clone()));
                }
            }
            KeyCode::Char('n') => {
                self.mode = Mode::NewDm;
                self.input.clear();
                self.cursor = 0;
                self.status = HINT_NEW_DM.into();
            }
            KeyCode::Char('g') => {
                self.mode = Mode::NewGroup;
                self.input.clear();
                self.cursor = 0;
                self.status = HINT_NEW_GROUP.into();
            }
            KeyCode::Char('r') => {
                self.cmd(Cmd::Sync);
                self.flash("Syncing…");
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
            KeyCode::Char('m') if self.input.is_empty() => {
                if self.active_id.is_some() {
                    self.mode = Mode::Members;
                    self.status = HINT_MEMBERS.into();
                    self.cmd(Cmd::LoadMembers);
                }
            }
            KeyCode::Enter => {
                let text = self.input.trim().to_owned();
                if !text.is_empty() && self.active_id.is_some() {
                    self.input.clear();
                    self.cursor = 0;
                    self.cmd(Cmd::Send(text));
                }
            }
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::PageDown => self.scroll_down(10),
            _ => self.edit_input(key.code),
        }
    }

    /// Merged overlay handler for `NewDm` / `NewGroup`.
    fn key_overlay(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel_overlay(),
            KeyCode::Enter => {
                let text = self.input.trim().to_owned();
                if !text.is_empty() {
                    let c = match self.mode {
                        Mode::NewDm => Cmd::CreateDm(text),
                        Mode::NewGroup => {
                            // Parse optional "name:addr1,addr2" format.
                            if let Some((name, addrs)) = text.split_once(':') {
                                let n = name.trim();
                                Cmd::CreateGroup {
                                    name: if n.is_empty() {
                                        None
                                    } else {
                                        Some(n.to_owned())
                                    },
                                    addrs: addrs.to_owned(),
                                }
                            } else {
                                Cmd::CreateGroup {
                                    name: None,
                                    addrs: text,
                                }
                            }
                        }
                        _ => unreachable!(),
                    };
                    self.cmd(c);
                }
                self.cancel_overlay();
            }
            _ => self.edit_input(key.code),
        }
    }

    /// Shared text editing (input bar + overlays). Eliminates duplication.
    fn edit_input(&mut self, code: KeyCode) {
        match code {
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let idx = byte_offset(&self.input, self.cursor);
                self.input.remove(idx);
            }
            KeyCode::Delete if self.cursor < self.input.chars().count() => {
                let idx = byte_offset(&self.input, self.cursor);
                self.input.remove(idx);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                if self.cursor < self.input.chars().count() {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.chars().count(),
            KeyCode::Char(c) => {
                let idx = byte_offset(&self.input, self.cursor);
                self.input.insert(idx, c);
                self.cursor += 1;
            }
            _ => {}
        }
    }

    fn cancel_overlay(&mut self) {
        self.mode = Mode::Normal;
        self.input.clear();
        self.cursor = 0;
        self.set_default_status();
    }

    fn nav(&mut self, delta: i32) {
        let len = self.sidebar().len();
        if len == 0 {
            return;
        }
        if delta > 0 {
            self.sidebar_idx = (self.sidebar_idx + 1) % len;
        } else {
            self.sidebar_idx = self.sidebar_idx.checked_sub(1).unwrap_or(len - 1);
        }
        self.open_selected();
    }

    fn open_selected(&mut self) {
        let Some(entry) = self.sidebar().get(self.sidebar_idx) else {
            return;
        };
        let id = entry.id.clone();
        if self.active_id.as_deref() == Some(&id) {
            return;
        }
        self.active_id = Some(id.clone());
        let idx = self.sidebar_idx;
        if let Some(e) = self.sidebar_mut().get_mut(idx) {
            e.unread = false;
        }
        self.messages.clear();
        self.scroll = 0;
        self.scroll_pinned = true;
        self.cmd(Cmd::Open(id));
    }

    fn switch_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.sidebar_idx = 0;
            self.active_id = None;
            self.messages.clear();
            self.set_default_status();
        }
    }

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

    fn clamp_sidebar(&mut self) {
        let len = self.sidebar().len();
        if len == 0 {
            self.sidebar_idx = 0;
        } else if self.sidebar_idx >= len {
            self.sidebar_idx = len - 1;
        }
    }

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

/// Char-index to byte-index in a UTF-8 string.
fn byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(i, _)| i)
}

/// Decode a message to a short preview string for the sidebar.
pub fn decode_preview(msg: &Message) -> String {
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
        DeliveryStatus::Unpublished => "○",
        DeliveryStatus::Failed => "✗",
    }
}

fn reply_text(ec: &xmtp::content::EncodedContent) -> String {
    ec.r#type
        .as_ref()
        .filter(|t| t.type_id == "text" || t.type_id == "markdown")
        .map_or_else(
            || "[reply]".into(),
            |_| String::from_utf8(ec.content.clone()).unwrap_or_else(|_| "[reply]".into()),
        )
}

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
