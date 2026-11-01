use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use ratatui::style::{Color, Modifier, Style};
use ratatui::{layout::Rect, Frame};
#[cfg(any(unix, test))]
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, error};
use unicode_width::UnicodeWidthStr;

use crate::layout::PaneId;


use super::cursor::{CursorPositionSettleState, DecscusrTracker};
use super::{
    input::{
        ghostty_key_event_from_terminal_key, ghostty_mouse_encoder_for_terminal,
        ghostty_mouse_event_from_button_kind, ghostty_mouse_event_from_motion_kind,
        ghostty_mouse_event_from_wheel_kind, ghostty_mouse_position_for_terminal,
        ghostty_prefers_sesh_mux_text_encoding,
    },
    kitty_keyboard::KittyKeyboardTracker,
    osc::{
        current_transient_default_color_owner, parse_reported_cwd,
        restore_host_terminal_theme_if_needed, write_host_terminal_theme_selective,
        AgentOscStateTracker, DefaultColorEvent, DefaultColorEventTracker, DefaultColorOscTracker,
        DefaultColorQuery, DefaultColorTrackedEvent, OscDebugTracker,
    },
    xtgettcap::{C1XtgettcapQueryTracker, C1XtgettcapResponse},
};

const DEFAULT_DETECTION_ROWS: usize = 24;
const KITTY_GRAPHICS_REDRAW_SETTLE: Duration = Duration::from_millis(20);
const CURSOR_POSITION_SETTLE_ENABLED: bool = cfg!(windows);
const MODE_MOUSE_X10: u16 = 9;
const MODE_MOUSE_PRESS_RELEASE: u16 = 1000;
const MODE_MOUSE_BUTTON_MOTION: u16 = 1002;
const MODE_MOUSE_ANY_MOTION: u16 = 1003;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollMetrics {
    pub offset_from_bottom: usize,
    pub max_offset_from_bottom: usize,
    pub viewport_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursorState {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    /// DECSCUSR parameter (0–6). 0 means terminal default.
    pub shape: u8,
}

fn decscusr_cursor_shape(style: crate::ghostty::CursorVisualStyle, blinking: bool) -> u8 {
    match (style, blinking) {
        (crate::ghostty::CursorVisualStyle::Block, true)
        | (crate::ghostty::CursorVisualStyle::BlockHollow, true) => 1,
        (crate::ghostty::CursorVisualStyle::Block, false)
        | (crate::ghostty::CursorVisualStyle::BlockHollow, false) => 2,
        (crate::ghostty::CursorVisualStyle::Underline, true) => 3,
        (crate::ghostty::CursorVisualStyle::Underline, false) => 4,
        (crate::ghostty::CursorVisualStyle::Bar, true) => 5,
        (crate::ghostty::CursorVisualStyle::Bar, false) => 6,
    }
}

#[cfg(any(unix, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputState {
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub focus_reporting: bool,
    pub mouse_protocol_mode: crate::input::MouseProtocolMode,
    pub mouse_protocol_encoding: crate::input::MouseProtocolEncoding,
    pub mouse_alternate_scroll: bool,
    #[serde(default)]
    pub modify_other_keys: bool,
    #[serde(default)]
    pub color_scheme_reporting: bool,
}

