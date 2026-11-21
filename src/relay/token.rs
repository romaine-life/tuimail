//! Checks the token a PC presents to the session (as shows' `remote_token.rs` does, made async
//! and checked with `ring`, which tuimail's TLS already uses).
//!
//! OpenBao signs it (nelsong6/openbao, role `tuimail`) for a home device that logged in with
//! its own key, for five minutes. The session holds no secret for this: it checks the ES256
//! signature against the keys OpenBao publishes, fetched when a token names a key it hasn't
//! seen. Which devices may ask for one is OpenBao's policies (each device's `token_policies`).

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use futures::future::BoxFuture;
use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use serde::Deserialize;
use tokio::sync::Mutex;

pub const ISSUER: &str = "https://bao.home.romaine.life/v1/identity/oidc";
const KEYS_HOST: &str = "bao.home.romaine.life";
const KEYS_PATH: &str = "/v1/identity/oidc/.well-known/keys";
pub const AUDIENCE: &str = "tuimail";
/// The audience of the token the NAS's backup job brings for the store's snapshot (`/backup`):
/// OpenBao gives it to the NAS's device alone.
pub const BACKUP_AUDIENCE: &str = "tuimail-backup";
/// How long fetching OpenBao's keys may take.
const FETCH_FOR: Duration = Duration::from_secs(10);
/// The keys are fetched again for an unknown key at most this often, so tokens naming made-up
/// keys can't make the session ask OpenBao over and over.
const REFETCH_AFTER: Duration = Duration::from_secs(60);
/// How far a token's times may be off (OpenBao and the session share the cluster's clock).
pub const LEEWAY: i64 = 30;
/// The longest token looked at.
const MAX_TOKEN: usize = 8 << 10;

type FetchKeys = Box<dyn Fn() -> BoxFuture<'static, Result<JwkSet, String>> + Send + Sync>;

/// OpenBao's published keys (a JWK set). Only P-256 keys are used; others are passed over.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct JwkSet {
    keys: Vec<Jwk>,
}

#[derive(Clone, Debug, Deserialize)]
struct Jwk {
    #[serde(default)]
    kid: String,
    #[serde(default)]
    kty: String,
    #[serde(default)]
    crv: String,
    #[serde(default)]
    x: String,
    #[serde(default)]
    y: String,
}

impl JwkSet {
    /// The public key `kid` names, as an uncompressed P-256 point.
    fn find(&self, kid: &str) -> Option<Vec<u8>> {
        let jwk = self
            .keys
            .iter()
            .find(|k| k.kid == kid && k.kty == "EC" && k.crv == "P-256")?;
        let (x, y) = (B64.decode(&jwk.x).ok()?, B64.decode(&jwk.y).ok()?);
        (x.len() == 32 && y.len() == 32).then(|| [&[4u8][..], &x, &y].concat())
    }
}

/// What a checked token says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// The machine that asked for it (OpenBao names each device's entity after the device).
    pub device: String,
    /// Unix seconds.
    pub expires: i64,
}

#[derive(Deserialize)]
struct Head {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    exp: i64,
    nbf: Option<i64>,
    iss: String,
    aud: Audience,
    #[serde(default)]
    device: String,
}

pub struct TokenChecker {
    fetch: FetchKeys,
    /// The keys, and when they were last fetched.
    keys: Mutex<(JwkSet, Option<std::time::Instant>)>,
}

