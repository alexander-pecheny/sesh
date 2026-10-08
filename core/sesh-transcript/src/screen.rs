//! Claude's screen, read with its styles, as the entries its Transcript will hold: the
//! replies it is still writing or has not saved yet, a command still running, and the
//! status line. See "Live items" in docs/PLAN.md.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::Entry;

/// A command shows only once the Transcript has had this long to name it.
const TOOL_DELAY: Duration = Duration::from_millis(500);
/// How long items outlive Claude's turn, while the Transcript catches up.
const GRACE: Duration = Duration::from_secs(3);
/// Enough of an item's words to tell it from other entries.
const PROBE: usize = 60;
/// How many entries' words are kept to recognise what the Transcript holds.
const CORPUS: usize = 400;
/// How far up from the bottom a menu in place of the prompt box can reach.
const MENU: usize = 30;
const SPINNERS: &str = "·✢✳✶✻✽*";
const MARKERS: [char; 2] = ['⏺', '●'];
/// Where a reply's link points is not on the screen; Sesh opens nothing for this.
pub const NO_LINK: &str = "about:blank";

// MARK: Styled rows

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Colour {
    #[default]
    Default,
    Index(u8),
    Rgb(u8, u8, u8),
}

impl Colour {
    /// Claude's text colour in a dark or light theme, as opposed to grey chrome or a hue.
    fn plain(self) -> bool {
        match self {
            Self::Default => true,
            Self::Index(n) => matches!(n, 0 | 7 | 15),
            Self::Rgb(r, g, b) => (r.min(g).min(b) >= 220) || (r.max(g).max(b) <= 50),
        }
    }

    fn grey(self) -> bool {
        match self {
            Self::Index(n) => matches!(n, 8 | 240..=250),
            Self::Rgb(r, g, b) => r == g && g == b && (51..=219).contains(&r),
            Self::Default => false,
        }
    }

    fn link(self) -> bool {
        self == Self::Index(12)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    fg: Colour,
    bg: Colour,
}

#[derive(Clone, Debug, PartialEq)]
struct Span {
    text: String,
    style: Style,
}

#[derive(Clone, Debug, Default)]
struct Row(Vec<Span>);

/// herdr gives a space next to a styled word either style, so only ink is compared.
impl PartialEq for Row {
    fn eq(&self, other: &Self) -> bool {
        let ink = |row: &Row| -> Vec<(char, Style)> {
            row.0.iter().flat_map(|span| span.text.chars().filter(|c| !c.is_whitespace()).map(move |c| (c, span.style))).collect()
        };
        self.text() == other.text() && ink(self) == ink(other)
    }
}

impl Row {
    fn text(&self) -> String {
        self.0.iter().map(|span| span.text.as_str()).collect::<String>().trim_end().to_string()
    }

    fn blank(&self) -> bool {
        self.text().trim().is_empty()
    }

    fn indent(&self) -> usize {
        let text = self.text();
        text.len() - text.trim_start().len()
    }

    /// The row without its first `columns` characters.
    fn skip(&self, columns: usize) -> Row {
        let mut left = columns;
        let mut spans = Vec::new();
        for span in &self.0 {
            let count = span.text.chars().count();
            if left >= count {
                left -= count;
                continue;
            }
            spans.push(Span { text: span.text.chars().skip(left).collect(), style: span.style });
            left = 0;
        }
        Row(spans)
    }

    /// The style of the first character that is not a space.
    fn first_style(&self) -> Option<Style> {
        self.0.iter().find(|span| !span.text.trim().is_empty()).map(|span| span.style)
    }
}

fn rows(ansi: &str) -> Vec<Row> {
    ansi.replace('\r', "").split('\n').map(row).collect()
}

fn row(line: &str) -> Row {
    let mut spans: Vec<Span> = Vec::new();
    let mut style = Style::default();
    let mut text = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            text.push(c);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        let mut params = String::new();
        let mut end = ' ';
        for c in chars.by_ref() {
            if c.is_ascii_alphabetic() {
                end = c;
                break;
            }
            params.push(c);
        }
        if end != 'm' {
            continue;
        }
        if !text.is_empty() {
            spans.push(Span { text: std::mem::take(&mut text), style });
        }
        sgr(&params, &mut style);
    }
    if !text.is_empty() {
        spans.push(Span { text, style });
    }
    Row(spans)
}

