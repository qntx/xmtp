//! Profile management commands: new, list, remove, clear, default.

use std::fs;
use std::io;

use xmtp::{AlloySigner, Client, LedgerSigner, Signer};

use super::NewArgs;
use super::config::{self, ProfileConfig, SignerKind};
use crate::app::truncate_id;

/// Create a new profile, register with the XMTP network, and save config.
///
/// Returns the config (with address) and a ready-to-use client.
pub fn create(args: &NewArgs) -> xmtp::Result<(ProfileConfig, Client)> {
    let dir = config::profile_dir(&args.name);
    if dir.join("profile.conf").exists() {
        return Err(xmtp::Error::InvalidArgument(format!(
            "profile '{}' already exists",
            args.name
        )));
    }

    fs::create_dir_all(&dir).map_err(|e| xmtp::Error::Ffi(format!("mkdir: {e}")))?;

    let key_path = dir.join("identity.key");
    let db_path = dir.join("messages.db3");

    // Determine signer kind and create signer.
    let (signer_kind, signer): (SignerKind, Box<dyn Signer>) = if let Some(index) = args.ledger { (
        SignerKind::Ledger(index),
        Box::new(LedgerSigner::new(index)?),
    ) } else {
        if let Some(ref hex) = args.import {
            import_hex_key(hex, &key_path)?;
        } else if let Some(ref src) = args.key {
            fs::copy(src, &key_path).map_err(|e| xmtp::Error::Ffi(format!("copy key: {e}")))?;
        }
        (SignerKind::File, Box::new(load_or_create_key(&key_path)?))
    };

    // Copy database if provided.
    if let Some(ref src) = args.db {
        fs::copy(src, &db_path).map_err(|e| xmtp::Error::Ffi(format!("copy db: {e}")))?;
    }

    // Register with the XMTP network.
    let address = signer.identifier().address;
    let cfg = ProfileConfig {
        env: args.env,
        rpc_url: args.rpc_url.clone(),
        signer: signer_kind,
        address: address.clone(),
    };
    let client = config::build_client(&cfg, &db_path.to_string_lossy(), Some(signer.as_ref()))?;
    let inbox_id = client.inbox_id()?;

    // Save profile config (address is now known after signer creation).
    cfg.save(&args.name)?;

    // Set as default if this is the first profile ever.
    if !config::data_dir().join(".default").exists() {
        config::set_default(&args.name)?;
    }

    println!("Profile '{}' created.", args.name);
    println!("  Address:  {address}");
    println!("  Inbox ID: {inbox_id}");
    println!("  Env:      {}", config::env_name(args.env));
    Ok((cfg, client))
}

/// List all saved profiles.
pub fn list() -> xmtp::Result<()> {
    let base = config::data_dir();
    if !base.exists() {
        println!("No profiles found.");
        return Ok(());
    }

    let default = config::default_profile();

    let mut entries: Vec<_> = fs::read_dir(&base)
        .map_err(|e| xmtp::Error::Ffi(format!("read dir: {e}")))?
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(fs::DirEntry::file_name);

    if entries.is_empty() {
        println!("No profiles found.");
        return Ok(());
    }

    for entry in &entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let star = if *name == default { " *" } else { "" };

        if let Ok(cfg) = ProfileConfig::load(&name) {
            let addr = if cfg.address.is_empty() {
                "—".into()
            } else {
                truncate_id(&cfg.address, 14)
            };
            println!(
                "  {name:<16} {addr:<16} [{:<10}] [{}]{star}",
                cfg.signer,
                config::env_name(cfg.env),
            );
        } else {
            println!("  {name:<16} [no config]{star}");
        }
    }
    println!("\n  * = default");
    Ok(())
}

/// Remove a single profile directory.
pub fn remove(name: &str) -> xmtp::Result<()> {
    let dir = config::profile_dir(name);
    if !dir.exists() {
        println!("Profile '{name}' does not exist.");
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(|e| xmtp::Error::Ffi(format!("remove: {e}")))?;
    println!("Removed profile '{name}'.");
    Ok(())
}

/// Remove ALL profiles after confirmation from stdin.
pub fn clear() -> xmtp::Result<()> {
    let base = config::data_dir();
    if !base.exists() {
        println!("Nothing to clear.");
        return Ok(());
    }

    eprint!(
        "This will delete ALL data in {}.  Continue? [y/N] ",
        base.display()
    );
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| xmtp::Error::Ffi(format!("stdin: {e}")))?;

    if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
        println!("Aborted.");
        return Ok(());
    }

    fs::remove_dir_all(&base).map_err(|e| xmtp::Error::Ffi(format!("clear: {e}")))?;
    println!("All profiles deleted.");
    Ok(())
}

/// Show or set the default profile.
pub fn default(name: Option<&str>) -> xmtp::Result<()> {
    match name {
        Some(name) => {
            if !config::profile_dir(name).exists() {
                return Err(xmtp::Error::InvalidArgument(format!(
                    "profile '{name}' does not exist"
                )));
            }
            config::set_default(name)?;
            println!("Default profile set to '{name}'.");
        }
        None => println!("{}", config::default_profile()),
    }
    Ok(())
}

/// Decode a hex string and write as identity.key.
fn import_hex_key(hex_str: &str, path: &std::path::Path) -> xmtp::Result<()> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    if hex_str.len() != 64 {
        return Err(xmtp::Error::InvalidArgument(format!(
            "key must be 64 hex chars (32 bytes), got {}",
            hex_str.len()
        )));
    }
    let bytes: Vec<u8> = (0..hex_str.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex_str[i..i + 2], 16))
        .collect::<Result<_, _>>()
        .map_err(|e| xmtp::Error::InvalidArgument(format!("invalid hex: {e}")))?;
    fs::write(path, &bytes).map_err(|e| xmtp::Error::Ffi(format!("write key: {e}")))
}

/// Load an existing key file or generate a new random key.
fn load_or_create_key(path: &std::path::Path) -> xmtp::Result<AlloySigner> {
    let key: [u8; 32] = if path.exists() {
        let bytes = fs::read(path).map_err(|e| xmtp::Error::Ffi(format!("read key: {e}")))?;
        bytes
            .try_into()
            .map_err(|_| xmtp::Error::InvalidArgument("key file must be 32 bytes".into()))?
    } else {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| xmtp::Error::Ffi(format!("rng: {e}")))?;
        fs::write(path, key).map_err(|e| xmtp::Error::Ffi(format!("write key: {e}")))?;
        key
    };
    AlloySigner::from_bytes(&key)
}
