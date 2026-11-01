//! `~/.sesh/mux/config.toml`, every key optional.

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

pub const CONFIG_PATH_ENV_VAR: &str = "SESH_MUX_CONFIG_PATH";
pub const DEFAULT_SCROLLBACK_LIMIT_BYTES: usize = 10_000_000;
pub const DEFAULT_HEADLESS_COLS: u16 = 120;
pub const DEFAULT_HEADLESS_ROWS: u16 = 40;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NewTerminalCwdConfig {
    #[default]
    Follow,
    Home,
    Current,
    Path(String),
}

impl<'de> Deserialize<'de> for NewTerminalCwdConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.trim() {
            "" | "follow" => Self::Follow,
            "home" => Self::Home,
            "current" => Self::Current,
            _ => Self::Path(value),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellModeConfig {
    #[default]
    Auto,
    Login,
    NonLogin,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Executable for new panes; empty means $SHELL, then /bin/sh.
    pub default_shell: String,
    pub shell_mode: ShellModeConfig,
    pub new_cwd: NewTerminalCwdConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    /// Resume agents into their conversations when the session is restored.
    pub resume_agents_on_restore: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            resume_agents_on_restore: true,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Pane size while no attach client holds a pane.
    pub headless_cols: u16,
    pub headless_rows: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            headless_cols: DEFAULT_HEADLESS_COLS,
            headless_rows: DEFAULT_HEADLESS_ROWS,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct WorktreesConfig {
    /// Root under which worktrees are created as <repo>/<branch-slug>.
    pub directory: String,
}

impl Default for WorktreesConfig {
    fn default() -> Self {
        Self {
            directory: "~/.sesh/worktrees".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AdvancedConfig {
    /// Scrollback retained per pane, in bytes.
    pub scrollback_limit_bytes: usize,
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            scrollback_limit_bytes: DEFAULT_SCROLLBACK_LIMIT_BYTES,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExperimentalConfig {
    /// Save recent pane screen history across full server restarts.
    pub pane_history: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub terminal: TerminalConfig,
    pub session: SessionConfig,
    pub server: ServerConfig,
    pub worktrees: WorktreesConfig,
    pub advanced: AdvancedConfig,
    pub experimental: ExperimentalConfig,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub diagnostics: Vec<String>,
}

pub fn config_dir() -> PathBuf {
    crate::session::data_dir()
}

pub fn config_path() -> PathBuf {
    std::env::var_os(CONFIG_PATH_ENV_VAR)
        .filter(|path| !path.is_empty())
        .map_or_else(|| config_dir().join("config.toml"), PathBuf::from)
}

impl Config {
    pub fn load() -> LoadedConfig {
        let path = config_path();
        let parsed = match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str::<Config>(&content).map_err(|err| err.to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(err) => Err(err.to_string()),
        };
        match parsed {
            Ok(config) => LoadedConfig {
                config,
                diagnostics: Vec::new(),
            },
            Err(err) => {
                let diagnostic = format!("{}: {err}; using defaults", path.display());
                tracing::warn!("{diagnostic}");
                LoadedConfig {
                    config: Config::default(),
                    diagnostics: vec![diagnostic],
                }
            }
        }
    }

    pub(crate) fn headless_size(&self) -> (u16, u16) {
        if self.server.headless_cols == 0 || self.server.headless_rows == 0 {
            (DEFAULT_HEADLESS_COLS, DEFAULT_HEADLESS_ROWS)
        } else {
            (self.server.headless_cols, self.server.headless_rows)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_to_missing_keys() {
        let config: Config = toml::from_str("[server]\nheadless_cols = 100\n").unwrap();
        assert_eq!(config.headless_size(), (100, DEFAULT_HEADLESS_ROWS));
        assert_eq!(config.worktrees.directory, "~/.sesh/worktrees");
        assert!(config.session.resume_agents_on_restore);
    }

    #[test]
    fn unknown_keys_from_old_configs_are_ignored() {
        let config: Config =
            toml::from_str("[ui]\nsidebar_width = 30\n[terminal]\nshell_mode = \"login\"\n")
                .unwrap();
        assert_eq!(config.terminal.shell_mode, ShellModeConfig::Login);
    }
}
