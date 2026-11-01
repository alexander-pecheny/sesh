use std::collections::HashSet;

use super::App;
use crate::layout::PaneId;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TerminalTitleChanges {
    pub(crate) raw_changed: bool,
    pub(crate) stripped_changed: bool,
}

impl App {
    pub(crate) fn sync_pending_terminal_titles(&mut self) -> TerminalTitleChanges {
        let sources = self.render_dirty.pending_terminal_title_sources();
        self.sync_terminal_titles(&sources)
    }

    pub(crate) fn sync_terminal_titles(
        &mut self,
        sources: &HashSet<PaneId>,
    ) -> TerminalTitleChanges {
        if sources.is_empty() {
            return TerminalTitleChanges::default();
        }

        let mut observations = Vec::with_capacity(sources.len());
        for pane_id in sources {
            let Some((ws_idx, terminal_id)) = self
                .find_pane(*pane_id)
                .map(|(ws_idx, pane)| (ws_idx, pane.attached_terminal_id.clone()))
            else {
                continue;
            };
            let Some(runtime) = self.terminal_runtimes.get(&terminal_id) else {
                continue;
            };
            observations.push((ws_idx, *pane_id, terminal_id, runtime.terminal_title()));
        }

        let mut changes = TerminalTitleChanges::default();
        let mut publish = Vec::new();
        for (ws_idx, pane_id, terminal_id, title) in observations {
            let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
                continue;
            };
            let change = terminal.set_terminal_title(title);
            changes.raw_changed |= change.raw_changed;
            changes.stripped_changed |= change.stripped_changed;
            if change.stripped_changed {
                publish.push((ws_idx, pane_id));
            }
        }

        for (ws_idx, pane_id) in publish {
            self.emit_pane_updated(ws_idx, pane_id);
        }

        changes
    }
}

#[cfg(test)]
mod tests {}
