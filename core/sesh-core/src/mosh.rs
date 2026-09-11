//! One mosh Session: start a server over ssh, then run rmosh's client over UDP.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use mosh::handshake::Handshake;
use mosh::launch::{announce_prefix, server_command};
use mosh::opts::Opts;
use mosh_client::prediction::DisplayPreference;
use mosh_client::session::{Event, Options};
use russh::ChannelMsg;
use tokio::sync::oneshot;

use crate::flags::{self, MoshFlags, Predict, RemoteIp};
use crate::session::{Command, Context, Events, Session, State};
use crate::ssh::{self, Credentials};

/// The locale both ends are told to use. iOS gives a process no environment to read one
/// from, and the two ends must agree on character widths or they disagree on the screen.
const LOCALE: &str = "en_US.UTF-8";

#[derive(Default)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub key: Option<String>,
    pub key_passphrase: Option<String>,
    pub known_hosts: PathBuf,
    pub remote_command: Option<String>,
    pub extra_flags: String,
    pub cols: u16,
    pub rows: u16,
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
    settle_environment();

    let flags = flags::parse_mosh(&config.extra_flags)?;
    let mut ssh_flags = flags.ssh.clone();
    ssh_flags.ipv4_only |= flags.ipv4_only;
    ssh_flags.ipv6_only |= flags.ipv6_only;
    if ssh_flags.jump.is_some() {
        return Err("mosh cannot reach a host through -J: its datagrams go direct".into());
    }
    let user = ssh_flags.user.clone().unwrap_or_else(|| config.user.clone());
    let port = ssh_flags.port.unwrap_or(config.port);

    events.state(State::Connecting, "");
    let client_config = Arc::new(ssh::client_config(&ssh_flags));
    let (mut handle, address) =
        ssh::connect_any(&client_config, &config.host, port, &ssh_flags, || ssh::Handler {
            events: events.clone(),
            answers: answers.clone(),
            known_hosts: config.known_hosts.clone(),
            host: config.host.clone(),
            port,
        })
        .await?;
    let credentials = Credentials {
        host: &config.host,
        key: config.key.as_deref(),
        key_passphrase: config.key_passphrase.as_deref(),
        password: config.password.as_deref(),
    };
    ssh::authenticate(&mut handle, &user, &credentials, &ssh_flags, &events, &answers).await?;

    events.state(State::Bootstrapping, "");
    let mut found = Handshake::default();
    if flags.remote_ip == RemoteIp::Local {
        found.ip = Some(address.ip().to_string());
    }
    bootstrap(&handle, &config, &flags, &events, &mut found).await?;
    drop(handle);

    let (udp_port, key) = found
        .connect
        .clone()
        .ok_or("the far end never said MOSH CONNECT: is mosh-server installed?")?;
    let target = found
        .target()
        .ok_or("the far end never announced its address; try --experimental-remote-ip=local")?
        .to_string();

    let sink = events.clone();
    let options = Options {
        cols: config.cols.max(1) as i32,
        rows: config.rows.max(1) as i32,
        predict: match flags.predict {
            Predict::Adaptive => DisplayPreference::Adaptive,
            Predict::Always => DisplayPreference::Always,
            Predict::Never => DisplayPreference::Never,
            Predict::Experimental => DisplayPreference::Experimental,
        },
        predict_overwrite: false,
        terminfo: false,
    };

    // The client owns non-Send state, so it is built on the thread that will run it.
    let (ready, started) = oneshot::channel();
    let mut worker = tokio::task::spawn_blocking(move || {
        let session = match mosh_client::Session::new(&key, &target, udp_port, options) {
            Ok(session) => {
                let _ = ready.send(Ok(session.handle()));
                session
            }
            Err(e) => {
                let _ = ready.send(Err(e.to_string()));
                return Err(e.to_string());
            }
        };
        session
            .run(|bytes| {
                sink.output(bytes);
                Ok(())
            })
            .map_err(|e| e.to_string())
    });

    let client = started
        .await
        .map_err(|_| "the mosh client did not start".to_string())??;
    events.state(State::Connected, "");

    loop {
        tokio::select! {
            finished = &mut worker => {
                return finished.map_err(|e| format!("the mosh client stopped: {e}"))?;
            }
            command = commands.recv() => match command {
                Some(Command::Write(bytes)) => client.send(Event::Bytes(bytes)),
                Some(Command::Resize(cols, rows)) => {
                    client.send(Event::Resize(cols.max(1) as i32, rows.max(1) as i32))
                }
                Some(Command::Close) | None => {
                    client.send(Event::Quit);
                    return worker.await.map_err(|e| format!("the mosh client stopped: {e}"))?;
                }
            },
        }
    }
}

