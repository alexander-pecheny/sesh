use std::path::{Path, PathBuf};

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, WorktreeInfo, WorktreeListParams,
    WorktreeOpenParams, WorktreeSourceInfo,
};
use crate::app::App;

use super::responses::{encode_error, encode_success};

mod deferred;

struct ApiFailure {
    code: &'static str,
    message: String,
}

impl ApiFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn absolute_user_path(path: &str) -> Result<PathBuf, ApiFailure> {
    let path = crate::worktree::expand_tilde_path(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(ApiFailure::new(
            "invalid_request",
            "worktree path must be absolute",
        ))
    }
}

struct WorktreeSource {
    workspace_idx: Option<usize>,
    source_checkout_path: PathBuf,
    source_repo_root: PathBuf,
    repo_key: String,
    repo_name: String,
}

impl App {
    pub(super) fn handle_worktree_list(
        &mut self,
        id: String,
        params: WorktreeListParams,
    ) -> String {
        let source = match self.resolve_worktree_list_source(
            params.workspace_id,
            params.cwd,
            params.trust_repository,
        ) {
            Ok(source) => source,
            Err(err) => return encode_error(id, err.code, err.message),
        };
        let entries = match crate::worktree::list_existing_worktrees(
            &source.source_repo_root,
            params.trust_repository,
        ) {
            Ok(entries) => entries,
            Err(err) => return encode_error(id, "worktree_list_failed", err),
        };
        let worktrees = entries
            .into_iter()
            .map(|entry| self.worktree_info_for_entry(&source, entry))
            .collect();

        encode_success(
            id,
            ResponseResult::WorktreeList {
                source: self.worktree_source_info(&source),
                worktrees,
            },
        )
    }

    pub(super) fn handle_worktree_open(
        &mut self,
        id: String,
        params: WorktreeOpenParams,
    ) -> String {
        if params.path.is_some() == params.branch.is_some() {
            return encode_error(
                id,
                "invalid_request",
                "exactly one of path or branch is required",
            );
        }
        let mut source = match self.resolve_worktree_source(params.workspace_id, params.cwd) {
            Ok(source) => source,
            Err(err) => return encode_error(id, err.code, err.message),
        };
        let entry = match self.find_worktree_entry(
            &source,
            params.path,
            params.branch,
            params.trust_repository,
        ) {
            Ok(entry) => entry,
            Err(err) => return encode_error(id, err.code, err.message),
        };
        if entry.is_bare || entry.is_prunable {
            return encode_error(id, "worktree_not_found", "worktree cannot be opened");
        }
        let canonical_path = crate::worktree::canonical_or_original(&entry.path);
        let canonical_source = crate::worktree::canonical_or_original(&source.source_checkout_path);
        let target_is_source = canonical_path == canonical_source;
        let already_open = self.open_workspace_idx_for_checkout(&canonical_path);
        let defer_source_created_event = target_is_source && already_open.is_none();
        let created_source_workspace =
            match self.ensure_source_parent_membership(&mut source, !defer_source_created_event) {
                Ok(created) => created,
                Err(err) => return encode_error(id, err.code, err.message),
            };
        let (ws_idx, created_workspace) = if let Some(ws_idx) = already_open {
            if params.focus {
                self.state.switch_workspace(ws_idx);
            }
            (ws_idx, false)
        } else if target_is_source {
            let ws_idx = source
                .workspace_idx
                .expect("source workspace should exist after membership ensure");
            if params.focus {
                self.state.switch_workspace(ws_idx);
            }
            (ws_idx, created_source_workspace)
        } else {
            match self.create_workspace_with_options(entry.path.clone(), params.focus) {
                Ok(ws_idx) => (ws_idx, true),
                Err(err) => return encode_error(id, "worktree_open_failed", err.to_string()),
            }
        };
        self.mark_worktree_membership(
            &source,
            ws_idx,
            entry.path.clone(),
            canonical_path != crate::worktree::canonical_or_original(&source.source_repo_root),
            !created_workspace,
        );
        if let Some(label) = params.label {
            let workspace_id = self.public_workspace_id(ws_idx);
            if let Some(ws) = self.state.workspaces.get_mut(ws_idx) {
                ws.set_custom_name(label.clone());
                crate::logging::workspace_renamed(&ws.id);
            }
            if !created_workspace {
                self.emit_event(EventEnvelope {
                    event: EventKind::WorkspaceRenamed,
                    data: EventData::WorkspaceRenamed {
                        workspace_id,
                        label,
                    },
                });
            }
        }
        self.state.mark_session_dirty();
        if created_workspace {
            self.emit_workspace_open_events(ws_idx);
        }

        let tab_idx = self.state.workspaces[ws_idx].active_tab;
        let worktree = self.worktree_info_for_entry(&source, entry);
        self.emit_worktree_opened_event(ws_idx, worktree.clone(), already_open.is_some());
        encode_success(
            id,
            ResponseResult::WorktreeOpened {
                workspace: self.workspace_info(ws_idx),
                tab: self
                    .tab_info(ws_idx, tab_idx)
                    .expect("opened worktree workspace should have an active tab"),
                root_pane: self
                    .root_pane_info(ws_idx, tab_idx)
                    .expect("opened worktree workspace should have an active root pane"),
                worktree,
                already_open: already_open.is_some(),
            },
        )
    }

