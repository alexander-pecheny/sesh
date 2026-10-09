use serde_json::json;

use super::*;
use crate::Transcript;

const AT: &str = "2026-10-07T10:00:00Z";

/// A question and its answer as each Agent writes them, after the line that opens its file.
fn exchange(agent: Agent) -> Vec<Value> {
    match agent {
        Agent::Claude => vec![
            json!({"type": "user", "timestamp": AT, "origin": {"kind": "human"}, "message": {"role": "user", "content": "Feed the walrus"}}),
            json!({"type": "assistant", "timestamp": AT, "message": {"stop_reason": "end_turn", "content": [{"type": "text", "text": "Fed."}]}}),
        ],
        Agent::Codex => vec![
            json!({"timestamp": AT, "type": "session_meta", "payload": {}}),
            json!({"timestamp": AT, "type": "event_msg", "payload": {"type": "user_message", "message": "Feed the walrus"}}),
            json!({"timestamp": AT, "type": "event_msg", "payload": {"type": "agent_message", "message": "Fed."}}),
        ],
        Agent::Pi => vec![
            json!({"type": "session", "id": "s0", "timestamp": AT}),
            json!({"type": "message", "id": "a1", "parentId": null, "timestamp": AT, "message": {"role": "user", "content": "Feed the walrus"}}),
            json!({"type": "message", "id": "a2", "parentId": "a1", "timestamp": AT, "message": {"role": "assistant", "content": [{"type": "text", "text": "Fed."}]}}),
        ],
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sesh-agent-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, lines: &[Value]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Pi];

#[test]
fn each_agent_is_known_by_its_name_and_by_its_transcripts_first_line() {
    for agent in ALL {
        assert_eq!(Agent::named(agent.name()), Some(agent));
        assert_eq!(Agent::of(&json!({"agent": agent.name()})), Some(agent));
        assert_eq!(Agent::wrote(&exchange(agent)[0]), agent);
    }
    assert_eq!(Agent::of(&json!({"agent": "aider"})), None);
    assert_eq!(Agent::of(&json!({})), None);
}

#[test]
fn a_session_id_finds_claudes_and_codexs_transcripts_but_never_pis() {
    let scratch = Scratch::new("find");
    let claude = scratch.write("claude/-home-u-walrus/abc.jsonl", &[]);
    let codex = scratch.write("codex/2026/10/05/rollout-2026-10-05T10-00-00-abc.jsonl", &[]);
    assert_eq!(Agent::Claude.find(&scratch.0.join("claude"), "abc"), Some(claude));
    assert_eq!(Agent::Codex.find(&scratch.0.join("codex"), "abc"), Some(codex));
    assert_eq!(Agent::Codex.find(&scratch.0.join("codex"), "xyz"), None);
    assert_eq!(Agent::Pi.locate("abc"), None);
    let reported = json!({"agent_session": {"agent": "pi", "kind": "path", "value": "/home/u/.pi/s.jsonl"}});
    assert_eq!(transcript_path(&reported).as_deref(), Some("/home/u/.pi/s.jsonl"));
    assert_eq!(transcript_path(&json!({"agent_session": {"agent": "pi", "kind": "id", "value": "abc"}})), None);
}

#[test]
fn each_agent_reads_its_own_transcript_as_a_message_and_a_reply() {
    let scratch = Scratch::new("parse");
    for agent in ALL {
        let mut transcript = Transcript::new(agent, scratch.write(&format!("{}.jsonl", agent.name()), &exchange(agent)));
        transcript.read(None).unwrap();
        let shown: Vec<(&str, Option<&str>)> = transcript.entries.iter().map(|entry| (entry.kind, entry.text.as_deref())).collect();
        assert_eq!(shown, [("user", Some("Feed the walrus")), ("text", Some("Fed."))], "{agent:?}");
    }
}

#[test]
fn only_claude_waits_on_background_work_once_its_turn_is_over() {
    let scratch = Scratch::new("waiting");
    let started = json!({"type": "assistant", "timestamp": AT, "message": {"stop_reason": "tool_use", "content": [{"type": "tool_use",
        "id": "toolu_1", "name": "Bash", "input": {"command": "sleep 600", "description": "Wait", "run_in_background": true}}]}});
    for agent in ALL {
        let mut lines = vec![started.clone()];
        lines.extend(exchange(agent));
        let mut transcript = Transcript::new(agent, scratch.write(&format!("{}.jsonl", agent.name()), &lines));
        transcript.read(None).unwrap();
        assert_eq!(transcript.waiting(), agent == Agent::Claude, "{agent:?}");
    }
    let mut busy = Transcript::new(Agent::Claude, scratch.write("busy.jsonl", &[started]));
    busy.read(None).unwrap();
    assert_eq!((busy.background().len(), busy.waiting()), (1, false));
}

#[test]
fn a_codex_copy_read_in_pieces_starts_again_once_it_shows_completed_items() {
    let scratch = Scratch::new("hint");
    let lines = exchange(Agent::Codex);
    let path = scratch.write("rollout-1.jsonl", &lines);
    let mut transcript = Transcript::new(Agent::Codex, &path);
    assert_eq!(transcript.read_from(0, 0).unwrap().len(), 2);
    let (offset, hint) = (transcript.offset, transcript.hint());
    let done = json!({"timestamp": AT, "type": "event_msg", "payload": {"type": "item_completed",
        "item": {"type": "UserMessage", "id": "u1", "content": [{"type": "text", "text": "Feed the walrus"}]}}});
    scratch.write("rollout-1.jsonl", &[&lines[..], &[done]].concat());
    let entries = transcript.read_from(offset, hint).unwrap();
    assert_eq!((transcript.start, entries.len(), transcript.hint()), (0, 1, 1));

    let claude = scratch.write("claude.jsonl", &exchange(Agent::Claude));
    let mut transcript = Transcript::new(Agent::Claude, &claude);
    transcript.read_from(0, 0).unwrap();
    let offset = transcript.offset;
    assert!(transcript.read_from(offset, 0).unwrap().is_empty());
    assert_eq!((transcript.start, transcript.hint()), (offset, 0));
}

#[test]
fn each_agent_answers_permissions_with_its_own_keys() {
    assert_eq!(Agent::Claude.permit(true), Some(("1", "Esc to cancel · Tab to amend")));
    assert_eq!(Agent::Codex.permit(false), Some(("esc", "Press enter to confirm or esc to cancel")));
    assert_eq!(Agent::Pi.permit(true), None);
    assert!(Agent::Claude.question_menu().is_some() && Agent::Codex.question_menu().is_none());
    assert_eq!(ALL.map(Agent::reads_screen), [true, false, false]);
}

#[test]
fn permission_lines_name_the_command_or_file() {
    assert_eq!(
        Agent::Claude.permission("1", "Bash", &json!({"command": "touch probe.txt", "description": "Create probe.txt"})),
        json!({"t": "permission", "id": "1", "tool": "Bash", "summary": "Bash: touch probe.txt", "command": "touch probe.txt"})
    );
    assert_eq!(
        Agent::Codex.permission("2", "Bash", &json!({"command": "touch probe.txt", "description": "The sandbox blocked it."})),
        json!({"t": "permission", "id": "2", "tool": "Bash", "summary": "Bash: touch probe.txt", "command": "touch probe.txt", "reason": "The sandbox blocked it."})
    );
    assert_eq!(
        Agent::Codex.permission("3", "apply_patch", &json!({"command": "*** Begin Patch\n*** Update File: /home/u/notes.txt\n@@\n-a\n+b\n*** End Patch"}))["file"],
        "/home/u/notes.txt"
    );
}
