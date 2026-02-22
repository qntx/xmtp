//! Single conversation operations: send, messages, members, metadata, permissions, consent.

use std::ffi::{CStr, CString, c_char};

use xmtp_db::group::DmIdExt;
use xmtp_mls::groups::UpdateAdminListType;

use crate::ffi::*;

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Free a conversation handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_free(conv: *mut XmtpConversation) {
    if !conv.is_null() {
        drop(unsafe { Box::from_raw(conv) });
    }
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Get the conversation's hex-encoded group ID. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_id(conv: *const XmtpConversation) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => to_c_string(&hex::encode(&c.inner.group_id)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the conversation created-at timestamp in nanoseconds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_created_at_ns(conv: *const XmtpConversation) -> i64 {
    match unsafe { ref_from(conv) } {
        Ok(c) => c.inner.created_at_ns,
        Err(_) => 0,
    }
}

/// Get the conversation type: 0=DM, 1=Group, 2=Sync, 3=Oneshot, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_type(conv: *const XmtpConversation) -> i32 {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.conversation_type {
            xmtp_db::group::ConversationType::Dm => 0,
            xmtp_db::group::ConversationType::Group => 1,
            xmtp_db::group::ConversationType::Sync => 2,
            xmtp_db::group::ConversationType::Oneshot => 3,
        },
        Err(_) => -1,
    }
}

/// Get the DM peer's inbox ID. Caller must free with [`xmtp_free_string`].
/// Returns null if not a DM or on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_dm_peer_inbox_id(
    conv: *const XmtpConversation,
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
pub unsafe extern "C" fn xmtp_conversation_sync(conv: *const XmtpConversation) -> i32 {
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
    conv: *const XmtpConversation,
    content_bytes: *const u8,
    content_len: i32,
    opts: *const XmtpSendOpts,
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
    conv: *const XmtpConversation,
    content_bytes: *const u8,
    content_len: i32,
    opts: *const XmtpSendOpts,
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
pub unsafe extern "C" fn xmtp_conversation_publish_messages(conv: *const XmtpConversation) -> i32 {
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
    conv: *const XmtpConversation,
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
    conv: *const XmtpConversation,
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
pub struct XmtpListMessagesOptions {
    /// Only messages sent after this timestamp (ns). 0 = no filter.
    pub sent_after_ns: i64,
    /// Only messages sent before this timestamp (ns). 0 = no filter.
    pub sent_before_ns: i64,
    /// Maximum number of messages. 0 = no limit.
    pub limit: i64,
    /// Filter by delivery status: -1 = all, 0 = Unpublished, 1 = Published, 2 = Failed.
    pub delivery_status: i32,
    /// Filter by message kind: -1 = all, 0 = Application, 1 = MembershipChange.
    pub kind: i32,
}

/// Parse message query options from C struct into `MsgQueryArgs`.
fn parse_msg_query_args(
    opts: *const XmtpListMessagesOptions,
) -> xmtp_db::group_message::MsgQueryArgs {
    let mut args = xmtp_db::group_message::MsgQueryArgs::default();
    if !opts.is_null() {
        let o = unsafe { &*opts };
        if o.sent_after_ns > 0 {
            args.sent_after_ns = Some(o.sent_after_ns);
        }
        if o.sent_before_ns > 0 {
            args.sent_before_ns = Some(o.sent_before_ns);
        }
        if o.limit > 0 {
            args.limit = Some(o.limit);
        }
        args.delivery_status = match o.delivery_status {
            0 => Some(xmtp_db::group_message::DeliveryStatus::Unpublished),
            1 => Some(xmtp_db::group_message::DeliveryStatus::Published),
            2 => Some(xmtp_db::group_message::DeliveryStatus::Failed),
            _ => None,
        };
        args.kind = match o.kind {
            0 => Some(xmtp_db::group_message::GroupMessageKind::Application),
            1 => Some(xmtp_db::group_message::GroupMessageKind::MembershipChange),
            _ => None,
        };
    }
    args
}

/// List messages in this conversation. Caller must free with [`xmtp_message_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_messages(
    conv: *const XmtpConversation,
    opts: *const XmtpListMessagesOptions,
    out: *mut *mut XmtpMessageList,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let args = parse_msg_query_args(opts);
        let messages = c.inner.find_messages(&args)?;
        unsafe { write_out(out, XmtpMessageList { items: messages })? };
        Ok(())
    })
}

