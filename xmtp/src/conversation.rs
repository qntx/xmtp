#![allow(unsafe_code)]
//! Conversation operations: send, messages, members, metadata, consent,
//! disappearing messages, admin management, and permissions.

use std::ffi::c_char;
use std::ptr;

use crate::error::{self, Result};
use crate::ffi::{
    OwnedHandle, identifiers_to_ffi, read_borrowed_strings, take_c_string, take_nullable_string,
    to_c_string, to_c_string_array,
};
use crate::types::*;

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

/// A decoded message from a conversation.
#[derive(Debug, Clone)]
pub struct Message {
    /// Hex-encoded message ID.
    pub id: String,
    /// Sender's inbox ID.
    pub sender_inbox_id: String,
    /// Sent timestamp in nanoseconds.
    pub sent_at_ns: i64,
    /// Message kind (application vs membership change).
    pub kind: MessageKind,
    /// Delivery status.
    pub delivery_status: DeliveryStatus,
    /// Raw decrypted content bytes (encoded `EncodedContent` protobuf).
    pub content: Vec<u8>,
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

    /// Raw const pointer for internal use by the stream module.
    pub(crate) fn handle_ptr(&self) -> *const xmtp_sys::XmtpFfiConversation {
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
    metadata_getter!(/// Inbox ID of the member who added this client to the conversation.
        added_by_inbox_id, xmtp_sys::xmtp_conversation_added_by_inbox_id);

    /// Check if conversation is paused for a version upgrade.
    pub fn paused_for_version(&self) -> Result<Option<String>> {
        let mut out: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_paused_for_version(self.handle.as_ptr(), &mut out)
        };
        error::check(rc)?;
        if out.is_null() {
            Ok(None)
        } else {
            unsafe { take_c_string(out) }.map(Some)
        }
    }

