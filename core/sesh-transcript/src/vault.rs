//! A Vault: records in a SQLite database and Transcript copies beside it, searchable together.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{Agent, Transcript};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS records (
    n INTEGER PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    body TEXT NOT NULL,
    seq INTEGER NOT NULL,
    deleted INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS records_seq ON records(seq);
CREATE VIRTUAL TABLE IF NOT EXISTS record_text USING fts5(title, text);
CREATE TRIGGER IF NOT EXISTS records_insert AFTER INSERT ON records WHEN NOT new.deleted BEGIN
    INSERT INTO record_text(rowid, title, text)
    VALUES (new.n, json_extract(new.body, '$.title'), json_extract(new.body, '$.text'));
END;
CREATE TRIGGER IF NOT EXISTS records_update AFTER UPDATE ON records BEGIN
    DELETE FROM record_text WHERE rowid = old.n;
    INSERT INTO record_text(rowid, title, text)
    SELECT new.n, json_extract(new.body, '$.title'), json_extract(new.body, '$.text')
    WHERE NOT new.deleted;
END;
CREATE TABLE IF NOT EXISTS copies (
    session TEXT NOT NULL,
    file TEXT NOT NULL,
    agent TEXT NOT NULL,
    indexed INTEGER NOT NULL,
    -- The Transcript's hint, under the name older helpers that share this file still use.
    items INTEGER NOT NULL,
    PRIMARY KEY (session, file)
);
CREATE VIRTUAL TABLE IF NOT EXISTS line_text
    USING fts5(session UNINDEXED, file UNINDEXED, item UNINDEXED, text);
";
const DEFAULT_LIMIT: usize = 20;
const SNIPPET_TOKENS: i32 = 12;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Self(err.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Self(err.to_string())
    }
}

impl From<Error> for String {
    fn from(err: Error) -> Self {
        err.0
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,
    pub kind: String,
    pub body: Value,
    pub seq: i64,
    pub deleted: bool,
}

impl Record {
    pub fn line(&self) -> Value {
        json!({"t": "record", "id": self.id, "kind": self.kind, "body": self.body,
            "seq": self.seq, "deleted": self.deleted})
    }

    fn edited(&self) -> i64 {
        self.body["edited"].as_i64().unwrap_or_default()
    }

