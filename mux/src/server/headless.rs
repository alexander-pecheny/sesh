//! The server loop: API requests, pane events, scheduled work and attached terminals.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use interprocess::local_socket::traits::Listener as _;
use interprocess::local_socket::ListenerNonblockingMode;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::api;
use crate::app;
use crate::config;
use crate::events::AppEvent;
use crate::ipc::{
    bind_local_listener, remove_socket_file_if_owned, socket_file_identity, LocalListener,
    SocketFileIdentity,
};
use crate::protocol::{
    self, AttachScrollDirection, AttachScrollSource, FrameData, ServerMessage, MAX_FRAME_SIZE,
};
use crate::server::attach_selection::{AttachSelection, MouseOutcome};
use crate::server::client_accept::{
    accept_pending_client_connections, reject_pending_client_connections,
};
use crate::server::client_transport::ServerEvent;
use crate::server::clients::{terminal_stream_client_ids, ClientConnection, ClientConnectionMode};
use crate::server::pane_input::{
    apply_client_pane_input_events, apply_terminal_attach_input, apply_terminal_attach_scroll,
    terminal_attach_mouse_position,
};
use crate::server::socket_paths::{
    client_socket_path, prepare_socket_path, restrict_socket_permissions,
};

mod bootstrap;
mod legacy;
mod lifecycle;
mod render;

pub use bootstrap::run_server;
use lifecycle::{wait_for_live_handoff_response_write, wait_for_old_public_sockets_to_close};

enum LoopEvent {
    Timer,
    Internal(AppEvent),
    Api(Box<api::ApiRequestMessage>),
    ServerEvent(ServerEvent),
    RenderRequested,
}

/// How often the idle loop wakes to accept attach clients from the non-blocking listener.
const CLIENT_ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(250);

struct AltScreenReadSpec {
    terminal_id: crate::terminal::TerminalId,
    lines: usize,
    unwrap: bool,
    initial: crate::terminal::ScreenSnapshot,
    content_seq: u64,
}

enum AltScreenReadConflict {
    None,
    Frozen(crate::pane::TerminalReadSnapshot),
    Defer,
}

pub struct HeadlessServer {
    app: app::App,
    api_tx: Option<api::ApiRequestSender>,
    api_server: Option<api::ServerHandle>,
    client_listener: LocalListener,
    client_socket_path: PathBuf,
    client_socket_identity: SocketFileIdentity,
    clients: HashMap<u64, ClientConnection>,
    next_client_id: u64,
    /// Writable direct attach owner per terminal id string.
    terminal_attach_owners: HashMap<String, u64>,
    pending_alt_screen_reads: Vec<crate::server::alt_screen_read::PendingAltScreenRead>,
    deferred_alt_screen_reads: Vec<api::ApiRequestMessage>,
    next_activity_stamp: u64,
    /// Pane size used while no attach client holds a pane.
    headless_size: (u16, u16),
    shutting_down: bool,
    handoff_in_progress: bool,
    pub(super) pending_handoff_repaint_nudge: bool,
    legacy: legacy::LegacyListener,
    should_quit: Arc<AtomicBool>,
    host_shutdown_requested: Arc<AtomicBool>,
    server_event_rx: mpsc::Receiver<ServerEvent>,
    server_event_tx: mpsc::Sender<ServerEvent>,
}

impl HeadlessServer {
    pub fn new(
        app: app::App,
        api_tx: Option<api::ApiRequestSender>,
        api_server: Option<api::ServerHandle>,
        should_quit: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        let client_path = client_socket_path();
        prepare_socket_path(&client_path)?;
        let listener = bind_local_listener(&client_path)?;
        restrict_socket_permissions(&client_path)?;
        let client_socket_identity = socket_file_identity(&client_path)?;
        listener.set_nonblocking(ListenerNonblockingMode::Accept)?;
        info!(path = %client_path.display(), "client protocol socket listening");

        let (server_event_tx, server_event_rx) = mpsc::channel(64);
        let headless_size = app.state.headless_size;
        Ok(Self {
            app,
            api_tx,
            api_server,
            client_listener: listener,
            client_socket_path: client_path,
            client_socket_identity,
            clients: HashMap::new(),
            next_client_id: 1,
            terminal_attach_owners: HashMap::new(),
            pending_alt_screen_reads: Vec::new(),
            deferred_alt_screen_reads: Vec::new(),
            next_activity_stamp: 1,
            headless_size,
            shutting_down: false,
            handoff_in_progress: false,
            pending_handoff_repaint_nudge: false,
            legacy: legacy::LegacyListener::default(),
            should_quit,
            host_shutdown_requested: Arc::new(AtomicBool::new(false)),
            server_event_rx,
            server_event_tx,
        })
    }