    fn resolve_worktree_source(
        &mut self,
        workspace_id: Option<String>,
        cwd: Option<String>,
    ) -> Result<WorktreeSource, ApiFailure> {
        if workspace_id.is_some() && cwd.is_some() {
            return Err(ApiFailure::new(
                "invalid_request",
                "only one of workspace_id or cwd may be supplied",
            ));
        }

        if let Some(workspace_id) = workspace_id {
            let Some(ws_idx) = self.parse_workspace_id(&workspace_id) else {
                return Err(ApiFailure::new(
                    "workspace_not_found",
                    format!("workspace {workspace_id} not found"),
                ));
            };
            return self.worktree_source_from_workspace(ws_idx);
        }

        if let Some(cwd) = cwd {
            let path = absolute_user_path(&cwd)?;
            let space = crate::workspace::git_space_metadata(&path).ok_or_else(|| {
                ApiFailure::new(
                    "not_git_worktree",
                    "sesh-mux worktree actions require a path inside a Git work tree",
                )
            })?;
            if space.is_linked_worktree {
                return Err(ApiFailure::new(
                    "linked_worktree_source",
                    "New and open worktree actions start from the repo parent workspace.",
                ));
            }
            let source = WorktreeSource {
                workspace_idx: self.find_parent_workspace_for_space(&space),
                source_checkout_path: space.repo_root.clone(),
                source_repo_root: space.repo_root,
                repo_key: space.key,
                repo_name: space.repo_name,
            };
            return Ok(source);
        }

        let Some(ws_idx) = self.state.active.or_else(|| {
            self.state
                .workspaces
                .get(self.state.selected)
                .map(|_| self.state.selected)
        }) else {
            return Err(ApiFailure::new(
                "invalid_request",
                "workspace_id or cwd is required when no workspace is active",
            ));
        };
        self.worktree_source_from_workspace(ws_idx)
    }

