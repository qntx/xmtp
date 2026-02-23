//! Single conversation operations: send, messages, members, metadata, permissions, consent.

use std::ffi::{CStr, CString, c_char};

use xmtp_db::group::DmIdExt;
use xmtp_mls::groups::UpdateAdminListType;

use crate::ffi::*;

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

free_opaque!(xmtp_conversation_free, FfiConversation);

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Get the conversation's hex-encoded group ID. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_id(conv: *const FfiConversation) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => to_c_string(&hex::encode(&c.inner.group_id)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the conversation created-at timestamp in nanoseconds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_created_at_ns(conv: *const FfiConversation) -> i64 {
    match unsafe { ref_from(conv) } {
        Ok(c) => c.inner.created_at_ns,
        Err(_) => 0,
    }
}

/// Get the conversation type. Returns `FfiConversationType` value, or -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_type(conv: *const FfiConversation) -> i32 {
    match unsafe { ref_from(conv) } {
        Ok(c) => conversation_type_to_ffi(c.inner.conversation_type) as i32,
        Err(_) => -1,
    }
}

/// Get the DM peer's inbox ID. Caller must free with [`xmtp_free_string`].
/// Returns null if not a DM or on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_dm_peer_inbox_id(
    conv: *const FfiConversation,
) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => {
            let inbox_id = c.inner.context.inbox_id();
            match &c.inner.dm_id {
                Some(dm_id) => to_c_string(&dm_id.other_inbox_id(inbox_id)),
                None => std::ptr::null_mut(),
            }
        }
        Err(_) => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// Sync this conversation with the network.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_sync(conv: *const FfiConversation) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        c.inner.sync().await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Send
// ---------------------------------------------------------------------------

/// Send raw encoded content bytes. Returns the message ID (hex) via `out_id`.
/// Caller must free `out_id` with [`xmtp_free_string`].
/// Pass null for `opts` to use defaults (should_push = true).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_send(
    conv: *const FfiConversation,
    content_bytes: *const u8,
    content_len: i32,
    opts: *const FfiSendOpts,
    out_id: *mut *mut c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        if content_bytes.is_null() || content_len <= 0 {
            return Err("null or empty content".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(content_bytes, content_len as usize) };

        let send_opts = if opts.is_null() {
            xmtp_mls::groups::send_message_opts::SendMessageOpts::default()
        } else {
            let o = unsafe { &*opts };
            xmtp_mls::groups::send_message_opts::SendMessageOpts {
                should_push: o.should_push != 0,
            }
        };
        let msg_id = c.inner.send_message(bytes, send_opts).await?;

        if !out_id.is_null() {
            unsafe {
                *out_id = to_c_string(&hex::encode(&msg_id));
            }
        }
        Ok(())
    })
}

/// Send raw encoded content bytes optimistically (returns immediately, publishes in background).
/// Returns the message ID (hex) via `out_id`. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_send_optimistic(
    conv: *const FfiConversation,
    content_bytes: *const u8,
    content_len: i32,
    opts: *const FfiSendOpts,
    out_id: *mut *mut c_char,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if content_bytes.is_null() || content_len <= 0 {
            return Err("null or empty content".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(content_bytes, content_len as usize) };

        let send_opts = if opts.is_null() {
            xmtp_mls::groups::send_message_opts::SendMessageOpts::default()
        } else {
            let o = unsafe { &*opts };
            xmtp_mls::groups::send_message_opts::SendMessageOpts {
                should_push: o.should_push != 0,
            }
        };
        let msg_id = c.inner.send_message_optimistic(bytes, send_opts)?;

        if !out_id.is_null() {
            unsafe {
                *out_id = to_c_string(&hex::encode(&msg_id));
            }
        }
        Ok(())
    })
}

/// Publish all queued (unpublished) messages in this conversation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_publish_messages(conv: *const FfiConversation) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        c.inner.publish_messages().await?;
        Ok(())
    })
}

/// Prepare a message for later publishing (optimistic send workflow).
/// Stores the message locally without publishing. Returns message ID (hex) via `out_id`.
/// Caller must free `out_id` with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_prepare_message(
    conv: *const FfiConversation,
    content_bytes: *const u8,
    content_len: i32,
    should_push: i32,
    out_id: *mut *mut c_char,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if content_bytes.is_null() || content_len <= 0 {
            return Err("null or empty content".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(content_bytes, content_len as usize) };
        let msg_id = c
            .inner
            .prepare_message_for_later_publish(bytes, should_push != 0)?;
        if !out_id.is_null() {
            unsafe {
                *out_id = to_c_string(&hex::encode(&msg_id));
            }
        }
        Ok(())
    })
}

/// Publish a previously prepared message by its hex-encoded ID.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_publish_stored_message(
    conv: *const FfiConversation,
    message_id_hex: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let id_hex = unsafe { c_str_to_string(message_id_hex)? };
        let id_bytes = hex::decode(&id_hex)?;
        c.inner.publish_stored_message(&id_bytes).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Options for listing messages.
#[repr(C)]
pub struct FfiListMessagesOptions {
    /// Only messages sent after this timestamp (ns). 0 = no filter.
    pub sent_after_ns: i64,
    /// Only messages sent before this timestamp (ns). 0 = no filter.
    pub sent_before_ns: i64,
    /// Only messages inserted after this timestamp (ns). 0 = no filter.
    pub inserted_after_ns: i64,
    /// Only messages inserted before this timestamp (ns). 0 = no filter.
    pub inserted_before_ns: i64,
    /// Maximum number of messages. 0 = no limit.
    pub limit: i64,
    /// Filter by delivery status: -1 = all, 0 = Unpublished, 1 = Published, 2 = Failed.
    pub delivery_status: i32,
    /// Filter by message kind: -1 = all, 0 = Application, 1 = MembershipChange.
    pub kind: i32,
    /// Sort direction: 0 = Ascending (default), 1 = Descending.
    pub direction: i32,
    /// Sort by: 0 = SentAt (default), 1 = InsertedAt.
    pub sort_by: i32,
    /// Include only these content types (nullable). Each element is a ContentType i32 value.
    pub content_types: *const i32,
    /// Number of elements in `content_types`. 0 = no filter.
    pub content_types_count: i32,
    /// Exclude these content types (nullable). Each element is a ContentType i32 value.
    pub exclude_content_types: *const i32,
    /// Number of elements in `exclude_content_types`. 0 = no filter.
    pub exclude_content_types_count: i32,
    /// Exclude messages from these sender inbox IDs (nullable C string array).
    pub exclude_sender_inbox_ids: *const *const c_char,
    /// Number of elements in `exclude_sender_inbox_ids`. 0 = no filter.
    pub exclude_sender_inbox_ids_count: i32,
    /// Whether to exclude disappearing messages. 0 = include (default), 1 = exclude.
    pub exclude_disappearing: i32,
}

