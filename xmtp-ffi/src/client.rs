//! Client lifecycle, properties, and consent operations.

use std::ffi::c_char;
use std::sync::Arc;

use crate::ffi::*;

// ---------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------

/// Options for creating a client. All string fields are borrowed (not freed by this library).
#[repr(C)]
pub struct XmtpClientOptions {
    /// gRPC host URL (required).
    pub host: *const c_char,
    /// Whether the connection is TLS-secured.
    pub is_secure: i32,
    /// Path to the SQLite database file. Null = ephemeral.
    pub db_path: *const c_char,
    /// 32-byte encryption key for the database. Null = unencrypted.
    pub encryption_key: *const u8,
    /// Inbox ID (required).
    pub inbox_id: *const c_char,
    /// Account identifier string (required).
    pub account_identifier: *const c_char,
    /// Identifier kind: 0 = Ethereum, 1 = Passkey.
    pub identifier_kind: i32,
    /// Optional auth handle for gateway authentication. Null = no auth.
    pub auth_handle: *const XmtpAuthHandle,
}

/// Create a new XMTP client. Caller must free with [`xmtp_client_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create(
    opts: *const XmtpClientOptions,
    out: *mut *mut XmtpClient,
) -> i32 {
    catch_async(|| async {
        let opts = unsafe { ref_from(opts)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }

        let host = unsafe { c_str_to_string(opts.host)? };
        let inbox_id = unsafe { c_str_to_string(opts.inbox_id)? };
        let ident_str = unsafe { c_str_to_string(opts.account_identifier)? };
        let ident_str_saved = ident_str.clone();
        let is_secure = opts.is_secure != 0;

        // Build identifier
        let identifier = match opts.identifier_kind {
            0 => xmtp_id::associations::Identifier::eth(ident_str)?,
            1 => xmtp_id::associations::Identifier::passkey_str(&ident_str_saved, None)?,
            _ => return Err("invalid identifier_kind".into()),
        };

        // Build API backend
        let mut backend = xmtp_api_d14n::MessageBackendBuilder::default();
        backend.v3_host(&host).is_secure(is_secure);

        // Optional gateway auth handle
        if !opts.auth_handle.is_null() {
            let ah = unsafe { ref_from(opts.auth_handle)? };
            backend.maybe_auth_handle(Some(ah.inner.clone()));
        }

        // Build database
        let db_path = unsafe { c_str_to_option(opts.db_path)? };
        let db_builder = if let Some(path) = db_path {
            xmtp_db::NativeDb::builder().persistent(path)
        } else {
            xmtp_db::NativeDb::builder().ephemeral()
        };

        let db = if !opts.encryption_key.is_null() {
            let key_slice = unsafe { std::slice::from_raw_parts(opts.encryption_key, 32) };
            let key: xmtp_db::EncryptionKey = key_slice
                .try_into()
                .map_err(|_| "encryption key must be 32 bytes")?;
            db_builder.key(key).build()?
        } else {
            db_builder.build_unencrypted()?
        };

        let store = xmtp_db::EncryptedMessageStore::new(db)?;

        // Identity strategy
        let identity_strategy = xmtp_mls::identity::IdentityStrategy::new(
            inbox_id, identifier, 1, // nonce
            None,
        );

        // Cursor store
        let cursor_store = xmtp_mls::cursor_store::SqliteCursorStore::new(store.db());
        backend.cursor_store(cursor_store);

        let api_client = backend.clone().build()?;
        let sync_api_client = backend.build()?;

        // Build client (must call enable_api_stats + enable_api_debug_wrapper
        // to produce the MlsContext type alias)
        let client = xmtp_mls::Client::builder(identity_strategy)
            .api_clients(api_client, sync_api_client)
            .enable_api_stats()?
            .enable_api_debug_wrapper()?
            .with_remote_verifier()?
            .store(store)
            .default_mls_store()?
            .build()
            .await?;

        unsafe {
            write_out(
                out,
                XmtpClient {
                    inner: Arc::new(client),
                    account_identifier: ident_str_saved,
                },
            )?
        };
        Ok(())
    })
}

free_opaque!(xmtp_client_free, XmtpClient);

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