/// OpenBao's published keys, over TLS checked by the system's verifier.
async fn openbao_keys() -> Result<JwkSet, String> {
    let fetch = async {
        let endpoint = crate::net::Endpoint {
            host: KEYS_HOST.into(),
            port: 443,
            security: crate::account::Security::Tls,
            tls: crate::net::tls_config(None)?,
        };
        let mut conn = crate::net::connect(&endpoint).await?;
        let (status, body) =
            crate::http::get(&mut conn, KEYS_HOST, KEYS_PATH, "application/json").await?;
        anyhow::ensure!(status == 200, "OpenBao answered {status}");
        Ok::<_, anyhow::Error>(body)
    };
    let body = tokio::time::timeout(FETCH_FOR, fetch)
        .await
        .map_err(|_| "OpenBao didn't answer in time".to_string())?
        .map_err(|e| format!("cannot read OpenBao's keys: {e:#}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("invalid OpenBao keys: {e}"))
}

/// A token part's JSON.
fn part<T: for<'a> Deserialize<'a>>(text: &str) -> Result<T, String> {
    let bytes = B64.decode(text).map_err(|_| "not a signed token")?;
    serde_json::from_slice(&bytes).map_err(|_| "not a signed token".into())
}

impl TokenChecker {
    pub fn openbao() -> Self {
        Self::with_fetch(Box::new(|| Box::pin(openbao_keys())))
    }

    pub fn with_fetch(fetch: FetchKeys) -> Self {
        Self {
            fetch,
            keys: Mutex::new((JwkSet::default(), None)),
        }
    }

    async fn key(&self, kid: &str) -> Result<Vec<u8>, String> {
        let mut kept = self.keys.lock().await;
        let recent = kept.1.is_some_and(|at| at.elapsed() < REFETCH_AFTER);
        if kept.0.find(kid).is_none() && !recent {
            kept.0 = (self.fetch)().await?;
            kept.1 = Some(std::time::Instant::now());
        }
        kept.0
            .find(kid)
            .ok_or_else(|| "the token's signing key isn't one of OpenBao's".into())
    }

    /// Checks a PC's token: for the session ([`AUDIENCE`]).
    pub async fn check(&self, token: &str) -> Result<Checked, String> {
        self.check_for(token, AUDIENCE).await
    }

    /// Checks an ES256 token: its signature by the OpenBao key it names, its issuer, that it's
    /// for `audience`, and its times.
    pub async fn check_for(&self, token: &str, audience: &str) -> Result<Checked, String> {
        if token.len() > MAX_TOKEN {
            return Err("not a signed token".into());
        }
        let Some((signed, signature)) = token.rsplit_once('.') else {
            return Err("not a signed token".into());
        };
        let Some((head, body)) = signed.split_once('.') else {
            return Err("not a signed token".into());
        };
        let head: Head = part(head)?;
        if head.alg != "ES256" {
            return Err("the token isn't signed as OpenBao signs".into());
        }
        let kid = head.kid.ok_or("the token names no signing key")?;
        let key = self.key(&kid).await?;
        let signature = B64.decode(signature).map_err(|_| "not a signed token")?;
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &key)
            .verify(signed.as_bytes(), &signature)
            .map_err(|_| "token refused: its signature isn't OpenBao's")?;
        let claims: Claims = part(body)?;
        if claims.iss != ISSUER {
            return Err("token refused: another issuer's".into());
        }
        let for_us = match &claims.aud {
            Audience::One(aud) => aud == audience,
            Audience::Many(auds) => auds.iter().any(|a| a == audience),
        };
        if !for_us {
            return Err("token refused: it's for another service".into());
        }
        let now = chrono::Utc::now().timestamp();
        if now > claims.exp + LEEWAY {
            return Err("token refused: it ran out".into());
        }
        if claims.nbf.is_some_and(|nbf| nbf > now + LEEWAY) {
            return Err("token refused: it isn't good yet".into());
        }
        Ok(Checked {
            device: crate::text::one_line(&claims.device, 40),
            expires: claims.exp,
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A throwaway P-256 pair for tests (shows' `remote_token.rs` tests use the same one).
    const KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgKL/xHHDaM1YrKnv2
mDuMrlN4kCyK6H6Zctd4R+n5AAmhRANCAAS5N+qDIf54e0j5JWN5q2/oG8JJm9YN
RXcElwx4NzzIjreNOTuPo860Vm9cKijgXPCxamwY+f4Lt32uD/NqurAW
-----END PRIVATE KEY-----
";
    const X: &str = "uTfqgyH-eHtI-SVjeatv6BvCSZvWDUV3BJcMeDc8yI4";
    const Y: &str = "t405O4-jzrRWb1wqKOBc8LFqbBj5_gu3fa4P82q6sBY";

    fn keys(kid: &str) -> JwkSet {
        serde_json::from_value(json!({"keys":[
            {"kty":"RSA","kid":kid,"n":"AQAB","e":"AQAB"},
            {"kty":"EC","crv":"P-256","use":"sig","alg":"ES256","kid":kid,"x":X,"y":Y}
        ]}))
        .unwrap()
    }

    /// `claims` signed with the test key as OpenBao signs, naming key `kid`.
    pub fn token(kid: &str, claims: Value) -> String {
        let pem: String = KEY.lines().filter(|l| !l.starts_with("-----")).collect();
        let der = base64::engine::general_purpose::STANDARD
            .decode(pem)
            .unwrap();
        let rng = ring::rand::SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &rng).unwrap();
        let head = B64.encode(json!({"alg": "ES256", "typ": "JWT", "kid": kid}).to_string());
        let body = B64.encode(claims.to_string());
        let signed = format!("{head}.{body}");
        let signature = pair.sign(&rng, signed.as_bytes()).unwrap();
        format!("{signed}.{}", B64.encode(signature.as_ref()))
    }

    pub fn claims(aud: &str, iss: &str, left: i64) -> Value {
        json!({"aud": aud, "iss": iss, "exp": chrono::Utc::now().timestamp() + left, "device": "nelsonpc"})
    }

    /// A token as OpenBao would sign it for the session, good for `left` seconds.
    pub fn good(left: i64) -> String {
        token("k1", claims(AUDIENCE, ISSUER, left))
    }

    pub fn checker(kid: &'static str) -> (TokenChecker, Arc<AtomicUsize>) {
        let fetches = Arc::new(AtomicUsize::new(0));
        let count = fetches.clone();
        (
            TokenChecker::with_fetch(Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(keys(kid)) })
            })),
            fetches,
        )
    }

    #[tokio::test]
    async fn a_tuimail_token_from_openbao_passes_and_keys_are_fetched_only_when_new() {
        let (checker, fetches) = checker("k1");
        for _ in 0..3 {
            let checked = checker.check(&good(300)).await.unwrap();
            assert_eq!(checked.device, "nelsonpc");
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        // An audience list naming the session passes too.
        let mut listed = claims(AUDIENCE, ISSUER, 300);
        listed["aud"] = json!(["other", AUDIENCE]);
        assert!(checker.check(&token("k1", listed)).await.is_ok());
    }

    #[tokio::test]
    async fn another_audience_issuer_key_or_an_expired_token_is_refused() {
        let (checker, _) = checker("k1");
        let mut early = claims(AUDIENCE, ISSUER, 300);
        early["nbf"] = json!(chrono::Utc::now().timestamp() + 600);
        let refused = [
            token("k1", claims("shows", ISSUER, 300)),
            token("k1", claims(AUDIENCE, "https://elsewhere", 300)),
            token("k1", claims(AUDIENCE, ISSUER, -600)),
            token("k1", early),
            token("k1", json!({"aud": AUDIENCE, "iss": ISSUER})),
            token("unknown", claims(AUDIENCE, ISSUER, 300)),
            "not a token".into(),
            "a.b.c.d".into(),
            "x".repeat(MAX_TOKEN + 1),
        ];
        for token in refused {
            assert!(checker.check(&token).await.is_err(), "{token}");
        }
    }

    #[tokio::test]
    async fn made_up_keys_dont_make_it_fetch_again_and_again() {
        let (checker, fetches) = checker("k1");
        for kid in ["a", "b", "c"] {
            let token = token(kid, claims(AUDIENCE, ISSUER, 300));
            assert!(checker.check(&token).await.is_err());
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert!(checker.check(&good(300)).await.is_ok());
    }

    #[tokio::test]
    async fn a_token_signed_by_another_key_with_a_known_kid_is_refused() {
        let (checker, _) = checker("k1");
        let good = good(300);
        let (head, rest) = good.split_once('.').unwrap();
        let (_, sig) = rest.rsplit_once('.').unwrap();
        let forged = serde_json::to_vec(
            &json!({"aud":AUDIENCE,"iss":ISSUER,"exp":4102444800u64,"device":"nelsonpc"}),
        )
        .unwrap();
        let body = B64.encode(forged);
        assert!(
            checker
                .check(&format!("{head}.{body}.{sig}"))
                .await
                .is_err()
        );
        // Nor one that says it's signed some other way.
        let none = B64.encode(json!({"alg": "none", "kid": "k1"}).to_string());
        assert!(checker.check(&format!("{none}.{rest}")).await.is_err());
    }

    /// Against OpenBao itself, by hand on a home PC: a token from `romaine-auth home-token --role
    /// tuimail --out FILE`, named by `TUIMAIL_TOKEN_FILE`, passes the real published keys.
    #[tokio::test]
    #[ignore = "asks OpenBao: run by hand with TUIMAIL_TOKEN_FILE"]
    async fn live_openbao_token_passes() {
        let file = std::env::var("TUIMAIL_TOKEN_FILE").expect("TUIMAIL_TOKEN_FILE");
        let json: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        let token = json["token"].as_str().unwrap();
        let checked = TokenChecker::openbao().check(token).await.unwrap();
        assert!(!checked.device.is_empty());
        let mut forged = token.to_string();
        forged.pop();
        forged.push(if token.ends_with('A') { 'B' } else { 'A' });
        assert!(TokenChecker::openbao().check(&forged).await.is_err());
    }

    #[tokio::test]
    async fn an_unreachable_key_source_refuses_rather_than_passes() {
        let checker = TokenChecker::with_fetch(Box::new(|| {
            Box::pin(async { Err("OpenBao is down".into()) })
        }));
        assert_eq!(
            checker.check(&good(300)).await.unwrap_err(),
            "OpenBao is down"
        );
    }
}