/// Parse message query options from C struct into `MsgQueryArgs`.
fn parse_msg_query_args(
    opts: *const FfiListMessagesOptions,
) -> xmtp_db::group_message::MsgQueryArgs {
    use xmtp_db::group_message::*;

    let mut args = MsgQueryArgs::default();
    if opts.is_null() {
        return args;
    }
    let o = unsafe { &*opts };

    if o.sent_after_ns > 0 {
        args.sent_after_ns = Some(o.sent_after_ns);
    }
    if o.sent_before_ns > 0 {
        args.sent_before_ns = Some(o.sent_before_ns);
    }
    if o.inserted_after_ns > 0 {
        args.inserted_after_ns = Some(o.inserted_after_ns);
    }
    if o.inserted_before_ns > 0 {
        args.inserted_before_ns = Some(o.inserted_before_ns);
    }
    if o.limit > 0 {
        args.limit = Some(o.limit);
    }
    args.delivery_status = match o.delivery_status {
        0 => Some(DeliveryStatus::Unpublished),
        1 => Some(DeliveryStatus::Published),
        2 => Some(DeliveryStatus::Failed),
        _ => None,
    };
    args.kind = match o.kind {
        0 => Some(GroupMessageKind::Application),
        1 => Some(GroupMessageKind::MembershipChange),
        _ => None,
    };
    args.direction = match o.direction {
        1 => Some(SortDirection::Descending),
        _ => None, // 0 or default = Ascending (MsgQueryArgs default)
    };
    args.sort_by = match o.sort_by {
        1 => Some(SortBy::InsertedAt),
        _ => None, // 0 or default = SentAt (MsgQueryArgs default)
    };
    if !o.content_types.is_null() && o.content_types_count > 0 {
        let slice =
            unsafe { std::slice::from_raw_parts(o.content_types, o.content_types_count as usize) };
        let cts: Vec<ContentType> = slice
            .iter()
            .filter_map(|&v| i32_to_content_type(v))
            .collect();
        if !cts.is_empty() {
            args.content_types = Some(cts);
        }
    }
    if !o.exclude_content_types.is_null() && o.exclude_content_types_count > 0 {
        let slice = unsafe {
            std::slice::from_raw_parts(
                o.exclude_content_types,
                o.exclude_content_types_count as usize,
            )
        };
        let cts: Vec<ContentType> = slice
            .iter()
            .filter_map(|&v| i32_to_content_type(v))
            .collect();
        if !cts.is_empty() {
            args.exclude_content_types = Some(cts);
        }
    }
    if !o.exclude_sender_inbox_ids.is_null() && o.exclude_sender_inbox_ids_count > 0 {
        let ids = unsafe {
            collect_strings(o.exclude_sender_inbox_ids, o.exclude_sender_inbox_ids_count)
        };
        if let Ok(ids) = ids {
            if !ids.is_empty() {
                args.exclude_sender_inbox_ids = Some(ids);
            }
        }
    }
    args.exclude_disappearing = o.exclude_disappearing != 0;

    args
}

/// List messages in this conversation. Caller must free with [`xmtp_message_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_messages(
    conv: *const FfiConversation,
    opts: *const FfiListMessagesOptions,
    out: *mut *mut FfiMessageList,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let args = parse_msg_query_args(opts);
        let messages = c.inner.find_messages(&args)?;
        unsafe { write_out(out, FfiMessageList { items: messages })? };
        Ok(())
    })
}

/// Count messages matching the given filter options.
/// Pass null for `opts` to count all messages.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_count_messages(
    conv: *const FfiConversation,
    opts: *const FfiListMessagesOptions,
) -> i64 {
    match unsafe { ref_from(conv) } {
        Ok(c) => {
            let args = parse_msg_query_args(opts);
            c.inner.count_messages(&args).unwrap_or(0)
        }
        Err(_) => 0,
    }
}

// --- Message list accessors ---

ffi_list_len!(xmtp_message_list_len, FfiMessageList);

/// Helper to safely access a message at index.
unsafe fn msg_at(
    list: *const FfiMessageList,
    idx: i32,
) -> Option<&'static xmtp_db::group_message::StoredGroupMessage> {
    let l = unsafe { ref_from(list).ok()? };
    l.items.get(idx as usize)
}

/// Get message ID (hex) at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_id(list: *const FfiMessageList, index: i32) -> *mut c_char {
    match unsafe { msg_at(list, index) } {
        Some(m) => to_c_string(&hex::encode(&m.id)),
        None => std::ptr::null_mut(),
    }
}

/// Get sender inbox ID at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_sender_inbox_id(
    list: *const FfiMessageList,
    index: i32,
) -> *mut c_char {
    match unsafe { msg_at(list, index) } {
        Some(m) => to_c_string(&m.sender_inbox_id),
        None => std::ptr::null_mut(),
    }
}

/// Get sent-at timestamp (ns) at index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_sent_at_ns(list: *const FfiMessageList, index: i32) -> i64 {
    unsafe { msg_at(list, index) }.map_or(0, |m| m.sent_at_ns)
}

