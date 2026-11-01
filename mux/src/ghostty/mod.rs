#[allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    clippy::all,
    rustdoc::all
)]
pub mod bindings;

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::os::raw::c_char;
use std::ptr;
use std::slice;
use std::sync::{Mutex, Once};

pub use bindings as ffi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(ffi::GhosttyResult);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatterFormat {
    Plain,
    Vt,
}

impl FormatterFormat {
    fn as_raw(self) -> ffi::GhosttyFormatterFormat {
        match self {
            Self::Plain => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            Self::Vt => ffi::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ghostty error {}", self.0)
    }
}

trait GhosttyResultExt {
    fn into_result(self) -> Result<(), Error>;
}

impl GhosttyResultExt for ffi::GhosttyResult {
    fn into_result(self) -> Result<(), Error> {
        if self == ffi::GhosttyResult_GHOSTTY_SUCCESS {
            Ok(())
        } else {
            Err(Error(self))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    Clean,
    Partial,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalCompressionResult {
    Unsupported,
    Pending,
    Complete,
}

impl Dirty {
    fn as_raw(self) -> ffi::GhosttyRenderStateDirty {
        match self {
            Self::Clean => ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE,
            Self::Partial => ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL,
            Self::Full => ffi::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    fn as_raw(self) -> ffi::GhosttyColorScheme {
        match self {
            Self::Light => ffi::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_LIGHT,
            Self::Dark => ffi::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK,
        }
    }
}

pub const MOD_SHIFT: u16 = ffi::GHOSTTY_MODS_SHIFT as u16;
pub const MOD_CTRL: u16 = ffi::GHOSTTY_MODS_CTRL as u16;
pub const MOD_ALT: u16 = ffi::GHOSTTY_MODS_ALT as u16;
pub const MOD_SUPER: u16 = ffi::GHOSTTY_MODS_SUPER as u16;

pub const MOUSE_ACTION_PRESS: ffi::GhosttyMouseAction =
    ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS;
pub const MOUSE_ACTION_RELEASE: ffi::GhosttyMouseAction =
    ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_RELEASE;
pub const MOUSE_ACTION_MOTION: ffi::GhosttyMouseAction =
    ffi::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION;
pub const MOUSE_BUTTON_LEFT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT;
pub const MOUSE_BUTTON_RIGHT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_RIGHT;
pub const MOUSE_BUTTON_MIDDLE: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_MIDDLE;
pub const MOUSE_BUTTON_WHEEL_UP: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FOUR;
pub const MOUSE_BUTTON_WHEEL_DOWN: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FIVE;
pub const MOUSE_BUTTON_WHEEL_LEFT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SIX;
pub const MOUSE_BUTTON_WHEEL_RIGHT: ffi::GhosttyMouseButton =
    ffi::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_SEVEN;
pub const MOUSE_FORMAT_SGR: ffi::GhosttyMouseFormat =
    ffi::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_SGR;
pub const MOUSE_FORMAT_SGR_PIXELS: ffi::GhosttyMouseFormat =
    ffi::GhosttyMouseFormat_GHOSTTY_MOUSE_FORMAT_SGR_PIXELS;

pub const MODE_APPLICATION_CURSOR_KEYS: u16 = 1;
pub const MODE_FOCUS_EVENT: u16 = 1004;
pub const MODE_MOUSE_UTF8: u16 = 1005;
pub const MODE_MOUSE_SGR: u16 = 1006;
pub const MODE_MOUSE_ALTERNATE_SCROLL: u16 = 1007;
pub const MODE_MOUSE_SGR_PIXELS: u16 = 1016;
pub const MODE_BRACKETED_PASTE: u16 = 2004;
pub const MODE_SYNCHRONIZED_OUTPUT: u16 = 2026;
pub const MODE_GRAPHEME_CLUSTER: u16 = 2027;
pub const MODE_COLOR_SCHEME_REPORT: u16 = 2031;
// These are documented in vendor/libghostty-vt/include/ghostty/vt/terminal.h,
// but the generated bindings do not currently expose named constants for them.
const TERMINAL_DATA_COLOR_FOREGROUND: ffi::GhosttyTerminalData = 18;
const TERMINAL_DATA_COLOR_CURSOR: ffi::GhosttyTerminalData = 20;

const KITTY_IMAGE_STORAGE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const APC_MAX_BYTES: usize = 16 * 1024 * 1024;
const APC_MAX_BYTES_KITTY: usize = 16 * 1024 * 1024;
pub(crate) const KITTY_UNICODE_PLACEHOLDER: u32 = 0x10EEEE;
// The vendored C headers expose these placement fields, but the checked-in
// generated bindings predate the names. Keep the explicit values aligned with
// vendor/libghostty-vt/include/ghostty/vt/kitty_graphics.h.

static INSTALL_PNG_DECODER: Once = Once::new();

#[derive(Debug, Clone, Copy)]
struct KittyImageFingerprintEntry {
    generation: u64,
    fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorVisualStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

impl CursorVisualStyle {
    fn from_raw(value: ffi::GhosttyRenderStateCursorVisualStyle) -> Self {
        match value {
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
                Self::Block
            }
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                Self::Underline
            }
            ffi::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
                Self::BlockHollow
            }
            _ => Self::Bar,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveScreen {
    Primary,
    Alternate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalScrollbar {
    pub total: usize,
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorViewport {
    pub x: u16,
    pub y: u16,
    pub wide_tail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderCursor {
    pub viewport: Option<CursorViewport>,
    pub visible: bool,
    pub blinking: bool,
    pub visual_style: CursorVisualStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<ffi::GhosttyColorRgb> for RgbColor {
    fn from(value: ffi::GhosttyColorRgb) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

pub fn default_palette() -> [RgbColor; 256] {
    let mut palette = [ffi::GhosttyColorRgb::default(); 256];
    unsafe { ffi::ghostty_color_palette_default(palette.as_mut_ptr()) };
    palette.map(Into::into)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellColor {
    Palette(u8),
    Rgb(RgbColor),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub fg_color: Option<CellColor>,
    pub bg_color: Option<CellColor>,
    pub underline_color: Option<CellColor>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: u8,
    pub underlined: bool,
}

impl From<ffi::GhosttyStyle> for CellStyle {
    fn from(value: ffi::GhosttyStyle) -> Self {
        Self {
            fg_color: cell_color_from_style_color(value.fg_color),
            bg_color: cell_color_from_style_color(value.bg_color),
            underline_color: cell_color_from_style_color(value.underline_color),
            bold: value.bold,
            italic: value.italic,
            faint: value.faint,
            blink: value.blink,
            inverse: value.inverse,
            invisible: value.invisible,
            strikethrough: value.strikethrough,
            overline: value.overline,
            underline: normalize_underline_style(value.underline),
            underlined: value.underline != 0,
        }
    }
}

fn normalize_underline_style(value: std::os::raw::c_int) -> u8 {
    match value {
        0..=5 => value as u8,
        _ => 1,
    }
}

fn cell_color_from_style_color(color: ffi::GhosttyStyleColor) -> Option<CellColor> {
    match color.tag {
        ffi::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            // SAFETY: Ghostty's tagged union stores `palette` when the tag is PALETTE.
            Some(CellColor::Palette(unsafe { color.value.palette }))
        }
        ffi::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            // SAFETY: Ghostty's tagged union stores `rgb` when the tag is RGB.
            Some(CellColor::Rgb(unsafe { color.value.rgb }.into()))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderColors {
    pub background: RgbColor,
    pub foreground: RgbColor,
    pub palette: [RgbColor; 256],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellWide {
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScreenTextCell {
    pub wide: CellWide,
    pub graphemes: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScreenTextRow {
    pub cells: Vec<ScreenTextCell>,
    pub soft_wrapped: bool,
    pub wrap_continuation: bool,
}

impl CellWide {
    fn from_raw(value: ffi::GhosttyCellWide) -> Self {
        match value {
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW => Self::Narrow,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => Self::Wide,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => Self::SpacerTail,
            ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => Self::SpacerHead,
            _ => Self::Narrow,
        }
    }
}

type WritePtyCallback = dyn FnMut(&[u8]) + Send;

const MAX_CLIPBOARD_BYTES: usize = 192 * 1024;

#[derive(Default)]
struct TerminalCallbackState {
    write_pty: Option<Box<WritePtyCallback>>,
    bell_count: u16,
    pwd_changes: Vec<Vec<u8>>,
    clipboard_writes: Vec<Vec<u8>>,
    size_report: ffi::GhosttySizeReportSize,
    color_scheme: Option<ColorScheme>,
}

unsafe extern "C" fn bell_trampoline(_terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    // SAFETY: userdata is the TerminalCallbackState installed with this terminal.
    let state = unsafe { &mut *userdata.cast::<TerminalCallbackState>() };
    state.bell_count = state.bell_count.saturating_add(1);
}

unsafe extern "C" fn color_scheme_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut ffi::GhosttyColorScheme,
) -> bool {
    if userdata.is_null() || out_scheme.is_null() {
        return false;
    }
    let state = unsafe { &*userdata.cast::<TerminalCallbackState>() };
    let Some(color_scheme) = state.color_scheme else {
        return false;
    };
    unsafe {
        out_scheme.write(color_scheme.as_raw());
    }
    true
}

unsafe extern "C" fn size_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut ffi::GhosttySizeReportSize,
) -> bool {
    if userdata.is_null() || out_size.is_null() {
        return false;
    }
    let state = unsafe { &*userdata.cast::<TerminalCallbackState>() };
    let size = state.size_report;
    if size.rows == 0 || size.columns == 0 || size.cell_width == 0 || size.cell_height == 0 {
        return false;
    }
    unsafe {
        out_size.write(size);
    }
    true
}

unsafe extern "C" fn write_pty_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() || (data.is_null() && len != 0) {
        return;
    }
    let state = unsafe { &mut *(userdata.cast::<TerminalCallbackState>()) };
    let Some(callback) = state.write_pty.as_mut() else {
        return;
    };
    let bytes = if len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(data, len) }
    };
    callback(bytes);
}

unsafe extern "C" fn clipboard_write_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const ffi::GhosttyClipboardWrite,
) {
    if write.is_null() || unsafe { (*write).size } < mem::size_of::<ffi::GhosttyClipboardWrite>() {
        return;
    }
    // The reply and request are borrowed only until this callback returns.
    let Some(reply) = (unsafe { (*write).reply }) else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: libghostty-vt owns these values for the synchronous callback.
        unsafe { capture_clipboard_write(userdata, write) }
    }))
    .unwrap_or(ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA);
    let response = ffi::GhosttyClipboardWriteReply {
        size: mem::size_of::<ffi::GhosttyClipboardWriteReply>(),
        result,
        remember: false,
    };
    unsafe { reply(write, &response) };
}

unsafe fn capture_clipboard_write(
    userdata: *mut c_void,
    write: *const ffi::GhosttyClipboardWrite,
) -> ffi::GhosttyClipboardWriteResult {
    if userdata.is_null() || write.is_null() {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }

    let required_size = std::mem::offset_of!(ffi::GhosttyClipboardWrite, contents_len)
        + std::mem::size_of::<usize>();
    // SAFETY: size is the leading field of the live request.
    if unsafe { (*write).size } < required_size {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }
    // SAFETY: the size check covers every field accessed below.
    let request = unsafe { &*write };
    if request.location != ffi::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_STANDARD {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }

    // SAFETY: userdata is the TerminalCallbackState installed with this terminal.
    let state = unsafe { &mut *userdata.cast::<TerminalCallbackState>() };
    if request.contents_len == 0 {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    }
    if request.contents_len != 1 {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }
    if request.contents.is_null() {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }

    // SAFETY: libghostty-vt keeps the single content and its strings alive for the callback.
    let content = unsafe { &*request.contents };
    // SAFETY: the MIME string is borrowed from the live callback request.
    let Some(mime) = (unsafe { borrowed_bytes(content.mime) }) else {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    };
    let is_text = std::str::from_utf8(mime)
        .ok()
        .and_then(|mime| mime.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/plain"));
    if !is_text {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }

    // SAFETY: the data string is borrowed from the live callback request.
    let Some(bytes) = (unsafe { borrowed_bytes(content.data) }) else {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    };
    if bytes.is_empty() {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    }
    if bytes.len() > MAX_CLIPBOARD_BYTES {
        return ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA;
    }
    state.clipboard_writes.push(bytes.to_vec());
    ffi::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS
}

unsafe fn borrowed_bytes<'a>(value: ffi::GhosttyString) -> Option<&'a [u8]> {
    if value.len == 0 {
        Some(&[])
    } else if value.ptr.is_null() {
        None
    } else {
        // SAFETY: the callback contract keeps pointer and length valid until return.
        Some(unsafe { slice::from_raw_parts(value.ptr, value.len) })
    }
}

unsafe extern "C" fn pwd_changed_trampoline(terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    if terminal.is_null() || userdata.is_null() {
        return;
    }
    let mut pwd = ffi::GhosttyString::default();
    let result = unsafe {
        ffi::ghostty_terminal_get(
            terminal,
            ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD,
            (&mut pwd as *mut ffi::GhosttyString).cast(),
        )
    };
    if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || (pwd.ptr.is_null() && pwd.len != 0) {
        return;
    }
    let bytes = if pwd.len == 0 {
        Vec::new()
    } else {
        unsafe { slice::from_raw_parts(pwd.ptr, pwd.len) }.to_vec()
    };
    let state = unsafe { &mut *(userdata.cast::<TerminalCallbackState>()) };
    state.pwd_changes.push(bytes);
}

fn install_png_decoder_once() {
    INSTALL_PNG_DECODER.call_once(|| unsafe {
        let _ = ffi::ghostty_sys_set(
            ffi::GhosttySysOption_GHOSTTY_SYS_OPT_DECODE_PNG,
            (decode_png_trampoline as *const ()).cast(),
        );
    });
}

unsafe extern "C" fn decode_png_trampoline(
    _userdata: *mut c_void,
    allocator: *const ffi::GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut ffi::GhosttySysImage,
) -> bool {
    if data.is_null() || out.is_null() {
        return false;
    }
    let bytes = unsafe { slice::from_raw_parts(data, data_len) };
    let Some(rgba) = decode_png_rgba(bytes) else {
        return false;
    };
    let ptr = unsafe { ffi::ghostty_alloc(allocator, rgba.data.len()) };
    if ptr.is_null() {
        return false;
    }
    unsafe {
        ptr::copy_nonoverlapping(rgba.data.as_ptr(), ptr, rgba.data.len());
        *out = ffi::GhosttySysImage {
            width: rgba.width,
            height: rgba.height,
            data: ptr,
            data_len: rgba.data.len(),
        };
    }
    true
}

struct DecodedPng {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn decode_png_rgba(bytes: &[u8]) -> Option<DecodedPng> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let frame = &buf[..info.buffer_size()];
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }

    let data = match info.color_type {
        png::ColorType::Rgba => frame.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for rgb in frame.as_chunks::<3>().0 {
                out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for gray in frame {
                out.extend_from_slice(&[*gray, *gray, *gray, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((info.width as usize) * (info.height as usize) * 4);
            for ga in frame.as_chunks::<2>().0 {
                out.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
            }
            out
        }
        png::ColorType::Indexed => return None,
    };

    Some(DecodedPng {
        width: info.width,
        height: info.height,
        data,
    })
}

pub fn unicode_codepoint_width(codepoint: u32) -> u8 {
    unsafe { ffi::ghostty_unicode_codepoint_width(codepoint) }
}

pub struct Terminal {
    raw: ffi::GhosttyTerminal,
    max_scrollback: usize,
    callback_state: Box<TerminalCallbackState>,
    kitty_fingerprints: Mutex<HashMap<u32, KittyImageFingerprintEntry>>,
    kitty_empty_generation: Cell<Option<u64>>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: valid out pointer, null allocator means default allocator.
        unsafe {
            ffi::ghostty_terminal_new(ptr::null(), &mut raw, cols, rows).into_result()?;
        }

        let mut terminal = Self {
            raw,
            max_scrollback,
            #[cfg(windows)]
            tracked_row: ptr::null_mut(),
            callback_state: Box::new(TerminalCallbackState {
                size_report: ffi::GhosttySizeReportSize {
                    rows,
                    columns: cols,
                    ..Default::default()
                },
                ..Default::default()
            }),
            kitty_fingerprints: Mutex::new(HashMap::new()),
            kitty_empty_generation: Cell::new(None),
        };
        let userdata = (&mut *terminal.callback_state as *mut TerminalCallbackState).cast();
        let glyph_protocol = false;
        let terminfo_name = ffi::GhosttyString {
            ptr: crate::pane::PANE_TERM.as_ptr().cast(),
            len: crate::pane::PANE_TERM.len(),
        };
        let grapheme_default = ffi::GhosttyTerminalModeConfig {
            mode: MODE_GRAPHEME_CLUSTER,
            value: true,
        };
        let clipboard_callback: ffi::GhosttyTerminalClipboardWriteFn =
            Some(clipboard_write_trampoline);
        unsafe {
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
                (&terminfo_name as *const ffi::GhosttyString).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                (&max_scrollback as *const usize).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
                (&grapheme_default as *const ffi::GhosttyTerminalModeConfig).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA,
                userdata,
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SIZE,
                (size_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_BELL,
                (bell_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
                (pwd_changed_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
                clipboard_callback.map_or(ptr::null(), |callback| (callback as *const ()).cast()),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
                (color_scheme_trampoline as *const ()).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                terminal.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL,
                (&glyph_protocol as *const bool).cast(),
            )
            .into_result()?;
        }
        Ok(terminal)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        // SAFETY: self.raw is a live terminal handle for self's lifetime.
        unsafe {
            ffi::ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len());
        }
    }

    pub(crate) fn compression_activity(&self) -> Result<u64, Error> {
        let mut activity = 0;
        // SAFETY: self.raw is a live terminal handle and activity is a valid out pointer.
        unsafe {
            ffi::ghostty_terminal_compression_activity(self.raw, &mut activity).into_result()?;
        }
        Ok(activity)
    }

    pub(crate) fn compress_incremental(&mut self) -> Result<TerminalCompressionResult, Error> {
        let mut result =
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED;
        // SAFETY: self.raw is a live terminal handle and result is a valid out pointer.
        unsafe {
            ffi::ghostty_terminal_compress(
                self.raw,
                ffi::GhosttyTerminalCompressionMode_GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL,
                &mut result,
            )
            .into_result()?;
        }
        match result {
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED => {
                Ok(TerminalCompressionResult::Unsupported)
            }
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING => {
                Ok(TerminalCompressionResult::Pending)
            }
            ffi::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE => {
                Ok(TerminalCompressionResult::Complete)
            }
            _ => Err(Error(ffi::GhosttyResult_GHOSTTY_INVALID_VALUE)),
        }
    }

    pub fn set_default_palette(&mut self, palette: &[RgbColor; 256]) -> Result<(), Error> {
        let palette = palette.map(|color| ffi::GhosttyColorRgb {
            r: color.r,
            g: color.g,
            b: color.b,
        });
        unsafe {
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                palette.as_ptr().cast(),
            )
            .into_result()
        }
    }

    pub fn default_palette(&self) -> Result<[RgbColor; 256], Error> {
        let mut out = [ffi::GhosttyColorRgb::default(); 256];
        // SAFETY: self.raw is a live terminal handle, and out is exactly the
        // 256-entry array this data kind writes.
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT,
                out.as_mut_ptr().cast(),
            )
            .into_result()?;
        }
        Ok(out.map(Into::into))
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), Error> {
        let size_report = ffi::GhosttySizeReportSize {
            rows,
            columns: cols,
            cell_width: cell_width_px,
            cell_height: cell_height_px,
        };
        // SAFETY: self.raw is valid and sizes are plain values.
        unsafe {
            ffi::ghostty_terminal_resize(
                self.raw,
                cols,
                rows,
                cell_width_px.max(1),
                cell_height_px.max(1),
            )
            .into_result()?;
        }
        self.callback_state.size_report = size_report;
        Ok(())
    }

    pub fn enable_kitty_graphics(&mut self) -> Result<(), Error> {
        install_png_decoder_once();
        let storage_limit = KITTY_IMAGE_STORAGE_LIMIT_BYTES;
        let enable_medium = true;
        let temp_dir = std::env::temp_dir();
        let temp_dir = temp_dir.to_string_lossy();
        let temp_dir = ffi::GhosttyString {
            ptr: temp_dir.as_ptr().cast(),
            len: temp_dir.len(),
        };
        unsafe {
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
                (&storage_limit as *const u64).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE,
                (&enable_medium as *const bool).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE,
                (&temp_dir as *const ffi::GhosttyString).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM,
                (&enable_medium as *const bool).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES,
                (&APC_MAX_BYTES as *const usize).cast(),
            )
            .into_result()?;
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES_KITTY,
                (&APC_MAX_BYTES_KITTY as *const usize).cast(),
            )
            .into_result()?;
        }
        Ok(())
    }

    pub fn set_write_pty_callback<F>(&mut self, callback: F) -> Result<(), Error>
    where
        F: FnMut(&[u8]) + Send + 'static,
    {
        unsafe {
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                (write_pty_trampoline as *const ()).cast(),
            )
            .into_result()?;
        }
        self.callback_state.write_pty = Some(Box::new(callback));
        Ok(())
    }

    pub fn set_color_scheme(&mut self, color_scheme: Option<ColorScheme>) -> Option<ColorScheme> {
        mem::replace(&mut self.callback_state.color_scheme, color_scheme)
    }

    pub fn take_bell_count(&mut self) -> u16 {
        mem::take(&mut self.callback_state.bell_count)
    }

    pub fn take_pwd_changes(&mut self) -> Vec<Vec<u8>> {
        mem::take(&mut self.callback_state.pwd_changes)
    }

    pub fn take_clipboard_writes(&mut self) -> Vec<Vec<u8>> {
        mem::take(&mut self.callback_state.clipboard_writes)
    }

    pub fn mode_get(&self, mode: u16) -> Result<bool, Error> {
        let mut config = ffi::GhosttyTerminalModeConfig { mode, value: false };
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MODE,
                (&mut config as *mut ffi::GhosttyTerminalModeConfig).cast(),
            )
            .into_result()?;
        }
        Ok(config.value)
    }

    pub fn mode_set(&mut self, mode: u16, value: bool) -> Result<(), Error> {
        let config = ffi::GhosttyTerminalModeConfig { mode, value };
        unsafe {
            ffi::ghostty_terminal_set(
                self.raw,
                ffi::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE,
                (&config as *const ffi::GhosttyTerminalModeConfig).cast(),
            )
            .into_result()
        }
    }

    pub fn kitty_keyboard_flags(&self) -> Result<u8, Error> {
        let mut out = 0u8;
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                (&mut out as *mut u8).cast(),
            )
            .into_result()?;
        }
        Ok(out)
    }

