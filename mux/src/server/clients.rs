use std::collections::HashMap;

use crate::protocol::RenderEncoding;
use crate::server::client_transport::ClientWriter;
use crate::server::render_stream::ClientRenderState;

/// Pixel size of one cell of a client's terminal; zero when the client did not report it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CellSize {
    pub(crate) width_px: u32,
    pub(crate) height_px: u32,
}

impl CellSize {
    pub(crate) fn is_known(self) -> bool {
        self.width_px > 0 && self.height_px > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientConnectionMode {
    TerminalPending,
    TerminalAttach { terminal_id: String },
}

/// A connected attach client.
pub(crate) struct ClientConnection {
    pub(crate) mode: ClientConnectionMode,
    /// The client's terminal size after clamping.
    pub(crate) terminal_size: (u16, u16),
    pub(crate) cell_size: CellSize,
    pub(crate) last_activity: u64,
    pub(crate) render_state: ClientRenderState,
    /// Whether this client preserves exact SGR pixel reports.
    pub(crate) pixel_mouse: bool,
    /// Whether an ordinary render was skipped because the render channel was full.
    pub(crate) render_pending: bool,
    pub(crate) host_mouse_capture_active: Option<bool>,
    pub(crate) host_sgr_pixels_active: Option<bool>,
    pub(crate) host_keyboard_protocol_active: Option<(u16, u8)>,
    pub(crate) attach_selection: crate::server::attach_selection::AttachSelection,
    pub(crate) writer: Option<ClientWriter>,
}

impl ClientConnection {
    pub(crate) fn new_with_mode(
        mode: ClientConnectionMode,
        terminal_size: (u16, u16),
        cell_size: CellSize,
        last_activity: u64,
        render_encoding: RenderEncoding,
        writer: Option<ClientWriter>,
    ) -> Self {
        Self {
            mode,
            terminal_size,
            cell_size,
            last_activity,
            render_state: ClientRenderState::new(render_encoding),
            pixel_mouse: false,
            render_pending: false,
            host_mouse_capture_active: None,
            host_sgr_pixels_active: None,
            host_keyboard_protocol_active: None,
            attach_selection: Default::default(),
            writer,
        }
    }

    /// Clears a render skipped while the writer was full, saying whether there was one.
    pub(crate) fn take_deferred_render(&mut self) -> bool {
        std::mem::take(&mut self.render_pending)
    }
}

pub(crate) fn terminal_stream_client_ids(
    clients: &HashMap<u64, ClientConnection>,
    terminal_id: &str,
) -> Vec<u64> {
    clients
        .iter()
        .filter_map(|(&client_id, client)| match &client.mode {
            ClientConnectionMode::TerminalAttach {
                terminal_id: attached,
            } if attached == terminal_id => Some(client_id),
            _ => None,
        })
        .collect()
}