    fn resolve_worktree_list_source(
        &mut self,
        workspace_id: Option<String>,
        cwd: Option<String>,
        trust_repository: bool,
    ) -> Result<WorktreeSource, ApiFailure> {
        if workspace_id.is_some() && cwd.is_some() {
            return Err(ApiFailure::new(
                "invalid_request",
                "only one of workspace_id or cwd may be supplied",
            ));
        }

        if let Some(workspace_id) = workspace_id {
            let Some(ws_idx) = self.parse_workspace_id(&workspace_id) else {
                return Err(ApiFailure::new(
                    "workspace_not_found",
                    format!("workspace {workspace_id} not found"),
                ));
            };
            return self.worktree_list_source_from_workspace(ws_idx, trust_repository);
        }

        if let Some(cwd) = cwd {
            let path = absolute_user_path(&cwd)?;
            let space = crate::workspace::git_space_metadata(&path).ok_or_else(|| {
                ApiFailure::new(
                    "not_git_worktree",
                    "sesh-mux worktree actions require a path inside a Git work tree",
                )
            })?;
            let workspace_idx = self.list_source_workspace_idx_for_space(&space, trust_repository);
            return Ok(worktree_source_from_space(
                space,
                workspace_idx,
                true,
                trust_repository,
            ));
        }

        let Some(ws_idx) = self.state.active.or_else(|| {
            self.state
                .workspaces
                .get(self.state.selected)
                .map(|_| self.state.selected)
        }) else {
            return Err(ApiFailure::new(
                "invalid_request",
                "workspace_id or cwd is required when no workspace is active",
            ));
        };
        self.worktree_list_source_from_workspace(ws_idx, trust_repository)
    }

    fn worktree_source_from_workspace(&self, ws_idx: usize) -> Result<WorktreeSource, ApiFailure> {
        let Some(ws) = self.state.workspaces.get(ws_idx) else {
            return Err(ApiFailure::new(
                "workspace_not_found",
                "workspace not found",
            ));
        };
        if let Some(membership) = ws.worktree_space() {
            if membership.is_linked_worktree {
                return Err(ApiFailure::new(
                    "linked_worktree_source",
                    "New and open worktree actions start from the repo parent workspace.",
                ));
            }
            return Ok(WorktreeSource {
                workspace_idx: Some(ws_idx),
                source_checkout_path: membership.checkout_path.clone(),
                source_repo_root: membership.repo_root.clone(),
                repo_key: membership.key.clone(),
                repo_name: membership.label.clone(),
            });
        }

        let git_space = ws.git_space().cloned().or_else(|| {
            ws.resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
                .as_deref()
                .and_then(crate::workspace::git_space_metadata)
        });
        let Some(space) = git_space else {
            return Err(ApiFailure::new(
                "not_git_worktree",
                "sesh-mux worktree actions require a workspace inside a Git work tree",
            ));
        };
        if space.is_linked_worktree {
            return Err(ApiFailure::new(
                "linked_worktree_source",
                "New and open worktree actions start from the repo parent workspace.",
            ));
        }
        Ok(WorktreeSource {
            workspace_idx: Some(ws_idx),
            source_checkout_path: space.repo_root.clone(),
            source_repo_root: space.repo_root,
            repo_key: space.key,
            repo_name: space.repo_name,
        })
    }

    fn worktree_list_source_from_workspace(
        &self,
        ws_idx: usize,
        trust_repository: bool,
    ) -> Result<WorktreeSource, ApiFailure> {
        let Some(ws) = self.state.workspaces.get(ws_idx) else {
            return Err(ApiFailure::new(
                "workspace_not_found",
                "workspace not found",
            ));
        };
        if let Some(membership) = ws.worktree_space() {
            let source_checkout_path = if membership.is_linked_worktree {
                membership.repo_root.clone()
            } else {
                membership.checkout_path.clone()
            };
            let workspace_idx = if membership.is_linked_worktree {
                self.open_workspace_idx_for_checkout(&membership.repo_root)
            } else {
                Some(ws_idx)
            };
            return Ok(WorktreeSource {
                workspace_idx,
                source_checkout_path,
                source_repo_root: membership.repo_root.clone(),
                repo_key: membership.key.clone(),
                repo_name: membership.label.clone(),
            });
        }

        let git_space = ws.git_space().cloned().or_else(|| {
            ws.resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
                .as_deref()
                .and_then(crate::workspace::git_space_metadata)
        });
        let Some(space) = git_space else {
            return Err(ApiFailure::new(
                "not_git_worktree",
                "sesh-mux worktree actions require a workspace inside a Git work tree",
            ));
        };
        let workspace_idx = if space.is_linked_worktree {
            self.list_source_workspace_idx_for_space(&space, trust_repository)
        } else {
            Some(ws_idx)
        };
        Ok(worktree_source_from_space(
            space,
            workspace_idx,
            true,
            trust_repository,
        ))
    }