    /// Sync this conversation with the network.
    pub fn sync(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_conversation_sync(self.handle.as_ptr()) })
    }

    /// Send raw encoded content bytes. Returns the hex-encoded message ID.
    pub fn send(&self, content: &[u8]) -> Result<String> {
        let mut out: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_send(
                self.handle.as_ptr(),
                content.as_ptr(),
                content.len() as i32,
                ptr::null(),
                &mut out,
            )
        };
        error::check(rc)?;
        unsafe { take_c_string(out) }
    }

    /// Send optimistically (returns immediately, publishes in background).
    pub fn send_optimistic(&self, content: &[u8]) -> Result<String> {
        let mut out: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_send_optimistic(
                self.handle.as_ptr(),
                content.as_ptr(),
                content.len() as i32,
                ptr::null(),
                &mut out,
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
        self.messages_with(&ListMessagesOptions::default())
    }

    /// List messages with filtering options.
    pub fn messages_with(&self, options: &ListMessagesOptions) -> Result<Vec<Message>> {
        let ffi_opts = msg_opts_to_ffi(options);
        let mut list: *mut xmtp_sys::XmtpFfiMessageList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_list_messages(self.handle.as_ptr(), &ffi_opts, &mut list)
        };
        error::check(rc)?;
        if list.is_null() {
            return Ok(vec![]);
        }
        let result = read_message_list(list);
        unsafe { xmtp_sys::xmtp_message_list_free(list) };
        result
    }

    /// Count messages matching filter options.
    pub fn count_messages(&self, options: &ListMessagesOptions) -> i64 {
        let ffi_opts = msg_opts_to_ffi(options);
        unsafe { xmtp_sys::xmtp_conversation_count_messages(self.handle.as_ptr(), &ffi_opts) }
    }

    /// List members of this conversation.
    pub fn members(&self) -> Result<Vec<GroupMember>> {
        let mut list: *mut xmtp_sys::XmtpFfiGroupMemberList = ptr::null_mut();
        let rc =
            unsafe { xmtp_sys::xmtp_conversation_list_members(self.handle.as_ptr(), &mut list) };
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
        let rc =
            unsafe { xmtp_sys::xmtp_conversation_consent_state(self.handle.as_ptr(), &mut out) };
        error::check(rc)?;
        ConsentState::from_ffi(out)
            .ok_or_else(|| crate::Error::Ffi(format!("unknown consent state: {out}")))
    }

    /// Update the consent state for this conversation.
    pub fn update_consent_state(&self, state: ConsentState) -> Result<()> {
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_update_consent_state(self.handle.as_ptr(), state as i32)
        })
    }

    metadata_getter!(/// Get the group name.
        group_name, xmtp_sys::xmtp_conversation_group_name);
    metadata_setter!(/// Update the group name.
        update_group_name, xmtp_sys::xmtp_conversation_update_group_name);
    metadata_getter!(/// Get the group description.
        group_description, xmtp_sys::xmtp_conversation_group_description);
    metadata_setter!(/// Update the group description.
        update_group_description, xmtp_sys::xmtp_conversation_update_group_description);
    metadata_getter!(/// Get the group image URL.
        group_image_url, xmtp_sys::xmtp_conversation_group_image_url);
    metadata_setter!(/// Update the group image URL.
        update_group_image_url, xmtp_sys::xmtp_conversation_update_group_image_url);
    metadata_getter!(/// Get app data string.
        app_data, xmtp_sys::xmtp_conversation_app_data);
    metadata_setter!(/// Update app data (max 8192 bytes).
        update_app_data, xmtp_sys::xmtp_conversation_update_app_data);

    /// Get the current disappearing message settings.
    pub fn disappearing_settings(&self) -> Option<DisappearingSettings> {
        let mut out = xmtp_sys::XmtpFfiDisappearingSettings::default();
        let rc = unsafe {
            xmtp_sys::xmtp_conversation_disappearing_settings(self.handle.as_ptr(), &mut out)
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

    /// Update disappearing message settings.
    pub fn update_disappearing_settings(&self, settings: DisappearingSettings) -> Result<()> {
        let ffi = xmtp_sys::XmtpFfiDisappearingSettings {
            from_ns: settings.from_ns,
            in_ns: settings.in_ns,
        };
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_update_disappearing_settings(self.handle.as_ptr(), &ffi)
        })
    }

    /// Remove disappearing message settings.
    pub fn remove_disappearing_settings(&self) -> Result<()> {
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_remove_disappearing_settings(self.handle.as_ptr())
        })
    }

    /// Whether disappearing messages are enabled.
    #[must_use]
    pub fn is_disappearing_enabled(&self) -> bool {
        unsafe { xmtp_sys::xmtp_conversation_is_disappearing_enabled(self.handle.as_ptr()) == 1 }
    }

    /// Update a permission policy on this conversation.
    pub fn update_permission_policy(
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

    /// Low-level admin list update. `action`: 0=AddAdmin, 1=RemoveAdmin, 2=AddSuperAdmin, 3=RemoveSuperAdmin.
    pub fn update_admin_list(&self, inbox_id: &str, action: i32) -> Result<()> {
        let c = to_c_string(inbox_id)?;
        error::check(unsafe {
            xmtp_sys::xmtp_conversation_update_admin_list(self.handle.as_ptr(), c.as_ptr(), action)
        })
    }

    /// List admin inbox IDs.
    pub fn list_admins(&self) -> Vec<String> {
        let mut count = 0i32;
        let ptr =
            unsafe { xmtp_sys::xmtp_conversation_list_admins(self.handle.as_ptr(), &mut count) };
        unsafe { read_borrowed_strings(ptr.cast_const(), count) }
    }

    /// List super admin inbox IDs.
    pub fn list_super_admins(&self) -> Vec<String> {
        let mut count = 0i32;
        let ptr = unsafe {
            xmtp_sys::xmtp_conversation_list_super_admins(self.handle.as_ptr(), &mut count)
        };
        unsafe { read_borrowed_strings(ptr.cast_const(), count) }
    }

    /// Check if the given inbox ID is an admin.
    #[must_use]
    pub fn is_admin(&self, inbox_id: &str) -> bool {
        to_c_string(inbox_id).map_or(false, |c| unsafe {
            xmtp_sys::xmtp_conversation_is_admin(self.handle.as_ptr(), c.as_ptr()) == 1
        })
    }

    /// Check if the given inbox ID is a super admin.
    #[must_use]
    pub fn is_super_admin(&self, inbox_id: &str) -> bool {
        to_c_string(inbox_id).map_or(false, |c| unsafe {
            xmtp_sys::xmtp_conversation_is_super_admin(self.handle.as_ptr(), c.as_ptr()) == 1
        })
    }

    /// Find duplicate DM conversations for this DM.
    pub fn duplicate_dms(&self) -> Result<Vec<Conversation>> {
        let mut list: *mut xmtp_sys::XmtpFfiConversationList = ptr::null_mut();
        let rc =
            unsafe { xmtp_sys::xmtp_conversation_duplicate_dms(self.handle.as_ptr(), &mut list) };
        error::check(rc)?;
        if list.is_null() {
            return Ok(vec![]);
        }
        let len = unsafe { xmtp_sys::xmtp_conversation_list_len(list) };
        let mut convs = Vec::with_capacity(len.max(0) as usize);
        for i in 0..len {
            let mut conv: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
            let rc = unsafe { xmtp_sys::xmtp_conversation_list_get(list, i, &mut conv) };
            if rc == 0 && !conv.is_null() {
                convs.push(Conversation::from_raw(conv)?);
            }
        }
        unsafe { xmtp_sys::xmtp_conversation_list_free(list) };
        Ok(convs)
    }
}