    pub fn mouse_tracking_enabled(&self) -> Result<bool, Error> {
        self.get_bool(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING)
    }

    pub fn modify_other_keys_enabled(&self) -> Result<bool, Error> {
        self.get_bool(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MODIFY_OTHER_KEYS)
    }

    pub fn active_screen(&self) -> Result<ActiveScreen, Error> {
        let mut out = ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                (&mut out as *mut ffi::GhosttyTerminalScreen).cast(),
            )
            .into_result()?;
        }
        Ok(match out {
            ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY => ActiveScreen::Primary,
            ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE => ActiveScreen::Alternate,
            _ => ActiveScreen::Primary,
        })
    }

    pub fn total_rows(&self) -> Result<usize, Error> {
        self.get_usize(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS)
    }

    pub fn scrollbar(&self) -> Result<TerminalScrollbar, Error> {
        let mut out = ffi::GhosttyTerminalScrollbar::default();
        unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                (&mut out as *mut ffi::GhosttyTerminalScrollbar).cast(),
            )
            .into_result()?;
        }
        Ok(TerminalScrollbar {
            total: out.total as usize,
            offset: out.offset as usize,
            len: out.len as usize,
        })
    }

    pub fn screen_cell(&self, x: u16, y: u32) -> Result<(CellWide, Vec<u32>), Error> {
        let grid_ref = self.grid_ref(ghostty_screen_point(x, y))?;
        let wide = grid_ref_wide(&grid_ref)?;
        let graphemes = grid_ref_graphemes(&grid_ref)?;
        Ok((wide, graphemes))
    }

    pub(crate) fn screen_text_rows(&self) -> Result<Vec<ScreenTextRow>, Error> {
        self.screen_text_rows_range(0, usize::MAX)
    }

    pub(crate) fn screen_text_rows_range(
        &self,
        start_row: usize,
        end_row_exclusive: usize,
    ) -> Result<Vec<ScreenTextRow>, Error> {
        let total_rows = self.total_rows()?;
        let start_row = start_row.min(total_rows);
        let end_row_exclusive = end_row_exclusive.min(total_rows).max(start_row);
        let cols = self.cols()?;
        let mut rows = Vec::with_capacity(end_row_exclusive.saturating_sub(start_row));
        for y in start_row..end_row_exclusive {
            let Some(y) = u32::try_from(y).ok() else {
                break;
            };
            let mut grid_ref = self.grid_ref(ghostty_screen_point(0, y))?;
            let (soft_wrapped, wrap_continuation) = grid_ref_wrap_state(&grid_ref)?;
            let mut cells = Vec::with_capacity(usize::from(cols));
            for x in 0..cols {
                grid_ref.x = x;
                cells.push(ScreenTextCell {
                    wide: grid_ref_wide(&grid_ref)?,
                    graphemes: grid_ref_graphemes(&grid_ref)?,
                });
            }
            rows.push(ScreenTextRow {
                cells,
                soft_wrapped,
                wrap_continuation,
            });
        }
        Ok(rows)
    }

    pub fn viewport_hyperlink_uri(&self, x: u16, y: u32) -> Result<Option<String>, Error> {
        let grid_ref = self.grid_ref(ghostty_viewport_point(x, y))?;
        grid_ref_hyperlink_uri(&grid_ref)
    }

    fn grid_ref(&self, point: ffi::GhosttyPoint) -> Result<ffi::GhosttyGridRef, Error> {
        let mut grid_ref = ffi::GhosttyGridRef {
            size: mem::size_of::<ffi::GhosttyGridRef>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_terminal_grid_ref(self.raw, point, &mut grid_ref).into_result()?;
        }
        Ok(grid_ref)
    }

    pub fn read_ansi_viewport(
        &self,
        start: (u16, u32),
        end: (u16, u32),
        rectangle: bool,
    ) -> Result<String, Error> {
        self.read_formatted_selection(
            ghostty_viewport_point(start.0, start.1),
            ghostty_viewport_point(end.0, end.1),
            rectangle,
            FormatterFormat::Vt,
            false,
            true,
        )
    }

    pub fn read_text_screen(
        &self,
        start: (u16, u32),
        end: (u16, u32),
        rectangle: bool,
    ) -> Result<String, Error> {
        self.read_formatted_selection(
            ghostty_screen_point(start.0, start.1),
            ghostty_screen_point(end.0, end.1),
            rectangle,
            FormatterFormat::Plain,
            true,
            true,
        )
    }

    pub fn read_ansi_screen(
        &self,
        start: (u16, u32),
        end: (u16, u32),
        rectangle: bool,
        unwrap: bool,
    ) -> Result<String, Error> {
        self.read_formatted_selection(
            ghostty_screen_point(start.0, start.1),
            ghostty_screen_point(end.0, end.1),
            rectangle,
            FormatterFormat::Vt,
            unwrap,
            true,
        )
    }

    fn read_formatted_selection(
        &self,
        start: ffi::GhosttyPoint,
        end: ffi::GhosttyPoint,
        rectangle: bool,
        format: FormatterFormat,
        unwrap: bool,
        trim: bool,
    ) -> Result<String, Error> {
        let mut start_ref = ffi::GhosttyGridRef {
            size: mem::size_of::<ffi::GhosttyGridRef>(),
            ..Default::default()
        };
        let mut end_ref = ffi::GhosttyGridRef {
            size: mem::size_of::<ffi::GhosttyGridRef>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_terminal_grid_ref(self.raw, start, &mut start_ref).into_result()?;
            ffi::ghostty_terminal_grid_ref(self.raw, end, &mut end_ref).into_result()?;
        }

        let selection = ffi::GhosttySelection {
            size: mem::size_of::<ffi::GhosttySelection>(),
            start: start_ref,
            end: end_ref,
            rectangle,
        };
        self.format_selection(&selection, format, unwrap, trim)
    }

    fn format_selection(
        &self,
        selection: &ffi::GhosttySelection,
        format: FormatterFormat,
        unwrap: bool,
        trim: bool,
    ) -> Result<String, Error> {
        let mut formatter: ffi::GhosttyFormatter = ptr::null_mut();
        let options = ffi::GhosttyFormatterTerminalOptions {
            size: mem::size_of::<ffi::GhosttyFormatterTerminalOptions>(),
            emit: format.as_raw(),
            unwrap,
            trim,
            extra: ffi::GhosttyFormatterTerminalExtra {
                size: mem::size_of::<ffi::GhosttyFormatterTerminalExtra>(),
                screen: ffi::GhosttyFormatterScreenExtra {
                    size: mem::size_of::<ffi::GhosttyFormatterScreenExtra>(),
                    ..Default::default()
                },
                ..Default::default()
            },
            selection,
        };
        unsafe {
            ffi::ghostty_formatter_terminal_new(ptr::null(), &mut formatter, self.raw, options)
                .into_result()?;
        }

        let mut out_ptr = ptr::null_mut();
        let mut out_len = 0usize;
        let result = unsafe {
            ffi::ghostty_formatter_format_alloc(formatter, ptr::null(), &mut out_ptr, &mut out_len)
        };
        unsafe {
            ffi::ghostty_formatter_free(formatter);
        }
        result.into_result()?;

        let text = if out_len == 0 {
            String::new()
        } else {
            let bytes = unsafe { slice::from_raw_parts(out_ptr.cast_const(), out_len) };
            String::from_utf8_lossy(bytes).into_owned()
        };

        if !out_ptr.is_null() {
            unsafe {
                ffi::ghostty_free(ptr::null(), out_ptr, out_len);
            }
        }

        Ok(text)
    }

    pub fn scroll_viewport_bottom(&mut self) {
        let viewport = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
            value: ffi::GhosttyTerminalScrollViewportValue::default(),
        };
        // SAFETY: self.raw is valid and viewport value matches the tag.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.raw, viewport);
        }
    }

    pub fn scroll_viewport_delta(&mut self, delta: isize) {
        let viewport = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_DELTA,
            value: ffi::GhosttyTerminalScrollViewportValue { delta },
        };
        // SAFETY: self.raw is valid and viewport value matches the tag.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.raw, viewport);
        }
    }

    pub fn scroll_viewport_row(&mut self, row: usize) {
        let viewport = ffi::GhosttyTerminalScrollViewport {
            tag: ffi::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_ROW,
            value: ffi::GhosttyTerminalScrollViewportValue { row },
        };
        // SAFETY: self.raw is valid and viewport value matches the tag.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.raw, viewport);
        }
    }

    pub fn cols(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS)
    }

    pub fn rows(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS)
    }

    pub fn cursor_y(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_Y)
    }

    pub fn effective_foreground_color(&self) -> Result<Option<RgbColor>, Error> {
        self.get_optional_rgb_color(TERMINAL_DATA_COLOR_FOREGROUND)
    }

    pub fn effective_cursor_color(&self) -> Result<Option<RgbColor>, Error> {
        self.get_optional_rgb_color(TERMINAL_DATA_COLOR_CURSOR)
    }

    pub(crate) fn width_px(&self) -> Result<u32, Error> {
        self.get_u32(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_WIDTH_PX)
    }

    pub(crate) fn height_px(&self) -> Result<u32, Error> {
        self.get_u32(ffi::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_HEIGHT_PX)
    }

    fn get_u16(&self, data: ffi::GhosttyTerminalData) -> Result<u16, Error> {
        let mut out = 0u16;
        // SAFETY: out points to a u16 matching the requested terminal data type.
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut u16).cast())
                .into_result()?;
        }
        Ok(out)
    }

    fn get_u32(&self, data: ffi::GhosttyTerminalData) -> Result<u32, Error> {
        let mut out = 0u32;
        // SAFETY: out points to a u32 matching the requested terminal data type.
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut u32).cast())
                .into_result()?;
        }
        Ok(out)
    }

    fn get_usize(&self, data: ffi::GhosttyTerminalData) -> Result<usize, Error> {
        let mut out = 0usize;
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut usize).cast())
                .into_result()?;
        }
        Ok(out)
    }

    fn get_bool(&self, data: ffi::GhosttyTerminalData) -> Result<bool, Error> {
        let mut out = false;
        unsafe {
            ffi::ghostty_terminal_get(self.raw, data, (&mut out as *mut bool).cast())
                .into_result()?;
        }
        Ok(out)
    }

    fn get_optional_rgb_color(
        &self,
        data: ffi::GhosttyTerminalData,
    ) -> Result<Option<RgbColor>, Error> {
        let mut out = ffi::GhosttyColorRgb::default();
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.raw,
                data,
                (&mut out as *mut ffi::GhosttyColorRgb).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS => Ok(Some(out.into())),
            ffi::GhosttyResult_GHOSTTY_NO_VALUE => Ok(None),
            other => Err(Error(other)),
        }
    }

    fn raw(&self) -> ffi::GhosttyTerminal {
        self.raw
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for Terminal {}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            ffi::ghostty_terminal_free(self.raw);
        }
    }
}