    fn conflict(&self) -> Self {
        let body = &self.body;
        Self {
            id: uuid(),
            kind: "conflict".into(),
            body: json!({"of": self.id, "kind": self.kind, "task": body["task"],
                "title": body["title"], "text": body["text"], "edited": body["edited"]}),
            seq: 0,
            deleted: false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Change {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub body: Value,
    #[serde(default)]
    pub base: i64,
    #[serde(default)]
    pub deleted: bool,
}

pub fn head_line(seq: i64) -> Value {
    json!({"t": "head", "seq": seq})
}

pub struct Vault {
    dir: PathBuf,
    db: Connection,
}

impl Vault {
    pub fn open(dir: impl Into<PathBuf>, create: bool) -> Result<Self> {
        let dir = dir.into();
        let path = dir.join("vault.db");
        if create {
            std::fs::create_dir_all(dir.join("transcripts"))?;
        } else if !path.exists() {
            return Err(Error(format!("no Vault in {}", dir.display())));
        }
        let db = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        db.busy_timeout(std::time::Duration::from_secs(10))?;
        db.pragma_update(None, "journal_mode", "wal")?;
        db.execute_batch(SCHEMA)?;
        Ok(Self { dir, db })
    }

    pub fn head(&self) -> Result<i64> {
        Ok(head(&self.db)?)
    }

    /// Changes whenever another connection commits, so `follow` looks only then.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("PRAGMA data_version", [], |row| row.get(0))?)
    }

    pub fn pull(&self, seq: i64) -> Result<(Vec<Record>, i64)> {
        let read = self.db.unchecked_transaction()?;
        let records = read
            .prepare(&format!("{SELECT_RECORD} WHERE seq > ?1 ORDER BY seq"))?
            .query_map([seq], record)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let head = head(&read)?;
        read.commit()?;
        Ok((records, head))
    }

    pub fn push(&mut self, changes: Vec<Change>) -> Result<(Vec<Record>, i64)> {
        let write = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut head = head(&write)?;
        let mut written = Vec::new();
        for change in changes {
            let stored = write
                .query_row(
                    &format!("{SELECT_RECORD} WHERE id = ?1"),
                    [&change.id],
                    record,
                )
                .optional()?;
            let incoming = Record {
                id: change.id,
                kind: change.kind,
                body: change.body,
                seq: 0,
                deleted: change.deleted,
            };
            let merges = matches!(incoming.kind.as_str(), "entry" | "document");
            let (winner, loser) = match stored {
                Some(stored) if merges && stored.seq != change.base => {
                    if incoming.edited() >= stored.edited() {
                        (Some(incoming), stored)
                    } else {
                        (None, incoming)
                    }
                }
                _ => {
                    written.push(incoming);
                    continue;
                }
            };
            written.extend(winner);
            if !loser.deleted {
                written.push(loser.conflict());
            }
        }
        for record in &mut written {
            head += 1;
            record.seq = head;
            write.execute(
                "INSERT INTO records (id, kind, body, seq, deleted) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, body = excluded.body,
                    seq = excluded.seq, deleted = excluded.deleted",
                params![
                    record.id,
                    record.kind,
                    record.body.to_string(),
                    record.seq,
                    record.deleted
                ],
            )?;
        }
        write.commit()?;
        Ok((written, head))
    }

    fn copy_path(&self, session: &str, file: &str) -> Result<PathBuf> {
        let plain = |name: &str| {
            !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
        };
        if !plain(session) || !plain(file) {
            return Err(Error(format!("not a Transcript copy: {session}/{file}")));
        }
        Ok(self.dir.join("transcripts").join(session).join(file))
    }

    pub fn size(&self, session: &str, file: &str) -> Result<u64> {
        match std::fs::metadata(self.copy_path(session, file)?) {
            Ok(metadata) => Ok(metadata.len()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(err) => Err(err.into()),
        }
    }

    pub fn append(&self, session: &str, file: &str, offset: u64, bytes: &[u8]) -> Result<u64> {
        let path = self.copy_path(session, file)?;
        std::fs::create_dir_all(path.parent().expect("a copy has a folder"))?;
        let mut copy = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = copy.metadata()?.len();
        if size != offset {
            return Err(Error(format!(
                "{session}/{file} is {size} bytes long, not {offset}"
            )));
        }
        copy.write_all(bytes)?;
        self.index(session, file)?;
        Ok(size + bytes.len() as u64)
    }

    pub fn copy(&self, session: &str, from: &Path) -> Result<u64> {
        let file = from
            .file_name()
            .ok_or_else(|| Error(format!("not a Transcript: {}", from.display())))?
            .to_string_lossy();
        let size = self.size(session, &file)?;
        let mut source = File::open(from)?;
        if source.metadata()?.len() < size {
            return Err(Error(format!(
                "{} is shorter than its copy of {size} bytes",
                from.display()
            )));
        }
        source.seek(SeekFrom::Start(size))?;
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes)?;
        self.append(session, &file, size, &bytes)
    }

    fn index(&self, session: &str, file: &str) -> Result<()> {
        let path = self.copy_path(session, file)?;
        let known: Option<(String, i64, i64)> = self
            .db
            .query_row(
                "SELECT agent, indexed, items FROM copies WHERE session = ?1 AND file = ?2",
                [session, file],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (agent, indexed, hint) = match known {
            Some((agent, indexed, hint)) => (Agent::named(&agent), indexed, hint),
            None => (agent_of(&path)?, 0, 0),
        };
        let Some(agent) = agent else {
            return Ok(());
        };
        let write = self.db.unchecked_transaction()?;
        let mut transcript = Transcript::new(agent, &path);
        let entries = transcript.read_from(indexed as u64, hint)?;
        if transcript.start < indexed as u64 {
            write.execute(
                "DELETE FROM line_text WHERE session = ?1 AND file = ?2",
                [session, file],
            )?;
        }
        for entry in &entries {
            if let ("user" | "text", Some(text)) = (entry.kind, &entry.text) {
                write.execute(
                    "INSERT INTO line_text (session, file, item, text) VALUES (?1, ?2, ?3, ?4)",
                    params![session, file, entry.id, text],
                )?;
            }
        }
        write.execute(
            "INSERT OR REPLACE INTO copies (session, file, agent, indexed, items)
            VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session,
                file,
                agent.name(),
                transcript.offset as i64,
                transcript.hint()
            ],
        )?;
        write.commit()?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: Option<usize>) -> Result<Vec<Value>> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        let words: Vec<String> = query
            .split_whitespace()
            .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
            .collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let query = words.join(" ");
        let mut hits: Vec<(f64, Value)> = Vec::new();
        let mut records = self.db.prepare(
            "SELECT r.kind, r.id, IIF(r.kind = 'task', r.id, json_extract(r.body, '$.task')),
                snippet(record_text, -1, '**', '**', '…', ?3), bm25(record_text, 2.0, 1.0) AS score
            FROM record_text JOIN records r ON r.n = record_text.rowid
            WHERE record_text MATCH ?1 ORDER BY score LIMIT ?2",
        )?;
        let found = records.query_map(params![query, limit as i64, SNIPPET_TOKENS], |row| {
            let hit = json!({"t": "hit", "kind": row.get::<_, String>(0)?,
                "id": row.get::<_, String>(1)?, "task": row.get::<_, Option<String>>(2)?,
                "session": null, "item": null, "snippet": row.get::<_, String>(3)?});
            Ok((row.get(4)?, hit))
        })?;
        hits.extend(found.collect::<rusqlite::Result<Vec<_>>>()?);
        let mut lines = self.db.prepare(
            "SELECT l.session, l.file, l.item, snippet(line_text, 3, '**', '**', '…', ?3),
                bm25(line_text) AS score,
                (SELECT json_extract(body, '$.task') FROM records WHERE id = l.session)
            FROM line_text l WHERE line_text MATCH ?1 ORDER BY score LIMIT ?2",
        )?;
        let found = lines.query_map(params![query, limit as i64, SNIPPET_TOKENS], |row| {
            let session: String = row.get(0)?;
            let hit = json!({"t": "hit", "kind": "transcript", "id": session,
                "task": row.get::<_, Option<String>>(5)?, "session": session,
                "file": row.get::<_, String>(1)?, "item": row.get::<_, String>(2)?,
                "snippet": row.get::<_, String>(3)?});
            Ok((row.get(4)?, hit))
        })?;
        hits.extend(found.collect::<rusqlite::Result<Vec<_>>>()?);
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(hits.into_iter().take(limit).map(|(_, hit)| hit).collect())
    }
}

const SELECT_RECORD: &str = "SELECT id, kind, body, seq, deleted FROM records";

fn record(row: &rusqlite::Row) -> rusqlite::Result<Record> {
    let body: String = row.get(2)?;
    Ok(Record {
        id: row.get(0)?,
        kind: row.get(1)?,
        body: serde_json::from_str(&body).unwrap_or_default(),
        seq: row.get(3)?,
        deleted: row.get(4)?,
    })
}

fn head(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row("SELECT COALESCE(MAX(seq), 0) FROM records", [], |row| {
        row.get(0)
    })
}

/// The Agent that wrote a Transcript, told by its first line; None until that line is whole.
fn agent_of(path: &Path) -> Result<Option<Agent>> {
    let mut line = Vec::new();
    std::io::BufReader::new(File::open(path)?).read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return Ok(None);
    }
    let first: Value = serde_json::from_slice(&line).unwrap_or_default();
    Ok(Some(Agent::wrote(&first)))
}

fn uuid() -> String {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .expect("/dev/urandom");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("vault-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn vault(&self) -> Vault {
            Vault::open(self.0.join("vault"), true).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn change(id: &str, kind: &str, base: i64, body: Value) -> Change {
        Change {
            id: id.into(),
            kind: kind.into(),
            body,
            base,
            deleted: false,
        }
    }

    fn claude_line(who: &str, text: &str) -> String {
        let line = match who {
            "user" => json!({"type": "user", "timestamp": "2026-10-07T10:00:00Z",
                "origin": {"kind": "human"}, "message": {"role": "user", "content": text}}),
            _ => json!({"type": "assistant", "timestamp": "2026-10-07T10:00:00Z",
                "message": {"content": [{"type": "text", "text": text}]}}),
        };
        format!("{line}\n")
    }

    #[test]
    fn pull_returns_what_push_wrote_after_a_seq() {
        let scratch = Scratch::new("pull");
        let mut vault = scratch.vault();
        assert_eq!(vault.pull(0).unwrap(), (Vec::new(), 0));
        let written = vault
            .push(vec![
                change("t1", "task", 0, json!({"title": "Fix the login test"})),
                change(
                    "e1",
                    "entry",
                    0,
                    json!({"task": "t1", "text": "Begun", "edited": 1}),
                ),
            ])
            .unwrap()
            .0;
        assert_eq!(
            written.iter().map(|record| record.seq).collect::<Vec<_>>(),
            [1, 2]
        );
        let (records, head) = vault.pull(1).unwrap();
        assert_eq!((records.len(), records[0].id.as_str(), head), (1, "e1", 2));
        assert_eq!(records[0].body["text"], "Begun");
        let reopened = Vault::open(scratch.0.join("vault"), false).unwrap();
        assert_eq!(reopened.head().unwrap(), 2);
        assert!(Vault::open(scratch.0.join("none"), false).is_err());
    }

    #[test]
    fn a_stale_entry_edit_keeps_the_later_one_and_the_other_as_a_conflict() {
        let scratch = Scratch::new("conflict");
        let mut vault = scratch.vault();
        let body = |text: &str, edited: i64| json!({"task": "t1", "text": text, "edited": edited});
        vault
            .push(vec![change("e1", "entry", 0, body("one", 1))])
            .unwrap();
        vault
            .push(vec![change("e1", "entry", 1, body("two", 20))])
            .unwrap();

        let older = vault
            .push(vec![change("e1", "entry", 1, body("three", 10))])
            .unwrap()
            .0;
        assert_eq!(older.len(), 1);
        assert_eq!(older[0].kind, "conflict");
        assert_eq!(
            older[0].body,
            json!({"of": "e1", "kind": "entry", "task": "t1", "title": null, "text": "three", "edited": 10})
        );

        let newer = vault
            .push(vec![change("e1", "entry", 1, body("four", 30))])
            .unwrap()
            .0;
        let kinds: Vec<&str> = newer.iter().map(|record| record.kind.as_str()).collect();
        assert_eq!(kinds, ["entry", "conflict"]);
        assert_eq!(newer[0].body["text"], "four");
        assert_eq!(newer[1].body["text"], "two");
        assert_ne!(newer[1].id, older[0].id);

        let task = vault
            .push(vec![change("t1", "task", 0, json!({"title": "A"}))])
            .unwrap()
            .0;
        vault
            .push(vec![change("t1", "task", 0, json!({"title": "B"}))])
            .unwrap();
        let (records, _) = vault.pull(task[0].seq).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].body["title"], "B");
    }

