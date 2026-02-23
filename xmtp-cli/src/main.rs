//! xmtp-cli — Interactive XMTP TUI chat client.
//!
//! Architecture: **main thread = UI only**, **worker thread = all FFI**.
//! Stream callbacks route through the worker via [`Cmd`], never blocking the UI.

#![allow(
    missing_docs,
    missing_debug_implementations,
    clippy::print_stderr,
    clippy::print_stdout
)]

mod app;
mod event;
mod tui;
mod ui;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, process};

use clap::Parser;
use xmtp::{AlloySigner, Client, EnsResolver, Env, LedgerSigner, Signer};

use crate::app::{App, truncate_id};
use crate::event::{Cmd, Event};

/// Interactive XMTP TUI chat client.
///
/// Supports Ethereum addresses (0x…), ENS names (name.eth), and XMTP Inbox IDs
/// as recipient identifiers for DMs and group conversations.
#[derive(Parser)]
#[command(name = "xmtp", version, about, long_about = None)]
struct Args {
    /// Profile name — keys and database are stored under this name
    /// in the platform data directory.
    #[arg(default_value = "default")]
    profile: String,

    /// Path to private key file (overrides profile default).
    #[arg(short = 'k', long = "key")]
    key_path: Option<PathBuf>,

    /// Path to database file (overrides profile default).
    #[arg(short = 'd', long = "db")]
    db_path: Option<PathBuf>,

    /// XMTP network environment.
    #[arg(short, long, default_value = "dev", value_parser = parse_env)]
    env: Env,

    /// Ethereum RPC URL for ENS name resolution.
    #[arg(long, default_value = "https://eth.llamarpc.com")]
    rpc_url: String,

    /// Disable ENS name resolution.
    #[arg(long)]
    no_ens: bool,

    /// Import a hex-encoded private key into the profile.
    #[arg(long = "import")]
    import_hex: Option<String>,

    /// Show identity information and exit.
    #[arg(long)]
    info: bool,

    /// List all installations for this identity and exit.
    #[arg(long)]
    installations: bool,

    /// Revoke all installations except the current one and exit.
    #[arg(long)]
    revoke_others: bool,

    /// Use a Ledger hardware wallet for signing instead of a local key file.
    #[arg(long)]
    ledger: bool,

    /// Ledger account index (Ledger Live HD path, default: 0).
    #[arg(long, default_value = "0")]
    ledger_index: usize,
}

fn parse_env(s: &str) -> Result<Env, String> {
    match s.to_ascii_lowercase().as_str() {
        "dev" | "development" => Ok(Env::Dev),
        "prod" | "production" => Ok(Env::Production),
        "local" | "localhost" => Ok(Env::Local),
        _ => Err(format!(
            "unknown environment: {s} (expected: dev, production, local)"
        )),
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    if let Err(e) = run() {
        let _ = tui::restore();
        eprintln!("fatal: {e}");
        process::exit(1);
    }
}

fn run() -> xmtp::Result<()> {
    let args = Args::parse();
    let (key_path, db_path) = resolve_paths(&args);

    // Validate conflicting options.
    if args.ledger && args.import_hex.is_some() {
        return Err(xmtp::Error::InvalidArgument(
            "--ledger and --import are mutually exclusive".into(),
        ));
    }

    // Handle key import (file signer only).
    if let Some(ref hex) = args.import_hex {
        import_key(hex, &key_path)?;
        eprintln!("Imported key to {}", key_path.display());
    }

    // Create signer — either Ledger hardware wallet or local key file.
    let signer: Box<dyn Signer> = if args.ledger {
        eprintln!("Connecting to Ledger (index {})...", args.ledger_index);
        Box::new(LedgerSigner::new(args.ledger_index)?)
    } else {
        Box::new(load_or_create_signer(&key_path)?)
    };
    let address = signer.identifier().address;
    let client = create_client(signer.as_ref(), &args, &db_path)?;
    let inbox_id = client.inbox_id()?;

    // Non-TUI commands — print output and exit.
    if args.info {
        print_info(&address, &inbox_id, &args, &key_path, &db_path);
        return Ok(());
    }
    if args.installations {
        return print_installations(&client, &inbox_id);
    }
    if args.revoke_others {
        return run_revoke_others(&client, signer.as_ref());
    }

    // TUI mode.
    eprintln!("address: {address}");
    eprintln!("inbox:   {inbox_id}");
    run_tui(client, address, inbox_id)
}

/// Resolve key and database paths from explicit flags or profile defaults.
fn resolve_paths(args: &Args) -> (PathBuf, PathBuf) {
    let dir = profile_dir(&args.profile);
    let key = args
        .key_path
        .clone()
        .unwrap_or_else(|| dir.join("identity.key"));
    let db = args
        .db_path
        .clone()
        .unwrap_or_else(|| dir.join("messages.db3"));
    (key, db)
}

/// Platform-specific data directory for a named profile.
fn profile_dir(profile: &str) -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("xmtp-cli")
        .join(profile)
}

