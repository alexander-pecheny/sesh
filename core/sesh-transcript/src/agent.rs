//! What sets one Agent apart from another: how each writes its Transcript and where, what of its
//! screen Sesh reads, and the keys that answer its menus. Each Agent is described by a file:
//! Claude's, Codex's and pi's ship with Sesh, and more may sit in `~/.config/sesh/agents`.

pub(crate) mod claude;
mod codex;
mod pi;
mod plugin;
mod spec;

use std::fs::File;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

pub use spec::{errors, specs, Spec};

use crate::{Background, Entry, Ids};

/// An Agent Sesh knows, by its place among the agent files; the built-in ones come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Agent(u8);

#[allow(non_upper_case_globals)]
impl Agent {
    pub const Claude: Self = Self(0);
    pub const Codex: Self = Self(1);
    pub const Pi: Self = Self(2);

    pub fn all() -> impl Iterator<Item = Self> {
        (0..specs().len()).map(|index| Self(index as u8))
    }

    pub fn spec(self) -> &'static Spec {
        &specs()[usize::from(self.0)]
    }

    pub fn named(name: &str) -> Option<Self> {
        Self::all().find(|agent| agent.name() == name)
    }

    /// The Agent sesh-mux says runs in `pane`, if it is one Sesh shows.
    pub fn of(pane: &Value) -> Option<Self> {
        Self::named(pane["agent"].as_str()?)
    }

    pub fn name(self) -> &'static str {
        &self.spec().name
    }

    /// The Agent that wrote a Transcript starting with `first`: the one whose first line it
    /// matches most closely, and Claude, whose first line varies, when none does.
    pub fn wrote(first: &Value) -> Self {
        Self::all()
            .filter(|agent| agent.spec().transcript.opens(first))
            .max_by_key(|agent| agent.spec().transcript.first.len())
            .unwrap_or(Self::Claude)
    }

    /// The Transcript of session `id`, found in the Agent's folders.
    pub fn locate(self, id: &str) -> Option<PathBuf> {
        self.locate_under(&PathBuf::from(std::env::var_os("HOME")?), id)
    }

    fn locate_under(self, home: &Path, id: &str) -> Option<PathBuf> {
        let files = &self.spec().transcript;
        files.folders_under(home).iter().find_map(|folder| find_file(folder, files.depth, &|file| files.names(file, id)))
    }

    /// Whether Sesh reads the Agent's screen for what its Transcript does not hold yet: only
    /// Claude's is understood.
    pub fn reads_screen(self) -> bool {
        self.spec().transcript.screen
    }

    /// Whether a blocked Agent's menu is read off its screen: Claude's when no hook explains it,
    /// and a plugin's, which reads the menu itself.
    pub fn reads_menus(self) -> bool {
        self.reads_screen() || self.spec().plugin.is_some()
    }

    /// The menu a plugin Agent's screen shows.
    pub fn menu(self, screen: &str) -> Option<crate::screen::Menu> {
        plugin::menu(self, screen)
    }

    /// The key that allows or denies in the Agent's permission menu, and what its screen shows
    /// while that menu is open.
    pub fn permit(self, allow: bool) -> Option<(&'static str, &'static str)> {
        let permissions = self.spec().permissions.as_ref()?;
        Some((if allow { &permissions.allow } else { "esc" }, &permissions.menu))
    }

    /// What the screen shows while the Agent's question menu is open, and on its last page,
    /// which asks to submit the answers.
    pub fn question_menu(self) -> Option<(&'static str, &'static str)> {
        let questions = self.spec().questions.as_ref()?;
        Some((&questions.menu, &questions.submit))
    }

    /// The `permission` line for a prompt the Agent's hook reported, with the reason the Agent
    /// gave when it gives one.
    pub fn permission(self, id: &str, tool: &str, input: &Value) -> Value {
        let entry = Entry::tool(String::new(), &Value::Null, tool, input);
        let reason = entry.description.filter(|_| self.spec().permissions.as_ref().is_some_and(|permissions| permissions.reason));
        let mut line = json!({"t": "permission", "id": id, "tool": tool, "summary": entry.summary});
        for (key, value) in [("command", entry.command), ("file", entry.file), ("reason", reason)] {
            if let Some(value) = value {
                line[key] = value.into();
            }
        }
        line
    }

    /// The command that resumes the conversation in `file`, when the Agent resumes by id.
    pub fn resume_command(self, file: &str) -> Option<String> {
        let spec = self.spec();
        let id = spec.transcript.session_of(file)?;
        spec.resume.args.iter().any(|arg| arg == "{id}").then(|| {
            let args = spec.resume.args.iter().map(|arg| arg.replace("{id}", &id));
            spec.command.iter().cloned().chain(args).collect::<Vec<_>>().join(" ")
        })
    }

    pub(crate) fn parser(self, path: &Path) -> Box<dyn Parse> {
        match self.spec().transcript.parser.as_str() {
            "claude" => Box::new(claude::Parser::new(path.parent().is_some_and(|folder| folder.ends_with("subagents")))),
            "codex" => Box::<codex::Parser>::default(),
            "pi" => Box::<pi::Parser>::default(),
            _ => Box::new(plugin::Parser::new(self)),
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
/// so the file is found by it, as it is when the file reported is gone: Claude moves its
/// Transcript to another project folder when it enters a worktree, and sesh-mux keeps the first
/// path it was told for a session.
pub fn transcript_path(pane: &Value) -> Option<String> {
    transcript_under(pane, &PathBuf::from(std::env::var_os("HOME")?))
}

fn transcript_under(pane: &Value, home: &Path) -> Option<String> {
    let reported = reported_path(pane);
    if let Some(path) = reported.filter(|path| Path::new(path).exists()) {
        return Some(path.to_string());
    }
    let session = &pane["agent_session"];
    // A sesh-mux that names no session gets no guess: the newest Transcript in the pane's
    // folder is as often an older session's as the one running.
    let found = (session["kind"] == "id")
        .then(|| Agent::named(session["agent"].as_str()?)?.locate_under(home, session["value"].as_str()?))
        .flatten();
    found.map(|path| path.to_string_lossy().into_owned()).or(reported.map(str::to_string))
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
