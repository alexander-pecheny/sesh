//! The C ABI. The only module with `unsafe`.
//!
//! Every callback in `sesh_callbacks_t` is invoked from a tokio worker thread, never from
//! the thread that called `sesh_ssh_connect`. Pointers handed to a callback are borrowed
//! for the duration of the call only; copy what you keep. Answer `on_host_key` with
//! `sesh_session_answer_host_key` and `on_auth_prompt` with `sesh_session_answer_prompt`;
//! until you do, the Session waits. Every function is safe to call from any thread.
#![allow(non_camel_case_types, clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::Arc;

use crate::flags::{self, Transport};
use crate::mosh;
use crate::session::{self, Prompt, Session, State};
use crate::ssh;

#[repr(C)]
#[derive(Clone, Copy)]
pub enum sesh_state_t {
    SESH_STATE_CONNECTING = 0,
    SESH_STATE_AUTHENTICATING = 1,
    SESH_STATE_CONNECTED = 2,
    SESH_STATE_CLOSED = 3,
    SESH_STATE_FAILED = 4,
    SESH_STATE_BOOTSTRAPPING = 5,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum sesh_transport_t {
    SESH_TRANSPORT_SSH = 0,
    SESH_TRANSPORT_MOSH = 1,
}

/// `previous_fingerprint` is NULL on a first sighting and the recorded fingerprint when the
/// host key changed.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct sesh_callbacks_t {
    pub on_output: Option<extern "C" fn(*mut c_void, *const u8, usize)>,
    pub on_state: Option<extern "C" fn(*mut c_void, sesh_state_t, *const c_char)>,
    pub on_host_key: Option<extern "C" fn(*mut c_void, *const c_char, *const c_char)>,
    pub on_auth_prompt: Option<
        extern "C" fn(*mut c_void, u32, *const c_char, *const c_char, *const *const c_char, *const bool, usize),
    >,
    /// The last call on `userdata`: no callback runs after it, so it is where the embedder
    /// releases whatever `userdata` points at.
    pub on_release: Option<extern "C" fn(*mut c_void)>,
}

/// Optional fields are NULL when unset. `term` defaults to `xterm-256color`.
#[repr(C)]
pub struct sesh_ssh_config_t {
    pub host: *const c_char,
    pub port: u16,
    pub user: *const c_char,
    pub password: *const c_char,
    pub key_pem: *const c_char,
    pub key_passphrase: *const c_char,
    pub known_hosts_path: *const c_char,
    pub term: *const c_char,
    pub remote_command: *const c_char,
    pub extra_flags: *const c_char,
    pub cols: u16,
    pub rows: u16,
    pub agent_forwarding: bool,
}

/// Optional fields are NULL when unset. `extra_flags` takes the mosh subset, whose
/// `--ssh=` value carries the ssh one.
#[repr(C)]
pub struct sesh_mosh_config_t {
    pub host: *const c_char,
    pub port: u16,
    pub user: *const c_char,
    pub password: *const c_char,
    pub key_pem: *const c_char,
    pub key_passphrase: *const c_char,
    pub known_hosts_path: *const c_char,
    pub remote_command: *const c_char,
    pub extra_flags: *const c_char,
    pub cols: u16,
    pub rows: u16,
}

pub struct sesh_session_t {
    session: Session,
}

struct Sink {
    callbacks: sesh_callbacks_t,
    userdata: *mut c_void,
}

// The embedder owns `userdata` and promises it outlives the Session; see the module note.
unsafe impl Send for Sink {}
unsafe impl Sync for Sink {}

impl Drop for Sink {
    fn drop(&mut self) {
        if let Some(callback) = self.callbacks.on_release {
            callback(self.userdata);
        }
    }
}

impl session::Events for Sink {
    fn output(&self, bytes: &[u8]) {
        if let Some(callback) = self.callbacks.on_output {
            callback(self.userdata, bytes.as_ptr(), bytes.len());
        }
    }

