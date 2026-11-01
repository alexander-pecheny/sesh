//! `sesh-mux terminal attach`: shows one pane's terminal in the calling terminal.

mod attach;

use std::io::{self, Read as _, Write as _};
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use interprocess::local_socket::traits::Stream as _;

use crate::ipc::LocalStream;
use crate::protocol::{self, ClientMessage, ServerMessage, MAX_FRAME_SIZE, PROTOCOL_VERSION};
use attach::{write_attach_semantic_action, AttachEscapeState, AttachInputAction};

const MOUSE_SCROLL_LINES: usize = 3;
const TICK: Duration = Duration::from_millis(100);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) trait ClientMessageSink {
    fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()>;
}

impl ClientMessageSink for LocalStream {
    fn send_client_message(&mut self, message: &ClientMessage) -> io::Result<()> {
        protocol::write_message(self, message).map_err(io::Error::other)
    }
}

pub(super) fn write_to_server(
    stream: &mut impl ClientMessageSink,
    message: &ClientMessage,
) -> io::Result<()> {
    stream.send_client_message(message)
}

enum Event {
    Server(ServerMessage),
    ServerGone(String),
    Input(Vec<u8>),
}

struct TerminalGuard {
    keyboard: crate::terminal_modes::DirectHostKeyboardState,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        set_mouse_capture(true, false)?;
        Ok(Self {
            keyboard: Default::default(),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = crate::terminal_modes::set_direct_host_keyboard_protocol(
            &mut stdout,
            &mut self.keyboard,
            0,
            0,
        );
        let _ = crate::terminal_modes::clear_host_mouse_reporting(&mut stdout);
        let _ = execute!(stdout, DisableBracketedPaste, LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn set_mouse_capture(enabled: bool, sgr_pixels: bool) -> io::Result<()> {
    let mut stdout = io::stdout();
    crate::terminal_modes::clear_host_mouse_reporting(&mut stdout)?;
    if enabled {
        execute!(stdout, EnableMouseCapture)?;
        if sgr_pixels {
            stdout.write_all(b"\x1b[?1016h")?;
        }
    }
    stdout.flush()
}

fn handshake(stream: &mut LocalStream, cols: u16, rows: u16) -> io::Result<()> {
    write_to_server(
        stream,
        &ClientMessage::TerminalHello {
            version: PROTOCOL_VERSION,
            cols,
            rows,
            cell_width_px: 0,
            cell_height_px: 0,
            pixel_mouse: false,
        },
    )?;
    stream.set_recv_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let welcome: ServerMessage =
        protocol::read_message(stream, MAX_FRAME_SIZE).map_err(io::Error::other)?;
    stream.set_recv_timeout(None)?;
    match welcome {
        ServerMessage::Welcome { error: None, .. } => Ok(()),
        ServerMessage::Welcome {
            error: Some(error), ..
        } => Err(io::Error::other(error)),
        other => Err(io::Error::other(format!(
            "unexpected handshake reply: {other:?}"
        ))),
    }
}

pub fn run_terminal_attach(terminal_id: String, takeover: bool) -> io::Result<()> {
    let socket_path = crate::server::socket_paths::client_socket_path();
    let mut stream = crate::ipc::connect_local_stream(&socket_path).map_err(|err| {
        io::Error::other(format!(
            "cannot reach sesh-mux at {}: {err}",
            socket_path.display()
        ))
    })?;
    let (mut cols, mut rows) = crossterm::terminal::size()?;
    handshake(&mut stream, cols, rows)?;
    write_to_server(
        &mut stream,
        &ClientMessage::AttachTerminal {
            terminal_id,
            takeover,
        },
    )?;

    let (tx, rx) = mpsc::channel();
    let mut reader = interprocess::TryClone::try_clone(&stream)?;
    let server_tx = tx.clone();
    std::thread::spawn(move || loop {
        match protocol::read_message::<_, ServerMessage>(
            &mut reader,
            protocol::MAX_GRAPHICS_FRAME_SIZE,
        ) {
            Ok(message) => {
                if server_tx.send(Event::Server(message)).is_err() {
                    return;
                }
            }
            Err(err) => {
                let _ = server_tx.send(Event::ServerGone(err.to_string()));
                return;
            }
        }
    });
    std::thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buffer = [0u8; 8192];
        while let Ok(read) = stdin.read(&mut buffer) {
            if read == 0 || tx.send(Event::Input(buffer[..read].to_vec())).is_err() {
                return;
            }
        }
    });

    let mut guard = TerminalGuard::enter()?;
    let mut escape = AttachEscapeState::default();
    let outcome = loop {
        let event = match rx.recv_timeout(TICK) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let size = crossterm::terminal::size()?;
                if size != (cols, rows) {
                    (cols, rows) = size;
                    write_to_server(
                        &mut stream,
                        &ClientMessage::Resize {
                            cols,
                            rows,
                            cell_width_px: 0,
                            cell_height_px: 0,
                            pixel_mouse: false,
                        },
                    )?;
                }
                if let Some(prefix) = escape.take_pending_prefix() {
                    write_to_server(&mut stream, &ClientMessage::Input { data: prefix })?;
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        };
        match event {
            Event::Input(data) => match escape.filter_input(data, rows, MOUSE_SCROLL_LINES) {
                AttachInputAction::Forward(data) => {
                    write_to_server(&mut stream, &ClientMessage::Input { data })?
                }
                AttachInputAction::ForwardPair(first, second) => {
                    write_to_server(&mut stream, &ClientMessage::Input { data: first })?;
                    write_to_server(&mut stream, &ClientMessage::Input { data: second })?;
                }
                AttachInputAction::Semantic(action) => {
                    write_attach_semantic_action(&mut stream, action)?
                }
                AttachInputAction::ForwardThenSemantic(prefix, action) => {
                    write_to_server(&mut stream, &ClientMessage::Input { data: prefix })?;
                    write_attach_semantic_action(&mut stream, action)?;
                }
                AttachInputAction::Detach => {
                    let _ = write_to_server(&mut stream, &ClientMessage::Detach);
                    break Ok(());
                }
                AttachInputAction::None => {}
            },
            Event::Server(message) => {
                let mut stdout = io::stdout();
                match message {
                    ServerMessage::Terminal(frame) => stdout.write_all(&frame.bytes)?,
                    ServerMessage::Clipboard { data } => {
                        use base64::Engine;
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
                            crate::selection::write_osc52_bytes(&bytes);
                        }
                    }
                    ServerMessage::MouseCapture { sgr_pixels, .. } => {
                        set_mouse_capture(true, sgr_pixels)?
                    }
                    ServerMessage::DirectTerminalKeyboardProtocol {
                        flags,
                        modify_other_keys_level,
                    } => crate::terminal_modes::set_direct_host_keyboard_protocol(
                        &mut stdout,
                        &mut guard.keyboard,
                        flags,
                        modify_other_keys_level,
                    )?,
                    ServerMessage::TerminalBell { count } => {
                        stdout.write_all(&vec![0x07; usize::from(count.min(8))])?
                    }
                    ServerMessage::ServerShutdown { reason } => {
                        break match reason.as_deref() {
                            None | Some("detached") => Ok(()),
                            Some(reason) => Err(io::Error::other(reason.to_owned())),
                        };
                    }
                    _ => {}
                }
                stdout.flush()?;
            }
            Event::ServerGone(err) => {
                break Err(io::Error::other(format!("connection lost: {err}")))
            }
        }
    };
    drop(guard);
    outcome
}
