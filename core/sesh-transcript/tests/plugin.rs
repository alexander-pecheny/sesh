//! An Agent described by a file in `SESH_AGENTS`, whose Transcript and menu a plugin reads.

use std::os::unix::fs::PermissionsExt;

use sesh_transcript::{Agent, Transcript};

const PLUGIN: &str = r#"#!/bin/sh
while read -r message; do
    case "$message" in
        *'"end"'*) ;;
        *'"screen"'*) echo '{"menu": {"title": "Go on?", "options": [["enter", "Yes"], ["down enter", "No"]]}}' ;;
        *'"call"'*) echo '{"entries": [{"kind": "tool", "call": "c1", "name": "bash", "input": {"command": "ls"}, "at": "t"}, {"kind": "result", "call": "c1", "output": "a b", "at": "t"}], "turn_over": true}' ;;
        *) echo '{"entries": [{"kind": "user", "text": "Feed the walrus", "at": "t"}], "turn_over": false}' ;;
    esac
done
"#;

#[test]
fn a_plugin_reads_its_agents_transcript_and_menu() {
    let dir = std::env::temp_dir().join(format!("sesh-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("walrus.toml"), "name = \"walrus\"\ncommand = [\"walrus\"]\nplugin = [\"./walrus-plugin\"]\n[transcript]\nfirst = { kind = \"walrus\" }\n").unwrap();
    std::fs::write(dir.join("walrus-plugin"), PLUGIN).unwrap();
    std::fs::set_permissions(dir.join("walrus-plugin"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(dir.join("t.jsonl"), "{\"kind\": \"walrus\"}\n{\"call\": 1}\n").unwrap();
    std::env::set_var("SESH_AGENTS", &dir);

    let agent = Agent::named("walrus").unwrap();
    assert_eq!(Agent::wrote(&serde_json::json!({"kind": "walrus"})), agent);
    let mut transcript = Transcript::new(agent, dir.join("t.jsonl"));
    transcript.read(None).unwrap();
    let menu = agent.menu("whatever the screen shows").unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    let shown: Vec<(&str, Option<&str>)> = transcript.entries.iter().map(|entry| (entry.kind, entry.text.as_deref())).collect();
    assert_eq!(shown, [("user", Some("Feed the walrus")), ("tool", None), ("result", Some("a b"))]);
    assert_eq!(transcript.entries[2].call.as_ref(), Some(&transcript.entries[1].id));
    assert_eq!(menu.title, "Go on?");
    assert_eq!(menu.options, [("enter".to_string(), "Yes".to_string()), ("down enter".to_string(), "No".to_string())]);
}
