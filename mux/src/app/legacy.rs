//! Panes taken over from the server Sesh ran before sesh-mux whose agents still run with that
//! server's environment and hooks.

use super::App;

/// The process group of the agent running in the foreground of a pane's shell, if any.
fn foreground_agent_pid(shell_pid: u32) -> Option<u32> {
    let pid = crate::detect::foreground_process_group_id(shell_pid)?;
    let job = crate::detect::foreground_job(shell_pid)?;
    (pid != shell_pid && crate::detect::identify_agent_in_job(&job).is_some()).then_some(pid)
}

impl App {
    /// Records the agent each imported pane runs, so its legacy hooks can be told apart.
    pub(crate) fn mark_imported_agents_legacy(&mut self) {
        for (terminal_id, terminal) in self.state.terminals.iter_mut() {
            terminal.legacy_agent_pid = self
                .terminal_runtimes
                .get(terminal_id)
                .and_then(|runtime| runtime.child_pid())
                .and_then(foreground_agent_pid);
        }
    }

    /// Forgets legacy agents that exited or left the foreground; says whether any remain.
    pub(crate) fn refresh_legacy_agents(&mut self) -> bool {
        let mut ended = Vec::new();
        for (terminal_id, terminal) in &self.state.terminals {
            let Some(pid) = terminal.legacy_agent_pid else {
                continue;
            };
            let current = self
                .terminal_runtimes
                .get(terminal_id)
                .and_then(|runtime| runtime.child_pid())
                .and_then(crate::detect::foreground_process_group_id);
            if current != Some(pid) || !crate::platform::process_exists(pid) {
                ended.push(terminal_id.clone());
            }
        }
        for terminal_id in &ended {
            if let Some(terminal) = self.state.terminals.get_mut(terminal_id) {
                terminal.legacy_agent_pid = None;
            }
            let pane = self
                .state
                .workspaces
                .iter()
                .enumerate()
                .find_map(|(ws_idx, ws)| {
                    ws.tabs.iter().find_map(|tab| {
                        tab.panes
                            .iter()
                            .find(|(_, pane)| &pane.attached_terminal_id == terminal_id)
                            .map(|(&pane_id, _)| (ws_idx, pane_id))
                    })
                });
            if let Some((ws_idx, pane_id)) = pane {
                self.emit_pane_updated(ws_idx, pane_id);
            }
        }
        if !ended.is_empty() {
            self.state.mark_session_dirty();
        }
        self.state
            .terminals
            .values()
            .any(|terminal| terminal.legacy_agent_pid.is_some())
    }
}
