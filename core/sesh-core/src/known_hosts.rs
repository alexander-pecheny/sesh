//! OpenSSH `known_hosts` files, read as ssh(1) reads them: plain, wildcard and hashed host
//! patterns, `[host]:port` entries and `@revoked` keys.

use std::path::{Path, PathBuf};

use base64ct::{Base64, Encoding};
use hmac::{Hmac, Mac};
use russh::keys::ssh_key::PublicKey;
use sha1::Sha1;

use crate::ssh_config::Strict;

#[derive(Debug)]
pub enum Verdict {
    Known,
    Unknown,
    Mismatch(PublicKey),
    Revoked,
}

/// Where a connection's host keys are looked up, and how an unknown one is treated.
#[derive(Clone, Debug)]
pub struct KnownHosts {
    /// The user's files; a newly accepted key goes in the first.
    pub files: Vec<PathBuf>,
    pub global: Vec<PathBuf>,
    pub strict: Strict,
    /// No one is there to ask, as with `BatchMode yes`.
    pub batch: bool,
}

impl KnownHosts {
    /// The file the phone keeps for its own Hosts, where every new key is asked about.
    pub fn app(path: PathBuf) -> Self {
        KnownHosts { files: vec![path], global: Vec::new(), strict: Strict::Ask, batch: false }
    }

    pub fn check(&self, host: &str, port: u16, key: &PublicKey) -> Verdict {
        let name = entry(host, port);
        let (mut known, mut changed) = (false, None);
        for file in self.files.iter().chain(&self.global) {
            let Ok(text) = std::fs::read_to_string(file) else { continue };
            for line in text.lines().filter_map(Line::parse) {
                let same = line.key.key_data() == key.key_data();
                match line.marker {
                    Some(Marker::Revoked) if same => return Verdict::Revoked,
                    Some(_) => {}
                    None if !line.matches(&name) => {}
                    None if same => known = true,
                    None if line.key.algorithm() == key.algorithm() => changed = changed.or(Some(line.key)),
                    None => {}
                }
            }
        }
        match (known, changed) {
            (true, _) => Verdict::Known,
            (false, changed) => changed.map_or(Verdict::Unknown, Verdict::Mismatch),
        }
    }

    /// Records `key` for the host in the first file, dropping only that host's lines for the
    /// key it replaces.
    pub fn accept(&self, host: &str, port: u16, key: &PublicKey, replacing: Option<&PublicKey>) -> Result<(), String> {
        let Some(file) = self.files.first() else { return Ok(()) };
        let name = entry(host, port);
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let mut kept: Vec<&str> = text
            .lines()
            .filter(|raw| match (replacing, Line::parse(raw)) {
                (Some(old), Some(line)) => !(line.marker.is_none() && line.matches(&name) && line.key.key_data() == old.key_data()),
                _ => true,
            })
            .collect();
        let openssh = key.to_openssh().map_err(|e| e.to_string())?;
        let mut words = openssh.split_whitespace();
        let fresh = format!("{name} {} {}", words.next().unwrap_or_default(), words.next().unwrap_or_default());
        kept.push(&fresh);
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        write(file, &(kept.join("\n") + "\n"))
    }
}

/// `/dev/null` takes writes but cannot be replaced by a renamed file.
fn write(file: &Path, text: &str) -> Result<(), String> {
    if !file.is_file() && file.exists() {
        return Ok(());
    }
    let staged = file.with_extension("sesh-new");
    std::fs::write(&staged, text).map_err(|e| format!("writing {}: {e}", file.display()))?;
    std::fs::rename(&staged, file).map_err(|e| format!("writing {}: {e}", file.display()))
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(Default::default()).to_string()
}

/// How a host is named in the file: bare on port 22, `[host]:port` otherwise.
fn entry(host: &str, port: u16) -> String {
    match port {
        22 => host.to_string(),
        _ => format!("[{host}]:{port}"),
    }
}

enum Marker {
    Revoked,
    CertAuthority,
}

struct Line<'a> {
    marker: Option<Marker>,
    patterns: &'a str,
    key: PublicKey,
}

impl<'a> Line<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let mut fields = line.split_whitespace();
        let mut first = fields.next()?;
        let marker = match first {
            "@revoked" => Some(Marker::Revoked),
            "@cert-authority" => Some(Marker::CertAuthority),
            _ => None,
        };
        if marker.is_some() {
            first = fields.next()?;
        }
        let (kind, blob) = (fields.next()?, fields.next()?);
        let key = PublicKey::from_openssh(&format!("{kind} {blob}")).ok()?;
        Some(Line { marker, patterns: first, key })
    }

    /// Whether the line names `name`: some pattern matches and no negated one does.
    fn matches(&self, name: &str) -> bool {
        let mut matched = false;
        for pattern in self.patterns.split(',') {
            match pattern.strip_prefix('!') {
                Some(negated) if named(negated, name) => return false,
                Some(_) => {}
                None => matched |= named(pattern, name),
            }
        }
        matched
    }
}