    fn state(&self, state: State, message: &str) {
        let Some(callback) = self.callbacks.on_state else { return };
        let message = CString::new(message).unwrap_or_default();
        let state = match state {
            State::Connecting => sesh_state_t::SESH_STATE_CONNECTING,
            State::Authenticating => sesh_state_t::SESH_STATE_AUTHENTICATING,
            State::Connected => sesh_state_t::SESH_STATE_CONNECTED,
            State::Closed => sesh_state_t::SESH_STATE_CLOSED,
            State::Failed => sesh_state_t::SESH_STATE_FAILED,
            State::Bootstrapping => sesh_state_t::SESH_STATE_BOOTSTRAPPING,
        };
        callback(self.userdata, state, message.as_ptr());
    }

    fn host_key(&self, fingerprint: &str, previous: Option<&str>) {
        let Some(callback) = self.callbacks.on_host_key else { return };
        let fingerprint = CString::new(fingerprint).unwrap_or_default();
        let previous = previous.map(|p| CString::new(p).unwrap_or_default());
        callback(
            self.userdata,
            fingerprint.as_ptr(),
            previous.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
        );
    }

    fn auth_prompt(&self, id: u32, name: &str, instruction: &str, prompts: &[Prompt]) {
        let Some(callback) = self.callbacks.on_auth_prompt else { return };
        let name = CString::new(name).unwrap_or_default();
        let instruction = CString::new(instruction).unwrap_or_default();
        let texts: Vec<CString> = prompts
            .iter()
            .map(|p| CString::new(p.prompt.as_str()).unwrap_or_default())
            .collect();
        let pointers: Vec<*const c_char> = texts.iter().map(|t| t.as_ptr()).collect();
        let echoes: Vec<bool> = prompts.iter().map(|p| p.echo).collect();
        callback(
            self.userdata,
            id,
            name.as_ptr(),
            instruction.as_ptr(),
            pointers.as_ptr(),
            echoes.as_ptr(),
            pointers.len(),
        );
    }
}

