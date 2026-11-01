use crate::config::NewTerminalCwdConfig;
use crate::layout::PaneId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneFocusTarget {
    pub workspace_id: String,
    pub pane_id: PaneId,
}

/// All application state: pure data, no channels or async runtime.
pub struct AppState {
    pub terminals:
        std::collections::HashMap<crate::terminal::TerminalId, crate::terminal::TerminalState>,
    /// Terminal ids whose size is currently owned by a direct attach client.
    pub direct_attach_resize_locks: std::collections::HashSet<crate::terminal::TerminalId>,
    pub(crate) pane_id_aliases: std::collections::HashMap<u32, PaneId>,
    pub(crate) public_pane_id_aliases: std::collections::HashMap<String, PaneId>,
    pub workspaces: Vec<crate::workspace::Workspace>,
    pub active: Option<usize>,
    pub(crate) previous_pane_focus: Option<PaneFocusTarget>,
    pub selected: usize,
    pub should_quit: bool,
    pub worktree_directory: std::path::PathBuf,
    /// Size of every pane no attach client holds, as (cols, rows).
    pub(crate) headless_size: (u16, u16),
    pub next_agent_state_change_seq: u64,
    pub copy_on_select: bool,
    pub default_shell: String,
    pub shell_mode: crate::config::ShellModeConfig,
    pub new_terminal_cwd: NewTerminalCwdConfig,
    pub pane_scrollback_limit_bytes: usize,
    pub host_terminal_theme: crate::terminal_theme::TerminalTheme,
    pub host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
    /// Set when a persisted session snapshot would change.
    pub session_dirty: bool,
    /// Terminal runtimes to shut down after state has detached their metadata.
    pub(crate) terminal_runtime_shutdowns: Vec<crate::terminal::TerminalId>,
}

impl AppState {
    pub(crate) fn new(
        config: &crate::config::Config,
        workspaces: Vec<crate::workspace::Workspace>,
        active: Option<usize>,
        selected: usize,
    ) -> Self {
        Self {
            terminals: std::collections::HashMap::new(),
            direct_attach_resize_locks: std::collections::HashSet::new(),
            pane_id_aliases: std::collections::HashMap::new(),
            public_pane_id_aliases: std::collections::HashMap::new(),
            workspaces,
            active,
            previous_pane_focus: None,
            selected,
            should_quit: false,
            worktree_directory: crate::worktree::expand_tilde_absolute_path(
                &config.worktrees.directory,
            ),
            headless_size: config.headless_size(),
            next_agent_state_change_seq: 0,
            copy_on_select: true,
            default_shell: config.terminal.default_shell.clone(),
            shell_mode: config.terminal.shell_mode,
            new_terminal_cwd: config.terminal.new_cwd.clone(),
            pane_scrollback_limit_bytes: config.advanced.scrollback_limit_bytes,
            host_terminal_theme: crate::terminal_theme::TerminalTheme::default(),
            host_terminal_appearance: None,
            session_dirty: false,
            terminal_runtime_shutdowns: Vec::new(),
        }
    }
}

impl AppState {
    pub(crate) fn mark_session_dirty(&mut self) {
        self.session_dirty = true;
    }

    pub(crate) fn remove_alias_shadowed_by_new_pane(&mut self, pane_id: PaneId) {
        self.pane_id_aliases.remove(&pane_id.raw());
    }

    /// (rows, cols) for a new pane.
    pub fn estimate_pane_size(&self) -> (u16, u16) {
        (self.headless_size.1, self.headless_size.0)
    }

    /// Returns true when the given (workspace, tab, pane) refers to the
    /// currently focused pane in the active workspace's active tab.
    pub(crate) fn runtime_for_pane_in_workspace<'a>(
        &'a self,
        terminal_runtimes: &'a crate::terminal::TerminalRuntimeRegistry,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<&'a crate::terminal::TerminalRuntime> {
        #[cfg(test)]
        if let Some(runtime) = self.workspaces.get(ws_idx)?.test_runtimes.get(&pane_id) {
            return Some(runtime);
        }
        #[cfg(test)]
        if let Some(runtime) = self
            .workspaces
            .get(ws_idx)?
            .tabs
            .iter()
            .find_map(|tab| tab.runtimes.get(&pane_id))
        {
            return Some(runtime);
        }
        let terminal_id = self.workspaces.get(ws_idx)?.terminal_id(pane_id)?;
        terminal_runtimes.get(terminal_id)
    }
}

#[cfg(test)]
impl AppState {
    /// An AppState for testing: no channels, no PTYs.
    pub fn test_new() -> Self {
        let mut state = Self::new(&crate::config::Config::default(), Vec::new(), None, 0);
        state.worktree_directory = std::path::PathBuf::from("/tmp/sesh-mux-worktrees");
        state
    }

    pub fn ensure_test_terminals(&mut self) {
        use crate::terminal::TerminalState;
        for ws in &self.workspaces {
            for tab in &ws.tabs {
                for pane in tab.panes.values() {
                    if !self.terminals.contains_key(&pane.attached_terminal_id) {
                        let cwd = ws.identity_cwd.clone();
                        self.terminals.insert(
                            pane.attached_terminal_id.clone(),
                            TerminalState::new(pane.attached_terminal_id.clone(), cwd),
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_size_estimate_uses_headless_size_before_first_view() {
        let mut state = AppState::test_new();
        state.headless_size = (132, 41);

        assert_eq!(state.estimate_pane_size(), (41, 132));
    }
}