fn ghostty_viewport_point(x: u16, y: u32) -> ffi::GhosttyPoint {
    ffi::GhosttyPoint {
        tag: ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_VIEWPORT,
        value: ffi::GhosttyPointValue {
            coordinate: ffi::GhosttyPointCoordinate { x, y },
        },
    }
}

fn ghostty_screen_point(x: u16, y: u32) -> ffi::GhosttyPoint {
    ffi::GhosttyPoint {
        tag: ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
        value: ffi::GhosttyPointValue {
            coordinate: ffi::GhosttyPointCoordinate { x, y },
        },
    }
}

fn grid_ref_graphemes(grid_ref: &ffi::GhosttyGridRef) -> Result<Vec<u32>, Error> {
    let mut required = 0usize;
    let result =
        unsafe { ffi::ghostty_grid_ref_graphemes(grid_ref, ptr::null_mut(), 0, &mut required) };
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        result.into_result()?;
    }
    let mut buffer = vec![0u32; required];
    if required == 0 {
        return Ok(buffer);
    }
    unsafe {
        ffi::ghostty_grid_ref_graphemes(grid_ref, buffer.as_mut_ptr(), buffer.len(), &mut required)
            .into_result()?;
    }
    buffer.truncate(required);
    Ok(buffer)
}

fn grid_ref_wide(grid_ref: &ffi::GhosttyGridRef) -> Result<CellWide, Error> {
    let mut raw = ffi::GhosttyCell::default();
    unsafe {
        ffi::ghostty_grid_ref_cell(grid_ref, &mut raw).into_result()?;
    }

    let mut wide = ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
    unsafe {
        ffi::ghostty_cell_get(
            raw,
            ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
            (&mut wide as *mut ffi::GhosttyCellWide).cast(),
        )
        .into_result()?;
    }
    Ok(CellWide::from_raw(wide))
}

