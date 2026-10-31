//! OpenSSH's client configuration, read the way ssh(1) reads it: `~/.ssh/config`, then
//! `/etc/ssh/ssh_config`, the first value obtained for each option winning, with `Host` and
//! `Match` blocks, `Include`, hostname canonicalization and the final pass it brings.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// What the config reader asks of the machine, kept apart so tests can stand in for it.
pub struct Local<'a> {
    pub home: PathBuf,
    pub user: String,
    /// The machine's own name, for `%l` and `%L`.
    pub host: String,
    pub uid: u32,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Runs a `Match exec` command and says whether it exited 0.
    pub exec: &'a dyn Fn(&str) -> bool,
    /// Whether a name resolves, for `CanonicalizeHostname`.
    pub resolves: &'a dyn Fn(&str) -> bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strict {
    Yes,
    Ask,
    AcceptNew,
    No,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Any,
    Inet,
    Inet6,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Proxy {
    Direct,
    /// Run through the user's shell, its stdin and stdout carrying the connection.
    Command(String),
    /// Each hop reached through the one before it, the first directly.
    Jump(Vec<Hop>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    PublicKey,
    KeyboardInteractive,
    Password,
}

/// Algorithm lists as written, `+`, `-` and `^` included; the dialer applies them to its own
/// defaults.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Algorithms {
    pub host_key: Option<String>,
    pub pubkey: Option<String>,
    pub kex: Option<String>,
    pub ciphers: Option<String>,
    pub macs: Option<String>,
}

/// Everything ssh(1) would use to reach one alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host {
    pub alias: String,
    pub host_name: String,
    pub port: u16,
    pub user: String,
    pub identity_files: Vec<PathBuf>,
    pub certificate_files: Vec<PathBuf>,
    pub identities_only: bool,
    /// The agent's socket, or nil for none.
    pub identity_agent: Option<PathBuf>,
    pub add_keys_to_agent: bool,
    /// The socket of the agent forwarded to the Host, or nil when none is.
    pub forward_agent: Option<PathBuf>,
    pub proxy: Proxy,
    pub alive_interval: Option<Duration>,
    pub alive_count_max: u32,
    pub strict: Strict,
    pub known_hosts: Vec<PathBuf>,
    pub global_known_hosts: Vec<PathBuf>,
    /// The name host keys are recorded under, when not the host name.
    pub host_key_alias: Option<String>,
    pub connect_timeout: Option<Duration>,
    pub family: Family,
    pub algorithms: Algorithms,
    pub compression: bool,
    pub methods: Vec<Method>,
    pub batch_mode: bool,
    pub set_env: Vec<(String, String)>,
}

/// Options as obtained so far: each is set only while unset.
#[derive(Clone, Debug, Default)]
struct Options {
    host_name: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_files: Vec<String>,
    certificate_files: Vec<String>,
    identities_only: Option<bool>,
    identity_agent: Option<String>,
    add_keys_to_agent: Option<bool>,
    forward_agent: Option<String>,
    proxy_command: Option<String>,
    proxy_jump: Option<String>,
    alive_interval: Option<u64>,
    alive_count_max: Option<u32>,
    strict: Option<Strict>,
    known_hosts: Option<Vec<String>>,
    global_known_hosts: Option<Vec<String>>,
    host_key_alias: Option<String>,
    connect_timeout: Option<u64>,
    family: Option<Family>,
    algorithms: Algorithms,
    compression: Option<bool>,
    preferred: Option<String>,
    pubkey: Option<bool>,
    password: Option<bool>,
    keyboard: Option<bool>,
    batch_mode: Option<bool>,
    set_env: Vec<(String, String)>,
    canonicalize: Option<Canonicalize>,
    canonical_domains: Option<Vec<String>>,
    canonical_max_dots: Option<usize>,
    canonical_fallback: Option<bool>,
    tag: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Canonicalize {
    No,
    Yes,
    Always,
}

/// Where a pass stands: which name `Host` lines match and which `Match` criteria hold.
struct Pass<'a> {
    original: &'a str,
    /// The name `Host` lines match: the alias, or its canonical form on the final pass.
    host: String,
    canonical: bool,
    last: bool,
    /// Set when a `Match final` was seen, which asks for another pass.
    wants_final: bool,
}

