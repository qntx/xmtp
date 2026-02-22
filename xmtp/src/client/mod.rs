#![allow(unsafe_code)]
//! XMTP client — the primary entry point for the SDK.

mod conversations;
mod identity;

use std::ffi::c_char;
use std::ptr;

use crate::error::{self, Result};
use crate::ffi::{OwnedHandle, read_borrowed_strings, take_c_string, to_c_string};
use crate::types::*;

/// Generate a deterministic inbox ID (no network access required).
pub fn generate_inbox_id(address: &str, kind: IdentifierKind, nonce: u64) -> Result<String> {
    let c = to_c_string(address)?;
    let ptr = unsafe { xmtp_sys::xmtp_generate_inbox_id(c.as_ptr(), kind as i32, nonce) };
    unsafe { take_c_string(ptr) }
}

/// Look up an inbox ID for an identifier on the network.
///
/// Returns `None` if the identifier is not registered.
pub fn get_inbox_id_for_identifier(
    host: &str,
    is_secure: bool,
    address: &str,
    kind: IdentifierKind,
) -> Result<Option<String>> {
    let c_host = to_c_string(host)?;
    let c_addr = to_c_string(address)?;
    let mut out: *mut c_char = ptr::null_mut();
    let rc = unsafe {
        xmtp_sys::xmtp_get_inbox_id_for_identifier(
            c_host.as_ptr(),
            i32::from(is_secure),
            c_addr.as_ptr(),
            kind as i32,
            &mut out,
        )
    };
    error::check(rc)?;
    if out.is_null() {
        Ok(None)
    } else {
        unsafe { take_c_string(out) }.map(Some)
    }
}

/// Initialize the FFI tracing logger. Call at most once.
pub fn init_logger(level: Option<&str>) -> Result<()> {
    let c = level.map(to_c_string).transpose()?;
    error::check(unsafe {
        xmtp_sys::xmtp_init_logger(c.as_ref().map_or(ptr::null(), |s| s.as_ptr()))
    })
}

/// Get the libxmtp version string.
pub fn libxmtp_version() -> Result<String> {
    unsafe { take_c_string(xmtp_sys::xmtp_libxmtp_version()) }
}

/// A connected XMTP client.
#[derive(Debug)]
pub struct Client {
    pub(crate) handle: OwnedHandle<xmtp_sys::XmtpFfiClient>,
}

impl Client {
    /// Create a new [`ClientBuilder`].
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The inbox ID for this client.
    pub fn inbox_id(&self) -> Result<String> {
        unsafe { take_c_string(xmtp_sys::xmtp_client_inbox_id(self.handle.as_ptr())) }
    }

    /// The hex-encoded installation ID.
    pub fn installation_id(&self) -> Result<String> {
        unsafe { take_c_string(xmtp_sys::xmtp_client_installation_id(self.handle.as_ptr())) }
    }

    /// Whether this client is registered on the network.
    #[must_use]
    pub fn is_registered(&self) -> bool {
        unsafe { xmtp_sys::xmtp_client_is_registered(self.handle.as_ptr()) == 1 }
    }

    /// The account identifier used to create this client.
    pub fn account_identifier(&self) -> Result<String> {
        unsafe {
            take_c_string(xmtp_sys::xmtp_client_account_identifier(
                self.handle.as_ptr(),
            ))
        }
    }

    /// The app version string (if set).
    pub fn app_version(&self) -> Result<String> {
        unsafe { take_c_string(xmtp_sys::xmtp_client_app_version(self.handle.as_ptr())) }
    }

