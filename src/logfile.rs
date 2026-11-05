//! `tuimail.log` in the data folder: what failed, on which account, and of
//! what kind. Other readers may see it, so it holds no mail: nothing here
//! takes a `String`. An error's own text can hold addresses, folder names,
//! search words or a server's echo of any of them, so it's never read: an
//! error is sorted by its type ([`class_of`]), and all that's written is
//! static words and numbers. An account is its place in `accounts.toml`,
//! since its name can be its address.
//!
//! Nothing is written until [`open`] or [`join`], which the demos and tests
//! don't call.

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Write};
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{Local, SecondsFormat};
use rmcp::service::ServerInitializeError;

use crate::{imap, oauth, pop3, secrets, smtp};

pub const FILE: &str = "tuimail.log";
pub const OLD_FILE: &str = "tuimail.log.old";
/// Past this it's moved to [`OLD_FILE`], and a new one begun.
const MAX_SIZE: u64 = 1 << 20;

static LOG: OnceLock<PathBuf> = OnceLock::new();

/// What sort of failure it was. Only static words and numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Timeout,
    ConnectionLost,
    Io(ErrorKind, Option<i32>),
    /// `certificate` or `handshake`.
    Tls(&'static str),
    /// With the IMAP response code, if it's one of [`IMAP_CODES`].
    LoginRefused(Option<&'static str>),
    NeedPassword,
    /// The refresh token is no good any more.
    SignInAgain,
    /// The provider's error, if it's one of [`OAUTH_ERRORS`].
    SignInRefused(&'static str),
    Http(u16),
    /// IMAP NO or BAD, POP3 -ERR, with the code as for `LoginRefused`.
    ServerRefused(Option<&'static str>),
    /// An SMTP reply's code.
    Smtp(u16),
    /// The server isn't speaking its protocol.
    Protocol,
    /// `tuimail mcp` got no answer: `not-listening`, `not-yours` (what
    /// answers at the socket is another user's) or `too-long`.
    Agent(&'static str),
    Keychain,
    /// SQLite's extended result code.
    Store(Option<i32>),
    Other,
}

impl Class {
    /// An error with `text` for the screen and this class for the log.
    pub fn with(self, text: impl Into<String>) -> Classed {
        Classed {
            class: self,
            text: text.into(),
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Class::Timeout => f.write_str("class=timeout"),
            Class::ConnectionLost => f.write_str("class=connection-lost"),
            Class::Io(kind, None) => write!(f, "class=io kind={kind:?}"),
            Class::Io(kind, Some(os)) => write!(f, "class=io kind={kind:?} os={os}"),
            Class::Tls(kind) => write!(f, "class=tls kind={kind}"),
            Class::LoginRefused(None) => f.write_str("class=login-refused"),
            Class::LoginRefused(Some(code)) => write!(f, "class=login-refused code={code}"),
            Class::NeedPassword => f.write_str("class=need-password"),
            Class::SignInAgain => f.write_str("class=sign-in-again"),
            Class::SignInRefused(code) => write!(f, "class=sign-in-refused code={code}"),
            Class::Http(status) => write!(f, "class=http status={status}"),
            Class::ServerRefused(None) => f.write_str("class=server-refused"),
            Class::ServerRefused(Some(code)) => write!(f, "class=server-refused code={code}"),
            Class::Smtp(code) => write!(f, "class=smtp code={code}"),
            Class::Protocol => f.write_str("class=protocol"),
            Class::Agent(kind) => write!(f, "class=agent kind={kind}"),
            Class::Keychain => f.write_str("class=keychain"),
            Class::Store(None) => f.write_str("class=store"),
            Class::Store(Some(code)) => write!(f, "class=store sqlite={code}"),
            Class::Other => f.write_str("class=other"),
        }
    }
}

/// An error whose text is for the screen and whose class is for the log.
#[derive(Debug)]
pub struct Classed {
    class: Class,
    text: String,
}

impl fmt::Display for Classed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Error for Classed {}

/// RFC 5530's response codes and the like. Compared with the server's
/// words, so what's logged is one of these, never the words.
const IMAP_CODES: &[&str] = &[
    "ALERT",
    "ALREADYEXISTS",
    "AUTHENTICATIONFAILED",
    "AUTHORIZATIONFAILED",
    "CANNOT",
    "CLIENTBUG",
    "CONTACTADMIN",
    "CORRUPTION",
    "EXPIRED",
    "EXPUNGEISSUED",
    "INUSE",
    "LIMIT",
    "NONEXISTENT",
    "NOPERM",
    "OVERQUOTA",
    "PRIVACYREQUIRED",
    "SERVERBUG",
    "THROTTLED",
    "TRYCREATE",
    "UNAVAILABLE",
];

/// RFC 6749's error codes and Google's own. As [`IMAP_CODES`].
const OAUTH_ERRORS: &[&str] = &[
    "access_denied",
    "admin_policy_enforced",
    "invalid_client",
    "invalid_grant",
    "invalid_request",
    "invalid_scope",
    "org_internal",
    "server_error",
    "temporarily_unavailable",
    "unauthorized_client",
    "unsupported_grant_type",
];

fn imap_code(words: &str) -> Option<&'static str> {
    let words = words.to_ascii_uppercase();
    IMAP_CODES
        .iter()
        .find(|code| words.contains(&format!("[{code}]")))
        .copied()
}

/// The provider's `error`, if it's one this knows.
pub fn oauth_error(error: &str) -> &'static str {
    OAUTH_ERRORS
        .iter()
        .find(|known| **known == error)
        .copied()
        .unwrap_or("unknown")
}

fn io_class(error: &std::io::Error) -> Class {
    match error.get_ref().map(|inner| inner.downcast_ref()) {
        Some(Some(rustls::Error::InvalidCertificate(_))) => Class::Tls("certificate"),
        Some(Some(_)) => Class::Tls("handshake"),
        _ if error.kind() == ErrorKind::TimedOut => Class::Timeout,
        _ => Class::Io(error.kind(), error.raw_os_error()),
    }
}

fn class_of_one(error: &(dyn Error + 'static)) -> Option<Class> {
    use async_imap::error::Error as Imap;
    if let Some(classed) = error.downcast_ref::<Classed>() {
        Some(classed.class)
    } else if let Some(imap::Refused(words)) = error.downcast_ref() {
        Some(Class::LoginRefused(imap_code(words)))
    } else if let Some(imap::Said(words)) = error.downcast_ref() {
        Some(Class::ServerRefused(imap_code(words)))
    } else if error.is::<smtp::Refused>() || error.is::<pop3::Refused>() {
        Some(Class::LoginRefused(None))
    } else if error.is::<pop3::ServerError>() {
        Some(Class::ServerRefused(None))
    } else if error.is::<secrets::NeedPassword>() {
        Some(Class::NeedPassword)
    } else if error.is::<oauth::SignInAgain>() {
        Some(Class::SignInAgain)
    } else if error.is::<tokio::time::error::Elapsed>() {
        Some(Class::Timeout)
    } else if let Some(imap) = error.downcast_ref::<Imap>() {
        Some(match imap {
            Imap::Io(io) => io_class(io),
            Imap::No(words) | Imap::Bad(words) => Class::ServerRefused(imap_code(words)),
            Imap::ConnectionLost => Class::ConnectionLost,
            Imap::Parse(_) => Class::Protocol,
            Imap::Validate(_) | Imap::Append => Class::ServerRefused(None),
            _ => Class::Other,
        })
    } else if let Some(start) = error.downcast_ref::<ServerInitializeError>() {
        Some(match start {
            ServerInitializeError::ConnectionClosed(_)
            | ServerInitializeError::TransportError { .. } => Class::ConnectionLost,
            ServerInitializeError::ExpectedInitializeRequest(_)
            | ServerInitializeError::UnexpectedInitializeResponse(_) => Class::Protocol,
            _ => Class::Other,
        })
    } else if let Some(io) = error.downcast_ref() {
        Some(io_class(io))
    } else {
        (error.downcast_ref::<rusqlite::Error>())
            .map(|sqlite| Class::Store(sqlite.sqlite_error().map(|e| e.extended_code)))
    }
}

/// `error`'s class, by the types in its chain: its text is never read.
pub fn class_of(error: &anyhow::Error) -> Class {
    error.chain().find_map(class_of_one).unwrap_or(Class::Other)
}

/// The line for a failure of `op`, without its time.
fn failure(op: &'static str, account: Option<usize>, class: Class) -> String {
    match account {
        Some(account) => format!("failed op=\"{op}\" account={account} {class}"),
        None => format!("failed op=\"{op}\" {class}"),
    }
}

/// A source file as a panic names it, `src/app.rs` or, in a dependency,
/// `mail-parser-0.11.0/src/lib.rs`: not the folders it was built in.
fn source_file(file: &str) -> String {
    let file = file.replace('\\', "/");
    let short = file
        .rsplit_once("/registry/src/")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, source)| source.to_string());
    short.unwrap_or(file)
}

/// How the log is opened: added to, made if it's not there, and only by
/// this user.
fn adding() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

/// Adds `text` as one line, with the time and its zone, and closes the
/// file, which is what tells whoever's watching it.
fn append(path: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_SIZE) {
        // Another tuimail may have got here first, or be writing.
        let _ = std::fs::rename(path, path.with_file_name(OLD_FILE));
    }
    let now = Local::now().to_rfc3339_opts(SecondsFormat::Millis, false);
    adding()
        .open(path)?
        .write_all(format!("{now} {text}\n").as_bytes())
}

