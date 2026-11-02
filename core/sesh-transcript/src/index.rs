//! Every Transcript the Agents left on this machine, adopted or not, indexed for search in
//! `~/.sesh/transcripts.db` and brought up to date before each search.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::vault::Result;
use crate::{Agent, Transcript};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    session TEXT NOT NULL,
    cwd TEXT,
    title TEXT,
    changed INTEGER NOT NULL,
    indexed INTEGER NOT NULL,
    hint INTEGER NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS lines USING fts5(path UNINDEXED, item UNINDEXED, at UNINDEXED, text);
";
const DEFAULT_LIMIT: usize = 20;
const SNIPPET_TOKENS: i32 = 12;
/// Lines read from a Transcript's start for its folder and id, which Agents write first.
const HEAD_LINES: usize = 20;

pub struct Index {
    db: Connection,
    home: PathBuf,
}

impl Index {
    /// The index of the Transcripts under `home`, kept in `home/.sesh`.
    pub fn open(home: impl Into<PathBuf>) -> Result<Self> {
        let home = home.into();
        std::fs::create_dir_all(home.join(".sesh"))?;
        let db = Connection::open(home.join(".sesh/transcripts.db"))?;
        db.busy_timeout(std::time::Duration::from_secs(30))?;
        db.pragma_update(None, "journal_mode", "wal")?;
        db.execute_batch(SCHEMA)?;
        Ok(Self { db, home })
    }

    /// Indexes what each Transcript gained since the last call and forgets deleted ones.
    pub fn refresh(&mut self) -> Result<()> {
        let found = sources(&self.home);
        let known: Vec<String> = self
            .db
            .prepare("SELECT path FROM files")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for path in known {
            if !found.iter().any(|(_, file)| file.to_string_lossy() == path) {
                let forget = self.db.unchecked_transaction()?;
                forget.execute("DELETE FROM lines WHERE path = ?1", [&path])?;
                forget.execute("DELETE FROM files WHERE path = ?1", [&path])?;
                forget.commit()?;
            }
        }
        for (agent, file) in found {
            self.update(agent, &file)?;
        }
        Ok(())
    }

    fn update(&mut self, agent: Agent, file: &Path) -> Result<()> {
        let Ok(metadata) = std::fs::metadata(file) else {
            return Ok(());
        };
        let size = metadata.len() as i64;
        let changed = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_millis() as i64);
        let path = file.to_string_lossy();
        // Read and written under one lock, so two searches never index the same lines twice.
        let write = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let known: Option<(i64, i64, Option<String>)> = write
            .query_row(
                "SELECT indexed, hint, title FROM files WHERE path = ?1",
                [&path],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (indexed, hint, mut title) = match known {
            Some((indexed, _, _)) if indexed == size => return Ok(()),
            // A Transcript only grows; a shorter one was written anew.
            Some((indexed, hint, title)) if indexed < size => (indexed, hint, title),
            _ => (0, 0, None),
        };
        let mut transcript = Transcript::new(agent, file);
        let entries = transcript.read_from(indexed as u64, hint)?;
        if transcript.start == 0 {
            write.execute("DELETE FROM lines WHERE path = ?1", [&path])?;
            title = None;
        }
        if let Some(named) = named(file, transcript.start)? {
            title = Some(named);
        }
        for entry in &entries {
            if let ("user" | "text", Some(text)) = (entry.kind, &entry.text) {
                if title.is_none() && entry.kind == "user" {
                    title = Some(crate::first_line(text));
                }
                write.execute(
                    "INSERT INTO lines (path, item, at, text) VALUES (?1, ?2, ?3, ?4)",
                    params![path, entry.id, entry.at, text],
                )?;
            }
        }
        let (session, cwd) = head(agent, file)?;
        write.execute(
            "INSERT OR REPLACE INTO files (path, agent, session, cwd, title, changed, indexed, hint)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                path,
                agent.name(),
                session,
                cwd,
                title,
                changed,
                transcript.offset as i64,
                transcript.hint()
            ],
        )?;
        write.commit()?;
        Ok(())
    }

    /// The Transcripts that hold every word of `query`, best first, each once with its best line.
    pub fn search(&self, query: &str, limit: Option<usize>) -> Result<Vec<Value>> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        let Some(query) = crate::vault::match_words(query) else {
            return Ok(Vec::new());
        };
        let mut lines = self.db.prepare(
            "SELECT l.path, l.item, l.at, snippet(lines, 3, '**', '**', '…', ?2),
                f.agent, f.session, f.cwd, f.title, f.changed
            FROM lines l JOIN files f ON f.path = l.path
            WHERE lines MATCH ?1 ORDER BY bm25(lines)",
        )?;
        let mut rows = lines.query(params![query, SNIPPET_TOKENS])?;
        let mut hits: Vec<Value> = Vec::new();
        while let Some(row) = rows.next()? {
            let path: String = row.get(0)?;
            if hits.iter().any(|hit| hit["path"] == path.as_str()) {
                continue;
            }
            hits.push(json!({"t": "hit", "kind": "found", "id": path, "path": path,
                "item": row.get::<_, String>(1)?, "at": row.get::<_, String>(2)?,
                "snippet": row.get::<_, String>(3)?, "agent": row.get::<_, String>(4)?,
                "session": row.get::<_, String>(5)?, "cwd": row.get::<_, Option<String>>(6)?,
                "title": row.get::<_, Option<String>>(7)?, "changed": row.get::<_, i64>(8)?}));
            if hits.len() == limit {
                break;
            }
        }
        Ok(hits)
    }
}

