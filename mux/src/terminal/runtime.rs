use std::sync::Arc;

use crate::render_signal::RenderSignal;

use bytes::Bytes;
use ratatui::{layout::Rect, Frame};
use tokio::sync::{mpsc, Notify};

use crate::events::AppEvent;
use crate::layout::PaneId;

/// Live runtime for a server-owned terminal.
///
/// The PTY implementation still delegates to the legacy pane runtime while the
/// migration proceeds, but production code now depends on this terminal-layer
/// type instead of the pane module's implementation detail.
pub struct TerminalRuntime(crate::pane::PaneRuntime);

impl TerminalRuntime {
    pub fn shutdown(self) {
        self.0.shutdown();
    }

    pub fn duplicate_handoff_fd(&self) -> std::io::Result<std::os::fd::RawFd> {
        self.0.duplicate_handoff_fd()
    }

    pub fn preserve_for_handoff(self) {
        self.0.preserve_for_handoff()
    }

    pub fn assume_handoff_ownership(&mut self) {
        self.0.assume_handoff_ownership();
    }

    pub fn set_handoff_reader_paused(&self, paused: bool) {
        self.0.set_handoff_reader_paused(paused);
    }

    pub fn pause_handoff_reader(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        self.0.pause_handoff_reader(timeout)
    }

    pub fn handoff_runtime_state(
        &self,
        pane_id: u32,
    ) -> crate::handoff_runtime::HandoffRuntimeState {
        self.0.handoff_runtime_state(pane_id)
    }

    pub fn handoff_history_ansi(&self) -> Option<String> {
        self.0.handoff_history_ansi()
    }

    pub fn from_handoff_fd(
        import: crate::handoff_runtime::ImportedHandoffRuntime,
        detected_agent: Option<crate::detect::Agent>,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        crate::pane::PaneRuntime::from_handoff_fd(
            import,
            detected_agent,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
        )
        .map(Self)
    }

