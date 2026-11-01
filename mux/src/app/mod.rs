//! Application state and the operations the API performs on it.
//!
//! - `state.rs` — AppState and pure data structs
//! - `actions.rs` — state mutations (testable without PTYs/async)

pub(crate) mod actions;
mod agent_resume;
mod agents;
pub(crate) use agents::{AGENT_START_SETTLE_DELAY, MAX_AGENT_START_TIMEOUT};
mod api;
mod api_helpers;
pub(crate) use api_helpers::limit_snapshot_lines;
mod creation;
mod ids;
mod legacy;
mod runtime;
mod session;
pub mod state;
mod terminal_targets;
mod terminal_titles;
mod worktrees;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(16);
const PENDING_AGENT_RESUME_THEME_WAIT: Duration = Duration::from_millis(750);
const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_secs(5);

use tokio::sync::{mpsc, Notify};
use tracing::info;

use crate::config::Config;
use crate::events::AppEvent;

pub use state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppPolicy {
    pub(crate) restore_session: bool,
    pub(crate) persist_session: bool,
}

impl AppPolicy {
    pub(crate) const PRODUCTION: Self = Self {
        restore_session: true,
        persist_session: true,
    };

    pub(crate) const HANDOFF_REPLACEMENT: Self = Self {
        restore_session: false,
        persist_session: true,
    };
}

pub struct App {
    pub state: AppState,
    pub(crate) terminal_runtimes: crate::terminal::TerminalRuntimeRegistry,
    pub event_tx: mpsc::Sender<AppEvent>,
    pub(crate) event_rx: mpsc::Receiver<AppEvent>,
    pub(crate) api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
    pub(crate) event_hub: crate::api::EventHub,
    pub(crate) last_focus: Option<(usize, crate::layout::PaneId)>,
    pub(crate) policy: AppPolicy,
    pub(crate) pending_api_worktree_creates: HashMap<std::path::PathBuf, u64>,
    pub(crate) pending_api_worktree_removes: HashMap<String, u64>,
    pub(crate) pending_api_worktree_remove_paths: HashMap<std::path::PathBuf, u64>,
    pub(crate) pending_worktree_remove_runtime_exits: HashMap<crate::layout::PaneId, usize>,
    pub(crate) pending_worktree_remove_runtime_restores: HashMap<crate::layout::PaneId, u64>,
    pub(crate) next_api_worktree_operation_id: u64,
    pub(crate) agent_metadata_deadline: Option<Instant>,
    pub(crate) pending_agent_resume_deadline: Option<Instant>,
    pub(crate) session_save_deadline: Option<Instant>,
    pub(crate) session_save_thread: Option<std::thread::JoinHandle<()>>,
    session_writer: Arc<std::sync::Mutex<crate::persist::SessionWriter>>,
    pane_exit_checkpoint_pending: bool,
    pub(crate) detached_process_children: Vec<std::process::Child>,
    pub(crate) persist_pane_history: bool,
    pub(crate) last_render_at: Option<Instant>,
    pub render_notify: Arc<Notify>,
    pub(crate) render_dirty: Arc<crate::render_signal::RenderSignal>,
    pub(crate) full_redraw_pending: bool,
}

pub(crate) const APP_EVENT_CHANNEL_CAPACITY: usize = 256;
pub(crate) const APP_EVENT_DRAIN_LIMIT: usize = 64;

