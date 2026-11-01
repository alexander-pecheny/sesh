//! Copying Transcripts into a Vault (ADR 0008, 0019). Each round a device asks the follower on
//! every machine its Agents run on for their Transcripts once, and the Vault's follower once to
//! take what came back; a Vault on the same machine copies its Transcripts itself.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::agent::transcript_path;
use crate::vault::Vault;

/// Bytes read from one Transcript per round, so one huge file cannot hold up the rest.
pub const CHUNK: u64 = 4 << 20;

type Result<T> = std::result::Result<T, String>;

/// Answers `transcripts`: for each session, the Transcript its pane runs now if the pane still
/// holds the session's Agent, and what each of its Transcripts holds past the size of the
/// Vault's copy, or, given a Vault on this machine, the Transcripts copied into it.
pub fn transcripts(panes: &[Value], request: &Value) -> Result<Value> {
    let vault = request["vault"].as_str().map(|dir| Vault::open(crate::serve::expand(dir), false)).transpose().map_err(|err| err.to_string())?;
    let mut sessions = Vec::new();
    for session in request["sessions"].as_array().into_iter().flatten() {
        let id = session["id"].as_str().unwrap_or_default();
        let pane = panes.iter().find(|pane| pane["pane_id"] == session["session"]);
        let current = pane.filter(|pane| owns(session, pane)).and_then(transcript_path);
        let mut paths: Vec<String> = session["transcripts"].as_array().into_iter().flatten().filter_map(|path| path.as_str().map(str::to_string)).collect();
        paths.extend(current.clone().filter(|path| !paths.contains(path)));
        let files: Vec<Value> = paths
            .iter()
            .map(|path| {
                let copied = match &vault {
                    Some(vault) => vault.copy(id, Path::new(path)).map(|size| json!({"size": size})).map_err(|err| err.to_string()),
                    None => read(path, session["copies"][name(path)].as_u64().unwrap_or(0)),
                };
                let mut file = copied.unwrap_or_else(|err| json!({"error": err}));
                file["path"] = path.clone().into();
                file
            })
            .collect();
        sessions.push(json!({"id": id, "transcript": current, "files": files}));
    }
    Ok(json!({"sessions": sessions}))
}

/// Whether `pane` still runs the session's own Agent: sesh-mux gives a closed pane's id to the next
/// pane, and Sesh names each Agent it starts. A session from before names was known by its folder.
fn owns(session: &Value, pane: &Value) -> bool {
    if let Some(mine) = session["name"].as_str() {
        return pane["name"].as_str() == Some(mine);
    }
    match (session["path"].as_str(), pane["cwd"].as_str()) {
        (Some(path), Some(cwd)) => name(path) == name(cwd),
        _ => true,
    }
}

fn name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// The Transcript's size, and its bytes from `from` on, as much as one round takes.
fn read(path: &str, from: u64) -> Result<Value> {
    let mut file = File::open(path).map_err(|err| format!("{path}: {err}"))?;
    let size = file.metadata().map_err(|err| err.to_string())?.len();
    let mut line = json!({"size": size, "from": from});
    if size > from {
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(from)).map_err(|err| err.to_string())?;
        file.take(CHUNK).read_to_end(&mut bytes).map_err(|err| err.to_string())?;
        line["data"] = base64::engine::general_purpose::STANDARD.encode(&bytes).into();
    }
    Ok(line)
}

