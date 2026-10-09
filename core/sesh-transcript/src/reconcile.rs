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
                _ => {}
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

    /// Whether the screen is still worth reading.
    pub fn watching(&self, now: Instant) -> bool {
        self.stopped.is_none_or(|stopped| now.duration_since(stopped) < GRACE)
    }

    /// The items shown now, each entry carrying its item's id, and the status line.
    pub fn shown(&self, now: Instant) -> Shown {
        let items = self.items.iter().filter(|item| shown(item, now)).map(|item| Entry { id: item.id.clone(), ..item.entry.clone() }).collect();
        Shown { items, status: self.status.clone() }
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
        for (id, item) in shown.into_iter().zip(items) {
            if !held.iter().any(|(old, was)| *old == id && was == item) {
                changes.push(Change::Show(id.clone(), serde_json::to_value(item).unwrap_or_default()));
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
        self.live.retain(|(live, _)| *live != id);
        Change::Rewrite { id, entry: entry.id.clone(), body }
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

    fn own(&self, live: &str) -> String {
        if self.adopted.iter().any(|id| id == live) {
            return live.to_string();
        }
        format!("{live}.{}", self.epoch)
    }
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
