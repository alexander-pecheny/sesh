use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsKeyRecord {
    pub key_down: bool,
    pub repeat_count: u16,
    pub virtual_key_code: u16,
    pub virtual_scan_code: u16,
    pub unicode: u16,
    pub control_key_state: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextCommit {
    text: String,
}

impl TextCommit {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PhysicalKeyId(u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeySource {
    Synthesized,
    Vt {
        bytes: Vec<u8>,
    },
    #[cfg(any(windows, test))]
    WindowsConsole {
        record: WindowsKeyRecord,
        physical_key: Option<PhysicalKeyId>,
    },
}

#[cfg(any(windows, test))]
impl WindowsKeyRecord {
    fn physical_key_id(self) -> Option<PhysicalKeyId> {
        const ENHANCED_KEY: u32 = 0x0100;
        (self.virtual_scan_code != 0).then(|| {
            PhysicalKeyId(
                u32::from(self.virtual_scan_code)
                    | (u32::from(self.control_key_state & ENHANCED_KEY != 0) << 16),
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalKey {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub kind: crossterm::event::KeyEventKind,
    pub repeat_count: u16,
    pub shifted_codepoint: Option<u32>,
    pub generated_text: Option<String>,
    physical_identity_hint: bool,
    windows_dead_key: bool,
    source: KeySource,
}

impl TerminalKey {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self {
            code,
            modifiers,
            kind: crossterm::event::KeyEventKind::Press,
            repeat_count: 1,
            shifted_codepoint: None,
            generated_text: None,
            physical_identity_hint: false,
            windows_dead_key: false,
            source: KeySource::Synthesized,
        }
    }

    pub fn with_kind(mut self, kind: crossterm::event::KeyEventKind) -> Self {
        if kind == crossterm::event::KeyEventKind::Release {
            self.repeat_count = 1;
            self.generated_text = None;
        }
        self.kind = kind;
        self
    }

    pub fn with_repeat_count(mut self, repeat_count: u16) -> Self {
        self.repeat_count = if self.kind == crossterm::event::KeyEventKind::Release {
            1
        } else {
            repeat_count.max(1)
        };
        self
    }

    pub(crate) fn with_modifiers(mut self, modifiers: KeyModifiers) -> Self {
        self.modifiers = modifiers;
        self
    }

    pub fn with_shifted_codepoint(mut self, shifted_codepoint: u32) -> Self {
        self.shifted_codepoint = Some(shifted_codepoint);
        self
    }

    pub(crate) fn with_generated_text(mut self, text: Option<String>) -> Self {
        self.generated_text = if self.kind == crossterm::event::KeyEventKind::Release {
            None
        } else {
            text
        };
        self
    }

    pub(crate) fn with_vt_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.source = KeySource::Vt { bytes };
        self
    }

    #[cfg(any(windows, test))]
    pub fn with_windows_record(mut self, record: WindowsKeyRecord) -> Self {
        self = self.with_windows_composition_hint(Some(record));
        self.repeat_count = if self.kind == crossterm::event::KeyEventKind::Release {
            1
        } else {
            record.repeat_count.max(1)
        };
        let physical_key = record.physical_key_id();
        self.physical_identity_hint = physical_key.is_some();
        self.source = KeySource::WindowsConsole {
            physical_key,
            record,
        };
        self
    }

    pub(crate) fn with_windows_composition_hint(
        mut self,
        record: Option<WindowsKeyRecord>,
    ) -> Self {
        // AltGr is normalized to text-only modifiers by the Windows input mapper.
        // Command chords can also have zero Unicode, so retain their fallback keys.
        self.windows_dead_key = matches!(self.code, KeyCode::Char(_))
            && self.modifiers.difference(KeyModifiers::SHIFT).is_empty()
            && record.is_some_and(|record| record.unicode == 0);
        self
    }

    pub(crate) fn with_physical_identity_hint(mut self, physical: bool) -> Self {
        self.physical_identity_hint = physical;
        self
    }

    pub(crate) fn is_windows_dead_key(&self) -> bool {
        self.windows_dead_key
    }

    pub(crate) fn has_physical_identity(&self) -> bool {
        self.physical_identity_hint || self.physical_key_id().is_some()
    }

    pub(crate) fn physical_key_id(&self) -> Option<u32> {
        match &self.source {
            #[cfg(any(windows, test))]
            KeySource::WindowsConsole {
                physical_key: Some(PhysicalKeyId(id)),
                ..
            } => Some(*id),
            #[cfg(any(windows, test))]
            KeySource::WindowsConsole {
                physical_key: None, ..
            } => None,
            KeySource::Synthesized | KeySource::Vt { .. } => None,
        }
    }

    pub fn with_text_commit(mut self) -> Self {
        let has_text_only_modifiers = match self.code {
            KeyCode::Char(ch) if ch.is_uppercase() => {
                self.modifiers == KeyModifiers::SHIFT || self.modifiers.is_empty()
            }
            KeyCode::Char(_) => self.modifiers.is_empty(),
            _ => false,
        };
        if has_text_only_modifiers && self.kind == crossterm::event::KeyEventKind::Press {
            self.generated_text = match self.code {
                KeyCode::Char(ch) => Some(ch.to_string()),
                _ => None,
            };
        }
        self
    }

    pub fn as_key_event(&self) -> KeyEvent {
        KeyEvent::new_with_kind(self.code, self.modifiers, self.kind)
    }
}

impl From<KeyEvent> for TerminalKey {
    fn from(value: KeyEvent) -> Self {
        Self::new(value.code, value.modifiers).with_kind(value.kind)
    }
}

pub(crate) const KITTY_FLAG_REPORT_ALL_KEYS: u16 = 0b0000_1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardProtocol {
    Legacy,
    Kitty { flags: u16 },
}

impl KeyboardProtocol {
    pub fn from_kitty_flags(flags: u16) -> Self {
        if flags == 0 {
            Self::Legacy
        } else {
            Self::Kitty { flags }
        }
    }

    pub(crate) fn reports_event_types(self) -> bool {
        matches!(self, Self::Kitty { flags } if flags & 0b0000_0010 != 0)
    }

    pub(crate) fn reports_all_keys(self) -> bool {
        matches!(self, Self::Kitty { flags } if flags & KITTY_FLAG_REPORT_ALL_KEYS != 0)
    }
}

#[cfg(any(unix, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseProtocolMode {
    None,
    Press,
    PressRelease,
    ButtonMotion,
    AnyMotion,
}

#[cfg(any(unix, test))]
impl MouseProtocolMode {
    #[cfg(test)]
    pub fn reporting_enabled(self) -> bool {
        self != Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseProtocolEncoding {
    Default,
    Utf8,
    Sgr,
    SgrPixels,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_uppercase_with_shift_is_committed_text() {
        let key = TerminalKey::new(KeyCode::Char('É'), KeyModifiers::SHIFT).with_text_commit();

        assert_eq!(key.generated_text.as_deref(), Some("É"));
    }

    #[test]
    fn protocol_from_zero_flags_is_legacy() {
        assert_eq!(
            KeyboardProtocol::from_kitty_flags(0),
            KeyboardProtocol::Legacy
        );
    }

    #[test]
    fn protocol_from_nonzero_flags_is_kitty() {
        assert_eq!(
            KeyboardProtocol::from_kitty_flags(7),
            KeyboardProtocol::Kitty { flags: 7 }
        );
    }
}
