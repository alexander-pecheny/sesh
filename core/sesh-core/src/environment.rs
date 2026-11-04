//! The environment every command Sesh runs gets: the machine's, as its login shell prints it,
//! without what belongs to one shell, terminal or Agent.

use mosh::opts::shell_quote;

/// Variables the shell sets for itself, which it refuses or ignores when given.
const OWN: &[&str] = &["_", "PWD", "OLDPWD", "SHLVL", "SHELLOPTS", "BASHOPTS", "BASHPID", "PPID", "UID", "EUID"];

/// Variables of the terminal or Agent that started the app or the shell, which would steer a
/// command to that Agent's account or that multiplexer's pane.
const SESSION: &[&str] = &["CLAUDE_CONFIG_DIR", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CODEX_THREAD_ID", "OMPCODE", "TMUX", "TMUX_PANE"];
const SESSION_PREFIXES: &[&str] = &["SESH_MUX_", "HERDR_", "ZELLIJ"];

fn session(name: &str) -> bool {
    SESSION.contains(&name) || SESSION_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Whether a command should get `name` as the machine's: not the shell's own, nor a session's.
pub fn kept(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric()) && !OWN.contains(&name) && !session(name)
}

/// What `env -0` printed, less the shell's own and every session's variables.
pub fn parse(env: &str) -> Vec<(String, String)> {
    env.split('\0')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(name, _)| kept(name))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// `vars` as `export` lines that give a plain shell the same environment.
pub fn exports(vars: &[(String, String)]) -> String {
    vars.iter().map(|(name, value)| format!("export {name}={}; ", shell_quote(value))).collect()
}

/// `command` run by `/bin/sh` after `exports`, so Sesh's snippets meet one dialect on every
/// machine whatever the user's login shell is.
pub fn plain(exports: &str, command: &str) -> String {
    format!("exec /bin/sh -c {}", shell_quote(&format!("{exports}{command}")))
}

/// Takes every session's variables out of this process, so nothing it starts inherits them
/// from the shell that started the app.
pub fn forget_sessions() {
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(session) {
            std::env::remove_var(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machines_variables_are_kept_and_a_sessions_dropped() {
        let env = "PATH=/bin\0CLAUDE_CONFIG_DIR=/a\0SESH_MUX_SOCKET_PATH=/s\0HERDR_PANE=1\0TMUX=/t\0ZELLIJ_SESSION_NAME=z\0SHLVL=3\0BAD-NAME=x\0HOME=/h\0";
        let names: Vec<String> = parse(env).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["PATH", "HOME"]);
    }

    #[test]
    fn a_plain_command_runs_in_sh_with_the_exports() {
        let vars = parse("GREETING=it's a \"walrus\"\nhere\0");
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &plain(&exports(&vars), r#"printf %s "$GREETING""#)])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "it's a \"walrus\"\nhere");
    }
}
