use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{block_text, creation_diff, Entry, Ids};

/// Lines Claude writes as the user that the user did not type.
const NOT_TYPED: [&str; 6] = [
    "<local-command-",
    "<bash-",
    "<task-notification>",
    "<system-reminder>",
    "[Request interrupted",
    "Caveat: ",
];

/// One piece of work left running in the background: the call that started it, what to call
/// it, and whether it is a subagent rather than a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Background {
    pub call: String,
    pub label: String,
    pub agent: bool,
}

#[derive(Default)]
pub(super) struct Parser {
    /// A subagent's own Transcript, whose every line is a sidechain of its parent's.
    pub(super) subagent: bool,
    /// Commands and Agents started in the background and not yet reported finished, by the
    /// call that started them, with what to call them.
    pub(super) background: Vec<Background>,
    /// Whether Claude's last reply ended its turn, so what herdr calls working is only Claude
    /// waiting on its background work.
    pub(super) turn_over: bool,
    questions: HashMap<String, String>,
    hidden_results: HashSet<String>,
    tasks: Vec<(String, String)>,
}

impl Parser {
    pub(super) fn new(subagent: bool) -> Self {
        Self {
            subagent,
            ..Default::default()
        }
    }

    pub(super) fn line(&mut self, line: &Value, ids: &mut Ids, out: &mut Vec<Entry>) {
        if (line["isSidechain"] == true && !self.subagent)
            || line["isMeta"] == true
            || line["isCompactSummary"] == true
        {
            return;
        }
        let at = &line["timestamp"];
        let content = &line["message"]["content"];
        match line["type"].as_str() {
            Some("user") => {
                if line["origin"]["kind"] == "task-notification" {
                    return self.finished(&block_text(content));
                }
                if line["origin"]["kind"]
                    .as_str()
                    .is_some_and(|kind| kind != "human")
                {
                    return;
                }
                if let Value::Array(blocks) = content {
                    for block in blocks.iter().filter(|block| block["type"] == "tool_result") {
                        self.result(line, block, ids, out);
                    }
                    if blocks.iter().any(|block| block["type"] == "tool_result") {
                        return;
                    }
                }
                if let Some(text) = typed_text(&block_text(content)) {
                    self.turn_over = false;
                    out.push(Entry::user(ids.next(), at, text, Vec::new()));
                }
            }
            Some("assistant") => {
                match line["message"]["stop_reason"].as_str() {
                    Some("end_turn") => self.turn_over = true,
                    Some(_) => self.turn_over = false,
                    None => {}
                }
                for block in content.as_array().into_iter().flatten() {
                    self.assistant_block(block, at, ids, out);
                }
            }
            _ => {}
        }
    }

