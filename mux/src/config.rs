use crossterm::event::{KeyCode, KeyModifiers};

mod io;
mod keybinds;
mod model;
mod sidebar;
mod sound;
mod tab_bar;
mod theme;
mod window_title;
mod write;

pub use self::{
    io::{
        config_diagnostic_summary, config_dir, config_path, load_live_config,
        remove_keybinding_config_sections, remove_section_key, state_dir, upsert_section_bool,
        upsert_section_value,
    },
    keybinds::{
        format_key_combo, normalize_key_combo, terminal_key_matches_combo, ActionKeybinds,
        BindingConfig, CommandKeybindConfig, CustomCommandAction, CustomCommandKeybind,
        IndexedKeybind, Keybinds, LiveKeybindConfig,
    },
    model::{
        validated_sidebar_bounds, AgentPanelSortConfig, Config, ConfigReloadReport,
        ConfigReloadStatus, HostCursorModeConfig, NewTerminalCwdConfig, PaneBordersConfig,
        ShellModeConfig, SidebarCollapsedModeConfig, StatusIndicatorStyle, TabBarPositionConfig,
        ToastClipboardPosition, ToastConfig, ToastDelivery, ToastHerdrPosition,
        MAX_TOAST_DELAY_SECONDS,
    },
    sidebar::{
        AgentSidebarToken, AgentsSidebarConfig, SidebarConfig, SidebarTokenStyle,
        SpaceSidebarToken, SpacesSidebarConfig,
    },
    sound::SoundConfig,
    tab_bar::TabBarRightEntryConfig,
    theme::{parse_color, CustomThemeColors, ModeThemeColors, ThemeConfig, THEME_NAMES},
    window_title::{WindowTitlePart, WindowTitleTemplate, WindowTitleToken},
};

pub(crate) use self::keybinds::parse_key_combo;
pub(crate) use self::write::{update_file_at, write_edit, ConfigEdit};
pub(crate) use self::{
    io::upsert_top_level_bool,
    tab_bar::{
        parse_tab_bar_datetime_format, tab_bar_right_diagnostics,
        MAX_TAB_BAR_COMMAND_INTERVAL_SECONDS, MAX_TAB_BAR_COMMAND_TIMEOUT_SECONDS,
        MAX_TAB_BAR_RIGHT_ENTRIES,
    },
    theme::canonical_theme_name,
    window_title::{sanitize_window_title_text, window_title_diagnostics},
};

pub(crate) use self::{keybinds::CommandKeybindType, model::KeysConfig};

pub const CONFIG_PATH_ENV_VAR: &str = "HERDR_CONFIG_PATH";

pub(crate) fn is_keybinding_config_diagnostic(diagnostic: &str) -> bool {
    if diagnostic.starts_with("config parse error:") || diagnostic.starts_with("config read error:")
    {
        return false;
    }
    diagnostic.contains("keybinding") || diagnostic.contains("keys.")
}

pub(crate) fn config_diagnostic_summary_without_keybindings(
    diagnostics: &[String],
) -> Option<String> {
    let diagnostics = diagnostics
        .iter()
        .filter(|diagnostic| !is_keybinding_config_diagnostic(diagnostic))
        .cloned()
        .collect::<Vec<_>>();
    config_diagnostic_summary(&diagnostics)
}
pub const DEFAULT_SCROLLBACK_LIMIT_BYTES: usize = 10_000_000;
pub const DEFAULT_MOUSE_SCROLL_LINES: usize = 3;
pub const DEFAULT_MOBILE_WIDTH_THRESHOLD: u16 = 64;
pub const DEFAULT_HEADLESS_COLS: u16 = 120;
pub const DEFAULT_HEADLESS_ROWS: u16 = 40;

#[cfg(test)]
pub(crate) fn app_dir_name() -> &'static str {
    io::app_dir_name()
}

#[cfg(test)]
pub(crate) fn test_config_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

impl Config {
    pub fn should_show_onboarding(&self) -> bool {
        self.onboarding.unwrap_or(true)
    }

    pub fn kitty_graphics_enabled(&self) -> bool {
        self.terminal
            .kitty_graphics
            .or(self.experimental.kitty_graphics)
            .unwrap_or(true)
    }

    pub fn prefix_key(&self) -> (KeyCode, KeyModifiers) {
        self.validated_keybinds().1
    }

    /// Parsed keybinds for Herdr actions.
    pub fn keybinds(&self) -> Keybinds {
        self.validated_keybinds().3
    }

    pub fn collect_diagnostics(&self) -> Vec<String> {
        let (prefix_diag, _, keybind_diags, _) = self.validated_keybinds();
        prefix_diag
            .into_iter()
            .chain(keybind_diags)
            .chain(self.theme.diagnostics())
            .chain(self.ui.sound.diagnostics())
            .chain(tab_bar_right_diagnostics(&self.ui.tab_bar_right))
            .chain(window_title_diagnostics(&self.ui.window_title))
            .chain(self.invalid_sidebar_bounds_diagnostic())
            .chain(self.invalid_headless_size_diagnostic())
            .collect()
    }

    pub(crate) fn headless_size(&self) -> (u16, u16) {
        if self.invalid_headless_size_diagnostic().is_some() {
            (DEFAULT_HEADLESS_COLS, DEFAULT_HEADLESS_ROWS)
        } else {
            (self.server.headless_cols, self.server.headless_rows)
        }
    }

    pub(crate) fn invalid_headless_size_diagnostic(&self) -> Option<String> {
        (self.server.headless_cols == 0 || self.server.headless_rows == 0).then(|| {
            format!(
                "server.headless_cols and server.headless_rows must be greater than zero (got {}x{})",
                self.server.headless_cols, self.server.headless_rows
            )
        })
    }

    pub(crate) fn invalid_sidebar_bounds_diagnostic(&self) -> Option<String> {
        validated_sidebar_bounds(self.ui.sidebar_min_width, self.ui.sidebar_max_width)
            .is_none()
            .then(|| {
                format!(
                    "ui.sidebar_min_width ({}) is greater than sidebar_max_width ({})",
                    self.ui.sidebar_min_width, self.ui.sidebar_max_width
                )
            })
    }

    pub(crate) fn live_keybinds_with_diagnostics(
        &self,
    ) -> Result<(LiveKeybindConfig, Vec<String>), Vec<String>> {
        let (prefix_diag, prefix, keybind_diags, keybinds) = self.validated_keybinds();
        if let Some(prefix_diag) = prefix_diag {
            Err(std::iter::once(prefix_diag).chain(keybind_diags).collect())
        } else {
            Ok((LiveKeybindConfig { prefix, keybinds }, keybind_diags))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_host_cursor_defaults_to_auto_and_parses_overrides() {
        let default_config = Config::default();
        assert_eq!(default_config.ui.host_cursor, HostCursorModeConfig::Auto);

        let native: Config = toml::from_str("[ui]\nhost_cursor = 'native'\n").unwrap();
        assert_eq!(native.ui.host_cursor, HostCursorModeConfig::Native);

        let drawn: Config = toml::from_str("[ui]\nhost_cursor = 'drawn'\n").unwrap();
        assert_eq!(drawn.ui.host_cursor, HostCursorModeConfig::Drawn);
    }
}
