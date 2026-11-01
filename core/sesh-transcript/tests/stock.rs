//! The helper against a stand-in for sesh-mux's socket that reports Claude's session id but no
//! Transcript path and no permission prompts.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::os::unix::net::{UnixListener, UnixStream};
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
        let mux = UnixListener::bind(root.join("mux.sock")).unwrap();
        let dir = root.clone();
        std::thread::spawn(move || {
            for stream in mux.incoming() {
                let dir = dir.clone();
                std::thread::spawn(move || answer(&dir, stream.unwrap()));
            }
        });
        Self { root }
    }

    fn session(&self, id: &str, status: &str) {
        let pane = json!({"pane_id": "w1:p1", "agent": "claude", "agent_status": status,
            "agent_session": {"source": "sesh-mux:claude", "agent": "claude", "kind": "id", "value": id}});
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
        command
            .args(args)
            .env("HOME", &self.root)
            .env("SESH_MUX_SOCKET_PATH", self.root.join("mux.sock"))
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

/// Answers one request on the stand-in's socket from the files in `dir`, and notes every input
/// it is given; while a `hold` file exists, an input waits, having said in `playing` that it started.
fn answer(dir: &Path, stream: UnixStream) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let request: Value = serde_json::from_str(&line).unwrap_or_default();
    let params = &request["params"];
    let read = |file: &str| std::fs::read_to_string(dir.join(file)).unwrap_or_default();
    let pane = params["pane_id"].as_str().or(params["target"].as_str()).unwrap_or_default();
    let input = match request["method"].as_str().unwrap_or_default() {
        "pane.send_keys" => Some(format!("pane send-keys {pane} {}", params["keys"].as_array().unwrap().iter().map(|key| key.as_str().unwrap()).collect::<Vec<_>>().join(" "))),
        "pane.send_text" => Some(format!("pane send-text {pane} {}", params["text"].as_str().unwrap())),
        "pane.close" => Some(format!("pane close {pane}")),
        "agent.prompt" => Some(format!("agent prompt {pane} {}", params["text"].as_str().unwrap())),
        _ => None,
    };
    let reply = match (request["method"].as_str().unwrap_or_default(), input) {
        (_, Some(input)) => {
            let note = |file: &str| writeln!(std::fs::OpenOptions::new().create(true).append(true).open(dir.join(file)).unwrap(), "{input}").unwrap();
            note("playing");
            while dir.join("hold").exists() {
                std::thread::sleep(Duration::from_millis(10));
            }
            note("inputs");
            json!({"result": {"type": "ok"}}).to_string()
        }
        ("pane.get", _) => read("pane.json"),
        ("pane.list", _) => read("panes.json"),
        ("agent.start", _) => json!({"result": {"type": "ok"}}).to_string(),
        ("agent.get", _) => json!({"result": {"agent": serde_json::from_str::<Value>(&read("agent.json")).unwrap()}}).to_string(),
        ("agent.list", _) => json!({"result": {"agents": []}}).to_string(),
        ("pane.read", _) => json!({"result": {"read": {"text": read("screen.txt")}}}).to_string(),
        (method, _) => json!({"error": {"code": "unknown_method", "message": method}}).to_string(),
    };
    let _ = writeln!(&stream, "{}", reply.trim());
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
        ["hello", "entry:user", "entry:text", "state", "live", "background", "cursor"]
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
fn the_follower_plays_a_devices_messages_keys_answers_and_stops_into_sesh_mux() {
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
fn the_follower_starts_an_agent_and_closes_its_pane_when_it_exits_on_start() {
    let host = Host::new("start");
    host.session("s1", "idle");
    let mut follower = host.follower(0);
    let start = json!({"op": "start", "session": "w1:p1", "name": "fix-claude", "agent": "claude", "args": ["--dangerously-skip-permissions"]});
    let agent = |ready: bool| json!({"agent": "claude", "name": "fix-claude", "agent_status": "idle", "interactive_ready": ready}).to_string();
    std::fs::write(host.root.join("agent.json"), agent(true)).unwrap();
    let ready = host.command(&["ask", &start.to_string()]).output().unwrap();
    std::fs::write(host.root.join("agent.json"), agent(false)).unwrap();
    std::fs::write(host.root.join("screen.txt"), "Error: no such model\n").unwrap();
    let exited = host.command(&["ask", &start.to_string()]).output().unwrap();
    follower.kill().unwrap();
    follower.wait().unwrap();
    assert!(ready.status.success(), "{}", String::from_utf8_lossy(&ready.stderr));
    assert_eq!(exited.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&exited.stderr), "agent process exited before becoming interactive\nError: no such model\n");
    assert_eq!(host.inputs(), "pane close w1:p1\n");
}

#[test]
fn a_round_of_copying_asks_the_agents_follower_for_chunks_and_the_vaults_to_store_them() {
    let host = Host::new("copy");
    host.session("s1", "idle");
    host.say("s1", "user", "hi");
    let vault = host.root.join("vault");
    assert!(host.command(&["vault", "init", vault.to_str().unwrap()]).output().unwrap().status.success());
    let mut follower = host.follower(0);
    let ask = |request: &Value| {
        let output = host.command(&["ask", &request.to_string()]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let read = ask(&json!({"op": "transcripts", "sessions": [{"id": "r1", "session": "w1:p1", "transcripts": [], "copies": {}}]}));
    let session = &read["sessions"][0];
    let file = &session["files"][0];
    let appends = json!([{"id": "r1", "file": "s1.jsonl", "from": file["from"], "data": file["data"]}]);
    let staged = host.root.join("store.json");
    std::fs::write(&staged, json!({"op": "store", "dir": vault, "appends": appends}).to_string()).unwrap();
    let stored = host.command(&["ask", "--file", staged.to_str().unwrap()]).output().unwrap();
    follower.kill().unwrap();
    follower.wait().unwrap();
    let size = host.transcript("s1").metadata().unwrap().len();
    assert_eq!((read["t"].as_str(), session["transcript"].as_str()), (Some("done"), host.transcript("s1").to_str()));
    assert_eq!(serde_json::from_slice::<Value>(&stored.stdout).unwrap()["copies"], json!({"r1": {"s1.jsonl": size}}));
    assert!(!staged.exists());
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