fn grid_ref_wrap_state(grid_ref: &ffi::GhosttyGridRef) -> Result<(bool, bool), Error> {
    let mut row = 0;
    unsafe {
        ffi::ghostty_grid_ref_row(grid_ref, &mut row).into_result()?;
    }
    let mut soft_wrapped = false;
    let mut wrap_continuation = false;
    unsafe {
        ffi::ghostty_row_get(
            row,
            ffi::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP,
            (&mut soft_wrapped as *mut bool).cast(),
        )
        .into_result()?;
        ffi::ghostty_row_get(
            row,
            ffi::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
            (&mut wrap_continuation as *mut bool).cast(),
        )
        .into_result()?;
    }
    Ok((soft_wrapped, wrap_continuation))
}

// Hashes the full payload. Callers cache the result per image id and only
// recompute it when the image's transmit time changes.

fn grid_ref_hyperlink_uri(grid_ref: &ffi::GhosttyGridRef) -> Result<Option<String>, Error> {
    let mut required = 0usize;
    let result =
        unsafe { ffi::ghostty_grid_ref_hyperlink_uri(grid_ref, ptr::null_mut(), 0, &mut required) };
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        result.into_result()?;
    }
    if required == 0 {
        return Ok(None);
    }
    let mut buffer = vec![0u8; required];
    unsafe {
        ffi::ghostty_grid_ref_hyperlink_uri(
            grid_ref,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut required,
        )
        .into_result()?;
    }
    buffer.truncate(required);
    Ok(Some(String::from_utf8_lossy(&buffer).into_owned()))
}

