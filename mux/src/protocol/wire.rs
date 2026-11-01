//! Wire protocol for sesh-mux server/client communication.
//!
//! Defines the message types, framing, version negotiation, and safety
//! constraints for the binary protocol over local sockets.
//!
//! `PROTOCOL_VERSION` still guards same-install direct-terminal and internal
//! operations. Client-owned shells negotiate the independent stable endpoint
//! contract in [`super::endpoint`].

use std::collections::HashMap;
use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Current protocol version. Bumped when wire format changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 100;

/// Maximum allowed frame payload size (2 MB). Frames larger than this are
/// rejected to prevent denial-of-service via oversized length prefixes.
pub const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

/// Maximum allowed server-to-client frame payload when Kitty graphics are enabled.
/// Normal traffic keeps `MAX_FRAME_SIZE`; this larger cap is only for explicit
/// image payloads that are naturally much larger after base64 encoding.
pub const MAX_GRAPHICS_FRAME_SIZE: usize = 32 * 1024 * 1024;

/// Length of the u32 little-endian length prefix in bytes.
const LENGTH_PREFIX_BYTES: usize = 4;

// ---------------------------------------------------------------------------
// Client → Server messages
// ---------------------------------------------------------------------------

/// Render payload encoding negotiated during client handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderEncoding {
    /// Send full semantic FrameData values. This is the local/default mode.
    SemanticFrame,
    /// Send already-diffed terminal ANSI byte streams.
    TerminalAnsi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKeyKind {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientKeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    Esc,
    Char(char),
    F(u8),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientMouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMouseKind {
    Down(ClientMouseButton),
    Up(ClientMouseButton),
    Drag(ClientMouseButton),
    Moved,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMousePosition {
    Cell {
        column: u16,
        row: u16,
    },
    Pixels {
        x: u32,
        y: u32,
        column: u16,
        row: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientMouseGeometry {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
}

/// Pane-domain input after the client has classified and consumed shell actions.
///
/// Keys are semantic rather than outer-terminal VT bytes so the target pane can
/// encode them for the child application's negotiated keyboard protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientPaneInputEvent {
    Key {
        code: ClientKeyCode,
        modifiers: u8,
        kind: ClientKeyKind,
        repeat_count: u16,
        shifted_codepoint: Option<u32>,
        generated_text: Option<String>,
        tracks_release: bool,
        physical_key_id: Option<u32>,
        windows_record: Option<crate::input::WindowsKeyRecord>,
    },
    TextCommit(String),
    Mouse {
        kind: ClientMouseKind,
        position: ClientMousePosition,
        geometry: Option<ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    },
    Paste(String),
}

impl ClientKeyKind {
    pub(crate) fn to_crossterm(self) -> crossterm::event::KeyEventKind {
        match self {
            Self::Press => crossterm::event::KeyEventKind::Press,
            Self::Repeat => crossterm::event::KeyEventKind::Repeat,
            Self::Release => crossterm::event::KeyEventKind::Release,
        }
    }
}

impl ClientKeyCode {
    pub(crate) fn to_crossterm(&self) -> crossterm::event::KeyCode {
        use crossterm::event::KeyCode;
        match self {
            Self::Backspace => KeyCode::Backspace,
            Self::Enter => KeyCode::Enter,
            Self::Left => KeyCode::Left,
            Self::Right => KeyCode::Right,
            Self::Up => KeyCode::Up,
            Self::Down => KeyCode::Down,
            Self::Home => KeyCode::Home,
            Self::End => KeyCode::End,
            Self::PageUp => KeyCode::PageUp,
            Self::PageDown => KeyCode::PageDown,
            Self::Tab => KeyCode::Tab,
            Self::BackTab => KeyCode::BackTab,
            Self::Delete => KeyCode::Delete,
            Self::Insert => KeyCode::Insert,
            Self::Esc => KeyCode::Esc,
            Self::Char(ch) => KeyCode::Char(*ch),
            Self::F(n) => KeyCode::F(*n),
            Self::Null => KeyCode::Null,
        }
    }
}

impl ClientMouseButton {
    pub(crate) fn from_crossterm(button: crossterm::event::MouseButton) -> Self {
        match button {
            crossterm::event::MouseButton::Left => Self::Left,
            crossterm::event::MouseButton::Right => Self::Right,
            crossterm::event::MouseButton::Middle => Self::Middle,
        }
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::MouseButton {
        match self {
            Self::Left => crossterm::event::MouseButton::Left,
            Self::Right => crossterm::event::MouseButton::Right,
            Self::Middle => crossterm::event::MouseButton::Middle,
        }
    }
}

impl ClientMouseKind {
    pub(crate) fn from_crossterm(kind: crossterm::event::MouseEventKind) -> Option<Self> {
        use crossterm::event::MouseEventKind;
        Some(match kind {
            MouseEventKind::Down(button) => Self::Down(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Up(button) => Self::Up(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Drag(button) => Self::Drag(ClientMouseButton::from_crossterm(button)),
            MouseEventKind::Moved => Self::Moved,
            MouseEventKind::ScrollUp => Self::ScrollUp,
            MouseEventKind::ScrollDown => Self::ScrollDown,
            MouseEventKind::ScrollLeft => Self::ScrollLeft,
            MouseEventKind::ScrollRight => Self::ScrollRight,
        })
    }

    pub(crate) fn to_crossterm(self) -> crossterm::event::MouseEventKind {
        use crossterm::event::MouseEventKind;
        match self {
            Self::Down(button) => MouseEventKind::Down(button.to_crossterm()),
            Self::Up(button) => MouseEventKind::Up(button.to_crossterm()),
            Self::Drag(button) => MouseEventKind::Drag(button.to_crossterm()),
            Self::Moved => MouseEventKind::Moved,
            Self::ScrollUp => MouseEventKind::ScrollUp,
            Self::ScrollDown => MouseEventKind::ScrollDown,
            Self::ScrollLeft => MouseEventKind::ScrollLeft,
            Self::ScrollRight => MouseEventKind::ScrollRight,
        }
    }
}

impl ClientPaneInputEvent {
    pub(crate) fn to_raw_input_event(&self) -> crate::raw_input::RawInputEvent {
        self.to_raw_input_event_inner()
    }

    fn to_raw_input_event_inner(&self) -> crate::raw_input::RawInputEvent {
        match self {
            Self::Key {
                code,
                modifiers,
                kind,
                repeat_count,
                shifted_codepoint,
                generated_text,
                tracks_release,
                windows_record,
                ..
            } => {
                let mut key = crate::input::TerminalKey::new(
                    code.to_crossterm(),
                    crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                )
                .with_kind(kind.to_crossterm())
                .with_repeat_count(*repeat_count)
                .with_generated_text(generated_text.clone())
                .with_physical_identity_hint(*tracks_release && generated_text.is_some())
                .with_windows_composition_hint(*windows_record);
                if let Some(shifted_codepoint) = shifted_codepoint {
                    key = key.with_shifted_codepoint(*shifted_codepoint);
                }
                crate::raw_input::RawInputEvent::Key(key)
            }
            Self::TextCommit(text) => {
                crate::raw_input::RawInputEvent::Text(crate::input::TextCommit::new(text.clone()))
            }
            Self::Mouse {
                kind,
                position,
                modifiers,
                ..
            } => {
                let (column, row) = match position {
                    ClientMousePosition::Cell { column, row }
                    | ClientMousePosition::Pixels { column, row, .. } => (*column, *row),
                };
                crate::raw_input::RawInputEvent::Mouse(crossterm::event::MouseEvent {
                    kind: kind.to_crossterm(),
                    column,
                    row,
                    modifiers: crossterm::event::KeyModifiers::from_bits_truncate(*modifiers),
                })
            }
            Self::Paste(text) => crate::raw_input::RawInputEvent::Paste(text.clone()),
        }
    }
}

/// Messages from an attach client to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Handshake: protocol version and terminal dimensions.
    TerminalHello {
        version: u32,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    /// Raw input bytes read from the client's stdin.
    Input {
        data: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        pixel_mouse: bool,
    },
    Detach,
    /// Switch this connection into direct terminal attach mode.
    AttachTerminal {
        terminal_id: String,
        takeover: bool,
    },
    /// Scroll input handled by the server for the attached terminal.
    AttachScroll {
        source: AttachScrollSource,
        direction: AttachScrollDirection,
        lines: u16,
        column: Option<u16>,
        row: Option<u16>,
        modifiers: u8,
    },
    /// One structured mouse event for the attached terminal.
    AttachMouse {
        kind: ClientMouseKind,
        position: ClientMousePosition,
        geometry: Option<ClientMouseGeometry>,
        modifiers: u8,
        lines: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachScrollSource {
    Wheel,
    PageKey {
        /// Original key bytes to forward when the child application owns page keys.
        input: Vec<u8>,
    },
}

// ---------------------------------------------------------------------------
// Server → Client messages
// ---------------------------------------------------------------------------

/// A single cell in a rendered frame, serialized independently from ratatui's
/// `Cell` type to keep the wire protocol stable.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, bincode::Decode)]
pub struct CellData {
    /// Grapheme cluster displayed in this cell (usually 1–2 chars).
    pub symbol: String,
    /// Foreground color as a packed u32 (0xAARRGGBB or ratatui Color index).
    pub fg: u32,
    /// Background color as a packed u32.
    pub bg: u32,
    /// Bitmask of style modifiers (bold, italic, etc.) plus sesh-mux extension bits.
    pub modifier: u16,
    /// Whether this cell should be skipped during diff-based rendering.
    pub skip: bool,
    /// Index into `FrameData::hyperlinks` for this cell's OSC 8 target, if any.
    pub hyperlink: Option<u32>,
}

impl Clone for CellData {
    fn clone(&self) -> Self {
        Self {
            symbol: self.symbol.clone(),
            ..*self
        }
    }

    fn clone_from(&mut self, source: &Self) {
        let mut symbol = std::mem::take(&mut self.symbol);
        symbol.clone_from(&source.symbol);
        *self = Self { symbol, ..*source };
    }
}

impl CellData {
    pub(crate) fn from_ratatui_cell(cell: &ratatui::buffer::Cell) -> Self {
        Self {
            symbol: cell.symbol().to_owned(),
            fg: color_to_u32(cell.fg),
            bg: color_to_u32(cell.bg),
            modifier: modifier_to_u16(cell.modifier),
            skip: cell.skip,
            hyperlink: None,
        }
    }
}

/// Cursor shape encoded as a DECSCUSR parameter.
///
/// 0 = terminal default, 1 = blinking block, 2 = steady block,
/// 3 = blinking underline, 4 = steady underline, 5 = blinking bar,
/// 6 = steady bar.
pub type CursorShapeParam = u8;

/// Cursor position within a rendered frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, bincode::Decode)]
pub struct CursorState {
    /// Column offset (0-based) of the cursor.
    pub x: u16,
    /// Row offset (0-based) of the cursor.
    pub y: u16,
    /// Whether the cursor is visible.
    pub visible: bool,
    /// Cursor shape as a DECSCUSR parameter.
    #[serde(default)]
    pub shape: CursorShapeParam,
}

/// A rendered frame to be displayed by the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameData {
    /// Cells in row-major order. Length must equal `width * height`.
    pub cells: Vec<CellData>,
    /// Frame width in columns.
    pub width: u16,
    /// Frame height in rows.
    pub height: u16,
    /// Cursor state for this frame, if applicable.
    pub cursor: Option<CursorState>,
    /// OSC 8 hyperlink URIs referenced by cells.
    pub hyperlinks: Vec<String>,
    /// Kitty graphics protocol bytes to apply after the text frame.
    pub graphics: Vec<u8>,
}

impl FrameData {
    pub fn from_ratatui_buffer_with_hyperlinks(
        buffer: &ratatui::buffer::Buffer,
        cursor: Option<CursorState>,
        hyperlinks: &[((u16, u16), String, String)],
    ) -> Self {
        let area = buffer.area;
        let width = area.width;
        let height = area.height;

        let mut hyperlink_uris = Vec::<String>::new();
        let mut hyperlink_indices = HashMap::<&str, u32>::new();
        let mut hyperlink_by_position = HashMap::<(u16, u16), (&str, &str)>::new();
        for ((x, y), symbol, uri) in hyperlinks {
            hyperlink_by_position.insert((*x, *y), (symbol.as_str(), uri.as_str()));
        }
        let mut cells = Vec::with_capacity((width as usize) * (height as usize));
        for row in 0..height {
            for col in 0..width {
                let cell = buffer.cell((col, row)).expect("cell within bounds");
                let hyperlink = hyperlink_by_position
                    .get(&(col, row))
                    .and_then(|(symbol, uri)| {
                        if *symbol != cell.symbol() {
                            return None;
                        }
                        Some(*hyperlink_indices.entry(*uri).or_insert_with(|| {
                            let index = hyperlink_uris.len() as u32;
                            hyperlink_uris.push((*uri).to_owned());
                            index
                        }))
                    });
                let mut cell = CellData::from_ratatui_cell(cell);
                cell.hyperlink = hyperlink;
                cells.push(cell);
            }
        }

        FrameData {
            cells,
            width,
            height,
            cursor,
            hyperlinks: hyperlink_uris,
            graphics: Vec::new(),
        }
    }
}

/// Terminal ANSI bytes encoded by the server for network-efficient clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalFrame {
    /// Monotonic per-client frame sequence.
    pub seq: u64,
    /// Frame width in columns.
    pub width: u16,
    /// Frame height in rows.
    pub height: u16,
    /// Whether bytes contain a full redraw rather than an incremental diff.
    pub full: bool,
    /// Terminal escape bytes ready to write directly to stdout.
    pub bytes: Vec<u8>,
}

/// Messages from the server to an attach client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Handshake reply; `error` says why the client was rejected.
    Welcome {
        version: u32,
        encoding: RenderEncoding,
        error: Option<String>,
    },
    /// Terminal bytes to write directly to the client's stdout.
    Terminal(TerminalFrame),
    ServerShutdown {
        reason: Option<String>,
    },
    /// OSC 52 clipboard data, base64-encoded.
    Clipboard {
        data: String,
    },
    /// Whether the client should capture host mouse input.
    MouseCapture {
        enabled: bool,
        sgr_pixels: bool,
    },
    /// BEL characters from one PTY read of the attached pane.
    TerminalBell {
        count: u16,
    },
    /// Kitty keyboard flags and modifyOtherKeys level requested by the attached pane.
    DirectTerminalKeyboardProtocol {
        flags: u16,
        modify_other_keys_level: u8,
    },
}

// ---------------------------------------------------------------------------
// Color / Modifier conversion helpers
// ---------------------------------------------------------------------------

/// Converts a ratatui `Color` to a packed u32 for wire transport.
///
/// Encoding:
/// - Named colors (Reset, Black, …, White) → `0x00_00_00_XX` where XX is 0..=16
/// - Indexed palette → `0x01_00_00_XX` where XX is the palette index
/// - RGB → `0x02_RR_GG_BB` with components in the lower 3 bytes
pub(crate) fn color_to_u32(color: ratatui::style::Color) -> u32 {
    match color {
        ratatui::style::Color::Reset => 0x00_00_00_00,
        ratatui::style::Color::Black => 0x00_00_00_01,
        ratatui::style::Color::Red => 0x00_00_00_02,
        ratatui::style::Color::Green => 0x00_00_00_03,
        ratatui::style::Color::Yellow => 0x00_00_00_04,
        ratatui::style::Color::Blue => 0x00_00_00_05,
        ratatui::style::Color::Magenta => 0x00_00_00_06,
        ratatui::style::Color::Cyan => 0x00_00_00_07,
        ratatui::style::Color::Gray => 0x00_00_00_08,
        ratatui::style::Color::DarkGray => 0x00_00_00_09,
        ratatui::style::Color::LightRed => 0x00_00_00_0A,
        ratatui::style::Color::LightGreen => 0x00_00_00_0B,
        ratatui::style::Color::LightYellow => 0x00_00_00_0C,
        ratatui::style::Color::LightBlue => 0x00_00_00_0D,
        ratatui::style::Color::LightMagenta => 0x00_00_00_0E,
        ratatui::style::Color::LightCyan => 0x00_00_00_0F,
        ratatui::style::Color::White => 0x00_00_00_10,
        ratatui::style::Color::Indexed(i) => 0x01_00_00_00 | (i as u32),
        ratatui::style::Color::Rgb(r, g, b) => {
            0x02_00_00_00 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        }
    }
}

const UNDERLINE_STYLE_SHIFT: u16 = 12;
const UNDERLINE_STYLE_MASK: u16 = 0xF000;

/// Converts a ratatui `Modifier` bitmask to a u16 for wire transport.
pub(crate) fn modifier_to_u16(modifier: ratatui::style::Modifier) -> u16 {
    modifier.bits()
}

pub(crate) fn underline_style_from_modifier(modifier: u16) -> u8 {
    ((modifier & UNDERLINE_STYLE_MASK) >> UNDERLINE_STYLE_SHIFT) as u8
}

pub(crate) fn modifier_with_underline_style(
    modifier: ratatui::style::Modifier,
    underline_style: u8,
) -> ratatui::style::Modifier {
    let bits = modifier.bits() | ((u16::from(underline_style) & 0x0F) << UNDERLINE_STYLE_SHIFT);
    ratatui::style::Modifier::from_bits_retain(bits)
}

// ---------------------------------------------------------------------------
// Framing: length-prefixed binary messages
// ---------------------------------------------------------------------------

/// Errors that can occur during framing operations.
#[derive(Debug)]
pub enum FramingError {
    /// The decoded payload length exceeds the configured maximum frame size.
    Oversized { claimed: usize, max: usize },
    /// An I/O error occurred while reading or writing.
    Io(io::Error),
    /// Bincode serialization or deserialization failed.
    Bincode(String),
    /// The connection was closed before a complete frame could be read.
    UnexpectedEof,
}

impl std::fmt::Display for FramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FramingError::Oversized { claimed, max } => {
                write!(f, "frame size {claimed} exceeds maximum {max}")
            }
            FramingError::Io(e) => write!(f, "I/O error: {e}"),
            FramingError::Bincode(e) => write!(f, "bincode error: {e}"),
            FramingError::UnexpectedEof => write!(f, "unexpected end of stream"),
        }
    }
}

impl std::error::Error for FramingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FramingError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for FramingError {
    fn from(e: io::Error) -> Self {
        FramingError::Io(e)
    }
}

/// Serializes a message and writes it as a length-prefixed frame:
/// `[u32LE length][bincode payload]`.
///
/// This is a blocking/synchronous write suitable for use with `std::os::unix::net::UnixStream`
/// in blocking mode, or with any `Write` implementor.
///
/// # Errors
///
/// Returns `FramingError::Bincode` if the payload length exceeds `u32::MAX`
/// (would be truncated by the length prefix cast).
pub fn write_message<W: Write, M: Serialize>(writer: &mut W, msg: &M) -> Result<(), FramingError> {
    let payload = bincode::serde::encode_to_vec(msg, bincode::config::standard())
        .map_err(|e| FramingError::Bincode(e.to_string()))?;

    let len = payload.len();
    if len > u32::MAX as usize {
        return Err(FramingError::Bincode(format!(
            "payload length {len} exceeds u32::MAX ({}), would be truncated by length prefix",
            u32::MAX
        )));
    }

    writer.write_all(&(len as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// Reads and deserializes a length-prefixed frame from a reader.
///
/// Reassembles partial reads correctly. Rejects frames whose declared
/// length exceeds `max_frame_size` without panicking or allocating
/// oversized buffers.
pub fn read_message<R: Read, M: for<'de> Deserialize<'de>>(
    reader: &mut R,
    max_frame_size: usize,
) -> Result<M, FramingError> {
    // Read the 4-byte length prefix, reassembling partial reads.
    let mut len_buf = [0u8; LENGTH_PREFIX_BYTES];
    read_exact_or_eof(reader, &mut len_buf)?;
    let claimed_len = u32::from_le_bytes(len_buf) as usize;

    if claimed_len > max_frame_size {
        return Err(FramingError::Oversized {
            claimed: claimed_len,
            max: max_frame_size,
        });
    }

    // Read the payload, reassembling partial reads.
    let mut payload = vec![0u8; claimed_len];
    read_exact_or_eof(reader, &mut payload)?;

    let (msg, consumed) = bincode::serde::decode_from_slice(&payload, bincode::config::standard())
        .map_err(|e| FramingError::Bincode(e.to_string()))?;

    // Enforce that the decoder consumed the full payload.
    // Trailing bytes after the decoded message indicate a protocol violation
    // (e.g., a corrupted length prefix or concatenated payloads).
    if consumed != claimed_len {
        return Err(FramingError::Bincode(format!(
            "decoded {} bytes but payload length was {claimed_len}; trailing bytes are not allowed",
            consumed
        )));
    }

    Ok(msg)
}

/// Like `Read::read_exact`, but returns `FramingError::UnexpectedEof`
/// when the reader hits end-of-stream before filling the buffer, instead
/// of the generic `io::ErrorKind::UnexpectedEof`.
fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Result<(), FramingError> {
    reader.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            FramingError::UnexpectedEof
        } else {
            FramingError::Io(e)
        }
    })
}

// ---------------------------------------------------------------------------
// Version negotiation
// ---------------------------------------------------------------------------

/// Result of checking a client's protocol version against the server's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionCheck {
    /// Versions are compatible. The server should reply with a successful Welcome.
    Compatible,
    /// Versions are incompatible. The server should reply with a Welcome error and close.
    Incompatible(String),
}

/// Checks whether a client's protocol version is compatible with this server.
///
/// Current rules:
/// - Version 0 (pre-persistence client) is always rejected.
/// - Matching major versions are accepted.
/// - A client with a newer version than the server is rejected.
/// - A client with an older version than the server is rejected
///   (backward compatibility is not yet supported).
pub fn check_client_version(client_version: u32) -> VersionCheck {
    if client_version == 0 {
        return VersionCheck::Incompatible(
            "pre-persistence client (version 0) is not supported".to_owned(),
        );
    }

    if client_version == PROTOCOL_VERSION {
        VersionCheck::Compatible
    } else if client_version < PROTOCOL_VERSION {
        VersionCheck::Incompatible(format!(
            "client version {client_version} is older than server version {PROTOCOL_VERSION}; please upgrade your sesh-mux client"
        ))
    } else {
        VersionCheck::Incompatible(format!(
            "client version {client_version} is newer than server version {PROTOCOL_VERSION}; please upgrade the sesh-mux server"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_messages_round_trip() {
        let message = ClientMessage::AttachTerminal {
            terminal_id: "term-1".into(),
            takeover: true,
        };
        let mut framed = Vec::new();
        write_message(&mut framed, &message).unwrap();
        let decoded: ClientMessage = read_message(&mut framed.as_slice(), MAX_FRAME_SIZE).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut framed = Vec::new();
        write_message(&mut framed, &ClientMessage::Input { data: vec![0; 64] }).unwrap();
        let result: Result<ClientMessage, _> = read_message(&mut framed.as_slice(), 8);
        assert!(matches!(result, Err(FramingError::Oversized { .. })));
    }
}
