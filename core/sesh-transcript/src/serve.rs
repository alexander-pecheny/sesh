//! The follower (ADR 0011): one process per machine that follows every Agent herdr runs
//! there into the Session log, and serves devices over a local socket (ADR 0012).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead as _, BufReader, ErrorKind, Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::follower::{transcript_path, Follower, Herdr, Source};
use crate::log::{Log, Writer};
use crate::{Transcript, AGENTS};

pub const PROTOCOL: u64 = 3;
const TICK: Duration = Duration::from_millis(100);
/// herdr's panes are listed less often than the screens of working Agents are read.
const LIST_EVERY: u32 = 2;
const GONE_AFTER: Duration = Duration::from_secs(2);
const IDLE_EXIT: Duration = Duration::from_secs(6 * 60 * 60);
const PING: Duration = Duration::from_secs(30);
/// Entries a session starts with when the follower first meets it.
const FIRST: usize = 200;
/// Items a device gets when it opens a session.
const OPENING: usize = 80;
const START_WAIT: Duration = Duration::from_secs(3);

type Result<T> = std::result::Result<T, String>;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".sesh")
}

fn socket() -> PathBuf {
    home().join("follower.sock")
}

// MARK: the machine

/// A shared Source, so every follower and the pane list go through one recorder.
#[derive(Clone)]
pub struct Shared(pub Rc<RefCell<dyn Source>>);

impl Source for Shared {
    fn panes(&mut self) -> Result<Vec<Value>> {
        self.0.borrow_mut().panes()
    }

    fn read(&mut self, pane_id: &str, ansi: bool) -> Result<String> {
        self.0.borrow_mut().read(pane_id, ansi)
    }
}

struct Followed {
    follower: Follower,
    writer: Writer,
    seen: Instant,
}

/// What the follower does each tick, apart from sockets, so a recording can drive it too.
pub struct Machine {
    pub log: Log,
    source: Shared,
    sessions: HashMap<String, Followed>,
}

impl Machine {
    pub fn new(log: Log, source: Shared) -> Self {
        Machine { log, source, sessions: HashMap::new() }
    }

    pub fn source(&self) -> Shared {
        self.source.clone()
    }

    pub fn following(&self) -> usize {
        self.sessions.len()
    }

    /// One look at every pane: new Agents are met, known ones followed, gone ones ended.
    pub fn tick(&mut self, panes: &[Value], now: Instant) -> Result<()> {
        for pane in panes {
            let (Some(key), Some(agent)) = (pane["pane_id"].as_str(), pane["agent"].as_str()) else { continue };
            if !AGENTS.contains(&agent) {
                continue;
            }
            if let Some(followed) = self.sessions.get_mut(key) {
                followed.seen = now;
                followed.follower.tick(pane, now)?;
                let lines = followed.follower.drain();
                followed.writer.apply(&self.log, &lines)?;
                continue;
            }
            let writer = Writer::new(key, &self.log)?;
            let mut follower = Follower::new(agent.to_string(), FIRST, Box::new(self.source.clone()));
            let since = writer.cursor().map(str::to_string);
            follower.start(pane, since.as_deref())?;
            let mut followed = Followed { follower, writer, seen: now };
            let lines = followed.follower.drain();
            followed.writer.apply(&self.log, &lines)?;
            self.sessions.insert(key.to_string(), followed);
        }
        let gone: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, followed)| now.duration_since(followed.seen) > GONE_AFTER)
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            let mut followed = self.sessions.remove(&key).expect("listed above");
            followed.writer.apply(&self.log, &[json!({"t": "state", "state": "ended"}), json!({"t": "live", "items": [], "status": ""})])?;
        }
        Ok(())
    }

    /// Items before `ord`, read from the Transcript when the log holds too few.
    pub fn page(&mut self, session: &str, ord: i64, limit: usize) -> Result<(Vec<Value>, bool)> {
        let page = self.log.page(session, ord, limit)?;
        if page.len() >= limit {
            return Ok((page, true));
        }
        let Some((agent, path)) = self.sessions.get(session).and_then(|followed| followed.follower.transcript()) else {
            return Ok((page, false));
        };
        let first = self.log.page(session, i64::MAX, usize::MAX)?;
        let Some(oldest) = first.iter().find_map(|item| item["entry"]["id"].as_str().filter(|id| !id.starts_with("live."))) else {
            return Ok((page, false));
        };
        let mut transcript = Transcript::new(&agent, path).expect("agent is supported");
        let Some((older, more)) = transcript.history(oldest, limit).map_err(|err| err.to_string())? else {
            return Ok((page, false));
        };
        let start = self.log.first_ord(session)?.unwrap_or(0);
        let count = older.len() as i64;
        for (index, entry) in older.iter().enumerate() {
            let line = crate::follower::entry_line(entry);
            let mut body = line.clone();
            body.as_object_mut().map(|body| body.remove("t"));
            self.log.put_item(session, &entry.id, Some(start - count + index as i64), true, Some(&entry.id), &body)?;
        }
        Ok((self.log.page(session, ord, limit)?, more))
    }
}