pub struct RenderState {
    raw: ffi::GhosttyRenderState,
}

impl RenderState {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: valid out pointer and null allocator use default allocator.
        unsafe {
            ffi::ghostty_render_state_new(ptr::null(), &mut raw).into_result()?;
        }
        Ok(Self { raw })
    }

    pub fn update(&mut self, terminal: &Terminal) -> Result<(), Error> {
        // SAFETY: both handles are valid for the duration of the call.
        unsafe { ffi::ghostty_render_state_update(self.raw, terminal.raw()).into_result() }
    }

    pub fn rows(&self) -> Result<u16, Error> {
        self.get_u16(ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS)
    }

    pub fn cursor(&self) -> Result<RenderCursor, Error> {
        let mut cursor = ffi::GhosttyRenderStateCursor {
            size: mem::size_of::<ffi::GhosttyRenderStateCursor>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_render_state_get(
                self.raw,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR,
                (&mut cursor as *mut ffi::GhosttyRenderStateCursor).cast(),
            )
            .into_result()?;
        }
        Ok(RenderCursor {
            // Upstream leaves these fields undefined when the cursor is offscreen.
            viewport: if cursor.viewport_has_value {
                Some(CursorViewport {
                    x: cursor.viewport_x,
                    y: cursor.viewport_y,
                    wide_tail: cursor.wide_tail,
                })
            } else {
                None
            },
            visible: cursor.visible,
            blinking: cursor.blinking,
            visual_style: CursorVisualStyle::from_raw(cursor.visual_style),
        })
    }

    pub fn colors(&self) -> Result<RenderColors, Error> {
        let mut colors = ffi::GhosttyRenderStateColors {
            size: mem::size_of::<ffi::GhosttyRenderStateColors>(),
            ..Default::default()
        };
        unsafe {
            ffi::ghostty_render_state_get(
                self.raw,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLORS,
                (&mut colors as *mut ffi::GhosttyRenderStateColors).cast(),
            )
            .into_result()?;
        }
        Ok(RenderColors {
            background: colors.background.into(),
            foreground: colors.foreground.into(),
            palette: colors.palette.map(Into::into),
        })
    }

    pub fn clean(&mut self) -> Result<(), Error> {
        unsafe { ffi::ghostty_render_state_clean(self.raw).into_result() }
    }

    pub fn set_dirty(&mut self, dirty: Dirty) -> Result<(), Error> {
        let value = dirty.as_raw();
        // SAFETY: value pointer matches the expected option type.
        unsafe {
            ffi::ghostty_render_state_set(
                self.raw,
                ffi::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                (&value as *const ffi::GhosttyRenderStateDirty).cast(),
            )
            .into_result()
        }
    }

    pub fn populate_row_iterator<'a>(
        &'a self,
        iterator: &'a mut RowIterator,
    ) -> Result<RowIter<'a>, Error> {
        // SAFETY: iterator raw handle is valid and will not outlive self.
        unsafe {
            ffi::ghostty_render_state_get(
                self.raw,
                ffi::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                (&mut iterator.raw as *mut ffi::GhosttyRenderStateRowIterator).cast(),
            )
            .into_result()?;
        }
        Ok(RowIter {
            iterator,
            _state: PhantomData,
        })
    }

    fn get_u16(&self, data: ffi::GhosttyRenderStateData) -> Result<u16, Error> {
        let mut out = 0u16;
        // SAFETY: out points to a u16 matching the requested render-state data type.
        unsafe {
            ffi::ghostty_render_state_get(self.raw, data, (&mut out as *mut u16).cast())
                .into_result()?;
        }
        Ok(out)
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for RenderState {}

impl Drop for RenderState {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            ffi::ghostty_render_state_free(self.raw);
        }
    }
}

