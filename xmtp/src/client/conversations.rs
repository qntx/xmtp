#![allow(unsafe_code)]
//! Conversation creation, listing, synchronization, and message lookup.

use std::ptr;

use crate::conversation::{
    Conversation, Message, read_conversation_list_inner, read_enriched_message_list,
    read_hmac_key_map,
};
use crate::error::{self, Result};
use crate::ffi::{
    c_str_ptr, identifiers_to_ffi, optional_c_string, to_c_string, to_c_string_array,
};
use crate::types::{
    AccountIdentifier, ConsentState, CreateDmOptions, CreateGroupOptions, HmacKeyEntry,
    IdentifierKind, ListConversationsOptions, SyncResult,
};

use super::Client;

impl Client {
    /// Create a group conversation.
    pub fn create_group(
        &self,
        member_inbox_ids: &[&str],
        options: &CreateGroupOptions,
    ) -> Result<Conversation> {
        let (_owned, ptrs) = to_c_string_array(member_inbox_ids)?;
        with_group_ffi_opts(options, |ffi_opts| {
            let ids_ptr = if ptrs.is_empty() {
                ptr::null()
            } else {
                ptrs.as_ptr()
            };
            let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
            let rc = unsafe {
                xmtp_sys::xmtp_client_create_group(
                    self.handle.as_ptr(),
                    ffi_opts,
                    ids_ptr,
                    ptrs.len() as i32,
                    &raw mut out,
                )
            };
            error::check(rc)?;
            Conversation::from_raw(out)
        })
    }

    /// Create a group by external identifiers (address + kind).
    pub fn create_group_by_identifiers(
        &self,
        identifiers: &[AccountIdentifier],
        options: &CreateGroupOptions,
    ) -> Result<Conversation> {
        let (_owned, ptrs, kinds) = identifiers_to_ffi(identifiers)?;
        with_group_ffi_opts(options, |ffi_opts| {
            let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
            let rc = unsafe {
                xmtp_sys::xmtp_client_create_group_by_identity(
                    self.handle.as_ptr(),
                    ffi_opts,
                    ptrs.as_ptr(),
                    kinds.as_ptr(),
                    ptrs.len() as i32,
                    &raw mut out,
                )
            };
            error::check(rc)?;
            Conversation::from_raw(out)
        })
    }

    /// Create a group without syncing (optimistic / offline).
    pub fn create_group_optimistic(&self, options: &CreateGroupOptions) -> Result<Conversation> {
        with_group_ffi_opts(options, |ffi_opts| {
            let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
            let rc = unsafe {
                xmtp_sys::xmtp_client_create_group_optimistic(
                    self.handle.as_ptr(),
                    ffi_opts,
                    &raw mut out,
                )
            };
            error::check(rc)?;
            Conversation::from_raw(out)
        })
    }

    /// Find or create a DM by address and identifier kind.
    pub fn create_dm(&self, address: &str, kind: IdentifierKind) -> Result<Conversation> {
        self.create_dm_with(address, kind, &CreateDmOptions::default())
    }

