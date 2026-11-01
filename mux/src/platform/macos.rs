use std::ffi::OsStr;
use std::io::Write;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{ClipboardCommand, ForegroundJob, ForegroundProcess, Signal};

pub(crate) use super::unix_common::{write_client_stream, ClientStreamReader};

mod bootstrap;
pub(crate) use bootstrap::{configure_server_daemon_context, prepare_server_process};

#[cfg(test)]
mod config_file_tests;

pub(crate) fn config_file_link_count(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::metadata(path)?.nlink())
}

pub(crate) fn check_config_write_target(_target: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

pub(crate) fn write_existing_config(
    _target: &std::path::Path,
    _contents: &[u8],
) -> std::io::Result<bool> {
    // Unix keeps atomic replacement for existing files too.
    Ok(false)
}

pub(crate) fn create_config_temporary(
    path: &Path,
    private: bool,
) -> std::io::Result<std::fs::File> {
    if !private {
        return std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path);
    }
    use std::os::fd::FromRawFd;
    // Darwin's opaque ACL/filesec APIs (<sys/acl.h>, <sys/fcntl.h>) are not
    // exposed by libc. Supply a non-inheriting empty ACL at creation: clearing
    // inherited ACEs later cannot revoke descriptors opened in the meantime.
    unsafe extern "C" {
        fn acl_init(count: libc::c_int) -> *mut libc::c_void;
        fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
        fn acl_get_flagset_np(acl: *mut libc::c_void, flags: *mut *mut libc::c_void)
            -> libc::c_int;
        fn acl_add_flag_np(flags: *mut libc::c_void, flag: libc::c_uint) -> libc::c_int;
        fn filesec_init() -> *mut libc::c_void;
        fn filesec_free(security: *mut libc::c_void);
        fn filesec_set_property(
            security: *mut libc::c_void,
            property: libc::c_int,
            value: *const libc::c_void,
        ) -> libc::c_int;
        fn openx_np(
            path: *const libc::c_char,
            flags: libc::c_int,
            security: *mut libc::c_void,
        ) -> libc::c_int;
    }
    const FILESEC_MODE: libc::c_int = 4;
    const FILESEC_ACL: libc::c_int = 5;
    const ACL_FLAG_NO_INHERIT: libc::c_uint = 1 << 17;
    let path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let acl = unsafe { acl_init(0) };
    if acl.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let security = unsafe { filesec_init() };
    if security.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe {
            acl_free(acl);
        }
        return Err(error);
    }
    let result = (|| {
        let mut flags = std::ptr::null_mut();
        let mode: libc::mode_t = 0o600;
        if unsafe { acl_get_flagset_np(acl, &mut flags) } != 0
            || unsafe { acl_add_flag_np(flags, ACL_FLAG_NO_INHERIT) } != 0
            || unsafe {
                filesec_set_property(security, FILESEC_MODE, std::ptr::from_ref(&mode).cast())
            } != 0
            || unsafe {
                filesec_set_property(security, FILESEC_ACL, std::ptr::from_ref(&acl).cast())
            } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let fd = unsafe {
            openx_np(
                path.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                security,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // The successful exclusive create returns one owned descriptor.
        Ok(unsafe { std::fs::File::from_raw_fd(fd) })
    })();
    unsafe {
        filesec_free(security);
        acl_free(acl);
    }
    result
}

pub(crate) fn write_config_temporary(
    source: Option<&Path>,
    temporary: &Path,
    contents: &[u8],
) -> std::io::Result<()> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(temporary)?;
    if let Some(source) = source {
        let input = std::fs::File::open(source)?;
        let metadata = input.metadata()?;
        let current = output.metadata()?;
        if (metadata.uid(), metadata.gid()) != (current.uid(), current.gid())
            && unsafe { libc::fchown(output.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        // Prepare access controls while the temporary is still empty. Copy ACLs
        // before mode bits so no inherited/default grant can expose the content.
        // Do not copy data or old timestamps.
        if unsafe {
            libc::fcopyfile(
                input.as_raw_fd(),
                output.as_raw_fd(),
                std::ptr::null_mut(),
                libc::COPYFILE_ACL | libc::COPYFILE_XATTR,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        output.set_permissions(metadata.permissions())?;
    }
    output.write_all(contents)?;
    output.sync_all()
}

const PROC_PGRP_ONLY: u32 = 2;
const SERVER_NOFILE_LIMIT_TARGET: libc::rlim_t = 8192;

pub(crate) fn interactive_shell_command(argv: &[String], shell_name: &str) -> Option<String> {
    super::interactive_unix_shell_command(argv, shell_name, shell_quote)
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
                )
        })
    {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {}

pub fn raise_server_nofile_limit() {
    match raise_nofile_limit(SERVER_NOFILE_LIMIT_TARGET) {
        Ok(None) => {}
        Ok(Some((previous, target))) => {
            tracing::info!(previous, target, "raised server file descriptor soft limit")
        }
        Err(err) => tracing::warn!(err = %err, "failed to raise server file descriptor limit"),
    }
}

fn raise_nofile_limit(
    target: libc::rlim_t,
) -> std::io::Result<Option<(libc::rlim_t, libc::rlim_t)>> {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let mut limit = unsafe { limit.assume_init() };
    let Some(target) = target_nofile_soft_limit(limit.rlim_cur, limit.rlim_max, target) else {
        return Ok(None);
    };

    let previous = limit.rlim_cur;
    limit.rlim_cur = target;
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(Some((previous, target)))
}

fn target_nofile_soft_limit(
    current: libc::rlim_t,
    hard: libc::rlim_t,
    target: libc::rlim_t,
) -> Option<libc::rlim_t> {
    let target = if hard == libc::RLIM_INFINITY {
        target
    } else {
        target.min(hard)
    };

    (current < target).then_some(target)
}

pub(crate) fn available_pane_shell(child_pid: u32) -> Option<String> {
    super::available_pane_shell_from_job(child_pid, foreground_job(child_pid)?)
}

/// Collect the foreground terminal job for a given child PID.
pub fn foreground_job(child_pid: u32) -> Option<ForegroundJob> {
    if child_pid == 0 {
        return None;
    }

    let fg_pgid = foreground_process_group_id(child_pid)?;
    let mut processes = Vec::new();

    for pid in process_group_pids(fg_pgid) {
        let Some(info) = process_bsdinfo(pid) else {
            continue;
        };
        if info.pbi_pgid != fg_pgid {
            continue;
        }

        let Some(name) = comm_from_bsdinfo(&info) else {
            continue;
        };
        let argv = process_argv(pid);
        processes.push(ForegroundProcess {
            pid,
            name,
            argv0: process_argv0_name(pid),
            cmdline: argv.as_ref().map(|parts| parts.join(" ")),
            argv,
        });
    }

    if processes.is_empty() {
        return None;
    }

    Some(ForegroundJob {
        process_group_id: fg_pgid,
        processes,
    })
}

pub fn foreground_group_leader_job(process_group_id: u32) -> Option<ForegroundJob> {
    let info = process_bsdinfo(process_group_id)?;
    if info.pbi_pgid != process_group_id {
        return None;
    }

    let name = comm_from_bsdinfo(&info)?;
    let argv = process_argv(process_group_id);
    Some(ForegroundJob {
        process_group_id,
        processes: vec![ForegroundProcess {
            pid: process_group_id,
            name,
            argv0: process_argv0_name(process_group_id),
            cmdline: argv.as_ref().map(|parts| parts.join(" ")),
            argv,
        }],
    })
}

fn process_group_pids(process_group_id: u32) -> Vec<u32> {
    let mut capacity = 16usize;

    for _ in 0..8 {
        let mut pids = vec![0 as libc::pid_t; capacity];
        let buffer_bytes = pids.len() * std::mem::size_of::<libc::pid_t>();
        let returned_bytes = unsafe {
            libc::proc_listpids(
                PROC_PGRP_ONLY,
                process_group_id,
                pids.as_mut_ptr() as *mut libc::c_void,
                buffer_bytes as libc::c_int,
            )
        };
        if returned_bytes <= 0 {
            return Vec::new();
        }

        let returned_bytes = returned_bytes as usize;
        let count = returned_bytes / std::mem::size_of::<libc::pid_t>();
        if returned_bytes < buffer_bytes {
            return collect_positive_pids(pids, count);
        }
        capacity = capacity.saturating_mul(2);
    }

    Vec::new()
}

/// Read `e_tpgid` (foreground process group of the controlling terminal)
/// for the given PID.
pub fn foreground_process_group_id(pid: u32) -> Option<u32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;

    let ret = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };

    if ret != size {
        return None;
    }

    let fg = info.e_tpgid;
    if fg > 0 {
        #[allow(clippy::unnecessary_cast)] // info.e_tpgid (pid_t) type is platform-dependent
        Some(fg as u32)
    } else {
        None
    }
}

pub fn foreground_process_group_id_for_tty_fd(fd: RawFd) -> Option<u32> {
    let pgid = unsafe { libc::tcgetpgrp(fd) };
    (pgid > 0).then_some(pgid as u32)
}

/// Get the effective process name from `argv[0]` via `sysctl(KERN_PROCARGS2)`.
///
/// This is the macOS equivalent of reading `/proc/{pid}/cmdline` on Linux.
/// It reflects runtime title changes like Node.js `process.title = "pi"`.
fn process_argv0_name(pid: u32) -> Option<String> {
    let buf = kern_procargs2(pid)?;

    // Layout: [argc: i32] [exec_path\0] [padding\0...] [argv[0]\0] [argv[1]\0] ...
    if buf.len() < 4 {
        return None;
    }

    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if argc < 1 {
        return None;
    }

    // Skip past exec_path and null padding to reach argv[0]
    let rest = &buf[4..];
    let exec_end = rest.iter().position(|&b| b == 0)?;
    let mut pos = exec_end;
    while pos < rest.len() && rest[pos] == 0 {
        pos += 1;
    }
    if pos >= rest.len() {
        return None;
    }

    // Read argv[0]
    let argv0_end = rest[pos..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(rest.len() - pos);
    let argv0 = std::str::from_utf8(&rest[pos..pos + argv0_end]).ok()?;

    if argv0.is_empty() {
        return None;
    }

    // Return basename (argv[0] may be a full path like "/usr/bin/node")
    let basename = Path::new(argv0).file_name()?.to_str()?;

    // Strip leading dash (login shells show as "-zsh")
    let name = basename.strip_prefix('-').unwrap_or(basename);
    if name.is_empty() {
        return None;
    }

    Some(name.to_string())
}

/// Raw `sysctl(KERN_PROCARGS2)` call. Returns the full buffer.
fn kern_procargs2(pid: u32) -> Option<Vec<u8>> {
    unsafe {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];

        // First call: query required buffer size
        let mut size: libc::size_t = 0;
        let ret = libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        );
        if ret != 0 || size == 0 {
            return None;
        }

        // Second call: read data
        let mut buf = vec![0u8; size];
        let ret = libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        );
        if ret != 0 {
            return None;
        }
        buf.truncate(size);
        Some(buf)
    }
}