fn sgr(params: &str, style: &mut Style) {
    let codes: Vec<u16> = params.split([';', ':']).map(|code| code.parse().unwrap_or(0)).collect();
    let mut codes = codes.iter().copied();
    while let Some(code) = codes.next() {
        match code {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            9 => style.strike = true,
            22 => (style.bold, style.dim) = (false, false),
            23 => style.italic = false,
            24 => style.underline = false,
            29 => style.strike = false,
            30..=37 => style.fg = Colour::Index((code - 30) as u8),
            90..=97 => style.fg = Colour::Index((code - 90 + 8) as u8),
            39 => style.fg = Colour::Default,
            40..=47 => style.bg = Colour::Index((code - 40) as u8),
            100..=107 => style.bg = Colour::Index((code - 100 + 8) as u8),
            49 => style.bg = Colour::Default,
            38 | 48 => {
                let colour = match codes.next() {
                    Some(5) => Colour::Index(codes.next().unwrap_or(0) as u8),
                    Some(2) => {
                        let mut next = || codes.next().unwrap_or(0) as u8;
                        Colour::Rgb(next(), next(), next())
                    }
                    _ => Colour::Default,
                };
                if code == 38 { style.fg = colour } else { style.bg = colour }
            }
            _ => {}
        }
    }
}

