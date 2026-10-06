//! A Link: an SSH connection with no terminal, on which Projects runs commands, follows
//! a Conversation and uploads.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use russh::client;
use russh::{ChannelMsg, Sig};
use tokio::sync::oneshot;

use crate::flags::{self, Transport};
use crate::session::{in_login_shell, Command, Context, Events, Session, State};
use crate::ssh::Dialer;
use crate::upload::{self, Pending};

/// Printed before the command, so whatever the rc files say is not taken for its output.
const MARK: &str = "--sesh-output--";

pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub key: Option<String>,
    pub key_passphrase: Option<String>,
    pub known_hosts: PathBuf,
    pub extra_flags: String,
    pub transport: Transport,
}

pub struct Ran {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn connect(config: Config, events: Arc<dyn Events>) -> Session {
    Session::start(events, move |context| drive(config, context))
}

async fn drive(config: Config, context: Context) -> Result<(), String> {
    let Context {
        events,
        answers,
        mut commands,
    } = context;
    let flags = match config.transport {
        Transport::Ssh => flags::parse_ssh(&config.extra_flags)?,
        Transport::Mosh => flags::parse_mosh(&config.extra_flags)?.ssh,
    };
    if flags.jump.is_some() {
        return Err("Projects cannot reach a Host through -J yet".into());
    }
    let dialer = Dialer {
        host: config.host.clone(),
        port: flags.port.unwrap_or(config.port),
        user: flags.user.clone().unwrap_or(config.user),
        password: config.password,
        key: config.key,
        key_passphrase: config.key_passphrase,
        known_hosts: config.known_hosts,
        flags,
    };
    events.state(State::Connecting, "");
    let handle = Arc::new(dialer.connect(&events, &answers).await?);
    events.state(State::Connected, "");

    let mut uploads = Pending::default();
    let mut streams: HashMap<u32, oneshot::Sender<()>> = HashMap::new();
    loop {
        match commands.recv().await {
            Some(Command::Run(id, command)) => {
                let (handle, events) = (handle.clone(), events.clone());
                tokio::spawn(async move {
                    let ran = run(&handle, &command).await.unwrap_or_else(failed);
                    events.ran(id, &ran);
                });
            }
            Some(Command::Stream(id, command)) => {
                let (cancel, cancelled) = oneshot::channel();
                streams.retain(|_, stream| !stream.is_closed());
                streams.insert(id, cancel);
                let (handle, events) = (handle.clone(), events.clone());
                tokio::spawn(async move {
                    let ran = exec(&handle, &command, |bytes| events.chunk(id, bytes), cancelled)
                        .await
                        .unwrap_or_else(failed);
                    events.ran(id, &ran);
                });
            }
            Some(Command::Cancel(id)) => {
                if let Some(stream) = streams.remove(&id) {
                    let _ = stream.send(());
                }
            }
            Some(Command::Upload(id, files)) => {
                let task = upload::over_ssh(handle.clone(), id, files, uploads.begin(id), events.clone());
                tokio::spawn(task);
            }
            Some(Command::CancelUpload(id)) => uploads.cancel(id),
            Some(Command::Put(id, local, remote)) => {
                let (handle, events) = (handle.clone(), events.clone());
                tokio::spawn(async move {
                    let (status, stderr) = match upload::put(&handle, &local, &remote).await {
                        Ok(()) => (0, String::new()),
                        Err(error) => (1, error),
                    };
                    events.ran(id, &Ran { status, stdout: String::new(), stderr });
                });
            }
            Some(Command::Close) | None => break,
            Some(_) => {}
        }
    }
    Ok(())
}

fn failed(error: String) -> Ran {
    Ran {
        status: -1,
        stdout: String::new(),
        stderr: error,
    }
}

pub async fn run<H: client::Handler>(handle: &client::Handle<H>, command: &str) -> Result<Ran, String> {
    let mut stdout = Vec::new();
    let ran = exec(handle, command, |bytes| stdout.extend_from_slice(bytes), std::future::pending::<()>()).await?;
    Ok(Ran {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        ..ran
    })
}

/// Hands stdout to `out` as it arrives, and stops the command once `cancel` resolves. The
/// returned `Ran` carries the status and stderr; its stdout is empty.
async fn exec<H: client::Handler, C: Future>(
    handle: &client::Handle<H>,
    command: &str,
    mut out: impl FnMut(&[u8]),
    cancel: C,
) -> Result<Ran, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("the connection to the Host is gone: {e}"))?;
    channel
        .exec(true, in_login_shell(&format!("echo {MARK}; {command}")))
        .await
        .map_err(|e| format!("running a command: {e}"))?;
    let (mut unmark, mut stderr, mut status) = (Unmark::default(), Vec::new(), -1);
    tokio::pin!(cancel);
    loop {
        tokio::select! {
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data }) => out(&unmark.feed(&data)),
                Some(ChannelMsg::ExtendedData { data, .. }) => stderr.extend_from_slice(&data),
                Some(ChannelMsg::ExitStatus { exit_status }) => status = exit_status as i32,
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            _ = &mut cancel => {
                let _ = channel.signal(Sig::TERM).await;
                let _ = channel.close().await;
                break;
            }
        }
    }
    out(&unmark.finish());
    Ok(Ran {
        status,
        stdout: String::new(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Drops whatever the rc files print before the mark, across however many chunks it spans.
#[derive(Default)]
struct Unmark {
    seen: bool,
    held: Vec<u8>,
}

impl Unmark {
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.seen {
            return bytes.to_vec();
        }
        self.held.extend_from_slice(bytes);
        let mark = format!("{MARK}\n");
        match self.held.windows(mark.len()).position(|w| w == mark.as_bytes()) {
            Some(at) => {
                self.seen = true;
                self.held.split_off(at + mark.len())
            }
            None => Vec::new(),
        }
    }

    /// Output with no mark at all is passed on whole, since then the shell never ran it.
    fn finish(self) -> Vec<u8> {
        if self.seen {
            Vec::new()
        } else {
            self.held
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn what_the_rc_files_print_is_dropped_even_when_the_mark_is_split() {
        let output = format!("Welcome to Ubuntu\n{}\n{{\"a\":1}}\n", super::MARK);
        let mut unmark = super::Unmark::default();
        let mut kept = Vec::new();
        for chunk in output.as_bytes().chunks(7) {
            kept.extend(unmark.feed(chunk));
        }
        kept.extend(unmark.finish());
        assert_eq!(String::from_utf8(kept).unwrap(), "{\"a\":1}\n");
    }

    use std::sync::{Arc, Mutex};

    use russh::keys::ssh_key::rand_core::OsRng;
    use russh::keys::PrivateKey;
    use russh::server::{self, Auth, Msg, Session};
    use russh::{client, Channel, ChannelId, CryptoVec, Sig};
    use tokio::sync::{mpsc, oneshot};

    /// Prints two lines after the mark and then waits, like `herdr agent follow`.
    struct Follow {
        signals: Arc<Mutex<Vec<Sig>>>,
    }

    impl server::Handler for Follow {
        type Error = russh::Error;

        async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }

        async fn channel_open_session(&mut self, _: Channel<Msg>, _: &mut Session) -> Result<bool, Self::Error> {
            Ok(true)
        }

        async fn exec_request(&mut self, channel: ChannelId, _: &[u8], session: &mut Session) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            let handle = session.handle();
            let lines = [format!("motd\n{}\n{{\"t\":", super::MARK), "\"hello\"}\n".into(), "{\"t\":\"state\"}\n".into()];
            tokio::spawn(async move {
                for line in lines {
                    let _ = handle.data(channel, CryptoVec::from(line.into_bytes())).await;
                }
            });
            Ok(())
        }

        async fn signal(&mut self, _: ChannelId, signal: Sig, _: &mut Session) -> Result<(), Self::Error> {
            self.signals.lock().unwrap().push(signal);
            Ok(())
        }
    }

    struct Trusting;

    impl client::Handler for Trusting {
        type Error = russh::Error;

        async fn check_server_key(&mut self, _: &russh::keys::PublicKey) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn a_stream_hands_over_its_output_as_it_comes_and_stops_when_cancelled() {
        let config = Arc::new(server::Config {
            keys: vec![PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap()],
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let signals = Arc::new(Mutex::new(Vec::new()));
        let follow = Follow { signals: signals.clone() };
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = server::run_stream(config, socket, follow).await;
        });
        let mut handle = client::connect(Arc::new(client::Config::default()), address, Trusting).await.unwrap();
        assert!(handle.authenticate_none("tester").await.unwrap().success());

        let (chunks, mut arrived) = mpsc::unbounded_channel();
        let (cancel, cancelled) = oneshot::channel();
        let stream = tokio::spawn(async move {
            super::exec(&handle, "herdr agent follow 1", |bytes| drop(chunks.send(bytes.to_vec())), cancelled).await
        });
        let mut seen = Vec::new();
        while !seen.ends_with(b"{\"t\":\"state\"}\n") {
            seen.extend(arrived.recv().await.unwrap());
        }
        assert_eq!(String::from_utf8(seen).unwrap(), "{\"t\":\"hello\"}\n{\"t\":\"state\"}\n");
        cancel.send(()).unwrap();
        let ran = stream.await.unwrap().unwrap();
        assert_eq!(ran.stdout, "");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(matches!(signals.lock().unwrap()[..], [Sig::TERM]));
    }
}
