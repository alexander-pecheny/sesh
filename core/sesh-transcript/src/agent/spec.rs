//! The agent files: what each Agent is called, where it keeps its Transcripts and how it is
//! resumed. Claude's, Codex's and pi's are built in; a file in `~/.config/sesh/agents` adds an
//! Agent, or replaces a built-in one of the same name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

const BUILT_IN: [&str; 3] = [
    include_str!("../../../../mux/agents/claude.toml"),
    include_str!("../../../../mux/agents/codex.toml"),
    include_str!("../../../../mux/agents/pi.toml"),
];

/// Agents are numbered in a byte.
const MAX_AGENTS: usize = 256;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Spec {
    pub name: String,
    pub command: Vec<String>,
    /// The program that reads this Agent's Transcripts and menus, one copy per machine.
    pub plugin: Option<Vec<String>>,
    /// Whether the Agent takes no message while it works, so Sesh holds it.
    pub queues: bool,
    pub resume: Resume,
    pub transcript: Files,
    pub permissions: Option<Permissions>,
    pub questions: Option<Questions>,
    /// The folder of the file this came from; a plugin's path is taken from there.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
    /// The whole file, for Sesh's apps.
    #[serde(skip)]
    pub raw: Value,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Resume {
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Files {
    pub parser: String,
    /// Fields of the Transcript's first line that tell this Agent's files from others; `*`
    /// matches any value.
    pub first: BTreeMap<String, String>,
    pub folders: Vec<String>,
    /// How many folders down from one of `folders` a Transcript sits.
    pub depth: usize,
    /// The file name, with `{id}` for the session id and `*` for anything.
    pub file: String,
    /// The session id's length, when `*` makes the name ambiguous.
    pub id_length: Option<usize>,
    /// Where the first lines may name the session, when the file name does not.
    pub session: Option<String>,
    pub cwd: Option<String>,
    /// Where a line names its session, rewritten when a Transcript is placed under a new id.
    pub ids: Vec<String>,
    /// The folder a Transcript from elsewhere is placed in.
    pub place: Option<String>,
    pub screen: bool,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            parser: String::new(),
            first: BTreeMap::new(),
            folders: Vec::new(),
            depth: 0,
            file: "{id}.jsonl".into(),
            id_length: None,
            session: None,
            cwd: None,
            ids: Vec::new(),
            place: None,
            screen: false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Permissions {
    pub allow: String,
    pub menu: String,
    #[serde(default)]
    pub reason: bool,
}

#[derive(Debug, Deserialize)]
pub struct Questions {
    pub menu: String,
    pub submit: String,
}

impl Files {
    pub fn opens(&self, first: &Value) -> bool {
        !self.first.is_empty()
            && self.first.iter().all(|(key, want)| match &first[key] {
                Value::Null => false,
                _ if want == "*" => true,
                value => value.as_str() == Some(want),
            })
    }

    /// The session a Transcript's file name names, if it is one of this Agent's.
    pub fn session_of(&self, name: &str) -> Option<String> {
        let (prefix, suffix) = self.file.split_once("{id}")?;
        let rest = name.strip_suffix(suffix)?;
        let (head, id) = match self.id_length {
            Some(length) => rest.split_at_checked(rest.len().checked_sub(length)?)?,
            None => (rest.get(..prefix.len())?, &rest[prefix.len()..]),
        };
        (!id.is_empty() && glob(prefix, head)).then(|| id.to_string())
    }

    pub fn names(&self, file: &str, id: &str) -> bool {
        self.session_of(file).as_deref() == Some(id)
    }

    pub fn folders(&self) -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        self.folders_under(&home)
    }

    /// Each of `folders` that exists under `home`, once, in order.
    pub fn folders_under(&self, home: &Path) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = Vec::new();
        for folder in self.folders.iter().flat_map(|pattern| expand(pattern, home)) {
            if !found.contains(&folder) {
                found.push(folder);
            }
        }
        found
    }

    /// The folder a Transcript named `name`, begun in `cwd`, is placed in. `{config}` is Claude's
    /// config folder, `{project}` the folder Claude names after `cwd`, `{cwd}` the one pi names
    /// after it, and `{day}` Codex's dated folder.
    pub fn place(&self, home: &Path, config: Option<&Path>, cwd: &Path, name: &str) -> Option<PathBuf> {
        let template = self.place.as_deref()?;
        let config = config.map_or_else(|| home.join(".claude"), Path::to_path_buf);
        let dashed = cwd.to_string_lossy();
        let dashed = dashed.strip_prefix('/').unwrap_or(&dashed).replace(['/', '\\', ':'], "-");
        let folder = template
            .replace("{config}", &config.to_string_lossy())
            .replace("{project}", &crate::place::project_dir_name(cwd))
            .replace("{cwd}", &dashed)
            // rollout-2026-11-02T10-00-00-<id>.jsonl, kept under 2026/11/02.
            .replace("{day}", &name.get(8..18).unwrap_or_default().replace('-', "/"));
        Some(home_relative(&folder, home)?.join(name))
    }
}

/// Every Agent, the built-in ones first.
pub fn specs() -> &'static [Spec] {
    &loaded().0
}

/// Why each agent file that was not read was not.
pub fn errors() -> &'static [String] {
    &loaded().1
}