#[cfg(test)]
impl InputState {
    pub fn mouse_reporting_enabled(self) -> bool {
        self.mouse_protocol_mode.reporting_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(self) -> bool {
        !self.alternate_screen
            && !self.mouse_reporting_enabled()
            // Bracketed paste distinguishes zsh's line editor (where it's on)
            // from e.g. less -X (where it's off).
            && (!self.application_cursor || self.bracketed_paste)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessBytesResult {
    pub request_render: bool,
    pub render_delay: Option<Duration>,
    pub terminal_title_changed: bool,
    pub terminal_bells: u16,
    pub clipboard_writes: Vec<Vec<u8>>,
    pub reported_cwd: Option<std::path::PathBuf>,
    pub terminal_responses: Vec<Bytes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TerminalReadSnapshot {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalCompressionStep {
    Busy,
    ActivityChanged(u64),
    Compressed(crate::ghostty::TerminalCompressionResult),
}

pub(crate) struct GhosttyPaneTerminal {
    pub core: Mutex<GhosttyPaneCore>,
    key_encoder: Mutex<crate::ghostty::KeyEncoder>,
    pending_pty_responses: Arc<Mutex<Vec<Bytes>>>,
}

pub(crate) struct GhosttyPaneCore {
    #[cfg(test)]
    pub dirty_collection_hook: Option<Box<dyn FnOnce() + Send>>,
    pub terminal: crate::ghostty::Terminal,
    pub render_state: crate::ghostty::RenderState,
    pub kitty_keyboard: KittyKeyboardTracker,
    pub initial_default_foreground: Option<crate::ghostty::RgbColor>,
    pub initial_default_background: Option<crate::ghostty::RgbColor>,
    pub host_terminal_theme: crate::terminal_theme::TerminalTheme,
    pub transient_default_color_owner_pgid: Option<u32>,
    pub default_color_tracker: DefaultColorOscTracker,
    pub default_color_event_tracker: DefaultColorEventTracker,
    c1_xtgettcap_tracker: C1XtgettcapQueryTracker,
    pub child_default_foreground_changed: bool,
    pub child_default_background_changed: bool,
    pub osc_debug_tracker: OscDebugTracker,
    pub agent_osc_state: AgentOscStateTracker,
    decscusr_tracker: DecscusrTracker,
    cursor_settle_state: CursorPositionSettleState,
    windows_powershell_prompt_cwd_reporting: bool,
}

pub(crate) struct PaneTerminal {
    pub(crate) ghostty: GhosttyPaneTerminal,
}

impl PaneTerminal {
    pub(crate) fn new(ghostty: GhosttyPaneTerminal) -> Self {
        Self { ghostty }
    }

    pub fn process_pty_bytes(
        &self,
        pane_id: PaneId,
        shell_pid: u32,
        bytes: &[u8],
        response_writer: &mpsc::Sender<Bytes>,
    ) -> ProcessBytesResult {
        self.ghostty
            .process_pty_bytes(pane_id, shell_pid, bytes, response_writer)
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Vec<Bytes> {
        self.ghostty
            .resize(rows, cols, cell_width_px, cell_height_px)
    }

    pub fn scroll_up(&self, lines: usize) {
        self.ghostty.scroll_up(lines);
    }

    pub fn scroll_down(&self, lines: usize) {
        self.ghostty.scroll_down(lines);
    }

    pub fn scroll_reset(&self) {
        self.ghostty.scroll_reset();
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        self.ghostty.scroll_metrics()
    }

    #[cfg(any(unix, test))]
    pub fn input_state(&self) -> Option<InputState> {
        self.ghostty.input_state()
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.ghostty.bracketed_paste_enabled()
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.ghostty.mouse_reporting_enabled()
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.ghostty.modify_other_keys_level()
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.ghostty.sgr_pixel_mouse_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        self.ghostty.plain_page_keys_use_host_scrollback()
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.ghostty.alternate_screen_active()
    }

    pub fn wheel_routing(&self) -> Option<crate::pane::WheelRouting> {
        self.ghostty.wheel_routing()
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        u16,
        Vec<crate::ghostty::ScreenTextRow>,
    )> {
        self.ghostty.screen_text_snapshot()
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.ghostty.cursor_state()
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.ghostty.synchronized_output_active()
    }

    pub fn visible_text(&self) -> String {
        self.ghostty.visible_text()
    }

    pub fn visible_ansi(&self) -> String {
        self.ghostty.visible_ansi()
    }

    pub fn detection_text(&self) -> String {
        self.ghostty.detection_text()
    }

    pub(crate) fn try_compression_activity(&self) -> Result<Option<u64>, crate::ghostty::Error> {
        self.ghostty.try_compression_activity()
    }

    pub(crate) fn try_compress_incremental_if_activity(
        &self,
        expected_activity: u64,
    ) -> Result<TerminalCompressionStep, crate::ghostty::Error> {
        self.ghostty
            .try_compress_incremental_if_activity(expected_activity)
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_text_snapshot(lines)
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_ansi_snapshot(lines)
    }

    pub(crate) fn recent_unwrapped_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_unwrapped_text_snapshot(lines)
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        self.ghostty.recent_unwrapped_ansi(lines)
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.ghostty.recent_unwrapped_ansi_snapshot(lines)
    }

    pub fn extract_selection<P>(
        &self,
        selection: &crate::selection::Selection<P>,
    ) -> Option<String> {
        self.ghostty.extract_selection(selection)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        self.ghostty.render(frame, area, show_cursor);
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.ghostty.visible_hyperlinks(area)
    }

    pub fn has_transient_default_color_override(&self) -> bool {
        self.ghostty.has_transient_default_color_override()
    }

    pub fn maybe_restore_host_terminal_theme(&self, pane_id: PaneId, shell_pid: u32) -> bool {
        self.ghostty
            .maybe_restore_host_terminal_theme(pane_id, shell_pid)
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.ghostty.terminal_title()
    }

    pub fn agent_osc_title(&self) -> String {
        self.ghostty.agent_osc_title()
    }

    pub fn agent_osc_progress(&self) -> String {
        self.ghostty.agent_osc_progress()
    }

    /// Clears retained OSC title/progress evidence on foreground agent change.
    pub fn clear_agent_osc_state(&self) {
        self.ghostty.clear_agent_osc_state()
    }

    pub fn keyboard_protocol(
        &self,
        fallback: crate::input::KeyboardProtocol,
    ) -> crate::input::KeyboardProtocol {
        self.ghostty.keyboard_protocol().unwrap_or(fallback)
    }

    pub fn kitty_keyboard_state_ansi(&self) -> Option<String> {
        self.ghostty
            .kitty_keyboard_state_ansi()
            .filter(|ansi| !ansi.is_empty())
    }

    pub fn encode_terminal_key(
        &self,
        key: crate::input::TerminalKey,
        protocol: crate::input::KeyboardProtocol,
    ) -> Vec<u8> {
        self.ghostty.encode_terminal_key(key, protocol)
    }

    pub(crate) fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_button(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_motion(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.ghostty.encode_mouse_wheel(kind, position, modifiers)
    }
}

impl GhosttyPaneTerminal {
    pub fn new(
        mut terminal: crate::ghostty::Terminal,
        _response_writer: mpsc::Sender<Bytes>,
    ) -> std::io::Result<Self> {
        let pending_pty_responses = Arc::new(Mutex::new(Vec::new()));
        let callback_responses = pending_pty_responses.clone();
        terminal
            .set_write_pty_callback(move |bytes| {
                if let Ok(mut responses) = callback_responses.lock() {
                    responses.push(Bytes::copy_from_slice(bytes));
                }
            })
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let mut render_state =
            crate::ghostty::RenderState::new().map_err(|e| std::io::Error::other(e.to_string()))?;
        let initial_colors = render_state
            .update(&terminal)
            .ok()
            .and_then(|_| render_state.colors().ok());
        let initial_default_foreground = initial_colors.map(|colors| colors.foreground);
        let initial_default_background = initial_colors.map(|colors| colors.background);
        let mut key_encoder =
            crate::ghostty::KeyEncoder::new().map_err(|e| std::io::Error::other(e.to_string()))?;
        key_encoder.set_from_terminal(&terminal);
        Ok(Self {
            core: Mutex::new(GhosttyPaneCore {
                #[cfg(test)]
                dirty_collection_hook: None,
                terminal,
                render_state,
                kitty_keyboard: KittyKeyboardTracker::default(),
                initial_default_foreground,
                initial_default_background,
                host_terminal_theme: crate::terminal_theme::TerminalTheme::default(),
                transient_default_color_owner_pgid: None,
                default_color_tracker: DefaultColorOscTracker::default(),
                default_color_event_tracker: DefaultColorEventTracker::default(),
                c1_xtgettcap_tracker: C1XtgettcapQueryTracker::default(),
                child_default_foreground_changed: false,
                child_default_background_changed: false,
                osc_debug_tracker: OscDebugTracker::default(),
                agent_osc_state: AgentOscStateTracker::default(),
                decscusr_tracker: DecscusrTracker::default(),
                cursor_settle_state: CursorPositionSettleState::default(),
                windows_powershell_prompt_cwd_reporting: false,
            }),
            key_encoder: Mutex::new(key_encoder),
            pending_pty_responses,
        })
    }

    pub(super) fn set_windows_powershell_prompt_cwd_reporting(&self, enabled: bool) {
        if let Ok(mut core) = self.core.lock() {
            core.windows_powershell_prompt_cwd_reporting = enabled;
        }
    }

    pub fn apply_host_terminal_theme(&self, theme: crate::terminal_theme::TerminalTheme) {
        if let Ok(mut core) = self.core.lock() {
            let foreground_unowned = !core.child_default_foreground_changed;
            let background_unowned = !core.child_default_background_changed;
            core.host_terminal_theme = theme;
            if foreground_unowned && background_unowned {
                core.transient_default_color_owner_pgid = None;
            }

            let mut palette = crate::ghostty::default_palette();
            for (index, color) in theme.palette.iter().enumerate() {
                if let Some(color) = color {
                    palette[index] = crate::ghostty::RgbColor {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                    };
                }
            }
            if let Err(err) = core.terminal.set_default_palette(&palette) {
                debug!(err = %err, "failed to apply host terminal palette");
            }

            write_host_terminal_theme_selective(
                &mut core.terminal,
                theme,
                foreground_unowned,
                background_unowned,
            );
        }
    }

    pub fn apply_host_terminal_appearance(
        &self,
        appearance: Option<crate::terminal_theme::HostAppearance>,
    ) -> Option<Bytes> {
        let mut core = self.core.lock().ok()?;
        let color_scheme = appearance.map(|appearance| match appearance {
            crate::terminal_theme::HostAppearance::Dark => crate::ghostty::ColorScheme::Dark,
            crate::terminal_theme::HostAppearance::Light => crate::ghostty::ColorScheme::Light,
        });
        let previous = core.terminal.set_color_scheme(color_scheme);

        let transitioned = matches!(
            (previous, color_scheme),
            (Some(previous), Some(current)) if previous != current
        );
        if !transitioned
            || !core
                .terminal
                .mode_get(crate::ghostty::MODE_COLOR_SCHEME_REPORT)
                .unwrap_or(false)
        {
            return None;
        }
        appearance.map(|appearance| Bytes::from_static(appearance.color_scheme_report()))
    }

    pub fn has_transient_default_color_override(&self) -> bool {
        self.core
            .lock()
            .map(|core| core.transient_default_color_owner_pgid.is_some())
            .unwrap_or(false)
    }

    pub fn maybe_restore_host_terminal_theme(&self, pane_id: PaneId, shell_pid: u32) -> bool {
        {
            let Ok(core) = self.core.lock() else {
                return false;
            };
            if !should_probe_host_terminal_theme_restore(&core) {
                return false;
            }
        }

        let foreground_job = crate::detect::foreground_job(shell_pid);
        let Ok(mut core) = self.core.lock() else {
            return false;
        };

        let alternate_screen = core
            .terminal
            .active_screen()
            .map(|screen| screen == crate::ghostty::ActiveScreen::Alternate)
            .unwrap_or(false);
        restore_host_terminal_theme_if_needed(
            &mut core,
            pane_id,
            shell_pid,
            alternate_screen,
            foreground_job.as_ref(),
        )
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.core
            .lock()
            .ok()
            .and_then(|core| core.agent_osc_state.terminal_title().map(str::to_string))
    }

    #[cfg(unix)]
    pub fn seed_terminal_title(&self, title: Option<String>) {
        if let Ok(mut core) = self.core.lock() {
            core.agent_osc_state.seed_terminal_title(title);
        }
    }

    /// Returns the latest OSC 0/2 title retained for agent detection, or `""`
    /// if no title has been seen or the last update was an empty clear.
    pub fn agent_osc_title(&self) -> String {
        self.core
            .lock()
            .map(|core| core.agent_osc_state.latest_title().to_owned())
            .unwrap_or_default()
    }

    /// Returns the latest OSC 9 progress payload retained for agent detection,
    /// or `""` if none has been seen.
    pub fn agent_osc_progress(&self) -> String {
        self.core
            .lock()
            .map(|core| core.agent_osc_state.latest_progress().to_owned())
            .unwrap_or_default()
    }

    /// Clears retained OSC title/progress evidence when the pane's foreground
    /// agent changes, so a new agent process starts from a blank OSC slate.
    pub fn clear_agent_osc_state(&self) {
        if let Ok(mut core) = self.core.lock() {
            core.agent_osc_state.clear_retained();
        }
    }

    pub fn process_pty_bytes(
        &self,
        pane_id: PaneId,
        shell_pid: u32,
        bytes: &[u8],
        _response_writer: &mpsc::Sender<Bytes>,
    ) -> ProcessBytesResult {
        let Ok(mut core) = self.core.lock() else {
            error!(pane = pane_id.raw(), "ghostty core lock poisoned in reader");
            return ProcessBytesResult {
                request_render: false,
                render_delay: None,
                terminal_title_changed: false,
                terminal_bells: 0,
                clipboard_writes: Vec::new(),
                reported_cwd: None,
                terminal_responses: Vec::new(),
            };
        };

        let _ = core.terminal.take_pwd_changes();
        // Restored history may have exercised terminal callbacks before this live PTY write.
        // Those effects must not be delivered as live pane output.
        let _ = core.terminal.take_bell_count();
        let _ = core.terminal.take_clipboard_writes();
        let default_color_observation = core.default_color_tracker.observe(bytes);
        if shell_pid > 0 && default_color_observation {
            if let Some(owner_pgid) = current_transient_default_color_owner(shell_pid) {
                core.transient_default_color_owner_pgid = Some(owner_pgid);
                debug!(
                    pane = pane_id.raw(),
                    owner_pgid, "tracked transient default color override"
                );
            }
        }

        core.osc_debug_tracker.observe(bytes);
        for event in core.osc_debug_tracker.drain_pending() {
            debug!(
                pane = pane_id.raw(),
                osc_command = %event.command,
                osc_payload = ?event.payload,
                "agent OSC evidence observed"
            );
        }
        let terminal_title_changed = core.agent_osc_state.observe(bytes);

        core.kitty_keyboard.observe(bytes);
        let mut terminal_responses = Vec::new();
        core.default_color_event_tracker.observe(bytes);
        core.c1_xtgettcap_tracker.observe(bytes);
        let c1_xtgettcap_responses = core.c1_xtgettcap_tracker.drain_pending();
        core.decscusr_tracker.observe(bytes);
        let in_progress_default_color_event = core.default_color_event_tracker.in_progress_event();
        let default_color_events = core.default_color_event_tracker.drain_pending();
        self.write_pty_bytes_with_ordered_responses(
            &mut core,
            bytes,
            default_color_events,
            in_progress_default_color_event,
            c1_xtgettcap_responses,
            &mut terminal_responses,
        );
        let terminal_bells = core.terminal.take_bell_count();
        let clipboard_writes = core.terminal.take_clipboard_writes();
        let reported_cwd = core
            .terminal
            .take_pwd_changes()
            .into_iter()
            .filter_map(|value| parse_reported_cwd(&value))
            .next_back();
        #[cfg(windows)]
        windows_recent_fallback::update_after_write(&mut core);

        let has_kitty_graphics_sequence = false && contains_kitty_graphics_sequence(bytes);
        if has_kitty_graphics_sequence {
            debug!(pane = pane_id.raw(), "processed kitty graphics sequence");
        }
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            key_encoder.set_from_terminal(&core.terminal);
        }
        let synchronized_output = core
            .terminal
            .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
            .unwrap_or(false);
        // Intermediate synchronized-frame positions must not become settled cursors.
        if CURSOR_POSITION_SETTLE_ENABLED && !synchronized_output {
            let cursor_after_write = current_cursor_state(&mut core);
            core.cursor_settle_state
                .observe(cursor_after_write, Instant::now());
        }
        #[cfg(windows)]
        let reported_cwd = if core.windows_powershell_prompt_cwd_reporting {
            reported_cwd.or_else(|| windows_powershell_current_prompt_cwd(&mut core))
        } else {
            reported_cwd
        };

        let request_render = !synchronized_output;
        let render_delay = render_delay_after_pty_write(
            synchronized_output,
            has_kitty_graphics_sequence,
            core.cursor_settle_state.render_delay(),
            CURSOR_POSITION_SETTLE_ENABLED,
        );
        ProcessBytesResult {
            request_render,
            render_delay,
            terminal_title_changed,
            terminal_bells,
            clipboard_writes,
            reported_cwd,
            terminal_responses,
        }
    }

    fn write_pty_bytes_with_ordered_responses(
        &self,
        core: &mut GhosttyPaneCore,
        bytes: &[u8],
        default_color_events: Vec<DefaultColorTrackedEvent>,
        in_progress_default_color_event: Option<DefaultColorEvent>,
        c1_xtgettcap_responses: Vec<C1XtgettcapResponse>,
        terminal_responses: &mut Vec<Bytes>,
    ) {
        // Only legacy C1 replies need merging; ordinary XTGETTCAP remains native.
        let mut events: Vec<_> = default_color_events
            .into_iter()
            .map(OrderedColorOrC1Event::Color)
            .chain(
                c1_xtgettcap_responses
                    .into_iter()
                    .map(OrderedColorOrC1Event::C1),
            )
            .collect();
        events.sort_by_key(OrderedColorOrC1Event::end_offset);
        let mut written = 0;
        for event in events {
            let end_offset = event.end_offset().min(bytes.len());
            if matches!(&event, OrderedColorOrC1Event::C1(response) if response.suppress_native) {
                // Suppress only the unhook byte's replies, not earlier native queries.
                let prefix_end = end_offset.saturating_sub(1).max(written);
                if prefix_end > written {
                    core.terminal.write(&bytes[written..prefix_end]);
                    terminal_responses.extend(self.drain_pending_pty_responses());
                    written = prefix_end;
                }
            }
            let mut libghostty_responses = Vec::new();
            if end_offset > written {
                core.terminal.write(&bytes[written..end_offset]);
                libghostty_responses = self.drain_pending_pty_responses();
                written = end_offset;
            }
            match event {
                OrderedColorOrC1Event::Color(event) => {
                    let replacement = respond_to_default_color_event(core, event.event);
                    if replacement.is_some() {
                        remove_last_matching_libghostty_color_reply(
                            &mut libghostty_responses,
                            event.event,
                        );
                    }
                    terminal_responses.extend(libghostty_responses);
                    terminal_responses.extend(replacement);
                }
                OrderedColorOrC1Event::C1(response) => {
                    if response.suppress_native {
                        libghostty_responses.retain(|reply| {
                            !reply.starts_with(b"\x1bP1+r") && !reply.starts_with(b"\x1bP0+r")
                        });
                    }
                    terminal_responses.extend(libghostty_responses);
                    if !response.suppress_native {
                        terminal_responses.push(response.bytes);
                    }
                }
            }
        }

        if written < bytes.len() {
            core.terminal.write(&bytes[written..]);
            let mut libghostty_responses = self.drain_pending_pty_responses();
            if let Some(event) = in_progress_default_color_event {
                if default_color_event_response(core, event).is_some() {
                    remove_last_matching_libghostty_color_reply(&mut libghostty_responses, event);
                }
            }
            terminal_responses.extend(libghostty_responses);
        }

        if !core.child_default_foreground_changed && !core.child_default_background_changed {
            core.transient_default_color_owner_pgid = None;
        }
    }

    fn drain_pending_pty_responses(&self) -> Vec<Bytes> {
        self.pending_pty_responses
            .lock()
            .map(|mut responses| std::mem::take(&mut *responses))
            .unwrap_or_default()
    }

    pub fn seed_history_ansi(&self, ansi: &str) {
        if ansi.is_empty() {
            return;
        }
        let Ok(mut core) = self.core.lock() else {
            return;
        };
        core.kitty_keyboard.observe(ansi.as_bytes());
        core.terminal.write(ansi.as_bytes());
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            key_encoder.set_from_terminal(&core.terminal);
        }
    }

    pub fn seed_handoff_input_state(&self, input_state: InputState) {
        let Ok(mut core) = self.core.lock() else {
            return;
        };

        if input_state.alternate_screen {
            core.terminal.write(b"\x1b[?1049h");
        }
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_APPLICATION_CURSOR_KEYS,
            input_state.application_cursor,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_BRACKETED_PASTE,
            input_state.bracketed_paste,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_FOCUS_EVENT,
            input_state.focus_reporting,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL,
            input_state.mouse_alternate_scroll,
        );
        let _ = core.terminal.mode_set(
            crate::ghostty::MODE_COLOR_SCHEME_REPORT,
            input_state.color_scheme_reporting,
        );

        core.terminal
            .write(b"\x1b[?9l\x1b[?1000l\x1b[?1002l\x1b[?1003l");
        let mouse_mode_ansi: Option<&[u8]> = match input_state.mouse_protocol_mode {
            crate::input::MouseProtocolMode::None => None,
            crate::input::MouseProtocolMode::Press => Some(b"\x1b[?9h"),
            crate::input::MouseProtocolMode::PressRelease => Some(b"\x1b[?1000h"),
            crate::input::MouseProtocolMode::ButtonMotion => Some(b"\x1b[?1002h"),
            crate::input::MouseProtocolMode::AnyMotion => Some(b"\x1b[?1003h"),
        };
        if let Some(ansi) = mouse_mode_ansi {
            core.terminal.write(ansi);
        }

        core.terminal.write(b"\x1b[?1005l\x1b[?1006l\x1b[?1016l");
        let mouse_encoding_ansi: Option<&[u8]> = match input_state.mouse_protocol_encoding {
            crate::input::MouseProtocolEncoding::Default => None,
            crate::input::MouseProtocolEncoding::Utf8 => Some(b"\x1b[?1005h"),
            crate::input::MouseProtocolEncoding::Sgr => Some(b"\x1b[?1006h"),
            crate::input::MouseProtocolEncoding::SgrPixels => Some(b"\x1b[?1016h"),
        };
        if let Some(ansi) = mouse_encoding_ansi {
            core.terminal.write(ansi);
        }

        if input_state.modify_other_keys {
            core.kitty_keyboard.observe(b"\x1b[>4;2m");
            core.terminal.write(b"\x1b[>4;2m");
        }

        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            key_encoder.set_from_terminal(&core.terminal);
        }
    }

    pub fn seed_keyboard_protocol_flags(&self, flags: u16) {
        if flags == 0 {
            return;
        }
        self.seed_keyboard_protocol_ansi(&format!("\x1b[>{flags}u"));
    }

    pub fn seed_keyboard_protocol_ansi(&self, ansi: &str) {
        if ansi.is_empty() {
            return;
        }
        let Ok(mut core) = self.core.lock() else {
            return;
        };
        core.kitty_keyboard.observe(ansi.as_bytes());
        core.terminal.write(ansi.as_bytes());
        if let Ok(mut key_encoder) = self.key_encoder.lock() {
            key_encoder.set_from_terminal(&core.terminal);
        }
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Vec<Bytes> {
        if let Ok(mut core) = self.core.lock() {
            let offset_from_bottom = core
                .terminal
                .scrollbar()
                .ok()
                .map(|scrollbar| {
                    scrollbar
                        .total
                        .saturating_sub(scrollbar.offset + scrollbar.len)
                })
                .unwrap_or(0);
            let bottom_before_resize = ghostty_detection_text(&mut core)
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false);
            let resize_recovery_probe_lines = usize::from(rows)
                .saturating_mul(8)
                .max(DEFAULT_DETECTION_ROWS);
            let replay_ansi = if core.terminal.active_screen().ok()
                == Some(crate::ghostty::ActiveScreen::Primary)
                && bottom_before_resize
            {
                ghostty_recent_ansi(&mut core, resize_recovery_probe_lines, true)
                    .ok()
                    .filter(|ansi| !ansi.trim().is_empty())
            } else {
                None
            };

            let _ = core
                .terminal
                .resize(cols, rows, cell_width_px, cell_height_px);
            let terminal_responses = self.drain_pending_pty_responses();

            let bottom_is_blank = ghostty_detection_text(&mut core)
                .map(|text| text.trim().is_empty())
                .unwrap_or(false);
            if bottom_is_blank {
                if let Some(ansi) = replay_ansi.as_deref() {
                    core.terminal.scroll_viewport_bottom();
                    core.terminal.write(ansi.as_bytes());
                }
            }
            ghostty_set_scroll_offset_from_bottom(&mut core.terminal, offset_from_bottom);
            if offset_from_bottom > 0 {
                let mut remaining = offset_from_bottom.min(resize_recovery_probe_lines);
                while remaining > 0
                    && ghostty_visible_text(&mut core)
                        .map(|text| text.trim().is_empty())
                        .unwrap_or(false)
                {
                    core.terminal.scroll_viewport_delta(1);
                    remaining -= 1;
                }
            }
            terminal_responses
        } else {
            Vec::new()
        }
    }

    pub fn scroll_up(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_delta(-(lines as isize));
        }
    }

    pub fn scroll_down(&self, lines: usize) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_delta(lines as isize);
        }
    }

    pub fn scroll_reset(&self) {
        if let Ok(mut core) = self.core.lock() {
            core.terminal.scroll_viewport_bottom();
        }
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let scrollbar = core.terminal.scrollbar().ok()?;
        Some(ScrollMetrics {
            offset_from_bottom: scrollbar
                .total
                .saturating_sub(scrollbar.offset + scrollbar.len),
            max_offset_from_bottom: scrollbar.total.saturating_sub(scrollbar.len),
            viewport_rows: scrollbar.len,
        })
    }

    pub fn keyboard_protocol(&self) -> Option<crate::input::KeyboardProtocol> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        Some(crate::input::KeyboardProtocol::from_kitty_flags(
            core.terminal.kitty_keyboard_flags().ok()? as u16,
        ))
    }

    pub fn kitty_keyboard_state_ansi(&self) -> Option<String> {
        let core = self.core.lock().ok()?;
        core.kitty_keyboard.replay_ansi()
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.mode_enabled(crate::ghostty::MODE_BRACKETED_PASTE)
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.core
            .lock()
            .is_ok_and(|core| core.terminal.mouse_tracking_enabled().unwrap_or(false))
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.core
            .lock()
            .map_or(0, |core| core.kitty_keyboard.modify_other_keys_level())
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.mode_enabled(crate::ghostty::MODE_MOUSE_SGR_PIXELS)
    }

    fn mode_enabled(&self, mode: u16) -> bool {
        self.core
            .lock()
            .is_ok_and(|core| core.terminal.mode_get(mode).unwrap_or(false))
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        let core = self.core.lock().ok()?;
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let mouse_reporting = core.terminal.mouse_tracking_enabled().ok()?;
        let application_cursor = core
            .terminal
            .mode_get(crate::ghostty::MODE_APPLICATION_CURSOR_KEYS)
            .ok()?;
        let bracketed_paste = core
            .terminal
            .mode_get(crate::ghostty::MODE_BRACKETED_PASTE)
            .ok()?;
        Some(!alternate_screen && !mouse_reporting && (!application_cursor || bracketed_paste))
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.core.lock().is_ok_and(|core| {
            core.terminal.active_screen().ok() == Some(crate::ghostty::ActiveScreen::Alternate)
        })
    }

    // This aggregate snapshot performs multiple terminal queries. Pane-scaled
    // callers should add a narrow accessor instead.
    #[cfg(any(unix, test))]
    pub fn input_state(&self) -> Option<InputState> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let application_cursor = core
            .terminal
            .mode_get(crate::ghostty::MODE_APPLICATION_CURSOR_KEYS)
            .ok()?;
        let bracketed_paste = core
            .terminal
            .mode_get(crate::ghostty::MODE_BRACKETED_PASTE)
            .ok()?;
        let focus_reporting = core
            .terminal
            .mode_get(crate::ghostty::MODE_FOCUS_EVENT)
            .ok()?;
        let mouse_sgr = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_SGR)
            .ok()?;
        let mouse_utf8 = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_UTF8)
            .ok()?;
        let mouse_sgr_pixels = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_SGR_PIXELS)
            .ok()?;
        let mouse_alternate_scroll = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL)
            .ok()?;
        let mouse_protocol_mode = if core.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()? {
            crate::input::MouseProtocolMode::AnyMotion
        } else if core.terminal.mode_get(MODE_MOUSE_BUTTON_MOTION).ok()? {
            crate::input::MouseProtocolMode::ButtonMotion
        } else if core.terminal.mode_get(MODE_MOUSE_PRESS_RELEASE).ok()? {
            crate::input::MouseProtocolMode::PressRelease
        } else if core.terminal.mode_get(MODE_MOUSE_X10).ok()? {
            crate::input::MouseProtocolMode::Press
        } else {
            crate::input::MouseProtocolMode::None
        };
        let mouse_protocol_encoding = if mouse_sgr_pixels {
            crate::input::MouseProtocolEncoding::SgrPixels
        } else if mouse_sgr {
            crate::input::MouseProtocolEncoding::Sgr
        } else if mouse_utf8 {
            crate::input::MouseProtocolEncoding::Utf8
        } else {
            crate::input::MouseProtocolEncoding::Default
        };
        Some(InputState {
            alternate_screen,
            application_cursor,
            bracketed_paste,
            focus_reporting,
            mouse_protocol_mode,
            mouse_protocol_encoding,
            mouse_alternate_scroll,
            modify_other_keys: core.terminal.modify_other_keys_enabled().ok()?,
            color_scheme_reporting: core
                .terminal
                .mode_get(crate::ghostty::MODE_COLOR_SCHEME_REPORT)
                .ok()?,
        })
    }

    pub fn wheel_routing(&self) -> Option<crate::pane::WheelRouting> {
        let Ok(core) = self.core.lock() else {
            return None;
        };
        let alternate_screen =
            core.terminal.active_screen().ok()? == crate::ghostty::ActiveScreen::Alternate;
        let mouse_alternate_scroll = core
            .terminal
            .mode_get(crate::ghostty::MODE_MOUSE_ALTERNATE_SCROLL)
            .ok()?;
        let mouse_reporting = core.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()?
            || core.terminal.mode_get(MODE_MOUSE_BUTTON_MOTION).ok()?
            || core.terminal.mode_get(MODE_MOUSE_PRESS_RELEASE).ok()?
            || core.terminal.mode_get(MODE_MOUSE_X10).ok()?;
        Some(if mouse_reporting {
            crate::pane::WheelRouting::MouseReport
        } else if alternate_screen && mouse_alternate_scroll {
            crate::pane::WheelRouting::AlternateScroll
        } else {
            crate::pane::WheelRouting::HostScroll
        })
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        let mut core = self.core.lock().ok()?;
        let current = current_cursor_state(&mut core);
        effective_cursor_state(&mut core, current)
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.core
            .lock()
            .ok()
            .and_then(|core| {
                core.terminal
                    .mode_get(crate::ghostty::MODE_SYNCHRONIZED_OUTPUT)
                    .ok()
            })
            .unwrap_or(false)
    }

    pub fn encode_terminal_key(
        &self,
        key: crate::input::TerminalKey,
        protocol: crate::input::KeyboardProtocol,
    ) -> Vec<u8> {
        let repeat_count = key.repeat_count;
        let first = key.with_repeat_count(1);
        let mut bytes = self.encode_terminal_key_once(first.clone(), protocol);
        if repeat_count > 1 && first.kind != crossterm::event::KeyEventKind::Release {
            let repeated = first.with_kind(crossterm::event::KeyEventKind::Repeat);
            let repeated_bytes = self.encode_terminal_key_once(repeated, protocol);
            for _ in 1..repeat_count {
                bytes.extend_from_slice(&repeated_bytes);
            }
        }
        bytes
    }

    fn encode_terminal_key_once(
        &self,
        key: crate::input::TerminalKey,
        protocol: crate::input::KeyboardProtocol,
    ) -> Vec<u8> {
        // Ghostty emits extended Enter sequences even without negotiation.
        // Ordinary shells need legacy input unless the child enabled an extension.
        if key.code == crossterm::event::KeyCode::Enter
            && !key.modifiers.is_empty()
            && self.core.lock().is_ok_and(|core| {
                core.terminal
                    .kitty_keyboard_flags()
                    .is_ok_and(|flags| flags == 0)
                    && core.kitty_keyboard.modify_other_keys_level() == 0
                    && core
                        .terminal
                        .modify_other_keys_enabled()
                        .is_ok_and(|enabled| !enabled)
            })
        {
            return crate::input::encode_terminal_key(key, crate::input::KeyboardProtocol::Legacy);
        }

        if matches!(protocol, crate::input::KeyboardProtocol::Legacy)
            && key.code == crossterm::event::KeyCode::Tab
            && key.modifiers == crossterm::event::KeyModifiers::CONTROL
        {
            return crate::input::encode_terminal_key(key, protocol);
        }

        if ghostty_prefers_sesh_mux_text_encoding(&key) {
            return crate::input::encode_terminal_key(key, protocol);
        }

        let Some(event) = ghostty_key_event_from_terminal_key(&key) else {
            return crate::input::encode_terminal_key(key, protocol);
        };

        let Ok(mut encoder) = self.key_encoder.lock() else {
            return crate::input::encode_terminal_key(key, protocol);
        };
        match encoder.encode(&event) {
            Ok(bytes)
                if !bytes.is_empty()
                    && encoded_key_preserves_event_kind(&bytes, &key, protocol) =>
            {
                bytes
            }
            Ok(_) | Err(_) => crate::input::encode_terminal_key(key, protocol),
        }
    }

    pub(crate) fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_button_kind(kind, 0, 0, modifiers)?,
            position,
            false,
        )
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_motion_kind(kind, 0, 0, modifiers)?,
            position,
            true,
        )
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.encode_mouse_event(
            ghostty_mouse_event_from_wheel_kind(kind, 0, 0, modifiers)?,
            position,
            false,
        )
    }

    fn encode_mouse_event(
        &self,
        mut event: crate::ghostty::MouseEvent,
        position: crate::input::mouse::Position,
        require_any_motion: bool,
    ) -> Option<Vec<u8>> {
        let core = self.core.lock().ok()?;
        if require_any_motion && !core.terminal.mode_get(MODE_MOUSE_ANY_MOTION).ok()? {
            return None;
        }
        let mut encoder = ghostty_mouse_encoder_for_terminal(&core.terminal, position)?;
        let (x, y) = ghostty_mouse_position_for_terminal(position)?;
        event.set_position(x, y);
        encoder
            .encode(&event)
            .ok()
            .filter(|bytes| !bytes.is_empty())
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        u16,
        Vec<crate::ghostty::ScreenTextRow>,
    )> {
        let core = self.core.lock().ok()?;
        Some((
            core.terminal.active_screen().ok()?,
            core.terminal.cols().ok()?,
            core.terminal.screen_text_rows().ok()?,
        ))
    }

    pub fn visible_text(&self) -> String {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_visible_text(&mut core).ok())
            .unwrap_or_default()
    }

    pub fn visible_ansi(&self) -> String {
        self.core
            .lock()
            .ok()
            .and_then(|core| ghostty_visible_ansi(&core).ok())
            .unwrap_or_default()
    }

    pub fn detection_text(&self) -> String {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_detection_text(&mut core).ok())
            .unwrap_or_default()
    }

    fn try_lock_core(&self) -> Option<std::sync::MutexGuard<'_, GhosttyPaneCore>> {
        match self.core.try_lock() {
            Ok(core) => Some(core),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        }
    }

    pub(crate) fn try_compression_activity(&self) -> Result<Option<u64>, crate::ghostty::Error> {
        let Some(core) = self.try_lock_core() else {
            return Ok(None);
        };
        core.terminal.compression_activity().map(Some)
    }

    pub(crate) fn try_compress_incremental_if_activity(
        &self,
        expected_activity: u64,
    ) -> Result<TerminalCompressionStep, crate::ghostty::Error> {
        let Some(mut core) = self.try_lock_core() else {
            return Ok(TerminalCompressionStep::Busy);
        };
        let activity = core.terminal.compression_activity()?;
        if activity != expected_activity {
            return Ok(TerminalCompressionStep::ActivityChanged(activity));
        }
        core.terminal
            .compress_incremental()
            .map(TerminalCompressionStep::Compressed)
    }

    #[cfg(test)]
    pub fn recent_text(&self, lines: usize) -> String {
        self.recent_text_snapshot(lines).text
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_recent_text_snapshot(&mut core, lines).ok())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn recent_ansi(&self, lines: usize) -> String {
        self.recent_ansi_snapshot(lines).text
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_recent_ansi_snapshot(&mut core, lines, false).ok())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub fn recent_unwrapped_text(&self, lines: usize) -> String {
        self.recent_unwrapped_text_snapshot(lines).text
    }

    pub(crate) fn recent_unwrapped_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_recent_text_unwrapped_snapshot(&mut core, lines).ok())
            .unwrap_or_default()
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        self.recent_unwrapped_ansi_snapshot(lines).text
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_recent_ansi_snapshot(&mut core, lines, true).ok())
            .unwrap_or_default()
    }

    pub fn extract_selection<P>(
        &self,
        selection: &crate::selection::Selection<P>,
    ) -> Option<String> {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_extract_selection(&mut core, selection).ok())
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.core
            .lock()
            .ok()
            .and_then(|mut core| ghostty_visible_hyperlinks(&mut core, area).ok())
            .unwrap_or_default()
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        let Ok(mut core) = self.core.lock() else {
            return;
        };
        let host_theme = core.host_terminal_theme;
        let initial_default_foreground = core.initial_default_foreground;
        let initial_default_background = core.initial_default_background;
        let GhosttyPaneCore {
            terminal,
            render_state,
            decscusr_tracker,
            ..
        } = &mut *core;
        if render_state.update(terminal).is_err() {
            return;
        }
        let colors = render_state.colors().ok();
        let default_bg = colors
            .and_then(|c| ghostty_default_bg(c.background, host_theme, initial_default_background));
        let default_fg = colors
            .and_then(|c| ghostty_default_fg(c.foreground, host_theme, initial_default_foreground));
        let resolved_fg = colors.map(|c| ghostty_color(c.foreground));
        let resolved_bg = colors.map(|c| ghostty_color(c.background));
        let palette_overrides = colors
            .zip(terminal.default_palette().ok())
            .and_then(|(colors, default)| PaletteOverrides::new(&colors.palette, &default));
        let hide_kitty_placeholders = false;

        let mut row_iterator = match crate::ghostty::RowIterator::new() {
            Ok(iterator) => iterator,
            Err(_) => return,
        };
        let mut row_cells = match crate::ghostty::RowCells::new() {
            Ok(cells) => cells,
            Err(_) => return,
        };
        {
            let buf = frame.buffer_mut();
            let mut rows = match render_state.populate_row_iterator(&mut row_iterator) {
                Ok(rows) => rows,
                Err(_) => return,
            };
            let mut grapheme_bytes = Vec::new();
            let mut symbol_scratch = String::new();
            let mut y = 0u16;
            while y < area.height && rows.next() {
                let mut cells = match rows.populate_cells(&mut row_cells) {
                    Ok(cells) => cells,
                    Err(_) => break,
                };
                let mut x = 0u16;
                while x < area.width && cells.next() {
                    let basic = cells.basic_data().unwrap_or_default();
                    let style = ghostty_cell_style(
                        &cells,
                        &basic,
                        default_fg,
                        default_bg,
                        resolved_fg,
                        resolved_bg,
                        palette_overrides.as_ref(),
                    );
                    let symbol = match ghostty_buffer_symbol_into(
                        &cells,
                        basic.wide,
                        hide_kitty_placeholders,
                        &mut grapheme_bytes,
                        &mut symbol_scratch,
                    ) {
                        Ok(symbol) => symbol,
                        Err(_) => {
                            symbol_scratch.clear();
                            symbol_scratch.push_str(ghostty_blank_symbol_for_width(basic.wide));
                            symbol_scratch.as_str()
                        }
                    };
                    let cell = &mut buf[(area.x + x, area.y + y)];
                    cell.reset();
                    cell.set_symbol(symbol);
                    cell.set_style(style);
                    x += 1;
                }
                while x < area.width {
                    let cell = &mut buf[(area.x + x, area.y + y)];
                    ghostty_reset_cell(cell, default_fg, default_bg);
                    x += 1;
                }
                y += 1;
            }
            while y < area.height {
                for x in 0..area.width {
                    let cell = &mut buf[(area.x + x, area.y + y)];
                    ghostty_reset_cell(cell, default_fg, default_bg);
                }
                y += 1;
            }
        }

        ghostty_clear_render_dirty(render_state, area.height);

        let current_cursor = cursor_state_from_render_state(render_state, decscusr_tracker);
        if show_cursor {
            if let Some(cursor) =
                effective_cursor_state(&mut core, current_cursor).filter(|cursor| cursor.visible)
            {
                if cursor.x < area.width && cursor.y < area.height {
                    frame.set_cursor_position((area.x + cursor.x, area.y + cursor.y));
                }
            }
        }
    }
}

