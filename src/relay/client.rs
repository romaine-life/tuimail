//! A PC's side of the relay: one connection to the session, renewing its token before it runs
//! out. What it carries is the caller's ([`ToSession`] in, [`FromSession`] out); the
//! connection's end shows as the end of [`Connection::rx`].

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use rustls::ClientConfig;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{self, Message};

use super::{FromSession, MAX_MESSAGE, PATH, TOKEN_EVERY, ToSession, decode, encode};
use crate::account::{self, Security};
use crate::net::{self, Endpoint};
use crate::text;

/// How long connecting, the upgrade included, may take.
const CONNECT_FOR: Duration = Duration::from_secs(20);

/// Gets a token OpenBao signed for the session (on a PC, `romaine-auth home-token --role
/// tuimail`).
pub type TokenSource = Arc<dyn Fn() -> BoxFuture<'static, Result<String>> + Send + Sync>;

/// Where the session is: `https://host[:port]`, or `http://` to this computer only (tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

impl Address {
    pub fn parse(url: &str) -> Result<Self> {
        let shown = text::one_line(url, 80);
        let (tls, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            bail!("{shown:?} isn't an https:// address");
        };
        let rest = rest.trim_end_matches('/');
        let (host, port) = match rest.rsplit_once(':') {
            Some((host, port)) if !port.contains(']') => (
                host,
                port.parse()
                    .with_context(|| format!("{shown:?} has no port number after its ':'"))?,
            ),
            _ => (rest, if tls { 443 } else { 80 }),
        };
        if !account::valid_host(host) {
            bail!("{shown:?} isn't a host name");
        }
        if !tls && !account::is_local(host) {
            bail!("{shown:?}: mail goes to the session only over TLS");
        }
        Ok(Self {
            host: host.to_string(),
            port,
            tls,
        })
    }
}

/// An open connection to the session; dropping it (its `task`) closes it.
pub struct Connection {
    pub tx: UnboundedSender<ToSession>,
    pub rx: UnboundedReceiver<FromSession>,
    pub task: Pump,
}

/// The task that carries a connection's messages; dropping it ends the connection.
pub struct Pump(JoinHandle<()>);

