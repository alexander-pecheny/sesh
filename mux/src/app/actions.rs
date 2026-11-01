//! Pure state mutations on AppState.
//! These don't need channels, async, or PTY runtime.

use std::time::Instant;

use tracing::warn;

use crate::detect::{Agent, AgentState};
use crate::events::AppEvent;
use crate::layout::PaneId;
use crate::terminal::{EffectiveStateChange, TerminalStateMutation};

use super::api_helpers::pane_agent_status;
use super::state::{AppState, PaneFocusTarget};

fn is_background_completion_transition(prev_state: AgentState, new_state: AgentState) -> bool {
    matches!(new_state, AgentState::Idle)
        && matches!(prev_state, AgentState::Working | AgentState::Blocked)
}

fn is_completion_transition(change: &EffectiveStateChange) -> bool {
    is_background_completion_transition(change.previous_state, change.state)
}

fn public_tab_id_for_index(ws: &crate::workspace::Workspace, tab_idx: usize) -> Option<String> {
    let tab_number = ws.public_tab_number(tab_idx)?;
    Some(crate::workspace::public_tab_id_for_number(
        &ws.id, tab_number,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneStateUpdate {
    pub pane_id: PaneId,
    pub ws_idx: usize,
    pub previous_agent_label: Option<String>,
    pub previous_known_agent: Option<Agent>,
    pub previous_state: AgentState,
    pub previous_seen: bool,
    pub previous_presentation: crate::terminal::EffectivePresentation,
    pub agent_label: Option<String>,
    pub known_agent: Option<Agent>,
    pub state: AgentState,
    pub seen: bool,
    pub presentation: crate::terminal::EffectivePresentation,
    pub agent_name_changed: bool,
    pub agent_released: bool,
    pub agent_release_status: Option<crate::api::schema::AgentStatus>,
    pub suppress_completion: bool,
}

// ---------------------------------------------------------------------------
// Focus tracking
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn current_pane_focus_target(&self) -> Option<PaneFocusTarget> {
        let ws_idx = self.active?;
        let ws = self.workspaces.get(ws_idx)?;
        let pane_id = ws.focused_pane_id()?;
        Some(PaneFocusTarget {
            workspace_id: ws.id.clone(),
            pane_id,
        })
    }

    fn record_pane_focus_after_navigation(&mut self, previous: Option<PaneFocusTarget>) {
        let current = self.current_pane_focus_target();
        if previous != current {
            self.previous_pane_focus = previous;
        }
    }

    pub(crate) fn focus_pane_in_workspace(&mut self, ws_idx: usize, pane_id: PaneId) -> bool {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return false;
        };
        let Some(tab_idx) = ws.find_tab_index_for_pane(pane_id) else {
            return false;
        };
        let previous = self.current_pane_focus_target();
        let target = PaneFocusTarget {
            workspace_id: ws.id.clone(),
            pane_id,
        };
        if previous.as_ref() == Some(&target) {
            return false;
        }

        self.switch_workspace_tab(ws_idx, tab_idx);
        if let Some(tab) = self
            .workspaces
            .get_mut(ws_idx)
            .and_then(|ws| ws.tabs.get_mut(tab_idx))
        {
            tab.layout.focus_pane(pane_id);
            self.previous_pane_focus = previous;
            self.mark_session_dirty();
            return true;
        }
        false
    }
}

// ---------------------------------------------------------------------------
// Workspace operations
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn next_agent_metadata_expiry(&self) -> Option<std::time::Instant> {
        self.terminals
            .values()
            .filter_map(|terminal| terminal.next_agent_metadata_expiry())
            .chain(
                self.terminals
                    .values()
                    .filter_map(|terminal| terminal.metadata_tokens.next_expiry()),
            )
            .chain(
                self.workspaces
                    .iter()
                    .filter_map(|workspace| workspace.metadata_tokens.next_expiry()),
            )
            .min()
    }

    pub(crate) fn expire_agent_metadata_at(
        &mut self,
        scheduled_deadline: std::time::Instant,
        now: std::time::Instant,
    ) -> Vec<PaneStateUpdate> {
        let pane_terminals: Vec<_> = self
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, ws)| {
                ws.tabs.iter().flat_map(move |tab| {
                    tab.layout
                        .pane_ids()
                        .into_iter()
                        .filter_map(move |pane_id| {
                            ws.pane_state(pane_id)
                                .map(|pane| (ws_idx, pane_id, pane.attached_terminal_id.clone()))
                        })
                })
            })
            .collect();
        pane_terminals
            .into_iter()
            .filter_map(|(ws_idx, pane_id, terminal_id)| {
                let previous_seen = self.workspaces[ws_idx].pane_state(pane_id)?.seen;
                let mutation = self
                    .terminals
                    .get_mut(&terminal_id)?
                    .expire_agent_metadata_at(scheduled_deadline, now)?;
                let change = mutation.effective_state_change?;
                let seen = self.apply_pane_state_change(ws_idx, pane_id, &change, false)?;
                let update = PaneStateUpdate {
                    pane_id,
                    ws_idx,
                    previous_agent_label: change.previous_agent_label.clone(),
                    previous_known_agent: change.previous_known_agent,
                    previous_state: change.previous_state,
                    previous_seen,
                    previous_presentation: change.previous_presentation.clone(),
                    agent_label: change.agent_label.clone(),
                    known_agent: change.known_agent,
                    state: change.state,
                    seen,
                    presentation: change.presentation.clone(),
                    agent_name_changed: false,
                    agent_released: false,
                    agent_release_status: None,
                    suppress_completion: false,
                };
                Some(update)
            })
            .collect()
    }

    pub(crate) fn expire_metadata_tokens(
        &mut self,
        now: std::time::Instant,
    ) -> (Vec<(usize, PaneId)>, Vec<usize>) {
        let pane_terminals = self
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, workspace)| {
                workspace.tabs.iter().flat_map(move |tab| {
                    tab.layout
                        .pane_ids()
                        .into_iter()
                        .filter_map(move |pane_id| {
                            workspace
                                .pane_state(pane_id)
                                .map(|pane| (ws_idx, pane_id, pane.attached_terminal_id.clone()))
                        })
                })
            })
            .collect::<Vec<_>>();
        let changed_panes = pane_terminals
            .into_iter()
            .filter_map(|(ws_idx, pane_id, terminal_id)| {
                let terminal = self.terminals.get_mut(&terminal_id)?;
                terminal.metadata_tokens.expire_at(now).then(|| {
                    terminal.revision = terminal.revision.saturating_add(1);
                    (ws_idx, pane_id)
                })
            })
            .collect();
        let changed_workspaces = self
            .workspaces
            .iter_mut()
            .enumerate()
            .filter_map(|(ws_idx, workspace)| {
                workspace.metadata_tokens.expire_at(now).then_some(ws_idx)
            })
            .collect();
        (changed_panes, changed_workspaces)
    }

    pub fn switch_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            let previous_focus = self.current_pane_focus_target();
            self.active = Some(idx);
            self.selected = idx;
            let workspace_id = self.workspaces[idx].id.clone();
            crate::logging::workspace_focused(&workspace_id);
            self.mark_session_dirty();
            if let Some(ws) = self.workspaces.get_mut(idx) {
                let active_tab = ws.active_tab;
                ws.switch_tab(active_tab);
                let tab_id =
                    public_tab_id_for_index(ws, active_tab).unwrap_or_else(|| workspace_id.clone());
                crate::logging::tab_focused(&workspace_id, &tab_id);
            }
            self.record_pane_focus_after_navigation(previous_focus);
        }
    }

    pub(crate) fn switch_workspace_tab(&mut self, ws_idx: usize, tab_idx: usize) -> bool {
        if ws_idx >= self.workspaces.len() {
            return false;
        }
        if self
            .workspaces
            .get(ws_idx)
            .is_none_or(|ws| tab_idx >= ws.tabs.len())
        {
            return false;
        }

        let previous_focus = self.current_pane_focus_target();
        let workspace_changed = self.active != Some(ws_idx);
        self.active = Some(ws_idx);
        self.selected = ws_idx;
        let workspace_id = self.workspaces[ws_idx].id.clone();
        if workspace_changed {
            crate::logging::workspace_focused(&workspace_id);
        }
        self.mark_session_dirty();
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.switch_tab(tab_idx);
            let tab_id =
                public_tab_id_for_index(ws, tab_idx).unwrap_or_else(|| workspace_id.clone());
            crate::logging::tab_focused(&workspace_id, &tab_id);
        }
        self.record_pane_focus_after_navigation(previous_focus);
        true
    }

    pub(crate) fn terminal_ids_for_workspace(
        &self,
        ws_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.workspaces
            .get(ws_idx)
            .into_iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect()
    }

    pub(crate) fn pane_ids_for_workspace(&self, ws_idx: usize) -> Vec<PaneId> {
        self.workspaces
            .get(ws_idx)
            .into_iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| tab.layout.pane_ids())
            .collect()
    }

    pub(crate) fn terminal_ids_for_tab(
        &self,
        ws_idx: usize,
        tab_idx: usize,
    ) -> Vec<crate::terminal::TerminalId> {
        self.workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs.get(tab_idx))
            .into_iter()
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect()
    }

    pub(crate) fn terminal_id_for_pane(
        &self,
        ws_idx: usize,
        pane_id: PaneId,
    ) -> Option<crate::terminal::TerminalId> {
        self.workspaces
            .get(ws_idx)?
            .pane_state(pane_id)
            .map(|pane| pane.attached_terminal_id.clone())
    }

    pub(crate) fn remove_unattached_terminal_ids(
        &mut self,
        terminal_ids: impl IntoIterator<Item = crate::terminal::TerminalId>,
    ) {
        for terminal_id in terminal_ids {
            let still_attached = self.workspaces.iter().any(|ws| {
                ws.tabs.iter().any(|tab| {
                    tab.panes
                        .values()
                        .any(|pane| pane.attached_terminal_id == terminal_id)
                })
            });
            if !still_attached
                && self.terminals.remove(&terminal_id).is_some()
                && !self.terminal_runtime_shutdowns.contains(&terminal_id)
            {
                self.terminal_runtime_shutdowns.push(terminal_id);
            }
        }
    }

    pub fn close_selected_workspace(&mut self) {
        if self.workspaces.is_empty() {
            return;
        }
        self.mark_session_dirty();
        let close_indices = self.workspace_close_indices(self.selected);

        let mut terminal_ids = Vec::new();
        let mut pane_ids = Vec::new();
        for idx in &close_indices {
            terminal_ids.extend(self.terminal_ids_for_workspace(*idx));
            pane_ids.extend(self.pane_ids_for_workspace(*idx));
            if let Some(workspace_id) = self.workspaces.get(*idx).map(|ws| ws.id.clone()) {
                crate::logging::workspace_closed(&workspace_id);
            }
        }
        let active_workspace_id = self
            .active
            .and_then(|idx| self.workspaces.get(idx))
            .map(|ws| ws.id.clone());
        for idx in close_indices.iter().rev() {
            self.workspaces.remove(*idx);
        }
        self.remove_unattached_terminal_ids(terminal_ids);
        if self.workspaces.is_empty() {
            self.active = None;
            self.selected = 0;
        } else {
            // Keep focus on the previously focused workspace
            if let Some(id) = active_workspace_id {
                if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == id) {
                    self.selected = idx;
                }
            }
            if self.selected >= self.workspaces.len() {
                self.selected = self.workspaces.len() - 1;
            }
            self.active = Some(self.selected);
        }
    }
}

