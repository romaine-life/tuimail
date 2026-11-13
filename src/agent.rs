//! AI agents' way to the user's mail. `tuimail mcp` (`mcp.rs`), the MCP
//! server an agent starts, asks the running tuimail over a socket in the
//! data folder, and tuimail answers (`App::on_agent`): it reads from its
//! cache, and changes mail as the user's own keys do, on the screen, kept
//! as changes that can be undone (`z`, `Z`), and asking the user first
//! unless the settings say not to. Nothing is ever deleted for good and
//! nothing is sent: there's no call for it.
//!
//! tuimail listens only while the "AI agents" setting is on. The socket is
//! in the data folder (only the user's), is made only the user's, and a
//! connection is answered only if it comes from a program of the same
//! user. A request is one line of JSON, at most 64 KB, and so is its
//! answer (at most 4 MB). On Windows the socket is a named pipe of the
//! same name, made this user's alone, and each side checks the process at
//! the other end runs as this user (`windows`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::text;

/// The socket's name in the data folder; the demo's (`tuimail --demo`,
/// reached with `tuimail mcp --demo`), so agents can be tried on made-up
/// mail.
pub const SOCKET: &str = "agent.sock";
pub const DEMO_SOCKET: &str = "demo-agent.sock";
/// The most messages one call names.
pub const MAX_IDS: usize = 50;
/// The most messages a search returns.
pub const MAX_RESULTS: usize = 50;
/// How much of a message's text is returned, unless asked for less.
pub const TEXT_CHARS: usize = 8_000;
/// The most of a message's text that's returned.
pub const MAX_TEXT_CHARS: usize = 20_000;
/// The most messages agents change in an hour, and move to Trash.
pub const CHANGES_PER_HOUR: usize = 300;
pub const TRASH_PER_HOUR: usize = 100;
/// The most messages agents' reads download in an hour, and bytes: they're
/// the user's connection and bandwidth.
pub const DOWNLOADS_PER_HOUR: usize = 200;
pub const DOWNLOAD_BYTES_PER_HOUR: u64 = 512 << 20;
/// How long a change waits for the user's `y`.
pub const ASK_FOR: Duration = Duration::from_secs(5 * 60);

/// What `tuimail mcp` asks, for the program named `client` (as it calls
/// itself: shown to the user, never trusted).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub client: String,
    pub call: Call,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum Call {
    ListAccounts(NoArgs),
    ListFolders(FolderArgs),
    SearchMessages(SearchArgs),
    ReadMessage(ReadArgs),
    MarkRead(ReadFlagArgs),
    Star(StarArgs),
    SetTag(TagArgs),
    MoveMessages(MoveArgs),
    MarkSpam(SpamArgs),
    MoveToTrash(IdsArgs),
    ListChanges(ChangesArgs),
    UndoChange(UndoArgs),
}