    fn ensure_source_parent_membership(
        &mut self,
        source: &mut WorktreeSource,
        emit_created_event: bool,
    ) -> Result<bool, ApiFailure> {
        if source.workspace_idx.is_none() {
            source.workspace_idx = self.find_parent_workspace_by_key(&source.repo_key);
        }
        let mut created_parent = false;
        if source.workspace_idx.is_none() {
            let ws_idx = self
                .create_workspace_with_options(source.source_checkout_path.clone(), false)
                .map_err(|err| ApiFailure::new("worktree_open_failed", err.to_string()))?;
            source.workspace_idx = Some(ws_idx);
            created_parent = true;
        }
        if let Some(ws_idx) = source.workspace_idx {
            let membership =
                worktree_membership(source, source.source_checkout_path.clone(), false);
            self.set_worktree_membership(ws_idx, membership, !created_parent);
            if created_parent && emit_created_event {
                self.emit_workspace_open_events(ws_idx);
            }
        }
        Ok(created_parent)
    }

    fn find_parent_workspace_for_space(
        &self,
        space: &crate::workspace::GitSpaceMetadata,
    ) -> Option<usize> {
        self.find_parent_workspace_by_key(&space.key)
            .or_else(|| self.open_workspace_idx_for_checkout(&space.repo_root))
    }

    fn list_source_workspace_idx_for_space(
        &self,
        space: &crate::workspace::GitSpaceMetadata,
        trust_repository: bool,
    ) -> Option<usize> {
        if space.is_linked_worktree {
            let parent_checkout = parent_checkout_path_for_space(space, trust_repository);
            self.open_workspace_idx_for_checkout(&parent_checkout)
        } else {
            self.find_parent_workspace_for_space(space)
        }
    }

    fn find_parent_workspace_by_key(&self, repo_key: &str) -> Option<usize> {
        self.state.workspaces.iter().position(|ws| {
            ws.worktree_space()
                .is_some_and(|space| space.key == repo_key && !space.is_linked_worktree)
                || ws
                    .git_space()
                    .is_some_and(|space| space.key == repo_key && !space.is_linked_worktree)
        })
    }

    fn mark_worktree_membership(
        &mut self,
        source: &WorktreeSource,
        target_ws_idx: usize,
        target_path: PathBuf,
        target_is_linked_worktree: bool,
        emit_update: bool,
    ) {
        let membership = worktree_membership(source, target_path, target_is_linked_worktree);
        self.set_worktree_membership(target_ws_idx, membership, emit_update);
    }

    pub(crate) fn set_worktree_membership(
        &mut self,
        ws_idx: usize,
        membership: crate::workspace::WorktreeSpaceMembership,
        emit_update: bool,
    ) {
        let changed = if let Some(workspace) = self.state.workspaces.get_mut(ws_idx) {
            if workspace.worktree_space.as_ref() == Some(&membership) {
                false
            } else {
                workspace.worktree_space = Some(membership);
                true
            }
        } else {
            false
        };
        if changed {
            self.state.mark_session_dirty();
            if emit_update {
                self.emit_workspace_updated(ws_idx);
            }
        }
    }

