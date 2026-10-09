//! An Agent session's Transcript as Conversation entries, shared by Claude, Codex and pi.

pub mod act;
pub mod agent;
pub use agent::claude::Background;
pub use agent::Agent;
pub mod follower;
pub mod log;
pub mod reconcile;
pub mod replay;
pub mod screen;
pub mod serve;
pub mod vault;

pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("SOURCE_HASH"));

use std::fs::File;
use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

pub const PROTOCOL: u32 = 2;

const CLIP_LINES: usize = 40;
const CLIP_BYTES: usize = 16 * 1024;
const SUMMARY_CHARS: usize = 120;
const WINDOW: u64 = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Entry {
    pub id: String,
    pub kind: &'static str,
    pub summary: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub questions: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answers: Option<Vec<String>>,
}

impl Entry {
    fn new(id: String, kind: &'static str, at: &Value, summary: String) -> Self {
        Self {
            id,
            kind,
            summary,
            at: at.as_str().unwrap_or_default().to_string(),
            ..Self::default()
        }
    }

    fn user(id: String, at: &Value, text: String, mut images: Vec<String>) -> Self {
        images.extend(image_paths(&text));
        images.dedup();
        Self {
            images: Some(images),
            ..Self::text_like(id, "user", at, text)
        }
    }

    fn text_like(id: String, kind: &'static str, at: &Value, text: String) -> Self {
        let summary = if kind == "thinking" {
            "Thinking".to_string()
        } else {
            first_line(&text)
        };
        Self {
            text: Some(text),
            ..Self::new(id, kind, at, summary)
        }
    }

    fn tool(id: String, at: &Value, name: &str, input: &Value) -> Self {
        let tool = tool_kind(name);
        let string = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| input[*key].as_str())
                .map(str::to_string)
        };
        let command = (tool == "bash")
            .then(|| match &input["command"] {
                Value::Array(argv) => shell_command(argv),
                _ => string(&["command", "cmd"]),
            })
            .flatten();
        let file = string(&["file_path", "path", "notebook_path"]).or_else(|| {
            string(&["input", "patch", "command"]).and_then(|patch| patch_files(&patch).next())
        });
        let description = string(&["description", "pattern", "query", "url"]);
        let detail = command
            .as_deref()
            .or(file.as_deref())
            .or(description.as_deref());
        let summary = match detail {
            Some(detail) => format!("{name}: {}", first_line(detail)),
            None => name.to_string(),
        };
        Self {
            tool: Some(tool),
            name: Some(name.to_string()),
            file,
            command,
            description,
            ..Self::new(id, "tool", at, summary)
        }
    }

    fn result(id: String, at: &Value, call: String, text: String, error: bool) -> Self {
        let summary = match first_line(&text) {
            line if line.is_empty() && error => "Error".to_string(),
            line if line.is_empty() => "Done".to_string(),
            line if error => format!("Error: {line}"),
            line => line,
        };
        Self {
            call: Some(call),
            text: Some(text),
            error: Some(error),
            truncated: Some(false),
            ..Self::new(id, "result", at, summary)
        }
    }

    fn with_diff(mut self, diff: Option<String>) -> Self {
        if let Some(diff) = diff.filter(|diff| !diff.is_empty()) {
            let (added, removed) = diff_counts(&diff);
            self.added = Some(added);
            self.removed = Some(removed);
            self.diff = Some(diff);
        }
        self
    }

    /// Deleted items stay in the Agent's list so later updates can find theirs by position.
    fn todo(id: String, at: &Value, items: &[(String, String)]) -> Self {
        let items: Vec<_> = items
            .iter()
            .filter(|(_, status)| status != "deleted")
            .collect();
        let done = items
            .iter()
            .filter(|(_, status)| status == "completed")
            .count();
        Self {
            items: Some(
                items
                    .iter()
                    .map(|(text, status)| serde_json::json!({"text": text, "status": status}))
                    .collect(),
            ),
            ..Self::new(
                id,
                "todo",
                at,
                format!("{done} of {} tasks done", items.len()),
            )
        }
    }

    /// The entry as `follow` sends it: result text and diff cut to their ends.
    pub fn clipped(&self) -> Self {
        let mut entry = self.clone();
        if entry.kind == "result" {
            let mut truncated = false;
            for field in [&mut entry.text, &mut entry.diff] {
                if let Some(value) = field.take() {
                    let (value, cut) = clip(value);
                    truncated |= cut;
                    *field = Some(value);
                }
            }
            entry.truncated = Some(truncated);
        }
        entry
    }
}

