mod account;
mod agent;
mod app;
mod compose;
mod composer;
mod config;
mod demo;
mod fzt;
mod icons;
mod imap;
mod info;
#[cfg(test)]
mod live;
mod logfile;
mod mail;
mod mcp;
mod net;
mod notify;
mod oauth;
mod palette;
mod pop3;
mod secrets;
mod settings;
mod setup;
mod smtp;
mod store;
mod tags;
mod text;
mod theme;
mod threads;
mod tools;
mod ui;
mod utf7;
mod worker;

use std::collections::HashMap;
use std::io::{Write, stdout};

use anyhow::{Context, Result, bail};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;

use crate::account::{Auth, PasswordFrom};

fn main() -> Result<()> {
    // Before any thread exists: PATH keeps only its absolute folders.
    // tuimail names every program it starts by its full path, but code it
    // uses starts some by name (crossterm's `tput`), and so does xdg-open:
    // an empty or relative entry would let one planted in the folder
    // tuimail was started from run.
    #[cfg(unix)]
    match tools::absolute_path_entries(std::env::var_os("PATH")) {
        // SAFETY: first thing in main, before the runtime or any thread.
        Some(path) => unsafe { std::env::set_var("PATH", path) },
        // SAFETY: as above.
        None => unsafe { std::env::remove_var("PATH") },
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    info::read_installed_path();
    config::load_dotenv();
    let mut args = std::env::args().skip(1);
    let mut check = false;
    match args.next().as_deref() {
        None => {}
        Some("--check") => check = true,
        Some("--demo") => return demo::run().await,
        // An agent's: stdout is the agent's, for MCP only.
        Some("mcp") => {
            let demo = match args.next().as_deref() {
                None => false,
                Some("--demo") => true,
                Some(other) => bail!(
                    "unknown argument {:?}; see tuimail --help",
                    text::clean(other)
                ),
            };
            return mcp::run(config::Config::load()?.data_dir, demo).await;
        }
        Some("-h" | "--help") => return print(&help()?),
        Some("-V" | "--version") => {
            return print(&format!("tuimail {}\n", info::VERSION));
        }
        Some(other) => bail!(
            "unknown argument {:?}; see tuimail --help",
            text::clean(other)
        ),
    }

    // Everything that can fail does so before the terminal is taken over,
    // so errors print as normal text.
    let config = config::Config::load()?;
    let settings_path = settings::path(&config.data_dir);
    let (settings, unknown_settings) = settings::Settings::load_checked(&settings_path)?;
    let (accounts, warnings) = account::load(&account::path(&config.data_dir))?;
    let keychain = secrets::init_keychain();
    if check {
        return print_check(&config, &accounts, &warnings, keychain);
    }
    let unlogged = logfile::open(&config.data_dir, accounts.len()).err();
    let store = store::Store::open(&config.data_dir.join("tuimail.db"))
        .inspect_err(|e| logfile::failed("opening the cache", None, e))?;

    // Password commands run now, with the terminal, so one that needs to
    // be unlocked (gpg, a password manager) can ask. What fails is said
    // once the app is up: printed here, it'd be under the full screen.
    let mut notes = Vec::new();
    let mut passwords = HashMap::new();
    for (i, account) in accounts.iter().enumerate() {
        let command = account.password == PasswordFrom::Command
            || (account.auth == Auth::Oauth2 && !oauth::built_in(account));
        if command {
            match secrets::run_command(&account.password_command, true).await {
                Ok(secret) if account.auth != Auth::Oauth2 => {
                    passwords.insert(account.id(), secret);
                }
                Ok(_) => {}
                Err(e) => {
                    logfile::failed("running password_command", Some(i + 1), &e);
                    notes.push(format!("{}: {e:#}", account.shown_name()));
                }
            }
        }
    }

    need_terminal()?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut terminal = ratatui::init();
    panic_hook();
    // A pasted path arrives as one event instead of keystrokes.
    execute!(stdout(), EnableBracketedPaste)?;
    let mut app = app::App::with_passwords(
        store,
        settings,
        accounts,
        passwords,
        keychain,
        Some(config.data_dir.clone()),
        tx,
    );
    app.agent_socket = Some(agent::socket(&config.data_dir, false));
    notes.extend(warnings);
    if let Some(e) = unlogged {
        notes.push(format!("{}: {e}", logfile::FILE));
    }
    if !unknown_settings.is_empty() {
        notes.push(format!(
            "settings.toml: tuimail doesn't know {}, so it's ignored",
            unknown_settings.join(", ")
        ));
    }
    if let Some(note) = notes.first() {
        app.error(note.clone());
    }
    let result = app.run(&mut terminal, rx).await;
    drop(app);
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    if let Err(e) = &result {
        logfile::failed("running", None, e);
    }
    logfile::stopped();
    result
}

/// A panic anywhere but in parsing or laying out a message (caught there)
/// ends the app: the terminal is put back, then the message is printed
/// without control characters, since it can quote a message. For the same
/// reason the log gets only where it happened.
pub fn panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        logfile::panicked(info.location(), mail::panic_is_contained());
        if mail::panic_is_contained() {
            return;
        }
        let _ = execute!(stdout(), DisableBracketedPaste, crossterm::cursor::Show);
        ratatui::restore();
        eprintln!("tuimail crashed: {}", text::clean(&info.to_string()));
        std::process::exit(101);
    }));
}

