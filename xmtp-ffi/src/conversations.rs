//! Conversation creation, listing, and sync operations.

use std::ffi::c_char;

use xmtp_db::group::GroupQueryArgs;
use xmtp_mls::groups::MlsGroup;

use crate::ffi::*;

// ---------------------------------------------------------------------------
// Group creation options
// ---------------------------------------------------------------------------

/// Options for creating a new group conversation.
#[repr(C)]
pub struct XmtpCreateGroupOptions {
    /// Permission preset: 0 = AllMembers (default), 1 = AdminOnly.
    pub permissions: i32,
    /// Group name (nullable).
    pub name: *const c_char,
    /// Group description (nullable).
    pub description: *const c_char,
    /// Group image URL (nullable).
    pub image_url: *const c_char,
    /// Custom app data string (nullable).
    pub app_data: *const c_char,
    /// Message disappearing "from" timestamp in ns. 0 = not set.
    pub message_disappear_from_ns: i64,
    /// Message disappearing "in" duration in ns. 0 = not set.
    pub message_disappear_in_ns: i64,
}

/// Options for listing conversations.
#[repr(C)]
pub struct XmtpListConversationsOptions {
    /// Conversation type filter: -1 = all, 0 = DM, 1 = Group.
    pub conversation_type: i32,
    /// Maximum number of conversations to return. 0 = no limit.
    pub limit: i64,
    /// Only include conversations created after this timestamp (ns). 0 = no filter.
    pub created_after_ns: i64,
    /// Only include conversations created before this timestamp (ns). 0 = no filter.
    pub created_before_ns: i64,
    /// Only include conversations with last activity after this timestamp (ns). 0 = no filter.
    pub last_activity_after_ns: i64,
    /// Only include conversations with last activity before this timestamp (ns). 0 = no filter.
    pub last_activity_before_ns: i64,
    /// Consent state filter (parallel array with `consent_states_count`).
    /// Values: 0 = Unknown, 1 = Allowed, 2 = Denied.
    pub consent_states: *const i32,
    /// Number of consent states in the filter. 0 = no filter.
    pub consent_states_count: i32,
    /// Order by: 0 = CreatedAt (default), 1 = LastActivity.
    pub order_by: i32,
    /// Whether to include duplicate DMs. 0 = no (default), 1 = yes.
    pub include_duplicate_dms: i32,
}

// ---------------------------------------------------------------------------
// Group / DM creation
// ---------------------------------------------------------------------------

/// Create a new group conversation, optionally adding members by inbox ID.
/// Pass null/0 for `member_inbox_ids`/`member_count` to create an empty group.
/// Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_group(
    client: *const XmtpClient,
    opts: *const XmtpCreateGroupOptions,
    member_inbox_ids: *const *const c_char,
    member_count: i32,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let (policy_set, metadata) = unsafe { parse_group_opts(opts)? };
        let group = c.inner.create_group(policy_set, metadata)?;

        if !member_inbox_ids.is_null() && member_count > 0 {
            let ids = unsafe { collect_strings(member_inbox_ids, member_count)? };
            group.add_members(&ids).await?;
        } else {
            group.sync().await?;
        }

        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

/// Create a new group, adding members by identity (address/passkey).
/// `identifiers` and `kinds` are parallel arrays of length `count`.
/// Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_group_by_identity(
    client: *const XmtpClient,
    opts: *const XmtpCreateGroupOptions,
    identifiers: *const *const c_char,
    kinds: *const i32,
    count: i32,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let (policy_set, metadata) = unsafe { parse_group_opts(opts)? };
        let group = c.inner.create_group(policy_set, metadata)?;

        if !identifiers.is_null() && count > 0 {
            let idents = unsafe { collect_identifiers(identifiers, kinds, count)? };
            group.add_members_by_identity(&idents).await?;
        } else {
            group.sync().await?;
        }

        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

