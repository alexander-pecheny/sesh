//! Uploads: an image or video from the phone, written into `~/.sesh/uploads` over SFTP.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use russh::client;
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::session::{Aside, Answers, Events, Upload};
use crate::ssh::Dialer;

const DIRECTORY: &str = ".sesh/uploads";
const CHUNK: usize = 256 * 1024;

/// The Upload in flight, since the app allows only one per Session.
#[derive(Default)]
pub struct Pending {
    current: Option<(u32, Arc<AtomicBool>)>,
}

impl Pending {
    pub fn begin(&mut self, id: u32) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.current = Some((id, flag.clone()));
        flag
    }

    pub fn cancel(&mut self, id: u32) {
        if let Some((pending, flag)) = &self.current {
            if *pending == id {
                flag.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// An SSH Session already holds an authenticated handle, so an Upload costs one channel.
pub async fn over_ssh<H: client::Handler>(
    handle: Arc<client::Handle<H>>,
    id: u32,
    files: Vec<Upload>,
    cancelled: Arc<AtomicBool>,
    events: Arc<dyn Events>,
) {
    let result = run(open(handle.as_ref()).await, id, &files, &cancelled, &events).await;
    report(id, result, &cancelled, &events);
}

/// A mosh Session dropped its handle once mosh-server was running, so it dials again and
/// hangs up afterwards: mosh roams, and a socket kept open would be stale when wanted.
pub async fn over_mosh(
    dialer: Arc<Dialer>,
    id: u32,
    files: Vec<Upload>,
    cancelled: Arc<AtomicBool>,
    events: Arc<dyn Events>,
    answers: Arc<Answers>,
) {
    let aside: Arc<dyn Events> = Arc::new(Aside(events.clone()));
    let result = match dialer.connect(&aside, &answers).await {
        Err(error) => Err(error),
        Ok(handle) => {
            let session = open(&handle).await;
            // `handle` is held to the end on purpose: dropping it takes the channel with it.
            run(session, id, &files, &cancelled, &events).await
        }
    };
    report(id, result, &cancelled, &events);
}

/// `request_subsystem` only posts the request, so the reply is read here: a Host with the
/// subsystem turned off would otherwise say nothing and time out two minutes later.
async fn open<H: client::Handler>(handle: &client::Handle<H>) -> Result<SftpSession, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("opening a channel: {e}"))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("starting sftp: {e}"))?;
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => break,
            // A window adjustment can arrive before the reply does.
            Some(ChannelMsg::WindowAdjusted { .. }) => continue,
            _ => {
                return Err("this Host refuses sftp: add `Subsystem sftp` to its sshd_config".into())
            }
        }
    }
    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("sftp: {e}"))
}

async fn run(
    session: Result<SftpSession, String>,
    id: u32,
    files: &[Upload],
    cancelled: &AtomicBool,
    events: &Arc<dyn Events>,
) -> Result<Vec<String>, String> {
    let sftp = session?;
    let mut batch = Batch {
        sftp: &sftp,
        events,
        cancelled,
        id,
        total: files.iter().map(|file| file.size).sum(),
        done: 0,
        percent: u64::MAX,
        written: Vec::new(),
    };
    let outcome = batch.send(files).await;
    if outcome.is_err() {
        batch.roll_back().await;
    }
    let _ = sftp.close().await;
    outcome.map(|()| batch.written)
}

/// A cancelled batch reports nothing at all: the app asked for it and already knows.
fn report(
    id: u32,
    result: Result<Vec<String>, String>,
    cancelled: &AtomicBool,
    events: &Arc<dyn Events>,
) {
    match result {
        Ok(paths) => events.upload_done(id, &paths, None),
        Err(_) if cancelled.load(Ordering::Relaxed) => events.upload_done(id, &[], None),
        Err(error) => events.upload_done(id, &[], Some(&error)),
    }
}

struct Batch<'a> {
    sftp: &'a SftpSession,
    events: &'a Arc<dyn Events>,
    cancelled: &'a AtomicBool,
    id: u32,
    total: u64,
    done: u64,
    percent: u64,
    written: Vec<String>,
}