/// Count messages matching the given filter options.
/// Pass null for `opts` to count all messages.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_count_messages(
    conv: *const XmtpConversation,
    opts: *const XmtpListMessagesOptions,
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

/// Get the number of messages in a list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_list_len(list: *const XmtpMessageList) -> i32 {
    match unsafe { ref_from(list) } {
        Ok(l) => l.items.len() as i32,
        Err(_) => 0,
    }
}

/// Helper to safely access a message at index.
unsafe fn msg_at(
    list: *const XmtpMessageList,
    idx: i32,
) -> Option<&'static xmtp_db::group_message::StoredGroupMessage> {
    let l = unsafe { ref_from(list).ok()? };
    l.items.get(idx as usize)
}

/// Get message ID (hex) at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_id(list: *const XmtpMessageList, index: i32) -> *mut c_char {
    match unsafe { msg_at(list, index) } {
        Some(m) => to_c_string(&hex::encode(&m.id)),
        None => std::ptr::null_mut(),
    }
}

/// Get sender inbox ID at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_sender_inbox_id(
    list: *const XmtpMessageList,
    index: i32,
) -> *mut c_char {
    match unsafe { msg_at(list, index) } {
        Some(m) => to_c_string(&m.sender_inbox_id),
        None => std::ptr::null_mut(),
    }
}

/// Get sent-at timestamp (ns) at index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_sent_at_ns(list: *const XmtpMessageList, index: i32) -> i64 {
    unsafe { msg_at(list, index) }.map_or(0, |m| m.sent_at_ns)
}

/// Get message kind at index: 0=Application, 1=MembershipChange, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_kind(list: *const XmtpMessageList, index: i32) -> i32 {
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
    list: *const XmtpMessageList,
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
    list: *const XmtpMessageList,
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

/// Free a message list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_list_free(list: *mut XmtpMessageList) {
    if !list.is_null() {
        drop(unsafe { Box::from_raw(list) });
    }
}

/// Free a single message.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_message_free(msg: *mut XmtpMessage) {
    if !msg.is_null() {
        drop(unsafe { Box::from_raw(msg) });
    }
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

/// List group members. Caller must free with [`xmtp_group_member_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_members(
    conv: *const XmtpConversation,
    out: *mut *mut XmtpGroupMemberList,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let members_raw = c.inner.members().await?;
        let members: Vec<XmtpGroupMember> = members_raw
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
                XmtpGroupMember {
                    inbox_id: to_c_string(&m.inbox_id),
                    permission_level: match m.permission_level {
                        PermissionLevel::Member => 0,
                        PermissionLevel::Admin => 1,
                        PermissionLevel::SuperAdmin => 2,
                    },
                    consent_state: consent_state_to_i32(m.consent_state),
                    account_identifiers: ident_ptrs,
                    account_identifiers_count: ident_count,
                    installation_ids: inst_ptrs,
                    installation_ids_count: inst_count,
                }
            })
            .collect();
        unsafe { write_out(out, XmtpGroupMemberList { members })? };
        Ok(())
    })
}

/// Get number of members in a list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_list_len(list: *const XmtpGroupMemberList) -> i32 {
    match unsafe { ref_from(list) } {
        Ok(l) => l.members.len() as i32,
        Err(_) => 0,
    }
}

/// Get member inbox ID at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_inbox_id(
    list: *const XmtpGroupMemberList,
    index: i32,
) -> *mut c_char {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return std::ptr::null_mut(),
    };
    match l.members.get(index as usize) {
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
    list: *const XmtpGroupMemberList,
    index: i32,
) -> i32 {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return -1,
    };
    l.members
        .get(index as usize)
        .map_or(-1, |m| m.permission_level)
}

/// Get member consent state at index: 0=Unknown, 1=Allowed, 2=Denied, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_consent_state(
    list: *const XmtpGroupMemberList,
    index: i32,
) -> i32 {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return -1,
    };
    l.members
        .get(index as usize)
        .map_or(-1, |m| m.consent_state)
}