const MAX_INCLUDE_DEPTH: usize = 16;

/// Reads `alias`'s options from the user's config, then the system's.
pub fn resolve(alias: &str, local: &Local) -> Result<Host, String> {
    let files = [local.home.join(".ssh/config"), PathBuf::from("/etc/ssh/ssh_config")];
    resolve_from(alias, &files, local)
}

pub fn resolve_from(alias: &str, files: &[PathBuf], local: &Local) -> Result<Host, String> {
    let mut options = Options::default();
    let mut pass = Pass { original: alias, host: alias.to_string(), canonical: false, last: false, wants_final: false };
    read_all(files, &mut options, &mut pass, local)?;

    let canonical = canonicalize(alias, &options, local);
    if canonical.is_some() || pass.wants_final {
        pass.host = canonical.unwrap_or_else(|| pass.host.clone());
        pass.canonical = true;
        pass.last = true;
        read_all(files, &mut options, &mut pass, local)?;
    }
    finish(alias, &pass.host, options, local)
}

fn read_all(files: &[PathBuf], options: &mut Options, pass: &mut Pass, local: &Local) -> Result<(), String> {
    for (index, file) in files.iter().enumerate() {
        let base = if index == 0 { local.home.join(".ssh") } else { PathBuf::from("/etc/ssh") };
        if let Ok(text) = std::fs::read_to_string(file) {
            read(&text, file, &base, options, pass, local, 0)?;
        }
    }
    Ok(())
}

/// The canonical name, when `CanonicalizeHostname` asks for one and a domain gives one that
/// resolves.
fn canonicalize(alias: &str, options: &Options, local: &Local) -> Option<String> {
    let how = options.canonicalize.unwrap_or(Canonicalize::No);
    let proxied = options.proxy_command.as_deref().is_some_and(|c| c != "none")
        || options.proxy_jump.as_deref().is_some_and(|j| j != "none");
    if how == Canonicalize::No || (how == Canonicalize::Yes && proxied) {
        return None;
    }
    let name = options.host_name.as_deref().map(|h| expand(h, &[('h', alias)])).unwrap_or_else(|| alias.to_string());
    if name.ends_with('.') || name.split('.').count() - 1 > options.canonical_max_dots.unwrap_or(1) {
        return None;
    }
    options.canonical_domains.iter().flatten().map(|domain| format!("{name}.{domain}")).find(|full| (local.resolves)(full))
}

fn read(
    text: &str,
    file: &Path,
    base: &Path,
    options: &mut Options,
    pass: &mut Pass,
    local: &Local,
    depth: usize,
) -> Result<(), String> {
    let mut active = true;
    for (number, line) in text.lines().enumerate() {
        let at = || format!("{}:{}", file.display(), number + 1);
        let Some((keyword, rest)) = split_keyword(line) else { continue };
        let args = words(rest).map_err(|e| format!("{}: {e}", at()))?;
        match keyword.as_str() {
            "host" => active = host_matches(&args, &pass.host),
            "match" => active = match_matches(&args, options, pass, local).map_err(|e| format!("{}: {e}", at()))?,
            "include" if active => {
                if depth >= MAX_INCLUDE_DEPTH {
                    return Err(format!("{}: Include nested too deeply", at()));
                }
                for pattern in &args {
                    let pattern = tilde(pattern, &local.home);
                    let pattern = if Path::new(&pattern).is_absolute() { PathBuf::from(pattern) } else { base.join(pattern) };
                    for path in glob(&pattern) {
                        if let Ok(included) = std::fs::read_to_string(&path) {
                            // An included file's Host and Match lines govern only that file.
                            read(&included, &path, base, options, pass, local, depth + 1)?;
                        }
                    }
                }
            }
            _ if !active => {}
            _ => set(options, &keyword, rest, &args).map_err(|e| format!("{}: {e}", at()))?,
        }
    }
    Ok(())
}

