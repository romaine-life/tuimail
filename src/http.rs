//! HTTP/1.1 POSTs of a form, over a connection made by `net.rs` (TLS, the
//! system's verifier): sign-in's token requests (`oauth.rs`) and one-click
//! unsubscribes (`unsubscribe.rs`); and GETs, the session's fetch of OpenBao's
//! published keys (`relay::token`). One request a connection, closed
//! after, nothing followed (no redirects); the time limits are the
//! caller's, and what's read is capped.

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The most of an answer read.
pub const MAX_RESPONSE: usize = 256 << 10;
/// The most of an answer's head read, when only its status is wanted.
pub const MAX_HEAD: usize = 16 << 10;

/// A form's POST: its head and body. `host` and `path` are the caller's,
/// checked to hold nothing that ends a line.
fn request(host: &str, path: &str, accept: &str, body: &str) -> Result<String> {
    let line_safe = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic());
    if !line_safe(host) || !line_safe(path) || !line_safe(accept) {
        bail!("that address can't be asked");
    }
    Ok(format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: tuimail\r\n\
         Accept: {accept}\r\nContent-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ))
}

/// GETs `path` and reads the answer: its status and body, at most [`MAX_RESPONSE`].
pub async fn get<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    path: &str,
    accept: &str,
) -> Result<(u16, Vec<u8>)> {
    let line_safe = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic());
    if !line_safe(host) || !line_safe(path) || !line_safe(accept) {
        bail!("that address can't be asked");
    }
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: tuimail\r\n\
         Accept: {accept}\r\nConnection: close\r\n\r\n"
    );
    send(stream, &request).await?;
    let answer = read_until(stream, host, MAX_RESPONSE, |_| false).await?;
    parse_response(&answer)
}

async fn send<S: AsyncWrite + Unpin>(stream: &mut S, request: &str) -> Result<()> {
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Reads into `answer` until `done` says it has enough, the other end
/// closes, or past `max` (an error).
async fn read_until<S: AsyncRead + Unpin>(
    stream: &mut S,
    host: &str,
    max: usize,
    done: impl Fn(&[u8]) -> bool,
) -> Result<Vec<u8>> {
    let mut answer = Vec::new();
    let mut chunk = [0u8; 8192];
    while !done(&answer) {
        let n = match stream.read(&mut chunk).await {
            Ok(n) => n,
            // TLS closed without its close_notify, as some servers do:
            // what came is all there is.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => 0,
            Err(e) => return Err(e.into()),
        };
        if n == 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..n]);
        if answer.len() > max {
            bail!("{host}'s answer is too long");
        }
    }
    Ok(answer)
}

/// POSTs a form and reads the answer: its status and body, at most
/// [`MAX_RESPONSE`].
pub async fn post_form<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    path: &str,
    accept: &str,
    body: &str,
) -> Result<(u16, Vec<u8>)> {
    send(stream, &request(host, path, accept, body)?).await?;
    let answer = read_until(stream, host, MAX_RESPONSE, |_| false).await?;
    parse_response(&answer)
}

/// POSTs a form and reads only the answer's head, at most [`MAX_HEAD`]:
/// its status is all that's wanted.
pub async fn post_for_status<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    host: &str,
    path: &str,
    body: &str,
) -> Result<u16> {
    send(stream, &request(host, path, "*/*", body)?).await?;
    let answer = read_until(stream, host, MAX_HEAD, |a| head_end(a).is_some()).await?;
    let end = head_end(&answer).context("the answer has no end of its head")?;
    status_of(&String::from_utf8_lossy(&answer[..end])).context("the answer has no status")
}

/// Where an answer's head ends (its blank line).
fn head_end(answer: &[u8]) -> Option<usize> {
    answer.windows(4).position(|w| w == b"\r\n\r\n")
}

/// The status of an answer's head.
fn status_of(head: &str) -> Option<u16> {
    head.split("\r\n")
        .next()
        .and_then(|l| l.strip_prefix("HTTP/1."))
        .and_then(|l| l.get(2..5))
        .and_then(|s| s.parse::<u16>().ok())
}

