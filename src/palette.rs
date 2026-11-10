//! The command palette: `/` (`Ctrl-p` while writing) opens fzt's search
//! over every command tuimail has, and runs the one picked.
//!
//! [`COMMANDS`] is the one list of what tuimail's keys do: the palette's
//! menu and the `?` key list are both made from it. A command runs by
//! pressing its keys where you are, so a key and its command can't do
//! different things; where those keys would do something else or nothing,
//! the command says why it can't run instead.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Serialize;

use crate::app::{App, Focus, Row};
use crate::composer::Field;
use crate::fzt;

/// Where a command's keys do what it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// Anywhere but the composer.
    Main,
    /// In the sidebar, the list or the message, whichever has the keys.
    Pane,
    /// On a message: in the list or the message, with one there.
    Message,
    /// In the list.
    List,
    /// Reading a message.
    Reader,
    /// On an account in the sidebar.
    Account,
    /// On an account or one of its folders in the sidebar.
    AccountOrFolder,
    /// Writing a message.
    Writing,
    /// Writing, on the From line.
    From,
    /// Writing, on the Files line.
    Files,
    /// Anywhere: the palette's own.
    Anywhere,
}

/// What picking a command does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    /// Presses these keys.
    Keys(&'static [(KeyCode, KeyModifiers)]),
    /// Opens the search box.
    Search,
    /// Shows where this build came from.
    Info,
    /// Opens the palette: listed with the keys, not in the palette.
    Palette,
}

pub struct Command {
    /// What it's found by, and its action in the menu: each is different.
    pub name: &'static str,
    /// The folder of the menu it's in.
    pub group: &'static str,
    /// Its keys as the `?` list shows them; empty for the palette's own.
    pub keys: &'static str,
    pub what: &'static str,
    pub at: Where,
    pub run: Run,
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

const fn key(c: char) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(c), NONE)
}

const fn ctrl(c: char) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(c), CTRL)
}

const fn command(
    group: &'static str,
    name: &'static str,
    keys: &'static str,
    what: &'static str,
    at: Where,
    run: Run,
) -> Command {
    Command {
        name,
        group,
        keys,
        what,
        at,
        run,
    }
}

const MAIL: &str = "mail";
const READING: &str = "reading";
const MOVING: &str = "moving around";
const WRITING: &str = "writing";
const ACCOUNTS: &str = "accounts";
const TUIMAIL: &str = "tuimail";

/// The palette's key outside the composer.
pub const KEY: KeyCode = KeyCode::Char('/');

