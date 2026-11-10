//! fzt, the home PCs' fuzzy menu, loaded in-process from the library its
//! managed install keeps (`fzt.dll`, fzt-terminal's `cmd/fztlib`): a menu
//! session that takes key presses and answers with what to draw, as data.
//! Every call passes and returns JSON. The command palette (`palette.rs`)
//! is its one user.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::logfile::Class;

type OpenFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_char;
type SessionFn = unsafe extern "C" fn(c_int, *const c_char) -> *mut c_char;
type ResizeFn = unsafe extern "C" fn(c_int, c_int, c_int) -> *mut c_char;
type CloseFn = unsafe extern "C" fn(c_int);
type FreeFn = unsafe extern "C" fn(*mut c_char);

struct Api {
    /// Keeps the functions below loaded.
    _library: libloading::Library,
    open: OpenFn,
    key: SessionFn,
    resize: ResizeFn,
    close: CloseFn,
    free: FreeFn,
}

/// Where home-sync installs fzt's library: the managed install's `current`
/// folder, which moves to each new release.
pub fn library_path() -> Result<PathBuf> {
    if !cfg!(windows) {
        bail!(
            Class::Palette("not-installed")
                .with("fzt is installed by home-sync, on the Windows home PCs only")
        );
    }
    let root = std::env::var_os("ProgramFiles").context("ProgramFiles isn't set")?;
    Ok(PathBuf::from(root).join(r"at-home\fzt\current\fzt.dll"))
}

/// The library, loaded the first time it's there: until then each opening
/// of the palette tries again, so fzt installed later works without a
/// restart.
fn api() -> Result<&'static Api> {
    static API: OnceLock<Api> = OnceLock::new();
    if let Some(api) = API.get() {
        return Ok(api);
    }
    let api = load()?;
    Ok(API.get_or_init(|| api))
}

/// Tests never load the installed library, except the one that's for it.
#[cfg(test)]
static LOAD_IN_TESTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn load() -> Result<Api> {
    #[cfg(test)]
    if !LOAD_IN_TESTS.load(std::sync::atomic::Ordering::Relaxed) {
        bail!(Class::Palette("not-installed").with("fzt isn't loaded in tests"));
    }
    let path = library_path()?;
    if !path.is_file() {
        bail!(Class::Palette("not-installed").with(format!(
            "fzt isn't installed at {} (home-sync installs it)",
            path.display()
        )));
    }
    // SAFETY: fzt's library has no initialization side effects, and each
    // symbol is cast to the signature cmd/fztlib exports it with.
    unsafe {
        let library = libloading::Library::new(&path).map_err(|e| {
            Class::Palette("not-loaded").with(format!("couldn't load {}: {e}", path.display()))
        })?;
        let get = |name: &[u8]| {
            Class::Palette("not-loaded").with(format!(
                "{} has no {}",
                path.display(),
                String::from_utf8_lossy(&name[..name.len() - 1])
            ))
        };
        Ok(Api {
            open: *library
                .get::<OpenFn>(b"fzt_open\0")
                .map_err(|_| get(b"fzt_open\0"))?,
            key: *library
                .get::<SessionFn>(b"fzt_key\0")
                .map_err(|_| get(b"fzt_key\0"))?,
            resize: *library
                .get::<ResizeFn>(b"fzt_resize\0")
                .map_err(|_| get(b"fzt_resize\0"))?,
            close: *library
                .get::<CloseFn>(b"fzt_close\0")
                .map_err(|_| get(b"fzt_close\0"))?,
            free: *library
                .get::<FreeFn>(b"fzt_free\0")
                .map_err(|_| get(b"fzt_free\0"))?,
            _library: library,
        })
    }
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    session: c_int,
    frame: Option<Frame>,
    #[serde(default)]
    error: String,
}

/// Reads and frees an answer from the library.
fn answer(api: &Api, out: *mut c_char) -> Result<Response> {
    ensure!(!out.is_null(), "fzt answered nothing");
    // SAFETY: the library returns a NUL-terminated string it allocated,
    // freed with `free` once read.
    let text = unsafe { CStr::from_ptr(out) }
        .to_string_lossy()
        .into_owned();
    unsafe { (api.free)(out) };
    let response: Response =
        serde_json::from_str(&text).context("fzt's answer couldn't be read")?;
    if !response.error.is_empty() {
        bail!(Class::Palette("refused").with(format!("fzt: {}", response.error)));
    }
    Ok(response)
}

fn frame(response: Response) -> Result<Frame> {
    response.frame.context("fzt answered without a frame")
}

fn json<T: Serialize>(value: &T) -> Result<CString> {
    Ok(CString::new(serde_json::to_string(value)?)?)
}

/// A session's options.
#[derive(Serialize)]
pub struct Options {
    pub title: String,
    pub width: u16,
    pub height: u16,
    /// The app's name in fzt's `:` folder.
    pub frontend: String,
    /// The app's own commands, first in fzt's `:` folder.
    pub commands: Vec<HostCommand>,
}

/// A command in fzt's `:` folder: picking it answers `command` with its
/// action.
#[derive(Serialize)]
pub struct HostCommand {
    pub name: String,
    pub description: String,
    pub action: String,
}

/// A key press: a named key, or `char` with the character.
#[derive(Serialize)]
pub struct Key {
    pub key: &'static str,
    pub char: String,
    pub shift: bool,
}

