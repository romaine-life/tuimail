//! `tuimail.log` in the data folder: what failed, on which account, and of
//! what kind. Other readers may see it, so it holds no mail: nothing here
//! takes a `String`. An error's own text can hold addresses, folder names,
//! search words or a server's echo of any of them, so it's never read: an
//! error is sorted by its type ([`class_of`]), and all that's written is
//! static words and numbers. An account is its place in `accounts.toml`,
//! since its name can be its address.
//!
//! Each line is the house's one log line (logline, AHT-204): the time with
//! milliseconds and its zone, the pid, `info`, `warn` or `error`, the event
//! as the message (`start`, `failed`, ...), then its fields (`op`, `account`,
//! `class`, ...). Fields are [`Value`]s, which hold only static words and
//! numbers. logline writes the file, closing it after each line, and moves
//! it to [`OLD_FILE`] past 1 MiB.
//!
//! Nothing is written until [`open`] or [`join`], which the demos and tests
//! don't call.

use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Write};
use std::panic::Location;
use std::path::Path;
use std::sync::OnceLock;

use chrono::Local;
use logline::Level;
use rmcp::service::ServerInitializeError;

use crate::{imap, oauth, pop3, secrets, smtp};

pub const FILE: &str = "tuimail.log";
/// Where logline moves [`FILE`] past 1 MiB (`logline::LIMIT`); the data
/// folder's check knows it by name (`config::make_private_dir`, Unix only).
#[cfg_attr(not(unix), allow(dead_code))]
pub const OLD_FILE: &str = "tuimail.log.old";