pub fn write_clipboard(bytes: &[u8]) -> bool {
    run_clipboard_command(
        &ClipboardCommand {
            program: "pbcopy",
            args: &[],
        },
        bytes,
    )
}

fn run_clipboard_command(command: &ClipboardCommand, bytes: &[u8]) -> bool {
    let mut child = match Command::new(command.program)
        .args(command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    };

    if stdin.write_all(bytes).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    drop(stdin);

    child.wait().map(|status| status.success()).unwrap_or(false)
}

fn process_bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;

    let ret = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };

    (ret == size).then_some(info)
}

fn comm_from_bsdinfo(info: &libc::proc_bsdinfo) -> Option<String> {
    let end = info
        .pbi_comm
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(info.pbi_comm.len());
    if end == 0 {
        return None;
    }

    let bytes: Vec<u8> = info.pbi_comm[..end].iter().map(|&b| b as u8).collect();
    String::from_utf8(bytes).ok()
}

fn process_argv(pid: u32) -> Option<Vec<String>> {
    let buf = kern_procargs2(pid)?;
    procargs2_argv(&buf)
}

/// Read a sesh-mux agent identity hint from a process environment.
pub fn process_agent_hint(pid: u32) -> Option<crate::detect::Agent> {
    if pid == 0 {
        return None;
    }
    let buf = kern_procargs2(pid)?;
    super::parse_agent_env_hint(procargs2_env(&buf)?)
}

