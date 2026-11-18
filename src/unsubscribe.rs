//! Unsubscribing from a mailing list for an AI agent (`App::agent_unsubscribe`),
//! the way its message says (List-Unsubscribe, RFC 2369): with one click
//! where List-Unsubscribe-Post offers it (RFC 8058: a POST of
//! [`ONE_CLICK`] to the https link, nothing else sent, nothing followed),
//! else by mail to its one address. A web page to open is left to the
//! user (`U`), as is a link tuimail won't POST to.
//!
//! Everything here is the sender's: the link's host must be a name on the
//! public internet, on port 443, the same site the user and the agent are
//! told (`app::link_host`); the address must be one address. DKIM isn't
//! checked (RFC 8058 asks senders to sign these headers): a forged header
//! gets at most one POST of [`ONE_CLICK`], with nothing of the user's in
//! it, to a public https site.

use std::time::Duration;

use anyhow::{Result, bail};

use crate::account::{self, Security};
use crate::compose::{self, Draft};
use crate::logfile::Class;
use crate::net::{self, Endpoint};
use crate::{app, http, mail};

/// What List-Unsubscribe-Post says, and what's POSTed (RFC 8058).
pub const ONE_CLICK: &str = "List-Unsubscribe=One-Click";
/// How long a one-click unsubscribe may take, connecting included.
const TIME_LIMIT: Duration = Duration::from_secs(30);

/// Where a one-click POST goes: a host name and the path with its query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub path: String,
}

