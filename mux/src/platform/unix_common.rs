pub(crate) fn classify_child_exit(status: &portable_pty::ExitStatus) -> super::ChildExitReason {
    if status.signal().is_some() {
        super::ChildExitReason::Interrupted
    } else {
        super::ChildExitReason::Exited
    }
}

pub(crate) fn shutdown_client_stream(stream: &crate::ipc::LocalStream) -> std::io::Result<()> {
    let crate::ipc::LocalStream::UdSocket(stream) = stream;
    stream.inner().shutdown(std::net::Shutdown::Both)
}

pub(crate) struct ClientStreamReader<'a>(pub(crate) &'a mut crate::ipc::LocalStream);

impl std::io::Read for ClientStreamReader<'_> {
    fn read(&mut self, data: &mut [u8]) -> std::io::Result<usize> {
        use std::os::fd::AsRawFd as _;

        loop {
            match self.0.read(data) {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    let crate::ipc::LocalStream::UdSocket(stream) = &*self.0;
                    let mut descriptor = libc::pollfd {
                        fd: stream.inner().as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // Sleep until input or shutdown, without polling quiet observers.
                    if unsafe { libc::poll(&mut descriptor, 1, -1) } < 0 {
                        let error = std::io::Error::last_os_error();
                        if error.kind() != std::io::ErrorKind::Interrupted {
                            return Err(error);
                        }
                    }
                }
                result => return result,
            }
        }
    }
}

pub(crate) fn write_client_stream(
    stream: &crate::ipc::LocalStream,
    mut data: &[u8],
) -> std::io::Result<()> {
    use std::io::{self, Write as _};
    use std::os::fd::AsRawFd as _;
    use std::time::Instant;

    let crate::ipc::LocalStream::UdSocket(socket) = stream;
    let mut socket = socket.inner();
    let Some(timeout) = socket.write_timeout()? else {
        return socket.write_all(data);
    };
    let timed_out = || {
        // Dropping the writer clone alone would leave the reader blocked.
        let _ = shutdown_client_stream(stream);
        io::Error::new(
            io::ErrorKind::TimedOut,
            "terminal observer stopped receiving output",
        )
    };
    let mut progress = Instant::now();
    while !data.is_empty() {
        match socket.write(data) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => {
                data = &data[written..];
                progress = Instant::now();
                continue;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
        let remaining = timeout
            .checked_sub(progress.elapsed())
            .ok_or_else(timed_out)?;
        let mut descriptor = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let wait_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let ready = unsafe { libc::poll(&mut descriptor, 1, wait_ms) };
        if ready == 0 {
            return Err(timed_out());
        }
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    Ok(())
}

fn set_sigpipe_disposition(handler: libc::sighandler_t) {
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    unsafe {
        libc::sigemptyset(&mut action.sa_mask);
        // Rust starts with SIGPIPE ignored. If this best-effort transition
        // fails, stdout retains the existing Rust behavior.
        libc::sigaction(libc::SIGPIPE, &action, std::ptr::null_mut());
    }
}

pub(crate) fn begin_cli_output() {
    set_sigpipe_disposition(libc::SIG_DFL);
}

#[cfg(test)]
mod tests {}
