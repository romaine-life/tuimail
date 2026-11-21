//! A PC's tuimail going through the session (the `session` setting): one connection, the
//! [`Link`], carries every account's requests and events, so each account's [`Worker`] here
//! is a forwarder and the session's worker does the work with the house's one sign-in.
//!
//! The link connects, and connects again after it's lost (backing off from one second to a
//! minute), opening every account again each time. While it's down, reads fail at once, as a
//! worker's do offline, and changes ([`Request::changes`]) are held, in order, and sent once
//! it's back. A sign-in or password given meanwhile goes once it's back too. A send can't wait:
//! it fails at once, and one under way when the link breaks is said to be unknown.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::future::BoxFuture;
use rustls::ClientConfig;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::client::{self, Address, Pump, TokenSource};
use super::{FromSession, ToSession};
use crate::secrets::{Credentials, Secret};
use crate::worker::{self, Endpoints, Event, Request, Sink, State, Worker};
use crate::{logfile, text};

/// The first wait before connecting again, doubled each failure up to [`RETRY_MOST`].
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MOST: Duration = Duration::from_secs(60);
/// How long getting a token may take.
const TOKEN_FOR: Duration = Duration::from_secs(30);

/// Where an archive search's answer goes.
type FoundReply = oneshot::Sender<Result<Vec<super::archive::Found>, String>>;
/// Where an archived message's bytes go.
type ArchivedReply = oneshot::Sender<Result<Option<Vec<u8>>, String>>;

/// What the app's side asks of the link's task.
enum Cmd {
    Register {
        account: String,
        sink: Sink,
    },
    Unregister {
        account: String,
    },
    Request {
        account: String,
        request: Request,
    },
    Send {
        account: String,
        from: String,
        to: Vec<String>,
        raw: Vec<u8>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Credential {
        account: String,
        secret: Secret,
    },
    Retry {
        account: String,
    },
    SearchArchive {
        query: String,
        reply: FoundReply,
    },
    ReadArchived {
        message: i64,
        reply: ArchivedReply,
    },
}

/// The connection to the session, shared by every account; dropping it closes it.
pub struct Link {
    cmds: UnboundedSender<Cmd>,
    task: JoinHandle<()>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// An account's way to the session, held by its [`Worker`].
#[derive(Clone)]
pub struct Remote {
    account: String,
    cmds: UnboundedSender<Cmd>,
}

impl Remote {
    pub fn send(
        &self,
        from: String,
        to: Vec<String>,
        raw: Vec<u8>,
    ) -> BoxFuture<'static, Result<()>> {
        let (reply, answer) = oneshot::channel();
        let sent = self.cmds.send(Cmd::Send {
            account: self.account.clone(),
            from,
            to,
            raw,
            reply,
        });
        Box::pin(async move {
            sent.map_err(|_| anyhow!("Not sent: tuimail's session link has stopped"))?;
            answer
                .await
                .map_err(|_| anyhow!("Not sent: tuimail's session link has stopped"))?
                .map_err(|e| anyhow!(e))
        })
    }

    /// What the account signs in with from now on (a sign-in made here, or a password typed).
    pub fn credential(&self, secret: Secret) {
        let _ = self.cmds.send(Cmd::Credential {
            account: self.account.clone(),
            secret,
        });
    }

    /// The account's worker here has stopped: the session's stops too.
    pub fn close(&self) {
        let _ = self.cmds.send(Cmd::Unregister {
            account: self.account.clone(),
        });
    }

    pub fn retry(&self) {
        let _ = self.cmds.send(Cmd::Retry {
            account: self.account.clone(),
        });
    }
}

impl Link {
    /// Starts connecting to the session at `address`.
    pub fn start(address: Address, tls: Arc<ClientConfig>, tokens: TokenSource) -> Self {
        let (cmds, rx) = unbounded_channel();
        let task = tokio::spawn(run(address, tls, tokens, rx));
        Self { cmds, task }
    }

    /// Searches the session's archive.
    pub fn search_archive(
        &self,
        query: String,
    ) -> BoxFuture<'static, Result<Vec<super::archive::Found>, String>> {
        let (reply, answer) = oneshot::channel();
        let asked = self.cmds.send(Cmd::SearchArchive { query, reply });
        Box::pin(async move {
            asked.map_err(|_| "tuimail's session link has stopped".to_string())?;
            answer
                .await
                .map_err(|_| "tuimail's session link has stopped".to_string())?
        })
    }