fn print_info(address: &str, inbox_id: &str, args: &Args, key_path: &Path, db_path: &Path) {
    let env_name = match args.env {
        Env::Dev => "dev",
        Env::Production => "production",
        Env::Local => "local",
    };
    println!("Profile:       {}", args.profile);
    println!("Environment:   {env_name}");
    println!("Address:       {address}");
    println!("Inbox ID:      {inbox_id}");
    if args.ledger {
        println!("Signer:        Ledger (index {})", args.ledger_index);
    } else {
        println!("Key file:      {}", key_path.display());
    }
    println!("Database:      {}", db_path.display());
}

fn print_installations(client: &Client, inbox_id: &str) -> xmtp::Result<()> {
    let current = client.installation_id()?;
    let states = client.inbox_state(true)?;
    let ids: Vec<&str> = states
        .iter()
        .flat_map(|s| s.installation_ids.iter().map(String::as_str))
        .collect();

    println!("Installations for inbox {inbox_id}\n");
    println!("  #  Installation ID                             Status");
    println!("  ─  ─────────────────────────────────────────── ──────");
    for (i, id) in ids.iter().enumerate() {
        let tag = if *id == current { " ← current" } else { "" };
        let display = truncate_id(id, 44);
        println!("  {}  {display:<44}  active{tag}", i + 1);
    }
    println!("\nTotal: {} / 10", ids.len());
    Ok(())
}

fn run_revoke_others(client: &Client, signer: &dyn Signer) -> xmtp::Result<()> {
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
    client.revoke_all_other_installations(signer)?;
    println!("Done. Only current installation remains.");
    Ok(())
}

fn run_tui(client: Client, address: String, inbox_id: String) -> xmtp::Result<()> {
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();

    event::spawn_poller(event_tx.clone(), Duration::from_millis(50));
    let _streams = worker::start_streams(&client, &cmd_tx)?;

    let worker_tx = event_tx;
    std::thread::spawn(move || worker::run(client, cmd_rx, worker_tx));

    let _ = cmd_tx.send(Cmd::Sync);

    let mut app = App::new(address, inbox_id, cmd_tx);

    tui::install_panic_hook();
    let mut terminal = tui::init().map_err(|e| xmtp::Error::Ffi(format!("terminal: {e}")))?;

    while !app.quit {
        terminal
            .draw(|f| ui::render(&mut app, f))
            .map_err(|e| xmtp::Error::Ffi(format!("render: {e}")))?;

        match event_rx.recv() {
            Ok(Event::Key(k)) => app.handle_key(k),
            Ok(Event::Tick) => app.tick(),
            Ok(Event::Resize) => {}
            Ok(ev) => app.apply(ev),
            Err(_) => break,
        }
    }

    tui::restore().map_err(|e| xmtp::Error::Ffi(format!("restore: {e}")))
}

/// Import a hex-encoded private key into the given path.
fn import_key(hex_str: &str, path: &Path) -> xmtp::Result<()> {
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
    ensure_parent(path)?;
    fs::write(path, &bytes).map_err(|e| xmtp::Error::Ffi(format!("write key: {e}")))
}

/// Load an existing private key or generate a new one.
fn load_or_create_signer(path: &Path) -> xmtp::Result<AlloySigner> {
    let key: [u8; 32] = if path.exists() {
        let bytes = fs::read(path).map_err(|e| xmtp::Error::Ffi(format!("read key: {e}")))?;
        bytes
            .try_into()
            .map_err(|_| xmtp::Error::InvalidArgument("key file must be 32 bytes".into()))?
    } else {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| xmtp::Error::Ffi(format!("rng: {e}")))?;
        ensure_parent(path)?;
        fs::write(path, key).map_err(|e| xmtp::Error::Ffi(format!("write key: {e}")))?;
        key
    };
    AlloySigner::from_bytes(&key)
}

/// Build and register an XMTP client with ENS resolver configured.
fn create_client(signer: &dyn Signer, args: &Args, db_path: &Path) -> xmtp::Result<Client> {
    ensure_parent(db_path)?;
    let db = db_path.to_string_lossy();

    let build = |db: &str| {
        let mut builder = Client::builder().env(args.env).db_path(db);
        if !args.no_ens
            && let Ok(resolver) = EnsResolver::new(&args.rpc_url)
        {
            builder = builder.resolver(resolver);
        }
        builder.build(signer)
    };

    match build(&db) {
        Ok(c) => Ok(c),
        // Stale DB with different InboxId — recreate.
        Err(e) if e.to_string().contains("does not match the stored InboxId") => {
            for ext in ["", "-shm", "-wal"] {
                let _ = fs::remove_file(format!("{db}{ext}"));
            }
            build(&db)
        }
        Err(e) => Err(e),
    }
}

/// Ensure the parent directory of a path exists.
fn ensure_parent(path: &Path) -> xmtp::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| xmtp::Error::Ffi(format!("create dir: {e}")))?;
    }
    Ok(())
}
