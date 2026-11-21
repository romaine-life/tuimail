//! `tuimail session`: the house's one sign-in to each mail account, serving the PCs' tuimail
//! over the relay ([`super`]). It runs on the home cluster, its accounts from the same
//! `accounts.toml` the PCs have (OpenBao's `secret/home/tuimail`), what they sign in with in its
//! encrypted store ([`super::vault`]), its log on stdout.
//!
//! Each PC that opens an account gets its own worker for it here, with the account's shared
//! [`Credentials`]: one sign-in, one access token renewed for every PC, each PC's requests and
//! events its own (a check answers the cache of the PC that asked).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::token::{Checked, TokenChecker};
use super::vault::{self, Key, Vault};
use super::{
    BACKUP, BACKUP_DEVICE, FromSession, HEALTH, MAX_MESSAGE, PATH, ToSession, decode, encode,
};
use crate::account::{self, Account};
use crate::oauth::Tokens;
use crate::secrets::{Credentials, Secret};
use crate::worker::{self, Endpoints, Event, Worker};
use crate::{config, logfile, net, text};

/// Where it listens, keeps its store, finds its key and its accounts.
#[derive(Debug, PartialEq)]
pub struct Options {
    pub listen: SocketAddr,
    pub state: PathBuf,
    pub key_file: PathBuf,
    pub accounts: PathBuf,
}

impl Options {
    pub const USAGE: &str = "tuimail session --listen ADDRESS:PORT --state FOLDER \
                             --key-file FILE --accounts FILE (or --self-test)";

    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let (mut listen, mut state, mut key_file, mut accounts) = (None, None, None, None);
        while let Some(arg) = args.next() {
            let value = args.next().with_context(|| {
                format!(
                    "{} needs a value; usage: {}",
                    text::clean(&arg),
                    Self::USAGE
                )
            })?;
            match arg.as_str() {
                "--listen" => {
                    listen = Some(value.parse().with_context(|| {
                        format!("--listen takes ADDRESS:PORT, not {:?}", text::clean(&value))
                    })?)
                }
                "--state" => state = Some(PathBuf::from(value)),
                "--key-file" => key_file = Some(PathBuf::from(value)),
                "--accounts" => accounts = Some(PathBuf::from(value)),
                other => bail!(
                    "unknown argument {:?}; usage: {}",
                    text::clean(other),
                    Self::USAGE
                ),
            }
        }
        let missing = |name: &str| format!("{name} is needed; usage: {}", Self::USAGE);
        Ok(Self {
            listen: listen.with_context(|| missing("--listen"))?,
            state: state.with_context(|| missing("--state"))?,
            key_file: key_file.with_context(|| missing("--key-file"))?,
            accounts: accounts.with_context(|| missing("--accounts"))?,
        })
    }
}

/// An account as the session serves it.
pub(super) struct Served {
    pub(super) creds: Credentials,
    pub(super) endpoints: Endpoints,
}

/// What every PC's connection shares.
pub struct Shared {
    pub(super) accounts: HashMap<String, Served>,
    /// The store, where the archive is; none in tests that need no archive.
    vault: Option<Arc<Vault>>,
    /// Where snapshots for the backup are made, a moment each: the state folder.
    pub snapshots: Option<PathBuf>,
    /// Their ids, in `accounts.toml`'s order.
    order: Vec<String>,
    checker: TokenChecker,
}

impl Shared {
    /// The accounts the session can serve, what each signs in with read from `vault` and kept
    /// there. An account whose password comes from a command, or whose certificate can't be
    /// read here, is left out (and logged by its place).
    pub async fn new(
        accounts: Vec<Account>,
        vault: Option<Arc<Vault>>,
        checker: TokenChecker,
    ) -> Result<Self> {
        let mut served = HashMap::new();
        let mut order = Vec::new();
        for (i, account) in accounts.into_iter().enumerate() {
            let number = i + 1;
            if account.problem().is_some() {
                logfile::note("left out: the account isn't usable", Some(number));
                continue;
            }
            let tls = match net::tls_config(account.certificate.as_deref()) {
                Ok(tls) => tls,
                Err(e) => {
                    logfile::failed("reading the certificate", Some(number), &e);
                    continue;
                }
            };
            let account = Arc::new(account);
            let id = account.id();
            let endpoints = Endpoints::of(&account, tls);
            let mut creds = Credentials::new(account, false).numbered(number);
            if creds.uses_command() {
                logfile::note("left out: its password comes from a command", Some(number));
                continue;
            }
            if let Some(vault) = &vault {
                let (kept, key) = (vault.clone(), id.clone());
                creds = creds.keeping(Arc::new(move |secret| kept.keep(&key, secret)));
                if let Some(secret) = vault.credential(&id)? {
                    give(&creds, secret).await;
                }
            }
            order.push(id.clone());
            served.insert(id, Served { creds, endpoints });
        }
        Ok(Self {
            accounts: served,
            snapshots: None,
            vault,
            order,
            checker,
        })
    }
}

