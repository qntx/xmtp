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
}

// ---------------------------------------------------------------------------
// Group / DM creation
// ---------------------------------------------------------------------------

/// Create a new group conversation. Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_group(
    client: *const XmtpClient,
    opts: *const XmtpCreateGroupOptions,
    out: *mut *mut XmtpConversation,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let (policy_set, metadata) = if opts.is_null() {
            (None, None)
        } else {
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
            (policy, Some(meta))
        };

        let group = c.inner.create_group(policy_set, metadata)?;
        unsafe { write_out(out, XmtpConversation { inner: group })? };
        Ok(())
    })
}

/// Find or create a DM by identifier. Caller must free result with [`xmtp_conversation_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_dm(
    client: *const XmtpClient,
    identifier: *const c_char,
    identifier_kind: i32,
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
        let group = c.inner.find_or_create_dm_by_identity(ident, None).await?;
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
            GroupQueryArgs {
                conversation_type: match o.conversation_type {
                    0 => Some(xmtp_db::group::ConversationType::Dm),
                    1 => Some(xmtp_db::group::ConversationType::Group),
                    _ => None,
                },
                limit: if o.limit > 0 { Some(o.limit) } else { None },
                created_after_ns: if o.created_after_ns > 0 { Some(o.created_after_ns) } else { None },
                created_before_ns: if o.created_before_ns > 0 { Some(o.created_before_ns) } else { None },
                ..Default::default()
            }
        };

        let items: Vec<XmtpConversationListItem> = c
            .inner
            .list_conversations(args)?
            .into_iter()
            .map(|item| XmtpConversationListItem {
                group: item.group,
                last_message: item.last_message,
            })
            .collect();

        unsafe { write_out(out, XmtpConversationList { items })? };
        Ok(())
    })
}

/// Get the number of conversations in a list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_len(list: *const XmtpConversationList) -> i32 {
    match unsafe { ref_from(list) } {
        Ok(l) => l.items.len() as i32,
        Err(_) => 0,
    }
}

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
        let src = &l.items[idx].group;
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

/// Free a conversation list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_free(list: *mut XmtpConversationList) {
    if !list.is_null() {
        drop(unsafe { Box::from_raw(list) });
    }
}

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

/// Sync all conversations. Writes summary counts to `out_synced` and `out_eligible`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_sync_all(
    client: *const XmtpClient,
    out_synced: *mut i32,
    out_eligible: *mut i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let summary = c.inner.sync_all_welcomes_and_groups(None).await?;
        if !out_synced.is_null() {
            unsafe { *out_synced = summary.num_synced as i32; }
        }
        if !out_eligible.is_null() {
            unsafe { *out_eligible = summary.num_eligible as i32; }
        }
        Ok(())
    })
}
