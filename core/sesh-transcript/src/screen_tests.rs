//! Screens captured from Claude Code 2.1.288 to 2.1.293 with `herdr pane read --format ansi`,
//! on the Mac (`⏺`) and on Linux (`●`).

use super::*;

fn screen(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/screens/{name}.ansi", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn view(name: &str) -> View {
    View::read(&screen(name)).unwrap()
}

fn replies(view: &View) -> Vec<String> {
    view.blocks
        .iter()
        .filter(|block| block.kind == Kind::Reply && !block.cut)
        .map(|block| markdown(&block.rows, view.width).0)
        .collect()
}

fn said(kind: &'static str, text: &str) -> Entry {
    Entry { kind, text: Some(text.into()), ..Entry::default() }
}

fn bash(command: &str) -> Entry {
    Entry { kind: "tool", tool: Some("bash"), command: Some(command.into()), ..Entry::default() }
}

fn items(live: &Live, now: Instant) -> Vec<Value> {
    live.line(now)["items"].as_array().unwrap().clone()
}

const ABOUT: &str = "**About calc.py**\n\n**calc.py** is an untracked file in this repository, next to a README.md. I have not opened it yet, so I can say only what its name suggests: a small `calc` module of arithmetic helpers. The [Python docs](about:blank) describe the conventions I will follow when I extend it.\n\nThree things matter before I change it:\n\n- The file has never been committed, so git holds no earlier copy.\n- Any new function should match the style already in the file.\n- The edit should touch nothing but the new function.\n\nThe plan has three steps:\n\n1. Run a six-second shell command.\n2. Read the file.\n3. Add `sub(a, b)` with Edit.";

#[test]
fn a_reply_reads_back_as_the_markdown_claude_wrote() {
    let inline = view("inline");
    assert_eq!(
        replies(&inline)[1],
        "Inline: **bold**, *italic*, ***both***, `code`, ~~struck~~, [a link](about:blank), https://example.org/x, snake_case_name, 2 \\* 3 \\* 4, a_b_c.\n\n```\nplain text code\n  indented line\n```\n\n```bash\necho hi | grep h\n```\n\n1. First item\n2. Second item\n   1. Nested step one\n   2. Nested step two\n   - Nested bullet\n3. Third item\n\nLast line."
    );
    assert_eq!(replies(&view("streaming"))[0], ABOUT);
}

#[test]
fn headings_tables_and_wrapped_lines_read_back() {
    let running = view("running");
    assert_eq!(
        replies(&running)[0],
        "# Heading one\n\n**Heading two**\n\n**Heading three**\n\n**A whole bold line**\n\n**Heading four**\n\nPlain *italic* end."
    );
    let table = view("table");
    assert_eq!(
        replies(&table)[0],
        "| Function | Operation |\n|---|---|\n| `add(a, b)` | Returns the sum of a and b |\n| `mul(a, b)` | Returns the product of a and b |\n| `sub(a, b)` | Returns a minus b |\n\nThe file now holds three functions, with `sub` added at the end."
    );
    let linux = view("background");
    assert_eq!(
        replies(&linux)[0],
        "Building has started on both: one agent on `chgksuite` and one on `dopesuite`, working in parallel against the same database schema. I'll report when both pull requests are open."
    );
}

#[test]
fn the_status_is_the_spinner_line_and_never_a_finished_turn() {
    assert_eq!(view("streaming").status(), "Newspapering… (5s · ↓ 489 tokens)");
    assert_eq!(view("background").status(), "Waiting for 2 background agents to finish");
    assert_eq!(view("table").status(), "");
    assert_eq!(view("long-prompt").status(), "");
    assert!(View::read("no prompt box here").is_none());
}

#[test]
fn chrome_notices_and_the_footer_are_never_replies() {
    let linux = view("background");
    let texts: Vec<String> = linux.blocks.iter().flat_map(|block| block.rows.iter().map(Row::text)).collect();
    assert!(!texts.iter().any(|text| text.contains("Update installed") || text.contains("◯ fork")));
    let kinds: Vec<Kind> = linux.blocks.iter().filter(|block| !block.cut).map(|block| block.kind).collect();
    assert_eq!(kinds, [Kind::Other, Kind::User, Kind::Tool, Kind::Reply]);
    for name in ["agent", "before-redraw"] {
        for block in view(name).blocks.iter().filter(|block| block.kind == Kind::Reply) {
            let text = markdown(&block.rows, 119).0;
            assert!(!text.contains('⎿') && !text.contains("Searched") && !text.contains("finished"), "{text}");
        }
    }
}

#[test]
fn a_running_command_reads_without_its_timer() {
    let running = view("running");
    let tool = running.blocks.iter().find(|block| block.kind == Kind::Tool).unwrap();
    assert_eq!(command(tool, running.width), Some(("sleep 4; ls".to_string(), None)));
}