/// Answers `store`: appends each chunk to its copy in the Vault, then tells the size of every
/// copy the sessions named have, so the next round reads each Transcript from where its copy ends.
pub fn store(request: &Value) -> Result<Value> {
    let vault = Vault::open(crate::serve::expand(request["dir"].as_str().unwrap_or_default()), false).map_err(|err| err.to_string())?;
    let mut ids: Vec<&str> = request["sessions"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut errors = Vec::new();
    for append in request["appends"].as_array().into_iter().flatten() {
        let (id, file) = (append["id"].as_str().unwrap_or_default(), append["file"].as_str().unwrap_or_default());
        let appended = base64::engine::general_purpose::STANDARD
            .decode(append["data"].as_str().unwrap_or_default())
            .map_err(|err| err.to_string())
            .and_then(|bytes| vault.append(id, file, append["from"].as_u64().unwrap_or(0), &bytes).map_err(|err| err.to_string()));
        if let Err(err) = appended {
            errors.push(json!(err));
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut copies = Map::new();
    for id in ids {
        copies.insert(id.to_string(), vault.copies(id).map_err(|err| err.to_string())?.into());
    }
    Ok(json!({"copies": copies, "errors": errors}))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sesh-copy-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }

        fn file(&self, name: &str, text: &str) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn pane(name: &str, path: &str) -> Value {
        json!({"pane_id": "w1:p1", "agent": "claude", "name": name, "agent_session": {"kind": "path", "value": path}})
    }

    fn bytes(file: &Value) -> String {
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(file["data"].as_str().unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn a_session_gets_its_panes_transcript_and_what_each_holds_past_the_vaults_copy() {
        let dir = Dir::new("read");
        let (old, new) = (dir.file("old.jsonl", "one\ntwo\n"), dir.file("new.jsonl", "fresh\n"));
        let request = json!({"sessions": [{"id": "s1", "session": "w1:p1", "name": "fix-claude", "transcripts": [old], "copies": {"old.jsonl": 4}}]});
        let answer = transcripts(&[pane("fix-claude", &new)], &request).unwrap();
        let session = &answer["sessions"][0];
        assert_eq!(session["transcript"], new.as_str());
        let files = session["files"].as_array().unwrap();
        assert_eq!((files[0]["path"].as_str(), files[0]["from"].as_u64(), bytes(&files[0])), (Some(old.as_str()), Some(4), "two\n".into()));
        assert_eq!((files[1]["path"].as_str(), files[1]["from"].as_u64(), bytes(&files[1])), (Some(new.as_str()), Some(0), "fresh\n".into()));
    }

    #[test]
    fn a_pane_another_agent_took_over_gives_the_session_no_transcript() {
        let dir = Dir::new("reused");
        let (old, theirs) = (dir.file("old.jsonl", "one\n"), dir.file("theirs.jsonl", "not ours\n"));
        let request = json!({"sessions": [{"id": "s1", "session": "w1:p1", "name": "fix-claude", "transcripts": [old], "copies": {"old.jsonl": 4}}]});
        let answer = transcripts(&[pane("other-claude", &theirs)], &request).unwrap();
        assert_eq!(answer["sessions"][0], json!({"id": "s1", "transcript": null, "files": [{"path": old, "size": 4, "from": 4}]}));
        let unnamed = json!({"sessions": [{"id": "s1", "session": "w1:p1", "path": "/work/app", "transcripts": []}]});
        let elsewhere = json!({"pane_id": "w1:p1", "agent": "claude", "cwd": "/work/lib", "agent_session": {"kind": "path", "value": theirs}});
        assert_eq!(transcripts(&[elsewhere], &unnamed).unwrap()["sessions"][0]["transcript"], Value::Null);
    }

    #[test]
    fn a_vault_takes_chunks_at_its_copys_size_and_says_how_long_each_copy_is() {
        let dir = Dir::new("store");
        let vault = dir.0.join("vault").to_string_lossy().into_owned();
        Vault::open(&vault, true).unwrap();
        let data = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
        let first = json!({"dir": vault, "sessions": ["s2"], "appends": [{"id": "s1", "file": "t.jsonl", "from": 0, "data": data("one\n")}]});
        assert_eq!(store(&first).unwrap(), json!({"copies": {"s2": {}, "s1": {"t.jsonl": 4}}, "errors": []}));
        let stale = json!({"dir": vault, "appends": [{"id": "s1", "file": "t.jsonl", "from": 0, "data": data("one\n")}]});
        let answer = store(&stale).unwrap();
        assert_eq!(answer["copies"], json!({"s1": {"t.jsonl": 4}}));
        assert_eq!(answer["errors"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_vault_on_the_same_machine_copies_the_transcripts_itself() {
        let dir = Dir::new("local");
        let vault = dir.0.join("vault").to_string_lossy().into_owned();
        Vault::open(&vault, true).unwrap();
        let new = dir.file("new.jsonl", "fresh\n");
        let request = json!({"vault": vault, "sessions": [{"id": "s1", "session": "w1:p1", "name": "fix-claude", "transcripts": []}]});
        let answer = transcripts(&[pane("fix-claude", &new)], &request).unwrap();
        assert_eq!(answer["sessions"][0]["files"], json!([{"path": new, "size": 6}]));
        assert_eq!(Vault::open(&vault, false).unwrap().size("s1", "new.jsonl").unwrap(), 6);
    }
}