    /// Find or create a DM with options (e.g. disappearing messages).
    pub fn create_dm_with(
        &self,
        address: &str,
        kind: IdentifierKind,
        opts: &CreateDmOptions,
    ) -> Result<Conversation> {
        let c = to_c_string(address)?;
        let ds = opts.disappearing.unwrap_or_default();
        let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_create_dm(
                self.handle.as_ptr(),
                c.as_ptr(),
                kind as i32,
                ds.from_ns,
                ds.in_ns,
                &raw mut out,
            )
        };
        error::check(rc)?;
        Conversation::from_raw(out)
    }

    /// Find or create a DM by inbox ID.
    pub fn create_dm_by_inbox_id(&self, inbox_id: &str) -> Result<Conversation> {
        self.create_dm_by_inbox_id_with(inbox_id, &CreateDmOptions::default())
    }

    /// Find or create a DM by inbox ID with options.
    pub fn create_dm_by_inbox_id_with(
        &self,
        inbox_id: &str,
        opts: &CreateDmOptions,
    ) -> Result<Conversation> {
        let c = to_c_string(inbox_id)?;
        let ds = opts.disappearing.unwrap_or_default();
        let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_create_dm_by_inbox_id(
                self.handle.as_ptr(),
                c.as_ptr(),
                ds.from_ns,
                ds.in_ns,
                &raw mut out,
            )
        };
        error::check(rc)?;
        Conversation::from_raw(out)
    }

    /// Find an existing DM by inbox ID.
    pub fn find_dm_by_inbox_id(&self, inbox_id: &str) -> Result<Option<Conversation>> {
        let c = to_c_string(inbox_id)?;
        let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_find_dm_by_inbox_id(
                self.handle.as_ptr(),
                c.as_ptr(),
                &raw mut out,
            )
        };
        error::check(rc)?;
        if out.is_null() {
            Ok(None)
        } else {
            Conversation::from_raw(out).map(Some)
        }
    }

    /// Get a conversation by its hex-encoded group ID.
    pub fn conversation(&self, hex_id: &str) -> Result<Option<Conversation>> {
        let c = to_c_string(hex_id)?;
        let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_get_conversation_by_id(
                self.handle.as_ptr(),
                c.as_ptr(),
                &raw mut out,
            )
        };
        error::check(rc)?;
        if out.is_null() {
            Ok(None)
        } else {
            Conversation::from_raw(out).map(Some)
        }
    }

    /// List all conversations with default options.
    pub fn conversations(&self) -> Result<Vec<Conversation>> {
        self.list_conversations(&ListConversationsOptions::default())
    }

    /// List conversations with filtering options.
    pub fn list_conversations(
        &self,
        options: &ListConversationsOptions,
    ) -> Result<Vec<Conversation>> {
        let consent_i32: Vec<i32> = options.consent_states.iter().map(|s| *s as i32).collect();
        let ffi_opts = xmtp_sys::XmtpFfiListConversationsOptions {
            conversation_type: options.conversation_type.map_or(-1, |t| t as i32),
            limit: options.limit,
            created_after_ns: options.created_after_ns,
            created_before_ns: options.created_before_ns,
            last_activity_after_ns: options.last_activity_after_ns,
            last_activity_before_ns: options.last_activity_before_ns,
            consent_states: if consent_i32.is_empty() {
                ptr::null()
            } else {
                consent_i32.as_ptr()
            },
            consent_states_count: consent_i32.len() as i32,
            order_by: options.order_by as i32,
            include_duplicate_dms: i32::from(options.include_duplicate_dms),
        };
        let mut list: *mut xmtp_sys::XmtpFfiConversationList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_list_conversations(
                self.handle.as_ptr(),
                &raw const ffi_opts,
                &raw mut list,
            )
        };
        error::check(rc)?;
        read_conversation_list_inner(list)
    }

    /// Sync welcomes (process new group invitations).
    pub fn sync_welcomes(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_client_sync_welcomes(self.handle.as_ptr()) })
    }

    /// Sync all conversations, optionally filtered by consent states.
    pub fn sync_all(&self, consent_states: &[ConsentState]) -> Result<SyncResult> {
        let cs: Vec<i32> = consent_states.iter().map(|s| *s as i32).collect();
        let (mut synced, mut eligible) = (0i32, 0i32);
        let rc = unsafe {
            xmtp_sys::xmtp_client_sync_all(
                self.handle.as_ptr(),
                if cs.is_empty() {
                    ptr::null()
                } else {
                    cs.as_ptr()
                },
                cs.len() as i32,
                &raw mut synced,
                &raw mut eligible,
            )
        };
        error::check(rc)?;
        Ok(SyncResult {
            synced: synced as u32,
            eligible: eligible as u32,
        })
    }

    /// Delete a message by its hex ID. Returns the number of deleted rows.
    pub fn delete_message(&self, message_id_hex: &str) -> Result<i32> {
        let c = to_c_string(message_id_hex)?;
        let rows =
            unsafe { xmtp_sys::xmtp_client_delete_message_by_id(self.handle.as_ptr(), c.as_ptr()) };
        if rows < 0 {
            Err(error::last_ffi_error())
        } else {
            Ok(rows)
        }
    }

    /// Get a message by its hex-encoded ID.
    pub fn message_by_id(&self, message_id_hex: &str) -> Result<Option<Message>> {
        let c = to_c_string(message_id_hex)?;
        let mut out: *mut xmtp_sys::XmtpFfiEnrichedMessageList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_get_enriched_message_by_id(
                self.handle.as_ptr(),
                c.as_ptr(),
                &raw mut out,
            )
        };
        error::check(rc)?;
        if out.is_null() {
            return Ok(None);
        }
        let msgs = read_enriched_message_list(out);
        unsafe { xmtp_sys::xmtp_enriched_message_list_free(out) };
        Ok(msgs.into_iter().next())
    }

    /// Sync preferences (device sync groups only).
    pub fn sync_preferences(&self) -> Result<SyncResult> {
        let (mut synced, mut eligible) = (0i32, 0i32);
        let rc = unsafe {
            xmtp_sys::xmtp_client_sync_preferences(
                self.handle.as_ptr(),
                &raw mut synced,
                &raw mut eligible,
            )
        };
        error::check(rc)?;
        Ok(SyncResult {
            synced: synced as u32,
            eligible: eligible as u32,
        })
    }

    /// Get HMAC keys for all conversations. For push notification verification.
    pub fn hmac_keys(&self) -> Result<Vec<HmacKeyEntry>> {
        let mut map: *mut xmtp_sys::XmtpFfiHmacKeyMap = ptr::null_mut();
        let rc = unsafe { xmtp_sys::xmtp_client_hmac_keys(self.handle.as_ptr(), &raw mut map) };
        error::check(rc)?;
        if map.is_null() {
            return Ok(vec![]);
        }
        let result = read_hmac_key_map(map);
        unsafe { xmtp_sys::xmtp_hmac_key_map_free(map) };
        Ok(result)
    }
}

/// Build FFI group options and pass them to a closure. `CStrings` live on the
/// stack and are valid for the duration of `f`.
fn with_group_ffi_opts<R>(
    options: &CreateGroupOptions,
    f: impl FnOnce(&xmtp_sys::XmtpFfiCreateGroupOptions) -> Result<R>,
) -> Result<R> {
    let c_name = optional_c_string(options.name.as_deref())?;
    let c_desc = optional_c_string(options.description.as_deref())?;
    let c_img = optional_c_string(options.image_url.as_deref())?;
    let c_app = optional_c_string(options.app_data.as_deref())?;
    let ds = options.disappearing.unwrap_or_default();
    let ffi = xmtp_sys::XmtpFfiCreateGroupOptions {
        permissions: options.permissions.map_or(0, |p| p as i32),
        name: c_str_ptr(c_name.as_ref()),
        description: c_str_ptr(c_desc.as_ref()),
        image_url: c_str_ptr(c_img.as_ref()),
        app_data: c_str_ptr(c_app.as_ref()),
        message_disappear_from_ns: ds.from_ns,
        message_disappear_in_ns: ds.in_ns,
    };
    f(&ffi)
}