// MARK: serve

/// Starts the follower unless one answers, and returns once it does.
pub fn serve(foreground: bool, record: Option<(PathBuf, String)>) -> Result<i32> {
    if UnixStream::connect(socket()).is_ok() {
        return Ok(0);
    }
    if foreground {
        return run(record);
    }
    std::fs::create_dir_all(home()).map_err(|err| err.to_string())?;
    let log = OpenOptions::new().create(true).append(true).open(home().join("follower.log")).map_err(|err| err.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|err| err.to_string())?);
    command.args(["serve", "--foreground"]);
    if let Some((dir, pane)) = &record {
        command.arg("--record").arg(dir).arg("--pane").arg(pane);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|err| format!("could not start the follower: {err}"))?;
    let started = Instant::now();
    while started.elapsed() < START_WAIT {
        if UnixStream::connect(socket()).is_ok() {
            return Ok(0);
        }
        std::thread::sleep(TICK);
    }
    Err("the follower did not start; see ~/.sesh/follower.log".into())
}

fn run(record: Option<(PathBuf, String)>) -> Result<i32> {
    std::fs::create_dir_all(home()).map_err(|err| err.to_string())?;
    let lock = File::create(home().join("follower.lock")).map_err(|err| err.to_string())?;
    if lock.try_lock().is_err() {
        return Ok(0);
    }
    let path = socket();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    listener.set_nonblocking(true).map_err(|err| err.to_string())?;
    let source: Rc<RefCell<dyn Source>> = match record {
        Some((dir, pane)) => Rc::new(RefCell::new(Recorder::new(&dir, pane)?)),
        None => Rc::new(RefCell::new(Herdr)),
    };
    let mut machine = Machine::new(Log::open(&home().join("sessions.db"))?, Shared(source.clone()));
    let mut clients: Vec<Client> = Vec::new();
    let mut busy = Instant::now();
    let mut ticks = 0u32;
    let mut panes: Vec<Value> = Vec::new();
    loop {
        let started = Instant::now();
        while let Ok((stream, _)) = listener.accept() {
            if stream.set_nonblocking(true).is_ok() {
                clients.push(Client::new(stream));
            }
        }
        if ticks % LIST_EVERY == 0 {
            panes = source.borrow_mut().panes().unwrap_or_default();
        }
        ticks += 1;
        if let Err(err) = machine.tick(&panes, started) {
            eprintln!("{err}");
        }
        if let Some(recorder) = source.borrow_mut().recorder() {
            recorder.transcripts(&panes);
        }
        for client in &mut clients {
            client.serve(&mut machine);
        }
        clients.retain(|client| client.open);
        if machine.following() > 0 || !clients.is_empty() {
            busy = Instant::now();
        } else if busy.elapsed() > IDLE_EXIT {
            let _ = std::fs::remove_file(&path);
            return Ok(0);
        }
        if let Some(rest) = TICK.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

// MARK: devices

struct Client {
    stream: UnixStream,
    read: Vec<u8>,
    write: Vec<u8>,
    open: bool,
    /// The last summary this device has, and whether it wants every session's or only those
    /// it watches; and the last item it has of each watched session.
    summaries: i64,
    all: bool,
    watched: HashMap<String, i64>,
    pinged: Instant,
    done_asking: bool,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        Client { stream, read: Vec::new(), write: Vec::new(), open: true, summaries: 0, all: false, watched: HashMap::new(), pinged: Instant::now(), done_asking: false }
    }

    fn send(&mut self, line: &Value) {
        self.write.extend_from_slice(line.to_string().as_bytes());
        self.write.push(b'\n');
    }

    fn serve(&mut self, machine: &mut Machine) {
        let mut buffer = [0u8; 4096];
        while !self.done_asking {
            match self.stream.read(&mut buffer) {
                // A device that has said all it wants still listens.
                Ok(0) => self.done_asking = true,
                Ok(count) => self.read.extend_from_slice(&buffer[..count]),
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(_) => return self.open = false,
            }
        }
        while let Some(end) = self.read.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.read.drain(..=end).collect();
            if let Ok(request) = serde_json::from_slice::<Value>(&line) {
                if let Err(err) = self.request(machine, &request) {
                    self.send(&json!({"t": "error", "op": request["op"], "message": err}));
                }
            }
        }
        if let Err(err) = self.push(machine) {
            self.send(&json!({"t": "error", "message": err}));
        }
        if self.pinged.elapsed() > PING {
            self.send(&json!({"t": "ping"}));
            self.pinged = Instant::now();
        }
        while !self.write.is_empty() {
            match self.stream.write(&self.write) {
                Ok(0) => return self.open = false,
                Ok(count) => drop(self.write.drain(..count)),
                Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                Err(_) => return self.open = false,
            }
        }
    }

    fn request(&mut self, machine: &mut Machine, request: &Value) -> Result<()> {
        let session = request["session"].as_str().unwrap_or_default().to_string();
        match request["op"].as_str().unwrap_or_default() {
            "hello" => {
                self.send(&json!({"t": "hello", "protocol": PROTOCOL, "version": crate::VERSION}));
                self.all = request["sessions"] == true;
            }
            "watch" => {
                if let Some(mut summary) = machine.log.session(&session)? {
                    summary["t"] = "session".into();
                    summary["session"] = session.clone().into();
                    self.send(&summary);
                }
                let since = request["since"].as_i64().unwrap_or(0);
                if since > 0 {
                    self.watched.insert(session, since);
                } else {
                    let head = machine.log.head()?;
                    for line in machine.log.last(&session, OPENING)? {
                        self.send(&line);
                    }
                    self.send(&json!({"t": "opened", "session": session, "seq": head}));
                    self.watched.insert(session, head);
                }
            }
            "unwatch" => drop(self.watched.remove(&session)),
            "page" => {
                let before = request["before"].as_i64().unwrap_or(i64::MAX);
                let limit = request["limit"].as_u64().unwrap_or(OPENING as u64) as usize;
                let (items, more) = machine.page(&session, before, limit)?;
                for line in &items {
                    self.send(line);
                }
                self.send(&json!({"t": "page_done", "session": session, "before": before, "more": more}));
            }
            other => return Err(format!("unknown op {other:?}")),
        }
        Ok(())
    }

    fn push(&mut self, machine: &Machine) -> Result<()> {
        let lines = machine.log.sessions_since(self.summaries)?;
        if let Some(last) = lines.last().and_then(|line| line["seq"].as_i64()) {
            self.summaries = last;
        }
        for line in &lines {
            let key = line["session"].as_str().unwrap_or_default();
            if self.all || self.watched.contains_key(key) {
                self.send(line);
            }
        }
        let watched: Vec<(String, i64)> = self.watched.iter().map(|(key, seq)| (key.clone(), *seq)).collect();
        for (session, seen) in watched {
            let lines = machine.log.items_since(&session, seen)?;
            if let Some(last) = lines.last().and_then(|line| line["seq"].as_i64()) {
                self.watched.insert(session, last);
            }
            for line in &lines {
                self.send(line);
            }
        }
        Ok(())
    }
}

