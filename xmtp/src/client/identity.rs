#![allow(unsafe_code)]
//! Identity management: add/remove accounts, revoke installations.

use std::ptr;

use crate::error::{self, Result};
use crate::ffi::{OwnedHandle, to_c_string};
use crate::types::*;

use super::{Client, apply_signature_request, sign_request};

impl Client {
    /// Add a new identifier to this inbox. Requires signing with both the
    /// existing signer and the new account's signer.
    pub fn add_account(&self, existing_signer: &dyn Signer, new_signer: &dyn Signer) -> Result<()> {
        let new_ident = new_signer.identifier();
        let c_addr = to_c_string(&new_ident.address)?;
        create_sign_apply(self, &[existing_signer, new_signer], |out| unsafe {
            xmtp_sys::xmtp_client_add_identifier_signature_request(
                self.handle.as_ptr(),
                c_addr.as_ptr(),
                new_ident.kind as i32,
                out,
            )
        })
    }

    /// Remove an identifier from this inbox.
    pub fn remove_account(
        &self,
        signer: &dyn Signer,
        identifier: &AccountIdentifier,
    ) -> Result<()> {
        let c = to_c_string(&identifier.address)?;
        create_sign_apply(self, &[signer], |out| unsafe {
            xmtp_sys::xmtp_client_revoke_identifier_signature_request(
                self.handle.as_ptr(),
                c.as_ptr(),
                identifier.kind as i32,
                out,
            )
        })
    }

    /// Revoke all installations except the current one.
    pub fn revoke_all_other_installations(&self, signer: &dyn Signer) -> Result<()> {
        create_sign_apply(self, &[signer], |out| unsafe {
            xmtp_sys::xmtp_client_revoke_all_other_installations(self.handle.as_ptr(), out)
        })
    }

    /// Revoke specific installations by their ID bytes.
    pub fn revoke_installations(
        &self,
        signer: &dyn Signer,
        installation_ids: &[&[u8]],
    ) -> Result<()> {
        let ptrs: Vec<*const u8> = installation_ids.iter().map(|id| id.as_ptr()).collect();
        let lens: Vec<i32> = installation_ids.iter().map(|id| id.len() as i32).collect();
        create_sign_apply(self, &[signer], |out| unsafe {
            xmtp_sys::xmtp_client_revoke_installations_signature_request(
                self.handle.as_ptr(),
                ptrs.as_ptr(),
                lens.as_ptr(),
                ptrs.len() as i32,
                out,
            )
        })
    }

    /// Change the recovery identifier for this inbox.
    pub fn change_recovery_identifier(
        &self,
        signer: &dyn Signer,
        new_identifier: &AccountIdentifier,
    ) -> Result<()> {
        let c = to_c_string(&new_identifier.address)?;
        create_sign_apply(self, &[signer], |out| unsafe {
            xmtp_sys::xmtp_client_change_recovery_identifier_signature_request(
                self.handle.as_ptr(),
                c.as_ptr(),
                new_identifier.kind as i32,
                out,
            )
        })
    }
}

/// Create a signature request, sign it with all given signers, then apply it.
fn create_sign_apply(
    client: &Client,
    signers: &[&dyn Signer],
    create: impl FnOnce(&mut *mut xmtp_sys::XmtpFfiSignatureRequest) -> i32,
) -> Result<()> {
    let mut raw: *mut xmtp_sys::XmtpFfiSignatureRequest = ptr::null_mut();
    error::check(create(&mut raw))?;
    if raw.is_null() {
        return Ok(());
    }
    let sig_req = OwnedHandle::new(raw, xmtp_sys::xmtp_signature_request_free)?;
    for signer in signers {
        sign_request(&sig_req, *signer)?;
    }
    apply_signature_request(client, &sig_req)
}
