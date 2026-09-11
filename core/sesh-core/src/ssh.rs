//! One SSH Session: connect, authenticate, run a shell on a pty, pump bytes.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use russh::client::{self, KeyboardInteractiveAuthResponse};
use russh::keys::ssh_key::{self, HashAlg, PublicKey};
use russh::keys::PrivateKeyWithHashAlg;
use russh::{ChannelMsg, MethodKind};
use tokio::sync::oneshot;

use crate::flags::{self, SshFlags};
use crate::known_hosts::{self, Verdict};
use crate::session::{ask, Answers, Command, Context, Events, Prompt, Session, State};

#[derive(Default)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub key: Option<String>,
    pub key_passphrase: Option<String>,
    pub known_hosts: PathBuf,
    pub term: String,
    pub remote_command: Option<String>,
    pub extra_flags: String,
    pub cols: u16,
    pub rows: u16,
    pub agent_forwarding: bool,
}

pub struct Handler {
    pub events: Arc<dyn Events>,
    pub answers: Arc<Answers>,
    pub known_hosts: PathBuf,
    pub host: String,
    pub port: u16,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKey) -> Result<bool, Self::Error> {
        let previous = match known_hosts::check(&self.known_hosts, &self.host, self.port, key) {
            Verdict::Known => return Ok(true),
            Verdict::Unknown => None,
            Verdict::Mismatch(old) => Some(old),
        };
        let (tx, rx) = oneshot::channel();
        *self.answers.host_key.lock().unwrap() = Some(tx);
        self.events.host_key(
            &known_hosts::fingerprint(key),
            previous.as_ref().map(known_hosts::fingerprint).as_deref(),
        );
        if !rx.await.unwrap_or(false) {
            return Ok(false);
        }
        known_hosts::accept(&self.known_hosts, &self.host, self.port, key, previous.as_ref())
            .map_err(|_| russh::Error::IO(std::io::ErrorKind::Other.into()))?;
        Ok(true)
    }
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
    let (events, answers) = (&events, &answers);
    let flags = flags::parse_ssh(&config.extra_flags)?;
    if config.agent_forwarding || flags.agent_forwarding {
        return Err("agent forwarding arrives in phase 5".into());
    }
    let user = flags.user.clone().unwrap_or_else(|| config.user.clone());
    let port = flags.port.unwrap_or(config.port);

    events.state(State::Connecting, "");
    let client_config = Arc::new(client_config(&flags));
    let handler = |host: &str, port| Handler {
        events: events.clone(),
        answers: answers.clone(),
        known_hosts: config.known_hosts.clone(),
        host: host.to_string(),
        port,
    };

    let mut _jump = None;
    let mut handle = match &flags.jump {
        None => {
            connect_any(&client_config, &config.host, port, &flags, || {
                handler(&config.host, port)
            })
            .await?
            .0
        }
        Some(jump) => {
            let (mut hop, _) = connect_any(&client_config, &jump.host, jump.port, &flags, || {
                handler(&jump.host, jump.port)
            })
            .await?;
            let hop_user = jump.user.clone().unwrap_or_else(|| user.clone());
            authenticate(&mut hop, &hop_user, &credentials(&config), &flags, events, answers).await?;
            let stream = hop
                .channel_open_direct_tcpip(&config.host, port as u32, "127.0.0.1", 0)
                .await
                .map_err(|e| format!("{} via {}: {e}", config.host, jump.host))?
                .into_stream();
            _jump = Some(hop);
            client::connect_stream(client_config, stream, handler(&config.host, port))
                .await
                .map_err(|e| format!("{}: {e}", config.host))?
        }
    };

    authenticate(&mut handle, &user, &credentials(&config), &flags, events, answers).await?;

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("opening a shell: {e}"))?;
    let (mut reader, writer) = channel.split();
    let (cols, rows) = (config.cols.max(1) as u32, config.rows.max(1) as u32);
    writer
        .request_pty(true, &config.term, cols, rows, 0, 0, &[])
        .await
        .map_err(|e| format!("requesting a pty: {e}"))?;
    match config.remote_command.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(command) => writer.exec(true, command).await,
        None => writer.request_shell(true).await,
    }
    .map_err(|e| format!("starting the remote command: {e}"))?;
    events.state(State::Connected, "");

    loop {
        tokio::select! {
            message = reader.wait() => match message {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    events.output(&data)
                }
                Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => break,
                Some(_) => {}
            },
            command = commands.recv() => match command {
                Some(Command::Write(bytes)) => writer
                    .data(&bytes[..])
                    .await
                    .map_err(|e| format!("writing to the remote: {e}"))?,
                Some(Command::Resize(cols, rows)) => {
                    let _ = writer.window_change(cols.max(1) as u32, rows.max(1) as u32, 0, 0).await;
                }
                Some(Command::Close) | None => break,
            },
        }
    }
    Ok(())
}

