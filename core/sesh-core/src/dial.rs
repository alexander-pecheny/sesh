//! Reaching a Host by its alias in the user's ssh config, as ssh(1) would: directly, through
//! a `ProxyCommand` or along a `ProxyJump` chain, signing in with the agent's keys and the
//! configured identity files.

use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};

use russh::client::{self, AuthResult};
use russh::keys::agent::client::AgentClient;
use russh::keys::ssh_key::{Algorithm, Certificate, PublicKey};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg};
use russh::{cipher, compression, kex, mac, MethodKind, Preferred};
use tokio::io::AsyncReadExt;

use crate::known_hosts::KnownHosts;
use crate::session::{ask, Answers, Events, State};
use crate::ssh::{self, Forward, Handler};
use crate::ssh_config::{self, wildcard, Host, Method, Proxy};

/// A chain of jumps longer than this is taken for a loop.
const MAX_HOPS: usize = 8;
/// How much of a proxy command's stderr is kept to explain its failure.
const STDERR_KEPT: usize = 4096;

/// A connection and what carries it.
pub struct Dialed {
    pub handle: client::Handle<Handler>,
    /// Where the Host was reached when nothing stood between, so mosh's datagrams can go there.
    pub direct: Option<IpAddr>,
    /// The target's `SetEnv`, for every command run on the connection.
    pub env: Vec<(String, String)>,
    pub carriers: Carriers,
}

/// A proxy command and the hops before, which must live as long as the connection.
#[derive(Default)]
pub struct Carriers {
    _held: Vec<Box<dyn std::any::Any + Send + Sync>>,
}

pub async fn dial(alias: &str, events: &Arc<dyn Events>, answers: &Arc<Answers>) -> Result<Dialed, String> {
    let host = resolve(alias, None, None).await?;
    events.state(State::Connecting, "");
    reach(host, events.clone(), answers.clone(), 0).await
}

/// `alias`'s options, with a jump hop's own user and port over what the config says.
async fn resolve(alias: &str, user: Option<String>, port: Option<u16>) -> Result<Host, String> {
    let alias = alias.to_string();
    tokio::task::spawn_blocking(move || {
        let mut host = ssh_config::resolve(&alias, &local())?;
        host.user = user.unwrap_or(host.user);
        host.port = port.unwrap_or(host.port);
        Ok(host)
    })
    .await
    .map_err(|e| e.to_string())?
}

type Reached = Pin<Box<dyn Future<Output = Result<Dialed, String>> + Send>>;

fn reach(host: Host, events: Arc<dyn Events>, answers: Arc<Answers>, depth: usize) -> Reached {
    Box::pin(async move {
        if depth > MAX_HOPS {
            return Err(format!("{}: ProxyJump goes round in a loop", host.alias));
        }
        let config = Arc::new(client_config(&host)?);
        let handler = handler(&host, &events, &answers);
        let connecting = async {
            match &host.proxy {
                Proxy::Direct => tcp(&host, config, handler).await.map(|(handle, ip)| (handle, Some(ip), Vec::<Box<dyn std::any::Any + Send + Sync>>::new())),
                Proxy::Command(command) => {
                    let (handle, child) = proxied(&host, command, config, handler).await?;
                    Ok((handle, None, vec![Box::new(child) as Box<dyn std::any::Any + Send + Sync>]))
                }
                Proxy::Jump(hops) => {
                    let first = &hops[0];
                    let mut via = reach(resolve(&first.host, first.user.clone(), first.port).await?, events.clone(), answers.clone(), depth + 1).await?;
                    for hop in &hops[1..] {
                        let mut next = resolve(&hop.host, hop.user.clone(), hop.port).await?;
                        next.proxy = Proxy::Direct;
                        via = through(via, next, &events, &answers).await?;
                    }
                    let handle = tunnel(&via, &host, config, handler).await?;
                    Ok((handle, None, vec![Box::new(via) as Box<dyn std::any::Any + Send + Sync>]))
                }
            }
        };
        let (mut handle, direct, carriers) = match host.connect_timeout {
            Some(limit) => tokio::time::timeout(limit, connecting)
                .await
                .map_err(|_| format!("{}: connection timed out", host.alias))??,
            None => connecting.await?,
        };
        authenticate(&mut handle, &host, &events, &answers).await?;
        Ok(Dialed { handle, direct, env: host.set_env.clone(), carriers: Carriers { _held: carriers } })
    })
}