// ---------------------------------------------------------------------------
// Pane operations
// ---------------------------------------------------------------------------

impl AppState {
    pub(crate) fn workspace_close_indices(&self, ws_idx: usize) -> Vec<usize> {
        self.workspaces
            .get(ws_idx)
            .and_then(|ws| ws.worktree_space())
            .filter(|space| !space.is_linked_worktree)
            .map(|space| {
                self.workspaces
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, ws)| {
                        ws.worktree_space()
                            .is_some_and(|member| member.key == space.key)
                            .then_some(idx)
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|indices| indices.len() >= 2)
            .unwrap_or_else(|| vec![ws_idx])
    }

    pub(crate) fn workspace_close_would_close_worktree_group(&self, ws_idx: usize) -> bool {
        self.workspace_close_indices(ws_idx).len() >= 2
    }

    pub(crate) fn confirm_implicit_worktree_group_close(&self, ws_idx: usize) -> bool {
        self.workspace_close_would_close_worktree_group(ws_idx)
    }

    pub(crate) fn close_pane_would_close_workspace(&self, ws_idx: usize, pane_id: PaneId) -> bool {
        self.workspaces.get(ws_idx).is_some_and(|ws| {
            ws.find_tab_index_for_pane(pane_id).is_some_and(|tab_idx| {
                ws.tabs[tab_idx].layout.pane_count() <= 1 && ws.tabs.len() <= 1
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextCell {
    ch: char,
    start_col: u16,
    end_col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellSpan {
    start: usize,
    end: usize,
}

impl CellSpan {
    fn contains(self, idx: usize) -> bool {
        idx >= self.start && idx <= self.end
    }

    fn columns(self, cells: &[TextCell]) -> (u16, u16) {
        (cells[self.start].start_col, cells[self.end].end_col)
    }
}

/// Finds the terminal display-column bounds for the token under a double-click.
///
/// The algorithm first maps text to terminal cells so wide characters and
/// zero-width marks use display columns, then prefers structured spans that
/// users expect to copy whole (URLs and quoted paths), and finally falls back
/// to a separator-delimited token.
pub(crate) fn word_bounds_at_column(row: &str, col: u16) -> Option<(u16, u16)> {
    // Map the row into display cells before doing any word-boundary work.
    let cells = text_cells(row);
    let clicked_idx = cell_index_at_column(&cells, col)?;

    // Prefer spans that can legally include punctuation or spaces.
    let span = url_span_at_column(&cells, clicked_idx)
        .or_else(|| quoted_path_span_at_column(&cells, clicked_idx))
        .or_else(|| token_span_at_column(&cells, clicked_idx))?;

    // Convert the internal cell span back to inclusive terminal columns.
    Some(span.columns(&cells))
}

fn token_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    if is_word_separator(cells[clicked_idx].ch) {
        return None;
    }

    let mut start = clicked_idx;
    while start > 0 && !is_word_separator(cells[start - 1].ch) {
        start -= 1;
    }

    let mut end = clicked_idx;
    while end + 1 < cells.len() && !is_word_separator(cells[end + 1].ch) {
        end += 1;
    }

    trim_token_edges(cells, CellSpan { start, end }).filter(|span| span.contains(clicked_idx))
}

fn text_cells(row: &str) -> Vec<TextCell> {
    let mut next_col = 0u16;
    row.chars()
        .map(|ch| {
            let width = u16::from(crate::ghostty::unicode_codepoint_width(ch as u32));
            let start_col = if width == 0 {
                next_col.saturating_sub(1)
            } else {
                next_col
            };
            if width > 0 {
                next_col = next_col.saturating_add(width);
            }
            TextCell {
                ch,
                start_col,
                end_col: next_col.saturating_sub(1),
            }
        })
        .collect()
}

fn cell_index_at_column(cells: &[TextCell], col: u16) -> Option<usize> {
    cells
        .iter()
        .position(|cell| cell.start_col <= col && col <= cell.end_col)
}

fn url_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    let mut start = 0;
    while start < cells.len() {
        if starts_with_chars(&cells[start..], "http://")
            || starts_with_chars(&cells[start..], "https://")
        {
            let mut end = start;
            while end + 1 < cells.len() && !cells[end + 1].ch.is_whitespace() {
                end += 1;
            }
            if clicked_idx >= start && clicked_idx <= end {
                let span = trim_url_edges(cells, CellSpan { start, end })?;
                return span.contains(clicked_idx).then_some(span);
            }
            start = end + 1;
        } else {
            start += 1;
        }
    }
    None
}

fn trim_url_edges(cells: &[TextCell], span: CellSpan) -> Option<CellSpan> {
    let start = span.start;
    let mut end = span.end;
    while start <= end && should_trim_trailing_url_cell(cells, start, end) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn should_trim_trailing_url_cell(cells: &[TextCell], start: usize, end: usize) -> bool {
    match cells[end].ch {
        '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?' => true,
        ')' => !trailing_url_closer_is_balanced(cells, start, end, '(', ')'),
        ']' => !trailing_url_closer_is_balanced(cells, start, end, '[', ']'),
        '}' => !trailing_url_closer_is_balanced(cells, start, end, '{', '}'),
        _ => false,
    }
}

fn trailing_url_closer_is_balanced(
    cells: &[TextCell],
    start: usize,
    end: usize,
    open: char,
    close: char,
) -> bool {
    let mut balance = 0i32;
    for cell in &cells[start..end] {
        if cell.ch == open {
            balance += 1;
        } else if cell.ch == close {
            balance -= 1;
        }
    }
    balance > 0
}

fn quoted_path_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    let clicked = cells.get(clicked_idx)?.ch;
    if clicked == '"' || clicked == '\'' || clicked == '`' {
        return None;
    }

    for quote in ['"', '\'', '`'] {
        let mut start = None;
        for (idx, cell) in cells.iter().copied().enumerate() {
            let ch = cell.ch;
            if ch != quote || is_escaped(cells, idx) {
                continue;
            }
            if let Some(open) = start {
                if clicked_idx > open
                    && clicked_idx < idx
                    && cells[open + 1..idx].iter().any(|cell| cell.ch == '/')
                {
                    return Some(CellSpan {
                        start: open + 1,
                        end: idx - 1,
                    });
                }
                start = None;
            } else {
                start = Some(idx);
            }
        }
    }
    None
}

fn is_escaped(cells: &[TextCell], idx: usize) -> bool {
    let mut slashes = 0;
    let mut cursor = idx;
    while cursor > 0 && cells[cursor - 1].ch == '\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

fn starts_with_chars(cells: &[TextCell], prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(idx, expected)| cells.get(idx).is_some_and(|cell| cell.ch == expected))
}

fn is_word_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '|' | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | ','
                | ';'
                | '!'
                | '（'
                | '）'
                | '：'
                | '、'
                | '。'
                | '，'
        )
}

fn trim_token_edges(cells: &[TextCell], span: CellSpan) -> Option<CellSpan> {
    let mut start = span.start;
    let mut end = span.end;
    while start <= end && is_leading_token_wrapper(cells[start].ch) {
        start += 1;
    }
    if start < end && cells[end].ch == '$' && is_trailing_token_wrapper(cells[end - 1].ch) {
        end -= 1;
    }
    while start <= end && is_trailing_token_wrapper(cells[end].ch) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn is_leading_token_wrapper(ch: char) -> bool {
    matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'' | '`')
}

fn is_trailing_token_wrapper(ch: char) -> bool {
    matches!(
        ch,
        ')' | ']' | '}' | '>' | '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?'
    )
}

// ---------------------------------------------------------------------------
// Event handling
// ---------------------------------------------------------------------------

impl AppState {
    pub fn handle_app_event(&mut self, event: AppEvent) -> Vec<PaneStateUpdate> {
        match event {
            AppEvent::PaneDied { pane_id, .. } => {
                self.handle_pane_died(pane_id);
                Vec::new()
            }
            AppEvent::WorktreeRuntimeRestoreFailed { .. } => Vec::new(),
            AppEvent::AgentProcessDetected {
                pane_id,
                agent,
                observed_at,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    Some(terminal.set_detected_agent_process_at(agent, observed_at))
                })
                .into_iter()
                .collect(),
            AppEvent::StateChanged {
                pane_id,
                agent,
                state,
                visible_blocker,
                visible_working,
                process_exited,
                observed_at,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    Some(terminal.set_detected_state_with_screen_signals_at(
                        agent,
                        state,
                        visible_blocker,
                        false,
                        visible_working,
                        process_exited,
                        observed_at,
                    ))
                })
                .into_iter()
                .collect(),
            AppEvent::HookStateReported {
                pane_id,
                source,
                agent_label,
                state,
                message,
                seq,
                session_ref,
            } => {
                if crate::agent_resume::is_reserved_native_state_source(&source, &agent_label) {
                    self.update_terminal_state(pane_id, |terminal| {
                        terminal.set_agent_session_ref(source, agent_label, session_ref, seq)
                    })
                    .into_iter()
                    .collect()
                } else {
                    self.update_terminal_state(pane_id, |terminal| {
                        terminal.set_hook_authority_with_session_ref(
                            source,
                            agent_label,
                            state,
                            message,
                            session_ref,
                            seq,
                        )
                    })
                    .into_iter()
                    .collect()
                }
            }
            AppEvent::AgentSessionReported {
                pane_id,
                source,
                agent_label,
                seq,
                session_ref,
                session_start_source,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    if session_start_source.is_some() {
                        terminal.agent_session_start = session_start_source.clone();
                    }
                    terminal.set_agent_session_ref_for_session_start(
                        source,
                        agent_label,
                        session_ref,
                        seq,
                        session_start_source,
                    )
                })
                .into_iter()
                .collect(),
            AppEvent::HookMetadataReported {
                pane_id,
                source,
                agent_label,
                applies_to_source,
                title,
                display_agent,
                state_labels,
                clear_title,
                clear_display_agent,
                clear_state_labels,
                seq,
                ttl,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    terminal.set_agent_metadata(crate::terminal::AgentMetadataReport {
                        source,
                        agent_label,
                        applies_to_source,
                        title,
                        display_agent,
                        state_labels,
                        clear_title,
                        clear_display_agent,
                        clear_state_labels,
                        ttl,
                        seq,
                    })
                })
                .into_iter()
                .collect(),
            AppEvent::HookAuthorityCleared {
                pane_id,
                source,
                seq,
            } => self
                .update_terminal_state(pane_id, |terminal| {
                    terminal.clear_hook_authority_with_mutation(source.as_deref(), seq)
                })
                .into_iter()
                .collect(),
            AppEvent::HookAgentReleased {
                pane_id,
                source,
                agent_label,
                seq,
                ..
            } => {
                if crate::agent_resume::is_official_agent_source(&source, &agent_label) {
                    Vec::new()
                } else {
                    self.update_terminal_state(pane_id, |terminal| {
                        terminal.release_agent_with_mutation(&source, &agent_label, seq)
                    })
                    .into_iter()
                    .collect()
                }
            }
            // Host-local effects are intercepted by HeadlessServer and forwarded to the
            // foreground client; they never touch AppState. Kept for AppEvent exhaustiveness.
            AppEvent::TerminalBell { .. } => Vec::new(),
            AppEvent::ClipboardWrite { .. } => Vec::new(),
            AppEvent::TerminalCwdReported { pane_id, cwd } => {
                if !cwd.is_absolute() || !cwd.is_dir() {
                    return Vec::new();
                }
                let Some(terminal_id) = self.workspaces.iter().find_map(|ws| {
                    ws.pane_state(pane_id)
                        .map(|pane| pane.attached_terminal_id.clone())
                }) else {
                    return Vec::new();
                };
                let Some(terminal) = self.terminals.get_mut(&terminal_id) else {
                    return Vec::new();
                };
                if terminal.cwd != cwd {
                    terminal.cwd = cwd;
                    self.mark_session_dirty();
                }
                Vec::new()
            }
            AppEvent::WorktreeAddFinished(_) => Vec::new(),
            AppEvent::WorktreeRemoveFinished(_) => Vec::new(),
        }
    }

