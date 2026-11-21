//! `tuimail session --self-test`: the image's smoke test. pipeline-templates' cluster-service
//! suite runs the image read-only with it on every PR that changes the image. It proves what the
//! running session rests on, with nothing from outside: SQLCipher is built in and encrypts the
//! store, the system's certificates are there for TLS, and the relay lets in a PC with a good
//! token (signed here by a key made for the test, standing in for OpenBao's) and no other.

use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};

use super::client::{self, Address, TokenSource};
use super::session::{self, Shared};
use super::token::{AUDIENCE, ISSUER, JwkSet, TokenChecker};
use super::vault::{self, Key, Vault};
use super::{FromSession, ToSession};
use crate::net;
use crate::secrets::Secret;
use crate::worker::Event;

pub async fn run() -> Result<()> {
    store()?;
    net::tls_config(None).context("the system's certificates")?;
    relay().await?;
    println!("tuimail session self-test passed");
    Ok(())
}

/// A folder the read-only image can write: the container's memory-backed /dev/shm.
fn scratch() -> Result<std::path::PathBuf> {
    let base = ["/dev/shm", "/tmp"]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("tuimail-self-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).context("cannot make a scratch folder")?;
    Ok(dir)
}

/// The store keeps a sign-in under a key, reads it back after reopening, holds it nowhere in
/// plain text, and refuses another key.
fn store() -> Result<()> {
    let dir = scratch()?;
    let path = dir.join(vault::FILE);
    let mut raw = [0u8; 32];
    ring::rand::SecureRandom::fill(&SystemRandom::new(), &mut raw)
        .map_err(|_| anyhow::anyhow!("no randomness"))?;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let key = Key::from_hex(&hex).context("a made key")?;
    let result = (|| {
        Vault::open(&path, &key)?.keep("self-test", &Secret::new("self-test-secret"))?;
        let kept = Vault::open(&path, &key)?.credential("self-test")?;
        ensure!(
            kept.as_ref().map(Secret::expose) == Some("self-test-secret"),
            "the store didn't keep the sign-in"
        );
        let bytes = std::fs::read(&path)?;
        ensure!(
            !bytes.windows(16).any(|w| w == b"self-test-secret"),
            "the store holds the sign-in in plain text"
        );
        ensure!(
            !bytes.starts_with(b"SQLite format 3"),
            "the store isn't encrypted"
        );
        let other = Key::from_hex(&"0".repeat(64)).context("a made key")?;
        if Vault::open(&path, &other).is_ok() {
            bail!("the store opened with another key");
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result.context("the session's store")
}

/// A key pair standing in for OpenBao's, and its published form.
struct Signer {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
}

impl Signer {
    fn new() -> Result<Self> {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| anyhow::anyhow!("cannot make a key"))?;
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .map_err(|_| anyhow::anyhow!("cannot read the made key"))?;
        Ok(Self { pair, rng })
    }

    fn keys(&self) -> Result<JwkSet> {
        let point = self.pair.public_key().as_ref();
        let (x, y) = (B64.encode(&point[1..33]), B64.encode(&point[33..65]));
        let set = serde_json::json!({"keys": [{"kty": "EC", "crv": "P-256", "kid": "self-test", "x": x, "y": y}]});
        serde_json::from_value(set).context("the made key's JWK")
    }

    fn token(&self) -> Result<String> {
        let head = B64.encode(serde_json::json!({"alg": "ES256", "kid": "self-test"}).to_string());
        let exp = chrono::Utc::now().timestamp() + 300;
        let body = B64.encode(
            serde_json::json!({"aud": AUDIENCE, "iss": ISSUER, "exp": exp, "device": "self-test"})
                .to_string(),
        );
        let signed = format!("{head}.{body}");
        let signature = self
            .pair
            .sign(&self.rng, signed.as_bytes())
            .map_err(|_| anyhow::anyhow!("cannot sign"))?;
        Ok(format!("{signed}.{}", B64.encode(signature.as_ref())))
    }
}

/// The relay answers its health check, takes a PC with a good token, and refuses one without.
async fn relay() -> Result<()> {
    let signer = Signer::new()?;
    let keys = signer.keys()?;
    let checker = TokenChecker::with_fetch(Box::new(move || {
        let keys = keys.clone();
        Box::pin(async move { Ok(keys) })
    }));
    let shared = Arc::new(Shared::new(Vec::new(), None, checker).await?);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(session::serve(listener, shared));
    let result = async {
        health(port).await?;
        let address = Address::parse(&format!("http://127.0.0.1:{port}"))?;
        let tls = net::tls_config(None)?;
        let token = signer.token()?;
        let good: TokenSource = Arc::new(move || {
            let token = token.clone();
            Box::pin(async move { Ok(token) })
        });
        let mut pc = client::connect(&address, tls.clone(), good).await?;
        let first = tokio::time::timeout(std::time::Duration::from_secs(10), pc.rx.recv())
            .await
            .context("the relay didn't answer")?;
        ensure!(
            matches!(first, Some(FromSession::Accounts(_))),
            "the relay didn't list its accounts first"
        );
        // A request reaches the session and is answered.
        pc.tx.send(ToSession::Open {
            account: "self-test".into(),
        })?;
        let answer = tokio::time::timeout(std::time::Duration::from_secs(10), pc.rx.recv())
            .await
            .context("the relay didn't answer a request")?;
        ensure!(
            matches!(
                answer,
                Some(FromSession::Event {
                    event: Event::Failed { .. },
                    ..
                })
            ),
            "the relay didn't say it has no such account"
        );
        let bad: TokenSource = Arc::new(|| Box::pin(async { Ok("not a token".to_string()) }));
        ensure!(
            client::connect(&address, tls, bad).await.is_err(),
            "the relay let in a PC without a good token"
        );
        Ok(())
    }
    .await;
    server.abort();
    result.context("the relay")
}

/// `/health` answers `ok`.
async fn health(port: u16) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await?;
    let (status, body) = crate::http::parse_response(&answer)?;
    ensure!(status == 200 && body == b"ok", "/health answered {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn the_relay_passes() {
        super::relay().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_whole_self_test_passes_with_sqlcipher() {
        super::run().await.unwrap();
    }
}
