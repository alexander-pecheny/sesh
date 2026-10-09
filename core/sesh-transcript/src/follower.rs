//! Follows one Agent session: its Transcript, its herdr pane and, if Sesh reads it, its screen,
//! as Conversation lines. `follow` prints them; the follower writes them to the Session log.

use std::hash::{Hash as _, Hasher as _};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use serde_json::{json, Value};

use crate::reconcile::{Reconciler, Shown};
use crate::screen::{Menu, View};
use crate::agent::{transcript_path, Agent};
use crate::{Entry, Transcript, PROTOCOL};

/// While a session's Transcript cannot be found, look again every this many polls.
const RETRY: u32 = 8;
/// How many entries a follower keeps parsed, for questions and matching.
const KEEP: usize = 500;

/// What the follower plays into a pane for a device (ADR 0012).
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// Key names, as herdr spells them.
    Keys(Vec<String>),
    /// Literal text, typed.
    Text(String),
    /// A message, which herdr submits as the Agent's next prompt.
    Prompt(String),
    Close,
}

/// What a follower asks of herdr: the panes, a pane's screen, and input for a pane. A
/// recording stands in for herdr when a session is replayed.
pub trait Source {
    fn panes(&mut self) -> Result<Vec<Value>, String>;
    fn read(&mut self, pane_id: &str, ansi: bool) -> Result<String, String>;
    fn input(&mut self, pane_id: &str, input: &Input) -> Result<(), String>;

    /// The recorder, when this Source records what it reads.
    fn recorder(&mut self) -> Option<&mut crate::serve::Recorder> {
        None
    }
}

/// herdr itself.
pub struct Herdr;

impl Source for Herdr {
    /// Every pane, with the Agent's name and finished turns, which only `agent list` gives.
    fn panes(&mut self) -> Result<Vec<Value>, String> {
        let read = |args: &[&str]| -> Result<Value, String> {
            serde_json::from_str(&herdr(args)?).map_err(|err| format!("herdr said something sesh-transcript cannot read: {err}"))
        };
        let mut panes = read(&["pane", "list"])?["result"]["panes"].as_array().cloned().unwrap_or_default();
        let agents = read(&["agent", "list"])?["result"]["agents"].as_array().cloned().unwrap_or_default();
        for pane in &mut panes {
            if let Some(agent) = agents.iter().find(|agent| agent["pane_id"] == pane["pane_id"]) {
                pane["name"] = agent["name"].clone();
                pane["completion_seq"] = agent["completion_seq"].clone();
            }
        }
        Ok(panes)
    }

    fn read(&mut self, pane_id: &str, ansi: bool) -> Result<String, String> {
        let format = if ansi { "ansi" } else { "text" };
        herdr(&["pane", "read", pane_id, "--source", "visible", "--format", format])
    }

    fn input(&mut self, pane_id: &str, input: &Input) -> Result<(), String> {
        match input {
            Input::Keys(keys) => herdr(&[&["pane", "send-keys", pane_id][..], &keys.iter().map(String::as_str).collect::<Vec<_>>()].concat()),
            Input::Text(text) => herdr(&["pane", "send-text", pane_id, text]),
            Input::Prompt(text) => herdr(&["agent", "prompt", pane_id, text]),
            Input::Close => herdr(&["pane", "close", pane_id]),
        }
        .map(drop)
    }
}


