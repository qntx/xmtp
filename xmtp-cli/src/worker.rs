//! Worker thread — owns the [`Client`] and handles all blocking FFI calls.
//!
//! The main thread sends [`Cmd`] requests; the worker processes them and
//! sends [`Event`] results back. Stream callbacks also route through here.

use std::collections::HashMap;
use std::sync::mpsc;

use xmtp::{
    Client, ConsentState, ConversationOrderBy, ConversationType, CreateGroupOptions,
    DeliveryStatus, EnsResolver, Env, IdentifierKind, ListConversationsOptions,
    ListMessagesOptions, Message, Recipient, SortDirection, stream,
};

use crate::app::{decode_preview, truncate_id};
use crate::event::{
    Cmd, CmdTx, ConvEntry, Event, GroupField, GroupInfo, MemberEntry, PermissionRow, Tx,
};

/// Run the worker loop. Builds the [`Client`], performs initial sync, then
/// processes [`Cmd`] and sends [`Event`].
///
/// Client construction + sync happen here (on the worker thread) so the TUI
/// renders immediately without blocking on network setup.
#[allow(clippy::needless_pass_by_value)]
pub fn run(
    rx: mpsc::Receiver<Cmd>,
    tx: Tx,
    cmd_tx: CmdTx,
    env: Env,
    db_path: String,
    rpc_url: String,
    address: String,
) {
    let _ = tx.send(Event::Flash("Connecting...".into()));

    let client = match connect(env, &db_path, &rpc_url, &address) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Event::Flash(format!("Fatal: {e}")));
            return;
        }
    };

    let mut w = Worker::new(client, tx, &rpc_url, &cmd_tx, &address);
    w.start_streams(&cmd_tx);

    // Initial sync — catch up on messages received while offline.
    w.flash("Syncing...");
    let _ = w.client.sync_welcomes();
    let _ = w.client.sync_all(&[]);
    w.send_conversations();
    w.flash("Ready");

    while let Ok(cmd) = rx.recv() {
        w.dispatch(cmd);
    }
}

/// Build a connected XMTP client with stale-DB recovery.
fn connect(env: Env, db_path: &str, rpc_url: &str, address: &str) -> xmtp::Result<Client> {
    let build = |path: &str| {
        let mut b = Client::builder().env(env).db_path(path);
        if let Ok(r) = EnsResolver::new(rpc_url) {
            b = b.resolver(r);
        }
        b.build_existing(address, IdentifierKind::Ethereum)
    };
    match build(db_path) {
        Ok(c) => Ok(c),
        Err(e) if e.to_string().contains("does not match the stored InboxId") => {
            for ext in ["", "-shm", "-wal"] {
                let _ = std::fs::remove_file(format!("{db_path}{ext}"));
            }
            build(db_path)
        }
        Err(e) => Err(e),
    }
}

/// Worker state — owns the [`Client`] and the active conversation handle.
struct Worker {
    client: Client,
    tx: Tx,
    active: Option<(String, xmtp::Conversation)>,
    list_opts: ListMessagesOptions,
    /// Current user's wallet address (lowercase).
    my_address: String,
    /// address (lowercase) → `Some("name.eth")` | `None` (no reverse record / pending).
    ens_cache: HashMap<String, Option<String>>,
    /// Send addresses to the background ENS resolver thread.
    ens_tx: Option<mpsc::Sender<String>>,
}

impl Worker {
    fn new(client: Client, tx: Tx, rpc_url: &str, cmd_tx: &CmdTx, address: &str) -> Self {
        let ens_tx = Self::start_ens_resolver(rpc_url, cmd_tx);
        let my_address = address.to_lowercase();

        // Queue own address for background ENS reverse resolution.
        if let Some(ref tx) = ens_tx {
            let _ = tx.send(my_address.clone());
        }

        Self {
            client,
            tx,
            active: None,
            list_opts: ListMessagesOptions {
                direction: Some(SortDirection::Ascending),
                ..Default::default()
            },
            my_address,
            ens_cache: HashMap::new(),
            ens_tx,
        }
    }

