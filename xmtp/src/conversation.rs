#![allow(unsafe_code)]
//! Conversation operations: send, messages, members, metadata, consent,
//! disappearing messages, admin management, permissions, debug info, and HMAC keys.

use std::ffi::{CStr, c_char};
use std::ptr;

use crate::error::{self, Result};
use crate::ffi::{
    OwnedHandle, identifiers_to_ffi, read_borrowed_strings, take_c_string, take_nullable_string,
    to_c_string, to_c_string_array,
};
use crate::types::{
    AccountIdentifier, ConsentState, ConversationDebugInfo, ConversationMetadata, ConversationType,
    Cursor, DeliveryStatus, DisappearingSettings, GroupPermissionsPreset, HmacKey, HmacKeyEntry,
    LastReadTime, ListMessagesOptions, MembershipState, MessageKind, PermissionLevel,
    PermissionPolicy, PermissionPolicySet, PermissionUpdateType, Permissions, SendOptions,
};

/// Generate a nullable-string getter method on `Conversation`.
macro_rules! metadata_getter {
    ($(#[$m:meta])* $name:ident, $ffi_fn:path) => {
        $(#[$m])*
        pub fn $name(&self) -> Option<String> {
            unsafe { take_nullable_string($ffi_fn(self.handle.as_ptr())) }
        }
    };
}

/// Generate a string setter method on `Conversation`.
macro_rules! metadata_setter {
    ($(#[$m:meta])* $name:ident, $ffi_fn:path) => {
        $(#[$m])*
        pub fn $name(&self, value: &str) -> Result<()> {
            let c = to_c_string(value)?;
            error::check(unsafe { $ffi_fn(self.handle.as_ptr(), c.as_ptr()) })
        }
    };
}

/// An enriched message from a conversation.
#[derive(Debug, Clone)]
pub struct Message {
    /// Hex-encoded message ID.
    pub id: String,
    /// Hex-encoded group/conversation ID.
    pub conversation_id: String,
    /// Sender's inbox ID.
    pub sender_inbox_id: String,
    /// Sender's installation ID (hex).
    pub sender_installation_id: String,
    /// Sent timestamp in nanoseconds.
    pub sent_at_ns: i64,
    /// Inserted-into-DB timestamp in nanoseconds.
    pub inserted_at_ns: i64,
    /// Message kind.
    pub kind: MessageKind,
    /// Delivery status.
    pub delivery_status: DeliveryStatus,
    /// Content type ID (e.g. `"xmtp.org/text:1.0"`).
    pub content_type: Option<String>,
    /// Fallback text for unsupported content types.
    pub fallback: Option<String>,
    /// Raw decrypted content bytes (protobuf-encoded `EncodedContent`).
    pub content: Vec<u8>,
    /// Expiration timestamp in nanoseconds (0 = no expiration).
    pub expires_at_ns: i64,
    /// Number of reactions to this message.
    pub num_reactions: i32,
    /// Number of replies to this message.
    pub num_replies: i32,
}

/// A member of a group conversation.
#[derive(Debug, Clone)]
pub struct GroupMember {
    /// Member's inbox ID.
    pub inbox_id: String,
    /// Permission level within the group.
    pub permission_level: PermissionLevel,
    /// Consent state.
    pub consent_state: ConsentState,
    /// Associated account identifiers (addresses).
    pub account_identifiers: Vec<String>,
    /// Installation IDs (hex).
    pub installation_ids: Vec<String>,
}

/// A conversation handle (DM or group).
#[derive(Debug)]
pub struct Conversation {
    handle: OwnedHandle<xmtp_sys::XmtpFfiConversation>,
}

impl Conversation {
    /// Wrap a raw FFI conversation pointer. Takes ownership.
    pub(crate) fn from_raw(ptr: *mut xmtp_sys::XmtpFfiConversation) -> Result<Self> {
        OwnedHandle::new(ptr, xmtp_sys::xmtp_conversation_free).map(|h| Self { handle: h })
    }

    /// Raw const pointer for the stream module.
    pub(crate) const fn handle_ptr(&self) -> *const xmtp_sys::XmtpFfiConversation {
        self.handle.as_ptr()
    }

    /// Hex-encoded group ID.
    pub fn id(&self) -> Result<String> {
        unsafe { take_c_string(xmtp_sys::xmtp_conversation_id(self.handle.as_ptr())) }
    }

    /// Conversation type (DM, Group, etc.).
    #[must_use]
    pub fn conversation_type(&self) -> Option<ConversationType> {
        ConversationType::from_ffi(unsafe {
            xmtp_sys::xmtp_conversation_type(self.handle.as_ptr())
        })
    }

    /// Created-at timestamp in nanoseconds.
    #[must_use]
    pub fn created_at_ns(&self) -> i64 {
        unsafe { xmtp_sys::xmtp_conversation_created_at_ns(self.handle.as_ptr()) }
    }

    /// Whether the conversation is active.
    #[must_use]
    pub fn is_active(&self) -> bool {
        unsafe { xmtp_sys::xmtp_conversation_is_active(self.handle.as_ptr()) == 1 }
    }

    /// Current membership state of this client in the conversation.
    #[must_use]
    pub fn membership_state(&self) -> Option<MembershipState> {
        MembershipState::from_ffi(unsafe {
            xmtp_sys::xmtp_conversation_membership_state(self.handle.as_ptr())
        })
    }

    metadata_getter!(/// DM peer's inbox ID. Returns `None` if not a DM.
        dm_peer_inbox_id, xmtp_sys::xmtp_conversation_dm_peer_inbox_id);
    metadata_getter!(/// Inbox ID of the member who added this client.
        added_by_inbox_id, xmtp_sys::xmtp_conversation_added_by_inbox_id);
    metadata_getter!(/// Get the group name.
        name, xmtp_sys::xmtp_conversation_group_name);
    metadata_setter!(/// Set the group name.
        set_name, xmtp_sys::xmtp_conversation_update_group_name);
    metadata_getter!(/// Get the group description.
        description, xmtp_sys::xmtp_conversation_group_description);
    metadata_setter!(/// Set the group description.
        set_description, xmtp_sys::xmtp_conversation_update_group_description);
    metadata_getter!(/// Get the group image URL.
        image_url, xmtp_sys::xmtp_conversation_group_image_url);
    metadata_setter!(/// Set the group image URL.
        set_image_url, xmtp_sys::xmtp_conversation_update_group_image_url);
    metadata_getter!(/// Get app data string.
        app_data, xmtp_sys::xmtp_conversation_app_data);
    metadata_setter!(/// Set app data (max 8192 bytes).
        set_app_data, xmtp_sys::xmtp_conversation_update_app_data);

    /// Check if conversation is paused for a version upgrade.
    pub fn paused_for_version(&self) -> Result<Option<String>> {
        let mut out: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_paused_for_version(self.handle.as_ptr(), &raw mut out)
        };
        error::check(rc)?;
        if out.is_null() {
            Ok(None)
        } else {
            unsafe { take_c_string(out) }.map(Some)
        }
    }

    /// Get the conversation metadata (creator inbox ID + type).
    pub fn metadata(&self) -> Result<ConversationMetadata> {
        let mut out: *mut xmtp_sys::XmtpFfiGroupMetadata = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_group_metadata(self.handle.as_ptr(), &raw mut out)
        };
        error::check(rc)?;
        if out.is_null() {
            return Err(crate::Error::NullPointer);
        }
        let meta = unsafe { &*out };
        let creator = unsafe { take_c_string(meta.creator_inbox_id) }.unwrap_or_default();
        let conv_type = ConversationType::from_ffi(meta.conversation_type as i32)
            .unwrap_or(ConversationType::Group);
        unsafe { xmtp_sys::xmtp_group_metadata_free(out) };
        Ok(ConversationMetadata {
            creator_inbox_id: creator,
            conversation_type: conv_type,
        })
    }

    /// Get the group permissions (preset + full policy set).
    pub fn permissions(&self) -> Result<Permissions> {
        let mut out: *mut xmtp_sys::XmtpFfiGroupPermissions = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_group_permissions(self.handle.as_ptr(), &raw mut out)
        };
        error::check(rc)?;
        if out.is_null() {
            return Err(crate::Error::NullPointer);
        }
        let perms = unsafe { &*out };
        let result = read_permissions(perms);
        unsafe { xmtp_sys::xmtp_group_permissions_free(out) };
        Ok(result)
    }

    /// Sync this conversation with the network.
    pub fn sync(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_conversation_sync(self.handle.as_ptr()) })
    }

    /// Send raw encoded content bytes. Returns the hex-encoded message ID.
    pub fn send(&self, content: &[u8]) -> Result<String> {
        self.send_inner(content, ptr::null(), xmtp_sys::xmtp_conversation_send)
    }

    /// Send with options. Returns the hex-encoded message ID.
    pub fn send_with(&self, content: &[u8], opts: &SendOptions) -> Result<String> {
        let ffi = send_opts_to_ffi(*opts);
        self.send_inner(content, &raw const ffi, xmtp_sys::xmtp_conversation_send)
    }

    /// Send optimistically (returns immediately, publishes in background).
    pub fn send_optimistic(&self, content: &[u8]) -> Result<String> {
        self.send_inner(
            content,
            ptr::null(),
            xmtp_sys::xmtp_conversation_send_optimistic,
        )
    }

    /// Send optimistically with options.
    pub fn send_optimistic_with(&self, content: &[u8], opts: &SendOptions) -> Result<String> {
        let ffi = send_opts_to_ffi(*opts);
        self.send_inner(
            content,
            &raw const ffi,
            xmtp_sys::xmtp_conversation_send_optimistic,
        )
    }

    /// Shared send implementation.
    fn send_inner(
        &self,
        content: &[u8],
        opts: *const xmtp_sys::XmtpFfiSendOpts,
        ffi_fn: unsafe extern "C" fn(
            *const xmtp_sys::XmtpFfiConversation,
            *const u8,
            i32,
            *const xmtp_sys::XmtpFfiSendOpts,
            *mut *mut c_char,
        ) -> i32,
    ) -> Result<String> {
        let mut out: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            ffi_fn(
                self.handle.as_ptr(),
                content.as_ptr(),
                content.len() as i32,
                opts,
                &raw mut out,
            )
        };
        error::check(rc)?;
        unsafe { take_c_string(out) }
    }

    /// Publish all queued (unpublished) messages.
    pub fn publish_messages(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_conversation_publish_messages(self.handle.as_ptr()) })
    }

    /// List all messages with default options.
    pub fn messages(&self) -> Result<Vec<Message>> {
        self.list_messages(&ListMessagesOptions::default())
    }

    /// List messages with filtering options.
    pub fn list_messages(&self, options: &ListMessagesOptions) -> Result<Vec<Message>> {
        let ffi_opts = msg_opts_to_ffi(options);
        let mut list: *mut xmtp_sys::XmtpFfiEnrichedMessageList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_list_enriched_messages(
                self.handle.as_ptr(),
                &raw const ffi_opts,
                &raw mut list,
            )
        };
        error::check(rc)?;
        if list.is_null() {
            return Ok(vec![]);
        }
        let result = read_enriched_message_list(list);
        unsafe { xmtp_sys::xmtp_enriched_message_list_free(list) };
        Ok(result)
    }

    /// Count messages matching filter options.
    #[must_use]
    pub fn count_messages(&self, options: &ListMessagesOptions) -> i64 {
        let ffi_opts = msg_opts_to_ffi(options);
        unsafe {
            xmtp_sys::xmtp_conversation_count_messages(self.handle.as_ptr(), &raw const ffi_opts)
        }
    }

    /// List members of this conversation.
    pub fn members(&self) -> Result<Vec<GroupMember>> {
        let mut list: *mut xmtp_sys::XmtpFfiGroupMemberList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_list_members(self.handle.as_ptr(), &raw mut list)
        };
        error::check(rc)?;
        if list.is_null() {
            return Ok(vec![]);
        }
        let result = read_member_list(list);
        unsafe { xmtp_sys::xmtp_group_member_list_free(list) };
        result
    }

    /// Add members by inbox IDs.
    pub fn add_members(&self, inbox_ids: &[&str]) -> Result<()> {
        let (_owned, ptrs) = to_c_string_array(inbox_ids)?;
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_add_members(
                self.handle.as_ptr(),
                ptrs.as_ptr(),
                ptrs.len() as i32,
            )
        })
    }

    /// Remove members by inbox IDs.
    pub fn remove_members(&self, inbox_ids: &[&str]) -> Result<()> {
        let (_owned, ptrs) = to_c_string_array(inbox_ids)?;
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_remove_members(
                self.handle.as_ptr(),
                ptrs.as_ptr(),
                ptrs.len() as i32,
            )
        })
    }

    /// Add members by external identifiers (address + kind).
    pub fn add_members_by_identity(&self, identifiers: &[AccountIdentifier]) -> Result<()> {
        let (_owned, ptrs, kinds) = identifiers_to_ffi(identifiers)?;
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_add_members_by_identity(
                self.handle.as_ptr(),
                ptrs.as_ptr(),
                kinds.as_ptr(),
                ptrs.len() as i32,
            )
        })
    }

    /// Remove members by external identifiers (address + kind).
    pub fn remove_members_by_identity(&self, identifiers: &[AccountIdentifier]) -> Result<()> {
        let (_owned, ptrs, kinds) = identifiers_to_ffi(identifiers)?;
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_remove_members_by_identity(
                self.handle.as_ptr(),
                ptrs.as_ptr(),
                kinds.as_ptr(),
                ptrs.len() as i32,
            )
        })
    }

    /// Leave this group conversation.
    pub fn leave(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_conversation_leave(self.handle.as_ptr()) })
    }

    /// Get the consent state for this conversation.
    pub fn consent_state(&self) -> Result<ConsentState> {
        let mut out = 0i32;
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_consent_state(self.handle.as_ptr(), &raw mut out)
        };
        error::check(rc)?;
        ConsentState::from_ffi(out)
            .ok_or_else(|| crate::Error::Ffi(format!("unknown consent state: {out}")))
    }

    /// Set the consent state for this conversation.
    pub fn set_consent(&self, state: ConsentState) -> Result<()> {
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_update_consent_state(self.handle.as_ptr(), state as i32)
        })
    }

    /// Get the current disappearing message settings.
    #[must_use]
    pub fn disappearing_settings(&self) -> Option<DisappearingSettings> {
        let mut out = xmtp_sys::XmtpFfiDisappearingSettings::default();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_disappearing_settings(self.handle.as_ptr(), &raw mut out)
        };
        if rc == 0 {
            Some(DisappearingSettings {
                from_ns: out.from_ns,
                in_ns: out.in_ns,
            })
        } else {
            None
        }
    }

    /// Set disappearing message settings.
    pub fn set_disappearing(&self, settings: DisappearingSettings) -> Result<()> {
        let ffi = xmtp_sys::XmtpFfiDisappearingSettings {
            from_ns: settings.from_ns,
            in_ns: settings.in_ns,
        };
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_update_disappearing_settings(
                self.handle.as_ptr(),
                &raw const ffi,
            )
        })
    }

    /// Clear disappearing message settings.
    pub fn clear_disappearing(&self) -> Result<()> {
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_remove_disappearing_settings(self.handle.as_ptr())
        })
    }

    /// Whether disappearing messages are enabled.
    #[must_use]
    pub fn is_disappearing_enabled(&self) -> bool {
        unsafe { xmtp_sys::xmtp_conversation_is_disappearing_enabled(self.handle.as_ptr()) == 1 }
    }

    /// Set a permission policy on this conversation.
    pub fn set_permission_policy(
        &self,
        update_type: PermissionUpdateType,
        policy: PermissionPolicy,
        metadata_field: Option<&str>,
    ) -> Result<()> {
        let c_field = metadata_field.map(to_c_string).transpose()?;
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_update_permission_policy(
                self.handle.as_ptr(),
                update_type as i32,
                policy as i32,
                c_field.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
            )
        })
    }

    /// Add an admin.
    pub fn add_admin(&self, inbox_id: &str) -> Result<()> {
        self.update_admin_list(inbox_id, 0)
    }

    /// Remove an admin.
    pub fn remove_admin(&self, inbox_id: &str) -> Result<()> {
        self.update_admin_list(inbox_id, 1)
    }

    /// Add a super admin.
    pub fn add_super_admin(&self, inbox_id: &str) -> Result<()> {
        self.update_admin_list(inbox_id, 2)
    }

    /// Remove a super admin.
    pub fn remove_super_admin(&self, inbox_id: &str) -> Result<()> {
        self.update_admin_list(inbox_id, 3)
    }

    /// Low-level admin list update.
    fn update_admin_list(&self, inbox_id: &str, action: i32) -> Result<()> {
        let c = to_c_string(inbox_id)?;
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_update_admin_list(self.handle.as_ptr(), c.as_ptr(), action)
        })
    }

    /// Admin inbox IDs.
    #[must_use]
    pub fn admins(&self) -> Vec<String> {
        let mut count = 0i32;
        let ptr = unsafe {
            xmtp_sys::xmtp_conversation_list_admins(self.handle.as_ptr(), &raw mut count)
        };
        unsafe { read_borrowed_strings(ptr.cast_const(), count) }
    }

    /// Super admin inbox IDs.
    #[must_use]
    pub fn super_admins(&self) -> Vec<String> {
        let mut count = 0i32;
        let ptr = unsafe {
            xmtp_sys::xmtp_conversation_list_super_admins(self.handle.as_ptr(), &raw mut count)
        };
        unsafe { read_borrowed_strings(ptr.cast_const(), count) }
    }

    /// Check if the given inbox ID is an admin.
    #[must_use]
    pub fn is_admin(&self, inbox_id: &str) -> bool {
        to_c_string(inbox_id).is_ok_and(|c| unsafe {
            xmtp_sys::xmtp_conversation_is_admin(self.handle.as_ptr(), c.as_ptr()) == 1
        })
    }

    /// Check if the given inbox ID is a super admin.
    #[must_use]
    pub fn is_super_admin(&self, inbox_id: &str) -> bool {
        to_c_string(inbox_id).is_ok_and(|c| unsafe {
            xmtp_sys::xmtp_conversation_is_super_admin(self.handle.as_ptr(), c.as_ptr()) == 1
        })
    }

    /// Find duplicate DM conversations for this DM.
    pub fn duplicate_dms(&self) -> Result<Vec<Self>> {
        let mut list: *mut xmtp_sys::XmtpFfiConversationList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_duplicate_dms(self.handle.as_ptr(), &raw mut list)
        };
        error::check(rc)?;
        read_conversation_list_inner(list)
    }

    /// Get debug information for this conversation.
    pub fn debug_info(&self) -> Result<ConversationDebugInfo> {
        let mut out = xmtp_sys::XmtpFfiConversationDebugInfo::default();
        let rc =
            unsafe { xmtp_sys::xmtp_conversation_debug_info(self.handle.as_ptr(), &raw mut out) };
        error::check(rc)?;
        let info = read_debug_info(&out);
        unsafe { xmtp_sys::xmtp_conversation_debug_info_free(&raw mut out) };
        Ok(info)
    }

    /// Get per-inbox last-read timestamps.
    pub fn last_read_times(&self) -> Result<Vec<LastReadTime>> {
        let mut list: *mut xmtp_sys::XmtpFfiLastReadTimeList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_last_read_times(self.handle.as_ptr(), &raw mut list)
        };
        error::check(rc)?;
        if list.is_null() {
            return Ok(vec![]);
        }
        let result = read_last_read_times(list);
        unsafe { xmtp_sys::xmtp_last_read_time_list_free(list) };
        Ok(result)
    }

    /// Get HMAC keys for this conversation (including duplicate DMs).
    pub fn hmac_keys(&self) -> Result<Vec<HmacKeyEntry>> {
        let mut map: *mut xmtp_sys::XmtpFfiHmacKeyMap = ptr::null_mut();
        let rc =
            unsafe { xmtp_sys::xmtp_conversation_hmac_keys(self.handle.as_ptr(), &raw mut map) };
        error::check(rc)?;
        if map.is_null() {
            return Ok(vec![]);
        }
        let result = read_hmac_key_map(map);
        unsafe { xmtp_sys::xmtp_hmac_key_map_free(map) };
        Ok(result)
    }
}

