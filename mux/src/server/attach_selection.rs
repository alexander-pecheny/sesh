//! Mouse selection for a directly attached terminal.
//!
//! The attached pane fills the whole client window, so this follows the full UI's gestures
//! with the window as the pane: drag selects, release copies, a double click selects a word,
//! and dragging onto the top or bottom row scrolls the selection through the scrollback.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;

use crate::selection::Selection;
use crate::terminal::TerminalRuntime;

const DOUBLE_CLICK: Duration = Duration::from_millis(350);
const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(30);
const WORD_HIGHLIGHT: Duration = Duration::from_millis(500);

#[derive(Debug, Default)]
pub(crate) struct AttachSelection {
    selection: Option<Selection<()>>,
    word: Option<WordGesture>,
    last_click: Option<(Instant, u16, u16)>,
    autoscroll: Option<Autoscroll>,
    clear_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
struct WordGesture {
    start: (u32, u16),
    end: (u32, u16),
    cursor: (u32, u16),
    dragged: bool,
}

#[derive(Debug, Clone, Copy)]
struct Autoscroll {
    up: bool,
    column: u16,
    row: u16,
    at: Instant,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MouseOutcome {
    Forward,
    Handled,
    Copy(String),
}

impl AttachSelection {
    pub(crate) fn selection(&self) -> Option<&Selection<()>> {
        self.selection.as_ref()
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.autoscroll
            .map(|autoscroll| autoscroll.at)
            .into_iter()
            .chain(self.clear_at)
            .min()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Ctrl+C or Cmd+C copies a retained selection when copy-on-select is off.
    pub(crate) fn take_copy_key(
        &mut self,
        runtime: &TerminalRuntime,
        data: &[u8],
    ) -> Option<String> {
        let selection = self
            .selection
            .as_ref()
            .filter(|selection| selection.is_visible())?;
        let mut events = crate::raw_input::parse_raw_input_bytes_sync(data);
        let Some(crate::raw_input::RawInputEvent::Key(key)) =
            events.pop().filter(|_| events.is_empty())
        else {
            return None;
        };
        if !matches!(key.code, KeyCode::Char('c' | 'C'))
            || !matches!(key.modifiers, KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return None;
        }
        let text = runtime.extract_selection(selection);
        self.clear();
        text
    }

    pub(crate) fn mouse(
        &mut self,
        runtime: &TerminalRuntime,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        modifiers: KeyModifiers,
        copy_on_select: bool,
        now: Instant,
    ) -> MouseOutcome {
        match kind {
            MouseEventKind::Drag(MouseButton::Left) if self.in_progress() => {
                self.drag(runtime, column, row, now);
                MouseOutcome::Handled
            }
            MouseEventKind::Up(MouseButton::Left) if self.selection.is_some() => {
                self.release(runtime, copy_on_select, now)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let previous = self.last_click.take();
                self.clear();
                if runtime.mouse_reporting_enabled() {
                    return MouseOutcome::Forward;
                }
                self.press(runtime, column, row, modifiers, previous, now);
                MouseOutcome::Handled
            }
            _ => MouseOutcome::Forward,
        }
    }

    /// The wheel scrolls a selection that is still being dragged and extends it to the pointer.
    pub(crate) fn wheel(
        &mut self,
        runtime: &TerminalRuntime,
        up: bool,
        lines: u16,
        column: u16,
        row: u16,
    ) -> bool {
        if !self.in_progress() {
            return false;
        }
        scroll(runtime, up, usize::from(lines.max(1)));
        self.update_cursor(runtime, column, row);
        true
    }

    /// Advances edge autoscroll and expires a double-click highlight. Returns whether to repaint.
    pub(crate) fn tick(&mut self, runtime: &TerminalRuntime, now: Instant) -> bool {
        let mut changed = false;
        if self.clear_at.is_some_and(|at| now >= at) {
            self.clear_at = None;
            self.selection = None;
            changed = true;
        }
        let Some(autoscroll) = self.autoscroll.filter(|autoscroll| now >= autoscroll.at) else {
            return changed;
        };
        let offset = || {
            runtime
                .scroll_metrics()
                .map(|metrics| metrics.offset_from_bottom)
        };
        let before = offset();
        scroll(runtime, autoscroll.up, 1);
        if offset() == before {
            self.autoscroll = None;
            return changed;
        }
        self.update_cursor(runtime, autoscroll.column, autoscroll.row);
        self.autoscroll = Some(Autoscroll {
            at: now + AUTOSCROLL_INTERVAL,
            ..autoscroll
        });
        true
    }

    fn in_progress(&self) -> bool {
        self.word.is_some()
            || self
                .selection
                .as_ref()
                .is_some_and(Selection::is_in_progress)
    }

    fn press(
        &mut self,
        runtime: &TerminalRuntime,
        column: u16,
        row: u16,
        modifiers: KeyModifiers,
        previous: Option<(Instant, u16, u16)>,
        now: Instant,
    ) {
        let area = area(runtime);
        let (row, col) = clamp(area, column, row);
        let metrics = runtime.scroll_metrics();
        let double_click = modifiers.is_empty()
            && previous.is_some_and(|(at, previous_row, previous_col)| {
                now.duration_since(at) <= DOUBLE_CLICK
                    && previous_row.abs_diff(row) <= 1
                    && previous_col.abs_diff(col) <= 1
            });
        if !double_click {
            if modifiers.is_empty() {
                self.last_click = Some((now, row, col));
            }
            self.selection = Some(Selection::anchor((), row, col, metrics));
            return;
        }
        let absolute_row = crate::selection::absolute_row_for_viewport(row, metrics);
        if let Some((start, end)) = word_at(runtime, area.width, absolute_row, col) {
            self.word = Some(WordGesture {
                start: (absolute_row, start),
                end: (absolute_row, end),
                cursor: (absolute_row, col),
                dragged: false,
            });
            self.selection = Some(Selection::absolute_range(
                (),
                (absolute_row, start),
                (absolute_row, end),
            ));
        }
    }

    fn drag(&mut self, runtime: &TerminalRuntime, column: u16, row: u16, now: Instant) {
        let area = area(runtime);
        let moved_from_anchor = self.selection.as_ref().is_some_and(|selection| {
            selection.anchor_screen_pos(area, runtime.scroll_metrics()) != (row, column)
        });
        self.update_cursor(runtime, column, row);
        let dragging = match self.word {
            Some(word) => word.dragged,
            None => {
                if let Some(selection) = self.selection.as_mut().filter(|_| moved_from_anchor) {
                    selection.force_dragging();
                }
                self.selection.as_ref().is_some_and(Selection::is_dragging)
            }
        };
        if !dragging || runtime.scroll_metrics().is_none() {
            self.autoscroll = None;
            return;
        }
        self.last_click = None;
        let bottom = area.height.saturating_sub(1);
        let up = match row {
            0 => true,
            row if row >= bottom => false,
            _ => {
                self.autoscroll = None;
                return;
            }
        };
        if row > bottom {
            scroll(
                runtime,
                false,
                usize::from(row - bottom).saturating_mul(3).clamp(3, 15),
            );
            self.update_cursor(runtime, column, row);
        }
        self.autoscroll = Some(Autoscroll {
            up,
            column,
            row,
            at: now + AUTOSCROLL_INTERVAL,
        });
    }

    fn update_cursor(&mut self, runtime: &TerminalRuntime, column: u16, row: u16) {
        let area = area(runtime);
        let metrics = runtime.scroll_metrics();
        let Some(word) = self.word.as_mut() else {
            if let Some(selection) = self.selection.as_mut() {
                selection.drag(column, row, area, metrics);
            }
            return;
        };
        let (row, col) = clamp(area, column, row);
        let cursor = (
            crate::selection::absolute_row_for_viewport(row, metrics),
            col,
        );
        if cursor == word.cursor {
            return;
        }
        word.cursor = cursor;
        word.dragged = true;
        let (start_col, end_col) =
            word_at(runtime, area.width, cursor.0, col).unwrap_or((col, col));
        let start = word.start.min((cursor.0, start_col));
        let end = word.end.max((cursor.0, end_col));
        self.selection = Some(Selection::absolute_range((), start, end));
    }

    fn release(
        &mut self,
        runtime: &TerminalRuntime,
        copy_on_select: bool,
        now: Instant,
    ) -> MouseOutcome {
        self.autoscroll = None;
        let word = self.word.take();
        let Some(selection) = self.selection.as_mut() else {
            return MouseOutcome::Handled;
        };
        let finished = selection.finish();
        let text = (finished && copy_on_select)
            .then(|| runtime.extract_selection(selection))
            .flatten();
        if finished {
            self.last_click = None;
        }
        match word {
            Some(word) if copy_on_select && !word.dragged => {
                self.clear_at = Some(now + WORD_HIGHLIGHT);
            }
            Some(_) if !copy_on_select => {}
            _ if copy_on_select || !finished => self.selection = None,
            _ => {}
        }
        text.filter(|text| !text.is_empty())
            .map_or(MouseOutcome::Handled, MouseOutcome::Copy)
    }
}

fn area(runtime: &TerminalRuntime) -> Rect {
    let (rows, cols) = runtime.current_size();
    Rect::new(0, 0, cols, rows)
}

fn clamp(area: Rect, column: u16, row: u16) -> (u16, u16) {
    (
        row.min(area.height.saturating_sub(1)),
        column.min(area.width.saturating_sub(1)),
    )
}

fn scroll(runtime: &TerminalRuntime, up: bool, lines: usize) {
    if up {
        runtime.scroll_up(lines);
    } else {
        runtime.scroll_down(lines);
    }
}

fn word_at(runtime: &TerminalRuntime, width: u16, row: u32, col: u16) -> Option<(u16, u16)> {
    let line = Selection::absolute_range((), (row, 0), (row, width.saturating_sub(1)));
    crate::app::actions::word_bounds_at_column(&runtime.extract_selection(&line)?, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: MouseButton = MouseButton::Left;

    fn mouse(
        state: &mut AttachSelection,
        runtime: &TerminalRuntime,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        copy_on_select: bool,
        now: Instant,
    ) -> MouseOutcome {
        state.mouse(
            runtime,
            kind,
            column,
            row,
            KeyModifiers::empty(),
            copy_on_select,
            now,
        )
    }

    #[tokio::test]
    async fn drag_highlights_and_release_copies_then_clears() {
        let runtime = TerminalRuntime::test_with_screen_bytes(20, 5, b"hello world\r\nnext line");
        let mut state = AttachSelection::default();
        let now = Instant::now();

        assert_eq!(
            mouse(
                &mut state,
                &runtime,
                MouseEventKind::Down(LEFT),
                6,
                0,
                true,
                now
            ),
            MouseOutcome::Handled
        );
        assert!(state.selection().is_some_and(|s| !s.is_visible()));
        mouse(
            &mut state,
            &runtime,
            MouseEventKind::Drag(LEFT),
            3,
            1,
            true,
            now,
        );
        assert!(state.selection().is_some_and(|s| s.contains(0, 10, None)));

        assert_eq!(
            mouse(
                &mut state,
                &runtime,
                MouseEventKind::Up(LEFT),
                3,
                1,
                true,
                now
            ),
            MouseOutcome::Copy("world\nnext".to_owned())
        );
        assert!(state.selection().is_none());
    }

    #[tokio::test]
    async fn without_copy_on_select_the_selection_stays_until_ctrl_c_copies_it() {
        let runtime = TerminalRuntime::test_with_screen_bytes(20, 5, b"hello world");
        let mut state = AttachSelection::default();
        let now = Instant::now();

        mouse(
            &mut state,
            &runtime,
            MouseEventKind::Down(LEFT),
            0,
            0,
            false,
            now,
        );
        mouse(
            &mut state,
            &runtime,
            MouseEventKind::Drag(LEFT),
            4,
            0,
            false,
            now,
        );
        assert_eq!(
            mouse(
                &mut state,
                &runtime,
                MouseEventKind::Up(LEFT),
                4,
                0,
                false,
                now
            ),
            MouseOutcome::Handled
        );
        assert!(state.selection().is_some_and(Selection::is_visible));
        assert_eq!(state.take_copy_key(&runtime, b"x"), None);
        assert_eq!(
            state.take_copy_key(&runtime, b"\x03"),
            Some("hello".to_owned())
        );
        assert!(state.selection().is_none());
    }

    #[tokio::test]
    async fn plain_click_leaves_no_selection() {
        let runtime = TerminalRuntime::test_with_screen_bytes(20, 5, b"hello world");
        let mut state = AttachSelection::default();
        let now = Instant::now();

        mouse(
            &mut state,
            &runtime,
            MouseEventKind::Down(LEFT),
            2,
            0,
            true,
            now,
        );
        assert_eq!(
            mouse(
                &mut state,
                &runtime,
                MouseEventKind::Up(LEFT),
                2,
                0,
                true,
                now
            ),
            MouseOutcome::Handled
        );
        assert!(state.selection().is_none());
    }

    #[tokio::test]
    async fn double_click_copies_the_word_and_briefly_keeps_its_highlight() {
        let runtime = TerminalRuntime::test_with_screen_bytes(20, 5, b"hello world");
        let mut state = AttachSelection::default();
        let now = Instant::now();

        for _ in 0..2 {
            mouse(
                &mut state,
                &runtime,
                MouseEventKind::Down(LEFT),
                8,
                0,
                true,
                now,
            );
        }
        assert_eq!(
            mouse(
                &mut state,
                &runtime,
                MouseEventKind::Up(LEFT),
                8,
                0,
                true,
                now
            ),
            MouseOutcome::Copy("world".to_owned())
        );
        assert!(state.selection().is_some_and(Selection::is_visible));
        assert!(state.tick(&runtime, now + WORD_HIGHLIGHT));
        assert!(state.selection().is_none());
    }

    #[tokio::test]
    async fn dragging_on_the_top_row_scrolls_the_selection_into_scrollback() {
        let mut bytes = Vec::new();
        for line in 0..20 {
            bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
        }
        let runtime = TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);
        let mut state = AttachSelection::default();
        let now = Instant::now();

        mouse(
            &mut state,
            &runtime,
            MouseEventKind::Down(LEFT),
            6,
            3,
            true,
            now,
        );
        mouse(
            &mut state,
            &runtime,
            MouseEventKind::Drag(LEFT),
            0,
            0,
            true,
            now,
        );
        let at = state.deadline().expect("autoscroll deadline");
        assert!(state.tick(&runtime, at));
        assert!(state.tick(&runtime, state.deadline().expect("next autoscroll")));
        assert_eq!(
            runtime
                .scroll_metrics()
                .map(|metrics| metrics.offset_from_bottom),
            Some(2)
        );

        let MouseOutcome::Copy(text) = mouse(
            &mut state,
            &runtime,
            MouseEventKind::Up(LEFT),
            0,
            0,
            true,
            now,
        ) else {
            panic!("expected a copy");
        };
        assert_eq!(text, "line 14\nline 15\nline 16\nline 17\nline 18\nline 19");
        assert!(state.deadline().is_none());
    }

    #[tokio::test]
    async fn a_program_that_reports_the_mouse_receives_the_press() {
        let runtime = TerminalRuntime::test_with_screen_bytes(20, 5, b"\x1b[?1000h\x1b[?1006h");
        let mut state = AttachSelection::default();

        assert_eq!(
            mouse(
                &mut state,
                &runtime,
                MouseEventKind::Down(LEFT),
                2,
                0,
                true,
                Instant::now()
            ),
            MouseOutcome::Forward
        );
        assert!(state.selection().is_none());
    }
}
