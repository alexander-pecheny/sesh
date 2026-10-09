//! What a device asks of an Agent (ADR 0012): a message, keys, an answer, a permission or a
//! stop, which the follower plays into the Agent's pane through its Source.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::follower::{transcript_path, Input, Source};
use crate::{Transcript, AGENTS};

const KEY_PAUSE: Duration = Duration::from_millis(200);
const MENU_TIMEOUT: Duration = Duration::from_secs(3);
/// How long an Agent gets to quit on ctrl+c before its pane is closed.
const STOP_PAUSE: Duration = Duration::from_secs(1);

type Result<T> = std::result::Result<T, String>;

/// The ops a device sends the follower to act on a session.
pub const OPS: [&str; 5] = ["send", "keys", "answer", "permit", "stop"];

/// The request for `op` with its arguments as a one-shot command takes them: the message,
/// key names, answers as JSON, or allow or deny.
pub fn request(op: &str, session: &str, args: &[String]) -> Option<Value> {
    let mut request = json!({"op": op, "session": session});
    match (op, args) {
        ("send", [text]) => request["text"] = text.clone().into(),
        ("keys", [_, ..]) => request["keys"] = args.into(),
        ("answer", [flag, answers]) if flag == "--json" => request["answers"] = serde_json::from_str(answers).ok()?,
        ("permit", [decision]) if decision == "allow" || decision == "deny" => request["allow"] = (decision == "allow").into(),
        ("stop", []) => {}
        _ => return None,
    }
    Some(request)
}

/// Plays `request` into `session`'s pane; `pane` is the pane as herdr last listed it.
pub fn act(source: &mut dyn Source, session: &str, pane: Option<&Value>, request: &Value) -> Result<()> {
    let strings = |key: &str| -> Option<Vec<String>> {
        request[key].as_array()?.iter().map(|value| value.as_str().map(str::to_string)).collect()
    };
    match request["op"].as_str().unwrap_or_default() {
        "send" => {
            let text = request["text"].as_str().ok_or("a message needs text")?;
            source.input(session, &Input::Prompt(text.into()))
        }
        "keys" => source.input(session, &Input::Keys(strings("keys").filter(|keys| !keys.is_empty()).ok_or("no keys to send")?)),
        "stop" => {
            // The Agent may have quit already, and the pane is closed either way.
            let _ = source.input(session, &Input::Keys(vec!["ctrl+c".into(), "ctrl+c".into()]));
            std::thread::sleep(STOP_PAUSE);
            source.input(session, &Input::Close)
        }
        "answer" => {
            let answers: Vec<Answer> = serde_json::from_value(request["answers"].clone()).map_err(|err| format!("invalid answers: {err}"))?;
            answer(source, session, agent_pane(session, pane)?, &answers)
        }
        "permit" => permit(source, session, agent_pane(session, pane)?, request["allow"] == true),
        other => Err(format!("unknown op {other:?}")),
    }
}

fn agent_pane<'a>(session: &str, pane: Option<&'a Value>) -> Result<(&'a Value, &'a str)> {
    pane.and_then(|pane| Some((pane, pane["agent"].as_str().filter(|agent| AGENTS.contains(agent))?)))
        .ok_or_else(|| format!("pane {session} does not run claude, codex or pi"))
}

#[derive(serde::Deserialize)]
struct Answer {
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    text: Option<String>,
}

fn answer(source: &mut dyn Source, session: &str, (pane, agent): (&Value, &str), answers: &[Answer]) -> Result<()> {
    let pending = &pane["permission"];
    if agent != "claude" || pending["tool"] != "AskUserQuestion" {
        return Err(format!("no question is open in pane {session}"));
    }
    let path = transcript_path(pane).ok_or_else(|| format!("{agent} in pane {session} has reported no transcript"))?;
    let question = Transcript::new(agent, path).expect("agent is supported").open_question(&pending["input"]);
    let inputs = question_inputs(question.questions.as_ref().unwrap_or(&Value::Null), answers)?;
    play(source, session, inputs, question_menu_open, |screen| {
        screen.contains("Ready to submit your answers?").then(|| vec![key("enter")])
    })
}

fn key(name: &str) -> Input {
    Input::Keys(vec![name.into()])
}

/// Claude's question menu: a digit picks an option (and moves on when only one
/// may be picked), the row after the options takes free text, and in a
/// multi-select the row after that moves on.
fn question_inputs(questions: &Value, answers: &[Answer]) -> Result<Vec<Input>> {
    let questions = questions.as_array().map(Vec::as_slice).unwrap_or_default();
    if questions.len() != answers.len() {
        return Err(format!("expected {} answers, got {}", questions.len(), answers.len()));
    }
    let mut inputs = Vec::new();
    for (question, answer) in questions.iter().zip(answers) {
        let labels: Vec<&str> =
            question["options"].as_array().into_iter().flatten().filter_map(|option| option["label"].as_str()).collect();
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
            .collect::<Result<Vec<_>>>()?;
        let text = answer.text.clone().filter(|text| !text.is_empty());
        let free_row = (labels.len() + 1).to_string();
        if question["multi"] == true {
            inputs.extend(digits.iter().map(|digit| key(digit)));
            inputs.extend((0..labels.len()).map(|_| key("down")));
            inputs.extend(text.map(Input::Text));
            inputs.extend([key("down"), key("enter")]);
        } else {
            match (digits.first(), text) {
                (Some(digit), _) => inputs.push(key(digit)),
                (None, Some(text)) => inputs.extend([key(&free_row), Input::Text(text), key("enter")]),
                (None, None) => return Err("each answer needs an option or text".into()),
            }
        }
    }
    Ok(inputs)
}

fn question_menu_open(screen: &str) -> bool {
    screen.contains("Enter to select ·") || screen.contains("Ready to submit your answers?")
}