fn encoded_key_preserves_event_kind(
    bytes: &[u8],
    key: &crate::input::TerminalKey,
    protocol: crate::input::KeyboardProtocol,
) -> bool {
    if !protocol.reports_event_types() || key.kind == crossterm::event::KeyEventKind::Press {
        return true;
    }

    std::str::from_utf8(bytes)
        .ok()
        .and_then(crate::input::parse_terminal_key_sequence)
        .is_some_and(|parsed| {
            parsed.code == key.code && parsed.modifiers == key.modifiers && parsed.kind == key.kind
        })
}

fn effective_cursor_state(
    core: &mut GhosttyPaneCore,
    current: Option<TerminalCursorState>,
) -> Option<TerminalCursorState> {
    if !CURSOR_POSITION_SETTLE_ENABLED {
        return current;
    }
    core.cursor_settle_state
        .reported_cursor(current, Instant::now())
}

fn render_delay_after_pty_write(
    synchronized_output: bool,
    has_kitty_graphics_sequence: bool,
    cursor_position_settle_delay: Option<Duration>,
    cursor_position_settle_enabled: bool,
) -> Option<Duration> {
    if synchronized_output {
        None
    } else {
        let cursor_delay = cursor_position_settle_enabled
            .then_some(cursor_position_settle_delay)
            .flatten();
        cursor_delay.max(has_kitty_graphics_sequence.then_some(KITTY_GRAPHICS_REDRAW_SETTLE))
    }
}