impl Batch<'_> {
    async fn send(&mut self, files: &[Upload]) -> Result<(), String> {
        let directory = self.directory().await?;
        self.progress();
        for file in files {
            let path = self.free_name(&directory, &file.name).await?;
            // Pushed before it is written, so a half-written file is rolled back too.
            self.written.push(path.clone());
            self.write(file, &path).await?;
        }
        Ok(())
    }

    /// Absolute, not `~/...`: a tilde only expands when a shell reads the word, and the
    /// path may be handed to a program that reads it itself.
    async fn directory(&self) -> Result<String, String> {
        let home = self
            .sftp
            .canonicalize(".")
            .await
            .map_err(|e| format!("finding the home directory: {e}"))?;
        let mut path = home.trim_end_matches('/').to_string();
        for part in DIRECTORY.split('/') {
            path = format!("{path}/{part}");
            if !self.sftp.try_exists(path.as_str()).await.unwrap_or(false) {
                self.sftp
                    .create_dir(path.as_str())
                    .await
                    .map_err(|e| format!("making {path}: {e}"))?;
            }
        }
        Ok(path)
    }

    /// Everything picked in one go shares a timestamp, so the name may already be taken.
    async fn free_name(&self, directory: &str, name: &str) -> Result<String, String> {
        let (stem, suffix) = match name.rsplit_once('.') {
            Some((stem, extension)) => (stem, format!(".{extension}")),
            None => (name, String::new()),
        };
        for attempt in 1..1000 {
            let path = match attempt {
                1 => format!("{directory}/{name}"),
                n => format!("{directory}/{stem}-{n}{suffix}"),
            };
            if !self.sftp.try_exists(path.as_str()).await.unwrap_or(false) {
                return Ok(path);
            }
        }
        Err(format!("{directory}/{name} and every name near it are taken"))
    }

    async fn write(&mut self, file: &Upload, path: &str) -> Result<(), String> {
        let mut source = fs::File::open(&file.local)
            .await
            .map_err(|e| format!("{}: {e}", file.name))?;
        let mut target = self
            .sftp
            .create(path)
            .await
            .map_err(|e| format!("creating {path}: {e}"))?;
        let mut buffer = vec![0u8; CHUNK];
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let read = source
                .read(&mut buffer)
                .await
                .map_err(|e| format!("{}: {e}", file.name))?;
            if read == 0 {
                break;
            }
            target
                .write_all(&buffer[..read])
                .await
                .map_err(|e| format!("writing {path}: {e}"))?;
            self.done += read as u64;
            self.progress();
        }
        target
            .shutdown()
            .await
            .map_err(|e| format!("closing {path}: {e}"))
    }

    /// One event per whole percent: a 200MB video would otherwise cross to the main queue
    /// eight hundred times to move a ring nobody can see move that finely.
    fn progress(&mut self) {
        let percent = match self.total {
            0 => 100,
            total => self.done * 100 / total,
        };
        if percent != self.percent {
            self.percent = percent;
            self.events.upload_progress(self.id, self.done, self.total);
        }
    }

    async fn roll_back(&self) {
        for path in &self.written {
            let _ = self.sftp.remove_file(path.as_str()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use russh::keys::ssh_key::rand_core::OsRng;
    use russh::keys::PrivateKey;
    use russh::server::{self, Auth, Msg, Session};
    use russh::{Channel, ChannelId, MethodKind};
    use russh_sftp::protocol::{Attrs, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode};
    use tokio::sync::Mutex as Shared;

    use super::*;
    use crate::session::{Prompt, State};

    /// A real SFTP server over a real ssh channel, writing into a temp directory, so what
    /// survives a failure can simply be listed.
    struct Remote {
        root: PathBuf,
        opened: Arc<Shared<HashMap<ChannelId, Channel<Msg>>>>,
        /// Set once the first file is closed, so the second one is cancelled mid-batch.
        trip: Option<Arc<AtomicBool>>,
    }

    impl server::Handler for Remote {
        type Error = russh::Error;

        async fn auth_password(&mut self, _: &str, _: &str) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }

        async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
            Ok(Auth::Reject {
                proceed_with_methods: Some([MethodKind::Password][..].into()),
                partial_success: false,
            })
        }

        async fn channel_open_session(
            &mut self,
            channel: Channel<Msg>,
            _: &mut Session,
        ) -> Result<bool, Self::Error> {
            self.opened.lock().await.insert(channel.id(), channel);
            Ok(true)
        }

        async fn subsystem_request(
            &mut self,
            id: ChannelId,
            name: &str,
            session: &mut Session,
        ) -> Result<(), Self::Error> {
            let Some(channel) = self.opened.lock().await.remove(&id) else {
                return session.channel_failure(id);
            };
            if name != "sftp" {
                return session.channel_failure(id);
            }
            session.channel_success(id)?;
            let files = Files {
                root: self.root.clone(),
                trip: self.trip.clone(),
                open: HashMap::new(),
            };
            // Spawned, not awaited: russh flushes the success reply only once this returns.
            tokio::spawn(russh_sftp::server::run(channel.into_stream(), files));
            Ok(())
        }
    }

    struct Files {
        root: PathBuf,
        trip: Option<Arc<AtomicBool>>,
        open: HashMap<String, std::fs::File>,
    }

    impl Files {
        fn at(&self, path: &str) -> PathBuf {
            self.root.join(path.trim_start_matches('/'))
        }

        fn ok(id: u32) -> Status {
            Status {
                id,
                status_code: StatusCode::Ok,
                error_message: String::new(),
                language_tag: "en-US".into(),
            }
        }
    }

    impl russh_sftp::server::Handler for Files {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn realpath(&mut self, id: u32, _: String) -> Result<Name, Self::Error> {
            Ok(Name {
                id,
                files: vec![russh_sftp::protocol::File::dummy("/")],
            })
        }

        async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            match self.at(&path).exists() {
                true => Ok(Attrs { id, attrs: FileAttributes::default() }),
                false => Err(StatusCode::NoSuchFile),
            }
        }

        async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            self.stat(id, path).await
        }

        async fn mkdir(&mut self, id: u32, path: String, _: FileAttributes) -> Result<Status, Self::Error> {
            std::fs::create_dir_all(self.at(&path)).map_err(|_| StatusCode::Failure)?;
            Ok(Self::ok(id))
        }

        async fn open(
            &mut self,
            id: u32,
            filename: String,
            _: OpenFlags,
            _: FileAttributes,
        ) -> Result<Handle, Self::Error> {
            let file = std::fs::File::create(self.at(&filename)).map_err(|_| StatusCode::Failure)?;
            self.open.insert(filename.clone(), file);
            Ok(Handle { id, handle: filename })
        }

        async fn write(&mut self, id: u32, handle: String, _: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
            use std::io::Write;
            let file = self.open.get_mut(&handle).ok_or(StatusCode::Failure)?;
            file.write_all(&data).map_err(|_| StatusCode::Failure)?;
            Ok(Self::ok(id))
        }

        async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
            if self.open.remove(&handle).is_some() {
                if let Some(trip) = &self.trip {
                    trip.store(true, Ordering::Relaxed);
                }
            }
            Ok(Self::ok(id))
        }

        async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
            std::fs::remove_file(self.at(&filename)).map_err(|_| StatusCode::NoSuchFile)?;
            Ok(Self::ok(id))
        }
    }

    /// Answers whatever the far end asks, so a test can never hang waiting for a person.
    #[derive(Default)]
    struct Quiet {
        answers: std::sync::Mutex<Option<Arc<crate::session::Answers>>>,
    }

    impl Events for Quiet {
        fn output(&self, _: &[u8]) {}
        fn state(&self, _: State, _: &str) {}
        fn upload_progress(&self, _: u32, _: u64, _: u64) {}
        fn upload_done(&self, _: u32, _: &[String], _: Option<&str>) {}

        fn host_key(&self, _: &str, _: Option<&str>) {
            let answers = self.answers.lock().unwrap().clone().unwrap();
            let sender = answers.host_key.lock().unwrap().take().unwrap();
            let _ = sender.send(true);
        }

        fn auth_prompt(&self, id: u32, _: &str, _: &str, prompts: &[Prompt]) {
            let answers = self.answers.lock().unwrap().clone().unwrap();
            let mut slot = answers.prompt.lock().unwrap();
            if let Some((pending, sender)) = slot.take() {
                assert_eq!(pending, id);
                let _ = sender.send(vec!["any".to_string(); prompts.len()]);
            }
        }
    }

    /// The handle comes back with the session: dropping it takes the channel with it.
    async fn connect(
        root: PathBuf,
        trip: Option<Arc<AtomicBool>>,
    ) -> (client::Handle<crate::ssh::Handler>, SftpSession) {
        let config = Arc::new(server::Config {
            keys: vec![PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap()],
            methods: [MethodKind::Password][..].into(),
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let opened = Arc::new(Shared::new(HashMap::new()));
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = server::run_stream(config, socket, Remote { root, opened, trip }).await;
        });

        let watcher = Arc::new(Quiet::default());
        let answers = Arc::new(crate::session::Answers::default());
        *watcher.answers.lock().unwrap() = Some(answers.clone());
        let events: Arc<dyn Events> = watcher;
        let known_hosts = tempfile::NamedTempFile::new().unwrap();
        let flags = crate::flags::SshFlags::default();
        let mut handle = client::connect(
            Arc::new(crate::ssh::client_config(&flags)),
            address,
            crate::ssh::Handler {
                events: events.clone(),
                answers: answers.clone(),
                known_hosts: known_hosts.path().to_path_buf(),
                host: "127.0.0.1".into(),
                port: address.port(),
                agent: None,
            },
        )
        .await
        .unwrap();
        let credentials = crate::ssh::Credentials {
            host: "127.0.0.1",
            key: None,
            key_passphrase: None,
            password: Some("any"),
        };
        crate::ssh::authenticate(&mut handle, "tester", &credentials, &flags, &events, &answers)
            .await
            .unwrap();
        let sftp = open(&handle).await.unwrap();
        (handle, sftp)
    }

    fn local(directory: &std::path::Path, name: &str, bytes: usize) -> Upload {
        let path = directory.join(name);
        std::fs::write(&path, vec![b'x'; bytes]).unwrap();
        Upload { local: path, name: name.into(), size: bytes as u64 }
    }

    fn landed(root: &std::path::Path) -> Vec<String> {
        let uploads = root.join(DIRECTORY);
        let mut names: Vec<String> = std::fs::read_dir(uploads)
            .map(|entries| {
                entries
                    .filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[tokio::test]
    async fn a_whole_batch_lands_under_the_names_it_was_given() {
        let root = tempfile::tempdir().unwrap();
        let here = tempfile::tempdir().unwrap();
        let (_handle, sftp) = connect(root.path().to_path_buf(), None).await;
        let files = vec![local(here.path(), "one.jpg", 40), local(here.path(), "two.mov", 90)];
        let cancelled = AtomicBool::new(false);
        let events: Arc<dyn Events> = Arc::new(Quiet::default());

        let paths = run(Ok(sftp), 1, &files, &cancelled, &events).await.unwrap();

        assert_eq!(paths, ["/.sesh/uploads/one.jpg", "/.sesh/uploads/two.mov"]);
        assert_eq!(landed(root.path()), ["one.jpg", "two.mov"]);
    }

    #[tokio::test]
    async fn a_file_the_core_cannot_read_takes_the_whole_batch_with_it() {
        let root = tempfile::tempdir().unwrap();
        let here = tempfile::tempdir().unwrap();
        let (_handle, sftp) = connect(root.path().to_path_buf(), None).await;
        let mut files = vec![local(here.path(), "one.jpg", 40), local(here.path(), "two.mov", 90)];
        files[1].local = here.path().join("gone.mov");
        let cancelled = AtomicBool::new(false);
        let events: Arc<dyn Events> = Arc::new(Quiet::default());

        let error = run(Ok(sftp), 1, &files, &cancelled, &events).await.unwrap_err();

        assert!(error.contains("two.mov"), "{error}");
        assert!(landed(root.path()).is_empty(), "{:?}", landed(root.path()));
    }

    #[tokio::test]
    async fn cancelling_part_way_leaves_nothing_behind() {
        let root = tempfile::tempdir().unwrap();
        let here = tempfile::tempdir().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (_handle, sftp) = connect(root.path().to_path_buf(), Some(cancelled.clone())).await;
        let files = vec![local(here.path(), "one.jpg", 40), local(here.path(), "two.mov", 90)];
        let events: Arc<dyn Events> = Arc::new(Quiet::default());

        let error = run(Ok(sftp), 1, &files, &cancelled, &events).await.unwrap_err();

        assert_eq!(error, "cancelled");
        assert!(landed(root.path()).is_empty(), "{:?}", landed(root.path()));
    }
}
