//! Agent forwarding: the app's Keys answered over `auth-agent@openssh.com` channels.

use russh::keys::agent::client::AgentClient;
use russh::keys::agent::server::serve;
use russh::keys::PrivateKey;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream};
use tokio::sync::mpsc;

const PIPE: usize = 64 * 1024;

#[derive(Clone)]
pub struct Agent {
    streams: mpsc::UnboundedSender<DuplexStream>,
}

/// russh's agent server owns its key store and only fills it from the wire, so the keys
/// are added by its own client over the first connection it is handed.
pub async fn start(keys: Vec<PrivateKey>) -> Result<Agent, String> {
    if keys.is_empty() {
        return Err("agent forwarding needs at least one Key".into());
    }
    let (streams, rx) = mpsc::unbounded_channel();
    let listener = Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|stream| (Ok(stream), rx))
    }));
    tokio::spawn(serve(listener, ()));

    let agent = Agent { streams };
    let mut client = AgentClient::connect(agent.pipe()?);
    for key in keys {
        client
            .add_identity(&key, &[])
            .await
            .map_err(|e| format!("adding a Key to the agent: {e}"))?;
    }
    Ok(agent)
}

impl Agent {
    pub fn attach<S>(&self, mut stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Ok(mut pipe) = self.pipe() else { return };
        tokio::spawn(async move {
            let _ = tokio::io::copy_bidirectional(&mut pipe, &mut stream).await;
        });
    }

    fn pipe(&self) -> Result<DuplexStream, String> {
        let (ours, theirs) = tokio::io::duplex(PIPE);
        self.streams
            .send(theirs)
            .map(|()| ours)
            .map_err(|_| "the agent stopped".to_string())
    }
}

pub fn decode(keys: &[(String, Option<String>)]) -> Result<Vec<PrivateKey>, String> {
    keys.iter()
        .map(|(pem, passphrase)| {
            russh::keys::decode_secret_key(pem, passphrase.as_deref())
                .map_err(|e| format!("reading a Key for the agent: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use russh::keys::ssh_key::rand_core::OsRng;

    #[tokio::test]
    async fn the_agent_lists_the_keys_it_was_given() {
        let key = super::PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap();
        let agent = super::start(vec![key.clone()]).await.unwrap();
        let mut client = super::AgentClient::connect(agent.pipe().unwrap());
        let listed = client.request_identities().await.unwrap();
        assert_eq!(listed, vec![key.public_key().clone()]);
    }
}