/// Every command, in the order the `?` list shows them.
pub const COMMANDS: &[Command] = &[
    command(
        MAIL,
        "search",
        "",
        "search here and the server's folder; tag:work searches a tag",
        Where::Main,
        Run::Search,
    ),
    command(
        MAIL,
        "write a message",
        "c",
        "write a new message",
        Where::Main,
        Run::Keys(&[key('c')]),
    ),
    command(
        MAIL,
        "reply",
        "r",
        "reply to the sender",
        Where::Message,
        Run::Keys(&[key('r')]),
    ),
    command(
        MAIL,
        "reply to all",
        "a",
        "reply to everyone on the message",
        Where::Message,
        Run::Keys(&[key('a')]),
    ),
    command(
        MAIL,
        "forward",
        "f",
        "forward the message",
        Where::Message,
        Run::Keys(&[key('f')]),
    ),
    command(
        MAIL,
        "delete",
        "d",
        "to Trash; in Trash, for good",
        Where::Message,
        Run::Keys(&[key('d')]),
    ),
    command(
        MAIL,
        "archive",
        "e",
        "move to Archive",
        Where::Message,
        Run::Keys(&[key('e')]),
    ),
    command(
        MAIL,
        "junk",
        "!",
        "move to Junk",
        Where::Message,
        Run::Keys(&[key('!')]),
    ),
    command(
        MAIL,
        "move to a folder",
        "m",
        "pick the folder to move to",
        Where::Message,
        Run::Keys(&[key('m')]),
    ),
    command(
        MAIL,
        "tag",
        "t",
        "put a tag on or take it off",
        Where::Message,
        Run::Keys(&[key('t')]),
    ),
    command(
        MAIL,
        "star",
        "s",
        "star or unstar",
        Where::Message,
        Run::Keys(&[key('s')]),
    ),
    command(
        MAIL,
        "read or unread",
        "u",
        "mark read or unread",
        Where::Message,
        Run::Keys(&[key('u')]),
    ),
    command(
        MAIL,
        "mark",
        "Space",
        "mark a message; actions take the marked",
        Where::List,
        Run::Keys(&[key(' ')]),
    ),
    command(
        MAIL,
        "mark all",
        "*",
        "mark every message, or none",
        Where::List,
        Run::Keys(&[key('*')]),
    ),
    command(
        MAIL,
        "undo",
        "z",
        "undo the last change",
        Where::Main,
        Run::Keys(&[key('z')]),
    ),
    command(
        MAIL,
        "changes",
        "Z",
        "the changes made, to undo one",
        Where::Main,
        Run::Keys(&[key('Z')]),
    ),
    command(
        MAIL,
        "check for mail",
        "R",
        "check every account now",
        Where::Main,
        Run::Keys(&[key('R')]),
    ),
    command(
        READING,
        "next message",
        "J  n",
        "the next message in the list",
        Where::Reader,
        Run::Keys(&[key('J')]),
    ),
    command(
        READING,
        "previous message",
        "K  p",
        "the previous message in the list",
        Where::Reader,
        Run::Keys(&[key('K')]),
    ),
    command(
        READING,
        "links",
        "o",
        "the message's links: open, or y copy",
        Where::Reader,
        Run::Keys(&[key('o')]),
    ),
    command(
        READING,
        "attachments",
        "A  w",
        "its files: open, or s save",
        Where::Reader,
        Run::Keys(&[key('A')]),
    ),
    command(
        READING,
        "html or text",
        "H",
        "show its HTML or its plain text",
        Where::Reader,
        Run::Keys(&[key('H')]),
    ),
    command(
        READING,
        "unsubscribe",
        "U",
        "unsubscribe from the mailing list",
        Where::Reader,
        Run::Keys(&[key('U')]),
    ),
    command(
        MOVING,
        "down",
        "j  ↓",
        "the next row; a line down in a message",
        Where::Pane,
        Run::Keys(&[(KeyCode::Down, NONE)]),
    ),
    command(
        MOVING,
        "up",
        "k  ↑",
        "the row above; a line up in a message",
        Where::Pane,
        Run::Keys(&[(KeyCode::Up, NONE)]),
    ),
    command(
        MOVING,
        "open",
        "Enter  l  →",
        "open the folder, account or message",
        Where::Pane,
        Run::Keys(&[(KeyCode::Enter, NONE)]),
    ),
    command(
        MOVING,
        "back",
        "Esc  h  ←",
        "end the search, unmark, or go back",
        Where::Pane,
        Run::Keys(&[(KeyCode::Esc, NONE)]),
    ),
    command(
        MOVING,
        "next pane",
        "Tab",
        "folders → messages → message",
        Where::Pane,
        Run::Keys(&[(KeyCode::Tab, NONE)]),
    ),
    command(
        MOVING,
        "previous pane",
        "Shift-Tab",
        "message → messages → folders",
        Where::Pane,
        Run::Keys(&[(KeyCode::BackTab, SHIFT)]),
    ),
    command(
        MOVING,
        "first",
        "gg  Home",
        "the first row, or the message's top",
        Where::Pane,
        Run::Keys(&[key('g'), key('g')]),
    ),
    command(
        MOVING,
        "last",
        "G  End",
        "the last row, or the message's end",
        Where::Pane,
        Run::Keys(&[key('G')]),
    ),
    command(
        MOVING,
        "half a page down",
        "Ctrl-d",
        "half a page down",
        Where::Pane,
        Run::Keys(&[ctrl('d')]),
    ),
    command(
        MOVING,
        "half a page up",
        "Ctrl-u",
        "half a page up",
        Where::Pane,
        Run::Keys(&[ctrl('u')]),
    ),
    command(
        MOVING,
        "page down",
        "PgDn  Space",
        "a page down (Space: reading)",
        Where::Message,
        Run::Keys(&[(KeyCode::PageDown, NONE)]),
    ),
    command(
        MOVING,
        "page up",
        "PgUp  b",
        "a page up (b: reading)",
        Where::Message,
        Run::Keys(&[(KeyCode::PageUp, NONE)]),
    ),
    command(
        WRITING,
        "send",
        "Ctrl-s",
        "send the message",
        Where::Writing,
        Run::Keys(&[ctrl('s')]),
    ),
    command(
        WRITING,
        "save draft",
        "Ctrl-d",
        "save in Drafts and close",
        Where::Writing,
        Run::Keys(&[ctrl('d')]),
    ),
    command(
        WRITING,
        "attach a file",
        "Ctrl-o",
        "attach a file; or drop it on the window",
        Where::Writing,
        Run::Keys(&[ctrl('o')]),
    ),
    command(
        WRITING,
        "editor",
        "Ctrl-e",
        "write the text in your editor",
        Where::Writing,
        Run::Keys(&[ctrl('e')]),
    ),
    command(
        WRITING,
        "next field",
        "Tab  ↓",
        "the next field; Tab takes a suggestion",
        Where::Writing,
        Run::Keys(&[(KeyCode::Tab, NONE)]),
    ),
    command(
        WRITING,
        "previous field",
        "Shift-Tab  ↑",
        "the field above",
        Where::Writing,
        Run::Keys(&[(KeyCode::BackTab, SHIFT)]),
    ),
    command(
        WRITING,
        "next account",
        "→ on From",
        "send from the next account",
        Where::From,
        Run::Keys(&[(KeyCode::Right, NONE)]),
    ),
    command(
        WRITING,
        "previous account",
        "← on From",
        "send from the account before",
        Where::From,
        Run::Keys(&[(KeyCode::Left, NONE)]),
    ),
    command(
        WRITING,
        "take a file off",
        "x on Files",
        "take the file picked off",
        Where::Files,
        Run::Keys(&[key('x')]),
    ),
    command(
        WRITING,
        "close",
        "Esc",
        "close; asks to save what's written",
        Where::Writing,
        Run::Keys(&[(KeyCode::Esc, NONE)]),
    ),
    command(
        ACCOUNTS,
        "add an account",
        "+",
        "add an account",
        Where::Main,
        Run::Keys(&[key('+')]),
    ),
    command(
        ACCOUNTS,
        "remove the account",
        "x",
        "remove the account picked in the sidebar",
        Where::Account,
        Run::Keys(&[key('x')]),
    ),
    command(
        ACCOUNTS,
        "password",
        "P",
        "type the account's password, or sign in again",
        Where::AccountOrFolder,
        Run::Keys(&[key('P')]),
    ),
    command(
        TUIMAIL,
        "commands",
        "/  Ctrl-p",
        "this: every command (Ctrl-p while writing)",
        Where::Anywhere,
        Run::Palette,
    ),
    command(
        TUIMAIL,
        "keys",
        "?",
        "every key",
        Where::Main,
        Run::Keys(&[key('?')]),
    ),
    command(
        TUIMAIL,
        "settings",
        "O",
        "settings and themes",
        Where::Main,
        Run::Keys(&[key('O')]),
    ),
    command(
        TUIMAIL,
        "info",
        "",
        "this build's repo, version and age, and where it runs",
        Where::Anywhere,
        Run::Info,
    ),
    command(
        TUIMAIL,
        "quit",
        "q  Ctrl-c",
        "quit tuimail",
        Where::Main,
        Run::Keys(&[ctrl('c')]),
    ),
];

