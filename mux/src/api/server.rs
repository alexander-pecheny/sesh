use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::{ListenerExt as _, Stream as _};
use tracing::{debug, error, info, warn};

#[cfg(test)]
use std::fs;

use crate::api::schema::{
    ErrorBody, ErrorResponse, Method, Request, ResponseResult, ServerCapabilities, SuccessResponse,
};
use crate::api::subscriptions::ActiveSubscription;
use crate::api::wait::{prompt_agent, wait_for_agent, wait_for_event, wait_for_output};
use crate::api::{socket_path, ApiRequestMessage, ApiRequestSender, EventHub};
use crate::ipc::{
    bind_local_listener, is_connection_closed_error, local_stream_peer_closed,
    poll_local_stream_read, remove_socket_file_if_owned, set_local_stream_polling,
    socket_file_identity, LocalStream, LocalStreamRead, SocketFileIdentity,
};

#[cfg(test)]
mod subscription_socket_tests;

const SOCKET_PERMISSION_MODE: u32 = 0o600;
pub(super) const CONNECTION_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const APP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const INITIAL_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_INITIAL_REQUEST_BYTES: usize = 1024 * 1024;

pub struct ServerHandle {
    _thread: std::thread::JoinHandle<()>,
    path: PathBuf,
    identity: SocketFileIdentity,
    running: Arc<AtomicBool>,
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);

        if let Err(err) = self.remove_socket_file_if_owned() {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %self.path.display(), err = %err, "failed to remove api socket on shutdown");
            }
        }
    }
}

impl ServerHandle {
    pub(crate) fn remove_socket_file_if_owned(&self) -> std::io::Result<()> {
        remove_socket_file_if_owned(&self.path, &self.identity)
    }
}

pub(crate) fn start_server_with_stop_control(
    api_tx: ApiRequestSender,
    event_hub: EventHub,
    server_stop: Arc<AtomicBool>,
) -> std::io::Result<ServerHandle> {
    start_server_inner(
        socket_path(),
        false,
        api_tx,
        event_hub,
        default_capabilities(),
        Some(server_stop),
    )
}

/// Serves the API on the socket of the server Sesh ran before sesh-mux, for agents that outlived
/// the handoff and still report there with that server's hook sources.
pub(crate) fn start_legacy_server(
    path: PathBuf,
    api_tx: ApiRequestSender,
    event_hub: EventHub,
) -> std::io::Result<ServerHandle> {
    start_server_inner(path, true, api_tx, event_hub, default_capabilities(), None)
}

fn canonicalize_legacy_sources(method: &mut Method) {
    let canonical = |source: &mut String| {
        *source = crate::agent_resume::canonical_source(source).into_owned();
    };
    match method {
        Method::PaneReportAgent(params) => canonical(&mut params.source),
        Method::PaneReportAgentSession(params) => canonical(&mut params.source),
        Method::PaneReportPermission(params) => canonical(&mut params.source),
        Method::PaneReportMetadata(params) => {
            canonical(&mut params.source);
            if let Some(source) = params.applies_to_source.as_mut() {
                canonical(source);
            }
        }
        Method::PaneClearAgentAuthority(params) => {
            if let Some(source) = params.source.as_mut() {
                canonical(source);
            }
        }
        Method::PaneReleaseAgent(params) => canonical(&mut params.source),
        _ => {}
    }
}

fn default_capabilities() -> Option<ServerCapabilities> {
    Some(ServerCapabilities {
        live_handoff: crate::platform::capabilities().live_handoff,
        detached_server_daemon: crate::platform::current_process_is_detached_server_daemon(),
        health_check: true,
    })
}

