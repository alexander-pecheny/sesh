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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{transcript_path, Agent};
use crate::follower::{Event, Follower, Herdr, Input, Source};
use crate::log::{Log, Writer};
use crate::reconcile::{HANDED, QUEUED, SENT};
use crate::Transcript;

pub const PROTOCOL: u64 = 5;
const TICK: Duration = Duration::from_millis(100);
/// herdr's panes are listed less often than the screens of working Agents are read.
const LIST_EVERY: u32 = 2;
const GONE_AFTER: Duration = Duration::from_secs(2);
/// With no device attached this long, the follower leaves; the Transcript fills the gap later.
const IDLE_EXIT: Duration = Duration::from_secs(6 * 60 * 60);
const PING: Duration = Duration::from_secs(30);
/// Entries a session starts with when the follower first meets it.
const FIRST: usize = 200;
/// Items a device gets when it opens a session.
const OPENING: usize = 80;
const START_WAIT: Duration = Duration::from_secs(3);
/// How long a follower asked to leave gives the acts it took to be played and answered.
const LEAVE_WAIT: Duration = Duration::from_secs(5);

type Result<T> = std::result::Result<T, String>;

/// The follower's protocol and its Session log's schema, as in `p4-s1`, which name its folder:
/// every helper build that shares both runs the same follower (ADR 0014).
pub fn follower() -> String {
    format!("p{PROTOCOL}-s{}", crate::log::VERSION)
}

/// Orders the builds of one follower, so a newer one takes over: build-helpers.sh stamps its
/// UTC time and a dev build is 0. SESH_BUILD at run time stands in, so a test can play either.
fn build() -> u64 {
    let stamp = std::env::var("SESH_BUILD").ok().or(option_env!("SESH_BUILD").map(str::to_string));
    stamp.and_then(|stamp| stamp.parse().ok()).unwrap_or(0)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".sesh/follower").join(follower())
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

    fn input(&mut self, pane_id: &str, input: &Input) -> Result<()> {
        self.0.borrow_mut().input(pane_id, input)
    }
}

/// Makes the Source an act plays through on a thread of its own.
pub type Hands = Arc<dyn Fn() -> Box<dyn Source + Send> + Send + Sync>;
/// One act for a pane's thread: the request, the pane as last listed, and where its reply goes.
type Job = (Value, Option<Value>, mpsc::Sender<Value>);
/// An act that failed: its session and its request.
type Failed = (String, Value);

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
    /// The panes as herdr last listed them, which answers and permissions are checked against.
    panes: Vec<Value>,
    hands: Hands,
    /// Each pane's queue of acts, played in order on the pane's own thread.
    acts: HashMap<String, mpsc::Sender<Job>>,
    /// How many acts are taken and not yet answered.
    playing: Arc<AtomicUsize>,
    /// When a newer build asked this follower to leave, after which it takes no more acts.
    leaving: Option<Instant>,
    /// Acts that failed, for the message each carried to be dropped or marked lost.
    failed: (mpsc::Sender<Failed>, mpsc::Receiver<Failed>),
    /// The time of the last tick: the clock, or a recording's.
    now: Instant,
}

impl Machine {
    pub fn new(log: Log, source: Shared) -> Self {
        let (hands, failed): (Hands, _) = (Arc::new(|| Box::new(Herdr)), mpsc::channel());
        Machine { log, source, sessions: HashMap::new(), panes: Vec::new(), hands, acts: HashMap::new(), playing: Arc::default(), leaving: None, failed, now: Instant::now() }
    }

    pub fn with_hands(self, hands: Hands) -> Self {
        Machine { hands, ..self }
    }

    pub fn source(&self) -> Shared {
        self.source.clone()
    }

    pub fn following(&self) -> usize {
        self.sessions.len()
    }