/// Get the client's inbox ID. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_inbox_id(client: *const XmtpClient) -> *mut c_char {
    match unsafe { ref_from(client) } {
        Ok(c) => to_c_string(c.inner.inbox_id()),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the client's installation ID (hex). Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_installation_id(client: *const XmtpClient) -> *mut c_char {
    match unsafe { ref_from(client) } {
        Ok(c) => to_c_string(&hex::encode(c.inner.installation_public_key())),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Check if the client identity is registered. Returns 1 = yes, 0 = no, -1 = error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_is_registered(client: *const XmtpClient) -> i32 {
    match unsafe { ref_from(client) } {
        Ok(c) => i32::from(c.inner.identity().is_ready()),
        Err(_) => -1,
    }
}

/// Register the client identity with an optional signature request.
/// Pass null for `sig_req` if no external signature is needed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_register_identity(
    client: *const XmtpClient,
    sig_req: *const XmtpSignatureRequest,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if !sig_req.is_null() {
            let sr = unsafe { ref_from(sig_req)? };
            let req = sr.request.lock().await.clone();
            c.inner.register_identity(req).await?;
        } else {
            // Create a default empty signature request for registration
            let req = c
                .inner
                .identity()
                .signature_request()
                .ok_or("no signature request available for registration")?;
            c.inner.register_identity(req).await?;
        }
        Ok(())
    })
}

/// Check which identifiers can receive messages.
/// `identifiers` is an array of C strings, `kinds` is a parallel array of identifier kinds.
/// Results are written to `out_results` (1 = can message, 0 = cannot).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_can_message(
    client: *const XmtpClient,
    identifiers: *const *const c_char,
    kinds: *const i32,
    count: i32,
    out_results: *mut i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if identifiers.is_null() || kinds.is_null() || out_results.is_null() || count <= 0 {
            return Err("null pointer or invalid count".into());
        }

        let mut idents = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let s = unsafe { c_str_to_string(*identifiers.add(i))? };
            let kind = unsafe { *kinds.add(i) };
            let ident = match kind {
                0 => xmtp_id::associations::Identifier::eth(s)?,
                1 => xmtp_id::associations::Identifier::passkey_str(&s, None)?,
                _ => return Err("invalid identifier kind".into()),
            };
            idents.push(ident);
        }

        let results = c.inner.can_message(&idents).await?;
        for (i, ident) in idents.iter().enumerate() {
            let can = results.get(ident).copied().unwrap_or(false);
            unsafe {
                *out_results.add(i) = i32::from(can);
            }
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Database lifecycle
// ---------------------------------------------------------------------------

/// Release the database connection pool.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_release_db_connection(client: *const XmtpClient) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        c.inner.release_db_connection()?;
        Ok(())
    })
}

/// Reconnect to the database.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_reconnect_db(client: *const XmtpClient) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        c.inner.reconnect_db()?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Consent
// ---------------------------------------------------------------------------

/// Set consent states for multiple entities.
/// `entity_types`: 0 = GroupId, 1 = InboxId.
/// `states`: 0 = Unknown, 1 = Allowed, 2 = Denied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_set_consent_states(
    client: *const XmtpClient,
    entity_types: *const i32,
    states: *const i32,
    entities: *const *const c_char,
    count: i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if entity_types.is_null() || states.is_null() || entities.is_null() || count <= 0 {
            return Err("null pointer or invalid count".into());
        }

        let now_ns = xmtp_common::time::now_ns() as i64;
        let mut records = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let entity = unsafe { c_str_to_string(*entities.add(i))? };
            let entity_type = i32_to_consent_type(unsafe { *entity_types.add(i) })?;
            let state = i32_to_consent_state(unsafe { *states.add(i) })?;
            records.push(xmtp_db::consent_record::StoredConsentRecord {
                entity_type,
                state,
                entity,
                consented_at_ns: now_ns,
            });
        }
        c.inner.set_consent_states(&records).await?;
        Ok(())
    })
}

/// Get consent state for a single entity. Result written to `out_state`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_get_consent_state(
    client: *const XmtpClient,
    entity_type: i32,
    entity: *const c_char,
    out_state: *mut i32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let entity = unsafe { c_str_to_string(entity)? };
        if out_state.is_null() {
            return Err("null output pointer".into());
        }
        let et = i32_to_consent_type(entity_type)?;
        let state = c.inner.get_consent_state(et, entity).await?;
        unsafe {
            *out_state = consent_state_to_i32(state);
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Inbox state
// ---------------------------------------------------------------------------

/// Get the inbox state for this client as a single-element list.
/// Caller must free with [`xmtp_inbox_state_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_inbox_state(
    client: *const XmtpClient,
    refresh: i32,
    out: *mut *mut XmtpInboxStateList,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let state = c.inner.inbox_state(refresh != 0).await?;
        let item = association_state_to_item(&state);
        unsafe { write_out(out, XmtpInboxStateList { items: vec![item] })? };
        Ok(())
    })
}

