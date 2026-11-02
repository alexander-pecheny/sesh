//! Puts a Transcript where its Agent looks for it on this machine, so `claude --resume`,
//! `codex resume` or `pi --session` can pick up a conversation begun elsewhere. Folder names
//! follow atuin's rehydrate writers (MIT, Atuin, Inc.).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::index::head;
use crate::vault::{uuid, Error, Result};
use crate::Agent;

/// Claude cuts a project folder's name to this length and adds a hash of the whole path.
const MAX_PROJECT_NAME: usize = 200;

pub struct Placed {
    pub session: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
}

impl Placed {
    pub fn line(&self) -> Value {
        json!({"t": "placed", "session": self.session, "path": self.path, "cwd": self.cwd})
    }
}

/// Writes `file` into the Agent's folder under `home`, Claude's in `config`, with its folder
/// moved to one that exists here. A fork, or a Transcript that has gone its own way here since,
/// gets a new session id; one that only grew replaces the shorter copy here.
pub fn place(agent: Agent, file: &Path, home: &Path, config: Option<&Path>, fork: bool) -> Result<Placed> {
    let (session, cwd) = head(agent, file)?;
    let was = cwd.ok_or_else(|| Error(format!("{} names no folder", file.display())))?;
    let cwd = settle(Path::new(&was), home);
    let text = std::fs::read_to_string(file)?;
    let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let write = |id: &str| -> Result<Option<Placed>> {
        let renamed = name.replace(&session, id);
        let path = folder(agent, &cwd, home, config, &renamed);
        let bytes = rewrite(&text, &was, &cwd, &session, id);
        match std::fs::read(&path) {
            Ok(here) if !bytes.as_bytes().starts_with(&here) => return Ok(None),
            Ok(here) if here.len() == bytes.len() => {}
            _ => {
                std::fs::create_dir_all(path.parent().expect("an Agent's folder"))?;
                std::fs::write(&path, bytes)?;
            }
        }
        Ok(Some(Placed { session: id.to_string(), path, cwd: cwd.clone() }))
    };
    if !fork {
        if let Some(placed) = write(&session)? {
            return Ok(placed);
        }
    }
    let id = uuid();
    write(&id)?.ok_or_else(|| Error(format!("a Transcript with the new id {id} is here already")))
}

/// The folder a conversation from `was` continues in: the same one, or the same place under
/// this machine's home, as `/Users/ann/zoo` becomes `/home/ann/zoo`; the home when neither exists.
fn settle(was: &Path, home: &Path) -> PathBuf {
    if was.is_dir() {
        return was.to_path_buf();
    }
    let mut parts = was.components().skip(1);
    let under_home = matches!(parts.next().map(|part| part.as_os_str().to_string_lossy().into_owned()).as_deref(), Some("Users" | "home"));
    let rest: PathBuf = parts.skip(1).collect();
    let moved = if rest.as_os_str().is_empty() { home.to_path_buf() } else { home.join(rest) };
    if under_home && moved.is_dir() {
        moved
    } else {
        home.to_path_buf()
    }
}

fn folder(agent: Agent, cwd: &Path, home: &Path, config: Option<&Path>, name: &str) -> PathBuf {
    match agent {
        Agent::Claude => {
            let config = config.map_or_else(|| home.join(".claude"), Path::to_path_buf);
            config.join("projects").join(project_dir_name(cwd)).join(name)
        }
        Agent::Codex => {
            // rollout-2026-11-02T10-00-00-<id>.jsonl, kept under sessions/2026/11/02.
            let day = name.get(8..18).unwrap_or_default().replace('-', "/");
            home.join(".codex/sessions").join(day).join(name)
        }
        Agent::Pi => {
            let cwd = cwd.to_string_lossy();
            let cwd = cwd.strip_prefix('/').unwrap_or(&cwd).replace(['/', '\\', ':'], "-");
            home.join(".pi/agent/sessions").join(format!("--{cwd}--")).join(name)
        }
    }
}

/// Each line's folder moved from `was` to `cwd`, and the session's id from `old` to `new`.
fn rewrite(text: &str, was: &str, cwd: &Path, old: &str, new: &str) -> String {
    let cwd = cwd.to_string_lossy();
    if was == cwd && old == new {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let Ok(mut value) = serde_json::from_str::<Value>(line) else {
            out.push_str(line);
            continue;
        };
        let session_line = value["type"] == "session";
        for (at, key) in [("", "cwd"), ("/payload", "cwd"), ("", "sessionId"), ("/payload", "id"), ("/payload", "session_id"), ("", "id")] {
            let Some(field) = value.pointer_mut(at).and_then(|object| object.get_mut(key)) else {
                continue;
            };
            let Some(text) = field.as_str() else { continue };
            if key == "cwd" {
                if let Some(rest) = text.strip_prefix(was) {
                    *field = json!(format!("{cwd}{rest}"));
                }
            } else if text == old && (key != "id" || at == "/payload" || session_line) {
                *field = json!(new);
            }
        }
        out.push_str(&value.to_string());
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// The project folder Claude keeps a folder's sessions in: every character but an ASCII letter
/// or digit made `-` per UTF-16 unit, and a long name cut short with a hash of the path.
fn project_dir_name(cwd: &Path) -> String {
    let cwd: PathBuf = cwd.components().collect();
    let raw = cwd.to_string_lossy();
    let mut name = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else {
            name.extend(std::iter::repeat_n('-', c.len_utf16()));
        }
    }
    if name.len() <= MAX_PROJECT_NAME {
        return name;
    }
    name.truncate(MAX_PROJECT_NAME);
    format!("{name}-{}", base36(js_string_hash(&raw).unsigned_abs()))
}

fn js_string_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, unit| h.wrapping_shl(5).wrapping_sub(h).wrapping_add(i32::from(unit)))
}