    #[test]
    fn a_deleted_record_stays_but_is_not_found() {
        let scratch = Scratch::new("delete");
        let mut vault = scratch.vault();
        let plan = json!({"title": "Plan", "text": "zebra", "edited": 1});
        vault.push(vec![change("d1", "document", 0, plan)]).unwrap();
        assert_eq!(vault.search("zebra", None).unwrap().len(), 1);
        let mut gone = change("d1", "document", 1, json!({}));
        gone.deleted = true;
        vault.push(vec![gone]).unwrap();
        let (records, head) = vault.pull(0).unwrap();
        assert_eq!((records.len(), records[0].deleted, head), (1, true, 2));
        assert!(vault.search("zebra", None).unwrap().is_empty());
    }

    #[test]
    fn append_needs_the_copys_exact_size() {
        let scratch = Scratch::new("append");
        let vault = scratch.vault();
        assert_eq!(vault.size("s1", "a.jsonl").unwrap(), 0);
        assert_eq!(vault.append("s1", "a.jsonl", 0, b"12345").unwrap(), 5);
        let err = vault.append("s1", "a.jsonl", 3, b"6").unwrap_err();
        assert!(err.0.contains("is 5 bytes long"), "{err}");
        assert_eq!(vault.size("s1", "a.jsonl").unwrap(), 5);
        assert!(vault.append("..", "a.jsonl", 0, b"").is_err());
        assert!(vault.size("s1", "../vault.db").is_err());
    }

