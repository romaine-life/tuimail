//! The session's archive (TUI-17): every message in every folder of every IMAP account, kept in
//! the session's encrypted store ([`Vault`]) for good, after it's moved or deleted on the server
//! too, and searchable.
//!
//! Each account is copied in passes, every [`PASS_EVERY`], on a connection of its own with the
//! account's one sign-in. A pass lists the folders, and in each finds the UIDs the archive
//! hasn't placed: their headers first, and a message whose Message-ID and size the archive
//! already holds (the same mail in another folder, a Gmail label) is placed without being
//! downloaded again. Each message is kept once, by the SHA-256 of its bytes, with a row for each
//! place it was seen (account, folder, UIDVALIDITY, UID) and when it left it. Downloads stop for
//! the day at [`DAY_BYTES`] an account, under Gmail's 2,500 MB a day of IMAP downloads, so the
//! first copy of a big mailbox takes several days and the account keeps working meanwhile. Its
//! words go in an FTS5 index in the same file (subject, sender, recipients, text), so nothing
//! readable of the mail is kept anywhere else.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::vault::Vault;
use crate::account::Protocol;
use crate::secrets::Credentials;
use crate::worker::{self, Endpoints};
use crate::{imap, logfile, mail, text};

/// How often each account is copied.
pub const PASS_EVERY: Duration = Duration::from_secs(15 * 60);
/// The most bytes downloaded for an account in a (UTC) day.
pub const DAY_BYTES: i64 = 2_000_000_000;
/// UIDs whose headers are asked for at once.
const BATCH: usize = 200;
/// The most results a search gives.
pub const MAX_FOUND: usize = 200;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS archive_messages (
    id INTEGER PRIMARY KEY,
    hash BLOB NOT NULL UNIQUE,
    message_id TEXT NOT NULL,
    size INTEGER NOT NULL,
    date INTEGER NOT NULL,
    sender TEXT NOT NULL,
    recipients TEXT NOT NULL,
    subject TEXT NOT NULL,
    raw BLOB NOT NULL,
    kept_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS archive_by_message_id ON archive_messages (message_id, size);
CREATE TABLE IF NOT EXISTS archive_places (
    account TEXT NOT NULL,
    folder TEXT NOT NULL,
    uidvalidity INTEGER NOT NULL,
    uid INTEGER NOT NULL,
    message INTEGER NOT NULL REFERENCES archive_messages (id),
    seen_at INTEGER NOT NULL,
    gone_at INTEGER,
    PRIMARY KEY (account, folder, uidvalidity, uid)
);
CREATE INDEX IF NOT EXISTS archive_places_of ON archive_places (message);
CREATE TABLE IF NOT EXISTS archive_downloads (
    account TEXT NOT NULL,
    day TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    PRIMARY KEY (account, day)
);
CREATE VIRTUAL TABLE IF NOT EXISTS archive_text USING fts5 (
    subject, sender, recipients, body, content = '', contentless_delete = 1
);
";

/// A message the archive found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    /// The archive's own id for it, which [`Vault::archived`] reads.
    pub id: i64,
    /// Unix seconds; 0 if it has no date.
    pub date: i64,
    pub sender: String,
    pub subject: String,
    /// Where it is or was: (account, folder, whether it's still there).
    pub places: Vec<(String, String, bool)>,
}

/// What a pass did, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    pub kept: usize,
    pub placed: usize,
    pub gone: usize,
    /// The day's download budget ran out before the pass was done.
    pub out_of_budget: bool,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