/// Get member account identifiers at index.
/// Returns a borrowed pointer to the internal string array. Do NOT free individual strings.
/// Use `out_count` to get the number of identifiers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_account_identifiers(
    list: *const XmtpGroupMemberList,
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
    match l.members.get(index as usize) {
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
    list: *const XmtpGroupMemberList,
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
    match l.members.get(index as usize) {
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

/// Free a group member list (including all owned strings).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_list_free(list: *mut XmtpGroupMemberList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for m in &l.members {
        if !m.inbox_id.is_null() {
            drop(unsafe { CString::from_raw(m.inbox_id) });
        }
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
    conv: *const XmtpConversation,
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
    conv: *const XmtpConversation,
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
pub unsafe extern "C" fn xmtp_conversation_leave(conv: *const XmtpConversation) -> i32 {
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
    conv: *const XmtpConversation,
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
// Metadata
// ---------------------------------------------------------------------------

/// Get group name. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_group_name(
    conv: *const XmtpConversation,
) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.group_name() {
            Ok(name) => to_c_string(&name),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Update group name.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_group_name(
    conv: *const XmtpConversation,
    name: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let name = unsafe { c_str_to_string(name)? };
        c.inner.update_group_name(name).await?;
        Ok(())
    })
}

/// Get group description. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_group_description(
    conv: *const XmtpConversation,
) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.group_description() {
            Ok(desc) => to_c_string(&desc),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Update group description.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_group_description(
    conv: *const XmtpConversation,
    description: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let desc = unsafe { c_str_to_string(description)? };
        c.inner.update_group_description(desc).await?;
        Ok(())
    })
}

/// Get group image URL. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_group_image_url(
    conv: *const XmtpConversation,
) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.group_image_url_square() {
            Ok(url) => to_c_string(&url),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Update group image URL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_group_image_url(
    conv: *const XmtpConversation,
    url: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let url = unsafe { c_str_to_string(url)? };
        c.inner.update_group_image_url_square(url).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Consent
// ---------------------------------------------------------------------------

/// Get conversation consent state. Writes to `out_state` (0=Unknown, 1=Allowed, 2=Denied).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_consent_state(
    conv: *const XmtpConversation,
    out_state: *mut i32,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out_state.is_null() {
            return Err("null output pointer".into());
        }
        let state = c.inner.consent_state()?;
        unsafe {
            *out_state = consent_state_to_i32(state);
        }
        Ok(())
    })
}

/// Update conversation consent state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_consent_state(
    conv: *const XmtpConversation,
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
    conv: *const XmtpConversation,
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
pub unsafe extern "C" fn xmtp_conversation_is_active(conv: *const XmtpConversation) -> i32 {
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
pub unsafe extern "C" fn xmtp_conversation_membership_state(conv: *const XmtpConversation) -> i32 {
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

/// Get the inbox ID of the member who added the current user.
/// Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_added_by_inbox_id(
    conv: *const XmtpConversation,
) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.added_by_inbox_id() {
            Ok(id) => to_c_string(&id),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

// ---------------------------------------------------------------------------
// Admin queries
// ---------------------------------------------------------------------------

/// Get the admin list as a null-terminated array of C strings.
/// `out_count` receives the number of admins.
/// Each string and the array itself must be freed by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_admins(
    conv: *const XmtpConversation,
    out_count: *mut i32,
) -> *mut *mut c_char {
    if out_count.is_null() {
        return std::ptr::null_mut();
    }
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.admin_list() {
            Ok(admins) => string_vec_to_c(admins, out_count),
            Err(_) => {
                unsafe {
                    *out_count = 0;
                }
                std::ptr::null_mut()
            }
        },
        Err(_) => {
            unsafe {
                *out_count = 0;
            }
            std::ptr::null_mut()
        }
    }
}

/// Get the super admin list. Same ownership semantics as [`xmtp_conversation_list_admins`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_super_admins(
    conv: *const XmtpConversation,
    out_count: *mut i32,
) -> *mut *mut c_char {
    if out_count.is_null() {
        return std::ptr::null_mut();
    }
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.super_admin_list() {
            Ok(admins) => string_vec_to_c(admins, out_count),
            Err(_) => {
                unsafe {
                    *out_count = 0;
                }
                std::ptr::null_mut()
            }
        },
        Err(_) => {
            unsafe {
                *out_count = 0;
            }
            std::ptr::null_mut()
        }
    }
}