fn start_server_inner(
    path: PathBuf,
    legacy: bool,
    api_tx: ApiRequestSender,
    event_hub: EventHub,
    capabilities: Option<ServerCapabilities>,
    server_stop: Option<Arc<AtomicBool>>,
) -> std::io::Result<ServerHandle> {
    prepare_socket_path(&path)?;

    let listener = bind_local_listener(&path)?;
    restrict_socket_permissions(&path)?;
    let identity = socket_file_identity(&path)?;
    info!(path = %path.display(), "api server listening");

    let running = Arc::new(AtomicBool::new(true));
    let listener_running = Arc::clone(&running);
    let thread = std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let api_tx = api_tx.clone();
                    let event_hub = event_hub.clone();
                    let capabilities = capabilities.clone();
                    let server_stop = server_stop.clone();
                    let connection_running = Arc::clone(&listener_running);
                    std::thread::spawn(move || {
                        if let Err(err) = handle_connection_with_stop(
                            stream,
                            &api_tx,
                            &event_hub,
                            &connection_running,
                            capabilities,
                            server_stop.as_ref(),
                            legacy,
                        ) {
                            warn!(err = %err, "api connection failed");
                        }
                    });
                }
                Err(err) => {
                    error!(err = %err, "api listener accept failed");
                    break;
                }
            }
        }
        debug!("api server thread exiting");
    });

    Ok(ServerHandle {
        _thread: thread,
        path,
        identity,
        running,
    })
}

fn prepare_socket_path(path: &Path) -> std::io::Result<()> {
    crate::ipc::prepare_socket_path(path, |path| {
        format!(
            "sesh-mux is already running (socket busy at {})",
            path.display()
        )
    })
}

fn restrict_socket_permissions(path: &Path) -> std::io::Result<()> {
    crate::ipc::restrict_socket_permissions(path, SOCKET_PERMISSION_MODE)
}

#[cfg(test)]
fn handle_connection(
    stream: LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
    capabilities: Option<ServerCapabilities>,
) -> std::io::Result<()> {
    handle_connection_with_stop(
        stream,
        api_tx,
        event_hub,
        running,
        capabilities,
        None,
        false,
    )
}