/// Convert `SendOptions` to the FFI struct.
fn send_opts_to_ffi(opts: SendOptions) -> xmtp_sys::XmtpFfiSendOpts {
    xmtp_sys::XmtpFfiSendOpts {
        should_push: i32::from(opts.should_push),
    }
}

/// Convert `ListMessagesOptions` to the FFI struct.
pub(crate) fn msg_opts_to_ffi(
    options: &ListMessagesOptions,
) -> xmtp_sys::XmtpFfiListMessagesOptions {
    xmtp_sys::XmtpFfiListMessagesOptions {
        sent_after_ns: options.sent_after_ns,
        sent_before_ns: options.sent_before_ns,
        limit: options.limit,
        delivery_status: options.delivery_status.map_or(-1, |d| d as i32),
        kind: options.kind.map_or(-1, |k| k as i32),
        direction: options.direction.map_or(0, |d| d as i32),
        ..Default::default()
    }
}

/// Read enriched messages from an FFI list. Caller must free the list.
pub(crate) fn read_enriched_message_list(
    list: *const xmtp_sys::XmtpFfiEnrichedMessageList,
) -> Vec<Message> {
    let len = unsafe { xmtp_sys::xmtp_enriched_message_list_len(list) };
    let mut msgs = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    for i in 0..len {
        let ptr = unsafe { xmtp_sys::xmtp_enriched_message_list_get(list, i) };
        if ptr.is_null() {
            continue;
        }
        let m = unsafe { &*ptr };
        let content = if m.content_bytes.is_null() || m.content_bytes_len <= 0 {
            Vec::new()
        } else {
            unsafe {
                std::slice::from_raw_parts(
                    m.content_bytes,
                    m.content_bytes_len.unsigned_abs() as usize,
                )
            }
            .to_vec()
        };
        msgs.push(Message {
            id: unsafe { c_str_to_string(m.id) },
            conversation_id: unsafe { c_str_to_string(m.group_id) },
            sender_inbox_id: unsafe { c_str_to_string(m.sender_inbox_id) },
            sender_installation_id: unsafe { c_str_to_string(m.sender_installation_id) },
            sent_at_ns: m.sent_at_ns,
            inserted_at_ns: m.inserted_at_ns,
            kind: MessageKind::from_ffi(m.kind as i32).unwrap_or(MessageKind::Application),
            delivery_status: DeliveryStatus::from_ffi(m.delivery_status as i32)
                .unwrap_or(DeliveryStatus::Unpublished),
            content_type: unsafe { nullable_c_str(m.content_type) },
            fallback: unsafe { nullable_c_str(m.fallback_text) },
            content,
            expires_at_ns: m.expires_at_ns,
            num_reactions: m.num_reactions,
            num_replies: m.num_replies,
        });
    }
    msgs
}