impl Call {
    /// Whether it changes mail.
    pub fn changes(&self) -> bool {
        matches!(
            self,
            Call::MarkRead(_)
                | Call::Star(_)
                | Call::SetTag(_)
                | Call::MoveMessages(_)
                | Call::MarkSpam(_)
                | Call::MoveToTrash(_)
                | Call::UndoChange(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FolderArgs {
    /// The account, by its name or address (from list_accounts).
    pub account: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Only this account (its name or address). Default: every account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Only this folder: its `folder` from list_folders, or a role (inbox,
    /// archive, sent, drafts, junk, trash). Default: with words, every
    /// folder; without, the inbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Words that must all be in the sender, recipients, subject or the
    /// start of the text; `tag:work` asks for a tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Only unread messages.
    #[serde(default)]
    pub unread_only: bool,
    /// Only starred messages.
    #[serde(default)]
    pub starred_only: bool,
    /// At most this many, newest first (up to 50; default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    /// The message's id, from search_messages.
    pub id: String,
    /// At most this many characters of its text (default 8000, up to
    /// 20000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_chars: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadFlagArgs {
    /// Message ids from search_messages (at most 50).
    pub ids: Vec<String>,
    /// true: mark read; false: mark unread.
    pub read: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StarArgs {
    /// Message ids from search_messages (at most 50).
    pub ids: Vec<String>,
    /// true: star; false: take the star off.
    pub starred: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagArgs {
    /// Message ids from search_messages (at most 50).
    pub ids: Vec<String>,
    /// One word: letters, digits, - _ or . ("receipts", "travel").
    pub tag: String,
    /// true: put the tag on; false: take it off.
    pub on: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    /// Message ids from search_messages (at most 50).
    pub ids: Vec<String>,
    /// Where to: a `folder` from list_folders, or a role (inbox, archive).
    /// Trash is move_to_trash's, junk mark_spam's.
    pub folder: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpamArgs {
    /// Message ids from search_messages (at most 50).
    pub ids: Vec<String>,
    /// true: move to Junk; false: back from Junk to the inbox.
    pub spam: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdsArgs {
    /// Message ids from search_messages (at most 50).
    pub ids: Vec<String>,
}

/// A call without arguments: none others either (a unit variant would
/// take any).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangesArgs {
    /// At most this many, newest first (default 20, up to 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UndoArgs {
    /// The change's id, from a change's answer or list_changes.
    pub change: i64,
}

/// What tuimail answers: what was asked for, or why not.
pub type Answer = Result<serde_json::Value, String>;

/// What an agent is called on the screen: as it calls itself, cleaned.
pub fn shown_client(client: &str) -> String {
    let name = text::one_line(client, 40);
    if name.is_empty() {
        "An AI agent".into()
    } else {
        name
    }
}

/// A message's id for agents: its account, folder, UIDVALIDITY and UID,
/// so it means that message and no other (read back with [`unhandle`]).
pub fn handle(account: &str, folder: &str, uidvalidity: u32, uid: u32) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(format!("{account}\n{folder}\n{uidvalidity}\n{uid}"))
}

/// What a [`handle`] names: account id, folder, UIDVALIDITY, UID.
pub fn unhandle(id: &str) -> Option<(String, String, u32, u32)> {
    if id.len() > 2000 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(id.trim())
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let mut parts = text.split('\n');
    let (account, folder) = (parts.next()?, parts.next()?);
    let uidvalidity = parts.next()?.parse().ok()?;
    let uid = parts.next()?.parse().ok()?;
    if parts.next().is_some() || account.is_empty() || folder.is_empty() {
        return None;
    }
    Some((account.into(), folder.into(), uidvalidity, uid))
}

/// Text from mail for an agent (a body, a subject, a name, a preview):
/// links shown by their site only, so a sender's link isn't one to follow
/// (the user opens links, with `o`). Every one, glued to a word or however
/// long (`mail::find_any_url`).
pub fn hide_links(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((start, end)) = crate::mail::find_any_url(rest) {
        out.push_str(&rest[..start]);
        // The site is at the start: a link of megabytes costs no more.
        let link = crate::text::first_chars(&rest[start..end], 4096);
        let host = crate::app::link_host(link);
        out.push_str(&format!("[link: {host}]"));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Where the socket is. On Windows the path names the pipe (`windows::pipe_name`).
pub fn socket(data_dir: &Path, demo: bool) -> PathBuf {
    data_dir.join(if demo { DEMO_SOCKET } else { SOCKET })
}

/// The longest request read, and answer.
pub const MAX_REQUEST: usize = 64 << 10;
pub const MAX_ANSWER: usize = 4 << 20;
/// How long `tuimail mcp` waits for an answer: a little longer than a
/// change waits for the user.
const ANSWER_WITHIN: Duration = Duration::from_secs(5 * 60 + 30);

/// Why `tuimail mcp` gets no answer.
pub const NOT_LISTENING: &str = "tuimail isn't open, or its AI agents setting is off. \
    Ask the user to open tuimail and turn on AI agents in its settings (O).";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Ok(serde_json::Value),
    Error(String),
}

/// Answers one request on a connection already checked to be this user's.
async fn serve<S>(
    stream: S,
    tx: tokio::sync::mpsc::UnboundedSender<crate::app::AppEvent>,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let (read, mut write) = tokio::io::split(stream);
    let mut line = Vec::new();
    let mut reader = BufReader::new(read.take(MAX_REQUEST as u64));
    tokio::time::timeout(Duration::from_secs(10), reader.read_until(b'\n', &mut line)).await??;
    let answer = match serde_json::from_slice::<Request>(&line) {
        Ok(request) => {
            let (reply, answer) = tokio::sync::oneshot::channel();
            tx.send(crate::app::AppEvent::Agent { request, reply })?;
            let waited = tokio::time::timeout(ASK_FOR + Duration::from_secs(15), answer);
            // An agent that leaves stops waiting: dropping the answer
            // takes its question off the screen, so a `y` can't make
            // a change nobody waits for.
            let mut byte = [0u8; 1];
            tokio::select! {
                answer = waited => match answer {
                    Ok(Ok(answer)) => answer,
                    _ => Err("tuimail didn't answer in time".into()),
                },
                _ = reader.read(&mut byte) => return Ok(()),
            }
        }
        Err(_) => Err("that isn't a request tuimail understands".into()),
    };
    let wire = match answer {
        Ok(value) => Wire::Ok(value),
        Err(error) => Wire::Error(error),
    };
    let mut out = serde_json::to_vec(&wire)?;
    out.truncate(MAX_ANSWER);
    out.push(b'\n');
    // One that never reads doesn't hold its slot for good.
    tokio::time::timeout(Duration::from_secs(30), write.write_all(&out)).await??;
    Ok(())
}

/// Sends one request down a connection already checked to be a tuimail of
/// this user's, and reads its answer. An error is why tuimail gave no
/// answer: its text is the agent's, and its class the log's.
async fn exchange<S>(mut stream: S, request: &Request) -> anyhow::Result<Answer>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use crate::logfile::Class;
    use anyhow::Context;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let mut out = serde_json::to_vec(request)?;
    if out.len() >= MAX_REQUEST {
        return Err(Class::Agent("too-long")
            .with("the request is too long")
            .into());
    }
    out.push(b'\n');
    stream.write_all(&out).await.context(NOT_LISTENING)?;
    let mut line = Vec::new();
    let mut reader = BufReader::new(stream.take(MAX_ANSWER as u64 + 2));
    tokio::time::timeout(ANSWER_WITHIN, reader.read_until(b'\n', &mut line))
        .await
        .context("tuimail didn't answer in time")??;
    // Taken and closed without a word: a tuimail that stopped listening
    // as the agent arrived (agents turned off, or quitting).
    if line.is_empty() {
        return Err(Class::Agent("not-listening").with(NOT_LISTENING).into());
    }
    match serde_json::from_slice::<Wire>(&line) {
        Ok(Wire::Ok(value)) => Ok(Ok(value)),
        Ok(Wire::Error(error)) => Ok(Err(error)),
        Err(_) => Err(Class::Protocol.with("tuimail's answer didn't read").into()),
    }
}

#[cfg(not(any(unix, windows)))]
pub use other::{Listener, ask, listen};
#[cfg(unix)]
pub use unix::{Listener, ask, listen};
#[cfg(windows)]
pub use windows::{Listener, ask, listen};

/// Neither a Unix socket nor a named pipe: not yet.
#[cfg(not(any(unix, windows)))]
mod other {
    use std::path::Path;

    use anyhow::{Result, bail};
    use tokio::sync::mpsc::UnboundedSender;

    use super::*;
    use crate::app::AppEvent;
    use crate::logfile::Class;

    pub struct Listener;

    pub fn listen(_: &Path, _: UnboundedSender<AppEvent>) -> Result<Listener> {
        bail!("AI agents can't reach tuimail on this system yet")
    }

    pub async fn ask(_: &Path, _: &Request) -> Result<Answer> {
        Err(Class::Agent("not-listening")
            .with("tuimail mcp isn't available on this system yet")
            .into())
    }
}

#[cfg(unix)]
mod unix {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use anyhow::{Context, Result, bail};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::{Semaphore, mpsc::UnboundedSender};
    use tokio::task::JoinHandle;

    use super::*;
    use crate::app::AppEvent;
    use crate::logfile::Class;

    /// tuimail listening for agents; dropping it stops, ends every
    /// connection open, and removes the socket (if it's still this one).
    pub struct Listener {
        path: PathBuf,
        /// The socket's device and inode, as bound.
        made: (u64, u64),
        task: JoinHandle<()>,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // The connections are the task's: they end with it.
            self.task.abort();
            // Another tuimail may have put its own there since.
            if identity(&self.path) == Some(self.made) {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    /// A socket's device and inode, if it's a socket of this user's alone
    /// (not a link, one name).
    fn identity(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let meta = std::fs::symlink_metadata(path).ok()?;
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        (meta.file_type().is_socket() && meta.nlink() == 1 && meta.uid() == me)
            .then(|| (meta.dev(), meta.ino()))
    }

    /// Whether a tuimail answers at `path`, asked for at most a moment: a
    /// listener that never accepts must not hold up the screen.
    fn answers(path: &Path) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(std::os::unix::net::UnixStream::connect(path).is_ok());
        });
        // No answer in time: something holds it, so it isn't taken.
        rx.recv_timeout(std::time::Duration::from_millis(500))
            .unwrap_or(true)
    }

    /// Listens at `path`; each request reaches the app as
    /// [`AppEvent::Agent`].
    pub fn listen(path: &Path, tx: UnboundedSender<AppEvent>) -> Result<Listener> {
        // A socket left by a tuimail that's gone is removed; one another
        // tuimail answers on is left to it. Anything else there (a link, a
        // socket not this user's) is never asked: it goes.
        if std::fs::symlink_metadata(path).is_ok() {
            if identity(path).is_some() && answers(path) {
                bail!("another tuimail is answering agents");
            }
            std::fs::remove_file(path).context("cannot remove the old agent socket")?;
        }
        // Socket paths are short: 104 bytes on macOS, 108 on Linux.
        if path.as_os_str().len() >= 100 {
            bail!("the data folder's path is too long for agents to reach tuimail through it");
        }
        let listener = UnixListener::bind(path).context("cannot listen for agents")?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        let made = identity(path).context("cannot listen for agents")?;
        // A few at a time: one agent's calls come one by one.
        let slots = Arc::new(Semaphore::new(8));
        let task = tokio::spawn(async move {
            // Owned here, so stopping the listener stops them too.
            let mut connections = tokio::task::JoinSet::new();
            loop {
                while connections.try_join_next().is_some() {}
                let stream = match listener.accept().await {
                    Ok((stream, _)) => stream,
                    // Out of files, say: not again at once.
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        continue;
                    }
                };
                // SAFETY: geteuid has no preconditions.
                let me = unsafe { libc::geteuid() };
                if !stream.peer_cred().is_ok_and(|c| c.uid() == me) {
                    continue;
                }
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let tx = tx.clone();
                connections.spawn(async move {
                    let _ = serve(stream, tx).await;
                    drop(slot);
                });
            }
        });
        Ok(Listener {
            path: path.to_path_buf(),
            made,
            task,
        })
    }

    /// Asks the tuimail listening at `path` (`tuimail mcp`'s side): only a
    /// socket of this user's, not through a link, and only a listener of
    /// this user's, since what it answers goes to the agent as the user's
    /// mail.
    pub async fn ask(path: &Path, request: &Request) -> Result<Answer> {
        if identity(path).is_none() {
            return Err(Class::Agent("not-listening").with(NOT_LISTENING).into());
        }
        let stream = UnixStream::connect(path).await.context(NOT_LISTENING)?;
        // SAFETY: geteuid has no preconditions.
        let me = unsafe { libc::geteuid() };
        if !stream.peer_cred().is_ok_and(|c| c.uid() == me) {
            return Err(Class::Agent("not-yours")
                .with("what answers at tuimail's socket isn't yours: not asked")
                .into());
        }
        exchange(stream, request).await
    }
}

/// Windows: a named pipe in place of the socket. Its name is the socket's
/// path, so each data folder (and the demo) has its own; it's made with an
/// access list naming this user alone, and each side checks the process at
/// the other end runs as this user before a word is exchanged, as the Unix
/// side checks the peer's uid.
#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::sync::Arc;

    use anyhow::{Context, Result, bail};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use tokio::sync::{Semaphore, mpsc::UnboundedSender};
    use tokio::task::JoinHandle;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, HANDLE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Pipes::{
        GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::*;
    use crate::app::AppEvent;
    use crate::logfile::Class;

    /// The longest pipe name Windows takes is 256 characters.
    const MAX_NAME: usize = 250;

    /// The pipe named by the socket's path: the path's bytes in hex after a
    /// fixed prefix, so no two data folders share one and no character of
    /// the path can shape the name.
    pub fn pipe_name(path: &Path) -> Result<String> {
        let mut name = String::from(r"\\.\pipe\tuimail-");
        for byte in path.as_os_str().as_encoded_bytes() {
            name.push_str(&format!("{byte:02x}"));
        }
        if name.len() > MAX_NAME {
            bail!("the data folder's path is too long for agents to reach tuimail through it");
        }
        Ok(name)
    }

    /// A handle closed when dropped.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle this opened, closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// The SID of the user a process runs as, as bytes; `None` for a
    /// process that can't be read (another user's, or gone).
    fn user_of(process: HANDLE) -> Option<Vec<u8>> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: a valid process handle; the token is closed below.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let token = Handle(token);
        let mut needed = 0u32;
        // SAFETY: asking the size; a failure is expected here.
        unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0u8; needed as usize];
        // SAFETY: the buffer is as large as asked.
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr() as *mut c_void,
                needed,
                &mut needed,
            )
        } == 0
        {
            return None;
        }
        // SAFETY: a TOKEN_USER was written at the start of the buffer, and
        // its SID points into the same buffer.
        let sid = unsafe { (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid };
        if sid.is_null() {
            return None;
        }
        // SAFETY: a SID the system wrote.
        let length = unsafe { GetLengthSid(sid) } as usize;
        // SAFETY: `length` bytes of SID at `sid`, inside the buffer.
        Some(unsafe { std::slice::from_raw_parts(sid as *const u8, length) }.to_vec())
    }

    /// This process's user.
    fn me() -> Result<Vec<u8>> {
        // SAFETY: the current process's pseudo-handle needs no closing.
        user_of(unsafe { GetCurrentProcess() }).context("cannot read who you are")
    }

    /// Whether the process `pid` runs as this user: the other end of a
    /// pipe, checked before anything is read or answered.
    fn same_user(pid: u32, me: &[u8]) -> bool {
        if pid == 0 {
            return false;
        }
        // SAFETY: a limited query handle, closed by `Handle`.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return false;
        }
        user_of(Handle(process).0).is_some_and(|user| user == me)
    }

    /// A security descriptor naming one user, freed when dropped.
    struct Descriptor(PSECURITY_DESCRIPTOR);

    // SAFETY: the descriptor is a block this process allocated, read by
    // the system only while an instance is made, and freed once.
    unsafe impl Send for Descriptor {}

    impl Drop for Descriptor {
        fn drop(&mut self) {
            // SAFETY: memory LocalAlloc'd by the conversion, freed once.
            unsafe { LocalFree(self.0) };
        }
    }

    /// Access for this user alone (`D:P(A;;GA;;;<sid>)`: a protected list
    /// with one entry, generic all for the user, inheriting nothing).
    fn only_me(me: &[u8]) -> Result<Descriptor> {
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: a SID of this process's user, as bytes it wrote.
        if unsafe { ConvertSidToStringSidW(me.as_ptr() as *mut c_void, &mut text) } == 0 {
            bail!("cannot name who you are");
        }
        // SAFETY: a NUL-ended UTF-16 string the system allocated.
        let sid = unsafe {
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
        };
        // SAFETY: allocated by the conversion above.
        unsafe { LocalFree(text as *mut c_void) };
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a NUL-ended string; the descriptor is freed by `Descriptor`.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            bail!("cannot make the agents' pipe yours alone");
        }
        Ok(Descriptor(descriptor))
    }