fn procargs2_argv_start(rest: &[u8]) -> Option<usize> {
    let exec_end = rest.iter().position(|&byte| byte == 0)?;
    let mut pos = exec_end;
    while pos < rest.len() && rest[pos] == 0 {
        pos += 1;
    }
    (pos < rest.len()).then_some(pos)
}

fn skip_nul_strings(bytes: &[u8], start: usize, count: usize) -> Option<usize> {
    let mut current = start;
    for _ in 0..count {
        let end = bytes.get(current..)?.iter().position(|&byte| byte == 0)?;
        current = current.checked_add(end)?.checked_add(1)?;
    }
    Some(current)
}

fn procargs2_argv(buf: &[u8]) -> Option<Vec<String>> {
    if buf.len() < 4 {
        return None;
    }

    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if argc < 1 {
        return None;
    }

    // Layout: [argc: i32] [exec_path\0] [padding\0...] [argv[0]\0] ... [env\0] ...
    let rest = &buf[4..];
    let mut current = procargs2_argv_start(rest)?;
    let mut argv = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        if current >= rest.len() {
            return None;
        }
        let end = rest[current..]
            .iter()
            .position(|&b| b == 0)
            .map(|offset| current + offset)
            .unwrap_or(rest.len());
        if end == current {
            return None;
        }
        argv.push(String::from_utf8_lossy(&rest[current..end]).into_owned());
        current = end + 1;
    }

    Some(argv)
}