/// Read all members from an FFI group member list. Caller must free the list.
fn read_member_list(list: *const xmtp_sys::XmtpFfiGroupMemberList) -> Result<Vec<GroupMember>> {
    let len = unsafe { xmtp_sys::xmtp_group_member_list_len(list) };
    let mut members = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    for i in 0..len {
        let inbox_id = unsafe { take_c_string(xmtp_sys::xmtp_group_member_inbox_id(list, i)) }?;
        let permission_level = PermissionLevel::from_ffi(unsafe {
            xmtp_sys::xmtp_group_member_permission_level(list, i)
        })
        .unwrap_or(PermissionLevel::Member);
        let consent_state =
            ConsentState::from_ffi(unsafe { xmtp_sys::xmtp_group_member_consent_state(list, i) })
                .unwrap_or(ConsentState::Unknown);

        let mut acct_count = 0i32;
        let acct_ptr = unsafe {
            xmtp_sys::xmtp_group_member_account_identifiers(list, i, &raw mut acct_count)
        };
        let account_identifiers = unsafe { read_borrowed_strings(acct_ptr, acct_count) };

        let mut inst_count = 0i32;
        let inst_ptr =
            unsafe { xmtp_sys::xmtp_group_member_installation_ids(list, i, &raw mut inst_count) };
        let installation_ids = unsafe { read_borrowed_strings(inst_ptr, inst_count) };

        members.push(GroupMember {
            inbox_id,
            permission_level,
            consent_state,
            account_identifiers,
            installation_ids,
        });
    }
    Ok(members)
}