    /// One look at every pane: new Agents are met, known ones followed, gone ones ended.
    pub fn tick(&mut self, panes: &[Value], now: Instant) -> Result<()> {
        (self.panes, self.now) = (panes.to_vec(), now);
        while let Ok((session, request)) = self.failed.1.try_recv() {
            let Some(followed) = self.sessions.get_mut(&session) else { continue };
            match (request["id"].as_str(), request["lost"].as_str()) {
                (Some(id), _) => followed.follower.live.forget(id),
                (_, Some(id)) => followed.follower.live.lose(id, now),
                _ => {}
            }
        }
        let mut hands = Vec::new();
        // A queue let go of ends its thread once the acts already in it are played.
        self.acts.retain(|key, _| panes.iter().any(|pane| pane["pane_id"] == key.as_str()));
        for pane in panes {
            let (Some(key), Some(agent)) = (pane["pane_id"].as_str(), Agent::of(pane)) else { continue };
            if let Some(followed) = self.sessions.get_mut(key) {
                followed.seen = now;
                followed.follower.tick(pane, now)?;
                // The queue goes to the Agent once it can take a message (ADR 0015).
                if followed.follower.state().is_some_and(|state| !matches!(state, "working" | "blocked")) {
                    if let Some((id, text)) = followed.follower.live.hand(SENT, now) {
                        followed.follower.emit_live()?;
                        hands.push((key.to_string(), json!({"op": "send", "text": text, "lost": id})));
                    }
                }
                let events = followed.follower.drain();
                followed.writer.apply(&self.log, &events, Some(pane))?;
                continue;
            }
            let writer = Writer::new(key, &self.log)?;
            let mut follower = Follower::new(agent, FIRST, Box::new(self.source.clone()));
            follower.adopt(&writer.adopted(), now);
            let since = writer.cursor().map(str::to_string);
            follower.start(pane, since.as_deref())?;
            let mut followed = Followed { follower, writer, seen: now };
            let events = followed.follower.drain();
            followed.writer.apply(&self.log, &events, Some(pane))?;
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
            followed.writer.apply(&self.log, &[Event::State("ended".into()), Event::Live(Default::default())], None)?;
        }
        for (session, request) in hands {
            self.play(&session, request, mpsc::channel().0);
        }
        Ok(())
    }

    /// Plays what a device asked into the session's pane off the follower's loop, since a stop
    /// or an answer takes seconds; `reply` gets the `done` or `error` line once it is played.
    pub fn act(&mut self, session: &str, request: Value, reply: mpsc::Sender<Value>) {
        if self.leaving.is_some() {
            let _ = reply.send(json!({"t": "leaving", "op": request["op"]}));
            return;
        }
        match self.message(session, &request) {
            Ok(Some(play)) => self.play(session, play, reply),
            Ok(None) => drop(reply.send(json!({"t": "done", "op": request["op"], "session": session}))),
            Err(err) => drop(reply.send(json!({"t": "error", "op": request["op"], "message": err}))),
        }
    }

    /// What an act does to the session's messages (ADR 0015), and what then goes to the pane.
    fn message(&mut self, session: &str, request: &Value) -> Result<Option<Value>> {
        let (op, id) = (request["op"].as_str().unwrap_or_default(), request["id"].as_str().unwrap_or_default());
        let ours = matches!(op, "unqueue" | "hand");
        let followed = match self.sessions.get_mut(session) {
            Some(followed) if ours || op == "send" && !id.is_empty() => followed,
            None if ours => return Err(format!("the follower does not follow {session}")),
            _ => return Ok(Some(request.clone())),
        };
        let (follower, now) = (&mut followed.follower, self.now);
        let busy = follower.state() == Some("working");
        let text = request["text"].as_str().unwrap_or_default();
        let state = if busy { HANDED } else { SENT };
        let play = match op {
            // pi takes no message while it works, so Sesh holds it.
            "send" if busy && follower.agent() == Agent::Pi => {
                follower.live.message(id, text, QUEUED, now);
                None
            }
            "send" => {
                follower.live.message(id, text, state, now);
                Some(request.clone())
            }
            "unqueue" if follower.live.unqueue(id) => None,
            "unqueue" => return Err("the Agent has that message already".into()),
            _ => follower.live.hand(state, now).map(|(id, text)| json!({"op": "send", "text": text, "lost": id})),
        };
        follower.emit_live()?;
        let events = follower.drain();
        followed.writer.apply(&self.log, &events, None)?;
        Ok(play)
    }

