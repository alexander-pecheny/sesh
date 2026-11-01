//! The socket of the server Sesh ran before sesh-mux, kept open while panes imported from it
//! still run the agents whose hooks report there.

use super::*;

const LEGACY_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// The API socket's name beside the handoff socket of the server Sesh ran before sesh-mux.
const LEGACY_SOCKET_NAME: &str = "herdr.sock";

#[derive(Default)]
pub(crate) struct LegacyListener {
    pub(super) path: Option<PathBuf>,
    server: Option<api::ServerHandle>,
    next_check: Option<Instant>,
}

impl LegacyListener {
    pub(super) fn beside_handoff_socket(handoff_socket: &Path) -> Self {
        Self {
            path: handoff_socket
                .parent()
                .map(|dir| dir.join(LEGACY_SOCKET_NAME)),
            ..Self::default()
        }
    }

    pub(super) fn at(path: Option<PathBuf>) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }
}

impl HeadlessServer {
    pub(super) fn sync_legacy_listener(&mut self) {
        let Some(path) = self.legacy.path.clone() else {
            return;
        };
        let now = Instant::now();
        if self.legacy.next_check.is_some_and(|at| now < at) {
            return;
        }
        self.legacy.next_check = Some(now + LEGACY_CHECK_INTERVAL);
        if !self.app.refresh_legacy_agents() {
            info!(path = %path.display(), "no legacy agents remain; closing their socket");
            self.legacy = LegacyListener::default();
            return;
        }
        if self.legacy.server.is_some() || self.handoff_in_progress {
            return;
        }
        let Some(api_tx) = self.api_tx.clone() else {
            return;
        };
        match api::start_legacy_server(path.clone(), api_tx, self.app.event_hub.clone()) {
            Ok(server) => {
                info!(path = %path.display(), "serving legacy agents on their old socket");
                self.legacy.server = Some(server);
            }
            Err(err) => warn!(path = %path.display(), err = %err, "cannot serve legacy agents"),
        }
    }

    /// Frees the old socket so a replacement server can bind it.
    pub(super) fn release_legacy_socket(&mut self) {
        self.legacy.server = None;
        self.legacy.next_check = None;
    }
}
