//! A command's standard streams over mosh's transport: mosh-server is started over a Link's
//! connection, and from then on the pipe needs nothing of it, so it outlives the Link and
//! picks up after the app was frozen in the background.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mosh::handshake::Handshake;
use mosh::launch::{announce_prefix, server_command};
use mosh::opts::Opts;
use mosh_pipe::Ending;
use russh::{client, ChannelMsg};


/// Every method is called from a thread of the core's own.
pub trait Events: Send + Sync + 'static {
    fn output(&self, bytes: &[u8]);
    /// The last call: the command's status, -1 when it never ran and 255 when the Host
    /// stopped answering, and its stderr or why it failed.
    fn ended(&self, status: i32, stderr: &str);
}

/// What the app holds of a pipe: the command's input, and the way to stop it.
pub struct Pipe {
    input: Mutex<Option<UnixStream>>,
    stop: Arc<AtomicBool>,
}

/// What the Link needs to start a pipe.
pub struct Start {
    pub server: String,
    pub command: String,
    input: UnixStream,
    stop: Arc<AtomicBool>,
    end: End,
}

/// Says `ended` exactly once, even for a pipe dropped before it started.
struct End(Option<Arc<dyn Events>>);

impl End {
    fn say(mut self, status: i32, stderr: &str) {
        if let Some(events) = self.0.take() {
            events.ended(status, stderr);
        }
    }
}

impl Drop for End {
    fn drop(&mut self) {
        if let Some(events) = self.0.take() {
            events.ended(-1, "the connection to the Host is gone");
        }
    }
}

impl Pipe {
    pub fn new(server: String, command: String, events: Arc<dyn Events>) -> std::io::Result<(Pipe, Start)> {
        let (writer, reader) = UnixStream::pair()?;
        let stop = Arc::new(AtomicBool::new(false));
        let pipe = Pipe { input: Mutex::new(Some(writer)), stop: stop.clone() };
        Ok((pipe, Start { server, command, input: reader, stop, end: End(Some(events)) }))
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Some(input) = self.input.lock().unwrap().as_mut() {
            let _ = input.write_all(bytes);
        }
    }

    /// Ends the command; `ended` still follows.
    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // Closing our end wakes the client to notice.
        self.input.lock().unwrap().take();
    }
}

/// Runs the pipe over mosh where the Host is reached directly, and over the connection itself
/// where a proxy stands between, since mosh's datagrams cannot follow it there.
pub async fn run<H: client::Handler>(handle: Arc<client::Handle<H>>, start: Start, direct: bool) {
    if !direct {
        return over_channel(&handle, start).await;
    }
    let Start { server, command, mut input, stop, end } = start;
    let (port, key, ip) = match bootstrap(&handle, &server, &command).await {
        Ok(found) => found,
        Err(problem) => return end.say(-1, &problem),
    };
    drop(handle);
    if stop.load(Ordering::SeqCst) {
        return end.say(0, "");
    }
    let events = end.0.clone().expect("not yet ended");
    let followed = tokio::task::spawn_blocking(move || {
        let mut stderr = Vec::new();
        let ending = mosh_pipe::follow(&key, &ip, port, &mut input, || stop.load(Ordering::SeqCst), |got| {
            if !got.out.is_empty() {
                events.output(&got.out);
            }
            stderr.extend_from_slice(&got.err);
            true
        });
        (ending, String::from_utf8_lossy(&stderr).into_owned())
    })
    .await;
    match followed {
        Ok((Ok(Ending::Exited(status)), stderr)) => end.say(status, &stderr),
        Ok((Ok(Ending::Stopped), _)) => end.say(0, ""),
        Ok((Ok(Ending::ServerEnded), _)) => end.say(1, "the server ended the pipe"),
        Ok((Ok(Ending::Lost(why)), _)) => end.say(255, &why),
        Ok((Err(e), _)) => end.say(-1, &format!("the mosh pipe failed: {e}")),
        Err(e) => end.say(-1, &format!("the mosh pipe failed: {e}")),
    }
}

/// Starts mosh-server and reads where to find it, keeping what the login printed to explain
/// a failure.
async fn bootstrap<H: client::Handler>(
    handle: &client::Handle<H>,
    server: &str,
    command: &str,
) -> Result<(u16, String, String), String> {
    let opts = Opts {
        server: server.into(),
        pipe: true,
        command: vec!["sh".into(), "-c".into(), command.into()],
        ..Default::default()
    };
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("the connection to the Host is gone: {e}"))?;
    channel
        .exec(true, format!("{}{}", announce_prefix(), server_command(&opts, &crate::servers::pipe_vars())))
        .await
        .map_err(|e| format!("starting mosh-server: {e}"))?;

    let (mut found, mut pending, mut chatter) = (Handshake::default(), Vec::new(), Vec::new());
    while found.connect.is_none() {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => pending.extend_from_slice(&data),
            Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => break,
            Some(_) => continue,
        }
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(text) = found.line(line.trim_end_matches(['\n', '\r']))?.filter(|t| !t.is_empty()) {
                chatter.push(text.to_string());
            }
        }
    }
    let Some((port, key)) = found.connect.clone() else {
        chatter.push(String::from_utf8_lossy(&pending).into_owned());
        return Err(format!("mosh-server did not start: {}", chatter.join("\n").trim()));
    };
    let ip = found.target().ok_or("mosh-server never said where it was")?.to_string();
    Ok((port, key, ip))
}

/// The command on a channel of the connection: its input copied in, its output handed over.
async fn over_channel<H: client::Handler>(handle: &client::Handle<H>, start: Start) {
    let Start { command, input, end, .. } = start;
    let mut channel = match handle.channel_open_session().await {
        Ok(channel) => channel,
        Err(e) => return end.say(-1, &format!("the connection to the Host is gone: {e}")),
    };
    if let Err(e) = channel.exec(true, command).await {
        return end.say(-1, &format!("running a command: {e}"));
    }
    let (sender, mut written) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    tokio::task::spawn_blocking(move || {
        let mut input = input;
        let mut chunk = [0u8; 4096];
        while let Ok(read @ 1..) = std::io::Read::read(&mut input, &mut chunk) {
            if sender.send(chunk[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    let events = end.0.clone().expect("not yet ended");
    let (mut status, mut stderr, mut open) = (-1, Vec::new(), true);
    loop {
        tokio::select! {
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) => events.output(&data),
                Some(ChannelMsg::ExtendedData { data, .. }) => stderr.extend_from_slice(&data),
                Some(ChannelMsg::ExitStatus { exit_status }) => status = exit_status as i32,
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            bytes = written.recv(), if open => match bytes {
                Some(bytes) => {
                    let _ = channel.data(&bytes[..]).await;
                }
                // The app closed the input, which ends the command as a mosh pipe's end does.
                None => {
                    open = false;
                    let _ = channel.eof().await;
                    let _ = channel.signal(russh::Sig::TERM).await;
                    let _ = channel.close().await;
                }
            },
        }
    }
    end.say(if open { status } else { 0 }, &String::from_utf8_lossy(&stderr));
}