    fn update_terminal_state<F>(&mut self, pane_id: PaneId, update: F) -> Option<PaneStateUpdate>
    where
        F: FnOnce(&mut crate::terminal::TerminalState) -> Option<TerminalStateMutation>,
    {
        self.update_terminal_state_with_completion_policy(pane_id, false, update)
    }

    fn update_terminal_state_with_completion_policy<F>(
        &mut self,
        pane_id: PaneId,
        force_suppress_completion: bool,
        update: F,
    ) -> Option<PaneStateUpdate>
    where
        F: FnOnce(&mut crate::terminal::TerminalState) -> Option<TerminalStateMutation>,
    {
        let ws_idx = self
            .workspaces
            .iter()
            .position(|ws| ws.pane_state(pane_id).is_some())?;
        let terminal_id = self.workspaces[ws_idx]
            .pane_state(pane_id)?
            .attached_terminal_id
            .clone();
        let previous_seen = self.workspaces[ws_idx].pane_state(pane_id)?.seen;
        let now = Instant::now();
        let (
            mutation,
            managed_changed,
            agent_name_changed,
            unchanged_change,
            suppress_acquisition_completion,
            completion_reset,
        ) = {
            let terminal = self.terminals.get_mut(&terminal_id)?;
            let previous_agent_name = terminal.agent_name.clone();
            let had_completion = terminal.last_agent_completion_seq.is_some() || !previous_seen;
            let mutation = update(terminal)?;
            let completion_reset = mutation.session_ref_changed
                || mutation
                    .effective_state_change
                    .as_ref()
                    .is_some_and(|change| change.previous_agent_label != change.agent_label);
            if completion_reset {
                terminal.last_agent_completion_seq = None;
            }
            let managed_changed = terminal.reconcile_managed_agent_at(now, false);
            let suppress_acquisition_completion = terminal.finish_agent_process_acquisition();
            let agent_name_changed = terminal.agent_name != previous_agent_name;
            let unchanged_change = (mutation.agent_released
                || agent_name_changed
                || (completion_reset && had_completion))
                .then(|| terminal.unchanged_effective_state_change_at(now));
            (
                mutation,
                managed_changed,
                agent_name_changed,
                unchanged_change,
                suppress_acquisition_completion,
                completion_reset,
            )
        };
        if completion_reset {
            self.workspaces[ws_idx].pane_state_mut(pane_id)?.seen = true;
        }
        if mutation.session_ref_changed || managed_changed || agent_name_changed {
            self.mark_session_dirty();
        }
        let agent_released = mutation.agent_released;
        let change = mutation.effective_state_change.or(unchanged_change)?;
        let suppress_completion = force_suppress_completion
            || (change.state == AgentState::Idle && suppress_acquisition_completion);
        if change.previous_state != change.state {
            self.next_agent_state_change_seq += 1;
            if let Some(terminal) = self.terminals.get_mut(&terminal_id) {
                terminal.last_agent_state_change_seq = Some(self.next_agent_state_change_seq);
                terminal.last_agent_completion_seq = (!suppress_completion
                    && is_completion_transition(&change))
                .then_some(self.next_agent_state_change_seq);
            }
        }
        let seen = self.apply_pane_state_change(ws_idx, pane_id, &change, suppress_completion)?;
        let update = PaneStateUpdate {
            pane_id,
            ws_idx,
            previous_agent_label: change.previous_agent_label.clone(),
            previous_known_agent: change.previous_known_agent,
            previous_state: change.previous_state,
            previous_seen,
            previous_presentation: change.previous_presentation.clone(),
            agent_label: if agent_released {
                change.previous_agent_label.clone()
            } else {
                change.agent_label.clone()
            },
            known_agent: if agent_released {
                change.previous_known_agent
            } else {
                change.known_agent
            },
            state: change.state,
            seen,
            presentation: change.presentation.clone(),
            agent_name_changed,
            agent_released,
            agent_release_status: agent_released.then(|| pane_agent_status(change.state, seen)),
            suppress_completion,
        };
        Some(update)
    }

