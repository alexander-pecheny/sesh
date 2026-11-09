//! Starting an Agent in a fresh pane (ADR 0020): sesh-mux starts it, and the follower gets it past
//! the menus some Agents open before their first prompt, watching from the Host rather than from
//! a device a round trip away. On failure the pane is closed, so no half-started Agent lingers.

use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::follower::{Input, Source};
use crate::listen::refusal;
use crate::screen::Login;
use crate::Agent;

/// Menus an Agent may open before its first prompt, and the keys that get past them. The user
/// picked the folder and pressed Start, which answers the trust question; a new Claude keeps its
/// default theme, updates are skipped and new hooks left for the user to trust.
const MENUS: [(&str, &[&str]); 5] = [
    ("Choose the text style that looks best", &["enter"]),
    ("Yes, I trust this folder", &["down", "enter"]),
    ("Do you trust the contents of this directory", &["enter"]),
    ("Skip until next version", &["down", "enter"]),
    ("Continue without trusting", &["down", "down", "enter"]),
];
const POLL: Duration = Duration::from_millis(100);
const TIMEOUT: Duration = Duration::from_secs(60);
/// How long a menu answered gets to close before it is taken for another of the same.
const MENU_CLOSE: Duration = Duration::from_secs(2);
/// How long sesh-mux may call the pane busy while its shell starts.
const SHELL_START: Duration = Duration::from_secs(2);
/// How much of the screen a failure shows.
const TAIL: usize = 12;

type Result<T> = std::result::Result<T, String>;

/// Starts the Agent `request` names in `pane`, and returns once it can take a prompt or waits on a
/// menu Sesh does not know, which the user answers in the Conversation.
pub fn start(source: &mut dyn Source, pane: &str, request: &Value) -> Result<()> {
    let name = request["name"].as_str().filter(|name| !name.is_empty()).ok_or("an Agent needs a name")?;
    let agent = request["agent"].as_str().and_then(Agent::named).ok_or("no such Agent")?;
    // Claude reads its settings from ~/.claude.json unless CLAUDE_CONFIG_DIR is set, even to
    // ~/.claude, where there are none and it asks to log in again.
    let default = crate::serve::expand("~/.claude");
    let mut env = serde_json::Map::new();
    if agent == Agent::Claude {
        let config = request["config"].as_str().filter(|config| !config.is_empty()).map_or(default.clone(), crate::serve::expand);
        install_hooks(&config);
        if config != default {
            env.insert("CLAUDE_CONFIG_DIR".into(), config.to_string_lossy().into_owned().into());
        }
    }
    let started = launch(source, pane, name, agent, request["args"].clone(), env, TIMEOUT);
    let Err(problem) = started else { return Ok(()) };
    let screen = source.call("pane.read", json!({"pane_id": pane, "source": "recent_unwrapped", "lines": 40}));
    let _ = source.input(pane, &Input::Close);
    let screen = screen.ok().and_then(|read| read["read"]["text"].as_str().map(str::to_string)).unwrap_or_default();
    let lines: Vec<&str> = screen.lines().filter(|line| !line.trim().is_empty()).collect();
    Err([&[problem.as_str()][..], &lines[lines.len().saturating_sub(TAIL)..]].concat().join("\n"))
}

/// Installs sesh-mux's hooks in Claude's config folder, which on a new Host may not have existed
/// when Sesh first looked, so sesh-mux learns which Transcript the pane's Claude writes.
fn install_hooks(dir: &std::path::Path) {
    if cfg!(test) || dir.join("hooks/sesh-mux-agent-state.sh").exists() {
        return;
    }
    // The CLI installs hooks itself; sesh-mux's server knows only its own config folder.
    let home = std::env::var("HOME").unwrap_or_default();
    let _ = Command::new(format!("{home}/.sesh/bin/sesh-mux")).args(["integration", "install", "claude"]).env("CLAUDE_CONFIG_DIR", dir).output();
}

fn launch(source: &mut dyn Source, pane: &str, name: &str, agent: Agent, args: Value, env: serde_json::Map<String, Value>, timeout: Duration) -> Result<()> {
    let params = json!({"name": name, "kind": agent.name(), "pane_id": pane, "args": args, "env": env, "timeout_ms": timeout.as_millis() as u64});
    let shell = Instant::now() + SHELL_START;
    while let Err(err) = source.call("agent.start", params.clone()) {
        if err["code"] != "agent_pane_busy" || Instant::now() > shell {
            return Err(refusal(&err));
        }
        std::thread::sleep(POLL);
    }
    let deadline = Instant::now() + timeout;
    let mut answers = MENUS.len();
    loop {
        if Instant::now() > deadline {
            return Err("timed out waiting for agent startup".into());
        }
        // Signing in is the user's to do, in the Conversation or simple mode's sheet.
        if Login::read(&source.read(pane, false).unwrap_or_default()).is_some() {
            return Ok(());
        }
        if answers > 0 && answer(source, pane)? {
            answers -= 1;
            continue;
        }
        let found = source.call("agent.get", json!({"target": name})).or_else(|_| source.call("agent.get", json!({"target": pane})));
        if let Ok(found) = found {
            let found = &found["agent"];
            if let Some(other) = found["agent"].as_str().filter(|other| *other != agent.name()) {
                return Err(format!("expected {}, detected {other}", agent.name()));
            }
            if found["name"].as_str() != Some(name) {
                return Err(format!("named agent {name} no longer owns the target terminal"));
            }
            match found["agent_status"].as_str() {
                // A menu drawn since the screen was read is answered on the next pass.
                Some("blocked") if answers > 0 && known(&source.read(pane, false).unwrap_or_default()).is_some() => continue,
                Some("blocked") => return Ok(()),
                Some("idle" | "done") if found["interactive_ready"] == true => return Ok(()),
                Some("idle" | "done") if found["launch_pending"] != true => return Err("agent process exited before becoming interactive".into()),
                _ => {}
            }
        }
        std::thread::sleep(POLL);
    }
}