/// Get message kind at index: 0=Application, 1=MembershipChange, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_kind(list: *const FfiMessageList, index: i32) -> i32 {
    match unsafe { msg_at(list, index) } {
        Some(m) => match m.kind {
            xmtp_db::group_message::GroupMessageKind::Application => 0,
            xmtp_db::group_message::GroupMessageKind::MembershipChange => 1,
        },
        None => -1,
    }
}

/// Get delivery status at index: 0=Unpublished, 1=Published, 2=Failed, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_delivery_status(
    list: *const FfiMessageList,
    index: i32,
) -> i32 {
    match unsafe { msg_at(list, index) } {
        Some(m) => match m.delivery_status {
            xmtp_db::group_message::DeliveryStatus::Unpublished => 0,
            xmtp_db::group_message::DeliveryStatus::Published => 1,
            xmtp_db::group_message::DeliveryStatus::Failed => 2,
        },
        None => -1,
    }
}

/// Get raw decrypted content bytes at index. Writes length to `out_len`.
/// The returned pointer is borrowed from the list — do NOT free it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_content_bytes(
    list: *const FfiMessageList,
    index: i32,
    out_len: *mut i32,
) -> *const u8 {
    if out_len.is_null() {
        return std::ptr::null();
    }
    match unsafe { msg_at(list, index) } {
        Some(m) => {
            unsafe {
                *out_len = m.decrypted_message_bytes.len() as i32;
            }
            m.decrypted_message_bytes.as_ptr()
        }
        None => {
            unsafe {
                *out_len = 0;
            }
            std::ptr::null()
        }
    }
}

free_opaque!(xmtp_message_list_free, FfiMessageList);
free_opaque!(xmtp_message_free, FfiMessage);

// --- Single-message accessors (for stream callback data extraction) ---

/// Get the message ID (hex) from a single message handle.
/// Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_single_message_id(msg: *const FfiMessage) -> *mut c_char {
    match unsafe { ref_from(msg) } {
        Ok(m) => to_c_string(&hex::encode(&m.inner.id)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the group ID (hex) from a single message handle.
/// Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_single_message_group_id(msg: *const FfiMessage) -> *mut c_char {
    match unsafe { ref_from(msg) } {
        Ok(m) => to_c_string(&hex::encode(&m.inner.group_id)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the sender inbox ID from a single message handle.
/// Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_single_message_sender_inbox_id(
    msg: *const FfiMessage,
) -> *mut c_char {
    match unsafe { ref_from(msg) } {
        Ok(m) => to_c_string(&m.inner.sender_inbox_id),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the sent-at timestamp (ns) from a single message handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_single_message_sent_at_ns(msg: *const FfiMessage) -> i64 {
    match unsafe { ref_from(msg) } {
        Ok(m) => m.inner.sent_at_ns,
        Err(_) => 0,
    }
}

/// Get raw content bytes from a single message handle.
/// The returned pointer is borrowed — valid only while the message handle is alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_single_message_content_bytes(
    msg: *const FfiMessage,
    out_len: *mut i32,
) -> *const u8 {
    if out_len.is_null() {
        return std::ptr::null();
    }
    match unsafe { ref_from(msg) } {
        Ok(m) => {
            unsafe { *out_len = m.inner.decrypted_message_bytes.len() as i32 };
            m.inner.decrypted_message_bytes.as_ptr()
        }
        Err(_) => {
            unsafe { *out_len = 0 };
            std::ptr::null()
        }
    }
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

/// List group members. Caller must free with [`xmtp_group_member_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_members(
    conv: *const FfiConversation,
    out: *mut *mut FfiGroupMemberList,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let members_raw = c.inner.members().await?;
        let members: Vec<FfiGroupMember> = members_raw
            .into_iter()
            .map(|m| {
                use xmtp_mls::groups::members::PermissionLevel;
                // Build account identifiers array
                let ident_strs: Vec<String> = m
                    .account_identifiers
                    .iter()
                    .map(|i| i.to_string())
                    .collect();
                let mut ident_count: i32 = 0;
                let ident_ptrs = string_vec_to_c(ident_strs, &mut ident_count);
                // Build installation IDs array (hex-encoded)
                let inst_strs: Vec<String> = m.installation_ids.iter().map(hex::encode).collect();
                let mut inst_count: i32 = 0;
                let inst_ptrs = string_vec_to_c(inst_strs, &mut inst_count);
                FfiGroupMember {
                    inbox_id: to_c_string(&m.inbox_id),
                    permission_level: match m.permission_level {
                        PermissionLevel::Member => FfiPermissionLevel::Member,
                        PermissionLevel::Admin => FfiPermissionLevel::Admin,
                        PermissionLevel::SuperAdmin => FfiPermissionLevel::SuperAdmin,
                    },
                    consent_state: consent_state_to_ffi(m.consent_state),
                    account_identifiers: ident_ptrs,
                    account_identifiers_count: ident_count,
                    installation_ids: inst_ptrs,
                    installation_ids_count: inst_count,
                }
            })
            .collect();
        unsafe { write_out(out, FfiGroupMemberList { items: members })? };
        Ok(())
    })
}

ffi_list_len!(xmtp_group_member_list_len, FfiGroupMemberList);

/// Get member inbox ID at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_inbox_id(
    list: *const FfiGroupMemberList,
    index: i32,
) -> *mut c_char {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return std::ptr::null_mut(),
    };
    match l.items.get(index as usize) {
        // Return a copy; original owned by the list
        Some(m) if !m.inbox_id.is_null() => {
            let s = unsafe { CStr::from_ptr(m.inbox_id) };
            to_c_string(s.to_str().unwrap_or(""))
        }
        _ => std::ptr::null_mut(),
    }
}

/// Get member permission level at index: 0=Member, 1=Admin, 2=SuperAdmin, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_permission_level(
    list: *const FfiGroupMemberList,
    index: i32,
) -> i32 {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return -1,
    };
    l.items
        .get(index as usize)
        .map_or(-1, |m| m.permission_level as i32)
}

/// Get member consent state at index: 0=Unknown, 1=Allowed, 2=Denied, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_consent_state(
    list: *const FfiGroupMemberList,
    index: i32,
) -> i32 {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return -1,
    };
    l.items
        .get(index as usize)
        .map_or(-1, |m| m.consent_state as i32)
}

