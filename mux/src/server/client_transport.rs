//! Blocking client socket transport for the server: handshake, read loop and writer loop,
//! turning socket I/O into [`ServerEvent`] values for `HeadlessServer`.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SendError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use interprocess::local_socket::traits::Stream as _;
use interprocess::TryClone as _;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::ipc::LocalStream;
use crate::protocol::{
    self, AttachScrollDirection, AttachScrollSource, ClientMessage, RenderEncoding, ServerMessage,
    MAX_FRAME_SIZE, MAX_GRAPHICS_FRAME_SIZE, PROTOCOL_VERSION,
};

/// Closes the connection within the client's 5-second handshake deadline despite timer slack.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(4);

/// Maximum input payload size (bytes) for a single `ClientMessage::Input`.
const MAX_INPUT_PAYLOAD: usize = 1024 * 1024;

/// Channels owned by the server side of a client writer thread.
#[derive(Clone, Debug)]
pub(crate) struct ClientWriter {
    /// Reliable control messages such as shutdown, notifications, and clipboard writes.
    pub(crate) control: ClientControlWriter,
    /// Droppable render messages. Capacity is one so slow clients cannot build lag.
    pub(crate) render: ClientRenderWriter,
}

#[derive(Debug)]
pub(crate) struct ClientControlWriter {
    queue: Arc<ClientWriterQueue>,
    #[cfg(test)]
    test_render: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
}

#[derive(Debug)]
pub(crate) struct ClientRenderWriter {
    queue: Arc<ClientWriterQueue>,
    #[cfg(test)]
    test_render: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
}

macro_rules! writer_handle {
    ($type:ty) => {
        impl Clone for $type {
            fn clone(&self) -> Self {
                self.queue.add_sender();
                Self {
                    queue: self.queue.clone(),
                    #[cfg(test)]
                    test_render: self.test_render.clone(),
                }
            }
        }
        impl Drop for $type {
            fn drop(&mut self) {
                self.queue.remove_sender();
            }
        }
    };
}
writer_handle!(ClientControlWriter);
writer_handle!(ClientRenderWriter);

impl ClientControlWriter {
    fn queue(queue: Arc<ClientWriterQueue>) -> Self {
        queue.add_sender();
        Self {
            queue,
            #[cfg(test)]
            test_render: None,
        }
    }

    pub(crate) fn send(&self, data: Vec<u8>) -> Result<(), SendError<Vec<u8>>> {
        self.queue.send_control(data)
    }
}

impl ClientRenderWriter {
    fn queue(queue: Arc<ClientWriterQueue>) -> Self {
        queue.add_sender();
        Self {
            queue,
            #[cfg(test)]
            test_render: None,
        }
    }

    pub(crate) fn try_send(&self, data: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        #[cfg(test)]
        if let Some(sender) = &self.test_render {
            return sender.try_send(data);
        }
        self.queue.try_send_render(data)
    }
}

#[derive(Debug)]
struct ClientWriterQueue {
    state: Mutex<ClientWriterQueueState>,
    ready: Condvar,
}