fn named(pattern: &str, name: &str) -> bool {
    match pattern.strip_prefix("|1|") {
        Some(hashed) => hashed_matches(hashed, name),
        None => crate::ssh_config::wildcard(pattern, name),
    }
}

/// `|1|salt|hash`: the HMAC-SHA1 of the name, keyed by the salt.
fn hashed_matches(hashed: &str, name: &str) -> bool {
    let Some((salt, hash)) = hashed.split_once('|') else { return false };
    let (Ok(salt), Ok(hash)) = (Base64::decode_vec(salt), Base64::decode_vec(hash)) else { return false };
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(&salt) else { return false };
    mac.update(name.as_bytes());
    mac.verify_slice(&hash).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPXAzMEFN2s5iwTeL7ExdxuL8YfWmWgF/yGFahzcEa1+";
    const SECOND: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFWCEzzZXFROTecVKRP7HImStdntwaFisBaPZc/5H4rl";

    fn known(text: &str) -> (tempfile::TempDir, KnownHosts) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, text).unwrap();
        (dir, KnownHosts::app(path))
    }

    #[test]
    fn tofu_then_known_then_mismatch() {
        let (_dir, hosts) = known("");
        let first = PublicKey::from_openssh(FIRST).unwrap();
        let second = PublicKey::from_openssh(SECOND).unwrap();

        assert!(matches!(hosts.check("localhost", 2222, &first), Verdict::Unknown));
        hosts.accept("localhost", 2222, &first, None).unwrap();
        assert!(matches!(hosts.check("localhost", 2222, &first), Verdict::Known));
        assert!(std::fs::read_to_string(&hosts.files[0]).unwrap().contains("[localhost]:2222"));

        let Verdict::Mismatch(old) = hosts.check("localhost", 2222, &second) else {
            panic!("expected a mismatch");
        };
        assert_eq!(fingerprint(&old), fingerprint(&first));
        hosts.accept("localhost", 2222, &second, Some(&old)).unwrap();
        assert!(matches!(hosts.check("localhost", 2222, &second), Verdict::Known));
        let text = std::fs::read_to_string(&hosts.files[0]).unwrap();
        assert_eq!(text.lines().filter(|l| !l.trim().is_empty()).count(), 1, "{text:?}");
    }

    #[test]
    fn replacing_a_key_keeps_other_hosts_that_share_it() {
        let (_dir, hosts) = known(&format!("other {FIRST}\nbox {FIRST}\n"));
        let (first, second) = (PublicKey::from_openssh(FIRST).unwrap(), PublicKey::from_openssh(SECOND).unwrap());
        hosts.accept("box", 22, &second, Some(&first)).unwrap();
        assert!(matches!(hosts.check("other", 22, &first), Verdict::Known));
        assert!(matches!(hosts.check("box", 22, &second), Verdict::Known));
    }

    #[test]
    fn hashed_wildcard_and_negated_patterns_match_as_ssh_matches_them() {
        // `ssh-keygen -H` output for the name "hashed.example".
        let salt = Base64::encode_string(b"0123456789abcdefghij");
        let mut mac = Hmac::<Sha1>::new_from_slice(b"0123456789abcdefghij").unwrap();
        mac.update(b"hashed.example");
        let hash = Base64::encode_string(&mac.finalize().into_bytes());
        let (_dir, hosts) = known(&format!("|1|{salt}|{hash} {FIRST}\n*.prod,!db.prod {FIRST}\n"));
        let key = PublicKey::from_openssh(FIRST).unwrap();
        assert!(matches!(hosts.check("hashed.example", 22, &key), Verdict::Known));
        assert!(matches!(hosts.check("web.prod", 22, &key), Verdict::Known));
        assert!(matches!(hosts.check("db.prod", 22, &key), Verdict::Unknown));
    }

    #[test]
    fn a_revoked_key_is_refused_for_any_host() {
        let (_dir, hosts) = known(&format!("box {FIRST}\n@revoked * {FIRST}\n"));
        assert!(matches!(hosts.check("box", 22, &PublicKey::from_openssh(FIRST).unwrap()), Verdict::Revoked));
    }
}