/// How many cells a character takes: two for wide ones, such as CJK and most emoji.
fn cells(c: char) -> usize {
    let wide = matches!(c as u32, 0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF
        | 0x2705 | 0x274C | 0x2753..=0x2755 | 0x2757 | 0x2B50 | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

fn measure(text: &str) -> usize {
    text.chars().map(cells).sum()
}

// MARK: The screen's parts

/// One run of rows that starts with a marker, or a run cut off at the top of the screen.
#[derive(Clone, Debug, PartialEq)]
struct Block {
    kind: Kind,
    rows: Vec<Row>,
    /// Its start is above the screen.
    cut: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Reply,
    User,
    Tool,
    Other,
}

/// What one read of the screen shows: the conversation's blocks, the status line and the
/// width Claude wraps its text to.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    blocks: Vec<Block>,
    status: String,
    width: usize,
}

impl View {
    /// None when the prompt box is not on the screen: a menu covers it, or a redraw is half done.
    pub fn read(ansi: &str) -> Option<View> {
        let rows = rows(ansi);
        let texts: Vec<String> = rows.iter().map(Row::text).collect();
        let prompt = (0..texts.len().saturating_sub(2)).rev().find(|&i| {
            rule(&texts[i]) && (texts[i + 1].starts_with('❯') || texts[i + 1].starts_with('>'))
                && texts[i + 2..].iter().any(|text| rule(text))
        });
        // A permission or question menu takes the prompt box's place, under a rule of its own.
        let menu = || (texts.len().saturating_sub(MENU)..texts.len()).find(|&i| rule(&texts[i]));
        let top = prompt.or_else(menu)?;
        let width = measure(texts[top].trim_end());
        // The status line sits above the prompt box, among notices, tips and the todo list.
        let mut end = top;
        let mut status = String::new();
        for i in (top.saturating_sub(16)..top).rev() {
            let text = &texts[i];
            if let Some(line) = spinner(text) {
                (end, status) = (i, if finished(line) { String::new() } else { line.to_string() });
                break;
            }
            if text.starts_with(MARKERS) || text.starts_with('❯') {
                break;
            }
        }
        while end > 0 && (texts[end - 1].trim().is_empty() || notice(&texts[end - 1])) {
            end -= 1;
        }
        Some(View { blocks: blocks(&rows[..end]), status, width })
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    /// Whether two reads show the same conversation; the spinner may have moved.
    pub fn agrees(&self, other: &View) -> bool {
        self.blocks == other.blocks
    }
}

/// A menu in the prompt box's place that no hook reported, such as a check that asks even with
/// permissions skipped: what it asks, and its choices by the number that picks each.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: String,
    pub options: Vec<(String, String)>,
}

impl Menu {
    pub fn read(screen: &str) -> Option<Menu> {
        let texts: Vec<&str> = screen.lines().collect();
        let start = (texts.len().saturating_sub(MENU)..texts.len()).find(|&i| rule(texts[i]))?;
        let mut title = Vec::new();
        let mut options = Vec::new();
        for text in &texts[start + 1..] {
            let text = text.trim();
            if let Some(option) = numbered(text) {
                options.push(option);
            } else if options.is_empty() && !text.is_empty() && !rule(text) {
                title.push(text);
            }
        }
        (options.len() >= 2).then(|| Menu { title: title.join("\n"), options })
    }
}

/// "❯ 1. Yes" or "2. No": the number, and the words after it.
fn numbered(text: &str) -> Option<(String, String)> {
    let text = text.strip_prefix('❯').unwrap_or(text).trim_start();
    let (number, label) = text.split_once(". ")?;
    (!number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
        .then(|| (number.to_string(), label.trim().to_string()))
}

fn rule(text: &str) -> bool {
    let text = text.trim();
    text.chars().filter(|&c| c == '─').count() >= 10 && text.starts_with('─') && text.ends_with('─')
}

/// Text at the right edge, such as "✔ Update installed · Restart to update".
fn notice(text: &str) -> bool {
    text.starts_with(&" ".repeat(20))
}

/// The spinner's words, or a finished turn's, as in "✻ Churned for 6s".
fn spinner(text: &str) -> Option<&str> {
    let mut chars = text.chars();
    let glyph = chars.next()?;
    let rest = chars.as_str().strip_prefix(' ')?.trim();
    (SPINNERS.contains(glyph) && rest.chars().next().is_some_and(char::is_alphabetic)).then_some(rest)
}

/// A turn's closing line: "Churned for 6s · done 7:53 PM", "Crunched for 1m 5s".
fn finished(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    line.contains(" · done ")
        || (words.get(1) == Some(&"for")
            && words.get(2).is_some_and(|word| {
                word.ends_with(['s', 'm', 'h']) && word[..word.len() - 1].chars().all(|c| c.is_ascii_digit())
            }))
}

fn blocks(rows: &[Row]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut after_blank = true;
    for row in rows {
        let text = row.text();
        let first = text.chars().next();
        let lead = row.0.first().map(|span| span.style).unwrap_or_default();
        let output = text.trim_start().starts_with('⎿');
        let kind = if first.is_some_and(|c| !c.is_whitespace()) {
            Some(marked(row))
        } else if output {
            blocks.last().is_some_and(|block| block.kind == Kind::Reply).then_some(Kind::Other)
        } else if row.indent() == 2 && lead.fg != Colour::Default {
            // A running tool's marker blinks; while it is off, its cells keep their colour.
            Some(Kind::Tool)
        } else if after_blank && !text.trim().is_empty() && chrome(row) {
            Some(Kind::Other)
        } else {
            None
        };
        after_blank = text.trim().is_empty();
        match (kind, blocks.last_mut()) {
            (Some(kind), _) => blocks.push(Block { kind, rows: vec![row.clone()], cut: false }),
            (None, Some(block)) => block.rows.push(row.clone()),
            (None, None) if after_blank => {}
            (None, None) => {
                let style = row.first_style().unwrap_or_default();
                let kind = if style.bg != Colour::Default {
                    Kind::User
                } else if output || chrome(row) {
                    Kind::Other
                } else {
                    Kind::Reply
                };
                blocks.push(Block { kind, rows: vec![row.clone()], cut: true })
            }
        }
    }
    for block in &mut blocks {
        while block.rows.last().is_some_and(Row::blank) {
            block.rows.pop();
        }
    }
    blocks
}

/// What a row with a marker in its first column starts.
fn marked(row: &Row) -> Kind {
    let text = row.text();
    if text.starts_with('❯') || text.starts_with('>') {
        return Kind::User;
    }
    if !text.starts_with(MARKERS) {
        return Kind::Other;
    }
    let marker = row.0[0].style.fg;
    if marker.plain() {
        return Kind::Reply;
    }
    let after = row.skip(2);
    if after.first_style().is_some_and(|style| style.bold) || summary(&after.text()) {
        Kind::Tool
    } else {
        Kind::Other
    }
}

/// Grey chrome between replies: "Read 1 file, ran 1 shell command", a tool's summary line.
fn chrome(row: &Row) -> bool {
    let text = row.text();
    let lead = row.first_style().unwrap_or_default();
    (lead.fg.grey() || lead.dim && lead.fg == Colour::Default) && !text.trim_start().starts_with('▎')
}

/// A tool group's header, such as "Running 1 shell command…" or "Read 2 files".
fn summary(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().take(2).collect();
    words.len() == 2 && words[1].chars().all(|c| c.is_ascii_digit())
}

// MARK: Markdown from the screen

/// A reply's rows, marker and margin included, as the Markdown Claude wrote and the words a
/// reader sees.
fn markdown(rows: &[Row], width: usize) -> (String, String) {
    // Grey is Claude's chrome, such as a " · summary" label after the last line.
    let rows: Vec<Row> = rows
        .iter()
        .map(|row| Row(row.skip(2).0.into_iter().filter(|span| span.text.trim().is_empty() || !span.style.fg.grey()).collect()))
        .collect();
    let margin = 2;
    let mut out: Vec<String> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    // A blank line inside a code block splits it into paragraphs; they are one block again.
    let mut code_block: Option<String> = None;
    let mut coloured = false;
    let fence = |text: String| (format!("```{}\n{text}\n```", language(&text)), text);
    for paragraph in rows.split(Row::blank).filter(|rows| !rows.is_empty()) {
        let first = paragraph[0].text();
        let more_code = code_block.is_some() && (paragraph[0].indent() > 0 || coloured && highlighted(paragraph));
        if more_code || code(paragraph, width, margin) {
            let text = paragraph.iter().map(Row::text).collect::<Vec<_>>().join("\n");
            code_block = Some(match code_block.take() {
                Some(before) if more_code => format!("{before}\n\n{text}"),
                Some(before) => {
                    let (md, words) = fence(before);
                    out.push(md);
                    plain.push(words);
                    text
                }
                None => text,
            });
            coloured = highlighted(paragraph);
            continue;
        }
        if let Some(text) = code_block.take() {
            let (md, words) = fence(text);
            out.push(md);
            plain.push(words);
        }
        let (md, words) = if first.starts_with(['┌', '│', '├']) {
            table(paragraph)
        } else if first.trim_start().starts_with('▎') {
            quote(paragraph)
        } else if first.trim() == "---" && paragraph.len() == 1 {
            ("---".to_string(), String::new())
        } else {
            prose(paragraph, width, margin)
        };
        out.push(md);
        plain.push(words);
    }
    if let Some(text) = code_block {
        let (md, words) = fence(text);
        out.push(md);
        plain.push(words);
    }
    (out.join("\n\n"), plain.join("\n"))
}

/// Logical lines of prose: list items and lines the reply broke itself, with soft wraps joined.
fn prose(rows: &[Row], width: usize, margin: usize) -> (String, String) {
    let mut lines: Vec<(Option<String>, usize, Vec<Span>)> = Vec::new();
    let mut previous: Option<&Row> = None;
    for row in rows {
        let text = row.text();
        let indent = row.indent();
        let item = list_marker(text.trim_start());
        let wrapped = previous.is_some_and(|previous| wraps(previous, &text, width, margin));
        match (&item, lines.last_mut()) {
            (None, Some((_, _, spans))) if wrapped => {
                *spans = trim(std::mem::take(spans));
                spans.push(Span { text: " ".into(), style: Style::default() });
                spans.extend(row.skip(indent).0);
            }
            _ => {
                let (marker, body) = match &item {
                    Some((marker, length)) => (Some(marker.clone()), row.skip(indent + length)),
                    None => (None, row.skip(indent)),
                };
                lines.push((marker, indent, body.0));
            }
        }
        previous = Some(row);
    }
    let base = lines.iter().map(|(_, indent, _)| *indent).min().unwrap_or(0);
    let single = lines.len() == 1 && lines[0].0.is_none();
    let mut md = Vec::new();
    let mut plain = Vec::new();
    for (marker, indent, spans) in &lines {
        let words: String = spans.iter().map(|span| span.text.as_str()).collect::<String>().trim().to_string();
        let styled = |test: fn(&Style) -> bool| {
            spans.iter().filter(|span| !span.text.trim().is_empty()).all(|span| test(&span.style))
        };
        let text = if single && styled(|style| style.bold && style.italic && style.underline) {
            format!("# {}", escape(&words))
        } else {
            inline(spans, marker.is_none())
        };
        let pad = " ".repeat(indent.saturating_sub(base));
        md.push(match marker {
            Some(marker) => format!("{pad}{marker} {text}"),
            None => format!("{pad}{text}"),
        });
        plain.push(words);
    }
    (md.join("\n"), plain.join("\n"))
}

fn width_of(row: &Row) -> usize {
    measure(&row.text())
}

/// A list item's marker, as Markdown, and how many columns it takes on the screen. Claude
/// letters the items of a nested numbered list.
fn list_marker(text: &str) -> Option<(String, usize)> {
    if let Some(rest) = text.strip_prefix("- ").or_else(|| text.strip_prefix("• ")) {
        return (!rest.is_empty()).then(|| ("-".to_string(), 2));
    }
    let (head, rest) = text.split_once(". ")?;
    if rest.is_empty() || head.is_empty() {
        return None;
    }
    let number = if head.chars().all(|c| c.is_ascii_digit()) && head.len() <= 3 {
        head.parse().ok()?
    } else if head.len() == 1 && head.chars().all(|c| c.is_ascii_lowercase()) {
        u32::from(head.as_bytes()[0] - b'a') + 1
    } else {
        return None;
    };
    Some((format!("{number}."), head.len() + 2))
}

fn quote(rows: &[Row]) -> (String, String) {
    let mut md = Vec::new();
    let mut plain = Vec::new();
    for row in rows {
        let at = row.text().find('▎').map_or(0, |at| row.text()[..at].chars().count());
        let mut spans = row.skip(at + 2).0;
        // Claude sets a quote in italics; the italics are its, not the reply's.
        for span in &mut spans {
            span.style.italic = false;
        }
        let words: String = spans.iter().map(|span| span.text.as_str()).collect();
        md.push(format!("> {}", inline(&spans, false)));
        plain.push(words.trim().to_string());
    }
    (md.join("\n"), plain.join("\n"))
}

/// Code is set in colours prose never has, or wholly in the inline-code colour when it is
/// more than one line.
fn code(rows: &[Row], width: usize, margin: usize) -> bool {
    let coded = |row: &Row| {
        row.0.iter().filter(|span| !span.text.trim().is_empty()).all(|span| inline_code(&span.style))
            && !row.blank()
    };
    let lines = rows
        .windows(2)
        .filter(|pair| !wraps(&pair[0], &pair[1].text(), width, margin))
        .count()
        + 1;
    highlighted(rows) || (lines > 1 && rows.iter().all(coded))
}

/// Coloured by a syntax highlighter, which a fence that names its language gets.
fn highlighted(rows: &[Row]) -> bool {
    rows.iter().flat_map(|row| &row.0).any(|span| {
        !span.text.trim().is_empty() && matches!(span.style.fg, Colour::Index(n) if n != 12 && !Colour::Index(n).plain())
    })
}

fn inline_code(style: &Style) -> bool {
    !style.fg.plain() && !style.fg.link() && !style.fg.grey()
}

/// The language a block's code looks like, for its colours; Claude names it, the screen does not.
fn language(code: &str) -> &'static str {
    let has = |words: &[&str]| words.iter().any(|word| code.contains(word));
    let first = code.trim_start();
    if first.starts_with(['{', '[']) && code.contains("\":") {
        "json"
    } else if code.lines().all(|line| line.starts_with(['+', '-', '@', ' '])) && has(&["@@"]) {
        "diff"
    } else if has(&["fn ", "let mut ", "impl ", "pub fn", "::new("]) {
        "rust"
    } else if has(&["#include", "->", "void ", "int main("]) && !has(&["func ", "def "]) {
        "c"
    } else if has(&["func ", "guard let", "@State", "import SwiftUI", "var body"]) && !has(&["def "]) {
        "swift"
    } else if has(&["def ", "import ", "print(", "self.", "elif "]) {
        "python"
    } else if has(&["const ", "function ", "=> ", "console."]) {
        "javascript"
    } else if first.starts_with('$')
        || ["cd ", "git ", "cargo ", "npm ", "ls", "echo ", "grep ", "just ", "uv ", "xcodebuild", "ssh ", "curl "]
            .iter()
            .any(|command| first.starts_with(command))
    {
        "bash"
    } else {
        ""
    }
}

/// A table drawn with box characters as GFM, its columns aligned as their cells sit.
fn table(rows: &[Row]) -> (String, String) {
    let border = rows.iter().map(Row::text).find(|text| text.starts_with(['┌', '├'])).unwrap_or_default();
    let bars: Vec<usize> = border.chars().enumerate().filter(|(_, c)| "┌┬┐├┼┤".contains(*c)).map(|(at, _)| at).collect();
    let mut logical: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut pads: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut open = false;
    for row in rows {
        let text = row.text();
        if !text.starts_with('│') {
            open = false;
            continue;
        }
        let cells: Vec<Row> = bars.windows(2).map(|pair| cell(row, pair[0] + 1, pair[1])).collect();
        if open {
            if let Some(last) = logical.last_mut() {
                for (into, more) in last.iter_mut().zip(cells) {
                    let more = trim(more.0);
                    if !more.is_empty() {
                        into.push(Span { text: " ".into(), style: Style::default() });
                        into.extend(more);
                    }
                }
            }
        } else {
            pads.push(cells.iter().map(|cell| {
                let text: String = cell.0.iter().map(|span| span.text.as_str()).collect();
                (text.len() - text.trim_start().len(), text.len() - text.trim_end().len())
            }).collect());
            logical.push(cells.into_iter().map(|cell| trim(cell.0)).collect());
            open = true;
        }
    }
    let columns = bars.len().saturating_sub(1);
    let align: Vec<&str> = (0..columns)
        .map(|column| {
            let body: Vec<(usize, usize)> = pads.iter().skip(1).filter_map(|row| row.get(column).copied()).collect();
            if body.iter().all(|&(lead, _)| lead <= 1) {
                "---"
            } else if body.iter().all(|&(_, trail)| trail <= 1) {
                "---:"
            } else {
                ":---:"
            }
        })
        .collect();
    let line = |cells: &Vec<Vec<Span>>| {
        format!("| {} |", cells.iter().map(|cell| inline(cell, false).replace('|', "\\|")).collect::<Vec<_>>().join(" | "))
    };
    let mut md: Vec<String> = Vec::new();
    for (index, cells) in logical.iter().enumerate() {
        md.push(line(cells));
        if index == 0 {
            md.push(format!("|{}|", align.join("|")));
        }
    }
    let plain = logical
        .iter()
        .flatten()
        .map(|cell| cell.iter().map(|span| span.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    (md.join("\n"), plain)
}

/// The spans of a row between two columns.
fn cell(row: &Row, from: usize, to: usize) -> Row {
    let mut column = 0;
    let mut spans = Vec::new();
    for span in &row.0 {
        let text: String = span
            .text
            .chars()
            .filter(|_| {
                column += 1;
                (from + 1..=to).contains(&column)
            })
            .collect();
        if !text.is_empty() {
            spans.push(Span { text, style: span.style });
        }
    }
    Row(spans)
}

fn trim(mut spans: Vec<Span>) -> Vec<Span> {
    while spans.first().is_some_and(|span| span.text.trim().is_empty()) {
        spans.remove(0);
    }
    while spans.last().is_some_and(|span| span.text.trim().is_empty()) {
        spans.pop();
    }
    if let Some(first) = spans.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = spans.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    spans
}

#[derive(Clone, Copy, PartialEq)]
struct Marks {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: bool,
}

/// Spans as inline Markdown, each style's spaces moved outside its marks.
fn inline(spans: &[Span], line_start: bool) -> String {
    let mut out = String::new();
    for (marks, text) in runs(spans) {
        let body = text.trim();
        let lead = &text[..text.len() - text.trim_start().len()];
        let trail = &text[text.trim_end().len()..];
        out.push_str(lead);
        if body.is_empty() {
            continue;
        }
        let mut piece = if marks.code {
            let ticks = if body.contains('`') { "``" } else { "`" };
            format!("{ticks}{body}{ticks}")
        } else {
            escape(body)
        };
        if marks.link && !marks.code && !body.starts_with("http://") && !body.starts_with("https://") {
            piece = format!("[{piece}]({NO_LINK})");
        }
        let wrap = match (marks.bold, marks.italic) {
            (true, true) => "***",
            (true, false) => "**",
            (false, true) => "*",
            _ => "",
        };
        piece = format!("{wrap}{piece}{wrap}");
        if marks.strike {
            piece = format!("~~{piece}~~");
        }
        out.push_str(&piece);
        out.push_str(trail);
    }
    if line_start && out.starts_with(['#', '>', '+', '-']) {
        out.insert(0, '\\');
    }
    out
}

/// Spans grouped by the marks they take, spaces joining the run before them.
fn runs(spans: &[Span]) -> Vec<(Marks, String)> {
    let mut runs: Vec<(Marks, String)> = Vec::new();
    for span in spans {
        let style = span.style;
        let marks = Marks {
            bold: style.bold,
            italic: style.italic,
            strike: style.strike,
            code: inline_code(&style),
            link: style.fg.link(),
        };
        match runs.last_mut() {
            Some((last, text)) if *last == marks || span.text.trim().is_empty() => text.push_str(&span.text),
            _ => runs.push((marks, span.text.clone())),
        }
    }
    runs
}

/// Whether `previous` ends where Claude wrapped it rather than where the reply broke the
/// line. Claude wraps the Markdown before drawing it, so its marks count; a link's address
/// is not on the screen, so a row with a link counts as wrapped.
fn wraps(previous: &Row, next: &str, width: usize, margin: usize) -> bool {
    let mut marks = 0;
    for (run, text) in runs(&previous.0) {
        if text.trim().is_empty() {
            continue;
        }
        if run.link && !run.code && !text.trim().starts_with("http") {
            return true;
        }
        marks += 2 * usize::from(run.code) + 4 * usize::from(run.strike)
            + match (run.bold, run.italic) {
                (true, true) => 6,
                (true, false) => 4,
                (false, true) => 2,
                _ => 0,
            };
    }
    let next = next.split_whitespace().next().unwrap_or_default();
    margin + width_of(previous) + marks + 1 + measure(next) > width
}

/// Plain text that Markdown would otherwise read as marks.
fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    for (at, &c) in chars.iter().enumerate() {
        let word = |offset: isize| {
            at.checked_add_signed(offset).and_then(|at| chars.get(at)).is_some_and(|c| c.is_alphanumeric())
        };
        let special = match c {
            '\\' | '`' | '*' | '<' | '~' => true,
            '_' => !(word(-1) && word(1)),
            '[' => text[at..].contains("]("),
            _ => false,
        };
        if special {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

// MARK: Running commands

/// The command a tool block shows running, and the line above it when that describes it.
fn command(block: &Block, width: usize) -> Option<(String, Option<String>)> {
    let texts: Vec<String> = block.rows.iter().map(Row::text).collect();
    let start = texts.iter().position(|text| text.trim_start().starts_with("⎿  $ "))?;
    let mut command = texts[start].trim_start()["⎿  $ ".len()..].to_string();
    let mut previous = texts[start].clone();
    for text in &texts[start + 1..] {
        let rest = text.trim_start();
        if text.len() - rest.len() < 5 || rest.starts_with(['(', '…', '⎿']) {
            break;
        }
        let next = rest.split_whitespace().next().unwrap_or_default();
        command.push_str(if measure(&previous) >= width { "" } else if measure(&previous) + 1 + measure(next) > width { " " } else { "\n" });
        command.push_str(rest);
        previous = text.clone();
    }
    if let Some(open) = command.rfind(" (") {
        let tail = &command[open + 2..];
        if tail.ends_with(')') && tail.trim_end_matches(')').split([' ', '·']).next().is_some_and(duration) {
            command.truncate(open);
        }
    }
    let header = texts[0].trim_start_matches(MARKERS).trim();
    let header = header.split(" · ").next().unwrap_or(header).trim_end_matches('…').trim();
    let description = (!summary(header) && !header.is_empty()).then(|| header.to_string());
    Some((command.trim().to_string(), description))
}


/// "3s", "1m", "2h".
fn duration(word: &str) -> bool {
    word.len() > 1 && word.ends_with(['s', 'm', 'h']) && word[..word.len() - 1].chars().all(|c| c.is_ascii_digit())
}

// MARK: Following the screen

/// Letters and digits of what a reader sees: Markdown's marks, link addresses, fences and
/// list markers dropped, so the screen's text and the Transcript's compare.
pub fn words(markdown: &str) -> String {
    let mut out = String::new();
    for line in markdown.lines() {
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
pub struct Live {
    items: Vec<Item>,
    status: String,
    corpus: Corpus,
    next: u64,
    /// When Claude stopped working, if it has.
    stopped: Option<Instant>,
    /// The last view used, so a changed screen is read twice.
    last: Option<View>,
}

impl Live {
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

    /// The `live` line for what is shown now.
    pub fn line(&self, now: Instant) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .filter(|item| shown(item, now))
            .map(|item| {
                let mut value = serde_json::to_value(&item.entry).unwrap_or_default();
                value["id"] = item.id.clone().into();
                value
            })
            .collect();
        json!({"t": "live", "items": items, "status": self.status})
    }
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
    let at = Value::Null;
    match kind {
        Kind::Tool => {
            let block = Block { kind, rows: rows.to_vec(), cut: false };
            let (command, description) = command(&block, width).unwrap_or_default();
            let mut input = json!({"command": command});
            if let Some(description) = description {
                input["description"] = description.into();
            }
            Entry::tool(String::new(), &at, "Bash", &input)
        }
        _ => {
            let (markdown, _) = markdown(rows, width);
            Entry::text_like(String::new(), "text", &at, markdown)
        }
    }
}

fn entry_words(entry: &Entry) -> String {
    words(entry.command.as_deref().or(entry.text.as_deref()).unwrap_or_default())
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
