//! Extra flags: the documented ssh and mosh subsets, validated before connecting.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Ssh,
    Mosh,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Jump {
    pub user: Option<String>,
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SshFlags {
    pub port: Option<u16>,
    pub user: Option<String>,
    pub ipv4_only: bool,
    pub ipv6_only: bool,
    pub jump: Option<Jump>,
    pub force_tty: bool,
    pub agent_forwarding: bool,
    pub alive_interval: Option<u64>,
    pub alive_count_max: Option<u32>,
    pub host_key_algorithms: Option<String>,
    pub pubkey_accepted_algorithms: Option<String>,
}

pub fn split(text: &str) -> Result<Vec<String>, String> {
    let (mut out, mut word, mut quote, mut started) = (Vec::new(), String::new(), None, false);
    for c in text.chars() {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => word.push(c),
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                started = true;
            }
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
        return Err("unbalanced quote in extra flags".into());
    }
    if started {
        out.push(word);
    }
    Ok(out)
}

fn value(args: &mut std::vec::IntoIter<String>, inline: Option<String>, flag: &str) -> Result<String, String> {
    inline
        .filter(|v| !v.is_empty())
        .or_else(|| args.next())
        .ok_or_else(|| format!("{flag} needs a value"))
}

pub fn parse_ssh(text: &str) -> Result<SshFlags, String> {
    let mut flags = SshFlags::default();
    let mut args = split(text)?.into_iter();
    while let Some(arg) = args.next() {
        let (head, inline) = arg.split_at(2.min(arg.len()));
        let inline = Some(inline.to_string());
        match head {
            "-p" => {
                let v = value(&mut args, inline, "-p")?;
                flags.port = Some(v.parse().map_err(|_| format!("-p wants a port number, got {v:?}"))?);
            }
            "-l" => flags.user = Some(value(&mut args, inline, "-l")?),
            "-J" => flags.jump = Some(parse_jump(&value(&mut args, inline, "-J")?)?),
            "-o" => parse_option(&value(&mut args, inline, "-o")?, &mut flags)?,
            "-4" if arg == "-4" => flags.ipv4_only = true,
            "-6" if arg == "-6" => flags.ipv6_only = true,
            "-t" if arg == "-t" => flags.force_tty = true,
            "-A" if arg == "-A" => flags.agent_forwarding = true,
            _ => return Err(format!("unsupported ssh flag: {arg}")),
        }
    }
    if flags.ipv4_only && flags.ipv6_only {
        return Err("-4 and -6 cannot both be given".into());
    }
    Ok(flags)
}

fn parse_jump(spec: &str) -> Result<Jump, String> {
    let (user, rest) = match spec.rsplit_once('@') {
        Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
        _ => (None, spec),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (
            h,
            p.parse().map_err(|_| format!("-J wants host[:port], got {spec:?}"))?,
        ),
        None => (rest, 22),
    };
    if host.is_empty() {
        return Err(format!("-J wants user@host[:port], got {spec:?}"));
    }
    Ok(Jump { user, host: host.to_string(), port })
}

fn parse_option(option: &str, flags: &mut SshFlags) -> Result<(), String> {
    let (key, val) = option
        .split_once(['=', ' '])
        .ok_or_else(|| format!("-o wants Key=Value, got {option:?}"))?;
    let number = |what: &str| -> Result<u64, String> {
        val.parse().map_err(|_| format!("-o {what} wants a number, got {val:?}"))
    };
    match key.to_ascii_lowercase().as_str() {
        "serveraliveinterval" => flags.alive_interval = Some(number("ServerAliveInterval")?),
        "serveralivecountmax" => flags.alive_count_max = Some(number("ServerAliveCountMax")? as u32),
        "port" => flags.port = Some(number("Port")? as u16),
        "user" => flags.user = Some(val.to_string()),
        "hostkeyalgorithms" => flags.host_key_algorithms = Some(val.to_string()),
        "pubkeyacceptedalgorithms" => flags.pubkey_accepted_algorithms = Some(val.to_string()),
        _ => return Err(format!("unsupported ssh option: -o {key}")),
    }
    Ok(())
}