/// Build `DMMetadataOptions` from disappearing-message timestamps.
fn build_dm_opts(
    disappear_from_ns: i64,
    disappear_in_ns: i64,
) -> Option<xmtp_mls_common::group::DMMetadataOptions> {
    use xmtp_mls_common::group_mutable_metadata::MessageDisappearingSettings;
    if disappear_from_ns > 0 && disappear_in_ns > 0 {
        Some(xmtp_mls_common::group::DMMetadataOptions {
            message_disappearing_settings: Some(MessageDisappearingSettings {
                from_ns: disappear_from_ns,
                in_ns: disappear_in_ns,
            }),
        })
    } else {
        None
    }
}

/// Find or create a DM by identifier. Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_dm(
    client: *const XmtpClient,
    identifier: *const c_char,
    identifier_kind: i32,
    disappear_from_ns: i64,
    disappear_in_ns: i64,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ident_str = unsafe { c_str_to_string(identifier)? };
        let ident = match identifier_kind {
            0 => xmtp_id::associations::Identifier::eth(ident_str)?,
            1 => xmtp_id::associations::Identifier::passkey_str(&ident_str, None)?,
            _ => return Err("invalid identifier kind".into()),
        };
        let dm_opts = build_dm_opts(disappear_from_ns, disappear_in_ns);
        let group = c
            .inner
            .find_or_create_dm_by_identity(ident, dm_opts)
            .await?;
        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

/// Find or create a DM by inbox ID. Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_find_dm_by_inbox_id(
    client: *const XmtpClient,
    inbox_id: *const c_char,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let inbox_id = unsafe { c_str_to_string(inbox_id)? };
        let group = c.inner.dm_group_from_target_inbox(inbox_id)?;
        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

/// Create a DM by target inbox ID. Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_dm_by_inbox_id(
    client: *const XmtpClient,
    inbox_id: *const c_char,
    disappear_from_ns: i64,
    disappear_in_ns: i64,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let inbox_id = unsafe { c_str_to_string(inbox_id)? };
        let dm_opts = build_dm_opts(disappear_from_ns, disappear_in_ns);
        let group = c.inner.find_or_create_dm(inbox_id, dm_opts).await?;
        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// Get a conversation by hex-encoded group ID.
/// Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_get_conversation_by_id(
    client: *const XmtpClient,
    hex_id: *const c_char,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let id_str = unsafe { c_str_to_string(hex_id)? };
        let group_id = hex::decode(&id_str)?;
        let group = c.inner.stitched_group(&group_id)?;
        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// List conversations. Caller must free result with [`xmtp_conversation_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_list_conversations(
    client: *const XmtpClient,
    opts: *const XmtpListConversationsOptions,
    out: *mut *mut XmtpConversationList,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let args = if opts.is_null() {
            GroupQueryArgs::default()
        } else {
            let o = unsafe { &*opts };
            let consent = parse_consent_filter(o.consent_states, o.consent_states_count);
            GroupQueryArgs {
                conversation_type: match o.conversation_type {
                    0 => Some(xmtp_db::group::ConversationType::Dm),
                    1 => Some(xmtp_db::group::ConversationType::Group),
                    _ => None,
                },
                limit: if o.limit > 0 { Some(o.limit) } else { None },
                created_after_ns: if o.created_after_ns > 0 {
                    Some(o.created_after_ns)
                } else {
                    None
                },
                created_before_ns: if o.created_before_ns > 0 {
                    Some(o.created_before_ns)
                } else {
                    None
                },
                last_activity_after_ns: if o.last_activity_after_ns > 0 {
                    Some(o.last_activity_after_ns)
                } else {
                    None
                },
                last_activity_before_ns: if o.last_activity_before_ns > 0 {
                    Some(o.last_activity_before_ns)
                } else {
                    None
                },
                consent_states: consent,
                include_duplicate_dms: o.include_duplicate_dms != 0,
                order_by: match o.order_by {
                    1 => Some(xmtp_db::group::GroupQueryOrderBy::LastActivity),
                    _ => Some(xmtp_db::group::GroupQueryOrderBy::CreatedAt),
                },
                ..Default::default()
            }
        };

        let items: Vec<InnerGroup> = c
            .inner
            .list_conversations(args)?
            .into_iter()
            .map(|item| item.group)
            .collect();

        unsafe { write_out(out, XmtpConversationList { items })? };
        Ok(())
    })
}