fn handle_connection_with_stop(
    mut stream: LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
    capabilities: Option<ServerCapabilities>,
    server_stop: Option<&Arc<AtomicBool>>,
    legacy: bool,
) -> std::io::Result<()> {
    if let Err(err) = stream.set_send_timeout(Some(STREAM_WRITE_TIMEOUT)) {
        debug!(err = %err, "api connection write timeout unavailable");
    }

    let Some(line) = read_initial_request_line(&mut stream)? else {
        return Ok(());
    };

    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }

    let mut request = match serde_json::from_str::<Request>(line) {
        Ok(request) => request,
        Err(request_error) => {
            // Recover correlation without relaxing typed request validation or accepting
            // ambiguous duplicate IDs. Invalid JSON and non-string IDs stay uncorrelated.
            #[derive(serde::Deserialize)]
            struct RequestId {
                id: String,
            }
            let id = if line.starts_with('{') {
                serde_json::from_str::<RequestId>(line)
                    .map(|request| request.id)
                    .unwrap_or_default()
            } else {
                String::new()
            };
            write_json_line_allow_disconnect(
                &mut stream,
                &ErrorResponse {
                    id,
                    error: ErrorBody {
                        code: "invalid_request".into(),
                        message: format!("invalid request: {request_error}"),
                    },
                },
            )?;
            return Ok(());
        }
    };

    if legacy {
        canonicalize_legacy_sources(&mut request.method);
    }
    let request_id = request.id.clone();
    let method = api_method_name(&request.method);
    let changes_ui = false;
    crate::logging::api_request_started(&request_id, method, changes_ui);

    match request.method {
        Method::EventsSubscribe(params) => {
            let result = stream_subscriptions(
                stream,
                request_id.clone(),
                params,
                api_tx,
                event_hub,
                running,
            );
            match &result {
                Ok(()) => crate::logging::api_request_completed(
                    &request_id,
                    method,
                    "stream_closed",
                    changes_ui,
                ),
                Err(err) => {
                    crate::logging::api_request_failed(&request_id, method, &err.to_string())
                }
            }
            result
        }
        Method::EventsWait(params) => {
            let response = wait_for_event(
                request_id.clone(),
                params,
                &mut stream,
                api_tx,
                event_hub,
                running,
            )?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        Method::AgentPrompt(params) => {
            let response = prompt_agent(
                request_id.clone(),
                params,
                &mut stream,
                api_tx,
                event_hub,
                running,
            )?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        Method::AgentWait(params) => {
            let response = wait_for_agent(
                request_id.clone(),
                params,
                &mut stream,
                api_tx,
                event_hub,
                running,
            )?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        Method::PaneWaitForOutput(params) => {
            let response =
                wait_for_output(request_id.clone(), params, &mut stream, api_tx, running)?;
            finish_wait_response(&mut stream, response, &request_id, method, changes_ui)
        }
        method_body => {
            let (response_write_tx, response_write_rx) = std::sync::mpsc::channel();
            let response = handle_request(
                Request {
                    id: request_id.clone(),
                    method: method_body,
                },
                api_tx,
                capabilities,
                server_stop,
                Some(response_write_rx),
            );
            let result = write_text_line_allow_disconnect(&mut stream, &response);
            let _ = response_write_tx.send(());
            match &result {
                Ok(()) => crate::logging::api_request_completed(
                    &request_id,
                    method,
                    api_response_outcome(&response),
                    changes_ui,
                ),
                Err(err) => {
                    crate::logging::api_request_failed(&request_id, method, &err.to_string())
                }
            }
            result
        }
    }
}

fn finish_wait_response(
    stream: &mut LocalStream,
    response: Option<String>,
    request_id: &str,
    method: &'static str,
    changes_ui: bool,
) -> std::io::Result<()> {
    let Some(response) = response else {
        crate::logging::api_request_completed(
            request_id,
            method,
            "client_disconnected",
            changes_ui,
        );
        return Ok(());
    };
    let result = write_text_line_allow_disconnect(stream, &response);
    match &result {
        Ok(()) => crate::logging::api_request_completed(
            request_id,
            method,
            api_response_outcome(&response),
            changes_ui,
        ),
        Err(err) => crate::logging::api_request_failed(request_id, method, &err.to_string()),
    }
    result
}

fn handle_request(
    request: Request,
    api_tx: &ApiRequestSender,
    capabilities: Option<ServerCapabilities>,
    server_stop: Option<&Arc<AtomicBool>>,
    response_write_complete: Option<std::sync::mpsc::Receiver<()>>,
) -> String {
    if matches!(&request.method, Method::Ping(_)) {
        return serde_json::to_string(&SuccessResponse {
            id: request.id,
            result: ResponseResult::Pong {
                version: crate::build_info::version(),
                protocol: crate::protocol::PROTOCOL_VERSION,
                capabilities,
            },
        })
        .unwrap_or_else(|_| {
            r#"{"id":"","error":{"code":"internal_error","message":"failed to encode response"}}"#
                .to_string()
        });
    }

    if matches!(&request.method, Method::ServerStop(_)) {
        if let Some(server_stop) = server_stop {
            server_stop.store(true, Ordering::Release);
            return serde_json::to_string(&SuccessResponse {
                id: request.id,
                result: ResponseResult::Ok {},
            })
            .unwrap_or_else(|_| "{}".to_string());
        }
    } else if server_stop.is_some_and(|stop| stop.load(Ordering::Acquire)) {
        return error_response_json(
            request.id,
            "server_unavailable",
            "server is shutting down".into(),
        );
    }

    dispatch_to_app(request, api_tx, None, response_write_complete, None)
}

pub(crate) fn api_method_name(method: &Method) -> &'static str {
    match method {
        Method::Ping(_) => "ping",
        Method::ServerStop(_) => "server.stop",
        Method::ServerLiveHandoff(_) => "server.live_handoff",
        Method::WorkspaceCreate(_) => "workspace.create",
        Method::WorkspaceList(_) => "workspace.list",
        Method::WorkspaceGet(_) => "workspace.get",
        Method::WorkspaceRename(_) => "workspace.rename",
        Method::WorkspaceReportMetadata(_) => "workspace.report_metadata",
        Method::WorkspaceClose(_) => "workspace.close",
        Method::WorktreeList(_) => "worktree.list",
        Method::WorktreeCreate(_) => "worktree.create",
        Method::WorktreeOpen(_) => "worktree.open",
        Method::WorktreeRemove(_) => "worktree.remove",
        Method::TabCreate(_) => "tab.create",
        Method::TabList(_) => "tab.list",
        Method::TabGet(_) => "tab.get",
        Method::TabRename(_) => "tab.rename",
        Method::TabClose(_) => "tab.close",
        Method::AgentList(_) => "agent.list",
        Method::AgentGet(_) => "agent.get",
        Method::AgentRead(_) => "agent.read",
        Method::AgentSendKeys(_) => "agent.send_keys",
        Method::AgentRename(_) => "agent.rename",
        Method::AgentStart(_) => "agent.start",
        Method::AgentPrompt(_) => "agent.prompt",
        Method::AgentWait(_) => "agent.wait",
        Method::PaneProcessInfo(_) => "pane.process_info",
        Method::PaneList(_) => "pane.list",
        Method::PaneCurrent(_) => "pane.current",
        Method::PaneGet(_) => "pane.get",
        Method::PaneRename(_) => "pane.rename",
        Method::PaneSendText(_) => "pane.send_text",
        Method::PaneSendKeys(_) => "pane.send_keys",
        Method::PaneSendInput(_) => "pane.send_input",
        Method::PaneRead(_) => "pane.read",
        Method::PaneReportAgent(_) => "pane.report_agent",
        Method::PaneReportAgentSession(_) => "pane.report_agent_session",
        Method::PaneReportPermission(_) => "pane.report_permission",
        Method::PaneReportMetadata(_) => "pane.report_metadata",
        Method::PaneClearAgentAuthority(_) => "pane.clear_agent_authority",
        Method::PaneReleaseAgent(_) => "pane.release_agent",
        Method::PaneClose(_) => "pane.close",
        Method::EventsSubscribe(_) => "events.subscribe",
        Method::EventsWait(_) => "events.wait",
        Method::PaneWaitForOutput(_) => "pane.wait_for_output",
        Method::IntegrationList(_) => "integration.list",
        Method::IntegrationInstall(_) => "integration.install",
        Method::IntegrationUninstall(_) => "integration.uninstall",
    }
}

fn api_response_outcome(response: &str) -> &'static str {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(response) else {
        return "error";
    };

    match value
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(|code| code.as_str())
    {
        Some("timeout") => "timeout",
        Some(_) => "error",
        None => "ok",
    }
}