pub struct KeyEvent {
    raw: ffi::GhosttyKeyEvent,
}

impl KeyEvent {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_key_event_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_action(&mut self, action: ffi::GhosttyKeyAction) {
        unsafe { ffi::ghostty_key_event_set_action(self.raw, action) }
    }

    pub fn set_key(&mut self, key: ffi::GhosttyKey) {
        unsafe { ffi::ghostty_key_event_set_key(self.raw, key) }
    }

    pub fn set_mods(&mut self, mods: u16) {
        unsafe { ffi::ghostty_key_event_set_mods(self.raw, mods) }
    }

    pub fn set_utf8(&mut self, text: &str) {
        unsafe {
            ffi::ghostty_key_event_set_utf8(self.raw, text.as_ptr().cast::<c_char>(), text.len())
        }
    }

    pub fn set_unshifted_codepoint(&mut self, codepoint: u32) {
        unsafe { ffi::ghostty_key_event_set_unshifted_codepoint(self.raw, codepoint) }
    }
}

impl Drop for KeyEvent {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_key_event_free(self.raw) }
    }
}

pub struct KeyEncoder {
    raw: ffi::GhosttyKeyEncoder,
}

impl KeyEncoder {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_key_encoder_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_from_terminal(&mut self, terminal: &Terminal) {
        unsafe { ffi::ghostty_key_encoder_setopt_from_terminal(self.raw, terminal.raw()) }
    }

    pub fn encode(&mut self, event: &KeyEvent) -> Result<Vec<u8>, Error> {
        encode_with_retry(|buf, len, out_len| unsafe {
            ffi::ghostty_key_encoder_encode(self.raw, event.raw, buf, len, out_len)
        })
    }
}

// SAFETY: the opaque encoder handle is only used behind external synchronization in pane runtime.
unsafe impl Send for KeyEncoder {}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_key_encoder_free(self.raw) }
    }
}

