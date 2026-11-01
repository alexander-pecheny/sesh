use std::time::Duration;

use russh::keys::ssh_key::rand_core::OsRng;
use russh::server;

use super::*;
use crate::flags::{spec, Record, Transport};
use crate::session::Prompt;
use crate::ssh_config::{Algorithms, Family, Key, Strict};

#[test]
fn algorithm_lists_append_remove_and_put_first() {
    let defaults = own(&[&mac::HMAC_SHA256, &mac::HMAC_SHA512]);
    let known = own(mac::ALL_MAC_ALGORITHMS);
    let names = |list: Vec<mac::Name>| list.iter().map(Named::name).collect::<Vec<_>>();
    assert_eq!(names(choose("MACs", None, &defaults, &known).unwrap()), ["hmac-sha2-256", "hmac-sha2-512"]);
    assert_eq!(names(choose("MACs", Some("-*512"), &defaults, &known).unwrap()), ["hmac-sha2-256"]);
    assert_eq!(names(choose("MACs", Some("^hmac-sha2-512"), &defaults, &known).unwrap()), ["hmac-sha2-512", "hmac-sha2-256"]);
    assert_eq!(names(choose("MACs", Some("+hmac-sha1"), &defaults, &known).unwrap()), ["hmac-sha2-256", "hmac-sha2-512", "hmac-sha1"]);
    assert_eq!(names(choose("MACs", Some("hmac-sha2-512,hmac-sha2-256"), &defaults, &known).unwrap()), ["hmac-sha2-512", "hmac-sha2-256"]);
    assert!(choose("MACs", Some("umac-1024"), &defaults, &known).unwrap_err().contains("names no algorithm"));
}

/// A Host that accepts one public key, and refuses everything else.
struct Remote {
    key: PublicKey,
}

impl server::Handler for Remote {
    type Error = russh::Error;

    async fn auth_publickey(&mut self, _: &str, key: &PublicKey) -> Result<server::Auth, Self::Error> {
        Ok(match key.key_data() == self.key.key_data() {
            true => server::Auth::Accept,
            false => server::Auth::reject(),
        })
    }

    async fn channel_open_session(&mut self, _: russh::Channel<server::Msg>, _: &mut server::Session) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Listens on a free port and serves one connection that accepts `key`.
async fn remote(key: PublicKey) -> u16 {
    let config = Arc::new(server::Config {
        keys: vec![PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap()],
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let _ = server::run_stream(config, socket, Remote { key }).await;
    });
    port
}

#[derive(Default)]
struct Quiet {
    prompts: Mutex<Vec<String>>,
}

impl Events for Quiet {
    fn output(&self, _: &[u8]) {}
    fn state(&self, _: State, _: &str) {}
    fn host_key(&self, _: &str, _: Option<&str>) {}
    fn auth_prompt(&self, _: u32, _: &str, _: &str, prompts: &[Prompt]) {
        self.prompts.lock().unwrap().extend(prompts.iter().map(|p| p.prompt.clone()));
    }
    fn upload_progress(&self, _: u32, _: u64, _: u64) {}
    fn upload_done(&self, _: u32, _: &[String], _: Option<&str>) {}
}

fn host(port: u16, key: PathBuf, proxy: Proxy) -> Host {
    Host {
        alias: "box".into(),
        host_name: "127.0.0.1".into(),
        port,
        user: "tester".into(),
        identity_files: vec![key],
        certificate_files: Vec::new(),
        identities_only: true,
        identity_agent: None,
        add_keys_to_agent: false,
        keys: Vec::new(),
        password: None,
        forward_agent: None,
        proxy,
        alive_interval: None,
        alive_count_max: 3,
        strict: Strict::No,
        known_hosts: Vec::new(),
        global_known_hosts: Vec::new(),
        host_key_alias: None,
        connect_timeout: Some(Duration::from_secs(10)),
        family: Family::Any,
        algorithms: Algorithms::default(),
        compression: false,
        methods: vec![Method::PublicKey],
        batch_mode: true,
        set_env: Vec::new(),
        origin: Origin::Config,
    }
}

fn key_file(dir: &Path) -> (PathBuf, PublicKey) {
    let key = PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap();
    let path = dir.join("id_ed25519");
    std::fs::write(&path, key.to_openssh(Default::default()).unwrap().as_bytes()).unwrap();
    (path, key.public_key().clone())
}

#[tokio::test]
async fn an_identity_file_signs_in_directly_and_the_address_is_kept_for_mosh() {
    let dir = tempfile::tempdir().unwrap();
    let (path, public) = key_file(dir.path());
    let port = remote(public).await;
    let events: Arc<dyn Events> = Arc::new(Quiet::default());
    let dialed = reach(host(port, path, Proxy::Direct), events, Arc::default(), 0).await.unwrap();
    assert_eq!(dialed.direct, Some("127.0.0.1".parse().unwrap()));
}

#[tokio::test]
async fn a_proxy_command_carries_the_connection_on_its_stdin_and_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let (path, public) = key_file(dir.path());
    let port = remote(public).await;
    let events: Arc<dyn Events> = Arc::new(Quiet::default());
    let proxy = Proxy::Command(format!("nc 127.0.0.1 {port}"));
    let dialed = reach(host(port, path, proxy), events, Arc::default(), 0).await.unwrap();
    assert_eq!(dialed.direct, None);
    assert!(dialed.handle.channel_open_session().await.is_ok());
}