fn read_initial_request_line(stream: &mut LocalStream) -> std::io::Result<Option<String>> {
    read_initial_request_line_with_timeout(stream, INITIAL_REQUEST_TIMEOUT)
}

fn read_initial_request_line_with_timeout(
    stream: &mut LocalStream,
    timeout: Duration,
) -> std::io::Result<Option<String>> {
    read_initial_request_line_with_limits(stream, timeout, MAX_INITIAL_REQUEST_BYTES)
}

fn read_initial_request_line_with_limits(
    stream: &mut LocalStream,
    timeout: Duration,
    max_bytes: usize,
) -> std::io::Result<Option<String>> {
    set_local_stream_polling(stream, true)?;
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut byte = [0u8; 1];

    let result = loop {
        let read = match poll_local_stream_read(stream, &mut byte) {
            Ok(read) => read,
            Err(err) => break Err(err),
        };
        match read {
            LocalStreamRead::Closed => break Ok(None),
            LocalStreamRead::Data => {
                bytes.push(byte[0]);
                if byte[0] == b'\n' {
                    break String::from_utf8(bytes)
                        .map(Some)
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err));
                }
                if bytes.len() > max_bytes {
                    break Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "api request line is too large",
                    ));
                }
            }
            LocalStreamRead::Pending => {
                if Instant::now() >= deadline {
                    break Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "timed out reading api request",
                    ));
                }
                std::thread::sleep(CONNECTION_POLL_INTERVAL);
            }
        }
    };
    set_local_stream_polling(stream, false)?;
    result
}