/// Convert an AssociationState to an XmtpInboxStateItem.
fn association_state_to_item(s: &xmtp_id::associations::AssociationState) -> XmtpInboxStateItem {
    let inbox_id = s.inbox_id().to_string();
    let recovery = s.recovery_identifier().to_string();
    let identifiers: Vec<String> = s.identifiers().into_iter().map(|i| i.to_string()).collect();
    let installations: Vec<String> = s.installation_ids().into_iter().map(hex::encode).collect();
    let mut ident_count: i32 = 0;
    let ident_ptrs = string_vec_to_c(identifiers, &mut ident_count);
    let mut inst_count: i32 = 0;
    let inst_ptrs = string_vec_to_c(installations, &mut inst_count);
    XmtpInboxStateItem {
        inbox_id: to_c_string(&inbox_id),
        recovery_identifier: to_c_string(&recovery),
        identifiers: ident_ptrs,
        identifiers_count: ident_count,
        installation_ids: inst_ptrs,
        installation_ids_count: inst_count,
    }
}

// ---------------------------------------------------------------------------
// Installation ID (raw bytes)
// ---------------------------------------------------------------------------

/// Get the client's installation ID as raw bytes.
/// Writes length to `out_len`. Caller must free with [`xmtp_free_bytes`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_installation_id_bytes(
    client: *const XmtpClient,
    out_len: *mut i32,
) -> *mut u8 {
    if out_len.is_null() {
        return std::ptr::null_mut();
    }
    match unsafe { ref_from(client) } {
        Ok(c) => {
            let id = c.inner.installation_public_key();
            let len = id.len();
            let mut copy = id.to_vec();
            let ptr = copy.as_mut_ptr();
            std::mem::forget(copy);
            unsafe {
                *out_len = len as i32;
            }
            ptr
        }
        Err(_) => {
            unsafe {
                *out_len = 0;
            }
            std::ptr::null_mut()
        }
    }
}

// ---------------------------------------------------------------------------
// Verify signature
// ---------------------------------------------------------------------------

/// Verify a signature produced by `xmtp_client_sign_with_installation_key`.
/// Returns 0 on success (valid), -1 on error (invalid or bad args).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_verify_signed_with_installation_key(
    client: *const XmtpClient,
    text: *const c_char,
    signature_bytes: *const u8,
    signature_len: i32,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        let text = unsafe { c_str_to_string(text)? };
        if signature_bytes.is_null() || signature_len != 64 {
            return Err("signature must be 64 bytes".into());
        }
        let sig_slice = unsafe { std::slice::from_raw_parts(signature_bytes, 64) };
        let sig: [u8; 64] = sig_slice.try_into().map_err(|_| "bad signature length")?;

        let pub_key = c.inner.installation_public_key();
        let pk: [u8; 32] = pub_key
            .as_slice()
            .try_into()
            .map_err(|_| "bad public key length")?;

        xmtp_id::associations::signature::verify_signed_with_public_context(text, &sig, &pk)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Message-level operations (client-scoped)
// ---------------------------------------------------------------------------

/// Get a message by its hex-encoded ID. Caller must free with [`xmtp_message_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_get_message_by_id(
    client: *const XmtpClient,
    message_id_hex: *const c_char,
    out: *mut *mut XmtpMessage,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let id_hex = unsafe { c_str_to_string(message_id_hex)? };
        let id_bytes = hex::decode(&id_hex)?;
        let msg = c.inner.message(id_bytes)?;
        unsafe { write_out(out, XmtpMessage { inner: msg })? };
        Ok(())
    })
}

/// Delete a message by its hex-encoded ID. Returns the number of deleted rows.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_delete_message_by_id(
    client: *const XmtpClient,
    message_id_hex: *const c_char,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        let id_hex = unsafe { c_str_to_string(message_id_hex)? };
        let id_bytes = hex::decode(&id_hex)?;
        c.inner.delete_message(id_bytes)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Version info
// ---------------------------------------------------------------------------

/// Get the libxmtp version string. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub extern "C" fn xmtp_libxmtp_version() -> *mut c_char {
    to_c_string(env!("CARGO_PKG_VERSION"))
}