/// Runs `herdr` with `args` and returns its stdout, or the message it failed with.
pub fn herdr(args: &[&str]) -> Result<String, String> {
    let output = Command::new("herdr")
        .args(args)
        .output()
        .map_err(|err| format!("herdr: {err}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    let json: Option<Value> = serde_json::from_str(&err).ok();
    Err(json
        .and_then(|json| json["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| err.trim().to_string()))
}


/// What a follower tells of its session: the Writer puts it in the Session log, and `follow`
/// prints it as protocol 2 spells it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Hello { agent: Agent, transcript: String },
    /// A Transcript entry, naming the shown item it replaces.
    Entry { entry: Entry, replaces: Option<String> },
    Live(Shown),
    Switch { reason: String, transcript: String },
    State(String),
    Background(Value),
    Cursor(String),
    /// A `permission` line, as `permission` makes it.
    Permission(Value),
    PermissionDone(String),
    Ping,
}

impl Event {
    pub fn line(&self) -> Value {
        match self {
            Event::Hello { agent, transcript } => json!({"t": "hello", "protocol": PROTOCOL, "agent": agent.name(), "transcript": transcript}),
            Event::Entry { entry, replaces } => {
                let mut line = entry_line(entry);
                if let Some(id) = replaces {
                    line["replaces"] = id.clone().into();
                }
                line
            }
            Event::Live(shown) => json!({"t": "live", "items": shown.items, "status": shown.status}),
            Event::Switch { reason, transcript } => json!({"t": "switch", "reason": reason, "transcript": transcript}),
            Event::State(state) => json!({"t": "state", "state": state}),
            Event::Background(tasks) => json!({"t": "background", "tasks": tasks}),
            Event::Cursor(cursor) => json!({"t": "cursor", "cursor": cursor}),
            Event::Permission(line) => line.clone(),
            Event::PermissionDone(id) => json!({"t": "permission_done", "id": id}),
            Event::Ping => json!({"t": "ping"}),
        }
    }
}

pub struct Follower {
    agent: Agent,
    last: usize,
    state: Option<String>,
    /// The pending permission's id, and whether it was Claude's question menu.
    permission: Option<(String, bool)>,
    transcript: Option<Transcript>,
    /// herdr's `agent_session` when its Transcript was last looked for.
    session: Value,
    /// While the session's Transcript is not found, polls since it was last looked for.
    lost: Option<u32>,
    wrote: bool,
    /// The background work last reported, so only a change is sent.
    background: Value,
    /// What herdr last said the Agent was doing.
    reported: Option<String>,
    /// What the screen shows beyond the Transcript.
    pub(crate) live: Reconciler,
    /// What was last sent of it.
    sent: Option<Shown>,
    /// Events not yet passed on.
    out: Vec<Event>,
    /// The time of the current tick: the clock, or a recording's.
    now: Instant,
    source: Box<dyn Source>,
    /// The id of the menu shown that no hook reported.
    menu: Option<String>,
}

impl Follower {
    pub fn new(agent: Agent, last: usize, source: Box<dyn Source>) -> Self {
        Follower {
            out: Vec::new(),
            now: Instant::now(),
            source,
            agent,
            last,
            state: None,
            permission: None,
            transcript: None,
            session: Value::Null,
            lost: None,
            wrote: false,
            background: json!([]),
            reported: None,
            live: Reconciler::default(),
            sent: None,
            menu: None,
        }
    }

    pub fn emit(&mut self, event: Event) -> Result<(), String> {
        self.out.push(event);
        self.wrote = true;
        Ok(())
    }

    /// The Agent and the Transcript followed now, for reading what came before it.
    pub fn transcript(&self) -> Option<(Agent, PathBuf)> {
        self.transcript.as_ref().map(|transcript| (self.agent, transcript.path.clone()))
    }

    pub fn agent(&self) -> Agent {
        self.agent
    }

    /// What the Agent is doing, as the follower last told it.
    pub fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }

    /// The events since the last call, for whoever passes them on.
    pub fn drain(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.out)
    }

    /// Carries on the provisional items a follower before this one left, from `now`.
    pub fn adopt(&mut self, items: &[Entry], now: Instant) {
        self.now = now;
        self.live.adopt(items, now);
    }

    pub fn start(&mut self, pane: &Value, since: Option<&str>) -> Result<(), String> {
        self.session = pane["agent_session"].clone();
        let found = transcript_path(pane);
        let path = found.as_deref();
        self.lost = path.is_none().then_some(1);
        self.emit(Event::Hello { agent: self.agent, transcript: path.unwrap_or_default().to_string() })?;
        if let Some(path) = path {
            let cursor = since.and_then(|cursor| cursor.split_once(':'));
            let mut transcript = Transcript::new(self.agent, path);
            match cursor.and_then(|(offset, cursor_path)| Some((offset.parse().ok()?, cursor_path)))
            {
                Some((offset, cursor_path)) if cursor_path == path => {
                    transcript
                        .read_tail(Some(offset), |entries| entries.len() >= self.last)
                        .map_err(|err| err.to_string())?;
                    transcript.scan_background().map_err(|err| err.to_string())?;
                    slim(&mut transcript.entries);
                    transcript.entries.iter().for_each(|entry| self.live.learn(entry));
                    self.transcript = Some(transcript);
                    self.read_new()?;
                }
                Some((_, cursor_path)) if !cursor_path.is_empty() => {
                    self.emit(switch(pane, path))?;
                    self.show(transcript)?;
                }
                _ => self.show(transcript)?,
            }
        }
        self.update_status(pane)?;
        self.update_live(pane)?;
        self.emit_cursor()
    }

    pub fn tick(&mut self, pane: &Value, now: Instant) -> Result<(), String> {
        self.now = now;
        self.wrote = false;
        self.update_status(pane)?;
        match self.moved(pane) {
            Some(path) => {
                if let Some(agent) = Agent::of(pane) {
                    self.agent = agent;
                }
                if self.transcript.is_some() {
                    self.emit(switch(pane, &path))?;
                }
                let transcript = Transcript::new(self.agent, path);
                self.show(transcript)?;
            }
            None => self.read_new()?,
        }
        self.update_live(pane)?;
        self.update_menu(pane)?;
        if self.wrote {
            self.emit_cursor()?;
        }
        Ok(())
    }

    /// An Agent blocked with no hook to say why shows a menu of its own; it is read off the screen.
    fn update_menu(&mut self, pane: &Value) -> Result<(), String> {
        let unexplained = self.agent.reads_screen() && self.reported.as_deref() == Some("blocked") && self.permission.is_none();
        let menu = match (unexplained, pane["pane_id"].as_str()) {
            (true, Some(pane_id)) => self.source.read(pane_id, false).ok().and_then(|screen| Menu::read(&screen)),
            _ => None,
        };
        let id = menu.as_ref().map(|menu| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            format!("{menu:?}").hash(&mut hasher);
            format!("menu.{:016x}", hasher.finish())
        });
        if id == self.menu {
            return Ok(());
        }
        if let Some(done) = self.menu.take() {
            self.emit(Event::PermissionDone(done))?;
        }
        let (Some(id), Some(menu)) = (id, menu) else {
            return Ok(());
        };
        let options: Vec<Value> = menu.options.iter().map(|(key, label)| json!({"key": key, "label": label})).collect();
        self.emit(Event::Permission(json!({"t": "permission", "id": id, "tool": "menu", "summary": menu.title, "options": options})))?;
        self.menu = Some(id);
        Ok(())
    }

    /// The Transcript the session has moved to. One is looked for when herdr's session
    /// changes, then every few polls until found, since Claude writes a session's file only
    /// once it is first prompted.
    fn moved(&mut self, pane: &Value) -> Option<String> {
        if pane["agent_session"] != self.session {
            self.session = pane["agent_session"].clone();
            self.lost = Some(0);
        }
        if let Some(polls @ 1..) = &mut self.lost {
            *polls = (*polls + 1) % RETRY;
        }
        if self.lost != Some(0) {
            return None;
        }
        let Some(path) = transcript_path(pane) else {
            self.lost = Some(1);
            return None;
        };
        self.lost = None;
        let current = self
            .transcript
            .as_ref()
            .map(|transcript| transcript.path.to_string_lossy());
        (current.as_deref() != Some(path.as_str())).then_some(path)
    }

    /// Takes over `transcript` and sends its last entries.
    fn show(&mut self, mut transcript: Transcript) -> Result<(), String> {
        transcript
            .read_tail(None, |entries| entries.len() >= self.last)
            .map_err(|err| err.to_string())?;
        transcript.scan_background().map_err(|err| err.to_string())?;
        slim(&mut transcript.entries);
        let entries = transcript.entries.clone();
        self.transcript = Some(transcript);
        let start = entries.len().saturating_sub(self.last);
        self.send(&entries[..start], &entries[start..])
    }

    fn read_new(&mut self) -> Result<(), String> {
        let Some(mut transcript) = self.transcript.take() else {
            return Ok(());
        };
        // A follower runs for days; what it sent is in the Session log, not needed here.
        if transcript.entries.len() > 2 * KEEP {
            let drop = transcript.entries.len() - KEEP;
            transcript.entries.drain(..drop);
        }
        let mark = transcript.entries.len();
        if transcript.read(None).map_err(|err| err.to_string())? {
            let path = transcript.path.to_string_lossy().into_owned();
            self.emit(Event::Switch { reason: "fork".into(), transcript: path })?;
            let entries = transcript.entries.clone();
            self.transcript = Some(transcript);
            let start = entries.len().saturating_sub(self.last);
            return self.send(&entries[..start], &entries[start..]);
        }
        slim(&mut transcript.entries[mark..]);
        let entries = transcript.entries[mark..].to_vec();
        self.transcript = Some(transcript);
        self.send(&[], &entries)
    }

    /// Sends `entries`, each naming the live item it replaces; `known` only tells the live
    /// items what the Transcript holds.
    fn send(&mut self, known: &[Entry], entries: &[Entry]) -> Result<(), String> {
        let now = self.now;
        for entry in known {
            self.live.deliver(entry, now);
        }
        for entry in entries {
            let replaces = self.live.deliver(entry, now);
            self.emit(Event::Entry { entry: entry.clone(), replaces })?;
        }
        Ok(())
    }

    /// What the Agent's screen shows beyond the Transcript, read at every poll while it works
    /// and a little after.
    fn update_live(&mut self, pane: &Value) -> Result<(), String> {
        let now = self.now;
        let screen = self.agent.reads_screen();
        let busy = matches!(self.reported.as_deref(), Some("working" | "blocked"));
        self.live.working(screen && busy, now);
        self.live.idle(self.reported.is_some() && !busy, now);
        if let (true, true, Some(pane_id)) = (screen, self.live.watching(now), pane["pane_id"].as_str()) {
            if let Some(view) = settled(self.source.as_mut(), pane_id, &self.live) {
                self.live.see(view, now);
            }
        }
        self.emit_live()
    }

    /// Tells what the live items now are, if they changed.
    pub fn emit_live(&mut self) -> Result<(), String> {
        let shown = self.live.shown(self.now);
        if self.sent.as_ref() == Some(&shown) {
            return Ok(());
        }
        self.sent = Some(shown.clone());
        let wrote = self.wrote;
        self.emit(Event::Live(shown))?;
        self.wrote = wrote;
        Ok(())
    }

    /// Permission prompts come only from the fork, whose hooks report them on the pane.
    fn update_status(&mut self, pane: &Value) -> Result<(), String> {
        let reported = pane["agent_status"]
            .as_str()
            .filter(|state| matches!(*state, "idle" | "working" | "blocked" | "done"));
        if let Some(reported) = reported {
            self.reported = Some(reported.to_string());
        }
        self.emit_state()?;
        let pending = &pane["permission"];
        let id = pending["id"].as_str();
        if id == self.permission.as_ref().map(|(id, _)| id.as_str()) {
            return Ok(());
        }
        if let Some((done, false)) = self.permission.take() {
            self.emit(Event::PermissionDone(done))?;
        }
        let Some(id) = id else {
            return Ok(());
        };
        let tool = pending["tool"].as_str().unwrap_or_default();
        let question = tool == "AskUserQuestion";
        self.permission = Some((id.to_string(), question));
        let event = match &self.transcript {
            Some(transcript) if question => Event::Entry { entry: transcript.open_question(&pending["input"]), replaces: None },
            _ if question => return Ok(()),
            _ => Event::Permission(self.agent.permission(id, tool, &pending["input"])),
        };
        self.emit(event)
    }

    /// herdr's state, except that an Agent with its turn over is only waiting on background
    /// work, and can take a message, though herdr's screen rules call that working.
    fn emit_state(&mut self) -> Result<(), String> {
        let Some(reported) = self.reported.clone() else {
            return Ok(());
        };
        let waiting = self.transcript.as_ref().is_some_and(Transcript::waiting);
        let state = if reported == "working" && waiting { "background".to_string() } else { reported };
        if self.state.as_deref() != Some(state.as_str()) {
            self.emit(Event::State(state.clone()))?;
            self.state = Some(state);
        }
        Ok(())
    }

    fn emit_cursor(&mut self) -> Result<(), String> {
        self.emit_state()?;
        let background: Value = self
            .transcript
            .as_ref()
            .map(|transcript| {
                transcript
                    .background()
                    .iter()
                    .map(|work| json!({"call": work.call, "label": work.label, "agent": work.agent}))
                    .collect()
            })
            .unwrap_or_default();
        if background != self.background {
            self.background = background.clone();
            self.emit(Event::Background(background))?;
        }
        let cursor = match &self.transcript {
            Some(transcript) => format!("{}:{}", transcript.offset, transcript.path.display()),
            None => "0:".to_string(),
        };
        self.emit(Event::Cursor(cursor))
    }
}

/// The screen, once two reads agree: a read can land in the middle of Claude's redraw.
fn settled(source: &mut dyn Source, pane_id: &str, live: &Reconciler) -> Option<View> {
    let mut read = || View::read(&source.read(pane_id, true).ok()?);
    let mut view = read()?;
    if !live.changed(&view) {
        return Some(view);
    }
    for _ in 0..2 {
        let again = read()?;
        if again.agrees(&view) {
            return Some(again);
        }
        view = again;
    }
    None
}

/// Results kept as they are sent: a follower holds hundreds of entries for days.
fn slim(entries: &mut [Entry]) {
    for entry in entries.iter_mut().filter(|entry| entry.kind == "result") {
        *entry = entry.clipped();
    }
}

pub fn entry_line(entry: &Entry) -> Value {
    let mut line = serde_json::to_value(entry.clipped()).unwrap_or_default();
    line["t"] = "entry".into();
    line
}

fn switch(pane: &Value, path: &str) -> Event {
    let reason = match pane["agent_session"]["start"].as_str() {
        Some(reason @ ("clear" | "resume" | "compact")) => reason,
        Some("startup" | "new") => "new",
        Some("fork" | "branch") => "fork",
        _ => "other",
    };
    Event::Switch { reason: reason.into(), transcript: path.into() }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_the_transcript_held_before_a_resumed_follow_never_shows_live() {
        let reply = "| Function | Operation |\n|---|---|\n| `add(a, b)` | Returns the sum of a and b |\n| `mul(a, b)` | Returns the product of a and b |\n| `sub(a, b)` | Returns a minus b |\n\nThe file now holds three functions, with `sub` added at the end.";
        let at = "2026-10-05T14:54:42.186Z";
        let lines = [
            json!({"type":"user","timestamp":at,"origin":{"kind":"human"},"message":{"role":"user","content":"Tabulate calc.py"}}),
            json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"text","text":reply}]}}),
        ];
        let dir = std::env::temp_dir().join(format!("herdr-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        let bytes: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::write(&path, &bytes).unwrap();
        let pane = json!({"agent": "claude", "agent_session": {"kind": "path", "value": path}});
        let mut follower = Follower::new(Agent::Claude, 20, Box::new(Herdr));
        follower.start(&pane, Some(&format!("{}:{}", bytes.len(), path.display()))).unwrap();
        let screen = std::fs::read_to_string(format!("{}/tests/screens/table.ansi", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let now = Instant::now();
        follower.live.see(View::read(&screen).unwrap(), now);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(follower.live.shown(now).items, []);
    }
}