/// The next hop, reached through the one before it.
async fn through(via: Dialed, host: Host, events: &Arc<dyn Events>, answers: &Arc<Answers>) -> Result<Dialed, String> {
    let config = Arc::new(client_config(&host)?);
    let mut handle = tunnel(&via, &host, config, handler(&host, events, answers)).await?;
    authenticate(&mut handle, &host, events, answers).await?;
    Ok(Dialed { handle, direct: None, env: host.set_env.clone(), carriers: Carriers { _held: vec![Box::new(via)] } })
}

async fn tunnel(via: &Dialed, host: &Host, config: Arc<client::Config>, handler: Handler) -> Result<client::Handle<Handler>, String> {
    let stream = via
        .handle
        .channel_open_direct_tcpip(&host.host_name, host.port as u32, "127.0.0.1", 0)
        .await
        .map_err(|e| format!("{}: reaching it through the jump host: {e}", host.alias))?
        .into_stream();
    client::connect_stream(config, stream, handler).await.map_err(|e| format!("{}: {e}", host.alias))
}

fn handler(host: &Host, events: &Arc<dyn Events>, answers: &Arc<Answers>) -> Handler {
    Handler {
        events: events.clone(),
        answers: answers.clone(),
        known_hosts: KnownHosts {
            files: host.known_hosts.clone(),
            global: host.global_known_hosts.clone(),
            strict: host.strict,
            batch: host.batch_mode,
        },
        host: host.host_key_alias.clone().unwrap_or_else(|| host.host_name.clone()),
        port: host.port,
        forward: host.forward_agent.clone().map(Forward::Socket),
    }
}

/// Each address the name resolves to, in turn, until one answers.
async fn tcp(host: &Host, config: Arc<client::Config>, handler: Handler) -> Result<(client::Handle<Handler>, IpAddr), String> {
    let addresses = tokio::net::lookup_host((host.host_name.as_str(), host.port))
        .await
        .map_err(|e| format!("{}: {e}", host.host_name))?
        .filter(|address| match host.family {
            ssh_config::Family::Any => true,
            ssh_config::Family::Inet => address.is_ipv4(),
            ssh_config::Family::Inet6 => address.is_ipv6(),
        });
    let mut failure = format!("{}: no address of the requested family", host.host_name);
    for address in addresses {
        let socket = match tokio::net::TcpStream::connect(address).await {
            Ok(socket) => socket,
            Err(e) => {
                failure = format!("{}: {e}", host.host_name);
                continue;
            }
        };
        let _ = socket.set_nodelay(true);
        match client::connect_stream(config.clone(), socket, handler.clone()).await {
            Ok(handle) => return Ok((handle, address.ip())),
            Err(e) => failure = format!("{}: {e}", host.host_name),
        }
    }
    Err(failure)
}

/// The connection over a proxy command's stdin and stdout, run as ssh(1) runs it.
async fn proxied(
    host: &Host,
    command: &str,
    config: Arc<client::Config>,
    handler: Handler,
) -> Result<(client::Handle<Handler>, tokio::process::Child), String> {
    let environment = login_environment();
    let shell = environment.iter().find(|(name, _)| name == "SHELL").map_or("/bin/sh", |(_, value)| value.as_str());
    let mut child = tokio::process::Command::new(shell)
        .arg("-c")
        .arg(format!("exec {command}"))
        .env_clear()
        .envs(environment.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{}: starting the ProxyCommand: {e}", host.alias))?;
    let said = Arc::new(Mutex::new(Vec::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let said = said.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 1024];
            while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
                let mut kept = said.lock().unwrap();
                kept.extend_from_slice(&chunk[..read]);
                let excess = kept.len().saturating_sub(STDERR_KEPT);
                kept.drain(..excess);
            }
        });
    }
    let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
        return Err(format!("{}: the ProxyCommand has no pipes", host.alias));
    };
    match client::connect_stream(config, tokio::io::join(stdout, stdin), handler).await {
        Ok(handle) => Ok((handle, child)),
        Err(e) => {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await;
            let said = String::from_utf8_lossy(&said.lock().unwrap()).trim().to_string();
            let why = if said.is_empty() { e.to_string() } else { said };
            Err(format!("{}: the ProxyCommand failed: {why}", host.alias))
        }
    }
}

