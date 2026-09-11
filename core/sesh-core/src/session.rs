//! What both transports share: the handle the app holds, and the events it receives.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy)]
pub enum State {
    Connecting,
    Authenticating,
    Connected,
    Closed,
    Failed,
    Bootstrapping,
    /// The saved password was refused; the app forgets it and waits for the prompt.
    PasswordRejected,
}

pub struct Prompt {
    pub prompt: String,
    pub echo: bool,
}

/// Every method is called from a tokio worker thread.
pub trait Events: Send + Sync + 'static {
    fn output(&self, bytes: &[u8]);
    fn state(&self, state: State, message: &str);
    fn host_key(&self, fingerprint: &str, previous: Option<&str>);
    fn auth_prompt(&self, id: u32, name: &str, instruction: &str, prompts: &[Prompt]);
}

pub enum Command {
    Write(Vec<u8>),
    Resize(u16, u16),
    Close,
}

#[derive(Default)]
pub struct Answers {
    pub host_key: Mutex<Option<oneshot::Sender<bool>>>,
    pub prompt: Mutex<Option<(u32, oneshot::Sender<Vec<String>>)>>,
    next_id: AtomicU32,
}

/// Everything a transport's driver is handed when it starts.
pub struct Context {
    pub events: Arc<dyn Events>,
    pub answers: Arc<Answers>,
    pub commands: mpsc::UnboundedReceiver<Command>,
}

pub struct Session {
    commands: mpsc::UnboundedSender<Command>,
    answers: Arc<Answers>,
}

pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

impl Session {
    pub fn start<F>(
        events: Arc<dyn Events>,
        drive: impl FnOnce(Context) -> F + Send + 'static,
    ) -> Session
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let (commands, rx) = mpsc::unbounded_channel();
        let answers = Arc::new(Answers::default());
        let session = Session {
            commands,
            answers: answers.clone(),
        };
        runtime().spawn(async move {
            let context = Context {
                events: events.clone(),
                answers,
                commands: rx,
            };
            match drive(context).await {
                Ok(()) => events.state(State::Closed, ""),
                Err(message) => events.state(State::Failed, &message),
            }
        });
        session
    }

    pub fn write(&self, bytes: &[u8]) {
        let _ = self.commands.send(Command::Write(bytes.to_vec()));
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.commands.send(Command::Resize(cols, rows));
    }

    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }

    pub fn answer_host_key(&self, accept: bool) {
        if let Some(tx) = self.answers.host_key.lock().unwrap().take() {
            let _ = tx.send(accept);
        }
    }

    pub fn answer_prompt(&self, id: u32, answers: Option<Vec<String>>) {
        let mut slot = self.answers.prompt.lock().unwrap();
        if slot.as_ref().is_some_and(|(pending, _)| *pending == id) {
            if let (Some((_, tx)), Some(answers)) = (slot.take(), answers) {
                let _ = tx.send(answers);
            }
        }
    }
}

pub async fn ask(
    events: &Arc<dyn Events>,
    answers: &Arc<Answers>,
    name: &str,
    instruction: &str,
    prompts: Vec<Prompt>,
) -> Result<Vec<String>, String> {
    if prompts.is_empty() {
        return Ok(Vec::new());
    }
    let id = answers.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let (tx, rx) = oneshot::channel();
    *answers.prompt.lock().unwrap() = Some((id, tx));
    events.auth_prompt(id, name, instruction, &prompts);
    rx.await.map_err(|_| "authentication cancelled".to_string())
}

/// A Remote command runs in the user's login shell, interactive, so it sees the PATH
/// and aliases their rc files set, as if typed at the prompt.
pub fn in_login_shell(command: &str) -> String {
    format!("exec \"$SHELL\" -lic {}", mosh::opts::shell_quote(command))
}

#[cfg(test)]
mod login_shell_tests {
    #[test]
    fn quotes_the_command_for_the_login_shell() {
        assert_eq!(super::in_login_shell("herdr"), r#"exec "$SHELL" -lic herdr"#);
        assert_eq!(
            super::in_login_shell("tmux new -A -s main"),
            r#"exec "$SHELL" -lic 'tmux new -A -s main'"#
        );
    }
}