/// Check if an inbox ID is an admin. Returns 1=yes, 0=no, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_is_admin(
    conv: *const XmtpConversation,
    inbox_id: *const c_char,
) -> i32 {
    let c = match unsafe { ref_from(conv) } {
        Ok(c) => c,
        Err(_) => return -1,
    };
    let id = match unsafe { c_str_to_string(inbox_id) } {
        Ok(s) => s,
        Err(_) => return -1,
    };
    match c.inner.admin_list() {
        Ok(list) => i32::from(list.contains(&id)),
        Err(_) => -1,
    }
}

/// Check if an inbox ID is a super admin. Returns 1=yes, 0=no, -1=error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_is_super_admin(
    conv: *const XmtpConversation,
    inbox_id: *const c_char,
) -> i32 {
    let c = match unsafe { ref_from(conv) } {
        Ok(c) => c,
        Err(_) => return -1,
    };
    let id = match unsafe { c_str_to_string(inbox_id) } {
        Ok(s) => s,
        Err(_) => return -1,
    };
    match c.inner.super_admin_list() {
        Ok(list) => i32::from(list.contains(&id)),
        Err(_) => -1,
    }
}

// ---------------------------------------------------------------------------
// Membership by identity (Identifier-based)
// ---------------------------------------------------------------------------

/// Add members by external identifiers (address/passkey).
/// `identifiers` and `kinds` are parallel arrays of length `count`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_add_members_by_identity(
    conv: *const XmtpConversation,
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
    conv: *const XmtpConversation,
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
pub struct XmtpDisappearingSettings {
    /// Timestamp (ns) from which messages start disappearing.
    pub from_ns: i64,
    /// Duration (ns) after which messages disappear.
    pub in_ns: i64,
}

/// Update the message disappearing settings for this conversation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_disappearing_settings(
    conv: *const XmtpConversation,
    settings: *const XmtpDisappearingSettings,
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
    conv: *const XmtpConversation,
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
    conv: *const XmtpConversation,
    out: *mut XmtpDisappearingSettings,
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
    conv: *const XmtpConversation,
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
// App data
// ---------------------------------------------------------------------------

/// Get the custom app data string. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_app_data(conv: *const XmtpConversation) -> *mut c_char {
    match unsafe { ref_from(conv) } {
        Ok(c) => match c.inner.app_data() {
            Ok(data) => to_c_string(&data),
            Err(_) => std::ptr::null_mut(),
        },
        Err(_) => std::ptr::null_mut(),
    }
}

/// Update the custom app data string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_update_app_data(
    conv: *const XmtpConversation,
    app_data: *const c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        let data = unsafe { c_str_to_string(app_data)? };
        c.inner.update_app_data(data).await?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Duplicate DMs & paused version
// ---------------------------------------------------------------------------

/// Find duplicate DM conversations for this DM.
/// Returns a conversation list. Caller must free with [`xmtp_conversation_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_duplicate_dms(
    conv: *const XmtpConversation,
    out: *mut *mut XmtpConversationList,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let dups = c.inner.find_duplicate_dms()?;
        let items: Vec<XmtpConversationListItem> = dups
            .into_iter()
            .map(|g| XmtpConversationListItem { group: g })
            .collect();
        unsafe { write_out(out, XmtpConversationList { items })? };
        Ok(())
    })
}

/// Check if the conversation is paused for a version upgrade.
/// Writes the version string to `out` if paused, or null if not paused.
/// Caller must free `out` with [`xmtp_free_string`].
/// Returns 0 on success, -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_paused_for_version(
    conv: *const XmtpConversation,
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
    conv: *const XmtpConversation,
    out: *mut XmtpConversationDebugInfo,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(conv)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let info = c.inner.debug_info().await?;
        unsafe {
            *out = XmtpConversationDebugInfo {
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
            };
        }
        Ok(())
    })
}