impl Vault {
    /// The archive's message with this Message-ID and size, if it holds one.
    fn archived_as(&self, message_id: &str, size: u32) -> Result<Option<i64>> {
        if message_id.is_empty() {
            return Ok(None);
        }
        Ok(self
            .db()
            .query_row(
                "SELECT id FROM archive_messages WHERE message_id = ?1 AND size = ?2 LIMIT 1",
                params![message_id, size],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Keeps a message (once, by its bytes' hash) with its words, and returns its id.
    pub fn archive(&self, raw: &[u8], body: &str) -> Result<i64> {
        use ring::digest::{SHA256, digest};
        let hash = digest(&SHA256, raw);
        let summary = mail::summarize(raw);
        let mut db = self.db();
        let tx = db.transaction()?;
        if let Some(id) = tx
            .query_row(
                "SELECT id FROM archive_messages WHERE hash = ?1",
                params![hash.as_ref()],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        let sender = summary.from.display();
        let recipients = [summary.to.as_str(), summary.cc.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        tx.execute(
            "INSERT INTO archive_messages
             (hash, message_id, size, date, sender, recipients, subject, raw, kept_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                hash.as_ref(),
                summary.message_id,
                raw.len() as i64,
                summary.date,
                sender,
                recipients,
                summary.subject,
                raw,
                now()
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO archive_text (rowid, subject, sender, recipients, body)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, summary.subject, sender, recipients, body],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Records that `message` is at this place now (a place seen before stays as it was).
    fn place(
        &self,
        account: &str,
        folder: &str,
        uidvalidity: u32,
        uid: u32,
        message: i64,
    ) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO archive_places
             (account, folder, uidvalidity, uid, message, seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![account, folder, uidvalidity, uid, message, now()],
        )?;
        Ok(())
    }

    /// The UIDs placed in a folder under `uidvalidity` and not gone.
    fn placed(&self, account: &str, folder: &str, uidvalidity: u32) -> Result<HashSet<u32>> {
        let db = self.db();
        let mut statement = db.prepare(
            "SELECT uid FROM archive_places
             WHERE account = ?1 AND folder = ?2 AND uidvalidity = ?3 AND gone_at IS NULL",
        )?;
        let uids = statement
            .query_map(params![account, folder, uidvalidity], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(uids)
    }

    /// Marks places gone: those in a folder under another UIDVALIDITY, and those under this one
    /// not in `present`. The messages stay.
    fn mark_gone(
        &self,
        account: &str,
        folder: &str,
        uidvalidity: u32,
        present: &HashSet<u32>,
    ) -> Result<usize> {
        let mut gone = self.db().execute(
            "UPDATE archive_places SET gone_at = ?4
             WHERE account = ?1 AND folder = ?2 AND uidvalidity != ?3 AND gone_at IS NULL",
            params![account, folder, uidvalidity, now()],
        )?;
        for uid in self.placed(account, folder, uidvalidity)? {
            if !present.contains(&uid) {
                gone += self.db().execute(
                    "UPDATE archive_places SET gone_at = ?5
                     WHERE account = ?1 AND folder = ?2 AND uidvalidity = ?3 AND uid = ?4",
                    params![account, folder, uidvalidity, uid, now()],
                )?;
            }
        }
        Ok(gone)
    }

    /// What's left of today's download budget for the account.
    fn budget_left(&self, account: &str) -> Result<i64> {
        let spent: Option<i64> = self
            .db()
            .query_row(
                "SELECT bytes FROM archive_downloads WHERE account = ?1 AND day = ?2",
                params![account, today()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(DAY_BYTES - spent.unwrap_or(0))
    }

    fn spend(&self, account: &str, bytes: usize) -> Result<()> {
        let db = self.db();
        db.execute(
            "INSERT INTO archive_downloads (account, day, bytes) VALUES (?1, ?2, ?3)
             ON CONFLICT(account, day) DO UPDATE SET bytes = bytes + excluded.bytes",
            params![account, today(), bytes as i64],
        )?;
        db.execute(
            "DELETE FROM archive_downloads WHERE day < date('now', '-7 days')",
            [],
        )?;
        Ok(())
    }

    /// The archive's messages matching `query` (FTS5's syntax: words, "phrases", OR, NOT,
    /// `subject:`), best first, at most `limit`.
    pub fn search_archive(&self, query: &str, limit: usize) -> Result<Vec<Found>> {
        let db = self.db();
        let mut statement = db.prepare(
            "SELECT m.id, m.date, m.sender, m.subject FROM archive_text
             JOIN archive_messages m ON m.id = archive_text.rowid
             WHERE archive_text MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows: Vec<(i64, i64, String, String)> = statement
            .query_map(params![query, limit.min(MAX_FOUND) as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .context("not a search the archive understands")?
            .collect::<rusqlite::Result<_>>()?;
        let mut places = db.prepare(
            "SELECT account, folder, gone_at IS NULL FROM archive_places WHERE message = ?1
             ORDER BY seen_at",
        )?;
        rows.into_iter()
            .map(|(id, date, sender, subject)| {
                let places = places
                    .query_map(params![id], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(Found {
                    id,
                    date,
                    sender: text::one_line(&sender, mail::MAX_NAME),
                    subject: text::one_line(&subject, mail::MAX_SUBJECT),
                    places,
                })
            })
            .collect()
    }

    /// An archived message's bytes.
    pub fn archived(&self, id: i64) -> Result<Option<Vec<u8>>> {
        Ok(self
            .db()
            .query_row(
                "SELECT raw FROM archive_messages WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?)
    }
}

/// A message's text for the index, laid out on a big-stack thread (HTML can nest deeply).
async fn words(raw: Vec<u8>) -> String {
    let (tx, rx) = tokio::sync::oneshot::channel();
    mail::in_background(
        move || {
            mail::parse(&raw)
                .map(|parsed| mail::plain_text(&parsed))
                .unwrap_or_default()
        },
        move |text| {
            let _ = tx.send(text.unwrap_or_default());
        },
    );
    rx.await.unwrap_or_default()
}

/// One pass over an account: every folder's new messages archived, and what left noted.
pub async fn pass(vault: &Vault, creds: &Credentials, endpoints: &Endpoints) -> Result<Pass> {
    let account = creds.account();
    let id = account.id();
    let mut done = Pass::default();
    let (mut session, _) = worker::imap_login(creds, &endpoints.incoming).await?;
    let folders = imap::folders(&mut session, account).await?;
    for folder in folders.iter().filter(|f| f.selectable) {
        let selected = imap::select(&mut session, &folder.name).await?;
        let present: HashSet<u32> = if selected.exists == 0 {
            HashSet::new()
        } else {
            imap::search(&mut session, "ALL")
                .await?
                .into_iter()
                .collect()
        };
        done.gone += vault.mark_gone(&id, &folder.name, selected.uidvalidity, &present)?;
        let placed = vault.placed(&id, &folder.name, selected.uidvalidity)?;
        let mut new: Vec<u32> = present.difference(&placed).copied().collect();
        new.sort_unstable();
        for batch in new.chunks(BATCH) {
            for header in imap::headers(&mut session, batch).await? {
                let known = vault.archived_as(&header.summary.message_id, header.size)?;
                let message = match known {
                    Some(message) => message,
                    None => {
                        if vault.budget_left(&id)? < i64::from(header.size) {
                            done.out_of_budget = true;
                            imap::logout(session).await;
                            return Ok(done);
                        }
                        let Some(raw) = imap::body(&mut session, header.uid).await? else {
                            continue;
                        };
                        vault.spend(&id, raw.len())?;
                        let body = words(raw.clone()).await;
                        done.kept += 1;
                        vault.archive(&raw, &body)?
                    }
                };
                vault.place(&id, &folder.name, selected.uidvalidity, header.uid, message)?;
                done.placed += 1;
            }
        }
    }
    imap::logout(session).await;
    Ok(done)
}

/// Archives `creds`'s account every [`PASS_EVERY`], from now on. POP3 accounts have no folders
/// on the server to copy, and aren't archived.
pub fn keep(
    vault: Arc<Vault>,
    creds: Credentials,
    endpoints: Endpoints,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if creds.account().protocol != Protocol::Imap {
            return;
        }
        let number = creds.number();
        let mut link = logfile::Link::new(number);
        loop {
            match pass(&vault, &creds, &endpoints).await {
                Ok(done) => {
                    link.back();
                    logfile::counted("archived", number, done.kept, done.placed, done.gone);
                    if done.out_of_budget {
                        logfile::note("archiving waits for tomorrow's downloads", Some(number));
                    }
                }
                Err(e) => link.failed("archiving", &e),
            }
            tokio::time::sleep(PASS_EVERY).await;
        }
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::relay::vault::Key;

    fn vault() -> (Vault, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "tuimail-archive-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let key = Key::from_hex(&"cd".repeat(32)).unwrap();
        (Vault::open(&dir.join("session.db"), &key).unwrap(), dir)
    }

    const RAW: &[u8] = b"From: Ann <ann@example.com>\r\nTo: bob@example.com\r\nSubject: Quarterly figures\r\nMessage-ID: <q1@example.com>\r\nDate: Thu, 19 Nov 2026 10:00:00 +0000\r\n\r\nThe numbers are in the spreadsheet.\r\n";

    #[test]
    fn a_message_is_kept_once_found_by_its_words_and_stays_after_it_leaves() {
        let (vault, dir) = vault();
        let id = vault
            .archive(RAW, "The numbers are in the spreadsheet.")
            .unwrap();
        assert_eq!(vault.archive(RAW, "again").unwrap(), id);
        vault.place("a", "INBOX", 7, 1, id).unwrap();
        vault.place("a", "[Gmail]/All Mail", 3, 40, id).unwrap();
        assert_eq!(
            vault
                .archived_as("q1@example.com", RAW.len() as u32)
                .unwrap(),
            Some(id)
        );
        let found = vault.search_archive("spreadsheet", 10).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].subject, "Quarterly figures");
        assert!(found[0].sender.contains("ann@example.com"));
        assert_eq!(found[0].places.len(), 2);
        // Deleted from the inbox: the place is gone, the message stays.
        let gone = vault.mark_gone("a", "INBOX", 7, &HashSet::new()).unwrap();
        assert_eq!(gone, 1);
        let found = vault.search_archive("subject:quarterly", 10).unwrap();
        assert!(
            found[0]
                .places
                .contains(&("a".into(), "INBOX".into(), false))
        );
        assert!(
            found[0]
                .places
                .contains(&("a".into(), "[Gmail]/All Mail".into(), true))
        );
        assert_eq!(vault.archived(id).unwrap().unwrap(), RAW);
        // A folder that was renumbered: everything under its old UIDVALIDITY is gone.
        assert_eq!(
            vault
                .mark_gone("a", "[Gmail]/All Mail", 4, &HashSet::new())
                .unwrap(),
            1
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn downloads_stop_at_the_days_budget() {
        let (vault, dir) = vault();
        assert_eq!(vault.budget_left("a").unwrap(), DAY_BYTES);
        vault.spend("a", 1_500_000_000).unwrap();
        vault.spend("a", 400_000_000).unwrap();
        assert_eq!(vault.budget_left("a").unwrap(), DAY_BYTES - 1_900_000_000);
        assert_eq!(vault.budget_left("b").unwrap(), DAY_BYTES);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_search_the_index_cant_read_is_an_error_not_a_crash() {
        let (vault, dir) = vault();
        assert!(vault.search_archive("\"unclosed", 10).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