// MARK: attach

/// Joins stdin and stdout to the follower's socket, starting the follower first if need be.
/// `requests` go first, so a device that cannot write to a running command's stdin can still
/// say what it wants: every session's summary, or sessions to watch from a sequence number.
pub fn attach(requests: &[Value]) -> Result<i32> {
    let stream = connect()?;
    let mut reader = stream.try_clone().map_err(|err| err.to_string())?;
    let mut writer = stream;
    for request in requests {
        writeln!(writer, "{request}").map_err(|err| err.to_string())?;
    }
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut writer);
        let _ = writer.shutdown(std::net::Shutdown::Write);
    });
    let mut out = std::io::stdout().lock();
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return Ok(0),
            Ok(count) => {
                if out.write_all(&buffer[..count]).and_then(|()| out.flush()).is_err() {
                    return Ok(0);
                }
            }
        }
    }
}

/// `page KEY --before ORD --limit N`: one page of a session's earlier items, for a device
/// that reads it with a command of its own.
pub fn page(session: &str, before: i64, limit: u64) -> Result<i32> {
    let mut stream = connect()?;
    writeln!(stream, "{}", json!({"op": "page", "session": session, "before": before, "limit": limit})).map_err(|err| err.to_string())?;
    let mut out = std::io::stdout().lock();
    for line in BufReader::new(stream).lines() {
        let line = line.map_err(|err| err.to_string())?;
        writeln!(out, "{line}").map_err(|err| err.to_string())?;
        let value: Value = serde_json::from_str(&line).unwrap_or_default();
        if value["t"] == "page_done" || value["t"] == "error" {
            break;
        }
    }
    Ok(0)
}

