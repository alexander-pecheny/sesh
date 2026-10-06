use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sesh_transcript::{permission, Entry, Transcript, AGENTS, PROTOCOL};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("SOURCE_HASH"));
const POLL: Duration = Duration::from_millis(250);
/// While a session's Transcript cannot be found, look again every this many polls.
const RETRY: u32 = 8;
const KEY_PAUSE: Duration = Duration::from_millis(200);
const MENU_TIMEOUT: Duration = Duration::from_secs(3);
const AGENT_GONE_AFTER: Duration = Duration::from_secs(2);
const DEFAULT_LAST: usize = 50;
const USAGE: &str = "usage: sesh-transcript --version | follow --protocol | follow <pane> [--since CURSOR] [--last N]
       | history <pane> --before ID [--last N] | entry <pane> ID
       | answer <pane> --json ANSWERS | permit <pane> allow|deny";

/// A failed command says why on stderr and exits 1; usage errors exit 2.
type Exit = Result<i32, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let exit = match args.first().map(String::as_str) {
        Some("--version") => {
            println!("sesh-transcript {VERSION}");
            Ok(0)
        }
        Some("follow") => follow(rest),
        Some("history") => history(rest),
        Some("entry") => entry(rest),
        Some("answer") => answer(rest),
        Some("permit") => permit(rest),
        _ => usage(),
    };
    std::process::exit(exit.unwrap_or_else(|message| {
        eprintln!("{message}");
        1
    }));
}

fn usage() -> Exit {
    eprintln!("{USAGE}");
    Ok(2)
}

/// The pane and the `--name value` options among `names`.
fn parse_args<'a>(
    args: &'a [String],
    names: &[&str],
) -> Option<(&'a str, HashMap<&'a str, &'a str>)> {
    let (mut target, mut options) = (None, HashMap::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.strip_prefix("--"), args.as_slice().first()) {
            (Some(name), Some(value)) if names.contains(&name) => {
                options.insert(name, value.as_str());
                args.next();
            }
            (None, _) if target.is_none() => target = Some(arg.as_str()),
            _ => return None,
        }
    }
    Some((target?, options))
}

fn parse_last(options: &HashMap<&str, &str>) -> Option<usize> {
    options
        .get("last")
        .map_or(Some(DEFAULT_LAST), |value| value.parse().ok())
}

// MARK: herdr

