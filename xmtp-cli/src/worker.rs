//! Worker thread — owns the [`Client`] and handles all blocking FFI calls.
//!
//! The main thread sends [`Cmd`] requests; the worker processes them and
//! sends [`Event`] results back. Stream callbacks also route through here.

use std::sync::mpsc;

use xmtp::{
    Client, ConsentState, ConversationOrderBy, ConversationType, CreateGroupOptions,
    DeliveryStatus, ListConversationsOptions, ListMessagesOptions, Message, Recipient,
    SortDirection, stream,
};

use crate::app::{decode_preview, truncate_id};
use crate::event::{
    Cmd, CmdTx, ConvEntry, Event, GroupField, GroupInfo, MemberEntry, PermissionRow, Tx,
};

/// Run the worker loop. Owns the [`Client`], processes [`Cmd`], sends [`Event`].
///
/// Starts streams and performs the initial sync before entering the main loop,
/// so the main (UI) thread is never blocked by these network operations.
#[allow(clippy::needless_pass_by_value)]
pub fn run(client: Client, rx: mpsc::Receiver<Cmd>, tx: Tx, cmd_tx: CmdTx) {
    let mut w = Worker::new(client, tx);

    // Start streams in the worker thread — avoids blocking TUI startup.
    w.start_streams(&cmd_tx);

    // Initial sync so conversations appear without a manual refresh.
    w.sync();

    while let Ok(cmd) = rx.recv() {
        w.dispatch(cmd);
    }
}

/// Worker state — owns the [`Client`] and the active conversation handle.
struct Worker {
    client: Client,
    tx: Tx,
    active: Option<(String, xmtp::Conversation)>,
    list_opts: ListMessagesOptions,
}

