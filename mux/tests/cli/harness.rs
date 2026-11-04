pub(super) use std::fs;
pub(super) use std::io::{BufRead, BufReader, Write};
pub(super) use std::os::unix::net::{UnixListener, UnixStream};
pub(super) use std::path::{Path, PathBuf};
pub(super) use std::process::{Command, Stdio};
pub(super) use std::thread;
pub(super) use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) use crate::support::{
    cleanup_test_base, register_runtime_dir, register_spawned_sesh_mux_pid,
    unregister_spawned_sesh_mux_pid, CURRENT_PROTOCOL,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

pub(super) fn unique_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!("/tmp/hcli-{}-{nanos}", std::process::id()))
}

pub(super) fn run_git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "git command failed: git -C {} {}",
        repo.display(),
        args.join(" ")
    );
}

pub(super) fn create_committed_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    run_git(path, &["init", "--quiet"]);
    run_git(path, &["config", "user.email", "sesh-mux@example.invalid"]);
    run_git(path, &["config", "user.name", "sesh-mux Test"]);
    fs::write(path.join("README.md"), "test\n").unwrap();
    run_git(path, &["add", "README.md"]);
    run_git(path, &["commit", "--quiet", "-m", "initial"]);
}

pub(super) struct SpawnedSeshMux {
    _master: Box<dyn MasterPty + Send>,
    pub(super) child: Box<dyn Child + Send + Sync>,
}

impl Drop for SpawnedSeshMux {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        let _ = self.child.kill();

        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let mut status = 0;
                let result =
                    unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                if result == pid as libc::pid_t || result == -1 {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }

            unregister_spawned_sesh_mux_pid(Some(pid));
        }
    }
}

pub(super) fn cleanup_spawned_sesh_mux(spawned: SpawnedSeshMux, base: PathBuf) {
    drop(spawned);
    cleanup_test_base(&base);
}

pub(super) fn wait_for_socket(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() && std::os::unix::net::UnixStream::connect(path).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("socket did not appear at {}", path.display());
}

pub(super) fn spawn_sesh_mux(
    config_home: &Path,
    runtime_dir: &Path,
    socket_path: &Path,
) -> SpawnedSeshMux {
    spawn_sesh_mux_with_config(
        config_home,
        runtime_dir,
        socket_path,
        None,
        "",
    )
}

pub(super) fn spawn_sesh_mux_with_path(
    config_home: &Path,
    runtime_dir: &Path,
    socket_path: &Path,
    path_override: Option<&Path>,
) -> SpawnedSeshMux {
    spawn_sesh_mux_with_config(
        config_home,
        runtime_dir,
        socket_path,
        path_override,
        "",
    )
}