/// Uses `secret` for the account from now on: a refresh token if it signs in, else its
/// password.
async fn give(creds: &Credentials, secret: Secret) {
    if creds.signs_in() {
        creds.set_tokens(Tokens {
            // None yet: the first login renews it with the refresh token.
            access: Secret::new(""),
            refresh: Some(secret),
            renew_at: Instant::now(),
        });
    } else {
        creds.set(secret).await;
    }
}

/// `tuimail session ...`: the session, or with `--self-test` alone the image's smoke test.
pub async fn main(args: impl Iterator<Item = String>) -> Result<()> {
    let args: Vec<String> = args.collect();
    if args == ["--self-test"] {
        return super::selftest::run().await;
    }
    run(Options::parse(args.into_iter())?).await
}

pub async fn run(options: Options) -> Result<()> {
    panic_hook();
    let (accounts, _warnings) = account::load(&options.accounts)?;
    logfile::to_stdout(accounts.len());
    let state = config::make_private_dir(&options.state)?;
    let key = Key::read(&options.key_file)?;
    let vault = Arc::new(Vault::open(&state.join(vault::FILE), &key)?);
    let mut shared = Shared::new(accounts, Some(vault.clone()), TokenChecker::openbao()).await?;
    shared.snapshots = Some(state.clone());
    let shared = Arc::new(shared);
    // Every account copied into the archive, for good (`archive.rs`).
    let _archiving: Vec<_> = shared
        .accounts
        .values()
        .map(|served| {
            super::archive::keep(
                vault.clone(),
                served.creds.clone(),
                served.endpoints.clone(),
            )
        })
        .collect();
    let listener = TcpListener::bind(options.listen)
        .await
        .with_context(|| format!("cannot listen on {}", options.listen))?;
    serve(listener, shared).await
}

/// Serves the relay and the health check on `listener` until it fails.
pub async fn serve(listener: TcpListener, shared: Arc<Shared>) -> Result<()> {
    let app = Router::new()
        .route(HEALTH, get(|| async { "ok" }))
        .route(PATH, get(upgrade))
        .route(BACKUP, get(backup))
        .with_state(shared);
    axum::serve(listener, app).await?;
    Ok(())
}

/// The token in `Authorization: Bearer`.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

async fn upgrade(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no token").into_response();
    };
    match shared.checker.check(token).await {
        Ok(checked) => ws
            .max_message_size(MAX_MESSAGE)
            .max_frame_size(MAX_MESSAGE)
            .on_upgrade(move |socket| serve_pc(socket, shared, checked)),
        Err(_) => {
            logfile::note("refused a PC: its token isn't good", None);
            (StatusCode::UNAUTHORIZED, "token refused").into_response()
        }
    }
}

/// A snapshot of the store for the NAS's backup job: only with a token OpenBao signed for the
/// NAS's device and audience `tuimail-backup`. Still encrypted with the store's key, which the
/// NAS never has; made, checked whole, given out and removed.
async fn backup(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let Some(token) = bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "no token").into_response();
    };
    match shared
        .checker
        .check_for(token, super::token::BACKUP_AUDIENCE)
        .await
    {
        Ok(checked) if checked.device == BACKUP_DEVICE => {}
        _ => {
            logfile::note("refused a backup: its token isn't the NAS's", None);
            return (StatusCode::UNAUTHORIZED, "token refused").into_response();
        }
    }
    let (Some(vault), Some(dir)) = (shared.vault.clone(), shared.snapshots.clone()) else {
        return (StatusCode::NOT_FOUND, "no store").into_response();
    };
    let name = format!(
        "snapshot-{}.db",
        crate::compose::random_hex().unwrap_or_default()
    );
    let path = dir.join(name);
    let made = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || vault.snapshot(&path)).await
    };
    if !matches!(made, Ok(Ok(()))) {
        let _ = std::fs::remove_file(&path);
        if let Ok(Err(e)) = &made {
            logfile::failed("making a snapshot", None, e);
        }
        return (StatusCode::INTERNAL_SERVER_ERROR, "no snapshot").into_response();
    }
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "no snapshot").into_response(),
    };
    let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    // Open, it's read to its end; gone from the folder now, it's gone for good once it is.
    let _ = std::fs::remove_file(&path);
    logfile::note("gave the NAS a snapshot", None);
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, size.to_string()),
        ],
        body,
    )
        .into_response()
}