impl Worker {
    fn new(client: Client, tx: Tx) -> Self {
        Self {
            client,
            tx,
            active: None,
            list_opts: ListMessagesOptions {
                direction: Some(SortDirection::Ascending),
                ..Default::default()
            },
        }
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
        }
    }

    fn open(&mut self, id: &str) {
        // Already active — re-send cached messages (the UI may have cleared
        // its state after a tab switch) but skip the network sync.
        if let Some((ref aid, ref conv)) = self.active
            && *aid == id
        {
            self.send_msgs(id, conv);
            return;
        }
        let Ok(Some(conv)) = self.client.conversation(id) else {
            return;
        };
        // Show locally cached messages instantly, then sync from network.
        self.send_msgs(id, &conv);
        let _ = conv.sync();
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

    fn send_text(&self, text: &str) {
        let Some((ref id, ref conv)) = self.active else {
            return;
        };
        match conv.send_text_optimistic(text) {
            Ok(_) => {
                self.send_msgs(id, conv);
                if let Err(e) = conv.publish_messages() {
                    self.flash(&format!("Publish: {e}"));
                }
                self.send_msgs(id, conv);
            }
            Err(e) => self.flash(&format!("Send: {e}")),
        }
    }

    fn set_consent(&self, id: &str, state: ConsentState) {
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

    fn sync(&self) {
        let _ = self.client.sync_welcomes();
        self.send_conversations();
        if let Some((ref id, ref conv)) = self.active {
            let _ = conv.sync();
            self.send_msgs(id, conv);
        }
        self.flash("Synced");
    }

    fn set_group_meta(&self, field: GroupField, value: &str) {
        let Some((_, ref conv)) = self.active else {
            return;
        };
        let result = match field {
            GroupField::Name => conv.set_name(value),
            GroupField::Description => conv.set_description(value),
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

    fn add_member(&self, input: &str) {
        let Some((_, ref conv)) = self.active else {
            return;
        };
        let recipient = Recipient::parse(input);
        if !self.check_reachable(&[&recipient]) {
            return;
        }
        match self.client.add_members(conv, &[recipient]) {
            Ok(()) => {
                self.flash("Member added");
                self.send_members();
                self.send_conversations();
            }
            Err(e) => self.flash(&format!("Add: {e}")),
        }
    }

    fn remove_member(&self, inbox_id: &str) {
        let Some((_, ref conv)) = self.active else {
            return;
        };
        match conv.remove_members_by_inbox_id(&[inbox_id]) {
            Ok(()) => {
                self.flash("Removed");
                self.send_members();
                self.send_conversations();
            }
            Err(e) => self.flash(&format!("Remove: {e}")),
        }
    }

    fn toggle_admin(&self, inbox_id: &str) {
        let Some((_, ref conv)) = self.active else {
            return;
        };
        let result = if conv.is_admin(inbox_id) {
            conv.remove_admin(inbox_id)
        } else {
            conv.add_admin(inbox_id)
        };
        match result {
            Ok(()) => {
                self.flash(if conv.is_admin(inbox_id) {
                    "Promoted"
                } else {
                    "Demoted"
                });
                self.send_members();
            }
            Err(e) => self.flash(&format!("Admin: {e}")),
        }
    }

    fn on_stream_msg(&self, msg_id: &str, conv_id: String) {
        let is_active = match &self.active {
            Some((id, conv)) if *id == conv_id => {
                self.send_msgs(&conv_id, conv);
                true
            }
            _ => false,
        };
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

    fn send_msgs(&self, conv_id: &str, conv: &xmtp::Conversation) {
        let address_map = Self::build_address_map(conv);
        let _ = self.tx.send(Event::Messages {
            conv_id: conv_id.to_owned(),
            msgs: self.load_messages(conv),
            address_map,
        });
    }

    /// Build an `inbox_id` → wallet address map from the conversation members.
    fn build_address_map(
        conv: &xmtp::Conversation,
    ) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        if let Ok(members) = conv.members() {
            for m in members {
                let display = m
                    .account_identifiers
                    .first()
                    .cloned()
                    .unwrap_or_else(|| m.inbox_id.clone());
                map.insert(m.inbox_id, display);
            }
        }
        map
    }

    fn send_conversations(&self) {
        let inbox = self.build_conv_list(&[ConsentState::Allowed]);
        let requests = self.build_conv_list(&[ConsentState::Unknown]);
        let hidden = self.build_conv_list(&[ConsentState::Denied]);
        let _ = self.tx.send(Event::Conversations {
            inbox,
            requests,
            hidden,
        });
    }

    fn send_members(&self) {
        let Some((_, ref conv)) = self.active else {
            return;
        };
        match conv.members() {
            Ok(members) => {
                let entries = members
                    .into_iter()
                    .map(|m| {
                        let address = m
                            .account_identifiers
                            .first()
                            .cloned()
                            .unwrap_or_else(|| m.inbox_id.clone());
                        MemberEntry {
                            inbox_id: m.inbox_id,
                            address,
                            permission: m.permission_level,
                        }
                    })
                    .collect();
                let info = GroupInfo {
                    description: conv.description().unwrap_or_default(),
                };
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

    fn build_conv_list(&self, consent: &[ConsentState]) -> Vec<ConvEntry> {
        let opts = ListConversationsOptions {
            consent_states: consent.to_vec(),
            order_by: ConversationOrderBy::LastActivity,
            ..Default::default()
        };
        self.client
            .list_conversations(&opts)
            .unwrap_or_default()
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
    /// Prefers the peer's wallet address (from members list) over the raw
    /// inbox ID, since addresses are more recognizable to users.
    fn dm_peer_label(&self, conv: &xmtp::Conversation) -> String {
        let my_inbox = self.client.inbox_id().unwrap_or_default();
        if let Ok(members) = conv.members()
            && let Some(peer) = members.iter().find(|m| m.inbox_id != my_inbox)
        {
            let display = peer
                .account_identifiers
                .first()
                .unwrap_or(&peer.inbox_id);
            return truncate_id(display, 16);
        }
        conv.dm_peer_inbox_id()
            .map_or_else(|| "DM".into(), |s| truncate_id(&s, 16))
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
