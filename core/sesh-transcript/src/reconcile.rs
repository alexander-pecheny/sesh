//! Which of Claude's screen rows the Transcript does not hold yet, which row a Transcript
//! entry rewrites, and where each lands in the Session log: matching decided in one place
//! (ADR 0010). See "Live items" in docs/PLAN.md.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::screen::{command, list_marker, markdown, Block, Kind, Row, View};
use crate::Entry;

/// A command shows only once the Transcript has had this long to name it.
const TOOL_DELAY: Duration = Duration::from_millis(500);
/// How long items outlive Claude's turn, while the Transcript catches up.
const GRACE: Duration = Duration::from_secs(3);
/// Enough of an item's words to tell it from other entries.
const PROBE: usize = 60;
/// How many entries' words are kept to recognise what the Transcript holds.
const CORPUS: usize = 400;
/// How long an Agent may sit idle with a message it was given and never showed (ADR 0015).
const LOST_AFTER: Duration = Duration::from_secs(10);

/// Where a message a device sent stands until the Transcript holds it (ADR 0015).
pub const QUEUED: &str = "queued";
pub const HANDED: &str = "handed";
pub const SENT: &str = "sent";
pub const SHOWN: &str = "shown";
pub const LOST: &str = "lost";

// MARK: Matching

/// Letters and digits of what a reader sees: Markdown's marks, link addresses, fences and
/// list markers dropped, so the screen's text and the Transcript's compare.
fn words(markdown: &str) -> String {
    let mut out = String::new();
    for line in markdown.lines() {
        // Claude's screen spells a link `text (url)` where the Transcript has `[text](url)`,
        // so addresses count on neither side.
        let line = without_urls(line);
        let line = line.trim_start();
        if line.starts_with("```") {
            continue;
        }
        let line: String = match list_marker(line) {
            Some((_, length)) => line.chars().skip(length).collect(),
            None => line.to_string(),
        };
        let mut rest = line.as_str();
        while let Some(at) = rest.find("](") {
            out.extend(rest[..at].chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase));
            rest = rest[at..].find(')').map_or("", |end| &rest[at + end + 1..]);
        }
        out.extend(rest.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase));
    }
    out
}