ffi_list_len!(xmtp_conversation_list_len, XmtpConversationList);

/// Get a conversation from a list by index. Caller must free with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_get(
    list: *const XmtpConversationList,
    index: i32,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch(|| {
        let l = unsafe { ref_from(list)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let idx = index as usize;
        if idx >= l.items.len() {
            return Err("index out of bounds".into());
        }
        let src = &l.items[idx];
        let group = MlsGroup::new(
            src.context.clone(),
            src.group_id.clone(),
            src.dm_id.clone(),
            src.conversation_type,
            src.created_at_ns,
        );
        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

free_opaque!(xmtp_conversation_list_free, XmtpConversationList);

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// Sync welcomes (process new group invitations).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_sync_welcomes(client: *const XmtpClient) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        c.inner.sync_welcomes().await?;
        Ok(())
    })
}

/// Sync all conversations, optionally filtering by consent states.
/// `consent_states` is a parallel array of consent state values (0=Unknown, 1=Allowed, 2=Denied).
/// Pass null and 0 to sync all.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_sync_all(
    client: *const XmtpClient,
    consent_states: *const i32,
    consent_states_count: i32,
    out_synced: *mut i32,
    out_eligible: *mut i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let consents = parse_consent_filter(consent_states, consent_states_count);
        let summary = c.inner.sync_all_welcomes_and_groups(consents).await?;
        if !out_synced.is_null() {
            unsafe {
                *out_synced = summary.num_synced as i32;
            }
        }
        if !out_eligible.is_null() {
            unsafe {
                *out_eligible = summary.num_eligible as i32;
            }
        }
        Ok(())
    })
}

/// Sync preferences (device sync groups only).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_sync_preferences(
    client: *const XmtpClient,
    out_synced: *mut i32,
    out_eligible: *mut i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let summary = c.inner.sync_all_welcomes_and_device_sync_groups().await?;
        if !out_synced.is_null() {
            unsafe { *out_synced = summary.num_synced as i32 };
        }
        if !out_eligible.is_null() {
            unsafe { *out_eligible = summary.num_eligible as i32 };
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// HMAC keys (all conversations)
// ---------------------------------------------------------------------------

/// Get HMAC keys for all conversations (including duplicate DMs).
/// Returns a map via `out`. Caller must free with [`xmtp_hmac_key_map_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_hmac_keys(
    client: *const XmtpClient,
    out: *mut *mut XmtpHmacKeyMap,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let conversations = c.inner.find_groups(GroupQueryArgs {
            include_duplicate_dms: true,
            ..Default::default()
        })?;

        let mut entries = Vec::new();
        for conv in conversations {
            if let Ok(keys) = conv.hmac_keys(-1..=1) {
                let mut c_keys: Vec<XmtpHmacKey> = keys
                    .into_iter()
                    .map(|k| {
                        let mut key_vec = k.key.to_vec();
                        let len = key_vec.len() as i32;
                        let ptr = key_vec.as_mut_ptr();
                        std::mem::forget(key_vec);
                        XmtpHmacKey {
                            key: ptr,
                            key_len: len,
                            epoch: k.epoch,
                        }
                    })
                    .collect();
                let keys_count = c_keys.len() as i32;
                let keys_ptr = c_keys.as_mut_ptr();
                std::mem::forget(c_keys);
                entries.push(XmtpHmacKeyEntry {
                    group_id: to_c_string(&hex::encode(&conv.group_id)),
                    keys: keys_ptr,
                    keys_count,
                });
            }
        }

        unsafe { write_out(out, XmtpHmacKeyMap { entries })? };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Process streamed messages
// ---------------------------------------------------------------------------

/// Process a raw welcome message received via push notification.
/// Returns a list of conversation handles. Caller must free with [`xmtp_conversation_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_process_streamed_welcome_message(
    client: *const XmtpClient,
    envelope_bytes: *const u8,
    envelope_bytes_len: i32,
    out: *mut *mut XmtpConversationList,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(envelope_bytes, envelope_bytes_len as usize) }
                .to_vec();
        let groups = c.inner.process_streamed_welcome_message(bytes).await?;
        let items: Vec<InnerGroup> = groups.into_iter().collect();
        let list = Box::new(XmtpConversationList { items });
        unsafe { *out = Box::into_raw(list) };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Enriched message by ID
