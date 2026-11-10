//! What a device asks of an Agent (ADR 0012): a message, keys, an answer, a permission, a stop
//! or a start (ADR 0020), which the follower plays into the Agent's pane through its Source.
//! Taking back a queued message, or handing the queue over, the follower does itself (ADR 0015).

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{transcript_path, Agent};
use crate::follower::{Input, Source};
use crate::screen::Login;
use crate::Transcript;

const KEY_PAUSE: Duration = Duration::from_millis(200);
const MENU_TIMEOUT: Duration = Duration::from_secs(3);
/// How long Claude gets to show its sign-in link, and to trade a code for a login.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
const CONTINUE: &str = "Press Enter to continue";
/// How long an Agent gets to quit on ctrl+c before its pane is closed.
const STOP_PAUSE: Duration = Duration::from_secs(1);

type Result<T> = std::result::Result<T, String>;

/// The ops a device sends the follower to act on a session.
pub const OPS: [&str; 8] = ["send", "unqueue", "hand", "keys", "answer", "permit", "stop", "start"];

/// The request for `op` with its arguments as a one-shot command takes them: the message's id
/// and text, key names, answers as JSON, or allow or deny.
pub fn request(op: &str, session: &str, args: &[String]) -> Option<Value> {
    let mut request = json!({"op": op, "session": session});
    match (op, args) {
        ("send", [id, text]) => (request["id"], request["text"]) = (id.clone().into(), text.clone().into()),
        ("unqueue", [id]) => request["id"] = id.clone().into(),
        ("hand", []) => {}
        ("keys", [_, ..]) => request["keys"] = args.into(),
        ("answer", [flag, answers]) if flag == "--json" => request["answers"] = serde_json::from_str(answers).ok()?,
        ("permit", [decision]) if decision == "allow" || decision == "deny" => request["allow"] = (decision == "allow").into(),
        ("stop", []) => {}
        _ => return None,
    }
    Some(request)
}

/// Plays `request` into `session`'s pane; `pane` is the pane as sesh-mux last listed it.
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
        "start" => crate::start::start(source, session, request),
        other => Err(format!("unknown op {other:?}")),
    }
}

fn agent_pane<'a>(session: &str, pane: Option<&'a Value>) -> Result<(&'a Value, Agent)> {
    pane.and_then(|pane| Some((pane, Agent::of(pane)?)))
        .ok_or_else(|| format!("pane {session} does not run claude, codex or pi"))
}

#[derive(serde::Deserialize)]
struct Answer {
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    text: Option<String>,
}

fn answer(source: &mut dyn Source, session: &str, (pane, agent): (&Value, Agent), answers: &[Answer]) -> Result<()> {
    let pending = &pane["permission"];
    let Some((menu, submit)) = agent.question_menu().filter(|_| pending["tool"] == "AskUserQuestion") else {
        return Err(format!("no question is open in pane {session}"));
    };
    let path = transcript_path(pane).ok_or_else(|| format!("{} in pane {session} has reported no transcript", agent.name()))?;
    let question = Transcript::new(agent, path).open_question(&pending["input"]);
    let inputs = question_inputs(question.questions.as_ref().unwrap_or(&Value::Null), answers)?;
    let open = |screen: &str| screen.contains(menu) || screen.contains(submit);
    play(source, session, inputs, open, |screen| screen.contains(submit).then(|| vec![key("enter")]))
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
            match (text, digits.first()) {
                (Some(text), _) => inputs.extend([key(&free_row), Input::Text(text), key("enter")]),
                (None, Some(digit)) => inputs.push(key(digit)),
                (None, None) => return Err("each answer needs an option or text".into()),
            }
        }
    }
    Ok(inputs)
}

fn permit(source: &mut dyn Source, session: &str, (_, agent): (&Value, Agent), allow: bool) -> Result<()> {
    let (name, open) = agent.permit(allow).ok_or_else(|| format!("{} asks no permissions", agent.name()))?;
    play(source, session, vec![key(name)], |screen| screen.contains(open), |_| None)
}

/// The link Claude asks to be signed in at, after typing `/login` if its sign-in expired and
/// picking a Claude plan if it asks which way.
pub fn login_link(source: &mut dyn Source, pane: &str) -> Result<String> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let (mut typed, mut picked) = (false, false);
    loop {
        match Login::read(&source.read(pane, false)?) {
            Some(Login::Link(url)) => return Ok(url),
            Some(Login::Method(_)) if !picked => {
                source.input(pane, &key("1"))?;
                picked = true;
            }
            Some(Login::Expired) if !typed => {
                source.input(pane, &Input::Text("/login".into()))?;
                std::thread::sleep(KEY_PAUSE);
                source.input(pane, &key("enter"))?;
                typed = true;
            }
            _ if !(typed || picked) || Instant::now() > deadline => return Err(format!("Claude in pane {pane} is not asking to sign in")),
            _ => {}
        }
        std::thread::sleep(KEY_PAUSE);
    }
}

