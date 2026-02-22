//! Single conversation operations: send, messages, members, metadata, permissions, consent.

use std::ffi::c_char;

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

        let messages = c.inner.find_messages(&args)?;
        unsafe { write_out(out, XmtpMessageList { items: messages })? };
        Ok(())
    })
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
                XmtpGroupMember {
                    inbox_id: to_c_string(&m.inbox_id),
                    permission_level: match m.permission_level {
                        PermissionLevel::Member => 0,
                        PermissionLevel::Admin => 1,
                        PermissionLevel::SuperAdmin => 2,
                    },
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

/// Free a group member list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_group_member_list_free(list: *mut XmtpGroupMemberList) {
    if !list.is_null() {
        let l = unsafe { Box::from_raw(list) };
        for m in &l.members {
            if !m.inbox_id.is_null() {
                drop(unsafe { CString::from_raw(m.inbox_id) });
            }
        }
    }
}

use std::ffi::CStr;
use std::ffi::CString;

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
            *out_state = match state {
                xmtp_db::consent_record::ConsentState::Unknown => 0,
                xmtp_db::consent_record::ConsentState::Allowed => 1,
                xmtp_db::consent_record::ConsentState::Denied => 2,
            };
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
        let consent_state = match state {
            0 => xmtp_db::consent_record::ConsentState::Unknown,
            1 => xmtp_db::consent_record::ConsentState::Allowed,
            2 => xmtp_db::consent_record::ConsentState::Denied,
            _ => return Err("invalid consent state".into()),
        };
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
// Free string arrays
// ---------------------------------------------------------------------------

/// Free a string array returned by `xmtp_conversation_list_admins` etc.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_free_string_array(arr: *mut *mut c_char, count: i32) {
    if arr.is_null() || count <= 0 {
        return;
    }
    for i in 0..count as usize {
        let s = unsafe { *arr.add(i) };
        if !s.is_null() {
            drop(unsafe { CString::from_raw(s) });
        }
    }
    drop(unsafe { Vec::from_raw_parts(arr, count as usize, count as usize) });
}
