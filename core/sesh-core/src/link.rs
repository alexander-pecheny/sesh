//! A Link: the one SSH connection a device keeps to a Host, on which it runs commands,
//! streams, pipes, uploads and terminals.

use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;

use russh::client;
use russh::{ChannelMsg, Sig};
use tokio::sync::oneshot;

use crate::dial::{self, Handler};
use crate::environment;
use crate::flags::MoshFlags;
use crate::session::{in_login_shell, Command, Context, Events, Session, State};
use crate::ssh_config::Host;
use crate::upload::{self, Pending};

/// Printed before the command, so whatever the rc files say is not taken for its output.
const MARK: &str = "--sesh-output--";

pub struct Ran {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A Link to the Host `spec` gives; `mosh` is how its terminals use mosh, nil for never.
pub fn connect<S>(spec: S, mosh: Option<MoshFlags>, events: Arc<dyn Events>) -> Session
where
    S: Future<Output = Result<Host, String>> + Send + 'static,
{
    Session::start(events, move |context| async move {
        let dialed = dial::dial(spec.await?, &context.events, &context.answers).await?;
        serve(dialed, mosh, context).await
    })
}

/// A Link's connection, lent to a terminal.
pub struct Lent {
    pub handle: Arc<client::Handle<Handler>>,
    /// Where the Host was reached when nothing stands between, so mosh's datagrams can go there.
    pub direct: Option<IpAddr>,
    /// The terminal's command, as the Host's shell runs it.
    pub command: String,
    pub env: Vec<(String, String)>,
    pub forward: bool,
    pub mosh: Option<MoshFlags>,
}

async fn serve(dialed: dial::Dialed, mosh: Option<MoshFlags>, context: Context) -> Result<(), String> {
    let Context { events, mut commands, .. } = context;
    let dial::Dialed { handle, direct, env, forward, carriers: _carriers } = dialed;
    let handle = Arc::new(handle);
    let env = Arc::new(env);
    let shell = Shell::read(&handle).await;
    events.state(State::Connected, "");
    if let (Some(_), Some(reap)) = (direct, crate::servers::reap()) {
        let (handle, command, env) = (handle.clone(), shell.wrap(&reap), env.clone());
        tokio::spawn(async move { run(&handle, &command, &env).await });
    }

    let mut uploads = Pending::default();
    let mut streams: HashMap<u32, oneshot::Sender<()>> = HashMap::new();
    loop {
        match commands.recv().await {
            Some(Command::Run(id, command)) => {
                let (handle, events, command, env) = (handle.clone(), events.clone(), shell.wrap(&command), env.clone());
                tokio::spawn(async move {
                    let ran = run(&handle, &command, &env).await.unwrap_or_else(failed);
                    events.ran(id, &ran);
                });
            }
            Some(Command::Stream(id, command)) => {
                let (cancel, cancelled) = oneshot::channel();
                streams.retain(|_, stream| !stream.is_closed());
                streams.insert(id, cancel);
                let (handle, events, command, env) = (handle.clone(), events.clone(), shell.wrap(&command), env.clone());
                tokio::spawn(async move {
                    let ran = exec(&handle, &command, &env, |bytes| events.chunk(id, bytes), cancelled)
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
            Some(Command::Lend(command, reply)) => {
                let command = shell.command(&command);
                let _ = reply.send(Lent { handle: handle.clone(), direct, command, env: (*env).clone(), forward, mosh: mosh.clone() });
            }
            Some(Command::Pipe(mut start)) => {
                start.command = shell.command(&start.command);
                tokio::spawn(crate::pipe::run(handle.clone(), start, direct.is_some()));
            }
            Some(Command::Close) | None => break,
            Some(_) => {}
        }
    }
    Ok(())
}

/// How a command reaches the Host. Each one used to start the login shell afresh, rc files and
/// all, which on a busy Host costs seconds; its environment is read once instead, and each
/// command runs in a plain `/bin/sh -c` with it.
enum Shell {
    Login,
    Plain(String),
}

impl Shell {
    async fn read<H: client::Handler>(handle: &client::Handle<H>) -> Self {
        let read = run(handle, &in_login_shell(&format!("echo {MARK}; env -0")), &[]).await;
        match read {
            Ok(Ran { status: 0, stdout, .. }) => Self::from_env(&stdout),
            _ => Self::Login,
        }
    }

    /// The shell that gives each command the environment `env -0` printed.
    fn from_env(env: &str) -> Self {
        let vars = environment::parse(env);
        if vars.iter().any(|(name, _)| name == "PATH") { Self::Plain(environment::exports(&vars)) } else { Self::Login }
    }

    fn wrap(&self, command: &str) -> String {
        self.command(&format!("echo {MARK}; {command}"))
    }

    /// `command` in `/bin/sh` with the login shell's environment, with no mark before its output.
    fn command(&self, command: &str) -> String {
        match self {
            Self::Login => in_login_shell(&environment::plain("", command)),
            Self::Plain(exports) => environment::plain(exports, command),
        }
    }
}

fn failed(error: String) -> Ran {
    Ran {
        status: -1,
        stdout: String::new(),
        stderr: error,
    }
}

pub async fn run<H: client::Handler>(handle: &client::Handle<H>, command: &str, env: &[(String, String)]) -> Result<Ran, String> {
    let mut stdout = Vec::new();
    let ran = exec(handle, command, env, |bytes| stdout.extend_from_slice(bytes), std::future::pending::<()>()).await?;
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
    env: &[(String, String)],
    mut out: impl FnMut(&[u8]),
    cancel: C,
) -> Result<Ran, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("the connection to the Host is gone: {e}"))?;
    for (name, value) in env {
        let _ = channel.set_env(false, name.as_str(), value.as_str()).await;
    }
    channel
        .exec(true, command)
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
    use super::*;

    #[test]
    fn a_command_gets_the_login_shells_environment_without_its_rc_files() {
        let shell = Shell::from_env("PATH=/usr/bin:/bin\0GREETING=it's a \"walrus\"\nhere\0SHLVL=7\0");
        let output = std::process::Command::new("sh")
            .args(["-c", &shell.wrap(r#"printf '%s|%s' "$GREETING" "$SHLVL""#)])
            .env("SHELL", "/bin/zsh")
            .output()
            .unwrap();
        let printed = String::from_utf8(output.stdout).unwrap();
        let (greeting, level) = printed.split_once('\n').unwrap().1.rsplit_once('|').unwrap();
        assert_eq!(greeting, "it's a \"walrus\"\nhere");
        assert_ne!(level, "7", "the shell's own variables are its to set");
    }

    #[test]
    fn an_environment_with_no_path_keeps_the_login_shell() {
        assert!(matches!(Shell::from_env("HOME=/home/u\0"), Shell::Login));
    }

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

    /// Prints two lines after the mark and then waits, like `sesh-mux agent follow`.
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
            super::exec(&handle, "sesh-mux agent follow 1", &[], |bytes| drop(chunks.send(bytes.to_vec())), cancelled).await
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