#[derive(Debug, Default)]
struct ClientWriterQueueState {
    control: VecDeque<Vec<u8>>,
    ordered: VecDeque<Vec<u8>>,
    render: Option<Vec<u8>>,
    senders: usize,
    writer_alive: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum ClientWriteItem {
    Control(Vec<u8>),
    Render(Vec<u8>),
}

impl ClientWriterQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ClientWriterQueueState {
                writer_alive: true,
                ..ClientWriterQueueState::default()
            }),
            ready: Condvar::new(),
        })
    }

    fn add_sender(&self) {
        let mut state = self.lock_state();
        state.senders = state.senders.saturating_add(1);
    }

    fn remove_sender(&self) {
        let mut state = self.lock_state();
        state.senders = state.senders.saturating_sub(1);
        self.ready.notify_one();
    }

    fn send_control(&self, data: Vec<u8>) -> Result<(), SendError<Vec<u8>>> {
        let mut state = self.lock_state();
        if !state.writer_alive {
            return Err(SendError(data));
        }
        state.control.push_back(data);
        self.ready.notify_one();
        Ok(())
    }

    fn try_send_render(&self, data: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        let mut state = self.lock_state();
        if !state.writer_alive {
            return Err(TrySendError::Disconnected(data));
        }
        if state.render.is_some() {
            return Err(TrySendError::Full(data));
        }
        state.render = Some(data);
        self.ready.notify_one();
        Ok(())
    }

    fn recv(&self) -> Option<ClientWriteItem> {
        let mut state = self.lock_state();
        loop {
            if let Some(data) = state.control.pop_front() {
                return Some(ClientWriteItem::Control(data));
            }
            if let Some(data) = state.ordered.pop_front() {
                self.ready.notify_one();
                return Some(ClientWriteItem::Render(data));
            }
            if let Some(data) = state.render.take() {
                return Some(ClientWriteItem::Render(data));
            }
            if state.senders == 0 {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn close_writer(&self) {
        let mut state = self.lock_state();
        state.writer_alive = false;
        state.render = None;
        state.ordered.clear();
        self.ready.notify_all();
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ClientWriterQueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Internal event sent from client transport threads to the main event loop.
#[derive(Debug)]
pub(crate) enum ServerEvent {
    ClientConnected {
        client_id: u64,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
        writer: ClientWriter,
    },
    ClientInput {
        client_id: u64,
        data: Vec<u8>,
    },
    /// A complete bracketed paste exceeded the input limit.
    ClientPasteRejected {
        client_id: u64,
        size: usize,
        max: usize,
    },
    ClientAttachTerminal {
        client_id: u64,
        terminal_id: String,
        takeover: bool,
    },
    ClientAttachScroll {
        client_id: u64,
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    },
    ClientAttachMouse {
        client_id: u64,
        kind: crate::protocol::ClientMouseKind,
        position: crate::protocol::ClientMousePosition,
        geometry: Option<crate::protocol::ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    },
    ClientResize {
        client_id: u64,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    ClientDetach {
        client_id: u64,
    },
    ClientDisconnected {
        client_id: u64,
    },
    /// A client writer drained its render slot and can accept another render.
    ClientWriterDrained {
        client_id: u64,
    },
    /// Ctrl+C or external shutdown signal received.
    QuitSignal,
}

/// Clamps client-reported terminal dimensions to at least one cell.
pub(crate) fn clamp_terminal_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(1), rows.max(1))
}

/// Reads the `TerminalHello`, answers with `Welcome`, then forwards client messages as events.
pub(crate) fn handle_client_handshake(
    mut stream: LocalStream,
    client_id: u64,
    server_event_tx: &mpsc::Sender<ServerEvent>,
    should_quit: &Arc<AtomicBool>,
) -> io::Result<()> {
    if should_quit.load(Ordering::Acquire) {
        return Ok(());
    }
    stream.set_nonblocking(false)?;
    stream.set_recv_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let hello: ClientMessage = match protocol::read_message(&mut stream, MAX_FRAME_SIZE) {
        Ok(msg) => msg,
        Err(err) => {
            debug!(client_id, err = %err, "failed to read client hello");
            return Ok(());
        }
    };
    let ClientMessage::TerminalHello {
        version,
        cols,
        rows,
        cell_width_px,
        cell_height_px,
        pixel_mouse,
    } = hello
    else {
        let welcome = ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            encoding: RenderEncoding::TerminalAnsi,
            error: Some("expected TerminalHello as first message".to_owned()),
        };
        let _ = protocol::write_message(&mut stream, &welcome);
        return Ok(());
    };
    if let protocol::VersionCheck::Incompatible(reason) = protocol::check_client_version(version) {
        let welcome = ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            encoding: RenderEncoding::TerminalAnsi,
            error: Some(reason),
        };
        let _ = protocol::write_message(&mut stream, &welcome);
        return Ok(());
    }
    let (cols, rows) = clamp_terminal_size(cols, rows);
    if should_quit.load(Ordering::Acquire) {
        return Ok(());
    }
    let welcome = ServerMessage::Welcome {
        version: PROTOCOL_VERSION,
        encoding: RenderEncoding::TerminalAnsi,
        error: None,
    };
    protocol::write_message(&mut stream, &welcome).map_err(|e| io::Error::other(e.to_string()))?;
    stream.set_recv_timeout(None)?;

    let writer_queue = ClientWriterQueue::new();
    let writer = ClientWriter {
        control: ClientControlWriter::queue(writer_queue.clone()),
        render: ClientRenderWriter::queue(writer_queue.clone()),
    };
    let write_stream = stream.try_clone()?;
    let writer_event_tx = server_event_tx.clone();
    std::thread::spawn(move || {
        client_writer_loop(write_stream, client_id, writer_queue, writer_event_tx);
    });
    if should_quit.load(Ordering::Acquire) {
        send_shutdown_to_unregistered_client(&writer);
        return Ok(());
    }
    let connected = ServerEvent::ClientConnected {
        client_id,
        cols,
        rows,
        cell_width_px,
        cell_height_px,
        pixel_mouse,
        writer,
    };
    if let Err(err) = server_event_tx.blocking_send(connected) {
        if let ServerEvent::ClientConnected { writer, .. } = err.0 {
            send_shutdown_to_unregistered_client(&writer);
        }
    }
    client_read_loop(stream, client_id, server_event_tx, should_quit)
}

fn send_shutdown_to_unregistered_client(writer: &ClientWriter) {
    let mut framed = Vec::new();
    if protocol::write_message(
        &mut framed,
        &ServerMessage::ServerShutdown {
            reason: Some("server is shutting down".to_owned()),
        },
    )
    .is_ok()
    {
        let _ = writer.control.send(framed);
    }
}

/// The client writer loop — prioritizes control messages over render frames.
fn client_writer_loop(
    mut stream: LocalStream,
    client_id: u64,
    writer_queue: Arc<ClientWriterQueue>,
    server_event_tx: mpsc::Sender<ServerEvent>,
) {
    while let Some(item) = writer_queue.recv() {
        let written = match item {
            ClientWriteItem::Control(data) => write_framed_bytes(&mut stream, &data),
            ClientWriteItem::Render(data) => {
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientWriterDrained { client_id });
                write_framed_bytes(&mut stream, &data)
            }
        };
        if !written {
            let _ = server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
            break;
        }
    }
    writer_queue.close_writer();
    debug!("client writer thread exiting");
}

