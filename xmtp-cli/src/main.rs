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
mod decode;
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
use crate::event::{Cmd as WorkerCmd, Event};

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    let cli = Cli::parse();
    let json_mode = cli.command.as_ref().is_some_and(Command::is_json);

    if let Err(e) = run(cli) {
        let _ = tui::restore();
        if json_mode {
            println!("{}", serde_json::json!({"error": e.to_string()}));
        } else {
            eprintln!("fatal: {e}");
        }
        process::exit(1);
    }
}

fn run(cli: Cli) -> xmtp::Result<()> {
    if let Some(ref command) = cli.command {
        return dispatch(command);
    }

    // TUI mode — resolve profile, auto-create default if needed.
    let name = resolve_profile(cli.profile);

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

    if cfg.address.is_empty() {
        eprintln!("  Migrating profile '{name}'");
        let (migrated, _) = config::open_client(&name)?;
        cfg = migrated;
    }

    run_tui(&name, &cfg)
}

fn dispatch(command: &Command) -> xmtp::Result<()> {
    match command {
        Command::New(args) => {
            cmd::profile::create(args)?;
            Ok(())
        }
        Command::List { output } => cmd::profile::list(output.json),
        Command::Remove { name } => cmd::profile::remove(name),
        Command::Clear => cmd::profile::clear(),
        Command::Default { name, output } => cmd::profile::default(name.as_deref(), output.json),
        Command::Info { name, output } => {
            cmd::inspect::info(&resolve_profile(name.clone()), output.json)
        }
        Command::Revoke { name } => cmd::inspect::revoke(&resolve_profile(name.clone())),

        Command::Conversations {
            consent,
            profile,
            output,
        } => cmd::agent::conversations(
            &resolve_profile(profile.clone()),
            consent.as_deref(),
            output.json,
        ),
        Command::Messages {
            conv,
            limit,
            profile,
            output,
        } => cmd::agent::messages(&resolve_profile(profile.clone()), conv, *limit, output.json),
        Command::Send {
            conv,
            text,
            with_push,
            profile,
            output,
        } => cmd::agent::send(&resolve_profile(profile.clone()), conv, text, with_push, output.json),
        Command::Dm {
            address,
            profile,
            output,
        } => cmd::agent::dm(&resolve_profile(profile.clone()), address, output.json),
        Command::CreateGroup {
            members,
            name,
            profile,
            output,
        } => cmd::agent::create_group(
            &resolve_profile(profile.clone()),
            members,
            name.as_deref(),
            output.json,
        ),
        Command::Members {
            conv,
            profile,
            output,
        } => cmd::agent::members(&resolve_profile(profile.clone()), conv, output.json),
        Command::CanMessage {
            addresses,
            profile,
            output,
        } => cmd::agent::can_message(&resolve_profile(profile.clone()), addresses, output.json),
        Command::Request {
            conv,
            action,
            profile,
            output,
        } => cmd::agent::request(&resolve_profile(profile.clone()), conv, action, output.json),
        Command::Stream { kind, profile } => {
            cmd::agent::stream_events(&resolve_profile(profile.clone()), kind)
        }
    }
}

fn resolve_profile(explicit: Option<String>) -> String {
    explicit.unwrap_or_else(config::default_profile)
}

fn run_tui(name: &str, cfg: &config::ProfileConfig) -> xmtp::Result<()> {
    let inbox_id = xmtp::generate_inbox_id(&cfg.address, xmtp::IdentifierKind::Ethereum, 1)?;
    let env_name = config::env_name(cfg.env).to_owned();

    eprintln!("  address: {}", cfg.address);
    eprintln!("  inbox:   {inbox_id}");
    eprintln!("  env:     {env_name}");
    eprintln!("  Starting TUI");

    let (event_tx, event_rx) = mpsc::channel::<Event>();
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();

    event::spawn_poller(event_tx.clone(), Duration::from_millis(50));

    let worker_tx = event_tx;
    let worker_cmd_tx = cmd_tx.clone();
    let profile = name.to_owned();
    std::thread::spawn(move || {
        worker::run(cmd_rx, worker_tx, worker_cmd_tx, &profile);
    });

    let mut app = App::new(cfg.address.clone(), inbox_id, env_name, cmd_tx);

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