    /// Spawn a background thread that resolves ENS names without blocking the worker.
    ///
    /// The thread stops automatically after 3 consecutive failures (e.g. RPC
    /// unreachable), avoiding minutes of futile retries.
    fn start_ens_resolver(rpc_url: &str, cmd_tx: &CmdTx) -> Option<mpsc::Sender<String>> {
        let resolver = EnsResolver::new(rpc_url).ok()?;
        let (tx, rx) = mpsc::channel::<String>();
        let cmd = cmd_tx.clone();
        std::thread::spawn(move || {
            use xmtp::Resolver;
            let mut failures: u8 = 0;
            while let Ok(addr) = rx.recv() {
                // Circuit breaker: stop resolving after 3 consecutive failures.
                let name = if failures >= 3 {
                    None
                } else if let Ok(n) = resolver.reverse_resolve(&addr) {
                    failures = 0;
                    n
                } else {
                    failures += 1;
                    None
                };
                if cmd
                    .send(Cmd::EnsResolved {
                        address: addr,
                        name,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Some(tx)
    }

    /// Wire up XMTP real-time streams via [`Subscription`] iterators.
    ///
    /// Each subscription is consumed in a dedicated thread that forwards events
    /// to `cmd_tx`. Threads exit naturally when the sender breaks (app exit).
    fn start_streams(&self, cmd_tx: &CmdTx) {
        match stream::messages(&self.client, None, &[]) {
            Ok(sub) => {
                let tx = cmd_tx.clone();
                std::thread::spawn(move || {
                    for ev in sub {
                        if tx
                            .send(Cmd::StreamMsg {
                                msg_id: ev.message_id,
                                conv_id: ev.conversation_id,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                });
            }
            Err(e) => self.flash(&format!("Message stream: {e}")),
        }
        match stream::conversations(&self.client, None) {
            Ok(sub) => {
                let tx = cmd_tx.clone();
                std::thread::spawn(move || {
                    for _ in sub {
                        if tx.send(Cmd::StreamConv).is_err() {
                            break;
                        }
                    }
                });
            }
            Err(e) => self.flash(&format!("Conversation stream: {e}")),
        }
    }

    fn dispatch(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open(id) => self.open(&id),
            Cmd::Send(text) => self.send_text(&text),
            Cmd::CreateDm(input) => self.create_dm(&input),
            Cmd::CreateGroup { name, addrs } => self.create_group(name, addrs),
            Cmd::SetConsent { id, state } => self.set_consent(&id, state),
            Cmd::Sync => self.sync(),
            Cmd::LoadMembers => self.send_members(),
            Cmd::LoadPermissions => self.send_permissions(),
            Cmd::SetGroupMeta { field, value } => self.set_group_meta(field, &value),
            Cmd::SetPermission {
                update_type,
                policy,
                metadata_field,
            } => self.set_permission(update_type, policy, metadata_field),
            Cmd::AddMember(input) => self.add_member(&input),
            Cmd::RemoveMember(id) => self.remove_member(&id),
            Cmd::ToggleAdmin(id) => self.toggle_admin(&id),
            Cmd::StreamMsg { msg_id, conv_id } => self.on_stream_msg(&msg_id, conv_id),
            Cmd::StreamConv => {
                let _ = self.client.sync_welcomes();
                self.send_conversations();
            }
            Cmd::EnsResolved { address, name } => self.on_ens_resolved(&address, name),
        }
    }

    /// Open a conversation — pure local DB read, zero network calls.
    ///
    /// Startup `sync_all` handles catch-up; streams deliver real-time
    /// updates; manual `r` does on-demand sync.  Keeping `open` non-blocking
    /// ensures instant navigation between conversations.
    fn open(&mut self, id: &str) {
        if self.active.as_ref().is_some_and(|(aid, _)| aid == id) {
            let (aid, conv) = self.active.take().expect("checked");
            self.send_msgs(id, &conv);
            self.active = Some((aid, conv));
            return;
        }
        let Ok(Some(conv)) = self.client.conversation(id) else {
            return;
        };
        self.send_msgs(id, &conv);
        self.active = Some((id.to_owned(), conv));
    }

    /// Shared post-creation setup for DM and group conversations.
    fn activate(&mut self, conv: xmtp::Conversation, label: &str) {
        let id = conv.id();
        let _ = conv.set_consent(ConsentState::Allowed);
        let _ = self.tx.send(Event::Created {
            conv_id: id.clone(),
        });
        self.send_msgs(&id, &conv);
        self.active = Some((id, conv));
        self.send_conversations();
        self.flash(label);
    }

    fn create_dm(&mut self, input: &str) {
        let recipient = Recipient::parse(input);
        if !self.check_reachable(&[&recipient]) {
            return;
        }
        match self.client.dm(&recipient) {
            Ok(conv) => self.activate(conv, "DM created"),
            Err(e) => self.flash(&format!("DM: {e}")),
        }
    }

    fn create_group(&mut self, name: Option<String>, addrs: Vec<String>) {
        let members: Vec<Recipient> = addrs
            .into_iter()
            .filter(|s| !s.is_empty())
            .map(|s| Recipient::parse(&s))
            .collect();
        if members.is_empty() {
            self.flash("No members");
            return;
        }
        if !self.check_reachable(&members.iter().collect::<Vec<_>>()) {
            return;
        }
        let group_name = name.or_else(|| {
            let names: Vec<_> = members
                .iter()
                .map(|r| truncate_id(&r.to_string(), 10))
                .collect();
            Some(names.join(", "))
        });
        let opts = CreateGroupOptions {
            name: group_name,
            ..Default::default()
        };
        match self.client.group(&members, &opts) {
            Ok(conv) => self.activate(conv, "Group created"),
            Err(e) => self.flash(&format!("Group: {e}")),
        }
    }

    fn send_text(&mut self, text: &str) {
        let Some((id, conv)) = self.active.take() else {
            return;
        };
        match conv.send_text_optimistic(text) {
            Ok(_) => {
                self.send_msgs(&id, &conv);
                if let Err(e) = conv.publish_messages() {
                    self.flash(&format!("Publish: {e}"));
                }
                self.send_msgs(&id, &conv);
            }
            Err(e) => self.flash(&format!("Send: {e}")),
        }
        self.active = Some((id, conv));
    }

    fn set_consent(&mut self, id: &str, state: ConsentState) {
        let Ok(Some(conv)) = self.client.conversation(id) else {
            return;
        };
        let _ = conv.set_consent(state);
        self.send_conversations();
        self.flash(match state {
            ConsentState::Allowed => "Accepted",
            ConsentState::Denied => "Hidden",
            ConsentState::Unknown => "Reset",
        });
    }

    fn sync(&mut self) {
        let _ = self.client.sync_all(&[]);
        self.send_conversations();
        if let Some((id, conv)) = self.active.take() {
            self.send_msgs(&id, &conv);
            self.active = Some((id, conv));
        }
        self.flash("Synced");
    }

    fn set_group_meta(&mut self, field: GroupField, value: &str) {
        let result = match &self.active {
            Some((_, conv)) => match field {
                GroupField::Name => conv.set_name(value),
                GroupField::Description => conv.set_description(value),
            },
            None => return,
        };
        match result {
            Ok(()) => {
                self.flash(match field {
                    GroupField::Name => "Renamed",
                    GroupField::Description => "Description updated",
                });
                self.send_conversations();
            }
            Err(e) => self.flash(&format!("Update: {e}")),
        }
    }

    fn set_permission(
        &self,
        update_type: xmtp::PermissionUpdateType,
        policy: xmtp::PermissionPolicy,
        metadata_field: Option<xmtp::MetadataField>,
    ) {
        let Some((_, ref conv)) = self.active else {
            return;
        };
        match conv.set_permission_policy(update_type, policy, metadata_field) {
            Ok(()) => self.flash("Policy updated"),
            Err(e) => self.flash(&format!("Permission: {e}")),
        }
    }

    fn add_member(&mut self, input: &str) {
        let recipient = Recipient::parse(input);
        if !self.check_reachable(&[&recipient]) {
            return;
        }
        let result = match &self.active {
            Some((_, conv)) => self.client.add_members(conv, &[recipient]),
            None => return,
        };
        match result {
            Ok(()) => {
                self.flash("Member added");
                self.send_members();
                self.send_conversations();
            }
            Err(e) => self.flash(&format!("Add: {e}")),
        }
    }

    fn remove_member(&mut self, inbox_id: &str) {
        let result = match &self.active {
            Some((_, conv)) => conv.remove_members_by_inbox_id(&[inbox_id]),
            None => return,
        };
        match result {
            Ok(()) => {
                self.flash("Removed");
                self.send_members();
                self.send_conversations();
            }
            Err(e) => self.flash(&format!("Remove: {e}")),
        }
    }

    fn toggle_admin(&mut self, inbox_id: &str) {
        let (result, was_admin) = match &self.active {
            Some((_, conv)) => {
                let is = conv.is_admin(inbox_id);
                let r = if is {
                    conv.remove_admin(inbox_id)
                } else {
                    conv.add_admin(inbox_id)
                };
                (r, is)
            }
            None => return,
        };
        match result {
            Ok(()) => {
                self.flash(if was_admin { "Demoted" } else { "Promoted" });
                self.send_members();
            }
            Err(e) => self.flash(&format!("Admin: {e}")),
        }
    }

    fn on_stream_msg(&mut self, msg_id: &str, conv_id: String) {
        let is_active = self.active.as_ref().is_some_and(|(id, _)| *id == conv_id);
        if is_active {
            let (id, conv) = self.active.take().expect("checked");
            self.send_msgs(&conv_id, &conv);
            self.active = Some((id, conv));
        }
        if let Ok(Some(msg)) = self.client.message_by_id(msg_id) {
            let _ = self.tx.send(Event::Preview {
                conv_id,
                text: decode_preview(&msg),
                time_ns: msg.sent_at_ns,
                unread: !is_active,
            });
        }
    }

    fn flash(&self, msg: &str) {
        let _ = self.tx.send(Event::Flash(msg.into()));
    }

    fn load_messages(&self, conv: &xmtp::Conversation) -> Vec<Message> {
        let mut msgs = conv.list_messages(&self.list_opts).unwrap_or_default();
        msgs.sort_by_key(|m| m.delivery_status == DeliveryStatus::Unpublished);
        msgs
    }

    fn send_msgs(&mut self, conv_id: &str, conv: &xmtp::Conversation) {
        let address_map = self.build_address_map(conv);
        let _ = self.tx.send(Event::Messages {
            conv_id: conv_id.to_owned(),
            msgs: self.load_messages(conv),
            address_map,
        });
    }

    /// Build an `inbox_id` → display name map from the conversation members.
    ///
    /// Resolution priority: ENS name > wallet address > inbox ID.
    fn build_address_map(&mut self, conv: &xmtp::Conversation) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Ok(members) = conv.members() {
            for m in members {
                let addr = m.account_identifiers.first().cloned();
                let display = self.display_name(addr.as_deref(), &m.inbox_id);
                map.insert(m.inbox_id, display);
            }
        }
        map
    }

    fn send_conversations(&mut self) {
        let inbox = self.build_conv_list(&[ConsentState::Allowed]);
        let requests = self.build_conv_list(&[ConsentState::Unknown]);
        let hidden = self.build_conv_list(&[ConsentState::Denied]);
        let _ = self.tx.send(Event::Conversations {
            inbox,
            requests,
            hidden,
        });
    }

    fn send_members(&mut self) {
        // Scope the borrow of self.active to extract owned data.
        let (members_result, desc) = match &self.active {
            Some((_, conv)) => (conv.members(), conv.description().unwrap_or_default()),
            None => return,
        };
        match members_result {
            Ok(members) => {
                let entries = members
                    .into_iter()
                    .map(|m| {
                        let label = self.display_name(
                            m.account_identifiers.first().map(String::as_str),
                            &m.inbox_id,
                        );
                        MemberEntry {
                            inbox_id: m.inbox_id,
                            label,
                            addresses: m.account_identifiers,
                            permission: m.permission_level,
                        }
                    })
                    .collect();
                let info = GroupInfo { description: desc };
                let _ = self.tx.send(Event::Members {
                    members: entries,
                    info,
                });
            }
            Err(e) => self.flash(&format!("Members: {e}")),
        }
    }

    fn send_permissions(&self) {
        use xmtp::{MetadataField, PermissionUpdateType};
        let Some((_, ref conv)) = self.active else {
            return;
        };
        match conv.permissions() {
            Ok(perms) => {
                let p = perms.policies;
                let rows = vec![
                    PermissionRow {
                        label: "Add Members",
                        policy: p.add_member,
                        update_type: PermissionUpdateType::AddMember,
                        metadata_field: None,
                    },
                    PermissionRow {
                        label: "Remove Members",
                        policy: p.remove_member,
                        update_type: PermissionUpdateType::RemoveMember,
                        metadata_field: None,
                    },
                    PermissionRow {
                        label: "Add Admins",
                        policy: p.add_admin,
                        update_type: PermissionUpdateType::AddAdmin,
                        metadata_field: None,
                    },
                    PermissionRow {
                        label: "Remove Admins",
                        policy: p.remove_admin,
                        update_type: PermissionUpdateType::RemoveAdmin,
                        metadata_field: None,
                    },
                    PermissionRow {
                        label: "Group Name",
                        policy: p.update_group_name,
                        update_type: PermissionUpdateType::UpdateMetadata,
                        metadata_field: Some(MetadataField::GroupName),
                    },
                    PermissionRow {
                        label: "Description",
                        policy: p.update_group_description,
                        update_type: PermissionUpdateType::UpdateMetadata,
                        metadata_field: Some(MetadataField::Description),
                    },
                ];
                let _ = self.tx.send(Event::Permissions(rows));
            }
            Err(e) => self.flash(&format!("Permissions: {e}")),
        }
    }

    fn build_conv_list(&mut self, consent: &[ConsentState]) -> Vec<ConvEntry> {
        let opts = ListConversationsOptions {
            consent_states: consent.to_vec(),
            order_by: ConversationOrderBy::LastActivity,
            ..Default::default()
        };
        let convs = self.client.list_conversations(&opts).unwrap_or_default();
        convs
            .iter()
            .map(|conv| {
                let id = conv.id();
                let is_group = conv.conversation_type() == Some(ConversationType::Group);
                let label = if is_group {
                    conv.name()
                        .unwrap_or_else(|| format!("Group {}", truncate_id(&id, 8)))
                } else {
                    self.dm_peer_label(conv)
                };
                let last = conv.last_message().ok().flatten();
                let preview = last.as_ref().map_or(String::new(), decode_preview);
                let last_ns = last.as_ref().map_or(0, |m| m.sent_at_ns);
                ConvEntry {
                    id,
                    label,
                    preview,
                    last_ns,
                    unread: false,
                }
            })
            .collect()
    }

    /// Resolve the best display label for a DM peer.
    ///
    /// Resolution priority: ENS name > wallet address > inbox ID.
    fn dm_peer_label(&mut self, conv: &xmtp::Conversation) -> String {
        let my_inbox = self.client.inbox_id().unwrap_or_default();
        if let Ok(members) = conv.members()
            && let Some(peer) = members.iter().find(|m| m.inbox_id != my_inbox)
        {
            return self.display_name(
                peer.account_identifiers.first().map(String::as_str),
                &peer.inbox_id,
            );
        }
        conv.dm_peer_inbox_id()
            .map_or_else(|| "DM".into(), |s| truncate_id(&s, 16))
    }

    /// Resolve a display name for a member identity (**non-blocking**).
    ///
    /// Priority: ENS name (cached) > wallet address > inbox ID.
    /// On cache miss, inserts `None` (pending) and queues the address for
    /// background resolution, returning the truncated address immediately.
    fn display_name(&mut self, address: Option<&str>, inbox_id: &str) -> String {
        let Some(raw) = address else {
            return truncate_id(inbox_id, 16);
        };
        let key = raw.to_lowercase();
        if let Some(cached) = self.ens_cache.get(&key) {
            return cached.clone().unwrap_or_else(|| truncate_id(raw, 16));
        }
        // Pre-cache as pending (dedup) and queue for background resolution.
        self.ens_cache.insert(key.clone(), None);
        if let Some(ref tx) = self.ens_tx {
            let _ = tx.send(key);
        }
        truncate_id(raw, 16)
    }

    /// Handle a resolved ENS name from the background thread.
    fn on_ens_resolved(&mut self, address: &str, name: Option<String>) {
        let key = address.to_lowercase();
        if self.ens_cache.get(&key) == Some(&name) {
            return;
        }

        // Update header if this is the current user's address.
        if key == self.my_address
            && let Some(ref n) = name
        {
            let _ = self.tx.send(Event::Identity(n.clone()));
        }

        self.ens_cache.insert(key, name);

        // Refresh sidebar and active conversation to show resolved names.
        self.send_conversations();
        if let Some((id, conv)) = self.active.take() {
            self.send_msgs(&id, &conv);
            self.active = Some((id, conv));
        }
    }

    /// Pre-check reachability for recipients.
    fn check_reachable(&self, recipients: &[&Recipient]) -> bool {
        match self.client.can_message_recipients(recipients) {
            Ok(results) => {
                let bad: Vec<_> = recipients
                    .iter()
                    .zip(&results)
                    .filter(|&(_, ok)| !*ok)
                    .map(|(r, _)| truncate_id(&r.to_string(), 12))
                    .collect();
                if bad.is_empty() {
                    true
                } else {
                    self.flash(&format!("Not on XMTP: {}", bad.join(", ")));
                    false
                }
            }
            Err(e) => {
                self.flash(&format!("canMessage: {e}"));
                false
            }
        }
    }
}
