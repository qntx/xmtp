//! ENS name resolver backed by [`alloy-ens`](https://docs.rs/alloy-ens).
//!
//! Enabled via the `ens` Cargo feature:
//!
//! ```toml
//! [dependencies]
//! xmtp = { version = "0.1", features = ["ens"] }
//! ```

use alloy_ens::ProviderEnsExt as _;
use alloy_provider::ProviderBuilder;
use tokio::runtime::Runtime;

use crate::error::{Error, Result};
use crate::resolve::Resolver;

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
        let addr = self
            .rt
            .block_on(provider.resolve_name(name))
            .map_err(|e| Error::Resolution(format!("{name}: {e}")))?;
        Ok(addr.to_string().to_lowercase())
    }
}