pub(super) fn spawn_sesh_mux_with_config(
    config_home: &Path,
    runtime_dir: &Path,
    socket_path: &Path,
    path_override: Option<&Path>,
    config_toml: &str,
) -> SpawnedSeshMux {
    fs::create_dir_all(config_home.join(".sesh/mux")).unwrap();
    fs::create_dir_all(runtime_dir).unwrap();
    register_runtime_dir(runtime_dir);
    fs::write(config_home.join(".sesh/mux/config.toml"), config_toml).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_sesh-mux"));
    cmd.arg("server");
    cmd.env("HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("SESH_MUX_SOCKET_PATH", socket_path);
    cmd.env_remove("SESH_MUX_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("SESH_MUX_ENV");
    if let Some(path) = path_override {
        cmd.env("PATH", path);
    }

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_sesh_mux_pid(child.process_id());
    SpawnedSeshMux {
        _master: pair.master,
        child,
    }
}

pub(super) fn run_cli(socket_path: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sesh-mux"));
    command.args(args);
    command.env("SESH_MUX_SOCKET_PATH", socket_path);
    command.output().unwrap()
}

pub(super) fn run_cli_in_dir(
    socket_path: &Path,
    args: &[&str],
    current_dir: &Path,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sesh-mux"));
    command.args(args);
    command.current_dir(current_dir);
    command.env("SESH_MUX_SOCKET_PATH", socket_path);
    command.output().unwrap()
}

pub(super) fn pane_topology_snapshot(list_response: &serde_json::Value) -> Vec<serde_json::Value> {
    list_response["result"]["panes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pane| {
            serde_json::json!({
                "pane_id": pane["pane_id"],
                "terminal_id": pane["terminal_id"],
                "workspace_id": pane["workspace_id"],
                "tab_id": pane["tab_id"],
                "focused": pane["focused"],
            })
        })
        .collect()
}

pub(super) fn run_cli_json(socket_path: &Path, args: &[&str]) -> serde_json::Value {
    let output = run_cli(socket_path, args);
    parse_cli_json_output(args, output)
}

pub(super) fn run_cli_json_in_dir(
    socket_path: &Path,
    args: &[&str],
    current_dir: &Path,
) -> serde_json::Value {
    let output = run_cli_in_dir(socket_path, args, current_dir);
    parse_cli_json_output(args, output)
}

pub(super) fn parse_cli_json_output(
    args: &[&str],
    output: std::process::Output,
) -> serde_json::Value {
    assert!(
        output.status.success(),
        "command failed: sesh-mux {}\nstatus: {:?}\nstderr: {}\nstdout: {}",
        args.join(" "),
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );

    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "failed to parse JSON response for `sesh-mux {}`: {}\nstdout: {}\nstderr: {}",
            args.join(" "),
            err,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

pub(super) fn wait_until(
    timeout: Duration,
    interval: Duration,
    mut condition: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(interval);
    }
    false
}

pub(super) fn process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        true
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

pub(super) fn wait_for_pid_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_exists(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    !process_exists(pid)
}

pub(super) fn wait_for_pid_file(pid_file: &Path, timeout: Duration) -> Result<u32, String> {
    const STABLE_PID_CONTENT_WINDOW: Duration = Duration::from_millis(250);

    let deadline = Instant::now() + timeout;
    let mut last_contents = String::new();
    let mut stable_candidate: Option<(String, u32, Instant)> = None;

    while Instant::now() < deadline {
        if let Ok(contents) = fs::read_to_string(pid_file) {
            let trimmed = contents.trim().to_string();
            last_contents = contents;

            if let Ok(pid) = trimmed.parse::<u32>() {
                match &stable_candidate {
                    Some((candidate_text, candidate_pid, stable_since))
                        if candidate_text == &trimmed && *candidate_pid == pid =>
                    {
                        if stable_since.elapsed() >= STABLE_PID_CONTENT_WINDOW {
                            return Ok(pid);
                        }
                    }
                    _ => {
                        stable_candidate = Some((trimmed, pid, Instant::now()));
                    }
                }
            } else {
                stable_candidate = None;
            }
        }

        thread::sleep(Duration::from_millis(25));
    }

    Err(format!(
        "pid file {} did not contain stable parseable pid before timeout; last contents={:?}",
        pid_file.display(),
        last_contents
    ))
}

#[test]
fn wait_for_pid_file_retries_until_pid_is_written() {
    let base = unique_test_dir();
    fs::create_dir_all(&base).unwrap();
    let pid_file = base.join("delayed.pid");
    fs::write(&pid_file, "").unwrap();

    let writer = thread::spawn({
        let pid_file = pid_file.clone();
        move || {
            thread::sleep(Duration::from_millis(100));
            fs::write(pid_file, "424242\n").unwrap();
        }
    });

    let pid = wait_for_pid_file(&pid_file, Duration::from_secs(2)).unwrap();
    assert_eq!(pid, 424242);

    writer.join().unwrap();
    cleanup_test_base(&base);
}

#[test]
fn wait_for_pid_file_errors_when_file_never_contains_pid() {
    let base = unique_test_dir();
    fs::create_dir_all(&base).unwrap();
    let pid_file = base.join("empty.pid");
    fs::write(&pid_file, "").unwrap();

    let err = wait_for_pid_file(&pid_file, Duration::from_millis(150)).unwrap_err();
    assert!(
        err.contains("did not contain stable parseable pid"),
        "unexpected error: {err}"
    );

    cleanup_test_base(&base);
}

#[test]
fn wait_for_pid_file_rejects_unparseable_partial_write_until_stable_contents() {
    let base = unique_test_dir();
    fs::create_dir_all(&base).unwrap();
    let pid_file = base.join("partial-race.pid");
    fs::write(&pid_file, "").unwrap();

    let writer = thread::spawn({
        let pid_file = pid_file.clone();
        move || {
            thread::sleep(Duration::from_millis(40));
            fs::write(&pid_file, "pid=").unwrap();
            thread::sleep(Duration::from_millis(40));
            fs::write(&pid_file, "pid=424242").unwrap();
            thread::sleep(Duration::from_millis(40));
            fs::write(&pid_file, "424242\n").unwrap();
        }
    });

    let start = Instant::now();
    let pid = wait_for_pid_file(&pid_file, Duration::from_secs(2)).unwrap();
    assert_eq!(pid, 424242);
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "helper should wait for stable complete contents, elapsed={:?}",
        start.elapsed()
    );

    writer.join().unwrap();
    cleanup_test_base(&base);
}

pub(super) fn send_request(socket_path: &Path, json: &str) -> serde_json::Value {
    let mut stream = UnixStream::connect(socket_path).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();

    let mut line = String::new();
    let mut reader = BufReader::new(stream);
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

pub(super) fn write_fake_pong(
    stream: &mut UnixStream,
    request: &serde_json::Value,
    version: &str,
    protocol: u32,
) {
    writeln!(
        stream,
        "{}",
        serde_json::json!({
            "id": request["id"],
            "result": {
                "type": "pong",
                "version": version,
                "protocol": protocol,
                "capabilities": {
                    "live_handoff": true,
                    "detached_server_daemon": true
                }
            }
        })
    )
    .unwrap();
    stream.flush().unwrap();
}

pub(super) fn accept_fake_cli_operation(listener: &UnixListener) -> (UnixStream, String) {
    loop {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        reader.read_line(&mut line).unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        if request["method"] != "ping" {
            return (stream, line);
        }

        write_fake_pong(
            &mut stream,
            &request,
            "different-build-same-protocol",
            CURRENT_PROTOCOL,
        );
    }
}
