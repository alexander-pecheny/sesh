use std::collections::HashMap;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sesh_transcript::agent::{transcript_path, Agent};
use sesh_transcript::follower::{entry_line, Event, Follower, Mux};
use sesh_transcript::{act, listen};
use sesh_transcript::index::Index;
use sesh_transcript::place::place;
use sesh_transcript::vault::{self, Vault};
use sesh_transcript::{replay, serve, version, Transcript, PROTOCOL};

/// Short enough that the chat keeps up with what Claude's screen shows.
const POLL: Duration = Duration::from_millis(100);
const VAULT_POLL: Duration = Duration::from_millis(500);
const AGENT_GONE_AFTER: Duration = Duration::from_secs(2);
/// A quiet stream still writes this often, since only a failed write shows that sshd's end
/// of the socket has gone; polling the socket does not.
const PING: Duration = Duration::from_secs(30);
const DEFAULT_LAST: usize = 50;
const USAGE: &str = "usage: sesh-transcript --version | follow --protocol | follow <pane> [--since CURSOR] [--last N]
       | serve [--foreground] [--record DIR --pane PANE] | attach [--sessions] [--watch KEY[:SEQ]]...
       | page KEY --before ORD [--limit N] | replay FILE [--lines]
       | send KEY ID TEXT | unqueue KEY ID | hand KEY | ask REQUEST | ask --file FILE
       | keys KEY NAME... | answer KEY --json ANSWERS | permit KEY allow|deny | stop KEY
       | history <pane> --before ID [--last N] | entry <pane> ID | background <pane>...
       | vault init DIR | vault pull|follow DIR [--since SEQ] | vault push DIR FILE [--batch ID]
       | vault size DIR SESSION FILE | vault append DIR SESSION FILE --offset N BYTES_FILE
       | vault copy DIR SESSION --from PATH | vault search DIR QUERY [--limit N]
       | transcripts search QUERY [--limit N] | transcripts place FILE --agent AGENT [--config DIR] [--fork]
       | search WORDS... [--vault NAME] [--limit N]
follow, history and entry take --file PATH --agent AGENT in place of <pane>.";

/// A failed command says why on stderr and exits 1; usage errors exit 2.
type Exit = Result<i32, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let exit = match args.first().map(String::as_str) {
        Some("--version") => {
            println!("sesh-transcript {}", version());
            Ok(0)
        }
        Some("follow") => follow(rest),
        Some("serve") => serve_command(rest),
        Some("attach") => attach(rest),
        Some("page") => page(rest),
        Some("replay") => match rest {
            [file] => replay::print(std::path::Path::new(file), false),
            [file, flag] if flag == "--lines" => replay::print(std::path::Path::new(file), true),
            _ => usage(),
        },
        Some("history") => history(rest),
        Some("background") => background(rest),
        Some("entry") => entry(rest),
        Some(op) if act::OPS.contains(&op) => match rest.split_first().and_then(|(session, args)| act::request(op, session, args)) {
            Some(request) => serve::ask(&request),
            None => usage(),
        },
        Some("vault") => vault(rest),
        Some("ask") => ask(rest),
        Some("transcripts") => transcripts(rest),
        Some("search") => search(rest),
        _ => usage(),
    };
    std::process::exit(exit.unwrap_or_else(|message| {
        eprintln!("{message}");
        1
    }));
}

fn serve_command(args: &[String]) -> Exit {
    let foreground = args.iter().any(|arg| arg == "--foreground");
    let args: Vec<String> = args.iter().filter(|arg| *arg != "--foreground").cloned().collect();
    let Some((rest, options)) = parse_args(&args, &["record", "pane"]) else {
        return usage();
    };
    let record = match (options.get("record"), options.get("pane")) {
        (Some(dir), Some(pane)) => Some((std::path::PathBuf::from(dir), pane.to_string())),
        (None, None) => None,
        _ => return usage(),
    };
    if !rest.is_empty() {
        return usage();
    }
    serve::serve(foreground, record)
}