fn client_config(host: &Host) -> Result<client::Config, String> {
    let defaults = Preferred::DEFAULT;
    let algorithms = &host.algorithms;
    let all_keys: Vec<Algorithm> = [
        "ssh-ed25519",
        "ecdsa-sha2-nistp256",
        "ecdsa-sha2-nistp384",
        "ecdsa-sha2-nistp521",
        "rsa-sha2-512",
        "rsa-sha2-256",
        "ssh-rsa",
    ]
    .iter()
    .filter_map(|name| Algorithm::new(name).ok())
    .collect();
    let preferred = Preferred {
        kex: choose("KexAlgorithms", algorithms.kex.as_deref(), &defaults.kex, &own(kex::ALL_KEX_ALGORITHMS))?.into(),
        key: choose("HostKeyAlgorithms", algorithms.host_key.as_deref(), &defaults.key, &all_keys)?.into(),
        cipher: choose("Ciphers", algorithms.ciphers.as_deref(), &defaults.cipher, &own(cipher::ALL_CIPHERS))?.into(),
        mac: choose("MACs", algorithms.macs.as_deref(), &defaults.mac, &own(mac::ALL_MAC_ALGORITHMS))?.into(),
        compression: match host.compression {
            true => vec![compression::ZLIB_LEGACY, compression::ZLIB, compression::NONE],
            false => vec![compression::NONE],
        }
        .into(),
    };
    Ok(client::Config {
        keepalive_interval: host.alive_interval,
        keepalive_max: host.alive_count_max as usize,
        nodelay: true,
        preferred,
        ..Default::default()
    })
}

fn own<T: Copy>(all: &[&T]) -> Vec<T> {
    all.iter().map(|name| **name).collect()
}

/// An algorithm list as ssh_config(5) writes it: a list of its own, or `+` to append to the
/// defaults, `-` to remove from them and `^` to put first, each entry a wildcard pattern.
fn choose<T: Clone + Named>(option: &str, spec: Option<&str>, defaults: &[T], known: &[T]) -> Result<Vec<T>, String> {
    let Some(spec) = spec else { return Ok(defaults.to_vec()) };
    let (how, list) = match spec.chars().next() {
        Some(c @ ('+' | '-' | '^')) => (c, &spec[1..]),
        _ => (' ', spec),
    };
    let patterns: Vec<&str> = list.split(',').map(str::trim).collect();
    let named = |item: &T| patterns.iter().any(|pattern| wildcard(pattern, &item.name()));
    let listed = |items: &[T]| -> Vec<T> {
        patterns.iter().flat_map(|pattern| items.iter().filter(|item| wildcard(pattern, &item.name()))).cloned().collect()
    };
    let chosen: Vec<T> = match how {
        '+' => defaults.iter().cloned().chain(listed(known).into_iter().filter(|item| !defaults.iter().any(|d| d.name() == item.name()))).collect(),
        '-' => defaults.iter().filter(|item| !named(item)).cloned().collect(),
        '^' => {
            let first = listed(known);
            first.iter().cloned().chain(defaults.iter().filter(|item| !first.iter().any(|f| f.name() == item.name())).cloned()).collect()
        }
        _ => listed(known),
    };
    let mut unique: Vec<T> = Vec::new();
    for item in chosen {
        if !unique.iter().any(|known| known.name() == item.name()) {
            unique.push(item);
        }
    }
    match unique.is_empty() {
        true => Err(format!("{option} {spec} names no algorithm Sesh supports")),
        false => Ok(unique),
    }
}

trait Named {
    fn name(&self) -> String;
}

impl Named for kex::Name {
    fn name(&self) -> String {
        self.as_ref().to_string()
    }
}

impl Named for cipher::Name {
    fn name(&self) -> String {
        self.as_ref().to_string()
    }
}

impl Named for mac::Name {
    fn name(&self) -> String {
        self.as_ref().to_string()
    }
}

impl Named for Algorithm {
    fn name(&self) -> String {
        self.as_str().to_string()
    }
}