fn current_cursor_state(core: &mut GhosttyPaneCore) -> Option<TerminalCursorState> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        decscusr_tracker,
        ..
    } = core;
    render_state.update(terminal).ok()?;
    cursor_state_from_render_state(render_state, decscusr_tracker)
}

fn cursor_state_from_render_state(
    render_state: &mut crate::ghostty::RenderState,
    decscusr_tracker: &DecscusrTracker,
) -> Option<TerminalCursorState> {
    let cursor = render_state.cursor().ok()?;
    let viewport = cursor.viewport?;
    let shape = if decscusr_tracker.cursor_shape_overridden() {
        decscusr_cursor_shape(cursor.visual_style, cursor.blinking)
    } else {
        0
    };
    Some(TerminalCursorState {
        x: viewport.x,
        y: viewport.y,
        visible: cursor.visible,
        shape,
    })
}

type VisibleHyperlinks = Vec<((u16, u16), String, String)>;

fn ghostty_clear_render_dirty(render_state: &mut crate::ghostty::RenderState, area_height: u16) {
    if render_state.rows().is_ok_and(|rows| area_height >= rows) && render_state.clean().is_ok() {
        return;
    }
    let Ok(mut row_iterator) = crate::ghostty::RowIterator::new() else {
        return;
    };
    let Ok(mut rows) = render_state.populate_row_iterator(&mut row_iterator) else {
        return;
    };
    let mut y = 0u16;
    while y < area_height && rows.next() {
        let _ = rows.clear_dirty();
        y += 1;
    }
    let _ = render_state.set_dirty(crate::ghostty::Dirty::Clean);
}

fn ghostty_visible_hyperlinks(
    core: &mut GhosttyPaneCore,
    area: Rect,
) -> Result<VisibleHyperlinks, crate::ghostty::Error> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    render_state.update(terminal)?;
    let mut row_iterator = crate::ghostty::RowIterator::new()?;
    let mut row_cells = crate::ghostty::RowCells::new()?;
    let mut rows = render_state.populate_row_iterator(&mut row_iterator)?;
    let mut links = Vec::new();
    let mut y = 0u16;
    while y < area.height && rows.next() {
        let mut cells = rows.populate_cells(&mut row_cells)?;
        let mut x = 0u16;
        while x < area.width && cells.next() {
            if cells.has_hyperlink()? {
                if let Some(uri) = terminal.viewport_hyperlink_uri(x, y.into())? {
                    links.push(((area.x + x, area.y + y), ghostty_cell_symbol(&cells)?, uri));
                }
            }
            x += 1;
        }
        y += 1;
    }
    Ok(links)
}

fn ghostty_visible_text(core: &mut GhosttyPaneCore) -> Result<String, crate::ghostty::Error> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    render_state.update(terminal)?;
    let mut row_iterator = crate::ghostty::RowIterator::new()?;
    let mut row_cells = crate::ghostty::RowCells::new()?;
    let mut rows = render_state.populate_row_iterator(&mut row_iterator)?;
    let mut lines = Vec::new();
    while rows.next() {
        let mut cells = rows.populate_cells(&mut row_cells)?;
        lines.push(ghostty_line_from_cells(&mut cells)?);
    }
    trim_trailing_blank_rows(&mut lines);
    Ok(lines_to_text(lines))
}

fn ghostty_visible_ansi(core: &GhosttyPaneCore) -> Result<String, crate::ghostty::Error> {
    let rows = core.terminal.rows()?;
    let cols = core.terminal.cols()?;
    if rows == 0 || cols == 0 {
        return Ok(String::new());
    }
    core.terminal.read_ansi_viewport(
        (0, 0),
        (cols.saturating_sub(1), u32::from(rows.saturating_sub(1))),
        false,
    )
}

fn ghostty_detection_text(core: &mut GhosttyPaneCore) -> Result<String, crate::ghostty::Error> {
    let lines = core
        .terminal
        .rows()
        .ok()
        .map(|rows| usize::from(rows).max(1))
        .unwrap_or(DEFAULT_DETECTION_ROWS);
    ghostty_recent_text(core, lines)
}

fn ghostty_recent_text(
    core: &mut GhosttyPaneCore,
    lines: usize,
) -> Result<String, crate::ghostty::Error> {
    ghostty_recent_text_snapshot(core, lines).map(|snapshot| snapshot.text)
}

fn ghostty_recent_text_snapshot(
    core: &mut GhosttyPaneCore,
    lines: usize,
) -> Result<TerminalReadSnapshot, crate::ghostty::Error> {
    let text = ghostty_recent_text_for_terminal(&core.terminal, lines)?;
    Ok(finish_recent_snapshot(core, text, lines, false))
}

