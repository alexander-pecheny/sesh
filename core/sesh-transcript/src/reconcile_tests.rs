//! The reconciler driven as the follower drives it: Transcript entries and screens in, the
//! items shown and the Session log's rows out.

use super::*;
use crate::follower::Event;
use crate::log::{Log, Writer};

fn view(name: &str) -> View {
    View::read(&std::fs::read_to_string(format!("{}/tests/screens/{name}.ansi", env!("CARGO_MANIFEST_DIR"))).unwrap()).unwrap()
}

/// A screen with one reply of Claude's above the prompt box.
fn reply(text: &str) -> View {
    let rule = "─".repeat(100);
    View::read(&format!("⏺ {text}\n\n{rule}\n❯ \n{rule}\n")).unwrap()
}

fn said(kind: &'static str, text: &str) -> Entry {
    Entry { kind, text: Some(text.into()), ..Entry::default() }
}

fn bash(command: &str) -> Entry {
    Entry { kind: "tool", tool: Some("bash"), command: Some(command.into()), ..Entry::default() }
}

fn items(live: &Reconciler, now: Instant) -> Vec<Value> {
    serde_json::to_value(live.shown(now).items).unwrap().as_array().unwrap().clone()
}

#[test]
fn replies_past_the_transcript_become_items_until_it_delivers_them() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.deliver(&said("user", "First write two short paragraphs about calc.py using **bold**, `inline code`, a bullet list of three items, a numbered list, a level-2 heading and a link to https://example.com. Then run the shell command `sleep 6 && echo done` with a description. Then read calc.py, then add a sub(a, b) function to it with Edit. Then write a small markdown table of the functions, and finish with one sentence."), now);
    live.see(view("streaming"), now);
    let shown = items(&live, now);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["kind"], "text");
    let about = shown[0]["text"].as_str().unwrap().to_string();
    assert!(about.starts_with("**About calc.py**"), "{about}");
    let id = shown[0]["id"].as_str().unwrap().to_string();
    // The screen scrolls: the reply's top goes, a table follows.
    live.see(view("table"), now);
    let shown = items(&live, now);
    assert_eq!(shown.len(), 2);
    assert_eq!((shown[0]["id"].as_str(), shown[0]["text"].as_str()), (Some(id.as_str()), Some(about.as_str())));
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
    let mut live = Reconciler::default();
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
fn a_command_run_again_in_a_later_turn_shows_again() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.deliver(&bash("sleep 4; ls"), now);
    live.deliver(&said("user", "Do these one at a time"), now);
    live.see(view("running"), now);
    let shown = items(&live, now + TOOL_DELAY);
    assert_eq!(shown.last().unwrap()["command"], "sleep 4; ls");
}

#[test]
fn a_half_drawn_screen_counts_as_a_change_and_items_end_after_the_turn() {
    let now = Instant::now();
    let mut live = Reconciler::default();
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
fn words_drop_marks_and_list_markers() {
    assert_eq!(words("## A [link](https://x.y/z) and `code`\n\n1. one\n   a. two"), "alinkandcodeonetwo");
}

#[test]
fn the_spinner_word_stays_while_a_long_reply_hides_the_spinner() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.see(view("streaming"), now);
    live.see(view("no-spinner"), now);
    assert_eq!(live.shown(now).status, "Newspapering…");
}

#[test]
fn a_line_claude_hides_while_its_link_streams_stays() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.see(view("link-streaming"), now);
    let before = items(&live, now);
    live.see(view("link-hidden"), now);
    assert_eq!(items(&live, now), before);
    assert!(before.last().unwrap()["text"].as_str().unwrap().ends_with("More on the [history of the slide"));
}

#[test]
fn a_reply_with_a_link_is_the_entry_the_transcript_delivers() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    let screen = || reply("The PR is open: #239290 (https://github.com/ppl-ai/agi/pull/239290), \"Flag friction\".");
    live.see(screen(), now);
    let id = items(&live, now)[0]["id"].as_str().map(str::to_string);
    let transcript = "The PR is open: [#239290](https://github.com/ppl-ai/agi/pull/239290), \"Flag friction\".";
    assert_eq!(live.deliver(&said("text", transcript), now), id);
    live.see(screen(), now);
    assert!(items(&live, now).is_empty(), "the reply came back below the real one");
}

#[test]
fn a_reply_that_grows_stays_one_item_and_another_starts_its_own() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.see(reply("Yes."), now);
    live.see(reply("Yesterday the build broke twice."), now);
    live.see(reply("Yesterday the build broke twice, both times on the linker."), now);
    live.see(reply("Tomorrow it should pass."), now);
    let shown: Vec<_> = items(&live, now).iter().map(|item| item["text"].as_str().unwrap().to_string()).collect();
    assert_eq!(shown, ["Yes.", "Yesterday the build broke twice, both times on the linker.", "Tomorrow it should pass."]);
}

