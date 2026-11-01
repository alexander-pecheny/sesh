mod agents;
mod env;
mod integrations;

mod panes;
pub(super) mod responses;
mod tabs;
mod workspaces;
mod worktrees;

use super::{api_helpers::pane_agent_status, App};
use crate::events::AppEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeExitAction {
    RespawnShell,
    ClosePane,
}

impl App {
    pub(crate) fn handle_internal_event_with_render_impact(&mut self, ev: AppEvent) -> bool {
        match ev {
            ev @ AppEvent::TerminalBell { .. } => {
                self.handle_internal_event(ev);
                false
            }
            ev => {
                self.handle_internal_event(ev);
                true
            }
        }
    }

    pub(crate) fn handle_internal_event(&mut self, ev: AppEvent) {
        let _ = self.handle_internal_event_with_pane_updates(ev);
    }

    pub(crate) fn handle_internal_event_with_pane_updates(
        &mut self,
        ev: AppEvent,
    ) -> Vec<crate::app::actions::PaneStateUpdate> {
        let mut worktree_restore_failed = false;
        let ev = match ev {
            AppEvent::WorktreeRuntimeRestoreFailed {
                pane_id,
                operation_id,
            } => {
                if !self.claim_worktree_runtime_restore_failure(pane_id, operation_id) {
                    return Vec::new();
                }
                worktree_restore_failed = true;
                AppEvent::PaneDied {
                    pane_id,
                    exit_reason: crate::platform::ChildExitReason::Exited,
                }
            }
            ev => ev,
        };
        if matches!(
            &ev,
            AppEvent::TerminalBell { .. } | AppEvent::ClipboardWrite { .. }
        ) {
            return Vec::new();
        }

        if let AppEvent::WorktreeAddFinished(result) = ev {
            self.handle_api_worktree_add_finished(*result);
            return Vec::new();
        }

        if let AppEvent::WorktreeRemoveFinished(result) = ev {
            return self.handle_api_worktree_remove_finished(*result);
        }

        let mut worktree_restore_updates = Vec::new();
        if let AppEvent::PaneDied { pane_id, .. } = &ev {
            if worktree_restore_failed {
                worktree_restore_updates
                    .extend(self.publish_worktree_runtime_agent_release(*pane_id));
            } else {
                let expected_exit = self
                    .pending_worktree_remove_runtime_exits
                    .get_mut(pane_id)
                    .map(|remaining| {
                        *remaining -= 1;
                        *remaining == 0
                    });
                if let Some(remove_entry) = expected_exit {
                    let restore_failed = if remove_entry {
                        self.pending_worktree_remove_runtime_exits.remove(pane_id);
                        let restore_requested = self
                            .pending_worktree_remove_runtime_restores
                            .remove(pane_id)
                            .is_some();
                        if restore_requested {
                            worktree_restore_updates
                                .extend(self.publish_worktree_runtime_agent_release(*pane_id));
                        }
                        restore_requested && !self.respawn_shell_for_launch_pane(*pane_id, false)
                    } else {
                        false
                    };
                    if !restore_failed {
                        return worktree_restore_updates;
                    }
                }
                if let Some(update) = self
                    .state
                    .publish_pane_process_exit_if_agent(*pane_id, false)
                {
                    self.sync_full_lifecycle_authority_detection_pauses();
                    self.emit_pane_state_update(&update);
                }
                if self.runtime_exit_action(*pane_id) == RuntimeExitAction::RespawnShell
                    && self.respawn_shell_for_launch_pane(*pane_id, true)
                {
                    self.render_dirty.request_generic();
                    self.render_notify.notify_one();
                    return worktree_restore_updates;
                }
            }
        }

        let checkpointed_pane_exit = matches!(
            &ev,
            AppEvent::PaneDied {
                pane_id,
                exit_reason,
            } if exit_reason.requires_session_checkpoint() && self.find_pane(*pane_id).is_some()
        );
        if checkpointed_pane_exit {
            self.checkpoint_session_before_pane_exit();
        }

        if let AppEvent::PaneDied { pane_id, .. } = &ev {
            if let Some((ws_idx, _)) = self.find_pane(*pane_id) {
                if let Some(public_pane_id) = self.public_pane_id(ws_idx, *pane_id) {
                    self.emit_event(crate::api::schema::EventEnvelope {
                        event: crate::api::schema::EventKind::PaneExited,
                        data: crate::api::schema::EventData::PaneExited {
                            pane_id: public_pane_id,
                            workspace_id: self.public_workspace_id(ws_idx),
                        },
                    });
                }
            }
        }

        let released_agent = if let AppEvent::HookAgentReleased {
            pane_id,
            known_agent,
            ..
        } = &ev
        {
            known_agent.map(|agent| (*pane_id, agent))
        } else {
            None
        };

        let terminal_cwd_reported = matches!(ev, AppEvent::TerminalCwdReported { .. });
        let mut pane_updates = self.state.handle_app_event(ev);
        if checkpointed_pane_exit {
            self.finish_checkpointed_pane_exit();
        }
        if let Some((pane_id, agent)) = released_agent {
            if pane_updates.iter().any(|update| update.pane_id == pane_id) {
                if let Some((ws_idx, _)) = self.find_pane(pane_id) {
                    if let Some(runtime) = self.state.runtime_for_pane_in_workspace(
                        &self.terminal_runtimes,
                        ws_idx,
                        pane_id,
                    ) {
                        runtime.begin_graceful_release(agent);
                    }
                }
            }
        }
        self.sync_full_lifecycle_authority_detection_pauses();
        if terminal_cwd_reported {
            self.render_dirty.request_generic();
            self.render_notify.notify_one();
        }
        for update in &pane_updates {
            self.emit_pane_state_update(update);
        }
        self.sync_agent_metadata_deadline();

        self.shutdown_detached_terminal_runtimes();
        pane_updates.extend(worktree_restore_updates);
        pane_updates
    }

