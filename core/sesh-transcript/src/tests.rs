use std::path::PathBuf;

use serde_json::json;

use super::*;

/// Anonymised excerpts of real Transcripts, one JSON value per line.
fn parse(agent: &str, lines: &[Value]) -> Transcript {
    let mut transcript = Transcript::new(agent, "/home/u/t.jsonl").unwrap();
    let bytes: String = lines.iter().map(|line| format!("{line}\n")).collect();
    transcript.feed(bytes.as_bytes());
    transcript
}

fn kinds(transcript: &Transcript) -> Vec<String> {
    transcript
        .entries
        .iter()
        .map(|entry| match entry.tool {
            Some(tool) => format!("tool:{tool}"),
            None => entry.kind.to_string(),
        })
        .collect()
}

fn claude_lines() -> Vec<Value> {
    let at = "2026-10-05T14:54:42.186Z";
    vec![
        json!({"type":"user","timestamp":at,"origin":{"kind":"human"},"message":{"role":"user","content":"Fix the greeting in /home/u/shot.png please"}}),
        json!({"type":"user","timestamp":at,"isMeta":true,"message":{"role":"user","content":[{"type":"text","text":"[Image: source: /home/u/shot.png]"}]}}),
        json!({"type":"user","timestamp":at,"origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification>done</task-notification>"}}),
        json!({"type":"user","timestamp":at,"message":{"role":"user","content":"<command-name>/compact</command-name>\n<command-message>compact</command-message>\n<command-args></command-args>"}}),
        json!({"type":"user","timestamp":at,"message":{"role":"user","content":"<local-command-stdout>Compacted</local-command-stdout>"}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"thinking","thinking":"","signature":"x"}]}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"thinking","thinking":"Read it first."},{"type":"text","text":"Let me edit notes.txt."}]}}),
        json!({"type":"assistant","timestamp":at,"isSidechain":true,"message":{"content":[{"type":"text","text":"subagent chatter"}]}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Edit","input":{"file_path":"/home/u/notes.txt","old_string":"hello","new_string":"hello world","replace_all":false}}]}}),
        json!({"type":"user","timestamp":at,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"The file /home/u/notes.txt has been updated successfully."}]},
            "toolUseResult":{"filePath":"/home/u/notes.txt","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-hello","+hello world"]}]}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"touch probe.txt","description":"Create probe.txt"}}]}}),
        json!({"type":"user","timestamp":at,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_2","content":"The user doesn't want to proceed with this tool use.","is_error":true}]}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":"toolu_3","name":"TaskCreate","input":{"subject":"Apply the stack","description":"..."}}]}}),
        json!({"type":"user","timestamp":at,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_3","content":"Task #1 created successfully"}]}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":"toolu_4","name":"TaskCreate","input":{"subject":"Push it"}}]}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":"toolu_5","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}),
        json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":"toolu_6","name":"AskUserQuestion","input":{"questions":[
            {"question":"Tea or coffee?","header":"Drink","multiSelect":false,"options":[{"label":"Tea","description":"Leaves"},{"label":"Coffee","description":"Beans"}]}]}}]}}),
        json!({"type":"user","timestamp":at,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_6","content":"The user answered: \"Tea or coffee?\"=\"Coffee\"."}]},
            "toolUseResult":{"questions":[{"question":"Tea or coffee?"}],"answers":{"Tea or coffee?":"Coffee"}}}),
    ]
}