/// The command named `name`.
pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}

impl Command {
    /// Why it can't run now, if it can't: said in the menu and when picked.
    pub fn unavailable(&self, app: &App) -> Option<&'static str> {
        let writing = app.composer.as_ref();
        let sidebar_row = || {
            (app.focus == Focus::Sidebar)
                .then(|| app.sidebar_rows().get(app.sidebar_selected).cloned())
                .flatten()
        };
        match self.at {
            Where::Anywhere => None,
            Where::Writing | Where::From | Where::Files if writing.is_none() => {
                Some("only while writing a message")
            }
            Where::From if writing.is_some_and(|c| c.field != Field::From) => {
                Some("go to the From line first")
            }
            Where::Files
                if writing
                    .is_some_and(|c| c.field != Field::Attachments || c.attachments.is_empty()) =>
            {
                Some("go to a file on the Files line first")
            }
            Where::Writing | Where::From | Where::Files => None,
            _ if writing.is_some() => Some("close the message you're writing first"),
            Where::Main | Where::Pane => None,
            Where::Message if app.focus == Focus::Sidebar => Some("go to the messages first"),
            Where::Message if app.items.is_empty() => Some("there's no message here"),
            Where::Message => None,
            Where::List if app.focus != Focus::List => Some("go to the messages first"),
            Where::List => None,
            Where::Reader if app.focus != Focus::Reader || app.reader.is_none() => {
                Some("open a message first")
            }
            Where::Reader => None,
            Where::Account => match sidebar_row() {
                Some(Row::Account(_)) => None,
                _ => Some("pick an account in the sidebar first"),
            },
            Where::AccountOrFolder => match sidebar_row() {
                Some(Row::Account(_) | Row::Folder(..)) => None,
                _ => Some("pick an account in the sidebar first"),
            },
        }
    }

    /// Its keys, as presses.
    pub fn presses(&self) -> Vec<KeyEvent> {
        match self.run {
            Run::Keys(keys) => keys.iter().map(|&(c, m)| KeyEvent::new(c, m)).collect(),
            Run::Search | Run::Info | Run::Palette => Vec::new(),
        }
    }
}

