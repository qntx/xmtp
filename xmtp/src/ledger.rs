//! Ledger hardware wallet signer backed by
//! [`alloy-signer-ledger`](https://docs.rs/alloy-signer-ledger).
//!
//! Enabled via the `ledger` Cargo feature:
//!
//! ```toml
//! [dependencies]
//! xmtp = { version = "0.1", features = ["ledger"] }
//! ```

use alloy_signer::Signer as AlloySigner;
use alloy_signer_ledger::{HDPath, LedgerSigner as Inner};
use tokio::runtime::Runtime;

use crate::error::{Error, Result};
use crate::types::{AccountIdentifier, IdentifierKind, Signer};

/// A Ledger hardware wallet signer powered by
/// [`alloy-signer-ledger`](https://docs.rs/alloy-signer-ledger).
///
/// Wraps [`LedgerSigner`](Inner) and implements the [`Signer`] trait for
/// seamless use with [`ClientBuilder`](crate::ClientBuilder).
///
/// # Note
///
/// This signer communicates with the Ledger device over USB. The user must
/// confirm signing operations on the device screen. Do **not** call from
/// within an async context — use [`tokio::task::spawn_blocking`] if needed.
pub struct LedgerSigner {
    inner: Inner,
    rt: Runtime,
}

impl std::fmt::Debug for LedgerSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerSigner")
            .field("address", &self.address())
            .finish_non_exhaustive()
    }
}

impl LedgerSigner {
    /// Connect to a Ledger device using the **Ledger Live** HD path at the
    /// given account index (e.g., `0` for the first account).
    ///
    /// This creates a lightweight tokio runtime internally to communicate
    /// with the device over USB.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Signing`] if the device is not connected, locked,
    /// or the Ethereum app is not open.
    pub fn new(account_index: usize) -> Result<Self> {
        Self::with_hd_path(HDPath::LedgerLive(account_index))
    }

    /// Connect to a Ledger device using the **legacy** HD path at the given
    /// account index.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Signing`] if the device is not connected or
    /// unavailable.
    pub fn legacy(account_index: usize) -> Result<Self> {
        Self::with_hd_path(HDPath::Legacy(account_index))
    }

    /// Connect to a Ledger device using a custom [`HDPath`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Signing`] if the device is not connected or
    /// unavailable.
    pub fn with_hd_path(hd_path: HDPath) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Signing(e.to_string()))?;
        let inner = rt
            .block_on(Inner::new(hd_path, None))
            .map_err(|e| Error::Signing(e.to_string()))?;
        Ok(Self { inner, rt })
    }

    /// Returns the Ethereum address as a checksummed hex string.
    #[must_use]
    pub fn address(&self) -> String {
        AlloySigner::address(&self.inner).to_checksum(None)
    }

    /// Query the Ledger device for the running Ethereum app version.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Signing`] if the device communication fails.
    pub fn version(&self) -> Result<String> {
        let ver = self
            .rt
            .block_on(self.inner.version())
            .map_err(|e| Error::Signing(e.to_string()))?;
        Ok(ver.to_string())
    }
}

impl Signer for LedgerSigner {
    fn identifier(&self) -> AccountIdentifier {
        AccountIdentifier {
            // XMTP uses lowercase addresses for identity matching.
            address: AlloySigner::address(&self.inner).to_string().to_lowercase(),
            kind: IdentifierKind::Ethereum,
        }
    }

    fn sign(&self, text: &str) -> Result<Vec<u8>> {
        let fut = self.inner.sign_message(text.as_bytes());
        let sig = self
            .rt
            .block_on(fut)
            .map_err(|e| Error::Signing(e.to_string()))?;
        Ok(sig.as_bytes().to_vec())
    }
}