// ---------------------------------------------------------------------------
// API Statistics
// ---------------------------------------------------------------------------

/// Get MLS API call statistics. Writes to `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_api_statistics(
    client: *const XmtpClient,
    out: *mut XmtpApiStats,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let stats = c.inner.api_stats();
        unsafe {
            *out = XmtpApiStats {
                upload_key_package: stats.upload_key_package.get_count() as i64,
                fetch_key_package: stats.fetch_key_package.get_count() as i64,
                send_group_messages: stats.send_group_messages.get_count() as i64,
                send_welcome_messages: stats.send_welcome_messages.get_count() as i64,
                query_group_messages: stats.query_group_messages.get_count() as i64,
                query_welcome_messages: stats.query_welcome_messages.get_count() as i64,
                subscribe_messages: stats.subscribe_messages.get_count() as i64,
                subscribe_welcomes: stats.subscribe_welcomes.get_count() as i64,
                publish_commit_log: stats.publish_commit_log.get_count() as i64,
                query_commit_log: stats.query_commit_log.get_count() as i64,
                get_newest_group_message: stats.get_newest_group_message.get_count() as i64,
            };
        }
        Ok(())
    })
}

/// Get identity API call statistics. Writes to `out`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_api_identity_statistics(
    client: *const XmtpClient,
    out: *mut XmtpIdentityStats,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let stats = c.inner.identity_api_stats();
        unsafe {
            *out = XmtpIdentityStats {
                publish_identity_update: stats.publish_identity_update.get_count() as i64,
                get_identity_updates_v2: stats.get_identity_updates_v2.get_count() as i64,
                get_inbox_ids: stats.get_inbox_ids.get_count() as i64,
                verify_smart_contract_wallet_signature: stats
                    .verify_smart_contract_wallet_signature
                    .get_count() as i64,
            };
        }
        Ok(())
    })
}

/// Get aggregate statistics as a debug string. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_api_aggregate_statistics(
    client: *const XmtpClient,
) -> *mut c_char {
    match unsafe { ref_from(client) } {
        Ok(c) => {
            let api = c.inner.api_stats();
            let identity = c.inner.identity_api_stats();
            let aggregate = xmtp_proto::api_client::AggregateStats { mls: api, identity };
            to_c_string(&format!("{:?}", aggregate))
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Clear all API call statistics.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_clear_all_statistics(client: *const XmtpClient) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        c.inner.clear_stats();
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Inbox ID lookup (client-bound)
// ---------------------------------------------------------------------------

/// Look up an inbox ID by account identifier using the client's connection.
/// Returns null if not found. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_get_inbox_id_by_identifier(
    client: *const XmtpClient,
    identifier: *const c_char,
    out: *mut *mut c_char,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ident_str = unsafe { c_str_to_string(identifier)? };
        let ident = xmtp_id::associations::Identifier::eth(&ident_str)?;
        let conn = c.inner.context.store().db();
        let inbox_id = c.inner.find_inbox_id_from_identifier(&conn, ident).await?;
        unsafe {
            *out = match inbox_id {
                Some(id) => to_c_string(&id),
                None => std::ptr::null_mut(),
            };
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Batch inbox state queries
// ---------------------------------------------------------------------------

/// Fetch inbox states for multiple inbox IDs.
/// `inbox_ids` is a null-terminated array of C strings with `count` elements.
/// Returns an opaque `XmtpInboxStateList` via `out`. Caller must free with [`xmtp_inbox_state_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_fetch_inbox_states(
    client: *const XmtpClient,
    inbox_ids: *const *const c_char,
    count: i32,
    refresh_from_network: i32,
    out: *mut *mut XmtpInboxStateList,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let mut ids = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let ptr = unsafe { *inbox_ids.add(i) };
            ids.push(unsafe { c_str_to_string(ptr)? });
        }
        let states = c
            .inner
            .inbox_addresses(
                refresh_from_network != 0,
                ids.iter().map(|s| s.as_str()).collect(),
            )
            .await?;
        let items: Vec<XmtpInboxStateItem> = states.iter().map(association_state_to_item).collect();
        unsafe { write_out(out, XmtpInboxStateList { items })? };
        Ok(())
    })
}

/// Get the number of inbox states in the list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_list_len(list: *const XmtpInboxStateList) -> i32 {
    match unsafe { ref_from(list) } {
        Ok(l) => l.items.len() as i32,
        Err(_) => 0,
    }
}

