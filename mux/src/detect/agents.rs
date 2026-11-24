//! The agent files sesh-mux and Sesh's transcript helper share: what each Agent's process is
//! called, how it is started and resumed, and where it says which session it holds. Claude's,
//! Codex's and pi's are built in; a file in `~/.config/sesh/agents` adds an Agent, or replaces a
//! built-in one of the same name.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

const BUILT_IN: [&str; 3] = [
    include_str!("../../agents/claude.toml"),
    include_str!("../../agents/codex.toml"),
    include_str!("../../agents/pi.toml"),
];

/// Agents are numbered in a byte.
const MAX_AGENTS: usize = 256;

/// An Agent sesh-mux knows, by its place among the agent files; the built-in ones come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Agent(u8);

#[allow(non_upper_case_globals)]
impl Agent {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const Claude: Self = Self(0);
    #[cfg_attr(not(test), allow(dead_code))]
    pub const Codex: Self = Self(1);
    pub const Pi: Self = Self(2);

    pub fn all() -> impl Iterator<Item = Self> {
        (0..specs().len()).map(|index| Self(index as u8))
    }

    pub fn spec(self) -> &'static Spec {
        &specs()[usize::from(self.0)]
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Spec {
    pub name: String,
    /// The program and any subcommand that start the Agent, as `["perplexity", "computer"]`.
    pub command: Vec<String>,
    /// Other names its process goes by.
    pub process_names: Vec<String>,
    /// Whether sesh-mux's own hooks report the Agent's state.
    pub hooks: bool,
    pub resume: Resume,
    pub session: Option<SessionFile>,
    #[serde(deserialize_with = "manifest_text")]
    pub detect: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Resume {
    /// What resumes a session, with `{id}` or `{path}` for it.
    pub args: Vec<String>,
    /// The flags and subcommands that name a session, each followed by it.
    pub flags: Vec<String>,
}

/// A file the Agent writes for each of its processes, naming the session it holds.
#[derive(Debug, Deserialize)]
pub struct SessionFile {
    /// The file, with `{pid}` for the process and `*` for any one folder.
    pub file: String,
    /// Where in the file the session id is.
    pub id: String,
    /// The session's Transcript, relative to the file's folder, with `{id}` for the session.
    pub transcript: String,
}

impl Resume {
    /// Whether the Agent is resumed by its Transcript's path rather than its session id.
    pub fn by_path(&self) -> bool {
        self.args.iter().any(|arg| arg == "{path}")
    }
}

/// The screen-detection rules, kept as text for the manifest parser.
fn manifest_text<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let table = Option::<toml::Value>::deserialize(deserializer)?;
    table.map(|table| toml::to_string(&table).map_err(serde::de::Error::custom)).transpose()
}

/// Every Agent, the built-in ones first.
pub fn specs() -> &'static [Spec] {
    loaded()
}

fn loaded() -> &'static Vec<Spec> {
    static LOADED: OnceLock<Vec<Spec>> = OnceLock::new();
    LOADED.get_or_init(|| {
        let mut specs: Vec<Spec> = BUILT_IN.iter().map(|text| parse(text).expect("a built-in agent file")).collect();
        let mut files: Vec<PathBuf> = std::fs::read_dir(folder()).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
        files.sort();
        for file in files.iter().filter(|file| file.extension().is_some_and(|ext| ext == "toml")) {
            match std::fs::read_to_string(file).map_err(|err| err.to_string()).and_then(|text| parse(&text)) {
                Ok(spec) => match specs.iter().position(|known| known.name == spec.name) {
                    Some(at) => specs[at] = spec,
                    None if specs.len() < MAX_AGENTS => specs.push(spec),
                    None => tracing::warn!("ignored {}: more than {MAX_AGENTS} agents", file.display()),
                },
                Err(err) => tracing::warn!("ignored agent file {}: {err}", file.display()),
            }
        }
        specs
    })
}

/// `SESH_AGENTS`, or `sesh/agents` in the config folder.
fn folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("SESH_AGENTS") {
        return dir.into();
    }
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from);
    config.unwrap_or_else(|| home().join(".config")).join("sesh/agents")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn parse(text: &str) -> Result<Spec, String> {
    let spec: Spec = toml::from_str(text).map_err(|err| err.to_string())?;
    if spec.name.is_empty() || spec.command.is_empty() {
        return Err("an agent needs a name and a command".into());
    }
    Ok(spec)
}

impl SessionFile {
    /// The session process `pid` holds, and its Transcript.
    pub fn read(&self, pid: u32) -> Option<(String, String)> {
        let pattern = self.file.replace("{pid}", &pid.to_string());
        let pattern = match pattern.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => PathBuf::from(pattern),
        };
        matching(&pattern).into_iter().find_map(|file| {
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).ok()?).ok()?;
            let id = value.pointer(&self.id)?.as_str()?.to_string();
            let transcript = file.parent()?.join(self.transcript.replace("{id}", &id));
            Some((id, normalized(&transcript).to_string_lossy().into_owned()))
        })
    }
}

/// The files `pattern` names, where a `*` part matches any one folder.
fn matching(pattern: &Path) -> Vec<PathBuf> {
    let mut found = vec![PathBuf::new()];
    for part in pattern.iter() {
        found = found
            .into_iter()
            .flat_map(|dir| match part.to_str() {
                Some("*") => std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|entry| entry.path()).collect(),
                _ => vec![dir.join(part)],
            })
            .collect();
    }
    found.retain(|file| file.is_file());
    found
}

/// `path` with each `..` taken back out.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_agents_come_first_in_order() {
        let names: Vec<&str> = [Agent::Claude, Agent::Codex, Agent::Pi].map(|agent| agent.spec().name.as_str()).to_vec();
        assert_eq!(names, ["claude", "codex", "pi"]);
        assert!(Agent::Pi.spec().resume.by_path());
        assert!(!Agent::Claude.spec().resume.by_path());
    }

    #[test]
    fn a_session_file_names_the_task_a_process_holds() {
        let runtime = std::env::temp_dir().join(format!("sesh-mux-session-{}", std::process::id()));
        std::fs::create_dir_all(runtime.join("staging/sessions")).unwrap();
        std::fs::write(runtime.join("staging/sessions/42.json"), r#"{"sessionId":"t1"}"#).unwrap();
        let session = SessionFile {
            file: format!("{}/*/sessions/{{pid}}.json", runtime.display()),
            id: "/sessionId".into(),
            transcript: "../transcripts/{id}.jsonl".into(),
        };
        let found = session.read(42);
        let missing = session.read(43);
        std::fs::remove_dir_all(&runtime).unwrap();
        let transcript = runtime.join("staging/transcripts/t1.jsonl").to_string_lossy().into_owned();
        assert_eq!(found, Some(("t1".to_string(), transcript)));
        assert_eq!(missing, None);
    }
}
