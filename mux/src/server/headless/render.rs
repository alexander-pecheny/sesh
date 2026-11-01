use ratatui::layout::Rect;

use super::*;

impl HeadlessServer {
    fn attached_terminal_ids(&self) -> HashSet<String> {
        self.clients
            .values()
            .filter(|client| client.writer.is_some())
            .filter_map(|client| match &client.mode {
                ClientConnectionMode::TerminalAttach { terminal_id } => Some(terminal_id.clone()),
                ClientConnectionMode::TerminalPending => None,
            })
            .collect()
    }

    pub(super) fn stream_host_mouse_capture_mode(&mut self) {
        let requested = self
            .clients
            .iter()
            .filter_map(|(&client_id, client)| {
                let ClientConnectionMode::TerminalAttach { terminal_id } = &client.mode else {
                    return None;
                };
                let runtime = self.runtime_for_terminal_id_string(terminal_id);
                let child_requests_mouse =
                    runtime.is_some_and(crate::terminal::TerminalRuntime::mouse_reporting_enabled);
                let sgr_pixels = child_requests_mouse
                    && client.pixel_mouse
                    && runtime
                        .is_some_and(crate::terminal::TerminalRuntime::sgr_pixel_mouse_enabled);
                Some((client_id, child_requests_mouse, sgr_pixels))
            })
            .collect::<Vec<_>>();
        for (client_id, enabled, sgr_pixels) in requested {
            let Some(client) = self.clients.get(&client_id) else {
                continue;
            };
            if client.host_mouse_capture_active == Some(enabled)
                && client.host_sgr_pixels_active == Some(sgr_pixels)
            {
                continue;
            }
            if self.send_to_client(
                client_id,
                ServerMessage::MouseCapture {
                    enabled,
                    sgr_pixels,
                },
            ) {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.host_mouse_capture_active = Some(enabled);
                    client.host_sgr_pixels_active = Some(sgr_pixels);
                }
            }
        }
    }

    pub(super) fn stream_direct_terminal_keyboard_mode(&mut self) {
        let requested = self
            .clients
            .iter()
            .filter_map(|(&client_id, client)| {
                let ClientConnectionMode::TerminalAttach { terminal_id } = &client.mode else {
                    return None;
                };
                let mode =
                    self.runtime_for_terminal_id_string(terminal_id)
                        .map_or((0, 0), |runtime| {
                            let flags = match runtime.keyboard_protocol() {
                                crate::input::KeyboardProtocol::Legacy => 0,
                                crate::input::KeyboardProtocol::Kitty { flags } => flags,
                            };
                            (flags, runtime.modify_other_keys_level())
                        });
                (client.host_keyboard_protocol_active != Some(mode)).then_some((client_id, mode))
            })
            .collect::<Vec<_>>();
        for (client_id, (flags, modify_other_keys_level)) in requested {
            if self.send_to_client(
                client_id,
                ServerMessage::DirectTerminalKeyboardProtocol {
                    flags,
                    modify_other_keys_level,
                },
            ) {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.host_keyboard_protocol_active = Some((flags, modify_other_keys_level));
                }
            }
        }
    }

    /// Tells PTY readers which panes someone is watching, so their output wakes the loop at once.
    pub(super) fn sync_immediate_pty_sources(&self) {
        let attached = self.attached_terminal_ids();
        let mut pane_ids = HashSet::new();
        if !attached.is_empty() {
            for workspace in &self.app.state.workspaces {
                for tab in &workspace.tabs {
                    pane_ids.extend(tab.panes.iter().filter_map(|(&pane_id, pane)| {
                        attached
                            .contains(pane.attached_terminal_id.as_str())
                            .then_some(pane_id)
                    }));
                }
            }
        }
        self.app.render_dirty.set_immediate_pty_sources(pane_ids);
    }

    pub(super) fn render_and_stream(&mut self) {
        let targets = self
            .clients
            .iter()
            .filter_map(|(&client_id, client)| match &client.mode {
                ClientConnectionMode::TerminalAttach { terminal_id } if client.writer.is_some() => {
                    Some((client_id, client.terminal_size, terminal_id.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut broken_clients = Vec::new();
        for (client_id, (cols, rows), terminal_id) in targets {
            let area = Rect::new(0, 0, cols, rows);
            let Some(runtime) = self.runtime_for_terminal_id_string(&terminal_id) else {
                self.send_to_client(
                    client_id,
                    ServerMessage::ServerShutdown {
                        reason: Some(format!(
                            "terminal attach ended: terminal {terminal_id} not found"
                        )),
                    },
                );
                broken_clients.push(client_id);
                continue;
            };
            let (mut buffer, cursor) =
                crate::server::render_stream::render_terminal_virtual(runtime, area);
            if let Some(selection) = self
                .clients
                .get(&client_id)
                .and_then(|client| client.attach_selection.selection())
                .filter(|selection| selection.is_visible())
            {
                let scroll = runtime.scroll_metrics();
                for y in 0..area.height {
                    for x in 0..area.width {
                        if selection.contains(y, x, scroll) {
                            let cell = &mut buffer[(x, y)];
                            cell.set_style(
                                cell.style()
                                    .add_modifier(ratatui::style::Modifier::REVERSED),
                            );
                        }
                    }
                }
            }
            let hyperlinks = runtime.visible_hyperlinks(area);
            let frame =
                FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, cursor, &hyperlinks);

            let Some(client) = self.clients.get_mut(&client_id) else {
                continue;
            };
            let Some(writer) = client.writer.as_ref().cloned() else {
                continue;
            };
            let Some(prepared) = client.render_state.prepare_frame(frame) else {
                client.render_pending = false;
                continue;
            };
            let serialized = match Self::frame_server_message(prepared.message()) {
                Ok(frame) => frame,
                Err(protocol::FramingError::Oversized { claimed, max }) => {
                    warn!(
                        client_id,
                        claimed, max, "skipping oversized frame for client"
                    );
                    continue;
                }
                Err(err) => {
                    warn!(client_id, err = %err, "failed to serialize frame");
                    broken_clients.push(client_id);
                    continue;
                }
            };
            match writer.render.try_send(serialized) {
                Ok(()) => {
                    client.render_state.commit_sent_frame(prepared);
                    client.render_pending = false;
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => client.render_pending = true,
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    broken_clients.push(client_id)
                }
            }
        }
        for client_id in broken_clients {
            self.remove_client(client_id);
        }
        self.app.full_redraw_pending = false;
    }
}
