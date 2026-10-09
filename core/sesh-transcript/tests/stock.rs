//! The helper against a stand-in for stock herdr, which reports Claude's session id but no
//! Transcript path and no permission prompts.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

struct Host {
    root: PathBuf,
}

impl Host {
    fn new(name: &str) -> Self {
        // Short, as the follower's socket path under it must be.
        let root = PathBuf::from(format!("/tmp/sesh-{name}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("claude/projects/-work")).unwrap();
        let herdr = root.join("herdr");
        // Answers what it is asked of the pane from files, and notes every input it is given;
        // while a `hold` file exists, an input waits, having said in `playing` that it started.
        let script = r#"#!/bin/sh
dir="$(dirname "$0")"
case "$1 $2" in
'pane get') exec cat "$dir/pane.json" ;;
'pane list') exec cat "$dir/panes.json" ;;
'agent list') echo '{"result":{"agents":[]}}' ;;
'pane read') exec cat "$dir/screen.txt" ;;
'pane send-keys'|'pane send-text'|'pane close'|'agent prompt') echo "$*" >> "$dir/playing"; while [ -e "$dir/hold" ]; do sleep 0.01; done; echo "$*" >> "$dir/inputs" ;;
*) exit 1 ;;
esac
"#;
        std::fs::write(&herdr, script).unwrap();
        let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        std::fs::set_permissions(&herdr, executable).unwrap();
        Self { root }
    }

    fn session(&self, id: &str, status: &str) {
        let pane = json!({"pane_id": "w1:p1", "agent": "claude", "agent_status": status,
            "agent_session": {"source": "herdr:claude", "agent": "claude", "kind": "id", "value": id}});
        for (name, reply) in [("pane.json", json!({"result": {"pane": pane}})), ("panes.json", json!({"result": {"panes": [pane]}}))] {
            std::fs::write(self.root.join("new.json"), reply.to_string()).unwrap();
            std::fs::rename(self.root.join("new.json"), self.root.join(name)).unwrap();
        }
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
            .env("HOME", &self.root)
            .env("CLAUDE_CONFIG_DIR", self.root.join("claude"));
        command
    }

    /// A follower of this Host's own from helper build `build`, run in the foreground so the
    /// test can end it, once it answers on its socket.
    fn follower(&self, build: u64) -> Child {
        let mut child = self.command(&["serve", "--foreground"]).env("SESH_BUILD", build.to_string()).spawn().unwrap();
        for _ in 0..50 {
            if self.hello().is_some_and(|hello| hello["build"] == build) {
                return child;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("the follower did not start");
    }

    /// What the follower answering on this Host says of itself, if one does.
    fn hello(&self) -> Option<Value> {
        let folders = std::fs::read_dir(self.root.join(".sesh/follower")).ok()?;
        let socket = folders.flatten().find_map(|dir| UnixStream::connect(dir.path().join("follower.sock")).ok())?;
        writeln!(&socket, "{}", json!({"op": "hello"})).ok()?;
        serde_json::from_str(&BufReader::new(&socket).lines().next()?.ok()?).ok()
    }

    /// `attach` as a device pinning helper build `build` runs it, and its lines as they come.
    fn attach(&self, build: u64, args: &[&str]) -> (Child, mpsc::Receiver<Value>) {
        let mut child = self.command(&[&["attach"], args].concat()).env("SESH_BUILD", build.to_string()).stdout(Stdio::piped()).spawn().unwrap();
        let (send, lines) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let _ = send.send(serde_json::from_str::<Value>(&line.unwrap()).unwrap());
            }
        });
        (child, lines)
    }

    /// The first line the follower answering on this Host gives `request`.
    fn ask(&self, request: &Value) -> Value {
        let socket = UnixStream::connect(self.root.join(".sesh/follower").join(sesh_transcript::serve::follower()).join("follower.sock")).unwrap();
        writeln!(&socket, "{request}").unwrap();
        BufReader::new(&socket).lines().next().and_then(|line| serde_json::from_str(&line.ok()?).ok()).unwrap_or_default()
    }

    fn inputs(&self) -> String {
        std::fs::read_to_string(self.root.join("inputs")).unwrap_or_default()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Waits for `done` to hold, for at most five seconds.
fn wait(done: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(5), "waited five seconds");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Lines up to and including the first that `last` picks.
fn until(lines: &mpsc::Receiver<Value>, last: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    while !seen.last().is_some_and(&last) {
        seen.push(lines.recv_timeout(Duration::from_secs(5)).expect("a line within 5 s"));
    }
    seen
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

#[test]
fn the_follower_plays_a_devices_messages_keys_answers_and_stops_into_herdr() {
    let host = Host::new("act");
    host.session("s1", "blocked");
    let mut follower = host.follower(0);
    for args in [&["send", "w1:p1", "sent.1", "hello there"][..], &["keys", "w1:p1", "esc"], &["stop", "w1:p1"]] {
        let output = host.command(args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
    assert_eq!(host.inputs(), "agent prompt w1:p1 hello there\npane send-keys w1:p1 esc\npane send-keys w1:p1 ctrl+c ctrl+c\npane close w1:p1\n");

    std::fs::write(host.root.join("screen.txt"), "Do you want to proceed?\n").unwrap();
    let refused = host.command(&["permit", "w1:p1", "allow"]).output().unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).starts_with("no menu is open in pane w1:p1"));
    let unknown = host.command(&["permit", "w1:p1", "maybe"]).output().unwrap();
    assert_eq!(unknown.status.code(), Some(2));
    follower.kill().unwrap();
    follower.wait().unwrap();
}

#[test]
fn a_newer_build_takes_over_an_older_follower_and_keeps_its_log() {
    let host = Host::new("newer");
    host.session("s1", "idle");
    host.say("s1", "user", "hi");
    let mut older = host.follower(1);
    let (mut device, lines) = host.attach(1, &["--watch", "w1:p1"]);
    let opened = until(&lines, |line| line["t"] == "opened");
    assert!(opened.iter().any(|line| line["entry"]["text"] == "hi"), "{opened:?}");
    let seq = opened.last().unwrap()["seq"].as_i64().unwrap();
    device.kill().unwrap();
    device.wait().unwrap();

    let mut newer = host.follower(2);
    assert!(older.wait().unwrap().success());
    host.say("s1", "assistant", "hello");
    let (mut device, lines) = host.attach(2, &["--watch", &format!("w1:p1:{seq}")]);
    let resumed = until(&lines, |line| line["entry"]["text"] == "hello");
    device.kill().unwrap();
    device.wait().unwrap();
    newer.kill().unwrap();
    newer.wait().unwrap();
    assert_eq!(resumed[0]["build"], 2);
    assert!(resumed.iter().all(|line| line["t"] != "opened"), "{resumed:?}");
    assert!(resumed.last().unwrap()["seq"].as_i64().unwrap() > seq);
}

#[test]
fn a_newer_build_takes_over_only_once_the_acts_in_flight_are_played_and_answered() {
    let host = Host::new("handoff");
    host.session("s1", "idle");
    std::fs::write(host.root.join("hold"), "").unwrap();
    let mut older = host.follower(1);
    let send = |text: &str| host.command(&["send", "w1:p1", &format!("sent.{text}"), text]).env("SESH_BUILD", "1").stderr(Stdio::piped()).spawn().unwrap();
    let held = send("before the handoff");
    wait(|| host.root.join("playing").exists());
    let mut newer = host.command(&["serve", "--foreground"]).env("SESH_BUILD", "2").spawn().unwrap();
    // The older follower refuses acts once it was asked to leave.
    wait(|| host.ask(&json!({"op": "unqueue", "session": "w1:p1", "id": "sent.none"}))["t"] == "leaving");
    let during = send("during the handoff");
    std::fs::remove_file(host.root.join("hold")).unwrap();
    let (held, during) = (held.wait_with_output().unwrap(), during.wait_with_output().unwrap());
    let left = older.wait().unwrap();
    let hello = host.hello();
    newer.kill().unwrap();
    newer.wait().unwrap();
    assert!(held.status.success(), "{}", String::from_utf8_lossy(&held.stderr));
    assert!(during.status.success(), "{}", String::from_utf8_lossy(&during.stderr));
    assert!(left.success());
    assert_eq!(hello.unwrap()["build"], 2);
    assert_eq!(host.inputs(), "agent prompt w1:p1 before the handoff\nagent prompt w1:p1 during the handoff\n");
}

#[test]
fn an_older_build_attaches_to_a_newer_follower_and_leaves_it_running() {
    let host = Host::new("older");
    host.session("s1", "idle");
    let mut newer = host.follower(2);
    let (mut device, lines) = host.attach(1, &["--sessions"]);
    let hello = until(&lines, |line| line["t"] == "hello");
    device.kill().unwrap();
    device.wait().unwrap();
    let running = newer.try_wait().unwrap().is_none();
    newer.kill().unwrap();
    newer.wait().unwrap();
    assert_eq!(hello[0]["build"], 2);
    assert!(running);
}

#[test]
fn a_session_log_of_another_schema_is_refused() {
    let host = Host::new("schema");
    let version = String::from_utf8(host.command(&["--version"]).output().unwrap().stdout).unwrap();
    let folder = host.root.join(".sesh/follower").join(version.trim().rsplit('.').next().unwrap());
    std::fs::create_dir_all(&folder).unwrap();
    let db = rusqlite::Connection::open(folder.join("sessions.db")).unwrap();
    db.execute_batch(&format!("CREATE TABLE items (id TEXT); PRAGMA user_version = {};", sesh_transcript::log::VERSION + 1)).unwrap();
    drop(db);
    let output = host.command(&["serve", "--foreground"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Session log schema"), "{output:?}");
}