/// Hands out entry ids that stay the same however often the Transcript is read.
struct Ids {
    tag: String,
    prefix: String,
    next: usize,
}

impl Ids {
    fn next(&mut self) -> String {
        self.next += 1;
        format!("{}.{}", self.prefix, self.next - 1)
    }

    /// A tool entry's id comes from the Agent's call id, so its result can name it
    /// without the call's line having been read.
    fn call(&self, call: &str) -> String {
        format!("{}.call.{call}", self.tag)
    }
}

/// One Transcript, read incrementally. `entries` is what the Conversation shows now.
pub struct Transcript {
    pub path: PathBuf,
    pub offset: u64,
    /// Where the parsed lines begin; past 0 after a tail read.
    pub start: u64,
    tag: String,
    parser: Box<dyn agent::Parse>,
    pub entries: Vec<Entry>,
    window: u64,
}

impl Transcript {
    pub fn new(agent: Agent, path: impl Into<PathBuf>) -> Self {
        let path: PathBuf = path.into();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        Self {
            tag: format!("{:08x}", fnv1a(name.as_bytes())),
            parser: agent.parser(&path),
            path,
            offset: 0,
            start: 0,
            entries: Vec::new(),
            window: WINDOW,
        }
    }

    /// Reads complete lines up to `limit` (or the end) and returns whether the shown
    /// branch changed, which rebuilds `entries` instead of appending to them.
    pub fn read(&mut self, limit: Option<u64>) -> std::io::Result<bool> {
        let Some(mut file) = self.open()? else {
            return Ok(false);
        };
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        match limit {
            Some(limit) => file
                .take(limit.saturating_sub(self.offset))
                .read_to_end(&mut bytes)?,
            None => file.read_to_end(&mut bytes)?,
        };
        Ok(self.feed(&bytes))
    }

    /// Parses the lines before `end` (the file's end when None) from a window that
    /// doubles backwards until `enough` holds or it reaches the start of the file.
    pub fn read_tail(
        &mut self,
        end: Option<u64>,
        enough: impl Fn(&[Entry]) -> bool,
    ) -> std::io::Result<()> {
        let Some(mut file) = self.open()? else {
            return Ok(());
        };
        let end = match end {
            Some(end) => end,
            None => file.metadata()?.len(),
        };
        let mut size = self.window;
        loop {
            let (start, bytes) = window(&mut file, end, size)?;
            self.parser = self.parser.fresh();
            self.parser.begin(&bytes, 0);
            self.entries.clear();
            (self.start, self.offset) = (start, start);
            self.feed(&bytes);
            if start == 0 || enough(&self.entries) {
                return Ok(());
            }
            size = size.saturating_mul(2);
        }
    }

    /// Every entry from `offset` on, on every branch, for search to index a Transcript copy a
    /// piece at a time. `hint` is what `hint` said after the piece before; when this piece
    /// needs another, every piece is read again and `start` is 0.
    pub fn read_from(&mut self, offset: u64, hint: i64) -> std::io::Result<Vec<Entry>> {
        self.parser = self.parser.fresh();
        self.entries.clear();
        (self.start, self.offset) = (offset, offset);
        let Some(mut file) = self.open()? else {
            return Ok(Vec::new());
        };
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        self.parser.begin(&bytes, hint);
        self.feed(&bytes);
        if offset > 0 && self.parser.hint() != hint {
            return self.read_from(0, self.parser.hint());
        }
        Ok(self.parser.every(std::mem::take(&mut self.entries)))
    }

    /// What the next `read_from` needs to know of what was read, kept by the caller.
    pub fn hint(&self) -> i64 {
        self.parser.hint()
    }