fn stream_subscriptions(
    mut stream: LocalStream,
    request_id: String,
    params: crate::api::schema::EventsSubscribeParams,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<()> {
    let event_start_sequence = event_hub.current_sequence();
    let mut subscriptions = Vec::with_capacity(params.subscriptions.len());
    for (index, subscription) in params.subscriptions.into_iter().enumerate() {
        let active = match ActiveSubscription::new(
            subscription,
            &request_id,
            index,
            api_tx,
            event_hub,
            event_start_sequence,
        ) {
            Ok(active) => active,
            Err(mut response) => {
                response.id = request_id;
                if let Err(err) = write_json_line(&mut stream, &response) {
                    if is_connection_closed_error(&err) {
                        return Ok(());
                    }
                    return Err(err);
                }
                return Ok(());
            }
        };
        subscriptions.push(active);
    }

    if let Err(err) = write_json_line(
        &mut stream,
        &SuccessResponse {
            id: request_id.clone(),
            result: ResponseResult::SubscriptionStarted {},
        },
    ) {
        if is_connection_closed_error(&err) {
            return Ok(());
        }
        return Err(err);
    }

    loop {
        if should_stop_connection(&mut stream, running)? {
            return Ok(());
        }

        for subscription in &mut subscriptions {
            let events = match subscription.poll_batch(api_tx, event_hub) {
                Ok(events) => events,
                Err(error) => {
                    write_json_line_allow_disconnect(
                        &mut stream,
                        &ErrorResponse {
                            id: request_id,
                            error,
                        },
                    )?;
                    return Ok(());
                }
            };
            for event in events {
                if should_stop_connection(&mut stream, running)? {
                    return Ok(());
                }
                if let Err(err) = write_json_line(&mut stream, &event) {
                    if is_connection_closed_error(&err) {
                        return Ok(());
                    }
                    return Err(err);
                }
            }
        }
        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

fn write_text_line(stream: &mut LocalStream, value: &str) -> std::io::Result<()> {
    stream.write_all(value.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn write_text_line_allow_disconnect(stream: &mut LocalStream, value: &str) -> std::io::Result<()> {
    match write_text_line(stream, value) {
        Err(err) if is_connection_closed_error(&err) => Ok(()),
        result => result,
    }
}

fn write_json_line<T: serde::Serialize>(
    stream: &mut LocalStream,
    value: &T,
) -> std::io::Result<()> {
    let encoded = serde_json::to_string(value)
        .map_err(|err| std::io::Error::other(format!("failed to encode json: {err}")))?;
    write_text_line(stream, &encoded)
}

fn write_json_line_allow_disconnect<T: serde::Serialize>(
    stream: &mut LocalStream,
    value: &T,
) -> std::io::Result<()> {
    let encoded = serde_json::to_string(value)
        .map_err(|err| std::io::Error::other(format!("failed to encode json: {err}")))?;
    write_text_line_allow_disconnect(stream, &encoded)
}

pub(super) fn should_stop_connection(
    stream: &mut LocalStream,
    running: &Arc<AtomicBool>,
) -> std::io::Result<bool> {
    if !running.load(Ordering::Relaxed) {
        return Ok(true);
    }

    local_stream_peer_closed(stream)
}

pub(super) fn dispatch_to_app_with_timeout(
    request: Request,
    api_tx: &ApiRequestSender,
    timeout: Option<Duration>,
) -> String {
    dispatch_to_app(request, api_tx, timeout, None, None)
}

pub(super) fn dispatch_to_app_with_caller_timeout(
    request: Request,
    api_tx: &ApiRequestSender,
    timeout: Option<Duration>,
) -> String {
    dispatch_to_app(
        request,
        api_tx,
        timeout,
        None,
        Some(("timeout", "timed out waiting for agent status")),
    )
}

fn dispatch_to_app(
    request: Request,
    api_tx: &ApiRequestSender,
    timeout: Option<Duration>,
    response_write_complete: Option<std::sync::mpsc::Receiver<()>>,
    timeout_response: Option<(&str, &str)>,
) -> String {
    let request_id = request.id.clone();
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    if let Err(err) = api_tx.send(ApiRequestMessage {
        request,
        respond_to,
        response_write_complete,
    }) {
        return error_response_json(
            request_id,
            "server_unavailable",
            format!("failed to dispatch request: {err}"),
        );
    }

    let response = match timeout {
        Some(timeout) => response_rx.recv_timeout(timeout).map_err(|err| match err {
            std::sync::mpsc::RecvTimeoutError::Timeout => std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "timed out waiting for app response after {} ms",
                    timeout.as_millis()
                ),
            ),
            std::sync::mpsc::RecvTimeoutError::Disconnected => std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "app response channel closed",
            ),
        }),
        None => response_rx
            .recv()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::BrokenPipe, err)),
    };

    match response {
        Ok(response) => response,
        Err(err) => {
            if err.kind() == std::io::ErrorKind::TimedOut {
                if let Some((code, message)) = timeout_response {
                    return error_response_json(request_id, code, message.into());
                }
            }
            error_response_json(
                request_id,
                "server_unavailable",
                format!("request handling failed: {err}"),
            )
        }
    }
}

