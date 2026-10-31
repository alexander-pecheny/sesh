use std::io::Write as _;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::api::schema::{
    Method, PaneReadParams, PaneSendKeysParams, PaneSendTextParams, PaneTarget, ReadFormat,
    ReadSource, Request,
};
use crate::conversation::{Transcript, AGENTS, PROTOCOL};

const POLL: Duration = Duration::from_millis(250);
const KEY_PAUSE: Duration = Duration::from_millis(200);
const MENU_TIMEOUT: Duration = Duration::from_secs(3);
const AGENT_GONE_AFTER: Duration = Duration::from_secs(2);
const DEFAULT_LAST: usize = 50;
const FOLLOW_USAGE: &str =
    "usage: herdr agent follow <pane> [--since CURSOR] [--last N] | herdr agent follow --protocol";
const HISTORY_USAGE: &str = "usage: herdr agent history <pane> --before ID [--last N]";

/// The pane and the `--name value` options among `names`, or the usage error to exit with.
fn parse_args<'a>(
    args: &'a [String],
    names: &[&str],
    usage: &str,
) -> Result<(String, std::collections::HashMap<&'a str, &'a str>), i32> {
    let (mut target, mut options) = (None, std::collections::HashMap::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.strip_prefix("--"), args.as_slice().first()) {
            (Some(name), Some(value)) if names.contains(&name) => {
                options.insert(name, value.as_str());
                args.next();
            }
            (None, _) if target.is_none() => target = Some(arg.as_str()),
            _ => {
                eprintln!("{usage}");
                return Err(2);
            }
        }
    }
    match target {
        Some(target) => Ok((super::normalize_pane_id(target), options)),
        None => {
            eprintln!("{usage}");
            Err(2)
        }
    }
}

/// Exits a command with `message`, which says why it could not run.
fn fail(message: &str) -> i32 {
    eprintln!("{message}");
    1
}

fn parse_last(options: &std::collections::HashMap<&str, &str>) -> Result<usize, i32> {
    match options.get("last") {
        None => Ok(DEFAULT_LAST),
        Some(value) => value.parse().map_err(|_| {
            eprintln!("invalid --last: {value}");
            2
        }),
    }
}

pub(super) fn follow(args: &[String]) -> std::io::Result<i32> {
    if args.first().is_some_and(|arg| arg == "--protocol") {
        println!("{PROTOCOL}");
        return Ok(0);
    }
    let (pane_id, options) = match parse_args(args, &["since", "last"], FOLLOW_USAGE) {
        Ok(parsed) => parsed,
        Err(code) => return Ok(code),
    };
    let last = match parse_last(&options) {
        Ok(last) => last,
        Err(code) => return Ok(code),
    };
    let since = options.get("since").copied();
    let (pane, agent) = match agent_pane(&pane_id)? {
        Ok(found) => found,
        Err(message) => {
            eprintln!("{message}");
            return Ok(1);
        }
    };

    crate::platform::begin_cli_output();
    let mut follower = Follower {
        agent,
        last,
        state: None,
        permission: None,
        transcript: None,
        wrote: false,
    };
    follower.start(&pane, since)?;
    let mut agent_seen = Instant::now();
    loop {
        if crate::platform::stdout_closed() {
            return Ok(0);
        }
        std::thread::sleep(POLL);
        let Some(pane) = pane_info(&pane_id)? else {
            return Ok(0);
        };
        if pane["agent"].is_string() {
            agent_seen = Instant::now();
        } else if agent_seen.elapsed() > AGENT_GONE_AFTER {
            return Ok(0);
        }
        follower.tick(&pane)?;
    }
}

struct Follower {
    agent: String,
    last: usize,
    state: Option<String>,
    /// The pending permission's id, and whether it was Claude's question menu.
    permission: Option<(String, bool)>,
    transcript: Option<Transcript>,
    wrote: bool,
}

impl Follower {
    fn emit(&mut self, line: Value) -> std::io::Result<()> {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{line}")?;
        out.flush()?;
        self.wrote = true;
        Ok(())
    }

