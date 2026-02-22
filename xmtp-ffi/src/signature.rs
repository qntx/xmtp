//! Signature request creation, application, and installation key signing.

use std::ffi::c_char;
use std::sync::Arc;

use crate::ffi::*;

// ---------------------------------------------------------------------------
// Signature request creation
// ---------------------------------------------------------------------------

/// Create an inbox registration signature request (if needed).
/// Returns null via `out` if no signature is needed.
/// Caller must free with [`xmtp_signature_request_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_create_inbox_signature_request(
    client: *const XmtpClient,
    out: *mut *mut XmtpSignatureRequest,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let Some(sig_req) = c.inner.identity().signature_request() else {
            unsafe { *out = std::ptr::null_mut(); }
            return Ok(());
        };
        let handle = XmtpSignatureRequest {
            request: Arc::new(tokio::sync::Mutex::new(sig_req)),
            scw_verifier: c.inner.scw_verifier().clone(),
        };
        unsafe { write_out(out, handle)? };
        Ok(())
    })
}

/// Create a signature request to add a new identifier.
/// Caller must free with [`xmtp_signature_request_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_add_identifier_signature_request(
    client: *const XmtpClient,
    identifier: *const c_char,
    identifier_kind: i32,
    out: *mut *mut XmtpSignatureRequest,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ident = unsafe { parse_identifier(identifier, identifier_kind)? };
        let sig_req = c.inner.identity_updates().associate_identity(ident).await?;
        let handle = XmtpSignatureRequest {
            request: Arc::new(tokio::sync::Mutex::new(sig_req)),
            scw_verifier: c.inner.scw_verifier().clone(),
        };
        unsafe { write_out(out, handle)? };
        Ok(())
    })
}

/// Create a signature request to revoke an identifier.
/// Caller must free with [`xmtp_signature_request_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_revoke_identifier_signature_request(
    client: *const XmtpClient,
    identifier: *const c_char,
    identifier_kind: i32,
    out: *mut *mut XmtpSignatureRequest,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ident = unsafe { parse_identifier(identifier, identifier_kind)? };
        let sig_req = c.inner.identity_updates().revoke_identities(vec![ident]).await?;
        let handle = XmtpSignatureRequest {
            request: Arc::new(tokio::sync::Mutex::new(sig_req)),
            scw_verifier: c.inner.scw_verifier().clone(),
        };
        unsafe { write_out(out, handle)? };
        Ok(())
    })
}

/// Create a signature request to revoke all other installations.
/// Returns null via `out` if there are no other installations.
/// Caller must free with [`xmtp_signature_request_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_revoke_all_other_installations(
    client: *const XmtpClient,
    out: *mut *mut XmtpSignatureRequest,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let my_id = c.inner.installation_public_key();
        let inbox_state = c.inner.inbox_state(true).await?;
        let other_ids: Vec<Vec<u8>> = inbox_state
            .installation_ids()
            .into_iter()
            .filter(|id| id != my_id)
            .collect();
        if other_ids.is_empty() {
            unsafe { *out = std::ptr::null_mut(); }
            return Ok(());
        }
        let sig_req = c.inner.identity_updates().revoke_installations(other_ids).await?;
        let handle = XmtpSignatureRequest {
            request: Arc::new(tokio::sync::Mutex::new(sig_req)),
            scw_verifier: c.inner.scw_verifier().clone(),
        };
        unsafe { write_out(out, handle)? };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Signature request operations
// ---------------------------------------------------------------------------

/// Get the human-readable signature text. Caller must free with [`xmtp_free_string`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_signature_request_text(
    req: *const XmtpSignatureRequest,
) -> *mut c_char {
    match unsafe { ref_from(req) } {
        Ok(r) => {
            let text = runtime().block_on(async {
                r.request.lock().await.signature_text()
            });
            to_c_string(&text)
        }
        Err(_) => std::ptr::null_mut(),
    }
}

/// Add an ECDSA signature to the request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_signature_request_add_ecdsa(
    req: *const XmtpSignatureRequest,
    signature_bytes: *const u8,
    signature_len: i32,
) -> i32 {
    catch_async(|| async {
        let r = unsafe { ref_from(req)? };
        if signature_bytes.is_null() || signature_len <= 0 {
            return Err("null or empty signature".into());
        }
        let sig = unsafe { std::slice::from_raw_parts(signature_bytes, signature_len as usize) };
        let signature = xmtp_id::associations::unverified::UnverifiedSignature::new_recoverable_ecdsa(sig.to_vec());
        let mut req_lock = r.request.lock().await;
        req_lock
            .add_signature(signature, &r.scw_verifier)
            .await?;
        Ok(())
    })
}

