//! `sesh-mux terminal attach` drives one pane from another terminal and detaches cleanly.

#![cfg(unix)]

pub mod support;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use support::{cleanup_test_base, wait_for_socket};

const ATTACH_ROWS: u16 = 30;
const ATTACH_COLS: u16 = 100;

struct Server {
    child: Child,
    home: PathBuf,
}

impl Server {
    fn start(home: &Path) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_sesh-mux"))
            .arg("server")
            .env("HOME", home)
            .env("SHELL", "/bin/sh")
            .env_remove("SESH_MUX_SOCKET_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        wait_for_socket(
            &home.join(".sesh/mux/sesh-mux.sock"),
            Duration::from_secs(10),
        );
        Self {
            child,
            home: home.to_path_buf(),
        }
    }

    fn cli(&self, args: &[&str]) -> serde_json::Value {
        let output = Command::new(env!("CARGO_BIN_EXE_sesh-mux"))
            .args(args)
            .env("HOME", &self.home)
            .env_remove("SESH_MUX_SOCKET_PATH")
            .output()
            .unwrap();
        serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&output.stdout).into_owned())
        })
    }

    fn pane_rows(&self, pane: &str) -> u64 {
        self.cli(&["pane", "get", pane])["result"]["pane"]["scroll"]["viewport_rows"]
            .as_u64()
            .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cli(&["server", "stop"]);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn attach_types_into_the_pane_follows_its_size_and_detaches() {
    let base = PathBuf::from(format!("/tmp/sm-attach-{}", std::process::id()));
    let server = Server::start(&base);
    let created = server.cli(&["workspace", "create", "--cwd", "/tmp"]);
    let pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let terminal = created["result"]["root_pane"]["terminal_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let headless_rows = server.pane_rows(&pane);

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: ATTACH_ROWS,
            cols: ATTACH_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_sesh-mux"));
    command.args(["terminal", "attach", &terminal, "--takeover"]);
    command.env("HOME", &base);
    command.env_remove("SESH_MUX_SOCKET_PATH");
    let mut client = pty.slave.spawn_command(command).unwrap();
    let mut reader = pty.master.try_clone_reader().unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    let (frames_tx, frames) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || frames_tx.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });

    assert!(wait_until(Duration::from_secs(5), || server
        .pane_rows(&pane)
        == u64::from(ATTACH_ROWS)));
    writer.write_all(b"echo attached-$((40+2))\r").unwrap();
    let mut received = Vec::new();
    assert!(wait_until(Duration::from_secs(5), || {
        received.extend(frames.try_iter().flatten());
        String::from_utf8_lossy(&received).contains("attached-42")
    }));
    let visible = server.cli(&["pane", "read", &pane, "--source", "visible"]);
    assert!(visible.as_str().unwrap().contains("attached-42"));

    writer.write_all(b"\x02q").unwrap();
    assert!(wait_until(Duration::from_secs(5), || client
        .try_wait()
        .unwrap()
        .is_some()));
    assert!(wait_until(Duration::from_secs(5), || server
        .pane_rows(&pane)
        == headless_rows));
    drop(server);
    cleanup_test_base(&base);
}