    pub(crate) fn next_managed_agent_deadline(&self) -> Option<Instant> {
        self.terminals
            .values()
            .filter_map(crate::terminal::TerminalState::next_managed_agent_deadline)
            .min()
    }

    pub(crate) fn publish_pane_process_exit_if_agent(
        &mut self,
        pane_id: PaneId,
        suppress_completion: bool,
    ) -> Option<PaneStateUpdate> {
        let observed_at = std::time::Instant::now();
        let update = self.update_terminal_state_with_completion_policy(
            pane_id,
            suppress_completion,
            |terminal| {
                let agent = terminal.effective_known_agent().or(terminal.detected_agent);
                if agent.is_none() && !terminal.full_lifecycle_hook_authority_active() {
                    return None;
                }
                Some(terminal.set_detected_state_with_screen_signals_at(
                    agent,
                    AgentState::Idle,
                    false,
                    true,
                    false,
                    true,
                    observed_at,
                ))
            },
        )?;
        update.agent_released.then_some(update)
    }

    fn apply_pane_state_change(
        &mut self,
        ws_idx: usize,
        pane_id: PaneId,
        change: &EffectiveStateChange,
        suppress_completion: bool,
    ) -> Option<bool> {
        let in_active_tab = self.active == Some(ws_idx)
            && self.workspaces[ws_idx]
                .find_tab_index_for_pane(pane_id)
                .is_some_and(|tab_idx| tab_idx == self.workspaces[ws_idx].active_tab);
        let pane = self.workspaces[ws_idx]
            .tabs
            .iter_mut()
            .find_map(|tab| tab.panes.get_mut(&pane_id))?;
        if change.state != AgentState::Idle {
            pane.seen = true;
        } else if !suppress_completion && is_completion_transition(change) {
            pane.seen = in_active_tab;
        }
        Some(pane.seen)
    }