    /// Release the database connection pool (for background/suspend).
    pub fn release_db_connection(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_client_release_db_connection(self.handle.as_ptr()) })
    }

    /// Reconnect the database after a prior release.
    pub fn reconnect_db(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_client_reconnect_db(self.handle.as_ptr()) })
    }

    /// Check which identifiers can receive XMTP messages.
    pub fn can_message(&self, identifiers: &[AccountIdentifier]) -> Result<Vec<bool>> {
        if identifiers.is_empty() {
            return Ok(vec![]);
        }
        let (_owned, ptrs, kinds) = crate::ffi::identifiers_to_ffi(identifiers)?;
        let mut results = vec![0i32; identifiers.len()];
        let rc = unsafe {
            xmtp_sys::xmtp_client_can_message(
                self.handle.as_ptr(),
                ptrs.as_ptr(),
                kinds.as_ptr(),
                ptrs.len() as i32,
                results.as_mut_ptr(),
            )
        };
        error::check(rc)?;
        Ok(results.into_iter().map(|r| r == 1).collect())
    }

    /// Look up an inbox ID by identifier using this client's connection.
    pub fn get_inbox_id_by_identifier(
        &self,
        address: &str,
        kind: IdentifierKind,
    ) -> Result<Option<String>> {
        let c = to_c_string(address)?;
        let mut out: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_get_inbox_id_by_identifier(
                self.handle.as_ptr(),
                c.as_ptr(),
                kind as i32,
                &mut out,
            )
        };
        error::check(rc)?;
        if out.is_null() {
            Ok(None)
        } else {
            unsafe { take_c_string(out) }.map(Some)
        }
    }

    /// Installation ID as raw bytes.
    pub fn installation_id_bytes(&self) -> Result<Vec<u8>> {
        let mut len = 0i32;
        let ptr =
            unsafe { xmtp_sys::xmtp_client_installation_id_bytes(self.handle.as_ptr(), &mut len) };
        if ptr.is_null() || len <= 0 {
            return Err(crate::Error::NullPointer);
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec();
        unsafe { xmtp_sys::xmtp_free_bytes(ptr, len) };
        Ok(bytes)
    }

    /// Get this client's inbox state. Set `refresh` to fetch from network.
    pub fn inbox_state(&self, refresh: bool) -> Result<Vec<InboxState>> {
        let mut out: *mut xmtp_sys::XmtpFfiInboxStateList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_inbox_state(self.handle.as_ptr(), i32::from(refresh), &mut out)
        };
        error::check(rc)?;
        let result = read_inbox_state_list(out);
        if !out.is_null() {
            unsafe { xmtp_sys::xmtp_inbox_state_list_free(out) };
        }
        result
    }

    /// Fetch inbox states for multiple inbox IDs.
    pub fn fetch_inbox_states(&self, inbox_ids: &[&str], refresh: bool) -> Result<Vec<InboxState>> {
        let c_ids: Vec<_> = inbox_ids
            .iter()
            .map(|s| to_c_string(s))
            .collect::<Result<_>>()?;
        let c_ptrs: Vec<*const c_char> = c_ids.iter().map(|c| c.as_ptr()).collect();
        let mut out: *mut xmtp_sys::XmtpFfiInboxStateList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_fetch_inbox_states(
                self.handle.as_ptr(),
                c_ptrs.as_ptr(),
                c_ptrs.len() as i32,
                i32::from(refresh),
                &mut out,
            )
        };
        error::check(rc)?;
        let result = read_inbox_state_list(out);
        if !out.is_null() {
            unsafe { xmtp_sys::xmtp_inbox_state_list_free(out) };
        }
        result
    }

    /// Sign text with the client's installation key. Returns signature bytes.
    pub fn sign_with_installation_key(&self, text: &str) -> Result<Vec<u8>> {
        let c = to_c_string(text)?;
        let mut out: *mut u8 = ptr::null_mut();
        let mut out_len = 0i32;
        let rc = unsafe {
            xmtp_sys::xmtp_client_sign_with_installation_key(
                self.handle.as_ptr(),
                c.as_ptr(),
                &mut out,
                &mut out_len,
            )
        };
        error::check(rc)?;
        if out.is_null() || out_len <= 0 {
            return Err(crate::Error::NullPointer);
        }
        let bytes = unsafe { std::slice::from_raw_parts(out, out_len as usize) }.to_vec();
        unsafe { xmtp_sys::xmtp_free_bytes(out, out_len) };
        Ok(bytes)
    }

    /// Verify a signature produced by [`sign_with_installation_key`](Self::sign_with_installation_key).
    pub fn verify_signed_with_installation_key(
        &self,
        text: &str,
        signature: &[u8],
    ) -> Result<bool> {
        let c = to_c_string(text)?;
        let rc = unsafe {
            xmtp_sys::xmtp_client_verify_signed_with_installation_key(
                self.handle.as_ptr(),
                c.as_ptr(),
                signature.as_ptr(),
                signature.len() as i32,
            )
        };
        Ok(rc == 0)
    }

    /// Set consent states for multiple entities.
    pub fn set_consent_states(
        &self,
        entries: &[(ConsentEntityType, ConsentState, &str)],
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let c_strs: Vec<_> = entries
            .iter()
            .map(|(_, _, e)| to_c_string(e))
            .collect::<Result<_>>()?;
        let c_ptrs: Vec<*const c_char> = c_strs.iter().map(|c| c.as_ptr()).collect();
        let types: Vec<i32> = entries.iter().map(|(t, _, _)| *t as i32).collect();
        let states: Vec<i32> = entries.iter().map(|(_, s, _)| *s as i32).collect();
        error::check(unsafe {
            xmtp_sys::xmtp_client_set_consent_states(
                self.handle.as_ptr(),
                types.as_ptr(),
                states.as_ptr(),
                c_ptrs.as_ptr(),
                entries.len() as i32,
            )
        })
    }

    /// Get consent state for a single entity.
    pub fn get_consent_state(
        &self,
        entity_type: ConsentEntityType,
        entity: &str,
    ) -> Result<ConsentState> {
        let c = to_c_string(entity)?;
        let mut out = 0i32;
        let rc = unsafe {
            xmtp_sys::xmtp_client_get_consent_state(
                self.handle.as_ptr(),
                entity_type as i32,
                c.as_ptr(),
                &mut out,
            )
        };
        error::check(rc)?;
        ConsentState::from_ffi(out)
            .ok_or_else(|| crate::Error::Ffi(format!("unknown consent state: {out}")))
    }
}