/// What to draw, and what was picked once it is.
#[derive(Deserialize, Debug, Default, Clone)]
pub struct Frame {
    #[serde(default)]
    pub rows: Vec<Row>,
    #[serde(default)]
    pub prompt: PromptState,
    pub result: Option<Pick>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub is_folder: bool,
    #[serde(default)]
    pub is_selected: bool,
    /// Which characters of the name the search matched (by character).
    #[serde(default)]
    pub name_match_indices: Option<Vec<usize>>,
    #[serde(default)]
    pub desc_match_indices: Option<Vec<usize>>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PromptState {
    /// The folders the search is in, outermost first.
    #[serde(default)]
    pub scope_path: Option<Vec<String>>,
    #[serde(default)]
    pub query: String,
    /// What Tab would complete the query with.
    #[serde(default)]
    pub ghost: String,
    /// Shown while nothing is typed.
    #[serde(default)]
    pub hint: String,
}

/// What was picked: `select` (an entry, with its action), `command` (one of
/// fzt's own `:` commands) or `cancel`.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub kind: String,
    #[serde(default)]
    pub action: String,
}

/// An open menu, closed when dropped.
#[derive(Debug)]
pub struct Session {
    id: c_int,
}

impl Session {
    /// Opens a menu in fzt's YAML tree format.
    pub fn open(menu: &str, options: &Options) -> Result<(Self, Frame)> {
        let api = api()?;
        let menu = CString::new(menu)?;
        let options = json(options)?;
        // SAFETY: both arguments are NUL-terminated strings that outlive the call.
        let response = answer(api, unsafe { (api.open)(menu.as_ptr(), options.as_ptr()) })?;
        let session = Self {
            id: response.session,
        };
        Ok((session, frame(response)?))
    }

    pub fn key(&self, key: &Key) -> Result<Frame> {
        let api = api()?;
        let key = json(key)?;
        // SAFETY: the argument is a NUL-terminated string that outlives the call.
        frame(answer(api, unsafe { (api.key)(self.id, key.as_ptr()) })?)
    }

    /// Lays the menu out again for a new size.
    pub fn resize(&self, width: u16, height: u16) -> Result<Frame> {
        let api = api()?;
        // SAFETY: plain integers.
        frame(answer(api, unsafe {
            (api.resize)(self.id, c_int::from(width), c_int::from(height))
        })?)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(api) = api() {
            // SAFETY: a plain integer; closing an unknown session does nothing.
            unsafe { (api.close)(self.id) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_reads_as_fztlib_writes_it() {
        let text = r#"{"session":3,"frame":{"rows":[{"name":"reply","description":"r: reply","depth":1,"isFolder":false,"isSelected":true,"isTopMatch":true,"nameMatchIndices":[0,1],"descMatchIndices":null,"tags":null,"matchedTags":null}],"prompt":{"mode":"search","icon":"","scopePath":["message"],"query":"re","cursor":2,"ghost":"ply","hint":"","tagTerms":null},"ui":{"title":"tuimail","titlePos":"","label":"","border":false,"treeOffset":0,"totalVisible":1},"result":{"kind":"select","action":"reply","name":"reply"}}}"#;
        let response: Response = serde_json::from_str(text).unwrap();
        assert_eq!(response.session, 3);
        let frame = frame(response).unwrap();
        assert_eq!(frame.rows[0].name, "reply");
        assert!(frame.rows[0].is_selected);
        assert_eq!(frame.rows[0].name_match_indices, Some(vec![0, 1]));
        assert_eq!(frame.prompt.scope_path, Some(vec!["message".into()]));
        assert_eq!(frame.prompt.ghost, "ply");
        let pick = frame.result.unwrap();
        assert_eq!(
            (pick.kind.as_str(), pick.action.as_str()),
            ("select", "reply")
        );
    }

    /// The installed library, run for real: needs home-sync's fzt, so only
    /// by hand (`cargo test -- --ignored installed_fzt`).
    #[test]
    #[ignore]
    fn installed_fzt_searches_and_picks() {
        LOAD_IN_TESTS.store(true, std::sync::atomic::Ordering::Relaxed);
        let menu = "- name: \"reply\"\n  description: \"r\"\n  action: \"reply\"\n- name: \"forward\"\n  description: \"f\"\n  action: \"forward\"\n";
        let options = Options {
            title: "tuimail".into(),
            width: 60,
            height: 10,
            frontend: "tuimail".into(),
            commands: vec![HostCommand {
                name: "info".into(),
                description: "where this build came from".into(),
                action: "info".into(),
            }],
        };
        let (session, frame) = Session::open(menu, &options).unwrap();
        let names: Vec<_> = frame.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["reply", "forward", ":"]);
        let char = |c: char| Key {
            key: "char",
            char: c.to_string(),
            shift: false,
        };
        for c in "forw".chars() {
            session.key(&char(c)).unwrap();
        }
        let frame = session
            .key(&Key {
                key: "enter",
                char: String::new(),
                shift: false,
            })
            .unwrap();
        let pick = frame.result.unwrap();
        assert_eq!(
            (pick.kind.as_str(), pick.action.as_str()),
            ("select", "forward")
        );
        // Info is in fzt's `:` folder, found by its name.
        let (session, _) = Session::open(menu, &options).unwrap();
        for c in "info".chars() {
            session.key(&char(c)).unwrap();
        }
        let enter = Key {
            key: "enter",
            char: String::new(),
            shift: false,
        };
        let pick = session.key(&enter).unwrap().result.unwrap();
        assert_eq!(
            (pick.kind.as_str(), pick.action.as_str()),
            ("command", "info")
        );
        assert!(!session.resize(80, 20).unwrap().rows.is_empty());
    }
}
