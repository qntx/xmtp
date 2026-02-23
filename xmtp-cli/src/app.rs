//! Pure state machine — **zero FFI calls**, **zero blocking**.
//!
//! All network / FFI operations are delegated to the worker thread via
//! [`Cmd`] messages sent through [`CmdTx`]. Worker results arrive as
//! [`Event`] variants and are applied via [`App::apply`].

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use xmtp::content::Content;
use xmtp::{ConsentState, DeliveryStatus, Message, MessageKind};

use crate::event::{Cmd, CmdTx, ConvEntry, Event, GroupField, MemberEntry, PermissionRow};
use xmtp::PermissionPolicy;

/// Active sidebar tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Inbox,
    Requests,
    Hidden,
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
    NewGroupName,
    NewGroupMembers,
    Members,
    GroupEdit(GroupField),
    Permissions,
    Help,
}

const HINT_SIDEBAR: &str =
    " Tab:input  ↑↓:nav  ←→:tab  n:DM  g:group  d:hide  r:sync  ?:help  q:quit";
const HINT_INPUT: &str = " Enter:send  Esc:sidebar  ↑/↓:scroll  Tab:members";
const HINT_NEW_DM: &str = " Address (0x…) / ENS (name.eth) / Inbox ID  Enter:create  Esc:cancel";
const HINT_GROUP_NAME: &str = " Group name (optional)  Enter:next step  Esc:cancel";
const HINT_REQUESTS: &str = " ↑↓:nav  a:accept  x:reject  Enter:preview  ←→:tab  q:quit";
const HINT_HIDDEN: &str = " ↑↓:nav  ←→:tab  a:allow  u:request  r:sync  ?:help  q:quit";
const HINT_MEMBERS: &str = " ↑↓:nav  x:kick  p:admin  r:name  e:desc  s:perms  Esc:close";
const HINT_GROUP_EDIT: &str = " Enter:save  Esc:cancel";
const HINT_PERMISSIONS: &str = " ↑↓:nav  Enter:cycle  Esc:back";
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

    pub inbox: Vec<ConvEntry>,
    pub requests: Vec<ConvEntry>,
    pub hidden: Vec<ConvEntry>,

    pub active_id: Option<String>,
    pub messages: Vec<Message>,
    pub members: Vec<MemberEntry>,
    pub member_idx: usize,
    pub group_desc: String,
    pub permissions: Vec<PermissionRow>,
    pub perm_idx: usize,

    pub input: String,
    pub cursor: usize,

    /// Pending group creation state.
    pub group_name: Option<String>,
    pub group_members: Vec<String>,

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
            inbox: Vec::new(),
            requests: Vec::new(),
            hidden: Vec::new(),
            active_id: None,
            messages: Vec::new(),
            members: Vec::new(),
            member_idx: 0,
            group_desc: String::new(),
            permissions: Vec::new(),
            perm_idx: 0,
            input: String::new(),
            cursor: 0,
            group_name: None,
            group_members: Vec::new(),
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
            Tab::Hidden => &self.hidden,
        }
    }

    const fn sidebar_mut(&mut self) -> &mut Vec<ConvEntry> {
        match self.tab {
            Tab::Inbox => &mut self.inbox,
            Tab::Requests => &mut self.requests,
            Tab::Hidden => &mut self.hidden,
        }
    }

    fn cmd(&self, c: Cmd) {
        let _ = self.cmd.send(c);
    }

    /// Apply a worker result event. Called from the main loop.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Conversations {
                inbox,
                requests,
                hidden,
            } => {
                self.inbox = inbox;
                self.requests = requests;
                self.hidden = hidden;
                self.clamp_sidebar();
            }
            Event::Messages { conv_id, msgs } => {
                if self.active_id.as_deref() == Some(&conv_id) {
                    self.messages = msgs;
                }
            }
            Event::Preview {
                conv_id,
                text,
                time_ns,
                unread,
            } => {
                for list in [&mut self.inbox, &mut self.requests, &mut self.hidden] {
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
            Event::Members { members, info } => {
                self.members = members;
                self.group_desc = info.description;
                if self.member_idx >= self.members.len() && !self.members.is_empty() {
                    self.member_idx = self.members.len() - 1;
                }
            }
            Event::Permissions(p) => {
                self.permissions = p;
                if self.perm_idx >= self.permissions.len() && !self.permissions.is_empty() {
                    self.perm_idx = self.permissions.len() - 1;
                }
            }
            Event::Created { conv_id } => {
                self.active_id = Some(conv_id);
                self.messages.clear();
                self.tab = Tab::Inbox;
                self.focus = Focus::Input;
                self.scroll = 0;
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
            Mode::Members => self.key_members(key),
            Mode::GroupEdit(_) => self.key_group_edit(key),
            Mode::Permissions => self.key_permissions(key),
            Mode::NewDm => self.key_overlay(key),
            Mode::NewGroupName => self.key_group_name(key),
            Mode::NewGroupMembers => self.key_group_members(key),
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
            KeyCode::Char('3') => self.switch_tab(Tab::Hidden),
            KeyCode::Left => self.switch_tab(self.prev_tab()),
            KeyCode::Right => self.switch_tab(self.next_tab()),
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
            KeyCode::Enter | KeyCode::Tab | KeyCode::Char('l') => {
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
                    self.cmd(Cmd::SetConsent {
                        id: e.id.clone(),
                        state: ConsentState::Allowed,
                    });
                }
            }
            KeyCode::Char('x') if self.tab == Tab::Requests => {
                if let Some(e) = self.requests.get(self.sidebar_idx) {
                    self.cmd(Cmd::SetConsent {
                        id: e.id.clone(),
                        state: ConsentState::Denied,
                    });
                }
            }
            KeyCode::Char('d') if self.tab == Tab::Inbox => {
                if let Some(e) = self.sidebar().get(self.sidebar_idx) {
                    let id = e.id.clone();
                    self.cmd(Cmd::SetConsent {
                        id: id.clone(),
                        state: ConsentState::Denied,
                    });
                    if self.active_id.as_deref() == Some(&id) {
                        self.active_id = None;
                        self.messages.clear();
                        self.scroll = 0;
                    }
                }
            }
            KeyCode::Char('a') if self.tab == Tab::Hidden => {
                if let Some(e) = self.sidebar().get(self.sidebar_idx) {
                    self.cmd(Cmd::SetConsent {
                        id: e.id.clone(),
                        state: ConsentState::Allowed,
                    });
                }
            }
            KeyCode::Char('u') if self.tab == Tab::Hidden => {
                if let Some(e) = self.sidebar().get(self.sidebar_idx) {
                    self.cmd(Cmd::SetConsent {
                        id: e.id.clone(),
                        state: ConsentState::Unknown,
                    });
                }
            }
            KeyCode::Char('n') => {
                self.mode = Mode::NewDm;
                self.input.clear();
                self.cursor = 0;
                self.status = HINT_NEW_DM.into();
            }
            KeyCode::Char('g') => {
                self.mode = Mode::NewGroupName;
                self.group_name = None;
                self.group_members.clear();
                self.input.clear();
                self.cursor = 0;
                self.status = HINT_GROUP_NAME.into();
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
            KeyCode::Esc => {
                self.focus = Focus::Sidebar;
                self.set_default_status();
            }
            KeyCode::Tab => {
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
            KeyCode::Up => self.scroll_up(3),
            KeyCode::Down => self.scroll_down(3),
            _ => self.edit_input(key.code),
        }
    }

    /// Overlay handler for `NewDm`.
    fn key_overlay(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel_overlay(),
            KeyCode::Enter => {
                let text = self.input.trim().to_owned();
                if !text.is_empty() {
                    self.cmd(Cmd::CreateDm(text));
                }
                self.cancel_overlay();
            }
            _ => self.edit_input(key.code),
        }
    }

    /// Step 1: enter group name (optional).
    fn key_group_name(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel_overlay(),
            KeyCode::Enter => {
                let name = self.input.trim().to_owned();
                self.group_name = if name.is_empty() { None } else { Some(name) };
                self.input.clear();
                self.cursor = 0;
                self.mode = Mode::NewGroupMembers;
                self.update_group_members_hint();
            }
            _ => self.edit_input(key.code),
        }
    }

    /// Step 2: add members one by one; Esc finishes and creates the group.
    fn key_group_members(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if self.group_members.is_empty() {
                    self.cancel_overlay();
                    return;
                }
                let addrs = std::mem::take(&mut self.group_members);
                let name = self.group_name.take();
                self.cmd(Cmd::CreateGroup { name, addrs });
                self.cancel_overlay();
            }
            KeyCode::Enter => {
                let addr = self.input.trim().to_owned();
                if !addr.is_empty() && !self.group_members.contains(&addr) {
                    self.group_members.push(addr);
                }
                self.input.clear();
                self.cursor = 0;
                self.update_group_members_hint();
            }
            _ => self.edit_input(key.code),
        }
    }

    /// Update status bar to show accumulated members.
    fn update_group_members_hint(&mut self) {
        let n = self.group_members.len();
        self.status = format!(
            " Address / ENS / Inbox ID  Esc:create group  ({n} member{})",
            if n == 1 { "" } else { "s" }
        );
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

    fn key_members(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => {
                self.mode = Mode::Normal;
                self.members.clear();
                self.member_idx = 0;
                self.set_default_status();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.members.is_empty() {
                    self.member_idx = (self.member_idx + 1) % self.members.len();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let len = self.members.len();
                if len > 0 {
                    self.member_idx = self.member_idx.checked_sub(1).unwrap_or(len - 1);
                }
            }
            KeyCode::Char('x') => {
                if let Some(m) = self.members.get(self.member_idx)
                    && m.inbox_id != self.inbox_id
                {
                    self.cmd(Cmd::RemoveMember(m.inbox_id.clone()));
                }
            }
            KeyCode::Char('p') => {
                if let Some(m) = self.members.get(self.member_idx)
                    && m.inbox_id != self.inbox_id
                {
                    self.cmd(Cmd::ToggleAdmin(m.inbox_id.clone()));
                }
            }
            KeyCode::Char('r') => {
                self.input = self.active_label().unwrap_or_default().to_owned();
                self.cursor = self.input.chars().count();
                self.mode = Mode::GroupEdit(GroupField::Name);
                self.status = HINT_GROUP_EDIT.into();
            }
            KeyCode::Char('e') => {
                self.input.clone_from(&self.group_desc);
                self.cursor = self.input.chars().count();
                self.mode = Mode::GroupEdit(GroupField::Description);
                self.status = HINT_GROUP_EDIT.into();
            }
            KeyCode::Char('s') => {
                self.perm_idx = 0;
                self.mode = Mode::Permissions;
                self.status = HINT_PERMISSIONS.into();
                self.cmd(Cmd::LoadPermissions);
            }
            _ => {}
        }
    }

    fn key_permissions(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Members;
                self.permissions.clear();
                self.perm_idx = 0;
                self.status = HINT_MEMBERS.into();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.permissions.is_empty() {
                    self.perm_idx = (self.perm_idx + 1) % self.permissions.len();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let len = self.permissions.len();
                if len > 0 {
                    self.perm_idx = self.perm_idx.checked_sub(1).unwrap_or(len - 1);
                }
            }
            KeyCode::Enter => {
                if let Some(row) = self.permissions.get(self.perm_idx) {
                    let new_policy = next_policy(row.policy);
                    let cmd = Cmd::SetPermission {
                        update_type: row.update_type,
                        policy: new_policy,
                        metadata_field: row.metadata_field,
                    };
                    self.permissions[self.perm_idx].policy = new_policy;
                    self.cmd(cmd);
                }
            }
            _ => {}
        }
    }

    fn key_group_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.input.clear();
                self.cursor = 0;
                self.mode = Mode::Members;
                self.status = HINT_MEMBERS.into();
            }
            KeyCode::Enter => {
                let value = self.input.trim().to_owned();
                if !value.is_empty() {
                    let Mode::GroupEdit(field) = self.mode else {
                        return;
                    };
                    self.cmd(Cmd::SetGroupMeta { field, value });
                }
                self.input.clear();
                self.cursor = 0;
                self.mode = Mode::Members;
                self.status = HINT_MEMBERS.into();
            }
            _ => self.edit_input(key.code),
        }
    }

    /// Get the label of the currently active conversation.
    fn active_label(&self) -> Option<&str> {
        self.active_id.as_ref().and_then(|id| {
            self.inbox
                .iter()
                .chain(self.requests.iter())
                .chain(self.hidden.iter())
                .find(|e| e.id == *id)
                .map(|e| e.label.as_str())
        })
    }

    fn cancel_overlay(&mut self) {
        self.mode = Mode::Normal;
        self.input.clear();
        self.cursor = 0;
        self.group_name = None;
        self.group_members.clear();
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
        self.cmd(Cmd::Open(id));
    }

    const fn next_tab(&self) -> Tab {
        match self.tab {
            Tab::Inbox => Tab::Requests,
            Tab::Requests => Tab::Hidden,
            Tab::Hidden => Tab::Inbox,
        }
    }

    const fn prev_tab(&self) -> Tab {
        match self.tab {
            Tab::Inbox => Tab::Hidden,
            Tab::Hidden => Tab::Requests,
            Tab::Requests => Tab::Inbox,
        }
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
    }

    pub const fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
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
                Tab::Hidden => HINT_HIDDEN,
            },
            Mode::NewDm => HINT_NEW_DM,
            Mode::NewGroupName => HINT_GROUP_NAME,
            Mode::NewGroupMembers => return, // hint managed by update_group_members_hint
            Mode::Members => HINT_MEMBERS,
            Mode::GroupEdit(_) => HINT_GROUP_EDIT,
            Mode::Permissions => HINT_PERMISSIONS,
        }
        .into();
        self.status_ttl = 0;
    }
}

/// Cycle: Allow → `AdminOnly` → `SuperAdminOnly` → Deny → Allow.
const fn next_policy(p: PermissionPolicy) -> PermissionPolicy {
    match p {
        PermissionPolicy::Allow => PermissionPolicy::AdminOnly,
        PermissionPolicy::AdminOnly => PermissionPolicy::SuperAdminOnly,
        PermissionPolicy::SuperAdminOnly => PermissionPolicy::Deny,
        PermissionPolicy::Deny | PermissionPolicy::DoesNotExist | PermissionPolicy::Other => {
            PermissionPolicy::Allow
        }
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