fn write(text: &str) {
    if let Some(path) = LOG.get() {
        let _ = append(path, text);
    }
}

/// Starts the log in `data_dir`, saying so. An error means it can't be
/// written, which the app says on the screen.
pub fn open(data_dir: &Path, accounts: usize) -> std::io::Result<()> {
    let path = data_dir.join(FILE);
    let start = format!(
        "start version={} accounts={accounts}",
        env!("CARGO_PKG_VERSION")
    );
    append(&path, &start)?;
    let _ = LOG.set(path);
    Ok(())
}

/// Starts the log in `data_dir` without a word: for `tuimail mcp`, which an
/// agent starts for each session, so that its coming and going would be most
/// of the log. An error means it can't be written.
pub fn join(data_dir: &Path) -> std::io::Result<()> {
    let path = data_dir.join(FILE);
    adding().open(&path)?;
    let _ = LOG.set(path);
    Ok(())
}

pub fn stopped() {
    write("stop");
}

/// Writes that `op` failed. Only `error`'s class is taken from it.
pub fn failed(op: &'static str, account: Option<usize>, error: &anyhow::Error) {
    write(&failure(op, account, class_of(error)));
}

/// [`failed`] for what a store write gave, which the app goes on from.
pub fn stored<T, E: Into<anyhow::Error>>(op: &'static str, result: Result<T, E>) {
    if let Err(e) = result {
        failed(op, None, &e.into());
    }
}