/// When a token that runs out at `expires` (Unix seconds) has run out.
/// With the same few seconds' grace the token check allows, so a renewal on its way when the
/// last token runs out still arrives.
fn deadline(expires: i64) -> tokio::time::Instant {
    let left = expires + super::token::LEEWAY - chrono::Utc::now().timestamp();
    tokio::time::Instant::now() + Duration::from_secs(left.max(0) as u64)
}

/// One PC's connection: its workers, and where their events go.
struct Pc {
    workers: HashMap<String, Worker>,
    tx: UnboundedSender<FromSession>,
}

impl Pc {
    async fn handle(&mut self, shared: &Shared, message: ToSession) {
        match message {
            ToSession::Token(_) => {}
            ToSession::Open { account } => {
                let Some(served) = shared.accounts.get(&account) else {
                    return self.failed(account, "the session doesn't have this account");
                };
                if self.workers.contains_key(&account) {
                    return;
                }
                let (tx, id) = (self.tx.clone(), account.clone());
                let sink: worker::Sink = Arc::new(move |event: Event| {
                    let _ = tx.send(FromSession::Event {
                        account: id.clone(),
                        event,
                    });
                });
                let worker = worker::spawn(served.creds.clone(), served.endpoints.clone(), sink);
                self.workers.insert(account, worker);
            }
            ToSession::Close { account } => {
                self.workers.remove(&account);
            }
            ToSession::Request { account, request } => match self.workers.get(&account) {
                Some(worker) => {
                    let _ = worker.tx.send(request);
                }
                None => self.failed(account, "the account isn't open"),
            },
            ToSession::Send {
                account,
                id,
                from,
                to,
                raw,
            } => {
                let Some(served) = shared.accounts.get(&account) else {
                    let _ = self.tx.send(FromSession::Sent {
                        account,
                        id,
                        error: Some("the session doesn't have this account".into()),
                    });
                    return;
                };
                let (creds, smtp, tx) = (
                    served.creds.clone(),
                    served.endpoints.smtp.clone(),
                    self.tx.clone(),
                );
                // Not stopped with the connection: a send under way finishes, or the user,
                // never told, might send it again.
                tokio::spawn(async move {
                    let result = worker::send(creds, smtp, from, to, raw).await;
                    let _ = tx.send(FromSession::Sent {
                        account,
                        id,
                        error: result.err().map(|e| text::one_line(&format!("{e:#}"), 300)),
                    });
                });
            }
            ToSession::Credential { account, secret } => {
                if let Some(served) = shared.accounts.get(&account) {
                    give(&served.creds, secret).await;
                }
            }
            ToSession::Retry { account } => {
                if let Some(served) = shared.accounts.get(&account) {
                    served.creds.retry();
                }
            }
            ToSession::SearchArchive { id, query } => {
                let (vault, tx) = (shared.vault.clone(), self.tx.clone());
                // On a blocking thread: a search of a big archive reads a lot.
                tokio::task::spawn_blocking(move || {
                    let result = match vault {
                        Some(vault) => vault
                            .search_archive(&query, super::archive::MAX_FOUND)
                            .map_err(|e| text::one_line(&format!("{e:#}"), 300)),
                        None => Err("the session keeps no archive".into()),
                    };
                    let _ = tx.send(FromSession::Found { id, result });
                });
            }
            ToSession::ReadArchived { id, message } => {
                let (vault, tx) = (shared.vault.clone(), self.tx.clone());
                tokio::task::spawn_blocking(move || {
                    let result = match vault {
                        Some(vault) => vault
                            .archived(message)
                            .map_err(|e| text::one_line(&format!("{e:#}"), 300)),
                        None => Err("the session keeps no archive".into()),
                    };
                    let _ = tx.send(FromSession::Archived { id, result });
                });
            }
        }
    }

    fn failed(&self, account: String, error: &str) {
        let _ = self.tx.send(FromSession::Event {
            account,
            event: Event::Failed {
                what: "relaying".into(),
                error: error.into(),
            },
        });
    }
}

