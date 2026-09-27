//! A Link: an SSH connection with no terminal, on which Projects runs short commands.

use std::path::PathBuf;
use std::sync::Arc;

use russh::client;
use russh::ChannelMsg;

use crate::flags::{self, Transport};
use crate::session::{in_login_shell, Command, Context, Events, Session, State};
use crate::ssh::Dialer;

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

    loop {
        match commands.recv().await {
            Some(Command::Run(id, command)) => {
                let (handle, events) = (handle.clone(), events.clone());
                tokio::spawn(async move {
                    let ran = run(&handle, &command).await.unwrap_or_else(|error| Ran {
                        status: -1,
                        stdout: String::new(),
                        stderr: error,
                    });
                    events.ran(id, &ran);
                });
            }
            Some(Command::Close) | None => break,
            Some(_) => {}
        }
    }
    Ok(())
}

pub async fn run<H: client::Handler>(handle: &client::Handle<H>, command: &str) -> Result<Ran, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("the connection to the Host is gone: {e}"))?;
    channel
        .exec(true, in_login_shell(&format!("echo {MARK}; {command}")))
        .await
        .map_err(|e| format!("running a command: {e}"))?;
    let (mut stdout, mut stderr, mut status) = (Vec::new(), Vec::new(), -1);
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => status = exit_status as i32,
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    Ok(Ran {
        status,
        stdout: after_mark(&String::from_utf8_lossy(&stdout)).to_string(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

fn after_mark(output: &str) -> &str {
    match output.split_once(&format!("{MARK}\n")) {
        Some((_, rest)) => rest,
        None => output,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn what_the_rc_files_print_is_dropped() {
        let output = format!("Welcome to Ubuntu\n{}\n{{\"a\":1}}\n", super::MARK);
        assert_eq!(super::after_mark(&output), "{\"a\":1}\n");
    }
}
