//! Built-in signer backed by [`alloy`](https://docs.rs/alloy-signer-local).
//!
//! Enabled via the `alloy` Cargo feature:
//!
//! ```toml
//! [dependencies]
//! xmtp = { version = "0.1", features = ["alloy"] }
//! ```

use alloy_signer::SignerSync;
use alloy_signer_local::PrivateKeySigner;

use crate::error::{Error, Result};
use crate::types::{AccountIdentifier, IdentifierKind, Signer};

/// A local Ethereum private-key signer powered by
/// [`alloy-signer-local`](https://docs.rs/alloy-signer-local).
///
/// Wraps [`PrivateKeySigner`] and implements the [`Signer`] trait for seamless
/// use with [`ClientBuilder`](crate::ClientBuilder).
#[derive(Debug, Clone)]
pub struct AlloySigner {
    inner: PrivateKeySigner,
}

impl AlloySigner {
    /// Create a signer from a hex-encoded private key.
    ///
    /// The key may optionally include a `0x` prefix.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Signing`] if the hex string is malformed or does not
    /// represent a valid secp256k1 secret key.
    pub fn from_hex(key: &str) -> Result<Self> {
        let inner: PrivateKeySigner = key
            .parse()
            .map_err(|e: alloy_signer_local::LocalSignerError| Error::Signing(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Create a signer from raw 32-byte private key material.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Signing`] if the bytes are not a valid secp256k1
    /// secret key.
    pub fn from_bytes(key: &[u8; 32]) -> Result<Self> {
        let inner = PrivateKeySigner::from_slice(key).map_err(|e| Error::Signing(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Generate a random signer. Primarily useful for testing.
    #[must_use]
    pub fn random() -> Self {
        Self {
            inner: PrivateKeySigner::random(),
        }
    }

    /// Returns the Ethereum address as a checksummed hex string.
    #[must_use]
    pub fn address(&self) -> String {
        self.inner.address().to_checksum(None)
    }

    /// Consume this wrapper and return the underlying [`PrivateKeySigner`].
    #[must_use]
    pub fn into_inner(self) -> PrivateKeySigner {
        self.inner
    }
}

impl From<PrivateKeySigner> for AlloySigner {
    fn from(inner: PrivateKeySigner) -> Self {
        Self { inner }
    }
}

impl Signer for AlloySigner {
    fn identifier(&self) -> AccountIdentifier {
        AccountIdentifier {
            // XMTP uses lowercase addresses for identity matching.
            address: self.inner.address().to_string().to_lowercase(),
            kind: IdentifierKind::Ethereum,
        }
    }

    fn sign(&self, text: &str) -> Result<Vec<u8>> {
        let sig = self
            .inner
            .sign_message_sync(text.as_bytes())
            .map_err(|e| Error::Signing(e.to_string()))?;
        Ok(sig.as_bytes().to_vec())
    }
}