pub struct MouseEvent {
    raw: ffi::GhosttyMouseEvent,
}

impl MouseEvent {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_mouse_event_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_action(&mut self, action: ffi::GhosttyMouseAction) {
        unsafe { ffi::ghostty_mouse_event_set_action(self.raw, action) }
    }

    pub fn set_button(&mut self, button: ffi::GhosttyMouseButton) {
        unsafe { ffi::ghostty_mouse_event_set_button(self.raw, button) }
    }

    pub fn clear_button(&mut self) {
        unsafe { ffi::ghostty_mouse_event_clear_button(self.raw) }
    }

    pub fn set_mods(&mut self, mods: u16) {
        unsafe { ffi::ghostty_mouse_event_set_mods(self.raw, mods) }
    }

    pub fn set_position(&mut self, x: f32, y: f32) {
        unsafe {
            ffi::ghostty_mouse_event_set_position(self.raw, ffi::GhosttyMousePosition { x, y })
        }
    }
}

impl Drop for MouseEvent {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_mouse_event_free(self.raw) }
    }
}

pub struct MouseEncoder {
    raw: ffi::GhosttyMouseEncoder,
}

impl MouseEncoder {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        unsafe { ffi::ghostty_mouse_encoder_new(ptr::null(), &mut raw).into_result()? };
        Ok(Self { raw })
    }

    pub fn set_from_terminal(&mut self, terminal: &Terminal) {
        unsafe { ffi::ghostty_mouse_encoder_setopt_from_terminal(self.raw, terminal.raw()) }
    }

    pub fn set_size(
        &mut self,
        screen_width: u32,
        screen_height: u32,
        cell_width: u32,
        cell_height: u32,
    ) {
        let size = ffi::GhosttyMouseEncoderSize {
            size: std::mem::size_of::<ffi::GhosttyMouseEncoderSize>(),
            screen_width,
            screen_height,
            cell_width,
            cell_height,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.raw,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&size as *const ffi::GhosttyMouseEncoderSize).cast(),
            )
        }
    }

    pub fn set_format(&mut self, format: ffi::GhosttyMouseFormat) {
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.raw,
                ffi::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_FORMAT,
                (&format as *const ffi::GhosttyMouseFormat).cast(),
            )
        }
    }

    pub fn encode(&mut self, event: &MouseEvent) -> Result<Vec<u8>, Error> {
        encode_with_retry(|buf, len, out_len| unsafe {
            ffi::ghostty_mouse_encoder_encode(self.raw, event.raw, buf, len, out_len)
        })
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_mouse_encoder_free(self.raw) }
    }
}

fn encode_with_retry(
    mut encode: impl FnMut(*mut c_char, usize, *mut usize) -> ffi::GhosttyResult,
) -> Result<Vec<u8>, Error> {
    let mut required = 0usize;
    let result = encode(ptr::null_mut(), 0, &mut required);
    if result != ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        result.into_result()?;
    }
    let mut buffer = vec![0u8; required.max(16)];
    let mut written = 0usize;
    encode(
        buffer.as_mut_ptr().cast::<c_char>(),
        buffer.len(),
        &mut written,
    )
    .into_result()?;
    buffer.truncate(written);
    Ok(buffer)
}

pub struct RowIterator {
    raw: ffi::GhosttyRenderStateRowIterator,
}

impl RowIterator {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: valid out pointer and null allocator use default allocator.
        unsafe {
            ffi::ghostty_render_state_row_iterator_new(ptr::null(), &mut raw).into_result()?;
        }
        Ok(Self { raw })
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for RowIterator {}

impl Drop for RowIterator {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            ffi::ghostty_render_state_row_iterator_free(self.raw);
        }
    }
}

pub struct RowIter<'a> {
    iterator: &'a mut RowIterator,
    _state: PhantomData<&'a RenderState>,
}

impl<'a> RowIter<'a> {
    pub fn next(&mut self) -> bool {
        // SAFETY: iterator handle is valid while self is alive.
        unsafe { ffi::ghostty_render_state_row_iterator_next(self.iterator.raw) }
    }

    pub fn clear_dirty(&mut self) -> Result<(), Error> {
        self.set_dirty(false)
    }

    pub fn set_dirty(&mut self, dirty: bool) -> Result<(), Error> {
        // SAFETY: dirty pointer matches the expected row option type.
        unsafe {
            ffi::ghostty_render_state_row_set(
                self.iterator.raw,
                ffi::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                (&dirty as *const bool).cast(),
            )
            .into_result()
        }
    }

    pub fn populate_cells<'b>(
        &'b mut self,
        cells: &'b mut RowCells,
    ) -> Result<RowCellIter<'b>, Error> {
        // SAFETY: cells raw handle is valid and will not outlive the current row borrow.
        unsafe {
            ffi::ghostty_render_state_row_get(
                self.iterator.raw,
                ffi::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                (&mut cells.raw as *mut ffi::GhosttyRenderStateRowCells).cast(),
            )
            .into_result()?;
        }
        Ok(RowCellIter { cells })
    }
}

pub struct RowCells {
    raw: ffi::GhosttyRenderStateRowCells,
}

impl RowCells {
    pub fn new() -> Result<Self, Error> {
        let mut raw = ptr::null_mut();
        // SAFETY: valid out pointer and null allocator use default allocator.
        unsafe {
            ffi::ghostty_render_state_row_cells_new(ptr::null(), &mut raw).into_result()?;
        }
        Ok(Self { raw })
    }
}

// SAFETY: these opaque handles are only used behind external synchronization in pane runtime.
unsafe impl Send for RowCells {}

impl Drop for RowCells {
    fn drop(&mut self) {
        // SAFETY: freeing a null or live handle is allowed by the C API.
        unsafe {
            ffi::ghostty_render_state_row_cells_free(self.raw);
        }
    }
}

pub struct RowCellIter<'a> {
    cells: &'a mut RowCells,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellBasicData {
    pub wide: CellWide,
    pub has_hyperlink: bool,
    pub has_styling: bool,
    pub style: CellStyle,
}

impl Default for CellBasicData {
    fn default() -> Self {
        Self {
            wide: CellWide::Narrow,
            has_hyperlink: false,
            has_styling: false,
            style: CellStyle::default(),
        }
    }
}

impl<'a> RowCellIter<'a> {
    pub fn next(&mut self) -> bool {
        // SAFETY: cells handle is valid while self is alive.
        unsafe { ffi::ghostty_render_state_row_cells_next(self.cells.raw) }
    }

