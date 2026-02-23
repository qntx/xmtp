//! xmtp-cli — Interactive XMTP TUI chat client.
//!
//! Architecture: **main thread = UI only**, **worker thread = all FFI**.
//! Stream callbacks route through the worker via [`Cmd`], never blocking the UI.

#![allow(
    missing_docs,
    missing_debug_implementations,
    clippy::print_stderr,
    clippy::print_stdout
)]

mod app;
mod event;
mod tui;
mod ui;

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, process};

use xmtp::{
    AccountIdentifier, AlloySigner, Client, ConsentState, ConversationOrderBy, ConversationType,
    CreateGroupOptions, Env, IdentifierKind, ListConversationsOptions, ListMessagesOptions,
    SortDirection, stream,
};

use crate::app::{App, decode_preview, truncate_id};
use crate::event::{Cmd, CmdTx, ConvEntry, Event, MemberEntry, Tx};

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    if let Err(e) = run() {
        let _ = tui::restore();
        eprintln!("fatal: {e}");
        process::exit(1);
    }
}

fn run() -> xmtp::Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: xmtp-cli <name>");
        process::exit(1);
    });

    let signer = load_or_create_signer(&format!("{name}.key"))?;
    let address = signer.address();
    eprintln!("address: {address}");

    let client = create_client(&signer, &format!("{name}.db3"))?;
    let inbox_id = client.inbox_id()?;
    eprintln!("inbox: {inbox_id}");

    // Channels: events (worker/poller → main), commands (app/streams → worker).
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();

    // Terminal input poller.
    event::spawn_poller(event_tx.clone(), Duration::from_millis(50));

    // XMTP streams → worker commands (not main thread events).
    let _streams = start_streams(&client, &cmd_tx)?;

    // Worker thread — owns the Client, handles ALL FFI.
    let worker_tx = event_tx;
    let worker_inbox = inbox_id.clone();
    std::thread::spawn(move || worker(client, worker_inbox, cmd_rx, worker_tx));

    // Trigger initial async load (non-blocking).
    let _ = cmd_tx.send(Cmd::Sync);

    // App — pure state machine, zero FFI.
    let mut app = App::new(address, inbox_id, cmd_tx);

    tui::install_panic_hook();
    let mut terminal = tui::init().map_err(|e| xmtp::Error::Ffi(format!("terminal: {e}")))?;

    while !app.quit {
        terminal
            .draw(|f| ui::render(&mut app, f))
            .map_err(|e| xmtp::Error::Ffi(format!("render: {e}")))?;

        match event_rx.recv() {
            Ok(Event::Key(k)) => app.handle_key(k),
            Ok(Event::Tick) => app.tick(),
            Ok(Event::Resize) => {}
            Ok(ev) => app.apply(ev),
            Err(_) => break,
        }
    }

    tui::restore().map_err(|e| xmtp::Error::Ffi(format!("restore: {e}")))
}