/// Writes where a panic happened. Never its message, which can quote mail:
/// `unwrap` on an error has the error's text in it.
pub fn panicked(location: Option<&Location>, contained: bool) {
    let event = if contained {
        "contained-panic"
    } else {
        "crash"
    };
    match location {
        Some(at) => write(&format!(
            "{event} at={}:{}:{}",
            source_file(at.file()),
            at.line(),
            at.column()
        )),
        None => write(event),
    }
}

/// One connection, and its account if it has one: a failure that keeps
/// coming back, as a retry every half a minute does while offline, is
/// written once, and when it ends.
pub struct Link {
    account: Option<usize>,
    down: Option<Class>,
}

impl Link {
    pub fn new(account: usize) -> Self {
        Self {
            account: Some(account),
            down: None,
        }
    }

    /// A connection that's no account's: `tuimail mcp`'s to tuimail.
    pub fn unnumbered() -> Self {
        Self {
            account: None,
            down: None,
        }
    }

    pub fn failed(&mut self, op: &'static str, error: &anyhow::Error) {
        let class = class_of(error);
        if self.down != Some(class) {
            self.down = Some(class);
            write(&failure(op, self.account, class));
        }
    }

    pub fn back(&mut self) {
        if self.down.take().is_some() {
            match self.account {
                Some(account) => write(&format!("recovered account={account}")),
                None => write("recovered"),
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::{mail, tools};
    use anyhow::anyhow;

    /// Everything that must stay out of the log, all in one error text.
    pub const PRIVATE: &[&str] = &[
        "private.person@example.org",
        "Merger talks",
        "Clients/Acme",
        "ya29.access-token",
        "1//refresh-token",
    ];

    pub fn private_words() -> String {
        PRIVATE.join(" ")
    }

    /// `error`'s text holds the private words, and its log line none.
    pub fn assert_kept_out(error: &anyhow::Error, class: Class) {
        let text = format!("{error:#}");
        assert!(
            PRIVATE.iter().any(|word| text.contains(word)),
            "the test's error has nothing private in it: {text}"
        );
        assert_eq!(class_of(error), class, "{text}");
        let line = failure("checking for mail", Some(1), class_of(error));
        for word in PRIVATE {
            assert!(!line.contains(word), "{word} is in {line}");
        }
    }

    #[test]
    fn nothing_private_passes_in_any_error_a_server_can_word() {
        let words = private_words();
        for (error, class) in [
            (anyhow!("plain text {words}"), Class::Other),
            (
                anyhow!(imap::Refused(format!("[AUTHENTICATIONFAILED] {words}"))),
                Class::LoginRefused(Some("AUTHENTICATIONFAILED")),
            ),
            (
                anyhow!(imap::Said(format!("SELECT: no response: {words}"))),
                Class::ServerRefused(None),
            ),
            (
                anyhow!(imap::Said(format!("[TRYCREATE] {words}"))),
                Class::ServerRefused(Some("TRYCREATE")),
            ),
            (
                anyhow!(smtp::Refused(words.clone())),
                Class::LoginRefused(None),
            ),
            (
                anyhow!(pop3::Refused(words.clone())),
                Class::LoginRefused(None),
            ),
            (
                anyhow!(pop3::ServerError(words.clone())),
                Class::ServerRefused(None),
            ),
            (
                anyhow!(oauth::SignInAgain(words.clone())),
                Class::SignInAgain,
            ),
            (
                anyhow!(std::io::Error::other(words.clone())),
                Class::Io(ErrorKind::Other, None),
            ),
            (
                anyhow!(async_imap::error::Error::Bad(words.clone())),
                Class::ServerRefused(None),
            ),
            (
                anyhow!(Class::Keychain.with(words.clone())),
                Class::Keychain,
            ),
            (
                anyhow!(words.clone()).context(format!("cannot read {words}")),
                Class::Other,
            ),
            (
                anyhow!(Class::Timeout.with("x")).context(words.clone()),
                Class::Timeout,
            ),
            (
                anyhow!(ServerInitializeError::ConnectionClosed(words.clone())),
                Class::ConnectionLost,
            ),
            (
                anyhow!(ServerInitializeError::ExpectedInitializeRequest(None))
                    .context(words.clone()),
                Class::Protocol,
            ),
        ] {
            assert_kept_out(&error, class);
        }
    }

    #[test]
    fn a_tls_failure_says_its_kind_and_not_the_names_on_the_certificate() {
        let bad = rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);
        let error = anyhow!(std::io::Error::new(ErrorKind::InvalidData, bad))
            .context(format!("TLS with {} failed", private_words()));
        assert_kept_out(&error, Class::Tls("certificate"));
    }

    #[test]
    fn a_sqlite_failure_is_only_its_code() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let bad = db
            .execute(
                &format!("INSERT INTO \"{}\" VALUES (1)", "Merger talks"),
                [],
            )
            .unwrap_err();
        let error = anyhow!(bad);
        assert!(format!("{error:#}").contains("Merger talks"));
        assert_eq!(class_of(&error), Class::Store(Some(1)));
    }

    #[test]
    fn wanting_a_password_is_its_own_class() {
        let error = anyhow!(secrets::NeedPassword);
        assert_eq!(class_of(&error), Class::NeedPassword);
    }

    #[test]
    fn an_unknown_provider_error_is_not_passed_on() {
        assert_eq!(oauth_error("invalid_client"), "invalid_client");
        assert_eq!(oauth_error("private.person@example.org"), "unknown");
    }

    fn folder() -> PathBuf {
        tools::private_temp_dir().unwrap()
    }

    #[test]
    fn each_line_starts_with_its_time_and_zone_and_ends_written() {
        let path = folder().join(FILE);
        append(&path, "one").unwrap();
        append(&path, "two").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(text.ends_with('\n'));
        for (line, words) in lines.iter().zip(["one", "two"]) {
            let (time, rest) = line.split_once(' ').unwrap();
            chrono::DateTime::parse_from_rfc3339(time).unwrap();
            assert_eq!(rest, words);
        }
    }

    #[test]
    fn a_full_log_rolls_and_the_new_one_starts_small() {
        let dir = folder();
        let path = dir.join(FILE);
        std::fs::write(&path, vec![b'x'; MAX_SIZE as usize + 1]).unwrap();
        append(&path, "after").unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < 100);
        let old = std::fs::metadata(dir.join(OLD_FILE)).unwrap();
        assert_eq!(old.len(), MAX_SIZE + 1);
    }