#[tokio::test]
async fn a_failing_proxy_command_explains_itself() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = key_file(dir.path());
    let events: Arc<dyn Events> = Arc::new(Quiet::default());
    let proxy = Proxy::Command("echo 'no session for you' >&2; exit 3".into());
    let error = reach(host(22, path, proxy), events, Arc::default(), 0).await.err().unwrap();
    assert!(error.contains("the ProxyCommand failed: no session for you"), "{error}");
}

#[tokio::test]
async fn a_refused_key_ends_in_permission_denied_without_asking_in_batch_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = key_file(dir.path());
    let other = PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap();
    let port = remote(other.public_key().clone()).await;
    let quiet = Arc::new(Quiet::default());
    let events: Arc<dyn Events> = quiet.clone();
    let error = reach(host(port, path, Proxy::Direct), events, Arc::default(), 0).await.err().unwrap();
    assert!(error.contains("permission denied"), "{error}");
    assert!(quiet.prompts.lock().unwrap().is_empty());
}

/// A Host that takes one password and keeps offering it, as OpenSSH does.
struct Guarded {
    password: String,
}

impl server::Handler for Guarded {
    type Error = russh::Error;

    async fn auth_password(&mut self, _: &str, password: &str) -> Result<server::Auth, Self::Error> {
        Ok(match password == self.password {
            true => server::Auth::Accept,
            false => server::Auth::Reject { proceed_with_methods: Some([MethodKind::Password][..].into()), partial_success: false },
        })
    }
}

/// Accepts every host key and types `typed` at every prompt, noting what it was told.
#[derive(Default)]
struct Typist {
    answers: Mutex<Option<Arc<Answers>>>,
    seen: Mutex<Vec<&'static str>>,
    typed: Option<String>,
}

impl Events for Typist {
    fn output(&self, _: &[u8]) {}
    fn state(&self, state: State, _: &str) {
        if let State::PasswordRejected = state {
            self.seen.lock().unwrap().push("rejected");
        }
    }
    fn host_key(&self, _: &str, _: Option<&str>) {
        let answers = self.answers.lock().unwrap().clone().unwrap();
        let _ = answers.host_key.lock().unwrap().take().unwrap().send(true);
    }
    fn auth_prompt(&self, _: u32, _: &str, _: &str, _: &[Prompt]) {
        self.seen.lock().unwrap().push("prompted");
        let answers = self.answers.lock().unwrap().clone().unwrap();
        let typed = self.typed.clone().expect("prompted with nothing to type");
        let pending = answers.prompt.lock().unwrap().take();
        if let Some((_, sender)) = pending {
            let _ = sender.send(vec![typed]);
        }
    }
    fn upload_progress(&self, _: u32, _: u64, _: u64) {}
    fn upload_done(&self, _: u32, _: &[String], _: Option<&str>) {}
}

async fn phone(typist: Arc<Typist>, record: Record) -> Result<Dialed, String> {
    let answers = Arc::new(Answers::default());
    *typist.answers.lock().unwrap() = Some(answers.clone());
    let events: Arc<dyn Events> = typist;
    dial(spec(record).unwrap().0, &events, &answers).await
}

fn record(port: u16, known_hosts: &Path) -> Record {
    Record { host: "127.0.0.1".into(), port, user: "tester".into(), known_hosts: known_hosts.to_path_buf(), ..Default::default() }
}

async fn guarded(password: &str) -> u16 {
    let config = Arc::new(server::Config {
        keys: vec![PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap()],
        methods: [MethodKind::Password][..].into(),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let password = password.to_string();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let _ = server::run_stream(config, socket, Guarded { password }).await;
    });
    port
}

