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
        let is_secure = opts.is_secure != 0;

        // Build identifier
        let identifier = match opts.identifier_kind {
            0 => xmtp_id::associations::Identifier::eth(ident_str)?,
            1 => xmtp_id::associations::Identifier::passkey_str(&ident_str, None)?,
            _ => return Err("invalid identifier_kind".into()),
        };

        // Build API backend
        let mut backend = xmtp_api_d14n::MessageBackendBuilder::default();
        backend.v3_host(&host).is_secure(is_secure);

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
            inbox_id,
            identifier,
            1, // nonce
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

        unsafe { write_out(out, XmtpClient { inner: Arc::new(client) })? };
        Ok(())
    })
}

/// Free a client handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_free(client: *mut XmtpClient) {
    if !client.is_null() {
        drop(unsafe { Box::from_raw(client) });
    }
}

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
            let req = c.inner.identity().signature_request()
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
            unsafe { *out_results.add(i) = i32::from(can); }
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
            let entity_type = match unsafe { *entity_types.add(i) } {
                0 => xmtp_db::consent_record::ConsentType::ConversationId,
                1 => xmtp_db::consent_record::ConsentType::InboxId,
                _ => return Err("invalid entity type".into()),
            };
            let state = match unsafe { *states.add(i) } {
                0 => xmtp_db::consent_record::ConsentState::Unknown,
                1 => xmtp_db::consent_record::ConsentState::Allowed,
                2 => xmtp_db::consent_record::ConsentState::Denied,
                _ => return Err("invalid consent state".into()),
            };
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
        let et = match entity_type {
            0 => xmtp_db::consent_record::ConsentType::ConversationId,
            1 => xmtp_db::consent_record::ConsentType::InboxId,
            _ => return Err("invalid entity type".into()),
        };
        let state = c.inner.get_consent_state(et, entity).await?;
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

// ---------------------------------------------------------------------------
// Inbox state
// ---------------------------------------------------------------------------

/// Opaque inbox state handle.
pub struct XmtpInboxState {
    inner: xmtp_id::associations::AssociationState,
}

/// Get the inbox state for this client. Caller must free with [`xmtp_inbox_state_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_inbox_state(
    client: *const XmtpClient,
    refresh: i32,
    out: *mut *mut XmtpInboxState,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let state = c.inner.inbox_state(refresh != 0).await?;
        unsafe { write_out(out, XmtpInboxState { inner: state })? };
        Ok(())
    })
}

/// Get the inbox ID from an inbox state. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_inbox_id(state: *const XmtpInboxState) -> *mut c_char {
    match unsafe { ref_from(state) } {
        Ok(s) => to_c_string(s.inner.inbox_id()),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Get the number of installations from an inbox state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_installation_count(state: *const XmtpInboxState) -> i32 {
    match unsafe { ref_from(state) } {
        Ok(s) => s.inner.installation_ids().len() as i32,
        Err(_) => 0,
    }
}

/// Free an inbox state handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_inbox_state_free(state: *mut XmtpInboxState) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state) });
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
            unsafe { *out_len = len as i32; }
            ptr
        }
        Err(_) => {
            unsafe { *out_len = 0; }
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
        let pk: [u8; 32] = pub_key.as_slice().try_into().map_err(|_| "bad public key length")?;

        xmtp_id::associations::signature::verify_signed_with_public_context(text, &sig, &pk)?;
        Ok(())
    })
}
