//! CLI argument definitions and subcommand routing.

pub mod config;
pub mod inspect;
pub mod profile;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use xmtp::Env;

/// Interactive XMTP TUI chat client.
///
/// Launch without a subcommand to enter the TUI chat interface.
/// Use subcommands for one-shot operations.
#[derive(Parser)]
#[command(name = "xmtp", version, about)]
pub struct Cli {
    /// Profile to use for the TUI session.
    #[arg(short, long)]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// One-shot operations (run and exit).
#[derive(Subcommand)]
pub enum Command {
    /// Create a new profile and register with the XMTP network.
    New(NewArgs),
    /// List all saved profiles.
    #[command(alias = "ls")]
    List,
    /// Remove a profile and its data.
    #[command(alias = "rm")]
    Remove {
        /// Profile name to remove.
        name: String,
    },
    /// Remove ALL profiles and data (requires confirmation).
    Clear,
    /// Show profile information and installations.
    Info {
        /// Profile to inspect (uses default if omitted).
        #[arg(short, long)]
        profile: Option<String>,
    },
    /// Revoke all installations except the current one.
    Revoke {
        /// Profile to revoke for (uses default if omitted).
        #[arg(short, long)]
        profile: Option<String>,
    },
    /// Show or set the default profile.
    Default {
        /// Profile name to set as default. Omit to show current.
        name: Option<String>,
    },
}

/// Arguments for the `new` subcommand.
#[derive(clap::Args)]
pub struct NewArgs {
    /// Profile name.
    pub name: String,

    /// XMTP network environment.
    #[arg(long, default_value = "dev", value_parser = parse_env)]
    pub env: Env,

    /// Ethereum RPC URL for ENS resolution.
    #[arg(long, default_value = "https://eth.llamarpc.com")]
    pub rpc_url: String,

    /// Import a hex-encoded private key.
    #[arg(long, conflicts_with_all = ["key", "ledger"])]
    pub import: Option<String>,

    /// Copy a private key file into the profile.
    #[arg(long, conflicts_with_all = ["import", "ledger"])]
    pub key: Option<PathBuf>,

    /// Copy a database file into the profile.
    #[arg(long)]
    pub db: Option<PathBuf>,

    /// Use a Ledger hardware wallet (optionally specify account index, default 0).
    #[arg(long, num_args = 0..=1, default_missing_value = "0",
          conflicts_with_all = ["import", "key"])]
    pub ledger: Option<usize>,
}

pub fn parse_env(s: &str) -> Result<Env, String> {
    match s.to_ascii_lowercase().as_str() {
        "dev" | "development" => Ok(Env::Dev),
        "prod" | "production" => Ok(Env::Production),
        "local" | "localhost" => Ok(Env::Local),
        _ => Err(format!(
            "unknown environment: {s} (expected: dev, production, local)"
        )),
    }
}