/// `tuimail --check`: where things are and what tuimail found.
fn print_check(
    config: &config::Config,
    accounts: &[account::Account],
    warnings: &[String],
    keychain: bool,
) -> Result<()> {
    let mut out = format!(
        "tuimail {}\ndata folder  {}\nlog          {}\nkeychain     {}\n",
        info::VERSION,
        config::shown(&config.data_dir),
        config::shown(&config.data_dir.join(logfile::FILE)),
        if keychain {
            "the system's"
        } else {
            "none: passwords are asked for at each start"
        }
    );
    if accounts.is_empty() {
        out.push_str("accounts     none yet: start tuimail to add one\n");
    }
    for (i, a) in accounts.iter().enumerate() {
        let protocol = match a.protocol {
            account::Protocol::Imap => "IMAP",
            account::Protocol::Pop3 => "POP3",
        };
        let signs_in = oauth::provider_for(a)
            .filter(|_| oauth::built_in(a))
            .map(|p| match a.password {
                PasswordFrom::Keychain => {
                    format!("signs in with {} (kept in the keychain)", p.name())
                }
                _ => format!("signs in with {} (each start)", p.name()),
            });
        let password = match (a.auth, a.password) {
            _ if signs_in.is_some() => signs_in.as_deref().unwrap_or_default(),
            (Auth::Oauth2, _) => "OAuth2 token from password_command",
            (_, PasswordFrom::Keychain) => "keychain",
            (_, PasswordFrom::Ask) => "asked",
            (_, PasswordFrom::Command) => "password_command",
        };
        out.push_str(&format!(
            "account {:<4} {} <{}>\n             {protocol} {}:{} ({:?}), sending {}:{} ({:?}), password: {password}\n",
            i + 1,
            a.shown_name(),
            text::one_line(&a.email, 80),
            text::one_line(&a.host, 80),
            a.port(),
            a.security,
            text::one_line(&a.smtp_host, 80),
            a.smtp_port(),
            a.smtp_security,
        ));
    }
    for warning in warnings {
        out.push_str(&format!("warning      {warning}\n"));
    }
    print(&out)
}

/// A plain error, before the screen is taken over, when tuimail isn't run
/// in a terminal (piped, or from a script): there'd be nothing to draw on.
pub fn need_terminal() -> Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !stdout().is_terminal() {
        bail!("tuimail needs a terminal to run in (tuimail --help says more)");
    }
    Ok(())
}

/// Writes to stdout. A reader that stops early (`tuimail --help | head -1`)
/// is fine, where `println!` would panic on the closed pipe.
fn print(text: &str) -> Result<()> {
    match stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result.context("cannot write")?),
    }
}

fn help() -> Result<String> {
    Ok(format!(
        "tuimail {version}
Email in your terminal: IMAP and POP3 to read, SMTP to send, with arrow
keys or vim keys.

Usage: tuimail [-h | --help] [-V | --version] [--check] [--demo]
       tuimail mcp [--demo]

Inside the app, ? lists the keys and q quits. The first start asks for
an account: your address and password, the servers filled in for the
usual providers. --check says what tuimail found; --demo shows made-up
mail, connecting nowhere.

tuimail mcp is for AI agents (Claude Code, Claude Desktop, Cursor, ...):
an MCP server that reads and sorts your mail through the tuimail you
have open, once AI agents is on in its settings. Agents can mark, star,
tag, move and move to Trash; each change shows, can be undone with z,
and asks you first unless you turn that off. Nothing is deleted or sent.
tuimail mcp --demo reaches tuimail --demo instead, to try it on made-up
mail.

Passwords are kept in the system's keychain (or asked for each start,
or printed by a command of yours: see the README). Accounts, settings
and a cache of your mail are kept only on this computer, in:
  {data}
tuimail.log there says what failed and when, with no mail in it, and
calls accounts by the numbers --check gives them.

Environment:
  TM_DATA_DIR   keep them somewhere else (a full path)
  TM_ICONS      which icons: auto, nerd (Nerd Font) or plain
                (also `icons` in settings.toml)
",
        version = info::VERSION,
        data = config::shown(&config::data_dir()?),
    ))
}
