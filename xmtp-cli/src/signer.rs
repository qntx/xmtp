//! Local ECDSA identity for development and testing.

use std::fs;
use std::path::Path;

use k256::ecdsa::signature::hazmat::PrehashSigner as _;
use k256::ecdsa::{RecoveryId, Signature, SigningKey};
use k256::elliptic_curve::rand_core::OsRng;
use sha3::{Digest as _, Keccak256};

use xmtp::Signer;
use xmtp::types::{AccountIdentifier, IdentifierKind};

/// A file-backed ECDSA signer for development use.
pub struct LocalSigner {
    key: SigningKey,
    /// The derived Ethereum address.
    pub address: String,
}

impl Signer for LocalSigner {
    fn identifier(&self) -> AccountIdentifier {
        AccountIdentifier {
            address: self.address.clone(),
            kind: IdentifierKind::Ethereum,
        }
    }

    fn sign(&self, text: &str) -> xmtp::Result<Vec<u8>> {
        let prefixed = format!("\x19Ethereum Signed Message:\n{}{}", text.len(), text);
        let hash = Keccak256::digest(prefixed.as_bytes());
        let (sig, recid): (Signature, RecoveryId) = self
            .key
            .sign_prehash(&hash)
            .map_err(|e| xmtp::Error::Ffi(format!("ecdsa: {e}")))?;
        let mut bytes = sig.to_bytes().to_vec();
        bytes.push(recid.to_byte());
        Ok(bytes)
    }
}

/// Load an existing key file or generate and persist a new one.
pub fn load_or_create(key_path: &str) -> LocalSigner {
    let key = if Path::new(key_path).exists() {
        let bytes = fs::read(key_path).expect("read key file");
        SigningKey::from_bytes(bytes.as_slice().into()).expect("parse key")
    } else {
        let key = SigningKey::random(&mut OsRng);
        fs::write(key_path, key.to_bytes()).expect("save key file");
        key
    };
    let address = eth_address(&key);
    LocalSigner { key, address }
}

/// Derive an Ethereum address from a signing key.
fn eth_address(key: &SigningKey) -> String {
    let pubkey = key.verifying_key().to_encoded_point(false);
    let hash = Keccak256::digest(&pubkey.as_bytes()[1..]);
    format!("0x{}", hex::encode(&hash[12..]))
}