fn ghostty_recent_text_unwrapped_snapshot(
    core: &mut GhosttyPaneCore,
    lines: usize,
) -> Result<TerminalReadSnapshot, crate::ghostty::Error> {
    let text = ghostty_recent_text_unwrapped_for_terminal(&core.terminal, lines)?;
    Ok(finish_recent_snapshot(core, text, lines, true))
}

fn ghostty_recent_ansi(
    core: &mut GhosttyPaneCore,
    lines: usize,
    unwrap: bool,
) -> Result<String, crate::ghostty::Error> {
    ghostty_recent_ansi_snapshot(core, lines, unwrap).map(|snapshot| snapshot.text)
}

fn ghostty_recent_ansi_snapshot(
    core: &mut GhosttyPaneCore,
    lines: usize,
    unwrap: bool,
) -> Result<TerminalReadSnapshot, crate::ghostty::Error> {
    let text = ghostty_recent_ansi_for_terminal(&core.terminal, lines, unwrap)?;
    Ok(finish_recent_snapshot(core, text, lines, unwrap))
}

fn finish_recent_snapshot(
    core: &mut GhosttyPaneCore,
    text: String,
    lines: usize,
    unwrap: bool,
) -> TerminalReadSnapshot {
    let _ = unwrap;

    // Recent read limits are measured in rendered rows, including blank or styled rows.
    TerminalReadSnapshot {
        text,
        truncated: core
            .terminal
            .total_rows()
            .is_ok_and(|total_rows| total_rows > lines),
    }
}

fn ghostty_recent_text_for_terminal(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<String, crate::ghostty::Error> {
    let Some((start, end, cols)) = ghostty_recent_read_range(terminal, lines)? else {
        return Ok(String::new());
    };
    let mut rows = Vec::with_capacity(end.saturating_sub(start).saturating_add(1));
    for y in start..=end {
        rows.push(ghostty_screen_row(terminal, cols, y as u32)?);
    }
    trim_trailing_blank_rows(&mut rows);
    Ok(recent_text_from_rows(&rows, lines))
}

fn ghostty_recent_text_unwrapped_for_terminal(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<String, crate::ghostty::Error> {
    let Some((start, end, cols)) = ghostty_recent_read_range(terminal, lines)? else {
        return Ok(String::new());
    };
    terminal.read_text_screen(
        (0, start as u32),
        (cols.saturating_sub(1), end as u32),
        false,
    )
}

fn ghostty_recent_ansi_for_terminal(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
    unwrap: bool,
) -> Result<String, crate::ghostty::Error> {
    let Some((start, end, cols)) = ghostty_recent_read_range(terminal, lines)? else {
        return Ok(String::new());
    };
    terminal.read_ansi_screen(
        (0, start as u32),
        (cols.saturating_sub(1), end as u32),
        false,
        unwrap,
    )
}

fn ghostty_recent_read_range(
    terminal: &crate::ghostty::Terminal,
    lines: usize,
) -> Result<Option<(usize, usize, u16)>, crate::ghostty::Error> {
    let total_rows = terminal.total_rows()?;
    let cols = terminal.cols()?;
    if total_rows == 0 || cols == 0 || lines == 0 {
        return Ok(None);
    }

    let physical_end = total_rows.saturating_sub(1);
    if terminal.active_screen()? != crate::ghostty::ActiveScreen::Primary {
        let start = physical_end.saturating_add(1).saturating_sub(lines);
        return Ok(Some((start, physical_end, cols)));
    }

    let rows = usize::from(terminal.rows()?);
    if rows == 0 {
        return Ok(None);
    }
    let viewport_start = total_rows.saturating_sub(rows);
    let cursor_row = viewport_start
        .saturating_add(usize::from(terminal.cursor_y()?))
        .min(total_rows.saturating_sub(1));
    let mut last_content_row = None;
    for row in (viewport_start..total_rows).rev() {
        if !ghostty_screen_row(terminal, cols, row as u32)?
            .trim()
            .is_empty()
        {
            last_content_row = Some(row);
            break;
        }
    }
    let end = last_content_row
        .map(|row| row.max(cursor_row))
        .unwrap_or_else(|| total_rows.saturating_sub(1));
    let start = end.saturating_add(1).saturating_sub(lines);
    Ok(Some((start, end, cols)))
}

fn ghostty_set_scroll_offset_from_bottom(
    terminal: &mut crate::ghostty::Terminal,
    offset_from_bottom: usize,
) {
    let Ok(scrollbar) = terminal.scrollbar() else {
        terminal.scroll_viewport_bottom();
        return;
    };
    let max_offset = scrollbar.total.saturating_sub(scrollbar.len);
    let offset_from_bottom = offset_from_bottom.min(max_offset);
    if offset_from_bottom == 0 {
        terminal.scroll_viewport_bottom();
    } else {
        terminal.scroll_viewport_row(max_offset - offset_from_bottom);
    }
}

fn ghostty_extract_selection<P>(
    core: &mut GhosttyPaneCore,
    selection: &crate::selection::Selection<P>,
) -> Result<String, crate::ghostty::Error> {
    let ((start_row, start_col), (end_row, end_col)) = selection.ordered_cells();
    core.terminal
        .read_text_screen((start_col, start_row), (end_col, end_row), false)
}

fn ghostty_screen_row(
    terminal: &crate::ghostty::Terminal,
    cols: u16,
    y: u32,
) -> Result<String, crate::ghostty::Error> {
    let mut line = String::new();
    for x in 0..cols {
        let (wide, graphemes) = terminal.screen_cell(x, y)?;
        if wide == crate::ghostty::CellWide::SpacerTail {
            continue;
        }
        if graphemes.is_empty()
            || graphemes.first().copied() == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER)
        {
            line.push(' ');
        } else {
            for codepoint in graphemes {
                if let Some(ch) = char::from_u32(codepoint) {
                    line.push(ch);
                }
            }
        }
    }
    Ok(line.trim_end().to_string())
}

fn ghostty_line_from_cells(
    cells: &mut crate::ghostty::RowCellIter<'_>,
) -> Result<String, crate::ghostty::Error> {
    let mut line = String::new();
    while cells.next() {
        line.push_str(&ghostty_cell_symbol(cells)?);
    }
    Ok(line.trim_end().to_string())
}

fn ghostty_cell_symbol(
    cells: &crate::ghostty::RowCellIter<'_>,
) -> Result<String, crate::ghostty::Error> {
    if cells.wide()? == crate::ghostty::CellWide::SpacerTail {
        return Ok(String::new());
    }
    let text = cells.grapheme_text()?;
    if text.chars().next().map(u32::from) == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER) {
        return Ok(" ".to_string());
    }
    if text.is_empty() {
        return Ok(" ".to_string());
    }
    Ok(text)
}

pub(super) fn ghostty_blank_symbol_for_width(wide: crate::ghostty::CellWide) -> &'static str {
    match wide {
        crate::ghostty::CellWide::Wide => "  ",
        crate::ghostty::CellWide::SpacerTail => "",
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::SpacerHead => " ",
    }
}

#[cfg(test)]
pub(super) fn ghostty_normalize_buffer_symbol(
    symbol: &str,
    wide: crate::ghostty::CellWide,
) -> String {
    let expected_width = match wide {
        crate::ghostty::CellWide::Wide => 2,
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::SpacerHead => 1,
        crate::ghostty::CellWide::SpacerTail => 0,
    };
    let actual_width = symbol.width();
    if actual_width == expected_width {
        return symbol.to_string();
    }

    if wide == crate::ghostty::CellWide::Narrow && actual_width == 2 {
        return symbol.to_string();
    }
    if wide == crate::ghostty::CellWide::Wide && is_halfwidth_katakana_voiced_grapheme(symbol) {
        return symbol.to_string();
    }

    ghostty_blank_symbol_for_width(wide).to_string()
}

fn is_halfwidth_katakana_voiced_grapheme(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(base) = chars.next() else {
        return false;
    };
    let Some(mark) = chars.next() else {
        return false;
    };
    chars.next().is_none()
        && ('\u{ff66}'..='\u{ff9d}').contains(&base)
        && matches!(mark, '\u{ff9e}' | '\u{ff9f}')
}

fn ghostty_buffer_symbol_into<'a>(
    cells: &crate::ghostty::RowCellIter<'_>,
    wide: crate::ghostty::CellWide,
    hide_kitty_placeholders: bool,
    grapheme_bytes: &mut Vec<u8>,
    symbol_scratch: &'a mut String,
) -> Result<&'a str, crate::ghostty::Error> {
    symbol_scratch.clear();
    match wide {
        crate::ghostty::CellWide::SpacerTail => {}
        crate::ghostty::CellWide::SpacerHead => symbol_scratch.push(' '),
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::Wide => {
            cells.grapheme_text_into(grapheme_bytes, symbol_scratch)?;
            let hidden_kitty_placeholder = hide_kitty_placeholders
                && symbol_scratch.chars().next().map(u32::from)
                    == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER);
            if hidden_kitty_placeholder || symbol_scratch.is_empty() {
                symbol_scratch.clear();
                symbol_scratch.push(' ');
            }
        }
    }

    let expected_width = match wide {
        crate::ghostty::CellWide::Wide => 2,
        crate::ghostty::CellWide::Narrow | crate::ghostty::CellWide::SpacerHead => 1,
        crate::ghostty::CellWide::SpacerTail => 0,
    };
    let actual_width = symbol_scratch.width();
    if actual_width != expected_width
        && !(wide == crate::ghostty::CellWide::Narrow && actual_width == 2)
        && !(wide == crate::ghostty::CellWide::Wide
            && is_halfwidth_katakana_voiced_grapheme(symbol_scratch))
    {
        symbol_scratch.clear();
        symbol_scratch.push_str(ghostty_blank_symbol_for_width(wide));
    }

    Ok(symbol_scratch.as_str())
}

fn ghostty_reset_cell(
    cell: &mut ratatui::buffer::Cell,
    default_fg: Option<Color>,
    default_bg: Option<Color>,
) {
    cell.reset();
    cell.set_symbol(" ");
    if let Some(bg) = default_bg {
        cell.set_bg(bg);
    }
    if let Some(fg) = default_fg {
        cell.set_fg(fg);
    }
}

fn ghostty_default_style(default_fg: Option<Color>, default_bg: Option<Color>) -> Style {
    let mut style = Style::default();
    if let Some(fg) = default_fg {
        style = style.fg(fg);
    }
    if let Some(bg) = default_bg {
        style = style.bg(bg);
    }
    style
}

fn ghostty_cell_style(
    cells: &crate::ghostty::RowCellIter<'_>,
    basic: &crate::ghostty::CellBasicData,
    default_fg: Option<Color>,
    default_bg: Option<Color>,
    resolved_fg: Option<Color>,
    resolved_bg: Option<Color>,
    palette_overrides: Option<&PaletteOverrides>,
) -> Style {
    let mut fg = basic
        .style
        .fg_color
        .map(|color| ghostty_cell_color(color, palette_overrides))
        .or_else(|| cells.fg_color().ok().flatten().map(ghostty_color))
        .or(default_fg);
    let mut bg = cells
        .content_bg_color()
        .ok()
        .flatten()
        .or(basic.style.bg_color)
        .map(|color| ghostty_cell_color(color, palette_overrides))
        .or_else(|| cells.bg_color().ok().flatten().map(ghostty_color))
        .or(default_bg);
    if basic.style.invisible {
        fg = bg.or(default_bg);
    }
    if basic.style.inverse {
        // When the background is transparent (None), resolve it to the
        // actual terminal background color before swapping.  Otherwise
        // the swapped fg becomes None (Color::Reset) which the host
        // terminal renders as its default foreground — the same hue as
        // the new bg, making inverse text invisible.
        if bg.is_none() {
            bg = resolved_bg;
        }
        if fg.is_none() {
            fg = resolved_fg;
        }
        std::mem::swap(&mut fg, &mut bg);
    }

    let mut style = ghostty_default_style(fg, bg);
    if let Some(underline_color) = basic
        .style
        .underline_color
        .map(|color| ghostty_cell_color(color, palette_overrides))
    {
        style = style.underline_color(underline_color);
    }
    let mut modifiers = Modifier::empty();
    if basic.style.bold {
        modifiers |= Modifier::BOLD;
    }
    if basic.style.italic {
        modifiers |= Modifier::ITALIC;
    }
    if basic.style.faint {
        modifiers |= Modifier::DIM;
    }
    if basic.style.blink {
        modifiers |= Modifier::SLOW_BLINK;
    }
    if basic.style.underlined {
        modifiers |= Modifier::UNDERLINED;
    }
    if basic.style.strikethrough {
        modifiers |= Modifier::CROSSED_OUT;
    }
    modifiers = crate::protocol::modifier_with_underline_style(modifiers, basic.style.underline);
    style.add_modifier(modifiers)
}

enum OrderedColorOrC1Event {
    Color(DefaultColorTrackedEvent),
    C1(C1XtgettcapResponse),
}

