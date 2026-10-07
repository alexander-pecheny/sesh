//! The helper against a stand-in for stock herdr, which reports Claude's session id but no
//! Transcript path and no permission prompts.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

struct Host {
    root: PathBuf,
}

impl Host {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("sesh-transcript-{name}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("claude/projects/-work")).unwrap();
        let herdr = root.join("herdr");
        std::fs::write(&herdr, "#!/bin/sh\n[ \"$1 $2\" = 'pane get' ] && exec cat \"$(dirname \"$0\")/pane.json\"\nexit 1\n").unwrap();
        let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        std::fs::set_permissions(&herdr, executable).unwrap();
        Self { root }
    }

    fn session(&self, id: &str, status: &str) {
        let pane = json!({"result": {"pane": {"pane_id": "w1:p1", "agent": "claude", "agent_status": status,
            "agent_session": {"source": "herdr:claude", "agent": "claude", "kind": "id", "value": id}}}});
        std::fs::write(self.root.join("pane.json.new"), pane.to_string()).unwrap();
        std::fs::rename(self.root.join("pane.json.new"), self.root.join("pane.json")).unwrap();
    }

    fn transcript(&self, id: &str) -> PathBuf {
        self.root.join(format!("claude/projects/-work/{id}.jsonl"))
    }

    fn say(&self, id: &str, who: &str, text: &str) {
        let at = "2026-10-06T10:00:00Z";
        let line = match who {
            "user" => json!({"type": "user", "timestamp": at, "origin": {"kind": "human"},
                "message": {"role": "user", "content": text}}),
            _ => json!({"type": "assistant", "timestamp": at,
                "message": {"content": [{"type": "text", "text": text}]}}),
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.transcript(id))
            .unwrap();
        std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes()).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sesh-transcript"));
        let path = format!("{}:{}", self.root.display(), std::env::var("PATH").unwrap());
        command
            .args(args)
            .env("PATH", path)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"));
        command
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Lines up to and including the next cursor.
fn burst(lines: &mpsc::Receiver<Value>) -> Vec<Value> {
    let mut burst = Vec::new();
    while burst
        .last()
        .is_none_or(|line: &Value| line["t"] != "cursor")
    {
        burst.push(
            lines
                .recv_timeout(Duration::from_secs(5))
                .expect("a line within 5 s"),
        );
    }
    burst
}

fn kinds(burst: &[Value]) -> Vec<String> {
    burst
        .iter()
        .map(|line| match line["kind"].as_str() {
            Some(kind) => format!("entry:{kind}"),
            None => line["t"].as_str().unwrap().to_string(),
        })
        .collect()
}

#[test]
fn follows_a_claude_session_found_by_its_id_and_its_switch_after_clear() {
    let host = Host::new("follow");
    host.session("s1", "idle");
    host.say("s1", "user", "hi");
    host.say("s1", "assistant", "hello");
    let mut child = host
        .command(&["follow", "w1:p1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (send, lines) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let _ = send.send(serde_json::from_str::<Value>(&line.unwrap()).unwrap());
        }
    });

    let first = burst(&lines);
    assert_eq!(
        kinds(&first),
        ["hello", "entry:user", "entry:text", "state", "live", "cursor"]
    );
    assert_eq!(first[0]["protocol"], 2);
    assert_eq!(
        first[0]["transcript"],
        host.transcript("s1").to_str().unwrap()
    );

    host.say("s1", "user", "more");
    host.session("s1", "working");
    let mut live = burst(&lines);
    if kinds(&live) == ["state", "cursor"] {
        live.extend(burst(&lines));
    }
    assert!(kinds(&live).contains(&"entry:user".to_string()), "{live:?}");

    host.session("s2", "idle");
    std::thread::sleep(Duration::from_millis(600));
    host.say("s2", "user", "fresh start");
    let mut switched = burst(&lines);
    while !kinds(&switched).contains(&"switch".to_string()) {
        switched = burst(&lines);
    }
    assert_eq!(
        kinds(&switched)
            .iter()
            .filter(|kind| kind.starts_with("entry"))
            .count(),
        1
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn history_pages_back_through_a_transcript_found_by_id() {
    let host = Host::new("history");
    host.session("s1", "idle");
    for n in 0..5 {
        host.say("s1", "user", &format!("message {n}"));
    }
    let output = host.command(&["follow", "--protocol"]).output().unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "2\n");

    let last = Path::new(&host.transcript("s1")).metadata().unwrap().len();
    let mut follow = host
        .command(&["follow", "w1:p1", "--last", "2"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (send, lines) = mpsc::channel();
    let stdout = follow.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let _ = send.send(serde_json::from_str::<Value>(&line.unwrap()).unwrap());
        }
    });
    let shown = burst(&lines);
    follow.kill().unwrap();
    follow.wait().unwrap();
    assert!(shown.last().unwrap()["cursor"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{last}:")));
    let oldest = shown.iter().find(|line| line["t"] == "entry").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let output = host
        .command(&["history", "w1:p1", "--before", &oldest, "--last", "2"])
        .output()
        .unwrap();
    let page: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        page.iter()
            .map(|line| line["text"].clone())
            .collect::<Vec<_>>(),
        [json!("message 1"), json!("message 2"), Value::Null]
    );
    assert_eq!(page.last().unwrap(), &json!({"t": "history", "more": true}));
}