    fn handle_pane_died(&mut self, pane_id: PaneId) {
        let ws_idx = self
            .workspaces
            .iter()
            .position(|ws| ws.find_tab_index_for_pane(pane_id).is_some());

        let Some(ws_idx) = ws_idx else {
            warn!(pane = pane_id.raw(), "PaneDied for unknown pane");
            return;
        };

        let pane_terminal_id = self.terminal_id_for_pane(ws_idx, pane_id);
        let workspace_terminal_ids = self.terminal_ids_for_workspace(ws_idx);
        self.pane_id_aliases.retain(|_, alias| *alias != pane_id);
        self.public_pane_id_aliases
            .retain(|_, alias| *alias != pane_id);
        let should_close_workspace = {
            let ws = &mut self.workspaces[ws_idx];
            ws.remove_pane(pane_id)
        };
        self.mark_session_dirty();

        if should_close_workspace {
            let active_workspace_id = self
                .active
                .and_then(|idx| self.workspaces.get(idx))
                .map(|ws| ws.id.clone());
            let selected_workspace_id = self.workspaces.get(self.selected).map(|ws| ws.id.clone());
            self.workspaces.remove(ws_idx);
            self.remove_unattached_terminal_ids(workspace_terminal_ids);
            if self.workspaces.is_empty() {
                self.active = None;
                self.selected = 0;
            } else {
                // Keep focus on the previously focused workspace
                if let Some(id) = active_workspace_id {
                    if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == id) {
                        self.active = Some(idx);
                    }
                }
                if let Some(active) = self.active {
                    if active >= self.workspaces.len() {
                        self.active = Some(self.workspaces.len() - 1);
                    }
                }
                if let Some(id) = selected_workspace_id {
                    if let Some(idx) = self.workspaces.iter().position(|ws| ws.id == id) {
                        self.selected = idx;
                    }
                }
                if self.selected >= self.workspaces.len() {
                    self.selected = self.workspaces.len() - 1;
                }
            }
        } else {
            self.remove_unattached_terminal_ids(pane_terminal_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn selected_word(row: &str, col: u16) -> Option<String> {
        let (start, end) = word_bounds_at_column(row, col)?;
        Some(text_in_cell_range(row, start, end))
    }

    fn text_in_cell_range(row: &str, start_col: u16, end_col: u16) -> String {
        text_cells(row)
            .into_iter()
            .filter(|cell| cell.start_col >= start_col && cell.end_col <= end_col)
            .map(|cell| cell.ch)
            .collect()
    }

    fn col_of(row: &str, needle: &str) -> u16 {
        let byte_idx = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not found in {row:?}"));
        let prefix = &row[..byte_idx];
        prefix
            .chars()
            .map(|ch| u16::from(crate::ghostty::unicode_codepoint_width(ch as u32)))
            .sum()
    }

    fn assert_selects(row: &str, click: &str, expected: &str) {
        assert_eq!(
            selected_word(row, col_of(row, click)).as_deref(),
            Some(expected),
            "row={row:?}, click={click:?}"
        );
    }

    fn assert_selects_nothing(row: &str, click: &str) {
        assert_eq!(
            selected_word(row, col_of(row, click)),
            None,
            "row={row:?}, click={click:?}"
        );
    }

    #[test]
    fn double_click_word_bounds_cover_terminal_text() {
        let cases = [
            (
                "see https://example.com/a-b_c?q=x@y.",
                "example.com",
                "https://example.com/a-b_c?q=x@y",
            ),
            (
                "open \"https://example.com/a,b;c?q=x\";",
                "example.com",
                "https://example.com/a,b;c?q=x",
            ),
            (
                "see https://en.wikipedia.org/wiki/Foo_(bar_(baz)),",
                "wikipedia",
                "https://en.wikipedia.org/wiki/Foo_(bar_(baz))",
            ),
            (
                "see https://example.com/a(b[c{d}e]f),",
                "example.com",
                "https://example.com/a(b[c{d}e]f)",
            ),
            (
                "see (https://example.com/a(b(c)d)))",
                "example.com",
                "https://example.com/a(b(c)d)",
            ),
            (
                "open /tmp/foo-bar/baz_qux/",
                "foo-bar",
                "/tmp/foo-bar/baz_qux/",
            ),
            (
                "open ./src/app/actions.rs:795",
                "actions",
                "./src/app/actions.rs:795",
            ),
            (
                "open ../sesh-mux-worktrees/issue-1",
                "sesh-mux",
                "../sesh-mux-worktrees/issue-1",
            ),
            (
                "edit src/app/actions.rs,then",
                "actions",
                "src/app/actions.rs",
            ),
            (
                "cat \"/tmp/build output/log.txt\"",
                "output",
                "/tmp/build output/log.txt",
            ),
            (
                "cat '/Users/me/Library/Application Support/app/config.json'",
                "Support",
                "/Users/me/Library/Application Support/app/config.json",
            ),
            ("echo 你好-world done", "好", "你好-world"),
            (
                "註解已補（slice 4 5b5fcc0715）：整合原本",
                "5b5fcc0715",
                "5b5fcc0715",
            ),
            ("先跑 cargo test", "cargo", "cargo"),
            (
                "export PATH=$HOME/.cargo/bin:$PATH",
                "$HOME",
                "PATH=$HOME/.cargo/bin:$PATH",
            ),
            (
                "git checkout feature/foo-bar_baz",
                "foo",
                "feature/foo-bar_baz",
            ),
            ("refs #123 and @owner/name", "#123", "#123"),
            ("refs #123 and @owner/name", "owner", "@owner/name"),
            (
                "cargo test --package=sesh-mux",
                "--package",
                "--package=sesh-mux",
            ),
            (
                "cargo test app::actions::tests",
                "app::",
                "app::actions::tests",
            ),
            (
                "image ghcr.io/org/app:latest",
                "ghcr",
                "ghcr.io/org/app:latest",
            ),
            ("ERROR [worker-1] request_id=abc-123", "worker", "worker-1"),
            (
                "tmux|newhoo|fixhoo|newmoo|notification|window_bell|sesh-mux",
                "newhoo",
                "newhoo",
            ),
            (
                "render_status_line(app, area)",
                "render",
                "render_status_line",
            ),
            ("render_status_line(app, area)", "app", "app"),
            ("render_status_line(app, area)", "area", "area"),
            ("if !enabled {", "enabled", "enabled"),
            ("println!(\"hi\")", "println", "println"),
            ("( master)$", "master", "master"),
            ("regex foo$", "foo", "foo$"),
        ];

        for (row, click, expected) in cases {
            assert_selects(row, click, expected);
        }

        let row = "echo 你好-world done";
        assert_eq!(
            selected_word(row, col_of(row, "好") + 1).as_deref(),
            Some("你好-world")
        );
    }

    #[test]
    fn double_click_word_bounds_treat_cjk_punctuation_as_delimiters() {
        for delimiter in ['（', '）', '：', '、', '。', '，'] {
            let row = format!("left{delimiter}right");
            assert_selects(&row, "left", "left");
            assert_selects(&row, "right", "right");
            assert_selects_nothing(&row, &delimiter.to_string());
            assert_eq!(
                selected_word(&row, col_of(&row, &delimiter.to_string()) + 1),
                None,
                "second display cell of {delimiter:?} should not select"
            );
        }
    }

    #[test]
    fn double_click_word_bounds_ignore_delimiters() {
        for (row, click) in [
            (
                "tmux|newhoo|fixhoo|newmoo|notification|window_bell|sesh-mux",
                "|",
            ),
            ("alpha,beta;gamma", ","),
            ("alpha,beta;gamma", ";"),
            ("render_status_line(app, area)", "("),
            ("render_status_line(app, area)", ")"),
            ("if !enabled {", "!"),
            ("if !enabled {", "{"),
            ("(done).", "("),
            ("(done).", "."),
        ] {
            assert_selects_nothing(row, click);
        }
    }
}