#[test]
fn claude_transcript_becomes_entries() {
    let transcript = parse("claude", &claude_lines());
    assert_eq!(
        kinds(&transcript),
        [
            "user",
            "user",
            "thinking",
            "text",
            "tool:edit",
            "result",
            "tool:bash",
            "result",
            "todo",
            "todo",
            "todo",
            "question",
            "result"
        ]
    );
    let entries = &transcript.entries;
    assert_eq!(
        entries[0].images.as_deref(),
        Some(&["/home/u/shot.png".to_string()][..])
    );
    assert_eq!(entries[1].text.as_deref(), Some("/compact"));

    let edit = &entries[4];
    assert_eq!(edit.file.as_deref(), Some("/home/u/notes.txt"));
    assert_eq!(edit.summary, "Edit: /home/u/notes.txt");
    let result = &entries[5];
    assert_eq!(result.call.as_deref(), Some(edit.id.as_str()));
    assert_eq!(
        result.diff.as_deref(),
        Some(
            "--- /home/u/notes.txt\n+++ /home/u/notes.txt\n@@ -1,1 +1,1 @@\n-hello\n+hello world\n"
        )
    );
    assert_eq!((result.added, result.removed), (Some(1), Some(1)));

    assert_eq!(entries[6].command.as_deref(), Some("touch probe.txt"));
    assert_eq!(entries[7].error, Some(true));

    assert_eq!(
        entries[10].items,
        Some(json!([
            {"text": "Apply the stack", "status": "completed"},
            {"text": "Push it", "status": "pending"}
        ]))
    );
    assert_eq!(entries[10].summary, "1 of 2 tasks done");

    let question = &entries[11];
    assert_eq!(
        question.questions,
        Some(
            json!([{"question": "Tea or coffee?", "header": "Drink", "multi": false,
            "options": [{"label": "Tea", "description": "Leaves"}, {"label": "Coffee", "description": "Beans"}]}])
        )
    );
    assert_eq!(entries[12].call.as_deref(), Some(question.id.as_str()));
    assert_eq!(entries[12].answers, Some(vec!["Coffee".to_string()]));

    let hooked = transcript.open_question(&claude_lines()[16]["message"]["content"][0]["input"]);
    assert_eq!(hooked.id, question.id);
    assert_eq!(hooked.questions, question.questions);
}

#[test]
fn codex_response_items_become_entries() {
    let at = "2026-06-03T11:54:39.234Z";
    let transcript = parse(
        "codex",
        &[
            json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions"}]}}),
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"user_message","message":"Why is this here?","images":[],"local_images":["/home/u/a.png"]}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"gAAA"}}),
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"agent_message","message":"I'll look.","phase":"commentary"}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"I'll look."}]}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"sed -n '1,40p' a.py\",\"workdir\":\"/home/u\"}","call_id":"call_1"}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"Chunk ID: d3ebcf\nWall time: 0.0000 seconds\nProcess exited with code 1\nOriginal token count: 5\nOutput:\nsed: a.py: No such file\n"}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"custom_tool_call","status":"completed","call_id":"call_2","name":"apply_patch","input":"*** Begin Patch\n*** Update File: /home/u/a.py\n@@\n-x = 1\n+x = 2\n*** End Patch"}}),
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"patch_apply_end","call_id":"call_2","success":true,"changes":{"/home/u/a.py":{"type":"update","unified_diff":"@@ -1 +1 @@\n-x = 1\n+x = 2\n"}}}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call_2","output":"Exit code: 0\nWall time: 0 seconds\nOutput:\nSuccess. Updated the following files:\nM /home/u/a.py\n"}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"function_call","name":"update_plan","arguments":"{\"plan\":[{\"step\":\"look\",\"status\":\"completed\"},{\"step\":\"fix\",\"status\":\"in_progress\"}]}","call_id":"call_3"}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"function_call_output","call_id":"call_3","output":"Plan updated"}}),
        ],
    );
    assert_eq!(
        kinds(&transcript),
        [
            "user",
            "text",
            "tool:bash",
            "result",
            "tool:edit",
            "result",
            "todo"
        ]
    );
    let entries = &transcript.entries;
    assert_eq!(
        entries[0].images.as_deref(),
        Some(&["/home/u/a.png".to_string()][..])
    );
    assert_eq!(entries[2].command.as_deref(), Some("sed -n '1,40p' a.py"));
    assert_eq!(
        entries[3].text.as_deref(),
        Some("sed: a.py: No such file\n")
    );
    assert_eq!(entries[3].error, Some(true));
    assert_eq!(entries[4].file.as_deref(), Some("/home/u/a.py"));
    assert_eq!(entries[5].error, Some(false));
    assert_eq!(
        entries[5].diff.as_deref(),
        Some("--- /home/u/a.py\n+++ /home/u/a.py\n@@ -1 +1 @@\n-x = 1\n+x = 2\n")
    );
    assert_eq!(entries[6].summary, "1 of 2 tasks done");
}