/// Get member account identifiers at index.
/// Returns a borrowed pointer to the internal string array. Do NOT free individual strings.
/// Use `out_count` to get the number of identifiers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_account_identifiers(
    list: *const FfiGroupMemberList,
    index: i32,
    out_count: *mut i32,
) -> *const *mut c_char {
    if out_count.is_null() {
        return std::ptr::null();
    }
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => {
            unsafe { *out_count = 0 };
            return std::ptr::null();
        }
    };
    match l.items.get(index as usize) {
        Some(m) => {
            unsafe { *out_count = m.account_identifiers_count };
            m.account_identifiers as *const *mut c_char
        }
        None => {
            unsafe { *out_count = 0 };
            std::ptr::null()
        }
    }
}

/// Get member installation IDs (hex) at index.
/// Returns a borrowed pointer to the internal string array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_installation_ids(
    list: *const FfiGroupMemberList,
    index: i32,
    out_count: *mut i32,
) -> *const *mut c_char {
    if out_count.is_null() {
        return std::ptr::null();
    }
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => {
            unsafe { *out_count = 0 };
            return std::ptr::null();
        }
    };
    match l.items.get(index as usize) {
        Some(m) => {
            unsafe { *out_count = m.installation_ids_count };
            m.installation_ids as *const *mut c_char
        }
        None => {
            unsafe { *out_count = 0 };
            std::ptr::null()
        }
    }
}

/// Free a group member list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_list_free(list: *mut FfiGroupMemberList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for m in &l.items {
        free_c_strings!(m, inbox_id);
        free_c_string_array(m.account_identifiers, m.account_identifiers_count);
        free_c_string_array(m.installation_ids, m.installation_ids_count);
    }
}

// ---------------------------------------------------------------------------
// Membership mutations
// ---------------------------------------------------------------------------

/// Add members by inbox IDs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_add_members(
    conv: *const FfiConversation,
    inbox_ids: *const *const c_char,
    count: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let ids = unsafe { collect_strings(inbox_ids, count)? };
        c.inner.add_members(&ids).await?;
        Ok(())
    })
}

/// Remove members by inbox IDs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_remove_members(
    conv: *const FfiConversation,
    inbox_ids: *const *const c_char,
    count: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let ids = unsafe { collect_strings(inbox_ids, count)? };
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        c.inner.remove_members(&refs).await?;
        Ok(())
    })
}

/// Leave the group.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_leave(conv: *const FfiConversation) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        c.inner.leave_group().await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Admin management
// ---------------------------------------------------------------------------

/// Add/remove admin or super admin. `action`: 0=AddAdmin, 1=RemoveAdmin, 2=AddSuperAdmin, 3=RemoveSuperAdmin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_admin_list(
    conv: *const FfiConversation,
    inbox_id: *const c_char,
    action: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let id = unsafe { c_str_to_string(inbox_id)? };
        let update_type = match action {
            0 => UpdateAdminListType::Add,
            1 => UpdateAdminListType::Remove,
            2 => UpdateAdminListType::AddSuper,
            3 => UpdateAdminListType::RemoveSuper,
            _ => return Err("invalid admin action".into()),
        };
        c.inner.update_admin_list(update_type, id).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Metadata (DRY via macros)
// ---------------------------------------------------------------------------

conv_string_getter!(xmtp_conversation_group_name, group_name);
conv_string_setter!(xmtp_conversation_update_group_name, update_group_name);
conv_string_getter!(xmtp_conversation_group_description, group_description);
conv_string_setter!(
    xmtp_conversation_update_group_description,
    update_group_description
);
conv_string_getter!(xmtp_conversation_group_image_url, group_image_url_square);
conv_string_setter!(
    xmtp_conversation_update_group_image_url,
    update_group_image_url_square
);

// ---------------------------------------------------------------------------
// Consent
// ---------------------------------------------------------------------------

/// Get conversation consent state. Writes to `out_state` (0=Unknown, 1=Allowed, 2=Denied).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_consent_state(
    conv: *const FfiConversation,
    out_state: *mut i32,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out_state.is_null() {
            return Err("null output pointer".into());
        }
        let state = c.inner.consent_state()?;
        unsafe {
            *out_state = consent_state_to_ffi(state) as i32;
        }
        Ok(())
    })
}

/// Update conversation consent state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_consent_state(
    conv: *const FfiConversation,
    state: i32,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        let consent_state = i32_to_consent_state(state)?;
        c.inner.update_consent_state(consent_state)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Update a permission policy on this conversation.
/// `update_type`: 1=AddMember, 2=RemoveMember, 3=AddAdmin, 4=RemoveAdmin, 5=UpdateMetadata.
/// `policy`: 1=Allow, 2=Deny, 3=AdminOnly, 4=SuperAdminOnly.
/// `metadata_field_name`: Only used when update_type=5 (e.g. "group_name"). Pass null otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_permission_policy(
    conv: *const FfiConversation,
    update_type: i32,
    policy: i32,
    metadata_field_name: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };

        use xmtp_mls::groups::intents::{PermissionPolicyOption, PermissionUpdateType};

        let perm_update = match update_type {
            1 => PermissionUpdateType::AddMember,
            2 => PermissionUpdateType::RemoveMember,
            3 => PermissionUpdateType::AddAdmin,
            4 => PermissionUpdateType::RemoveAdmin,
            5 => PermissionUpdateType::UpdateMetadata,
            _ => return Err("invalid permission update type".into()),
        };

        let perm_policy = match policy {
            1 => PermissionPolicyOption::Allow,
            2 => PermissionPolicyOption::Deny,
            3 => PermissionPolicyOption::AdminOnly,
            4 => PermissionPolicyOption::SuperAdminOnly,
            _ => return Err("invalid permission policy option".into()),
        };

        let meta_field_str = unsafe { c_str_to_option(metadata_field_name)? };
        // Convert string field name to MetadataField enum
        // MetadataField is re-exported through xmtp_mls::groups::group_permissions
        use xmtp_mls_common::group_mutable_metadata::MetadataField;
        let meta_field = match meta_field_str {
            Some(s) => Some(match s.as_str() {
                "group_name" => MetadataField::GroupName,
                "description" => MetadataField::Description,
                "group_image_url_square" => MetadataField::GroupImageUrlSquare,
                "message_disappear_from_ns" => MetadataField::MessageDisappearFromNS,
                "message_disappear_in_ns" => MetadataField::MessageDisappearInNS,
                other => return Err(format!("unknown metadata field: {other}").into()),
            }),
            None => None,
        };

        c.inner
            .update_permission_policy(perm_update, perm_policy, meta_field)
            .await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Query: is_active, membership_state, added_by
