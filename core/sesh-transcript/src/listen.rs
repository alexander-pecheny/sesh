//! sesh-mux's socket: its requests, made as its own CLI makes them, and its pane list, asked for
//! again only when sesh-mux says something changed. Listing every few ticks kept sesh-mux's
//! server busy enough on a slow Host to delay a shell's echo.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::follower::{Input, Source};

/// How long sesh-mux gets to answer one request.
const ANSWER: Duration = Duration::from_secs(30);

/// The socket of the sesh-mux server.
pub fn socket() -> PathBuf {
    std::env::var_os("SESH_MUX_SOCKET_PATH")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".sesh/mux/sesh-mux.sock"))
}

/// The version of sesh-mux's socket API this helper reads (ADR 0023). A sesh-mux that names none
/// speaks 1, the API from before it said so.
pub const MUX_API: u64 = 1;

/// Whether the sesh-mux behind the socket said it speaks `MUX_API`; asked again after the
/// socket fails or the event stream ends, as when sesh-mux is handed over to another build.
static AGREED: AtomicBool = AtomicBool::new(false);

/// Asks sesh-mux's server `method` over its socket, which takes one request per connection: the
/// result, or the error, which carries a code and a message. It refuses to ask a sesh-mux whose
/// API is not this helper's, which would answer in a shape the helper misreads.
pub fn call(method: &str, params: Value) -> Result<Value, Value> {
    if !AGREED.load(Ordering::SeqCst) {
        agree(&ask("ping", json!({}))?)?;
        AGREED.store(true, Ordering::SeqCst);
    }
    ask(method, params).inspect_err(|err| {
        if err["code"] == "unreachable" {
            AGREED.store(false, Ordering::SeqCst);
        }
    })
}

fn agree(pong: &Value) -> Result<(), Value> {
    let api = pong["api"].as_u64().unwrap_or(1);
    if api == MUX_API {
        return Ok(());
    }
    let fix = if api > MUX_API { "Update Sesh on this device" } else { "Connect with the newest Sesh to update it" };
    Err(json!({"code": "api_mismatch", "message": format!(
        "sesh-mux {} speaks API {api}, and this helper speaks API {MUX_API}. {fix}.",
        pong["version"].as_str().unwrap_or_default())}))
}

fn ask(method: &str, params: Value) -> Result<Value, Value> {
    let failed = |err: std::io::Error| json!({"code": "unreachable", "message": format!("sesh-mux: {err}")});
    let stream = UnixStream::connect(socket()).map_err(failed)?;
    stream.set_read_timeout(Some(ANSWER)).map_err(failed)?;
    writeln!(&stream, "{}", json!({"id": "sesh-transcript", "method": method, "params": params})).map_err(failed)?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).map_err(failed)?;
    let reply: Value = serde_json::from_str(&line)
        .map_err(|err| json!({"code": "unreadable", "message": format!("sesh-mux said something sesh-transcript cannot read: {err}")}))?;
    match reply.get("error") {
        Some(error) => Err(error.clone()),
        None => Ok(reply["result"].clone()),
    }
}

/// What a sesh-mux error says, for a person to read.
pub fn refusal(error: &Value) -> String {
    error["message"].as_str().unwrap_or("sesh-mux failed").to_string()
}

/// `inner`, with its pane list kept until sesh-mux reports a change. Where sesh-mux cannot be
/// listened to, every call lists afresh, as before.
pub struct Listening<S: Source> {
    inner: S,
    socket: PathBuf,
    panes: Vec<Value>,
    changed: Arc<AtomicBool>,
    events: Option<Events>,
}

impl<S: Source> Listening<S> {
    pub fn new(inner: S, socket: PathBuf) -> Self {
        Listening { inner, socket, panes: Vec::new(), changed: Arc::default(), events: None }
    }
}

impl<S: Source> Source for Listening<S> {
    fn panes(&mut self) -> Result<Vec<Value>, String> {
        let heard = self.events.as_ref().is_some_and(|events| events.alive.load(Ordering::SeqCst));
        if heard && !self.changed.swap(false, Ordering::SeqCst) {
            return Ok(self.panes.clone());
        }
        self.panes = self.inner.panes()?;
        let ids: Vec<String> = self.panes.iter().filter_map(|pane| pane["pane_id"].as_str().map(str::to_string)).collect();
        if !heard || self.events.as_ref().is_some_and(|events| events.panes != ids) {
            self.events = Events::subscribe(&self.socket, ids, self.changed.clone()).ok();
        }
        Ok(self.panes.clone())
    }

