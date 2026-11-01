//! Where sesh-mux keeps its sockets and files, and stopping a running server.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Stream as _;

use crate::ipc::LocalStream;

const STOP_WAIT_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_WAIT_POLL: Duration = Duration::from_millis(25);
const MIN_SOCKET_TIMEOUT: Duration = Duration::from_millis(1);

/// Drops `--session NAME` and `--session=NAME` before any `--`, so command lines written for
/// sesh-mux-sesh still work.
pub fn strip_session_args(args: &[String]) -> Vec<String> {
    let mut cleaned = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            cleaned.extend_from_slice(&args[index..]);
            break;
        }
        if arg == "--session" {
            index += 2;
            continue;
        }
        if !arg.starts_with("--session=") {
            cleaned.push(arg.clone());
        }
        index += 1;
    }
    cleaned
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn data_dir() -> PathBuf {
    home_dir().join(".sesh").join("mux")
}

pub fn active_api_socket_path() -> PathBuf {
    match std::env::var_os(crate::api::SOCKET_PATH_ENV_VAR) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => data_dir().join("sesh-mux.sock"),
    }
}

pub(crate) fn stop_active_server() -> Result<(), String> {
    let socket_path = active_api_socket_path();
    let client_socket_path = crate::server::socket_paths::client_socket_path();
    stop_socket_with_timeout(
        socket_path.clone(),
        vec![socket_path, client_socket_path],
        STOP_WAIT_TIMEOUT,
    )
}

fn stop_socket_with_timeout(
    socket_path: PathBuf,
    stopped_socket_paths: Vec<PathBuf>,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let request = serde_json::json!({
        "id": "cli:server:stop",
        "method": "server.stop",
        "params": {}
    });
    let stream = crate::ipc::connect_local_stream(&socket_path).map_err(|err| {
        format!(
            "server is not running or cannot be reached at {}: {err}",
            socket_path.display()
        )
    })?;
    if let Some(response) = send_stop_request(stream, &request, deadline)? {
        if let Some(error) = response.get("error") {
            return Err(error.to_string());
        }
    }
    if !wait_until_stopped_until(&stopped_socket_paths, deadline) {
        return Err(format!(
            "server did not stop within {}ms",
            timeout.as_millis()
        ));
    }
    Ok(())
}

fn send_stop_request(
    mut stream: LocalStream,
    request: &serde_json::Value,
    deadline: Instant,
) -> Result<Option<serde_json::Value>, String> {
    let Some(write_timeout) = socket_timeout_until(deadline) else {
        return Ok(None);
    };
    if let Err(err) = stream.set_send_timeout(Some(write_timeout)) {
        if err.kind() != std::io::ErrorKind::InvalidInput {
            return Err(err.to_string());
        }
    }
    match send_stop_request_inner(&mut stream, request, deadline) {
        Ok(Some(line)) => serde_json::from_str(&line)
            .map(Some)
            .map_err(|err| err.to_string()),
        Ok(None) => Ok(None),
        Err(err) if stop_request_error_allows_wait(&err) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

fn send_stop_request_inner(
    stream: &mut LocalStream,
    request: &serde_json::Value,
    deadline: Instant,
) -> std::io::Result<Option<String>> {
    stream.write_all(request.to_string().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let Some(read_timeout) = socket_timeout_until(deadline) else {
        return Ok(None);
    };
    if let Err(err) = stream.set_recv_timeout(Some(read_timeout)) {
        if err.kind() == std::io::ErrorKind::InvalidInput {
            return Ok(None);
        }
        return Err(err);
    }
    let mut line = String::new();
    if BufReader::new(stream).read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line))
}

fn stop_request_error_allows_wait(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
    )
}

fn is_running_at(socket_path: &Path) -> bool {
    socket_path.exists() && crate::ipc::connect_local_stream(socket_path).is_ok()
}

fn wait_until_stopped_until(socket_paths: &[PathBuf], deadline: Instant) -> bool {
    while Instant::now() < deadline {
        if socket_paths.iter().all(|path| !is_running_at(path)) {
            return true;
        }
        std::thread::sleep(STOP_WAIT_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
    socket_paths.iter().all(|path| !is_running_at(path))
}

fn socket_timeout_until(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then(|| remaining.max(MIN_SOCKET_TIMEOUT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn session_flags_are_ignored_before_the_separator() {
        let args = strings(&[
            "sesh-mux",
            "--session",
            "sesh",
            "pane",
            "--session=x",
            "list",
        ]);
        assert_eq!(
            strip_session_args(&args),
            strings(&["sesh-mux", "pane", "list"])
        );
        let args = strings(&["sesh-mux", "agent", "start", "--", "--session", "keep"]);
        assert_eq!(strip_session_args(&args), args);
    }
}