#[test]
fn replies_past_the_transcript_become_items_until_it_delivers_them() {
    let now = Instant::now();
    let mut live = Live::default();
    live.deliver(&said("user", "First write two short paragraphs about calc.py using **bold**, `inline code`, a bullet list of three items, a numbered list, a level-2 heading and a link to https://example.com. Then run the shell command `sleep 6 && echo done` with a description. Then read calc.py, then add a sub(a, b) function to it with Edit. Then write a small markdown table of the functions, and finish with one sentence."), now);
    live.see(view("streaming"), now);
    let shown = items(&live, now);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["kind"], "text");
    assert_eq!(shown[0]["text"], ABOUT);
    let id = shown[0]["id"].as_str().unwrap().to_string();
    // The screen scrolls: the reply's top goes, a table follows.
    live.see(view("table"), now);
    let shown = items(&live, now);
    assert_eq!(shown.len(), 2);
    assert_eq!((shown[0]["id"].as_str(), shown[0]["text"].as_str()), (Some(id.as_str()), Some(ABOUT)));
    assert!(shown[1]["text"].as_str().unwrap().starts_with("| Function | Operation |"));
    let transcript = "## About calc.py\n\n**calc.py** is an untracked file in this repository, next to a README.md. I have not opened it yet, so I can say only what its name suggests: a small `calc` module of arithmetic helpers. The [Python docs](https://example.com) describe the conventions I will follow when I extend it.";
    assert_eq!(live.deliver(&said("text", transcript), now), Some(id));
    assert_eq!(items(&live, now).len(), 1);
    live.see(view("table"), now);
    assert_eq!(items(&live, now).len(), 1, "a delivered reply still on the screen comes back");
}

#[test]
fn a_command_shows_once_the_transcript_has_had_time_to_name_it() {
    let now = Instant::now();
    let mut live = Live::default();
    for text in ["Step 1: I will check whether TodoWrite is available, since it is not in my tool list.", "TodoWrite does not exist in this session, so I skip step 1 and move to step 2."] {
        live.deliver(&said("text", text), now);
    }
    live.see(view("running"), now);
    assert!(items(&live, now).is_empty());
    let later = now + TOOL_DELAY;
    let shown = items(&live, later);
    assert_eq!((shown[0]["tool"].as_str(), shown[0]["command"].as_str()), (Some("bash"), Some("sleep 4; ls")));
    let id = shown[0]["id"].as_str().map(str::to_string);
    assert_eq!(live.deliver(&bash("sleep 4; ls"), later), id);
    assert!(items(&live, later).is_empty());
    // One the Transcript names in time never shows.
    live.see(view("running"), later);
    assert!(items(&live, later + TOOL_DELAY).is_empty());
}

#[test]
fn a_half_drawn_screen_counts_as_a_change_and_items_end_after_the_turn() {
    let now = Instant::now();
    let mut live = Live::default();
    live.see(view("before-redraw"), now);
    assert!(live.changed(&view("redraw")));
    assert!(!live.changed(&view("before-redraw")));
    assert!(!items(&live, now).is_empty());
    live.working(false, now);
    assert!(!items(&live, now).is_empty() && live.watching(now));
    live.working(false, now + GRACE);
    assert!(items(&live, now + GRACE).is_empty() && !live.watching(now + GRACE));
}

#[test]
fn words_drop_marks_addresses_and_list_markers() {
    assert_eq!(words("## A [link](https://x.y/z) and `code`\n\n1. one\n   a. two"), "alinkandcodeonetwo");
    assert_eq!(words(ABOUT), words(&ABOUT.replace("about:blank", "https://example.com")));
}

#[test]
fn claude_wraps_its_markdown_so_marks_count_toward_the_width() {
    let streaming = view("link-streaming");
    let replies = replies(&streaming);
    assert_eq!(replies[0], "Floating point sacrifices exactness for huge range (0.1 isn't stored exactly), but IEEE 754 hardware support makes it fast—decimal types are worth the tradeoff mainly for money calculations.", "the grey label goes");
    let slide = replies.iter().find(|reply| reply.starts_with("The slide rule")).unwrap();
    assert!(!slide.contains('\n'), "{slide}");
    assert!(slide.contains("**William Oughtred** set two such scales"));
}

#[test]
fn the_spinner_word_stays_while_a_long_reply_hides_the_spinner() {
    let now = Instant::now();
    let mut live = Live::default();
    live.see(view("streaming"), now);
    live.see(view("no-spinner"), now);
    assert_eq!(live.line(now)["status"], "Newspapering…");
}

#[test]
fn a_code_block_with_blank_lines_stays_one_block() {
    let code = view("code");
    let block = code.blocks.iter().find(|block| block.kind == Kind::Reply).unwrap();
    let text = markdown(&block.rows, code.width).0;
    assert_eq!(text.matches("```").count(), 2, "{text}");
    assert!(text.contains("```python\nclass Table:\n    def __init__(self, size=8):"), "{text}");
    assert!(text.contains("\n\n    def put(self, key, value):") && text.contains("\n\n    def get(self, key):"), "{text}");
}

