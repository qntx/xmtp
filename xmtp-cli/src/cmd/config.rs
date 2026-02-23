//! Profile configuration persistence and shared infrastructure.

use std::path::PathBuf;
use std::{fmt, fs};

use xmtp::{AlloySigner, Client, EnsResolver, Env, IdentifierKind, LedgerSigner, Signer};

/// Base data directory for all profiles.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xmtp-cli")
}

/// Data directory for a specific profile.
pub fn profile_dir(name: &str) -> PathBuf {
    data_dir().join(name)
}

/// Read the default profile name (falls back to `"default"`).
pub fn default_profile() -> String {
    let path = data_dir().join(".default");
    fs::read_to_string(path).map_or_else(|_| "default".into(), |s| s.trim().to_owned())
}

/// Persist the default profile name.
pub fn set_default(name: &str) -> xmtp::Result<()> {
    let base = data_dir();
    fs::create_dir_all(&base).map_err(|e| xmtp::Error::Ffi(format!("mkdir: {e}")))?;
    fs::write(base.join(".default"), name).map_err(|e| xmtp::Error::Ffi(format!("write: {e}")))
}

/// How a profile signs messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerKind {
    /// Local key file (`identity.key`).
    File,
    /// Ledger hardware wallet with account index.
    Ledger(usize),
}

impl fmt::Display for SignerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File => f.write_str("file"),
            Self::Ledger(i) => write!(f, "ledger:{i}"),
        }
    }
}

/// Persistent per-profile configuration stored as `profile.conf`.
#[derive(Debug, Clone)]
pub struct ProfileConfig {
    pub env: Env,
    pub rpc_url: String,
    pub signer: SignerKind,
    /// Cached wallet address (avoids needing signer just to read address).
    pub address: String,
}

impl ProfileConfig {
    /// Load from `<profile_dir>/profile.conf`.
    pub fn load(profile: &str) -> xmtp::Result<Self> {
        let path = profile_dir(profile).join("profile.conf");
        let text =
            fs::read_to_string(&path).map_err(|e| xmtp::Error::Ffi(format!("load config: {e}")))?;

        let mut env = Env::Dev;
        let mut rpc_url = String::from("https://eth.llamarpc.com");
        let mut signer = SignerKind::File;
        let mut address = String::new();

        for line in text.lines() {
            if let Some((k, v)) = line.trim().split_once('=') {
                match k.trim() {
                    "env" => {
                        env = super::parse_env(v.trim()).map_err(xmtp::Error::Ffi)?;
                    }
                    "rpc_url" => v.trim().clone_into(&mut rpc_url),
                    "signer" => {
                        signer = if v.trim().starts_with("ledger") {
                            let idx = v
                                .trim()
                                .strip_prefix("ledger:")
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(0);
                            SignerKind::Ledger(idx)
                        } else {
                            SignerKind::File
                        };
                    }
                    "address" => v.trim().clone_into(&mut address),
                    _ => {}
                }
            }
        }

        Ok(Self {
            env,
            rpc_url,
            signer,
            address,
        })
    }

    /// Save to `<profile_dir>/profile.conf`.
    pub fn save(&self, profile: &str) -> xmtp::Result<()> {
        let dir = profile_dir(profile);
        fs::create_dir_all(&dir).map_err(|e| xmtp::Error::Ffi(format!("mkdir: {e}")))?;
        let content = format!(
            "env={}\nrpc_url={}\nsigner={}\naddress={}\n",
            env_name(self.env),
            self.rpc_url,
            self.signer,
            self.address,
        );
        fs::write(dir.join("profile.conf"), content)
            .map_err(|e| xmtp::Error::Ffi(format!("write config: {e}")))
    }
}

/// Open a profile without a signer (for TUI and info — no signing needed).
///
/// If the profile was created before the `address` field existed, falls back
/// to signer-based opening once to discover and persist the address.
pub fn open_client(profile: &str) -> xmtp::Result<(ProfileConfig, Client)> {
    let cfg = ProfileConfig::load(profile)?;

    if cfg.address.is_empty() {
        // Legacy profile: need signer to discover wallet address.
        let (mut cfg, signer, client) = open_with_signer(profile)?;
        cfg.address = signer.identifier().address;
        cfg.save(profile)?;
        return Ok((cfg, client));
    }

    let db = profile_dir(profile).join("messages.db3");
    let client = build_client(&cfg, &db.to_string_lossy(), None)?;
    Ok((cfg, client))
}

/// Open a profile with a signer (for operations that need signing, e.g. revoke).
pub fn open_with_signer(profile: &str) -> xmtp::Result<(ProfileConfig, Box<dyn Signer>, Client)> {
    let cfg = ProfileConfig::load(profile)?;
    let dir = profile_dir(profile);

    let signer: Box<dyn Signer> = match cfg.signer {
        SignerKind::File => {
            let bytes = fs::read(dir.join("identity.key"))
                .map_err(|e| xmtp::Error::Ffi(format!("read key: {e}")))?;
            let key: [u8; 32] = bytes
                .try_into()
                .map_err(|_| xmtp::Error::InvalidArgument("key must be 32 bytes".into()))?;
            Box::new(AlloySigner::from_bytes(&key)?)
        }
        SignerKind::Ledger(index) => {
            eprintln!("Connecting to Ledger (index {index})...");
            Box::new(LedgerSigner::new(index)?)
        }
    };

    let db = dir.join("messages.db3");
    let client = build_client(&cfg, &db.to_string_lossy(), Some(signer.as_ref()))?;
    Ok((cfg, signer, client))
}

/// Build an XMTP client with automatic stale-DB recovery.
///
/// When `signer` is `Some`, uses `build(signer)` which may register.
/// When `None`, uses `build_existing()` with the stored address (no signing).
pub fn build_client(
    cfg: &ProfileConfig,
    db_path: &str,
    signer: Option<&dyn Signer>,
) -> xmtp::Result<Client> {
    let build = |path: &str| {
        let mut b = Client::builder().env(cfg.env).db_path(path);
        if let Ok(r) = EnsResolver::new(&cfg.rpc_url) {
            b = b.resolver(r);
        }
        match signer {
            Some(s) => b.build(s),
            None => b.build_existing(&cfg.address, IdentifierKind::Ethereum),
        }
    };

    match build(db_path) {
        Ok(c) => Ok(c),
        Err(e) if e.to_string().contains("does not match the stored InboxId") => {
            for ext in ["", "-shm", "-wal"] {
                let _ = fs::remove_file(format!("{db_path}{ext}"));
            }
            build(db_path)
        }
        Err(e) => Err(e),
    }
}

/// Human-readable environment name.
pub const fn env_name(env: Env) -> &'static str {
    match env {
        Env::Dev => "dev",
        Env::Production => "production",
        Env::Local => "local",
    }
}