#[test]
fn a_restarted_follower_keeps_its_rows_apart_from_the_last_ones() {
    let dir = std::env::temp_dir().join(format!("sesh-reconcile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = Log::open(&dir.join("sessions.db")).unwrap();
    let now = Instant::now();
    let entry = |id: &str, kind, text: &str| Entry { id: id.into(), ..said(kind, text) };
    // One follower's run: entries it sends again, a question, and the reply it sees, then reads.
    let follow = |again: &[Entry], user: Entry, text: Entry| {
        let mut writer = Writer::new("p1", &log).unwrap();
        let mut live = Reconciler::default();
        let mut events: Vec<Event> = again.iter().map(|entry| Event::Entry { entry: entry.clone(), replaces: live.deliver(entry, now) }).collect();
        events.push(Event::Entry { entry: user.clone(), replaces: live.deliver(&user, now) });
        live.see(reply(text.text.as_deref().unwrap()), now);
        events.push(Event::Live(live.shown(now)));
        events.push(Event::Entry { entry: text.clone(), replaces: live.deliver(&text, now) });
        writer.apply(&log, &events, None).unwrap();
    };
    let hello = entry("t1", "text", "Hello there, how can I help today?");
    follow(&[], entry("u1", "user", "Say hello"), hello.clone());
    follow(&[hello], entry("u2", "user", "Say goodbye"), entry("t2", "text", "Goodbye for now, and good luck with the build."));
    let rows: Vec<(String, bool)> = log.last("p1", usize::MAX).unwrap().iter().map(|row| (row["entry"]["id"].as_str().unwrap().into(), row["final"] == true)).collect();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(rows, [("u1".into(), true), ("t1".into(), true), ("u2".into(), true), ("t2".into(), true)]);
}

#[test]
fn an_open_question_stays_below_the_words_claude_wrote_before_it() {
    let dir = std::env::temp_dir().join(format!("sesh-question-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = Log::open(&dir.join("sessions.db")).unwrap();
    let now = Instant::now();
    let question = Entry { id: "q1".into(), ..said("question", "Tea?") };
    let text = |id: &str| Entry { id: id.into(), ..said("text", &format!("Questions before I start, {id}:")) };
    let mut writer = Writer::new("p1", &log).unwrap();
    let mut live = Reconciler::default();
    live.see(reply(text("t1").text.as_deref().unwrap()), now);
    let shown = Event::Live(live.shown(now));
    let rewrite = Event::Entry { entry: text("t1"), replaces: live.deliver(&text("t1"), now) };
    let landed = Event::Entry { entry: text("t2"), replaces: None };
    let ask = || Event::Entry { entry: question.clone(), replaces: None };
    writer.apply(&log, &[shown, ask(), rewrite, ask(), landed], None).unwrap();
    let rows: Vec<String> = log.last("p1", usize::MAX).unwrap().iter().map(|row| row["entry"]["id"].as_str().unwrap().into()).collect();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(rows, ["t1", "t2", "q1"]);
}

#[test]
fn a_message_sent_after_a_dropped_turn_takes_the_prompt_that_carries_both() {
    let dir = std::env::temp_dir().join(format!("sesh-dropped-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = Log::open(&dir.join("sessions.db")).unwrap();
    let mut writer = Writer::new("p1", &log).unwrap();
    let now = Instant::now();
    let mut live = Reconciler::default();
    let entry = |id: &str, text: &str| Entry { id: id.into(), ..said("user", text) };
    let deliver = |live: &mut Reconciler, entry: Entry| Event::Entry { replaces: live.deliver(&entry, now), entry };
    live.message("sent.1", "lets do it", SENT, now);
    let first = deliver(&mut live, entry("u1", "lets do it"));
    live.message("sent.2", "and log everything", SENT, now);
    let both = deliver(&mut live, entry("u2", "lets do itand log everything"));
    writer.apply(&log, &[first, Event::Dropped("u1".into()), both, Event::Live(live.shown(now))], None).unwrap();
    let rows: Vec<(String, String)> = log.last("p1", usize::MAX).unwrap().iter()
        .map(|row| (row["id"].as_str().unwrap().into(), row["entry"]["text"].as_str().unwrap().into())).collect();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(rows, [("sent.2".to_string(), "lets do itand log everything".to_string())]);
}

#[test]
fn both_messages_take_the_prompt_when_claude_put_the_first_back_unread() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.message("sent.1", "lets do it", SENT, now);
    live.message("sent.2", "and log everything", SENT, now);
    assert_eq!(live.deliver(&said("user", "lets do itand log everything"), now), Some("sent.1".into()));
    assert!(live.shown(now).items.is_empty());
}

#[test]
fn a_message_matches_its_entry_however_its_letters_or_images_are_spelled() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.message("sent.1", "на\u{0438}\u{0306}ти данные", SENT, now);
    live.message("sent.2", "'/Users/me/.sesh/uploads/pasted-2026-10-10T131849.png' look", SENT, now);
    assert_eq!(live.deliver(&said("user", "на\u{0439}ти данные"), now), Some("sent.1".into()));
    assert_eq!(live.deliver(&said("user", "[Image #3] look"), now), Some("sent.2".into()));
}

#[test]
fn an_entry_lands_after_the_final_rows_below_a_message_left_unmatched() {
    let dir = std::env::temp_dir().join(format!("sesh-straggler-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = Log::open(&dir.join("sessions.db")).unwrap();
    let body = |id: &str| json!({"id": id});
    for change in [
        Change::Show("sent.1".into(), body("sent.1")),
        Change::Show("live.1".into(), body("live.1")),
        Change::Rewrite { id: "live.1".into(), entry: "t1".into(), body: body("t1") },
        Change::Land { id: "t2".into(), entry: Some("t2".into()), body: body("t2") },
    ] {
        log.change("p1", change).unwrap();
    }
    let rows: Vec<(String, i64)> = log.last("p1", usize::MAX).unwrap().iter().map(|row| (row["id"].as_str().unwrap().into(), row["ord"].as_i64().unwrap())).collect();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(rows, [("sent.1".into(), 1), ("live.1".into(), 2), ("t2".into(), 3)]);
}

#[test]
fn an_idle_agent_still_holding_a_message_gets_one_more_enter() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.message("sent.1", "hello", SENT, now);
    live.idle(true, now);
    assert!(!live.nudge(now + Duration::from_secs(1)));
    assert!(live.nudge(now + NUDGE_AFTER));
    assert!(!live.nudge(now + NUDGE_AFTER * 2), "Enter is pressed again only once");
}

#[test]
fn a_message_claude_read_but_the_transcript_never_gave_settles_once_idle() {
    let now = Instant::now();
    let mut live = Reconciler::default();
    live.message("sent.1", "why does the check fail?", SHOWN, now);
    live.message("sent.2", "and this one?", SHOWN, now);
    live.idle(true, now);
    let held = [Entry { id: "u1".into(), ..said("user", "why does the check fail?") }];
    assert!(live.settle(&held, now).is_empty());
    let later = now + LOST_AFTER;
    assert!(live.unsettled(later));
    let settled: Vec<_> = live.settle(&held, later).into_iter().map(|(id, entry)| (id, entry.id)).collect();
    assert_eq!(settled, [("sent.1".to_string(), "u1".to_string())]);
    assert_eq!(live.shown(later).items.iter().map(|item| item.state).collect::<Vec<_>>(), [Some(LOST)]);
}

#[test]
fn a_rewritten_message_goes_back_above_the_rows_written_after_it() {
    let dir = std::env::temp_dir().join(format!("sesh-settle-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = Log::open(&dir.join("sessions.db")).unwrap();
    let at = |id: &str, at: &str| json!({"id": id, "at": at});
    for change in [
        Change::Land { id: "t1".into(), entry: Some("t1".into()), body: at("t1", "2026-10-11T23:40:00.000Z") },
        Change::Show("sent.1".into(), json!({"id": "sent.1"})),
        Change::Land { id: "t2".into(), entry: Some("t2".into()), body: at("t2", "2026-10-11T23:41:00.000Z") },
        Change::Land { id: "t3".into(), entry: Some("t3".into()), body: at("t3", "2026-10-11T23:42:00.000Z") },
        Change::Rewrite { id: "sent.1".into(), entry: "u1".into(), body: at("u1", "2026-10-11T23:40:30.000Z") },
        Change::Show("sent.2".into(), json!({"id": "sent.2"})),
        Change::Rewrite { id: "sent.2".into(), entry: "t3".into(), body: at("t3", "2026-10-11T23:42:00.000Z") },
    ] {
        log.change("p1", change).unwrap();
    }
    let rows: Vec<String> = log.last("p1", usize::MAX).unwrap().iter().map(|row| row["id"].as_str().unwrap().into()).collect();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(rows, ["t1", "sent.1", "t2", "t3"]);
}