// ---------------------------------------------------------------------------

/// Check if the conversation is active. Returns 1=active, 0=inactive, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_is_active(conv: *const FfiConversation) -> i32 {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.is_active() {
            Ok(true) => 1,
            Ok(false) => 0,
            Err(_) => -1,
        },
        Err(_) => -1,
    }
}

/// Get the membership state of the current user in this conversation.
/// 0=Allowed, 1=Rejected, 2=Pending, 3=Restored, 4=PendingRemove, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_membership_state(conv: *const FfiConversation) -> i32 {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.membership_state() {
            Ok(state) => {
                use xmtp_db::group::GroupMembershipState;
                match state {
                    GroupMembershipState::Allowed => 0,
                    GroupMembershipState::Rejected => 1,
                    GroupMembershipState::Pending => 2,
                    GroupMembershipState::Restored => 3,
                    GroupMembershipState::PendingRemove => 4,
                }
            }
            Err(_) => -1,
        },
        Err(_) => -1,
    }
}

conv_string_getter!(xmtp_conversation_added_by_inbox_id, added_by_inbox_id);

// ---------------------------------------------------------------------------
// Admin queries
// ---------------------------------------------------------------------------

conv_string_list_getter!(xmtp_conversation_list_admins, admin_list);
conv_string_list_getter!(xmtp_conversation_list_super_admins, super_admin_list);
conv_inbox_check!(xmtp_conversation_is_admin, admin_list);
conv_inbox_check!(xmtp_conversation_is_super_admin, super_admin_list);

// ---------------------------------------------------------------------------
// Membership by identity (Identifier-based)
// ---------------------------------------------------------------------------

/// Add members by external identifiers (address/passkey).
/// `identifiers` and `kinds` are parallel arrays of length `count`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_add_members_by_identity(
    conv: *const FfiConversation,
    identifiers: *const *const c_char,
    kinds: *const i32,
    count: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let idents = unsafe { collect_identifiers(identifiers, kinds, count)? };
        c.inner.add_members_by_identity(&idents).await?;
        Ok(())
    })
}

/// Remove members by external identifiers (address/passkey).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_remove_members_by_identity(
    conv: *const FfiConversation,
    identifiers: *const *const c_char,
    kinds: *const i32,
    count: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let idents = unsafe { collect_identifiers(identifiers, kinds, count)? };
        c.inner.remove_members_by_identity(&idents).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Disappearing messages
// ---------------------------------------------------------------------------

/// Options for message disappearing settings.
#[repr(C)]
pub struct FfiDisappearingSettings {
    /// Timestamp (ns) from which messages start disappearing.
    pub from_ns: i64,
    /// Duration (ns) after which messages disappear.
    pub in_ns: i64,
}

/// Update the message disappearing settings for this conversation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_disappearing_settings(
    conv: *const FfiConversation,
    settings: *const FfiDisappearingSettings,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let s = unsafe { ref_from(settings)? };
        let mds = xmtp_mls_common::group_mutable_metadata::MessageDisappearingSettings::new(
            s.from_ns, s.in_ns,
        );
        c.inner
            .update_conversation_message_disappearing_settings(mds)
            .await?;
        Ok(())
    })
}

/// Remove (disable) message disappearing settings for this conversation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_remove_disappearing_settings(
    conv: *const FfiConversation,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        c.inner
            .remove_conversation_message_disappearing_settings()
            .await?;
        Ok(())
    })
}

/// Get the current message disappearing settings.
/// Returns 0 on success (writes to `out`), -1 if not set or on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_disappearing_settings(
    conv: *const FfiConversation,
    out: *mut FfiDisappearingSettings,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let settings = c
            .inner
            .disappearing_settings()?
            .ok_or("disappearing settings not set")?;
        unsafe {
            (*out).from_ns = settings.from_ns;
            (*out).in_ns = settings.in_ns;
        }
        Ok(())
    })
}

/// Check if message disappearing is enabled.
/// Returns 1=enabled, 0=disabled, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_is_disappearing_enabled(
    conv: *const FfiConversation,
) -> i32 {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.disappearing_settings() {
            Ok(Some(s)) => i32::from(s.from_ns > 0 && s.in_ns > 0),
            Ok(None) => 0,
            Err(_) => -1,
        },
        Err(_) => -1,
    }
}

// ---------------------------------------------------------------------------
// App data (DRY via macros)
// ---------------------------------------------------------------------------

conv_string_getter!(xmtp_conversation_app_data, app_data);
conv_string_setter!(xmtp_conversation_update_app_data, update_app_data);

// ---------------------------------------------------------------------------
// Duplicate DMs & paused version
// ---------------------------------------------------------------------------

/// Find duplicate DM conversations for this DM.
/// Returns a conversation list. Caller must free with [`xmtp_conversation_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_duplicate_dms(
    conv: *const FfiConversation,
    out: *mut *mut FfiConversationList,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let dups = c.inner.find_duplicate_dms()?;
        let items: Vec<FfiConversationListItemInner> = dups
            .into_iter()
            .map(|g| FfiConversationListItemInner {
                group: g,
                last_message: None,
                is_commit_log_forked: None,
            })
            .collect();
        unsafe { write_out(out, FfiConversationList { items })? };
        Ok(())
    })
}