fn without_urls(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(at) = ["https://", "http://"].iter().filter_map(|scheme| rest.find(scheme)).min() {
        out.push_str(&rest[..at]);
        let end = rest[at..].find(|c: char| c.is_whitespace() || c == ')').map_or(rest.len(), |end| at + end);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn probe(words: &str) -> &str {
    let end = words.char_indices().nth(PROBE).map_or(words.len(), |(at, _)| at);
    &words[..end]
}

/// Whether `held`, an entry's words, covers `shown`, an item's: it starts the same way, or
/// for a short item, is the same.
fn covers(held: &str, shown: &str) -> bool {
    let probe = probe(shown);
    !probe.is_empty() && held.starts_with(probe)
}

/// The words of what the Transcript holds, by kind, newest last.
#[derive(Default)]
struct Corpus {
    replies: Vec<String>,
    users: Vec<String>,
    commands: Vec<String>,
}

impl Corpus {
    fn learn(&mut self, entry: &Entry) {
        // The same command often runs again in a later turn; only this turn's count.
        if entry.kind == "user" {
            self.commands.clear();
        }
        let list = match entry.kind {
            "text" => &mut self.replies,
            "user" => &mut self.users,
            "tool" => &mut self.commands,
            _ => return,
        };
        let text = match entry.kind {
            "tool" => entry.command.clone().or_else(|| entry.description.clone()).unwrap_or_default(),
            _ => entry.text.clone().unwrap_or_default(),
        };
        list.push(words(&text));
        if list.len() > CORPUS {
            list.remove(0);
        }
    }

    /// Whether the Transcript holds a block: cut blocks by any of their words, whole ones by
    /// their start.
    fn holds(&self, block: &Block, width: usize) -> bool {
        let (list, shown) = match block.kind {
            Kind::Reply => (&self.replies, markdown(&block.rows, width).1),
            Kind::User => (&self.users, block.rows.iter().map(|row| row.skip(2).text()).collect::<Vec<_>>().join(" ")),
            Kind::Tool => match command(block, width) {
                Some((command, _)) => (&self.commands, command),
                None => return false,
            },
            Kind::Other => return false,
        };
        let shown = words(&shown);
        if block.cut {
            let probe = probe(&shown);
            return !probe.is_empty() && list.iter().any(|held| held.contains(probe));
        }
        list.iter().any(|held| covers(held, &shown))
    }
}

/// A message a device sent, from the moment it is sent until the Transcript holds it.
struct Message {
    id: String,
    text: String,
    words: String,
    state: &'static str,
    /// When it last took its state.
    at: Instant,
}

struct Item {
    id: String,
    kind: Kind,
    rows: Vec<Row>,
    seen: Instant,
    entry: Entry,
    /// What a reader sees, for comparing with the Transcript.
    words: String,
}

/// The provisional items, kept from read to read, and what the Transcript already holds.
#[derive(Default)]
pub struct Reconciler {
    items: Vec<Item>,
    messages: Vec<Message>,
    /// Since when the Agent has been idle, if it is.
    idle: Option<Instant>,
    status: String,
    corpus: Corpus,
    next: u64,
    /// When Claude stopped working, if it has.
    stopped: Option<Instant>,
    /// The last view used, so a changed screen is read twice.
    last: Option<View>,
}

impl Reconciler {
    /// Takes over a previous run's provisional items, which keep their ids.
    pub fn adopt(&mut self, items: &[Entry], now: Instant) {
        for item in items {
            if item.kind == "user" {
                self.message(&item.id, item.text.as_deref().unwrap_or_default(), item.state.unwrap_or(SENT), now);
                continue;
            }
            let kind = if item.kind == "tool" { Kind::Tool } else { Kind::Reply };
            let entry = Entry { id: String::new(), ..item.clone() };
            let seen = now.checked_sub(TOOL_DELAY).unwrap_or(now);
            self.items.push(Item { id: item.id.clone(), kind, rows: Vec::new(), seen, words: entry_words(&entry), entry });
        }
    }

    /// Learns what the Transcript held before this run, which no item can be.
    pub fn learn(&mut self, entry: &Entry) {
        self.corpus.learn(entry);
    }

    /// Adds an entry the Transcript delivered and returns the item it replaces, if one was shown.
    pub fn deliver(&mut self, entry: &Entry, now: Instant) -> Option<String> {
        self.corpus.learn(entry);
        if entry.kind == "user" {
            let text = entry.text.as_deref().unwrap_or_default();
            let said = words(text);
            // A message of only emoji has no words to match by.
            let sent = |message: &Message| covers(&said, &message.words) || message.words.is_empty() && message.text.trim() == text.trim();
            let at = self.messages.iter().position(|message| message.state != QUEUED && sent(message))?;
            return Some(self.messages.remove(at).id);
        }
        let held = match entry.kind {
            "text" => words(entry.text.as_deref().unwrap_or_default()),
            "tool" if entry.tool == Some("bash") => words(entry.command.as_deref().unwrap_or_default()),
            _ => return None,
        };
        let kind = if entry.kind == "text" { Kind::Reply } else { Kind::Tool };
        let at = self.items.iter().position(|item| item.kind == kind && covers(&held, &item.words))?;
        let replaced = shown(&self.items[at], now).then(|| self.items[at].id.clone());
        self.items.drain(..=at);
        replaced
    }

    /// Whether the screen changed in what this follows, so it should be read again to be
    /// sure no redraw was half done.
    pub fn changed(&self, view: &View) -> bool {
        self.last.as_ref().is_none_or(|last| last.blocks != view.blocks)
    }

    /// Takes in one settled read of the screen.
    pub fn see(&mut self, view: View, now: Instant) {
        // Claude hides its spinner while a long reply streams; its word, not its stale count, stays.
        self.status = match (view.status.is_empty(), self.stopped) {
            (true, None) => self.status.split(" (").next().unwrap_or_default().to_string(),
            _ => view.status.clone(),
        };
        let held = view.blocks.iter().rposition(|block| self.corpus.holds(block, view.width));
        let fresh: Vec<&Block> = view
            .blocks
            .iter()
            .enumerate()
            .filter(|(index, block)| held.is_none_or(|held| *index > held) && !self.corpus.holds(block, view.width))
            .map(|(_, block)| block)
            .collect();
        let mut from = 0;
        for block in fresh {
            match block.kind {
                Kind::Reply => self.reply(block, view.width, now, &mut from),
                Kind::Tool => self.tool(block, view.width, now, &mut from),
                Kind::User => self.said(block, now),
                Kind::Other => {}
            }
        }
        self.last = Some(view);
    }

    fn reply(&mut self, block: &Block, width: usize, now: Instant, from: &mut usize) {
        if block.cut {
            let Some((index, rows)) = self.items[*from..].iter().enumerate().rev().find_map(|(index, item)| {
                (item.kind == Kind::Reply).then(|| Some((*from + index, extend(&item.rows, &block.rows)?))).flatten()
            }) else {
                return;
            };
            self.update(index, rows, width);
            *from = index + 1;
            return;
        }
        let (_, shown) = markdown(&block.rows, width);
        let start = words(&shown);
        if start.is_empty() {
            return;
        }
        let same = self.items[*from..].iter().position(|item| {
            item.kind == Kind::Reply && {
                let (a, b) = (probe(&item.words), probe(&start));
                let length = a.len().min(b.len()).min(20);
                (length >= 8 || a == b) && a[..floor(a, length)] == b[..floor(b, length)]
            }
        });
        match same {
            Some(index) => {
                self.update(*from + index, block.rows.clone(), width);
                *from += index + 1;
            }
            None => {
                self.add(Kind::Reply, block.rows.clone(), width, now);
                *from = self.items.len();
            }
        }
    }

    fn tool(&mut self, block: &Block, width: usize, now: Instant, from: &mut usize) {
        let Some((command, _)) = command(block, width) else { return };
        let words = words(&command);
        let same = self.items[*from..].iter().position(|item| item.kind == Kind::Tool && item.words == words);
        match same {
            Some(index) => {
                self.update(*from + index, block.rows.clone(), width);
                *from += index + 1;
            }
            None if !words.is_empty() => {
                self.add(Kind::Tool, block.rows.clone(), width, now);
                *from = self.items.len();
            }
            None => {}
        }
    }

    /// A message Claude shows as read; one it holds still says it can be sent now.
    fn said(&mut self, block: &Block, now: Instant) {
        if block.rows.iter().any(|row| row.text().trim_end().ends_with("to send now")) {
            return;
        }
        let text: Vec<String> = block.rows.iter().map(|row| row.skip(2).text()).collect();
        let text = text.join("\n");
        if let Some(message) = self.messages.iter_mut().find(|message| matches!(message.state, SENT | HANDED) && shows(&text, block.cut, message)) {
            (message.state, message.at) = (SHOWN, now);
        }
    }

    fn add(&mut self, kind: Kind, rows: Vec<Row>, width: usize, now: Instant) {
        self.next += 1;
        let entry = entry(kind, &rows, width);
        self.items.push(Item {
            id: format!("live.{}.{}", std::process::id(), self.next),
            kind,
            words: entry_words(&entry),
            rows,
            seen: now,
            entry,
        });
    }

    fn update(&mut self, index: usize, rows: Vec<Row>, width: usize) {
        let item = &mut self.items[index];
        // Claude hides a line while its Markdown is incomplete, such as a link still streaming.
        let shrinks = rows.len() < item.rows.len()
            && rows[..rows.len().saturating_sub(1)].iter().zip(&item.rows).all(|(new, old)| new.text() == old.text());
        if item.rows == rows || shrinks {
            return;
        }
        item.entry = entry(item.kind, &rows, width);
        item.words = entry_words(&item.entry);
        item.rows = rows;
    }

    /// Tells whether Claude works; once it has stopped for a while, every item goes.
    pub fn working(&mut self, working: bool, now: Instant) {
        if working {
            self.stopped = None;
            return;
        }
        let stopped = *self.stopped.get_or_insert(now);
        self.status.clear();
        if now.duration_since(stopped) >= GRACE {
            self.items.clear();
            self.last = None;
        }
    }

    /// Takes a message a device sent, or a new state for one held.
    pub fn message(&mut self, id: &str, text: &str, state: &'static str, now: Instant) {
        let message = Message { id: id.into(), text: text.into(), words: words(text), state, at: now };
        match self.messages.iter_mut().find(|held| held.id == id) {
            Some(held) => *held = message,
            None => self.messages.push(message),
        }
    }

    /// Takes back a message the Agent was not given or did not take, and says whether it could.
    pub fn unqueue(&mut self, id: &str) -> bool {
        let count = self.messages.len();
        self.messages.retain(|message| message.id != id || !matches!(message.state, QUEUED | LOST));
        self.messages.len() < count
    }

    /// Drops a message whose send failed, which the device that sent it takes back.
    pub fn forget(&mut self, id: &str) {
        self.messages.retain(|message| message.id != id);
    }

    pub fn lose(&mut self, id: &str, now: Instant) {
        if let Some(message) = self.messages.iter_mut().find(|message| message.id == id) {
            (message.state, message.at) = (LOST, now);
        }
    }

    /// The queued messages as one, under the first one's id, given the Agent in `state`: the
    /// Agent gets them in one prompt, so they show as one.
    pub fn hand(&mut self, state: &'static str, now: Instant) -> Option<(String, String)> {
        let queued: Vec<&Message> = self.messages.iter().filter(|message| message.state == QUEUED).collect();
        let id = queued.first()?.id.clone();
        let text = queued.iter().map(|message| message.text.as_str()).collect::<Vec<_>>().join("\n\n");
        self.messages.retain(|message| message.state != QUEUED || message.id == id);
        self.message(&id, &text, state, now);
        Some((id, text))
    }

    /// Tells whether the Agent is idle; a message it was given and has not shown after
    /// `LOST_AFTER` of that is lost.
    pub fn idle(&mut self, idle: bool, now: Instant) {
        let Some(since) = idle.then(|| *self.idle.get_or_insert(now)) else {
            self.idle = None;
            return;
        };
        for message in self.messages.iter_mut().filter(|message| matches!(message.state, SENT | HANDED)) {
            if now.duration_since(since.max(message.at)) >= LOST_AFTER {
                (message.state, message.at) = (LOST, now);
            }
        }
    }

    /// Whether the screen is still worth reading.
    pub fn watching(&self, now: Instant) -> bool {
        self.stopped.is_none_or(|stopped| now.duration_since(stopped) < GRACE)
    }

    /// The items shown now, each entry carrying its item's id, and the status line.
    /// The messages come first, so one the Agent takes lands above the reply read with it.
    pub fn shown(&self, now: Instant) -> Shown {
        let messages = self.messages.iter().map(|message| Entry { state: Some(message.state), ..Entry::text_like(message.id.clone(), "user", &Value::Null, message.text.clone()) });
        let items = self.items.iter().filter(|item| shown(item, now)).map(|item| Entry { id: item.id.clone(), ..item.entry.clone() });
        Shown { items: messages.chain(items).collect(), status: self.status.clone() }
    }
}

/// What the screen shows beyond the Transcript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub items: Vec<Entry>,
    pub status: String,
}

fn shown(item: &Item, now: Instant) -> bool {
    item.kind == Kind::Reply || now.duration_since(item.seen) >= TOOL_DELAY
}

/// `seen` carried on by `more`, the rows a later read shows from somewhere inside it.
fn extend(seen: &[Row], more: &[Row]) -> Option<Vec<Row>> {
    let skip = more.iter().take_while(|row| row.blank()).count();
    let more = &more[skip..];
    let first = more.first()?;
    (0..seen.len()).find_map(|at| {
        let overlap = seen.len() - at;
        let agrees = seen[at] == *first
            && seen[at..].iter().zip(more).take(overlap.saturating_sub(1)).all(|(a, b)| a == b);
        agrees.then(|| [&seen[..at], more].concat())
    })
}

/// Whether a user block on Claude's screen, as `text`, shows `message`. Claude shows a long
/// paste as "[Pasted text #1 +12 lines]", which stands for that many more lines.
fn shows(text: &str, cut: bool, message: &Message) -> bool {
    let mut parts = Vec::new();
    let mut lines = None;
    let mut rest = text;
    while let Some(start) = rest.find("[Pasted text #") {
        let Some(end) = rest[start..].find(']').map(|end| start + end) else { break };
        parts.push(words(&rest[..start]));
        let more = rest[start..end].split(" +").nth(1).and_then(|count| count.trim_end_matches(" lines").parse::<usize>().ok());
        lines = Some(lines.unwrap_or(0) + more.unwrap_or(0));
        rest = &rest[end + 1..];
    }
    parts.push(words(rest));
    let Some(lines) = lines else {
        let shown = &parts[0];
        return !shown.is_empty() && match cut {
            true => message.words.contains(probe(shown)),
            false => *shown == message.words || shown.chars().count() >= PROBE && message.words.starts_with(shown.as_str()),
        };
    };
    let mut from = 0;
    for part in parts.iter().filter(|part| !part.is_empty()) {
        let Some(at) = message.words[from..].find(part.as_str()) else { return false };
        from += at + part.len();
    }
    message.text.trim().lines().count().abs_diff(lines + 1) <= 1
}

fn floor(text: &str, mut at: usize) -> usize {
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// An item's entry, undated: Sesh dates it when it first shows it.
fn entry(kind: Kind, rows: &[Row], width: usize) -> Entry {
    match kind {
        Kind::Tool => {
            let block = Block { kind, rows: rows.to_vec(), cut: false };
            let (command, description) = command(&block, width).unwrap_or_default();
            item_entry(String::new(), kind, command, description)
        }
        _ => item_entry(String::new(), kind, markdown(rows, width).0, None),
    }
}

fn item_entry(id: String, kind: Kind, text: String, description: Option<String>) -> Entry {
    let at = Value::Null;
    if kind != Kind::Tool {
        return Entry::text_like(id, "text", &at, text);
    }
    let mut input = json!({"command": text});
    if let Some(description) = description {
        input["description"] = description.into();
    }
    Entry::tool(id, &at, "Bash", &input)
}

/// A provisional row's entry as the Session log holds it, under the row's id.
fn row_entry(id: &str, body: &Value) -> Option<Entry> {
    let text = |key: &str| body[key].as_str().map(str::to_string);
    match body["kind"].as_str()? {
        "text" => Some(item_entry(id.into(), Kind::Reply, text("text")?, None)),
        "tool" => Some(item_entry(id.into(), Kind::Tool, text("command")?, text("description"))),
        "user" => {
            let state = [QUEUED, HANDED, SENT, SHOWN, LOST].into_iter().find(|state| body["state"] == *state)?;
            Some(Entry { state: Some(state), ..Entry::text_like(id.into(), "user", &Value::Null, text("text")?) })
        }
        _ => None,
    }
}

fn entry_words(entry: &Entry) -> String {
    words(entry.command.as_deref().or(entry.text.as_deref()).unwrap_or_default())
}

// MARK: Placing

/// A write the Session log makes for what the reconciler decided.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// A provisional row: a new one goes last, others keep their place.
    Show(String, Value),
    /// A provisional row the Transcript never held.
    Drop(String),
    /// A Transcript entry rewriting the provisional row that showed it.
    Rewrite { id: String, entry: String, body: Value },
    /// A message the Agent took, which only now finds its place: after every row but the
    /// messages still `waiting`. Its entry is the Transcript's, when that is how it was taken.
    Taken { id: String, entry: Option<String>, body: Value, waiting: Vec<String> },
    /// A final row that showed nowhere before: it lands above `above`, the provisional rows,
    /// or rewrites the row already holding `entry`, as after a restart.
    Land { id: String, entry: Option<String>, body: Value, above: Vec<String> },
}

/// One follower's rows in the Session log: the provisional ones shown now, under ids kept
/// apart from those of a follower before it, which numbered its items from one as well.
pub struct Rows {
    /// The log's head when this follower began.
    epoch: i64,
    live: Vec<(String, Entry)>,
    /// The provisional rows an earlier follower left, which this one carries on under their ids.
    adopted: Vec<String>,
    switches: u64,
}

impl Rows {
    /// `left` is the provisional rows an earlier follower left, as ids and bodies.
    pub fn new(epoch: i64, left: &[(String, Value)]) -> Self {
        let live: Vec<(String, Entry)> = left.iter().filter_map(|(id, body)| Some((id.clone(), row_entry(id, body)?))).collect();
        let adopted = live.iter().map(|(id, _)| id.clone()).collect();
        Rows { epoch, live, adopted, switches: 0 }
    }

    /// The provisional items an earlier follower left, each carrying its row's id.
    pub fn adopted(&self) -> Vec<Entry> {
        self.live.iter().map(|(_, entry)| entry.clone()).collect()
    }

    /// What the screen shows now, in place of what it showed before.
    pub fn show(&mut self, items: &[Entry]) -> Vec<Change> {
        let shown: Vec<String> = items.iter().map(|item| self.own(&item.id)).collect();
        let mut changes: Vec<Change> = self.live.iter().filter(|(id, _)| !shown.contains(id)).map(|(id, _)| Change::Drop(id.clone())).collect();
        let held = std::mem::take(&mut self.live);
        let waiting: Vec<String> = shown.iter().zip(items).filter(|(_, item)| waits(item)).map(|(id, _)| id.clone()).collect();
        for (id, item) in shown.into_iter().zip(items) {
            let was = held.iter().find(|(old, _)| *old == id).map(|(_, was)| was);
            let body = serde_json::to_value(item).unwrap_or_default();
            if was.is_some_and(|was| waits(was) && !waits(item)) {
                changes.push(Change::Taken { id: id.clone(), entry: None, body, waiting: waiting.clone() });
            } else if was != Some(item) {
                changes.push(Change::Show(id.clone(), body));
            }
            self.live.push((id, item.clone()));
        }
        changes
    }

    /// A Transcript entry, naming the shown item it replaces.
    pub fn land(&mut self, entry: &Entry, replaces: Option<&str>) -> Change {
        let body = serde_json::to_value(entry.clipped()).unwrap_or_default();
        let Some(id) = replaces.map(|id| self.own(id)) else {
            return Change::Land { id: entry.id.clone(), entry: Some(entry.id.clone()), body, above: self.above() };
        };
        let waited = self.live.iter().any(|(live, held)| *live == id && waits(held));
        self.live.retain(|(live, _)| *live != id);
        let waiting = self.live.iter().filter(|(_, held)| waits(held)).map(|(live, _)| live.clone()).collect();
        match waited {
            true => Change::Taken { id, entry: Some(entry.id.clone()), body, waiting },
            false => Change::Rewrite { id, entry: entry.id.clone(), body },
        }
    }

    /// The mark where the Agent session moved to another Transcript.
    pub fn switch(&mut self, reason: &str, transcript: &str) -> Change {
        self.switches += 1;
        let id = format!("switch.{}.{}", self.epoch, self.switches);
        let body = json!({"id": id, "kind": "switch", "summary": reason, "transcript": transcript});
        Change::Land { id, entry: None, body, above: self.above() }
    }

    fn above(&self) -> Vec<String> {
        self.live.iter().map(|(id, _)| id.clone()).collect()
    }

    /// A screen item's id, made this follower's own; a message keeps the id its device gave it.
    fn own(&self, live: &str) -> String {
        if !live.starts_with("live.") || self.adopted.iter().any(|id| id == live) {
            return live.to_string();
        }
        format!("{live}.{}", self.epoch)
    }
}

/// A message the Agent has not taken, which has no place in the order yet: it goes last once
/// taken, where a message sent to an idle Agent already is.
fn waits(entry: &Entry) -> bool {
    matches!(entry.state, Some(QUEUED | HANDED | SENT | LOST))
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