impl Drop for Pump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Connects to the session at `address` with a token from `tokens`, renewed every
/// [`TOKEN_EVERY`].
pub async fn connect(
    address: &Address,
    tls: Arc<ClientConfig>,
    tokens: TokenSource,
) -> Result<Connection> {
    let token = tokens()
        .await
        .context("cannot get a token for the session")?;
    let endpoint = Endpoint {
        host: address.host.clone(),
        port: address.port,
        security: if address.tls {
            Security::Tls
        } else {
            Security::None
        },
        tls,
    };
    let scheme = if address.tls { "wss" } else { "ws" };
    let mut request = format!("{scheme}://{}:{}{PATH}", address.host, address.port)
        .into_client_request()
        .context("not an address for the session")?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {token}")
            .parse()
            .context("the token isn't one a header can carry")?,
    );
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    let connecting = async {
        let stream = net::connect(&endpoint).await?;
        tokio_tungstenite::client_async_with_config(request, stream, Some(config))
            .await
            .map_err(|e| match e {
                tungstenite::Error::Http(answer) if answer.status() == 401 => {
                    anyhow::anyhow!("the session refused this PC's token")
                }
                other => {
                    anyhow::Error::from(other).context("the session didn't take the connection")
                }
            })
    };
    let (socket, _) = tokio::time::timeout(CONNECT_FOR, connecting)
        .await
        .map_err(|_| anyhow::anyhow!("{} didn't answer", endpoint.shown()))??;
    let (tx, mut outgoing) = unbounded_channel::<ToSession>();
    let (incoming_tx, rx) = unbounded_channel::<FromSession>();
    let task = tokio::spawn(async move {
        let (mut out, mut incoming) = socket.split();
        let mut renew =
            tokio::time::interval_at(tokio::time::Instant::now() + TOKEN_EVERY, TOKEN_EVERY);
        loop {
            tokio::select! {
                message = outgoing.recv() => {
                    let Some(message) = message else { break };
                    let Ok(bytes) = encode(&message) else { continue };
                    if out.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                message = incoming.next() => match message {
                    Some(Ok(Message::Binary(bytes))) => match decode::<FromSession>(&bytes) {
                        Ok(message) => {
                            if incoming_tx.send(message).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    },
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                    _ => break,
                },
                _ = renew.tick() => {
                    // A token that can't be had now is tried at the next tick; the session ends
                    // the connection if the last one runs out first.
                    let Ok(token) = tokens().await else { continue };
                    let Ok(bytes) = encode(&ToSession::Token(token)) else { continue };
                    if out.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = out.close().await;
    });
    Ok(Connection {
        tx,
        rx,
        task: Pump(task),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Account;
    use crate::relay::session::{self, Shared};
    use crate::relay::token::tests::{checker, good};
    use crate::worker::{Event, Request, State};

    #[test]
    fn addresses_are_tls_but_to_this_computer() {
        assert_eq!(
            Address::parse("https://tuimail.home.romaine.life").unwrap(),
            Address {
                host: "tuimail.home.romaine.life".into(),
                port: 443,
                tls: true
            }
        );
        assert_eq!(Address::parse("https://x.y:8443/").unwrap().port, 8443);
        assert!(Address::parse("http://127.0.0.1:9").is_ok());
        assert!(Address::parse("http://tuimail.home.romaine.life").is_err());
        assert!(Address::parse("ftp://x.y").is_err());
        assert!(Address::parse("https://x y").is_err());
        assert!(Address::parse("https://x.y:port").is_err());
    }

    /// A session on this computer serving `accounts`, with tokens checked against the test key.
    async fn session(accounts: Vec<Account>) -> Address {
        let (checker, _) = checker("k1");
        let shared = Arc::new(Shared::new(accounts, None, checker).await.unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(session::serve(listener, shared));
        Address::parse(&format!("http://127.0.0.1:{port}")).unwrap()
    }

    fn tokens(left: i64) -> TokenSource {
        Arc::new(move || Box::pin(async move { Ok(good(left)) }))
    }

    fn tls() -> Arc<ClientConfig> {
        net::tls_config(None).unwrap()
    }

    /// An account whose server is a port on this computer nothing answers.
    fn unreachable() -> Account {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
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

    async fn next(connection: &mut Connection) -> FromSession {
        tokio::time::timeout(Duration::from_secs(20), connection.rx.recv())
            .await
            .expect("the session answered in time")
            .expect("the connection is open")
    }

    #[tokio::test]
    async fn a_request_reaches_the_accounts_worker_and_its_events_come_back() {
        let account = unreachable();
        let id = account.id();
        let address = session(vec![account]).await;
        let mut connection = connect(&address, tls(), tokens(300)).await.unwrap();
        match next(&mut connection).await {
            FromSession::Accounts(ids) => assert_eq!(ids, vec![id.clone()]),
            other => panic!("{other:?}"),
        }
        connection
            .tx
            .send(ToSession::Open {
                account: id.clone(),
            })
            .unwrap();
        // A password typed on the PC, which the session uses from now on.
        connection
            .tx
            .send(ToSession::Credential {
                account: id.clone(),
                secret: crate::secrets::Secret::new("password"),
            })
            .unwrap();
        connection
            .tx
            .send(ToSession::Request {
                account: id.clone(),
                request: Request::Folders,
            })
            .unwrap();
        // The worker tried the server, which isn't there, and said so.
        loop {
            match next(&mut connection).await {
                FromSession::Event {
                    account,
                    event: Event::State(State::Offline(_)),
                } => {
                    assert_eq!(account, id);
                    break;
                }
                FromSession::Event {
                    event:
                        Event::Failed { .. } | Event::Waiting(_) | Event::State(State::Connecting),
                    ..
                } => {}
                other => panic!("{other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn an_account_the_session_lacks_is_said_so() {
        let address = session(vec![]).await;
        let mut connection = connect(&address, tls(), tokens(300)).await.unwrap();
        assert!(
            matches!(next(&mut connection).await, FromSession::Accounts(ids) if ids.is_empty())
        );
        connection
            .tx
            .send(ToSession::Open {
                account: "x".into(),
            })
            .unwrap();
        match next(&mut connection).await {
            FromSession::Event {
                event: Event::Failed { error, .. },
                ..
            } => assert!(error.contains("doesn't have")),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_pc_without_a_good_token_is_refused() {
        let address = session(vec![]).await;
        let bad: TokenSource = Arc::new(|| Box::pin(async { Ok("not a token".to_string()) }));
        let error = connect(&address, tls(), bad).await.err().unwrap();
        assert!(format!("{error:#}").contains("refused this PC's token"));
        let error = connect(&address, tls(), tokens(-600)).await.err().unwrap();
        assert!(format!("{error:#}").contains("refused this PC's token"));
    }

    #[tokio::test]
    async fn the_session_ends_a_connection_whose_token_ran_out() {
        let address = session(vec![]).await;
        // Good for one more second, and never renewed in that time.
        let mut connection = connect(&address, tls(), tokens(1)).await.unwrap();
        assert!(matches!(
            next(&mut connection).await,
            FromSession::Accounts(_)
        ));
        let ended = tokio::time::timeout(Duration::from_secs(20), connection.rx.recv())
            .await
            .expect("the session ended it in time");
        assert!(ended.is_none());
    }
}