fn write_framed_bytes(stream: &mut LocalStream, data: &[u8]) -> bool {
    let result = crate::platform::write_client_stream(stream, data);
    if let Err(err) = result {
        debug!(err = %err, "client write failed, closing writer");
        return false;
    }
    if let Err(err) = stream.flush() {
        debug!(err = %err, "client flush failed, closing writer");
        return false;
    }
    true
}

fn client_read_loop(
    mut stream: LocalStream,
    client_id: u64,
    server_event_tx: &mpsc::Sender<ServerEvent>,
    should_quit: &Arc<AtomicBool>,
) -> io::Result<()> {
    while !should_quit.load(Ordering::Acquire) {
        let message = protocol::read_message(
            &mut crate::platform::ClientStreamReader(&mut stream),
            MAX_GRAPHICS_FRAME_SIZE,
        );
        let msg: ClientMessage = match message {
            Ok(msg) => msg,
            Err(err) => {
                debug!(client_id, err = %err, "client read ended, closing");
                let _ =
                    server_event_tx.blocking_send(ServerEvent::ClientDisconnected { client_id });
                break;
            }
        };
        let event = match msg {
            ClientMessage::Input { data } if data.len() > MAX_INPUT_PAYLOAD => {
                if crate::raw_input::is_complete_text_bracketed_paste(&data) {
                    warn!(
                        client_id,
                        size = data.len(),
                        "oversized paste from client, rejecting"
                    );
                    ServerEvent::ClientPasteRejected {
                        client_id,
                        size: data.len(),
                        max: MAX_INPUT_PAYLOAD,
                    }
                } else {
                    warn!(
                        client_id,
                        size = data.len(),
                        "oversized input from client, closing"
                    );
                    let _ = server_event_tx
                        .blocking_send(ServerEvent::ClientDisconnected { client_id });
                    break;
                }
            }
            ClientMessage::Input { data } => ServerEvent::ClientInput { client_id, data },
            ClientMessage::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
                pixel_mouse,
            } => {
                let (cols, rows) = clamp_terminal_size(cols, rows);
                ServerEvent::ClientResize {
                    client_id,
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                    pixel_mouse,
                }
            }
            ClientMessage::Detach => {
                let _ = server_event_tx.blocking_send(ServerEvent::ClientDetach { client_id });
                break;
            }
            ClientMessage::AttachTerminal {
                terminal_id,
                takeover,
            } => ServerEvent::ClientAttachTerminal {
                client_id,
                terminal_id,
                takeover,
            },
            ClientMessage::AttachScroll {
                source,
                direction,
                lines,
                column,
                row,
                modifiers,
            } => ServerEvent::ClientAttachScroll {
                client_id,
                source,
                direction,
                lines,
                column,
                row,
                modifiers,
            },
            ClientMessage::AttachMouse {
                kind,
                position,
                geometry,
                modifiers,
                lines,
            } => ServerEvent::ClientAttachMouse {
                client_id,
                kind,
                position,
                geometry,
                modifiers,
                lines,
            },
            ClientMessage::TerminalHello { .. } => continue,
        };
        if server_event_tx.blocking_send(event).is_err() {
            break;
        }
    }
    debug!(client_id, "client read thread exiting");
    Ok(())
}