/// Read a conversation list into a `Vec<Conversation>`. Handles null.
pub(crate) fn read_conversation_list_inner(
    list: *mut xmtp_sys::XmtpFfiConversationList,
) -> Result<Vec<Conversation>> {
    if list.is_null() {
        return Ok(vec![]);
    }
    let len = unsafe { xmtp_sys::xmtp_conversation_list_len(list) };
    let mut convs = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    for i in 0..len {
        let mut conv: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe { xmtp_sys::xmtp_conversation_list_get(list, i, &raw mut conv) };
        if rc == 0 && !conv.is_null() {
            convs.push(Conversation::from_raw(conv)?);
        }
    }
    unsafe { xmtp_sys::xmtp_conversation_list_free(list) };
    Ok(convs)
}

/// Read permissions from an FFI struct.
fn read_permissions(p: &xmtp_sys::XmtpFfiGroupPermissions) -> Permissions {
    let ps = &p.policy_set;
    let policy = |v: xmtp_sys::XmtpFfiPermissionPolicy| -> PermissionPolicy {
        PermissionPolicy::from_ffi(v as i32).unwrap_or(PermissionPolicy::Deny)
    };
    Permissions {
        preset: GroupPermissionsPreset::from_ffi(p.policy_type as i32)
            .unwrap_or(GroupPermissionsPreset::Custom),
        policies: PermissionPolicySet {
            add_member: policy(ps.add_member_policy),
            remove_member: policy(ps.remove_member_policy),
            add_admin: policy(ps.add_admin_policy),
            remove_admin: policy(ps.remove_admin_policy),
            update_group_name: policy(ps.update_group_name_policy),
            update_group_description: policy(ps.update_group_description_policy),
            update_group_image_url: policy(ps.update_group_image_url_square_policy),
            update_message_disappearing: policy(ps.update_message_disappearing_policy),
            update_app_data: policy(ps.update_app_data_policy),
        },
    }
}