    fn raw_cell(&self) -> Result<ffi::GhosttyCell, Error> {
        let mut raw = ffi::GhosttyCell::default();
        unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                (&mut raw as *mut ffi::GhosttyCell).cast(),
            )
            .into_result()?;
        }
        Ok(raw)
    }

    pub fn basic_data(&self) -> Result<CellBasicData, Error> {
        let mut raw = ffi::GhosttyCell::default();
        let mut style = ffi::GhosttyStyle {
            size: mem::size_of::<ffi::GhosttyStyle>(),
            ..Default::default()
        };
        let mut has_styling = false;
        let row_keys = [
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
        ];
        let mut row_values = [
            (&mut raw as *mut ffi::GhosttyCell).cast::<c_void>(),
            (&mut style as *mut ffi::GhosttyStyle).cast::<c_void>(),
            (&mut has_styling as *mut bool).cast::<c_void>(),
        ];
        let mut written = 0usize;
        unsafe {
            ffi::ghostty_render_state_row_cells_get_multi(
                self.cells.raw,
                row_keys.len(),
                row_keys.as_ptr(),
                row_values.as_mut_ptr(),
                &mut written,
            )
            .into_result()?;
        }

        let mut wide = ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
        let mut has_hyperlink = false;
        let cell_keys = [
            ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
            ffi::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK,
        ];
        let mut cell_values = [
            (&mut wide as *mut ffi::GhosttyCellWide).cast::<c_void>(),
            (&mut has_hyperlink as *mut bool).cast::<c_void>(),
        ];
        unsafe {
            ffi::ghostty_cell_get_multi(
                raw,
                cell_keys.len(),
                cell_keys.as_ptr(),
                cell_values.as_mut_ptr(),
                &mut written,
            )
            .into_result()?;
        }

        Ok(CellBasicData {
            wide: CellWide::from_raw(wide),
            has_hyperlink,
            has_styling,
            style: style.into(),
        })
    }

    pub fn wide(&self) -> Result<CellWide, Error> {
        let raw = self.raw_cell()?;
        let mut wide = ffi::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
                (&mut wide as *mut ffi::GhosttyCellWide).cast(),
            )
            .into_result()?;
        }
        Ok(CellWide::from_raw(wide))
    }

    pub fn has_hyperlink(&self) -> Result<bool, Error> {
        let raw = self.raw_cell()?;
        let mut has_hyperlink = false;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK,
                (&mut has_hyperlink as *mut bool).cast(),
            )
            .into_result()?;
        }
        Ok(has_hyperlink)
    }

    pub fn content_bg_color(&self) -> Result<Option<CellColor>, Error> {
        let raw = self.raw_cell()?;
        let mut tag = ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
                (&mut tag as *mut ffi::GhosttyCellContentTag).cast(),
            )
            .into_result()?;
        }

        match tag {
            ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
                let mut index = 0u8;
                unsafe {
                    ffi::ghostty_cell_get(
                        raw,
                        ffi::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_PALETTE,
                        (&mut index as *mut u8).cast(),
                    )
                    .into_result()?;
                }
                Ok(Some(CellColor::Palette(index)))
            }
            ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
                let mut color = ffi::GhosttyColorRgb::default();
                unsafe {
                    ffi::ghostty_cell_get(
                        raw,
                        ffi::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_RGB,
                        (&mut color as *mut ffi::GhosttyColorRgb).cast(),
                    )
                    .into_result()?;
                }
                Ok(Some(CellColor::Rgb(color.into())))
            }
            _ => Ok(None),
        }
    }

    pub fn fg_color(&self) -> Result<Option<RgbColor>, Error> {
        let mut color = ffi::GhosttyColorRgb::default();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                (&mut color as *mut ffi::GhosttyColorRgb).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS => Ok(Some(color.into())),
            ffi::GhosttyResult_GHOSTTY_INVALID_VALUE => Ok(None),
            other => Err(Error(other)),
        }
    }

    pub fn bg_color(&self) -> Result<Option<RgbColor>, Error> {
        let mut color = ffi::GhosttyColorRgb::default();
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                (&mut color as *mut ffi::GhosttyColorRgb).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS => Ok(Some(color.into())),
            ffi::GhosttyResult_GHOSTTY_INVALID_VALUE => Ok(None),
            other => Err(Error(other)),
        }
    }

    fn raw_cell_text_into(&self, text: &mut String) -> Result<(), Error> {
        let raw = self.raw_cell()?;
        let mut has_text = false;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_TEXT,
                (&mut has_text as *mut bool).cast(),
            )
            .into_result()?;
        }
        if !has_text {
            return Ok(());
        }

        let mut codepoint = 0u32;
        unsafe {
            ffi::ghostty_cell_get(
                raw,
                ffi::GhosttyCellData_GHOSTTY_CELL_DATA_CODEPOINT,
                (&mut codepoint as *mut u32).cast(),
            )
            .into_result()?;
        }
        if let Some(ch) = char::from_u32(codepoint) {
            text.push(ch);
        }
        Ok(())
    }

    pub fn grapheme_text(&self) -> Result<String, Error> {
        let mut bytes = Vec::new();
        let mut text = String::new();
        self.grapheme_text_into(&mut bytes, &mut text)?;
        Ok(text)
    }

    pub fn grapheme_text_into(&self, bytes: &mut Vec<u8>, text: &mut String) -> Result<(), Error> {
        text.clear();
        bytes.clear();

        let mut buffer = ffi::GhosttyBuffer {
            ptr: ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                (&mut buffer as *mut ffi::GhosttyBuffer).cast(),
            )
        };
        match result {
            ffi::GhosttyResult_GHOSTTY_SUCCESS if buffer.len == 0 => {
                return self.raw_cell_text_into(text);
            }
            ffi::GhosttyResult_GHOSTTY_SUCCESS => {
                return Err(Error(ffi::GhosttyResult_GHOSTTY_INVALID_VALUE));
            }
            ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE => {}
            other => return Err(Error(other)),
        }

        if buffer.len == 0 {
            return self.raw_cell_text_into(text);
        }
        bytes.resize(buffer.len, 0);
        let mut buffer = ffi::GhosttyBuffer {
            ptr: bytes.as_mut_ptr(),
            cap: bytes.len(),
            len: 0,
        };
        unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.cells.raw,
                ffi::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                (&mut buffer as *mut ffi::GhosttyBuffer).cast(),
            )
            .into_result()?;
        }
        if buffer.len > bytes.len() {
            return Err(Error(ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE));
        }
        bytes.truncate(buffer.len);
        match std::str::from_utf8(bytes) {
            Ok(value) => text.push_str(value),
            Err(_) => text.push_str(&String::from_utf8_lossy(bytes)),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_compression_preserves_cold_scrollback() {
        let mut terminal = Terminal::new(80, 24, 20_000_000).unwrap();
        let initial_activity = terminal.compression_activity().unwrap();
        let suffix = "x".repeat(66);
        for line in 1..=10_000 {
            terminal.write(format!("{line:05} {suffix}\r\n").as_bytes());
        }
        assert_ne!(terminal.compression_activity().unwrap(), initial_activity);

        let mut complete = false;
        for _ in 0..10_000 {
            match terminal.compress_incremental().unwrap() {
                TerminalCompressionResult::Unsupported => return,
                TerminalCompressionResult::Pending => {}
                TerminalCompressionResult::Complete => {
                    complete = true;
                    break;
                }
            }
        }
        assert!(complete, "incremental compression did not converge");

        let oldest = terminal.read_text_screen((0, 0), (79, 0), false).unwrap();
        assert!(oldest.starts_with("00001 "));
        let last_row = terminal.total_rows().unwrap() as u32 - 1;
        let newest = terminal
            .read_text_screen((0, last_row - 1), (79, last_row), false)
            .unwrap();
        assert!(newest.contains("10000"));
    }
}
