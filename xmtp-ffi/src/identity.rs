//! Standalone identity query functions (no client handle required).

use std::ffi::c_char;

use crate::ffi::*;

/// Generate an inbox ID from an identifier. Caller must free with [`xmtp_free_string`].
/// `nonce` defaults to 1 if 0 is passed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_generate_inbox_id(
    identifier: *const c_char,
    identifier_kind: i32,
    nonce: u64,
) -> *mut c_char {
    let result: Result<String, Box<dyn std::error::Error>> = (|| {
        let val = unsafe { c_str_to_string(identifier)? };
        let ident = match identifier_kind {
            0 => xmtp_id::associations::Identifier::eth(val)?,
            1 => xmtp_id::associations::Identifier::passkey_str(&val, None)?,
            _ => return Err("invalid identifier kind".into()),
        };
        let n = if nonce == 0 { 1 } else { nonce };
        Ok(ident.inbox_id(n)?)
    })();

    match result {
        Ok(id) => to_c_string(&id),
        Err(e) => {
            set_last_error(e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Get the inbox ID for an identifier by querying the network.
/// `api_url` is the gRPC host, `is_secure` controls TLS.
/// Writes the inbox ID to `out` (caller must free with [`xmtp_free_string`]).
/// Writes null to `out` if no inbox ID is found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xmtp_get_inbox_id_for_identifier(
    api_url: *const c_char,
    is_secure: i32,
    identifier: *const c_char,
    identifier_kind: i32,
    out: *mut *mut c_char,
) -> i32 {
    catch_async(|| async {
        if out.is_null() {
            return Err("null output pointer".into());
        }
        let url = unsafe { c_str_to_string(api_url)? };
        let val = unsafe { c_str_to_string(identifier)? };
        let ident = match identifier_kind {
            0 => xmtp_id::associations::Identifier::eth(val)?,
            1 => xmtp_id::associations::Identifier::passkey_str(&val, None)?,
            _ => return Err("invalid identifier kind".into()),
        };

        let mut backend = xmtp_api_d14n::MessageBackendBuilder::default();
        backend.v3_host(&url).is_secure(is_secure != 0);
        let api = backend.build()?;

        use xmtp_api::ApiClientWrapper;
        let api_wrapper = ApiClientWrapper::new(api, Default::default());

        let inbox_id = api_wrapper
            .get_inbox_ids(vec![ident.into()])
            .await?
            .into_values()
            .next();

        unsafe {
            *out = match inbox_id {
                Some(id) => to_c_string(&id),
                None => std::ptr::null_mut(),
            };
        }
        Ok(())
    })
}
