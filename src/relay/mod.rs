//! The relay between the PCs' tuimail and the session (`tuimail session`), as spotify-player's
//! home cast has one: the session, on the home cluster, holds the house's one sign-in for each
//! account and runs tuimail's own workers (`worker.rs`) against the servers; a PC sends it the
//! [`Request`]s its worker would have run and gets the [`Event`]s back. Sending goes through it
//! too. So no PC signs in to the providers, and mail is reachable from any PC.
//!
//! One WebSocket per PC at [`PATH`], through the home Gateway's TLS (`tuimail.home.romaine.life`).
//! The PC proves itself with a five-minute token OpenBao signs for it ([`token`]), given at the
//! upgrade and renewed in the connection before it runs out ([`ToSession::Token`]); the session
//! ends a connection whose newest token has run out. Messages are binary frames, one postcard
//! encoded [`ToSession`] or [`FromSession`] each.

pub mod client;
mod selftest;
pub mod session;
pub mod token;
pub mod vault;

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::secrets::Secret;
use crate::worker::{Event, Request};

/// Where the relay is answered.
pub const PATH: &str = "/relay";
/// Where the session says it's up (the Gateway's down alert, Kubernetes' probes).
pub const HEALTH: &str = "/health";
/// How often a PC sends a new token: well inside the five minutes one lasts, and often enough
/// to keep the connection from looking idle to the Gateway.
pub const TOKEN_EVERY: Duration = Duration::from_secs(120);
/// The biggest message either side takes: a POP3 check's download ([`crate::worker`]'s
/// `POP_BYTES`) with room for its framing.
pub const MAX_MESSAGE: usize = 300 << 20;

/// What a PC tells the session.
#[derive(Debug, Serialize, Deserialize)]
pub enum ToSession {
    /// A new token, before the last one runs out.
    Token(String),
    /// Starts this PC's worker for the account (by [`crate::account::Account::id`]).
    Open { account: String },
    /// Stops it.
    Close { account: String },
    /// What the PC's worker would have run.
    Request { account: String, request: Request },
    /// Sends a message, answered by [`FromSession::Sent`] with the same `id`.
    Send {
        account: String,
        id: u64,
        from: String,
        to: Vec<String>,
        raw: Vec<u8>,
    },
    /// What the account signs in with, from a sign-in made on the PC (a refresh token) or a
    /// password typed there: the session uses it from now on and keeps it once a login with it
    /// works.
    Credential { account: String, secret: Secret },
    /// Tries the account's sign-in again after a refusal (the PC's `R`).
    Retry { account: String },
}

/// What the session tells a PC.
#[derive(Debug, Serialize, Deserialize)]
pub enum FromSession {
    /// The accounts the session serves, by id: sent first.
    Accounts(Vec<String>),
    /// What the account's worker for this PC said.
    Event { account: String, event: Event },
    Sent {
        account: String,
        id: u64,
        error: Option<String>,
    },
}

pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>> {
    postcard::to_stdvec(message).context("cannot encode a relay message")
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T> {
    postcard::from_bytes(bytes).context("not a relay message")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Known;

    #[test]
    fn messages_survive_the_wire() {
        let sent = ToSession::Request {
            account: "a".into(),
            request: Request::Sync {
                folder: "INBOX".into(),
                known: Known {
                    uidvalidity: 7,
                    low: 1,
                    high: 9,
                },
                count: 50,
            },
        };
        let back: ToSession = decode(&encode(&sent).unwrap()).unwrap();
        assert_eq!(format!("{sent:?}"), format!("{back:?}"));
        let raw = vec![0u8, 255, 13, 10];
        let event = FromSession::Event {
            account: "a".into(),
            event: Event::Body {
                folder: "INBOX".into(),
                uid: 3,
                result: Ok(Some(raw.clone())),
            },
        };
        let bytes = encode(&event).unwrap();
        // Bytes go as bytes, not as a list of numbers.
        assert!(bytes.len() < 40, "{}", bytes.len());
        let back: FromSession = decode(&bytes).unwrap();
        assert_eq!(format!("{event:?}"), format!("{back:?}"));
    }

    #[test]
    fn a_credential_never_shows_in_debug() {
        let message = ToSession::Credential {
            account: "a".into(),
            secret: Secret::new("hunter2"),
        };
        assert!(!format!("{message:?}").contains("hunter2"));
    }

    #[test]
    fn garbage_is_refused() {
        assert!(decode::<ToSession>(&[0xff, 0xff, 0xff]).is_err());
    }
}