fn procargs2_env(buf: &[u8]) -> Option<&[u8]> {
    if buf.len() < 4 {
        return None;
    }

    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if argc < 1 {
        return None;
    }

    let rest = &buf[4..];
    let argv_start = procargs2_argv_start(rest)?;
    let env_start = skip_nul_strings(rest, argv_start, argc as usize)?;
    rest.get(env_start..)
}

/// Get the current working directory of a process.
///
/// Uses `proc_pidinfo(PROC_PIDVNODEPATHINFO)` to read `pvi_cdir.vip_path`.
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    if pid == 0 {
        return None;
    }

    let mut pathinfo: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;

    let ret = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut pathinfo as *mut _ as *mut libc::c_void,
            size,
        )
    };

    if ret != size {
        return None;
    }

    // vip_path is [[c_char; 32]; 32] in libc (workaround for old Rust const generics).
    // Reinterpret as flat bytes (total MAXPATHLEN = 1024).
    let vip_path = unsafe {
        std::slice::from_raw_parts(
            pathinfo.pvi_cdir.vip_path.as_ptr() as *const u8,
            libc::MAXPATHLEN as usize,
        )
    };

    let nul = vip_path.iter().position(|&b| b == 0)?;
    if nul == 0 {
        return None;
    }
    Some(PathBuf::from(OsStr::from_bytes(&vip_path[..nul])))
}

pub fn session_processes(child_pid: u32) -> Vec<u32> {
    if child_pid == 0 {
        return Vec::new();
    }

    let target_session = unsafe { libc::getsid(child_pid as libc::c_int) };
    if target_session <= 0 {
        return Vec::new();
    }

    all_pids()
        .into_iter()
        .filter(|pid| unsafe { libc::getsid(*pid as libc::pid_t) } == target_session)
        .collect()
}