    #[test]
    fn a_repeating_failure_is_written_once_until_it_changes_or_ends() {
        assert_eq!(
            failure("logging in", Some(2), Class::Timeout),
            "failed op=\"logging in\" account=2 class=timeout"
        );
        let mut link = Link::new(2);
        let timeout = anyhow!(Class::Timeout.with("x"));
        link.failed("logging in", &timeout);
        assert_eq!(link.down, Some(Class::Timeout));
        link.failed("logging in", &timeout);
        link.failed("logging in", &anyhow!(Class::Keychain.with("x")));
        assert_eq!(link.down, Some(Class::Keychain));
        link.back();
        assert_eq!(link.down, None);
    }

    #[test]
    fn joining_says_nothing_and_later_lines_are_written() {
        if let Some(dir) = child_dir() {
            join(&dir).unwrap();
            assert_eq!(std::fs::metadata(dir.join(FILE)).unwrap().len(), 0);
            failed("x", None, &anyhow!("y"));
            return;
        }
        let (run, log) =
            log_of_child("logfile::tests::joining_says_nothing_and_later_lines_are_written");
        assert!(run.status.success(), "{run:?}");
        let events: Vec<_> = log.lines().map(|l| l.split_once(' ').unwrap().1).collect();
        assert_eq!(events, ["failed op=\"x\" class=other"], "{log}");
    }

