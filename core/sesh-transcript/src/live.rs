//! What Claude's screen shows that its Transcript does not hold yet: the text it is still
//! writing, or has written and not saved, and its status line.

use crate::Entry;

/// How many of the latest entries the screen is checked against.
const RECENT: usize = 60;
/// Enough of a line to tell it apart from other entries' text.
const PROBE: usize = 40;
const MAX_CHARS: usize = 4000;
/// The status line sits this close above the prompt box; further up it is the Agent's text.
const STATUS_REACH: usize = 6;

#[derive(Debug, Default, PartialEq)]
pub struct Live {
    pub text: String,
    pub status: String,
}

#[derive(PartialEq)]
enum Kind {
    Prose,
    User,
    Other,
}

struct Paragraph<'a> {
    kind: Kind,
    lines: Vec<&'a str>,
}

/// The Live tail of Claude's `screen`, given the entries the Transcript holds so far.
pub fn live(screen: &str, entries: &[Entry]) -> Live {
    let lines: Vec<&str> = screen.lines().map(str::trim_end).collect();
    let Some(prompt) = (1..lines.len())
        .rev()
        .find(|&i| is_prompt(lines[i]) && is_rule(lines[i - 1]))
    else {
        return Live::default();
    };
    let mut content = &lines[..prompt - 1];
    let mut status = String::new();
    let reach = content.len().saturating_sub(STATUS_REACH);
    if let Some(at) = (reach..content.len())
        .rev()
        .find(|&i| status_line(content[i]).is_some())
    {
        status = status_line(content[at]).unwrap_or_default().to_string();
        content = &content[..at];
    }
    let corpus: Vec<String> = entries[entries.len().saturating_sub(RECENT)..]
        .iter()
        .flat_map(|entry| {
            [&entry.text, &entry.command, &entry.description, &entry.file]
                .into_iter()
                .flatten()
                .chain([&entry.summary])
                .map(|text| normalize(text))
        })
        .collect();
    let paragraphs = paragraphs(content);
    // A running tool reads "Reading <path>", whose entry holds only the path.
    let held = |line: &str| {
        let probe: String = normalize(line).chars().take(PROBE).collect();
        !probe.is_empty() && corpus.iter().any(|text| text.contains(&probe))
    };
    let covered = paragraphs.iter().rposition(|paragraph| {
        let line = first_line(paragraph);
        held(&line) || line.split_once(' ').is_some_and(|(_, rest)| held(rest))
    });
    let after = covered.map_or(0, |at| at + 1);
    let start = (after..paragraphs.len()).find(|&i| paragraphs[i].kind != Kind::Other);
    let text = start.map_or(String::new(), |start| {
        paragraphs[start..]
            .iter()
            .map(paragraph_text)
            .collect::<Vec<_>>()
            .join("\n\n")
    });
    let skip = text.chars().count().saturating_sub(MAX_CHARS);
    Live {
        text: text.chars().skip(skip).collect(),
        status,
    }
}

fn is_rule(line: &str) -> bool {
    let line = line.trim();
    line.chars().count() >= 10 && line.chars().all(|c| c == '─')
}

fn is_prompt(line: &str) -> bool {
    line.starts_with('❯') || line.starts_with('>')
}

/// Claude's spinner: a symbol, then a word ending in an ellipsis, then how long and how much.
fn status_line(line: &str) -> Option<&str> {
    let mut chars = line.chars();
    let symbol = chars.next()?;
    let rest = chars.as_str().strip_prefix(' ')?;
    let word = rest.split_whitespace().next()?;
    (!symbol.is_alphanumeric() && !"⏺●⎿❯│─ ".contains(symbol) && word.ends_with('…'))
        .then(|| rest.trim())
}

fn paragraphs<'a>(content: &[&'a str]) -> Vec<Paragraph<'a>> {
    content
        .split(|line| line.trim().is_empty())
        .filter(|lines| !lines.is_empty())
        // Notices such as "Update installed" sit at the right edge.
        .filter(|lines| !lines[0].starts_with(&" ".repeat(20)))
        .map(|lines| Paragraph {
            kind: if lines[0].starts_with("⏺ ") || lines[0].starts_with("● ") {
                Kind::Prose
            } else if is_prompt(lines[0]) {
                Kind::User
            } else {
                Kind::Other
            },
            lines: lines.to_vec(),
        })
        .collect()
}

/// The paragraph's first line without its marker or a running tool's " · 3s".
fn first_line(paragraph: &Paragraph) -> String {
    let line = strip(paragraph.lines[0]);
    let line = line.rsplit_once(" · ").map_or(line, |(line, _)| line);
    if paragraph.kind == Kind::User && line.starts_with("[Pasted text") {
        return String::new();
    }
    line.to_string()
}

fn strip(line: &str) -> &str {
    let line = line.trim_start();
    ["⏺ ", "● ", "❯ ", "> ", "⎿ "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker))
        .unwrap_or(line)
        .trim()
}

/// Prose rejoined into lines the chat can wrap at its own width; tool output keeps its own.
fn paragraph_text(paragraph: &Paragraph) -> String {
    let lines = paragraph.lines.iter().map(|line| strip(line));
    if paragraph.lines.iter().any(|line| line.contains('⎿')) {
        return lines.collect::<Vec<_>>().join("\n");
    }
    let mut text = String::new();
    for line in lines {
        let item = line.starts_with("- ")
            || line.starts_with("• ")
            || line
                .split_once(". ")
                .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if !text.is_empty() {
            text.push(if item { '\n' } else { ' ' });
        }
        text.push_str(line);
    }
    text
}

/// Letters and digits only, so wrapping and Markdown do not matter.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
