pub mod client;
mod event_hub;
pub mod schema;
mod server;
mod status;
mod subscriptions;
mod wait;

pub use event_hub::EventHub;
pub(crate) use server::start_server_with_stop_control;
pub use server::ServerHandle;
pub use status::RuntimeStatus;

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::api::schema::Request;

pub const SOCKET_PATH_ENV_VAR: &str = "SESH_MUX_SOCKET_PATH";

pub struct ApiRequestMessage {
    pub request: Request,
    pub respond_to: std::sync::mpsc::Sender<String>,
    pub response_write_complete: Option<std::sync::mpsc::Receiver<()>>,
}

pub type ApiRequestSender = mpsc::UnboundedSender<ApiRequestMessage>;

pub fn socket_path() -> PathBuf {
    crate::session::active_api_socket_path()
}
