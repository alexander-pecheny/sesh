use std::time::Duration;

use russh::keys::ssh_key::rand_core::OsRng;
use russh::server;

use super::*;
use crate::session::Prompt;
use crate::ssh_config::{Algorithms, Family, Strict};

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