    #[test]
    fn a_panic_in_a_dependency_is_placed_without_the_builds_folders() {
        let built_at =
            r"C:\Users\me\.cargo\registry\src\index.crates.io-1\html2text-0.17\src\lib.rs";
        assert_eq!(source_file(built_at), "html2text-0.17/src/lib.rs");
        assert_eq!(source_file("src/app.rs"), "src/app.rs");
    }

    const CHILD: &str = "TM_LOG_TEST_DIR";

    /// In the child [`log_of_child`] starts, the folder for its log.
    pub fn child_dir() -> Option<PathBuf> {
        std::env::var_os(CHILD).map(PathBuf::from)
    }

    /// Runs the test `name` in a process of its own, since the log and the
    /// panic hook are the process's, and gives how it ended and its log.
    pub fn log_of_child(name: &str) -> (std::process::Output, String) {
        let dir = folder();
        let run = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env(CHILD, &dir)
            .output()
            .unwrap();
        let log = std::fs::read_to_string(dir.join(FILE)).unwrap_or_default();
        (run, log)
    }

    pub fn assert_nothing_private_in(log: &str) {
        for word in PRIVATE {
            assert!(!log.contains(word), "{word} is in {log}");
        }
    }

    /// The panic is std's own for `unwrap`, which quotes the error.
    #[test]
    fn a_crash_says_where_and_never_what() {
        if let Some(dir) = child_dir() {
            open(&dir, 2).unwrap();
            crate::panic_hook();
            mail::contained(|| panic!("{}", private_words()));
            let parsed: Result<(), String> = Err(private_words());
            std::hint::black_box(parsed).unwrap();
        }
        let (run, log) = log_of_child("logfile::tests::a_crash_says_where_and_never_what");
        assert_eq!(run.status.code(), Some(101), "{run:?}");
        let screen = String::from_utf8_lossy(&run.stderr);
        assert!(screen.contains("Merger talks"), "not quoted: {screen}");
        let events: Vec<_> = log
            .lines()
            .filter_map(|line| line.split(' ').nth(1))
            .collect();
        assert_eq!(events, ["start", "contained-panic", "crash"], "{log}");
        assert!(log.contains("at=src"), "{log}");
        assert_nothing_private_in(&log);
    }

    #[test]
    fn the_demo_and_tests_write_nowhere_unless_opened() {
        assert!(LOG.get().is_none());
        stopped();
        failed("x", None, &anyhow!("y"));
        assert!(LOG.get().is_none());
    }
}