#[test]
fn codex_completed_items_replace_response_items() {
    let at = "2026-10-05T14:58:54.424Z";
    let item = |item: Value| json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed","item":item}});
    let transcript = parse(
        "codex",
        &[
            json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Run ls"}]}}),
            item(
                json!({"type":"UserMessage","id":"u1","content":[{"type":"text","text":"Run ls","text_elements":[]}]}),
            ),
            item(
                json!({"type":"AgentMessage","id":"m1","content":[{"type":"Text","text":"Running it."}],"phase":"commentary"}),
            ),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Running it."}]}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"custom_tool_call","call_id":"c1","name":"exec","input":"text(await tools.exec_command({cmd:\"ls\"}))"}}),
            item(json!({"type":"Reasoning","id":"r1","summary_text":[],"raw_content":[]})),
            item(
                json!({"type":"CommandExecution","id":"e1","command":["/bin/zsh","-lc","cat notes.txt"],"parsed_cmd":[{"type":"read","cmd":"cat notes.txt","name":"notes.txt"}],"status":"completed","aggregated_output":"hello\n","exit_code":0}),
            ),
            item(
                json!({"type":"FileChange","id":"f1","changes":{"/home/u/notes.txt":{"type":"update","unified_diff":"@@ -1 +1 @@\n-hello\n+hi\n","move_path":null}},"status":"failed","stdout":"","stderr":"execution error: TurnAborted"}),
            ),
        ],
    );
    assert_eq!(
        kinds(&transcript),
        ["user", "text", "tool:read", "result", "tool:edit", "result"]
    );
    let entries = &transcript.entries;
    assert_eq!(entries[2].command.as_deref(), Some("cat notes.txt"));
    assert_eq!(entries[3].text.as_deref(), Some("hello\n"));
    assert_eq!(entries[3].call.as_deref(), Some(entries[2].id.as_str()));
    assert_eq!(entries[4].file.as_deref(), Some("/home/u/notes.txt"));
    assert_eq!(entries[5].error, Some(true));
    assert_eq!((entries[5].added, entries[5].removed), (Some(1), Some(1)));
}

fn pi_line(id: &str, parent: Option<&str>, message: Value) -> Value {
    json!({"type":"message","id":id,"parentId":parent,"timestamp":"2026-09-27T19:57:36.729Z","message":message})
}

#[test]
fn pi_follows_the_branch_ending_at_the_newest_entry() {
    let lines = [
        json!({"type":"session","version":3,"id":"01a0e473","timestamp":"2026-09-27T19:59:49.929Z","cwd":"/home/u"}),
        json!({"type":"model_change","id":"m0","parentId":null,"provider":"local","modelId":"q"}),
        pi_line(
            "a1",
            Some("m0"),
            json!({"role":"user","content":[{"type":"text","text":"Fix stats.py"}]}),
        ),
        pi_line(
            "a2",
            Some("a1"),
            json!({"role":"assistant","content":[
            {"type":"thinking","thinking":"\nLook first.\n"},{"type":"text","text":"\n\n"},
            {"type":"toolCall","id":"call_1","name":"edit","arguments":{"path":"/home/u/stats.py","edits":[]}}]}),
        ),
        pi_line(
            "a3",
            Some("a2"),
            json!({"role":"toolResult","toolCallId":"call_1","toolName":"edit","content":[{"type":"text","text":"Edited"}],"isError":false,
            "details":{"patch":"--- /home/u/stats.py\n+++ /home/u/stats.py\n@@ -1,1 +1,1 @@\n-x + 1\n+x\n"}}),
        ),
    ];
    let mut transcript = parse("pi", &lines);
    assert_eq!(
        kinds(&transcript),
        ["user", "thinking", "tool:edit", "result"]
    );
    assert_eq!(transcript.entries[1].text.as_deref(), Some("Look first."));
    assert_eq!(transcript.entries[3].added, Some(1));

    let continued = pi_line(
        "a4",
        Some("a3"),
        json!({"role":"assistant","content":[{"type":"text","text":"Done."}]}),
    );
    assert!(!transcript.feed(format!("{continued}\n").as_bytes()));
    assert_eq!(transcript.entries.len(), 5);

    let branch = pi_line(
        "b1",
        Some("a1"),
        json!({"role":"user","content":"Actually, explain it"}),
    );
    assert!(transcript.feed(format!("{branch}\n").as_bytes()));
    assert_eq!(kinds(&transcript), ["user", "user"]);
    assert_eq!(
        transcript.entries[1].text.as_deref(),
        Some("Actually, explain it")
    );
    assert!(transcript
        .find(&transcript_entry_id(&transcript, "Done."))
        .is_none());
}

