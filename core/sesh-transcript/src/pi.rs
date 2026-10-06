use std::collections::HashMap;

use serde_json::Value;

use super::{block_text, creation_diff, Entry, Ids};

struct Node {
    parent: Option<String>,
    entries: Vec<Entry>,
}

/// pi's Transcript is a tree; the Conversation follows the branch ending at the
/// newest entry, which is where pi appends.
#[derive(Default)]
pub(super) struct Parser {
    nodes: HashMap<String, Node>,
    leaf: Option<String>,
    /// A write call's diff, which its result does not repeat.
    writes: HashMap<String, String>,
}

impl Parser {
    /// Returns true when the new entry starts another branch, so `out` was rebuilt.
    pub(super) fn line(&mut self, line: &Value, ids: &mut Ids, out: &mut Vec<Entry>) -> bool {
        let Some(id) = line["id"].as_str().filter(|_| line["type"] != "session") else {
            return false;
        };
        let parent = line["parentId"].as_str().map(str::to_string);
        let entries = match line["type"].as_str() {
            Some("message") => self.message(&line["message"], &line["timestamp"], ids),
            _ => Vec::new(),
        };
        let continues = parent == self.leaf;
        if continues {
            out.extend(entries.iter().cloned());
        }
        self.nodes.insert(id.to_string(), Node { parent, entries });
        self.leaf = Some(id.to_string());
        if !continues {
            *out = self.branch();
        }
        !continues
    }

    fn branch(&self) -> Vec<Entry> {
        let mut path = Vec::new();
        let mut cursor = self.leaf.as_deref();
        while let Some(node) = cursor.and_then(|id| self.nodes.get(id)) {
            path.push(node);
            cursor = node.parent.as_deref();
        }
        path.iter()
            .rev()
            .flat_map(|node| node.entries.iter().cloned())
            .collect()
    }

    pub(super) fn find(&self, id: &str) -> Option<&Entry> {
        self.all().find(|entry| entry.id == id)
    }

    pub(super) fn all(&self) -> impl Iterator<Item = &Entry> {
        self.nodes.values().flat_map(|node| &node.entries)
    }

    fn message(&mut self, message: &Value, at: &Value, ids: &mut Ids) -> Vec<Entry> {
        let content = &message["content"];
        match message["role"].as_str() {
            Some("user") => {
                let text = block_text(content).trim().to_string();
                vec![Entry::user(ids.next(), at, text, Vec::new())]
            }
            Some("assistant") => content
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| self.assistant_block(block, at, ids))
                .collect(),
            Some("toolResult") => {
                let Some(call) = message["toolCallId"].as_str() else {
                    return Vec::new();
                };
                let diff = message["details"]["patch"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| self.writes.remove(call));
                let error = message["isError"] == true;
                vec![
                    Entry::result(ids.next(), at, ids.call(call), block_text(content), error)
                        .with_diff(diff),
                ]
            }
            _ => Vec::new(),
        }
    }

    fn assistant_block(&mut self, block: &Value, at: &Value, ids: &mut Ids) -> Option<Entry> {
        let text = |key: &str| {
            block[key]
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        match block["type"].as_str()? {
            "text" => Some(Entry::text_like(ids.next(), "text", at, text("text")?)),
            "thinking" => Some(Entry::text_like(
                ids.next(),
                "thinking",
                at,
                text("thinking")?,
            )),
            "toolCall" => {
                let (call, name) = (block["id"].as_str()?, block["name"].as_str()?);
                let input = &block["arguments"];
                if let (true, Some(path), Some(content)) = (
                    name == "write",
                    input["path"].as_str(),
                    input["content"].as_str(),
                ) {
                    self.writes
                        .insert(call.to_string(), creation_diff(path, content));
                }
                Some(Entry::tool(ids.call(call), at, name, input))
            }
            _ => None,
        }
    }
}