#[derive(Serialize)]
struct Entry {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<Entry>,
}

/// The menu for fzt: a folder for each group, the commands that can run
/// first, the rest saying why not. JSON, which fzt reads as the YAML it is.
pub fn menu(app: &App) -> String {
    let mut groups: Vec<(&str, Vec<Entry>, Vec<Entry>)> = Vec::new();
    for command in COMMANDS {
        // The palette's key isn't in it; info is in fzt's `:` folder.
        if matches!(command.run, Run::Palette | Run::Info) {
            continue;
        }
        let keys = if command.keys.is_empty() {
            String::new()
        } else {
            format!("{}  ·  ", command.keys)
        };
        let why = command.unavailable(app);
        let entry = Entry {
            name: command.name.to_string(),
            description: match why {
                Some(why) => format!("{keys}{} (not now: {why})", command.what),
                None => format!("{keys}{}", command.what),
            },
            action: Some(command.name.to_string()),
            children: Vec::new(),
        };
        let at = match groups.iter().position(|(g, ..)| *g == command.group) {
            Some(at) => at,
            None => {
                groups.push((command.group, Vec::new(), Vec::new()));
                groups.len() - 1
            }
        };
        if why.is_some() {
            groups[at].2.push(entry);
        } else {
            groups[at].1.push(entry);
        }
    }
    // The groups with something to run come first.
    groups.sort_by_key(|(_, can, _)| can.is_empty());
    let entries: Vec<Entry> = groups
        .into_iter()
        .map(|(group, can, cannot)| Entry {
            name: group.to_string(),
            description: String::new(),
            action: None,
            children: can.into_iter().chain(cannot).collect(),
        })
        .collect();
    serde_json::to_string(&entries).expect("plain data")
}

/// The commands for fzt's `:` folder, where each app keeps its info.
pub fn host_commands() -> Vec<fzt::HostCommand> {
    COMMANDS
        .iter()
        .filter(|c| c.run == Run::Info)
        .map(|c| fzt::HostCommand {
            name: c.name.to_string(),
            description: c.what.to_string(),
            action: c.name.to_string(),
        })
        .collect()
}

/// A key press for fzt, if it's one fzt takes.
pub fn fzt_key(key: KeyEvent) -> Option<fzt::Key> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let named = |name| {
        Some(fzt::Key {
            key: name,
            char: String::new(),
            shift,
        })
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        // Ctrl-c, as Esc, closes it; fzt takes no other Ctrl key.
        return match key.code {
            KeyCode::Char('c') => named("esc"),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char(c) => Some(fzt::Key {
            key: "char",
            char: c.to_string(),
            shift: false,
        }),
        KeyCode::Enter => named("enter"),
        KeyCode::Esc => named("esc"),
        KeyCode::Up => named("up"),
        KeyCode::Down => named("down"),
        KeyCode::Left => named("left"),
        KeyCode::Right => named("right"),
        KeyCode::Tab => named("tab"),
        KeyCode::BackTab => named("backtab"),
        KeyCode::Backspace => named("backspace"),
        KeyCode::Delete => named("delete"),
        KeyCode::Home => named("home"),
        KeyCode::End => named("end"),
        KeyCode::PageUp => named("pgup"),
        KeyCode::PageDown => named("pgdn"),
        _ => None,
    }
}