/// The lowercased keyword and what follows it, for a line that has one.
fn split_keyword(line: &str) -> Option<(String, &str)> {
    let line = line.trim_start();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=').unwrap_or(line.len());
    let rest = line[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    Some((line[..end].to_ascii_lowercase(), rest))
}

/// A line's arguments, split on whitespace with double or single quotes keeping words whole.
fn words(text: &str) -> Result<Vec<String>, String> {
    let (mut out, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (_, '\\') => match chars.next() {
                Some(next) => {
                    word.push(next);
                    started = true;
                }
                None => word.push('\\'),
            },
            (Some(_), _) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, '#') if !started => break,
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".into());
    }
    if started {
        out.push(word);
    }
    Ok(out)
}

/// A `Host` line holds when some pattern matches and no negated one does.
fn host_matches(patterns: &[String], host: &str) -> bool {
    let mut matched = false;
    for pattern in patterns {
        match pattern.strip_prefix('!') {
            Some(negated) if wildcard(negated, host) => return false,
            Some(_) => {}
            None => matched |= wildcard(pattern, host),
        }
    }
    matched
}

/// A comma-separated list as `Match` takes it: negated entries veto, as on a `Host` line.
fn list_matches(list: &str, value: &str) -> bool {
    host_matches(&list.split(',').map(str::to_string).collect::<Vec<_>>(), value)
}

/// `*` and `?` globbing, case-insensitive, as OpenSSH matches host patterns.
fn wildcard(pattern: &str, text: &str) -> bool {
    globs(&pattern.to_lowercase(), &text.to_lowercase())
}

fn globs(pattern: &str, text: &str) -> bool {
    let (pattern, text): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut p, mut t, mut star, mut mark) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

fn match_matches(args: &[String], options: &Options, pass: &mut Pass, local: &Local) -> Result<bool, String> {
    if args.is_empty() {
        return Err("Match needs criteria".into());
    }
    let mut result = true;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (negated, criterion) = match arg.strip_prefix('!') {
            Some(rest) => (true, rest.to_ascii_lowercase()),
            None => (false, arg.to_ascii_lowercase()),
        };
        let mut value = || args.next().ok_or_else(|| format!("Match {criterion} needs an argument"));
        let held = match criterion.as_str() {
            "all" => true,
            "canonical" => pass.canonical,
            "final" => {
                pass.wants_final = true;
                pass.last
            }
            "host" => {
                let host = options.host_name.as_deref().map(|h| expand(h, &[('h', pass.original)]));
                list_matches(value()?, host.as_deref().unwrap_or(&pass.host))
            }
            "originalhost" => list_matches(value()?, pass.original),
            "user" => list_matches(value()?, options.user.as_deref().unwrap_or(&local.user)),
            "localuser" => list_matches(value()?, &local.user),
            "tagged" => list_matches(value()?, options.tag.as_deref().unwrap_or("")),
            "exec" => {
                let command = value()?.clone();
                let tokens = tokens(pass.original, &pass.host, options, local);
                (local.exec)(&expand(&command, &tokens.iter().map(|(c, v)| (*c, v.as_str())).collect::<Vec<_>>()))
            }
            "localnetwork" => {
                value()?;
                return Err("Match localnetwork is not supported".into());
            }
            other => return Err(format!("unknown Match criterion {other}")),
        };
        result &= held != negated;
    }
    Ok(result)
}

fn yes_no(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" => Ok(true),
        "no" | "false" => Ok(false),
        other => Err(format!("expected yes or no, not {other}")),
    }
}

fn first<T>(slot: &mut Option<T>, value: impl FnOnce() -> Result<T, String>) -> Result<(), String> {
    if slot.is_none() {
        *slot = Some(value()?);
    }
    Ok(())
}