/// Free a conversation debug info struct (its string fields).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_debug_info_free(info: *mut XmtpConversationDebugInfo) {
    if info.is_null() {
        return;
    }
    let i = unsafe { &mut *info };
    if !i.fork_details.is_null() {
        drop(unsafe { CString::from_raw(i.fork_details) });
        i.fork_details = std::ptr::null_mut();
    }
    if !i.local_commit_log.is_null() {
        drop(unsafe { CString::from_raw(i.local_commit_log) });
        i.local_commit_log = std::ptr::null_mut();
    }
    if !i.remote_commit_log.is_null() {
        drop(unsafe { CString::from_raw(i.remote_commit_log) });
        i.remote_commit_log = std::ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// HMAC keys
// ---------------------------------------------------------------------------

/// Get HMAC keys for this conversation (including duplicate DMs).
/// Returns a map via `out`. Caller must free with [`xmtp_hmac_key_map_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_hmac_keys(
    conv: *const XmtpConversation,
    out: *mut *mut XmtpHmacKeyMap,
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

        unsafe { write_out(out, XmtpHmacKeyMap { entries })? };
        Ok(())
    })
}

/// Convert a Vec<HmacKey> into a C-compatible XmtpHmacKeyEntry.
fn hmac_keys_to_entry(
    group_id: &[u8],
    keys: Vec<xmtp_db::user_preferences::HmacKey>,
) -> XmtpHmacKeyEntry {
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
    XmtpHmacKeyEntry {
        group_id: to_c_string(&hex::encode(group_id)),
        keys: keys_ptr,
        keys_count,
    }
}

/// Get the number of entries in an HMAC key map.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_hmac_key_map_len(map: *const XmtpHmacKeyMap) -> i32 {
    match unsafe { ref_from(map) } {
        Ok(m) => m.entries.len() as i32,
        Err(_) => 0,
    }
}

/// Get the group ID (hex) at index. Returns a borrowed pointer; do NOT free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_hmac_key_map_group_id(
    map: *const XmtpHmacKeyMap,
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
    map: *const XmtpHmacKeyMap,
    index: i32,
    out_count: *mut i32,
) -> *const XmtpHmacKey {
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
            e.keys as *const XmtpHmacKey
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
    conversation: *const XmtpConversation,
    envelope_bytes: *const u8,
    envelope_bytes_len: i32,
    out: *mut *mut XmtpMessageList,
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
        let list = Box::new(XmtpMessageList { items: messages });
        unsafe { *out = Box::into_raw(list) };
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
    conversation: *const XmtpConversation,
    out: *mut *mut XmtpGroupMetadata,
) -> i32 {
    catch_async(|| async {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let metadata = conv.inner.metadata().await?;
        let conv_type = match metadata.conversation_type {
            xmtp_db::group::ConversationType::Group => 0,
            xmtp_db::group::ConversationType::Dm => 1,
            xmtp_db::group::ConversationType::Sync => 2,
            xmtp_db::group::ConversationType::Oneshot => 3,
        };
        let result = Box::new(XmtpGroupMetadata {
            creator_inbox_id: to_c_string(&metadata.creator_inbox_id),
            conversation_type: conv_type,
        });
        unsafe { *out = Box::into_raw(result) };
        Ok(())
    })
}

/// Free a group metadata struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_metadata_free(meta: *mut XmtpGroupMetadata) {
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

/// Map a MembershipPolicies to i32: 0=Allow, 1=Deny, 2=Admin, 3=SuperAdmin, 5=Other.
fn membership_policy_to_i32(p: &xmtp_mls::groups::group_permissions::MembershipPolicies) -> i32 {
    use xmtp_mls::groups::group_permissions::{BasePolicies, MembershipPolicies};
    if let MembershipPolicies::Standard(base) = p {
        match base {
            BasePolicies::Allow => 0,
            BasePolicies::Deny => 1,
            BasePolicies::AllowIfAdminOrSuperAdmin => 2,
            BasePolicies::AllowIfSuperAdmin => 3,
            BasePolicies::AllowSameMember => 5,
        }
    } else {
        5 // Other
    }
}

