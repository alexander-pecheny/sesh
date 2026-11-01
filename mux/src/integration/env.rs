use std::io;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::{Mutex, MutexGuard, OnceLock};

use portable_pty::CommandBuilder;

pub(crate) const SESH_MUX_PANE_ID_ENV_VAR: &str = "SESH_MUX_PANE_ID";
pub(crate) const SESH_MUX_TAB_ID_ENV_VAR: &str = "SESH_MUX_TAB_ID";
pub(crate) const SESH_MUX_WORKSPACE_ID_ENV_VAR: &str = "SESH_MUX_WORKSPACE_ID";

pub(crate) const PI_CODING_AGENT_DIR_ENV_VAR: &str = "PI_CODING_AGENT_DIR";
pub(crate) const CLAUDE_CONFIG_DIR_ENV_VAR: &str = "CLAUDE_CONFIG_DIR";
pub(crate) const CODEX_HOME_ENV_VAR: &str = "CODEX_HOME";

pub(crate) fn apply_pane_base_env(cmd: &mut CommandBuilder) {
    cmd.env(crate::api::SOCKET_PATH_ENV_VAR, crate::api::socket_path());
    if let Ok(executable) = crate::platform::launch_executable() {
        cmd.env("SESH_MUX_BIN_PATH", executable);
    }
}

pub(crate) fn pi_extension_dir() -> io::Result<PathBuf> {
    Ok(
        config_dir_from_env_or_home(PI_CODING_AGENT_DIR_ENV_VAR, &[".pi", "agent"])?
            .join("extensions"),
    )
}

pub(crate) fn claude_dir() -> io::Result<PathBuf> {
    config_dir_from_env_or_home(CLAUDE_CONFIG_DIR_ENV_VAR, &[".claude"])
}

pub(crate) fn codex_dir() -> io::Result<PathBuf> {
    config_dir_from_env_or_home(CODEX_HOME_ENV_VAR, &[".codex"])
}

pub(crate) fn config_dir_from_env_or_home(
    env_var: &str,
    home_relative_segments: &[&str],
) -> io::Result<PathBuf> {
    if let Some(value) = std::env::var_os(env_var).filter(|value| !value.is_empty()) {
        return expand_tilde_path(PathBuf::from(value));
    }

    let mut path = home_dir()?;
    for segment in home_relative_segments {
        path.push(segment);
    }
    Ok(path)
}

pub(crate) fn expand_tilde_path(path: PathBuf) -> io::Result<PathBuf> {
    let Some(raw) = path.to_str() else {
        return Ok(path);
    };

    if raw == "~" {
        return home_dir();
    }

    if let Some(rest) = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\"))
        .or_else(|| raw.strip_prefix('~'))
    {
        return Ok(home_dir()?.join(rest));
    }

    Ok(path)
}

pub(crate) fn home_dir() -> io::Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }

    Err(io::Error::other(
        "home directory is not set; cannot locate home directory",
    ))
}

#[cfg(test)]
pub(crate) struct IntegrationEnvLock {
    _guard: MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for IntegrationEnvLock {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(appdata) = self.appdata.take() {
            std::env::set_var("APPDATA", appdata);
        } else {
            std::env::remove_var("APPDATA");
        }
    }
}

#[cfg(test)]
pub(crate) fn integration_env_lock() -> IntegrationEnvLock {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    IntegrationEnvLock { _guard: guard }
}

#[cfg(test)]
mod tests {}