#[tokio::test]
async fn a_saved_password_is_tried_before_the_user_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    let typist = Arc::new(Typist::default());
    let port = guarded("saved").await;
    phone(typist.clone(), Record { password: Some("saved".into()), ..record(port, &dir.path().join("known_hosts")) }).await.unwrap();
    assert!(typist.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_refused_saved_password_is_reported_and_then_asked_for() {
    let dir = tempfile::tempdir().unwrap();
    let typist = Arc::new(Typist { typed: Some("typed".into()), ..Default::default() });
    let port = guarded("typed").await;
    phone(typist.clone(), Record { password: Some("wrong".into()), ..record(port, &dir.path().join("known_hosts")) }).await.unwrap();
    assert_eq!(*typist.seen.lock().unwrap(), ["rejected", "prompted"]);
}

#[tokio::test]
async fn a_phone_key_signs_in_and_its_host_key_is_kept_in_the_phones_file() {
    let dir = tempfile::tempdir().unwrap();
    let key = PrivateKey::random(&mut OsRng, russh::keys::Algorithm::Ed25519).unwrap();
    let port = remote(key.public_key().clone()).await;
    let pem = key.to_openssh(Default::default()).unwrap().to_string();
    let known_hosts = dir.path().join("known_hosts");
    let given = Record { key: Some(Key { pem, passphrase: None }), ..record(port, &known_hosts) };
    let dialed = phone(Arc::new(Typist::default()), given).await.unwrap();
    assert_eq!(dialed.direct, Some("127.0.0.1".parse().unwrap()));
    assert!(std::fs::read_to_string(known_hosts).unwrap().starts_with(&format!("[127.0.0.1]:{port} ssh-ed25519 ")));
}

/// The local sshd from `scripts/local-sshd.sh`, or nil when it is not running.
fn local_sshd() -> Option<Record> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local");
    let pem = std::fs::read_to_string(root.join("testkey")).ok()?;
    std::net::TcpStream::connect("127.0.0.1:2222").ok()?;
    let user = std::env::var("USER").ok()?;
    Some(Record { host: "127.0.0.1".into(), port: 2222, user, key: Some(Key { pem, passphrase: None }), ..Default::default() })
}

#[derive(Default)]
struct Screen {
    shown: Mutex<Vec<u8>>,
    ran: Mutex<Vec<String>>,
}

impl Events for Screen {
    fn output(&self, bytes: &[u8]) {
        self.shown.lock().unwrap().extend_from_slice(bytes);
    }
    fn state(&self, _: State, _: &str) {}
    fn host_key(&self, _: &str, _: Option<&str>) {}
    fn auth_prompt(&self, _: u32, _: &str, _: &str, _: &[Prompt]) {}
    fn upload_progress(&self, _: u32, _: u64, _: u64) {}
    fn upload_done(&self, _: u32, _: &[String], _: Option<&str>) {}
    fn ran(&self, _: u32, ran: &crate::link::Ran) {
        self.ran.lock().unwrap().push(ran.stdout.clone());
    }
}

fn until(what: impl Fn() -> bool) {
    for _ in 0..200 {
        if what() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("gave up waiting");
}

#[test]
fn a_phone_host_runs_commands_and_terminals_on_its_link_directly_through_a_jump_and_over_mosh() {
    let Some(base) = local_sshd() else { return eprintln!("skipped: scripts/local-sshd.sh is not running") };
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let scanned = std::process::Command::new("ssh-keyscan").args(["-p", "2222", "127.0.0.1"]).output().unwrap().stdout;
    std::fs::write(&known_hosts, scanned).unwrap();
    let server = Path::new("/opt/homebrew/bin/mosh-server");
    let cases = [
        (Transport::Ssh, String::new()),
        (Transport::Ssh, format!("-J {}@127.0.0.1:2222 -o ServerAliveInterval=5", base.user)),
        (Transport::Mosh, format!("--predict=never --server={}", server.display())),
    ];
    for (transport, flags) in cases.into_iter().filter(|(transport, _)| *transport == Transport::Ssh || server.exists()) {
        let given = Record { known_hosts: known_hosts.clone(), transport, extra_flags: flags, ..local_sshd().unwrap() };
        let (spec, mosh) = spec(given).unwrap();
        let link_events = Arc::new(Screen::default());
        let link = crate::link::connect(async { Ok(spec) }, mosh, link_events.clone());
        link.run("echo over-the-link".into());
        let screen = Arc::new(Screen::default());
        let terminal = crate::terminal::open(&link, Some("mosh-server".into()), "echo in-a-terminal; sleep 1".into(), 80, 24, screen.clone());
        until(|| link_events.ran.lock().unwrap().iter().any(|out| out == "over-the-link\n"));
        until(|| String::from_utf8_lossy(&screen.shown.lock().unwrap()).contains("in-a-terminal"));
        terminal.close();
        link.close();
    }
}