/// Check if the conversation is paused for a version upgrade.
/// Writes the version string to `out` if paused, or null if not paused.
/// Caller must free `out` with [`xmtp_free_string`].
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_paused_for_version(
    conv: *const FfiConversation,
    out: *mut *mut c_char,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let version = c.inner.paused_for_version()?;
        unsafe {
            *out = match version {
                Some(v) => to_c_string(&v),
                None => std::ptr::null_mut(),
            };
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Debug info
// ---------------------------------------------------------------------------

/// Get debug info for this conversation.
/// Caller must free string fields with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_debug_info(
    conv: *const FfiConversation,
    out: *mut FfiConversationDebugInfo,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let info = c.inner.debug_info().await?;
        // Build cursor array on the heap
        let cursors: Vec<FfiCursor> = info
            .cursor
            .iter()
            .map(|c| FfiCursor {
                originator_id: c.originator_id,
                sequence_id: c.sequence_id,
            })
            .collect();
        let cursors_count = cursors.len() as i32;
        let (cursors_ptr, _, _) = cursors.into_raw_parts();

        unsafe {
            *out = FfiConversationDebugInfo {
                epoch: info.epoch,
                maybe_forked: i32::from(info.maybe_forked),
                fork_details: to_c_string(&info.fork_details),
                is_commit_log_forked: match info.is_commit_log_forked {
                    Some(true) => 1,
                    Some(false) => 0,
                    None => -1,
                },
                local_commit_log: to_c_string(&info.local_commit_log),
                remote_commit_log: to_c_string(&info.remote_commit_log),
                cursors: cursors_ptr,
                cursors_count,
            };
        }
        Ok(())
    })
}

/// Free a conversation debug info struct (its string fields and cursor array).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_debug_info_free(info: *mut FfiConversationDebugInfo) {
    if info.is_null() {
        return;
    }
    let i = unsafe { &mut *info };
    free_c_strings!(i, fork_details, local_commit_log, remote_commit_log);
    if !i.cursors.is_null() && i.cursors_count > 0 {
        drop(unsafe {
            Vec::from_raw_parts(
                i.cursors,
                i.cursors_count as usize,
                i.cursors_count as usize,
            )
        });
        i.cursors = std::ptr::null_mut();
        i.cursors_count = 0;
    }
}

// ---------------------------------------------------------------------------
// HMAC keys
// ---------------------------------------------------------------------------

/// Get HMAC keys for this conversation (including duplicate DMs).
/// Returns a map via `out`. Caller must free with [`xmtp_hmac_key_map_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_hmac_keys(
    conv: *const FfiConversation,
    out: *mut *mut FfiHmacKeyMap,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let mut entries = Vec::new();

        // Include duplicate DMs
        if let Ok(dups) = c.inner.find_duplicate_dms() {
            for dup in dups {
                if let Ok(keys) = dup.hmac_keys(-1..=1) {
                    entries.push(hmac_keys_to_entry(&dup.group_id, keys));
                }
            }
        }

        // Include this conversation
        let keys = c.inner.hmac_keys(-1..=1)?;
        entries.push(hmac_keys_to_entry(&c.inner.group_id, keys));

        unsafe { write_out(out, FfiHmacKeyMap { entries })? };
        Ok(())
    })
}

/// Convert a Vec<HmacKey> into a C-compatible FfiHmacKeyEntry.
pub(crate) fn hmac_keys_to_entry(
    group_id: &[u8],
    keys: Vec<xmtp_db::user_preferences::HmacKey>,
) -> FfiHmacKeyEntry {
    let c_keys: Vec<FfiHmacKey> = keys
        .into_iter()
        .map(|k| {
            let key_vec = k.key.to_vec();
            let len = key_vec.len() as i32;
            let (ptr, _, _) = key_vec.into_raw_parts();
            FfiHmacKey {
                key: ptr,
                key_len: len,
                epoch: k.epoch,
            }
        })
        .collect();
    let keys_count = c_keys.len() as i32;
    let (keys_ptr, _, _) = c_keys.into_raw_parts();
    FfiHmacKeyEntry {
        group_id: to_c_string(&hex::encode(group_id)),
        keys: keys_ptr,
        keys_count,
    }
}

/// Get the number of entries in an HMAC key map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_hmac_key_map_len(map: *const FfiHmacKeyMap) -> i32 {
    match unsafe { ref_from(map) } {
        Ok(m) => m.entries.len() as i32,
        Err(_) => 0,
    }
}

/// Get the group ID (hex) at index. Returns a borrowed pointer; do NOT free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_hmac_key_map_group_id(
    map: *const FfiHmacKeyMap,
    index: i32,
) -> *const c_char {
    let m = match unsafe { ref_from(map) } {
        Ok(m) => m,
        Err(_) => return std::ptr::null(),
    };
    match m.entries.get(index as usize) {
        Some(e) => e.group_id as *const c_char,
        None => std::ptr::null(),
    }
}

/// Get the HMAC keys at index. Writes count to `out_count`.
/// Returns a borrowed pointer to the key array; do NOT free individual keys.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_hmac_key_map_keys(
    map: *const FfiHmacKeyMap,
    index: i32,
    out_count: *mut i32,
) -> *const FfiHmacKey {
    if out_count.is_null() {
        return std::ptr::null();
    }
    let m = match unsafe { ref_from(map) } {
        Ok(m) => m,
        Err(_) => {
            unsafe { *out_count = 0 };
            return std::ptr::null();
        }
    };
    match m.entries.get(index as usize) {
        Some(e) => {
            unsafe { *out_count = e.keys_count };
            e.keys as *const FfiHmacKey
        }
        None => {
            unsafe { *out_count = 0 };
            std::ptr::null()
        }
    }
}

// ---------------------------------------------------------------------------
// Process streamed group message
// ---------------------------------------------------------------------------