/// Builder for constructing a [`Client`].
#[derive(Debug, Clone, Default)]
pub struct ClientBuilder {
    env: Env,
    db_path: Option<String>,
    encryption_key: Option<Vec<u8>>,
    app_version: Option<String>,
    api_url: Option<String>,
    gateway_host: Option<String>,
    nonce: u64,
    disable_device_sync: bool,
}

impl ClientBuilder {
    /// Set the network environment (default: [`Env::Dev`]).
    #[must_use]
    pub fn env(mut self, env: Env) -> Self {
        self.env = env;
        self
    }

    /// Set the local database path. `None` = ephemeral (in-memory).
    #[must_use]
    pub fn db_path(mut self, p: impl Into<String>) -> Self {
        self.db_path = Some(p.into());
        self
    }

    /// Set a 32-byte encryption key for the local database.
    #[must_use]
    pub fn encryption_key(mut self, k: Vec<u8>) -> Self {
        self.encryption_key = Some(k);
        self
    }

    /// Override the API URL (instead of deriving from `env`).
    #[must_use]
    pub fn api_url(mut self, u: impl Into<String>) -> Self {
        self.api_url = Some(u.into());
        self
    }

    /// Set the gateway host URL for decentralized API.
    #[must_use]
    pub fn gateway_host(mut self, h: impl Into<String>) -> Self {
        self.gateway_host = Some(h.into());
        self
    }

    /// Set a custom app version string.
    #[must_use]
    pub fn app_version(mut self, v: impl Into<String>) -> Self {
        self.app_version = Some(v.into());
        self
    }

    /// Set the nonce for inbox ID generation (default: 0, which FFI treats as 1).
    #[must_use]
    pub fn nonce(mut self, n: u64) -> Self {
        self.nonce = n;
        self
    }

    /// Disable the device sync worker.
    #[must_use]
    pub fn disable_device_sync(mut self) -> Self {
        self.disable_device_sync = true;
        self
    }

    /// Build the client, registering identity if needed.
    pub fn build(self, signer: &dyn Signer) -> Result<Client> {
        let ident = signer.identifier();
        let host = self.api_url.as_deref().unwrap_or_else(|| self.env.url());
        let c_host = to_c_string(host)?;
        let c_gateway = self.gateway_host.as_deref().map(to_c_string).transpose()?;
        let c_db = self.db_path.as_deref().map(to_c_string).transpose()?;
        let c_account = to_c_string(&ident.address)?;
        let nonce = if self.nonce == 0 { 1 } else { self.nonce };
        let inbox_id = generate_inbox_id(&ident.address, ident.kind, nonce)?;
        let c_inbox = to_c_string(&inbox_id)?;
        let c_app = self.app_version.as_deref().map(to_c_string).transpose()?;

        let opts = xmtp_sys::XmtpFfiClientOptions {
            host: c_host.as_ptr(),
            gateway_host: c_gateway.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
            is_secure: i32::from(host.starts_with("https")),
            db_path: c_db.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
            encryption_key: self
                .encryption_key
                .as_deref()
                .map_or(ptr::null(), <[u8]>::as_ptr),
            inbox_id: c_inbox.as_ptr(),
            account_identifier: c_account.as_ptr(),
            identifier_kind: ident.kind as i32,
            nonce,
            auth_handle: ptr::null(),
            app_version: c_app.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
            device_sync_worker_mode: i32::from(self.disable_device_sync),
            allow_offline: 0,
            client_mode: 0,
            max_db_pool_size: 0,
            min_db_pool_size: 0,
        };

        let mut raw: *mut xmtp_sys::XmtpFfiClient = ptr::null_mut();
        error::check(unsafe { xmtp_sys::xmtp_client_create(&opts, &mut raw) })?;
        let handle = OwnedHandle::new(raw, xmtp_sys::xmtp_client_free)?;
        let client = Client { handle };

        if !client.is_registered() {
            register_identity(&client, signer)?;
        }
        Ok(client)
    }
}