    fn find_worktree_entry(
        &self,
        source: &WorktreeSource,
        path: Option<String>,
        branch: Option<String>,
        trust_repository: bool,
    ) -> Result<crate::worktree::ExistingWorktree, ApiFailure> {
        let entries =
            crate::worktree::list_existing_worktrees(&source.source_repo_root, trust_repository)
                .map_err(|err| ApiFailure::new("worktree_list_failed", err))?;
        if let Some(path) = path {
            let expected = absolute_user_path(&path)?;
            let expected = crate::worktree::canonical_or_original(&expected);
            entries
                .into_iter()
                .find(|entry| crate::worktree::canonical_or_original(&entry.path) == expected)
                .ok_or_else(|| ApiFailure::new("worktree_not_found", "worktree path not found"))
        } else if let Some(branch) = branch {
            let matches = entries
                .into_iter()
                .filter(|entry| {
                    !entry.is_bare
                        && !entry.is_prunable
                        && !entry.is_detached
                        && entry.branch.as_deref() == Some(branch.as_str())
                })
                .collect::<Vec<_>>();
            match matches.len() {
                0 => Err(ApiFailure::new(
                    "worktree_not_found",
                    "worktree branch not found",
                )),
                1 => Ok(matches.into_iter().next().expect("one match should exist")),
                _ => Err(ApiFailure::new(
                    "ambiguous_worktree_branch",
                    "multiple worktrees matched branch",
                )),
            }
        } else {
            Err(ApiFailure::new(
                "invalid_request",
                "exactly one of path or branch is required",
            ))
        }
    }

    fn worktree_source_info(&self, source: &WorktreeSource) -> WorktreeSourceInfo {
        WorktreeSourceInfo {
            repo_key: source.repo_key.clone(),
            repo_name: source.repo_name.clone(),
            repo_root: source.source_repo_root.display().to_string(),
            source_checkout_path: source.source_checkout_path.display().to_string(),
            source_workspace_id: source
                .workspace_idx
                .map(|idx| self.public_workspace_id(idx)),
        }
    }

    fn worktree_info_for_entry(
        &self,
        source: &WorktreeSource,
        entry: crate::worktree::ExistingWorktree,
    ) -> WorktreeInfo {
        let canonical_path = crate::worktree::canonical_or_original(&entry.path);
        let repo_root = crate::worktree::canonical_or_original(&source.source_repo_root);
        WorktreeInfo {
            path: entry.path.display().to_string(),
            branch: entry.branch,
            is_bare: entry.is_bare,
            is_detached: entry.is_detached,
            is_prunable: entry.is_prunable,
            is_linked_worktree: canonical_path != repo_root,
            open_workspace_id: self
                .open_workspace_idx_for_checkout(&canonical_path)
                .map(|idx| self.public_workspace_id(idx)),
            label: source.repo_name.clone(),
        }
    }

    pub(crate) fn worktree_info_for_membership(
        &self,
        membership: &crate::workspace::WorktreeSpaceMembership,
        open_workspace_id: Option<String>,
    ) -> WorktreeInfo {
        let branch = crate::workspace::git_branch(&membership.checkout_path);
        let is_detached = branch.is_none();
        WorktreeInfo {
            path: membership.checkout_path.display().to_string(),
            branch,
            is_bare: false,
            is_detached,
            is_prunable: false,
            is_linked_worktree: membership.is_linked_worktree,
            open_workspace_id,
            label: membership.label.clone(),
        }
    }

    pub(crate) fn open_workspace_idx_for_checkout(&self, checkout_path: &Path) -> Option<usize> {
        let canonical_checkout = crate::worktree::canonical_or_original(checkout_path);
        let checkout_key = canonical_checkout.display().to_string();
        self.state.workspaces.iter().position(|ws| {
            if let Some(space) = ws.worktree_space() {
                // Explicit checkout provenance must not be overridden by shell navigation.
                return crate::worktree::canonical_or_original(&space.checkout_path)
                    == canonical_checkout;
            }

            let git_space = ws.git_space().cloned().or_else(|| {
                ws.resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
                    .as_deref()
                    .and_then(crate::workspace::git_space_metadata)
            });
            if git_space
                .as_ref()
                .is_some_and(|metadata| metadata.checkout_key == checkout_key)
            {
                return true;
            }

            ws.resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
                .as_deref()
                .is_some_and(|cwd| {
                    crate::worktree::canonical_or_original(cwd) == canonical_checkout
                })
        })
    }