fn set(options: &mut Options, keyword: &str, rest: &str, args: &[String]) -> Result<(), String> {
    let arg = || args.first().cloned().ok_or_else(|| format!("{keyword} needs a value"));
    let number = |text: String| text.parse::<u64>().map_err(|_| format!("{keyword}: {text} is not a number"));
    match keyword {
        "hostname" => first(&mut options.host_name, arg),
        "user" => first(&mut options.user, arg),
        "port" => first(&mut options.port, || {
            let text = arg()?;
            text.parse().map_err(|_| format!("bad port {text}"))
        }),
        "identityfile" => {
            options.identity_files.push(arg()?);
            Ok(())
        }
        "certificatefile" => {
            options.certificate_files.push(arg()?);
            Ok(())
        }
        "identitiesonly" => first(&mut options.identities_only, || yes_no(&arg()?)),
        "identityagent" => first(&mut options.identity_agent, arg),
        "addkeystoagent" => first(&mut options.add_keys_to_agent, || {
            let text = arg()?.to_ascii_lowercase();
            // `ask`, `confirm` and a lifetime all still add the key.
            Ok(text != "no")
        }),
        "forwardagent" => first(&mut options.forward_agent, arg),
        // The rest of the line, as ssh(1) takes it, quotes and all.
        "proxycommand" => first(&mut options.proxy_command, || Ok(rest.to_string())),
        "proxyjump" => first(&mut options.proxy_jump, arg),
        "serveraliveinterval" => first(&mut options.alive_interval, || number(arg()?)),
        "serveralivecountmax" => first(&mut options.alive_count_max, || Ok(number(arg()?)? as u32)),
        "stricthostkeychecking" => first(&mut options.strict, || {
            Ok(match arg()?.to_ascii_lowercase().as_str() {
                "yes" | "true" => Strict::Yes,
                "accept-new" => Strict::AcceptNew,
                "no" | "off" | "false" => Strict::No,
                "ask" => Strict::Ask,
                other => return Err(format!("bad StrictHostKeyChecking {other}")),
            })
        }),
        "userknownhostsfile" => first(&mut options.known_hosts, || Ok(args.to_vec())),
        "globalknownhostsfile" => first(&mut options.global_known_hosts, || Ok(args.to_vec())),
        "hostkeyalias" => first(&mut options.host_key_alias, arg),
        "connecttimeout" => first(&mut options.connect_timeout, || number(arg()?)),
        "addressfamily" => first(&mut options.family, || {
            Ok(match arg()?.to_ascii_lowercase().as_str() {
                "inet" => Family::Inet,
                "inet6" => Family::Inet6,
                "any" => Family::Any,
                other => return Err(format!("bad AddressFamily {other}")),
            })
        }),
        "hostkeyalgorithms" => first(&mut options.algorithms.host_key, arg),
        "pubkeyacceptedalgorithms" | "pubkeyacceptedkeytypes" => first(&mut options.algorithms.pubkey, arg),
        "kexalgorithms" => first(&mut options.algorithms.kex, arg),
        "ciphers" => first(&mut options.algorithms.ciphers, arg),
        "macs" => first(&mut options.algorithms.macs, arg),
        "compression" => first(&mut options.compression, || yes_no(&arg()?)),
        "preferredauthentications" => first(&mut options.preferred, arg),
        "pubkeyauthentication" => first(&mut options.pubkey, || Ok(arg()?.to_ascii_lowercase() != "no")),
        "passwordauthentication" => first(&mut options.password, || yes_no(&arg()?)),
        "kbdinteractiveauthentication" | "challengeresponseauthentication" => {
            first(&mut options.keyboard, || yes_no(&arg()?))
        }
        "batchmode" => first(&mut options.batch_mode, || yes_no(&arg()?)),
        "setenv" => {
            for pair in args {
                let (name, value) = pair.split_once('=').ok_or_else(|| format!("SetEnv {pair} has no ="))?;
                if !options.set_env.iter().any(|(known, _)| known == name) {
                    options.set_env.push((name.to_string(), value.to_string()));
                }
            }
            Ok(())
        }
        "canonicalizehostname" => first(&mut options.canonicalize, || {
            Ok(match arg()?.to_ascii_lowercase().as_str() {
                "yes" => Canonicalize::Yes,
                "always" => Canonicalize::Always,
                "no" => Canonicalize::No,
                other => return Err(format!("bad CanonicalizeHostname {other}")),
            })
        }),
        "canonicaldomains" => first(&mut options.canonical_domains, || Ok(args.to_vec())),
        "canonicalizemaxdots" => first(&mut options.canonical_max_dots, || Ok(number(arg()?)? as usize)),
        "canonicalizefallbacklocal" => first(&mut options.canonical_fallback, || yes_no(&arg()?)),
        "tag" => first(&mut options.tag, arg),
        // Everything else shapes what ssh(1) does around a connection, not how it reaches one.
        _ => Ok(()),
    }
}

