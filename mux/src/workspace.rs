use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};

use crate::events::AppEvent;
use crate::layout::PaneId;
#[cfg(test)]
use crate::layout::TileLayout;
use crate::pane::{PaneLaunchEnv, PaneState};
use crate::render_signal::RenderSignal;
use crate::terminal::{TerminalId, TerminalRuntime, TerminalRuntimeRegistry, TerminalState};

mod aggregate;
mod git;
mod tab;

use self::git::git_status_cache_key_for_space;
pub use self::{
    git::{
        fallback_label_from_cwd, git_branch, git_space_metadata, GitSpaceMetadata,
        },
    tab::Tab,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorktreeSpaceMembership {
    pub key: String,
    pub label: String,
    pub repo_root: PathBuf,
    pub checkout_path: PathBuf,
    pub is_linked_worktree: bool,
}



pub(crate) fn discover_workspace_git_identity(
    cwd: &std::path::Path,
) -> (Option<GitSpaceMetadata>, String, PathBuf) {
    let space = git_space_metadata(cwd);
    let auto_label = space
        .as_ref()
        .map(|space| self::git::automatic_workspace_label(cwd, &space.repo_root))
        .unwrap_or_else(|| fallback_label_from_cwd(cwd));
    let status_cache_key = space
        .as_ref()
        .map(git_status_cache_key_for_space)
        .unwrap_or_else(|| cwd.to_path_buf());
    (space, auto_label, status_cache_key)
}

static NEXT_WORKSPACE_ID: AtomicU64 = AtomicU64::new(1);
const PUBLIC_ID_ALPHABET: &[u8; 32] = b"123456789ABCDEFGHJKMNPQRSTVWXYZ0";

pub(crate) fn generate_workspace_id() -> String {
    let counter = NEXT_WORKSPACE_ID.fetch_add(1, Ordering::Relaxed);
    format!("w{}", encode_public_number(counter as usize))
}

pub(crate) fn encode_public_number(mut value: usize) -> String {
    if value == 0 {
        return "0".to_string();
    }

    let mut encoded = Vec::new();
    while value > 0 {
        let digit = (value - 1) % PUBLIC_ID_ALPHABET.len();
        encoded.push(PUBLIC_ID_ALPHABET[digit] as char);
        value = (value - 1) / PUBLIC_ID_ALPHABET.len();
    }
    encoded.iter().rev().collect()
}

pub(crate) fn decode_public_number(value: &str) -> Option<usize> {
    let mut decoded = 0usize;
    for ch in value.chars() {
        let digit = PUBLIC_ID_ALPHABET
            .iter()
            .position(|candidate| *candidate as char == ch)?;
        decoded = decoded
            .checked_mul(PUBLIC_ID_ALPHABET.len())?
            .checked_add(digit + 1)?;
    }
    Some(decoded)
}

pub(crate) fn public_workspace_number(id: &str) -> Option<usize> {
    id.strip_prefix('w').and_then(decode_public_number)
}

pub(crate) fn public_pane_id_for_number(workspace_id: &str, pane_number: usize) -> String {
    format!("{workspace_id}:p{}", encode_public_number(pane_number))
}

pub(crate) fn public_tab_id_for_number(workspace_id: &str, tab_number: usize) -> String {
    format!("{workspace_id}:t{}", encode_public_number(tab_number))
}

pub(crate) fn reserve_workspace_ids(workspaces: &[Workspace]) {
    let Some(next) = workspaces
        .iter()
        .filter_map(|workspace| public_workspace_number(&workspace.id))
        .max()
        .and_then(|max| u64::try_from(max.checked_add(1)?).ok())
    else {
        return;
    };

    let mut current = NEXT_WORKSPACE_ID.load(Ordering::Relaxed);
    while current < next {
        match NEXT_WORKSPACE_ID.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

/// A named workspace containing tabs.
pub struct Workspace {
    /// Stable public workspace identity, independent of display order.
    pub id: String,
    /// User-provided override. If set, auto-derived identity stops updating.
    pub custom_name: Option<String>,
    /// Fallback workspace identity source for tests, old snapshots, or missing runtimes.
    pub identity_cwd: PathBuf,
    /// CWD from which the cached automatic label and Git metadata were derived.
    pub(crate) cached_identity_cwd: PathBuf,
    /// Automatic workspace label cached outside the render path.
    pub(crate) cached_auto_label: String,
    /// Cache key for periodic Git status associated with `cached_identity_cwd`.
    pub(crate) cached_git_status_key: PathBuf,
    /// Cached current git branch for the workspace repo.
    pub(crate) cached_git_branch: Option<String>,
    /// Cached ahead/behind counts for the workspace repo's current branch upstream.
    pub(crate) cached_git_ahead_behind: Option<(usize, usize)>,
    /// Cached derived Git repo metadata for worktree actions and status display.
    pub(crate) cached_git_space: Option<GitSpaceMetadata>,
    /// Explicit sesh-mux-managed worktree grouping provenance.
    pub worktree_space: Option<WorktreeSpaceMembership>,
    pub(crate) metadata_tokens: crate::metadata_tokens::MetadataTokens,
    pub(crate) metadata_token_sequences: HashMap<String, u64>,
    /// Public pane numbers within this workspace. Closed pane numbers are not reused.
    pub public_pane_numbers: HashMap<PaneId, usize>,
    pub(crate) next_public_pane_number: usize,
    pub(crate) next_public_tab_number: usize,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    #[cfg(test)]
    pub(crate) test_runtimes: HashMap<PaneId, TerminalRuntime>,
}

impl Deref for Workspace {
    type Target = Tab;

    fn deref(&self) -> &Self::Target {
        self.active_tab()
            .expect("workspace must always have at least one active tab")
    }
}

impl DerefMut for Workspace {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.active_tab_mut()
            .expect("workspace must always have at least one active tab")
    }
}

impl Workspace {
    pub fn new(
        initial_cwd: PathBuf,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: crate::pane::PaneShellConfig<'_>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<(Self, TerminalState, TerminalRuntime)> {
        Self::new_with_tab(
            initial_cwd,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            events,
            render_notify,
            render_dirty,
            None,
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_extra_env(
        initial_cwd: PathBuf,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: crate::pane::PaneShellConfig<'_>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
        extra_env: Vec<(String, String)>,
    ) -> std::io::Result<(Self, TerminalState, TerminalRuntime)> {
        if extra_env.is_empty() {
            return Self::new(
                initial_cwd,
                rows,
                cols,
                scrollback_limit_bytes,
                host_terminal_theme,
                host_terminal_appearance,
                shell_config,
                events,
                render_notify,
                render_dirty,
            );
        }
        Self::new_with_tab(
            initial_cwd,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            events,
            render_notify,
            render_dirty,
            None,
            extra_env,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_tab(
        initial_cwd: PathBuf,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: crate::pane::PaneShellConfig<'_>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
        argv: Option<&[String]>,
        extra_env: Vec<(String, String)>,
    ) -> std::io::Result<(Self, TerminalState, TerminalRuntime)> {
        let id = generate_workspace_id();
        let launch_env = PaneLaunchEnv::from_extra(extra_env).with_identity(
            id.clone(),
            public_tab_id_for_number(&id, 1),
            public_pane_id_for_number(&id, 1),
        );
        let (tab, terminal, runtime) = if let Some(argv) = argv {
            Tab::new_argv_command(
                1,
                initial_cwd.clone(),
                rows,
                cols,
                argv,
                scrollback_limit_bytes,
                host_terminal_theme,
                host_terminal_appearance,
                &launch_env,
                events,
                render_notify,
                render_dirty,
            )?
        } else {
            Tab::new(
                1,
                initial_cwd.clone(),
                rows,
                cols,
                scrollback_limit_bytes,
                host_terminal_theme,
                host_terminal_appearance,
                shell_config,
                &launch_env,
                events,
                render_notify,
                render_dirty,
            )?
        };
        let mut public_pane_numbers = HashMap::new();
        public_pane_numbers.insert(tab.root_pane, 1);
        let (cached_git_space, cached_auto_label, cached_git_status_key) =
            discover_workspace_git_identity(&initial_cwd);
        Ok((
            Self {
                id,
                custom_name: None,
                identity_cwd: initial_cwd.clone(),
                cached_identity_cwd: initial_cwd.clone(),
                cached_auto_label,
                cached_git_status_key,
                cached_git_branch: git_branch(&initial_cwd),
                cached_git_ahead_behind: None,
                cached_git_space,
                worktree_space: None,
                metadata_tokens: crate::metadata_tokens::MetadataTokens::default(),
                metadata_token_sequences: HashMap::new(),
                public_pane_numbers,
                next_public_pane_number: 2,
                next_public_tab_number: 2,
                tabs: vec![tab],
                active_tab: 0,
                #[cfg(test)]
                test_runtimes: HashMap::new(),
            },
            terminal,
            runtime,
        ))
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    pub fn active_tab_index(&self) -> usize {
        self.active_tab
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab)
    }

    pub fn tab_display_name(&self, tab_idx: usize) -> Option<String> {
        let tab = self.tabs.get(tab_idx)?;
        Some(
            tab.custom_name
                .clone()
                .unwrap_or_else(|| (tab_idx + 1).to_string()),
        )
    }

    pub fn switch_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                for pane in tab.panes.values_mut() {
                    pane.seen = true;
                }
            }
        }
    }

    pub fn create_tab(
        &mut self,
        rows: u16,
        cols: u16,
        cwd: PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: crate::pane::PaneShellConfig<'_>,
        extra_env: Vec<(String, String)>,
    ) -> std::io::Result<(usize, TerminalState, TerminalRuntime)> {
        self.create_tab_with_runtime(
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            None,
            extra_env,
        )
    }

    fn create_tab_with_runtime(
        &mut self,
        rows: u16,
        cols: u16,
        cwd: PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: crate::pane::PaneShellConfig<'_>,
        argv: Option<&[String]>,
        extra_env: Vec<(String, String)>,
    ) -> std::io::Result<(usize, TerminalState, TerminalRuntime)> {
        let number = self.next_public_tab_number;
        self.next_public_tab_number += 1;
        let pane_number = self.next_public_pane_number;
        let launch_env = self.launch_env_for_new_pane(number, pane_number, extra_env);
        let events = self
            .active_tab()
            .map(|tab| tab.events.clone())
            .expect("workspace must always have at least one tab");
        let render_notify = self
            .active_tab()
            .map(|tab| tab.render_notify.clone())
            .expect("workspace must always have at least one tab");
        let render_dirty = self
            .active_tab()
            .map(|tab| tab.render_dirty.clone())
            .expect("workspace must always have at least one tab");

        let (tab, terminal, runtime) = if let Some(argv) = argv {
            Tab::new_argv_command(
                number,
                cwd,
                rows,
                cols,
                argv,
                scrollback_limit_bytes,
                host_terminal_theme,
                host_terminal_appearance,
                &launch_env,
                events,
                render_notify,
                render_dirty,
            )?
        } else {
            Tab::new(
                number,
                cwd,
                rows,
                cols,
                scrollback_limit_bytes,
                host_terminal_theme,
                host_terminal_appearance,
                shell_config,
                &launch_env,
                events,
                render_notify,
                render_dirty,
            )?
        };
        self.register_new_pane_with_number(tab.root_pane, pane_number);
        self.tabs.push(tab);
        Ok((self.tabs.len() - 1, terminal, runtime))
    }

    pub fn close_tab(&mut self, idx: usize) -> bool {
        if self.tabs.len() <= 1 || idx >= self.tabs.len() {
            return false;
        }
        let tab = self.tabs.remove(idx);
        for pane_id in tab.panes.keys() {
            self.unregister_pane(*pane_id);
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if idx <= self.active_tab && self.active_tab > 0 {
            self.active_tab -= 1;
        }
        true
    }

    // Workspace split routing carries pane identity, geometry, host context, and focus policy.

    /// Remove a specific pane from this workspace without terminating its runtime.
    /// Returns true if the workspace should close.
    pub fn remove_pane(&mut self, pane_id: PaneId) -> bool {
        let Some(tab_idx) = self.find_tab_index_for_pane(pane_id) else {
            return false;
        };
        let pane_count = self.tabs[tab_idx].layout.pane_count();
        let tab_count = self.tabs.len();
        if pane_count <= 1 {
            if tab_count <= 1 {
                return true;
            }
            self.tabs.remove(tab_idx);
            self.unregister_pane(pane_id);
            if self.active_tab >= self.tabs.len() {
                self.active_tab = self.tabs.len() - 1;
            } else if tab_idx <= self.active_tab && self.active_tab > 0 {
                self.active_tab -= 1;
            }
            return false;
        }

        if let Some((removed, _terminal_id)) = self.tabs[tab_idx].remove_pane(pane_id) {
            self.unregister_pane(removed);
        }
        false
    }

    pub fn public_pane_number(&self, pane_id: PaneId) -> Option<usize> {
        self.public_pane_numbers.get(&pane_id).copied()
    }

    fn launch_env_for_new_pane(
        &self,
        tab_number: usize,
        pane_number: usize,
        extra_env: Vec<(String, String)>,
    ) -> PaneLaunchEnv {
        PaneLaunchEnv::from_extra(extra_env).with_identity(
            self.id.clone(),
            public_tab_id_for_number(&self.id, tab_number),
            public_pane_id_for_number(&self.id, pane_number),
        )
    }

    pub fn public_tab_number(&self, tab_idx: usize) -> Option<usize> {
        self.tabs.get(tab_idx).map(|tab| tab.number)
    }

    pub fn set_custom_name(&mut self, name: String) {
        self.custom_name = Some(name);
    }

    pub fn resolved_identity_cwd_from(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> Option<PathBuf> {
        self.tabs
            .first()
            .and_then(|tab| tab.cwd_for_pane(tab.root_pane, terminals, terminal_runtimes))
            .or_else(|| Some(self.identity_cwd.clone()))
    }

    pub fn display_name_from(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> String {
        if let Some(name) = &self.custom_name {
            return name.clone();
        }

        self.resolved_identity_cwd_from(terminals, terminal_runtimes)
            .map(|cwd| self.automatic_display_name_for_cwd(&cwd))
            .unwrap_or_else(|| "workspace".into())
    }

    fn automatic_display_name_for_cwd(&self, cwd: &std::path::Path) -> String {
        if cwd == self.cached_identity_cwd {
            self.cached_auto_label.clone()
        } else {
            fallback_label_from_cwd(cwd)
        }
    }

    pub fn git_space(&self) -> Option<&GitSpaceMetadata> {
        self.cached_git_space.as_ref()
    }

    pub fn worktree_space(&self) -> Option<&WorktreeSpaceMembership> {
        self.worktree_space.as_ref()
    }

    pub fn find_tab_index_for_pane(&self, pane_id: PaneId) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.panes.contains_key(&pane_id))
    }

    pub fn pane_state(&self, pane_id: PaneId) -> Option<&PaneState> {
        self.tabs.iter().find_map(|tab| tab.panes.get(&pane_id))
    }

    pub fn pane_state_mut(&mut self, pane_id: PaneId) -> Option<&mut PaneState> {
        self.tabs
            .iter_mut()
            .find_map(|tab| tab.panes.get_mut(&pane_id))
    }

    pub fn terminal_id(&self, pane_id: PaneId) -> Option<&TerminalId> {
        self.tabs.iter().find_map(|tab| tab.terminal_id(pane_id))
    }

    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.active_tab().map(|tab| tab.layout.focused())
    }

    pub fn close_pane(&mut self, pane_id: PaneId) -> bool {
        let tab_idx = match self.find_tab_index_for_pane(pane_id) {
            Some(idx) => idx,
            None => return false,
        };
        let pane_count = self.tabs[tab_idx].layout.pane_count();
        let tab_count = self.tabs.len();
        if pane_count <= 1 {
            if tab_count <= 1 {
                return true;
            }
            self.tabs.remove(tab_idx);
            self.unregister_pane(pane_id);
            if self.active_tab >= self.tabs.len() {
                self.active_tab = self.tabs.len() - 1;
            } else if tab_idx <= self.active_tab && self.active_tab > 0 {
                self.active_tab -= 1;
            }
            return false;
        }

        if let Some((removed, _terminal_id)) = self.tabs[tab_idx].close_pane(pane_id) {
            self.unregister_pane(removed);
        }
        false
    }

    #[cfg(test)]
    fn register_new_pane(&mut self, pane_id: PaneId) {
        self.register_new_pane_with_number(pane_id, self.next_public_pane_number);
    }

    fn register_new_pane_with_number(&mut self, pane_id: PaneId, number: usize) {
        self.public_pane_numbers.insert(pane_id, number);
        self.next_public_pane_number = self.next_public_pane_number.max(number + 1);
    }

    fn unregister_pane(&mut self, pane_id: PaneId) {
        self.public_pane_numbers.remove(&pane_id);
    }
}

#[cfg(test)]
impl Workspace {
    pub(crate) fn test_new(name: &str) -> Self {
        let (events, _) = mpsc::channel(64);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(RenderSignal::new());
        let identity_cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
        let (layout, root_id) = TileLayout::new();
        let terminal_id = TerminalId::alloc();
        let mut panes = HashMap::new();
        panes.insert(root_id, PaneState::new(terminal_id));
        let tab = Tab {
            custom_name: None,
            number: 1,
            root_pane: root_id,
            layout,
            panes,
            runtimes: HashMap::new(),
            zoomed: false,
            events,
            render_notify,
            render_dirty,
        };
        let mut public_pane_numbers = HashMap::new();
        public_pane_numbers.insert(tab.root_pane, 1);
        Self {
            id: generate_workspace_id(),
            custom_name: Some(name.to_string()),
            identity_cwd: identity_cwd.clone(),
            cached_identity_cwd: identity_cwd.clone(),
            cached_auto_label: fallback_label_from_cwd(&identity_cwd),
            cached_git_status_key: identity_cwd.clone(),
            cached_git_branch: git_branch(&identity_cwd),
            cached_git_ahead_behind: None,
            cached_git_space: None,
            worktree_space: None,
            metadata_tokens: crate::metadata_tokens::MetadataTokens::default(),
            metadata_token_sequences: HashMap::new(),
            public_pane_numbers,
            next_public_pane_number: 2,
            next_public_tab_number: 2,
            tabs: vec![tab],
            active_tab: 0,
            test_runtimes: HashMap::new(),
        }
    }

    pub(crate) fn test_add_tab(&mut self, name: Option<&str>) -> usize {
        let (events, _) = mpsc::channel(64);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(RenderSignal::new());
        let (layout, root_id) = TileLayout::new();
        let mut panes = HashMap::new();
        panes.insert(root_id, PaneState::new(TerminalId::alloc()));
        let tab = Tab {
            custom_name: name.map(str::to_string),
            number: self.next_public_tab_number,
            root_pane: root_id,
            layout,
            panes,
            runtimes: HashMap::new(),
            zoomed: false,
            events,
            render_notify,
            render_dirty,
        };
        self.next_public_tab_number += 1;
        self.register_new_pane(root_id);
        self.tabs.push(tab);
        self.tabs.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserving_restored_workspace_ids_prevents_reuse() {
        let mut restored = Workspace::test_new("restored");
        restored.id = "wZ".to_string();

        reserve_workspace_ids(&[restored]);

        let generated = generate_workspace_id();
        assert_ne!(generated, "wZ");
        assert!(public_workspace_number(&generated) > public_workspace_number("wZ"));
    }
}