    pub(crate) fn worktree_info_for_workspace(&self, ws_idx: usize) -> Option<WorktreeInfo> {
        let membership = self.state.workspaces.get(ws_idx)?.worktree_space()?;
        Some(self.worktree_info_for_membership(membership, Some(self.public_workspace_id(ws_idx))))
    }

    pub(crate) fn emit_worktree_created_event(&mut self, ws_idx: usize, worktree: WorktreeInfo) {
        self.emit_event(EventEnvelope {
            event: EventKind::WorktreeCreated,
            data: EventData::WorktreeCreated {
                workspace: self.workspace_info(ws_idx),
                worktree,
            },
        });
    }

    fn emit_worktree_opened_event(
        &mut self,
        ws_idx: usize,
        worktree: WorktreeInfo,
        already_open: bool,
    ) {
        self.emit_event(EventEnvelope {
            event: EventKind::WorktreeOpened,
            data: EventData::WorktreeOpened {
                workspace: self.workspace_info(ws_idx),
                worktree,
                already_open,
            },
        });
    }

    pub(crate) fn emit_worktree_removed_event(
        &mut self,
        workspace_id: String,
        workspace: Option<crate::api::schema::WorkspaceInfo>,
        worktree: WorktreeInfo,
        forced: bool,
    ) {
        self.emit_event(EventEnvelope {
            event: EventKind::WorktreeRemoved,
            data: EventData::WorktreeRemoved {
                workspace_id,
                workspace,
                worktree,
                forced,
            },
        });
    }

    fn emit_workspace_updated(&mut self, ws_idx: usize) {
        self.emit_event(EventEnvelope {
            event: EventKind::WorkspaceUpdated,
            data: EventData::WorkspaceUpdated {
                workspace: self.workspace_info(ws_idx),
            },
        });
    }
}

fn worktree_source_from_space(
    space: crate::workspace::GitSpaceMetadata,
    workspace_idx: Option<usize>,
    allow_linked: bool,
    trust_repository: bool,
) -> WorktreeSource {
    let source_checkout_path = if allow_linked {
        parent_checkout_path_for_space(&space, trust_repository)
    } else {
        space.repo_root.clone()
    };
    WorktreeSource {
        workspace_idx,
        source_checkout_path: source_checkout_path.clone(),
        source_repo_root: source_checkout_path,
        repo_key: space.key,
        repo_name: space.repo_name,
    }
}

fn parent_checkout_path_for_space(
    space: &crate::workspace::GitSpaceMetadata,
    trust_repository: bool,
) -> PathBuf {
    if !space.is_linked_worktree {
        return space.repo_root.clone();
    }

    crate::worktree::list_existing_worktrees(&space.repo_root, trust_repository)
        .ok()
        .and_then(|entries| {
            entries.into_iter().find_map(|entry| {
                let entry_space = crate::workspace::git_space_metadata(&entry.path)?;
                if entry_space.key == space.key && !entry_space.is_linked_worktree {
                    Some(entry_space.repo_root)
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| space.repo_root.clone())
}

fn worktree_membership(
    source: &WorktreeSource,
    checkout_path: PathBuf,
    is_linked_worktree: bool,
) -> crate::workspace::WorktreeSpaceMembership {
    crate::workspace::WorktreeSpaceMembership {
        key: source.repo_key.clone(),
        label: source.repo_name.clone(),
        repo_root: source.source_repo_root.clone(),
        checkout_path,
        is_linked_worktree,
    }
}

#[cfg(test)]
mod tests {}