/// `--sessions` asks for every session's summary; each `--watch KEY[:SEQ]` for a session's
/// items, all of them from SEQ or its last ones when none is given.
fn attach(args: &[String]) -> Exit {
    let mut requests = vec![json!({"op": "hello", "protocol": serve::PROTOCOL, "sessions": args.iter().any(|arg| arg == "--sessions")})];
    let mut args = args.iter().filter(|arg| *arg != "--sessions");
    while let Some(arg) = args.next() {
        let (Some("--watch"), Some(target)) = (Some(arg.as_str()), args.next()) else { return usage() };
        let (key, since) = match target.rsplit_once(':').and_then(|(key, seq)| Some((key, seq.parse::<i64>().ok()?))) {
            Some((key, seq)) if key.contains(':') => (key, seq),
            _ => (target.as_str(), 0),
        };
        requests.push(json!({"op": "watch", "session": key, "since": since}));
    }
    serve::attach(&requests)
}

fn page(args: &[String]) -> Exit {
    let Some((rest, options)) = parse_args(args, &["before", "limit"]) else { return usage() };
    let ([session], Some(before)) = (rest.as_slice(), options.get("before").and_then(|ord| ord.parse::<i64>().ok())) else { return usage() };
    let limit: u64 = options.get("limit").and_then(|limit| limit.parse().ok()).unwrap_or(50);
    serve::ask(&json!({"op": "page", "session": session, "before": before, "limit": limit}))
}

/// Any request as JSON, or read from a file that is then removed, for one too long for a command
/// line, such as the Transcripts a Vault takes (ADR 0020).
fn ask(args: &[String]) -> Exit {
    let text = match args {
        [flag, file] if flag == "--file" => {
            let text = std::fs::read_to_string(file).map_err(|err| format!("{file}: {err}"))?;
            std::fs::remove_file(file).map_err(|err| format!("{file}: {err}"))?;
            text
        }
        [json] => json.clone(),
        _ => return usage(),
    };
    let request: Value = serde_json::from_str(&text).map_err(|err| format!("not a request: {err}"))?;
    serve::ask(&request)
}

fn usage() -> Exit {
    eprintln!("{USAGE}");
    Ok(2)
}

type Options<'a> = HashMap<&'a str, &'a str>;

/// The plain arguments and the `--name value` options among `names`.
fn parse_args<'a>(args: &'a [String], names: &[&str]) -> Option<(Vec<&'a str>, Options<'a>)> {
    let (mut plain, mut options) = (Vec::new(), HashMap::new());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.strip_prefix("--"), args.as_slice().first()) {
            (Some(name), Some(value)) if names.contains(&name) => {
                options.insert(name, value.as_str());
                args.next();
            }
            (None, _) => plain.push(arg.as_str()),
            _ => return None,
        }
    }
    Some((plain, options))
}

enum Target<'a> {
    Pane(&'a str),
    File(Value),
}

impl Target<'_> {
    /// The pane as sesh-mux reports it; a file's is made up to name its Agent and path.
    fn pane(&self) -> Result<Value, String> {
        match self {
            Self::Pane(pane_id) => pane_info(pane_id),
            Self::File(pane) => Ok(pane.clone()),
        }
    }
}

impl std::fmt::Display for Target<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Pane(pane_id) => write!(f, "pane {pane_id}"),
            Self::File(pane) => write!(f, "{}", pane["agent_session"]["path"]),
        }
    }
}

fn parse_target<'a>(
    args: &'a [String],
    names: &[&str],
) -> Option<(Target<'a>, Vec<&'a str>, Options<'a>)> {
    let names = [names, &["file", "agent"]].concat();
    let (mut plain, options) = parse_args(args, &names)?;
    let target = match (options.get("file"), options.get("agent")) {
        (Some(path), Some(agent)) => {
            Target::File(json!({"agent": agent, "agent_session": {"agent": agent, "path": path}}))
        }
        (None, None) if !plain.is_empty() => Target::Pane(plain.remove(0)),
        _ => return None,
    };
    Some((target, plain, options))
}

fn parse_last(options: &HashMap<&str, &str>) -> Option<usize> {
    options
        .get("last")
        .map_or(Some(DEFAULT_LAST), |value| value.parse().ok())
}

// MARK: sesh-mux
fn pane_info(pane_id: &str) -> Result<Value, String> {
    let result = listen::call("pane.get", json!({"pane_id": pane_id})).map_err(|err| listen::refusal(&err))?;
    Ok(result["pane"].clone())
}

/// The pane and its Agent, or why it has none Conversations support.
fn agent_pane(target: &Target) -> Result<(Value, Agent), String> {
    let pane = target.pane()?;
    let agent = Agent::of(&pane).ok_or_else(|| format!("{target} does not run claude, codex or pi"))?;
    Ok((pane, agent))
}

