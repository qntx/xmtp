//! Identity inspection commands: info (merged with installations), revoke.

use crate::app::truncate_id;

use super::config::{self, SignerKind, env_name};

/// Show profile information and all installations.
pub fn info(profile: &str) -> xmtp::Result<()> {
    let (cfg, signer, client) = config::open(profile)?;
    let address = signer.identifier().address;
    let inbox_id = client.inbox_id()?;

    // Profile info.
    println!("Profile:       {profile}");
    println!("Environment:   {}", env_name(cfg.env));
    println!("Address:       {address}");
    println!("Inbox ID:      {inbox_id}");
    match cfg.signer {
        SignerKind::File => {
            let key = config::profile_dir(profile).join("identity.key");
            println!("Signer:        key file ({})", key.display());
        }
        SignerKind::Ledger(i) => {
            println!("Signer:        Ledger (index {i})");
        }
    }
    println!(
        "Database:      {}",
        config::profile_dir(profile).join("messages.db3").display()
    );

    // Installations.
    let current = client.installation_id()?;
    let states = client.inbox_state(true)?;
    let ids: Vec<&str> = states
        .iter()
        .flat_map(|s| s.installation_ids.iter().map(String::as_str))
        .collect();

    println!("\nInstallations ({} / 10):\n", ids.len());
    for (i, id) in ids.iter().enumerate() {
        let tag = if *id == current { " ← current" } else { "" };
        let display = truncate_id(id, 44);
        println!("  {}  {display:<44}  active{tag}", i + 1);
    }
    Ok(())
}

/// Revoke all installations except the current one.
pub fn revoke(profile: &str) -> xmtp::Result<()> {
    let (_cfg, signer, client) = config::open(profile)?;

    let current = client.installation_id()?;
    let states = client.inbox_state(true)?;
    let count = states
        .iter()
        .flat_map(|s| &s.installation_ids)
        .filter(|id| id.as_str() != current)
        .count();

    if count == 0 {
        println!("No other installations to revoke.");
        return Ok(());
    }

    println!("Revoking {count} other installation(s)...");
    client.revoke_all_other_installations(signer.as_ref())?;
    println!("Done. Only current installation remains.");
    Ok(())
}