fn base36(mut n: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("place-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("zoo")).unwrap();
            Self(dir)
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn claude(home: &Home, lines: &[&str]) -> PathBuf {
        let text: String = lines
            .iter()
            .map(|text| format!("{}\n", json!({"type": "user", "sessionId": "abc", "cwd": "/Users/ann/zoo/src",
                "message": {"role": "user", "content": text}})))
            .collect();
        let file = home.0.join("staged/abc.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        file
    }

    #[test]
    fn a_claude_transcript_moves_to_the_same_folder_under_this_home() {
        let home = Home::new("claude");
        std::fs::create_dir_all(home.0.join("zoo/src")).unwrap();
        let file = claude(&home, &["Feed the walrus"]);
        let placed = place(Agent::Claude, &file, &home.0, None, false).unwrap();
        assert_eq!((placed.session.as_str(), &placed.cwd), ("abc", &home.0.join("zoo/src")));
        assert_eq!(placed.path, home.0.join(".claude/projects").join(project_dir_name(&home.0.join("zoo/src"))).join("abc.jsonl"));
        let line: Value = serde_json::from_str(std::fs::read_to_string(&placed.path).unwrap().lines().next().unwrap()).unwrap();
        assert_eq!(line["cwd"], home.0.join("zoo/src").to_string_lossy().as_ref());
        assert_eq!(line["sessionId"], "abc");
    }

    #[test]
    fn a_home_folder_becomes_this_home_without_a_trailing_slash() {
        let home = Home::new("home");
        assert_eq!(settle(Path::new("/Users/ann"), &home.0), home.0);
        assert_eq!(settle(Path::new("/Users/ann/zoo"), &home.0), home.0.join("zoo"));
        assert_eq!(settle(Path::new("/srv/zoo"), &home.0), home.0);
    }

    #[test]
    fn a_grown_transcript_replaces_its_copy_and_a_diverged_one_is_forked() {
        let home = Home::new("diverge");
        let first = place(Agent::Claude, &claude(&home, &["one"]), &home.0, None, false).unwrap();
        let grown = place(Agent::Claude, &claude(&home, &["one", "two"]), &home.0, None, false).unwrap();
        assert_eq!((grown.session.as_str(), &grown.path), ("abc", &first.path));
        assert_eq!(std::fs::read_to_string(&grown.path).unwrap().lines().count(), 2);
        let other = place(Agent::Claude, &claude(&home, &["one", "three"]), &home.0, None, false).unwrap();
        assert_ne!(other.session, "abc");
        assert_eq!(other.path.file_name().unwrap().to_string_lossy(), format!("{}.jsonl", other.session));
        let line: Value = serde_json::from_str(std::fs::read_to_string(&other.path).unwrap().lines().last().unwrap()).unwrap();
        assert_eq!(line["sessionId"], other.session.as_str());
    }

    #[test]
    fn a_codex_fork_takes_a_new_id_in_its_name_and_meta() {
        let home = Home::new("codex");
        let id = "0199aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee";
        let name = format!("rollout-2026-11-02T10-00-00-{id}.jsonl");
        let file = home.0.join(&name);
        let meta = json!({"type": "session_meta", "payload": {"id": id, "cwd": "/gone/elsewhere"}});
        std::fs::write(&file, format!("{meta}\n")).unwrap();
        let placed = place(Agent::Codex, &file, &home.0, None, true).unwrap();
        assert_ne!(placed.session, id);
        assert_eq!(placed.cwd, home.0);
        let expected = home.0.join(".codex/sessions/2026/11/02").join(name.replace(id, &placed.session));
        assert_eq!(placed.path, expected);
        let line: Value = serde_json::from_str(&std::fs::read_to_string(&placed.path).unwrap()).unwrap();
        assert_eq!(line["payload"]["id"], placed.session.as_str());
    }

    #[test]
    fn long_project_names_are_cut_and_hashed_as_claude_does() {
        assert_eq!(project_dir_name(Path::new("/Users/ann/.sesh/zoo")), "-Users-ann--sesh-zoo");
        let long = format!("/{}", "a".repeat(250));
        let name = project_dir_name(Path::new(&long));
        assert!(name.starts_with(&format!("-{}", "a".repeat(199))) && name.len() > MAX_PROJECT_NAME, "{name}");
    }
}