/// Map a MetadataPolicies to i32: 0=Allow, 1=Deny, 2=Admin, 3=SuperAdmin, 5=Other.
fn metadata_policy_to_i32(p: &xmtp_mls::groups::group_permissions::MetadataPolicies) -> i32 {
    use xmtp_mls::groups::group_permissions::{MetadataBasePolicies, MetadataPolicies};
    if let MetadataPolicies::Standard(base) = p {
        match base {
            MetadataBasePolicies::Allow => 0,
            MetadataBasePolicies::Deny => 1,
            MetadataBasePolicies::AllowIfActorAdminOrSuperAdmin => 2,
            MetadataBasePolicies::AllowIfActorSuperAdmin => 3,
        }
    } else {
        5 // Other
    }
}

/// Map a PermissionsPolicies to i32: 1=Deny, 2=Admin, 3=SuperAdmin, 5=Other.
fn permissions_policy_to_i32(p: &xmtp_mls::groups::group_permissions::PermissionsPolicies) -> i32 {
    use xmtp_mls::groups::group_permissions::{PermissionsBasePolicies, PermissionsPolicies};
    if let PermissionsPolicies::Standard(base) = p {
        match base {
            PermissionsBasePolicies::Deny => 1,
            PermissionsBasePolicies::AllowIfActorAdminOrSuperAdmin => 2,
            PermissionsBasePolicies::AllowIfActorSuperAdmin => 3,
        }
    } else {
        5 // Other
    }
}

/// Get the group permissions (policy type + full policy set).
/// Caller must free with [`xmtp_group_permissions_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_group_permissions(
    conversation: *const XmtpConversation,
    out: *mut *mut XmtpGroupPermissions,
) -> i32 {
    catch(|| {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let perms = conv.inner.permissions()?;
        let policy_type = match perms.preconfigured_policy() {
            Ok(xmtp_mls::groups::PreconfiguredPolicies::Default) => 0,
            Ok(xmtp_mls::groups::PreconfiguredPolicies::AdminsOnly) => 1,
            Err(_) => 2, // CustomPolicy
        };

        let ps = &perms.policies;
        let meta = &ps.update_metadata_policy;
        let get_meta = |field: &str| -> i32 {
            meta.get(field)
                .map(|p| metadata_policy_to_i32(p))
                .unwrap_or(4) // DoesNotExist
        };

        use xmtp_mls::mls_common::group_mutable_metadata::MetadataField;
        let policy_set = XmtpPermissionPolicySet {
            add_member_policy: membership_policy_to_i32(&ps.add_member_policy),
            remove_member_policy: membership_policy_to_i32(&ps.remove_member_policy),
            add_admin_policy: permissions_policy_to_i32(&ps.add_admin_policy),
            remove_admin_policy: permissions_policy_to_i32(&ps.remove_admin_policy),
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

        let result = Box::new(XmtpGroupPermissions {
            policy_type,
            policy_set,
        });
        unsafe { *out = Box::into_raw(result) };
        Ok(())
    })
}

/// Free a group permissions struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_permissions_free(perms: *mut XmtpGroupPermissions) {
    if !perms.is_null() {
        drop(unsafe { Box::from_raw(perms) });
    }
}

// ---------------------------------------------------------------------------
// Enriched messages + last read times
// ---------------------------------------------------------------------------

/// Convert a DecodedMessage to an XmtpEnrichedMessage.
pub(crate) fn decoded_to_enriched(
    msg: &xmtp_mls::messages::decoded_message::DecodedMessage,
) -> XmtpEnrichedMessage {
    let ct = &msg.metadata.content_type;
    let ct_str = format!(
        "{}/{}:{}.{}",
        ct.authority_id, ct.type_id, ct.version_major, ct.version_minor
    );
    XmtpEnrichedMessage {
        id: to_c_string(&hex::encode(&msg.metadata.id)),
        group_id: to_c_string(&hex::encode(&msg.metadata.group_id)),
        sender_inbox_id: to_c_string(&msg.metadata.sender_inbox_id),
        sender_installation_id: to_c_string(&hex::encode(&msg.metadata.sender_installation_id)),
        sent_at_ns: msg.metadata.sent_at_ns,
        inserted_at_ns: msg.metadata.inserted_at_ns,
        kind: msg.metadata.kind as i32,
        delivery_status: msg.metadata.delivery_status as i32,
        content_type: to_c_string(&ct_str),
        fallback_text: match &msg.fallback_text {
            Some(t) => to_c_string(t),
            None => std::ptr::null_mut(),
        },
        num_reactions: msg.reactions.len() as i32,
        num_replies: msg.num_replies as i32,
    }
}