    fn start(&mut self, pane: &Value, since: Option<&str>) -> std::io::Result<()> {
        let found = transcript_path(pane);
        let path = found.as_deref();
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
                    transcript.read_tail(Some(offset), |entries| entries.len() >= self.last)?;
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

    fn tick(&mut self, pane: &Value) -> std::io::Result<()> {
        self.wrote = false;
        self.update_status(pane)?;
        let current = self
            .transcript
            .as_ref()
            .map(|transcript| transcript.path.to_string_lossy().into_owned());
        match reported_path(pane) {
            Some(path) if current.as_deref() != Some(path) => {
                if let Some(agent) = pane["agent"]
                    .as_str()
                    .filter(|agent| AGENTS.contains(agent))
                {
                    self.agent = agent.to_string();
                }
                if current.is_some() {
                    self.emit(switch(pane, path))?;
                }
                let transcript = Transcript::new(&self.agent, path).expect("agent is supported");
                self.show(transcript)?;
            }
            _ => self.read_new()?,
        }
        if self.wrote {
            self.emit_cursor()?;
        }
        Ok(())
    }

    /// Takes over `transcript` and sends its last entries.
    fn show(&mut self, mut transcript: Transcript) -> std::io::Result<()> {
        transcript.read_tail(None, |entries| entries.len() >= self.last)?;
        let start = transcript.entries.len().saturating_sub(self.last);
        let lines: Vec<Value> = transcript.entries[start..].iter().map(entry_line).collect();
        self.transcript = Some(transcript);
        lines.into_iter().try_for_each(|line| self.emit(line))
    }

    fn read_new(&mut self) -> std::io::Result<()> {
        let Some(mut transcript) = self.transcript.take() else {
            return Ok(());
        };
        let mark = transcript.entries.len();
        if transcript.read(None)? {
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

    fn update_status(&mut self, pane: &Value) -> std::io::Result<()> {
        let state = pane["agent_status"]
            .as_str()
            .filter(|state| matches!(*state, "idle" | "working" | "blocked" | "done"));
        if let Some(state) = state.filter(|state| self.state.as_deref() != Some(state)) {
            self.state = Some(state.to_string());
            self.emit(json!({"t": "state", "state": state}))?;
        }
        let permission = &pane["permission"];
        let id = permission["id"].as_str();
        if id == self.permission.as_ref().map(|(id, _)| id.as_str()) {
            return Ok(());
        }
        if let Some((done, false)) = self.permission.take() {
            self.emit(json!({"t": "permission_done", "id": done}))?;
        }
        let Some(id) = id else {
            return Ok(());
        };
        let tool = permission["tool"].as_str().unwrap_or_default();
        let question = tool == "AskUserQuestion";
        self.permission = Some((id.to_string(), question));
        let line = match &self.transcript {
            Some(transcript) if question => {
                entry_line(&transcript.open_question(&permission["input"]))
            }
            _ if question => return Ok(()),
            _ => crate::conversation::permission(&self.agent, id, tool, &permission["input"]),
        };
        self.emit(line)
    }

    fn emit_cursor(&mut self) -> std::io::Result<()> {
        let cursor = match &self.transcript {
            Some(transcript) => format!("{}:{}", transcript.offset, transcript.path.display()),
            None => "0:".to_string(),
        };
        self.emit(json!({"t": "cursor", "cursor": cursor}))
    }
}

fn entry_line(entry: &crate::conversation::Entry) -> Value {
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

fn reported_path(pane: &Value) -> Option<&str> {
    pane["agent_session"]["path"]
        .as_str()
        .filter(|path| !path.is_empty())
}

/// Agents started under an older herdr reported only their session id.
fn transcript_path(pane: &Value) -> Option<String> {
    if let Some(path) = reported_path(pane) {
        return Some(path.to_string());
    }
    let session = &pane["agent_session"];
    if session["kind"] != "id" {
        return None;
    }
    let id = session["value"].as_str()?;
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let dir = |var: &str, default: &str| {
        std::env::var_os(var).map_or_else(|| home.join(default), std::path::PathBuf::from)
    };
    let name = format!("{id}.jsonl");
    let found = match session["agent"].as_str()? {
        "claude" => find_file(
            &dir("CLAUDE_CONFIG_DIR", ".claude").join("projects"),
            1,
            &|file| file == name,
        ),
        "codex" => find_file(&dir("CODEX_HOME", ".codex").join("sessions"), 3, &|file| {
            file.ends_with(&format!("-{name}"))
        }),
        _ => None,
    };
    found.map(|path| path.to_string_lossy().into_owned())
}

fn find_file(
    dir: &std::path::Path,
    depth: usize,
    matches: &dyn Fn(&str) -> bool,
) -> Option<std::path::PathBuf> {
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

pub(super) fn entry(args: &[String]) -> std::io::Result<i32> {
    let [target, id] = args else {
        eprintln!("usage: herdr agent entry <pane> <id>");
        return Ok(2);
    };
    let mut transcript = match pane_transcript(&super::normalize_pane_id(target))? {
        Ok((transcript, _, _)) => transcript,
        Err(message) => {
            eprintln!("{message}");
            return Ok(1);
        }
    };
    if let Some(end) = transcript.locate(id)? {
        transcript.read_tail(Some(end), |entries| {
            entries.iter().any(|entry| entry.id == *id)
        })?;
    }
    let Some(entry) = transcript.find(id) else {
        eprintln!("no entry {id} in {}", transcript.path.to_string_lossy());
        return Ok(1);
    };
    let mut line = serde_json::to_value(entry).map_err(std::io::Error::other)?;
    line["t"] = "entry".into();
    println!("{line}");
    Ok(0)
}

pub(super) fn history(args: &[String]) -> std::io::Result<i32> {
    let (pane_id, options) = match parse_args(args, &["before", "last"], HISTORY_USAGE) {
        Ok(parsed) => parsed,
        Err(code) => return Ok(code),
    };
    let (Some(before), Ok(last)) = (options.get("before"), parse_last(&options)) else {
        eprintln!("{HISTORY_USAGE}");
        return Ok(2);
    };
    let mut transcript = match pane_transcript(&pane_id)? {
        Ok((transcript, _, _)) => transcript,
        Err(message) => return Ok(fail(&message)),
    };
    let Some((entries, more)) = transcript.history(before, last)? else {
        eprintln!("no entry {before} in {}", transcript.path.to_string_lossy());
        return Ok(1);
    };
    let mut out = std::io::stdout().lock();
    for entry in &entries {
        writeln!(out, "{}", entry_line(entry))?;
    }
    writeln!(out, "{}", json!({"t": "history", "more": more}))?;
    Ok(0)
}

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

pub(super) fn answer(args: &[String]) -> std::io::Result<i32> {
    let [target, flag, answers] = args else {
        eprintln!("usage: herdr agent answer <pane> --json '<answers>'");
        return Ok(2);
    };
    if flag != "--json" {
        eprintln!("usage: herdr agent answer <pane> --json '<answers>'");
        return Ok(2);
    }
    let answers: Vec<Answer> = match serde_json::from_str(answers) {
        Ok(answers) => answers,
        Err(err) => {
            eprintln!("invalid answers: {err}");
            return Ok(2);
        }
    };
    let pane_id = super::normalize_pane_id(target);
    let (transcript, agent, pane) = match pane_transcript(&pane_id)? {
        Ok(found) => found,
        Err(message) => {
            eprintln!("{message}");
            return Ok(1);
        }
    };
    let permission = &pane["permission"];
    if agent != "claude" || permission["tool"] != "AskUserQuestion" {
        eprintln!("no question is open in pane {pane_id}");
        return Ok(1);
    }
    let question = transcript.open_question(&permission["input"]);
    let inputs = match question_inputs(
        question.questions.as_ref().unwrap_or(&Value::Null),
        &answers,
    ) {
        Ok(inputs) => inputs,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };
    play(&pane_id, inputs, question_menu_open, |screen| {
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

pub(super) fn permit(args: &[String]) -> std::io::Result<i32> {
    let [target, decision] = args else {
        eprintln!("usage: herdr agent permit <pane> allow|deny");
        return Ok(2);
    };
    let pane_id = super::normalize_pane_id(target);
    let agent = match agent_pane(&pane_id)? {
        Ok((_, agent)) => agent,
        Err(message) => {
            eprintln!("{message}");
            return Ok(1);
        }
    };
    let (key, open): (&str, fn(&str) -> bool) = match (agent.as_str(), decision.as_str()) {
        ("claude", "allow") => ("1", claude_permission_open),
        ("claude", "deny") => ("esc", claude_permission_open),
        ("codex", "allow") => ("y", codex_permission_open),
        ("codex", "deny") => ("esc", codex_permission_open),
        ("pi", _) => {
            eprintln!("pi asks no permissions");
            return Ok(1);
        }
        _ => {
            eprintln!("usage: herdr agent permit <pane> allow|deny");
            return Ok(2);
        }
    };
    play(&pane_id, vec![Input::Key(key.into())], open, |_| None)
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
) -> std::io::Result<i32> {
    let screen = read_screen(pane_id)?;
    if !open(&screen) {
        eprintln!("no menu is open in pane {pane_id}:\n{screen}");
        return Ok(1);
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
        if let Some(inputs) = confirm(&screen) {
            for input in inputs {
                send(pane_id, input)?;
                std::thread::sleep(KEY_PAUSE);
            }
        }
        if Instant::now() >= deadline {
            eprintln!("the menu is still open in pane {pane_id}:\n{screen}");
            return Ok(1);
        }
        std::thread::sleep(KEY_PAUSE);
    }
}

fn send(pane_id: &str, input: Input) -> std::io::Result<()> {
    let method = match input {
        Input::Key(key) => Method::PaneSendKeys(PaneSendKeysParams {
            pane_id: pane_id.to_string(),
            keys: vec![key],
        }),
        Input::Text(text) => Method::PaneSendText(PaneSendTextParams {
            pane_id: pane_id.to_string(),
            text,
        }),
    };
    let response = super::send_request_unchecked(&Request {
        id: "cli:agent:play".into(),
        method,
    })?;
    match response.get("error") {
        Some(error) => Err(std::io::Error::other(error.to_string())),
        None => Ok(()),
    }
}

fn read_screen(pane_id: &str) -> std::io::Result<String> {
    let response = super::send_request_unchecked(&Request {
        id: "cli:agent:screen".into(),
        method: Method::PaneRead(PaneReadParams {
            pane_id: pane_id.to_string(),
            source: ReadSource::Visible,
            lines: None,
            format: ReadFormat::Text,
            strip_ansi: true,
            intent: Default::default(),
        }),
    })?;
    Ok(response["result"]["read"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

fn pane_info(pane_id: &str) -> std::io::Result<Option<Value>> {
    let response = super::send_request_unchecked(&Request {
        id: "cli:agent:pane".into(),
        method: Method::PaneGet(PaneTarget {
            pane_id: pane_id.to_string(),
        }),
    })?;
    Ok(response.get("result").map(|result| result["pane"].clone()))
}

/// The pane and its Agent, or why it has none Conversations support.
fn agent_pane(pane_id: &str) -> std::io::Result<Result<(Value, String), String>> {
    let response = super::send_request(&Request {
        id: "cli:agent:pane".into(),
        method: Method::PaneGet(PaneTarget {
            pane_id: pane_id.to_string(),
        }),
    })?;
    if let Some(error) = response.get("error") {
        return Ok(Err(error.to_string()));
    }
    let pane = response["result"]["pane"].clone();
    Ok(match pane["agent"].as_str() {
        Some(agent) if AGENTS.contains(&agent) => Ok((pane.clone(), agent.to_string())),
        _ => Err(format!("pane {pane_id} does not run claude, codex or pi")),
    })
}

fn pane_transcript(pane_id: &str) -> std::io::Result<Result<(Transcript, String, Value), String>> {
    let (pane, agent) = match agent_pane(pane_id)? {
        Ok(found) => found,
        Err(message) => return Ok(Err(message)),
    };
    let Some(path) = transcript_path(&pane) else {
        return Ok(Err(format!(
            "{agent} in pane {pane_id} has reported no transcript"
        )));
    };
    let transcript = Transcript::new(&agent, &path).expect("agent is supported");
    Ok(Ok((transcript, agent, pane)))
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