pub fn parse_mosh(text: &str) -> Result<(), String> {
    let mut args = split(text)?.into_iter();
    while let Some(arg) = args.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        match key.as_str() {
            "--ssh" => parse_ssh(strip_command(&value(&mut args, inline, "--ssh")?)).map(|_| ())?,
            "--server" => drop(value(&mut args, inline, "--server")?),
            "--predict" => match value(&mut args, inline, "--predict")?.as_str() {
                "adaptive" | "always" | "never" | "experimental" => {}
                other => return Err(format!("--predict wants adaptive|always|never, got {other:?}")),
            },
            "--experimental-remote-ip" => {
                match value(&mut args, inline, "--experimental-remote-ip")?.as_str() {
                    "local" | "remote" => {}
                    other => {
                        return Err(format!("--experimental-remote-ip wants local|remote, got {other:?}"))
                    }
                }
            }
            "-p" | "--port" => {
                let v = value(&mut args, inline, &key)?;
                let range = v.split_once(':').map_or((v.as_str(), v.as_str()), |(a, b)| (a, b));
                for part in [range.0, range.1] {
                    part.parse::<u16>()
                        .map_err(|_| format!("{key} wants a port or port range, got {v:?}"))?;
                }
            }
            "-a" | "-n" | "--no-init" | "-4" | "-6" => {}
            _ => return Err(format!("unsupported mosh flag: {arg}")),
        }
    }
    Ok(())
}

/// `--ssh` carries a whole command line, so its first word is the program, not a flag.
fn strip_command(value: &str) -> &str {
    match value.trim_start().split_once(char::is_whitespace) {
        Some((head, rest)) if !head.starts_with('-') => rest,
        _ => value,
    }
}

pub fn validate(transport: Transport, text: &str) -> Result<(), String> {
    match transport {
        Transport::Ssh => parse_ssh(text).map(|_| ()),
        Transport::Mosh => parse_mosh(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_ssh_subset() {
        let f = parse_ssh("-4 -t -A -p 2222 -l bob -J jump@gw.example:2022 -o ServerAliveInterval=30").unwrap();
        assert_eq!(f.port, Some(2222));
        assert_eq!(f.user.as_deref(), Some("bob"));
        assert!(f.ipv4_only && f.force_tty && f.agent_forwarding);
        assert_eq!(f.alive_interval, Some(30));
        let jump = f.jump.unwrap();
        assert_eq!((jump.user.as_deref(), jump.host.as_str(), jump.port), (Some("jump"), "gw.example", 2022));
    }

    #[test]
    fn accepts_attached_and_quoted_values() {
        let f = parse_ssh("-p2222 -o 'HostKeyAlgorithms=ssh-ed25519,rsa-sha2-512'").unwrap();
        assert_eq!(f.port, Some(2222));
        assert_eq!(f.host_key_algorithms.as_deref(), Some("ssh-ed25519,rsa-sha2-512"));
    }

    #[test]
    fn jump_without_port_defaults_to_22() {
        assert_eq!(parse_ssh("-J gw").unwrap().jump.unwrap().port, 22);
    }

    #[test]
    fn rejections_name_the_flag() {
        for (text, needle) in [
            ("-X", "-X"),
            ("-D 8080", "-D"),
            ("-o Compression=yes", "-o Compression"),
            ("-p http", "-p"),
            ("-4 -6", "-4 and -6"),
            ("-l", "-l needs a value"),
            ("-o nonsense", "-o wants Key=Value"),
            ("-p '2222", "unbalanced quote"),
        ] {
            let err = parse_ssh(text).unwrap_err();
            assert!(err.contains(needle), "{text:?} gave {err:?}, wanted {needle:?}");
        }
    }

    #[test]
    fn mosh_subset_and_its_rejections() {
        parse_mosh("-a -n --no-init -4 --predict=adaptive --port 60000:60010 --server=mosh-server").unwrap();
        parse_mosh("--ssh='ssh -p 2222'").unwrap();
        assert!(parse_mosh("--ssh='ssh -p 2222'").is_ok());
        for (text, needle) in [
            ("--bogus", "--bogus"),
            ("--predict=maybe", "--predict"),
            ("--experimental-remote-ip=elsewhere", "--experimental-remote-ip"),
            ("--port=nope", "--port"),
        ] {
            let err = parse_mosh(text).unwrap_err();
            assert!(err.contains(needle), "{text:?} gave {err:?}, wanted {needle:?}");
        }
    }
}