/// The open palette.
pub enum Palette {
    /// fzt's menu, and what it last said to draw.
    Open {
        session: fzt::Session,
        frame: Box<fzt::Frame>,
        /// The size fzt lays it out for: the rows' area.
        size: (u16, u16),
    },
    /// Why it couldn't open; any key closes it.
    Failed(String),
    /// The info panel; any key closes it.
    Info(Vec<(&'static str, String)>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Focus;
    use crate::app::tests::{app, with_mail};
    use std::collections::HashSet;

    fn names(menu: &serde_json::Value) -> Vec<String> {
        menu.as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["children"].as_array().unwrap())
            .map(|entry| entry["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn every_command_has_its_own_name_and_says_what_it_does() {
        let mut seen = HashSet::new();
        for command in COMMANDS {
            assert!(seen.insert(command.name), "{} twice", command.name);
            assert!(!command.what.is_empty(), "{}", command.name);
            assert!(
                command.keys.is_empty() == matches!(command.run, Run::Search | Run::Info),
                "{}: only the palette's own have no key",
                command.name
            );
        }
    }

    #[tokio::test]
    async fn the_menu_has_every_command_and_says_why_one_cant_run() {
        let mut app = app();
        with_mail(&mut app);
        app.focus = Focus::Sidebar;
        let menu: serde_json::Value = serde_json::from_str(&menu(&app)).unwrap();
        let listed = names(&menu);
        let all: Vec<_> = COMMANDS
            .iter()
            .filter(|c| !matches!(c.run, Run::Palette | Run::Info))
            .map(|c| c.name.to_string())
            .collect();
        assert_eq!(listed.len(), all.len());
        assert!(all.iter().all(|name| listed.contains(name)));
        let reply = menu
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["children"].as_array().unwrap())
            .find(|entry| entry["name"] == "reply")
            .unwrap();
        assert_eq!(
            reply["description"],
            "r  ·  reply to the sender (not now: go to the messages first)"
        );
        assert_eq!(reply["action"], "reply");
        // In each group what can run comes first.
        let mail = &menu.as_array().unwrap()[0];
        assert_eq!(mail["name"], "mail");
        assert_eq!(mail["children"][0]["name"], "search");
    }

    #[tokio::test]
    async fn a_command_does_what_its_key_does() {
        let mut app = app();
        with_mail(&mut app);
        app.focus = Focus::List;
        app.selected = 0;
        let starred = |app: &App| app.selected_item().unwrap().message.flags.flagged;
        let before = starred(&app);
        app.run_command("star");
        assert_eq!(starred(&app), !before);
        app.on_key(KeyEvent::new(KeyCode::Char('s'), NONE));
        assert_eq!(starred(&app), before);
        // gg: two presses.
        app.selected = 3;
        app.run_command("first");
        assert_eq!(app.selected, 0);
        app.run_command("write a message");
        assert!(app.composer.is_some());
        app.run_command("reply");
        assert!(app.status.as_ref().unwrap().error);
        assert_eq!(
            app.status.as_ref().unwrap().text,
            "“reply” can't run now: close the message you're writing first"
        );
    }

    #[tokio::test]
    async fn the_palette_opens_from_slash_and_from_ctrl_p_while_writing() {
        let mut app = app();
        with_mail(&mut app);
        app.on_key(KeyEvent::new(KEY, NONE));
        // Tests never load fzt: it says why, and any key closes that.
        assert!(matches!(app.palette, Some(Palette::Failed(_))));
        app.on_key(KeyEvent::new(KeyCode::Char('j'), NONE));
        assert!(app.palette.is_none());
        app.run_command("write a message");
        let composer = app.composer.as_ref().unwrap();
        assert_eq!(composer.to.lines(), [""]);
        app.on_key(KeyEvent::new(KeyCode::Char('/'), NONE));
        assert!(app.palette.is_none(), "/ is text while writing");
        app.on_key(KeyEvent::new(KeyCode::Char('p'), CTRL));
        assert!(matches!(app.palette, Some(Palette::Failed(_))));
        app.on_key(KeyEvent::new(KeyCode::Esc, NONE));
        assert!(app.palette.is_none());
        assert!(
            app.composer.is_some(),
            "Esc closed the palette, not the composer"
        );
        app.run_command("info");
        let Some(Palette::Info(lines)) = &app.palette else {
            panic!("no info");
        };
        assert_eq!(lines[0].0, "Repo");
    }

    #[test]
    fn keys_reach_fzt_as_it_names_them() {
        let press =
            |code, modifiers| fzt_key(KeyEvent::new(code, modifiers)).map(|k| (k.key, k.char));
        assert_eq!(press(KeyCode::Char('r'), NONE), Some(("char", "r".into())));
        assert_eq!(press(KeyCode::Char('R'), SHIFT), Some(("char", "R".into())));
        assert_eq!(press(KeyCode::Enter, NONE), Some(("enter", String::new())));
        assert_eq!(
            press(KeyCode::Char('c'), CTRL),
            Some(("esc", String::new()))
        );
        assert_eq!(press(KeyCode::Char('d'), CTRL), None);
        assert_eq!(press(KeyCode::F(1), NONE), None);
    }
}