async fn serve_pc(socket: WebSocket, shared: Arc<Shared>, checked: Checked) {
    logfile::note("a PC connected", None);
    let (mut out, mut incoming) = socket.split();
    let (tx, mut rx) = unbounded_channel();
    let _ = tx.send(FromSession::Accounts(shared.order.clone()));
    let mut pc = Pc {
        workers: HashMap::new(),
        tx,
    };
    let mut until = deadline(checked.expires);
    let why = loop {
        tokio::select! {
            message = incoming.next() => match message {
                Some(Ok(Message::Binary(bytes))) => match decode::<ToSession>(&bytes) {
                    Ok(ToSession::Token(token)) => match shared.checker.check(&token).await {
                        Ok(checked) => until = deadline(checked.expires),
                        Err(_) => break "a PC's new token isn't good",
                    },
                    Ok(message) => pc.handle(&shared, message).await,
                    Err(_) => break "a PC sent something that isn't a relay message",
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Text(_))) => break "a PC sent something that isn't a relay message",
                Some(Ok(Message::Close(_))) | None => break "a PC left",
                Some(Err(_)) => break "a PC's connection broke",
            },
            Some(reply) = rx.recv() => {
                let Ok(bytes) = encode(&reply) else { continue };
                if out.send(Message::Binary(bytes.into())).await.is_err() {
                    break "a PC's connection broke";
                }
            }
            () = tokio::time::sleep_until(until) => break "a PC's token ran out",
        }
    };
    logfile::note(why, None);
    let _ = out.close().await;
}

/// A crash is logged by where it happened, never its message, which can quote mail.
fn panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if crate::mail::panic_is_contained() {
            return;
        }
        logfile::panicked(info.location(), false);
        std::process::exit(101);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn options_name_everything() {
        let options = Options::parse(args(&[
            "--listen",
            "0.0.0.0:8080",
            "--state",
            "/state",
            "--key-file",
            "/keys/key",
            "--accounts",
            "/config/accounts.toml",
        ]))
        .unwrap();
        assert_eq!(options.listen, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(options.key_file, PathBuf::from("/keys/key"));
        assert!(Options::parse(args(&["--listen", "0.0.0.0:8080"])).is_err());
        assert!(Options::parse(args(&["--listen"])).is_err());
        assert!(Options::parse(args(&["--listen", "nowhere"])).is_err());
        assert!(Options::parse(args(&["--other", "x"])).is_err());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod backup_tests {
    use super::*;
    use crate::relay::token::tests::{checker, claims, good, token};
    use crate::relay::token::{BACKUP_AUDIENCE, ISSUER};
    use crate::relay::vault::{Key, Vault};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn get(port: u16, token: &str) -> (u16, Vec<u8>) {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let request = format!(
            "GET /backup HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.unwrap();
        crate::http::parse_response(&answer).unwrap()
    }

    #[tokio::test]
    async fn only_the_nas_gets_a_snapshot_and_it_stays_encrypted() {
        let dir = std::env::temp_dir().join(format!("tuimail-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = Key::from_hex(&"9a".repeat(32)).unwrap();
        let vault = Arc::new(Vault::open(&dir.join("session.db"), &key).unwrap());
        vault.keep("a", &Secret::new("refresh-token-a")).unwrap();
        let (checker, _) = checker("k1");
        let mut shared = Shared::new(Vec::new(), Some(vault), checker).await.unwrap();
        shared.snapshots = Some(dir.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(serve(listener, Arc::new(shared)));

        let mut nas = claims(BACKUP_AUDIENCE, ISSUER, 300);
        nas["device"] = serde_json::json!("nas");
        let (status, body) = get(port, &token("k1", nas)).await;
        assert_eq!(status, 200);
        assert!(!body.starts_with(b"SQLite format 3"));
        assert!(!body.windows(15).any(|w| w == b"refresh-token-a"));
        let copy = dir.join("from-the-nas.db");
        std::fs::write(&copy, &body).unwrap();
        let restored = Vault::open(&copy, &key).unwrap();
        assert_eq!(
            restored.credential("a").unwrap().unwrap().expose(),
            "refresh-token-a"
        );
        // Nothing of the snapshot stays behind.
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("snapshot-"))
            .collect();
        assert!(left.is_empty());

        // A PC's token, a backup token of another device's, and none: refused.
        assert_eq!(get(port, &good(300)).await.0, 401);
        let other = claims(BACKUP_AUDIENCE, ISSUER, 300);
        assert_eq!(get(port, &token("k1", other)).await.0, 401);
        assert_eq!(get(port, "").await.0, 401);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