    fn read(&mut self, pane_id: &str, ansi: bool) -> Result<String, String> {
        self.inner.read(pane_id, ansi)
    }

    fn input(&mut self, pane_id: &str, input: &Input) -> Result<(), String> {
        self.inner.input(pane_id, input)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.inner.call(method, params)
    }
}

/// One subscription to sesh-mux's events, which ends with the connection.
struct Events {
    stream: UnixStream,
    /// The panes whose Agents' state changes it hears of; sesh-mux names each one.
    panes: Vec<String>,
    alive: Arc<AtomicBool>,
}

impl Events {
    fn subscribe(socket: &PathBuf, panes: Vec<String>, changed: Arc<AtomicBool>) -> std::io::Result<Self> {
        let mut stream = UnixStream::connect(socket)?;
        let kinds = ["pane.created", "pane.closed", "pane.updated", "pane.exited", "pane.agent_detected", "workspace.closed"];
        let mut subscriptions: Vec<Value> = kinds.iter().map(|kind| json!({"type": kind})).collect();
        subscriptions.extend(panes.iter().map(|pane| json!({"type": "pane.agent_status_changed", "pane_id": pane})));
        let request = json!({"id": "sesh-transcript", "method": "events.subscribe", "params": {"subscriptions": subscriptions}});
        writeln!(stream, "{request}")?;
        let reader = stream.try_clone()?;
        let alive = Arc::new(AtomicBool::new(true));
        let heard = alive.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                // A refusal, such as for a pane closed since the list, ends the subscription
                // and the next call lists and subscribes again.
                if serde_json::from_str::<Value>(&line).is_ok_and(|line| line.get("error").is_some()) {
                    break;
                }
                changed.store(true, Ordering::SeqCst);
            }
            heard.store(false, Ordering::SeqCst);
            AGREED.store(false, Ordering::SeqCst);
        });
        Ok(Events { stream, panes, alive })
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::sync::Mutex;
    use std::time::Instant;

    use super::*;

    /// sesh-mux with one pane, counting how often it is listed.
    struct Counted(Arc<Mutex<usize>>);

    impl Source for Counted {
        fn panes(&mut self) -> Result<Vec<Value>, String> {
            *self.0.lock().unwrap() += 1;
            Ok(vec![json!({"pane_id": "w1:p1", "agent": "claude"})])
        }

        fn read(&mut self, _: &str, _: bool) -> Result<String, String> {
            Ok(String::new())
        }

        fn input(&mut self, _: &str, _: &Input) -> Result<(), String> {
            Ok(())
        }
    }

    fn wait_until(done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_list_is_asked_for_again_only_once_sesh_mux_says_something_changed() {
        let socket = std::env::temp_dir().join(format!("sesh-listen-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let mux = UnixListener::bind(&socket).unwrap();
        let listed = Arc::new(Mutex::new(0));
        let mut listening = Listening::new(Counted(listed.clone()), socket.clone());

        listening.panes().unwrap();
        let (mut client, _) = mux.accept().unwrap();
        let mut subscribed = String::new();
        BufReader::new(client.try_clone().unwrap()).read_line(&mut subscribed).unwrap();
        writeln!(client, r#"{{"id":"sesh-transcript","result":{{"type":"subscribed"}}}}"#).unwrap();
        wait_until(|| listening.changed.load(Ordering::SeqCst));
        listening.panes().unwrap();
        let after_ack = *listed.lock().unwrap();
        for _ in 0..5 {
            listening.panes().unwrap();
        }
        let quiet = *listed.lock().unwrap();
        writeln!(client, r#"{{"event":"pane.closed","data":{{"pane_id":"w1:p2"}}}}"#).unwrap();
        wait_until(|| listening.changed.load(Ordering::SeqCst));
        listening.panes().unwrap();
        let _ = std::fs::remove_file(&socket);

        assert!(subscribed.contains(r#""pane_id":"w1:p1""#), "{subscribed}");
        assert_eq!(quiet, after_ack, "listed again with nothing changed");
        assert_eq!(*listed.lock().unwrap(), after_ack + 1);
    }

    #[test]
    fn without_sesh_mux_events_every_call_lists_afresh() {
        let listed = Arc::new(Mutex::new(0));
        let mut listening = Listening::new(Counted(listed.clone()), std::env::temp_dir().join("sesh-listen-nobody.sock"));
        for _ in 0..3 {
            listening.panes().unwrap();
        }
        assert_eq!(*listed.lock().unwrap(), 3);
    }
}