fn transcript_entry_id(transcript: &Transcript, text: &str) -> String {
    transcript
        .entries
        .iter()
        .find(|entry| entry.text.as_deref() == Some(text))
        .map(|entry| entry.id.clone())
        .unwrap_or_default()
}

#[test]
fn reading_resumes_at_a_cursor_without_repeating_or_missing_entries() {
    let dir = std::env::temp_dir().join(format!("herdr-conversation-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.jsonl");
    let lines: Vec<String> = claude_lines()
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&path, lines[..6].concat()).unwrap();

    let mut first = Transcript::new("claude", &path).unwrap();
    first.read(None).unwrap();
    let cursor = first.offset;
    let seen: Vec<String> = first.entries.iter().map(|entry| entry.id.clone()).collect();

    let partial = &lines[6][..10];
    std::fs::write(&path, [lines[..6].concat(), partial.to_string()].concat()).unwrap();
    first.read(None).unwrap();
    assert_eq!(first.offset, cursor, "a partial line waits for its newline");

    std::fs::write(&path, lines.concat()).unwrap();
    let mut resumed = Transcript::new("claude", &path).unwrap();
    resumed.read(Some(cursor)).unwrap();
    let mark = resumed.entries.len();
    resumed.read(None).unwrap();
    let new: Vec<String> = resumed.entries[mark..]
        .iter()
        .map(|entry| entry.id.clone())
        .collect();

    let mut whole = Transcript::new("claude", &path).unwrap();
    whole.read(None).unwrap();
    let all: Vec<String> = whole.entries.iter().map(|entry| entry.id.clone()).collect();
    assert_eq!([seen, new].concat(), all);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_copy_gives_the_same_ids_as_its_source() {
    let lines = &claude_lines();
    let source = parse("claude", lines);
    let mut copy = Transcript::new("claude", "/vault/transcripts/s1/t.jsonl").unwrap();
    let bytes: String = lines.iter().map(|line| format!("{line}\n")).collect();
    copy.feed(bytes.as_bytes());
    assert_eq!(ids(&copy.entries), ids(&source.entries));
    let mut other = Transcript::new("claude", "/home/u/u.jsonl").unwrap();
    other.feed(bytes.as_bytes());
    assert_ne!(ids(&other.entries), ids(&source.entries));
}

#[test]
fn results_are_clipped_to_their_ends() {
    let long: String = (0..100).map(|line| format!("line {line}\n")).collect();
    let entry = Entry::result("r".into(), &Value::Null, "c".into(), long.clone(), false);
    let clipped = entry.clipped();
    assert_eq!(clipped.truncated, Some(true));
    let text = clipped.text.unwrap();
    assert!(text.starts_with("line 0\n") && text.ends_with("line 99\n"));
    assert_eq!(text.lines().count(), 80);
    assert!(!text.contains("line 40\n"));
    assert_eq!(entry.text.as_deref(), Some(long.as_str()));

    let wide = "é".repeat(20_000);
    let clipped = Entry::result("r".into(), &Value::Null, "c".into(), wide, false).clipped();
    assert!(clipped.text.unwrap().len() <= CLIP_BYTES);

    let short = Entry::result("r".into(), &Value::Null, "c".into(), "ok".into(), false);
    assert_eq!(short.clipped().truncated, Some(false));
}

#[test]
fn permission_lines_name_the_command_or_file() {
    assert_eq!(
        permission(
            "claude",
            "1",
            "Bash",
            &json!({"command": "touch probe.txt", "description": "Create probe.txt"})
        ),
        json!({"t": "permission", "id": "1", "tool": "Bash", "summary": "Bash: touch probe.txt", "command": "touch probe.txt"})
    );
    assert_eq!(
        permission(
            "codex",
            "2",
            "Bash",
            &json!({"command": "touch probe.txt", "description": "The sandbox blocked it."})
        ),
        json!({"t": "permission", "id": "2", "tool": "Bash", "summary": "Bash: touch probe.txt", "command": "touch probe.txt", "reason": "The sandbox blocked it."})
    );
    assert_eq!(
        permission(
            "codex",
            "3",
            "apply_patch",
            &json!({"command": "*** Begin Patch\n*** Update File: /home/u/notes.txt\n@@\n-a\n+b\n*** End Patch"})
        )["file"],
        "/home/u/notes.txt"
    );
}

fn write_transcript(name: &str, lines: &[Value]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("herdr-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.jsonl");
    let bytes: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(&path, bytes).unwrap();
    path
}

fn ids(entries: &[Entry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id.clone()).collect()
}

/// Rounds of a typed message, a Bash call and its result.
fn claude_rounds(rounds: usize) -> Vec<Value> {
    let at = "2026-10-05T14:54:42.186Z";
    (0..rounds)
        .flat_map(|round| {
            let call = format!("toolu_{round}");
            [
                json!({"type":"user","timestamp":at,"origin":{"kind":"human"},"message":{"role":"user","content":format!("Step {round}")}}),
                json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":call,"name":"Bash","input":{"command":format!("echo {round}")}}]}}),
                json!({"type":"user","timestamp":at,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":call,"content":format!("{round}")}]}}),
            ]
        })
        .collect()
}

#[test]
fn a_tail_read_grows_its_window_until_it_holds_enough_entries() {
    let path = write_transcript("tail", &claude_rounds(20));
    let mut whole = Transcript::new("claude", &path).unwrap();
    whole.read(None).unwrap();
    assert_eq!(whole.entries.len(), 60);

    let mut tail = Transcript::new("claude", &path).unwrap();
    tail.window = 64;
    tail.read_tail(None, |entries| entries.len() >= 7).unwrap();
    assert!(tail.start > 0);
    assert!(tail.entries.len() >= 7 && tail.entries.len() < 60);
    let shown = &tail.entries[tail.entries.len() - 7..];
    assert_eq!(shown, &whole.entries[53..]);
    assert_eq!(tail.offset, whole.offset);

    tail.read_tail(None, |entries| entries.len() >= 1000)
        .unwrap();
    assert_eq!(tail.start, 0);
    assert_eq!(tail.entries, whole.entries);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn history_pages_back_to_the_start() {
    let path = write_transcript("history", &claude_rounds(10));
    let mut whole = Transcript::new("claude", &path).unwrap();
    whole.read(None).unwrap();

    let mut transcript = Transcript::new("claude", &path).unwrap();
    transcript.window = 64;
    transcript
        .read_tail(None, |entries| entries.len() >= 4)
        .unwrap();
    let mut seen = transcript.entries[transcript.entries.len() - 4..].to_vec();
    loop {
        let (page, more) = transcript.history(&seen[0].id, 4).unwrap().unwrap();
        assert!(page.len() <= 4);
        seen.splice(0..0, page);
        if !more {
            break;
        }
    }
    assert_eq!(ids(&seen), ids(&whole.entries));
    assert_eq!(
        transcript.history(&seen[0].id, 4).unwrap(),
        Some((Vec::new(), false))
    );
    assert!(transcript.history("feedface.0.0", 4).unwrap().is_none());
    let unknown = format!("{}.call.toolu_none", &seen[0].id[..8]);
    assert!(transcript.history(&unknown, 4).unwrap().is_none());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn results_name_calls_outside_the_window() {
    let at = "2026-10-05T14:54:42.186Z";
    let cases = [
        (
            "claude",
            json!({"type":"assistant","timestamp":at,"message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}),
            json!({"type":"user","timestamp":at,"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"a"}]}}),
        ),
        (
            "codex",
            json!({"timestamp":at,"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls\"}","call_id":"call_1"}}),
            json!({"timestamp":at,"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"Process exited with code 0\nOutput:\na\n"}}),
        ),
        (
            "pi",
            pi_line(
                "a1",
                None,
                json!({"role":"assistant","content":[{"type":"toolCall","id":"call_1","name":"bash","arguments":{"command":"ls"}}]}),
            ),
            pi_line(
                "a2",
                Some("a1"),
                json!({"role":"toolResult","toolCallId":"call_1","toolName":"bash","content":[{"type":"text","text":"a"}],"isError":false}),
            ),
        ),
    ];
    for (agent, call, result) in cases {
        let path = write_transcript(&format!("link-{agent}"), &[call, result.clone()]);
        let mut whole = Transcript::new(agent, &path).unwrap();
        whole.read(None).unwrap();
        assert_eq!(kinds(&whole), ["tool:bash", "result"], "{agent}");

        let mut tail = Transcript::new(agent, &path).unwrap();
        tail.window = result.to_string().len() as u64 + 1;
        tail.read_tail(None, |entries| !entries.is_empty()).unwrap();
        assert_eq!(kinds(&tail), ["result"], "{agent}");
        assert_eq!(
            tail.entries[0].call.as_deref(),
            Some(whole.entries[0].id.as_str())
        );
        assert!(whole.entries[0].id.ends_with(".call.call_1") || agent == "claude");

        let (page, more) = tail.history(&whole.entries[1].id, 5).unwrap().unwrap();
        assert_eq!(
            (ids(&page), more),
            (ids(&whole.entries[..1]), false),
            "{agent}"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

#[test]
fn pi_follows_its_branch_within_a_window() {
    let text = |text: &str| json!({"role":"assistant","content":[{"type":"text","text":text}]});
    let lines = [
        pi_line("a1", None, json!({"role":"user","content":"Start"})),
        pi_line("a2", Some("a1"), text("Left")),
        pi_line("b1", Some("a1"), text("Right")),
        pi_line("b2", Some("b1"), text("Right again")),
    ];
    let path = write_transcript("pi-window", &lines);
    let texts = |transcript: &Transcript| -> Vec<String> {
        transcript
            .entries
            .iter()
            .filter_map(|entry| entry.text.clone())
            .collect()
    };

    let mut transcript = Transcript::new("pi", &path).unwrap();
    transcript.window = [&lines[1], &lines[2], &lines[3]]
        .iter()
        .map(|line| line.to_string().len() as u64 + 1)
        .sum();
    transcript
        .read_tail(None, |entries| entries.len() >= 2)
        .unwrap();
    assert!(transcript.start > 0);
    assert_eq!(texts(&transcript), ["Right", "Right again"]);

    transcript
        .read_tail(None, |entries| entries.len() >= 3)
        .unwrap();
    assert_eq!(texts(&transcript), ["Start", "Right", "Right again"]);

    let right = transcript.entries[1].id.clone();
    let (page, more) = transcript.history(&right, 5).unwrap().unwrap();
    assert_eq!((ids(&page), more), (ids(&transcript.entries[..1]), false));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