unsafe fn text(pointer: *const c_char) -> Option<String> {
    (!pointer.is_null())
        .then(|| CStr::from_ptr(pointer).to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

fn raw(text: String) -> *mut c_char {
    CString::new(text).unwrap_or_default().into_raw()
}

/// Starts a Session and returns immediately; progress arrives through the callbacks.
/// Returns NULL only when `config` is NULL.
#[no_mangle]
pub unsafe extern "C" fn sesh_ssh_connect(
    config: *const sesh_ssh_config_t,
    callbacks: sesh_callbacks_t,
    userdata: *mut c_void,
) -> *mut sesh_session_t {
    let Some(config) = config.as_ref() else {
        return std::ptr::null_mut();
    };
    let session = ssh::connect(
        ssh::Config {
            host: text(config.host).unwrap_or_default(),
            port: if config.port == 0 { 22 } else { config.port },
            user: text(config.user).unwrap_or_default(),
            password: text(config.password),
            key: text(config.key_pem),
            key_passphrase: text(config.key_passphrase),
            known_hosts: text(config.known_hosts_path).map(PathBuf::from).unwrap_or_default(),
            term: text(config.term).unwrap_or_else(|| "xterm-256color".into()),
            remote_command: text(config.remote_command),
            extra_flags: text(config.extra_flags).unwrap_or_default(),
            cols: config.cols,
            rows: config.rows,
            agent_forwarding: config.agent_forwarding,
        },
        Arc::new(Sink { callbacks, userdata }),
    );
    Box::into_raw(Box::new(sesh_session_t { session }))
}

/// Starts a mosh Session: `mosh-server` over ssh, then rmosh's client over UDP.
/// Returns NULL only when `config` is NULL.
#[no_mangle]
pub unsafe extern "C" fn sesh_mosh_connect(
    config: *const sesh_mosh_config_t,
    callbacks: sesh_callbacks_t,
    userdata: *mut c_void,
) -> *mut sesh_session_t {
    let Some(config) = config.as_ref() else {
        return std::ptr::null_mut();
    };
    let session = mosh::connect(
        mosh::Config {
            host: text(config.host).unwrap_or_default(),
            port: if config.port == 0 { 22 } else { config.port },
            user: text(config.user).unwrap_or_default(),
            password: text(config.password),
            key: text(config.key_pem),
            key_passphrase: text(config.key_passphrase),
            known_hosts: text(config.known_hosts_path).map(PathBuf::from).unwrap_or_default(),
            remote_command: text(config.remote_command),
            extra_flags: text(config.extra_flags).unwrap_or_default(),
            cols: config.cols,
            rows: config.rows,
        },
        Arc::new(Sink { callbacks, userdata }),
    );
    Box::into_raw(Box::new(sesh_session_t { session }))
}

#[no_mangle]
pub unsafe extern "C" fn sesh_session_write(
    session: *mut sesh_session_t,
    bytes: *const u8,
    len: usize,
) {
    if let (Some(session), false) = (session.as_ref(), bytes.is_null() || len == 0) {
        session.session.write(std::slice::from_raw_parts(bytes, len));
    }
}

#[no_mangle]
pub unsafe extern "C" fn sesh_session_resize(session: *mut sesh_session_t, cols: u16, rows: u16) {
    if let Some(session) = session.as_ref() {
        session.session.resize(cols, rows);
    }
}

#[no_mangle]
pub unsafe extern "C" fn sesh_session_answer_host_key(session: *mut sesh_session_t, accept: bool) {
    if let Some(session) = session.as_ref() {
        session.session.answer_host_key(accept);
    }
}

/// Pass `answers` = NULL to cancel the prompt and so the Session.
#[no_mangle]
pub unsafe extern "C" fn sesh_session_answer_prompt(
    session: *mut sesh_session_t,
    id: u32,
    answers: *const *const c_char,
    count: usize,
) {
    let Some(session) = session.as_ref() else { return };
    let given = (!answers.is_null()).then(|| {
        std::slice::from_raw_parts(answers, count)
            .iter()
            .map(|p| text(*p).unwrap_or_default())
            .collect()
    });
    session.session.answer_prompt(id, given);
}

#[no_mangle]
pub unsafe extern "C" fn sesh_session_close(session: *mut sesh_session_t) {
    if let Some(session) = session.as_ref() {
        session.session.close();
    }
}

#[no_mangle]
pub unsafe extern "C" fn sesh_session_free(session: *mut sesh_session_t) {
    if !session.is_null() {
        drop(Box::from_raw(session));
    }
}

/// Validates Extra flags. On failure returns false and, when `error` is non-NULL, stores a
/// message naming the offending flag; free it with `sesh_string_free`.
#[no_mangle]
pub unsafe extern "C" fn sesh_parse_flags(
    transport: sesh_transport_t,
    input: *const c_char,
    error: *mut *mut c_char,
) -> bool {
    let transport = match transport {
        sesh_transport_t::SESH_TRANSPORT_SSH => Transport::Ssh,
        sesh_transport_t::SESH_TRANSPORT_MOSH => Transport::Mosh,
    };
    match flags::validate(transport, &text(input).unwrap_or_default()) {
        Ok(()) => true,
        Err(message) => {
            if !error.is_null() {
                *error = raw(message);
            }
            false
        }
    }
}

/// Returns the OpenSSH public key line for a private key, or NULL with `error` set.
#[no_mangle]
pub unsafe extern "C" fn sesh_public_key(
    pem: *const c_char,
    passphrase: *const c_char,
    error: *mut *mut c_char,
) -> *mut c_char {
    let result = russh::keys::decode_secret_key(
        &text(pem).unwrap_or_default(),
        text(passphrase).as_deref(),
    )
    .map_err(|e| e.to_string())
    .and_then(|key| key.public_key().to_openssh().map_err(|e| e.to_string()));
    match result {
        Ok(line) => raw(line),
        Err(message) => {
            if !error.is_null() {
                *error = raw(message);
            }
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn sesh_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(CString::from_raw(text));
    }
}
