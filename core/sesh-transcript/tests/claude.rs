//! Real Claude Code Transcripts, anonymised, read through the parser. Each must end with the
//! entries, background work and turn its `.expected` file lists, so a change in how Claude
//! writes its Transcript fails here against real output instead of hand-written lines.
//! `SESH_BLESS=1` rewrites the files; tests/claude/README.md says how to refresh the corpus.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sesh_transcript::{Agent, Transcript};

/// Each Transcript in the corpus, what it shows, and what must hold at its end however its
/// entries read: how much work is left in the background and whether Claude's turn is over.
const SAMPLES: &[(&str, &str, usize, bool)] = &[
    ("background-bash/session.jsonl", "one background command finishing while another still runs", 1, false),
    ("bash-mode-and-background-agent/session.jsonl", "an Agent left running in the background, then a ! command", 1, false),
    ("clear/session.jsonl", "/clear, then a scheduled task's prompt", 0, false),
    ("compaction/session.jsonl", "an automatic compaction, then a prompt with an image", 0, false),
    ("ctrl-b-in-subagent/subagents/agent-aa309d597ebfca9e2.jsonl", "a subagent's shells moved to the background, told finished on meta lines", 1, true),
    ("fork-stops-its-shells/session.jsonl", "a fork started in the background", 0, false),
    ("fork-stops-its-shells/subagents/agent-ae641ae1682ad639e.jsonl", "a fork's own Transcript: its parent's call, TaskStop, a Monitor, timed-out shells", 0, true),
    ("half-written-line/session.jsonl", "a message written onto the end of a half-written line", 0, false),
    ("image-in-prompt/session.jsonl", "a pasted image and the meta line naming its file", 0, false),
    ("interrupted-turn/session.jsonl", "a turn interrupted during a command, and the messages after it", 0, false),
    ("manual-compact/session.jsonl", "/compact typed, then a skill's slash command", 0, false),
    ("queued-message-and-question/session.jsonl", "a message queued mid-turn, an answered question, a background command", 1, false),
    ("slash-command-with-args/session.jsonl", "a skill's slash command with arguments", 0, false),
    ("subagent-waits-on-its-shells/session.jsonl", "background Agents, one reporting early that it waits on its own shells", 4, false),
    ("subagent-waits-on-its-shells/subagents/agent-a917d397ed67a8134.jsonl", "that subagent's own shells and Monitors", 0, false),
    ("taskstop-and-queue/session.jsonl", "a command moved to the background by a message, then TaskStop", 0, false),
];

/// `SESH_CLAUDE_CORPUS` reads a recording made with `--raw` instead, to compare with its copy.
fn corpus() -> PathBuf {
    std::env::var_os("SESH_CLAUDE_CORPUS").map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/claude"), PathBuf::from)
}

fn transcripts(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            transcripts(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            found.push(path);
        }
    }
}

/// What a device would be shown: one line per entry, then the background work and the turn.
fn outcome(path: &Path) -> (String, usize, bool) {
    let mut transcript = Transcript::new(Agent::Claude, path);
    transcript.read(None).unwrap();
    let read = transcript.background().to_vec();
    transcript.scan_background().unwrap();
    assert_eq!(read, transcript.background(), "{}: the scan found other background work than the full read", path.display());
    let mut out = String::new();
    for entry in &transcript.entries {
        let name = entry.name.as_deref().map(|name| format!(" {name}")).unwrap_or_default();
        let images = entry.images.as_ref().filter(|images| !images.is_empty()).map(|images| format!(" [{} images]", images.len())).unwrap_or_default();
        writeln!(out, "{}{name}: {}{images}", entry.kind, entry.summary).unwrap();
    }
    for work in &read {
        writeln!(out, "background{}: {}", if work.agent { " agent" } else { "" }, work.label).unwrap();
    }
    writeln!(out, "turn over: {}", transcript.turn_over()).unwrap();
    (out, read.len(), transcript.turn_over())
}

#[test]
fn real_claude_transcripts_read_as_they_did() {
    let root = corpus();
    let mut found = Vec::new();
    transcripts(&root, &mut found);
    let mut listed: Vec<PathBuf> = SAMPLES.iter().map(|(file, ..)| root.join(file)).collect();
    found.sort();
    listed.sort();
    assert_eq!(found, listed, "every Transcript in the corpus is listed in SAMPLES, and only those");
    let mut failed = Vec::new();
    for (file, shows, background, turn_over) in SAMPLES {
        let path = root.join(file);
        let (got, left, over) = outcome(&path);
        let expected = path.with_extension("expected");
        if std::env::var_os("SESH_BLESS").is_some() {
            std::fs::write(&expected, &got).unwrap();
        }
        if (left, over) != (*background, *turn_over) {
            failed.push(format!("{file} ({shows}) ends with {left} pieces of background work and turn over {over}"));
        }
        let want = std::fs::read_to_string(&expected).unwrap_or_default();
        if got != want {
            failed.push(format!("{file} ({shows}) reads differently:\n{}", diff(&want, &got)));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// The lines of `got` that `want` lacks and the other way round.
fn diff(want: &str, got: &str) -> String {
    let lost = want.lines().filter(|line| !got.lines().any(|other| other == *line)).map(|line| format!("- {line}"));
    let new = got.lines().filter(|line| !want.lines().any(|other| other == *line)).map(|line| format!("+ {line}"));
    lost.chain(new).collect::<Vec<_>>().join("\n")
}