// ---------------------------------------------------------------------------

/// Get an enriched (decoded) message by its hex-encoded ID.
/// Caller must free with [`xmtp_enriched_message_list_free`] (single-item list).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_get_enriched_message_by_id(
    client: *const XmtpClient,
    message_id: *const c_char,
    out: *mut *mut XmtpEnrichedMessageList,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let id_str = unsafe { c_str_to_string(message_id)? };
        let id_bytes = hex::decode(&id_str)?;
        let msg = c.inner.message_v2(id_bytes)?;
        let item = crate::conversation::decoded_to_enriched(&msg);
        let list = Box::new(XmtpEnrichedMessageList { items: vec![item] });
        unsafe { *out = Box::into_raw(list) };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Optimistic group creation
// ---------------------------------------------------------------------------

/// Create a group without syncing members (optimistic / offline-capable).
/// Caller must free with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_group_optimistic(
    client: *const XmtpClient,
    opts: *const XmtpCreateGroupOptions,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let (policy_set, metadata_opts) = unsafe { parse_group_opts(opts)? };
        let group = c.inner.create_group(policy_set, metadata_opts)?;
        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse group creation options into (PolicySet, GroupMetadataOptions).
unsafe fn parse_group_opts(
    opts: *const XmtpCreateGroupOptions,
) -> Result<
    (
        Option<xmtp_mls::groups::group_permissions::PolicySet>,
        Option<xmtp_mls::mls_common::group::GroupMetadataOptions>,
    ),
    Box<dyn std::error::Error>,
> {
    if opts.is_null() {
        return Ok((None, None));
    }
    let o = unsafe { &*opts };
    let policy = match o.permissions {
        0 => Some(xmtp_mls::groups::PreconfiguredPolicies::Default.to_policy_set()),
        1 => Some(xmtp_mls::groups::PreconfiguredPolicies::AdminsOnly.to_policy_set()),
        _ => None,
    };
    let mut meta = xmtp_mls::mls_common::group::GroupMetadataOptions::default();
    meta.name = unsafe { c_str_to_option(o.name)? };
    meta.description = unsafe { c_str_to_option(o.description)? };
    meta.image_url_square = unsafe { c_str_to_option(o.image_url)? };
    meta.app_data = unsafe { c_str_to_option(o.app_data)? };
    if o.message_disappear_from_ns > 0 && o.message_disappear_in_ns > 0 {
        meta.message_disappearing_settings = Some(
            xmtp_mls_common::group_mutable_metadata::MessageDisappearingSettings::new(
                o.message_disappear_from_ns,
                o.message_disappear_in_ns,
            ),
        );
    }
    Ok((policy, Some(meta)))
}

/// Parse a consent state filter from a raw int array.
fn parse_consent_filter(
    states: *const i32,
    count: i32,
) -> Option<Vec<xmtp_db::consent_record::ConsentState>> {
    if states.is_null() || count <= 0 {
        return None;
    }
    let mut result = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let s = unsafe { *states.add(i) };
        result.push(match s {
            0 => xmtp_db::consent_record::ConsentState::Unknown,
            1 => xmtp_db::consent_record::ConsentState::Allowed,
            2 => xmtp_db::consent_record::ConsentState::Denied,
            _ => continue,
        });
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}