fn connect() -> Result<UnixStream> {
    serve(false, None)?;
    UnixStream::connect(socket()).map_err(|err| format!("the follower does not answer: {err}"))
}

// MARK: recording

/// One pane's share of herdr's answers and its Transcript's growth, timed, for replaying the
/// session in a test; other panes are left out, since they hold other conversations.
pub struct Recorder {
    file: File,
    started: Instant,
    sizes: HashMap<String, u64>,
    pane: String,
}

impl Recorder {
    fn new(dir: &Path, pane: String) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        let file = File::create(dir.join(format!("{stamp}.jsonl"))).map_err(|err| err.to_string())?;
        Ok(Recorder { file, started: Instant::now(), sizes: HashMap::new(), pane })
    }

    fn write(&mut self, mut event: Value) {
        event["at"] = (self.started.elapsed().as_millis() as u64).into();
        let _ = writeln!(self.file, "{event}");
    }

    /// Whole lines added to each followed Transcript since the last look.
    fn transcripts(&mut self, panes: &[Value]) {
        let paths: Vec<String> = panes.iter().filter(|pane| pane["pane_id"] == self.pane.as_str()).filter_map(transcript_path).collect();
        for path in paths {
            let Ok(file) = File::open(&path) else { continue };
            let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
            let known = *self.sizes.entry(path.clone()).or_insert(0);
            if size <= known {
                continue;
            }
            let mut reader = BufReader::new(file);
            let _ = std::io::Seek::seek(&mut reader, std::io::SeekFrom::Start(known));
            let mut added = String::new();
            let mut line = String::new();
            let mut read = 0;
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line.ends_with('\n') {
                read += line.len();
                if parsed(&line) {
                    added.push_str(&line);
                }
                line.clear();
            }
            self.sizes.insert(path.clone(), known + read as u64);
            if !added.is_empty() {
                self.write(json!({"transcript": path, "lines": added}));
            }
        }
    }
}

/// Whether a Transcript line is one the parsers read. Claude also records its whole system
/// prompt, the user's instructions and email among it, which a recording must never keep.
fn parsed(line: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(line) else { return false };
    if value["isMeta"] == true {
        return false;
    }
    match value["type"].as_str() {
        Some("user" | "assistant" | "queue-operation") => true,
        Some("attachment") => value["attachment"]["type"] == "queued_command",
        Some(_) => false,
        // Codex and pi lines carry no Claude type.
        None => true,
    }
}

impl Source for Recorder {
    fn panes(&mut self) -> Result<Vec<Value>> {
        let panes = Herdr.panes()?;
        let mine: Vec<&Value> = panes.iter().filter(|pane| pane["pane_id"] == self.pane.as_str()).collect();
        self.write(json!({"panes": mine}));
        Ok(panes)
    }

    fn read(&mut self, pane_id: &str, ansi: bool) -> Result<String> {
        let screen = Herdr.read(pane_id, ansi)?;
        if pane_id == self.pane {
            self.write(json!({"read": pane_id, "ansi": ansi, "screen": screen}));
        }
        Ok(screen)
    }

    fn recorder(&mut self) -> Option<&mut Recorder> {
        Some(self)
    }
}