fn loaded() -> &'static (Vec<Spec>, Vec<String>) {
    static LOADED: OnceLock<(Vec<Spec>, Vec<String>)> = OnceLock::new();
    LOADED.get_or_init(|| {
        let mut specs: Vec<Spec> = BUILT_IN.iter().map(|text| parse(text, None).expect("a built-in agent file")).collect();
        let mut errors = Vec::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(folder()).into_iter().flatten().flatten().map(|entry| entry.path()).collect();
        files.sort();
        for file in files.iter().filter(|file| file.extension().is_some_and(|ext| ext == "toml")) {
            let spec = std::fs::read_to_string(file).map_err(|err| err.to_string()).and_then(|text| parse(&text, file.parent()));
            match spec {
                Ok(spec) => match specs.iter().position(|known| known.name == spec.name) {
                    Some(at) => specs[at] = spec,
                    None if specs.len() < MAX_AGENTS => specs.push(spec),
                    None => errors.push(format!("{}: more than {MAX_AGENTS} agents", file.display())),
                },
                Err(err) => errors.push(format!("{}: {err}", file.display())),
            }
        }
        (specs, errors)
    })
}

/// `SESH_AGENTS`, or `sesh/agents` in the config folder.
fn folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("SESH_AGENTS") {
        return dir.into();
    }
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    config.unwrap_or_else(|| home.join(".config")).join("sesh/agents")
}

fn parse(text: &str, dir: Option<&Path>) -> Result<Spec, String> {
    let raw: toml::Value = toml::from_str(text).map_err(|err| err.to_string())?;
    let mut spec: Spec = raw.clone().try_into().map_err(|err: toml::de::Error| err.to_string())?;
    if spec.name.is_empty() || spec.command.is_empty() {
        return Err("an agent needs a name and a command".into());
    }
    spec.dir = dir.map(Path::to_path_buf);
    spec.raw = serde_json::to_value(raw).map_err(|err| err.to_string())?;
    Ok(spec)
}

/// The folders `pattern` names under `home`: `~` is the home, `$NAME` and `${NAME:-default}`
/// are variables, a folder named by an unset variable is none, and `*` matches part of a name.
fn expand(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let Some(path) = substitute(pattern).and_then(|path| home_relative(&path, home)) else {
        return Vec::new();
    };
    let mut found = vec![PathBuf::from("/")];
    for part in path.iter().skip(1).map(|part| part.to_string_lossy().into_owned()) {
        found = found
            .into_iter()
            .flat_map(|dir| {
                if !part.contains('*') {
                    return vec![dir.join(&part)];
                }
                let mut matched: Vec<PathBuf> = std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|entry| glob(&part, &entry.file_name().to_string_lossy()))
                    .map(|entry| entry.path())
                    .collect();
                matched.sort();
                matched
            })
            .collect();
    }
    found.retain(|dir| dir.is_dir());
    found
}

fn substitute(pattern: &str) -> Option<String> {
    let Some(rest) = pattern.strip_prefix('$') else {
        return Some(pattern.to_string());
    };
    let (name, default, rest) = match rest.strip_prefix('{') {
        Some(braced) => {
            let (inside, rest) = braced.split_once('}')?;
            let (name, default) = inside.split_once(":-").map_or((inside, None), |(name, default)| (name, Some(default)));
            (name, default, rest)
        }
        None => {
            let end = rest.find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_').unwrap_or(rest.len());
            (&rest[..end], None, &rest[end..])
        }
    };
    let value = std::env::var(name).ok().filter(|value| !value.is_empty());
    Some(format!("{}{rest}", value.or(default.map(str::to_string))?))
}

fn home_relative(path: &str, home: &Path) -> Option<PathBuf> {
    match path.strip_prefix('~') {
        Some(rest) => Some(home.join(rest.trim_start_matches('/'))),
        None => Path::new(path).is_absolute().then(|| PathBuf::from(path)),
    }
}

/// Whether `text` matches `pattern`, in which `*` stands for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else {
        return pattern == text;
    };
    let Some(mut rest) = text.strip_prefix(first) else { return false };
    for part in middle {
        let Some(at) = rest.find(part) else { return false };
        rest = &rest[at + part.len()..];
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_with_a_wildcard_gives_its_session_by_length() {
        let codex = &specs()[1].transcript;
        let name = "rollout-2026-11-02T10-00-00-0199a213-81c0-7800-8aa1-bbab2a035a53.jsonl";
        assert_eq!(codex.session_of(name).as_deref(), Some("0199a213-81c0-7800-8aa1-bbab2a035a53"));
        assert_eq!(codex.session_of("notes.jsonl"), None);
        assert_eq!(specs()[0].transcript.session_of("abc.jsonl").as_deref(), Some("abc"));
    }

    #[test]
    fn variables_and_wildcards_name_folders() {
        let home = std::env::temp_dir().join(format!("sesh-spec-{}", std::process::id()));
        for dir in [".claude/projects", ".claude-work/projects", ".claudette"] {
            std::fs::create_dir_all(home.join(dir)).unwrap();
        }
        let found = expand("~/.claude*/projects", &home);
        let unset = expand("$SESH_SPEC_UNSET/projects", &home);
        let fallback = expand("${SESH_SPEC_UNSET:-~/.claude}/projects", &home);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(found, [home.join(".claude/projects"), home.join(".claude-work/projects")]);
        assert!(unset.is_empty());
        assert_eq!(fallback, [home.join(".claude/projects")]);
    }

    #[test]
    fn globs_match_runs_of_anything() {
        assert!(glob("rollout-*-", "rollout-2026-11-02T10-00-00-"));
        assert!(glob(".claude*", ".claude"));
        assert!(!glob("a*b", "ab-"));
        assert!(glob("abc", "abc"));
    }
}
