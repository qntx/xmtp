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
use xmtp::Client;

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

    if !config::profile_dir(&name).join("profile.conf").exists() {
        eprintln!("Creating profile '{name}'...");
        cmd::profile::create(&cmd::NewArgs {
            name: name.clone(),
            env: xmtp::Env::Dev,
            rpc_url: "https://eth.llamarpc.com".into(),
            import: None,
            key: None,
            db: None,
            ledger: None,
        })?;
    }

    let (_cfg, signer, client) = config::open(&name)?;
    let address = signer.identifier().address;
    let inbox_id = client.inbox_id()?;

    eprintln!("address: {address}");
    eprintln!("inbox:   {inbox_id}");
    run_tui(client, address, inbox_id)
}

fn dispatch(command: &Command) -> xmtp::Result<()> {
    match command {
        Command::New(args) => cmd::profile::create(args),
        Command::List => cmd::profile::list(),
        Command::Remove { name } => cmd::profile::remove(name),
        Command::Clear => cmd::profile::clear(),
        Command::Default { name } => cmd::profile::default(name.as_deref()),
        Command::Info { profile } => cmd::inspect::info(&resolve_profile(profile.clone())),
        Command::Revoke { profile } => cmd::inspect::revoke(&resolve_profile(profile.clone())),
    }
}

/// Resolve profile name: explicit or default.
fn resolve_profile(explicit: Option<String>) -> String {
    explicit.unwrap_or_else(config::default_profile)
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