impl OrderedColorOrC1Event {
    fn end_offset(&self) -> usize {
        match self {
            Self::Color(event) => event.end_offset,
            Self::C1(response) => response.end_offset,
        }
    }
}

fn remove_last_matching_libghostty_color_reply(
    responses: &mut Vec<Bytes>,
    event: DefaultColorEvent,
) {
    if let Some(index) = responses
        .iter()
        .rposition(|response| is_matching_libghostty_color_reply(response, event))
    {
        responses.remove(index);
    }
}

fn is_matching_libghostty_color_reply(response: &Bytes, event: DefaultColorEvent) -> bool {
    let prefix = match event {
        DefaultColorEvent::Query(query) => format!("\x1b]{};rgb:", query.osc_number()),
        DefaultColorEvent::PaletteQuery(index) => format!("\x1b]4;{index};rgb:"),
        DefaultColorEvent::Set(_) | DefaultColorEvent::Reset(_) => return false,
    };
    response.starts_with(prefix.as_bytes())
        && (response.ends_with(b"\x07") || response.ends_with(b"\x1b\\"))
}

fn respond_to_default_color_event(
    core: &mut GhosttyPaneCore,
    event: DefaultColorEvent,
) -> Option<Bytes> {
    match event {
        DefaultColorEvent::Query(_) | DefaultColorEvent::PaletteQuery(_) => {
            default_color_event_response(core, event)
        }
        DefaultColorEvent::Set(query) => {
            mark_child_default_color_changed(core, query, true);
            None
        }
        DefaultColorEvent::Reset(query) => {
            mark_child_default_color_changed(core, query, false);
            apply_cached_host_default_color(core, query);
            None
        }
    }
}

fn default_color_event_response(
    core: &mut GhosttyPaneCore,
    event: DefaultColorEvent,
) -> Option<Bytes> {
    match event {
        DefaultColorEvent::Query(query) => default_color_query_response(query, core),
        DefaultColorEvent::PaletteQuery(index) => palette_color_query_response(index, core),
        DefaultColorEvent::Set(_) | DefaultColorEvent::Reset(_) => None,
    }
}

fn default_color_query_response(
    query: DefaultColorQuery,
    core: &mut GhosttyPaneCore,
) -> Option<Bytes> {
    let color = match query {
        DefaultColorQuery::Foreground if !core.child_default_foreground_changed => core
            .host_terminal_theme
            .foreground
            .map(host_theme_color_to_ghostty),
        DefaultColorQuery::Background if !core.child_default_background_changed => core
            .host_terminal_theme
            .background
            .map(host_theme_color_to_ghostty),
        DefaultColorQuery::Cursor => cursor_color_query_color(core),
        _ => None,
    }?;
    Some(osc_rgb_response(
        &query.osc_number().to_string(),
        color.r,
        color.g,
        color.b,
    ))
}

fn cursor_color_query_color(core: &mut GhosttyPaneCore) -> Option<crate::ghostty::RgbColor> {
    let host_foreground = core.host_terminal_theme.foreground;
    let child_foreground_changed = core.child_default_foreground_changed;
    core.terminal
        .effective_cursor_color()
        .ok()
        .flatten()
        .or_else(|| {
            if child_foreground_changed {
                core.terminal.effective_foreground_color().ok().flatten()
            } else {
                host_foreground
                    .map(host_theme_color_to_ghostty)
                    .or_else(|| core.terminal.effective_foreground_color().ok().flatten())
            }
        })
}

fn palette_color_query_response(index: u8, core: &mut GhosttyPaneCore) -> Option<Bytes> {
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    render_state.update(terminal).ok()?;
    let colors = render_state.colors().ok()?;
    let color = colors.palette[usize::from(index)];
    Some(osc_rgb_response(
        &format!("4;{index}"),
        color.r,
        color.g,
        color.b,
    ))
}

fn osc_rgb_response(command: &str, r: u8, g: u8, b: u8) -> Bytes {
    let r = u16::from(r) * 257;
    let g = u16::from(g) * 257;
    let b = u16::from(b) * 257;
    Bytes::from(format!("\x1b]{command};rgb:{r:04x}/{g:04x}/{b:04x}\x1b\\"))
}