/// Snapshot of an inbox's identity state.
#[derive(Debug, Clone)]
pub struct InboxState {
    /// The inbox ID.
    pub inbox_id: String,
    /// Recovery identifier.
    pub recovery_identifier: String,
    /// Associated account identifiers.
    pub identifiers: Vec<String>,
    /// Installation IDs (hex).
    pub installation_ids: Vec<String>,
}

/// Sign a signature request using the given signer.
pub(crate) fn sign_request(
    sig_req: &OwnedHandle<xmtp_sys::XmtpFfiSignatureRequest>,
    signer: &dyn Signer,
) -> Result<()> {
    let text = unsafe { take_c_string(xmtp_sys::xmtp_signature_request_text(sig_req.as_ptr())) }?;
    let signature = signer.sign(&text)?;
    if signer.is_smart_wallet() {
        let ident = signer.identifier();
        let c_addr = to_c_string(&ident.address)?;
        error::check(unsafe {
            xmtp_sys::xmtp_signature_request_add_scw(
                sig_req.as_ptr(),
                c_addr.as_ptr(),
                signature.as_ptr(),
                signature.len() as i32,
                signer.chain_id(),
                signer.block_number(),
            )
        })
    } else {
        error::check(unsafe {
            xmtp_sys::xmtp_signature_request_add_ecdsa(
                sig_req.as_ptr(),
                signature.as_ptr(),
                signature.len() as i32,
            )
        })
    }
}

/// Apply a completed signature request to the client.
pub(crate) fn apply_signature_request(
    client: &Client,
    sig_req: &OwnedHandle<xmtp_sys::XmtpFfiSignatureRequest>,
) -> Result<()> {
    error::check(unsafe {
        xmtp_sys::xmtp_client_apply_signature_request(client.handle.as_ptr(), sig_req.as_ptr())
    })
}

fn register_identity(client: &Client, signer: &dyn Signer) -> Result<()> {
    let mut raw: *mut xmtp_sys::XmtpFfiSignatureRequest = ptr::null_mut();
    error::check(unsafe {
        xmtp_sys::xmtp_client_create_inbox_signature_request(client.handle.as_ptr(), &mut raw)
    })?;
    if raw.is_null() {
        return Ok(());
    }
    let sig_req = OwnedHandle::new(raw, xmtp_sys::xmtp_signature_request_free)?;
    sign_request(&sig_req, signer)?;
    apply_signature_request(client, &sig_req)?;
    error::check(unsafe {
        xmtp_sys::xmtp_client_register_identity(client.handle.as_ptr(), sig_req.as_ptr())
    })
}

/// Read an FFI inbox state list into `Vec<InboxState>`. Does NOT free the list.
fn read_inbox_state_list(list: *const xmtp_sys::XmtpFfiInboxStateList) -> Result<Vec<InboxState>> {
    if list.is_null() {
        return Ok(vec![]);
    }
    let len = unsafe { xmtp_sys::xmtp_inbox_state_list_len(list) };
    let mut states = Vec::with_capacity(len.max(0) as usize);
    for i in 0..len {
        let inbox_id = unsafe { take_c_string(xmtp_sys::xmtp_inbox_state_inbox_id(list, i)) }?;
        let recovery_identifier =
            unsafe { take_c_string(xmtp_sys::xmtp_inbox_state_recovery_identifier(list, i)) }?;
        let mut ident_count = 0i32;
        let ident_ptr =
            unsafe { xmtp_sys::xmtp_inbox_state_identifiers(list, i, &mut ident_count) };
        let identifiers = unsafe { read_borrowed_strings(ident_ptr, ident_count) };
        let mut inst_count = 0i32;
        let inst_ptr =
            unsafe { xmtp_sys::xmtp_inbox_state_installation_ids(list, i, &mut inst_count) };
        let installation_ids = unsafe { read_borrowed_strings(inst_ptr, inst_count) };
        states.push(InboxState {
            inbox_id,
            recovery_identifier,
            identifiers,
            installation_ids,
        });
    }
    Ok(states)
}