    fn sync_full_lifecycle_authority_detection_pauses(&self) {
        for workspace in &self.state.workspaces {
            for tab in &workspace.tabs {
                for pane in tab.panes.values() {
                    let Some(terminal) = self.state.terminals.get(&pane.attached_terminal_id)
                    else {
                        continue;
                    };
                    let Some(runtime) = self.terminal_runtimes.get(&pane.attached_terminal_id)
                    else {
                        continue;
                    };
                    runtime.set_full_lifecycle_authority_active(
                        terminal.full_lifecycle_hook_authority_active(),
                    );
                }
            }
        }
    }

    fn runtime_exit_action(&self, pane_id: crate::layout::PaneId) -> RuntimeExitAction {
        let Some((_, pane_state)) = self.find_pane(pane_id) else {
            return RuntimeExitAction::ClosePane;
        };
        let Some(terminal) = self.state.terminals.get(&pane_state.attached_terminal_id) else {
            return RuntimeExitAction::ClosePane;
        };

        if terminal.respawn_shell_on_exit {
            RuntimeExitAction::RespawnShell
        } else {
            RuntimeExitAction::ClosePane
        }
    }

    fn respawn_shell_for_launch_pane(
        &mut self,
        pane_id: crate::layout::PaneId,
        focus_pane: bool,
    ) -> bool {
        let Some((ws_idx, pane_state)) = self.find_pane(pane_id) else {
            return false;
        };
        let terminal_id = pane_state.attached_terminal_id.clone();
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return false;
        };

