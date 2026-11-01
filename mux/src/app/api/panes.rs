use bytes::Bytes;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, PaneClearAgentAuthorityParams, PaneCurrentParams,
    PaneListParams, PaneProcessInfo, PaneProcessInfoParams, PaneProcessInfoProcess, PaneReadParams,
    PaneReadResult, PaneReleaseAgentParams, PaneRenameParams, PaneReportAgentParams,
    PaneReportAgentSessionParams, PaneReportMetadataParams, PaneReportPermissionParams,
    PaneSendInputParams, PaneSendKeysParams, PaneSendTextParams, PaneTarget, ResponseResult,
};
use crate::app::App;
use crate::layout::PaneId;

use super::super::api_helpers::{
    detect_state_from_api, encode_api_keys, normalize_metadata_source, normalize_metadata_tokens,
    normalize_metadata_ttl, normalize_reported_agent_label, MAX_METADATA_TOKEN_KEYS_PER_RESOURCE,
};
use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_pane_list(&mut self, id: String, params: PaneListParams) -> String {
        match self.collect_panes_for_workspace(params.workspace_id.as_deref()) {
            Ok(panes) => encode_success(id, ResponseResult::PaneList { panes }),
            Err((code, message)) => encode_error(id, &code, message),
        }
    }

    pub(super) fn handle_pane_current(&mut self, id: String, params: PaneCurrentParams) -> String {
        let target = match params.caller_pane_id.as_deref() {
            Some(caller_pane_id) => self.parse_pane_id(caller_pane_id),
            None => self.resolve_optional_pane(None),
        };
        let Some((ws_idx, pane_id)) = target else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };

        encode_success(id, ResponseResult::PaneCurrent { pane })
    }

    pub(super) fn handle_pane_get(&mut self, id: String, target: PaneTarget) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&target.pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return pane_not_found(id, &target.pane_id);
        };

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_process_info(
        &mut self,
        id: String,
        params: PaneProcessInfoParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.resolve_optional_pane(params.pane_id.as_deref()) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let Some((runtime, _workspace_id)) = self.lookup_runtime(ws_idx, pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        let shell_pid = runtime.child_pid();
        let foreground_job = shell_pid.and_then(crate::detect::foreground_job);
        let foreground_process_group_id = foreground_job.as_ref().map(|job| job.process_group_id);
        let foreground_processes = foreground_job
            .map(|job| {
                job.processes
                    .into_iter()
                    .map(|process| PaneProcessInfoProcess {
                        pid: process.pid,
                        name: process.name,
                        argv0: process.argv0,
                        argv: process.argv,
                        cmdline: process.cmdline,
                        cwd: crate::platform::process_cwd(process.pid)
                            .map(|cwd| cwd.display().to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        encode_success(
            id,
            ResponseResult::PaneProcessInfo {
                process_info: PaneProcessInfo {
                    pane_id: public_pane_id,
                    shell_pid,
                    foreground_process_group_id,
                    tty: None,
                    foreground_processes,
                },
            },
        )
    }

    pub(super) fn handle_pane_rename(&mut self, id: String, params: PaneRenameParams) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.terminal_id(pane_id))
            .cloned()
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        match params.label.map(|label| label.trim().to_string()) {
            Some(label) if !label.is_empty() => terminal.set_manual_label(label),
            _ => terminal.clear_manual_label(),
        }
        self.state.mark_session_dirty();
        let pane = self.pane_info(ws_idx, pane_id).unwrap();

        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(super) fn handle_pane_read(&mut self, id: String, params: PaneReadParams) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(tab_idx) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.find_tab_index_for_pane(pane_id))
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let snapshot = crate::app::api_helpers::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: public_pane_id,
                    workspace_id,
                    tab_id: self.public_tab_id(ws_idx, tab_idx).unwrap(),
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: 0,
                    truncated: snapshot.truncated,
                },
            },
        )
    }

    pub(super) fn handle_pane_report_agent(
        &mut self,
        id: String,
        params: PaneReportAgentParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(agent_label) = normalize_reported_agent_label(&params.agent) else {
            return invalid_agent(id);
        };
        self.reporting(ws_idx, pane_id, |app| {
            app.handle_internal_event(crate::events::AppEvent::HookStateReported {
                pane_id,
                session_ref: crate::agent_resume::session_ref_from_report(
                    &params.source,
                    &agent_label,
                    params.agent_session_id,
                    params.agent_session_path,
                ),
                source: params.source,
                agent_label,
                state: detect_state_from_api(params.state),
                message: params.message,
                seq: params.seq,
            })
        });

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_report_agent_session(
        &mut self,
        id: String,
        params: PaneReportAgentSessionParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(agent_label) = normalize_reported_agent_label(&params.agent) else {
            return invalid_agent(id);
        };
        self.reporting(ws_idx, pane_id, |app| {
            app.handle_internal_event(crate::events::AppEvent::AgentSessionReported {
                pane_id,
                session_ref: crate::agent_resume::session_ref_from_report(
                    &params.source,
                    &agent_label,
                    params.agent_session_id,
                    params.agent_session_path,
                ),
                source: params.source,
                agent_label,
                seq: params.seq,
                session_start_source: crate::agent_resume::normalize_session_start_source(
                    params.session_start_source,
                ),
            })
        });

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_report_permission(
        &mut self,
        id: String,
        params: PaneReportPermissionParams,
    ) -> String {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal) = self
            .state
            .terminal_id_for_pane(ws_idx, pane_id)
            .and_then(|terminal_id| self.state.terminals.get_mut(&terminal_id))
        else {
            return pane_not_found(id, &params.pane_id);
        };
        terminal.pending_permission = Some(crate::terminal::state::PendingPermission {
            id: NEXT_ID
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .to_string(),
            tool: params.tool,
            input: params.input,
        });
        self.emit_pane_updated(ws_idx, pane_id);
        encode_success(id, ResponseResult::Ok {})
    }

    /// Plays a hook's report, and tells subscribers of the pane when it named another Agent
    /// session, which no state change announces, so a follower need not list panes on a timer.
    fn reporting(&mut self, ws_idx: usize, pane_id: PaneId, report: impl FnOnce(&mut Self)) {
        let session = |app: &Self| {
            app.pane_info(ws_idx, pane_id)
                .and_then(|pane| pane.agent_session)
        };
        let before = session(self);
        report(self);
        if session(self) != before {
            self.emit_pane_updated(ws_idx, pane_id);
        }
    }

    pub(super) fn handle_pane_report_metadata(
        &mut self,
        id: String,
        params: PaneReportMetadataParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let agent_label = match params.agent.as_deref() {
            Some(agent) => match normalize_reported_agent_label(agent) {
                Some(agent_label) => Some(agent_label),
                None => return invalid_agent(id),
            },
            None => None,
        };
        let source = match normalize_metadata_source(params.source) {
            Ok(source) => source,
            Err(message) => return encode_error(id, "invalid_metadata_source", message),
        };
        let raw_title_set = params.title.is_some();
        let raw_display_agent_set = params.display_agent.is_some();
        let raw_state_labels_set = !params.state_labels.is_empty();
        let tokens = if params.tokens.is_empty() {
            None
        } else {
            match normalize_metadata_tokens(params.tokens) {
                Ok(tokens) => Some(tokens),
                Err(message) => return encode_error(id, "invalid_metadata_token", message),
            }
        };
        let ttl = match normalize_metadata_ttl(params.ttl_ms) {
            Ok(ttl) => ttl,
            Err(message) => return encode_error(id, "invalid_metadata_ttl", message),
        };
        let title = normalize_presentation_text(params.title);
        let display_agent = normalize_presentation_text(params.display_agent);
        let applies_to_source = match params.applies_to_source {
            Some(applies_to_source) => match normalize_metadata_source(applies_to_source) {
                Ok(applies_to_source) => Some(applies_to_source),
                Err(message) => return encode_error(id, "invalid_metadata_source", message),
            },
            None => None,
        };
        let state_labels = match normalize_state_labels(params.state_labels) {
            Ok(labels) => labels,
            Err(status) => {
                return encode_error(
                    id,
                    "invalid_state_label",
                    format!("unknown state label: {status}"),
                );
            }
        };
        if raw_title_set && params.clear_title
            || raw_display_agent_set && params.clear_display_agent
            || raw_state_labels_set && params.clear_state_labels
        {
            return encode_error(
                id,
                "invalid_metadata_request",
                "cannot set and clear the same metadata field",
            );
        }
        if title.is_none()
            && display_agent.is_none()
            && state_labels.is_empty()
            && tokens.is_none()
            && !params.clear_title
            && !params.clear_display_agent
            && !params.clear_state_labels
        {
            return encode_error(
                id,
                "invalid_metadata_request",
                "missing metadata field to set or clear",
            );
        }
        let presentation_requested = title.is_some()
            || display_agent.is_some()
            || !state_labels.is_empty()
            || params.clear_title
            || params.clear_display_agent
            || params.clear_state_labels;
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|workspace| workspace.pane_state(pane_id))
            .map(|pane| pane.attached_terminal_id.clone())
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(terminal) = self.state.terminals.get_mut(&terminal_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        if terminal.metadata_report_blocked_by_process_exit(
            &source,
            agent_label.as_deref(),
            applies_to_source.as_deref(),
        ) {
            return encode_success(id, ResponseResult::Ok {});
        }
        if !terminal.metadata_report_sequence_is_fresh(&source, params.seq) {
            return encode_success(id, ResponseResult::Ok {});
        }
        let metadata_agent = crate::terminal::TerminalState::metadata_report_agent(
            &source,
            agent_label.as_deref(),
            applies_to_source.as_deref(),
        );
        if let Some(tokens) = tokens.as_ref() {
            if terminal.metadata_tokens.key_count_after_patch(tokens)
                > MAX_METADATA_TOKEN_KEYS_PER_RESOURCE
            {
                return encode_error(
                    id,
                    "metadata_token_limit",
                    format!(
                        "pane metadata may contain at most {MAX_METADATA_TOKEN_KEYS_PER_RESOURCE} tokens"
                    ),
                );
            }
        }
        match terminal.accept_metadata_report(&source, params.seq, tokens.is_some(), metadata_agent)
        {
            Ok(true) => {}
            Ok(false) => return encode_success(id, ResponseResult::Ok {}),
            Err(()) => {
                return encode_error(
                    id,
                    "metadata_sequence_source_limit",
                    format!(
                        "pane metadata may track at most {} sequenced sources",
                        crate::metadata_tokens::MAX_SEQUENCE_SOURCES
                    ),
                );
            }
        }
        let token_changed = tokens.is_some_and(|tokens| {
            let changed = terminal
                .metadata_tokens
                .patch(tokens, ttl, std::time::Instant::now());
            if changed {
                terminal.revision = terminal.revision.saturating_add(1);
            }
            changed
        });

        if presentation_requested {
            self.handle_internal_event(crate::events::AppEvent::HookMetadataReported {
                pane_id,
                source,
                agent_label,
                applies_to_source,
                title,
                display_agent,
                state_labels,
                clear_title: params.clear_title,
                clear_display_agent: params.clear_display_agent,
                clear_state_labels: params.clear_state_labels,
                seq: None,
                ttl,
            });
        }
        if token_changed {
            self.sync_agent_metadata_deadline();
            self.emit_pane_updated(ws_idx, pane_id);
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_clear_agent_authority(
        &mut self,
        id: String,
        params: PaneClearAgentAuthorityParams,
    ) -> String {
        let Some((_ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        self.handle_internal_event(crate::events::AppEvent::HookAuthorityCleared {
            pane_id,
            source: params.source,
            seq: params.seq,
        });

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_release_agent(
        &mut self,
        id: String,
        params: PaneReleaseAgentParams,
    ) -> String {
        let Some((_ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(agent_label) = normalize_reported_agent_label(&params.agent) else {
            return invalid_agent(id);
        };
        self.handle_internal_event(crate::events::AppEvent::HookAgentReleased {
            pane_id,
            source: params.source,
            known_agent: crate::detect::parse_agent_label(&agent_label),
            agent_label,
            seq: params.seq,
        });

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_send_text(
        &mut self,
        id: String,
        params: PaneSendTextParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(params.text)) {
            return encode_error(id, "pane_send_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_send_input(
        &mut self,
        id: String,
        params: PaneSendInputParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let bytes = match super::super::api_helpers::encode_api_input(
            runtime,
            &params.text,
            &params.keys,
        ) {
            Ok(bytes) => bytes,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            return encode_error(id, "pane_send_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(super) fn handle_pane_close(&mut self, id: String, target: PaneTarget) -> String {
        match self.close_pane(id.clone(), &target) {
            Ok(()) => encode_success(id, ResponseResult::Ok {}),
            Err(response) => response,
        }
    }

    /// Close a pane; `Err` carries the encoded error response.
    pub(super) fn close_pane(&mut self, id: String, target: &PaneTarget) -> Result<(), String> {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&target.pane_id) else {
            return Err(pane_not_found(id, &target.pane_id));
        };
        let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) else {
            return Err(pane_not_found(id, &target.pane_id));
        };
        let workspace_id = self.public_workspace_id(ws_idx);
        if self.state.close_pane_would_close_workspace(ws_idx, pane_id)
            && self.state.confirm_implicit_worktree_group_close(ws_idx)
        {
            return Err(encode_error(
                id,
                "confirmation_required",
                "closing this pane would close a worktree group",
            ));
        }
        let workspace_snapshot = self.workspace_info(ws_idx);
        let terminal_id = self.state.terminal_id_for_pane(ws_idx, pane_id);
        let should_close_workspace = {
            let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
                return Err(pane_not_found(id, &target.pane_id));
            };
            ws.close_pane(pane_id)
        };
        if should_close_workspace {
            self.state.selected = ws_idx;
            self.state.close_selected_workspace();
            self.shutdown_detached_terminal_runtimes();
            self.emit_event(EventEnvelope {
                event: EventKind::PaneClosed,
                data: EventData::PaneClosed {
                    pane_id: public_pane_id,
                    workspace_id: workspace_id.clone(),
                },
            });
            self.emit_event(EventEnvelope {
                event: EventKind::WorkspaceClosed,
                data: EventData::WorkspaceClosed {
                    workspace_id,
                    workspace: Some(workspace_snapshot),
                },
            });
        } else {
            self.state.remove_unattached_terminal_ids(terminal_id);
            self.shutdown_detached_terminal_runtimes();
            self.schedule_session_save();
            self.emit_event(EventEnvelope {
                event: EventKind::PaneClosed,
                data: EventData::PaneClosed {
                    pane_id: public_pane_id,
                    workspace_id,
                },
            });
        }

        Ok(())
    }

    pub(super) fn handle_pane_send_keys(
        &mut self,
        id: String,
        params: PaneSendKeysParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let encoded_keys = match encode_api_keys(runtime, &params.keys) {
            Ok(encoded_keys) => encoded_keys,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        for bytes in encoded_keys {
            if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
                return encode_error(id, "pane_send_failed", err.to_string());
            }
        }

        encode_success(id, ResponseResult::Ok {})
    }
}

fn normalize_presentation_text(value: Option<String>) -> Option<String> {
    let trimmed = value?.trim().to_string();
    let normalized: String = trimmed
        .chars()
        .filter(|ch| !ch.is_control())
        .take(80)
        .collect();
    (!normalized.trim().is_empty()).then(|| normalized.trim().to_string())
}

fn normalize_state_labels(
    labels: std::collections::HashMap<String, String>,
) -> Result<std::collections::HashMap<String, String>, String> {
    labels
        .into_iter()
        .map(|(status, label)| {
            let status = status.trim().to_ascii_lowercase();
            if !matches!(
                status.as_str(),
                "idle" | "working" | "blocked" | "done" | "unknown"
            ) {
                return Err(status);
            }
            Ok(normalize_presentation_text(Some(label)).map(|label| (status, label)))
        })
        .filter_map(Result::transpose)
        .collect()
}

fn pane_not_found(id: String, pane_id: &str) -> String {
    encode_error(id, "pane_not_found", format!("pane {pane_id} not found"))
}

impl App {
    fn resolve_optional_pane(&self, pane_id: Option<&str>) -> Option<(usize, PaneId)> {
        match pane_id {
            Some(pane_id) => self.parse_pane_id(pane_id),
            None => {
                let ws_idx = self.state.active?;
                let pane_id = self.state.workspaces.get(ws_idx)?.focused_pane_id()?;
                Some((ws_idx, pane_id))
            }
        }
    }
}

fn invalid_agent(id: String) -> String {
    encode_error(id, "invalid_agent", "agent label must not be empty")
}

#[cfg(test)]
mod tests {}
