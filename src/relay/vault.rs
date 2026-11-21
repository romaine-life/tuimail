//! The session's own store: an SQLCipher file, encrypted with a key the session reads from a
//! file (on the home cluster, OpenBao's `secret/services/tuimail` through an ExternalSecret).
//! It holds what each account signs in with (the refresh token, or the password), kept only
//! once a login with it worked, so the house's one sign-in survives restarts.
//!
//! Only a build with SQLCipher opens it (Linux's: Cargo.toml); any other refuses, rather than
//! write the sign-ins to a plain file.

use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};

use crate::secrets::Secret;
use crate::{config, store};

/// The store's file, in the session's state folder.
pub const FILE: &str = "session.db";

/// The key file's most bytes: 64 hex digits and a line end.
const MAX_KEY_FILE: u64 = 128;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS credentials (
    account TEXT PRIMARY KEY,
    secret TEXT NOT NULL,
    kept_at INTEGER NOT NULL
);
";

/// A 256-bit key, given to SQLCipher as is (no passphrase to stretch).
pub struct Key([u8; 32]);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(hidden)")
    }
}

impl Key {
    /// The key in `path`: 64 hex digits, nothing else but a line end.
    pub fn read(path: &Path) -> Result<Self> {
        let shown = config::shown(path);
        let bytes = config::read_capped(path, MAX_KEY_FILE)
            .with_context(|| format!("cannot read the key in {shown}"))?;
        let text = std::str::from_utf8(&bytes)
            .ok()
            .map(str::trim)
            .with_context(|| format!("{shown} isn't a key"))?;
        Self::from_hex(text).with_context(|| format!("{shown} isn't 64 hex digits"))
    }

    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 64 {
            return None;
        }
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(key))
    }

    fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

pub struct Vault {
    db: Mutex<Connection>,
}

impl Vault {
    /// Opens (or makes) the store at `path` with `key`. A key that isn't the file's is refused,
    /// as is a build without SQLCipher.
    pub fn open(path: &Path, key: &Key) -> Result<Self> {
        let shown = config::shown(path);
        let flags = rusqlite::OpenFlags::default() | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let db = Connection::open_with_flags(path, flags)
            .with_context(|| format!("cannot open {shown}"))?;
        Self::key(&db, key).with_context(|| format!("cannot open {shown}"))?;
        db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA secure_delete = ON;")?;
        db.execute_batch(SCHEMA)
            .and_then(|()| db.execute_batch(super::archive::SCHEMA))
            .with_context(|| format!("cannot set up {shown}"))?;
        store::Store::protect(path).with_context(|| format!("cannot protect {shown}"))?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// Gives SQLCipher the key, after checking this build has SQLCipher, and checks the key
    /// opens the file.
    fn key(db: &Connection, key: &Key) -> Result<()> {
        if !has_sqlcipher(db) {
            bail!("this build of tuimail has no SQLCipher, so it can't keep the session's store");
        }
        // The key is 64 hex digits, made here: nothing in it can end the statement.
        db.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key.hex()))?;
        db.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .context("the key isn't this store's")
    }

    pub(super) fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What `account` signs in with, if it's kept.
    pub fn credential(&self, account: &str) -> Result<Option<Secret>> {
        let secret: Option<String> = self
            .db()
            .query_row(
                "SELECT secret FROM credentials WHERE account = ?1",
                params![account],
                |row| row.get(0),
            )
            .optional()?;
        Ok(secret.map(Secret::new))
    }

    /// Keeps what `account` signed in with, replacing what was kept.
    pub fn keep(&self, account: &str, secret: &Secret) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.db().execute(
            "INSERT INTO credentials (account, secret, kept_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(account) DO UPDATE SET secret = excluded.secret, kept_at = excluded.kept_at
             WHERE secret != excluded.secret",
            params![account, secret.expose(), now],
        )?;
        Ok(())
    }
}

/// Whether SQLite here is SQLCipher (it answers `cipher_version`).
fn has_sqlcipher(db: &Connection) -> bool {
    db.query_row("PRAGMA cipher_version", [], |row| row.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn keys_are_64_hex_digits() {
        assert!(Key::from_hex(KEY).is_some());
        assert!(Key::from_hex(&KEY[1..]).is_none());
        assert!(Key::from_hex(&KEY.replace('0', "g")).is_none());
        assert!(Key::from_hex(&format!("{}é", &KEY[..62])).is_none());
        assert_eq!(Key::from_hex(KEY).unwrap().hex(), KEY);
        assert_eq!(format!("{:?}", Key::from_hex(KEY).unwrap()), "Key(hidden)");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn keeps_sign_ins_encrypted() {
        let dir = std::env::temp_dir().join(format!("tuimail-vault-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        let key = Key::from_hex(KEY).unwrap();
        {
            let vault = Vault::open(&path, &key).unwrap();
            assert!(vault.credential("a").unwrap().is_none());
            vault.keep("a", &Secret::new("refresh-1")).unwrap();
            vault.keep("a", &Secret::new("refresh-2")).unwrap();
        }
        let vault = Vault::open(&path, &key).unwrap();
        assert_eq!(
            vault.credential("a").unwrap().unwrap().expose(),
            "refresh-2"
        );
        drop(vault);
        // Nothing kept is readable in the file.
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(9).any(|w| w == b"refresh-2"));
        assert!(!raw.starts_with(b"SQLite format 3"));
        // Another key doesn't open it.
        let other = Key::from_hex(&KEY.replace('0', "1")).unwrap();
        assert!(Vault::open(&path, &other).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn refuses_without_sqlcipher() {
        let dir = std::env::temp_dir().join(format!("tuimail-vault-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let error = Vault::open(&dir.join(FILE), &Key::from_hex(KEY).unwrap())
            .err()
            .expect("a plain SQLite build refuses");
        assert!(format!("{error:#}").contains("no SQLCipher"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
