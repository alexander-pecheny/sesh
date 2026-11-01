//! Session persistence — save/restore workspaces, layouts, and working directories.
//!
//! Stored at `~/.sesh/mux/session.json`.
//! Optional pane screen history is stored separately at `session-history.json`.

mod io;
mod restore;
mod snapshot;
mod writer;

pub use self::io::{load, load_history};
pub use self::restore::restore;
pub use self::restore::{handoff_pane_aliases, restore_handoff};
pub use self::snapshot::{
    capture, capture_history, DirectionSnapshot, LayoutSnapshot, SessionHistorySnapshot,
    SessionSnapshot, TabSnapshot, WorkspaceSnapshot,
};
pub(crate) use self::writer::SessionWriter;