fn pane_transcript(target: &Target) -> Result<Transcript, String> {
    let (pane, agent) = agent_pane(target)?;
    let path = transcript_path(&pane)
        .ok_or_else(|| format!("{} in {target} has reported no transcript", agent.name()))?;
    Ok(Transcript::new(agent, path))
}

// MARK: follow

fn follow(args: &[String]) -> Exit {
    if args.first().is_some_and(|arg| arg == "--protocol") {
        println!("{PROTOCOL}");
        return Ok(0);
    }
    let Some((target, rest, options)) = parse_target(args, &["since", "last"]) else {
        return usage();
    };
    let (Some(last), true) = (parse_last(&options), rest.is_empty()) else {
        return usage();
    };
    let (pane, agent) = agent_pane(&target)?;
    let mut follower = Follower::new(agent, last, Box::new(Mux));
    follower.start(&pane, options.get("since").copied())?;
    print_events(&follower.drain())?;
    let mut agent_seen = Instant::now();
    let mut pinged = Instant::now();
    loop {
        std::thread::sleep(POLL);
        if sesh_transcript::poll::hung_up(1) {
            return Ok(0);
        }
        if pinged.elapsed() > PING {
            follower.emit(Event::Ping)?;
            pinged = Instant::now();
        }
        match target.pane() {
            Ok(pane) if pane["agent"].is_string() => {
                agent_seen = Instant::now();
                follower.tick(&pane, Instant::now())?;
            }
            _ if agent_seen.elapsed() > AGENT_GONE_AFTER => return Ok(0),
            _ => {}
        }
        print_events(&follower.drain())?;
    }
}

// MARK: entry and history

fn entry(args: &[String]) -> Exit {
    let Some((target, rest, _)) = parse_target(args, &[]) else {
        return usage();
    };
    let [id] = rest[..] else {
        return usage();
    };
    let mut transcript = pane_transcript(&target)?;
    if let Some(end) = transcript.locate(id).map_err(|err| err.to_string())? {
        transcript
            .read_tail(Some(end), |entries| {
                entries.iter().any(|entry| entry.id == *id)
            })
            .map_err(|err| err.to_string())?;
    }
    let entry = transcript
        .find(id)
        .ok_or_else(|| format!("no entry {id} in {}", transcript.path.to_string_lossy()))?;
    let mut line = serde_json::to_value(entry).map_err(|err| err.to_string())?;
    line["t"] = "entry".into();
    println!("{line}");
    Ok(0)
}

fn history(args: &[String]) -> Exit {
    let Some((target, rest, options)) = parse_target(args, &["before", "last"]) else {
        return usage();
    };
    let (Some(before), Some(last), true) =
        (options.get("before"), parse_last(&options), rest.is_empty())
    else {
        return usage();
    };
    let mut transcript = pane_transcript(&target)?;
    let (entries, more) = transcript
        .history(before, last)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("no entry {before} in {}", transcript.path.to_string_lossy()))?;
    let mut out = std::io::stdout().lock();
    for entry in &entries {
        writeln!(out, "{}", entry_line(entry)).map_err(|err| err.to_string())?;
    }
    writeln!(out, "{}", json!({"t": "history", "more": more})).map_err(|err| err.to_string())?;
    Ok(0)
}

/// What each pane's Agent left running in the background, one JSON line per pane, read from
/// the end of its Transcript, so Projects can mark an idle Agent that is not yet done.
fn background(panes: &[String]) -> Exit {
    if panes.is_empty() {
        return usage();
    }
    let mut out = std::io::stdout().lock();
    for pane in panes {
        let (tasks, last, turn_over): (Vec<Value>, Option<String>, bool) = match pane_transcript(&Target::Pane(pane)) {
            Ok(mut transcript) => {
                transcript
                    .read_tail(None, |entries| entries.len() >= DEFAULT_LAST)
                    .map_err(|err| err.to_string())?;
                transcript.scan_background().map_err(|err| err.to_string())?;
                let tasks = transcript
                    .background()
                    .iter()
                    .map(|work| json!({"call": work.call, "label": work.label, "agent": work.agent}))
                    .collect();
                // The last message's time, which orders Tasks by their latest change.
                let last = transcript.entries.last().map(|entry| entry.at.clone());
                (tasks, last, transcript.turn_over())
            }
            Err(_) => (Vec::new(), None, false),
        };
        writeln!(out, "{}", json!({"pane": pane, "tasks": tasks, "last": last, "turn_over": turn_over}))
            .map_err(|err| err.to_string())?;
    }
    Ok(0)
}