    pub async fn run(&mut self) -> io::Result<()> {
        crate::logging::startup("server");
        let should_quit = self.should_quit.clone();
        let quit_notify = self.server_event_tx.clone();
        ctrlc_handler(should_quit, quit_notify);
        let quit_notify = self.server_event_tx.clone();
        let _host_shutdown = crate::platform::HostShutdownMonitor::start(
            self.host_shutdown_requested.clone(),
            move || {
                let _ = quit_notify.try_send(ServerEvent::QuitSignal);
            },
        );

        let mut needs_render = true;
        loop {
            self.app.reap_finished_detached_processes();
            if self.shutting_down {
                self.complete_shutdown().await?;
                break;
            }
            // logind's delay lock stays held until the final session save, so pane deaths
            // are not drained here.
            if self.host_shutdown_requested.load(Ordering::Acquire) {
                self.initiate_shutdown();
                continue;
            }
            if self.app.state.should_quit || self.should_quit.load(Ordering::Acquire) {
                self.drain_internal_events_up_to(crate::app::APP_EVENT_CHANNEL_CAPACITY);
                self.initiate_shutdown();
                continue;
            }

            needs_render |= self.app.render_dirty.is_pending();
            needs_render |= self
                .drain_internal_events_up_to(crate::app::APP_EVENT_DRAIN_LIMIT)
                .1;
            if self.should_quit.load(Ordering::Acquire) {
                continue;
            }
            needs_render |= self.app.expire_due_metadata(Instant::now());
            needs_render |= self.drain_api_requests();
            if self.should_quit.load(Ordering::Acquire) {
                continue;
            }
            self.app.sync_session_save_schedule();
            self.accept_client_connections()?;
            needs_render |= self.drain_server_events();
            if self.should_quit.load(Ordering::Acquire) {
                continue;
            }

            let now = Instant::now();
            needs_render |= self.handle_scheduled_tasks(now);
            self.poll_pending_alt_screen_reads(now);
            needs_render |= self.process_deferred_alt_screen_reads();
            self.sync_legacy_listener();
            self.sync_immediate_pty_sources();
            self.stream_host_mouse_capture_mode();
            self.stream_direct_terminal_keyboard_mode();

            if needs_render && self.app.can_render_now(now) {
                let request = self.app.render_dirty.take();
                self.app
                    .sync_terminal_titles(&request.terminal_title_sources);
                self.render_and_stream();
                self.app.record_render_attempt(now);
                needs_render = false;
                continue;
            }

            let next_deadline = self
                .app
                .next_loop_deadline(now, needs_render)
                .map_or(now + CLIENT_ACCEPT_POLL_INTERVAL, |deadline| {
                    deadline.min(now + CLIENT_ACCEPT_POLL_INTERVAL)
                });
            let next_deadline = self
                .pending_alt_screen_reads
                .iter()
                .map(|pending| pending.next_deadline())
                .chain(
                    self.clients
                        .values()
                        .filter_map(|client| client.attach_selection.deadline()),
                )
                .fold(next_deadline, Instant::min);
            let event = tokio::select! {
                maybe_api = self.app.api_rx.recv() => match maybe_api {
                    Some(msg) => LoopEvent::Api(Box::new(msg)),
                    None => LoopEvent::Timer,
                },
                maybe_ev = self.app.event_rx.recv() => match maybe_ev {
                    Some(ev) => LoopEvent::Internal(ev),
                    None => LoopEvent::Timer,
                },
                maybe_server_ev = self.server_event_rx.recv() => match maybe_server_ev {
                    Some(ev) => LoopEvent::ServerEvent(ev),
                    None => LoopEvent::Timer,
                },
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(next_deadline)) => LoopEvent::Timer,
                _ = self.app.render_notify.notified() => LoopEvent::RenderRequested,
            };

            if self.should_quit.load(Ordering::Acquire)
                || self.host_shutdown_requested.load(Ordering::Acquire)
            {
                match event {
                    LoopEvent::Internal(ev) => {
                        self.handle_internal_event(ev);
                    }
                    LoopEvent::ServerEvent(ServerEvent::ClientConnected { writer, .. }) => {
                        if let Ok(message) =
                            Self::frame_server_message(&ServerMessage::ServerShutdown {
                                reason: Some("server is shutting down".to_owned()),
                            })
                        {
                            let _ = writer.control.send(message);
                        }
                    }
                    _ => {}
                }
                continue;
            }

            needs_render |= match event {
                LoopEvent::Timer => false,
                LoopEvent::Internal(ev) => self.handle_internal_event(ev),
                LoopEvent::Api(msg) => self.handle_api_request(*msg),
                LoopEvent::ServerEvent(ev) => self.handle_server_event(ev),
                LoopEvent::RenderRequested => self.app.render_dirty.is_pending(),
            };
        }