/// Convert `ListMessagesOptions` to the FFI struct.
fn msg_opts_to_ffi(options: &ListMessagesOptions) -> xmtp_sys::XmtpFfiListMessagesOptions {
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

/// Read all messages from an FFI message list. The list must be freed by the caller.
fn read_message_list(list: *const xmtp_sys::XmtpFfiMessageList) -> Result<Vec<Message>> {
    let len = unsafe { xmtp_sys::xmtp_message_list_len(list) };
    let mut msgs = Vec::with_capacity(len.max(0) as usize);
    for i in 0..len {
        let id = unsafe { take_c_string(xmtp_sys::xmtp_message_id(list, i)) }?;
        let sender = unsafe { take_c_string(xmtp_sys::xmtp_message_sender_inbox_id(list, i)) }?;
        let sent_at_ns = unsafe { xmtp_sys::xmtp_message_sent_at_ns(list, i) };
        let kind = MessageKind::from_ffi(unsafe { xmtp_sys::xmtp_message_kind(list, i) })
            .unwrap_or(MessageKind::Application);
        let delivery_status =
            DeliveryStatus::from_ffi(unsafe { xmtp_sys::xmtp_message_delivery_status(list, i) })
                .unwrap_or(DeliveryStatus::Unpublished);

        // Content bytes are borrowed from the list -- copy before the list is freed.
        let mut content_len = 0i32;
        let content_ptr =
            unsafe { xmtp_sys::xmtp_message_content_bytes(list, i, &mut content_len) };
        let content = if content_ptr.is_null() || content_len <= 0 {
            vec![]
        } else {
            unsafe { std::slice::from_raw_parts(content_ptr, content_len as usize) }.to_vec()
        };

        msgs.push(Message {
            id,
            sender_inbox_id: sender,
            sent_at_ns,
            kind,
            delivery_status,
            content,
        });
    }
    Ok(msgs)
}

/// Read all members from an FFI group member list. The list must be freed by the caller.
fn read_member_list(list: *const xmtp_sys::XmtpFfiGroupMemberList) -> Result<Vec<GroupMember>> {
    let len = unsafe { xmtp_sys::xmtp_group_member_list_len(list) };
    let mut members = Vec::with_capacity(len.max(0) as usize);
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
        let acct_ptr =
            unsafe { xmtp_sys::xmtp_group_member_account_identifiers(list, i, &mut acct_count) };
        let account_identifiers = unsafe { read_borrowed_strings(acct_ptr, acct_count) };

        let mut inst_count = 0i32;
        let inst_ptr =
            unsafe { xmtp_sys::xmtp_group_member_installation_ids(list, i, &mut inst_count) };
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
