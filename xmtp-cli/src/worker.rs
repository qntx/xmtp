//! Worker thread — owns the [`Client`] and handles all blocking FFI calls.
//!
//! The main thread sends [`Cmd`] requests; the worker processes them and
//! sends [`Event`] results back. Stream callbacks also route through here.

use std::sync::mpsc;

use xmtp::{
    AccountIdentifier, Client, ConsentState, ConversationOrderBy, ConversationType,
    CreateGroupOptions, DeliveryStatus, IdentifierKind, ListConversationsOptions,
    ListMessagesOptions, Message, Recipient, SortDirection, stream,
};

use crate::app::{decode_preview, truncate_id};
use crate::event::{
    Cmd, CmdTx, ConvEntry, Event, GroupField, GroupInfo, MemberEntry, PermissionRow, Tx,
};

/// Run the worker loop. Owns the [`Client`], processes [`Cmd`], sends [`Event`].
///
/// All blocking FFI calls happen here — the main thread never waits.
#[allow(clippy::needless_pass_by_value)]
pub fn run(client: Client, rx: mpsc::Receiver<Cmd>, tx: Tx) {
    let mut w = Worker::new(client, tx);
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

    /// Load messages with unpublished sorted to the end.
    fn load_messages(&self, conv: &xmtp::Conversation) -> Vec<Message> {
        let mut msgs = conv.list_messages(&self.list_opts).unwrap_or_default();
        msgs.sort_by_key(|m| m.delivery_status == DeliveryStatus::Unpublished);
        msgs
    }

    /// Send loaded messages to the UI thread.
    fn send_msgs(&self, conv_id: &str, conv: &xmtp::Conversation) {
        let msgs = self.load_messages(conv);
        let _ = self.tx.send(Event::Messages {
            conv_id: conv_id.to_owned(),
            msgs,
        });
    }

    fn flash(&self, msg: &str) {
        let _ = self.tx.send(Event::Flash(msg.into()));
    }

    fn dispatch(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open(id) => self.open(&id),
            Cmd::Send(text) => self.send_text(&text),
            Cmd::CreateDm(input) => self.create_dm(&input),
            Cmd::CreateGroup { name, addrs } => self.create_group(name, addrs),
            Cmd::SetConsent { id, state } => self.set_consent(&id, state),
            Cmd::Sync => self.sync(),
            Cmd::LoadMembers => {
                if let Some((_, ref conv)) = self.active {
                    send_members(conv, &self.tx);
                }
            }
            Cmd::SetGroupMeta { field, value } => self.set_group_meta(field, &value),
            Cmd::LoadPermissions => {
                if let Some((_, ref conv)) = self.active {
                    send_permissions(conv, &self.tx);
                }
            }
            Cmd::SetPermission {
                update_type,
                policy,
                metadata_field,
            } => self.set_permission(update_type, policy, metadata_field),
            Cmd::AddMember(input) => self.add_member(&input),
            Cmd::RemoveMember(id) => self.remove_member(&id),
            Cmd::ToggleAdmin(id) => self.toggle_admin(&id),
            Cmd::NewMessage { msg_id, conv_id } => self.new_message(&msg_id, conv_id),
            Cmd::NewConversation => {
                let _ = self.client.sync_welcomes();
                send_conversations(&self.client, &self.tx);
            }
        }
    }

    fn open(&mut self, id: &str) {
        if self.active.as_ref().is_some_and(|(aid, _)| *aid == id) {
            return;
        }
        if let Ok(Some(conv)) = self.client.conversation(id) {
            let _ = conv.sync();
            self.send_msgs(id, &conv);
            self.active = Some((id.to_owned(), conv));
        }
    }

    fn send_text(&self, text: &str) {
        let Some((ref id, ref conv)) = self.active else {
            return;
        };
        let encoded = xmtp::content::encode_text(text);
        match conv.send_optimistic(&encoded) {
            Ok(_) => {
                // Show message with ○ instantly.
                self.send_msgs(id, conv);
                // Publish to network (blocking but UI already updated).
                if let Err(e) = conv.publish_messages() {
                    self.flash(&format!("Publish: {e}"));
                }
                // Update status to ✓.
                self.send_msgs(id, conv);
            }
            Err(e) => self.flash(&format!("Send: {e}")),
        }
    }

    fn create_dm(&mut self, input: &str) {
        let recipient = Recipient::parse(input);
        if !self.check_reachable(&[&recipient]) {
            return;
        }
        match self.client.dm(&recipient) {
            Ok(conv) => {
                let cid = conv.id().unwrap_or_default();
                let _ = conv.set_consent(ConsentState::Allowed);
                let _ = self.tx.send(Event::Created {
                    conv_id: cid.clone(),
                });
                self.send_msgs(&cid, &conv);
                self.active = Some((cid, conv));
                send_conversations(&self.client, &self.tx);
                self.flash("DM created");
            }
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
        // Auto-generate name from member strings if not provided.
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
            Ok(conv) => {
                let cid = conv.id().unwrap_or_default();
                let _ = conv.set_consent(ConsentState::Allowed);
                let _ = self.tx.send(Event::Created {
                    conv_id: cid.clone(),
                });
                self.send_msgs(&cid, &conv);
                self.active = Some((cid, conv));
                send_conversations(&self.client, &self.tx);
                self.flash("Group created");
            }
            Err(e) => self.flash(&format!("Group: {e}")),
        }
    }

    fn set_consent(&self, id: &str, state: ConsentState) {
        if let Ok(Some(conv)) = self.client.conversation(id) {
            let _ = conv.set_consent(state);
            send_conversations(&self.client, &self.tx);
            let msg = match state {
                ConsentState::Allowed => "Accepted",
                ConsentState::Denied => "Hidden",
                ConsentState::Unknown => "Reset",
            };
            self.flash(msg);
        }
    }

    fn sync(&self) {
        let _ = self.client.sync_welcomes();
        send_conversations(&self.client, &self.tx);
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
                let msg = match field {
                    GroupField::Name => "Renamed",
                    GroupField::Description => "Description updated",
                };
                self.flash(msg);
                send_conversations(&self.client, &self.tx);
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
                send_members(conv, &self.tx);
                send_conversations(&self.client, &self.tx);
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
                send_members(conv, &self.tx);
                send_conversations(&self.client, &self.tx);
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
                let msg = if conv.is_admin(inbox_id) {
                    "Promoted"
                } else {
                    "Demoted"
                };
                self.flash(msg);
                send_members(conv, &self.tx);
            }
            Err(e) => self.flash(&format!("Admin: {e}")),
        }
    }

    fn new_message(&self, msg_id: &str, conv_id: String) {
        let is_active = matches!(&self.active, Some((id, _)) if *id == conv_id);
        if let Some((_, ref conv)) = self.active
            && is_active
        {
            self.send_msgs(&conv_id, conv);
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
    /// Pre-check reachability for address recipients. Returns `false` if any are unreachable.
    fn check_reachable(&self, recipients: &[&Recipient]) -> bool {
        let idents: Vec<AccountIdentifier> = recipients
            .iter()
            .filter_map(|r| match r {
                Recipient::Address(a) => Some(AccountIdentifier {
                    address: a.clone(),
                    kind: IdentifierKind::Ethereum,
                }),
                _ => None,
            })
            .collect();
        if idents.is_empty() {
            return true;
        }
        match self.client.can_message(&idents) {
            Ok(results) => {
                let bad: Vec<_> = idents
                    .iter()
                    .zip(&results)
                    .filter(|&(_, ok)| !*ok)
                    .map(|(a, _)| truncate_id(&a.address, 12))
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

/// Wire up XMTP streams to the worker command channel.
pub fn start_streams(
    client: &Client,
    cmd_tx: &CmdTx,
) -> xmtp::Result<(xmtp::StreamHandle, xmtp::StreamHandle)> {
    let msg_tx = cmd_tx.clone();
    let msg_stream = stream::stream_all_messages(client, None, &[], move |msg_id, conv_id| {
        let _ = msg_tx.send(Cmd::NewMessage { msg_id, conv_id });
    })?;

    let conv_tx = cmd_tx.clone();
    let conv_stream = stream::stream_conversations(client, None, move |_| {
        let _ = conv_tx.send(Cmd::NewConversation);
    })?;

    Ok((msg_stream, conv_stream))
}

/// Load and send permission policies for the given conversation.
fn send_permissions(conv: &xmtp::Conversation, tx: &Tx) {
    use xmtp::{MetadataField, PermissionUpdateType};
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
            let _ = tx.send(Event::Permissions(rows));
        }
        Err(e) => {
            let _ = tx.send(Event::Flash(format!("Permissions: {e}")));
        }
    }
}

/// Load and send group members + group info for the given conversation.
fn send_members(conv: &xmtp::Conversation, tx: &Tx) {
    match conv.members() {
        Ok(members) => {
            let entries: Vec<MemberEntry> = members
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
            let _ = tx.send(Event::Members {
                members: entries,
                info,
            });
        }
        Err(e) => {
            let _ = tx.send(Event::Flash(format!("Members: {e}")));
        }
    }
}

/// Build and send conversation lists for Inbox, Requests, and Hidden.
fn send_conversations(client: &Client, tx: &Tx) {
    let inbox = build_conv_list(client, &[ConsentState::Allowed]);
    let requests = build_conv_list(client, &[ConsentState::Unknown]);
    let hidden = build_conv_list(client, &[ConsentState::Denied]);
    let _ = tx.send(Event::Conversations {
        inbox,
        requests,
        hidden,
    });
}

/// Build a sidebar list from conversations matching the given consent states.
fn build_conv_list(client: &Client, consent: &[ConsentState]) -> Vec<ConvEntry> {
    let opts = ListConversationsOptions {
        consent_states: consent.to_vec(),
        order_by: ConversationOrderBy::LastActivity,
        ..Default::default()
    };
    client
        .list_conversations(&opts)
        .unwrap_or_default()
        .iter()
        .map(|conv| {
            let id = conv.id().unwrap_or_default();
            let is_group = conv.conversation_type() == Some(ConversationType::Group);
            let label = if is_group {
                conv.name()
                    .unwrap_or_else(|| format!("Group {}", truncate_id(&id, 8)))
            } else {
                conv.dm_peer_inbox_id()
                    .map_or_else(|| "DM".into(), |s| truncate_id(&s, 16))
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