/// Runs `herdr` with `args` and returns its stdout, or the message it failed with.
fn herdr(args: &[&str]) -> Result<String, String> {
    let output = Command::new("herdr")
        .args(args)
        .output()
        .map_err(|err| format!("herdr: {err}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    let json: Option<Value> = serde_json::from_str(&err).ok();
    Err(json
        .and_then(|json| json["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| err.trim().to_string()))
}

fn pane_info(pane_id: &str) -> Result<Value, String> {
    let reply: Value = serde_json::from_str(&herdr(&["pane", "get", pane_id])?)
        .map_err(|err| format!("herdr said something sesh-transcript cannot read: {err}"))?;
    Ok(reply["result"]["pane"].clone())
}

/// The pane and its Agent, or why it has none Conversations support.
fn agent_pane(pane_id: &str) -> Result<(Value, String), String> {
    let pane = pane_info(pane_id)?;
    match pane["agent"].as_str() {
        Some(agent) if AGENTS.contains(&agent) => Ok((pane.clone(), agent.to_string())),
        _ => Err(format!("pane {pane_id} does not run claude, codex or pi")),
    }
}

fn pane_transcript(pane_id: &str) -> Result<(Transcript, String, Value), String> {
    let (pane, agent) = agent_pane(pane_id)?;
    let path = transcript_path(&pane)
        .ok_or_else(|| format!("{agent} in pane {pane_id} has reported no transcript"))?;
    let transcript = Transcript::new(&agent, path).expect("agent is supported");
    Ok((transcript, agent, pane))
}

/// The path herdr reports: the fork's `path`, or stock herdr's value of kind `path` (pi).
fn reported_path(pane: &Value) -> Option<&str> {
    let session = &pane["agent_session"];
    let value = (session["kind"] == "path")
        .then(|| session["value"].as_str())
        .flatten();
    session["path"]
        .as_str()
        .or(value)
        .filter(|path| !path.is_empty())
}

/// Claude and Codex report only their session id to stock herdr, so the file is found by it.
fn transcript_path(pane: &Value) -> Option<String> {
    if let Some(path) = reported_path(pane) {
        return Some(path.to_string());
    }
    let session = &pane["agent_session"];
    if session["kind"] != "id" {
        return None;
    }
    let id = session["value"].as_str()?;
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = |var: &str, default: &str| {
        std::env::var_os(var).map_or_else(|| home.join(default), PathBuf::from)
    };
    let name = format!("{id}.jsonl");
    let found = match session["agent"].as_str()? {
        "claude" => find_file(
            &dir("CLAUDE_CONFIG_DIR", ".claude").join("projects"),
            1,
            &|file| file == name,
        ),
        "codex" => find_file(&dir("CODEX_HOME", ".codex").join("sessions"), 3, &|file| {
            file.starts_with("rollout-") && file.ends_with(&format!("-{name}"))
        }),
        _ => None,
    };
    found.map(|path| path.to_string_lossy().into_owned())
}

fn find_file(dir: &Path, depth: usize, matches: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if depth == 0 {
            if entry.file_name().to_str().is_some_and(matches) {
                return Some(path);
            }
        } else if path.is_dir() {
            if let Some(found) = find_file(&path, depth - 1, matches) {
                return Some(found);
            }
        }
    }
    None
}

// MARK: follow

fn follow(args: &[String]) -> Exit {
    if args.first().is_some_and(|arg| arg == "--protocol") {
        println!("{PROTOCOL}");
        return Ok(0);
    }
    let Some((pane_id, options)) = parse_args(args, &["since", "last"]) else {
        return usage();
    };
    let Some(last) = parse_last(&options) else {
        return usage();
    };
    let (pane, agent) = agent_pane(pane_id)?;
    let mut follower = Follower {
        agent,
        last,
        state: None,
        permission: None,
        transcript: None,
        session: Value::Null,
        lost: None,
        wrote: false,
    };
    follower.start(&pane, options.get("since").copied())?;
    let mut agent_seen = Instant::now();
    loop {
        std::thread::sleep(POLL);
        if stdout_closed() {
            return Ok(0);
        }
        match pane_info(pane_id) {
            Ok(pane) if pane["agent"].is_string() => {
                agent_seen = Instant::now();
                follower.tick(&pane)?;
            }
            _ if agent_seen.elapsed() > AGENT_GONE_AFTER => return Ok(0),
            _ => {}
        }
    }
}

struct Follower {
    agent: String,
    last: usize,
    state: Option<String>,
    /// The pending permission's id, and whether it was Claude's question menu.
    permission: Option<(String, bool)>,
    transcript: Option<Transcript>,
    /// herdr's `agent_session` when its Transcript was last looked for.
    session: Value,
    /// While the session's Transcript is not found, polls since it was last looked for.
    lost: Option<u32>,
    wrote: bool,
}

impl Follower {
    fn emit(&mut self, line: Value) -> Result<(), String> {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{line}")
            .and_then(|()| out.flush())
            .map_err(|err| err.to_string())?;
        self.wrote = true;
        Ok(())
    }

    fn start(&mut self, pane: &Value, since: Option<&str>) -> Result<(), String> {
        self.session = pane["agent_session"].clone();
        let found = transcript_path(pane);
        let path = found.as_deref();
        self.lost = path.is_none().then_some(1);
        self.emit(json!({
            "t": "hello",
            "protocol": PROTOCOL,
            "agent": self.agent,
            "transcript": path.unwrap_or_default(),
        }))?;
        if let Some(path) = path {
            let cursor = since.and_then(|cursor| cursor.split_once(':'));
            let mut transcript = Transcript::new(&self.agent, path).expect("agent is supported");
            match cursor.and_then(|(offset, cursor_path)| Some((offset.parse().ok()?, cursor_path)))
            {
                Some((offset, cursor_path)) if cursor_path == path => {
                    transcript
                        .read_tail(Some(offset), |entries| entries.len() >= self.last)
                        .map_err(|err| err.to_string())?;
                    self.transcript = Some(transcript);
                    self.read_new()?;
                }
                Some((_, cursor_path)) if !cursor_path.is_empty() => {
                    self.emit(switch(pane, path))?;
                    self.show(transcript)?;
                }
                _ => self.show(transcript)?,
            }
        }
        self.update_status(pane)?;
        self.emit_cursor()
    }

    fn tick(&mut self, pane: &Value) -> Result<(), String> {
        self.wrote = false;
        self.update_status(pane)?;
        match self.moved(pane) {
            Some(path) => {
                if let Some(agent) = pane["agent"]
                    .as_str()
                    .filter(|agent| AGENTS.contains(agent))
                {
                    self.agent = agent.to_string();
                }
                if self.transcript.is_some() {
                    self.emit(switch(pane, &path))?;
                }
                let transcript = Transcript::new(&self.agent, path).expect("agent is supported");
                self.show(transcript)?;
            }
            None => self.read_new()?,
        }
        if self.wrote {
            self.emit_cursor()?;
        }
        Ok(())
    }

    /// The Transcript the session has moved to. One is looked for when herdr's session
    /// changes, then every few polls until found, since Claude writes a session's file only
    /// once it is first prompted.
    fn moved(&mut self, pane: &Value) -> Option<String> {
        if pane["agent_session"] != self.session {
            self.session = pane["agent_session"].clone();
            self.lost = Some(0);
        }
        if let Some(polls @ 1..) = &mut self.lost {
            *polls = (*polls + 1) % RETRY;
        }
        if self.lost != Some(0) {
            return None;
        }
        let Some(path) = transcript_path(pane) else {
            self.lost = Some(1);
            return None;
        };
        self.lost = None;
        let current = self
            .transcript
            .as_ref()
            .map(|transcript| transcript.path.to_string_lossy());
        (current.as_deref() != Some(path.as_str())).then_some(path)
    }

    /// Takes over `transcript` and sends its last entries.
    fn show(&mut self, mut transcript: Transcript) -> Result<(), String> {
        transcript
            .read_tail(None, |entries| entries.len() >= self.last)
            .map_err(|err| err.to_string())?;
        let start = transcript.entries.len().saturating_sub(self.last);
        let lines: Vec<Value> = transcript.entries[start..].iter().map(entry_line).collect();
        self.transcript = Some(transcript);
        lines.into_iter().try_for_each(|line| self.emit(line))
    }

    fn read_new(&mut self) -> Result<(), String> {
        let Some(mut transcript) = self.transcript.take() else {
            return Ok(());
        };
        let mark = transcript.entries.len();
        if transcript.read(None).map_err(|err| err.to_string())? {
            let path = transcript.path.to_string_lossy().into_owned();
            self.emit(json!({"t": "switch", "reason": "fork", "transcript": path}))?;
            let start = transcript.entries.len().saturating_sub(self.last);
            let lines: Vec<Value> = transcript.entries[start..].iter().map(entry_line).collect();
            self.transcript = Some(transcript);
            return lines.into_iter().try_for_each(|line| self.emit(line));
        }
        let lines: Vec<Value> = transcript.entries[mark..].iter().map(entry_line).collect();
        self.transcript = Some(transcript);
        lines.into_iter().try_for_each(|line| self.emit(line))
    }

    /// Permission prompts come only from the fork, whose hooks report them on the pane.
    fn update_status(&mut self, pane: &Value) -> Result<(), String> {
        let state = pane["agent_status"]
            .as_str()
            .filter(|state| matches!(*state, "idle" | "working" | "blocked" | "done"));
        if let Some(state) = state.filter(|state| self.state.as_deref() != Some(state)) {
            self.state = Some(state.to_string());
            self.emit(json!({"t": "state", "state": state}))?;
        }
        let pending = &pane["permission"];
        let id = pending["id"].as_str();
        if id == self.permission.as_ref().map(|(id, _)| id.as_str()) {
            return Ok(());
        }
        if let Some((done, false)) = self.permission.take() {
            self.emit(json!({"t": "permission_done", "id": done}))?;
        }
        let Some(id) = id else {
            return Ok(());
        };
        let tool = pending["tool"].as_str().unwrap_or_default();
        let question = tool == "AskUserQuestion";
        self.permission = Some((id.to_string(), question));
        let line = match &self.transcript {
            Some(transcript) if question => {
                entry_line(&transcript.open_question(&pending["input"]))
            }
            _ if question => return Ok(()),
            _ => permission(&self.agent, id, tool, &pending["input"]),
        };
        self.emit(line)
    }

    fn emit_cursor(&mut self) -> Result<(), String> {
        let cursor = match &self.transcript {
            Some(transcript) => format!("{}:{}", transcript.offset, transcript.path.display()),
            None => "0:".to_string(),
        };
        self.emit(json!({"t": "cursor", "cursor": cursor}))
    }
}

fn entry_line(entry: &Entry) -> Value {
    let mut line = serde_json::to_value(entry.clipped()).unwrap_or_default();
    line["t"] = "entry".into();
    line
}

fn switch(pane: &Value, path: &str) -> Value {
    let reason = match pane["agent_session"]["start"].as_str() {
        Some(reason @ ("clear" | "resume" | "compact")) => reason,
        Some("startup" | "new") => "new",
        Some("fork" | "branch") => "fork",
        _ => "other",
    };
    json!({"t": "switch", "reason": reason, "transcript": path})
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[cfg(target_os = "linux")]
type Nfds = std::ffi::c_ulong;
#[cfg(not(target_os = "linux"))]
type Nfds = std::ffi::c_uint;

extern "C" {
    fn poll(fds: *mut PollFd, count: Nfds, timeout: i32) -> i32;
}

/// Whether Sesh has hung up, which a quiet Agent session would otherwise never notice.
fn stdout_closed() -> bool {
    const POLLOUT: i16 = 0x4;
    const POLLERR: i16 = 0x8;
    const POLLHUP: i16 = 0x10;
    let mut descriptor = PollFd {
        fd: 1,
        events: POLLOUT,
        revents: 0,
    };
    let ready = unsafe { poll(&mut descriptor, 1, 0) };
    ready > 0 && descriptor.revents & (POLLERR | POLLHUP) != 0
}

// MARK: entry and history

fn entry(args: &[String]) -> Exit {
    let [pane_id, id] = args else {
        return usage();
    };
    let (mut transcript, _, _) = pane_transcript(pane_id)?;
    if let Some(end) = transcript.locate(id).map_err(|err| err.to_string())? {
        transcript
            .read_tail(Some(end), |entries| {
                entries.iter().any(|entry| entry.id == *id)
            })
            .map_err(|err| err.to_string())?;
    }
    let entry = transcript
        .find(id)
        .ok_or_else(|| format!("no entry {id} in {}", transcript.path.to_string_lossy()))?;
    let mut line = serde_json::to_value(entry).map_err(|err| err.to_string())?;
    line["t"] = "entry".into();
    println!("{line}");
    Ok(0)
}

fn history(args: &[String]) -> Exit {
    let Some((pane_id, options)) = parse_args(args, &["before", "last"]) else {
        return usage();
    };
    let (Some(before), Some(last)) = (options.get("before"), parse_last(&options)) else {
        return usage();
    };
    let (mut transcript, _, _) = pane_transcript(pane_id)?;
    let (entries, more) = transcript
        .history(before, last)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("no entry {before} in {}", transcript.path.to_string_lossy()))?;
    let mut out = std::io::stdout().lock();
    for entry in &entries {
        writeln!(out, "{}", entry_line(entry)).map_err(|err| err.to_string())?;
    }
    writeln!(out, "{}", json!({"t": "history", "more": more})).map_err(|err| err.to_string())?;
    Ok(0)
}

// MARK: answer and permit

#[derive(serde::Deserialize)]
struct Answer {
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    text: Option<String>,
}

/// One step of playing a menu: a key name or literal text.
#[derive(Debug, PartialEq)]
enum Input {
    Key(String),
    Text(String),
}

fn answer(args: &[String]) -> Exit {
    let [pane_id, flag, answers] = args else {
        return usage();
    };
    if flag != "--json" {
        return usage();
    }
    let answers: Vec<Answer> =
        serde_json::from_str(answers).map_err(|err| format!("invalid answers: {err}"))?;
    let (transcript, agent, pane) = pane_transcript(pane_id)?;
    let pending = &pane["permission"];
    if agent != "claude" || pending["tool"] != "AskUserQuestion" {
        return Err(format!("no question is open in pane {pane_id}"));
    }
    let question = transcript.open_question(&pending["input"]);
    let questions = question.questions.as_ref().unwrap_or(&Value::Null);
    let inputs = question_inputs(questions, &answers)?;
    play(pane_id, inputs, question_menu_open, |screen| {
        screen
            .contains("Ready to submit your answers?")
            .then(|| vec![Input::Key("enter".into())])
    })
}

/// Claude's question menu: a digit picks an option (and moves on when only one
/// may be picked), the row after the options takes free text, and in a
/// multi-select the row after that moves on.
fn question_inputs(questions: &Value, answers: &[Answer]) -> Result<Vec<Input>, String> {
    let questions = questions.as_array().map(Vec::as_slice).unwrap_or_default();
    if questions.len() != answers.len() {
        return Err(format!(
            "expected {} answers, got {}",
            questions.len(),
            answers.len()
        ));
    }
    let key = |key: String| Input::Key(key);
    let mut inputs = Vec::new();
    for (question, answer) in questions.iter().zip(answers) {
        let labels: Vec<&str> = question["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| option["label"].as_str())
            .collect();
        let digits = answer
            .options
            .iter()
            .map(|label| {
                labels
                    .iter()
                    .position(|candidate| candidate == label)
                    .map(|index| (index + 1).to_string())
                    .ok_or_else(|| format!("no option {label:?} in {labels:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let text = answer.text.clone().filter(|text| !text.is_empty());
        let free_row = (labels.len() + 1).to_string();
        if question["multi"] == true {
            inputs.extend(digits.into_iter().map(key));
            inputs.extend((0..labels.len()).map(|_| key("down".into())));
            inputs.extend(text.map(Input::Text));
            inputs.extend(["down", "enter"].map(|name| key(name.into())));
        } else {
            match (digits.into_iter().next(), text) {
                (Some(digit), _) => inputs.push(key(digit)),
                (None, Some(text)) => {
                    inputs.extend([key(free_row), Input::Text(text), key("enter".into())])
                }
                (None, None) => return Err("each answer needs an option or text".into()),
            }
        }
    }
    Ok(inputs)
}

fn question_menu_open(screen: &str) -> bool {
    screen.contains("Enter to select ·") || screen.contains("Ready to submit your answers?")
}

fn permit(args: &[String]) -> Exit {
    let [pane_id, decision] = args else {
        return usage();
    };
    let (_, agent) = agent_pane(pane_id)?;
    let (key, open): (&str, fn(&str) -> bool) = match (agent.as_str(), decision.as_str()) {
        ("claude", "allow") => ("1", claude_permission_open),
        ("claude", "deny") => ("esc", claude_permission_open),
        ("codex", "allow") => ("y", codex_permission_open),
        ("codex", "deny") => ("esc", codex_permission_open),
        ("pi", _) => return Err("pi asks no permissions".into()),
        _ => return usage(),
    };
    play(pane_id, vec![Input::Key(key.into())], open, |_| None)
}

fn claude_permission_open(screen: &str) -> bool {
    screen.contains("Esc to cancel · Tab to amend")
}

fn codex_permission_open(screen: &str) -> bool {
    screen.contains("Press enter to confirm or esc to cancel")
}

/// Plays `inputs` into a menu that `open` sees on screen, then waits for it to
/// close, answering any follow-up screen `confirm` recognises.
fn play(
    pane_id: &str,
    inputs: Vec<Input>,
    open: fn(&str) -> bool,
    confirm: impl Fn(&str) -> Option<Vec<Input>>,
) -> Exit {
    let screen = read_screen(pane_id)?;
    if !open(&screen) {
        return Err(format!("no menu is open in pane {pane_id}:\n{screen}"));
    }
    for input in inputs {
        send(pane_id, input)?;
        std::thread::sleep(KEY_PAUSE);
    }
    let deadline = Instant::now() + MENU_TIMEOUT;
    loop {
        let screen = read_screen(pane_id)?;
        if !open(&screen) {
            return Ok(0);
        }
        for input in confirm(&screen).into_iter().flatten() {
            send(pane_id, input)?;
            std::thread::sleep(KEY_PAUSE);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the menu is still open in pane {pane_id}:\n{screen}"
            ));
        }
        std::thread::sleep(KEY_PAUSE);
    }
}

fn send(pane_id: &str, input: Input) -> Result<(), String> {
    match input {
        Input::Key(key) => herdr(&["pane", "send-keys", pane_id, &key]),
        Input::Text(text) => herdr(&["pane", "send-text", pane_id, &text]),
    }
    .map(drop)
}

fn read_screen(pane_id: &str) -> Result<String, String> {
    herdr(&["pane", "read", pane_id, "--source", "visible"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_file_reaches_codex_rollouts_by_id() {
        let root = std::env::temp_dir().join(format!("herdr-find-{}", std::process::id()));
        let day = root.join("2026/10/05");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-2026-10-05T10-00-00-abc.jsonl"), "").unwrap();
        let found = find_file(&root, 3, &|file| file.ends_with("-abc.jsonl"));
        let missing = find_file(&root, 3, &|file| file.ends_with("-xyz.jsonl"));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            found,
            Some(day.join("rollout-2026-10-05T10-00-00-abc.jsonl"))
        );
        assert_eq!(missing, None);
    }

    fn answers(json: &str) -> Vec<Answer> {
        serde_json::from_str(json).unwrap()
    }

    fn keys(inputs: &[Input]) -> Vec<String> {
        inputs
            .iter()
            .map(|input| match input {
                Input::Key(key) => key.clone(),
                Input::Text(text) => format!("text:{text}"),
            })
            .collect()
    }

    #[test]
    fn question_inputs_play_claudes_menu() {
        let questions = json!([
            {"question": "Colour?", "multi": false, "options": [{"label": "Red"}, {"label": "Blue"}]},
            {"question": "Pets?", "multi": true, "options": [{"label": "Cat"}, {"label": "Dog"}, {"label": "Fish"}]},
            {"question": "Tea?", "multi": false, "options": [{"label": "Tea"}, {"label": "Coffee"}]},
        ]);
        let inputs = question_inputs(
            &questions,
            &answers(
                r#"[{"options":["Blue"]},{"options":["Cat","Fish"],"text":"hamster"},{"options":[],"text":"water"}]"#,
            ),
        )
        .unwrap();
        assert_eq!(
            keys(&inputs),
            [
                "2",
                "1",
                "3",
                "down",
                "down",
                "down",
                "text:hamster",
                "down",
                "enter",
                "3",
                "text:water",
                "enter"
            ]
        );
    }

    #[test]
    fn question_inputs_reject_unknown_labels_and_count_mismatches() {
        let questions = json!([{"multi": false, "options": [{"label": "Red"}]}]);
        assert!(question_inputs(&questions, &answers(r#"[{"options":["Green"]}]"#)).is_err());
        assert!(question_inputs(&questions, &answers("[]")).is_err());
        assert!(question_inputs(&questions, &answers(r#"[{"options":[]}]"#)).is_err());
    }
}
