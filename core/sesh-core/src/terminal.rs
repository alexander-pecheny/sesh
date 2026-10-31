//! A terminal on a Link's connection: over mosh where the Host is reached directly, so it
//! roams and predicts, and on a pty channel where a proxy stands between.

use std::sync::Arc;

use ::mosh::handshake::Handshake;
use russh::ChannelMsg;
use tokio::sync::{mpsc, oneshot};

use crate::flags::{Predict, RemoteIp};
use crate::link::Lent;
use crate::mosh;
use crate::session::{Command, Context, Events, Session, State};

/// Starts `command` in a terminal on `link`'s connection; `server` is a mosh-server on the
/// Host, or nil for a plain pty.
pub fn open(link: &Session, server: Option<String>, command: String, cols: u16, rows: u16, events: Arc<dyn Events>) -> Session {
    let lent = link.lend(command);
    Session::start(events, move |context| drive(lent, server, cols, rows, context))
}

async fn drive(lent: oneshot::Receiver<Lent>, server: Option<String>, cols: u16, rows: u16, context: Context) -> Result<(), String> {
    let Context { events, mut commands, .. } = context;
    let lent = lent.await.map_err(|_| "the connection to the Host is gone".to_string())?;
    events.state(State::Bootstrapping, "");
    match (lent.direct, server) {
        (true, Some(server)) => {
            let mut found = Handshake::default();
            let line = mosh::server_line(&server, None, RemoteIp::Remote, "", Some(lent.command.clone()));
            mosh::bootstrap(&lent.handle, &line, cols, rows, &events, &mut found).await?;
            drop(lent);
            mosh::run_client(&found, cols, rows, Predict::Adaptive, &events, &mut commands, |_| {}).await
        }
        _ => pty(&lent, cols, rows, &events, &mut commands).await,
    }
}

async fn pty(lent: &Lent, cols: u16, rows: u16, events: &Arc<dyn Events>, commands: &mut mpsc::UnboundedReceiver<Command>) -> Result<(), String> {
    let channel = lent.handle.channel_open_session().await.map_err(|e| format!("the connection to the Host is gone: {e}"))?;
    let (mut reader, writer) = channel.split();
    writer
        .request_pty(true, "xterm-256color", cols.max(1) as u32, rows.max(1) as u32, 0, 0, &[])
        .await
        .map_err(|e| format!("requesting a pty: {e}"))?;
    for (name, value) in &lent.env {
        let _ = writer.set_env(false, name.as_str(), value.as_str()).await;
    }
    writer.exec(true, lent.command.as_str()).await.map_err(|e| format!("starting the terminal: {e}"))?;
    events.state(State::Connected, "");
    loop {
        tokio::select! {
            message = reader.wait() => match message {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => events.output(&data),
                Some(ChannelMsg::Close) | Some(ChannelMsg::Eof) | None => return Ok(()),
                Some(_) => {}
            },
            command = commands.recv() => match command {
                Some(Command::Write(bytes)) => writer.data(&bytes[..]).await.map_err(|e| format!("writing to the remote: {e}"))?,
                Some(Command::Resize(cols, rows)) => {
                    let _ = writer.window_change(cols.max(1) as u32, rows.max(1) as u32, 0, 0).await;
                }
                Some(Command::Close) | None => return Ok(()),
                Some(_) => {}
            },
        }
    }
}
