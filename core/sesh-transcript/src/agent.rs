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

    /// The Agent sesh-mux says runs in `pane`, if it is one Sesh shows.
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
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        match self {
            Self::Claude => claude_configs(&home).iter().find_map(|config| self.find(&config.join("projects"), id)),
            Self::Codex => {
                let codex = std::env::var_os("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from);
                self.find(&codex.join("sessions"), id)
            }
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

/// The path sesh-mux reports: its `path`, or a value of kind `path` (pi).
fn reported_path(pane: &Value) -> Option<&str> {
    let session = &pane["agent_session"];
    let value = (session["kind"] == "path").then(|| session["value"].as_str()).flatten();
    session["path"].as_str().or(value).filter(|path| !path.is_empty())
}

/// The Transcript of the session in `pane`. Claude and Codex may report only their session id,
/// so the file is found by it.
pub fn transcript_path(pane: &Value) -> Option<String> {
    if let Some(path) = reported_path(pane) {
        return Some(path.to_string());
    }
    let session = &pane["agent_session"];
    // A sesh-mux that names no session gets no guess: the newest Transcript in the pane's
    // folder is as often an older session's as the one running.
    if session["kind"] != "id" {
        let configs = claude_configs(&PathBuf::from(std::env::var_os("HOME")?));
        return (Agent::of(pane) == Some(Agent::Claude)).then(|| running_claude(&configs, pane, Path::new("/proc"))).flatten()
            .map(|path| path.to_string_lossy().into_owned());
    }
    let agent = Agent::named(session["agent"].as_str()?)?;
    Some(agent.locate(session["value"].as_str()?)?.to_string_lossy().into_owned())
}

/// The Transcript of a Claude that started before sesh-mux's hook was installed, from the record
/// Claude keeps of each running session: the one whose process runs in the pane, or else the
/// only one in the pane's folder that runs in no pane.
fn running_claude(configs: &[PathBuf], pane: &Value, proc: &Path) -> Option<PathBuf> {
    let folders = [pane["foreground_cwd"].as_str(), pane["cwd"].as_str()];
    let running: Vec<(&PathBuf, Value)> = configs
        .iter()
        .flat_map(|config| std::fs::read_dir(config.join("sessions")).into_iter().flatten().flatten().map(move |entry| (config, entry)))
        .filter_map(|(config, entry)| Some((config, serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?)))
        .filter(|(_, session): &(_, Value)| session["cwd"].as_str().is_some_and(|cwd| folders.contains(&Some(cwd))))
        .filter(|(_, session)| !proc.exists() || session["pid"].as_u64().is_some_and(|pid| proc.join(pid.to_string()).exists()))
        .collect();
    // A shell herdr-sesh started, and so an Agent run in it, knows its pane by herdr's name.
    let panes_of = |(_, session): &&(&PathBuf, Value)| -> Vec<String> {
        let env = std::fs::read(proc.join(format!("{}/environ", session["pid"].as_u64().unwrap_or_default()))).unwrap_or_default();
        env.split(|byte| *byte == 0)
            .filter_map(|var| std::str::from_utf8(var).ok())
            .filter_map(|var| var.strip_prefix("SESH_MUX_PANE_ID=").or_else(|| var.strip_prefix("HERDR_PANE_ID=")).map(str::to_string))
            .collect()
    };
    let here = pane["pane_id"].as_str().unwrap_or_default();
    // A lone Claude that another pane runs is that pane's, not this one's.
    let unplaced: Vec<_> = running.iter().filter(|session| panes_of(session).is_empty()).collect();
    let (config, session) = running.iter().find(|session| panes_of(session).iter().any(|pane| pane == here)).or(match unplaced.as_slice() {
        [one] => Some(*one),
        _ => None,
    })?;
    Agent::Claude.find(&config.join("projects"), session["sessionId"].as_str()?)
}

/// Every folder Claude keeps an account in: `CLAUDE_CONFIG_DIR` first, then each `~/.claude*`.
/// The follower's own `CLAUDE_CONFIG_DIR` is whatever started it, often not a session's.
pub(crate) fn claude_configs(home: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".claude"))
        .map(|entry| entry.path())
        .filter(|path| path.join("projects").is_dir())
        .collect();
    found.sort();
    let mut configs: Vec<PathBuf> = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).into_iter().collect();
    for path in found {
        if !configs.contains(&path) {
            configs.push(path);
        }
    }
    configs
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

    /// The entries the Agent dropped since this was last asked, already gone from `out`.
    fn dropped(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// The entries already read that a later line added to, as an image's file to its message.
    fn amended(&mut self) -> Vec<String> {
        Vec::new()
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