// MARK: search

/// Searches this Host's Vaults for the user, as `vault search` does for the app.
fn search(args: &[String]) -> Exit {
    let Some((words, options)) = parse_args(args, &["vault", "limit"]) else {
        return usage();
    };
    let Ok(limit) = options.get("limit").map(|limit| limit.parse::<usize>()).transpose() else {
        return usage();
    };
    if words.is_empty() {
        return usage();
    }
    let root = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".sesh/vaults");
    let names: Vec<String> = match options.get("vault") {
        Some(name) => vec![name.to_string()],
        None => {
            let folders = std::fs::read_dir(&root).map_err(|err| format!("{}: {err}", root.display()))?;
            let mut names: Vec<String> = folders
                .flatten()
                .filter(|folder| folder.path().join("vault.db").exists())
                .map(|folder| folder.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    };
    let bold = std::io::stdout().is_terminal();
    let mut out = String::new();
    for name in &names {
        let hits = Vault::open(root.join(name), false)?.search(&words.join(" "), limit)?;
        if names.len() > 1 && !hits.is_empty() {
            out += &format!("# {name}\n\n");
        }
        out += &describe(&hits, bold);
    }
    print!("{out}");
    Ok(0)
}

/// One entry per Transcript, with every line of it that matched, and one per record.
fn describe(hits: &[Value], bold: bool) -> String {
    let copy = |hit: &Value| (hit["kind"] == "transcript").then(|| (hit["session"].clone(), hit["file"].clone()));
    let mut groups: Vec<Vec<&Value>> = Vec::new();
    for hit in hits {
        match copy(hit).and_then(|key| groups.iter_mut().find(|group| copy(group[0]).as_ref() == Some(&key))) {
            Some(group) => group.push(hit),
            None => groups.push(vec![hit]),
        }
    }
    groups.iter().map(|group| hit_entry(group, bold)).collect()
}

fn hit_entry(hits: &[&Value], bold: bool) -> String {
    let hit = hits[0];
    let text = |key: &str| hit[key].as_str().filter(|value| !value.is_empty());
    let title = text("title").unwrap_or("Untitled");
    let machine = text("machine").unwrap_or("the Mac");
    let agent = Agent::named(text("agent").unwrap_or_default());
    let mut head = match hit["kind"].as_str() {
        Some("transcript") => format!("{title} ({}) on {machine}", agent.map_or("agent", Agent::name)),
        kind => format!("{title} ({})", kind.unwrap_or_default()),
    };
    if let Some(date) = text("date") {
        head += &format!(", {date}");
    }
    let mut out = head + "\n";
    for hit in hits {
        out += &format!("  {}\n", marked(hit["snippet"].as_str().unwrap_or_default(), bold));
    }
    let id = text("file").unwrap_or_default().trim_end_matches(".jsonl");
    let resume = match agent {
        Some(Agent::Claude) => Some(format!("claude --resume {id}")),
        Some(Agent::Codex) => Some(format!("codex resume {}", &id[id.len().saturating_sub(36)..])),
        _ => None,
    };
    if let (Some(resume), Some(path)) = (resume, text("path")) {
        out += &format!("  on {machine}: cd '{path}' && {resume}\n");
    }
    out + "\n"
}

/// The snippet on one line, its matches in bold on a terminal.
fn marked(text: &str, bold: bool) -> String {
    let text = text.replace('\n', " ");
    if !bold {
        return text;
    }
    text.split("**")
        .enumerate()
        .map(|(index, part)| match index {
            0 => part.to_string(),
            _ if index % 2 == 1 => format!("\x1b[1m{part}"),
            _ => format!("\x1b[22m{part}"),
        })
        .collect()
}

// MARK: vault

fn vault(args: &[String]) -> Exit {
    let Some((plain, options)) = parse_args(args, &["since", "offset", "from", "limit", "batch"]) else {
        return usage();
    };
    let number = |name| {
        options
            .get(name)
            .map(|value| value.parse::<u64>())
            .transpose()
    };
    let (Ok(since), Ok(offset), Ok(limit)) = (number("since"), number("offset"), number("limit"))
    else {
        return usage();
    };
    let since = since.unwrap_or_default() as i64;
    let lines = match (plain.as_slice(), offset, options.get("from")) {
        (&["init", dir], None, None) => {
            let vault = Vault::open(dir, true)?;
            vec![vault.head_line(vault.head()?)]
        }
        (&["pull", dir], None, None) => return vault_follow(dir, since, false),
        (&["follow", dir], None, None) => return vault_follow(dir, since, true),
        (&["push", dir, file], None, None) => vault_push(dir, file, options.get("batch").copied())?,
        (&["size", dir, session, file], None, None) => {
            let vault = Vault::open(dir, false)?;
            vec![json!({"size": vault.size(session, file)?})]
        }
        (&["append", dir, session, file, bytes], Some(offset), None) => {
            let vault = Vault::open(dir, false)?;
            let bytes = std::fs::read(bytes).map_err(|err| format!("{bytes}: {err}"))?;
            vec![json!({"size": vault.append(session, file, offset, &bytes)?})]
        }
        (&["copy", dir, session], None, Some(from)) => {
            let vault = Vault::open(dir, false)?;
            vec![json!({"size": vault.copy(session, Path::new(from))?})]
        }
        (&["search", dir, query], None, None) => {
            let vault = Vault::open(dir, false)?;
            vault.search(query, limit.map(|limit| limit as usize))?
        }
        _ => return usage(),
    };
    print_lines(&lines)
}

fn transcripts(args: &[String]) -> Exit {
    let fork = args.iter().any(|arg| arg == "--fork");
    let args: Vec<String> = args.iter().filter(|arg| *arg != "--fork").cloned().collect();
    let Some((plain, options)) = parse_args(&args, &["limit", "agent", "config"]) else {
        return usage();
    };
    let Ok(limit) = options.get("limit").map(|value| value.parse::<usize>()).transpose() else {
        return usage();
    };
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return Err("HOME is not set".into());
    };
    match (plain.as_slice(), options.get("agent").and_then(|name| Agent::named(name))) {
        (["search", query], None) => {
            let mut index = Index::open(home)?;
            index.refresh()?;
            print_lines(&index.search(query, limit)?)
        }
        (["place", file], Some(agent)) => {
            let config = options.get("config").map(Path::new);
            print_lines(&[place(agent, Path::new(file), &home, config, fork)?.line()])
        }
        _ => usage(),
    }
}

