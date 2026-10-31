mod activation;
mod control;
mod health;
mod message_policy;
mod registry;
mod writer;

pub(crate) use activation::*;
pub(crate) use control::*;
pub(crate) use message_policy::*;
pub(crate) use registry::*;
pub(crate) use writer::NativeEndpointTransport;

/// The endpoint a client talks to. Herdr connects only to the local server.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) enum ClientEndpointId {
    #[default]
    Local,
}

impl ClientEndpointId {
    pub(crate) fn is_local(&self) -> bool {
        true
    }

    pub(crate) fn storage_key(&self) -> String {
        "local".into()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientEndpointStatus {
    Connecting,
    Online,
}
