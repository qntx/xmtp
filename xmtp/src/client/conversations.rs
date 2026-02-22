#![allow(unsafe_code)]
//! Conversation creation, listing, and synchronization.

use std::ptr;

use crate::conversation::Conversation;
use crate::error::{self, Result};
use crate::ffi::{
    c_str_ptr, identifiers_to_ffi, optional_c_string, to_c_string, to_c_string_array,
};
use crate::types::*;

use super::Client;

impl Client {
    /// Create a group conversation with default options.
    pub fn create_group(&self, member_inbox_ids: &[&str]) -> Result<Conversation> {
        self.create_group_with_options(member_inbox_ids, &CreateGroupOptions::default())
    }

    /// Create a group conversation with custom options.
    pub fn create_group_with_options(
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
                    &mut out,
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
                    &mut out,
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
                    &mut out,
                )
            };
            error::check(rc)?;
            Conversation::from_raw(out)
        })
    }

    /// Find or create a DM by address and identifier kind.
    pub fn create_dm(&self, address: &str, kind: IdentifierKind) -> Result<Conversation> {
        let c = to_c_string(address)?;
        let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_create_dm(
                self.handle.as_ptr(),
                c.as_ptr(),
                kind as i32,
                0,
                0,
                &mut out,
            )
        };
        error::check(rc)?;
        Conversation::from_raw(out)
    }

    /// Find or create a DM by inbox ID.
    pub fn create_dm_by_inbox_id(
        &self,
        inbox_id: &str,
        disappearing: Option<DisappearingSettings>,
    ) -> Result<Conversation> {
        let c = to_c_string(inbox_id)?;
        let ds = disappearing.unwrap_or_default();
        let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_create_dm_by_inbox_id(
                self.handle.as_ptr(),
                c.as_ptr(),
                ds.from_ns,
                ds.in_ns,
                &mut out,
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
            xmtp_sys::xmtp_client_find_dm_by_inbox_id(self.handle.as_ptr(), c.as_ptr(), &mut out)
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
            xmtp_sys::xmtp_client_get_conversation_by_id(self.handle.as_ptr(), c.as_ptr(), &mut out)
        };
        error::check(rc)?;
        if out.is_null() {
            Ok(None)
        } else {
            Conversation::from_raw(out).map(Some)
        }
    }

    /// List conversations with default options (all types).
    pub fn conversations(&self) -> Result<Vec<Conversation>> {
        self.conversations_with(&ListConversationsOptions::default())
    }

    /// List conversations with filtering options.
    pub fn conversations_with(
        &self,
        options: &ListConversationsOptions,
    ) -> Result<Vec<Conversation>> {
        let consent_i32: Vec<i32> = options.consent_states.iter().map(|s| *s as i32).collect();
        let ffi_opts = xmtp_sys::XmtpFfiListConversationsOptions {
            conversation_type: options.conversation_type.map_or(-1, |t| t as i32),
            limit: options.limit,
            created_after_ns: options.created_after_ns,
            created_before_ns: options.created_before_ns,
            consent_states: if consent_i32.is_empty() {
                ptr::null()
            } else {
                consent_i32.as_ptr()
            },
            consent_states_count: consent_i32.len() as i32,
            ..Default::default()
        };
        let mut list: *mut xmtp_sys::XmtpFfiConversationList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_list_conversations(self.handle.as_ptr(), &ffi_opts, &mut list)
        };
        error::check(rc)?;
        let result = read_conversation_list(list);
        if !list.is_null() {
            unsafe { xmtp_sys::xmtp_conversation_list_free(list) };
        }
        result
    }

    /// Sync welcomes (process new group invitations).
    pub fn sync_welcomes(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_client_sync_welcomes(self.handle.as_ptr()) })
    }

    /// Sync all conversations. Returns `(synced, eligible)` counts.
    pub fn sync_all(&self) -> Result<(i32, i32)> {
        self.sync_all_with(&[])
    }

    /// Sync all conversations filtered by consent states.
    pub fn sync_all_with(&self, consent_states: &[ConsentState]) -> Result<(i32, i32)> {
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
                &mut synced,
                &mut eligible,
            )
        };
        error::check(rc)?;
        Ok((synced, eligible))
    }

    /// Delete a message by its hex ID. Returns the number of deleted rows.
    pub fn delete_message_by_id(&self, message_id_hex: &str) -> Result<i32> {
        let c = to_c_string(message_id_hex)?;
        let rows =
            unsafe { xmtp_sys::xmtp_client_delete_message_by_id(self.handle.as_ptr(), c.as_ptr()) };
        if rows < 0 {
            Err(error::last_ffi_error())
        } else {
            Ok(rows)
        }
    }
}

/// Build FFI group options and pass them to a closure. CStrings live on the
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
        name: c_str_ptr(&c_name),
        description: c_str_ptr(&c_desc),
        image_url: c_str_ptr(&c_img),
        app_data: c_str_ptr(&c_app),
        message_disappear_from_ns: ds.from_ns,
        message_disappear_in_ns: ds.in_ns,
    };
    f(&ffi)
}

/// Read an FFI conversation list into `Vec<Conversation>`. Does NOT free the list.
fn read_conversation_list(
    list: *mut xmtp_sys::XmtpFfiConversationList,
) -> Result<Vec<Conversation>> {
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
    Ok(convs)
}