pub fn client_config(flags: &SshFlags) -> client::Config {
    let mut config = client::Config {
        keepalive_interval: flags.alive_interval.map(std::time::Duration::from_secs),
        nodelay: true,
        ..Default::default()
    };
    if let Some(max) = flags.alive_count_max {
        config.keepalive_max = max as usize;
    }
    if let Some(list) = &flags.host_key_algorithms {
        let algorithms: Vec<_> = list
            .split(',')
            .filter_map(|name| ssh_key::Algorithm::new(name.trim()).ok())
            .collect();
        if !algorithms.is_empty() {
            config.preferred.key = algorithms.into();
        }
    }
    config
}

/// A name can carry several addresses, and only some of them may be listening.
pub async fn connect_any(
    client_config: &Arc<client::Config>,
    host: &str,
    port: u16,
    flags: &SshFlags,
    mut handler: impl FnMut() -> Handler,
) -> Result<(client::Handle<Handler>, SocketAddr), String> {
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("{host}: {e}"))?
        .filter(|a| !(flags.ipv4_only && a.is_ipv6() || flags.ipv6_only && a.is_ipv4()))
        .collect();
    let mut failure = format!("{host}: no address of the requested family");
    for address in addresses {
        match client::connect(client_config.clone(), address, handler()).await {
            Ok(handle) => return Ok((handle, address)),
            Err(error) => failure = format!("{host}: {error}"),
        }
    }
    Err(failure)
}

pub struct Credentials<'a> {
    pub host: &'a str,
    pub key: Option<&'a str>,
    pub key_passphrase: Option<&'a str>,
    pub password: Option<&'a str>,
}

pub async fn authenticate(
    handle: &mut client::Handle<Handler>,
    user: &str,
    config: &Credentials<'_>,
    flags: &SshFlags,
    events: &Arc<dyn Events>,
    answers: &Arc<Answers>,
) -> Result<(), String> {
    events.state(State::Authenticating, "");
    let failed = |e: russh::Error| format!("authenticating as {user}: {e}");
    let mut result = handle.authenticate_none(user).await.map_err(failed)?;

    if let Some(pem) = config.key {
        if !result.success() && offers(&result, MethodKind::PublicKey) {
            let key = russh::keys::decode_secret_key(pem, config.key_passphrase)
                .map_err(|e| format!("reading the key: {e}"))?;
            let hash = rsa_hash(flags, handle).await;
            result = handle
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(failed)?;
        }
    }
    if let Some(password) = config.password {
        if !result.success() && offers(&result, MethodKind::Password) {
            result = handle.authenticate_password(user, password).await.map_err(failed)?;
        }
    }

    for _ in 0..3 {
        if result.success() {
            return Ok(());
        }
        if offers(&result, MethodKind::KeyboardInteractive) {
            result = keyboard_interactive(handle, user, events, answers).await?;
        } else if offers(&result, MethodKind::Password) {
            let given = ask(events, answers, "Password", "", vec![prompt("Password:", false)]).await?;
            result = handle
                .authenticate_password(user, given.first().cloned().unwrap_or_default())
                .await
                .map_err(failed)?;
        } else {
            return Err(format!("{user}@{}: no supported authentication method left", config.host));
        }
    }
    result
        .success()
        .then_some(())
        .ok_or_else(|| format!("{user}@{}: authentication failed", config.host))
}

async fn keyboard_interactive(
    handle: &mut client::Handle<Handler>,
    user: &str,
    events: &Arc<dyn Events>,
    answers: &Arc<Answers>,
) -> Result<client::AuthResult, String> {
    let failed = |e: russh::Error| format!("authenticating as {user}: {e}");
    let mut response = handle
        .authenticate_keyboard_interactive_start(user, None)
        .await
        .map_err(failed)?;
    loop {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(client::AuthResult::Success),
            KeyboardInteractiveAuthResponse::Failure {
                remaining_methods,
                partial_success,
            } => {
                return Ok(client::AuthResult::Failure {
                    remaining_methods,
                    partial_success,
                })
            }
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let asked: Vec<_> = prompts.iter().map(|p| prompt(&p.prompt, p.echo)).collect();
                let given = ask(events, answers, &name, &instructions, asked).await?;
                response = handle
                    .authenticate_keyboard_interactive_respond(given)
                    .await
                    .map_err(failed)?;
            }
        }
    }
}

fn credentials(config: &Config) -> Credentials<'_> {
    Credentials {
        host: &config.host,
        key: config.key.as_deref(),
        key_passphrase: config.key_passphrase.as_deref(),
        password: config.password.as_deref(),
    }
}

pub fn prompt(text: &str, echo: bool) -> Prompt {
    Prompt { prompt: text.to_string(), echo }
}

fn offers(result: &client::AuthResult, method: MethodKind) -> bool {
    match result {
        client::AuthResult::Success => false,
        client::AuthResult::Failure { remaining_methods, .. } => {
            remaining_methods.contains(&method)
        }
    }
}

pub async fn rsa_hash(flags: &SshFlags, handle: &client::Handle<Handler>) -> Option<HashAlg> {
    match flags.pubkey_accepted_algorithms.as_deref() {
        Some(list) if list.contains("rsa-sha2-512") => Some(HashAlg::Sha512),
        Some(list) if list.contains("rsa-sha2-256") => Some(HashAlg::Sha256),
        Some(_) => None,
        None => handle.best_supported_rsa_hash().await.ok().flatten().flatten(),
    }
}
