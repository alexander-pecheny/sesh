//! Frames for `terminal attach` clients, encoded as terminal ANSI diffs.

use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::layout::{Position, Rect, Size};

use crate::protocol::render_ansi::{BlitEncoder, EncodedBlit};
use crate::protocol::{CursorState, FrameData, RenderEncoding, ServerMessage, TerminalFrame};

/// Per-client terminal diff encoder and sequence number.
pub(crate) struct ClientRenderState {
    blit_encoder: BlitEncoder,
    seq: u64,
    repaint_pending: bool,
    screen: Option<ratatui::Terminal<CursorTrackingBackend>>,
}

impl ClientRenderState {
    pub(crate) fn new(_render_encoding: RenderEncoding) -> Self {
        Self {
            blit_encoder: BlitEncoder::new(),
            seq: 0,
            repaint_pending: false,
            screen: None,
        }
    }

    pub(crate) fn reset_baseline(&mut self) {
        self.blit_encoder = BlitEncoder::new();
        self.repaint_pending = false;
    }

    pub(crate) fn request_repaint(&mut self) {
        self.repaint_pending = true;
    }

    /// Renders into a screen kept between frames, so frames reuse its buffers.
    pub(crate) fn render_terminal(
        &mut self,
        runtime: &crate::terminal::TerminalRuntime,
        area: Rect,
    ) -> (&ratatui::buffer::Buffer, Option<CursorState>) {
        if self
            .screen
            .as_ref()
            .is_none_or(|screen| screen.backend().buffer().area != area)
        {
            let backend = CursorTrackingBackend::new(area.width, area.height);
            self.screen =
                Some(ratatui::Terminal::new(backend).expect("TestBackend::new should never fail"));
        }
        let terminal = self.screen.as_mut().expect("screen was just set");
        let suppress_cursor = runtime.synchronized_output_active();
        terminal
            .draw(|frame| {
                runtime.render(frame, area, true);
            })
            .expect("render to TestBackend should never fail");

        let cursor = (!suppress_cursor)
            .then(|| runtime.cursor_state(area, true))
            .flatten()
            .map(|cursor| CursorState {
                x: cursor.x,
                y: cursor.y,
                visible: cursor.visible
                    && runtime
                        .scroll_metrics()
                        .is_none_or(|metrics| metrics.offset_from_bottom == 0),
                shape: cursor.shape,
            })
            .or_else(|| {
                (!suppress_cursor)
                    .then(|| terminal.backend().rendered_cursor())
                    .flatten()
            });
        (terminal.backend().buffer(), cursor)
    }

    pub(crate) fn prepare_frame(&mut self, frame: FrameData) -> Option<PreparedRender> {
        if !self.repaint_pending && self.blit_encoder.is_current(&frame) {
            return None;
        }
        let encoded = self.blit_encoder.encode(&frame, self.repaint_pending);
        Some(PreparedRender {
            message: ServerMessage::Terminal(TerminalFrame {
                seq: self.seq + 1,
                width: frame.width,
                height: frame.height,
                full: encoded.full,
                bytes: encoded.bytes.clone(),
            }),
            frame,
            encoded,
        })
    }

    pub(crate) fn commit_sent_frame(&mut self, prepared: PreparedRender) {
        self.blit_encoder.commit(prepared.frame, prepared.encoded);
        self.seq += 1;
        self.repaint_pending = false;
    }
}

/// A prepared frame plus the encoder state to commit once it is sent.
pub(crate) struct PreparedRender {
    message: ServerMessage,
    frame: FrameData,
    encoded: EncodedBlit,
}

impl PreparedRender {
    pub(crate) fn message(&self) -> &ServerMessage {
        &self.message
    }
}

struct CursorTrackingBackend {
    inner: TestBackend,
    rendered_cursor: Option<Position>,
}

impl CursorTrackingBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
            rendered_cursor: None,
        }
    }

    fn buffer(&self) -> &ratatui::buffer::Buffer {
        self.inner.buffer()
    }

    fn rendered_cursor(&self) -> Option<CursorState> {
        self.rendered_cursor.map(|pos| CursorState {
            x: pos.x,
            y: pos.y,
            visible: true,
            shape: 0,
        })
    }
}

impl Backend for CursorTrackingBackend {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()?;
        self.rendered_cursor = None;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let position = position.into();
        self.inner.set_cursor_position(position)?;
        self.rendered_cursor = Some(position);
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

