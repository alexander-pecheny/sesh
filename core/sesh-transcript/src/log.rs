//! The Session log (ADR 0010): what every device shows of each Agent session on this
//! machine, one row per item, rewritten in place and stamped with the machine's next
//! sequence number on every write.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::follower::Event;
use crate::reconcile::{Change, Rows};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (key TEXT PRIMARY KEY, seq INTEGER NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS items (
    session TEXT NOT NULL,
    id TEXT NOT NULL,
    ord INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    final INTEGER NOT NULL,
    gone INTEGER NOT NULL DEFAULT 0,
    entry TEXT,
    body TEXT NOT NULL,
    PRIMARY KEY (session, id)
);
CREATE INDEX IF NOT EXISTS items_seq ON items (session, seq);
CREATE INDEX IF NOT EXISTS items_ord ON items (session, ord);
CREATE INDEX IF NOT EXISTS items_entry ON items (session, entry);
";

type Result<T> = std::result::Result<T, String>;

fn sql(err: rusqlite::Error) -> String {
    format!("Session log: {err}")
}

pub struct Log {
    db: Connection,
}

impl Log {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path).map_err(sql)?;
        db.busy_timeout(std::time::Duration::from_secs(10)).map_err(sql)?;
        db.pragma_update(None, "journal_mode", "wal").map_err(sql)?;
        db.execute_batch(SCHEMA).map_err(sql)?;
        // A new log numbers from its creation time, above any older log's numbers, so a
        // device holding a number from another log can be told by it.
        let base = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64 * 1000;
        db.execute("INSERT OR IGNORE INTO meta (key, value) VALUES ('seq', ?1)", [base]).map_err(sql)?;
        db.execute("INSERT OR IGNORE INTO meta (key, value) SELECT 'base', value FROM meta WHERE key = 'seq'", [])
            .map_err(sql)?;
        Ok(Log { db })
    }

    /// The first number this log gave out; a device's number below it is another log's.
    pub fn base(&self) -> Result<i64> {
        let base: Option<i64> = self
            .db
            .query_row("SELECT value FROM meta WHERE key = 'base'", [], |row| row.get(0))
            .optional()
            .map_err(sql)?;
        Ok(base.unwrap_or(0))
    }

    pub fn head(&self) -> Result<i64> {
        let head: Option<i64> = self
            .db
            .query_row("SELECT value FROM meta WHERE key = 'seq'", [], |row| row.get(0))
            .optional()
            .map_err(sql)?;
        Ok(head.unwrap_or(0))
    }

    fn next(&self) -> Result<i64> {
        let seq = self.head()? + 1;
        self.db
            .execute("INSERT INTO meta (key, value) VALUES ('seq', ?1) ON CONFLICT(key) DO UPDATE SET value = ?1", [seq])
            .map_err(sql)?;
        Ok(seq)
    }

    pub fn session(&self, key: &str) -> Result<Option<Value>> {
        let body: Option<String> = self
            .db
            .query_row("SELECT body FROM sessions WHERE key = ?1", [key], |row| row.get(0))
            .optional()
            .map_err(sql)?;
        Ok(body.and_then(|body| serde_json::from_str(&body).ok()))
    }

    pub fn put_session(&self, key: &str, body: &Value) -> Result<()> {
        let seq = self.next()?;
        self.db
            .execute(
                "INSERT INTO sessions (key, seq, body) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET seq = ?2, body = ?3",
                params![key, seq, body.to_string()],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Every session whose summary changed after `seq`, as `session` lines.
    pub fn sessions_since(&self, seq: i64) -> Result<Vec<Value>> {
        let mut query = self
            .db
            .prepare("SELECT key, seq, body FROM sessions WHERE seq > ?1 ORDER BY seq")
            .map_err(sql)?;
        let rows = query
            .query_map([seq], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)))
            .map_err(sql)?;
        rows.map(|row| {
            let (key, seq, body) = row.map_err(sql)?;
            let mut line = serde_json::from_str::<Value>(&body).unwrap_or_default();
            line["t"] = "session".into();
            line["session"] = key.into();
            line["seq"] = seq.into();
            Ok(line)
        })
        .collect()
    }

    /// The item for `entry`, when one holds it already.
    fn item_of(&self, session: &str, entry: &str) -> Result<Option<String>> {
        self.db
            .query_row("SELECT id FROM items WHERE session = ?1 AND entry = ?2 AND gone = 0", [session, entry], |row| row.get(0))
            .optional()
            .map_err(sql)
    }

    fn ord_of(&self, session: &str, id: &str) -> Result<Option<i64>> {
        self.db
            .query_row("SELECT ord FROM items WHERE session = ?1 AND id = ?2", [session, id], |row| row.get(0))
            .optional()
            .map_err(sql)
    }

    fn last_ord(&self, session: &str) -> Result<i64> {
        let ord: Option<i64> = self
            .db
            .query_row("SELECT MAX(ord) FROM items WHERE session = ?1", [session], |row| row.get(0))
            .map_err(sql)?;
        Ok(ord.unwrap_or(0))
    }

    pub fn first_ord(&self, session: &str) -> Result<Option<i64>> {
        self.db
            .query_row("SELECT MIN(ord) FROM items WHERE session = ?1 AND gone = 0", [session], |row| row.get(0))
            .map_err(sql)
    }

    /// Writes an item, keeping its place when it exists; a new one goes at `ord`, or last.
    pub fn put_item(&self, session: &str, id: &str, ord: Option<i64>, final_: bool, entry: Option<&str>, body: &Value) -> Result<()> {
        let ord = match self.ord_of(session, id)? {
            Some(ord) => ord,
            None => match ord {
                Some(ord) => ord,
                None => self.last_ord(session)? + 1,
            },
        };
        let seq = self.next()?;
        self.db
            .execute(
                "INSERT INTO items (session, id, ord, seq, final, gone, entry, body) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7)
                 ON CONFLICT(session, id) DO UPDATE SET ord = ?3, seq = ?4, final = ?5, gone = 0, entry = ?6, body = ?7",
                params![session, id, ord, seq, final_, entry, body.to_string()],
            )
            .map_err(sql)?;
        Ok(())
    }

    /// Moves an item, as a provisional one is when a final entry lands above it.
    fn move_item(&self, session: &str, id: &str, ord: i64) -> Result<()> {
        let seq = self.next()?;
        self.db
            .execute("UPDATE items SET ord = ?3, seq = ?4 WHERE session = ?1 AND id = ?2", params![session, id, ord, seq])
            .map_err(sql)?;
        Ok(())
    }

    /// Writes what the reconciler decided.
    pub fn change(&self, session: &str, change: Change) -> Result<()> {
        match change {
            Change::Show(id, body) => self.put_item(session, &id, None, false, None, &body),
            Change::Drop(id) => self.drop_item(session, &id),
            Change::Rewrite { id, entry, body } => self.put_item(session, &id, None, true, Some(&entry), &body),
            Change::Land { id, entry, body, above } => {
                if let Some(held) = entry.as_deref().map(|entry| self.item_of(session, entry)).transpose()?.flatten() {
                    return self.put_item(session, &held, None, true, entry.as_deref(), &body);
                }
                let first = match above.first() {
                    Some(live) => self.ord_of(session, live)?,
                    None => None,
                };
                if let Some(first) = first {
                    for (offset, live) in above.iter().enumerate().rev() {
                        self.move_item(session, live, first + 1 + offset as i64)?;
                    }
                }
                self.put_item(session, &id, first, true, entry.as_deref(), &body)
            }
        }
    }

    /// The Transcript entry of the first row that holds one, to read older entries before it.
    pub fn first_entry(&self, session: &str) -> Result<Option<String>> {
        self.db
            .query_row(
                "SELECT entry FROM items WHERE session = ?1 AND entry IS NOT NULL AND gone = 0 ORDER BY ord LIMIT 1",
                [session],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    /// The one deletion: a provisional item the Transcript never held. Its row stays, marked
    /// gone, so a device that saw it learns to drop it.
    fn drop_item(&self, session: &str, id: &str) -> Result<()> {
        let seq = self.next()?;
        self.db
            .execute("UPDATE items SET gone = 1, seq = ?3 WHERE session = ?1 AND id = ?2", params![session, id, seq])
            .map_err(sql)?;
        Ok(())
    }

    /// Every item of `session` written after `seq`, oldest write first, as `item` lines.
    pub fn items_since(&self, session: &str, seq: i64) -> Result<Vec<Value>> {
        self.items("WHERE session = ?1 AND seq > ?2 ORDER BY seq", params![session, seq])
    }

    /// Up to `limit` items of `session` before `ord`, in order.
    pub fn page(&self, session: &str, ord: i64, limit: usize) -> Result<Vec<Value>> {
        let mut page = self.items(
            "WHERE session = ?1 AND ord < ?2 AND gone = 0 ORDER BY ord DESC LIMIT ?3",
            params![session, ord, limit as i64],
        )?;
        page.reverse();
        Ok(page)
    }

    /// The last `limit` items of `session`, in order, for a device opening it.
    pub fn last(&self, session: &str, limit: usize) -> Result<Vec<Value>> {
        self.page(session, i64::MAX, limit)
    }

    fn items(&self, filter: &str, args: impl rusqlite::Params) -> Result<Vec<Value>> {
        let mut query = self
            .db
            .prepare(&format!("SELECT session, id, ord, seq, final, gone, body FROM items {filter}"))
            .map_err(sql)?;
        let rows = query
            .query_map(args, |row| {
                Ok(json!({
                    "t": "item",
                    "session": row.get::<_, String>(0)?,
                    "id": row.get::<_, String>(1)?,
                    "ord": row.get::<_, i64>(2)?,
                    "seq": row.get::<_, i64>(3)?,
                    "final": row.get::<_, bool>(4)?,
                    "gone": row.get::<_, bool>(5)?,
                    "entry": serde_json::from_str::<Value>(&row.get::<_, String>(6)?).unwrap_or_default(),
                }))
            })
            .map_err(sql)?;
        rows.map(|row| row.map_err(sql)).collect()
    }
}

/// Turns one session's follower events into the Session log's rows and summary.
pub struct Writer {
    pub session: String,
    rows: Rows,
    summary: Value,
}

impl Writer {
    pub fn new(session: &str, log: &Log) -> Result<Self> {
        let summary = log.session(session)?.unwrap_or_else(|| json!({"permissions": []}));
        Ok(Writer { session: session.to_string(), rows: Rows::new(log.head()?), summary })
    }

    /// The Transcript cursor the session was last read to, so a restart picks up there.
    pub fn cursor(&self) -> Option<&str> {
        self.summary["cursor"].as_str()
    }

    /// Writes `events`, also taking what herdr says of the pane: its folder, the Agent's
    /// name and how many turns it has finished.
    pub fn apply(&mut self, log: &Log, events: &[Event], pane: Option<&Value>) -> Result<()> {
        let before = self.summary.clone();
        if let Some(pane) = pane {
            self.summary["cwd"] = pane["cwd"].clone();
            self.summary["name"] = pane["name"].clone();
            self.summary["done"] = pane["completion_seq"].clone();
        }
        for event in events {
            for change in self.event(event) {
                log.change(&self.session, change)?;
            }
        }
        if self.summary != before {
            log.put_session(&self.session, &self.summary)?;
        }
        Ok(())
    }

    fn event(&mut self, event: &Event) -> Vec<Change> {
        match event {
            Event::Hello { agent, transcript } => {
                self.summary["agent"] = agent.name().into();
                self.summary["transcript"] = transcript.clone().into();
            }
            Event::Entry { entry, replaces } => {
                self.summary["last"] = entry.at.clone().into();
                return vec![self.rows.land(entry, replaces.as_deref())];
            }
            Event::Live(shown) => {
                self.summary["status"] = shown.status.clone().into();
                return self.rows.show(&shown.items);
            }
            Event::Switch { reason, transcript } => {
                self.summary["transcript"] = transcript.clone().into();
                return vec![self.rows.switch(reason, transcript)];
            }
            Event::State(state) => self.summary["state"] = state.clone().into(),
            Event::Background(tasks) => self.summary["background"] = tasks.clone(),
            Event::Cursor(cursor) => self.summary["cursor"] = cursor.clone().into(),
            Event::Permission(line) => {
                let mut list: Vec<Value> = self.summary["permissions"].as_array().cloned().unwrap_or_default();
                list.retain(|open| open["id"] != line["id"]);
                let mut open = line.clone();
                open.as_object_mut().map(|open| open.remove("t"));
                list.push(open);
                self.summary["permissions"] = list.into();
            }
            Event::PermissionDone(id) => {
                let mut list: Vec<Value> = self.summary["permissions"].as_array().cloned().unwrap_or_default();
                list.retain(|open| open["id"] != id.as_str());
                self.summary["permissions"] = list.into();
            }
            Event::Ping => {}
        }
        Vec::new()
    }
}
