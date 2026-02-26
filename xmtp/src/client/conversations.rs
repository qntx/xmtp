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
use crate::resolve::Recipient;
use crate::types::{
    AccountIdentifier, ConsentState, ConversationType, CreateDmOptions, CreateGroupOptions,
    HmacKeyEntry, IdentifierKind, ListConversationsOptions, SyncResult,
};

use super::Client;

impl Client {
    /// Create a group with any recipient types.
    ///
    /// Accepts Ethereum addresses, inbox IDs, and ENS names (if a
    /// [`Resolver`](crate::Resolver) is configured). Mixed types are supported.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn example(client: &xmtp::Client) -> xmtp::Result<()> {
    /// use xmtp::{CreateGroupOptions, Recipient};
    ///
    /// let members = [
    ///     Recipient::parse("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045"),
    ///     Recipient::parse("vitalik.eth"),
    /// ];
    /// let opts = CreateGroupOptions { name: Some("My Group".into()), ..Default::default() };
    /// client.group(&members, &opts)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn group(&self, members: &[Recipient], opts: &CreateGroupOptions) -> Result<Conversation> {
        let (identifiers, inbox_ids) = self.resolve_recipients(members)?;
        // Pick the most efficient FFI path.
        if inbox_ids.is_empty() {
            self.group_by_identifiers(&identifiers, opts)
        } else if identifiers.is_empty() {
            let ids: Vec<&str> = inbox_ids.iter().map(String::as_str).collect();
            self.group_by_inbox_ids(&ids, opts)
        } else {
            // Mixed: resolve identifiers → inbox IDs, then create by inbox IDs.
            let mut all_ids = inbox_ids;
            for ident in &identifiers {
                let id = self
                    .inbox_id_for(&ident.address, ident.kind)?
                    .ok_or_else(|| {
                        crate::Error::Resolution(format!("no inbox for {}", ident.address))
                    })?;
                all_ids.push(id);
            }
            let ids: Vec<&str> = all_ids.iter().map(String::as_str).collect();
            self.group_by_inbox_ids(&ids, opts)
        }
    }

    /// Add members to a group conversation by any recipient type.
    ///
    /// Accepts Ethereum addresses, inbox IDs, and ENS names (if a
    /// [`Resolver`](crate::Resolver) is configured).
    pub fn add_members(&self, conv: &Conversation, members: &[Recipient]) -> Result<()> {
        let (idents, inbox_ids) = self.resolve_recipients(members)?;
        if !idents.is_empty() {
            conv.add_members_by_identity(&idents)?;
        }
        if !inbox_ids.is_empty() {
            let ids: Vec<&str> = inbox_ids.iter().map(String::as_str).collect();
            conv.add_members_by_inbox_id(&ids)?;
        }
        Ok(())
    }

    /// Remove members from a group conversation by any recipient type.
    ///
    /// Accepts Ethereum addresses, inbox IDs, and ENS names (if a
    /// [`Resolver`](crate::Resolver) is configured).
    pub fn remove_members(&self, conv: &Conversation, members: &[Recipient]) -> Result<()> {
        let (idents, inbox_ids) = self.resolve_recipients(members)?;
        if !idents.is_empty() {
            conv.remove_members_by_identity(&idents)?;
        }
        if !inbox_ids.is_empty() {
            let ids: Vec<&str> = inbox_ids.iter().map(String::as_str).collect();
            conv.remove_members_by_inbox_id(&ids)?;
        }
        Ok(())
    }

    /// Check which recipients can receive XMTP messages.
    ///
    /// Returns a parallel `Vec<bool>` — one entry per recipient.
    /// Inbox-ID recipients are assumed reachable (always `true`).
    pub fn can_message_recipients(&self, recipients: &[&Recipient]) -> Result<Vec<bool>> {
        let mut results = vec![true; recipients.len()];
        // Collect address-based recipients that need an on-network check.
        let checks: Vec<(usize, AccountIdentifier)> = recipients
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Recipient::Address(a) => Some((
                    i,
                    AccountIdentifier {
                        address: a.clone(),
                        kind: IdentifierKind::Ethereum,
                    },
                )),
                Recipient::Ens(name) => self.resolve_ens(name).ok().map(|addr| {
                    (
                        i,
                        AccountIdentifier {
                            address: addr,
                            kind: IdentifierKind::Ethereum,
                        },
                    )
                }),
                Recipient::InboxId(_) => None,
            })
            .collect();
        if checks.is_empty() {
            return Ok(results);
        }
        let idents: Vec<AccountIdentifier> = checks.iter().map(|(_, id)| id.clone()).collect();
        let flags = self.can_message(&idents)?;
        for ((idx, _), reachable) in checks.into_iter().zip(flags) {
            results[idx] = reachable;
        }
        Ok(results)
    }

    /// Resolve an ENS name to an Ethereum address.
    fn resolve_ens(&self, name: &str) -> Result<String> {
        self.resolver
            .as_ref()
            .ok_or(crate::Error::NoResolver)?
            .resolve(name)
    }

    /// Reverse-resolve an Ethereum address to a human-readable name (e.g. ENS).
    ///
    /// Returns `None` if no resolver is configured or no reverse record exists.
    #[must_use]
    pub fn reverse_resolve(&self, address: &str) -> Option<String> {
        self.resolver
            .as_ref()
            .and_then(|r| r.reverse_resolve(address).ok().flatten())
    }

    /// Resolve recipients into identifiers and inbox IDs.
    pub(crate) fn resolve_recipients(
        &self,
        members: &[Recipient],
    ) -> Result<(Vec<AccountIdentifier>, Vec<String>)> {
        let mut idents = Vec::new();
        let mut inbox_ids = Vec::new();
        for m in members {
            match m {
                Recipient::Address(addr) => idents.push(AccountIdentifier {
                    address: addr.clone(),
                    kind: IdentifierKind::Ethereum,
                }),
                Recipient::InboxId(id) => inbox_ids.push(id.clone()),
                Recipient::Ens(name) => idents.push(AccountIdentifier {
                    address: self.resolve_ens(name)?,
                    kind: IdentifierKind::Ethereum,
                }),
            }
        }
        Ok((idents, inbox_ids))
    }

    /// Create a group without syncing (optimistic / offline).
    pub fn group_optimistic(&self, opts: &CreateGroupOptions) -> Result<Conversation> {
        with_group_ffi_opts(opts, |ffi_opts| {
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

    fn group_by_inbox_ids(
        &self,
        inbox_ids: &[&str],
        opts: &CreateGroupOptions,
    ) -> Result<Conversation> {
        let (_owned, ptrs) = to_c_string_array(inbox_ids)?;
        with_group_ffi_opts(opts, |ffi_opts| {
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

    fn group_by_identifiers(
        &self,
        identifiers: &[AccountIdentifier],
        opts: &CreateGroupOptions,
    ) -> Result<Conversation> {
        let (_owned, ptrs, kinds) = identifiers_to_ffi(identifiers)?;
        with_group_ffi_opts(opts, |ffi_opts| {
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

    /// Find or create a DM with any recipient type.
    ///
    /// Accepts Ethereum addresses, inbox IDs, and ENS names (if a
    /// [`Resolver`](crate::Resolver) is configured).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # fn example(client: &xmtp::Client) -> xmtp::Result<()> {
    /// // By address
    /// client.dm(&"0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045".into())?;
    /// // By inbox ID
    /// client.dm(&xmtp::Recipient::InboxId("abc123".into()))?;
    /// // By ENS (requires resolver)
    /// client.dm(&"vitalik.eth".into())?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn dm(&self, to: &Recipient) -> Result<Conversation> {
        self.dm_with(to, &CreateDmOptions::default())
    }

    /// Find or create a DM with options (e.g. disappearing messages).
    pub fn dm_with(&self, to: &Recipient, opts: &CreateDmOptions) -> Result<Conversation> {
        match to {
            Recipient::Address(addr) => self.dm_by_address(addr, opts),
            Recipient::InboxId(id) => self.dm_by_inbox_id(id, opts),
            Recipient::Ens(name) => self.dm_by_address(&self.resolve_ens(name)?, opts),
        }
    }

    /// Find an existing DM by inbox ID.
    pub fn find_dm(&self, inbox_id: &str) -> Result<Option<Conversation>> {
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

    fn dm_by_address(&self, address: &str, opts: &CreateDmOptions) -> Result<Conversation> {
        let c = to_c_string(address)?;
        let ds = opts.disappearing.unwrap_or_default();
        let mut out: *mut xmtp_sys::XmtpFfiConversation = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_create_dm(
                self.handle.as_ptr(),
                c.as_ptr(),
                IdentifierKind::Ethereum as i32,
                ds.from_ns,
                ds.in_ns,
                &raw mut out,
            )
        };
        error::check(rc)?;
        Conversation::from_raw(out)
    }

    fn dm_by_inbox_id(&self, inbox_id: &str, opts: &CreateDmOptions) -> Result<Conversation> {
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

    /// List only group conversations.
    pub fn list_groups(&self) -> Result<Vec<Conversation>> {
        self.list_conversations(&ListConversationsOptions {
            conversation_type: Some(ConversationType::Group),
            ..Default::default()
        })
    }

    /// List only DM conversations.
    pub fn list_dms(&self) -> Result<Vec<Conversation>> {
        self.list_conversations(&ListConversationsOptions {
            conversation_type: Some(ConversationType::Dm),
            ..Default::default()
        })
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