    /// Plays `request` on the pane's own thread, after the acts before it.
    fn play(&mut self, session: &str, request: Value, reply: mpsc::Sender<Value>) {
        let pane = self.panes.iter().find(|pane| pane["pane_id"] == session).cloned();
        let (hands, playing, failed) = (&self.hands, &self.playing, &self.failed.0);
        let queue = self.acts.entry(session.to_string()).or_insert_with(|| {
            let (queue, jobs) = mpsc::channel::<Job>();
            let (mut source, session, playing, failed) = (hands(), session.to_string(), playing.clone(), failed.clone());
            std::thread::spawn(move || {
                for (request, pane, reply) in jobs {
                    let line = match crate::act::act(source.as_mut(), &session, pane.as_ref(), &request) {
                        Ok(()) => json!({"t": "done", "op": request["op"], "session": session}),
                        Err(err) => {
                            let _ = failed.send((session.clone(), request.clone()));
                            json!({"t": "error", "op": request["op"], "message": err})
                        }
                    };
                    let _ = reply.send(line);
                    playing.fetch_sub(1, Ordering::SeqCst);
                }
            });
            queue
        });
        self.playing.fetch_add(1, Ordering::SeqCst);
        let _ = queue.send((request, pane, reply));
    }

    /// Whether this follower was asked to leave and every act it took is answered.
    fn left(&self) -> bool {
        self.leaving.is_some() && self.playing.load(Ordering::SeqCst) == 0
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
        let Some(oldest) = self.log.first_entry(session)? else {
            return Ok((page, false));
        };
        let mut transcript = Transcript::new(agent, path);
        let Some((older, more)) = transcript.history(&oldest, limit).map_err(|err| err.to_string())? else {
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

/// Starts the follower unless one of this build or a newer one answers, and returns once it does.
pub fn serve(foreground: bool, record: Option<(PathBuf, String)>) -> Result<i32> {
    if UnixStream::connect(socket()).is_ok_and(|running| !took_over(running)) {
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

/// Asks a follower of an older build to leave and waits until it has hung up, so the log
/// it kept goes on under this build; a follower of this build or a newer one stays.
fn took_over(running: UnixStream) -> bool {
    let _ = running.set_read_timeout(Some(START_WAIT));
    let mut lines = BufReader::new(&running).lines();
    let _ = writeln!(&running, "{}", json!({"op": "hello"}));
    let hello: Value = lines.next().and_then(|line| serde_json::from_str(&line.ok()?).ok()).unwrap_or_default();
    if hello["build"].as_u64().unwrap_or(0) >= build() {
        return false;
    }
    let _ = writeln!(&running, "{}", json!({"op": "leave", "build": build()}));
    let _ = running.set_read_timeout(Some(LEAVE_WAIT + START_WAIT));
    lines.map_while(std::result::Result::ok).for_each(drop);
    true
}

fn run(record: Option<(PathBuf, String)>) -> Result<i32> {
    std::fs::create_dir_all(home()).map_err(|err| err.to_string())?;
    let lock = File::create(home().join("follower.lock")).map_err(|err| err.to_string())?;
    let started = Instant::now();
    while lock.try_lock().is_err() {
        if started.elapsed() > START_WAIT {
            return Ok(0);
        }
        std::thread::sleep(TICK);
    }
    let log = Log::open(&home().join("sessions.db"))?;
    let path = socket();
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    listener.set_nonblocking(true).map_err(|err| err.to_string())?;
    let source: Rc<RefCell<dyn Source>> = match record {
        Some((dir, pane)) => Rc::new(RefCell::new(Recorder::new(&dir, pane)?)),
        None => Rc::new(RefCell::new(Herdr)),
    };
    let mut machine = Machine::new(log, Shared(source.clone()));
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
        // Read before serving, so the replies to every act played go out below.
        let left = machine.left();
        for client in &mut clients {
            client.serve(&mut machine);
        }
        clients.retain(|client| client.open);
        let answered = left && clients.iter().all(|client| client.write.is_empty());
        if answered || machine.leaving.is_some_and(|since| since.elapsed() > LEAVE_WAIT) {
            let _ = std::fs::remove_file(&path);
            return Ok(0);
        }
        if !clients.is_empty() {
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
    /// Replies to this device's acts, as their threads finish them.
    replies: (mpsc::Sender<Value>, mpsc::Receiver<Value>),
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        Client { stream, read: Vec::new(), write: Vec::new(), open: true, summaries: 0, all: false, watched: HashMap::new(), pinged: Instant::now(), done_asking: false, replies: mpsc::channel() }
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
        while let Ok(reply) = self.replies.1.try_recv() {
            self.send(&reply);
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
                self.send(&json!({"t": "hello", "protocol": PROTOCOL, "version": crate::version(), "build": build()}));
                self.all = request["sessions"] == true;
            }
            "leave" => {
                if request["build"].as_u64().unwrap_or(0) > build() {
                    machine.leaving.get_or_insert_with(Instant::now);
                }
            }
            "watch" => {
                if let Some(mut summary) = machine.log.session(&session)? {
                    summary["t"] = "session".into();
                    summary["session"] = session.clone().into();
                    self.send(&summary);
                }
                let since = request["since"].as_i64().unwrap_or(0);
                if since > 0 && (machine.log.base()?..=machine.log.head()?).contains(&since) {
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
            op if crate::act::OPS.contains(&op) => machine.act(&session, request.clone(), self.replies.0.clone()),
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

/// One request from a device that runs a command of its own for each, as the phone must
/// (ADR 0012): `page` prints the page's lines, and an act fails with the follower's error.
/// An act a leaving follower refuses goes to the newer one once it answers.
pub fn ask(request: &Value) -> Result<i32> {
    let mut stream = connect()?;
    let deadline = Instant::now() + LEAVE_WAIT + START_WAIT;
    loop {
        if let Some(code) = ask_once(stream, request)? {
            return Ok(code);
        }
        if Instant::now() > deadline {
            return Err("the follower would not take it".into());
        }
        stream = loop {
            std::thread::sleep(TICK);
            match UnixStream::connect(socket()) {
                Ok(stream) => break stream,
                Err(_) if Instant::now() < deadline => {}
                Err(_) => break connect()?,
            }
        };
    }
}

/// `request` asked once; nothing when the follower is leaving and took no act, or left before
/// it read the request, as one does that connected in its last tick.
fn ask_once(mut stream: UnixStream, request: &Value) -> Result<Option<i32>> {
    if writeln!(stream, "{request}").is_err() {
        return Ok(None);
    }
    let page = request["op"] == "page";
    let mut out = std::io::stdout().lock();
    let mut heard = false;
    for line in BufReader::new(stream).lines() {
        heard = true;
        let line = line.map_err(|err| err.to_string())?;
        let value: Value = serde_json::from_str(&line).unwrap_or_default();
        if page {
            writeln!(out, "{line}").map_err(|err| err.to_string())?;
        }
        match value["t"].as_str() {
            Some("leaving") => return Ok(None),
            Some("error") if !page => return Err(value["message"].as_str().unwrap_or("the follower failed").to_string()),
            Some("page_done" | "error" | "done") => return Ok(Some(0)),
            _ => {}
        }
    }
    match (page, heard) {
        (true, _) => Ok(Some(0)),
        (false, false) => Ok(None),
        (false, true) => Err("the follower hung up".into()),
    }
}

/// The follower's socket, once one answers on it: a follower that was answering may be handing
/// over to a newer build, which binds the socket a moment after it leaves.
fn connect() -> Result<UnixStream> {
    serve(false, None)?;
    let deadline = Instant::now() + START_WAIT;
    loop {
        match UnixStream::connect(socket()) {
            Ok(stream) => return Ok(stream),
            Err(_) if Instant::now() < deadline => std::thread::sleep(TICK),
            Err(err) => return Err(format!("the follower does not answer: {err}")),
        }
    }
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

    fn input(&mut self, pane_id: &str, input: &Input) -> Result<()> {
        Herdr.input(pane_id, input)
    }

    fn recorder(&mut self) -> Option<&mut Recorder> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::follower::Input;

    /// herdr with nothing to list or read; the test hands it the panes itself.
    struct Quiet;

    impl Source for Quiet {
        fn panes(&mut self) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        fn read(&mut self, _: &str, _: bool) -> Result<String> {
            Err("nothing on screen".into())
        }

        fn input(&mut self, _: &str, _: &Input) -> Result<()> {
            Err("the follower's own Source takes no input".into())
        }
    }

    /// herdr taking its time over every input.
    struct Slow(Arc<Mutex<Vec<Input>>>);

    impl Source for Slow {
        fn panes(&mut self) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        fn read(&mut self, _: &str, _: bool) -> Result<String> {
            Ok(String::new())
        }

        fn input(&mut self, _: &str, input: &Input) -> Result<()> {
            std::thread::sleep(Duration::from_millis(500));
            self.0.lock().unwrap().push(input.clone());
            Ok(())
        }
    }

    /// herdr refusing every input.
    struct Broken;

    impl Source for Broken {
        fn panes(&mut self) -> Result<Vec<Value>> {
            Ok(Vec::new())
        }

        fn read(&mut self, _: &str, _: bool) -> Result<String> {
            Ok(String::new())
        }

        fn input(&mut self, _: &str, _: &Input) -> Result<()> {
            Err("no such pane".into())
        }
    }

    #[test]
    fn a_message_whose_send_failed_is_dropped_for_its_device_to_take_back() {
        let dir = std::env::temp_dir().join(format!("sesh-failed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let panes = [json!({"pane_id": "w1:p1", "agent": "codex", "agent_status": "idle"})];
        let mut machine = Machine::new(Log::open(&dir.join("sessions.db")).unwrap(), Shared(Rc::new(RefCell::new(Quiet))))
            .with_hands(Arc::new(|| Box::new(Broken)));
        let start = Instant::now();
        machine.tick(&panes, start).unwrap();
        let (reply, replies) = mpsc::channel();
        machine.act("w1:p1", json!({"op": "send", "id": "sent.1", "text": "hello"}), reply);
        assert_eq!(machine.log.last("w1:p1", 10).unwrap()[0]["entry"]["state"], "sent");
        assert_eq!(replies.recv_timeout(Duration::from_secs(5)).unwrap()["t"], "error");
        machine.tick(&panes, start + TICK).unwrap();
        let items = machine.log.last("w1:p1", 10).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(items, Vec::<Value>::new());
    }

    #[test]
    fn a_slow_act_holds_up_neither_the_log_nor_the_acts_order() {
        let dir = std::env::temp_dir().join(format!("sesh-act-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("t.jsonl");
        let say = |text: &str| {
            let line = json!({"type": "user", "timestamp": "2026-10-09T10:00:00Z", "origin": {"kind": "human"}, "message": {"role": "user", "content": text}});
            let mut file = OpenOptions::new().create(true).append(true).open(&transcript).unwrap();
            writeln!(file, "{line}").unwrap();
        };
        say("first");
        let panes = [json!({"pane_id": "w1:p1", "agent": "claude", "agent_status": "idle",
            "agent_session": {"kind": "path", "value": transcript.to_str().unwrap()}})];
        let played = Arc::new(Mutex::new(Vec::new()));
        let hands = played.clone();
        let mut machine = Machine::new(Log::open(&dir.join("sessions.db")).unwrap(), Shared(Rc::new(RefCell::new(Quiet))))
            .with_hands(Arc::new(move || Box::new(Slow(hands.clone()))));
        let start = Instant::now();
        machine.tick(&panes, start).unwrap();
        let seen = machine.log.head().unwrap();

        let (reply, replies) = mpsc::channel();
        machine.act("w1:p1", json!({"op": "keys", "keys": ["esc"]}), reply.clone());
        machine.act("w1:p1", json!({"op": "send", "text": "second"}), reply);
        say("second");
        machine.tick(&panes, start + TICK).unwrap();
        let items = machine.log.items_since("w1:p1", seen).unwrap();
        assert!(items.iter().any(|item| item["entry"]["text"] == "second"), "{items:?}");
        assert!(replies.try_recv().is_err(), "the act finished before the log moved on");

        let ops: Vec<Value> = (0..2).map(|_| replies.recv_timeout(Duration::from_secs(5)).unwrap()["op"].clone()).collect();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(ops, ["keys", "send"]);
        assert_eq!(*played.lock().unwrap(), [Input::Keys(vec!["esc".into()]), Input::Prompt("second".into())]);
    }
}