    // Wrapper mirrors pane runtime construction arguments.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: crate::pane::PaneShellConfig<'_>,
        launch_env: &crate::pane::PaneLaunchEnv,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        crate::pane::PaneRuntime::spawn(
            pane_id,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            launch_env,
            events,
            render_notify,
            render_dirty,
        )
        .map(Self)
    }

    // Wrapper mirrors pane runtime construction arguments.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_initial_history(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: crate::pane::PaneShellConfig<'_>,
        launch_env: &crate::pane::PaneLaunchEnv,
        initial_history_ansi: Option<&str>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        crate::pane::PaneRuntime::spawn_with_initial_history(
            pane_id,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            launch_env,
            initial_history_ansi,
            events,
            render_notify,
            render_dirty,
        )
        .map(Self)
    }

    // Wrapper mirrors pane runtime construction arguments.

    // Wrapper mirrors pane runtime construction arguments, including detection policy.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_argv_command(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        argv: &[String],
        launch_env: &crate::pane::PaneLaunchEnv,
        agent_detection: crate::pane::AgentDetection,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        crate::pane::PaneRuntime::spawn_argv_command(
            pane_id,
            rows,
            cols,
            cwd,
            argv,
            launch_env,
            agent_detection,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
        )
        .map(Self)
    }

    pub fn begin_graceful_release(&self, agent: crate::detect::Agent) {
        self.0.begin_graceful_release(agent);
    }

    pub fn set_full_lifecycle_authority_active(&self, active: bool) {
        self.0.set_full_lifecycle_authority_active(active);
    }

    pub fn resize(&self, rows: u16, cols: u16, cell_width_px: u32, cell_height_px: u32) {
        self.0.resize(rows, cols, cell_width_px, cell_height_px);
    }

    pub fn nudge_child_redraw_after_handoff(&self) {
        self.0.nudge_child_redraw_after_handoff();
    }

    pub fn scroll_up(&self, lines: usize) {
        self.0.scroll_up(lines);
    }

    pub fn scroll_down(&self, lines: usize) {
        self.0.scroll_down(lines);
    }

    pub fn scroll_reset(&self) {
        self.0.scroll_reset();
    }

    pub fn scroll_metrics(&self) -> Option<crate::pane::ScrollMetrics> {
        self.0.scroll_metrics()
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.0.bracketed_paste_enabled()
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.0.mouse_reporting_enabled()
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.0.sgr_pixel_mouse_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        self.0.plain_page_keys_use_host_scrollback()
    }

    pub fn cursor_state(
        &self,
        area: Rect,
        show_cursor: bool,
    ) -> Option<crate::pane::TerminalCursorState> {
        self.0.cursor_state(area, show_cursor)
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.0.synchronized_output_active()
    }

    pub fn visible_text(&self) -> String {
        self.0.visible_text()
    }

    pub fn visible_ansi(&self) -> String {
        self.0.visible_ansi()
    }

    pub fn detection_text(&self) -> String {
        self.0.detection_text()
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.0.terminal_title()
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> crate::pane::TerminalReadSnapshot {
        self.0.recent_text_snapshot(lines)
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> crate::pane::TerminalReadSnapshot {
        self.0.recent_ansi_snapshot(lines)
    }

    #[cfg(test)]
    pub fn recent_unwrapped_text(&self, lines: usize) -> String {
        self.0.recent_unwrapped_text_snapshot(lines).text
    }

    pub(crate) fn recent_unwrapped_text_snapshot(
        &self,
        lines: usize,
    ) -> crate::pane::TerminalReadSnapshot {
        self.0.recent_unwrapped_text_snapshot(lines)
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(
        &self,
        lines: usize,
    ) -> crate::pane::TerminalReadSnapshot {
        self.0.recent_unwrapped_ansi_snapshot(lines)
    }

    pub fn snapshot_history(&self) -> Option<String> {
        self.0.snapshot_history()
    }

    pub fn extract_selection<P>(
        &self,
        selection: &crate::selection::Selection<P>,
    ) -> Option<String> {
        self.0.extract_selection(selection)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        self.0.render(frame, area, show_cursor);
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.0.visible_hyperlinks(area)
    }

    pub fn keyboard_protocol(&self) -> crate::input::KeyboardProtocol {
        self.0.keyboard_protocol()
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.0.modify_other_keys_level()
    }

    pub fn encode_terminal_key(&self, key: crate::input::TerminalKey) -> Vec<u8> {
        self.0.encode_terminal_key(key)
    }

    pub fn try_send_bytes(&self, bytes: Bytes) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.0.try_send_bytes(bytes)
    }

    pub fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<std::sync::mpsc::Receiver<std::io::Result<()>>> {
        self.0
            .queue_user_input_submission(text, enter, delay, deadline)
    }

    pub fn try_send_paste(&self, text: String) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.0.try_send_paste(text)
    }

    pub fn wheel_routing(&self) -> Option<crate::pane::WheelRouting> {
        self.0.wheel_routing()
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        crate::terminal::ScreenSnapshot,
    )> {
        let (screen, cols, rows) = self.0.screen_text_snapshot()?;
        Some((screen, crate::terminal::ScreenSnapshot { cols, rows }))
    }

    pub(crate) fn screen_text_snapshot_with_seq(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        crate::terminal::ScreenSnapshot,
        u64,
    )> {
        for _ in 0..3 {
            let before = self.content_seq();
            if !before.is_multiple_of(2) {
                continue;
            }
            let (screen, snapshot) = self.screen_text_snapshot()?;
            let after = self.content_seq();
            if before == after {
                return Some((screen, snapshot, after));
            }
        }
        None
    }

    pub fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.0.encode_mouse_button(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.0.encode_mouse_motion(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.0.encode_mouse_wheel(kind, position, modifiers)
    }

    pub(crate) fn pixel_size(&self) -> Option<(u32, u32)> {
        self.0.pixel_size()
    }

    pub fn encode_alternate_scroll(
        &self,
        kind: crossterm::event::MouseEventKind,
    ) -> Option<Vec<u8>> {
        self.0.encode_alternate_scroll(kind)
    }

    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        self.0.cwd()
    }

    pub fn cwd_for_persistence(&self) -> Option<std::path::PathBuf> {
        self.0.cwd_for_persistence()
    }

    pub fn follow_cwd(&self) -> Option<std::path::PathBuf> {
        self.0.follow_cwd()
    }

    pub fn foreground_cwd(&self) -> Option<std::path::PathBuf> {
        self.0.foreground_cwd()
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.0.child_pid()
    }

    pub(crate) fn current_size(&self) -> (u16, u16) {
        self.0.current_size()
    }

    pub(crate) fn content_seq(&self) -> u64 {
        self.0.content_seq()
    }
}

#[cfg(test)]
impl TerminalRuntime {
    pub(crate) fn test_with_channel_capacity(
        cols: u16,
        rows: u16,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<Bytes>) {
        let (runtime, rx) =
            crate::pane::PaneRuntime::test_with_channel_capacity(cols, rows, capacity);
        (Self(runtime), rx)
    }

    pub(crate) fn test_with_screen_bytes(cols: u16, rows: u16, bytes: &[u8]) -> Self {
        Self(crate::pane::PaneRuntime::test_with_screen_bytes(
            cols, rows, bytes,
        ))
    }

    pub(crate) fn test_process_pty_bytes(&self, bytes: &[u8]) {
        self.0.test_process_pty_bytes(bytes);
    }

    pub(crate) fn test_with_scrollback_bytes(
        cols: u16,
        rows: u16,
        scrollback_limit_bytes: usize,
        bytes: &[u8],
    ) -> Self {
        Self(crate::pane::PaneRuntime::test_with_scrollback_bytes(
            cols,
            rows,
            scrollback_limit_bytes,
            bytes,
        ))
    }
}