async fn authenticate(handle: &mut client::Handle<Handler>, host: &Host, events: &Arc<dyn Events>, answers: &Arc<Answers>) -> Result<(), String> {
    events.state(State::Authenticating, "");
    let user = host.user.as_str();
    let failed = |e: russh::Error| format!("authenticating as {user}: {e}");
    let mut result = handle.authenticate_none(user).await.map_err(failed)?;
    for method in &host.methods {
        if result.success() {
            break;
        }
        let kind = match method {
            Method::PublicKey => MethodKind::PublicKey,
            Method::KeyboardInteractive => MethodKind::KeyboardInteractive,
            Method::Password => MethodKind::Password,
        };
        if !ssh::offers(&result, kind) || (host.batch_mode && *method != Method::PublicKey) {
            continue;
        }
        result = match method {
            Method::PublicKey => publickey(handle, host, result, events, answers).await?,
            Method::KeyboardInteractive => {
                for _ in 0..3 {
                    result = ssh::keyboard_interactive(handle, user, events, answers).await?;
                    if result.success() || !ssh::offers(&result, kind) {
                        break;
                    }
                }
                result
            }
            Method::Password => {
                for _ in 0..3 {
                    let asked = vec![ssh::prompt(&format!("{user}@{}'s password:", host.host_name), false)];
                    let given = ask(events, answers, "Password", "", asked).await?;
                    result = handle.authenticate_password(user, given.into_iter().next().unwrap_or_default()).await.map_err(failed)?;
                    if result.success() || !ssh::offers(&result, kind) {
                        break;
                    }
                }
                result
            }
        };
    }
    match result {
        AuthResult::Success => Ok(()),
        AuthResult::Failure { remaining_methods, .. } => {
            let left: Vec<&str> = remaining_methods.iter().map(|m| <&str>::from(m)).collect();
            Err(format!("{user}@{}: permission denied ({})", host.alias, left.join(",")))
        }
    }
}

/// The agent's keys the config names, then its others unless `IdentitiesOnly`, then the
/// identity files it lacks, asking for a passphrase where one is needed.
async fn publickey(
    handle: &mut client::Handle<Handler>,
    host: &Host,
    mut result: AuthResult,
    events: &Arc<dyn Events>,
    answers: &Arc<Answers>,
) -> Result<AuthResult, String> {
    let user = host.user.as_str();
    let mut agent = match &host.identity_agent {
        Some(socket) => AgentClient::connect_uds(socket).await.ok(),
        None => None,
    };
    let held = match agent.as_mut() {
        Some(agent) => agent.request_identities().await.unwrap_or_default(),
        None => Vec::new(),
    };
    let hash = rsa_hash(host.algorithms.pubkey.as_deref(), handle).await;
    let files: Vec<(&PathBuf, Option<PublicKey>)> = host.identity_files.iter().map(|file| (file, public_of(file))).collect();
    let named: Vec<&PublicKey> = files.iter().filter_map(|(_, key)| key.as_ref()).filter(|key| held.iter().any(|h| h.key_data() == key.key_data())).collect();
    let others: Vec<&PublicKey> = match host.identities_only {
        true => Vec::new(),
        false => held.iter().filter(|key| !named.iter().any(|n| n.key_data() == key.key_data())).collect(),
    };
    let mut tried = Vec::new();
    for key in named.into_iter().chain(others) {
        let Some(agent) = agent.as_mut() else { break };
        tried.push(key.key_data().clone());
        result = handle
            .authenticate_publickey_with(user, key.clone(), hash, agent)
            .await
            .map_err(|e| format!("signing with the agent: {e:?}"))?;
        if result.success() || !ssh::offers(&result, MethodKind::PublicKey) {
            return Ok(result);
        }
    }
    for (file, public) in &files {
        if public.as_ref().is_some_and(|key| tried.contains(key.key_data())) || !file.exists() {
            continue;
        }
        let Some(key) = private_key(file, host.batch_mode, events, answers).await else { continue };
        let key = Arc::new(key);
        for certificate in certificates(file, &host.certificate_files) {
            if certificate.public_key() == key.public_key().key_data() {
                result = handle.authenticate_openssh_cert(user, key.clone(), certificate).await.map_err(|e| e.to_string())?;
                if result.success() || !ssh::offers(&result, MethodKind::PublicKey) {
                    return Ok(result);
                }
            }
        }
        result = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key.clone(), hash))
            .await
            .map_err(|e| e.to_string())?;
        if result.success() {
            if let (true, Some(agent)) = (host.add_keys_to_agent, agent.as_mut()) {
                let _ = agent.add_identity(&key, &[]).await;
            }
        }
        if result.success() || !ssh::offers(&result, MethodKind::PublicKey) {
            return Ok(result);
        }
    }
    Ok(result)
}