    /// One more instance of the pipe, for the next agent to connect to.
    fn instance(
        name: &str,
        descriptor: &Descriptor,
        first: bool,
    ) -> std::io::Result<NamedPipeServer> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: a valid SECURITY_ATTRIBUTES for the call's duration.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(
                    name,
                    &mut attributes as *mut SECURITY_ATTRIBUTES as *mut c_void,
                )
        }
    }

    /// tuimail listening for agents; dropping it stops, ends every
    /// connection open, and closes the pipe.
    pub struct Listener {
        task: JoinHandle<()>,
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            // The pipe's instances and connections are the task's: they
            // close with it.
            self.task.abort();
        }
    }

    /// Listens at the pipe named by `path`; each request reaches the app
    /// as [`AppEvent::Agent`].
    pub fn listen(path: &Path, tx: UnboundedSender<AppEvent>) -> Result<Listener> {
        let name = pipe_name(path)?;
        let me = me()?;
        let descriptor = only_me(&me)?;
        // The first instance: a name already held, by another tuimail or
        // anything else, is never taken.
        let first = match instance(&name, &descriptor, true) {
            Ok(server) => server,
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                bail!("another tuimail is answering agents")
            }
            Err(e) => return Err(e).context("cannot listen for agents"),
        };
        // A few at a time: one agent's calls come one by one.
        let slots = Arc::new(Semaphore::new(8));
        let task = tokio::spawn(async move {
            // Owned here, so stopping the listener stops them too.
            let mut connections = tokio::task::JoinSet::new();
            let mut server = first;
            loop {
                while connections.try_join_next().is_some() {}
                if server.connect().await.is_err() {
                    // Out of handles, say: not again at once.
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
                let connected = server;
                server = loop {
                    match instance(&name, &descriptor, false) {
                        Ok(next) => break next,
                        Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                };
                let mut pid = 0u32;
                // SAFETY: a connected pipe handle this task owns.
                unsafe {
                    GetNamedPipeClientProcessId(connected.as_raw_handle() as HANDLE, &mut pid)
                };
                if !same_user(pid, &me) {
                    continue;
                }
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let tx = tx.clone();
                connections.spawn(async move {
                    let _ = serve(connected, tx).await;
                    drop(slot);
                });
            }
        });
        Ok(Listener { task })
    }

    /// Asks the tuimail listening at the pipe named by `path` (`tuimail
    /// mcp`'s side): only one answered by a process of this user's, since
    /// what it answers goes to the agent as the user's mail. The client
    /// handle identifies this process to the server, and no more (tokio's
    /// default), so a pipe that isn't tuimail's can't act as the user.
    pub async fn ask(path: &Path, request: &Request) -> Result<Answer> {
        let name = pipe_name(path)?;
        let me = me()?;
        let mut tries = 0;
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => break client,
                // Every instance is taken for the moment: a few more tries.
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && tries < 20 => {
                    tries += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
                    return Err(Class::Agent("not-listening").with(NOT_LISTENING).into());
                }
                Err(e) => return Err(e).context(NOT_LISTENING),
            }
        };
        let mut pid = 0u32;
        // SAFETY: a connected pipe handle this function owns.
        unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as HANDLE, &mut pid) };
        if !same_user(pid, &me) {
            return Err(Class::Agent("not-yours")
                .with("what answers at tuimail's pipe isn't yours: not asked")
                .into());
        }
        exchange(client, request).await
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_pipe_is_named_by_the_path_and_a_long_one_is_refused() {
            let name = pipe_name(Path::new(r"C:\x\agent.sock")).unwrap();
            assert!(name.starts_with(r"\\.\pipe\tuimail-"));
            assert_eq!(
                &name[r"\\.\pipe\tuimail-".len()..],
                "433a5c785c6167656e742e736f636b"
            );
            assert_ne!(name, pipe_name(Path::new(r"C:\x\demo-agent.sock")).unwrap());
            let long = format!(r"C:\{}\agent.sock", "a".repeat(200));
            assert!(pipe_name(Path::new(&long)).is_err());
        }

        #[test]
        fn this_process_is_this_user_and_no_process_is_nobody() {
            let me = me().unwrap();
            assert!(same_user(std::process::id(), &me));
            assert!(!same_user(0, &me));
        }

        #[tokio::test]
        async fn asking_where_nothing_listens_says_so() {
            let dir = crate::tools::private_temp_dir().unwrap();
            let socket = super::super::socket(&dir, false);
            let request = Request {
                client: "test".into(),
                call: Call::ListAccounts(NoArgs {}),
            };
            assert_eq!(
                ask(&socket, &request).await.unwrap_err().to_string(),
                NOT_LISTENING
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handle_names_one_message_and_nothing_else_reads_as_one() {
        let id = handle("me@example.org", "Archive/2026", 7, 42);
        assert_eq!(
            unhandle(&id),
            Some(("me@example.org".into(), "Archive/2026".into(), 7, 42))
        );
        for bad in ["", "x", &"A".repeat(3000), "bWVAZXhhbXBsZS5vcmcKSU5CT1gKNw"] {
            assert_eq!(unhandle(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_call_takes_only_its_own_arguments() {
        let call = |json: &str| serde_json::from_str::<Call>(json);
        assert!(call(r#"{"tool":"list_accounts"}"#).is_ok());
        assert!(call(r#"{"tool":"list_accounts","delete":true}"#).is_err());
        assert!(call(r#"{"tool":"move_to_trash","ids":[],"forever":true}"#).is_err());
    }

    #[test]
    fn links_show_only_their_site() {
        assert_eq!(
            hide_links("Reset it: https://evil.example/r?t=1 or mailto:x@y.z now"),
            "Reset it: [link: evil.example] or [link: y.z] now"
        );
        assert_eq!(hide_links("no links"), "no links");
        // Glued to a word, or longer than the reader numbers: hidden too.
        assert_eq!(
            hide_links("Clickhttps://evil.example/r?t=2 now"),
            "Click[link: evil.example] now"
        );
        let long = format!("https://evil.example/l?p={}", "a".repeat(5000));
        assert_eq!(
            hide_links(&format!("Long: {long}.")),
            "Long: [link: evil.example]."
        );
    }

    #[test]
    fn calls_read_as_json_and_nothing_unknown_does() {
        let request: Request = serde_json::from_str(
            r#"{"client":"Claude Code","call":{"tool":"move_to_trash","ids":["a"]}}"#,
        )
        .unwrap();
        assert_eq!(
            request.call,
            Call::MoveToTrash(IdsArgs {
                ids: vec!["a".into()]
            })
        );
        assert!(request.call.changes());
        for bad in [
            r#"{"client":"x","call":{"tool":"delete","ids":["a"]}}"#,
            r#"{"client":"x","call":{"tool":"move_to_trash","ids":["a"],"forever":true}}"#,
            r#"{"client":"x","call":{"tool":"list_accounts"},"admin":true}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_nameless_agent_is_still_named() {
        assert_eq!(shown_client(""), "An AI agent");
        assert_eq!(shown_client("Claude\u{1b}[31m Code"), "Claude[31m Code");
    }
}
