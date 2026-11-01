mod encode;
mod model;
pub(crate) mod mouse;
mod parse;

#[allow(unused_imports)]
pub use encode::encode_terminal_key;
#[cfg(any(unix, test))]
pub use model::MouseProtocolMode;
pub use model::WindowsKeyRecord;
pub use model::{KeyboardProtocol, MouseProtocolEncoding, TerminalKey, TextCommit};
pub use parse::parse_terminal_key_sequence;
