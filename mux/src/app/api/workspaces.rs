use std::path::PathBuf;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, WorkspaceCloseParams,
    WorkspaceCreateParams, WorkspaceRenameParams, WorkspaceReportMetadataParams, WorkspaceTarget,
};
use crate::app::App;

use super::super::api_helpers::{normalize_metadata_source, normalize_metadata_ttl};
use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_workspace_list(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::WorkspaceList {
                workspaces: self.workspace_list_info(),
            },
        )
    }

    pub(super) fn handle_workspace_get(&mut self, id: String, target: WorkspaceTarget) -> String {
        let Some(index) = self.parse_workspace_id(&target.workspace_id) else {
            return workspace_not_found(id, &target.workspace_id);
        };
        let Some(_) = self.state.workspaces.get(index) else {
            return workspace_not_found(id, &target.workspace_id);
        };

        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    pub(super) fn handle_workspace_create(
        &mut self,
        id: String,
        params: WorkspaceCreateParams,
    ) -> String {
        let source_workspace_index = if params.cwd.is_some() {
            None
        } else {
            match params.source_workspace_id.as_deref() {
                Some(workspace_id) => match self
                    .parse_workspace_id(workspace_id)
                    .filter(|index| self.state.workspaces.get(*index).is_some())
                {
                    Some(index) => Some(index),
                    None => return workspace_not_found(id, workspace_id),
                },
                None => self.workspace_creation_source(),
            }
        };
        let cwd = params.cwd.map(PathBuf::from).unwrap_or_else(|| {
            source_workspace_index.map_or_else(
                || self.resolve_new_terminal_cwd(None),
                |index| self.resolved_new_workspace_cwd_from(index),
            )
        });
        let extra_env = match super::env::normalize_launch_env(params.env) {
            Ok(env) => env,
            Err((code, message)) => return encode_error(id, &code, message),
        };
        match self.create_workspace_with_launch_env(cwd, params.focus, extra_env) {
            Ok(index) => {
                if let Some(label) = params.label {
                    if let Some(workspace) = self.state.workspaces.get_mut(index) {
                        workspace.set_custom_name(label);
                        crate::logging::workspace_renamed(&workspace.id);
                    }
                }
                self.emit_workspace_open_events(index);
                encode_success(
                    id,
                    self.workspace_created_result(index)
                        .expect("new workspace should produce a complete create response"),
                )
            }
            Err(err) => encode_error(id, "workspace_create_failed", err.to_string()),
        }
    }

    pub(super) fn handle_workspace_rename(
        &mut self,
        id: String,
        params: WorkspaceRenameParams,
    ) -> String {
        let Some(index) = self.parse_workspace_id(&params.workspace_id) else {
            return workspace_not_found(id, &params.workspace_id);
        };
        let Some(ws) = self.state.workspaces.get_mut(index) else {
            return workspace_not_found(id, &params.workspace_id);
        };
        ws.set_custom_name(params.label.clone());
        crate::logging::workspace_renamed(&ws.id);
        self.schedule_session_save();
        self.emit_event(EventEnvelope {
            event: EventKind::WorkspaceRenamed,
            data: EventData::WorkspaceRenamed {
                workspace_id: self.public_workspace_id(index),
                label: params.label,
            },
        });

        encode_success(
            id,
            ResponseResult::WorkspaceInfo {
                workspace: self.workspace_info(index),
            },
        )
    }

    pub(super) fn handle_workspace_report_metadata(
        &mut self,
        id: String,
        params: WorkspaceReportMetadataParams,
    ) -> String {
        let Some(index) = self.parse_workspace_id(&params.workspace_id) else {
            return workspace_not_found(id, &params.workspace_id);
        };
        let source = match normalize_metadata_source(params.source) {
            Ok(source) => source,
            Err(message) => return encode_error(id, "invalid_metadata_source", message),
        };
        let ttl = match normalize_metadata_ttl(params.ttl_ms) {
            Ok(ttl) => ttl,
            Err(message) => return encode_error(id, "invalid_metadata_ttl", message),
        };
        let tokens = match super::super::api_helpers::normalize_metadata_tokens(params.tokens) {
            Ok(tokens) => tokens,
            Err(message) => return encode_error(id, "invalid_metadata_token", message),
        };
        let Some(workspace) = self.state.workspaces.get_mut(index) else {
            return workspace_not_found(id, &params.workspace_id);
        };
        if !crate::metadata_tokens::sequence_is_fresh(
            &workspace.metadata_token_sequences,
            &source,
            params.seq,
        ) {
            return encode_success(id, ResponseResult::Ok {});
        }
        if workspace.metadata_tokens.key_count_after_patch(&tokens)
            > super::super::api_helpers::MAX_METADATA_TOKEN_KEYS_PER_RESOURCE
        {
            return encode_error(
                id,
                "metadata_token_limit",
                format!(
                    "workspace metadata may contain at most {} tokens",
                    super::super::api_helpers::MAX_METADATA_TOKEN_KEYS_PER_RESOURCE
                ),
            );
        }
        match crate::metadata_tokens::accept_sequence(
            &mut workspace.metadata_token_sequences,
            &source,
            params.seq,
        ) {
            Ok(true) => {}
            Ok(false) => return encode_success(id, ResponseResult::Ok {}),
            Err(()) => {
                return encode_error(
                    id,
                    "metadata_sequence_source_limit",
                    format!(
                        "workspace metadata may track at most {} sequenced sources",
                        crate::metadata_tokens::MAX_SEQUENCE_SOURCES
                    ),
                );
            }
        }
        let changed = workspace
            .metadata_tokens
            .patch(tokens, ttl, std::time::Instant::now());
        if changed {
            self.sync_agent_metadata_deadline();
            self.emit_workspace_token_updated(index);
        }
        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_workspace_close(
        &mut self,
        id: String,
        params: WorkspaceCloseParams,
    ) -> String {
        let Some(index) = self.parse_workspace_id(&params.workspace_id) else {
            return workspace_not_found(id, &params.workspace_id);
        };
        if self.state.workspaces.get(index).is_none() {
            return workspace_not_found(id, &params.workspace_id);
        }
        let close_indices = self.state.workspace_close_indices(index);
        if close_indices.len() >= 2 && !params.close_group {
            return encode_error(
                id,
                "workspace_group_close_required",
                "workspace has linked worktree workspaces; use --group (close_group=true in the API) to close the group",
            );
        }
        let closed_workspaces = close_indices
            .iter()
            .map(|index| {
                (
                    self.public_workspace_id(*index),
                    self.workspace_info(*index),
                )
            })
            .collect::<Vec<_>>();
        self.state.selected = index;
        self.state.close_selected_workspace();
        self.shutdown_detached_terminal_runtimes();
        for (workspace_id, workspace) in closed_workspaces {
            self.emit_event(EventEnvelope {
                event: EventKind::WorkspaceClosed,
                data: EventData::WorkspaceClosed {
                    workspace_id,
                    workspace: Some(workspace),
                },
            });
        }

        encode_success(id, ResponseResult::Ok {})
    }

    fn workspace_list_info(&self) -> Vec<crate::api::schema::WorkspaceInfo> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .map(|(idx, _)| self.workspace_info(idx))
            .collect()
    }
}

fn workspace_not_found(id: String, workspace_id: &str) -> String {
    encode_error(
        id,
        "workspace_not_found",
        format!("workspace {workspace_id} not found"),
    )
}

#[cfg(test)]
mod tests {

    // `new_cwd = follow` must anchor on the focused pane for every creation
    // surface. Splits and tabs already do; a new workspace must follow the
    // focused pane too, not the source workspace's first-tab root pane.
}