/// Every Transcript the Agents keep under `home`: Claude's in each `~/.claude*` folder, as one
/// person may keep a second account in `~/.claude-personal`, then Codex's and pi's.
fn sources(home: &Path) -> Vec<(Agent, PathBuf)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(home).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(".claude") {
            files(&entry.path().join("projects"), 1, &mut |path| found.push((Agent::Claude, path)));
        }
    }
    files(&home.join(".codex/sessions"), 3, &mut |path| found.push((Agent::Codex, path)));
    files(&home.join(".pi/agent/sessions"), 1, &mut |path| found.push((Agent::Pi, path)));
    found
}

/// The `.jsonl` files exactly `depth` folders below `dir`, so Claude's subagents, one deeper, stay out.
fn files(dir: &Path, depth: usize, found: &mut dyn FnMut(PathBuf)) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if depth > 0 && path.is_dir() {
            files(&path, depth - 1, found);
        } else if depth == 0 && path.extension().is_some_and(|ext| ext == "jsonl") {
            found(path);
        }
    }
}

/// The session id and folder the Agent wrote near the Transcript's start; Claude's id is its
/// file's name.
pub(crate) fn head(agent: Agent, file: &Path) -> Result<(String, Option<String>)> {
    let stem = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut session = match agent {
        Agent::Codex => stem.get(stem.len().saturating_sub(36)..).unwrap_or(&stem).to_string(),
        _ => stem,
    };
    let mut cwd = None;
    for line in BufReader::new(File::open(file)?).lines().take(HEAD_LINES) {
        let line: Value = serde_json::from_str(&line?).unwrap_or_default();
        let meta = if agent == Agent::Codex { &line["payload"] } else { &line };
        if agent == Agent::Pi && line["type"] == "session" {
            session = meta["id"].as_str().unwrap_or(&session).to_string();
        }
        if let Some(folder) = meta["cwd"].as_str() {
            cwd = Some(folder.to_string());
            break;
        }
    }
    Ok((session, cwd))
}