        if self.app.policy.persist_session {
            self.app.save_session_on_shutdown();
        }
        info!("headless server exiting");
        Ok(())
    }

    fn remove_client(&mut self, client_id: u64) {
        let Some(removed) = self.clients.remove(&client_id) else {
            return;
        };
        let ClientConnectionMode::TerminalAttach { terminal_id } = removed.mode else {
            return;
        };
        self.terminal_attach_owners.remove(&terminal_id);
        let Some(terminal_id) = self.terminal_id_by_string(&terminal_id) else {
            return;
        };
        self.app
            .state
            .direct_attach_resize_locks
            .remove(&terminal_id);
        if let Some(runtime) = self.app.terminal_runtimes.get(&terminal_id) {
            let (cols, rows) = self.headless_size;
            runtime.resize(rows, cols, 0, 0);
        }
    }

    fn remove_client_and_resize_if_needed(&mut self, client_id: u64) {
        self.remove_client(client_id);
    }

    fn accept_client_connections(&mut self) -> io::Result<()> {
        if self.handoff_in_progress {
            return reject_pending_client_connections(&self.client_listener);
        }
        accept_pending_client_connections(
            &self.client_listener,
            &mut self.next_client_id,
            &self.should_quit,
            &self.server_event_tx,
        )
    }

    fn drain_server_events(&mut self) -> bool {
        let mut changed = false;
        while !self.should_quit.load(Ordering::Acquire) {
            let Ok(ev) = self.server_event_rx.try_recv() else {
                break;
            };
            changed |= self.handle_server_event(ev);
        }
        changed
    }

    fn disconnect_all_clients_for_handoff(&mut self) {
        let client_ids = self.clients.keys().copied().collect::<Vec<_>>();
        for client_id in client_ids {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(
                        "live update in progress; reconnect after handoff completes".to_owned(),
                    ),
                },
            );
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.writer = None;
            }
            self.remove_client(client_id);
        }
    }

    fn handle_server_event(&mut self, ev: ServerEvent) -> bool {
        if self.handoff_in_progress
            && !matches!(
                ev,
                ServerEvent::ClientConnected { .. }
                    | ServerEvent::ClientDisconnected { .. }
                    | ServerEvent::ClientWriterDrained { .. }
                    | ServerEvent::QuitSignal
            )
        {
            return false;
        }
        match ev {
            ServerEvent::ClientConnected {
                client_id,
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse,
                writer,
            } => {
                if self.handoff_in_progress {
                    if let Ok(message) =
                        Self::frame_server_message(&ServerMessage::ServerShutdown {
                            reason: Some(
                                "live update in progress; reconnect after handoff completes"
                                    .to_owned(),
                            ),
                        })
                    {
                        let _ = writer.control.send(message);
                    }
                    return false;
                }
                info!(client_id, cols, rows, "terminal client connected");
                let last_activity = self.allocate_activity_stamp();
                let cell_size = crate::server::clients::CellSize {
                    width_px: cell_width_px,
                    height_px: cell_height_px,
                };
                let mut connection = ClientConnection::new_with_mode(
                    ClientConnectionMode::TerminalPending,
                    (cols, rows),
                    cell_size,
                    last_activity,
                    protocol::RenderEncoding::TerminalAnsi,
                    Some(writer),
                );
                connection.pixel_mouse = pixel_mouse && cell_size.is_known();
                self.clients.insert(client_id, connection);
                false
            }
            ServerEvent::ClientAttachTerminal {
                client_id,
                terminal_id,
                takeover,
            } => {
                let attached = self.attach_terminal_client(client_id, terminal_id, takeover);
                if attached && self.pending_handoff_repaint_nudge {
                    self.pending_handoff_repaint_nudge = false;
                    self.app
                        .terminal_runtimes
                        .nudge_child_redraw_after_handoff();
                }
                attached
            }
            ServerEvent::ClientAttachScroll {
                client_id,
                source,
                direction,
                lines,
                column,
                row,
                modifiers,
            } => self.handle_terminal_attach_scroll(
                client_id, source, direction, lines, column, row, modifiers,
            ),
            ServerEvent::ClientAttachMouse {
                client_id,
                kind,
                position,
                geometry,
                modifiers,
                lines,
            } => self.handle_terminal_attach_mouse(
                client_id, kind, position, geometry, modifiers, lines,
            ),
            ServerEvent::ClientInput { client_id, data } => {
                let copy_on_select = self.app.state.copy_on_select;
                let copied = self.with_attach_selection(client_id, |selection, runtime| {
                    let text = (!copy_on_select)
                        .then(|| selection.take_copy_key(runtime, &data))
                        .flatten();
                    selection.clear();
                    text
                });
                if let Some(text) = copied.flatten() {
                    self.send_attach_clipboard(client_id, &text);
                    return true;
                }
                let Some(ClientConnection {
                    mode: ClientConnectionMode::TerminalAttach { terminal_id },
                    ..
                }) = self.clients.get(&client_id)
                else {
                    return false;
                };
                if let Some(runtime) = self.runtime_for_terminal_id_string(terminal_id) {
                    if let Err(err) = apply_terminal_attach_input(runtime, data) {
                        warn!(client_id, terminal_id = %terminal_id, err = %err);
                    }
                }
                true
            }
            ServerEvent::ClientPasteRejected { client_id, .. } => {
                debug!(client_id, "rejected an oversized paste");
                false
            }
            ServerEvent::ClientResize {
                client_id,
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse,
            } => {
                let cell_size = crate::server::clients::CellSize {
                    width_px: cell_width_px,
                    height_px: cell_height_px,
                };
                let Some(client) = self.clients.get_mut(&client_id) else {
                    return false;
                };
                client.terminal_size = (cols, rows);
                client.cell_size = cell_size;
                client.pixel_mouse = pixel_mouse && cell_size.is_known();
                client.render_state.request_repaint();
                if let ClientConnectionMode::TerminalAttach { terminal_id } = &client.mode {
                    let terminal_id = terminal_id.clone();
                    if let Some(runtime) = self.runtime_for_terminal_id_string(&terminal_id) {
                        runtime.resize(rows, cols, cell_size.width_px, cell_size.height_px);
                    }
                }
                true
            }
            ServerEvent::ClientDetach { client_id } => {
                info!(client_id, "client detached");
                self.send_terminal_stream_detach_shutdown(client_id);
                self.remove_client(client_id);
                true
            }
            ServerEvent::ClientDisconnected { client_id } => {
                info!(client_id, "client disconnected");
                self.remove_client(client_id);
                true
            }
            ServerEvent::ClientWriterDrained { client_id } => self
                .clients
                .get_mut(&client_id)
                .is_some_and(|client| client.take_deferred_render()),
            ServerEvent::QuitSignal => false,
        }
    }

    fn handle_internal_event(&mut self, ev: AppEvent) -> bool {
        if self.host_shutdown_requested.load(Ordering::Acquire) {
            return false;
        }
        match &ev {
            AppEvent::TerminalBell { pane_id, count } => {
                for client_id in self.terminal_attach_client_ids(*pane_id) {
                    self.send_to_client(client_id, ServerMessage::TerminalBell { count: *count });
                }
                false
            }
            AppEvent::ClipboardWrite { pane_id, content } => {
                let data = base64::engine::general_purpose::STANDARD.encode(content.as_slice());
                for client_id in self.terminal_attach_client_ids(*pane_id) {
                    self.send_to_client(client_id, ServerMessage::Clipboard { data: data.clone() });
                }
                false
            }
            AppEvent::StateChanged { .. } | AppEvent::HookStateReported { .. } => {
                self.app.handle_internal_event_with_pane_updates(ev);
                true
            }
            AppEvent::WorktreeRemoveFinished(result) => {
                let shutdown_terminals = result
                    .api_request
                    .as_ref()
                    .map(|request| {
                        request
                            .shutdown_panes
                            .iter()
                            .filter_map(|pane_id| {
                                self.app.find_pane(*pane_id).map(|(_, pane)| {
                                    (*pane_id, pane.attached_terminal_id.to_string())
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.app.handle_internal_event_with_pane_updates(ev);
                for (pane_id, terminal_id) in shutdown_terminals {
                    if self.app.find_pane(pane_id).is_none() {
                        self.shutdown_terminal_stream_clients(
                            &terminal_id,
                            format!("terminal {terminal_id} exited"),
                        );
                    }
                }
                true
            }
            AppEvent::PaneDied { pane_id, .. }
            | AppEvent::WorktreeRuntimeRestoreFailed { pane_id, .. } => {
                let pane_id = *pane_id;
                let terminal_id = self
                    .app
                    .find_pane(pane_id)
                    .map(|(_, pane)| pane.attached_terminal_id.to_string());
                if matches!(&ev, AppEvent::PaneDied { .. })
                    && !self
                        .app
                        .pending_worktree_remove_runtime_exits
                        .contains_key(&pane_id)
                {
                    if let Some(update) = self
                        .app
                        .state
                        .publish_pane_process_exit_if_agent(pane_id, false)
                    {
                        self.app.emit_pane_state_update(&update);
                    }
                }
                self.app.handle_internal_event_with_pane_updates(ev);
                if self.app.find_pane(pane_id).is_none() {
                    if let Some(terminal_id) = terminal_id {
                        self.shutdown_terminal_stream_clients(
                            &terminal_id,
                            format!("terminal {terminal_id} exited"),
                        );
                    }
                }
                true
            }
            _ => self.app.handle_internal_event_with_render_impact(ev),
        }
    }

    fn drain_all_internal_events(&mut self) -> bool {
        let mut changed = false;
        loop {
            let (had_event, batch_changed) =
                self.drain_internal_events_up_to(crate::app::APP_EVENT_DRAIN_LIMIT);
            changed |= batch_changed;
            if !had_event || self.should_quit.load(Ordering::Acquire) {
                break;
            }
        }
        changed
    }

    fn drain_internal_events_up_to(&mut self, limit: usize) -> (bool, bool) {
        let mut had_event = false;
        let mut changed = false;
        for _ in 0..limit {
            if self.host_shutdown_requested.load(Ordering::Acquire) {
                break;
            }
            let Ok(ev) = self.app.event_rx.try_recv() else {
                break;
            };
            had_event = true;
            changed |= self.handle_internal_event(ev);
        }
        (had_event, changed)
    }

    fn drain_api_requests(&mut self) -> bool {
        let mut changed = false;
        while !self.should_quit.load(Ordering::Acquire) {
            let Ok(msg) = self.app.api_rx.try_recv() else {
                break;
            };
            changed |= self.handle_api_request(msg);
        }
        changed
    }

    fn respond_error(msg: api::ApiRequestMessage, code: &str, message: String) {
        let response = serde_json::to_string(&api::schema::ErrorResponse {
            id: msg.request.id,
            error: api::schema::ErrorBody {
                code: code.into(),
                message,
            },
        })
        .unwrap_or_else(|_| "{}".to_string());
        let _ = msg.respond_to.send(response);
    }

    fn handle_api_request(&mut self, msg: api::ApiRequestMessage) -> bool {
        if self.shutting_down {
            Self::respond_error(msg, "server_unavailable", "server is shutting down".into());
            return false;
        }
        let frozen_alt_screen_read = match self.alt_screen_read_conflict(&msg.request) {
            AltScreenReadConflict::None => None,
            AltScreenReadConflict::Frozen(snapshot) => Some(snapshot),
            AltScreenReadConflict::Defer => {
                self.deferred_alt_screen_reads.push(msg);
                return false;
            }
        };
        let mut changed = self.app.expire_due_metadata(Instant::now());

        if let api::schema::Method::ServerLiveHandoff(params) = &msg.request.method {
            match self.perform_live_handoff(params.clone()) {
                Ok(()) => {
                    let response = serde_json::to_string(&api::schema::SuccessResponse {
                        id: msg.request.id,
                        result: api::schema::ResponseResult::Ok {},
                    })
                    .unwrap_or_else(|_| "{}".to_string());
                    let _ = msg.respond_to.send(response);
                    wait_for_live_handoff_response_write(msg.response_write_complete);
                    self.finish_live_handoff_shutdown();
                }
                Err(err) => Self::respond_error(msg, "handoff_failed", err.to_string()),
            }
            return true;
        }

        changed |= self.drain_all_internal_events();
        if let Some(error) = self.agent_read_not_idle_error(&msg.request) {
            Self::respond_error(msg, &error.code, error.message);
            return changed;
        }
        let alt_screen_read_spec = self.alt_screen_read_spec(&msg.request);
        if matches!(&msg.request.method, api::schema::Method::AgentPrompt(_)) {
            return changed
                | self
                    .app
                    .handle_deferred_agent_api_request(msg.request, msg.respond_to);
        }
        if matches!(
            &msg.request.method,
            api::schema::Method::WorktreeCreate(_) | api::schema::Method::WorktreeRemove(_)
        ) {
            return changed
                | self
                    .app
                    .handle_deferred_worktree_api_request(msg.request, msg.respond_to);
        }
        let mut response = self
            .app
            .handle_api_request_after_internal_events_drained(msg.request);
        if let Some(snapshot) = frozen_alt_screen_read {
            if let Ok(mut success) = serde_json::from_str::<api::schema::SuccessResponse>(&response)
            {
                if let api::schema::ResponseResult::PaneRead { read } = &mut success.result {
                    read.text = snapshot.text;
                    read.truncated = snapshot.truncated;
                    if let Ok(serialized) = serde_json::to_string(&success) {
                        response = serialized;
                    }
                }
            }
        }
        if let Some(spec) = alt_screen_read_spec {
            if let Ok(success) = serde_json::from_str::<api::schema::SuccessResponse>(&response) {
                if let api::schema::ResponseResult::PaneRead { read } = success.result {
                    let pending = crate::server::alt_screen_read::PendingAltScreenRead::start(
                        spec.terminal_id,
                        success.id,
                        msg.respond_to,
                        response,
                        read,
                        spec.lines,
                        spec.unwrap,
                        spec.initial,
                        spec.content_seq,
                        Instant::now(),
                    );
                    self.pending_alt_screen_reads.push(pending);
                    return changed;
                }
            }
        }
        let _ = msg.respond_to.send(response);
        changed
    }

    fn handle_scheduled_tasks(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self
            .app
            .session_save_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            self.app.start_background_session_save();
        }
        if let Some(deadline) = self
            .app
            .agent_metadata_deadline
            .filter(|deadline| now >= *deadline)
        {
            self.app.expire_metadata_at(deadline, now);
            changed = true;
        }
        changed |= self.tick_attach_selections(now);
        self.app.sync_pending_agent_resume_deadline(now);
        changed |= self
            .app
            .start_pending_agent_resumes(self.app.pending_agent_resume_due(now));
        changed
    }

    fn handle_terminal_attach_scroll(
        &mut self,
        client_id: u64,
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    ) -> bool {
        if let (AttachScrollSource::Wheel, Some(column), Some(row)) = (&source, column, row) {
            let up = direction == AttachScrollDirection::Up;
            if self.with_attach_selection(client_id, |selection, runtime| {
                selection.wheel(runtime, up, lines, column, row)
            }) == Some(true)
            {
                return true;
            }
        }
        let Some(ClientConnection {
            mode: ClientConnectionMode::TerminalAttach { terminal_id },
            ..
        }) = self.clients.get(&client_id)
        else {
            return false;
        };
        let Some(runtime) = self.runtime_for_terminal_id_string(terminal_id) else {
            return false;
        };

        if let Err(err) =
            apply_terminal_attach_scroll(runtime, source, direction, lines, column, row, modifiers)
        {
            warn!(client_id, terminal_id = %terminal_id, err = %err, "terminal attach scroll failed");
        }
        true
    }

    fn handle_terminal_attach_mouse(
        &mut self,
        client_id: u64,
        kind: protocol::ClientMouseKind,
        position: protocol::ClientMousePosition,
        geometry: Option<protocol::ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    ) -> bool {
        if self.handoff_in_progress {
            return false;
        }
        let (protocol::ClientMousePosition::Cell { column, row }
        | protocol::ClientMousePosition::Pixels { column, row, .. }) = position;
        let copy_on_select = self.app.state.copy_on_select;
        let now = Instant::now();
        match self.with_attach_selection(client_id, |selection, runtime| {
            selection.mouse(
                runtime,
                kind.to_crossterm(),
                column,
                row,
                crossterm::event::KeyModifiers::from_bits_truncate(modifiers),
                copy_on_select,
                now,
            )
        }) {
            None => return false,
            Some(MouseOutcome::Forward) => {}
            Some(MouseOutcome::Handled) => return true,
            Some(MouseOutcome::Copy(text)) => {
                self.send_attach_clipboard(client_id, &text);
                return true;
            }
        }
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let ClientConnectionMode::TerminalAttach { terminal_id } = &client.mode else {
            return false;
        };
        let terminal_id = terminal_id.clone();
        let terminal_size = client.terminal_size;
        let cell_size = client.cell_size;
        let pixel_mouse = client.pixel_mouse;
        let host_sgr_pixels_active = client.host_sgr_pixels_active == Some(true);
        let Some(runtime) = self.runtime_for_terminal_id_string(&terminal_id) else {
            return false;
        };
        let Some(position) = terminal_attach_mouse_position(
            runtime,
            terminal_size,
            cell_size,
            pixel_mouse,
            host_sgr_pixels_active,
            position,
            geometry,
        ) else {
            return false;
        };
        let event = protocol::ClientPaneInputEvent::Mouse {
            kind,
            position,
            geometry: None,
            modifiers,
            lines: lines.max(1),
        };
        if let Err(err) = apply_client_pane_input_events(runtime, &[event]) {
            warn!(client_id, terminal_id = %terminal_id, err = %err, "terminal attach mouse input failed");
        }
        true
    }

    /// Lends a direct attach client's selection together with its attached runtime.
    fn with_attach_selection<T>(
        &mut self,
        client_id: u64,
        f: impl FnOnce(&mut AttachSelection, &crate::terminal::TerminalRuntime) -> T,
    ) -> Option<T> {
        let client = self.clients.get_mut(&client_id)?;
        let ClientConnectionMode::TerminalAttach { terminal_id } = &client.mode else {
            return None;
        };
        let terminal_id = terminal_id.clone();
        let mut selection = std::mem::take(&mut client.attach_selection);
        let result = self
            .runtime_for_terminal_id_string(&terminal_id)
            .map(|runtime| f(&mut selection, runtime));
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.attach_selection = selection;
        }
        result
    }

    fn send_attach_clipboard(&mut self, client_id: u64, text: &str) {
        let data = base64::engine::general_purpose::STANDARD.encode(text);
        self.send_to_client(client_id, ServerMessage::Clipboard { data });
    }

    fn tick_attach_selections(&mut self, now: Instant) -> bool {
        let due = self
            .clients
            .iter()
            .filter(|(_, client)| {
                client
                    .attach_selection
                    .deadline()
                    .is_some_and(|at| now >= at)
            })
            .map(|(&client_id, _)| client_id)
            .collect::<Vec<_>>();
        let mut changed = false;
        for client_id in due {
            changed |= self
                .with_attach_selection(client_id, |selection, runtime| selection.tick(runtime, now))
                .unwrap_or(false);
        }
        changed
    }

    /// Encodes a server message into a length-prefixed frame.
    fn frame_server_message(msg: &ServerMessage) -> Result<Vec<u8>, protocol::FramingError> {
        Self::frame_server_message_with_max(msg, MAX_FRAME_SIZE)
    }

    /// Encodes a server message using an explicit payload cap.
    fn frame_server_message_with_max(
        msg: &ServerMessage,
        max_frame_size: usize,
    ) -> Result<Vec<u8>, protocol::FramingError> {
        let mut framed = Vec::new();
        protocol::write_message(&mut framed, msg)?;
        let payload_len = framed.len().saturating_sub(4);
        if payload_len > max_frame_size {
            return Err(protocol::FramingError::Oversized {
                claimed: payload_len,
                max: max_frame_size,
            });
        }
        Ok(framed)
    }

    /// Sends a message to all connected clients.
    /// Broken connections are tracked and cleaned up.
    fn send_to_all_clients(&mut self, msg: ServerMessage) {
        let serialized = match Self::frame_server_message(&msg) {
            Ok(framed) => framed,
            Err(err) => {
                warn!(err = %err, "failed to serialize message for clients");
                return;
            }
        };

        let mut broken_clients: Vec<u64> = Vec::new();
        for (&client_id, client) in &mut self.clients {
            if let Some(writer) = &client.writer {
                if writer.control.send(serialized.clone()).is_err() {
                    debug!(client_id, "client writer channel closed during broadcast");
                    broken_clients.push(client_id);
                }
            }
        }

        // Remove broken clients.
        for client_id in broken_clients {
            self.remove_client_and_resize_if_needed(client_id);
        }
    }

    fn terminal_attach_client_ids(&self, pane_id: crate::layout::PaneId) -> Vec<u64> {
        let Some(terminal_id) = self
            .app
            .state
            .workspaces
            .iter()
            .find_map(|ws| ws.terminal_id(pane_id))
        else {
            return Vec::new();
        };
        let terminal_id = terminal_id.to_string();
        self.clients
            .iter()
            .filter(|(_, client)| {
                matches!(&client.mode, ClientConnectionMode::TerminalAttach { terminal_id: attached } if *attached == terminal_id)
            })
            .map(|(&client_id, _)| client_id)
            .collect()
    }

    /// Sends a message to a specific client. Returns false if the client
    /// was not found or the send failed (client removed).
    fn send_to_client(&mut self, client_id: u64, msg: ServerMessage) -> bool {
        let serialized = match Self::frame_server_message(&msg) {
            Ok(framed) => framed,
            Err(err) => {
                warn!(client_id, err = %err, "failed to serialize message for client");
                return false;
            }
        };

        if let Some(client) = self.clients.get(&client_id) {
            if let Some(writer) = &client.writer {
                if writer.control.send(serialized).is_err() {
                    debug!(
                        client_id,
                        "client writer channel closed during targeted send"
                    );
                    self.remove_client_and_resize_if_needed(client_id);
                    return false;
                }
            }
            true
        } else {
            false
        }
    }

    fn shutdown_terminal_stream_clients(&mut self, terminal_id: &str, reason: String) {
        let client_ids = terminal_stream_client_ids(&self.clients, terminal_id);

        for client_id in client_ids {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(reason.clone()),
                },
            );
            self.remove_client_and_resize_if_needed(client_id);
        }
    }

    fn send_terminal_stream_detach_shutdown(&mut self, client_id: u64) {
        if matches!(
            self.clients.get(&client_id).map(|client| &client.mode),
            Some(ClientConnectionMode::TerminalAttach { .. })
        ) {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some("detached".to_owned()),
                },
            );
        }
    }

    fn terminal_id_by_string(&self, terminal_id: &str) -> Option<crate::terminal::TerminalId> {
        self.app
            .state
            .terminals
            .keys()
            .find(|id| id.to_string() == terminal_id)
            .cloned()
    }

    fn runtime_for_terminal_id_string(
        &self,
        terminal_id: &str,
    ) -> Option<&crate::terminal::TerminalRuntime> {
        let terminal_id = self.terminal_id_by_string(terminal_id)?;
        self.app.terminal_runtimes.get(&terminal_id)
    }

    fn agent_read_not_idle_error(
        &self,
        request: &api::schema::Request,
    ) -> Option<api::schema::ErrorBody> {
        use api::schema::{Method, ReadFormat, ReadSource};

        let Method::AgentRead(params) = &request.method else {
            return None;
        };
        let requested = params.lines?;
        if params.format != ReadFormat::Text
            || !matches!(
                params.source,
                ReadSource::Recent | ReadSource::RecentUnwrapped
            )
        {
            return None;
        }
        let target = self.app.resolve_agent_target(&params.target).ok()?;
        let terminal = self
            .app
            .state
            .terminals
            .values()
            .find(|terminal| terminal.id.as_str() == target.terminal_id)?;
        if terminal.effective_known_agent().is_none()
            || terminal.state == crate::detect::AgentState::Idle
        {
            return None;
        }
        let runtime = self.app.terminal_runtimes.get(&terminal.id)?;
        let (screen, snapshot) = runtime.screen_text_snapshot()?;
        if screen != crate::ghostty::ActiveScreen::Alternate
            || snapshot.rows.len() >= requested.min(1000) as usize
        {
            return None;
        }
        let status = crate::detect::manifest::agent_state_label(terminal.state);
        Some(api::schema::ErrorBody {
            code: "agent_not_idle".into(),
            message: format!(
                "cannot read {requested} lines while {} is {status}: its alternate-screen history can only be captured by scrolling while idle. Wait and retry, or use --source visible",
                params.target
            ),
        })
    }

    fn alt_screen_read_spec(&self, request: &api::schema::Request) -> Option<AltScreenReadSpec> {
        use api::schema::{Method, ReadFormat, ReadIntent, ReadSource};

        let (target, source, lines, format) = match &request.method {
            Method::AgentRead(params) => (
                self.app.resolve_agent_target(&params.target).ok()?,
                params.source,
                params.lines,
                params.format,
            ),
            Method::PaneRead(params) if params.intent == ReadIntent::Interactive => (
                self.app.resolve_terminal_target(&params.pane_id).ok()?,
                params.source,
                params.lines,
                params.format,
            ),
            _ => return None,
        };
        if format != ReadFormat::Text
            || !matches!(source, ReadSource::Recent | ReadSource::RecentUnwrapped)
        {
            return None;
        }
        let lines = lines.unwrap_or(80).min(1000) as usize;
        if lines == 0
            || self
                .terminal_attach_owners
                .contains_key(target.terminal_id.as_str())
            || self
                .pending_alt_screen_reads
                .iter()
                .any(|pending| pending.terminal_id.as_str() == target.terminal_id)
        {
            return None;
        }
        let terminal = self
            .app
            .state
            .terminals
            .values()
            .find(|terminal| terminal.id.as_str() == target.terminal_id)?;
        if terminal.effective_known_agent().is_none()
            || terminal.state != crate::detect::AgentState::Idle
        {
            return None;
        }
        let runtime = self.app.terminal_runtimes.get(&terminal.id)?;
        if runtime.wheel_routing() != Some(crate::pane::WheelRouting::MouseReport) {
            return None;
        }
        let (screen, initial, content_seq) = runtime.screen_text_snapshot_with_seq()?;
        if screen != crate::ghostty::ActiveScreen::Alternate || initial.rows.len() >= lines {
            return None;
        }
        Some(AltScreenReadSpec {
            terminal_id: terminal.id.clone(),
            lines,
            unwrap: source == ReadSource::RecentUnwrapped,
            initial,
            content_seq,
        })
    }

    fn poll_pending_alt_screen_reads(&mut self, now: Instant) {
        let pending = std::mem::take(&mut self.pending_alt_screen_reads);
        for read in pending {
            let runtime = self.app.terminal_runtimes.get(&read.terminal_id);
            let remains_idle = self
                .app
                .state
                .terminals
                .get(&read.terminal_id)
                .is_some_and(|terminal| terminal.state == crate::detect::AgentState::Idle);
            let attached = self
                .terminal_attach_owners
                .contains_key(read.terminal_id.as_str());
            let outcome = if remains_idle && !attached {
                read.poll(runtime, now)
            } else {
                read.abort(runtime, now)
            };
            if let Some(read) = outcome {
                self.pending_alt_screen_reads.push(read);
            }
        }
    }

    fn alt_screen_read_conflict(&self, request: &api::schema::Request) -> AltScreenReadConflict {
        let (target, source, lines, format) = match &request.method {
            api::schema::Method::AgentRead(params) => (
                self.app.resolve_agent_target(&params.target).ok(),
                params.source,
                params.lines,
                params.format,
            ),
            api::schema::Method::PaneRead(params) => (
                self.app.resolve_terminal_target(&params.pane_id).ok(),
                params.source,
                params.lines,
                params.format,
            ),
            _ => return AltScreenReadConflict::None,
        };
        let Some(target) = target else {
            return AltScreenReadConflict::None;
        };
        let Some(pending) = self
            .pending_alt_screen_reads
            .iter()
            .find(|pending| pending.terminal_id.as_str() == target.terminal_id)
        else {
            return AltScreenReadConflict::None;
        };
        if format == api::schema::ReadFormat::Text {
            AltScreenReadConflict::Frozen(pending.frozen_snapshot(source, lines))
        } else {
            AltScreenReadConflict::Defer
        }
    }

    fn process_deferred_alt_screen_reads(&mut self) -> bool {
        let deferred = std::mem::take(&mut self.deferred_alt_screen_reads);
        let mut changed = false;
        for msg in deferred {
            match self.alt_screen_read_conflict(&msg.request) {
                AltScreenReadConflict::None => {
                    changed |= self.handle_api_request(msg);
                }
                AltScreenReadConflict::Frozen(_) | AltScreenReadConflict::Defer => {
                    self.deferred_alt_screen_reads.push(msg);
                }
            }
        }
        changed
    }

    fn reject_queued_api_requests_for_shutdown(&mut self) {
        for _ in 0..self.app.api_rx.len() {
            let Ok(msg) = self.app.api_rx.try_recv() else {
                break;
            };
            self.handle_api_request(msg);
        }
    }

    async fn reject_late_client_connections(&mut self) {
        self.server_event_rx.close();
        while let Some(event) = self.server_event_rx.recv().await {
            if let ServerEvent::ClientConnected { writer, .. } = event {
                if let Ok(message) = Self::frame_server_message(&ServerMessage::ServerShutdown {
                    reason: Some("server is shutting down".to_owned()),
                }) {
                    let _ = writer.control.send(message);
                }
            }
        }
    }

    fn allocate_activity_stamp(&mut self) -> u64 {
        let stamp = self.next_activity_stamp;
        self.next_activity_stamp = self.next_activity_stamp.saturating_add(1);
        stamp
    }

    fn client_is_pending_terminal_mode(&self, client_id: u64) -> bool {
        self.clients
            .get(&client_id)
            .is_some_and(|client| matches!(client.mode, ClientConnectionMode::TerminalPending))
    }

    fn attach_terminal_client(
        &mut self,
        client_id: u64,
        terminal_id: String,
        takeover: bool,
    ) -> bool {
        if !self.client_is_pending_terminal_mode(client_id) {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(
                        "terminal attach failed: connection is not pending terminal attach"
                            .to_owned(),
                    ),
                },
            );
            self.remove_client_and_resize_if_needed(client_id);
            return false;
        }

        let Some(real_terminal_id) = self.terminal_id_by_string(&terminal_id) else {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(format!(
                        "terminal attach failed: terminal {terminal_id} not found"
                    )),
                },
            );
            self.remove_client_and_resize_if_needed(client_id);
            return false;
        };

        if self
            .pending_alt_screen_reads
            .iter()
            .any(|pending| pending.terminal_id == real_terminal_id)
        {
            self.send_to_client(
                client_id,
                ServerMessage::ServerShutdown {
                    reason: Some(format!(
                        "terminal attach failed: terminal {terminal_id} has a read in progress; retry"
                    )),
                },
            );
            self.remove_client_and_resize_if_needed(client_id);
            return false;
        }

        if let Some(existing_owner) = self.terminal_attach_owners.get(&terminal_id).copied() {
            if existing_owner != client_id && !takeover {
                self.send_to_client(
                    client_id,
                    ServerMessage::ServerShutdown {
                        reason: Some(format!(
                            "terminal attach failed: terminal {terminal_id} already has an attached client; retry with --takeover"
                        )),
                    },
                );
                self.remove_client_and_resize_if_needed(client_id);
                return false;
            }
            if existing_owner != client_id {
                self.send_to_client(
                    existing_owner,
                    ServerMessage::ServerShutdown {
                        reason: Some("terminal attach taken over".to_owned()),
                    },
                );
                self.remove_client_and_resize_if_needed(existing_owner);
            }
        }

        let stamp = self.allocate_activity_stamp();
        let Some(client) = self.clients.get_mut(&client_id) else {
            return false;
        };
        let (cols, rows) = client.terminal_size;
        let cell_size = client.cell_size;
        client.mode = ClientConnectionMode::TerminalAttach {
            terminal_id: terminal_id.clone(),
        };
        client.render_state.reset_baseline();
        client.last_activity = stamp;
        info!(client_id, cols, rows, terminal_id = %terminal_id, "terminal attach client connected");
        self.terminal_attach_owners
            .insert(terminal_id.clone(), client_id);
        self.app
            .state
            .direct_attach_resize_locks
            .insert(real_terminal_id.clone());
        self.app
            .start_pending_agent_resume_for_terminal(&real_terminal_id, rows, cols, true);
        if let Some(runtime) = self.app.terminal_runtimes.get(&real_terminal_id) {
            runtime.resize(rows, cols, cell_size.width_px, cell_size.height_px);
        }
        true
    }
}

impl Drop for HeadlessServer {
    fn drop(&mut self) {
        let _ = self.cleanup_sockets();
    }
}

/// Sets the quit flag on Ctrl+C or SIGTERM and wakes the loop to notice it.
fn ctrlc_handler(should_quit: Arc<AtomicBool>, server_event_tx: mpsc::Sender<ServerEvent>) {
    let _ = ctrlc::set_handler(move || {
        should_quit.store(true, Ordering::Release);
        let _ = server_event_tx.try_send(ServerEvent::QuitSignal);
    });
}
