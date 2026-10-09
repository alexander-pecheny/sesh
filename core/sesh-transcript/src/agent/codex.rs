use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::{block_text, creation_diff, shell_command, Entry, Ids};

/// Codex rollouts come in two shapes. Older ones log each response item; newer ones
/// add `item_completed` events that say everything once, so seeing one switches
/// the parser to those and it ignores the rest.
#[derive(Default)]
pub(super) struct Parser {
    items: bool,
    hidden_results: HashSet<String>,
    diffs: HashMap<String, String>,
}

impl super::Parse for Parser {
    fn line(&mut self, line: &Value, ids: &mut Ids, out: &mut Vec<Entry>) -> bool {
        self.read(line, ids, out);
        false
    }

    fn fresh(&self) -> Box<dyn super::Parse> {
        Box::<Self>::default()
    }

    /// A piece read on its own may come after the first `item_completed`.
    fn begin(&mut self, bytes: &[u8], hint: i64) {
        self.items = hint != 0 || crate::contains(bytes, b"\"item_completed\"");
    }

    fn hint(&self) -> i64 {
        self.items.into()
    }
}

impl Parser {
    fn read(&mut self, line: &Value, ids: &mut Ids, out: &mut Vec<Entry>) {
        let at = &line["timestamp"];
        let payload = &line["payload"];
        let kind = payload["type"].as_str().unwrap_or_default();
        if line["type"] == "event_msg" && kind == "item_completed" {
            self.items = true;
            item(&payload["item"], at, ids, out);
            return;
        }
        if self.items {
            return;
        }
        match (line["type"].as_str().unwrap_or_default(), kind) {
            ("event_msg", "user_message") => {
                let text = payload["message"].as_str().unwrap_or_default().trim();
                let images = strings(&payload["local_images"]);
                if !text.is_empty() || !images.is_empty() {
                    out.push(Entry::user(ids.next(), at, text.to_string(), images));
                }
            }
            ("event_msg", "agent_message") => {
                let text = payload["message"].as_str().unwrap_or_default().trim();
                if !text.is_empty() {
                    out.push(Entry::text_like(ids.next(), "text", at, text.to_string()));
                }
            }
            ("response_item", "reasoning") => {
                let text = block_text(&payload["summary"]);
                if !text.trim().is_empty() {
                    out.push(Entry::text_like(ids.next(), "thinking", at, text));
                }
            }
            ("response_item", "function_call" | "custom_tool_call") => {
                let (Some(call), Some(name)) =
                    (payload["call_id"].as_str(), payload["name"].as_str())
                else {
                    return;
                };
                let input = match &payload["arguments"] {
                    Value::String(arguments) => serde_json::from_str(arguments).unwrap_or_default(),
                    _ => serde_json::json!({ "input": payload["input"] }),
                };
                if name == "update_plan" {
                    let items: Vec<(String, String)> = input["plan"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|step| {
                            Some((
                                step["step"].as_str()?.to_string(),
                                step["status"].as_str()?.to_string(),
                            ))
                        })
                        .collect();
                    self.hidden_results.insert(call.to_string());
                    out.push(Entry::todo(ids.next(), at, &items));
                    return;
                }
                out.push(Entry::tool(ids.call(call), at, name, &input));
            }
            ("event_msg", "patch_apply_end") => {
                if let Some(call) = payload["call_id"].as_str() {
                    self.diffs
                        .insert(call.to_string(), changes_diff(&payload["changes"]));
                }
            }
            ("response_item", "function_call_output" | "custom_tool_call_output") => {
                let Some(call) = payload["call_id"].as_str() else {
                    return;
                };
                if self.hidden_results.contains(call) || payload["output"] == "Plan updated" {
                    return;
                }
                let output = match &payload["output"] {
                    Value::Object(output) => output
                        .get("content")
                        .or_else(|| output.get("output"))
                        .map(block_text)
                        .unwrap_or_default(),
                    output => block_text(output),
                };
                let (text, error) = exec_output(&output);
                out.push(
                    Entry::result(ids.next(), at, ids.call(call), text, error)
                        .with_diff(self.diffs.remove(call)),
                );
            }
            ("response_item", "web_search_call") => {
                let action = &payload["action"];
                out.push(Entry::tool(ids.next(), at, "web_search", action));
            }
            _ => {}
        }
    }
}