/// Get inbox ID at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_inbox_id(
    list: *const XmtpInboxStateList,
    index: i32,
) -> *mut c_char {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return std::ptr::null_mut(),
    };
    match l.items.get(index as usize) {
        Some(item) if !item.inbox_id.is_null() => {
            let s = unsafe { std::ffi::CStr::from_ptr(item.inbox_id) };
            to_c_string(s.to_str().unwrap_or(""))
        }
        _ => std::ptr::null_mut(),
    }
}

/// Get recovery identifier at index. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_recovery_identifier(
    list: *const XmtpInboxStateList,
    index: i32,
) -> *mut c_char {
    let l = match unsafe { ref_from(list) } {
        Ok(l) => l,
        Err(_) => return std::ptr::null_mut(),
    };
    match l.items.get(index as usize) {
        Some(item) if !item.recovery_identifier.is_null() => {
            let s = unsafe { std::ffi::CStr::from_ptr(item.recovery_identifier) };
            to_c_string(s.to_str().unwrap_or(""))
        }
        _ => std::ptr::null_mut(),
    }
}

/// Get identifiers array at index. Returns a borrowed pointer; do NOT free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_identifiers(
    list: *const XmtpInboxStateList,
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
        Some(item) => {
            unsafe { *out_count = item.identifiers_count };
            item.identifiers as *const *mut c_char
        }
        None => {
            unsafe { *out_count = 0 };
            std::ptr::null()
        }
    }
}

/// Get installation IDs (hex) at index. Returns a borrowed pointer; do NOT free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_installation_ids(
    list: *const XmtpInboxStateList,
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
        Some(item) => {
            unsafe { *out_count = item.installation_ids_count };
            item.installation_ids as *const *mut c_char
        }
        None => {
            unsafe { *out_count = 0 };
            std::ptr::null()
        }
    }
}

/// Free an inbox state list (including all owned strings).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_list_free(list: *mut XmtpInboxStateList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for item in &l.items {
        if !item.inbox_id.is_null() {
            drop(unsafe { std::ffi::CString::from_raw(item.inbox_id) });
        }
        if !item.recovery_identifier.is_null() {
            drop(unsafe { std::ffi::CString::from_raw(item.recovery_identifier) });
        }
        free_c_string_array(item.identifiers, item.identifiers_count);
        free_c_string_array(item.installation_ids, item.installation_ids_count);
    }
}

// ---------------------------------------------------------------------------
// Gateway Auth
// ---------------------------------------------------------------------------

/// Create a new gateway auth handle. Caller must free with [`xmtp_auth_handle_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_auth_handle_create(out: *mut *mut XmtpAuthHandle) -> i32 {
    catch(|| {
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let handle = XmtpAuthHandle {
            inner: xmtp_api_d14n::AuthHandle::new(),
        };
        unsafe { write_out(out, handle)? };
        Ok(())
    })
}

/// Set a credential on an auth handle.
/// `name` is an optional HTTP header name (null = "authorization").
/// `value` is the header value (required).
/// `expires_at_seconds` is the Unix timestamp when the credential expires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_auth_handle_set(
    handle: *const XmtpAuthHandle,
    name: *const c_char,
    value: *const c_char,
    expires_at_seconds: i64,
) -> i32 {
    catch_async(|| async {
        let h = unsafe { ref_from(handle)? };
        let value_str = unsafe { c_str_to_string(value)? };
        let name_opt = unsafe { c_str_to_option(name)? };
        let header_name = if let Some(n) = name_opt {
            Some(
                n.parse::<http::header::HeaderName>()
                    .map_err(|_| "invalid header name")?,
            )
        } else {
            None
        };
        let header_value = value_str
            .parse::<http::header::HeaderValue>()
            .map_err(|_| "invalid header value")?;
        let credential =
            xmtp_api_d14n::Credential::new(header_name, header_value, expires_at_seconds);
        h.inner.set(credential).await;
        Ok(())
    })
}

/// Get the unique ID of an auth handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_auth_handle_id(handle: *const XmtpAuthHandle) -> usize {
    match unsafe { ref_from(handle) } {
        Ok(h) => h.inner.id(),
        Err(_) => 0,
    }
}

free_opaque!(xmtp_auth_handle_free, XmtpAuthHandle);

// ---------------------------------------------------------------------------
// Inbox updates count
// ---------------------------------------------------------------------------