    #[test]
    fn copy_follows_its_source_and_search_finds_records_and_transcript_lines() {
        let scratch = Scratch::new("copy");
        let mut vault = scratch.vault();
        let source = scratch.0.join("projects/abc.jsonl");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        let first = claude_line("user", "Why is the walrus test flaky?");
        let reply = claude_line("assistant", "The walrus fixture races the clock.");
        std::fs::write(&source, format!("{first}{}", &reply[..10])).unwrap();
        vault.copy("s1", &source).unwrap();
        assert!(vault.search("races", None).unwrap().is_empty());
        std::fs::write(&source, format!("{first}{reply}")).unwrap();
        let size = vault.copy("s1", &source).unwrap();
        assert_eq!(size, (first.len() + reply.len()) as u64);
        let copy = std::fs::read(scratch.0.join("vault/transcripts/s1/abc.jsonl")).unwrap();
        assert_eq!(copy, std::fs::read(&source).unwrap());

        vault
            .push(vec![
                change("s1", "session", 0, json!({"task": "t1"})),
                change("t1", "task", 0, json!({"title": "Walrus"})),
            ])
            .unwrap();
        let mut transcript = Transcript::new(Agent::Claude, &source);
        transcript.read(None).unwrap();
        let hits = vault.search("walrus", None).unwrap();
        assert_eq!(hits.len(), 3, "{hits:?}");
        assert_eq!(
            (&hits[0]["kind"], &hits[0]["task"]),
            (&json!("task"), &json!("t1"))
        );
        let lines: Vec<&Value> = hits
            .iter()
            .filter(|hit| hit["kind"] == "transcript")
            .collect();
        assert_eq!(lines.len(), 2);
        for hit in lines {
            assert_eq!(hit["session"], "s1");
            assert_eq!(hit["task"], "t1");
            assert_eq!(hit["file"], "abc.jsonl");
            assert!(transcript
                .entries
                .iter()
                .any(|entry| entry.id == hit["item"]));
            assert!(
                hit["snippet"].as_str().unwrap().contains("**walrus**"),
                "{hit}"
            );
        }
        assert_eq!(vault.search("walrus clock", None).unwrap().len(), 1);
        assert_eq!(vault.search("walrus", Some(1)).unwrap().len(), 1);
        assert!(vault.search("\"walrus OR (", None).unwrap().is_empty());
    }