    /// An archived message's bytes.
    pub fn read_archived(
        &self,
        message: i64,
    ) -> BoxFuture<'static, Result<Option<Vec<u8>>, String>> {
        let (reply, answer) = oneshot::channel();
        let asked = self.cmds.send(Cmd::ReadArchived { message, reply });
        Box::pin(async move {
            asked.map_err(|_| "tuimail's session link has stopped".to_string())?;
            answer
                .await
                .map_err(|_| "tuimail's session link has stopped".to_string())?
        })
    }

    /// The account's worker: its requests go to the session, its events come from there.
    pub fn spawn(&self, creds: Credentials, endpoints: Endpoints, sink: Sink) -> Worker {
        let account = creds.account().id();
        let _ = self.cmds.send(Cmd::Register {
            account: account.clone(),
            sink,
        });
        let (tx, mut requests) = unbounded_channel::<Request>();
        let cmds = self.cmds.clone();
        let id = account.clone();
        let forward = tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                if cmds
                    .send(Cmd::Request {
                        account: id.clone(),
                        request,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let remote = Remote {
            account,
            cmds: self.cmds.clone(),
        };
        Worker::remote(tx, creds, endpoints, forward, remote)
    }
}

/// The link's state: the accounts, the connection when there is one, what waits for it.
struct Task {
    accounts: Vec<(String, Sink)>,
    tx: Option<UnboundedSender<ToSession>>,
    held: VecDeque<(String, Request)>,
    credentials: HashMap<String, Secret>,
    sends: HashMap<u64, oneshot::Sender<Result<(), String>>>,
    next_send: u64,
    searches: HashMap<u64, FoundReply>,
    reads: HashMap<u64, ArchivedReply>,
    /// A connection has been tried: until then (the app's first checks, made as it starts)
    /// reads wait for it with the changes, rather than failing before it's known.
    tried: bool,
}

impl Task {
    fn sink(&self, account: &str) -> Option<&Sink> {
        self.accounts
            .iter()
            .find(|(id, _)| id == account)
            .map(|(_, s)| s)
    }

    fn to_session(&self, message: ToSession) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(message).is_ok())
    }

    fn tell_all(&self, state: State) {
        for (_, sink) in &self.accounts {
            sink(Event::State(state.clone()));
        }
    }

    /// The connection is up: every account opened and said to be connecting (the app checks
    /// it, catching up on what it missed), then what was held, in order.
    fn up(&mut self, tx: UnboundedSender<ToSession>) {
        self.tx = Some(tx);
        self.tried = true;
        self.tell_all(State::Connecting);
        for (account, _) in &self.accounts {
            self.to_session(ToSession::Open {
                account: account.clone(),
            });
        }
        for (account, secret) in std::mem::take(&mut self.credentials) {
            self.to_session(ToSession::Credential { account, secret });
        }
        while let Some((account, request)) = self.held.pop_front() {
            self.to_session(ToSession::Request { account, request });
        }
    }

    /// The connection is gone: sends under way can't be known.
    fn down(&mut self, why: &str) {
        self.tx = None;
        // The first try failed: the reads that waited for it fail now; changes still wait.
        if !self.tried {
            self.tried = true;
            for (account, request) in std::mem::take(&mut self.held) {
                if request.changes() {
                    self.held.push_back((account, request));
                } else if let Some(sink) = self.sink(&account) {
                    worker::report_failure(&request, "offline".into(), sink);
                }
            }
        }
        for (_, reply) in self.sends.drain() {
            let _ = reply.send(Err(
                "The link to tuimail's session broke while sending: look in Sent before sending again"
                    .into(),
            ));
        }
        for (_, reply) in self.searches.drain() {
            let _ = reply.send(Err("lost tuimail's session".into()));
        }
        for (_, reply) in self.reads.drain() {
            let _ = reply.send(Err("lost tuimail's session".into()));
        }
        self.tell_all(State::Offline(text::one_line(why, 300)));
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Register { account, sink } => {
                self.accounts.retain(|(id, _)| *id != account);
                self.accounts.push((account.clone(), sink));
                self.to_session(ToSession::Open { account });
            }
            Cmd::Unregister { account } => {
                self.accounts.retain(|(id, _)| *id != account);
                self.held.retain(|(id, _)| *id != account);
                self.to_session(ToSession::Close { account });
            }
            Cmd::Request { account, request } => {
                if self.tx.is_some() {
                    self.to_session(ToSession::Request { account, request });
                } else if !self.tried {
                    self.held.push_back((account, request));
                } else if request.changes() {
                    self.held.push_back((account.clone(), request));
                    let waiting = self.held.iter().filter(|(id, _)| *id == account).count();
                    if let Some(sink) = self.sink(&account) {
                        sink(Event::Waiting(waiting));
                    }
                } else if let Some(sink) = self.sink(&account) {
                    worker::report_failure(&request, "offline".into(), sink);
                }
            }
            Cmd::Send {
                account,
                from,
                to,
                raw,
                reply,
            } => {
                if self.tx.is_none() {
                    let _ = reply.send(Err("Not sent: can't reach tuimail's session".into()));
                    return;
                }
                self.next_send += 1;
                let id = self.next_send;
                self.sends.insert(id, reply);
                self.to_session(ToSession::Send {
                    account,
                    id,
                    from,
                    to,
                    raw,
                });
            }
            Cmd::Credential { account, secret } => {
                if self.tx.is_none() {
                    self.credentials.insert(account, secret);
                } else {
                    self.to_session(ToSession::Credential { account, secret });
                }
            }
            Cmd::Retry { account } => {
                self.to_session(ToSession::Retry { account });
            }
            Cmd::SearchArchive { query, reply } => {
                self.next_send += 1;
                let id = self.next_send;
                if self.to_session(ToSession::SearchArchive { id, query }) {
                    self.searches.insert(id, reply);
                } else {
                    let _ = reply.send(Err("can't reach tuimail's session".into()));
                }
            }
            Cmd::ReadArchived { message, reply } => {
                self.next_send += 1;
                let id = self.next_send;
                if self.to_session(ToSession::ReadArchived { id, message }) {
                    self.reads.insert(id, reply);
                } else {
                    let _ = reply.send(Err("can't reach tuimail's session".into()));
                }
            }
        }
    }

    fn received(&mut self, message: FromSession) {
        match message {
            FromSession::Accounts(served) => {
                for (account, sink) in &self.accounts {
                    if !served.contains(account) {
                        sink(Event::State(State::Offline(
                            "tuimail's session doesn't have this account".into(),
                        )));
                    }
                }
            }
            FromSession::Event { account, event } => {
                if let Some(sink) = self.sink(&account) {
                    sink(event);
                }
            }
            FromSession::Sent { id, error, .. } => {
                if let Some(reply) = self.sends.remove(&id) {
                    let _ = reply.send(error.map_or(Ok(()), Err));
                }
            }
            FromSession::Found { id, result } => {
                if let Some(reply) = self.searches.remove(&id) {
                    let _ = reply.send(result);
                }
            }
            FromSession::Archived { id, result } => {
                if let Some(reply) = self.reads.remove(&id) {
                    let _ = reply.send(result);
                }
            }
        }
    }
}