fn permit(source: &mut dyn Source, session: &str, (_, agent): (&Value, &str), allow: bool) -> Result<()> {
    let (name, open): (&str, fn(&str) -> bool) = match (agent, allow) {
        ("claude", true) => ("1", claude_permission_open),
        ("claude", false) => ("esc", claude_permission_open),
        ("codex", true) => ("y", codex_permission_open),
        ("codex", false) => ("esc", codex_permission_open),
        _ => return Err("pi asks no permissions".into()),
    };
    play(source, session, vec![key(name)], open, |_| None)
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
    source: &mut dyn Source,
    pane_id: &str,
    inputs: Vec<Input>,
    open: fn(&str) -> bool,
    confirm: impl Fn(&str) -> Option<Vec<Input>>,
) -> Result<()> {
    let screen = source.read(pane_id, false)?;
    if !open(&screen) {
        return Err(format!("no menu is open in pane {pane_id}:\n{screen}"));
    }
    for input in inputs {
        source.input(pane_id, &input)?;
        std::thread::sleep(KEY_PAUSE);
    }
    let deadline = Instant::now() + MENU_TIMEOUT;
    loop {
        let screen = source.read(pane_id, false)?;
        if !open(&screen) {
            return Ok(());
        }
        for input in confirm(&screen).into_iter().flatten() {
            source.input(pane_id, &input)?;
            std::thread::sleep(KEY_PAUSE);
        }
        if Instant::now() >= deadline {
            return Err(format!("the menu is still open in pane {pane_id}:\n{screen}"));
        }
        std::thread::sleep(KEY_PAUSE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// herdr as a test sees it: a screen that shows `menu` until a key is pressed, and every
    /// input played, in order.
    struct Played {
        menu: &'static str,
        inputs: Vec<Input>,
    }

    impl Source for Played {
        fn panes(&mut self) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        fn read(&mut self, _: &str, _: bool) -> Result<String> {
            Ok(if self.inputs.is_empty() { self.menu.into() } else { String::new() })
        }

        fn input(&mut self, _: &str, input: &Input) -> Result<()> {
            self.inputs.push(input.clone());
            Ok(())
        }
    }

    fn played(menu: &'static str, pane: Option<&Value>, request: Value) -> (Result<()>, Vec<Input>) {
        let mut source = Played { menu, inputs: Vec::new() };
        let result = act(&mut source, "w1:p1", pane, &request);
        (result, source.inputs)
    }

    fn answers(json: &str) -> Vec<Answer> {
        serde_json::from_str(json).unwrap()
    }

    fn keys(inputs: &[Input]) -> Vec<String> {
        inputs
            .iter()
            .map(|input| match input {
                Input::Keys(keys) => keys.join(" "),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_request_is_built_from_a_one_shot_commands_arguments() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert_eq!(request("send", "w1:p1", &args(&["hi"])), Some(json!({"op": "send", "session": "w1:p1", "text": "hi"})));
        assert_eq!(request("keys", "w1:p1", &args(&["esc", "enter"])).unwrap()["keys"], json!(["esc", "enter"]));
        assert_eq!(request("permit", "w1:p1", &args(&["deny"])).unwrap()["allow"], false);
        assert_eq!(request("answer", "w1:p1", &args(&["--json", "[{\"options\":[\"A\"]}]"])).unwrap()["answers"][0]["options"], json!(["A"]));
        assert_eq!(request("keys", "w1:p1", &[]), None);
        assert_eq!(request("permit", "w1:p1", &args(&["maybe"])), None);
        assert_eq!(request("stop", "w1:p1", &args(&["now"])), None);
    }

    #[test]
    fn a_message_keys_and_a_stop_go_straight_to_the_pane() {
        let (sent, inputs) = played("", None, json!({"op": "send", "text": "hello"}));
        assert_eq!((sent, inputs), (Ok(()), vec![Input::Prompt("hello".into())]));
        let (_, inputs) = played("", None, json!({"op": "keys", "keys": ["ctrl+enter"]}));
        assert_eq!(inputs, [Input::Keys(vec!["ctrl+enter".into()])]);
        let (_, inputs) = played("", None, json!({"op": "stop"}));
        assert_eq!(keys(&inputs), ["ctrl+c ctrl+c", "Close"]);
        assert!(played("", None, json!({"op": "keys", "keys": []})).0.is_err());
    }

    #[test]
    fn a_permission_presses_the_agents_own_key_while_its_menu_is_open() {
        let codex = json!({"agent": "codex"});
        let (allowed, inputs) = played("Press enter to confirm or esc to cancel", Some(&codex), json!({"op": "permit", "allow": true}));
        assert_eq!((allowed, keys(&inputs)), (Ok(()), vec!["y".to_string()]));
        let claude = json!({"agent": "claude"});
        let (denied, inputs) = played("Esc to cancel · Tab to amend", Some(&claude), json!({"op": "permit", "allow": false}));
        assert_eq!((denied, keys(&inputs)), (Ok(()), vec!["esc".to_string()]));
        let (closed, inputs) = played("", Some(&claude), json!({"op": "permit", "allow": true}));
        assert!(closed.unwrap_err().starts_with("no menu is open"));
        assert!(inputs.is_empty());
        assert!(played("", None, json!({"op": "permit", "allow": true})).0.is_err());
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
            &answers(r#"[{"options":["Blue"]},{"options":["Cat","Fish"],"text":"hamster"},{"options":[],"text":"water"}]"#),
        )
        .unwrap();
        assert_eq!(
            keys(&inputs),
            ["2", "1", "3", "down", "down", "down", "Text(\"hamster\")", "down", "enter", "3", "Text(\"water\")", "enter"]
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