/// Worker thread: owns the [`Client`], processes [`Cmd`], sends [`Event`] results.
/// All blocking FFI calls happen here — the main thread never waits.
#[allow(clippy::needless_pass_by_value)]
fn worker(client: Client, inbox_id: String, rx: mpsc::Receiver<Cmd>, tx: Tx) {
    let mut active: Option<(String, xmtp::Conversation)> = None;

    let list_opts = ListMessagesOptions {
        direction: Some(SortDirection::Ascending),
        ..Default::default()
    };

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Open(id) => {
                if active.as_ref().is_some_and(|(aid, _)| *aid == id) {
                    continue;
                }
                if let Ok(Some(conv)) = client.conversation(&id) {
                    let msgs = conv.list_messages(&list_opts).unwrap_or_default();
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
                        // Show message with ⏳ instantly.
                        let msgs = conv.list_messages(&list_opts).unwrap_or_default();
                        let _ = tx.send(Event::Messages {
                            conv_id: id.clone(),
                            msgs,
                        });
                        // Publish to network (blocking but UI already updated).
                        if let Err(e) = conv.publish_messages() {
                            let _ = tx.send(Event::Flash(format!("Publish: {e}")));
                        }
                        // Update status to ✓.
                        let msgs = conv.list_messages(&list_opts).unwrap_or_default();
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

            Cmd::CreateDm(addr) => {
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
                match client.create_dm(&addr, IdentifierKind::Ethereum) {
                    Ok(conv) => {
                        let cid = conv.id().unwrap_or_default();
                        let _ = conv.set_consent(ConsentState::Allowed);
                        let _ = tx.send(Event::Created {
                            conv_id: cid.clone(),
                        });
                        let msgs = conv.list_messages(&list_opts).unwrap_or_default();
                        let _ = tx.send(Event::Messages {
                            conv_id: cid.clone(),
                            msgs,
                        });
                        active = Some((cid, conv));
                        send_conversations(&client, &inbox_id, &tx);
                        let _ = tx.send(Event::Flash("DM created".into()));
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Flash(format!("DM: {e}")));
                    }
                }
            }

            Cmd::CreateGroup { name, addrs: raw } => {
                let addrs: Vec<AccountIdentifier> = raw
                    .split(',')
                    .map(|s| AccountIdentifier {
                        address: s.trim().to_owned(),
                        kind: IdentifierKind::Ethereum,
                    })
                    .filter(|a| !a.address.is_empty())
                    .collect();
                if addrs.is_empty() {
                    let _ = tx.send(Event::Flash("No addresses".into()));
                    continue;
                }
                match client.can_message(&addrs) {
                    Ok(results) => {
                        let bad: Vec<_> = addrs
                            .iter()
                            .zip(&results)
                            .filter(|&(_, ok)| !*ok)
                            .map(|(a, _)| truncate_id(&a.address, 12))
                            .collect();
                        if !bad.is_empty() {
                            let _ =
                                tx.send(Event::Flash(format!("Not on XMTP: {}", bad.join(", "))));
                            continue;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Event::Flash(format!("canMessage: {e}")));
                        continue;
                    }
                }
                // Auto-generate name from member addresses if not provided.
                let group_name = name.or_else(|| {
                    let names: Vec<_> = addrs.iter().map(|a| truncate_id(&a.address, 10)).collect();
                    Some(names.join(", "))
                });
                let opts = CreateGroupOptions {
                    name: group_name,
                    ..Default::default()
                };
                match client.create_group_by_identifiers(&addrs, &opts) {
                    Ok(conv) => {
                        let cid = conv.id().unwrap_or_default();
                        let _ = conv.set_consent(ConsentState::Allowed);
                        let _ = tx.send(Event::Created {
                            conv_id: cid.clone(),
                        });
                        let msgs = conv.list_messages(&list_opts).unwrap_or_default();
                        let _ = tx.send(Event::Messages {
                            conv_id: cid.clone(),
                            msgs,
                        });
                        active = Some((cid, conv));
                        send_conversations(&client, &inbox_id, &tx);
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
                    send_conversations(&client, &inbox_id, &tx);
                    let _ = tx.send(Event::Flash("Accepted".into()));
                }
            }

            Cmd::Reject(id) => {
                if let Ok(Some(conv)) = client.conversation(&id) {
                    let _ = conv.set_consent(ConsentState::Denied);
                    send_conversations(&client, &inbox_id, &tx);
                    let _ = tx.send(Event::Flash("Rejected".into()));
                }
            }

            Cmd::Sync => {
                let _ = client.sync_welcomes();
                send_conversations(&client, &inbox_id, &tx);
                if let Some((ref id, ref conv)) = active {
                    let _ = conv.sync();
                    let msgs = conv.list_messages(&list_opts).unwrap_or_default();
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
                let is_active = active.as_ref().is_some_and(|(aid, _)| *aid == conv_id);
                if is_active && let Some((ref id, ref conv)) = active {
                    let msgs = conv.list_messages(&list_opts).unwrap_or_default();
                    let _ = tx.send(Event::Messages {
                        conv_id: id.clone(),
                        msgs,
                    });
                }
                if let Ok(Some(msg)) = client.message_by_id(&msg_id) {
                    let preview = decode_preview(&msg);
                    let _ = tx.send(Event::Preview {
                        conv_id,
                        text: preview,
                        time_ns: msg.sent_at_ns,
                        unread: !is_active,
                    });
                }
            }

            Cmd::NewConversation => {
                let _ = client.sync_welcomes();
                send_conversations(&client, &inbox_id, &tx);
            }
        }
    }
}

/// Build and send conversation lists for both Inbox and Requests.
fn send_conversations(client: &Client, inbox_id: &str, tx: &Tx) {
    let inbox = build_conv_list(client, &[ConsentState::Allowed], inbox_id);
    let requests = build_conv_list(client, &[ConsentState::Unknown], inbox_id);
    let _ = tx.send(Event::Conversations { inbox, requests });
}

/// Build a sidebar list from conversations matching the given consent states.
fn build_conv_list(client: &Client, consent: &[ConsentState], _inbox_id: &str) -> Vec<ConvEntry> {
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
                is_group,
                unread: false,
            }
        })
        .collect()
}

/// XMTP streams wired to the **worker** command channel (not the main event channel).
fn start_streams(
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

fn load_or_create_signer(key_path: &str) -> xmtp::Result<AlloySigner> {
    let key: [u8; 32] = if Path::new(key_path).exists() {
        let bytes = fs::read(key_path).map_err(|e| xmtp::Error::Ffi(format!("read key: {e}")))?;
        bytes
            .try_into()
            .map_err(|_| xmtp::Error::InvalidArgument("key file must be 32 bytes".into()))?
    } else {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| xmtp::Error::Ffi(format!("rng: {e}")))?;
        fs::write(key_path, key).map_err(|e| xmtp::Error::Ffi(format!("write key: {e}")))?;
        key
    };
    AlloySigner::from_bytes(&key)
}

fn create_client(signer: &AlloySigner, db_path: &str) -> xmtp::Result<Client> {
    match Client::builder()
        .env(Env::Dev)
        .db_path(db_path)
        .build(signer)
    {
        Ok(c) => Ok(c),
        Err(e) if format!("{e}").contains("does not match the stored InboxId") => {
            for ext in ["", "-shm", "-wal"] {
                let _ = fs::remove_file(format!("{db_path}{ext}"));
            }
            Client::builder()
                .env(Env::Dev)
                .db_path(db_path)
                .build(signer)
        }
        Err(e) => Err(e),
    }
}