    /// Up to `n` entries before entry `id`, oldest first, and whether earlier ones
    /// exist; None when the Transcript has no such entry.
    pub fn history(&mut self, id: &str, n: usize) -> std::io::Result<Option<(Vec<Entry>, bool)>> {
        let Some(end) = self.locate(id)? else {
            return Ok(None);
        };
        let before = |entries: &[Entry]| {
            entries
                .iter()
                .position(|entry| entry.id == id)
                .unwrap_or(entries.len())
        };
        self.read_tail(Some(end), |entries| before(entries) > n)?;
        let count = before(&self.entries);
        let page = self.entries[count.saturating_sub(n)..count].to_vec();
        Ok(Some((page, count > n)))
    }

    /// The end of the line that makes entry `id`. Offset ids name their line; a
    /// call or question id is found by searching backwards from the end.
    pub fn locate(&self, id: &str) -> std::io::Result<Option<u64>> {
        let Some(rest) = id
            .strip_prefix(self.tag.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            return Ok(None);
        };
        let Some(mut file) = self.open()? else {
            return Ok(None);
        };
        let needle = if let Some(call) = rest.strip_prefix("call.") {
            format!("\"{call}\"")
        } else if rest.starts_with('q') {
            "\"AskUserQuestion\"".to_string()
        } else {
            let Some(offset) = rest
                .split('.')
                .next()
                .and_then(|offset| offset.parse().ok())
            else {
                return Ok(None);
            };
            file.seek(SeekFrom::Start(offset))?;
            let mut line = Vec::new();
            std::io::BufReader::new(file).read_until(b'\n', &mut line)?;
            let end = offset + line.len() as u64;
            return Ok(self.makes(offset, &line, id).then_some(end));
        };
        let (mut end, mut size) = (file.metadata()?.len(), self.window);
        loop {
            let (start, bytes) = window(&mut file, end, size)?;
            let mut lines = Vec::new();
            let mut from = 0;
            while let Some(newline) = bytes[from..].iter().position(|&byte| byte == b'\n') {
                lines.push(from..from + newline + 1);
                from += newline + 1;
            }
            for line in lines.into_iter().rev() {
                let offset = start + line.start as u64;
                let line = &bytes[line];
                if contains(line, needle.as_bytes()) && self.makes(offset, line, id) {
                    return Ok(Some(offset + line.len() as u64));
                }
            }
            if start == 0 {
                return Ok(None);
            }
            (end, size) = (start, size.saturating_mul(2));
        }
    }

    /// Whether the line at `offset`, read on its own, makes entry `id`.
    fn makes(&self, offset: u64, line: &[u8], id: &str) -> bool {
        let mut alone = Self {
            path: self.path.clone(),
            offset,
            start: offset,
            tag: self.tag.clone(),
            parser: self.parser.fresh(),
            entries: Vec::new(),
            window: self.window,
        };
        alone.feed(line);
        alone.entries.iter().any(|entry| entry.id == id)
    }