fn known(screen: &str) -> Option<&'static [&'static str]> {
    MENUS.iter().find(|(prompt, _)| screen.contains(prompt)).map(|(_, keys)| *keys)
}

/// Gets past the menu on screen, if Sesh knows it, and waits a moment for it to close.
pub(crate) fn answer(source: &mut dyn Source, pane: &str) -> Result<bool> {
    let screen = source.read(pane, false).unwrap_or_default();
    let Some(keys) = known(&screen) else { return Ok(false) };
    source.input(pane, &Input::Keys(keys.iter().map(|key| key.to_string()).collect()))?;
    let closing = Instant::now() + MENU_CLOSE;
    while Instant::now() < closing && known(&source.read(pane, false).unwrap_or_default()) == Some(keys) {
        std::thread::sleep(POLL);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sesh-mux starting an Agent: the screens it shows in turn, each until a key is pressed, and
    /// the states agent.get reports in turn, the last one for good.
    struct Starting {
        screens: Vec<&'static str>,
        states: Vec<Value>,
        inputs: Vec<Input>,
        starts: Vec<Value>,
    }

    impl Starting {
        fn new(screens: &[&'static str], states: &[Value]) -> Self {
            Starting { screens: screens.to_vec(), states: states.to_vec(), inputs: Vec::new(), starts: Vec::new() }
        }
    }

    impl Source for Starting {
        fn panes(&mut self) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        fn read(&mut self, _: &str, _: bool) -> Result<String> {
            Ok(self.screens.first().copied().unwrap_or_default().to_string())
        }

        fn input(&mut self, _: &str, input: &Input) -> Result<()> {
            if matches!(input, Input::Keys(_)) && !self.screens.is_empty() {
                self.screens.remove(0);
            }
            self.inputs.push(input.clone());
            Ok(())
        }

        fn call(&mut self, method: &str, params: Value) -> std::result::Result<Value, Value> {
            match method {
                "agent.start" => {
                    self.starts.push(params);
                    Ok(json!({}))
                }
                "agent.get" if self.states.len() > 1 => Ok(json!({"agent": self.states.remove(0)})),
                "agent.get" => Ok(json!({"agent": self.states[0]})),
                "pane.read" => Ok(json!({"read": {"text": "Welcome\n\n  Error: no such model  \n"}})),
                other => Err(json!({"code": "unknown", "message": other})),
            }
        }
    }

    fn claude(status: &str, ready: bool, pending: bool) -> Value {
        json!({"agent": "claude", "name": "fix-claude", "agent_status": status, "interactive_ready": ready, "launch_pending": pending})
    }

    fn started(source: &mut Starting) -> Result<()> {
        start(source, "w1:p1", &json!({"name": "fix-claude", "agent": "claude", "args": ["--dangerously-skip-permissions"]}))
    }

    #[test]
    fn another_config_folder_reaches_claude_on_its_command_line_and_its_own_is_left_unset() {
        let mut source = Starting::new(&[], &[claude("idle", true, false)]);
        let home = std::env::var("HOME").unwrap();
        for config in ["~/.claude", &format!("{home}/.claude"), "/nowhere/.claude-work"] {
            start(&mut source, "w1:p1", &json!({"name": "fix-claude", "agent": "claude", "config": config})).unwrap();
        }
        let env: Vec<&Value> = source.starts.iter().map(|start| &start["env"]).collect();
        assert_eq!(env, [&json!({}), &json!({}), &json!({"CLAUDE_CONFIG_DIR": "/nowhere/.claude-work"})]);
        assert!(source.inputs.is_empty(), "nothing is typed into the shell");
    }

    #[test]
    fn the_trust_menu_is_answered_and_the_agent_waited_for_until_it_takes_a_prompt() {
        let mut source = Starting::new(&["Yes, I trust this folder\nNo, exit"], &[claude("working", false, true), claude("idle", true, false)]);
        assert_eq!(started(&mut source), Ok(()));
        assert_eq!(source.inputs, [Input::Keys(vec!["down".into(), "enter".into()])]);
    }

    #[test]
    fn a_menu_sesh_does_not_know_is_left_for_the_user() {
        let mut source = Starting::new(&["Pick a theme"], &[claude("blocked", false, false)]);
        assert_eq!(started(&mut source), Ok(()));
        assert_eq!(source.inputs, []);
    }

    #[test]
    fn an_agent_that_exits_on_start_closes_its_pane_and_says_what_it_showed() {
        let mut source = Starting::new(&[], &[claude("idle", false, false)]);
        let problem = started(&mut source).unwrap_err();
        assert_eq!(problem, "agent process exited before becoming interactive\nWelcome\n  Error: no such model  ");
        assert_eq!(source.inputs, [Input::Close]);
    }

    #[test]
    fn another_agent_in_the_pane_is_a_failure() {
        let mut source = Starting::new(&[], &[json!({"agent": "codex", "name": "fix-claude", "agent_status": "idle"})]);
        assert!(started(&mut source).unwrap_err().starts_with("expected claude, detected codex"));
    }
}