/// Read debug info from an FFI struct.
fn read_debug_info(d: &xmtp_sys::XmtpFfiConversationDebugInfo) -> ConversationDebugInfo {
    let cursors = if d.cursors.is_null() || d.cursors_count <= 0 {
        vec![]
    } else {
        let slice = unsafe {
            std::slice::from_raw_parts(d.cursors, d.cursors_count.unsigned_abs() as usize)
        };
        slice
            .iter()
            .map(|c| Cursor {
                originator_id: c.originator_id,
                sequence_id: c.sequence_id,
            })
            .collect()
    };
    ConversationDebugInfo {
        epoch: d.epoch,
        maybe_forked: d.maybe_forked != 0,
        fork_details: unsafe { nullable_c_str(d.fork_details) },
        is_commit_log_forked: match d.is_commit_log_forked {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        local_commit_log: unsafe { nullable_c_str(d.local_commit_log) },
        remote_commit_log: unsafe { nullable_c_str(d.remote_commit_log) },
        cursors,
    }
}

/// Read last-read timestamps from an FFI list. Caller must free the list.
fn read_last_read_times(list: *const xmtp_sys::XmtpFfiLastReadTimeList) -> Vec<LastReadTime> {
    let len = unsafe { xmtp_sys::xmtp_last_read_time_list_len(list) };
    let mut result = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    for i in 0..len {
        let ptr = unsafe { xmtp_sys::xmtp_last_read_time_list_get(list, i) };
        if ptr.is_null() {
            continue;
        }
        let entry = unsafe { &*ptr };
        result.push(LastReadTime {
            inbox_id: unsafe { c_str_to_string(entry.inbox_id) },
            timestamp_ns: entry.timestamp_ns,
        });
    }
    result
}

/// Read HMAC key map from an FFI handle. Caller must free the map.
pub(crate) fn read_hmac_key_map(map: *const xmtp_sys::XmtpFfiHmacKeyMap) -> Vec<HmacKeyEntry> {
    let len = unsafe { xmtp_sys::xmtp_hmac_key_map_len(map) };
    let mut entries = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    for i in 0..len {
        let gid_ptr = unsafe { xmtp_sys::xmtp_hmac_key_map_group_id(map, i) };
        let group_id = if gid_ptr.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(gid_ptr) }
                .to_str()
                .unwrap_or_default()
                .to_owned()
        };
        let mut key_count = 0i32;
        let keys_ptr = unsafe { xmtp_sys::xmtp_hmac_key_map_keys(map, i, &raw mut key_count) };
        let keys = if keys_ptr.is_null() || key_count <= 0 {
            vec![]
        } else {
            let slice =
                unsafe { std::slice::from_raw_parts(keys_ptr, key_count.unsigned_abs() as usize) };
            slice
                .iter()
                .map(|k| {
                    let key = if k.key.is_null() || k.key_len <= 0 {
                        vec![]
                    } else {
                        unsafe {
                            std::slice::from_raw_parts(k.key, k.key_len.unsigned_abs() as usize)
                        }
                        .to_vec()
                    };
                    HmacKey {
                        key,
                        epoch: k.epoch,
                    }
                })
                .collect()
        };
        entries.push(HmacKeyEntry { group_id, keys });
    }
    entries
}

/// Read a borrowed (non-owned) C string pointer. Returns empty string if null.
unsafe fn c_str_to_string(ptr: *mut c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .unwrap_or_default()
            .to_owned()
    }
}

/// Read a nullable borrowed C string pointer. Returns `None` if null.
unsafe fn nullable_c_str(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(
            unsafe { CStr::from_ptr(ptr) }
                .to_str()
                .unwrap_or_default()
                .to_owned(),
        )
    }
}