/// Process a raw group message received via push notification.
/// Returns a list of stored messages. Caller must free with [`xmtp_message_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_process_streamed_group_message(
    conversation: *const FfiConversation,
    envelope_bytes: *const u8,
    envelope_bytes_len: i32,
    out: *mut *mut FfiMessageList,
) -> i32 {
    catch_async(|| async {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(envelope_bytes, envelope_bytes_len as usize) }
                .to_vec();
        let messages = conv.inner.process_streamed_group_message(bytes).await?;
        unsafe { write_out(out, FfiMessageList { items: messages })? };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Group metadata
// ---------------------------------------------------------------------------

/// Get the full group metadata (creator inbox ID + conversation type).
/// Caller must free with [`xmtp_group_metadata_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_group_metadata(
    conversation: *const FfiConversation,
    out: *mut *mut FfiGroupMetadata,
) -> i32 {
    catch_async(|| async {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let metadata = conv.inner.metadata().await?;
        unsafe {
            write_out(
                out,
                FfiGroupMetadata {
                    creator_inbox_id: to_c_string(&metadata.creator_inbox_id),
                    conversation_type: conversation_type_to_ffi(metadata.conversation_type),
                },
            )?
        };
        Ok(())
    })
}

/// Free a group metadata struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_metadata_free(meta: *mut FfiGroupMetadata) {
    if meta.is_null() {
        return;
    }
    let m = unsafe { Box::from_raw(meta) };
    if !m.creator_inbox_id.is_null() {
        drop(unsafe { CString::from_raw(m.creator_inbox_id) });
    }
}

// ---------------------------------------------------------------------------
// Group permissions
// ---------------------------------------------------------------------------

fn membership_policy_to_ffi(
    p: &xmtp_mls::groups::group_permissions::MembershipPolicies,
) -> FfiPermissionPolicy {
    use xmtp_mls::groups::group_permissions::{BasePolicies, MembershipPolicies};
    if let MembershipPolicies::Standard(base) = p {
        match base {
            BasePolicies::Allow => FfiPermissionPolicy::Allow,
            BasePolicies::Deny => FfiPermissionPolicy::Deny,
            BasePolicies::AllowIfAdminOrSuperAdmin => FfiPermissionPolicy::Admin,
            BasePolicies::AllowIfSuperAdmin => FfiPermissionPolicy::SuperAdmin,
            BasePolicies::AllowSameMember => FfiPermissionPolicy::Other,
        }
    } else {
        FfiPermissionPolicy::Other
    }
}

fn metadata_policy_to_ffi(
    p: &xmtp_mls::groups::group_permissions::MetadataPolicies,
) -> FfiPermissionPolicy {
    use xmtp_mls::groups::group_permissions::{MetadataBasePolicies, MetadataPolicies};
    if let MetadataPolicies::Standard(base) = p {
        match base {
            MetadataBasePolicies::Allow => FfiPermissionPolicy::Allow,
            MetadataBasePolicies::Deny => FfiPermissionPolicy::Deny,
            MetadataBasePolicies::AllowIfActorAdminOrSuperAdmin => FfiPermissionPolicy::Admin,
            MetadataBasePolicies::AllowIfActorSuperAdmin => FfiPermissionPolicy::SuperAdmin,
        }
    } else {
        FfiPermissionPolicy::Other
    }
}

fn permissions_policy_to_ffi(
    p: &xmtp_mls::groups::group_permissions::PermissionsPolicies,
) -> FfiPermissionPolicy {
    use xmtp_mls::groups::group_permissions::{PermissionsBasePolicies, PermissionsPolicies};
    if let PermissionsPolicies::Standard(base) = p {
        match base {
            PermissionsBasePolicies::Deny => FfiPermissionPolicy::Deny,
            PermissionsBasePolicies::AllowIfActorAdminOrSuperAdmin => FfiPermissionPolicy::Admin,
            PermissionsBasePolicies::AllowIfActorSuperAdmin => FfiPermissionPolicy::SuperAdmin,
        }
    } else {
        FfiPermissionPolicy::Other
    }
}

/// Get the group permissions (policy type + full policy set).
/// Caller must free with [`xmtp_group_permissions_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_group_permissions(
    conversation: *const FfiConversation,
    out: *mut *mut FfiGroupPermissions,
) -> i32 {
    catch(|| {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let perms = conv.inner.permissions()?;
        let policy_type = match perms.preconfigured_policy() {
            Ok(xmtp_mls::groups::PreconfiguredPolicies::Default) => {
                FfiGroupPermissionsPreset::AllMembers
            }
            Ok(xmtp_mls::groups::PreconfiguredPolicies::AdminsOnly) => {
                FfiGroupPermissionsPreset::AdminOnly
            }
            Err(_) => FfiGroupPermissionsPreset::Custom,
        };

        let ps = &perms.policies;
        let meta = &ps.update_metadata_policy;
        let get_meta = |field: &str| -> FfiPermissionPolicy {
            meta.get(field)
                .map(|p| metadata_policy_to_ffi(p))
                .unwrap_or(FfiPermissionPolicy::DoesNotExist)
        };

        use xmtp_mls::mls_common::group_mutable_metadata::MetadataField;
        let policy_set = FfiPermissionPolicySet {
            add_member_policy: membership_policy_to_ffi(&ps.add_member_policy),
            remove_member_policy: membership_policy_to_ffi(&ps.remove_member_policy),
            add_admin_policy: permissions_policy_to_ffi(&ps.add_admin_policy),
            remove_admin_policy: permissions_policy_to_ffi(&ps.remove_admin_policy),
            update_group_name_policy: get_meta(MetadataField::GroupName.as_str()),
            update_group_description_policy: get_meta(MetadataField::Description.as_str()),
            update_group_image_url_square_policy: get_meta(
                MetadataField::GroupImageUrlSquare.as_str(),
            ),
            update_message_disappearing_policy: get_meta(
                MetadataField::MessageDisappearInNS.as_str(),
            ),
            update_app_data_policy: get_meta(MetadataField::AppData.as_str()),
        };

        unsafe {
            write_out(
                out,
                FfiGroupPermissions {
                    policy_type,
                    policy_set,
                },
            )?
        };
        Ok(())
    })
}

free_opaque!(xmtp_group_permissions_free, FfiGroupPermissions);