/// How a message can be unsubscribed from.
#[derive(Clone, Debug, PartialEq)]
pub enum Way {
    /// One POST to this.
    OneClick(Target),
    /// A message to one address: the draft, and the address.
    Email { draft: Draft, address: String },
    /// A web page, for the user to open.
    NeedsUser(String),
    /// Nothing tuimail sends to, and why.
    Refused(&'static str),
    /// No List-Unsubscribe.
    Unsaid,
}

/// How `raw`'s head says to unsubscribe: one click if it offers it to a
/// link tuimail POSTs to, else mail to one address, else a page for the
/// user.
pub fn way(raw: &[u8]) -> Way {
    let found = mail::unsubscribe_of(raw);
    if found.one_click
        && let Some(target) = found.https.as_deref().and_then(one_click_target)
    {
        return Way::OneClick(target);
    }
    if let Some((draft, address)) = found.mailto.as_deref().and_then(email) {
        return Way::Email { draft, address };
    }
    match (found.https, found.mailto) {
        (Some(url), _) => Way::NeedsUser(url),
        (None, Some(_)) => Way::Refused("its unsubscribe address isn't one address"),
        (None, None) => Way::Unsaid,
    }
}

/// The https link as a place to POST to, if it's one: no user or password
/// in it, port 443, a host name (not an address, nor this computer's) that
/// is the site `app::link_host` names. The fragment is dropped, and bytes
/// a request line can't carry are escaped.
pub fn one_click_target(url: &str) -> Option<Target> {
    let rest = (url.get(..8))
        .filter(|s| s.eq_ignore_ascii_case("https://"))
        .map(|_| &url[8..])?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(end);
    // Browsers read `\` as `/`, and `%` decoded: neither in the host.
    if authority.contains(['@', '\\', '%']) {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        Some((host, "443")) => host,
        Some(_) => return None,
        None => authority,
    }
    .to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    // The last label starts with a letter: `0x7f.1` and `2130706433` are
    // addresses to the system's resolver.
    let named = labels.len() > 1
        && labels.iter().all(|l| !l.is_empty())
        && (labels.last()?.chars().next()).is_some_and(|c| c.is_ascii_alphabetic());
    if !named
        || !account::valid_host(&host)
        || host.contains([':', '[', ']'])
        || account::is_local(&host)
        || host.ends_with(".localhost")
        || !same_site(&host, &app::link_host(url))
    {
        return None;
    }
    let path = rest.split('#').next().unwrap_or("");
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let path = (path.bytes())
        .map(|b| {
            if b.is_ascii_graphic() {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    Some(Target { host, path })
}

/// Whether `host` is the site `shown` names (which keeps a port).
fn same_site(host: &str, shown: &str) -> bool {
    shown == host || shown.strip_suffix(":443") == Some(host)
}

/// The message a `mailto:` link asks for: to its one address only, its
/// subject (or "unsubscribe"), and "unsubscribe" as the text. A link
/// naming more than one address, or none, gets none.
pub fn email(link: &str) -> Option<(Draft, String)> {
    let mut draft = app::mailto_draft(link)?;
    let addresses = compose::parse_addresses(&draft.to).ok()?;
    let [address] = addresses.as_slice() else {
        return None;
    };
    draft.to = address.addr.clone();
    if draft.subject.trim().is_empty() {
        draft.subject = "unsubscribe".into();
    }
    draft.body = "unsubscribe\n".into();
    Some((draft, address.addr.clone()))
}

/// The domain of an address, all that's said of it.
pub fn domain(address: &str) -> String {
    address
        .rsplit_once('@')
        .map_or("", |(_, domain)| domain)
        .to_ascii_lowercase()
}

/// Unsubscribes with one click: [`ONE_CLICK`] POSTed to `target` over
/// TLS. Done if the site answers 2xx; a redirect isn't followed.
pub async fn one_click(target: &Target) -> Result<()> {
    let endpoint = Endpoint {
        host: target.host.clone(),
        port: 443,
        security: Security::Tls,
        tls: net::tls_config(None)?,
    };
    let host = &target.host;
    let posted = tokio::time::timeout(TIME_LIMIT, async {
        let mut conn = net::connect(&endpoint).await?;
        http::post_for_status(&mut conn, host, &target.path, ONE_CLICK).await
    });
    let status = posted
        .await
        .map_err(|_| Class::Timeout.with(format!("{host} didn't answer in time")))??;
    match status {
        200..=299 => Ok(()),
        300..=399 => bail!(Class::Http(status).with(format!(
            "{host} answered with a redirect (HTTP {status}), which isn't followed"
        ))),
        _ => bail!(Class::Http(status).with(format!("{host} answered HTTP {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(headers: &str) -> Vec<u8> {
        format!("From: news@shop.example\r\nSubject: Sale\r\n{headers}\r\nhi\r\n").into_bytes()
    }

    #[test]
    fn one_click_is_only_what_rfc_8058_says() {
        let link = "List-Unsubscribe: <mailto:u@shop.example?subject=stop>, \
                    <https://shop.example/u?id=7#top>\r\n";
        let post = "List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n";
        assert_eq!(
            way(&message(&format!("{link}{post}"))),
            Way::OneClick(Target {
                host: "shop.example".into(),
                path: "/u?id=7".into(),
            })
        );
        // Without the Post header, or with anything else in it: mail.
        for other in [
            "",
            "List-Unsubscribe-Post: List-Unsubscribe=One-Click-Please\r\n",
            "List-Unsubscribe-Post: yes\r\n",
            "X-List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n",
        ] {
            let way = way(&message(&format!("{link}{other}")));
            assert!(matches!(way, Way::Email { .. }), "{other:?}: {way:?}");
        }
        // A page and no address: the user's.
        let page = "List-Unsubscribe: <https://shop.example/u?id=7>\r\n";
        assert_eq!(
            way(&message(page)),
            Way::NeedsUser("https://shop.example/u?id=7".into())
        );
        assert_eq!(way(&message("")), Way::Unsaid);
        // Only the head counts: a body that looks like one says nothing.
        let body = format!("From: a@b.example\r\n\r\n{link}{post}").into_bytes();
        assert_eq!(way(&body), Way::Unsaid);
    }

    #[test]
    fn a_one_click_link_is_a_public_https_site_on_443() {
        let target = |url: &str| one_click_target(url).map(|t| (t.host, t.path));
        assert_eq!(
            target("https://Lists.Shop.example:443/u?a=1&b=two words"),
            Some(("lists.shop.example".into(), "/u?a=1&b=two%20words".into()))
        );
        assert_eq!(
            target("https://shop.example?u=1"),
            Some(("shop.example".into(), "/?u=1".into()))
        );
        assert_eq!(
            target("https://shop.example/ü"),
            Some(("shop.example".into(), "/%C3%BC".into()))
        );
        for bad in [
            "http://shop.example/u",
            "https://user:pw@shop.example/u",
            "https://shop.example:8443/u",
            "https://shop.example\\@evil.example/u",
            "https://127.0.0.1/u",
            "https://[::1]/u",
            "https://localhost/u",
            "https://admin.localhost/u",
            "https://0x7f.1/u",
            "https://2130706433/u",
            "https://intranet/u",
            "https://shop%2Eexample/u",
            "https://xn--shp-bla..example/u",
            "https://shоp.example/u",
        ] {
            assert_eq!(target(bad), None, "{bad}");
        }
    }

    #[test]
    fn mail_goes_to_the_links_one_address_only() {
        let (draft, address) =
            email("mailto:leave@lists.example?subject=Stop%20it&cc=boss@x.example&body=yes")
                .unwrap();
        assert_eq!(address, "leave@lists.example");
        assert_eq!(draft.to, "leave@lists.example");
        assert_eq!(draft.subject, "Stop it");
        assert_eq!(draft.body, "unsubscribe\n");
        assert!(draft.cc.is_empty() && draft.bcc.is_empty() && draft.attachments.is_empty());
        let (draft, _) = email("mailto:leave@lists.example").unwrap();
        assert_eq!(draft.subject, "unsubscribe");
        for bad in [
            "mailto:a@lists.example,b@other.example",
            "mailto:a@lists.example%2Cb@other.example",
            "mailto:",
            "mailto:not-an-address",
            "https://lists.example/u",
        ] {
            assert!(email(bad).is_none(), "{bad}");
        }
        assert_eq!(domain("Leave@Lists.Example"), "lists.example");
    }

    #[tokio::test]
    async fn the_one_click_request_is_rfc_8058s() {
        let (mut client, served) =
            http::tests::serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let status = http::post_for_status(&mut client, "shop.example", "/u?id=7", ONE_CLICK)
            .await
            .unwrap();
        assert_eq!(status, 200);
        let (head, body) = served.await.unwrap();
        assert!(
            head.starts_with("POST /u?id=7 HTTP/1.1\r\nHost: shop.example\r\n"),
            "{head}"
        );
        assert!(
            head.contains("Content-Type: application/x-www-form-urlencoded\r\n"),
            "{head}"
        );
        assert!(!head.contains("Cookie"), "{head}");
        assert_eq!(body, "List-Unsubscribe=One-Click");
    }
}