fn all_pids() -> Vec<u32> {
    let initial_count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let mut capacity = if initial_count > 0 {
        initial_count as usize + 128
    } else {
        4096
    };

    for _ in 0..8 {
        let mut pids = vec![0 as libc::pid_t; capacity];
        let count = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr() as *mut libc::c_void,
                (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int,
            )
        };
        if count <= 0 {
            return Vec::new();
        }

        let count = count as usize;
        if count < capacity {
            return collect_positive_pids(pids, count);
        }
        capacity = capacity.saturating_mul(2);
    }

    Vec::new()
}

fn collect_positive_pids(pids: Vec<libc::pid_t>, count: usize) -> Vec<u32> {
    pids.into_iter()
        .take(count)
        .filter(|pid| *pid > 0)
        .map(|pid| pid as u32)
        .collect()
}

pub fn signal_processes(pids: &[u32], signal: Signal) {
    let sig = match signal {
        Signal::Hangup => libc::SIGHUP,
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };

    for &pid in pids {
        if pid == 0 {
            continue;
        }
        unsafe {
            libc::kill(pid as libc::c_int, sig);
        }
    }
}

pub fn process_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let result = unsafe { libc::kill(pid as libc::c_int, 0) };
    if result == 0 {
        true
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nofile_target_raises_low_soft_limit_to_cap_when_hard_is_unlimited() {
        assert_eq!(
            target_nofile_soft_limit(256, libc::RLIM_INFINITY, 8192),
            Some(8192)
        );
    }

    #[test]
    fn nofile_target_respects_finite_hard_limit() {
        assert_eq!(target_nofile_soft_limit(256, 4096, 8192), Some(4096));
    }

    #[test]
    fn nofile_target_does_not_lower_existing_soft_limit() {
        assert_eq!(
            target_nofile_soft_limit(16_384, libc::RLIM_INFINITY, 8192),
            None
        );
    }

    fn build_procargs2(exec_path: &str, argv: &[&str], env: &[&str]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(argv.len() as i32).to_ne_bytes());
        buf.extend_from_slice(exec_path.as_bytes());
        buf.push(0);
        buf.push(0);
        for arg in argv {
            buf.extend_from_slice(arg.as_bytes());
            buf.push(0);
        }
        for entry in env {
            buf.extend_from_slice(entry.as_bytes());
            buf.push(0);
        }
        buf
    }

    #[test]
    fn procargs2_argv_excludes_environment_entries() {
        let buf = build_procargs2(
            "/usr/bin/node",
            &["node", "/Users/can/.local/bin/pi"],
            &[
                "PATH=/usr/bin:/var/run/com.apple.security.cryptexd/codex.system/bootstrap/usr/bin",
                "TERM=tmux-256color",
            ],
        );

        let argv = procargs2_argv(&buf).expect("expected argv");
        assert_eq!(argv, vec!["node", "/Users/can/.local/bin/pi"]);
        assert_eq!(argv.join(" "), "node /Users/can/.local/bin/pi");
        assert!(!argv.join(" ").contains("codex.system"));
    }

    #[test]
    fn procargs2_env_reads_agent_hint_after_argv() {
        let buf = build_procargs2(
            "/opt/homebrew/bin/nono",
            &["nono", "run", "SESH_MUX_AGENT=codex", "--", "claude"],
            &[
                "PATH=/usr/bin",
                "SESH_MUX_AGENT=claude",
                "TERM=xterm-256color",
            ],
        );

        let env = procargs2_env(&buf).expect("expected env block");
        assert_eq!(
            crate::platform::parse_agent_env_hint(env),
            Some(crate::detect::Agent::Claude)
        );
    }

    #[test]
    fn procargs2_env_does_not_treat_argv_as_environment() {
        let buf = build_procargs2(
            "/opt/homebrew/bin/nono",
            &["nono", "run", "SESH_MUX_AGENT=claude"],
            &["PATH=/usr/bin"],
        );

        let env = procargs2_env(&buf).expect("expected env block");
        assert_eq!(crate::platform::parse_agent_env_hint(env), None);
    }
}
