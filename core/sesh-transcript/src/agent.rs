//! What sets Claude, Codex and pi apart: how each writes its Transcript and where, what of its
//! screen Sesh reads, and the keys that answer its menus.

pub(crate) mod claude;
mod codex;
mod pi;

use std::fs::File;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::{Background, Entry, Ids};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Pi,
}

impl Agent {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "pi" => Some(Self::Pi),
            _ => None,
        }
    }

    /// The Agent herdr says runs in `pane`, if it is one Sesh shows.
    pub fn of(pane: &Value) -> Option<Self> {
        Self::named(pane["agent"].as_str()?)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }

    /// The Agent that wrote a Transcript starting with `first`.
    pub fn wrote(first: &Value) -> Self {
        match first["type"].as_str() {
            Some("session_meta") => Self::Codex,
            Some("session") => Self::Pi,
            _ => Self::Claude,
        }
    }

    /// The Transcript of session `id`, which Claude and Codex keep under their own folders.
    pub fn locate(self, id: &str) -> Option<PathBuf> {
        self.find(&self.folder()?, id)
    }

    fn folder(self) -> Option<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let dir = |var: &str, default: &str| std::env::var_os(var).map_or_else(|| home.join(default), PathBuf::from);
        match self {
            Self::Claude => Some(dir("CLAUDE_CONFIG_DIR", ".claude").join("projects")),
            Self::Codex => Some(dir("CODEX_HOME", ".codex").join("sessions")),
            Self::Pi => None,
        }
    }

    fn find(self, folder: &Path, id: &str) -> Option<PathBuf> {
        let name = format!("{id}.jsonl");
        match self {
            Self::Claude => find_file(folder, 1, &|file| file == name),
            _ => find_file(folder, 3, &|file| file.starts_with("rollout-") && file.ends_with(&format!("-{name}"))),
        }
    }

    /// Whether Sesh reads the Agent's screen for what its Transcript does not hold yet: only
    /// Claude's is understood.
    pub fn reads_screen(self) -> bool {
        self == Self::Claude
    }

    /// The key that allows or denies in the Agent's permission menu, and what its screen shows
    /// while that menu is open.
    pub fn permit(self, allow: bool) -> Option<(&'static str, &'static str)> {
        let (key, open) = match self {
            Self::Claude => ("1", "Esc to cancel · Tab to amend"),
            Self::Codex => ("y", "Press enter to confirm or esc to cancel"),
            Self::Pi => return None,
        };
        Some((if allow { key } else { "esc" }, open))
    }

    /// What the screen shows while the Agent's question menu is open, and on its last page,
    /// which asks to submit the answers; only Claude asks questions.
    pub fn question_menu(self) -> Option<(&'static str, &'static str)> {
        (self == Self::Claude).then_some(("Enter to select ·", "Ready to submit your answers?"))
    }

    /// The `permission` line for a prompt the Agent's hook reported; only Codex gives a reason.
    pub fn permission(self, id: &str, tool: &str, input: &Value) -> Value {
        let entry = Entry::tool(String::new(), &Value::Null, tool, input);
        let reason = entry.description.filter(|_| self == Self::Codex);
        let mut line = json!({"t": "permission", "id": id, "tool": tool, "summary": entry.summary});
        for (key, value) in [("command", entry.command), ("file", entry.file), ("reason", reason)] {
            if let Some(value) = value {
                line[key] = value.into();
            }
        }
        line
    }

    pub(crate) fn parser(self, path: &Path) -> Box<dyn Parse> {
        match self {
            Self::Claude => Box::new(claude::Parser::new(path.parent().is_some_and(|folder| folder.ends_with("subagents")))),
            Self::Codex => Box::<codex::Parser>::default(),
            Self::Pi => Box::<pi::Parser>::default(),
        }
    }
}

/// The path herdr reports: the fork's `path`, or stock herdr's value of kind `path` (pi).
fn reported_path(pane: &Value) -> Option<&str> {
    let session = &pane["agent_session"];
    let value = (session["kind"] == "path").then(|| session["value"].as_str()).flatten();
    session["path"].as_str().or(value).filter(|path| !path.is_empty())
}

/// The Transcript of the session in `pane`. Claude and Codex report only their session id to
/// stock herdr, so the file is found by it.
pub fn transcript_path(pane: &Value) -> Option<String> {
    if let Some(path) = reported_path(pane) {
        return Some(path.to_string());
    }
    let session = &pane["agent_session"];
    // A herdr that names no session gets no guess: the newest Transcript in the pane's
    // folder is as often an older session's as the one running.
    if session["kind"] != "id" {
        return None;
    }
    let agent = Agent::named(session["agent"].as_str()?)?;
    Some(agent.locate(session["value"].as_str()?)?.to_string_lossy().into_owned())
}

fn find_file(dir: &Path, depth: usize, matches: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if depth == 0 {
            if entry.file_name().to_str().is_some_and(matches) {
                return Some(path);
            }
        } else if path.is_dir() {
            if let Some(found) = find_file(&path, depth - 1, matches) {
                return Some(found);
            }
        }
    }
    None
}

/// One Agent's reading of its Transcript, line by line.
pub(crate) trait Parse {
    /// Adds what `line` makes to `out`; true when it moved to another branch, so `out` was
    /// rebuilt instead.
    fn line(&mut self, line: &Value, ids: &mut Ids, out: &mut Vec<Entry>) -> bool;

    /// A parser of the same Transcript that has read nothing.
    fn fresh(&self) -> Box<dyn Parse>;

    /// Readies a fresh parser for `bytes` from the middle of the Transcript, under the hint
    /// the parser gave after reading what came before.
    fn begin(&mut self, _bytes: &[u8], _hint: i64) {}

    /// What a later `begin` needs to know of what this parser has read.
    fn hint(&self) -> i64 {
        0
    }

    /// Every entry parsed, `shown` and any on branches the Agent has left.
    fn every(&self, shown: Vec<Entry>) -> Vec<Entry> {
        shown
    }

    fn find<'a>(&'a self, shown: &'a [Entry], id: &str) -> Option<&'a Entry> {
        shown.iter().find(|entry| entry.id == id)
    }

    /// Whether the Agent's last reply ended its turn.
    fn turn_over(&self) -> bool {
        false
    }

    /// What the Agent left running in the background.
    fn background(&self) -> &[Background] {
        &[]
    }

    /// Finds the background work over the whole Transcript, so work started long ago counts.
    fn scan(&mut self, _file: File) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