#[test]
fn a_menu_in_place_of_the_prompt_box_leaves_the_conversation_above_it() {
    let question = view("question");
    let reply = replies(&question).pop().unwrap();
    assert!(reply.starts_with("Colour theory holds") && reply.ends_with("make the sharpest contrast."), "{reply}");
    assert!(!question.blocks.iter().any(|block| block.rows.iter().any(|row| row.text().contains("Warm"))));
    let permission = view("permission");
    let kinds: Vec<Kind> = permission.blocks.iter().map(|block| block.kind).collect();
    assert_eq!(kinds[kinds.len() - 2..], [Kind::Reply, Kind::Tool]);
    let tool = permission.blocks.last().unwrap();
    let described = Some("Creating empty file perm-test.txt and printing confirmation".to_string());
    assert_eq!(command(tool, permission.width), Some(("touch perm-test.txt && echo made".to_string(), described)));
}

#[test]
fn code_that_starts_with_a_dim_keyword_stays_in_its_reply() {
    let code = view("dim-code");
    let reply = code.blocks.iter().rev().find(|block| block.kind == Kind::Reply).unwrap();
    let text = markdown(&reply.rows, code.width).0;
    assert!(text.contains("The rules fit in a few lines of C:\n\n```c\nvoid on_ack(struct tcp *t) {\n    if (t->cwnd < t->ssthresh)"), "{text}");
}

#[test]
fn a_link_wrapped_across_rows_keeps_one_space() {
    let rows = rows("\x1b[38;2;255;255;255m⏺ \x1b[0mThe \x1b[38;5;12mLLVM \x1b[0m\n  \x1b[38;5;12mdocumentation\x1b[0m describes it.");
    assert_eq!(markdown(&rows, 119).0, "The [LLVM documentation](about:blank) describes it.");
}

#[test]
fn a_line_claude_hides_while_its_link_streams_stays() {
    let now = Instant::now();
    let mut live = Live::default();
    live.see(view("link-streaming"), now);
    let before = items(&live, now);
    live.see(view("link-hidden"), now);
    assert_eq!(items(&live, now), before);
    assert!(before.last().unwrap()["text"].as_str().unwrap().ends_with("More on the [history of the slide"));
}

/// `REPLAY=DIR cargo test replay -- --ignored --nocapture` plays a recording, DIR/log.jsonl
/// of screen frames and Transcript lines, through `Live`.
#[test]
#[ignore]
fn replay() {
    let dir = std::env::var("REPLAY").unwrap();
    let path = std::env::temp_dir().join(format!("replay-{}.jsonl", std::process::id()));
    // REPLAY_BEFORE names the session's Transcript, whose lines before the recording the
    // helper already knew.
    let log = std::fs::read_to_string(format!("{dir}/log.jsonl")).unwrap();
    let first_line = log.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).find_map(|event| event["line"]["uuid"].as_str().map(str::to_string));
    let before: String = std::env::var("REPLAY_BEFORE")
        .map(|file| {
            std::fs::read_to_string(file).unwrap().lines()
                .take_while(|line| first_line.as_deref().is_none_or(|uuid| !line.contains(uuid)))
                .map(|line| format!("{line}\n"))
                .collect()
        })
        .unwrap_or_default();
    std::fs::write(&path, before).unwrap();
    let mut transcript = crate::Transcript::new("claude", &path).unwrap();
    let mut live = Live::default();
    transcript.read(None).unwrap();
    for entry in &transcript.entries {
        live.deliver(entry, Instant::now());
    }
    let start = Instant::now();
    let mut first: Option<f64> = None;
    let mut last = Value::Null;
    for line in std::fs::read_to_string(format!("{dir}/log.jsonl")).unwrap().lines() {
        let event: Value = serde_json::from_str(line).unwrap();
        let t = event["t"].as_f64().unwrap();
        let at = t - *first.get_or_insert(t);
        let now = start + Duration::from_secs_f64(at);
        if let Some(line) = event.get("line") {
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes()).unwrap();
            let mark = transcript.entries.len();
            transcript.read(None).unwrap();
            for entry in &transcript.entries[mark..] {
                if let Some(id) = live.deliver(entry, now) {
                    println!("{at:7.2} replaced {id} by {} {:?}", entry.kind, entry.summary);
                }
            }
            continue;
        }
        let ansi = std::fs::read_to_string(format!("{dir}/{}", event["frame"].as_str().unwrap())).unwrap();
        let Some(view) = View::read(&ansi) else {
            println!("{at:7.2} no prompt box in {}", event["frame"]);
            continue;
        };
        live.see(view, now);
        let line = live.line(now);
        if line["items"] != last["items"] {
            let tails: Vec<String> = line["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| {
                    let text = item["text"].as_str().or(item["command"].as_str()).unwrap_or_default();
                    let tail: String = text.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
                    format!("{}:{}", item["id"].as_str().unwrap().rsplit('.').next().unwrap(), tail.replace('\n', "⏎"))
                })
                .collect();
            println!("{at:7.2} {} {tails:?}", event["frame"]);
        }
        last = line;
    }
    std::fs::remove_file(&path).unwrap();
}