async fn run(
    address: Address,
    tls: Arc<ClientConfig>,
    tokens: TokenSource,
    mut cmds: UnboundedReceiver<Cmd>,
) {
    let mut task = Task {
        accounts: Vec::new(),
        tx: None,
        held: VecDeque::new(),
        credentials: HashMap::new(),
        sends: HashMap::new(),
        next_send: 0,
        searches: HashMap::new(),
        reads: HashMap::new(),
        tried: false,
    };
    let mut incoming: Option<(UnboundedReceiver<FromSession>, Pump)> = None;
    let mut wait = RETRY_FIRST;
    let mut retry_at = tokio::time::Instant::now();
    let mut link = logfile::Link::unnumbered();
    loop {
        if incoming.is_none() && tokio::time::Instant::now() >= retry_at {
            // What was asked meanwhile is handled first, so registrations made at start are
            // opened by `up`.
            while let Ok(cmd) = cmds.try_recv() {
                task.handle(cmd);
            }
            match client::connect(&address, tls.clone(), tokens.clone()).await {
                Ok(connection) => {
                    link.back();
                    wait = RETRY_FIRST;
                    task.up(connection.tx);
                    incoming = Some((connection.rx, connection.task));
                }
                Err(e) => {
                    link.failed("reaching the session", &e);
                    task.down(&format!("can't reach tuimail's session: {e:#}"));
                    retry_at = tokio::time::Instant::now() + wait;
                    wait = (wait * 2).min(RETRY_MOST);
                }
            }
            continue;
        }
        let step = {
            let down = incoming.is_none();
            let message = async {
                match incoming.as_mut() {
                    Some((rx, _)) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                cmd = cmds.recv() => Step::Cmd(cmd),
                message = message => Step::Message(message),
                () = tokio::time::sleep_until(retry_at), if down => Step::Retry,
            }
        };
        match step {
            Step::Cmd(Some(cmd)) => task.handle(cmd),
            Step::Cmd(None) => return,
            Step::Message(Some(message)) => task.received(message),
            Step::Message(None) => {
                incoming = None;
                let lost = logfile::Class::ConnectionLost.with("the session's connection ended");
                link.failed("reaching the session", &anyhow!(lost));
                task.down("lost tuimail's session: connecting again");
                retry_at = tokio::time::Instant::now() + RETRY_FIRST;
            }
            Step::Retry => {}
        }
    }
}

/// What woke the link's task.
enum Step {
    Cmd(Option<Cmd>),
    Message(Option<FromSession>),
    Retry,
}

/// Tokens for the session on a home PC: `romaine-auth home-token --role tuimail`, which keeps
/// the token it signed and gives it again while more than a minute of it is left.
pub fn home_tokens(file: std::path::PathBuf) -> Result<TokenSource> {
    let program = crate::tools::on_path("romaine-auth")
        .context("romaine-auth isn't installed, and it gets the tokens tuimail's session takes")?;
    Ok(Arc::new(move || {
        let (program, file) = (program.clone(), file.clone());
        Box::pin(async move { romaine_auth(&program, &file).await })
    }))
}

async fn romaine_auth(program: &std::path::Path, file: &std::path::Path) -> Result<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(["home-token", "--role", "tuimail", "--out"])
        .arg(file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(TOKEN_FOR, command.status())
        .await
        .map_err(|_| anyhow!(logfile::Class::Timeout.with("romaine-auth took too long")))?
        .context("cannot run romaine-auth")?;
    anyhow::ensure!(
        status.success(),
        "romaine-auth couldn't get a token ({status})"
    );
    let bytes =
        crate::config::read_capped(file, 64 << 10).context("cannot read romaine-auth's token")?;
    let json: serde_json::Value =
        serde_json::from_slice(&bytes).context("romaine-auth's token isn't JSON")?;
    json["token"]
        .as_str()
        .map(str::to_string)
        .context("romaine-auth's file has no token")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Account, Security};
    use crate::imap::FlagKind;
    use crate::relay::session::{self, Shared};
    use crate::relay::token::tests::{checker, good};
    use crate::store::Known;

    fn tokens() -> TokenSource {
        Arc::new(|| Box::pin(async { Ok(good(300)) }))
    }

    /// A port on this computer nothing answers.
    fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn account(port: u16) -> Account {
        Account {
            name: "A".into(),
            email: "a@example.com".into(),
            host: "127.0.0.1".into(),
            port: Some(port),
            security: Security::None,
            smtp_host: "127.0.0.1".into(),
            smtp_port: Some(port),
            smtp_security: Security::None,
            ..Account::default()
        }
    }

    fn worker(link: &Link, account: Account) -> (Worker, UnboundedReceiver<Event>) {
        let account = Arc::new(account);
        let tls = crate::net::tls_config(None).unwrap();
        let endpoints = Endpoints::of(&account, tls);
        let creds = Credentials::new(account, false);
        let (tx, rx) = unbounded_channel();
        let tx = std::sync::Mutex::new(tx);
        let sink: Sink = Arc::new(move |event| {
            let _ = tx.lock().unwrap().send(event);
        });
        (link.spawn(creds, endpoints, sink), rx)
    }

    async fn until<T>(
        rx: &mut UnboundedReceiver<Event>,
        mut pick: impl FnMut(Event) -> Option<T>,
    ) -> T {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(found) = pick(rx.recv().await.expect("the link is up")) {
                    return found;
                }
            }
        })
        .await
        .expect("no such event in time")
    }

    #[tokio::test]
    async fn an_accounts_requests_reach_the_sessions_worker() {
        let account = account(closed_port());
        let (checker, _) = checker("k1");
        let shared = Arc::new(
            Shared::new(vec![account.clone()], None, checker)
                .await
                .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(session::serve(listener, shared));
        let address = Address::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let link = Link::start(address, crate::net::tls_config(None).unwrap(), tokens());
        let (worker, mut events) = worker(&link, account);
        worker.use_password(Secret::new("password")).await;
        worker.tx.send(Request::Folders).unwrap();
        // The session's worker tried the account's server, which isn't there, and said so.
        until(&mut events, |e| match e {
            Event::State(State::Offline(why)) if !why.contains("session") => Some(()),
            _ => None,
        })
        .await;
    }

    #[tokio::test]
    async fn once_the_session_is_reached_every_account_is_said_to_be_connecting() {
        let (checker, _) = checker("k1");
        let shared = Arc::new(Shared::new(Vec::new(), None, checker).await.unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(session::serve(listener, shared));
        let address = Address::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let link = Link::start(address, crate::net::tls_config(None).unwrap(), tokens());
        let (worker, mut events) = worker(&link, account(closed_port()));
        // Asked at once, as the app does as it starts, before the link has connected: it waits
        // for the session rather than failing, and the session's worker answers it.
        worker.use_password(Secret::new("password")).await;
        worker.tx.send(Request::Folders).unwrap();
        until(&mut events, |e| match e {
            Event::State(State::Connecting) => Some(()),
            Event::Failed { what, .. } if what == "listing folders" => {
                panic!("failed before connecting")
            }
            _ => None,
        })
        .await;
    }

    #[tokio::test]
    async fn while_the_session_is_out_of_reach_reads_fail_changes_wait_and_sends_dont_go() {
        let address = Address::parse(&format!("http://127.0.0.1:{}", closed_port())).unwrap();
        let link = Link::start(address, crate::net::tls_config(None).unwrap(), tokens());
        let (worker, mut events) = worker(&link, account(closed_port()));
        until(&mut events, |e| match e {
            Event::State(State::Offline(why)) if why.contains("session") => Some(()),
            _ => None,
        })
        .await;
        worker
            .tx
            .send(Request::Flag {
                folder: "INBOX".into(),
                uids: vec![1],
                flag: FlagKind::Seen,
                on: true,
                uidvalidity: 1,
            })
            .unwrap();
        assert_eq!(
            until(&mut events, |e| match e {
                Event::Waiting(n) => Some(n),
                _ => None,
            })
            .await,
            1
        );
        worker
            .tx
            .send(Request::Sync {
                folder: "INBOX".into(),
                known: Known::default(),
                count: 10,
            })
            .unwrap();
        until(&mut events, |e| match e {
            Event::Failed { .. } => Some(()),
            _ => None,
        })
        .await;
        let error = worker
            .send(
                "a@example.com".into(),
                vec!["b@example.com".into()],
                b"x".to_vec(),
            )
            .await
            .unwrap_err();
        assert!(format!("{error}").contains("can't reach tuimail's session"));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod archive_tests {
    use super::*;
    use crate::relay::session::{self, Shared};
    use crate::relay::token::tests::{checker, good};
    use crate::relay::vault::{Key, Vault};

    #[tokio::test]
    async fn a_pc_searches_and_reads_the_sessions_archive() {
        let dir = std::env::temp_dir().join(format!("tuimail-pc-archive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = Key::from_hex(&"ef".repeat(32)).unwrap();
        let vault = Arc::new(Vault::open(&dir.join("session.db"), &key).unwrap());
        let raw = b"From: ann@example.com\r\nSubject: Boat insurance\r\nMessage-ID: <b@x>\r\n\r\nRenewal attached.\r\n";
        let id = vault.archive(raw, "Renewal attached.").unwrap();
        let (checker, _) = checker("k1");
        let shared = Arc::new(Shared::new(Vec::new(), Some(vault), checker).await.unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(session::serve(listener, shared));
        let address = Address::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let tokens: TokenSource = Arc::new(|| Box::pin(async { Ok(good(300)) }));
        let link = Link::start(address, crate::net::tls_config(None).unwrap(), tokens);
        let found = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                // The link may still be connecting: a search then fails, and is asked again.
                if let Ok(found) = link.search_archive("renewal".into()).await {
                    return found;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, id);
        assert_eq!(found[0].subject, "Boat insurance");
        assert_eq!(link.read_archived(id).await.unwrap().unwrap(), raw);
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Against the house's session itself, by hand on a home PC: this PC's own token (romaine-auth)
/// gets it in, and it lists the accounts it serves.
#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "reaches tuimail.home.romaine.life: run by hand on a home PC"]
    async fn live_this_pc_reaches_the_session() {
        let dir = std::env::temp_dir().join(format!("tuimail-live-session-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tokens = home_tokens(dir.join("token.json")).unwrap();
        let address = Address::parse("https://tuimail.home.romaine.life").unwrap();
        let mut connection =
            client::connect(&address, crate::net::tls_config(None).unwrap(), tokens)
                .await
                .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(20), connection.rx.recv())
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        match first {
            Some(FromSession::Accounts(ids)) => assert!(!ids.is_empty()),
            other => panic!("{other:?}"),
        }
    }
}
