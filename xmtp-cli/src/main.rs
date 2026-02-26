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
mod cmd;
mod event;
mod tui;
mod ui;
mod worker;

use std::process;
use std::sync::mpsc;
use std::time::Duration;

use clap::Parser;

use crate::app::App;
use crate::cmd::config;
use crate::cmd::{Cli, Command};
use crate::event::{Cmd, Event};

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
    let cli = Cli::parse();

    // Dispatch subcommands.
    if let Some(ref command) = cli.command {
        return dispatch(command);
    }

    // TUI mode — resolve profile, auto-create default if needed.
    let name = resolve_profile(cli.profile);

    // New profile: must register on main thread (requires signer + network).
    if !config::profile_dir(&name).join("profile.conf").exists() {
        eprintln!("  Creating profile '{name}'");
        cmd::profile::create(&cmd::NewArgs {
            name: name.clone(),
            env: xmtp::Env::Dev,
            rpc_url: xmtp::DEFAULT_RPC.into(),
            import: None,
            key: None,
            db: None,
            ledger: None,
        })?;
    }

    let mut cfg = config::ProfileConfig::load(&name)?;

    // Legacy profile without cached address — one-time migration.
    if cfg.address.is_empty() {
        eprintln!("  Migrating profile '{name}'");
        let (migrated, _) = config::open_client(&name)?;
        cfg = migrated;
    }

    // inbox_id is pure computation — no network required.
    let inbox_id = xmtp::generate_inbox_id(&cfg.address, xmtp::IdentifierKind::Ethereum, 1)?;
    let env_name = config::env_name(cfg.env).to_owned();
    let db_path = config::profile_dir(&name)
        .join("messages.db3")
        .to_string_lossy()
        .into_owned();

    eprintln!("  address: {}", cfg.address);
    eprintln!("  inbox:   {inbox_id}");
    eprintln!("  env:     {env_name}");
    eprintln!("  Starting TUI");
    run_tui(
        cfg.env,
        db_path,
        cfg.address,
        inbox_id,
        env_name,
        cfg.rpc_url,
    )
}

fn dispatch(command: &Command) -> xmtp::Result<()> {
    match command {
        Command::New(args) => {
            cmd::profile::create(args)?;
            Ok(())
        }
        Command::List => cmd::profile::list(),
        Command::Remove { name } => cmd::profile::remove(name),
        Command::Clear => cmd::profile::clear(),
        Command::Default { name } => cmd::profile::default(name.as_deref()),
        Command::Info { name } => cmd::inspect::info(&resolve_profile(name.clone())),
        Command::Revoke { name } => cmd::inspect::revoke(&resolve_profile(name.clone())),
    }
}

/// Resolve profile name: explicit or default.
fn resolve_profile(explicit: Option<String>) -> String {
    explicit.unwrap_or_else(config::default_profile)
}

fn run_tui(
    env: xmtp::Env,
    db_path: String,
    address: String,
    inbox_id: String,
    env_name: String,
    rpc_url: String,
) -> xmtp::Result<()> {
    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();

    event::spawn_poller(event_tx.clone(), Duration::from_millis(50));

    // Client construction + sync happen on the worker thread so the TUI
    // renders immediately — typically under 100 ms from command to first frame.
    let worker_tx = event_tx;
    let worker_cmd_tx = cmd_tx.clone();
    let worker_addr = address.clone();
    std::thread::spawn(move || {
        worker::run(
            cmd_rx,
            worker_tx,
            worker_cmd_tx,
            env,
            db_path,
            rpc_url,
            worker_addr,
        );
    });

    let mut app = App::new(address, inbox_id, env_name, cmd_tx);

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