#[cfg(test)]
#[test]
fn caller_timeout_dispatch_uses_timeout_error() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let response = dispatch_to_app_with_caller_timeout(
        Request {
            id: "prompt-timeout".into(),
            method: Method::AgentPrompt(crate::api::schema::AgentPromptParams {
                target: "reviewer".into(),
                text: "review this".into(),
                wait: None,
            }),
        },
        &tx,
        Some(Duration::ZERO),
    );
    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "timeout");
}

fn error_response_json(id: String, code: &str, message: String) -> String {
    serde_json::to_string(&ErrorResponse {
        id,
        error: ErrorBody {
            code: code.into(),
            message,
        },
    })
    .unwrap_or_else(|_| {
        r#"{"id":"","error":{"code":"internal_error","message":"failed to encode error response"}}"#
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use interprocess::local_socket::traits::Listener as _;
    use std::io::{BufRead, BufReader, Read};
    use tokio::sync::mpsc;

    fn unique_test_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sm-{name}-{}-{nanos}", std::process::id()))
    }

    fn read_line(stream: &mut LocalStream) -> String {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        line
    }

    fn local_stream_pair(name: &str) -> (LocalStream, LocalStream, PathBuf) {
        let path = unique_test_path(name);
        let listener = crate::ipc::bind_local_listener(&path).unwrap();
        let client = crate::ipc::connect_local_stream(&path).unwrap();
        let server = listener.accept().unwrap();
        (client, server, path)
    }

    #[test]
    fn invalid_requests_preserve_only_unambiguous_string_ids() {
        let cases = [
            (
                r#"{"id":"mine","method":"pane.report_agent","params":{"pane_id":"w1:p1","status":"working","source":"x"}}"#,
                "mine",
            ),
            (r#"{"id":"escaped\"id","method":"unknown"}"#, "escaped\"id"),
            (r#"{"method":"unknown","params":{"id":"nested"}}"#, ""),
            (r#"{"id":123,"method":"unknown"}"#, ""),
            (
                r#"{"id":"first","id":"second","method":"ping","params":{}}"#,
                "",
            ),
            (r#"{"id":"truncated","method":"ping""#, ""),
            (r#"["not-an-object"]"#, ""),
        ];
        for (request, expected_id) in cases {
            let (api_tx, mut api_rx) = mpsc::unbounded_channel();
            let (mut client, server, path) = local_stream_pair("invalid-request-id");
            writeln!(client, "{request}").unwrap();
            let running = Arc::new(AtomicBool::new(true));
            handle_connection(server, &api_tx, &EventHub::default(), &running, None).unwrap();

            let mut response = String::new();
            BufReader::new(client)
                .read_to_string(&mut response)
                .unwrap();
            let response: ErrorResponse = serde_json::from_str(&response).unwrap();
            assert_eq!(response.id, expected_id, "{request}");
            assert_eq!(response.error.code, "invalid_request");
            assert!(response.error.message.starts_with("invalid request: "));
            assert!(
                api_rx.try_recv().is_err(),
                "invalid requests must not dispatch"
            );
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn subscription_setup_errors_preserve_request_id_and_reject_entire_stream() {
        let (api_tx, mut api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
        let event_hub = EventHub::default();
        let responder_event_hub = event_hub.clone();
        let responder = std::thread::spawn(move || {
            let msg = api_rx.blocking_recv().unwrap();
            let Method::PaneGet(params) = msg.request.method else {
                panic!("unexpected request: {:?}", msg.request.method);
            };
            assert_eq!(params.pane_id, "w999:p9");
            responder_event_hub.push(crate::api::schema::EventEnvelope {
                event: crate::api::schema::EventKind::PaneClosed,
                data: crate::api::schema::EventData::PaneClosed {
                    pane_id: "w999:p9".into(),
                    workspace_id: "w999".into(),
                },
            });
            msg.respond_to
                .send(error_response_json(
                    msg.request.id,
                    "pane_not_found",
                    "pane w999:p9 not found".into(),
                ))
                .unwrap();
            assert!(
                api_rx.blocking_recv().is_none(),
                "rejection must not start polling"
            );
        });
        let (mut client, server, path) = local_stream_pair("subscription-error-id");
        let request = r#"{"id":"panefold:events","method":"events.subscribe","params":{"subscriptions":[{"type":"workspace.created"},{"type":"pane.closed"},{"type":"pane.agent_status_changed","pane_id":"w999:p9"}]}}"#;
        writeln!(client, "{request}").unwrap();
        let running = Arc::new(AtomicBool::new(true));
        handle_connection(server, &api_tx, &event_hub, &running, None).unwrap();
        drop(api_tx);
        responder.join().unwrap();

        let mut response = String::new();
        BufReader::new(client)
            .read_to_string(&mut response)
            .unwrap();
        let response: ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(response.id, "panefold:events");
        assert_eq!(response.error.code, "pane_not_found");
        assert_eq!(response.error.message, "pane w999:p9 not found");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn subscriptions_stop_when_client_disconnects() {
        let (api_tx, _api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
        let (mut client, server, _path) = local_stream_pair("api-sub-disconnect");
        client
            .write_all(
                br#"{"id":"sub_1","method":"events.subscribe","params":{"subscriptions":[{"type":"workspace.created"}]}}"#,
            )
            .unwrap();
        client.write_all(b"\n").unwrap();
        client.flush().unwrap();

        let running = Arc::new(AtomicBool::new(true));
        let server_running = Arc::clone(&running);
        let event_hub = EventHub::default();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let result = handle_connection(server, &api_tx, &event_hub, &server_running, None);
            done_tx.send(result).unwrap();
        });

        let ack = read_line(&mut client);
        let ack: serde_json::Value = serde_json::from_str(&ack).unwrap();
        assert_eq!(ack["result"]["type"], "subscription_started");
        assert_eq!(ack["id"], "sub_1");

        drop(client);

        let result = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(result.is_ok());
        server_thread.join().unwrap();
    }

    #[test]
    fn subscriptions_stop_when_server_shuts_down() {
        let (api_tx, _api_rx) = mpsc::unbounded_channel::<ApiRequestMessage>();
        let (mut client, server, _path) = local_stream_pair("api-sub-shutdown");
        client
            .write_all(
                br#"{"id":"sub_2","method":"events.subscribe","params":{"subscriptions":[{"type":"workspace.created"}]}}"#,
            )
            .unwrap();
        client.write_all(b"\n").unwrap();
        client.flush().unwrap();

        let running = Arc::new(AtomicBool::new(true));
        let server_running = Arc::clone(&running);
        let event_hub = EventHub::default();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let result = handle_connection(server, &api_tx, &event_hub, &server_running, None);
            done_tx.send(result).unwrap();
        });

        let ack = read_line(&mut client);
        let ack: serde_json::Value = serde_json::from_str(&ack).unwrap();
        assert_eq!(ack["result"]["type"], "subscription_started");

        running.store(false, Ordering::Relaxed);

        let result = done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(result.is_ok());
        server_thread.join().unwrap();
    }
}
