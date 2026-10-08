//! Replays a recording through the same Machine the follower runs, on the recording's own
//! clock, and reports the Session log it ends with and any item it ever showed twice.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::follower::Source;
use crate::log::Log;
use crate::serve::{Machine, Shared};

type Result<T> = std::result::Result<T, String>;

/// herdr as recorded: each pane's screen as it last was at the replay's time.
struct Recorded {
    screens: HashMap<(String, bool), Vec<(u64, String)>>,
    now: Rc<RefCell<u64>>,
}

impl Source for Recorded {
    fn panes(&mut self) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }

    fn read(&mut self, pane_id: &str, ansi: bool) -> Result<String> {
        let now = *self.now.borrow();
        self.screens
            .get(&(pane_id.to_string(), ansi))
            .and_then(|screens| screens.iter().rev().find(|(at, _)| *at <= now))
            .map(|(_, screen)| screen.clone())
            .ok_or_else(|| "no screen recorded yet".to_string())
    }
}

pub struct Outcome {
    /// Each session's items at the end, in order, as `kind: text`.
    pub items: HashMap<String, Vec<String>>,
    /// Every moment a message showed twice at once.
    pub doubles: Vec<String>,
}

pub fn replay(recording: &Path, work: &Path) -> Result<Outcome> {
    let text = std::fs::read_to_string(recording).map_err(|err| format!("{}: {err}", recording.display()))?;
    let events: Vec<Value> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    std::fs::create_dir_all(work).map_err(|err| err.to_string())?;
    let mut screens: HashMap<(String, bool), Vec<(u64, String)>> = HashMap::new();
    for event in &events {
        if let (Some(pane), Some(screen)) = (event["read"].as_str(), event["screen"].as_str()) {
            let key = (pane.to_string(), event["ansi"].as_bool().unwrap_or(false));
            screens.entry(key).or_default().push((event["at"].as_u64().unwrap_or(0), screen.to_string()));
        }
    }
    let now = Rc::new(RefCell::new(0u64));
    let source: Rc<RefCell<dyn Source>> = Rc::new(RefCell::new(Recorded { screens, now: now.clone() }));
    let db = work.join("sessions.db");
    let _ = std::fs::remove_file(&db);
    let mut machine = Machine::new(Log::open(&db)?, Shared(source));
    let mut moved: HashMap<String, PathBuf> = HashMap::new();
    let start = Instant::now();
    let mut doubles = Vec::new();
    for event in &events {
        let at = event["at"].as_u64().unwrap_or(0);
        *now.borrow_mut() = at;
        if let (Some(path), Some(lines)) = (event["transcript"].as_str(), event["lines"].as_str()) {
            let local = local(&mut moved, work, path);
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&local).map_err(|err| err.to_string())?;
            file.write_all(lines.as_bytes()).map_err(|err| err.to_string())?;
        }
        let Some(panes) = event["panes"].as_array() else { continue };
        let panes: Vec<Value> = panes.iter().map(|pane| relocate(pane, &mut moved, work)).collect();
        machine.tick(&panes, start + Duration::from_millis(at))?;
        for (session, shown) in shown(&machine.log, &panes)? {
            let mut seen = std::collections::HashSet::new();
            for text in shown.iter().filter(|text| text.starts_with("user:") || text.starts_with("text:")) {
                if !seen.insert(text) {
                    doubles.push(format!("{session} at {at} ms: {text}"));
                }
            }
        }
    }
    let panes: Vec<Value> = events.iter().filter_map(|event| event["panes"].as_array()).flatten().cloned().collect();
    Ok(Outcome { items: shown(&machine.log, &panes)?.into_iter().collect(), doubles })
}

/// Each session's items as `kind: text`, in order.
fn shown(log: &Log, panes: &[Value]) -> Result<Vec<(String, Vec<String>)>> {
    let mut sessions: Vec<String> = panes.iter().filter_map(|pane| pane["pane_id"].as_str().map(str::to_string)).collect();
    sessions.sort();
    sessions.dedup();
    sessions
        .into_iter()
        .map(|session| {
            let items = log.last(&session, usize::MAX)?;
            let texts = items
                .iter()
                .map(|item| {
                    let entry = &item["entry"];
                    let text = entry["text"].as_str().or(entry["command"].as_str()).or(entry["summary"].as_str()).unwrap_or_default();
                    format!("{}: {}", entry["kind"].as_str().unwrap_or("item"), text.trim())
                })
                .collect();
            Ok((session, texts))
        })
        .collect()
}

fn local(moved: &mut HashMap<String, PathBuf>, work: &Path, path: &str) -> PathBuf {
    let count = moved.len();
    moved
        .entry(path.to_string())
        .or_insert_with(|| {
            let folder = work.join(format!("t{count}"));
            let _ = std::fs::create_dir_all(&folder);
            folder.join(Path::new(path).file_name().unwrap_or_default())
        })
        .clone()
}

/// The pane as recorded, its Transcript moved into the replay's folder.
fn relocate(pane: &Value, moved: &mut HashMap<String, PathBuf>, work: &Path) -> Value {
    let mut pane = pane.clone();
    let session = &pane["agent_session"];
    let path = session["path"].as_str().or((session["kind"] == "path").then(|| session["value"].as_str()).flatten());
    if let Some(path) = path.map(str::to_string) {
        let local = local(moved, work, &path).to_string_lossy().into_owned();
        pane["agent_session"]["path"] = local.clone().into();
        if pane["agent_session"]["kind"] == "path" {
            pane["agent_session"]["value"] = local.into();
        }
    }
    pane
}

/// `sesh-transcript replay FILE`: the items each session ends with, then any doubles.
pub fn print(recording: &Path) -> Result<i32> {
    let work = std::env::temp_dir().join(format!("sesh-replay-{}", std::process::id()));
    let outcome = replay(recording, &work)?;
    let _ = std::fs::remove_dir_all(&work);
    let mut sessions: Vec<_> = outcome.items.into_iter().collect();
    sessions.sort();
    for (session, items) in sessions {
        println!("{}", json!({"session": session, "items": items}));
    }
    for double in &outcome.doubles {
        println!("{}", json!({"double": double}));
    }
    Ok(if outcome.doubles.is_empty() { 0 } else { 1 })
}