/// Replaces `%x` tokens; `%%` is a percent sign.
fn expand(text: &str, tokens: &[(char, &str)]) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some(token) => match tokens.iter().find(|(name, _)| *name == token) {
                Some((_, value)) => out.push_str(value),
                None => {
                    out.push('%');
                    out.push(token);
                }
            },
            None => out.push('%'),
        }
    }
    out
}

/// Replaces `${NAME}` with the variable's value.
fn environment(text: &str, local: &Local) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}').ok_or_else(|| format!("unterminated ${{ in {text}"))? + start;
        let name = &rest[start + 2..end];
        out.push_str(&(local.env)(name).ok_or_else(|| format!("{text}: {name} is not set"))?);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn tilde(path: &str, home: &Path) -> String {
    match path.strip_prefix('~') {
        Some("") => home.display().to_string(),
        Some(rest) if rest.starts_with('/') => format!("{}{rest}", home.display()),
        _ => path.to_string(),
    }
}

/// The tokens ssh(1) offers in paths and commands, `%C` and `%k` included.
fn tokens(original: &str, host: &str, options: &Options, local: &Local) -> Vec<(char, String)> {
    let host_name = options.host_name.as_deref().map(|h| expand(h, &[('h', host)])).unwrap_or_else(|| host.to_string());
    let port = options.port.unwrap_or(22).to_string();
    let user = options.user.clone().unwrap_or_else(|| local.user.clone());
    let jump = options.proxy_jump.clone().filter(|j| j != "none").unwrap_or_default();
    let short = local.host.split('.').next().unwrap_or_default().to_string();
    let hash = {
        use sha1::{Digest, Sha1};
        let digest = Sha1::digest(format!("{}{host_name}{port}{user}{jump}", local.host));
        digest.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    vec![
        ('h', host_name.clone()),
        ('n', original.to_string()),
        ('p', port),
        ('r', user),
        ('u', local.user.clone()),
        ('d', local.home.display().to_string()),
        ('i', local.uid.to_string()),
        ('l', local.host.clone()),
        ('L', short),
        ('j', jump),
        ('k', options.host_key_alias.clone().unwrap_or(host_name)),
        ('C', hash),
    ]
}

fn finish(original: &str, host: &str, options: Options, local: &Local) -> Result<Host, String> {
    let tokens = tokens(original, host, &options, local);
    let tokens: Vec<(char, &str)> = tokens.iter().map(|(c, v)| (*c, v.as_str())).collect();
    let path = |raw: &str| -> Result<PathBuf, String> { Ok(PathBuf::from(expand(&tilde(&environment(raw, local)?, &local.home), &tokens))) };
    let paths = |raws: &[String]| raws.iter().filter(|r| r.as_str() != "none").map(|r| path(r)).collect::<Result<Vec<_>, _>>();

    let identity_files = match options.identity_files.is_empty() {
        true => ["id_rsa", "id_ecdsa", "id_ecdsa_sk", "id_ed25519", "id_ed25519_sk"]
            .iter()
            .map(|name| local.home.join(".ssh").join(name))
            .collect(),
        false => paths(&options.identity_files)?,
    };
    let agent = |value: &str| -> Result<Option<PathBuf>, String> {
        match value {
            "none" => Ok(None),
            "SSH_AUTH_SOCK" => Ok((local.env)("SSH_AUTH_SOCK").map(PathBuf::from)),
            env if env.starts_with('$') => Ok((local.env)(&env[1..]).map(PathBuf::from)),
            socket => path(socket).map(Some),
        }
    };
    let identity_agent = agent(options.identity_agent.as_deref().unwrap_or("SSH_AUTH_SOCK"))?;
    let forward_agent = match options.forward_agent.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None | Some("no") | Some("false") => None,
        Some("yes") | Some("true") => identity_agent.clone(),
        Some(_) => agent(options.forward_agent.as_deref().unwrap_or_default())?,
    };
    let proxy = match (options.proxy_jump.as_deref(), options.proxy_command.as_deref()) {
        (Some(jump), _) if jump != "none" => Proxy::Jump(jump.split(',').map(hop).collect::<Result<_, _>>()?),
        (_, Some(command)) if command != "none" => {
            Proxy::Command(expand(command, &tokens.iter().copied().filter(|(c, _)| "hnpr".contains(*c)).collect::<Vec<_>>()))
        }
        _ => Proxy::Direct,
    };
    let preferred = options.preferred.as_deref().unwrap_or("publickey,keyboard-interactive,password");
    let methods = preferred
        .split(',')
        .filter_map(|name| match name.trim() {
            "publickey" if options.pubkey != Some(false) => Some(Method::PublicKey),
            "keyboard-interactive" if options.keyboard != Some(false) => Some(Method::KeyboardInteractive),
            "password" if options.password != Some(false) => Some(Method::Password),
            _ => None,
        })
        .collect();
    let known_hosts = match &options.known_hosts {
        Some(files) => paths(files)?,
        None => vec![local.home.join(".ssh/known_hosts"), local.home.join(".ssh/known_hosts2")],
    };
    let global_known_hosts = match &options.global_known_hosts {
        Some(files) => paths(files)?,
        None => vec![PathBuf::from("/etc/ssh/ssh_known_hosts"), PathBuf::from("/etc/ssh/ssh_known_hosts2")],
    };
    let host_name = tokens.iter().find(|(c, _)| *c == 'h').map(|(_, v)| v.to_string()).unwrap_or_default();
    Ok(Host {
        alias: original.to_string(),
        host_name,
        port: options.port.unwrap_or(22),
        user: options.user.unwrap_or_else(|| local.user.clone()),
        identity_files,
        certificate_files: paths(&options.certificate_files)?,
        identities_only: options.identities_only.unwrap_or(false),
        identity_agent,
        add_keys_to_agent: options.add_keys_to_agent.unwrap_or(false),
        forward_agent,
        proxy,
        alive_interval: options.alive_interval.filter(|&s| s > 0).map(Duration::from_secs),
        alive_count_max: options.alive_count_max.unwrap_or(3),
        strict: options.strict.unwrap_or(Strict::Ask),
        known_hosts,
        global_known_hosts,
        host_key_alias: options.host_key_alias,
        connect_timeout: options.connect_timeout.filter(|&s| s > 0).map(Duration::from_secs),
        family: options.family.unwrap_or(Family::Any),
        algorithms: options.algorithms,
        compression: options.compression.unwrap_or(false),
        methods,
        batch_mode: options.batch_mode.unwrap_or(false),
        set_env: options.set_env,
    })
}