/// Types the code Claude's sign-in page showed, then gets past the screens a first login
/// shows, and the trust question a start would have answered.
pub fn login(source: &mut dyn Source, pane: &str, code: &str) -> Result<()> {
    if !matches!(Login::read(&source.read(pane, false)?), Some(Login::Link(_))) {
        return Err(format!("Claude in pane {pane} is not asking for a sign-in code"));
    }
    source.input(pane, &Input::Text(code.trim().into()))?;
    std::thread::sleep(KEY_PAUSE);
    source.input(pane, &key("enter"))?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let mut signed = false;
    loop {
        std::thread::sleep(KEY_PAUSE);
        let screen = source.read(pane, false)?;
        if let Some(error) = screen.lines().map(str::trim).find(|line| line.starts_with("OAuth error")) {
            // Enter brings the code prompt back for another try.
            source.input(pane, &key("enter"))?;
            return Err(error.to_string());
        }
        signed |= screen.contains("Login successful");
        if signed && screen.contains(CONTINUE) {
            source.input(pane, &key("enter"))?;
            let closing = Instant::now() + MENU_TIMEOUT;
            while Instant::now() < closing && source.read(pane, false)? == screen {
                std::thread::sleep(KEY_PAUSE);
            }
        } else if signed && !crate::start::answer(source, pane)? {
            return Ok(());
        }
        if Instant::now() > deadline {
            let tail: Vec<&str> = screen.lines().filter(|line| !line.trim().is_empty()).collect();
            return Err(format!("Claude did not finish signing in:\n{}", tail[tail.len().saturating_sub(6)..].join("\n")));
        }
    }
}

/// Plays `inputs` into a menu that `open` sees on screen, then waits for it to
/// close, answering any follow-up screen `confirm` recognises.
fn play(
    source: &mut dyn Source,
    pane_id: &str,
    inputs: Vec<Input>,
    open: impl Fn(&str) -> bool,
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

    /// sesh-mux as a test sees it: a screen that shows `menu` until a key is pressed, and every
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

    /// A screen that changes as inputs arrive: the first screen until one does, and so on.
    struct Screens(Vec<&'static str>, Vec<Input>);

    impl Source for Screens {
        fn panes(&mut self) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        fn read(&mut self, _: &str, _: bool) -> Result<String> {
            Ok(self.0[self.1.len().min(self.0.len() - 1)].into())
        }

        fn input(&mut self, _: &str, input: &Input) -> Result<()> {
            self.1.push(input.clone());
            Ok(())
        }
    }

    const LINK: &str = "https://claude.com/cai/oauth/authorize?code=true\n Paste code here if prompted >";

    #[test]
    fn a_sign_in_code_is_typed_and_the_screens_after_a_login_are_passed() {
        let mut source = Screens(vec![LINK, LINK, "Login successful. Press Enter to continue", "Security notes: Press Enter to continue", "❯ "], Vec::new());
        assert_eq!(login(&mut source, "w1:p1", " abc#def \n"), Ok(()));
        assert_eq!(keys(&source.1), ["Text(\"abc#def\")", "enter", "enter", "enter"]);
    }

    #[test]
    fn a_wrong_sign_in_code_says_why_and_asks_again() {
        let mut source = Screens(vec![LINK, LINK, "OAuth error: Invalid code. Please make sure the full code was copied\n Press Enter to retry."], Vec::new());
        assert_eq!(login(&mut source, "w1:p1", "x"), Err("OAuth error: Invalid code. Please make sure the full code was copied".into()));
        assert_eq!(keys(&source.1).last().unwrap(), "enter");
        assert!(login(&mut Screens(vec!["❯ "], Vec::new()), "w1:p1", "x").is_err());
    }

    #[test]
    fn the_sign_in_link_comes_after_picking_a_claude_plan() {
        let mut source = Screens(vec![" Select login method:\n ❯ 1. Claude account\n   2. Console", LINK], Vec::new());
        assert_eq!(login_link(&mut source, "w1:p1"), Ok("https://claude.com/cai/oauth/authorize?code=true".into()));
        assert_eq!(keys(&source.1), ["1"]);
        assert!(login_link(&mut Screens(vec!["❯ "], Vec::new()), "w1:p1").is_err());
    }

    #[test]
    fn an_expired_sign_in_is_started_with_slash_login() {
        let expired = "❯ Hello\n● Login expired · Please run /login\n          Not logged in · Run /login\n───\n❯ ";
        let mut source = Screens(vec![expired, expired, " Select login method:\n ❯ 1. Claude account\n   2. Console", LINK], Vec::new());
        assert_eq!(login_link(&mut source, "w1:p1"), Ok("https://claude.com/cai/oauth/authorize?code=true".into()));
        assert_eq!(keys(&source.1), ["Text(\"/login\")", "enter", "1"]);
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
        assert_eq!(request("send", "w1:p1", &args(&["sent.1", "hi"])), Some(json!({"op": "send", "session": "w1:p1", "id": "sent.1", "text": "hi"})));
        assert_eq!(request("unqueue", "w1:p1", &args(&["sent.1"])).unwrap()["id"], "sent.1");
        assert_eq!(request("send", "w1:p1", &args(&["hi"])), None);
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
            &answers(r#"[{"options":["Blue"]},{"options":["Cat","Fish"],"text":"hamster"},{"options":["Tea"],"text":"water"}]"#),
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