fn item(item: &Value, at: &Value, ids: &mut Ids, out: &mut Vec<Entry>) {
    let tool_id = |ids: &mut Ids| match item["id"].as_str() {
        Some(id) => ids.call(id),
        None => ids.next(),
    };
    let text_of = |content: &Value| {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    match item["type"].as_str().unwrap_or_default() {
        "UserMessage" => {
            let images = item["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["path"].as_str().map(str::to_string))
                .collect();
            let text = text_of(&item["content"]).trim().to_string();
            out.push(Entry::user(ids.next(), at, text, images));
        }
        "AgentMessage" | "Plan" => {
            let text = match &item["text"] {
                Value::String(text) => text.clone(),
                _ => text_of(&item["content"]),
            };
            if !text.trim().is_empty() {
                out.push(Entry::text_like(
                    ids.next(),
                    "text",
                    at,
                    text.trim().to_string(),
                ));
            }
        }
        "Reasoning" => {
            let text = strings(&item["summary_text"]).join("\n");
            if !text.trim().is_empty() {
                out.push(Entry::text_like(ids.next(), "thinking", at, text));
            }
        }
        "CommandExecution" => {
            let command = match &item["command"] {
                Value::Array(argv) => shell_command(argv),
                command => command.as_str().map(str::to_string),
            };
            let parsed: Vec<&str> = item["parsed_cmd"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["type"].as_str())
                .collect();
            let only = |kinds: &[&str]| {
                !parsed.is_empty() && parsed.iter().all(|kind| kinds.contains(kind))
            };
            let mut tool = Entry::tool(
                tool_id(ids),
                at,
                "shell",
                &serde_json::json!({ "command": command }),
            );
            if only(&["read"]) {
                tool.tool = Some("read");
            } else if only(&["search", "list_files"]) {
                tool.tool = Some("search");
            }
            let output = item["aggregated_output"]
                .as_str()
                .or(item["formatted_output"].as_str())
                .unwrap_or_default();
            let error = item["exit_code"].as_i64().is_some_and(|code| code != 0)
                || matches!(item["status"].as_str(), Some("failed" | "declined"));
            let result = Entry::result(ids.next(), at, tool.id.clone(), output.to_string(), error);
            out.extend([tool, result]);
        }
        "FileChange" => {
            let files: Vec<&String> = item["changes"]
                .as_object()
                .map(|changes| changes.keys().collect())
                .unwrap_or_default();
            let input = match files.as_slice() {
                [file] => serde_json::json!({ "path": file }),
                files => serde_json::json!({ "description": format!("{} files", files.len()) }),
            };
            let tool = Entry::tool(tool_id(ids), at, "apply_patch", &input);
            let output = [item["stdout"].as_str(), item["stderr"].as_str()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("")
                .trim()
                .to_string();
            let error = item["status"] != "completed";
            let result = Entry::result(ids.next(), at, tool.id.clone(), output, error)
                .with_diff(Some(changes_diff(&item["changes"])));
            out.extend([tool, result]);
        }
        "WebSearch" => out.push(Entry::tool(ids.next(), at, "web_search", item)),
        "ImageView" => out.push(Entry::tool(ids.next(), at, "view_image", item)),
        "McpToolCall" | "DynamicToolCall" => {
            let name = item["tool"].as_str().unwrap_or("tool");
            out.push(Entry::tool(ids.next(), at, name, &item["arguments"]));
        }
        "CollabAgentToolCall" => {
            let input = serde_json::json!({ "description": item["prompt"] });
            out.push(Entry::tool(ids.next(), at, "agent", &input));
        }
        _ => {}
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// Codex wraps command output in a header; keep the output and read the exit code.
fn exec_output(output: &str) -> (String, bool) {
    let (header, body) = match output.split_once("\nOutput:\n") {
        Some((header, body)) => (header, body),
        None => ("", output),
    };
    let code = header.lines().find_map(|line| {
        line.strip_prefix("Process exited with code ")
            .or_else(|| line.strip_prefix("Exit code: "))
            .and_then(|code| code.trim().parse::<i64>().ok())
    });
    let error = code.is_some_and(|code| code != 0)
        || output.starts_with("apply_patch verification failed")
        || output.starts_with("aborted");
    (body.to_string(), error)
}

fn changes_diff(changes: &Value) -> String {
    let mut diff = String::new();
    for (file, change) in changes.as_object().into_iter().flatten() {
        match change["unified_diff"].as_str() {
            Some(hunks) => {
                diff.push_str(&format!("--- {file}\n+++ {file}\n{hunks}"));
                if !hunks.ends_with('\n') {
                    diff.push('\n');
                }
            }
            None => diff.push_str(&creation_diff(
                file,
                change["content"].as_str().unwrap_or_default(),
            )),
        }
    }
    diff
}
