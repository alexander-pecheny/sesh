//! An Agent whose Transcript and menus a program of its own reads. One copy of that program runs
//! for the whole helper and reads every Transcript of its Agent, told apart by stream. It takes
//! one JSON object per line on stdin and answers each but `end` with one line on stdout:
//!
//! - `{"stream": 1, "line": {...}}`, a Transcript line, gets
//!   `{"entries": [...], "turn_over": false}`. An entry is `{"kind": "user" | "text" | "thinking",
//!   "text", "at"}`, `{"kind": "tool", "call", "name", "input", "at"}` or
//!   `{"kind": "result", "call", "output", "error", "at"}`.
//! - `{"stream": 1, "end": true}` says the stream is done with.
//! - `{"screen": "..."}` gets `{"menu": null}` or `{"menu": {"title", "options": [[keys, label]]}}`,
//!   where `keys` are what to press, as `"down enter"`.

use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::Agent;
use crate::screen::Menu;
use crate::{Entry, Ids};

struct Child {
    process: std::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

static CHILDREN: Mutex<Option<HashMap<Agent, Child>>> = Mutex::new(None);
static STREAMS: AtomicU64 = AtomicU64::new(1);

/// Sends `message` to the Agent's program, started if it is not running, and returns its answer.
/// A program that fails is dropped and started afresh next time, having lost its streams.
fn ask(agent: Agent, message: &Value, answer: bool) -> Option<Value> {
    let mut children = CHILDREN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let children = children.get_or_insert_with(HashMap::new);
    let child = match children.entry(agent) {
        Occupied(child) => child.into_mut(),
        Vacant(slot) => slot.insert(start(agent)?),
    };
    let reply = talk(child, message, answer);
    if reply.is_err() {
        children.remove(&agent);
    }
    reply.ok().flatten()
}

fn talk(child: &mut Child, message: &Value, answer: bool) -> std::io::Result<Option<Value>> {
    writeln!(child.stdin, "{message}")?;
    child.stdin.flush()?;
    if !answer {
        return Ok(None);
    }
    let mut line = String::new();
    if child.stdout.read_line(&mut line)? == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(serde_json::from_str(&line).ok())
}

fn start(agent: Agent) -> Option<Child> {
    let spec = agent.spec();
    let (program, args) = spec.plugin.as_deref()?.split_first()?;
    let program = match (&spec.dir, program.contains('/')) {
        (Some(dir), true) => dir.join(program),
        _ => program.into(),
    };
    let mut process = Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let stdin = process.stdin.take()?;
    let stdout = BufReader::new(process.stdout.take()?);
    Some(Child { process, stdin, stdout })
}

pub(super) fn menu(agent: Agent, screen: &str) -> Option<Menu> {
    agent.spec().plugin.as_ref()?;
    let reply = ask(agent, &json!({"screen": screen}), true)?;
    let menu = &reply["menu"];
    let options = menu["options"]
        .as_array()?
        .iter()
        .filter_map(|option| Some((option[0].as_str()?.to_string(), option[1].as_str()?.to_string())))
        .collect();
    Some(Menu { title: menu["title"].as_str().unwrap_or_default().to_string(), options })
}

pub(super) struct Parser {
    agent: Agent,
    stream: u64,
    used: bool,
    turn_over: bool,
}

impl Parser {
    pub(super) fn new(agent: Agent) -> Self {
        Self { agent, stream: STREAMS.fetch_add(1, Ordering::Relaxed), used: false, turn_over: false }
    }
}

impl Drop for Parser {
    fn drop(&mut self) {
        if self.used {
            ask(self.agent, &json!({"stream": self.stream, "end": true}), false);
        }
    }
}

impl super::Parse for Parser {
    fn line(&mut self, line: &Value, ids: &mut Ids, out: &mut Vec<Entry>) -> bool {
        self.used = true;
        let Some(reply) = ask(self.agent, &json!({"stream": self.stream, "line": line}), true) else {
            return false;
        };
        self.turn_over = reply["turn_over"] == true;
        out.extend(reply["entries"].as_array().into_iter().flatten().filter_map(|entry| entry_of(entry, ids)));
        false
    }

    fn fresh(&self) -> Box<dyn super::Parse> {
        Box::new(Self::new(self.agent))
    }

    fn turn_over(&self) -> bool {
        self.turn_over
    }
}

fn entry_of(entry: &Value, ids: &mut Ids) -> Option<Entry> {
    let at = &entry["at"];
    let text = || entry["text"].as_str().map(str::to_string);
    let call = entry["call"].as_str().map(|call| ids.call(call));
    Some(match entry["kind"].as_str()? {
        "user" => Entry::user(ids.next(), at, text()?, Vec::new()),
        "text" => Entry::text_like(ids.next(), "text", at, text()?),
        "thinking" => Entry::text_like(ids.next(), "thinking", at, text()?),
        "tool" => Entry::tool(call?, at, entry["name"].as_str().unwrap_or("tool"), &entry["input"]),
        "result" => {
            let output = entry["output"].as_str().unwrap_or_default().to_string();
            Entry::result(ids.next(), at, call?, output, entry["error"] == true)
        }
        _ => return None,
    })
}