/// One `ProxyJump` hop: `[user@]host[:port]` or `ssh://[user@]host[:port]`.
fn hop(text: &str) -> Result<Hop, String> {
    let text = text.trim();
    let text = text.strip_prefix("ssh://").unwrap_or(text);
    let (user, rest) = match text.rsplit_once('@') {
        Some((user, rest)) => (Some(user.to_string()), rest),
        None => (None, text),
    };
    let (host, port) = match rest.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']').ok_or_else(|| format!("bad ProxyJump {text}"))?;
            (host, after.strip_prefix(':'))
        }
        None => match rest.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            _ => (rest, None),
        },
    };
    let port = port.map(|p| p.parse().map_err(|_| format!("bad port in ProxyJump {text}"))).transpose()?;
    if host.is_empty() {
        return Err(format!("bad ProxyJump {text}"));
    }
    Ok(Hop { user, host: host.to_string(), port })
}

/// The files a pattern names, sorted as glob(3) sorts them.
fn glob(pattern: &Path) -> Vec<PathBuf> {
    let text = pattern.display().to_string();
    if !text.contains(['*', '?', '[']) {
        return vec![pattern.to_path_buf()];
    }
    let mut found = vec![PathBuf::from("/")];
    for part in pattern.components().skip(1) {
        let part = part.as_os_str().to_string_lossy().to_string();
        found = found
            .into_iter()
            .flat_map(|dir| {
                if !part.contains(['*', '?']) {
                    return vec![dir.join(&part)];
                }
                let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|entry| {
                        let name = entry.file_name().to_string_lossy().to_string();
                        !name.starts_with('.') && globs(&part, &name)
                    })
                    .map(|entry| entry.path())
                    .collect();
                entries.sort();
                entries
            })
            .collect();
    }
    found.into_iter().filter(|path| path.exists()).collect()
}

#[cfg(test)]
mod tests;