    fn open(&self) -> std::io::Result<Option<File>> {
        match File::open(&self.path) {
            Ok(file) => Ok(Some(file)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> bool {
        let mut branched = false;
        let mut start = 0;
        while let Some(end) = bytes[start..].iter().position(|&byte| byte == b'\n') {
            let line = &bytes[start..start + end];
            let offset = self.offset + start as u64;
            start += end + 1;
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let mut ids = Ids {
                tag: self.tag.clone(),
                prefix: format!("{}.{offset}", self.tag),
                next: 0,
            };
            branched |= self.parser.line(&value, &mut ids, &mut self.entries);
        }
        self.offset += start as u64;
        branched
    }

    /// Whether the Agent's last reply ended its turn.
    pub fn turn_over(&self) -> bool {
        self.parser.turn_over()
    }

    /// What the Agent left running in the background: the starting call and its label.
    pub fn background(&self) -> &[Background] {
        self.parser.background()
    }

    /// Whether the Agent has ended its turn and waits only on work it left in the background.
    pub fn waiting(&self) -> bool {
        self.turn_over() && !self.background().is_empty()
    }

    /// Background work over the whole Transcript, so work started long ago still counts.
    pub fn scan_background(&mut self) -> std::io::Result<()> {
        match self.open()? {
            Some(file) => self.parser.scan(file),
            None => Ok(()),
        }
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.parser.find(&self.entries, id)
    }

    /// Claude writes AskUserQuestion to its Transcript only once it is answered, so
    /// the open question comes from its permission hook, under the id the
    /// Transcript will give it, dated like the entry before it.
    pub fn open_question(&self, input: &Value) -> Entry {
        let at = self
            .entries
            .last()
            .map(|entry| entry.at.clone())
            .unwrap_or_default();
        agent::claude::question(&self.tag, &Value::String(at), input)
    }
}

/// The complete lines among the `size` bytes before `end`, and where they start.
fn window(file: &mut File, end: u64, size: u64) -> std::io::Result<(u64, Vec<u8>)> {
    let from = end.saturating_sub(size);
    let skip = from.saturating_sub(1);
    file.seek(SeekFrom::Start(skip))?;
    let mut bytes = Vec::new();
    file.take(end - skip).read_to_end(&mut bytes)?;
    if from == 0 {
        return Ok((0, bytes));
    }
    Ok(match bytes.iter().position(|&byte| byte == b'\n') {
        Some(newline) => (skip + newline as u64 + 1, bytes.split_off(newline + 1)),
        None => (end, Vec::new()),
    })
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

pub fn tool_kind(name: &str) -> &'static str {
    let base = name
        .rsplit("__")
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    match base.as_str() {
        "bash" | "shell" | "exec_command" | "local_shell" | "shell_command" => "bash",
        "edit" | "multiedit" | "notebookedit" | "apply_patch" => "edit",
        "write" => "write",
        "read" | "view_image" => "read",
        "grep" | "glob" | "find" | "ls" | "search" | "toolsearch" => "search",
        "webfetch" | "websearch" | "fetch" | "web_search" => "fetch",
        "agent" | "task" | "subagent" => "task",
        _ => "other",
    }
}

/// `["/bin/zsh", "-lc", "ls"]` is the command `ls`.
fn shell_command(argv: &[Value]) -> Option<String> {
    let argv: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
    match argv.as_slice() {
        [_, flag, command] if flag.starts_with('-') && flag.ends_with('c') => {
            Some(command.to_string())
        }
        [] => None,
        argv => Some(argv.join(" ")),
    }
}

/// Files a Codex `*** Begin Patch` touches.
fn patch_files(patch: &str) -> impl Iterator<Item = String> + '_ {
    patch.lines().filter_map(|line| {
        ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .map(str::to_string)
    })
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(SUMMARY_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

fn image_paths(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| "'\"`()[]<>,;".contains(c)))
        .filter(|word| word.starts_with('/') || word.starts_with("~/"))
        .filter(|word| {
            word.rsplit_once('.').is_some_and(|(_, ext)| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "heif"
                )
            })
        })
        .map(str::to_string)
        .collect()
}

fn diff_counts(diff: &str) -> (usize, usize) {
    diff.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            (added, removed)
        } else if line.starts_with('+') {
            (added + 1, removed)
        } else if line.starts_with('-') {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

/// A unified diff that creates `file` with `content`.
fn creation_diff(file: &str, content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut diff = format!("--- /dev/null\n+++ {file}\n@@ -0,0 +1,{} @@\n", lines.len());
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

fn clip(text: String) -> (String, bool) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut cut = false;
    let mut text = if lines.len() > CLIP_LINES * 2 {
        cut = true;
        let mut kept = lines[..CLIP_LINES].concat();
        kept.push_str(&lines[lines.len() - CLIP_LINES..].concat());
        kept
    } else {
        text
    };
    if text.len() > CLIP_BYTES {
        cut = true;
        let head = floor_char_boundary(&text, CLIP_BYTES / 2);
        let tail = ceil_char_boundary(&text, text.len() - CLIP_BYTES / 2);
        text = format!("{}{}", &text[..head], &text[tail..]);
    }
    (text, cut)
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// Text from a string or an array of `{type: text, text}` blocks.
fn block_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests;