    fn assistant_block(&mut self, block: &Value, at: &Value, ids: &mut Ids, out: &mut Vec<Entry>) {
        let text = |key: &str| {
            block[key]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        match block["type"].as_str() {
            Some("text") => {
                if let Some(text) = text("text") {
                    out.push(Entry::text_like(ids.next(), "text", at, text));
                }
            }
            Some("thinking") => {
                if let Some(text) = text("thinking") {
                    out.push(Entry::text_like(ids.next(), "thinking", at, text));
                }
            }
            Some("tool_use") => {
                let (Some(call), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                else {
                    return;
                };
                let input = &block["input"];
                // Claude writes the flag as a string; older versions wrote a boolean.
                if input["run_in_background"] == true || input["run_in_background"] == "true" {
                    let label = ["description", "command", "prompt"]
                        .iter()
                        .find_map(|key| input[*key].as_str())
                        .unwrap_or(name);
                    self.background.push(Background {
                        call: call.to_string(),
                        label: label.lines().next().unwrap_or_default().to_string(),
                        agent: super::tool_kind(name) == "task",
                    });
                }
                out.push(match name {
                    "AskUserQuestion" => {
                        let entry = question(&ids.tag, at, input);
                        self.questions.insert(call.to_string(), entry.id.clone());
                        entry
                    }
                    "TodoWrite" | "TaskCreate" | "TaskUpdate" => {
                        self.update_tasks(name, input);
                        self.hidden_results.insert(call.to_string());
                        Entry::todo(ids.next(), at, &self.tasks)
                    }
                    _ => Entry::tool(ids.call(call), at, name, input),
                });
            }
            _ => {}
        }
    }

    /// A `<task-notification>` names the call it reports on; any end status ends it.
    fn finished(&mut self, text: &str) {
        let tag = |name: &str| {
            let start = text.find(&format!("<{name}>"))? + name.len() + 2;
            let end = text[start..].find(&format!("</{name}>"))? + start;
            Some(text[start..end].trim().to_string())
        };
        let (Some(call), Some(status)) = (tag("tool-use-id"), tag("status")) else {
            return;
        };
        if status != "running" {
            self.background.retain(|work| work.call != call);
        }
    }

    fn update_tasks(&mut self, name: &str, input: &Value) {
        let field = |value: &Value, key: &str| value[key].as_str().map(str::to_string);
        match name {
            "TodoWrite" => {
                self.tasks = input["todos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|todo| Some((field(todo, "content")?, field(todo, "status")?)))
                    .collect();
            }
            "TaskCreate" => {
                if let Some(subject) = field(input, "subject") {
                    self.tasks.push((subject, "pending".into()));
                }
            }
            _ => {
                let Some(index) = input["taskId"]
                    .as_str()
                    .and_then(|id| id.parse::<usize>().ok())
                    .and_then(|id| id.checked_sub(1))
                    .filter(|index| *index < self.tasks.len())
                else {
                    return;
                };
                if let Some(subject) = field(input, "subject") {
                    self.tasks[index].0 = subject;
                }
                if let Some(status) = field(input, "status") {
                    self.tasks[index].1 = status;
                }
            }
        }
    }

    fn result(&mut self, line: &Value, block: &Value, ids: &mut Ids, out: &mut Vec<Entry>) {
        let Some(call) = block["tool_use_id"].as_str() else {
            return;
        };
        let result = &line["toolUseResult"];
        if self.hidden_results.contains(call) || todo_result(result) {
            return;
        }
        let at = &line["timestamp"];
        let call_id = match (self.questions.get(call), answers(result)) {
            (Some(id), _) => id.clone(),
            (None, Some(_)) => question(&ids.tag, at, result).id,
            (None, None) => ids.call(call),
        };
        let error = block["is_error"] == true;
        let text = block_text(&block["content"]);
        let mut entry =
            Entry::result(ids.next(), at, call_id, text, error).with_diff(edit_diff(result));
        entry.answers = answers(result);
        out.push(entry);
    }
}

/// What TodoWrite, TaskCreate and TaskUpdate return, which shows as their todo entry.
fn todo_result(result: &Value) -> bool {
    !result["newTodos"].is_null()
        || !result["updatedFields"].is_null()
        || result["task"]["subject"].is_string()
}

/// An answered AskUserQuestion's answers, in the order of its questions.
fn answers(result: &Value) -> Option<Vec<String>> {
    let answers = result["answers"].as_object()?;
    let questions = result["questions"].as_array()?;
    Some(
        questions
            .iter()
            .map(|question| {
                question["question"]
                    .as_str()
                    .and_then(|question| answers.get(question)?.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
    )
}

fn typed_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || NOT_TYPED.iter().any(|prefix| text.starts_with(prefix)) {
        return None;
    }
    if text.contains("<command-name>") {
        let tag = |name: &str| {
            let start = text.find(&format!("<{name}>"))? + name.len() + 2;
            let end = text[start..].find(&format!("</{name}>"))? + start;
            Some(text[start..end].trim().to_string())
        };
        let command = tag("command-name")?;
        return Some(match tag("command-args").filter(|args| !args.is_empty()) {
            Some(args) => format!("{command} {args}"),
            None => command,
        });
    }
    Some(text.to_string())
}

/// Its id comes from the questions, so the copy made from the permission hook
/// before Claude writes it and the copy in the Transcript are one entry.
pub(super) fn question(tag: &str, at: &Value, input: &Value) -> Entry {
    let questions: Vec<Value> = input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|question| {
            serde_json::json!({
                "question": question["question"],
                "header": question["header"],
                "multi": question["multiSelect"] == true,
                "options": question["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|option| serde_json::json!({
                        "label": option["label"],
                        "description": option["description"],
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let summary = super::first_line(
        questions
            .first()
            .and_then(|question| question["question"].as_str())
            .unwrap_or("Question"),
    );
    let questions = Value::from(questions);
    let id = format!(
        "{tag}.q{:08x}",
        super::fnv1a(questions.to_string().as_bytes())
    );
    Entry {
        questions: Some(questions),
        ..Entry::new(id, "question", at, summary)
    }
}

/// Claude's Edit and Write results carry a structured patch of the change.
fn edit_diff(result: &Value) -> Option<String> {
    let file = result["filePath"].as_str()?;
    let hunks = result["structuredPatch"]
        .as_array()
        .filter(|hunks| !hunks.is_empty());
    let Some(hunks) = hunks else {
        return (result["type"] == "create")
            .then(|| creation_diff(file, result["content"].as_str().unwrap_or_default()));
    };
    let mut diff = format!("--- {file}\n+++ {file}\n");
    for hunk in hunks {
        diff.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk["oldStart"], hunk["oldLines"], hunk["newStart"], hunk["newLines"]
        ));
        for line in hunk["lines"].as_array().into_iter().flatten() {
            diff.push_str(line.as_str().unwrap_or_default());
            diff.push('\n');
        }
    }
    Some(diff)
}