    #[test]
    fn a_codex_copy_is_indexed_again_once_it_shows_completed_items() {
        let scratch = Scratch::new("codex");
        let vault = scratch.vault();
        let at = "2026-10-07T10:00:00Z";
        let event = |payload: Value| {
            format!(
                "{}\n",
                json!({"timestamp": at, "type": "event_msg", "payload": payload})
            )
        };
        let meta = format!(
            "{}\n",
            json!({"timestamp": at, "type": "session_meta", "payload": {}})
        );
        let asked = event(json!({"type": "user_message", "message": "Feed the walrus"}));
        let done = event(
            json!({"type": "item_completed", "item": {"type": "UserMessage", "id": "u1",
            "content": [{"type": "text", "text": "Feed the walrus"}]}}),
        );
        let source = scratch.0.join("rollout-1.jsonl");
        std::fs::write(&source, format!("{meta}{asked}")).unwrap();
        vault.copy("s1", &source).unwrap();
        assert_eq!(vault.search("walrus", None).unwrap().len(), 1);
        std::fs::write(&source, format!("{meta}{asked}{done}")).unwrap();
        vault.copy("s1", &source).unwrap();

        let hits = vault.search("walrus", None).unwrap();
        let mut transcript = Transcript::new(Agent::Codex, &source);
        transcript.read_tail(None, |_| false).unwrap();
        let ids: Vec<&str> = transcript
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(ids, [hits[0]["item"].as_str().unwrap()]);
    }
}
