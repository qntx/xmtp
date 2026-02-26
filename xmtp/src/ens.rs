//! ENS name resolver backed by [`alloy-ens`](https://docs.rs/alloy-ens).
//!
//! Enabled via the `ens` Cargo feature:
//!
//! ```toml
//! [dependencies]
//! xmtp = { version = "0.1", features = ["ens"] }
//! ```

use std::time::Duration;

use alloy_ens::ProviderEnsExt as _;
use alloy_provider::ProviderBuilder;
use tokio::runtime::Runtime;

use crate::error::{Error, Result};
use crate::resolve::Resolver;

/// Per-call timeout for RPC operations (connect + execute).
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Default public Ethereum RPC endpoint for ENS resolution.
const DEFAULT_RPC: &str = "https://eth.llamarpc.com";

/// ENS name resolver connecting to an Ethereum JSON-RPC endpoint.
///
/// Resolves `.eth` names (and subdomains) to Ethereum addresses via the
/// on-chain ENS registry contract.
///
/// # Examples
///
/// ```no_run
/// use xmtp::{Client, EnsResolver, Env};
///
/// # fn example(signer: &dyn xmtp::Signer) -> xmtp::Result<()> {
/// let client = Client::builder()
///     .env(Env::Dev)
///     .resolver(EnsResolver::mainnet()?)
///     .build(signer)?;
///
/// // ENS names now resolve automatically
/// client.dm(&"vitalik.eth".into())?;
/// # Ok(())
/// # }
/// ```
pub struct EnsResolver {
    rt: Runtime,
    rpc_url: url::Url,
}

impl std::fmt::Debug for EnsResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnsResolver")
            .field("rpc_url", &self.rpc_url.as_str())
            .finish_non_exhaustive()
    }
}

impl EnsResolver {
    /// Create a resolver using a public Ethereum mainnet RPC.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal tokio runtime cannot be created.
    pub fn mainnet() -> Result<Self> {
        Self::new(DEFAULT_RPC)
    }

    /// Create a resolver targeting a custom Ethereum RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL is malformed or the runtime cannot be created.
    pub fn new(rpc_url: &str) -> Result<Self> {
        let rpc_url: url::Url = rpc_url
            .parse()
            .map_err(|e| Error::InvalidArgument(format!("bad RPC URL: {e}")))?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Resolution(e.to_string()))?;
        Ok(Self { rt, rpc_url })
    }
}

impl Resolver for EnsResolver {
    fn resolve(&self, name: &str) -> Result<String> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.clone());
        let addr = self.rt.block_on(async {
            tokio::time::timeout(RPC_TIMEOUT, provider.resolve_name(name))
                .await
                .map_err(|_| Error::Resolution(format!("{name}: timeout")))?
                .map_err(|e| Error::Resolution(format!("{name}: {e}")))
        })?;
        Ok(addr.to_string().to_lowercase())
    }

    fn reverse_resolve(&self, address: &str) -> Result<Option<String>> {
        let addr: alloy_primitives::Address = address
            .parse()
            .map_err(|e| Error::Resolution(format!("{address}: {e}")))?;
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.clone());
        self.rt.block_on(async {
            match tokio::time::timeout(RPC_TIMEOUT, provider.lookup_address(&addr)).await {
                Ok(Ok(name)) => Ok(Some(name)),
                Ok(Err(_)) => Ok(None),
                Err(_) => Err(Error::Resolution(format!("{address}: timeout"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probe multiple RPC endpoints. Requires network.
    /// Run: `cargo test -p xmtp --all-features -- --ignored --nocapture probe`
    #[test]
    #[ignore = "requires network access to Ethereum RPC"]
    fn probe_rpc_endpoints() {
        let rpcs = [
            ("cloudflare", "https://cloudflare-eth.com"),
            ("llamarpc", "https://eth.llamarpc.com"),
            ("publicnode", "https://ethereum-rpc.publicnode.com"),
        ];
        for (label, url) in rpcs {
            let resolver = EnsResolver::new(url).expect("create resolver");
            let t = std::time::Instant::now();
            let r = resolver.resolve("vitalik.eth");
            eprintln!("[{label}] resolve: {r:?} ({:.1?})", t.elapsed());
            let t = std::time::Instant::now();
            let r = resolver.reverse_resolve("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
            eprintln!("[{label}] reverse: {r:?} ({:.1?})", t.elapsed());
        }
    }

    /// Smoke test: forward + reverse resolve via default RPC.
    /// Run: `cargo test -p xmtp --all-features -- --ignored --nocapture smoke`
    #[test]
    #[ignore = "requires network access to Ethereum RPC"]
    fn smoke_resolve() {
        let resolver = EnsResolver::mainnet().expect("create resolver");
        let fwd = resolver.resolve("qntx.eth");
        eprintln!("forward: {fwd:?}");
        assert!(fwd.is_ok(), "forward failed: {fwd:?}");

        let rev = resolver.reverse_resolve("0xE350Ef4E8557a3e2a24D11327d9F25B382Ac93Cb");
        eprintln!("reverse: {rev:?}");
        assert!(rev.is_ok(), "reverse failed: {rev:?}");
        assert_eq!(rev.unwrap().as_deref(), Some("qntx.eth"));
    }
}