static LOG: OnceLock<logline::Logger> = OnceLock::new();
/// The session (`tuimail session`) writes its lines to stdout instead, where its pod's log is
/// shipped to the house's log pile; the same words, so the same privacy.
static TO_STDOUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    /// The command palette's fzt: `not-installed`, `not-loaded` (its
    /// library is there but isn't fzt's), `refused` (fzt answered an error).
    Palette(&'static str),
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

impl Class {
    /// The class's fields on a line: `class`, then its kind, code or number.
    fn fields(self) -> Vec<(&'static str, Value)> {
        use Value::{Kind, Number, Word};
        let (class, detail) = match self {
            Class::Timeout => ("timeout", None),
            Class::ConnectionLost => ("connection-lost", None),
            Class::Io(kind, os) => {
                let mut fields = vec![("class", Word("io")), ("kind", Kind(kind))];
                fields.extend(os.map(|os| ("os", Number(os.into()))));
                return fields;
            }
            Class::Tls(kind) => ("tls", Some(("kind", Word(kind)))),
            Class::LoginRefused(code) => ("login-refused", code.map(|c| ("code", Word(c)))),
            Class::NeedPassword => ("need-password", None),
            Class::SignInAgain => ("sign-in-again", None),
            Class::SignInRefused(code) => ("sign-in-refused", Some(("code", Word(code)))),
            Class::Http(status) => ("http", Some(("status", Number(status.into())))),
            Class::ServerRefused(code) => ("server-refused", code.map(|c| ("code", Word(c)))),
            Class::Smtp(code) => ("smtp", Some(("code", Number(code.into())))),
            Class::Protocol => ("protocol", None),
            Class::Agent(kind) => ("agent", Some(("kind", Word(kind)))),
            Class::Keychain => ("keychain", None),
            Class::Palette(kind) => ("palette", Some(("kind", Word(kind)))),
            Class::Store(code) => ("store", code.map(|c| ("sqlite", Number(c.into())))),
            Class::Other => ("other", None),
        };
        let mut fields = vec![("class", Word(class))];
        fields.extend(detail);
        fields
    }
}

/// A field's value: a static word, a number, an I/O error's kind or a
/// source location. Never text from outside, so never a `String` a caller
/// hands in.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    Word(&'static str),
    Number(i64),
    Kind(ErrorKind),
    /// A panic's place, `src/app.rs:12:5` ([`source_file`]).
    At(String, u32, u32),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Word(word) => f.write_str(word),
            Value::Number(n) => write!(f, "{n}"),
            Value::Kind(kind) => write!(f, "{kind:?}"),
            Value::At(file, line, column) => write!(f, "{file}:{line}:{column}"),
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

type Fields = Vec<(&'static str, Value)>;

/// The fields of a failure of `op`: the op, the account, the class.
fn failure(op: &'static str, account: Option<usize>, class: Class) -> Fields {
    let mut fields = vec![("op", Value::Word(op))];
    fields.extend(numbered(account));
    fields.extend(class.fields());
    fields
}

/// `account=<n>`, when there is one.
fn numbered(account: Option<usize>) -> Option<(&'static str, Value)> {
    account.map(|n| ("account", Value::Number(n as i64)))
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

/// Writes one line: to stdout for the session, else to the log once it's
/// opened, else nowhere.
fn write(level: Level, event: &'static str, fields: &[(&'static str, Value)]) {
    let fields: Vec<(&str, &dyn fmt::Display)> = fields
        .iter()
        .map(|(key, value)| (*key, value as &dyn fmt::Display))
        .collect();
    if TO_STDOUT.load(std::sync::atomic::Ordering::Relaxed) {
        let now = Local::now().fixed_offset();
        let line = logline::format(now, std::process::id(), level, event, &fields);
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
        return;
    }
    if let Some(log) = LOG.get() {
        log.log(level, event, &fields);
    }
}

/// Makes the log in `data_dir` if it's not there (on Unix 0600, never
/// through a link) and has logline add to it from now on. An error means it
/// can't be written.
fn start(data_dir: &Path) -> std::io::Result<()> {
    let path = data_dir.join(FILE);
    adding().open(&path)?;
    let _ = LOG.set(logline::Logger::open_file(path));
    Ok(())
}

/// Starts the log in `data_dir`, saying so. An error means it can't be
/// written, which the app says on the screen.
pub fn open(data_dir: &Path, accounts: usize) -> std::io::Result<()> {
    start(data_dir)?;
    write(
        Level::Info,
        "start",
        &[
            ("version", Value::Word(crate::info::VERSION)),
            ("accounts", Value::Number(accounts as i64)),
        ],
    );
    Ok(())
}

/// Starts the log in `data_dir` without a word: for `tuimail mcp`, which an
/// agent starts for each session, so that its coming and going would be most
/// of the log. An error means it can't be written.
pub fn join(data_dir: &Path) -> std::io::Result<()> {
    start(data_dir)
}

/// Starts the log on stdout, saying so: for the session, whose pod's output is its log.
pub fn to_stdout(accounts: usize) {
    TO_STDOUT.store(true, std::sync::atomic::Ordering::Relaxed);
    write(
        Level::Info,
        "start",
        &[
            ("mode", Value::Word("session")),
            ("version", Value::Word(crate::info::VERSION)),
            ("accounts", Value::Number(accounts as i64)),
        ],
    );
}

/// Writes an event of static words, and its account if it has one: the session's own
/// (a PC connected, left), which have no error to sort.
pub fn note(text: &'static str, number: Option<usize>) {
    write(Level::Info, text, &Vec::from_iter(numbered(number)));
}

/// [`note`] at `warn`: something refused or left out.
pub fn warning(text: &'static str, number: Option<usize>) {
    write(Level::Warn, text, &Vec::from_iter(numbered(number)));
}

/// Writes what an account's archive pass did: messages kept, places noted, places gone.
pub fn counted(text: &'static str, number: usize, kept: usize, placed: usize, gone: usize) {
    let count = |n: usize| Value::Number(n as i64);
    write(
        Level::Info,
        text,
        &[
            ("account", count(number)),
            ("kept", count(kept)),
            ("placed", count(placed)),
            ("gone", count(gone)),
        ],
    );
}

pub fn stopped() {
    write(Level::Info, "stop", &[]);
}

/// Writes that `op` failed. Only `error`'s class is taken from it.
pub fn failed(op: &'static str, account: Option<usize>, error: &anyhow::Error) {
    write(
        Level::Error,
        "failed",
        &failure(op, account, class_of(error)),
    );
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
    let at = location.map(|at| {
        let place = Value::At(source_file(at.file()), at.line(), at.column());
        ("at", place)
    });
    write(Level::Error, event, &Vec::from_iter(at));
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
            write(Level::Error, "failed", &failure(op, self.account, class));
        }
    }

    pub fn back(&mut self) {
        if self.down.take().is_some() {
            write(
                Level::Info,
                "recovered",
                &Vec::from_iter(numbered(self.account)),
            );
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::{mail, tools};
    use anyhow::anyhow;
    use std::path::PathBuf;

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

    /// The line `write` would make for an error event, checked to read back
    /// as the standard line.
    fn rendered(event: &'static str, fields: &[(&'static str, Value)]) -> String {
        let fields: Vec<(&str, &dyn fmt::Display)> = fields
            .iter()
            .map(|(key, value)| (*key, value as &dyn fmt::Display))
            .collect();
        let now = Local::now().fixed_offset();
        let line = logline::format(now, 1, Level::Error, event, &fields);
        logline::parse(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
        line
    }

    /// A log's lines from their level on (`error failed op=...`), each
    /// checked to be the standard line.
    pub fn events(log: &str) -> Vec<&str> {
        events_of(&log.lines().collect::<Vec<_>>())
    }

    /// `error`'s text holds the private words, and its log line none.
    pub fn assert_kept_out(error: &anyhow::Error, class: Class) {
        let text = format!("{error:#}");
        assert!(
            PRIVATE.iter().any(|word| text.contains(word)),
            "the test's error has nothing private in it: {text}"
        );
        assert_eq!(class_of(error), class, "{text}");
        let line = rendered(
            "failed",
            &failure("checking for mail", Some(1), class_of(error)),
        );
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
    fn each_event_is_a_standard_line_with_its_level_and_fields() {
        if let Some(dir) = child_dir() {
            open(&dir, 2).unwrap();
            failed("logging in", Some(2), &anyhow!(Class::Smtp(535).with("x")));
            let mut link = Link::new(3);
            link.failed("checking for mail", &anyhow!(Class::Timeout.with("x")));
            link.back();
            warning("left out: the account isn't usable", Some(1));
            counted("archived", 1, 4, 5, 0);
            failed(
                "reading",
                None,
                &anyhow!(std::io::Error::from_raw_os_error(5)),
            );
            stopped();
            return;
        }
        let (run, log) =
            log_of_child("logfile::tests::each_event_is_a_standard_line_with_its_level_and_fields");
        assert!(run.status.success(), "{run:?}");
        let events = events(&log);
        let start = format!("info start version={} accounts=2", crate::info::VERSION);
        assert_eq!(events[0], start, "{log}");
        assert_eq!(
            events[1..events.len() - 2],
            [
                "error failed op=\"logging in\" account=2 class=smtp code=535",
                "error failed op=\"checking for mail\" account=3 class=timeout",
                "info recovered account=3",
                "warn left out: the account isn't usable account=1",
                "info archived account=1 kept=4 placed=5 gone=0",
            ],
            "{log}"
        );
        let io = events[events.len() - 2];
        assert!(
            io.starts_with("error failed op=reading class=io kind="),
            "{log}"
        );
        assert_eq!(events[events.len() - 1], "info stop", "{log}");
    }

    /// logline moves a full log to [`OLD_FILE`], which the data folder's
    /// check (`config::make_private_dir`) knows by name.
    #[test]
    fn a_full_log_rolls_and_the_new_one_starts_small() {
        if let Some(dir) = child_dir() {
            let path = dir.join(FILE);
            std::fs::write(&path, vec![b'x'; logline::LIMIT as usize + 1]).unwrap();
            open(&dir, 1).unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() < 200);
            let old = std::fs::metadata(dir.join(OLD_FILE)).unwrap();
            assert_eq!(old.len(), logline::LIMIT + 1);
            return;
        }
        let (run, log) =
            log_of_child("logfile::tests::a_full_log_rolls_and_the_new_one_starts_small");
        assert!(run.status.success(), "{run:?}");
        assert_eq!(events(&log).len(), 1, "{log}");
    }

    /// The session's log is its stdout: the same lines.
    #[test]
    fn the_session_logs_standard_lines_to_stdout() {
        if child_dir().is_some() {
            to_stdout(2);
            note("a PC connected", None);
            warning("refused a PC: its token isn't good", None);
            failed("making a snapshot", None, &anyhow!(private_words()));
            return;
        }
        let (run, _) = log_of_child("logfile::tests::the_session_logs_standard_lines_to_stdout");
        assert!(run.status.success(), "{run:?}");
        let out = String::from_utf8_lossy(&run.stdout);
        // The test harness's words share the lines; the log's start at its time.
        let lines: Vec<_> = out
            .lines()
            .filter(|line| line.contains(" pid="))
            .map(|line| &line[line.find(|c: char| c.is_ascii_digit()).unwrap()..])
            .collect();
        let events = events_of(&lines);
        let start = format!(
            "info start mode=session version={} accounts=2",
            crate::info::VERSION
        );
        assert_eq!(
            events,
            [
                start.as_str(),
                "info a PC connected",
                "warn refused a PC: its token isn't good",
                "error failed op=\"making a snapshot\" class=other",
            ],
            "{out}"
        );
        assert_nothing_private_in(&out);
    }

    fn events_of<'a>(lines: &[&'a str]) -> Vec<&'a str> {
        lines
            .iter()
            .map(|line| {
                logline::parse(line).unwrap_or_else(|e| panic!("{e}: {line}"));
                line.splitn(3, ' ').nth(2).unwrap()
            })
            .collect()
    }

    #[test]
    fn a_repeating_failure_is_written_once_until_it_changes_or_ends() {
        let line = rendered("failed", &failure("logging in", Some(2), Class::Timeout));
        assert_eq!(
            events(&line),
            ["error failed op=\"logging in\" account=2 class=timeout"]
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
        assert_eq!(events(&log), ["error failed op=x class=other"], "{log}");
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
    /// panic hook are the process's, and gives how it ended and its log. The
    /// child's folder goes once its log is read.
    pub fn log_of_child(name: &str) -> (std::process::Output, String) {
        let dir = folder();
        let run = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env(CHILD, &dir)
            .output();
        let log = std::fs::read_to_string(dir.join(FILE)).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        (run.unwrap(), log)
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
        let events: Vec<_> = events(&log)
            .iter()
            .map(|event| event.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(
            events,
            ["info start", "error contained-panic", "error crash"],
            "{log}"
        );
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