/// Fetch the number of identity updates for multiple inbox IDs.
/// Caller must free the result with [`xmtp_inbox_update_count_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_fetch_inbox_updates_count(
    client: *const XmtpClient,
    inbox_ids: *const *const c_char,
    inbox_ids_count: i32,
    refresh: i32,
    out: *mut *mut XmtpInboxUpdateCountList,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ids = unsafe { collect_strings(inbox_ids, inbox_ids_count)? };
        let id_refs: Vec<&str> = ids.iter().map(|s: &String| s.as_str()).collect();
        let counts = c
            .inner
            .fetch_inbox_updates_count(refresh != 0, id_refs)
            .await?;
        let items: Vec<XmtpInboxUpdateCount> = counts
            .into_iter()
            .map(|(id, cnt)| XmtpInboxUpdateCount {
                inbox_id: to_c_string(&id),
                count: cnt,
            })
            .collect();
        let list = Box::new(XmtpInboxUpdateCountList { items });
        unsafe { *out = Box::into_raw(list) };
        Ok(())
    })
}

/// Fetch the number of identity updates for the client's own inbox.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_fetch_own_inbox_updates_count(
    client: *const XmtpClient,
    refresh: i32,
    out: *mut u32,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let count = c.inner.fetch_own_inbox_updates_count(refresh != 0).await?;
        unsafe { *out = count };
        Ok(())
    })
}

ffi_list_len!(xmtp_inbox_update_count_list_len, XmtpInboxUpdateCountList);
ffi_list_get!(
    xmtp_inbox_update_count_list_get,
    XmtpInboxUpdateCountList,
    XmtpInboxUpdateCount
);

/// Free an inbox update count list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_update_count_list_free(list: *mut XmtpInboxUpdateCountList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for item in &l.items {
        free_c_strings!(item, inbox_id);
    }
}

// ---------------------------------------------------------------------------
// Key package statuses
// ---------------------------------------------------------------------------

/// Fetch key package statuses for a list of installation IDs (hex-encoded).
/// Caller must free with [`xmtp_key_package_status_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_fetch_key_package_statuses(
    client: *const XmtpClient,
    installation_ids: *const *const c_char,
    installation_ids_count: i32,
    out: *mut *mut XmtpKeyPackageStatusList,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let id_strs = unsafe { collect_strings(installation_ids, installation_ids_count)? };
        let id_bytes: Vec<Vec<u8>> = id_strs
            .iter()
            .map(|s| hex::decode(s).map_err(|e| e.into()))
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

        let results = c
            .inner
            .get_key_packages_for_installation_ids(id_bytes)
            .await?;

        let items: Vec<XmtpKeyPackageStatus> = results
            .into_iter()
            .map(|(id, result)| match result {
                Ok(kp) => {
                    let lifetime = kp.life_time();
                    XmtpKeyPackageStatus {
                        installation_id: to_c_string(&hex::encode(&id)),
                        valid: 1,
                        not_before: lifetime.as_ref().map(|l| l.not_before).unwrap_or(0),
                        not_after: lifetime.as_ref().map(|l| l.not_after).unwrap_or(0),
                        validation_error: std::ptr::null_mut(),
                    }
                }
                Err(e) => XmtpKeyPackageStatus {
                    installation_id: to_c_string(&hex::encode(&id)),
                    valid: 0,
                    not_before: 0,
                    not_after: 0,
                    validation_error: to_c_string(&e.to_string()),
                },
            })
            .collect();

        let list = Box::new(XmtpKeyPackageStatusList { items });
        unsafe { *out = Box::into_raw(list) };
        Ok(())
    })
}

ffi_list_len!(xmtp_key_package_status_list_len, XmtpKeyPackageStatusList);
ffi_list_get!(
    xmtp_key_package_status_list_get,
    XmtpKeyPackageStatusList,
    XmtpKeyPackageStatus
);

/// Free a key package status list.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_key_package_status_list_free(list: *mut XmtpKeyPackageStatusList) {
    if list.is_null() {
        return;
    }
    let l = unsafe { Box::from_raw(list) };
    for item in &l.items {
        free_c_strings!(item, installation_id, validation_error);
    }
}

// ---------------------------------------------------------------------------
// Client property getters
// ---------------------------------------------------------------------------

/// Get the account identifier string used to create this client.
/// Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_account_identifier(client: *const XmtpClient) -> *mut c_char {
    match unsafe { ref_from(client) } {
        Ok(c) => to_c_string(&c.account_identifier),
        Err(_) => std::ptr::null_mut(),
    }
}
