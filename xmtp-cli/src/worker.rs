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
use crate::event::{Cmd, CmdTx, ConvEntry, Event, MemberEntry, Tx};

/// Run the worker loop. Owns the [`Client`], processes [`Cmd`], sends [`Event`].
///
/// All blocking FFI calls happen here — the main thread never waits.
#[allow(clippy::needless_pass_by_value)]
pub fn run(client: Client, rx: mpsc::Receiver<Cmd>, tx: Tx) {
    let mut active: Option<(String, xmtp::Conversation)> = None;

    let list_opts = ListMessagesOptions {
        direction: Some(SortDirection::Ascending),
        ..Default::default()
    };

    // Stable sort: unpublished messages always appear last.
    let load_messages = |conv: &xmtp::Conversation| -> Vec<Message> {
        let mut msgs = conv.list_messages(&list_opts).unwrap_or_default();
        msgs.sort_by_key(|m| m.delivery_status == DeliveryStatus::Unpublished);
        msgs
    };

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Open(id) => {
                if active.as_ref().is_some_and(|(aid, _)| *aid == id) {
                    continue;
                }
                if let Ok(Some(conv)) = client.conversation(&id) {
                    let _ = conv.sync();
                    let msgs = load_messages(&conv);
                    let _ = tx.send(Event::Messages {
                        conv_id: id.clone(),
                        msgs,
                    });
                    active = Some((id, conv));
                }
            }

            Cmd::Send(text) => {
                let Some((ref id, ref conv)) = active else {
                    continue;
                };
                let encoded = xmtp::content::encode_text(&text);
                match conv.send_optimistic(&encoded) {
                    Ok(_) => {
                        // Show message with ○ instantly.
                        let msgs = load_messages(conv);
                        let _ = tx.send(Event::Messages {
                            conv_id: id.clone(),
                            msgs,
                        });
                        // Publish to network (blocking but UI already updated).
                        if let Err(e) = conv.publish_messages() {
                            let _ = tx.send(Event::Flash(format!("Publish: {e}")));
                        }
                        // Update status to ✓.
                        let msgs = load_messages(conv);
                        let _ = tx.send(Event::Messages {
                            conv_id: id.clone(),
                            msgs,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Flash(format!("Send: {e}")));
                    }
                }
            }

            Cmd::CreateDm(input) => {
                let recipient = Recipient::parse(&input);
                // Pre-check reachability for addresses.
                if let Recipient::Address(ref addr) = recipient {
                    let ident = AccountIdentifier {
                        address: addr.clone(),
                        kind: IdentifierKind::Ethereum,
                    };
                    match client.can_message(&[ident]) {
                        Ok(r) if r.first() == Some(&true) => {}
                        Ok(_) => {
                            let _ = tx.send(Event::Flash("Not on XMTP".into()));
                            continue;
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Flash(format!("canMessage: {e}")));
                            continue;
                        }
                    }
                }
                match client.dm(&recipient) {
                    Ok(conv) => {
                        let cid = conv.id().unwrap_or_default();
                        let _ = conv.set_consent(ConsentState::Allowed);
                        let _ = tx.send(Event::Created {
                            conv_id: cid.clone(),
                        });
                        let msgs = load_messages(&conv);
                        let _ = tx.send(Event::Messages {
                            conv_id: cid.clone(),
                            msgs,
                        });
                        active = Some((cid, conv));
                        send_conversations(&client, &tx);
                        let _ = tx.send(Event::Flash("DM created".into()));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Flash(format!("DM: {e}")));
                    }
                }
            }

            Cmd::CreateGroup { name, addrs: raw } => {
                let members: Vec<Recipient> = raw
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| Recipient::parse(&s))
                    .collect();
                if members.is_empty() {
                    let _ = tx.send(Event::Flash("No members".into()));
                    continue;
                }
                // Pre-check reachability for address recipients.
                let addr_idents: Vec<AccountIdentifier> = members
                    .iter()
                    .filter_map(|r| match r {
                        Recipient::Address(a) => Some(AccountIdentifier {
                            address: a.clone(),
                            kind: IdentifierKind::Ethereum,
                        }),
                        _ => None,
                    })
                    .collect();
                if !addr_idents.is_empty() {
                    match client.can_message(&addr_idents) {
                        Ok(results) => {
                            let bad: Vec<_> = addr_idents
                                .iter()
                                .zip(&results)
                                .filter(|&(_, ok)| !*ok)
                                .map(|(a, _)| truncate_id(&a.address, 12))
                                .collect();
                            if !bad.is_empty() {
                                let _ = tx
                                    .send(Event::Flash(format!("Not on XMTP: {}", bad.join(", "))));
                                continue;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Flash(format!("canMessage: {e}")));
                            continue;
                        }
                    }
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
                match client.group(&members, &opts) {
                    Ok(conv) => {
                        let cid = conv.id().unwrap_or_default();
                        let _ = conv.set_consent(ConsentState::Allowed);
                        let _ = tx.send(Event::Created {
                            conv_id: cid.clone(),
                        });
                        let msgs = load_messages(&conv);
                        let _ = tx.send(Event::Messages {
                            conv_id: cid.clone(),
                            msgs,
                        });
                        active = Some((cid, conv));
                        send_conversations(&client, &tx);
                        let _ = tx.send(Event::Flash("Group created".into()));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Flash(format!("Group: {e}")));
                    }
                }
            }

            Cmd::Accept(id) => {
                if let Ok(Some(conv)) = client.conversation(&id) {
                    let _ = conv.set_consent(ConsentState::Allowed);
                    send_conversations(&client, &tx);
                    let _ = tx.send(Event::Flash("Accepted".into()));
                }
            }

            Cmd::Reject(id) => {
                if let Ok(Some(conv)) = client.conversation(&id) {
                    let _ = conv.set_consent(ConsentState::Denied);
                    send_conversations(&client, &tx);
                    let _ = tx.send(Event::Flash("Rejected".into()));
                }
            }

            Cmd::Sync => {
                let _ = client.sync_welcomes();
                send_conversations(&client, &tx);
                if let Some((ref id, ref conv)) = active {
                    let _ = conv.sync();
                    let msgs = load_messages(conv);
                    let _ = tx.send(Event::Messages {
                        conv_id: id.clone(),
                        msgs,
                    });
                }
                let _ = tx.send(Event::Flash("Synced".into()));
            }

            Cmd::LoadMembers => {
                if let Some((_, ref conv)) = active {
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
                                    let role = match m.permission_level {
                                        xmtp::PermissionLevel::SuperAdmin => "super_admin",
                                        xmtp::PermissionLevel::Admin => "admin",
                                        xmtp::PermissionLevel::Member => "member",
                                    };
                                    MemberEntry { address, role }
                                })
                                .collect();
                            let _ = tx.send(Event::Members(entries));
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Flash(format!("Members: {e}")));
                        }
                    }
                }
            }

            Cmd::NewMessage { msg_id, conv_id } => {
                let is_active = matches!(&active, Some((id, _)) if *id == conv_id);
                if let Some((_, ref conv)) = active
                    && is_active
                {
                    let msgs = load_messages(conv);
                    let _ = tx.send(Event::Messages {
                        conv_id: conv_id.clone(),
                        msgs,
                    });
                }
                if let Ok(Some(msg)) = client.message_by_id(&msg_id) {
                    let _ = tx.send(Event::Preview {
                        conv_id,
                        text: decode_preview(&msg),
                        time_ns: msg.sent_at_ns,
                        unread: !is_active,
                    });
                }
            }

            Cmd::NewConversation => {
                let _ = client.sync_welcomes();
                send_conversations(&client, &tx);
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

/// Build and send conversation lists for both Inbox and Requests.
fn send_conversations(client: &Client, tx: &Tx) {
    let inbox = build_conv_list(client, &[ConsentState::Allowed]);
    let requests = build_conv_list(client, &[ConsentState::Unknown]);
    let _ = tx.send(Event::Conversations { inbox, requests });
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