impl App {
    pub fn new(
        config: &Config,
        policy: AppPolicy,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel::<AppEvent>(APP_EVENT_CHANNEL_CAPACITY);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(crate::render_signal::RenderSignal::new());
        let headless_size = config.headless_size();

        let mut restored_terminals = HashMap::new();
        let mut restored_terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let snapshot = policy.restore_session.then(crate::persist::load).flatten();
        let session_writer = Arc::new(std::sync::Mutex::new(crate::persist::SessionWriter::new(
            policy.restore_session && snapshot.is_none(),
        )));
        let (workspaces, active, selected) = if let Some(snap) = snapshot {
            let history = config
                .experimental
                .pane_history
                .then(crate::persist::load_history)
                .flatten();
            let (ws, terminals, terminal_runtimes) = crate::persist::restore(
                &snap,
                history.as_ref(),
                headless_size.1,
                headless_size.0,
                config.advanced.scrollback_limit_bytes,
                &config.terminal.default_shell,
                config.terminal.shell_mode,
                config.session.resume_agents_on_restore,
                event_tx.clone(),
                render_notify.clone(),
                render_dirty.clone(),
            );
            restored_terminals = terminals;
            restored_terminal_runtimes = terminal_runtimes.into();
            crate::logging::session_restored(ws.len(), if ws.is_empty() { "empty" } else { "ok" });
            let active = snap.active.filter(|&i| i < ws.len());
            let selected = snap.selected.min(ws.len().saturating_sub(1));
            (ws, active, selected)
        } else {
            (Vec::new(), None, 0)
        };

        info!(
            pane_scrollback_limit_bytes = config.advanced.scrollback_limit_bytes,
            "using pane scrollback configuration"
        );

        let mut state = AppState::new(config, workspaces, active, selected);
        state.terminals = restored_terminals;
        for ws_idx in 0..state.workspaces.len() {
            let cwd = state.workspaces[ws_idx]
                .resolved_identity_cwd_from(&state.terminals, &restored_terminal_runtimes);
            state.workspaces[ws_idx].cached_git_branch =
                cwd.as_deref().and_then(crate::workspace::git_branch);
        }
        let last_focus = state.active.and_then(|idx| {
            state
                .workspaces
                .get(idx)
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });

        Self {
            state,
            terminal_runtimes: restored_terminal_runtimes,
            event_tx,
            event_rx,
            pending_api_worktree_creates: HashMap::new(),
            pending_api_worktree_removes: HashMap::new(),
            pending_api_worktree_remove_paths: HashMap::new(),
            pending_worktree_remove_runtime_exits: HashMap::new(),
            pending_worktree_remove_runtime_restores: HashMap::new(),
            next_api_worktree_operation_id: 1,
            agent_metadata_deadline: None,
            pending_agent_resume_deadline: None,
            session_save_deadline: None,
            session_save_thread: None,
            session_writer,
            pane_exit_checkpoint_pending: false,
            detached_process_children: Vec::new(),
            persist_pane_history: config.experimental.pane_history,
            last_render_at: None,
            api_rx,
            event_hub,
            last_focus,
            policy,
            render_notify,
            render_dirty,
            full_redraw_pending: false,
        }
    }

    pub fn new_from_handoff(
        config: &Config,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
        snapshot: &crate::persist::SessionSnapshot,
        imports: &mut HashMap<u32, crate::handoff_runtime::ImportedHandoffRuntime>,
    ) -> io::Result<Self> {
        let mut app = Self::new(config, AppPolicy::HANDOFF_REPLACEMENT, api_rx, event_hub);
        let (workspaces, terminals, runtimes) = crate::persist::restore_handoff(
            snapshot,
            config.advanced.scrollback_limit_bytes,
            &config.terminal.default_shell,
            config.terminal.shell_mode,
            imports,
            app.event_tx.clone(),
            app.render_notify.clone(),
            app.render_dirty.clone(),
        )?;
        app.state.pane_id_aliases = crate::persist::handoff_pane_aliases(snapshot, &workspaces);
        app.state.workspaces = workspaces;
        app.state.terminals = terminals;
        app.terminal_runtimes = runtimes.into();
        app.state.active = snapshot
            .active
            .filter(|&idx| idx < app.state.workspaces.len());
        app.state.selected = snapshot
            .selected
            .min(app.state.workspaces.len().saturating_sub(1));
        app.last_focus = app.state.active.and_then(|idx| {
            app.state
                .workspaces
                .get(idx)
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
        Ok(app)
    }

    pub fn unpause_handoff_readers(&self) {
        self.terminal_runtimes.set_handoff_readers_paused(false);
    }

    pub fn assume_handoff_ownership(&mut self) {
        self.terminal_runtimes.assume_handoff_ownership();
    }
}

#[cfg(test)]
mod tests;
