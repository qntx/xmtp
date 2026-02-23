#![allow(unsafe_code)]
//! XMTP client — the primary entry point for the SDK.

mod conversations;
mod identity;

use std::ffi::c_char;
use std::ptr;

use crate::error::{self, Result};
use crate::ffi::{OwnedHandle, read_borrowed_strings, take_c_string, to_c_string};
use crate::types::{
    AccountIdentifier, ApiStats, ConsentEntityType, ConsentState, Env, IdentifierKind,
    IdentityStats, InboxState, KeyPackageStatus, Signer,
};

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
            &raw mut out,
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
    pub fn release_db(&self) -> Result<()> {
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
    pub fn inbox_id_for(&self, address: &str, kind: IdentifierKind) -> Result<Option<String>> {
        let c = to_c_string(address)?;
        let mut out: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_get_inbox_id_by_identifier(
                self.handle.as_ptr(),
                c.as_ptr(),
                kind as i32,
                &raw mut out,
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
        let ptr = unsafe {
            xmtp_sys::xmtp_client_installation_id_bytes(self.handle.as_ptr(), &raw mut len)
        };
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
            xmtp_sys::xmtp_client_inbox_state(
                self.handle.as_ptr(),
                i32::from(refresh),
                &raw mut out,
            )
        };
        error::check(rc)?;
        let result = read_inbox_state_list(out);
        if !out.is_null() {
            unsafe { xmtp_sys::xmtp_inbox_state_list_free(out) };
        }
        result
    }

    /// Fetch inbox states for multiple inbox IDs.
    pub fn inbox_states(&self, inbox_ids: &[&str], refresh: bool) -> Result<Vec<InboxState>> {
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
                &raw mut out,
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
                &raw mut out,
                &raw mut out_len,
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
    pub fn verify_installation_signature(&self, text: &str, signature: &[u8]) -> Result<bool> {
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
    pub fn set_consent(&self, entries: &[(ConsentEntityType, ConsentState, &str)]) -> Result<()> {
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
    pub fn consent_state(
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
                &raw mut out,
            )
        };
        error::check(rc)?;
        ConsentState::from_ffi(out)
            .ok_or_else(|| crate::Error::Ffi(format!("unknown consent state: {out}")))
    }

    /// Get MLS API call statistics.
    pub fn mls_stats(&self) -> Result<ApiStats> {
        let mut out = xmtp_sys::XmtpFfiApiStats::default();
        error::check(unsafe {
            xmtp_sys::xmtp_client_api_statistics(self.handle.as_ptr(), &raw mut out)
        })?;
        Ok(ApiStats {
            upload_key_package: out.upload_key_package,
            fetch_key_package: out.fetch_key_package,
            send_group_messages: out.send_group_messages,
            send_welcome_messages: out.send_welcome_messages,
            query_group_messages: out.query_group_messages,
            query_welcome_messages: out.query_welcome_messages,
            subscribe_messages: out.subscribe_messages,
            subscribe_welcomes: out.subscribe_welcomes,
            publish_commit_log: out.publish_commit_log,
            query_commit_log: out.query_commit_log,
            get_newest_group_message: out.get_newest_group_message,
        })
    }

    /// Get identity API call statistics.
    pub fn identity_stats(&self) -> Result<IdentityStats> {
        let mut out = xmtp_sys::XmtpFfiIdentityStats::default();
        error::check(unsafe {
            xmtp_sys::xmtp_client_api_identity_statistics(self.handle.as_ptr(), &raw mut out)
        })?;
        Ok(IdentityStats {
            publish_identity_update: out.publish_identity_update,
            get_identity_updates_v2: out.get_identity_updates_v2,
            get_inbox_ids: out.get_inbox_ids,
            verify_smart_contract_wallet_signature: out.verify_smart_contract_wallet_signature,
        })
    }

    /// Get aggregate statistics as a human-readable debug string.
    pub fn aggregate_stats(&self) -> Result<String> {
        unsafe {
            take_c_string(xmtp_sys::xmtp_client_api_aggregate_statistics(
                self.handle.as_ptr(),
            ))
        }
    }

    /// Clear all API call statistics.
    pub fn clear_stats(&self) -> Result<()> {
        error::check(unsafe { xmtp_sys::xmtp_client_clear_all_statistics(self.handle.as_ptr()) })
    }

    /// Fetch key package statuses for a list of installation IDs (hex).
    pub fn key_package_statuses(&self, installation_ids: &[&str]) -> Result<Vec<KeyPackageStatus>> {
        let (_owned, ptrs) = crate::ffi::to_c_string_array(installation_ids)?;
        let mut out: *mut xmtp_sys::XmtpFfiKeyPackageStatusList = ptr::null_mut();
        let rc = unsafe {
            xmtp_sys::xmtp_client_fetch_key_package_statuses(
                self.handle.as_ptr(),
                ptrs.as_ptr(),
                ptrs.len() as i32,
                &raw mut out,
            )
        };
        error::check(rc)?;
        if out.is_null() {
            return Ok(vec![]);
        }
        let result = read_key_package_status_list(out);
        unsafe { xmtp_sys::xmtp_key_package_status_list_free(out) };
        Ok(result)
    }

    /// Send a device sync request to retrieve records from another installation.
    pub fn request_device_sync(&self) -> Result<()> {
        let opts = xmtp_sys::XmtpFfiArchiveOptions::default();
        error::check(unsafe {
            xmtp_sys::xmtp_device_sync_send_request(
                self.handle.as_ptr(),
                &raw const opts,
                ptr::null(),
            )
        })
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
    pub const fn env(mut self, env: Env) -> Self {
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
    pub const fn nonce(mut self, n: u64) -> Self {
        self.nonce = n;
        self
    }

    /// Disable the device sync worker.
    #[must_use]
    pub const fn disable_device_sync(mut self) -> Self {
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
        error::check(unsafe { xmtp_sys::xmtp_client_create(&raw const opts, &raw mut raw) })?;
        let handle = OwnedHandle::new(raw, xmtp_sys::xmtp_client_free)?;
        let client = Client { handle };

        if !client.is_registered() {
            register_identity(&client, signer)?;
        }
        Ok(client)
    }
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
        xmtp_sys::xmtp_client_create_inbox_signature_request(client.handle.as_ptr(), &raw mut raw)
    })?;
    if raw.is_null() {
        return Ok(());
    }
    let sig_req = OwnedHandle::new(raw, xmtp_sys::xmtp_signature_request_free)?;
    sign_request(&sig_req, signer)?;
    // register_identity publishes the identity update AND uploads key packages.
    // Do NOT call apply_signature_request separately or the identity update will
    // be published twice, causing "Multiple create operations detected".
    error::check(unsafe {
        xmtp_sys::xmtp_client_register_identity(client.handle.as_ptr(), sig_req.as_ptr())
    })
}

/// Verify a signature produced by `sign_with_installation_key` using a public key.
/// No client handle required.
pub fn verify_signed_with_public_key(
    text: &str,
    signature: &[u8],
    public_key: &[u8],
) -> Result<bool> {
    let c = to_c_string(text)?;
    let rc = unsafe {
        xmtp_sys::xmtp_verify_signed_with_public_key(
            c.as_ptr(),
            signature.as_ptr(),
            signature.len() as i32,
            public_key.as_ptr(),
            public_key.len() as i32,
        )
    };
    Ok(rc == 0)
}

/// Check whether an Ethereum address belongs to an inbox. No client required.
pub fn is_address_authorized(env: Env, inbox_id: &str, address: &str) -> Result<bool> {
    let c_url = to_c_string(env.url())?;
    let c_inbox = to_c_string(inbox_id)?;
    let c_addr = to_c_string(address)?;
    let mut out = 0i32;
    let rc = unsafe {
        xmtp_sys::xmtp_is_address_authorized(
            c_url.as_ptr(),
            i32::from(env.is_secure()),
            c_inbox.as_ptr(),
            c_addr.as_ptr(),
            &raw mut out,
        )
    };
    error::check(rc)?;
    Ok(out == 1)
}

/// Check whether an installation (public key bytes) belongs to an inbox. No client required.
pub fn is_installation_authorized(
    env: Env,
    inbox_id: &str,
    installation_id: &[u8],
) -> Result<bool> {
    let c_url = to_c_string(env.url())?;
    let c_inbox = to_c_string(inbox_id)?;
    let mut out = 0i32;
    let rc = unsafe {
        xmtp_sys::xmtp_is_installation_authorized(
            c_url.as_ptr(),
            i32::from(env.is_secure()),
            c_inbox.as_ptr(),
            installation_id.as_ptr(),
            installation_id.len() as i32,
            &raw mut out,
        )
    };
    error::check(rc)?;
    Ok(out == 1)
}

/// Read an FFI key package status list. Does NOT free the list.
fn read_key_package_status_list(
    list: *const xmtp_sys::XmtpFfiKeyPackageStatusList,
) -> Vec<KeyPackageStatus> {
    if list.is_null() {
        return vec![];
    }
    let len = unsafe { xmtp_sys::xmtp_key_package_status_list_len(list) };
    let mut statuses = Vec::with_capacity(len.max(0) as usize);
    for i in 0..len {
        let ptr = unsafe { xmtp_sys::xmtp_key_package_status_list_get(list, i) };
        if ptr.is_null() {
            continue;
        }
        let s = unsafe { &*ptr };
        let installation_id = unsafe { take_c_string(s.installation_id) }.unwrap_or_default();
        let validation_error = if s.validation_error.is_null() {
            None
        } else {
            unsafe { take_c_string(s.validation_error) }.ok()
        };
        statuses.push(KeyPackageStatus {
            installation_id,
            valid: s.valid == 1,
            not_before: s.not_before,
            not_after: s.not_after,
            validation_error,
        });
    }
    statuses
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
            unsafe { xmtp_sys::xmtp_inbox_state_identifiers(list, i, &raw mut ident_count) };
        let identifiers = unsafe { read_borrowed_strings(ident_ptr, ident_count) };
        let mut inst_count = 0i32;
        let inst_ptr =
            unsafe { xmtp_sys::xmtp_inbox_state_installation_ids(list, i, &raw mut inst_count) };
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