/// List enriched (decoded) messages for a conversation.
/// Caller must free with [`xmtp_enriched_message_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_list_enriched_messages(
    conversation: *const XmtpConversation,
    opts: *const XmtpListMessagesOptions,
    out: *mut *mut XmtpEnrichedMessageList,
) -> i32 {
    catch(|| {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let args = parse_msg_query_args(opts);
        let messages = conv.inner.find_messages_v2(&args)?;
        let items: Vec<XmtpEnrichedMessage> = messages.iter().map(decoded_to_enriched).collect();
        let list = Box::new(XmtpEnrichedMessageList { items });
        unsafe { *out = Box::into_raw(list) };
        Ok(())
    })
}

/// Get the number of entries in an enriched message list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_enriched_message_list_len(
    list: *const XmtpEnrichedMessageList,
) -> i32 {
    match unsafe { ref_from(list) } {
        Ok(l) => l.items.len() as i32,
        Err(_) => 0,
    }
}

/// Get a borrowed pointer to an enriched message by index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_enriched_message_list_get(
    list: *const XmtpEnrichedMessageList,
    index: i32,
) -> *const XmtpEnrichedMessage {
    match unsafe { ref_from(list) } {
        Ok(l) => {
            let i = index as usize;
            if i < l.items.len() {
                &l.items[i] as *const _
            } else {
                std::ptr::null()
            }
        }
        Err(_) => std::ptr::null(),
    }
}

/// Free an enriched message list (including all owned strings).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_enriched_message_list_free(list: *mut XmtpEnrichedMessageList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for item in &l.items {
        for ptr in [
            item.id,
            item.group_id,
            item.sender_inbox_id,
            item.sender_installation_id,
            item.content_type,
            item.fallback_text,
        ] {
            if !ptr.is_null() {
                drop(unsafe { CString::from_raw(ptr) });
            }
        }
    }
}

/// Get per-inbox last read times for a conversation.
/// Caller must free with [`xmtp_last_read_time_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_conversation_last_read_times(
    conversation: *const XmtpConversation,
    out: *mut *mut XmtpLastReadTimeList,
) -> i32 {
    catch(|| {
        let conv = unsafe { ref_from(conversation)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let times = conv.inner.get_last_read_times()?;
        let items: Vec<XmtpLastReadTimeEntry> = times
            .into_iter()
            .map(|(inbox_id, ts)| XmtpLastReadTimeEntry {
                inbox_id: to_c_string(&inbox_id),
                timestamp_ns: ts,
            })
            .collect();
        let list = Box::new(XmtpLastReadTimeList { items });
        unsafe { *out = Box::into_raw(list) };
        Ok(())
    })
}

/// Get the number of entries in a last-read-time list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_last_read_time_list_len(list: *const XmtpLastReadTimeList) -> i32 {
    match unsafe { ref_from(list) } {
        Ok(l) => l.items.len() as i32,
        Err(_) => 0,
    }
}

/// Get a borrowed pointer to a last-read-time entry by index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_last_read_time_list_get(
    list: *const XmtpLastReadTimeList,
    index: i32,
    out_inbox_id: *mut *const c_char,
    out_timestamp_ns: *mut i64,
) -> i32 {
    catch(|| {
        let l = unsafe { ref_from(list)? };
        let i = index as usize;
        if i >= l.items.len() {
            return Err("index out of bounds".into());
        }
        if !out_inbox_id.is_null() {
            unsafe { *out_inbox_id = l.items[i].inbox_id };
        }
        if !out_timestamp_ns.is_null() {
            unsafe { *out_timestamp_ns = l.items[i].timestamp_ns };
        }
        Ok(())
    })
}

/// Free a last-read-time list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_last_read_time_list_free(list: *mut XmtpLastReadTimeList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for item in &l.items {
        if !item.inbox_id.is_null() {
            drop(unsafe { CString::from_raw(item.inbox_id) });
        }
    }
}

/// Free an HMAC key map (including all owned data).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_hmac_key_map_free(map: *mut XmtpHmacKeyMap) {
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
