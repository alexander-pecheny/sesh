//! OpenSSH `known_hosts` lookups against a file the app owns.

use std::path::Path;

use russh::keys::known_hosts::{check_known_hosts_path, known_host_keys_path, learn_known_hosts_path};
use russh::keys::ssh_key::PublicKey;

pub enum Verdict {
    Known,
    Unknown,
    Mismatch(PublicKey),
}

pub fn check(path: &Path, host: &str, port: u16, key: &PublicKey) -> Verdict {
    match check_known_hosts_path(host, port, key, path) {
        Ok(true) => Verdict::Known,
        Ok(false) => Verdict::Unknown,
        Err(_) => known_host_keys_path(host, port, path)
            .ok()
            .and_then(|keys| {
                keys.into_iter()
                    .map(|(_, k)| k)
                    .find(|k| k.algorithm() == key.algorithm())
            })
            .map_or(Verdict::Unknown, Verdict::Mismatch),
    }
}

pub fn accept(
    path: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
    replacing: Option<&PublicKey>,
) -> Result<(), String> {
    if let Some(old) = replacing {
        forget(path, old)?;
    }
    learn_known_hosts_path(host, port, key, path).map_err(|e| e.to_string())
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(Default::default()).to_string()
}

fn forget(path: &Path, key: &PublicKey) -> Result<(), String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(());
    };
    let openssh = key.to_openssh().map_err(|e| e.to_string())?;
    let blob = openssh.split_whitespace().nth(1).unwrap_or_default();
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.split_whitespace().any(|f| f == blob))
        .collect();
    std::fs::write(path, kept.join("\n") + "\n").map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::PublicKey;

    const FIRST: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPXAzMEFN2s5iwTeL7ExdxuL8YfWmWgF/yGFahzcEa1+";
    const SECOND: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFWCEzzZXFROTecVKRP7HImStdntwaFisBaPZc/5H4rl";

    #[test]
    fn tofu_then_known_then_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let first = PublicKey::from_openssh(FIRST).unwrap();
        let second = PublicKey::from_openssh(SECOND).unwrap();

        assert!(matches!(check(&path, "localhost", 2222, &first), Verdict::Unknown));
        accept(&path, "localhost", 2222, &first, None).unwrap();
        assert!(matches!(check(&path, "localhost", 2222, &first), Verdict::Known));
        assert!(std::fs::read_to_string(&path).unwrap().contains("[localhost]:2222"));

        let Verdict::Mismatch(old) = check(&path, "localhost", 2222, &second) else {
            panic!("expected a mismatch");
        };
        assert_eq!(fingerprint(&old), fingerprint(&first));
        accept(&path, "localhost", 2222, &second, Some(&old)).unwrap();
        assert!(matches!(check(&path, "localhost", 2222, &second), Verdict::Known));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().filter(|l| !l.trim().is_empty()).count(), 1, "{text:?}");
    }
}