/// The public half of an identity file: its `.pub`, or the key itself when unencrypted.
fn public_of(file: &Path) -> Option<PublicKey> {
    let mut public = file.as_os_str().to_owned();
    public.push(".pub");
    russh::keys::load_public_key(PathBuf::from(public))
        .ok()
        .or_else(|| russh::keys::load_secret_key(file, None).ok().map(|key| key.public_key().clone()))
}

async fn private_key(file: &Path, batch: bool, events: &Arc<dyn Events>, answers: &Arc<Answers>) -> Option<PrivateKey> {
    match russh::keys::load_secret_key(file, None) {
        Ok(key) => return Some(key),
        Err(russh::keys::Error::KeyIsEncrypted) if !batch => {}
        Err(_) => return None,
    }
    for _ in 0..3 {
        let asked = vec![ssh::prompt(&format!("Enter passphrase for key '{}':", file.display()), false)];
        let given = ask(events, answers, "Passphrase", "", asked).await.ok()?;
        let passphrase = given.into_iter().next().filter(|p| !p.is_empty())?;
        if let Ok(key) = russh::keys::load_secret_key(file, Some(&passphrase)) {
            return Some(key);
        }
    }
    None
}

/// `CertificateFile`s, and the `-cert.pub` beside the identity file as ssh(1) looks for it.
fn certificates(file: &Path, configured: &[PathBuf]) -> Vec<Certificate> {
    let mut beside = file.as_os_str().to_owned();
    beside.push("-cert.pub");
    configured
        .iter()
        .cloned()
        .chain([PathBuf::from(beside)])
        .filter_map(|path| russh::keys::load_openssh_certificate(path).ok())
        .collect()
}

pub async fn rsa_hash(accepted: Option<&str>, handle: &client::Handle<Handler>) -> Option<HashAlg> {
    match accepted {
        Some(list) if list.contains("rsa-sha2-512") => Some(HashAlg::Sha512),
        Some(list) if list.contains("rsa-sha2-256") => Some(HashAlg::Sha256),
        Some(list) if !list.starts_with(['+', '-', '^']) => None,
        _ => handle.best_supported_rsa_hash().await.ok().flatten().flatten(),
    }
}

/// What the config reader asks of this machine, from the user's login environment.
fn local() -> ssh_config::Local<'static> {
    static ENV: fn(&str) -> Option<String> = |name| login_environment().iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    static EXEC: fn(&str) -> bool = |command| {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .env_clear()
            .envs(login_environment().iter().cloned())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    static RESOLVES: fn(&str) -> bool = |name| std::net::ToSocketAddrs::to_socket_addrs(&(name, 22)).is_ok_and(|mut found| found.next().is_some());
    let env = |name: &str| ENV(name);
    let home = env("HOME").map(PathBuf::from).unwrap_or_default();
    ssh_config::Local {
        user: env("USER").or_else(|| env("LOGNAME")).unwrap_or_default(),
        host: hostname(),
        // SAFETY: getuid cannot fail.
        uid: unsafe { libc::getuid() },
        home,
        env: &ENV,
        exec: &EXEC,
        resolves: &RESOLVES,
    }
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer outlives the call and its length is passed.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    if status == 0 { String::from_utf8_lossy(&buffer[..end]).into_owned() } else { String::new() }
}

/// The environment the user's terminal would have: an app started from the Dock gets a bare
/// one, without the PATH a proxy command such as `aws ssm` needs. Read once.
pub fn login_environment() -> &'static [(String, String)] {
    static READ: OnceLock<Vec<(String, String)>> = OnceLock::new();
    READ.get_or_init(|| {
        const MARK: &str = "--sesh-environment--";
        let own: Vec<(String, String)> = std::env::vars().collect();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let read = std::process::Command::new(&shell)
            .args(["-l", "-i", "-c", &format!("printf '{MARK}'; env -0")])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let Ok(read) = read else { return own };
        let text = String::from_utf8_lossy(&read.stdout);
        let Some((_, env)) = text.split_once(MARK) else { return own };
        let mut found: Vec<(String, String)> =
            env.split('\0').filter_map(|pair| pair.split_once('=')).map(|(n, v)| (n.to_string(), v.to_string())).collect();
        // The app's own variables, such as the agent's socket, stay where the shell has none.
        for (name, value) in own {
            if !found.iter().any(|(known, _)| *known == name) {
                found.push((name, value));
            }
        }
        found
    })
}

#[cfg(test)]
mod tests;
