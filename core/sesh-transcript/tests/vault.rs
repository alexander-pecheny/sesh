//! The helper serving a Vault and showing a Transcript file without herdr.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("sesh-vault-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn say(&self, file: &str, who: &str, text: &str) {
        let at = "2026-10-07T10:00:00Z";
        let line = match who {
            "user" => json!({"type": "user", "timestamp": at, "origin": {"kind": "human"},
                "message": {"role": "user", "content": text}}),
            _ => json!({"type": "assistant", "timestamp": at,
                "message": {"content": [{"type": "text", "text": text}]}}),
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.0.join(file))
            .unwrap();
        std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes()).unwrap();
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn helper(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sesh-transcript"));
    command.args(args).env("PATH", "/nonexistent");
    command
}

fn lines(output: Output) -> Vec<Value> {
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn spawn(args: &[&str]) -> (Child, mpsc::Receiver<Value>) {
    let mut child = helper(args).stdout(Stdio::piped()).spawn().unwrap();
    let (send, lines) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let _ = send.send(serde_json::from_str::<Value>(&line.unwrap()).unwrap());
        }
    });
    (child, lines)
}

fn until(lines: &mpsc::Receiver<Value>, until: &str) -> Vec<Value> {
    let mut burst = Vec::new();
    while burst.last().is_none_or(|line: &Value| line["t"] != until) {
        burst.push(
            lines
                .recv_timeout(Duration::from_secs(5))
                .expect("a line within 5 s"),
        );
    }
    burst
}

#[test]
fn a_transcript_file_is_followed_paged_and_read_without_herdr() {
    let scratch = Scratch::new("file");
    for n in 0..4 {
        scratch.say("t.jsonl", "user", &format!("message {n}"));
    }
    let file = scratch.path("t.jsonl");
    let (mut follow, shown) = spawn(&[
        "follow", "--file", &file, "--agent", "claude", "--last", "2",
    ]);
    let first = until(&shown, "cursor");
    assert_eq!(first[0]["t"], "hello");
    assert_eq!(first[0]["transcript"], file.as_str());
    let texts: Vec<&Value> = first
        .iter()
        .filter(|line| line["t"] == "entry")
        .map(|line| &line["text"])
        .collect();
    assert_eq!(texts, ["message 2", "message 3"]);

    scratch.say("t.jsonl", "assistant", "a reply");
    let live = until(&shown, "cursor");
    assert_eq!(live[0]["text"], "a reply");
    follow.kill().unwrap();
    follow.wait().unwrap();

    let oldest = first[1]["id"].as_str().unwrap();
    let page = lines(
        helper(&[
            "history", "--file", &file, "--agent", "claude", "--before", oldest, "--last", "1",
        ])
        .output()
        .unwrap(),
    );
    assert_eq!(page[0]["text"], "message 1");
    assert_eq!(page[1], json!({"t": "history", "more": true}));
    let entry = lines(
        helper(&["entry", "--file", &file, "--agent", "claude", oldest])
            .output()
            .unwrap(),
    );
    assert_eq!(entry[0]["text"], "message 2");
    assert!(!helper(&["entry", "--file", &file, oldest])
        .status()
        .unwrap()
        .success());
}

#[test]
fn a_vault_is_served_over_the_command_line() {
    let scratch = Scratch::new("cli");
    let dir = scratch.path("vault");
    let init = lines(helper(&["vault", "init", &dir]).output().unwrap());
    assert_eq!(init, [json!({"t": "head", "seq": 0})]);
    let (mut follow, changes) = spawn(&["vault", "follow", &dir, "--since", "0"]);
    assert_eq!(until(&changes, "head"), [json!({"t": "head", "seq": 0})]);

    let push = scratch.path("push.jsonl");
    std::fs::write(&push, [
        json!({"id": "t1", "kind": "task", "body": {"title": "Feed the walrus"}, "base": 0, "deleted": false}).to_string(),
        json!({"id": "e1", "kind": "entry", "body": {"task": "t1", "text": "Fish bought", "edited": 5}, "base": 0}).to_string(),
    ].join("\n")).unwrap();
    let pushed = lines(helper(&["vault", "push", &dir, &push]).output().unwrap());
    assert_eq!(pushed.len(), 3);
    assert_eq!(
        pushed[1],
        json!({"t": "record", "id": "e1", "kind": "entry",
        "body": {"task": "t1", "text": "Fish bought", "edited": 5}, "seq": 2, "deleted": false})
    );
    assert_eq!(pushed[2], json!({"t": "head", "seq": 2}));
    assert!(!std::path::Path::new(&push).exists());
    let followed = until(&changes, "head");
    assert_eq!(followed, pushed);
    follow.kill().unwrap();
    follow.wait().unwrap();
    assert_eq!(
        lines(
            helper(&["vault", "pull", &dir, "--since", "1"])
                .output()
                .unwrap()
        ),
        pushed[1..]
    );

    scratch.say("abc.jsonl", "user", "Is the walrus fed?");
    let size = std::fs::metadata(scratch.0.join("abc.jsonl"))
        .unwrap()
        .len();
    let copied = lines(
        helper(&[
            "vault",
            "copy",
            &dir,
            "t1",
            "--from",
            &scratch.path("abc.jsonl"),
        ])
        .output()
        .unwrap(),
    );
    assert_eq!(copied, [json!({"size": size})]);
    let sized = lines(
        helper(&["vault", "size", &dir, "t1", "abc.jsonl"])
            .output()
            .unwrap(),
    );
    assert_eq!(sized, copied);
    std::fs::write(scratch.0.join("bytes"), "more").unwrap();
    let stale = helper(&[
        "vault",
        "append",
        &dir,
        "t1",
        "abc.jsonl",
        "--offset",
        "1",
        &scratch.path("bytes"),
    ])
    .output()
    .unwrap();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains(&format!("is {size} bytes long")));

    let hits = lines(
        helper(&["vault", "search", &dir, "walrus"])
            .output()
            .unwrap(),
    );
    let kinds: Vec<&str> = hits
        .iter()
        .map(|hit| hit["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.len(), 2, "{hits:?}");
    assert!(kinds.contains(&"task") && kinds.contains(&"transcript"));
    assert!(hits.iter().all(|hit| hit["t"] == "hit"));
}