fn print_events(events: &[Event]) -> Exit {
    print_lines(&events.iter().map(Event::line).collect::<Vec<_>>())
}

fn print_lines(lines: &[Value]) -> Exit {
    let mut out = std::io::stdout().lock();
    lines
        .iter()
        .try_for_each(|line| writeln!(out, "{line}"))
        .and_then(|()| out.flush())
        .map_err(|err| err.to_string())?;
    Ok(0)
}

fn vault_follow(dir: &str, mut since: i64, follow: bool) -> Exit {
    let vault = Vault::open(dir, false)?;
    let mut version = None;
    let mut pinged = Instant::now();
    loop {
        if follow && pinged.elapsed() > PING {
            print_lines(&[json!({"t": "ping"})])?;
            pinged = Instant::now();
        }
        let now = Some(vault.data_version()?);
        if version != now {
            let (records, head) = vault.pull(since)?;
            if version.is_none() || !records.is_empty() {
                let mut lines: Vec<Value> = records.iter().map(vault::Record::line).collect();
                lines.push(vault.head_line(head));
                print_lines(&lines)?;
            }
            (version, since) = (now, head);
        }
        if !follow {
            return Ok(0);
        }
        std::thread::sleep(VAULT_POLL);
        if sesh_transcript::poll::hung_up(1) {
            return Ok(0);
        }
    }
}

fn vault_push(dir: &str, file: &str, batch: Option<&str>) -> Result<Vec<Value>, String> {
    let text = std::fs::read_to_string(file).map_err(|err| format!("{file}: {err}"))?;
    let changes = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<Vec<vault::Change>, _>>()
        .map_err(|err| format!("{file}: {err}"))?;
    let mut vault = Vault::open(dir, false)?;
    let (records, head) = vault.push(batch, changes)?;
    std::fs::remove_file(file).map_err(|err| format!("{file}: {err}"))?;
    let mut lines: Vec<Value> = records.iter().map(vault::Record::line).collect();
    lines.push(vault.head_line(head));
    Ok(lines)
}