/// Apply a signature request to the client.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_apply_signature_request(
    client: *const XmtpClient,
    req: *const XmtpSignatureRequest,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        let r = unsafe { ref_from(req)? };
        let req_clone = r.request.lock().await.clone();
        c.inner.identity_updates().apply_signature_request(req_clone).await?;
        Ok(())
    })
}

/// Free a signature request handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_signature_request_free(req: *mut XmtpSignatureRequest) {
    if !req.is_null() {
        drop(unsafe { Box::from_raw(req) });
    }
}

// ---------------------------------------------------------------------------
// Revoke specific installations
// ---------------------------------------------------------------------------

/// Create a signature request to revoke specific installations by their IDs.
/// `installation_ids` is an array of byte arrays, each `id_len` bytes long.
/// Caller must free with [`xmtp_signature_request_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_revoke_installations_signature_request(
    client: *const XmtpClient,
    installation_ids: *const *const u8,
    id_lengths: *const i32,
    count: i32,
    out: *mut *mut XmtpSignatureRequest,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() || installation_ids.is_null() || id_lengths.is_null() || count <= 0 {
            return Err("null pointer or invalid count".into());
        }
        let mut ids = Vec::with_capacity(count as usize);
        for i in 0..count as usize {
            let len = unsafe { *id_lengths.add(i) } as usize;
            let ptr = unsafe { *installation_ids.add(i) };
            if ptr.is_null() {
                return Err("null installation ID pointer".into());
            }
            ids.push(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec());
        }
        let sig_req = c.inner.identity_updates().revoke_installations(ids).await?;
        let handle = XmtpSignatureRequest {
            request: Arc::new(tokio::sync::Mutex::new(sig_req)),
            scw_verifier: c.inner.scw_verifier().clone(),
        };
        unsafe { write_out(out, handle)? };
        Ok(())
    })
}

/// Create a signature request to change the recovery identifier.
/// Caller must free with [`xmtp_signature_request_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_change_recovery_identifier_signature_request(
    client: *const XmtpClient,
    new_identifier: *const c_char,
    identifier_kind: i32,
    out: *mut *mut XmtpSignatureRequest,
) -> i32 {
    catch_async(|| async {
        let c = unsafe { ref_from(client)? };
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let ident = unsafe { parse_identifier(new_identifier, identifier_kind)? };
        let sig_req = c.inner
            .identity_updates()
            .change_recovery_identifier(ident)
            .await?;
        let handle = XmtpSignatureRequest {
            request: Arc::new(tokio::sync::Mutex::new(sig_req)),
            scw_verifier: c.inner.scw_verifier().clone(),
        };
        unsafe { write_out(out, handle)? };
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Installation key signing
// ---------------------------------------------------------------------------

/// Sign text with the client's installation key.
/// Writes signature bytes to `out` and length to `out_len`.
/// Caller must free `out` with [`xmtp_free_bytes`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_client_sign_with_installation_key(
    client: *const XmtpClient,
    text: *const c_char,
    out: *mut *mut u8,
    out_len: *mut i32,
) -> i32 {
    catch(|| {
        let c = unsafe { ref_from(client)? };
        if out.is_null() || out_len.is_null() {
            return Err("null output pointer".into());
        }
        let text = unsafe { c_str_to_string(text)? };
        let sig = c.inner.context.sign_with_public_context(text)?;
        let len = sig.len();
        let ptr = sig.leak().as_mut_ptr();
        unsafe {
            *out = ptr;
            *out_len = len as i32;
        }
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse an identifier from C string + kind.
unsafe fn parse_identifier(
    s: *const c_char,
    kind: i32,
) -> Result<xmtp_id::associations::Identifier, Box<dyn std::error::Error>> {
    let val = unsafe { c_str_to_string(s)? };
    match kind {
        0 => Ok(xmtp_id::associations::Identifier::eth(val)?),
        1 => Ok(xmtp_id::associations::Identifier::passkey_str(&val, None)?),
        _ => Err("invalid identifier kind".into()),
    }
}