/// An HTTP/1.1 answer's status and body (by its length, its chunks, or
/// all that came).
pub fn parse_response(answer: &[u8]) -> Result<(u16, Vec<u8>)> {
    let end = head_end(answer).context("the answer has no end of its head")?;
    let head = String::from_utf8_lossy(&answer[..end]);
    let rest = &answer[end + 4..];
    let status = status_of(&head).context("the answer has no status")?;
    let header = |name: &str| {
        head.split("\r\n").skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    };
    let body = if header("transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        unchunk(rest)?
    } else if let Some(length) = header("content-length").and_then(|l| l.parse::<usize>().ok()) {
        rest.get(..length)
            .context("the answer is cut short")?
            .to_vec()
    } else {
        rest.to_vec()
    };
    Ok((status, body))
}

/// A chunked body put back together.
fn unchunk(mut rest: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .context("a chunk has no size")?;
        let size_text = String::from_utf8_lossy(&rest[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).context("a chunk's size didn't read")?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        out.extend_from_slice(rest.get(..size).context("a chunk is cut short")?);
        rest = rest.get(size + 2..).context("a chunk is cut short")?;
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn answers_are_read_by_length_chunks_or_to_the_end() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello and more";
        assert_eq!(parse_response(plain).unwrap(), (200, b"hello".to_vec()));
        let chunked = b"HTTP/1.1 400 Bad Request\r\nTransfer-Encoding: chunked\r\n\r\n\
            4\r\n{\"a\"\r\n3;x=y\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(
            parse_response(chunked).unwrap(),
            (400, b"{\"a\":1}".to_vec())
        );
        let open = b"HTTP/1.0 200 OK\r\n\r\nall of it";
        assert_eq!(parse_response(open).unwrap(), (200, b"all of it".to_vec()));
        for bad in [
            &b"garbage"[..],
            b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\nshort",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
        ] {
            assert!(parse_response(bad).is_err());
        }
    }

    /// A server on the other end of a pipe: reads one request, answers
    /// `answer`, and gives back the request's head and body.
    pub fn serve_once(
        answer: &'static [u8],
    ) -> (
        tokio::io::DuplexStream,
        tokio::task::JoinHandle<(String, String)>,
    ) {
        let (client, mut server) = tokio::io::duplex(64 << 10);
        let served = tokio::spawn(async move {
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            // The head and the short body come together.
            while head_end(&request).is_none() {
                let n = server.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..n]);
            }
            server.write_all(answer).await.unwrap();
            drop(server);
            let request = String::from_utf8(request).unwrap();
            let (head, body) = request.split_once("\r\n\r\n").unwrap();
            (head.to_string(), body.to_string())
        });
        (client, served)
    }

    #[tokio::test]
    async fn a_form_is_posted_and_its_answer_read() {
        let (mut client, served) = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        let answer = post_form(
            &mut client,
            "login.example",
            "/token",
            "application/json",
            "a=1",
        )
        .await
        .unwrap();
        assert_eq!(answer, (200, b"{}".to_vec()));
        let (head, body) = served.await.unwrap();
        assert!(
            head.starts_with("POST /token HTTP/1.1\r\nHost: login.example\r\n"),
            "{head}"
        );
        assert!(head.contains("Content-Length: 3\r\n"));
        assert_eq!(body, "a=1");
    }

    #[tokio::test]
    async fn only_the_status_is_read_when_thats_all_wanted() {
        // A body past what's read whole: the head is enough.
        let (mut client, _served) =
            serve_once(b"HTTP/1.1 202 Accepted\r\nContent-Length: 999999\r\n\r\n<html>");
        let status = post_for_status(&mut client, "list.example", "/u?x=1", "a=1")
            .await
            .unwrap();
        assert_eq!(status, 202);
        let (mut client, _served) = serve_once(b"nonsense\r\n\r\n");
        assert!(
            post_for_status(&mut client, "list.example", "/", "a=1")
                .await
                .is_err()
        );
    }

    #[test]
    fn nothing_that_ends_a_line_goes_in_a_request() {
        for (host, path) in [
            ("list.example", "/a b"),
            ("list.example", "/a\r\nX-Evil: 1"),
            ("list.example\r\n", "/"),
            ("", "/"),
            ("list.example", ""),
        ] {
            assert!(request(host, path, "*/*", "").is_err(), "{host:?} {path:?}");
        }
        assert!(request("list.example", "/u?a=1&b=%20", "*/*", "x").is_ok());
    }
}