/// Run `mosh-server` on the far end and read its announcement, showing the user whatever
/// the remote login says on the way.
async fn bootstrap(
    handle: &russh::client::Handle<ssh::Handler>,
    config: &Config,
    flags: &MoshFlags,
    events: &Arc<dyn Events>,
    found: &mut Handshake,
) -> Result<(), String> {
    let command = command_line(config, flags)?;
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("starting mosh-server: {e}"))?;
    channel
        .request_pty(
            true,
            "xterm-256color",
            config.cols.max(1) as u32,
            config.rows.max(1) as u32,
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| format!("requesting a pty: {e}"))?;
    channel
        .exec(true, command.as_bytes())
        .await
        .map_err(|e| format!("starting mosh-server: {e}"))?;

    let mut pending = Vec::new();
    while !found.complete() {
        match channel.wait().await {
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                pending.extend_from_slice(&data);
            }
            Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => break,
            Some(_) => continue,
        }
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            match found.line(line.trim_end_matches(['\n', '\r']))? {
                Some(text) if !text.is_empty() => events.output(format!("{text}\r\n").as_bytes()),
                _ => {}
            }
        }
    }
    Ok(())
}

/// What the far end is asked to run: the server, and under `remote` the announce that
/// tells us the address ssh reached it on.
fn command_line(config: &Config, flags: &MoshFlags) -> Result<String, String> {
    let opts = Opts {
        server: flags.server.clone(),
        userhost: Some(config.host.clone()),
        port_request: flags.ports.map(|(low, high)| {
            if low == high {
                low.to_string()
            } else {
                format!("{low}:{high}")
            }
        }),
        command: flags::split(config.remote_command.as_deref().unwrap_or(""))?,
        ..Default::default()
    };
    let announce = match flags.remote_ip {
        RemoteIp::Remote => announce_prefix(),
        RemoteIp::Local => String::new(),
    };
    let locale = [("LANG".to_string(), LOCALE.to_string())];
    Ok(format!("{announce}{}", server_command(&opts, &locale)))
}

/// mosh's own code reads the terminal type and the locale from the environment, which an
/// iOS process does not have. Give it one, once.
fn settle_environment() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("TERM", "xterm-256color");
        for name in [LOCALE, "C.UTF-8", "UTF-8"] {
            if mosh_sys::locale::set_locale(name) && mosh_sys::is_utf8_locale() {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(extra: &str, remote_command: &str) -> String {
        let config = Config {
            host: "example.com".into(),
            remote_command: Some(remote_command.into()),
            ..Default::default()
        };
        command_line(&config, &flags::parse_mosh(extra).unwrap()).unwrap()
    }

    #[test]
    fn the_server_command_carries_the_locale_and_the_announce() {
        settle_environment();
        let command = line("", "");
        assert!(command.starts_with("sh -c '[ -n \"$SSH_CONNECTION\""), "{command}");
        assert!(command.contains("mosh-server new -c 256 -s"), "{command}");
        assert!(command.contains("-l LANG=en_US.UTF-8"), "{command}");
    }

    #[test]
    fn a_local_address_needs_no_announce() {
        let command = line("--experimental-remote-ip=local -p 60001 --server=/opt/bin/mosh-server", "tmux attach");
        assert!(command.starts_with("/opt/bin/mosh-server new"), "{command}");
        assert!(command.contains("-p 60001"), "{command}");
        assert!(command.ends_with("-- tmux attach"), "{command}");
    }

    #[test]
    fn the_handshake_survives_a_pty_and_a_login_banner() {
        let mut found = Handshake::default();
        let mut shown = Vec::new();
        for line in "Last login: today\r\nMOSH CONNECT 60001 iA4fBYJ0bLZ0CnyQLXTFLg\r\n".split_inclusive('\n') {
            if let Some(text) = found.line(line.trim_end_matches(['\n', '\r'])).unwrap() {
                shown.push(text.to_string());
            }
        }
        assert_eq!(shown, ["Last login: today"]);
        assert_eq!(found.connect, Some((60001, "iA4fBYJ0bLZ0CnyQLXTFLg".into())));
    }
}