/// The newest title Claude gave the conversation in the lines from `start` on.
fn named(file: &Path, start: u64) -> Result<Option<String>> {
    let mut reader = File::open(file)?;
    reader.seek(SeekFrom::Start(start))?;
    let mut title = None;
    for line in BufReader::new(reader.take(u64::MAX)).split(b'\n') {
        let line = line?;
        if !line.windows(10).any(|window| window == b"\"ai-title\"") {
            continue;
        }
        let line: Value = serde_json::from_slice(&line).unwrap_or_default();
        if let Some(named) = line["aiTitle"].as_str() {
            title = Some(named.to_string());
        }
    }
    Ok(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("index-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn write(&self, path: &str, text: &str) -> PathBuf {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn claude(who: &str, text: &str) -> String {
        let line = match who {
            "user" => json!({"type": "user", "timestamp": "2026-11-02T10:00:00Z", "cwd": "/work/zoo",
                "origin": {"kind": "human"}, "message": {"role": "user", "content": text}}),
            "title" => json!({"type": "ai-title", "aiTitle": text}),
            _ => json!({"type": "assistant", "timestamp": "2026-11-02T10:00:00Z",
                "message": {"content": [{"type": "text", "text": text}]}}),
        };
        format!("{line}\n")
    }

    #[test]
    fn finds_each_transcript_once_in_every_claude_folder_and_codex() {
        let home = Home::new("find");
        let asked = claude("user", "Why does the walrus test fail?");
        home.write(".claude/projects/-work-zoo/a1.jsonl", &asked);
        home.write(".claude-personal/projects/-work-zoo/b2.jsonl", &format!("{asked}{}", claude("assistant", "The walrus races the clock.")));
        home.write(".claude/projects/-work-zoo/a1/subagents/agent-1.jsonl", &asked);
        let at = "2026-11-02T10:00:00Z";
        let codex = [
            json!({"timestamp": at, "type": "session_meta", "payload": {"id": "c3", "cwd": "/work/pond"}}),
            json!({"timestamp": at, "type": "event_msg", "payload": {"type": "user_message", "message": "Feed the walrus"}}),
        ];
        let rollout = "rollout-2026-11-02T10-00-00-0199aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee.jsonl";
        home.write(&format!(".codex/sessions/2026/11/02/{rollout}"), &codex.map(|line| format!("{line}\n")).concat());

        let mut index = Index::open(&home.0).unwrap();
        index.refresh().unwrap();
        let hits = index.search("walrus", None).unwrap();
        assert_eq!(hits.len(), 3, "{hits:?}");
        let b2 = hits.iter().find(|hit| hit["session"] == "b2").unwrap();
        assert_eq!((&b2["agent"], &b2["cwd"]), (&json!("claude"), &json!("/work/zoo")));
        assert_eq!(b2["title"], "Why does the walrus test fail?");
        let codex = hits.iter().find(|hit| hit["agent"] == "codex").unwrap();
        assert_eq!((&codex["session"], &codex["cwd"]), (&json!("0199aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee"), &json!("/work/pond")));
        assert!(codex["snippet"].as_str().unwrap().contains("**walrus**"));
        assert_eq!(index.search("walrus clock", None).unwrap().len(), 1);
    }

    #[test]
    fn indexes_what_was_appended_and_forgets_deleted_files() {
        let home = Home::new("grow");
        let first = claude("user", "Paint the fence");
        let file = home.write(".claude/projects/-work-zoo/a1.jsonl", &first);
        let mut index = Index::open(&home.0).unwrap();
        index.refresh().unwrap();
        assert!(index.search("zebra", None).unwrap().is_empty());
        std::fs::write(&file, format!("{first}{}{}", claude("assistant", "Painted it zebra stripes."), claude("title", "Fence painting"))).unwrap();
        index.refresh().unwrap();
        let hits = index.search("zebra", None).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["title"], "Fence painting");
        assert_eq!(index.search("fence", None).unwrap().len(), 1);
        std::fs::remove_file(&file).unwrap();
        index.refresh().unwrap();
        assert!(index.search("zebra", None).unwrap().is_empty());
    }
}