fn host_theme_color_to_ghostty(color: crate::terminal_theme::RgbColor) -> crate::ghostty::RgbColor {
    crate::ghostty::RgbColor {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn apply_cached_host_default_color(core: &mut GhosttyPaneCore, query: DefaultColorQuery) {
    write_host_terminal_theme_selective(
        &mut core.terminal,
        core.host_terminal_theme,
        matches!(query, DefaultColorQuery::Foreground),
        matches!(query, DefaultColorQuery::Background),
    );
}

fn mark_child_default_color_changed(
    core: &mut GhosttyPaneCore,
    query: DefaultColorQuery,
    changed: bool,
) {
    match query {
        DefaultColorQuery::Foreground => core.child_default_foreground_changed = changed,
        DefaultColorQuery::Background => core.child_default_background_changed = changed,
        DefaultColorQuery::Cursor => {}
    }
}

fn ghostty_default_fg(
    color: crate::ghostty::RgbColor,
    host_theme: crate::terminal_theme::TerminalTheme,
    initial_default_foreground: Option<crate::ghostty::RgbColor>,
) -> Option<Color> {
    if let Some(host_foreground) = host_theme.foreground {
        if host_foreground == terminal_theme_color(color) {
            None
        } else {
            Some(ghostty_color(color))
        }
    } else if initial_default_foreground.is_some_and(|initial| initial != color) {
        Some(ghostty_color(color))
    } else {
        None
    }
}

fn ghostty_default_bg(
    color: crate::ghostty::RgbColor,
    host_theme: crate::terminal_theme::TerminalTheme,
    initial_default_background: Option<crate::ghostty::RgbColor>,
) -> Option<Color> {
    if let Some(host_background) = host_theme.background {
        if host_background == terminal_theme_color(color) {
            None
        } else {
            Some(ghostty_color(color))
        }
    } else if initial_default_background.is_some_and(|initial| initial != color) {
        Some(ghostty_color(color))
    } else {
        None
    }
}

fn terminal_theme_color(color: crate::ghostty::RgbColor) -> crate::terminal_theme::RgbColor {
    crate::terminal_theme::RgbColor {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

// Palette entries the program redefined with OSC 4. Forwarding a palette index to the
// host makes it resolve against the host's own palette, discarding the redefinition.
// Only overridden entries become RGB; the rest stay indexed and keep following the
// host theme. None when nothing was redefined, which is the common case.
struct PaletteOverrides([Option<crate::ghostty::RgbColor>; 256]);

impl PaletteOverrides {
    fn new(
        active: &[crate::ghostty::RgbColor; 256],
        default: &[crate::ghostty::RgbColor; 256],
    ) -> Option<Self> {
        let mut overrides = [None; 256];
        let mut any = false;
        for (index, (active, default)) in active.iter().zip(default.iter()).enumerate() {
            if active != default {
                overrides[index] = Some(*active);
                any = true;
            }
        }
        any.then_some(Self(overrides))
    }

    fn get(&self, index: u8) -> Option<crate::ghostty::RgbColor> {
        self.0[usize::from(index)]
    }
}

fn ghostty_cell_color(
    color: crate::ghostty::CellColor,
    palette_overrides: Option<&PaletteOverrides>,
) -> Color {
    match color {
        crate::ghostty::CellColor::Palette(index) => {
            match palette_overrides.and_then(|overrides| overrides.get(index)) {
                Some(color) => ghostty_color(color),
                None => Color::Indexed(index),
            }
        }
        crate::ghostty::CellColor::Rgb(color) => ghostty_color(color),
    }
}

fn ghostty_color(color: crate::ghostty::RgbColor) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

fn lines_to_text(lines: Vec<String>) -> String {
    let text = lines.join("\n");
    if text.is_empty() {
        text
    } else {
        format!("{text}\n")
    }
}

pub(super) fn trim_trailing_blank_rows(rows: &mut Vec<String>) {
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
}

fn recent_text_from_rows(rows: &[String], lines: usize) -> String {
    let start = rows.len().saturating_sub(lines);
    let text = rows[start..].join("\n");
    if text.is_empty() {
        text
    } else {
        format!("{text}\n")
    }
}

fn contains_kitty_graphics_sequence(bytes: &[u8]) -> bool {
    bytes.windows(3).any(|window| window == b"\x1b_G")
}

fn should_probe_host_terminal_theme_restore(core: &GhosttyPaneCore) -> bool {
    if core.transient_default_color_owner_pgid.is_none() || core.host_terminal_theme.is_empty() {
        return false;
    }

    !core
        .terminal
        .active_screen()
        .map(|screen| screen == crate::ghostty::ActiveScreen::Alternate)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{layout::Rect, style::Color};
    use tokio::sync::mpsc;

    #[test]
    fn plain_page_keys_host_scroll_for_shell_like_decckm_with_bracketed_paste() {
        assert!(InputState {
            alternate_screen: false,
            application_cursor: true,
            bracketed_paste: true,
            focus_reporting: false,
            mouse_protocol_mode: crate::input::MouseProtocolMode::None,
            mouse_protocol_encoding: crate::input::MouseProtocolEncoding::Default,
            mouse_alternate_scroll: false,
            modify_other_keys: false,
            color_scheme_reporting: false,
        }
        .plain_page_keys_use_host_scrollback());
    }

    fn rgb(r: u8, g: u8, b: u8) -> crate::ghostty::RgbColor {
        crate::ghostty::RgbColor { r, g, b }
    }

    #[test]
    fn palette_overrides_are_none_without_an_osc4_write() {
        let default = [rgb(1, 2, 3); 256];
        assert!(PaletteOverrides::new(&default, &default).is_none());
    }

    #[test]
    fn ghostty_pane_characterizes_ctrl_backspace_encoding() {
        let (tx, _rx) = mpsc::channel(4);
        let legacy = GhosttyPaneTerminal::new(
            crate::ghostty::Terminal::new(80, 24, 0).unwrap(),
            tx.clone(),
        )
        .unwrap();

        let ctrl_backspace = crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Backspace,
            crossterm::event::KeyModifiers::CONTROL,
        );
        assert_eq!(
            legacy.encode_terminal_key(
                ctrl_backspace.clone(),
                crate::input::KeyboardProtocol::Legacy
            ),
            b"\x08"
        );

        let plain_backspace = crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Backspace,
            crossterm::event::KeyModifiers::empty(),
        );
        assert_eq!(
            legacy.encode_terminal_key(plain_backspace, crate::input::KeyboardProtocol::Legacy),
            b"\x7f"
        );

        let kitty = GhosttyPaneTerminal::new(
            crate::ghostty::Terminal::new(80, 24, 0).unwrap(),
            tx.clone(),
        )
        .unwrap();
        let pane_id = PaneId::from_raw(1);
        kitty.process_pty_bytes(pane_id, 0, b"\x1b[>1u", &tx);

        assert_eq!(
            kitty.encode_terminal_key(ctrl_backspace, crate::input::KeyboardProtocol::Legacy),
            b"\x1b[127;5u"
        );
    }

    #[test]
    fn ghostty_key_encoders_are_isolated_per_pane() {
        let (tx, _rx) = mpsc::channel(4);
        let first = GhosttyPaneTerminal::new(
            crate::ghostty::Terminal::new(80, 24, 0).unwrap(),
            tx.clone(),
        )
        .unwrap();
        let second = GhosttyPaneTerminal::new(
            crate::ghostty::Terminal::new(80, 24, 0).unwrap(),
            tx.clone(),
        )
        .unwrap();

        first.process_pty_bytes(PaneId::from_raw(1), 0, b"\x1b[?1h", &tx);

        let first_encoded = first.encode_terminal_key(
            crate::input::TerminalKey::new(
                crossterm::event::KeyCode::Up,
                crossterm::event::KeyModifiers::empty(),
            ),
            crate::input::KeyboardProtocol::Legacy,
        );
        let second_encoded = second.encode_terminal_key(
            crate::input::TerminalKey::new(
                crossterm::event::KeyCode::Up,
                crossterm::event::KeyModifiers::empty(),
            ),
            crate::input::KeyboardProtocol::Legacy,
        );

        assert_eq!(first_encoded, b"\x1bOA");
        assert_eq!(second_encoded, b"\x1b[A");
    }

    #[test]
    fn ghostty_mouse_button_encoding_uses_live_terminal_state() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        terminal.write(b"\x1b[?1000h\x1b[?1006h");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let encoded = pane.encode_mouse_button(
            crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
            crate::input::mouse::Position::Cell { column: 11, row: 9 },
            crossterm::event::KeyModifiers::empty(),
        );

        assert_eq!(encoded.as_deref(), Some(&b"\x1b[<0;12;10m"[..]));
    }

    #[test]
    fn ghostty_mouse_drag_encoding_uses_motion_reporting_state() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        terminal.write(b"\x1b[?1002h\x1b[?1006h");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let encoded = pane.encode_mouse_button(
            crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            crate::input::mouse::Position::Cell { column: 4, row: 6 },
            crossterm::event::KeyModifiers::SHIFT,
        );

        assert_eq!(encoded.as_deref(), Some(&b"\x1b[<36;5;7M"[..]));
    }

    #[test]
    fn ghostty_mouse_drag_without_motion_reporting_is_not_forwarded() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        terminal.write(b"\x1b[?1000h\x1b[?1006h");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let encoded = pane.encode_mouse_button(
            crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            crate::input::mouse::Position::Cell { column: 4, row: 6 },
            crossterm::event::KeyModifiers::empty(),
        );

        assert_eq!(encoded, None);
    }

    #[test]
    fn ghostty_mouse_moved_encoding_uses_any_motion_state() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        terminal.write(b"\x1b[?1003h\x1b[?1006h");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let encoded = pane.encode_mouse_motion(
            crossterm::event::MouseEventKind::Moved,
            crate::input::mouse::Position::Cell { column: 4, row: 6 },
            crossterm::event::KeyModifiers::empty(),
        );

        assert_eq!(encoded.as_deref(), Some(&b"\x1b[<35;5;7M"[..]));
    }

    #[test]
    fn ghostty_mouse_sgr_pixels_preserves_exact_and_downgrades_cell_input() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        terminal.resize(80, 24, 10, 20).unwrap();
        terminal.write(b"\x1b[?1003h\x1b[?1006h\x1b[?1016h");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let exact = pane.encode_mouse_motion(
            crossterm::event::MouseEventKind::Moved,
            crate::input::mouse::Position::Pixels { x: 48, y: 139 },
            crossterm::event::KeyModifiers::empty(),
        );
        let fallback = pane.encode_mouse_motion(
            crossterm::event::MouseEventKind::Moved,
            crate::input::mouse::Position::Cell { column: 4, row: 6 },
            crossterm::event::KeyModifiers::empty(),
        );

        assert_eq!(exact.as_deref(), Some(&b"\x1b[<35;48;139M"[..]));
        assert_eq!(fallback.as_deref(), Some(&b"\x1b[<35;5;7M"[..]));
    }

    #[test]
    fn ghostty_normalize_buffer_symbol_prefers_grapheme_width_when_metadata_disagrees() {
        const WIDE_GRAPHEME: &str = "🙂";
        const FLAG_GRAPHEME: &str = "🇧🇷";
        const FAMILY_GRAPHEME: &str = "👨‍👩‍👧";
        const VS16_GRAPHEME: &str = "⚠️";
        const EMOJI_GRAPHEME: &str = "💳";

        assert_eq!(
            ghostty_normalize_buffer_symbol(WIDE_GRAPHEME, crate::ghostty::CellWide::Wide),
            WIDE_GRAPHEME
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol("a", crate::ghostty::CellWide::Wide),
            "  "
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol(FLAG_GRAPHEME, crate::ghostty::CellWide::Wide),
            FLAG_GRAPHEME
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol(FAMILY_GRAPHEME, crate::ghostty::CellWide::Wide),
            FAMILY_GRAPHEME
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol("⌨️", crate::ghostty::CellWide::Narrow),
            "⌨️"
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol(VS16_GRAPHEME, crate::ghostty::CellWide::Narrow),
            VS16_GRAPHEME
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol(EMOJI_GRAPHEME, crate::ghostty::CellWide::Narrow),
            EMOJI_GRAPHEME
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol(" ", crate::ghostty::CellWide::SpacerTail),
            ""
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol("xx", crate::ghostty::CellWide::SpacerHead),
            " "
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol("ｶ\u{ff9e}", crate::ghostty::CellWide::Wide),
            "ｶ\u{ff9e}"
        );
        assert_eq!(
            ghostty_normalize_buffer_symbol("ﾊ\u{ff9f}", crate::ghostty::CellWide::Wide),
            "ﾊ\u{ff9f}"
        );
    }

    fn render_cells_to_symbols(
        terminal: &mut crate::ghostty::Terminal,
    ) -> Vec<(crate::ghostty::CellWide, String)> {
        let mut render_state = crate::ghostty::RenderState::new().unwrap();
        render_state.update(terminal).unwrap();

        let mut row_iterator = crate::ghostty::RowIterator::new().unwrap();
        let mut rows = render_state
            .populate_row_iterator(&mut row_iterator)
            .unwrap();
        let mut row_cells = crate::ghostty::RowCells::new().unwrap();
        let mut grapheme_bytes = Vec::new();
        let mut symbol_scratch = String::new();
        let mut out = Vec::new();

        if rows.next() {
            let mut cells = rows.populate_cells(&mut row_cells).unwrap();
            while cells.next() {
                let wide = cells.wide().unwrap_or(crate::ghostty::CellWide::Narrow);
                let symbol = ghostty_buffer_symbol_into(
                    &cells,
                    wide,
                    false,
                    &mut grapheme_bytes,
                    &mut symbol_scratch,
                )
                .unwrap()
                .to_string();
                out.push((wide, symbol));
            }
        }

        out
    }

    #[test]
    fn grapheme_cluster_mode_renders_flag_emoji_in_single_wide_cell() {
        let mut terminal = crate::ghostty::Terminal::new(40, 1, 0).unwrap();
        terminal.write("🇧🇷".as_bytes());

        let cells = render_cells_to_symbols(&mut terminal);

        assert!(
            cells
                .iter()
                .any(|(wide, symbol)| *wide == crate::ghostty::CellWide::Wide && symbol == "🇧🇷"),
            "expected a wide cell containing the full flag grapheme, got {cells:?}"
        );
    }

    #[test]
    fn grapheme_cluster_mode_renders_zwj_family_in_single_wide_cell() {
        let mut terminal = crate::ghostty::Terminal::new(40, 1, 0).unwrap();
        terminal.write("👨\u{200d}👩\u{200d}👧".as_bytes());

        let cells = render_cells_to_symbols(&mut terminal);

        assert!(
            cells
                .iter()
                .any(|(wide, symbol)| *wide == crate::ghostty::CellWide::Wide
                    && symbol == "👨\u{200d}👩\u{200d}👧"),
            "expected a wide cell containing the full ZWJ grapheme, got {cells:?}"
        );
    }

    #[test]
    fn halfwidth_katakana_voiced_marks_render() {
        let mut terminal = crate::ghostty::Terminal::new(40, 1, 0).unwrap();
        terminal.write("ｱｲｳｴｵ ｶﾞｷﾞｸﾞｹﾞｺﾞ ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ".as_bytes());

        let cells = render_cells_to_symbols(&mut terminal);
        let rendered: String = cells.iter().map(|(_, symbol)| symbol.as_str()).collect();

        assert!(
            rendered.contains("ｱｲｳｴｵ ｶﾞｷﾞｸﾞｹﾞｺﾞ ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ"),
            "expected halfwidth katakana with voiced marks to survive, got {cells:?}"
        );
    }

    #[test]
    fn render_keeps_halfwidth_katakana_voiced_tail_empty() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(20, 1, 0).unwrap();
        terminal.write("ｶﾞZ".as_bytes());
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let backend = ratatui::backend::TestBackend::new(20, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 1), false))
            .unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(buffer[(0, 0)].symbol(), "ｶ\u{ff9e}");
        assert_eq!(
            buffer[(1, 0)].symbol(),
            "",
            "wide spacer tail must stay empty so the host terminal does not overwrite the voiced kana"
        );
        assert_eq!(buffer[(2, 0)].symbol(), "Z");
    }

    #[test]
    fn empty_or_short_resize_keeps_following_bottom_when_output_creates_scrollback() {
        for initial in [b"".as_slice(), b"seed\r\n".as_slice()] {
            let (tx, _rx) = mpsc::channel(4);
            let mut terminal = crate::ghostty::Terminal::new(10, 3, 100).unwrap();
            terminal.write(initial);
            let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
            let pane_id = PaneId::from_raw(1);

            pane.resize(3, 10, 0, 0);
            pane.process_pty_bytes(
                pane_id,
                0,
                b"000000\r\n000001\r\n000002\r\n000003\r\n000004",
                &tx,
            );

            let metrics = pane.scroll_metrics().expect("scroll metrics after output");
            assert_eq!(metrics.offset_from_bottom, 0);
            assert!(pane.visible_text().contains("000004"));
        }
    }

    #[test]
    fn recent_reads_include_viewport_before_scrollback_exists() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal =
            crate::ghostty::Terminal::new(20, 20, crate::config::DEFAULT_SCROLLBACK_LIMIT_BYTES)
                .unwrap();
        terminal.write(b"hello123");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        assert_eq!(pane.recent_text(3), "hello123\n");
        assert_eq!(pane.recent_unwrapped_text(3), "hello123");
    }

    #[test]
    fn alternate_screen_recent_reads_keep_physical_row_ranges() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(20, 20, 100).unwrap();
        terminal.write(b"\x1b[?1049hhello123");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        assert_eq!(pane.recent_text(3), "");
        assert_eq!(pane.recent_unwrapped_text(3), "");
    }

    #[test]
    fn recent_unwrapped_text_ignores_soft_wraps() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(5, 3, 100).unwrap();
        terminal.write(b"ABCDEFGHIJ");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        assert_eq!(pane.recent_text(3), "ABCDE\nFGHIJ\n");
        assert_eq!(pane.recent_unwrapped_text(3), "ABCDEFGHIJ");
    }

    #[test]
    fn recent_snapshots_report_omitted_rendered_rows() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
        terminal.write(b"one\r\ntwo\r\nthree\r\nfour");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        assert!(pane.recent_text_snapshot(2).truncated);
        assert!(pane.recent_ansi_snapshot(2).truncated);
        assert!(pane.recent_unwrapped_text_snapshot(2).truncated);
        assert!(pane.recent_unwrapped_ansi_snapshot(2).truncated);
        assert!(!pane.recent_text_snapshot(100).truncated);
    }

    #[test]
    fn plain_text_reads_skip_wide_character_spacer_cells() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(40, 3, 100).unwrap();
        terminal.write("日本語テスト ABC 123".as_bytes());
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        assert_eq!(pane.visible_text(), "日本語テスト ABC 123\n");
        assert_eq!(pane.recent_text(3), "日本語テスト ABC 123\n");
        assert_eq!(pane.recent_unwrapped_text(3), "日本語テスト ABC 123");
        assert_eq!(pane.detection_text(), "日本語テスト ABC 123\n");
    }

    #[test]
    fn visible_ansi_preserves_cell_style_sequences() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
        terminal.write(b"\x1b[31;1mred\x1b[0m plain");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let ansi = pane.visible_ansi();
        assert!(ansi.contains("red"));
        assert!(ansi.contains("plain"));
        assert!(ansi.contains("\x1b["));
    }

    #[test]
    fn recent_ansi_can_read_styled_scrollback() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
        terminal.write(b"\x1b[34mblue\x1b[0m\r\nline2\r\nline3\r\nline4");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let ansi = pane.recent_ansi(4);
        assert!(ansi.contains("blue"));
        assert!(ansi.contains("line4"));
        assert!(ansi.contains("\x1b["));
    }

    #[test]
    fn resize_shrinks_both_axes_with_cursor_at_old_bottom() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(8, 4, 10_000).unwrap();
        terminal.write(b"alpha\r\nbeta\r\ngamma\r\ndelta");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        pane.resize(3, 7, 8, 16);

        assert_eq!(pane.visible_text(), "beta\ngamma\ndelta\n");
        assert_eq!(pane.detection_text(), "beta\ngamma\ndelta\n");
        assert_eq!(
            pane.scroll_metrics(),
            Some(ScrollMetrics {
                offset_from_bottom: 0,
                max_offset_from_bottom: 1,
                viewport_rows: 3,
            })
        );
    }

    #[test]
    fn resize_recovery_does_not_replay_history_when_visible_screen_was_blank() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(20, 3, 10_000).unwrap();
        terminal.write(b"old history\r\n\x1b[2J\x1b[H");
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        assert!(pane.visible_text().trim().is_empty());
        assert!(pane.detection_text().trim().is_empty());

        pane.resize(3, 20, 0, 0);

        assert!(pane.visible_text().trim().is_empty());
        assert!(pane.detection_text().trim().is_empty());
        assert!(pane.recent_text(3).trim().is_empty());
    }

    #[test]
    fn process_pty_bytes_answers_xtwinops_size_queries() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.resize(24, 80, 9, 18);

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[14t\x1b[16t\x1b[18t", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![
                Bytes::from_static(b"\x1b[4;432;720t"),
                Bytes::from_static(b"\x1b[6;18;9t"),
                Bytes::from_static(b"\x1b[8;24;80t"),
            ]
        );
    }

    #[test]
    fn xtwinops_size_queries_follow_successful_resize() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.resize(24, 80, 9, 18);
        pane.resize(30, 100, 10, 20);

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[14t\x1b[16t\x1b[18t", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![
                Bytes::from_static(b"\x1b[4;600;1000t"),
                Bytes::from_static(b"\x1b[6;20;10t"),
                Bytes::from_static(b"\x1b[8;30;100t"),
            ]
        );
    }

    #[test]
    fn xtwinops_size_queries_stay_silent_without_pixel_geometry() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        for (cell_width_px, cell_height_px) in [(0, 0), (0, 18), (9, 0)] {
            pane.resize(24, 80, cell_width_px, cell_height_px);
            let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[14t\x1b[16t\x1b[18t", &tx);
            assert!(result.terminal_responses.is_empty());
        }
    }

    #[test]
    fn enabling_in_band_size_reports_after_alt_screen_resize_reports_current_size() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(91, 24, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.process_pty_bytes(pane_id, 0, b"\x1b[?1049h", &tx);
        assert!(pane.resize(24, 92, 9, 18).is_empty());

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[?2048h", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b[48;24;92;432;828t")]
        );
    }

    #[test]
    fn resize_returns_in_band_size_report_response() {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        terminal.mode_set(2048, true).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

        let responses = pane.resize(40, 100, 9, 18);

        assert_eq!(
            responses,
            vec![Bytes::from_static(b"\x1B[48;40;100;720;900t")]
        );
    }

    #[test]
    fn synchronized_output_suppresses_intermediate_render_requests_until_batch_ends() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        let pane_terminal = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        let begin = pane_terminal.process_pty_bytes(pane_id, 0, b"\x1b[?2026h", &tx);
        assert!(!begin.request_render);

        let body = pane_terminal.process_pty_bytes(pane_id, 0, b"hello", &tx);
        assert!(!body.request_render);

        let end = pane_terminal.process_pty_bytes(pane_id, 0, b"\x1b[?2026l", &tx);
        assert!(end.request_render);
    }

    #[test]
    fn seeded_history_is_rendered_on_next_draw() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 100).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        pane.seed_history_ansi("restored history");

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row = (0..16).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        assert_eq!(row, "restored history");
    }

    #[test]
    fn render_leaves_unknown_host_default_background_transparent() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(b"hi");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "h");
        assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Reset));
        assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Reset));
        assert_eq!(buffer[(2, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
        assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
    }

    #[test]
    fn render_keeps_explicit_cell_background_when_host_is_unknown() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(b"\x1b[48;2;68;85;102mhi\x1b[0m");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let expected_bg = Some(Color::Rgb(0x44, 0x55, 0x66));
        assert_eq!(buffer[(0, 0)].symbol(), "h");
        assert_eq!(buffer[(0, 0)].style().bg, expected_bg);
        assert_eq!(buffer[(2, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
    }

    #[test]
    fn render_preserves_palette_colors_instead_of_flattening_to_rgb() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(
                b"\x1b[31mR\x1b[0m \x1b[38;5;171mI\x1b[0m \x1b[48;5;4mB\x1b[0m \x1b[38;2;1;2;3mT",
            );
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "R");
        assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Indexed(1)));
        assert_eq!(buffer[(2, 0)].symbol(), "I");
        assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Indexed(171)));
        assert_eq!(buffer[(4, 0)].symbol(), "B");
        assert_eq!(buffer[(4, 0)].style().bg, Some(Color::Indexed(4)));
        assert_eq!(buffer[(6, 0)].symbol(), "T");
        assert_eq!(buffer[(6, 0)].style().fg, Some(Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn render_preserves_palette_background_fill_cells() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(b"\x1b[48;5;4m\x1b[K");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        for x in 0..20 {
            assert_eq!(buffer[(x, 0)].symbol(), " ");
            assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Indexed(4)));
        }
    }

    #[test]
    fn render_preserves_rgb_background_fill_cells() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(b"\x1b[48;2;17;34;51m\x1b[K");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        for x in 0..20 {
            assert_eq!(buffer[(x, 0)].symbol(), " ");
            assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Rgb(17, 34, 51)));
        }
    }

    #[test]
    fn process_pty_bytes_does_not_advertise_unsupported_glyph_protocol() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b_25a1;s\x1b\\", &tx);

        assert!(result.terminal_responses.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_returns_libghostty_query_responses_without_queuing_input() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[6n", &tx);

        assert_eq!(result.terminal_responses.len(), 1);
        assert!(String::from_utf8_lossy(&result.terminal_responses[0]).contains('R'));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn color_scheme_queries_and_live_updates_follow_terminal_mode() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        assert!(pane
            .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
            .is_none());
        let query = pane.process_pty_bytes(pane_id, 0, b"\x1b[?996n", &tx);
        assert_eq!(
            query.terminal_responses,
            vec![Bytes::from_static(b"\x1b[?997;1n")]
        );

        pane.process_pty_bytes(pane_id, 0, b"\x1b[?2031h", &tx);
        assert!(pane
            .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
            .is_none());
        assert_eq!(
            pane.apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Light)),
            Some(Bytes::from_static(b"\x1b[?997;2n"))
        );

        assert!(pane.apply_host_terminal_appearance(None).is_none());
        let unknown_query = pane.process_pty_bytes(pane_id, 0, b"\x1b[?996n", &tx);
        assert!(unknown_query.terminal_responses.is_empty());
        assert!(pane
            .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
            .is_none());

        pane.process_pty_bytes(pane_id, 0, b"\x1bc", &tx);
        assert!(pane
            .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Light))
            .is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_ignores_unknown_and_unsupported_xtgettcap_queries() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1bP+q6E6F7065;4D7\x1b\\", &tx);

        assert!(result.terminal_responses.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_preserves_earlier_aggregate_palette_reply() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        let result =
            pane.process_pty_bytes(pane_id, 0, b"\x1b]4;0;?;1;?\x1b\\\x1b]4;0;?\x1b\\", &tx);

        assert_eq!(result.terminal_responses.len(), 2);
        assert_eq!(
            result.terminal_responses[0]
                .windows(4)
                .filter(|window| *window == b"rgb:")
                .count(),
            2
        );
        assert!(result.terminal_responses[1].starts_with(b"\x1b]4;0;rgb:"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_preserves_libghostty_reply_for_child_color_override() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        pane.process_pty_bytes(pane_id, 0, b"\x1b]10;rgb:11/22/33\x07", &tx);
        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?\x1b\\", &tx);

        assert_eq!(result.terminal_responses.len(), 1);
        assert!(result.terminal_responses[0].starts_with(b"\x1b]10;rgb:1111/2222/3333"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_tracks_later_multi_value_color_set() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?;rgb:44/55/66\x1b\\", &tx);

        let core = pane.core.lock().unwrap();
        assert!(!core.child_default_foreground_changed);
        assert!(core.child_default_background_changed);
    }

    #[test]
    fn process_pty_bytes_returns_cursor_color_query_response_from_foreground_fallback() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0x65,
                g: 0x7b,
                b: 0x83,
            }),
            background: None,
            ..Default::default()
        });

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12;?\x07", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b]12;rgb:6565/7b7b/8383\x1b\\")]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_returns_cursor_color_query_response_from_child_foreground() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0x65,
                g: 0x7b,
                b: 0x83,
            }),
            background: None,
            ..Default::default()
        });

        pane.process_pty_bytes(pane_id, 0, b"\x1b]10;rgb:11/22/33\x07", &tx);
        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12;?\x07", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b]12;rgb:1111/2222/3333\x1b\\")]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_returns_explicit_cursor_color_query_response() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0x65,
                g: 0x7b,
                b: 0x83,
            }),
            background: None,
            ..Default::default()
        });

        pane.process_pty_bytes(pane_id, 0, b"\x1b]12;rgb:11/22/33\x07", &tx);
        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12;?\x07", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b]12;rgb:1111/2222/3333\x1b\\")]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_returns_default_color_query_responses_in_order() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0x65,
                g: 0x7b,
                b: 0x83,
            }),
            background: Some(crate::terminal_theme::RgbColor {
                r: 0xfd,
                g: 0xf6,
                b: 0xe3,
            }),
            ..Default::default()
        });

        let result =
            pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![
                Bytes::from_static(b"\x1b]10;rgb:6565/7b7b/8383\x1b\\"),
                Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\"),
                Bytes::from_static(b"\x1b]12;rgb:6565/7b7b/8383\x1b\\"),
            ]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_returns_split_default_color_query_response() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: None,
            background: Some(crate::terminal_theme::RgbColor {
                r: 0xfd,
                g: 0xf6,
                b: 0xe3,
            }),
            ..Default::default()
        });

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11", &tx);
        assert!(result.terminal_responses.is_empty());
        assert!(rx.try_recv().is_err());
        let result = pane.process_pty_bytes(pane_id, 0, b";?\x1b", &tx);
        assert!(result.terminal_responses.is_empty());
        assert!(rx.try_recv().is_err());
        let result = pane.process_pty_bytes(pane_id, 0, b"\\", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\")]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_returns_split_cursor_color_query_response() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0xfd,
                g: 0xf6,
                b: 0xe3,
            }),
            background: None,
            ..Default::default()
        });

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12", &tx);
        assert!(result.terminal_responses.is_empty());
        assert!(rx.try_recv().is_err());
        let result = pane.process_pty_bytes(pane_id, 0, b";?\x1b", &tx);
        assert!(result.terminal_responses.is_empty());
        assert!(rx.try_recv().is_err());
        let result = pane.process_pty_bytes(pane_id, 0, b"\\", &tx);

        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b]12;rgb:fdfd/f6f6/e3e3\x1b\\")]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn process_pty_bytes_tracks_default_color_set_and_reset_before_replying() {
        let (tx, mut rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);
        pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
            foreground: None,
            background: Some(crate::terminal_theme::RgbColor {
                r: 0xfd,
                g: 0xf6,
                b: 0xe3,
            }),
            ..Default::default()
        });

        let result =
            pane.process_pty_bytes(pane_id, 0, b"\x1b]11;rgb:11/22/33\x07\x1b]11;?\x07", &tx);
        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b]11;rgb:1111/2222/3333\x07")]
        );
        assert!(rx.try_recv().is_err());

        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]111\x07\x1b]11;?\x07", &tx);
        assert_eq!(
            result.terminal_responses,
            vec![Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\")]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn render_leaves_host_default_background_transparent() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        let host_theme = crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
        pane.apply_host_terminal_theme(host_theme);
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(b"hi");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "h");
        assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Reset));
        assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Reset));
        assert_eq!(buffer[(2, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
        assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
    }

    #[test]
    fn render_keeps_explicit_default_foreground_when_it_differs_from_host() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        let host_theme = crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
        pane.apply_host_terminal_theme(host_theme);
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(b"\x1b]10;rgb:44/55/66\x1b\\hi");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let expected_fg = Some(Color::Rgb(0x44, 0x55, 0x66));
        assert_eq!(buffer[(0, 0)].symbol(), "h");
        assert_eq!(buffer[(0, 0)].style().fg, expected_fg);
        assert_eq!(buffer[(2, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 0)].style().fg, expected_fg);
    }

    #[test]
    fn render_keeps_explicit_default_background_when_it_differs_from_host() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        let host_theme = crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
        pane.apply_host_terminal_theme(host_theme);
        {
            let mut core = pane.core.lock().unwrap();
            core.terminal.write(b"\x1b]11;rgb:44/55/66\x1b\\hi");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let expected_bg = Some(Color::Rgb(0x44, 0x55, 0x66));
        assert_eq!(buffer[(0, 0)].symbol(), "h");
        assert_eq!(buffer[(0, 0)].style().bg, expected_bg);
        assert_eq!(buffer[(2, 0)].symbol(), " ");
        assert_eq!(buffer[(2, 0)].style().bg, expected_bg);
    }

    #[test]
    fn render_inverse_text_swaps_fg_and_resolved_bg_when_bg_is_transparent() {
        let (tx, _rx) = mpsc::channel(4);
        let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        let host_theme = crate::terminal_theme::TerminalTheme {
            foreground: Some(crate::terminal_theme::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
        pane.apply_host_terminal_theme(host_theme);
        {
            let mut core = pane.core.lock().unwrap();
            // SGR 7 enables inverse/reverse video
            core.terminal.write(b"\x1b[7mhi\x1b[27m");
        }

        let backend = ratatui::backend::TestBackend::new(20, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let cell = &buffer[(0, 0)];
        assert_eq!(cell.symbol(), "h");
        // After inverse: fg should be the resolved bg, bg should be the original fg.
        // fg must NOT be Color::Reset (which would be the same hue as bg).
        assert_eq!(cell.style().fg, Some(Color::Rgb(0x11, 0x22, 0x33)));
        assert_eq!(cell.style().bg, Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
    }

    #[test]
    fn trim_trailing_blank_rows_drops_empty_viewport_tail() {
        let mut rows = vec!["hello".to_string(), "".to_string(), "   ".to_string()];
        trim_trailing_blank_rows(&mut rows);
        assert_eq!(rows, vec!["hello".to_string()]);
    }
}