// ---------------------------------------------------------------------------
// Enriched messages + last read times
// ---------------------------------------------------------------------------

/// Convert a DecodedMessage + raw content bytes to an FfiEnrichedMessage.
pub(crate) fn decoded_to_enriched(
    msg: &xmtp_mls::messages::decoded_message::DecodedMessage,
    raw_content: &[u8],
) -> FfiEnrichedMessage {
    let ct = &msg.metadata.content_type;
    let ct_str = format!(
        "{}/{}:{}.{}",
        ct.authority_id, ct.type_id, ct.version_major, ct.version_minor
    );
    let (content_bytes, content_bytes_len) = if raw_content.is_empty() {
        (std::ptr::null_mut(), 0)
    } else {
        let b = raw_content.to_vec().into_boxed_slice();
        let len = b.len() as i32;
        (Box::into_raw(b) as *mut u8, len)
    };
    FfiEnrichedMessage {
        id: to_c_string(&hex::encode(&msg.metadata.id)),
        group_id: to_c_string(&hex::encode(&msg.metadata.group_id)),
        sender_inbox_id: to_c_string(&msg.metadata.sender_inbox_id),
        sender_installation_id: to_c_string(&hex::encode(&msg.metadata.sender_installation_id)),
        sent_at_ns: msg.metadata.sent_at_ns,
        inserted_at_ns: msg.metadata.inserted_at_ns,
        kind: match msg.metadata.kind {
            xmtp_db::group_message::GroupMessageKind::Application => FfiMessageKind::Application,
            xmtp_db::group_message::GroupMessageKind::MembershipChange => {
                FfiMessageKind::MembershipChange
            }
        },
        delivery_status: match msg.metadata.delivery_status {
            xmtp_db::group_message::DeliveryStatus::Unpublished => FfiDeliveryStatus::Unpublished,
            xmtp_db::group_message::DeliveryStatus::Published => FfiDeliveryStatus::Published,
            xmtp_db::group_message::DeliveryStatus::Failed => FfiDeliveryStatus::Failed,
        },
        content_type: to_c_string(&ct_str),
        fallback_text: match &msg.fallback_text {
            Some(t) => to_c_string(t),
            None => std::ptr::null_mut(),
        },
        expires_at_ns: msg.metadata.expires_at_ns.unwrap_or(0),
        num_reactions: msg.reactions.len() as i32,
        num_replies: msg.num_replies as i32,
        content_bytes,
        content_bytes_len,
    }
}

/// List enriched (decoded) messages for a conversation.
/// Fetches both enriched metadata and raw content bytes in a single call.
/// Caller must free with [`xmtp_enriched_message_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_enriched_messages(
    conversation: *const FfiConversation,
    opts: *const FfiListMessagesOptions,
    out: *mut *mut FfiEnrichedMessageList,
) -> i32 {
    catch(|| {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let args = parse_msg_query_args(opts);
        let raw = conv.inner.find_messages(&args)?;
        let enriched = conv.inner.find_messages_v2(&args)?;
        let items: Vec<FfiEnrichedMessage> = enriched
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let bytes = raw
                    .get(i)
                    .map(|r| r.decrypted_message_bytes.as_slice())
                    .unwrap_or(&[]);
                decoded_to_enriched(e, bytes)
            })
            .collect();
        unsafe { write_out(out, FfiEnrichedMessageList { items })? };
        Ok(())
    })
}

ffi_list_len!(xmtp_enriched_message_list_len, FfiEnrichedMessageList);
ffi_list_get!(
    xmtp_enriched_message_list_get,
    FfiEnrichedMessageList,
    FfiEnrichedMessage
);

/// Free an enriched message list (including owned strings and content bytes).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_enriched_message_list_free(list: *mut FfiEnrichedMessageList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for item in &l.items {
        free_c_strings!(
            item,
            id,
            group_id,
            sender_inbox_id,
            sender_installation_id,
            content_type,
            fallback_text
        );
        if !item.content_bytes.is_null() && item.content_bytes_len > 0 {
            drop(unsafe {
                Vec::from_raw_parts(
                    item.content_bytes,
                    item.content_bytes_len as usize,
                    item.content_bytes_len as usize,
                )
            });
        }
    }
}

/// Get per-inbox last read times for a conversation.
/// Caller must free with [`xmtp_last_read_time_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_last_read_times(
    conversation: *const FfiConversation,
    out: *mut *mut FfiLastReadTimeList,
) -> i32 {
    catch(|| {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let times = conv.inner.get_last_read_times()?;
        let items: Vec<FfiLastReadTimeEntry> = times
            .into_iter()
            .map(|(inbox_id, ts)| FfiLastReadTimeEntry {
                inbox_id: to_c_string(&inbox_id),
                timestamp_ns: ts,
            })
            .collect();
        unsafe { write_out(out, FfiLastReadTimeList { items })? };
        Ok(())
    })
}

ffi_list_len!(xmtp_last_read_time_list_len, FfiLastReadTimeList);
ffi_list_get!(
    xmtp_last_read_time_list_get,
    FfiLastReadTimeList,
    FfiLastReadTimeEntry
);

ffi_list_free!(
    xmtp_last_read_time_list_free,
    FfiLastReadTimeList,
    [inbox_id]
);

/// Free an HMAC key map (including all owned data).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_hmac_key_map_free(map: *mut FfiHmacKeyMap) {
    if map.is_null() {
        return;
    }
    let m = unsafe { Box::from_raw(map) };
    for entry in &m.entries {
        if !entry.group_id.is_null() {
            drop(unsafe { CString::from_raw(entry.group_id) });
        }
        if !entry.keys.is_null() && entry.keys_count > 0 {
            let keys = unsafe {
                Vec::from_raw_parts(
                    entry.keys,
                    entry.keys_count as usize,
                    entry.keys_count as usize,
                )
            };
            for k in &keys {
                if !k.key.is_null() && k.key_len > 0 {
                    drop(unsafe {
                        Vec::from_raw_parts(k.key, k.key_len as usize, k.key_len as usize)
                    });
                }
            }
        }
    }
}