        let cwd = terminal.cwd.clone();
        let (rows, cols) = self
            .terminal_runtimes
            .get(&terminal_id)
            .map(|runtime| runtime.current_size())
            .unwrap_or_else(|| self.state.estimate_pane_size());
        let Some(launch_env) = self.pane_launch_env(ws_idx, pane_id, Vec::new()) else {
            return false;
        };
        let runtime = match crate::terminal::TerminalRuntime::spawn(
            pane_id,
            rows,
            cols,
            cwd,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.state.host_terminal_appearance,
            crate::pane::PaneShellConfig::new(&self.state.default_shell, self.state.shell_mode),
            &launch_env,
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!(
                    pane = pane_id.raw(),
                    terminal = %terminal_id,
                    err = %err,
                    "failed to respawn shell after launch command exited"
                );
                return false;
            }
        };

        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        if let Some(terminal) = self.state.terminals.get_mut(&terminal_id) {
            terminal.clear_agent_runtime_identity_after_respawn();
        }
        if focus_pane {
            self.state.focus_pane_in_workspace(ws_idx, pane_id);
        }
        self.schedule_session_save();
        true
    }

    pub(crate) fn claim_worktree_runtime_restore_failure(
        &mut self,
        pane_id: crate::layout::PaneId,
        operation_id: u64,
    ) -> bool {
        if self.pending_worktree_remove_runtime_restores.get(&pane_id) != Some(&operation_id) {
            return false;
        }
        self.pending_worktree_remove_runtime_restores
            .remove(&pane_id);
        self.pending_worktree_remove_runtime_exits.remove(&pane_id);
        true
    }

    pub(crate) fn publish_worktree_runtime_agent_release(
        &mut self,
        pane_id: crate::layout::PaneId,
    ) -> Option<crate::app::actions::PaneStateUpdate> {
        let update = self
            .state
            .publish_pane_process_exit_if_agent(pane_id, true)?;
        self.sync_full_lifecycle_authority_detection_pauses();
        self.emit_pane_state_update(&update);
        Some(update)
    }

    fn queue_worktree_runtime_restore_failed(
        &self,
        pane_id: crate::layout::PaneId,
        operation_id: u64,
    ) {
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let _ = event_tx
                .send(AppEvent::WorktreeRuntimeRestoreFailed {
                    pane_id,
                    operation_id,
                })
                .await;
        });
    }

    pub(crate) fn emit_pane_state_update(&mut self, update: &crate::app::actions::PaneStateUpdate) {
        let Some(pane_id) = self.public_pane_id(update.ws_idx, update.pane_id) else {
            return;
        };
        let workspace_id = self.public_workspace_id(update.ws_idx);

        if update.agent_name_changed {
            self.emit_pane_updated(update.ws_idx, update.pane_id);
        }

        if update.previous_agent_label != update.agent_label || update.agent_released {
            self.emit_event(crate::api::schema::EventEnvelope {
                event: crate::api::schema::EventKind::PaneAgentDetected,
                data: crate::api::schema::EventData::PaneAgentDetected {
                    pane_id: pane_id.clone(),
                    workspace_id: workspace_id.clone(),
                    agent: update.agent_label.clone(),
                    released: update.agent_released,
                    final_status: update.agent_release_status,
                },
            });
        }

        let previous_agent_status = pane_agent_status(update.previous_state, update.previous_seen);
        let agent_status = self
            .state
            .workspaces
            .get(update.ws_idx)
            .and_then(|ws| ws.pane_state(update.pane_id))
            .map(|pane| pane_agent_status(update.state, pane.seen))
            .unwrap_or_else(|| pane_agent_status(update.state, update.seen));

        if previous_agent_status != agent_status
            || update.previous_presentation != update.presentation
        {
            let presentation = update.presentation.clone();
            self.emit_event(crate::api::schema::EventEnvelope {
                event: crate::api::schema::EventKind::PaneAgentStatusChanged,
                data: crate::api::schema::EventData::PaneAgentStatusChanged {
                    pane_id,
                    workspace_id,
                    agent_status,
                    agent: update.agent_label.clone(),
                    title: presentation.title,
                    display_agent: presentation.display_agent,
                    state_labels: presentation.state_labels,
                },
            });
        }
    }

    pub(super) fn emit_event(&mut self, event: crate::api::schema::EventEnvelope) {
        self.event_hub.push(event);
    }

    pub(crate) fn emit_pane_updated(&mut self, ws_idx: usize, pane_id: crate::layout::PaneId) {
        if let Some(pane) = self.pane_info(ws_idx, pane_id) {
            self.emit_event(crate::api::schema::EventEnvelope {
                event: crate::api::schema::EventKind::PaneUpdated,
                data: crate::api::schema::EventData::PaneUpdated { pane },
            });
        }
    }

    pub(crate) fn emit_workspace_token_updated(&mut self, ws_idx: usize) {
        self.event_hub.push(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::WorkspaceMetadataUpdated,
            data: crate::api::schema::EventData::WorkspaceMetadataUpdated {
                workspace: self.workspace_info(ws_idx),
            },
        });
    }

    pub(crate) fn handle_api_request_after_internal_events_drained(
        &mut self,
        request: crate::api::schema::Request,
    ) -> String {
        self.sync_pending_terminal_titles();
        use crate::api::schema::{
            ErrorBody, ErrorResponse, Method, ResponseResult, SuccessResponse,
        };

        let response = match request.method {
            Method::ServerStop(_) => {
                self.state.should_quit = true;
                SuccessResponse {
                    id: request.id,
                    result: ResponseResult::Ok {},
                }
            }
            Method::ServerLiveHandoff(_) => {
                let response = ErrorResponse {
                    id: request.id,
                    error: ErrorBody {
                        code: "unsupported_in_app_mode".into(),
                        message: "live handoff is only supported by the headless server".into(),
                    },
                };
                return serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
            }
            Method::WorkspaceList(_) => return self.handle_workspace_list(request.id),
            Method::WorkspaceGet(target) => return self.handle_workspace_get(request.id, target),
            Method::WorkspaceCreate(params) => {
                return self.handle_workspace_create(request.id, params);
            }
            Method::WorkspaceRename(params) => {
                return self.handle_workspace_rename(request.id, params);
            }
            Method::WorkspaceReportMetadata(params) => {
                return self.handle_workspace_report_metadata(request.id, params);
            }
            Method::WorkspaceClose(target) => {
                return self.handle_workspace_close(request.id, target);
            }
            Method::WorktreeList(params) => return self.handle_worktree_list(request.id, params),
            Method::WorktreeCreate(params) => {
                let _ = params;
                return responses::encode_error(
                    request.id,
                    "invalid_request",
                    "worktree.create is handled asynchronously by the app runtime",
                );
            }
            Method::WorktreeOpen(params) => return self.handle_worktree_open(request.id, params),
            Method::WorktreeRemove(params) => {
                let _ = params;
                return responses::encode_error(
                    request.id,
                    "invalid_request",
                    "worktree.remove is handled asynchronously by the app runtime",
                );
            }
            Method::TabList(params) => return self.handle_tab_list(request.id, params),
            Method::TabGet(target) => return self.handle_tab_get(request.id, target),
            Method::TabCreate(params) => return self.handle_tab_create(request.id, params),
            Method::TabRename(params) => return self.handle_tab_rename(request.id, params),
            Method::TabClose(target) => return self.handle_tab_close(request.id, target),
            Method::AgentList(_) => return self.handle_agent_list(request.id),
            Method::AgentGet(target) => return self.handle_agent_get(request.id, target),
            Method::AgentRename(params) => return self.handle_agent_rename(request.id, params),
            Method::AgentStart(params) => return self.handle_agent_start(request.id, params),
            Method::AgentPrompt(_) => {
                return responses::encode_error(
                    request.id,
                    "invalid_request",
                    "agent.prompt is handled asynchronously by the app runtime",
                );
            }
            Method::AgentWait(_) => {
                return responses::encode_error(
                    request.id,
                    "invalid_request",
                    "agent.wait is handled by the api server",
                );
            }
            Method::AgentRead(params) => return self.handle_agent_read(request.id, params),
            Method::AgentSendKeys(params) => {
                return self.handle_agent_send_keys(request.id, params);
            }
            Method::PaneProcessInfo(params) => {
                return self.handle_pane_process_info(request.id, params);
            }
            Method::PaneList(params) => return self.handle_pane_list(request.id, params),
            Method::PaneCurrent(params) => return self.handle_pane_current(request.id, params),
            Method::PaneGet(target) => return self.handle_pane_get(request.id, target),
            Method::PaneRename(params) => return self.handle_pane_rename(request.id, params),
            Method::PaneRead(params) => return self.handle_pane_read(request.id, params),
            Method::PaneReportAgent(params) => {
                return self.handle_pane_report_agent(request.id, params);
            }
            Method::PaneReportAgentSession(params) => {
                return self.handle_pane_report_agent_session(request.id, params);
            }
            Method::PaneReportPermission(params) => {
                return self.handle_pane_report_permission(request.id, params);
            }
            Method::PaneReportMetadata(params) => {
                return self.handle_pane_report_metadata(request.id, params);
            }
            Method::PaneClearAgentAuthority(params) => {
                return self.handle_pane_clear_agent_authority(request.id, params);
            }
            Method::PaneReleaseAgent(params) => {
                return self.handle_pane_release_agent(request.id, params);
            }
            Method::PaneSendText(params) => return self.handle_pane_send_text(request.id, params),
            Method::PaneSendInput(params) => {
                return self.handle_pane_send_input(request.id, params);
            }
            Method::PaneClose(target) => return self.handle_pane_close(request.id, target),
            Method::PaneSendKeys(params) => return self.handle_pane_send_keys(request.id, params),
            Method::IntegrationList(_) => {
                return self.handle_integration_list(request.id);
            }
            Method::IntegrationInstall(params) => {
                return self.handle_integration_install(request.id, params);
            }
            Method::IntegrationUninstall(params) => {
                return self.handle_integration_uninstall(request.id, params);
            }
            _ => {
                return responses::encode_error(
                    request.id,
                    "not_implemented",
                    "method not implemented yet",
                );
            }
        };

        serde_json::to_string(&response).unwrap()
    }
}

#[cfg(test)]
pub(super) mod test_support {}

#[cfg(test)]
mod tests {}
