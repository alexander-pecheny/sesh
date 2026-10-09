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
fn claude_wraps_its_markdown_so_marks_count_toward_the_width() {
    let streaming = view("link-streaming");
    let replies = replies(&streaming);
    assert_eq!(replies[0], "Floating point sacrifices exactness for huge range (0.1 isn't stored exactly), but IEEE 754 hardware support makes it fast—decimal types are worth the tradeoff mainly for money calculations.", "the grey label goes");
    let slide = replies.iter().find(|reply| reply.starts_with("The slide rule")).unwrap();
    assert!(!slide.contains('\n'), "{slide}");
    assert!(slide.contains("**William Oughtred** set two such scales"));
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
fn a_menu_no_hook_reported_is_read_off_the_screen() {
    let screen = "⏺ Bash(rm -rf build)\n\n────────────────────────────────\n Bash command\n\n   rm -rf build\n   Remove the build folder\n\n This command was flagged for review.\n Do you want to proceed?\n ❯ 1. Yes\n   2. No, and tell Claude what to do differently (esc)\n\n Esc to cancel · Tab to amend\n";
    let menu = Menu::read(screen).unwrap();
    assert_eq!(menu.title, "Bash command\nrm -rf build\nRemove the build folder\nThis command was flagged for review.\nDo you want to proceed?");
    assert_eq!(menu.options, vec![("1".into(), "Yes".into()), ("2".into(), "No, and tell Claude what to do differently (esc)".into())]);
    assert_eq!(Menu::read("────────────────\n❯ \n────────────────\n"), None);
}
