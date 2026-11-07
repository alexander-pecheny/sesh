use std::cell::Cell;
use std::io;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

use bytes::Bytes;
use portable_pty::CommandBuilder;
use ratatui::{layout::Rect, Frame};
#[cfg(test)]
use tokio::sync::watch;
use tokio::sync::{mpsc, Notify};
use tracing::debug;
use tracing::{error, info, warn};

use crate::detect::{Agent, AgentState};
use crate::events::AppEvent;
use crate::layout::PaneId;
use crate::pty::actor::{PtyIoActor, PtyIoActorConfig, PtyIoActorHandle, PtyReadResult};
use crate::render_signal::RenderSignal;

mod agent_detection;
mod cursor;
mod input;
mod kitty_keyboard;
mod osc;
mod state;
mod terminal;
mod xtgettcap;

use self::agent_detection::{
    decide_detection_screen_read, decide_screen_detection_publish,
    detection_update_for_publish_with_osc, mark_detection_content_changed,
    observe_detection_content_change, DetectionPublishDecision, DetectionScreenReadDecision,
    DetectionScreenReadInput, PendingIdleConfirmation, ScreenDetectionPublishInput,
    AGENT_PENDING_IDLE_RECHECK, AGENT_STARTUP_GRACE_WINDOW,
};
pub use self::terminal::InputState;
use self::terminal::{GhosttyPaneTerminal, PaneTerminal};
pub(crate) use self::terminal::{TerminalCompressionStep, TerminalReadSnapshot};
pub use self::{
    state::PaneState,
    terminal::{ScrollMetrics, TerminalCursorState},
};

const RELEASE_REACQUIRE_SUPPRESSION: std::time::Duration = std::time::Duration::from_secs(1);
const TERMINAL_COMPRESSION_IDLE: std::time::Duration = std::time::Duration::from_millis(250);
const TERMINAL_COMPRESSION_STEP: std::time::Duration = std::time::Duration::from_millis(1);
pub(crate) const PANE_TERM: &str = "xterm-256color";
const PANE_COLORTERM: &str = "truecolor";

fn terminal_compression_permits() -> Arc<tokio::sync::Semaphore> {
    static PERMITS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    PERMITS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone()
}

fn spawn_blocking_with_compression_permit<T, F>(
    permit: tokio::sync::OwnedSemaphorePermit,
    operation: F,
) -> tokio::task::JoinHandle<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
}

fn apply_pane_terminal_env(cmd: &mut CommandBuilder) {
    // Each pane is rendered by sesh-mux's own terminal layer, not the outer terminal
    // that launched the app. Advertising the inherited TERM leaks the host terminal
    // identity into shells and across SSH, which breaks redraw and cursor movement
    // when the remote side lacks matching terminfo entries.
    cmd.env("TERM", PANE_TERM);
    cmd.env("COLORTERM", PANE_COLORTERM);
    cmd.env_remove("WT_SESSION");
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PaneLaunchEnv {
    extra: Vec<(String, String)>,
    identity: PaneLaunchIdentity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum PaneLaunchIdentity {
    #[default]
    Inherit,
    Managed {
        workspace_id: String,
        tab_id: String,
        pane_id: String,
    },
    OmitPane,
}

impl PaneLaunchEnv {
    pub(crate) fn from_extra(extra: Vec<(String, String)>) -> Self {
        Self {
            extra,
            identity: PaneLaunchIdentity::Inherit,
        }
    }

    pub(crate) fn with_identity(
        mut self,
        workspace_id: String,
        tab_id: String,
        pane_id: String,
    ) -> Self {
        self.identity = PaneLaunchIdentity::Managed {
            workspace_id,
            tab_id,
            pane_id,
        };
        self
    }
}

fn apply_pane_launch_env(cmd: &mut CommandBuilder, launch_env: &PaneLaunchEnv) {
    cmd.env_remove("CODEX_THREAD_ID");
    // OMP sets OMPCODE for shells it spawns. A pane launched from inside OMP
    // must not inherit it or its root agent would look like a nested session.
    cmd.env_remove("OMPCODE");
    for (key, value) in &launch_env.extra {
        cmd.env(key, value);
    }
    crate::integration::apply_pane_base_env(cmd);
    crate::platform::apply_pane_runtime_marker(cmd);
    match &launch_env.identity {
        PaneLaunchIdentity::Inherit => {}
        PaneLaunchIdentity::Managed {
            workspace_id,
            tab_id,
            pane_id,
        } => {
            cmd.env(
                crate::integration::SESH_MUX_WORKSPACE_ID_ENV_VAR,
                workspace_id,
            );
            cmd.env(crate::integration::SESH_MUX_TAB_ID_ENV_VAR, tab_id);
            cmd.env(crate::integration::SESH_MUX_PANE_ID_ENV_VAR, pane_id);
        }
        PaneLaunchIdentity::OmitPane => {
            cmd.env_remove(crate::integration::SESH_MUX_PANE_ID_ENV_VAR);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingAgentRelease {
    agent: Agent,
    until: std::time::Instant,
}

#[derive(Clone, Copy, Default)]
struct SpawnInitialState<'a> {
    detected_agent: Option<Agent>,
    history_ansi: Option<&'a str>,
    windows_powershell_prompt_cwd_reporting: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentDetection {
    Enabled,
    Disabled,
}

fn active_pending_release(
    pending_release: &Mutex<Option<PendingAgentRelease>>,
    now: std::time::Instant,
) -> Option<Agent> {
    let mut pending_release = pending_release.lock().ok()?;
    match *pending_release {
        Some(pending) if now < pending.until => Some(pending.agent),
        Some(_) => {
            *pending_release = None;
            None
        }
        None => None,
    }
}

async fn publish_state_changed_event(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Option<Agent>,
    state: AgentState,
    visible_blocker: bool,
    visible_working: bool,
    process_exited: bool,
    observed_at: std::time::Instant,
) {
    // This runs on the async detector task, not the PTY reader thread.
    // Waiting for queue space here preserves correctness-critical state transitions
    // without blocking pane I/O.
    if let Err(e) = state_events
        .send(AppEvent::StateChanged {
            pane_id,
            agent,
            state,
            visible_blocker,
            visible_working,
            process_exited,
            observed_at,
        })
        .await
    {
        warn!(
            pane = pane_id.raw(),
            err = %e,
            "failed to deliver StateChanged event"
        );
    }
}

async fn publish_agent_process_detected_event(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Agent,
    observed_at: std::time::Instant,
) {
    if let Err(e) = state_events
        .send(AppEvent::AgentProcessDetected {
            pane_id,
            agent,
            observed_at,
        })
        .await
    {
        warn!(
            pane = pane_id.raw(),
            err = %e,
            "failed to deliver AgentProcessDetected event"
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct AgentDetectionPublishUpdate {
    state: AgentState,
    visible_idle: bool,
    visible_blocker: bool,
    visible_working: bool,
    process_exited: bool,
}

async fn apply_agent_detection_publish_update(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Option<Agent>,
    update: AgentDetectionPublishUpdate,
    observed_at: std::time::Instant,
    state: &mut AgentState,
    last_visible_idle: &mut bool,
    last_visible_blocker: &mut bool,
    last_visible_working: &mut bool,
    last_visible_signal_refresh: &mut Option<std::time::Instant>,
    foreground_shell_exit_reported: &mut bool,
) {
    *state = update.state;
    *last_visible_idle = update.visible_idle;
    *last_visible_blocker = update.visible_blocker;
    *last_visible_working = update.visible_working;
    *last_visible_signal_refresh = if update.visible_blocker || update.visible_working {
        Some(observed_at)
    } else {
        None
    };
    if update.process_exited {
        *foreground_shell_exit_reported = true;
    }
    publish_state_changed_event(
        state_events,
        pane_id,
        agent,
        update.state,
        update.visible_blocker,
        update.visible_working,
        update.process_exited,
        observed_at,
    )
    .await;
}

const AGENT_MISS_CONFIRMATION_ATTEMPTS: u8 = 6;
const PROCESS_RECHECK_IDENTIFIED: std::time::Duration = std::time::Duration::from_secs(5);
const PROCESS_RECHECK_MISSING_FOREGROUND_GROUP: std::time::Duration =
    std::time::Duration::from_secs(30);
const PROCESS_ACQUISITION_WINDOW: std::time::Duration = std::time::Duration::from_secs(8);
const PROCESS_ACQUISITION_FAST_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);
const PROCESS_ACQUISITION_FAST_RECHECK: std::time::Duration = std::time::Duration::from_millis(500);
const PROCESS_ACQUISITION_SLOW_RECHECK: std::time::Duration = std::time::Duration::from_secs(2);
const PROCESS_ACQUISITION_IDLE_RESET: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
struct AgentDetectionPresence {
    current_agent: Option<Agent>,
    consecutive_misses: u8,
}

fn absolute_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    crate::platform::process_cwd(pid).filter(|cwd| cwd.is_absolute())
}

fn usable_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    absolute_process_cwd(pid).filter(|cwd| cwd.is_dir())
}

fn foreground_member_cwd_different_from_shell(
    shell_pid: u32,
    shell_cwd: Option<&std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let job = crate::detect::foreground_job(shell_pid)?;
    for process in job.processes {
        if process.pid == shell_pid {
            continue;
        }
        let Some(cwd) = absolute_process_cwd(process.pid) else {
            continue;
        };
        if shell_cwd != Some(&cwd) {
            return Some(cwd);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForegroundShellAgentAction {
    ObserveProbe,
    ReportProcessExit,
    ReportReplacementProcess,
    ClearAgent,
}

fn foreground_shell_agent_action(
    previous_agent: Option<Agent>,
    new_agent: Option<Agent>,
    foreground_is_pane_shell: bool,
    process_exit_reported: bool,
) -> ForegroundShellAgentAction {
    let Some(previous_agent) = previous_agent else {
        return ForegroundShellAgentAction::ObserveProbe;
    };
    if process_exit_reported {
        return if new_agent == Some(previous_agent) {
            ForegroundShellAgentAction::ReportReplacementProcess
        } else if new_agent.is_none() {
            ForegroundShellAgentAction::ClearAgent
        } else {
            ForegroundShellAgentAction::ObserveProbe
        };
    }
    if new_agent.is_some() {
        return ForegroundShellAgentAction::ObserveProbe;
    }

    if foreground_is_pane_shell {
        // Do not clear identity immediately. First publish an idle process-exit
        // transition for the previous agent so notifications and wait-agent callers
        // observe completion before the pane becomes unknown.
        return ForegroundShellAgentAction::ReportProcessExit;
    }

    ForegroundShellAgentAction::ObserveProbe
}

/// Drops retained OSC evidence when changing away from an identified agent.
/// First acquisition keeps bytes that the newly identified process may have
/// emitted before the process probe recognized it.
fn clear_osc_evidence_for_agent_transition(terminal: &PaneTerminal, previous_agent: Option<Agent>) {
    if previous_agent.is_some() {
        terminal.clear_agent_osc_state();
    }
}

fn apply_foreground_shell_agent_action(
    agent_presence: &mut AgentDetectionPresence,
    action: ForegroundShellAgentAction,
    previous_agent: Option<Agent>,
    new_agent: Option<Agent>,
    pending_foreground_shell_clear: &mut bool,
    foreground_shell_exit_reported: &mut bool,
) -> bool {
    match action {
        ForegroundShellAgentAction::ReportReplacementProcess => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.observe_process_probe(previous_agent);
            true
        }
        ForegroundShellAgentAction::ReportProcessExit => {
            *pending_foreground_shell_clear = true;
            false
        }
        ForegroundShellAgentAction::ClearAgent => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.clear_current_agent()
        }
        ForegroundShellAgentAction::ObserveProbe => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.observe_process_probe(new_agent)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcessProbeInput {
    current_agent: Option<Agent>,
    suppressed_agent: Option<Agent>,
    foreground_pgid: Option<u32>,
    last_foreground_pgid: Option<u32>,
    has_process_probe: bool,
    acquisition_age: Option<std::time::Duration>,
    pending_foreground_shell_clear: bool,
    pending_restore_probe: bool,
    elapsed_since_process_check: std::time::Duration,
}

fn foreground_group_changed(
    foreground_pgid: Option<u32>,
    last_foreground_pgid: Option<u32>,
) -> bool {
    foreground_pgid != last_foreground_pgid
        && (foreground_pgid.is_some() || last_foreground_pgid.is_some())
}

// Only kernel-observed foreground groups drive change detection. Remembering an
// inferred group would look like a change on every tick while the kernel stays silent.
fn process_group_for_change_tracking(
    observed_foreground_pgid: Option<u32>,
    probed_process_group_id: Option<u32>,
) -> Option<u32> {
    observed_foreground_pgid?;
    probed_process_group_id.or(observed_foreground_pgid)
}

fn should_skip_process_probe_for_lifecycle_authority(
    full_lifecycle_authority_active: bool,
    input: ProcessProbeInput,
) -> bool {
    full_lifecycle_authority_active
        && input.foreground_pgid.is_some()
        && !input.pending_foreground_shell_clear
        && input.suppressed_agent.is_none()
        && input.has_process_probe
        && !foreground_group_changed(input.foreground_pgid, input.last_foreground_pgid)
}

fn should_probe_foreground_job(input: ProcessProbeInput) -> bool {
    if input.pending_foreground_shell_clear || input.pending_restore_probe {
        return true;
    }

    let foreground_group_changed =
        foreground_group_changed(input.foreground_pgid, input.last_foreground_pgid);

    if input.suppressed_agent.is_some() {
        return !input.has_process_probe || foreground_group_changed;
    }

    if let Some(acquisition_age) = input.acquisition_age {
        let acquisition_interval = if acquisition_age <= PROCESS_ACQUISITION_FAST_WINDOW {
            PROCESS_ACQUISITION_FAST_RECHECK
        } else {
            PROCESS_ACQUISITION_SLOW_RECHECK
        };
        if acquisition_age <= PROCESS_ACQUISITION_WINDOW
            && input.elapsed_since_process_check >= acquisition_interval
        {
            return true;
        }
    }

    if input.current_agent.is_none() {
        return !input.has_process_probe
            || foreground_group_changed
            || (input.foreground_pgid.is_none()
                && input.elapsed_since_process_check >= PROCESS_RECHECK_MISSING_FOREGROUND_GROUP);
    }

    foreground_group_changed || input.elapsed_since_process_check >= PROCESS_RECHECK_IDENTIFIED
}

fn sync_content_change_acquisition(
    current_agent: Option<Agent>,
    suppressed_agent: Option<Agent>,
    process_group_changed: bool,
    content_changed: bool,
    now: std::time::Instant,
    acquisition_started_at: &mut Option<std::time::Instant>,
    last_content_change_at: &mut Option<std::time::Instant>,
) {
    if current_agent.is_some() || suppressed_agent.is_some() || process_group_changed {
        return;
    }

    if content_changed {
        let should_start = acquisition_started_at.is_none_or(|started| {
            now.duration_since(started) > PROCESS_ACQUISITION_WINDOW
                && last_content_change_at.is_none_or(|last_change| {
                    now.duration_since(last_change) >= PROCESS_ACQUISITION_IDLE_RESET
                })
        });
        if should_start {
            *acquisition_started_at = Some(now);
        }
        *last_content_change_at = Some(now);
        return;
    }

    let Some(acquisition_started) = *acquisition_started_at else {
        return;
    };
    let Some(last_content_change) = *last_content_change_at else {
        return;
    };

    if now.duration_since(acquisition_started) > PROCESS_ACQUISITION_WINDOW
        && now.duration_since(last_content_change) >= PROCESS_ACQUISITION_IDLE_RESET
    {
        *acquisition_started_at = None;
        *last_content_change_at = None;
    }
}

#[derive(Debug, Clone)]
struct ProcessProbeResult {
    process_group_id: Option<u32>,
    foreground_is_pane_shell: bool,
    agent: Option<Agent>,
    process_name: Option<String>,
}

fn agent_hint_for_foreground_job_members(
    job: &crate::platform::ForegroundJob,
    read_hint: impl Fn(u32) -> Option<Agent>,
) -> Option<Agent> {
    read_hint(job.process_group_id)
        .or_else(|| agent_hint_for_non_leader_foreground_job_members(job, read_hint))
}

fn agent_hint_for_non_leader_foreground_job_members(
    job: &crate::platform::ForegroundJob,
    read_hint: impl Fn(u32) -> Option<Agent>,
) -> Option<Agent> {
    job.processes
        .iter()
        .filter(|process| process.pid != job.process_group_id)
        .find_map(|process| read_hint(process.pid))
}

fn identify_process_group_leader_in_job(
    job: &crate::platform::ForegroundJob,
) -> Option<(Agent, String)> {
    let leader = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group_id)?;
    let leader_job = crate::platform::ForegroundJob {
        process_group_id: job.process_group_id,
        processes: vec![leader.clone()],
    };
    crate::detect::identify_agent_in_job(&leader_job)
}

fn process_probe_result(
    job: &crate::platform::ForegroundJob,
    pid: u32,
    agent: Agent,
    process_name: String,
) -> ProcessProbeResult {
    ProcessProbeResult {
        process_group_id: Some(job.process_group_id),
        foreground_is_pane_shell: job.processes.iter().any(|process| process.pid == pid),
        agent: Some(agent),
        process_name: Some(process_name),
    }
}

fn hinted_process_probe_result(
    job: &crate::platform::ForegroundJob,
    pid: u32,
    read_hint: impl Fn(u32) -> Option<Agent>,
) -> Option<ProcessProbeResult> {
    let agent = agent_hint_for_foreground_job_members(job, read_hint)?;
    Some(process_probe_result(
        job,
        pid,
        agent,
        crate::detect::agent_label(agent).to_string(),
    ))
}

fn probe_foreground_process_from_jobs(
    pid: u32,
    foreground_pgid: Option<u32>,
    leader_job: Option<crate::platform::ForegroundJob>,
    foreground_job: impl FnOnce() -> Option<crate::platform::ForegroundJob>,
    read_hint: impl Fn(u32) -> Option<Agent> + Copy,
) -> ProcessProbeResult {
    if let Some(job) = leader_job.as_ref() {
        if let Some(hinted) = hinted_process_probe_result(job, pid, read_hint) {
            return hinted;
        }
        if let Some((agent, process_name)) = crate::detect::identify_agent_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
    }

    let foreground_job = foreground_job();
    if let Some(job) = foreground_job.as_ref() {
        if let Some(agent) = read_hint(job.process_group_id) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::detect::agent_label(agent).to_string(),
            );
        }
        if let Some((agent, process_name)) = identify_process_group_leader_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
        if let Some(agent) = agent_hint_for_non_leader_foreground_job_members(job, read_hint) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::detect::agent_label(agent).to_string(),
            );
        }

        let identified = crate::detect::identify_agent_in_job(job);
        return ProcessProbeResult {
            process_group_id: Some(job.process_group_id),
            foreground_is_pane_shell: job.processes.iter().any(|process| process.pid == pid),
            agent: identified.as_ref().map(|(agent, _)| *agent),
            process_name: identified.map(|(_, process_name)| process_name),
        };
    }

    ProcessProbeResult {
        process_group_id: foreground_pgid,
        foreground_is_pane_shell: false,
        agent: None,
        process_name: None,
    }
}

fn probe_foreground_process(pid: u32, foreground_pgid: Option<u32>) -> ProcessProbeResult {
    probe_foreground_process_from_jobs(
        pid,
        foreground_pgid,
        foreground_pgid.and_then(crate::detect::foreground_group_leader_job),
        || crate::detect::foreground_job(pid),
        crate::platform::process_agent_hint,
    )
}

fn spawn_basic_detection_task(
    pane_id: PaneId,
    child_pid: Arc<AtomicU32>,
    terminal: Arc<PaneTerminal>,
    detection_content_seq: Arc<AtomicU64>,
    full_lifecycle_authority_active: Arc<AtomicBool>,
    state_events: mpsc::Sender<AppEvent>,
    detected_agent: Option<Agent>,
) -> (
    tokio::task::AbortHandle,
    Arc<Notify>,
    Arc<Mutex<Option<PendingAgentRelease>>>,
) {
    let detect_reset_notify = Arc::new(Notify::new());
    let detect_reset = detect_reset_notify.clone();
    let pending_release = Arc::new(Mutex::new(None));
    let pending_release_for_task = pending_release.clone();

    let handle = tokio::spawn(async move {
        let mut agent_presence = AgentDetectionPresence::from_agent(detected_agent);
        let mut state = AgentState::Unknown;
        let mut last_visible_idle = false;
        let mut last_visible_blocker = false;
        let mut last_visible_working = false;
        let mut last_visible_signal_refresh = None;
        let mut last_process_check = std::time::Instant::now();
        let mut last_foreground_pgid = None;
        let mut has_process_probe = false;
        let mut acquisition_started_at = None;
        let mut last_content_change_at = None;
        let mut pending_foreground_shell_clear = false;
        let mut foreground_shell_exit_reported = false;
        let mut release_was_active = false;
        let mut last_detection_text = String::new();
        let mut last_screen_scan_detection_content_seq = None;
        let mut agent_startup_grace_until = None;
        let mut pending_idle = PendingIdleConfirmation::default();

        loop {
            let sleep_duration = if pending_idle.active() {
                AGENT_PENDING_IDLE_RECHECK
            } else {
                std::time::Duration::from_millis(300)
            };
            tokio::select! {
                _ = tokio::time::sleep(sleep_duration) => {}
                _ = detect_reset.notified() => {
                    agent_presence = AgentDetectionPresence::from_agent(None);
                    state = AgentState::Unknown;
                    last_visible_idle = false;
                    last_visible_blocker = false;
                    last_visible_working = false;
                    last_visible_signal_refresh = None;
                    last_process_check = std::time::Instant::now();
                    last_foreground_pgid = None;
                    has_process_probe = false;
                    acquisition_started_at = None;
                    last_content_change_at = None;
                    pending_foreground_shell_clear = false;
                    foreground_shell_exit_reported = false;
                    release_was_active = false;
                    last_detection_text.clear();
                    last_screen_scan_detection_content_seq = None;
                    agent_startup_grace_until = None;
                    pending_idle.clear();
                }
            }

            let now = std::time::Instant::now();
            let suppressed_agent = active_pending_release(&pending_release_for_task, now);
            if suppressed_agent.is_none() && release_was_active {
                has_process_probe = false;
                acquisition_started_at = None;
                last_content_change_at = None;
            }
            release_was_active = suppressed_agent.is_some();
            let pid = child_pid.load(Ordering::Acquire);
            let mut agent_changed = false;
            let mut agent = agent_presence.current_agent();
            let lifecycle_authority_active =
                full_lifecycle_authority_active.load(Ordering::Acquire);
            let foreground_pgid = (pid > 0)
                .then(|| crate::detect::foreground_process_group_id(pid))
                .flatten();
            let process_group_changed =
                foreground_group_changed(foreground_pgid, last_foreground_pgid);
            let should_check_process = pid > 0 && {
                let process_probe_input = ProcessProbeInput {
                    current_agent: agent,
                    suppressed_agent,
                    foreground_pgid,
                    last_foreground_pgid,
                    has_process_probe,
                    acquisition_age: acquisition_started_at
                        .map(|started| now.duration_since(started)),
                    pending_foreground_shell_clear,
                    pending_restore_probe: false,
                    elapsed_since_process_check: now.duration_since(last_process_check),
                };
                !should_skip_process_probe_for_lifecycle_authority(
                    lifecycle_authority_active,
                    process_probe_input,
                ) && should_probe_foreground_job(process_probe_input)
            };

            if should_check_process {
                last_process_check = now;
                let had_process_probe = has_process_probe;
                has_process_probe = true;
                let probe = probe_foreground_process(pid, foreground_pgid);
                let process_group_id = probe.process_group_id;
                let tracked_process_group_id =
                    process_group_for_change_tracking(foreground_pgid, process_group_id);
                let foreground_is_pane_shell = probe.foreground_is_pane_shell;
                let mut new_agent = probe.agent;
                if let Some(suppressed_agent) = suppressed_agent {
                    if new_agent == Some(suppressed_agent) {
                        new_agent = None;
                    } else if let Ok(mut pending_release) = pending_release_for_task.lock() {
                        *pending_release = None;
                    }
                }
                let previous_agent = agent_presence.current_agent();
                let foreground_action = foreground_shell_agent_action(
                    previous_agent,
                    new_agent,
                    foreground_is_pane_shell,
                    foreground_shell_exit_reported,
                );
                let changed = apply_foreground_shell_agent_action(
                    &mut agent_presence,
                    foreground_action,
                    previous_agent,
                    new_agent,
                    &mut pending_foreground_shell_clear,
                    &mut foreground_shell_exit_reported,
                );
                last_foreground_pgid = tracked_process_group_id;
                if new_agent.is_some() {
                    acquisition_started_at = None;
                    last_content_change_at = None;
                } else if agent_presence.current_agent().is_none()
                    && had_process_probe
                    && process_group_changed
                {
                    acquisition_started_at = Some(now);
                }
                if changed {
                    agent = agent_presence.current_agent();
                    agent_changed = previous_agent != agent
                        || foreground_action
                            == ForegroundShellAgentAction::ReportReplacementProcess;
                    if agent_changed {
                        pending_idle.clear();
                        last_screen_scan_detection_content_seq = None;
                        // A replacement agent must not inherit OSC evidence
                        // from the previous process; a first acquisition keeps
                        // the evidence its own process already emitted.
                        clear_osc_evidence_for_agent_transition(&terminal, previous_agent);
                        if let Some(agent) = agent {
                            agent_startup_grace_until = Some(now + AGENT_STARTUP_GRACE_WINDOW);
                            state = AgentState::Unknown;
                            last_visible_idle = false;
                            last_visible_blocker = false;
                            last_visible_working = false;
                            last_visible_signal_refresh = None;
                            publish_agent_process_detected_event(
                                state_events.clone(),
                                pane_id,
                                agent,
                                now,
                            )
                            .await;
                        } else {
                            agent_startup_grace_until = None;
                        }
                    }
                }
            }

            let process_exited = pending_foreground_shell_clear
                && agent.is_some()
                && !foreground_shell_exit_reported;

            if lifecycle_authority_active && !process_exited {
                pending_idle.clear();
                continue;
            }

            if let Some(until) = agent_startup_grace_until {
                if process_exited {
                    agent_startup_grace_until = None;
                    pending_idle.clear();
                } else {
                    if now < until {
                        pending_idle.clear();
                        continue;
                    }
                    agent_startup_grace_until = None;
                    last_screen_scan_detection_content_seq = None;
                    pending_idle.clear();
                    continue;
                }
            }

            let current_detection_content_seq = if agent.is_some() {
                Some(detection_content_seq.load(Ordering::Relaxed))
            } else {
                None
            };
            match decide_detection_screen_read(DetectionScreenReadInput {
                state,
                agent,
                pending_idle_active: pending_idle.active(),
                agent_changed,
                process_exited,
                current_detection_content_seq,
                last_screen_scan_detection_content_seq,
            }) {
                DetectionScreenReadDecision::Read => {}
                DetectionScreenReadDecision::Skip => continue,
            }

            let content = terminal.detection_text();
            last_screen_scan_detection_content_seq = current_detection_content_seq;
            let content_changed = content != last_detection_text;
            last_detection_text.clone_from(&content);
            if !process_exited && crate::detect::should_skip_state_update(agent, &content) {
                pending_idle.clear();
                continue;
            }
            sync_content_change_acquisition(
                agent_presence.current_agent(),
                suppressed_agent,
                process_group_changed,
                content_changed,
                now,
                &mut acquisition_started_at,
                &mut last_content_change_at,
            );

            let osc_title = terminal.agent_osc_title();
            let osc_progress = terminal.agent_osc_progress();
            let osc_status = terminal.agent_osc_status();
            let Some(screen_detection) = detection_update_for_publish_with_osc(
                agent,
                &content,
                &osc_title,
                &osc_progress,
                &osc_status,
                process_exited,
            ) else {
                pending_idle.clear();
                continue;
            };
            match decide_screen_detection_publish(
                ScreenDetectionPublishInput {
                    screen_detection,
                    current_state: state,
                    last_visible_idle,
                    last_visible_blocker,
                    last_visible_working,
                    last_visible_signal_refresh,
                    process_exited,
                    agent_changed,
                    now,
                },
                &mut pending_idle,
            ) {
                DetectionPublishDecision::NoPublish => {}
                DetectionPublishDecision::Publish {
                    state: new_state,
                    visible_idle,
                    visible_blocker,
                    visible_working,
                    process_exited: publish_process_exited,
                } => {
                    apply_agent_detection_publish_update(
                        state_events.clone(),
                        pane_id,
                        agent,
                        AgentDetectionPublishUpdate {
                            state: new_state,
                            visible_idle,
                            visible_blocker,
                            visible_working,
                            process_exited: publish_process_exited,
                        },
                        now,
                        &mut state,
                        &mut last_visible_idle,
                        &mut last_visible_blocker,
                        &mut last_visible_working,
                        &mut last_visible_signal_refresh,
                        &mut foreground_shell_exit_reported,
                    )
                    .await;
                }
            }
        }
    });

    (handle.abort_handle(), detect_reset_notify, pending_release)
}

impl AgentDetectionPresence {
    fn from_agent(current_agent: Option<Agent>) -> Self {
        Self {
            current_agent,
            consecutive_misses: 0,
        }
    }

    fn current_agent(&self) -> Option<Agent> {
        self.current_agent
    }

    fn clear_current_agent(&mut self) -> bool {
        if self.current_agent.is_none() {
            self.consecutive_misses = 0;
            return false;
        }
        self.current_agent = None;
        self.consecutive_misses = 0;
        true
    }

    fn observe_process_probe(&mut self, identified_agent: Option<Agent>) -> bool {
        match identified_agent {
            Some(agent) => {
                self.consecutive_misses = 0;
                if Some(agent) == self.current_agent {
                    return false;
                }
                self.current_agent = Some(agent);
                true
            }
            None => {
                if self.current_agent.is_none() {
                    self.consecutive_misses = 0;
                    return false;
                }
                self.consecutive_misses = self.consecutive_misses.saturating_add(1);
                if self.consecutive_misses < AGENT_MISS_CONFIRMATION_ATTEMPTS {
                    return false;
                }
                self.current_agent = None;
                self.consecutive_misses = 0;
                true
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PaneRuntime — PTY, parser, channels, background tasks
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TerminalCompressionWake {
    notify: Arc<Notify>,
    generation: Arc<AtomicU64>,
}

impl TerminalCompressionWake {
    fn wake(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.notify.notify_one();
    }
}

/// Drives libghostty-vt's caller-owned compression after terminal activity settles.
struct TerminalCompressionTask {
    wake: TerminalCompressionWake,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    completed_passes: Arc<AtomicU64>,
    handle: tokio::task::AbortHandle,
}

impl Drop for TerminalCompressionTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl TerminalCompressionTask {
    fn spawn(pane_id: PaneId, terminal: Arc<PaneTerminal>) -> Self {
        let wake = TerminalCompressionWake {
            notify: Arc::new(Notify::new()),
            generation: Arc::new(AtomicU64::new(0)),
        };
        let task_notify = wake.notify.clone();
        let task_generation = wake.generation.clone();
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let completed_passes = Arc::new(AtomicU64::new(0));
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let task_completed_passes = completed_passes.clone();
        let handle = tokio::spawn(async move {
            run_terminal_compression_task(
                pane_id,
                terminal,
                task_notify,
                task_generation,
                #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
                task_completed_passes,
            )
            .await;
        })
        .abort_handle();
        Self {
            wake,
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            completed_passes,
            handle,
        }
    }

    fn wake(&self) {
        self.wake.wake();
    }

    fn notifier(&self) -> TerminalCompressionWake {
        self.wake.clone()
    }

    fn abort(&self) {
        self.handle.abort();
    }

    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    fn completed_passes(&self) -> u64 {
        self.completed_passes.load(Ordering::Acquire)
    }
}

async fn run_terminal_compression_task(
    pane_id: PaneId,
    terminal: Arc<PaneTerminal>,
    notify: Arc<Notify>,
    generation: Arc<AtomicU64>,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))] completed_passes: Arc<
        AtomicU64,
    >,
) {
    let mut observed_generation = generation.load(Ordering::Acquire);
    let mut activity = loop {
        match terminal.try_compression_activity() {
            Ok(Some(activity)) => break activity,
            Ok(None) => tokio::time::sleep(TERMINAL_COMPRESSION_IDLE).await,
            Err(err) => {
                warn!(pane = pane_id.raw(), err = %err, "failed to read terminal compression activity");
                return;
            }
        }
    };

    'schedule: loop {
        loop {
            tokio::time::sleep(TERMINAL_COMPRESSION_IDLE).await;
            let current = match terminal.try_compression_activity() {
                Ok(Some(current)) => current,
                Ok(None) => continue,
                Err(err) => {
                    warn!(pane = pane_id.raw(), err = %err, "failed to read terminal compression activity");
                    return;
                }
            };
            let current_generation = generation.load(Ordering::Acquire);
            if activity == current && observed_generation == current_generation {
                break;
            }
            activity = current;
            observed_generation = current_generation;
        }

        loop {
            let current_generation = generation.load(Ordering::Acquire);
            if observed_generation != current_generation {
                observed_generation = current_generation;
                continue 'schedule;
            }

            let permit = match terminal_compression_permits().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let current_generation = generation.load(Ordering::Acquire);
            if observed_generation != current_generation {
                observed_generation = current_generation;
                continue 'schedule;
            }

            let terminal_for_step = terminal.clone();
            let step = spawn_blocking_with_compression_permit(permit, move || {
                terminal_for_step.try_compress_incremental_if_activity(activity)
            })
            .await;
            let step = match step {
                Ok(Ok(step)) => step,
                Ok(Err(err)) => {
                    warn!(pane = pane_id.raw(), err = %err, "failed to compress terminal scrollback");
                    return;
                }
                Err(err) => {
                    warn!(pane = pane_id.raw(), err = %err, "terminal compression worker failed");
                    return;
                }
            };

            match step {
                TerminalCompressionStep::Busy => continue 'schedule,
                TerminalCompressionStep::ActivityChanged(current) => {
                    activity = current;
                    observed_generation = generation.load(Ordering::Acquire);
                    continue 'schedule;
                }
                TerminalCompressionStep::Compressed(
                    crate::ghostty::TerminalCompressionResult::Unsupported,
                ) => return,
                TerminalCompressionStep::Compressed(
                    crate::ghostty::TerminalCompressionResult::Pending,
                ) => tokio::time::sleep(TERMINAL_COMPRESSION_STEP).await,
                TerminalCompressionStep::Compressed(
                    crate::ghostty::TerminalCompressionResult::Complete,
                ) => {
                    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
                    completed_passes.fetch_add(1, Ordering::Release);
                    loop {
                        notify.notified().await;
                        let current_generation = generation.load(Ordering::Acquire);
                        if observed_generation != current_generation {
                            observed_generation = current_generation;
                            continue 'schedule;
                        }
                    }
                }
            }
        }
    }
}

/// PTY runtime for a pane. Owns the terminal, I/O channels, and background tasks.
/// Dropping this aborts async tasks and closes the PTY. An already-running bounded
/// compression step may finish before releasing its terminal reference.
pub struct PaneRuntime {
    pane_id: PaneId,
    terminal: Arc<PaneTerminal>,
    io: PaneRuntimeIo,
    current_size: Cell<(u16, u16, u32, u32)>,
    child_pid: Arc<AtomicU32>,
    reported_cwd: Arc<Mutex<Option<std::path::PathBuf>>>,
    persistence_cwd: Mutex<Option<std::path::PathBuf>>,
    cwd_process_exited: Arc<AtomicBool>,
    child_wait_completed: Option<Arc<AtomicBool>>,
    kitty_keyboard_flags: Arc<AtomicU16>,
    content_seq: Arc<AtomicU64>,
    content_write_lock: Arc<Mutex<()>>,
    detection_content_seq: Arc<AtomicU64>,
    full_lifecycle_authority_active: Arc<AtomicBool>,
    detect_reset_notify: Arc<Notify>,
    pending_release: Arc<Mutex<Option<PendingAgentRelease>>>,
    preserve_processes_on_drop: bool,
    // Task handles for deterministic shutdown
    compression: TerminalCompressionTask,
    detect_handle: Option<tokio::task::AbortHandle>,
}

enum PaneRuntimeIo {
    Actor(PtyIoActorHandle),
    #[cfg(test)]
    TestChannel {
        sender: mpsc::Sender<Bytes>,
        resize_tx: watch::Sender<(u16, u16, u32, u32)>,
    },
}

impl PaneRuntimeIo {
    fn shutdown(&self) {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.shutdown(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => {}
        }
    }

    fn duplicate_handoff_fd(&self) -> std::io::Result<std::os::fd::RawFd> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.duplicate_for_handoff(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => {
                Err(std::io::Error::other("test runtime has no PTY master fd"))
            }
        }
    }

    fn foreground_process_group_id(&self) -> Option<u32> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.foreground_process_group_id(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => None,
        }
    }

    fn begin_handoff(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.begin_handoff(timeout),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => Ok(()),
        }
    }

    fn set_handoff_paused(&self, paused: bool) -> std::io::Result<()> {
        match self {
            PaneRuntimeIo::Actor(actor) => {
                if paused {
                    actor.begin_handoff(std::time::Duration::from_secs(1))
                } else {
                    actor.rollback_handoff()
                }
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => Ok(()),
        }
    }

    fn release_after_commit(&self) -> std::io::Result<()> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.release_after_commit(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => Ok(()),
        }
    }

    fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        terminal_responses: Vec<Bytes>,
    ) {
        match self {
            PaneRuntimeIo::Actor(actor) => {
                actor.resize(
                    rows,
                    cols,
                    cell_width_px,
                    cell_height_px,
                    terminal_responses,
                );
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { resize_tx, .. } => {
                let _ = resize_tx.send((rows, cols, cell_width_px, cell_height_px));
            }
        }
    }

    fn nudge_child_redraw_after_handoff(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) {
        match self {
            PaneRuntimeIo::Actor(actor) => {
                actor.nudge_child_redraw_after_handoff(rows, cols, cell_width_px, cell_height_px);
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => {}
        }
    }

    fn try_send_bytes(&self, bytes: Bytes) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.try_write_user_input(bytes),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, .. } => sender.try_send(bytes),
        }
    }

    fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<std::sync::mpsc::Receiver<std::io::Result<()>>> {
        match self {
            PaneRuntimeIo::Actor(actor) => {
                #[cfg(unix)]
                {
                    let _ = deadline;
                    actor.queue_user_input_submission(text, enter, delay)
                }
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, .. } => {
                let _ = deadline;
                let sender = sender.clone();
                let (reply_tx, reply_rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result = sender
                        .try_send(text)
                        .map_err(std::io::Error::other)
                        .and_then(|()| {
                            std::thread::sleep(delay);
                            sender.try_send(enter).map_err(std::io::Error::other)
                        });
                    let _ = reply_tx.send(result);
                });
                Ok(reply_rx)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelRouting {
    HostScroll,
    MouseReport,
    AlternateScroll,
}

impl Drop for PaneRuntime {
    fn drop(&mut self) {
        // Abort detection task immediately and terminate the owned session.
        // The PTY actor shuts down before the process/session policy runs.
        if let Some(handle) = &self.detect_handle {
            handle.abort();
        }
        self.compression.abort();
        self.io.shutdown();
        if !self.preserve_processes_on_drop {
            shutdown_pane_processes(
                self.pane_id,
                self.child_pid.load(Ordering::Acquire),
                self.child_wait_completed.as_deref(),
            );
        }
    }
}

fn process_alive_for_shutdown(
    pid: u32,
    child_pid: u32,
    child_wait_completed: bool,
    process_exists: impl FnOnce(u32) -> bool,
) -> bool {
    if pid == child_pid && child_wait_completed {
        return false;
    }
    process_exists(pid)
}

fn wait_for_processes_to_exit(
    pids: &[u32],
    child_pid: u32,
    child_wait_completed: Option<&AtomicBool>,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let child_wait_completed =
            child_wait_completed.is_some_and(|flag| flag.load(Ordering::Acquire));
        if pids.iter().all(|pid| {
            !process_alive_for_shutdown(
                *pid,
                child_pid,
                child_wait_completed,
                crate::platform::process_exists,
            )
        }) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn shutdown_pane_processes(
    pane_id: PaneId,
    child_pid: u32,
    child_wait_completed: Option<&AtomicBool>,
) {
    if child_pid == 0 {
        return;
    }

    let mut pids = crate::platform::session_processes(child_pid);
    if pids.is_empty() {
        pids.push(child_pid);
    }
    pids.sort_unstable();
    pids.dedup();

    for (signal, grace) in [
        (
            crate::platform::Signal::Hangup,
            std::time::Duration::from_millis(250),
        ),
        (
            crate::platform::Signal::Terminate,
            std::time::Duration::from_millis(250),
        ),
        (
            crate::platform::Signal::Kill,
            std::time::Duration::from_millis(250),
        ),
    ] {
        crate::platform::signal_processes(&pids, signal);
        if wait_for_processes_to_exit(&pids, child_pid, child_wait_completed, grace) {
            info!(
                pane = pane_id.raw(),
                pid = child_pid,
                ?signal,
                "pane session terminated"
            );
            return;
        }
    }

    warn!(
        pane = pane_id.raw(),
        pid = child_pid,
        pids = ?pids,
        "pane session still alive after forced shutdown"
    );
}

fn truncate_handoff_history(history: String, max_bytes: usize) -> String {
    if history.len() <= max_bytes {
        return history;
    }
    let mut start = history.len().saturating_sub(max_bytes);
    while !history.is_char_boundary(start) {
        start += 1;
    }
    let Some(newline_offset) = history[start..].find('\n') else {
        return String::new();
    };
    start += newline_offset + 1;
    history[start..].to_owned()
}

fn pane_shell(configured_shell: &str) -> String {
    pane_shell_from(configured_shell, std::env::var("SHELL").ok())
}

fn pane_shell_from(configured_shell: &str, env_shell: Option<String>) -> String {
    let configured_shell = configured_shell.trim();
    if !configured_shell.is_empty() {
        return configured_shell.to_string();
    }

    env_shell
        .map(|shell| shell.trim().to_string())
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(default_pane_shell)
}

fn default_pane_shell() -> String {
    "/bin/sh".into()
}

#[derive(Clone, Copy)]
pub(crate) struct PaneShellConfig<'a> {
    pub(crate) default_shell: &'a str,
    pub(crate) mode: crate::config::ShellModeConfig,
}

impl<'a> PaneShellConfig<'a> {
    pub(crate) fn new(default_shell: &'a str, mode: crate::config::ShellModeConfig) -> Self {
        Self {
            default_shell,
            mode,
        }
    }
}

/// Target platform for shell launch policy. Parameterized (instead of raw
/// `cfg!` checks at each decision point) so every branch stays testable on
/// every host platform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ShellLaunchTarget {
    Windows,
    Macos,
    OtherUnix,
}

impl ShellLaunchTarget {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::OtherUnix
        }
    }
}

fn shell_mode_uses_login_shell(
    mode: crate::config::ShellModeConfig,
    target: ShellLaunchTarget,
) -> bool {
    match mode {
        crate::config::ShellModeConfig::Auto => target == ShellLaunchTarget::Macos,
        crate::config::ShellModeConfig::Login => true,
        crate::config::ShellModeConfig::NonLogin => false,
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn resolve_shell_for_login_mode(shell: &str) -> io::Result<String> {
    if shell.contains(std::path::MAIN_SEPARATOR) {
        let path = Path::new(shell);
        return is_executable_file(path)
            .then(|| shell.to_string())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("login shell {shell:?} is not executable"),
                )
            });
    }

    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(shell))
                .find(|candidate| is_executable_file(candidate))
        })
        .and_then(|path| path.into_os_string().into_string().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("login shell {shell:?} was not found on PATH"),
            )
        })
}

/// Sourced via `-NoExit -Command` when launching PowerShell on Windows. It
/// wraps whatever `prompt` function the user's profile left behind so each
/// prompt render appends the cwd as OSC 9;9 — the sequence Windows Terminal
/// and ConEmu standardized for shell integration. PowerShell never updates
/// its Win32 process cwd on `Set-Location`, so the prompt hook updates it when
/// possible before reporting the cwd.
///
/// The snippet must not contain double quotes: powershell.exe parses its
/// command line with its own rules that disagree with the ArgvQuote escaping
/// portable-pty applies, and embedded `\"` sequences get corrupted in
/// transit. Single-quoted strings and `[char]` codes keep the round-trip
/// byte-exact, and the OSC 9;9 payload is emitted unquoted (the original
/// ConEmu form, which the cwd tracker accepts).
///
/// The original prompt must be invoked before any other statement in the
/// wrapper: anything that runs first resets `$?`, so a status-aware user
/// prompt would show success after a failed command (verified on 5.1).
pub(crate) const WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND: &str = r"if ($null -eq $global:__SeshMuxOriginalPrompt) { $global:__SeshMuxOriginalPrompt = $function:prompt; function global:prompt { $out = @(& $global:__SeshMuxOriginalPrompt) -join ' '; $loc = $ExecutionContext.SessionState.Path.CurrentLocation; if ($loc.Provider.Name -eq 'FileSystem') { try { [Environment]::CurrentDirectory = $loc.ProviderPath } catch {}; $esc = [string][char]27; $out += $esc + ']9;9;' + $loc.ProviderPath + $esc + '\' }; $out } }";

/// Login flags appended when launching POSIX-style shells on Windows.
///
/// `portable-pty`'s default-program path resolves to `%ComSpec%` (cmd.exe) on
/// Windows and ignores the `SHELL` env override, and the Unix argv0-prefix
/// login convention does not exist there. A login shell therefore has to be
/// launched directly with the shell's own login flag. Only POSIX-style shells
/// that define one (the sh family, fish, and csh/tcsh) get it; Windows-native
/// shells such as cmd.exe and PowerShell have no login concept and launch
/// plain.
fn windows_login_shell_args(shell: &str) -> &'static [&'static str] {
    let name = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match name {
        "sh" | "bash" | "zsh" | "ksh" | "dash" | "ash" | "mksh" | "fish" | "csh" | "tcsh" => {
            &["-l"]
        }
        _ => &[],
    }
}

/// The pane's shell running `command` once its startup files have run, rather than typed into
/// it while they still run and may swallow the input.
pub(crate) fn pane_shell_command_argv(shell_config: PaneShellConfig<'_>, command: &str) -> Vec<String> {
    let mut argv = vec![pane_shell(shell_config.default_shell)];
    if shell_mode_uses_login_shell(shell_config.mode, ShellLaunchTarget::current()) {
        argv.push("-l".into());
    }
    argv.extend(["-i".into(), "-c".into(), command.into()]);
    argv
}

fn pane_shell_command_builder_for_target(
    shell_config: PaneShellConfig<'_>,
    target: ShellLaunchTarget,
) -> io::Result<CommandBuilder> {
    let shell = pane_shell(shell_config.default_shell);
    // Unix login shells go through portable-pty's default-program builder so
    // they receive the login argv0 convention. Windows has no such convention
    // and the default-program builder would launch cmd.exe, so Windows login
    // shells are built directly below like every other direct-shell target.
    if shell_mode_uses_login_shell(shell_config.mode, target)
        && target != ShellLaunchTarget::Windows
    {
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.env("SHELL", resolve_shell_for_login_mode(&shell)?);
        return Ok(cmd);
    }

    let mut cmd = CommandBuilder::new(&shell);
    if shell_mode_uses_login_shell(shell_config.mode, target) {
        cmd.args(windows_login_shell_args(&shell));
    }
    if uses_windows_powershell_pane_shell_for_target(shell_config, target) {
        cmd.args([
            "-NoExit",
            "-Command",
            WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND,
        ]);
    }
    Ok(cmd)
}

fn pane_shell_command_builder(shell_config: PaneShellConfig<'_>) -> io::Result<CommandBuilder> {
    pane_shell_command_builder_for_target(shell_config, ShellLaunchTarget::current())
}

/// True when panes launch an interactive PowerShell directly on Windows.
/// Gates the prompt-based cwd reporting pipeline and the agent-exit shell
/// respawn recovery.
pub(crate) fn uses_windows_powershell_pane_shell(shell_config: PaneShellConfig<'_>) -> bool {
    uses_windows_powershell_pane_shell_for_target(shell_config, ShellLaunchTarget::current())
}

fn uses_windows_powershell_pane_shell_for_target(
    shell_config: PaneShellConfig<'_>,
    target: ShellLaunchTarget,
) -> bool {
    // Login mode no longer routes Windows through the default-program builder,
    // so login PowerShell panes are launched directly and can carry the same
    // prompt-based cwd reporting as non-login panes.
    target == ShellLaunchTarget::Windows
        && is_powershell_shell(&pane_shell(shell_config.default_shell))
}

fn is_powershell_shell(shell: &str) -> bool {
    // Split on both separators by hand: `Path::file_name` only treats `\` as
    // a separator on Windows hosts, and this predicate must evaluate Windows
    // shell paths correctly from tests on any host.
    let name = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    )
}

fn usable_reported_cwd(cwd: std::path::PathBuf) -> Option<std::path::PathBuf> {
    (cwd.is_absolute() && cwd.is_dir()).then_some(cwd)
}

fn publish_terminal_bells(pane_id: PaneId, count: u16, events: &mpsc::Sender<AppEvent>) {
    if count == 0 {
        return;
    }
    if let Err(err) = events.try_send(AppEvent::TerminalBell { pane_id, count }) {
        warn!(
            pane = pane_id.raw(),
            count,
            err = %err,
            "failed to queue terminal bell"
        );
    }
}

fn publish_reported_cwd(
    pane_id: PaneId,
    cwd: std::path::PathBuf,
    reported_cwd: &Arc<Mutex<Option<std::path::PathBuf>>>,
    events: &mpsc::Sender<AppEvent>,
) {
    let Some(cwd) = usable_reported_cwd(cwd) else {
        return;
    };
    if let Ok(mut current) = reported_cwd.lock() {
        if current.as_ref() == Some(&cwd) {
            return;
        }
        *current = Some(cwd.clone());
    }
    if let Err(err) = events.try_send(AppEvent::TerminalCwdReported { pane_id, cwd }) {
        warn!(
            pane = pane_id.raw(),
            err = %err,
            "failed to send terminal cwd report"
        );
    }
}

impl PaneRuntime {
    pub fn shutdown(mut self) {
        if let Some(handle) = self.detect_handle.take() {
            handle.abort();
        }
        self.compression.abort();
        self.io.shutdown();
        shutdown_pane_processes(
            self.pane_id,
            self.child_pid.load(Ordering::Acquire),
            self.child_wait_completed.as_deref(),
        );
        self.preserve_processes_on_drop = true;
    }

    pub fn duplicate_handoff_fd(&self) -> std::io::Result<std::os::fd::RawFd> {
        self.io.duplicate_handoff_fd()
    }

    pub fn preserve_for_handoff(mut self) {
        if let Err(err) = self.io.release_after_commit() {
            warn!(
                pane = self.pane_id.raw(),
                err = %err,
                "failed to release PTY actor after handoff commit; dropping runtime will still close the actor handle"
            );
        }
        if let Some(handle) = self.detect_handle.take() {
            handle.abort();
        }
        self.compression.abort();
        self.preserve_processes_on_drop = true;
    }

    pub fn assume_handoff_ownership(&mut self) {
        self.preserve_processes_on_drop = false;
    }

    pub fn set_handoff_reader_paused(&self, paused: bool) {
        if let Err(err) = self.io.set_handoff_paused(paused) {
            warn!(
                pane = self.pane_id.raw(),
                err = %err,
                paused,
                "failed to update PTY actor handoff pause state"
            );
        }
    }

    pub fn pause_handoff_reader(&self, timeout: std::time::Duration) -> std::io::Result<()> {
        self.io.begin_handoff(timeout)
    }

    pub fn handoff_runtime_state(
        &self,
        pane_id: u32,
    ) -> crate::handoff_runtime::HandoffRuntimeState {
        let child_pid = self.child_pid.load(Ordering::Acquire);
        let (rows, cols, cell_width_px, cell_height_px) = self.current_size.get();
        crate::handoff_runtime::HandoffRuntimeState {
            pane_id,
            child_pid,
            rows,
            cols,
            cell_width_px,
            cell_height_px,
            keyboard_protocol_flags: match self.keyboard_protocol() {
                crate::input::KeyboardProtocol::Legacy => 0,
                crate::input::KeyboardProtocol::Kitty { flags } => flags,
            },
            keyboard_protocol_ansi: self.terminal.kitty_keyboard_state_ansi(),
            input_state: self.input_state(),
            terminal_title: self.terminal_title(),
            initial_history_ansi: None,
        }
    }

    pub fn handoff_history_ansi(&self) -> Option<String> {
        if self.terminal.alternate_screen_active() {
            return None;
        }
        self.snapshot_history().map(|history| {
            truncate_handoff_history(history, crate::server::handoff::MAX_REPLAY_BYTES_PER_PANE)
        })
    }

    // Runtime construction threads PTY geometry, host context, launch policy, and render hooks.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: PaneShellConfig<'_>,
        launch_env: &PaneLaunchEnv,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        Self::spawn_with_initial_history(
            pane_id,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            launch_env,
            None,
            events,
            render_notify,
            render_dirty,
        )
    }

    // Runtime construction needs to thread PTY size, environment, theme, and render hooks together.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_with_initial_history(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        shell_config: PaneShellConfig<'_>,
        launch_env: &PaneLaunchEnv,
        initial_history_ansi: Option<&str>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        let windows_powershell_prompt_cwd_reporting =
            uses_windows_powershell_pane_shell(shell_config);
        let mut cmd = pane_shell_command_builder(shell_config)?;
        cmd.cwd(crate::platform::normalize_cwd_for_launch(&cwd));
        apply_pane_terminal_env(&mut cmd);
        apply_pane_launch_env(&mut cmd, launch_env);
        Self::spawn_command_builder(
            pane_id,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
            cmd,
            "failed to spawn shell",
            SpawnInitialState {
                detected_agent: None,
                history_ansi: initial_history_ansi,
                windows_powershell_prompt_cwd_reporting,
            },
            AgentDetection::Enabled,
        )
    }

    // Runtime construction needs to thread PTY size, environment, theme, and render hooks together.

    // Runtime construction needs to thread PTY size, environment, theme, render hooks, and detection policy together.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_argv_command(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        argv: &[String],
        launch_env: &PaneLaunchEnv,
        agent_detection: AgentDetection,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        let Some((program, args)) = argv.split_first() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "argv must not be empty",
            ));
        };
        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(crate::platform::normalize_cwd_for_launch(&cwd));
        apply_pane_terminal_env(&mut cmd);
        apply_pane_launch_env(&mut cmd, launch_env);
        Self::spawn_command_builder(
            pane_id,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
            cmd,
            "failed to spawn argv command pane",
            SpawnInitialState::default(),
            agent_detection,
        )
    }

    pub fn from_handoff_fd(
        import: crate::handoff_runtime::ImportedHandoffRuntime,
        detected_agent: Option<Agent>,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        let crate::handoff_runtime::ImportedHandoffRuntime { master_fd, state } = import;
        let crate::handoff_runtime::HandoffRuntimeState {
            pane_id,
            child_pid,
            rows,
            cols,
            cell_width_px,
            cell_height_px,
            keyboard_protocol_flags,
            keyboard_protocol_ansi,
            input_state,
            terminal_title,
            initial_history_ansi,
        } = state;
        let pane_id = PaneId::from_raw(pane_id);
        use std::os::fd::FromRawFd;

        let master_fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(master_fd) };

        let (response_tx, _response_rx) = mpsc::channel::<Bytes>(1);
        let mut terminal = crate::ghostty::Terminal::new(cols, rows, scrollback_limit_bytes)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        terminal
            .resize(cols, rows, cell_width_px, cell_height_px)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if false {
            terminal
                .enable_kitty_graphics()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        let pane_terminal = GhosttyPaneTerminal::new(terminal, response_tx.clone())?;
        pane_terminal.apply_host_terminal_theme(host_terminal_theme);
        let _ = pane_terminal.apply_host_terminal_appearance(host_terminal_appearance);
        pane_terminal.seed_terminal_title(terminal_title);
        if let Some(input_state) = input_state {
            pane_terminal.seed_handoff_input_state(input_state);
        }
        if let Some(ansi) = keyboard_protocol_ansi.as_deref() {
            pane_terminal.seed_keyboard_protocol_ansi(ansi);
        } else {
            pane_terminal.seed_keyboard_protocol_flags(keyboard_protocol_flags);
        }
        if let Some(ansi) = initial_history_ansi.as_deref() {
            pane_terminal.seed_history_ansi(ansi);
        }
        let terminal = Arc::new(PaneTerminal::new(pane_terminal));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        let child_pid = Arc::new(AtomicU32::new(child_pid));
        let reported_cwd = Arc::new(Mutex::new(None));
        let cwd_process_exited = Arc::new(AtomicBool::new(false));
        let kitty_keyboard_flags = Arc::new(AtomicU16::new(keyboard_protocol_flags));
        let content_seq = Arc::new(AtomicU64::new(0));
        let content_write_lock = Arc::new(Mutex::new(()));
        let detection_content_seq = Arc::new(AtomicU64::new(0));

        let io = {
            let terminal = terminal.clone();
            let response_writer = response_tx.clone();
            let render_notify = render_notify.clone();
            let render_dirty = render_dirty.clone();
            let content_seq = content_seq.clone();
            let content_write_lock = content_write_lock.clone();
            let detection_content_seq = detection_content_seq.clone();
            let child_pid = child_pid.clone();
            let read_events = events.clone();
            let reported_cwd = reported_cwd.clone();
            let compression_wake = compression.notifier();
            let rt = tokio::runtime::Handle::current();
            let delay_rt = rt.clone();
            let on_read = Box::new(move |bytes: &[u8]| {
                let _content_write_guard = match content_write_lock.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                content_seq.fetch_add(1, Ordering::AcqRel);
                let shell_pid = child_pid.load(Ordering::Acquire);
                let result =
                    terminal.process_pty_bytes(pane_id, shell_pid, bytes, &response_writer);
                content_seq.fetch_add(1, Ordering::Release);
                drop(_content_write_guard);
                compression_wake.wake();
                publish_terminal_bells(pane_id, result.terminal_bells, &read_events);
                observe_detection_content_change(bytes, &detection_content_seq);
                let title_requested =
                    result.terminal_title_changed && render_dirty.request_terminal_title(pane_id);
                let render_requested = result.request_render && render_dirty.request_pty(pane_id);
                if title_requested || render_requested {
                    render_notify.notify_one();
                }
                if let Some(delay) = result.render_delay {
                    let render_notify = render_notify.clone();
                    let render_dirty = render_dirty.clone();
                    delay_rt.spawn(async move {
                        tokio::time::sleep(delay).await;
                        if render_dirty.request_pty(pane_id) {
                            render_notify.notify_one();
                        }
                    });
                }
                if let Some(cwd) = result.reported_cwd.clone() {
                    publish_reported_cwd(pane_id, cwd, &reported_cwd, &read_events);
                }
                for content in result.clipboard_writes {
                    if let Err(err) =
                        read_events.try_send(AppEvent::ClipboardWrite { pane_id, content })
                    {
                        warn!(
                            pane = pane_id.raw(),
                            err = %err,
                            "failed to queue OSC 52 clipboard write"
                        );
                    }
                }
                PtyReadResult {
                    terminal_responses: result.terminal_responses,
                }
            });
            let exit_events = events.clone();
            let cwd_process_exited = cwd_process_exited.clone();
            let on_reader_exit = Box::new(move || {
                cwd_process_exited.store(true, Ordering::Release);
                // Imported handoff panes have no child wait handle, so their exit cause is
                // unknowable. Checkpoint conservatively; normal autosave settles clean exits.
                let _ = rt.block_on(exit_events.send(AppEvent::PaneDied {
                    pane_id,
                    exit_reason: crate::platform::ChildExitReason::Handoff,
                }));
                debug!(pane = pane_id.raw(), "handoff PTY actor exiting");
            });
            PaneRuntimeIo::Actor(PtyIoActor::spawn(PtyIoActorConfig {
                pane_id: pane_id.raw(),
                master_fd,
                initially_quiesced: true,
                on_read,
                on_reader_exit: Some(on_reader_exit),
            })?)
        };

        let full_lifecycle_authority_active = Arc::new(AtomicBool::new(false));
        let (detect_handle, detect_reset_notify, pending_release) = spawn_basic_detection_task(
            pane_id,
            child_pid.clone(),
            terminal.clone(),
            detection_content_seq.clone(),
            full_lifecycle_authority_active.clone(),
            events,
            detected_agent,
        );

        Ok(Self {
            pane_id,
            terminal,
            io,
            current_size: Cell::new((rows, cols, cell_width_px, cell_height_px)),
            child_pid,
            reported_cwd,
            persistence_cwd: Mutex::new(None),
            cwd_process_exited,
            child_wait_completed: None,
            kitty_keyboard_flags,
            content_seq,
            content_write_lock,
            detection_content_seq,
            full_lifecycle_authority_active,
            detect_reset_notify,
            pending_release,
            preserve_processes_on_drop: true,
            compression,
            detect_handle: Some(detect_handle),
        })
    }

    // Runtime construction needs to thread PTY size, environment, theme, render hooks, and detection policy together.
    #[allow(clippy::too_many_arguments)]
    fn spawn_command_builder(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
        cmd: CommandBuilder,
        spawn_error_message: &'static str,
        initial_state: SpawnInitialState<'_>,
        agent_detection: AgentDetection,
    ) -> std::io::Result<Self> {
        crate::logging::pane_spawn_started(pane_id.raw(), rows, cols, scrollback_limit_bytes);

        let (response_tx, _response_rx) = mpsc::channel::<Bytes>(1);
        let mut terminal = crate::ghostty::Terminal::new(cols, rows, scrollback_limit_bytes)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if false {
            terminal
                .enable_kitty_graphics()
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        let pane_terminal = GhosttyPaneTerminal::new(terminal, response_tx.clone())?;
        pane_terminal.apply_host_terminal_theme(host_terminal_theme);
        let _ = pane_terminal.apply_host_terminal_appearance(host_terminal_appearance);
        pane_terminal.set_windows_powershell_prompt_cwd_reporting(
            initial_state.windows_powershell_prompt_cwd_reporting,
        );
        if let Some(ansi) = initial_state.history_ansi {
            pane_terminal.seed_history_ansi(ansi);
        }
        let terminal = Arc::new(PaneTerminal::new(pane_terminal));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        let kitty_keyboard_flags = Arc::new(AtomicU16::new(0));
        let content_write_lock = Arc::new(Mutex::new(()));

        let spawned = crate::pty::backend::spawn_with_portable_pty(rows, cols, cmd)
            .inspect_err(|err| error!(pane = pane_id.raw(), err = %err, "{spawn_error_message}"))?;

        // --- Child watcher task ---
        let child_pid = Arc::new(AtomicU32::new(0));
        let reported_cwd = Arc::new(Mutex::new(None));
        let child_wait_completed = Arc::new(AtomicBool::new(false));
        let content_seq = Arc::new(AtomicU64::new(0));
        let detection_content_seq = Arc::new(AtomicU64::new(0));
        let full_lifecycle_authority_active = Arc::new(AtomicBool::new(false));
        {
            let child_pid = child_pid.clone();
            let child_wait_completed = child_wait_completed.clone();
            let events = events.clone();
            let rt = tokio::runtime::Handle::current();
            let mut child = spawned.child;
            if let Some(pid) = child.process_id() {
                child_pid.store(pid, Ordering::Release);
                crate::logging::pane_spawned(pane_id.raw(), pid);
            }
            tokio::task::spawn_blocking(move || {
                let exit_reason = match child.wait() {
                    Ok(status) => {
                        let exit_reason = crate::platform::classify_child_exit(&status);
                        let status_text = format!("{status:?}");
                        crate::logging::pane_exited(pane_id.raw(), &status_text);
                        exit_reason
                    }
                    Err(e) => {
                        crate::logging::pane_exit_failed(pane_id.raw(), &e.to_string());
                        crate::platform::ChildExitReason::WaitFailed
                    }
                };
                child_wait_completed.store(true, Ordering::Release);
                // Use blocking send — PaneDied is critical, must not be dropped
                if let Err(e) = rt.block_on(events.send(AppEvent::PaneDied {
                    pane_id,
                    exit_reason,
                })) {
                    error!(pane = pane_id.raw(), err = %e, "failed to send PaneDied event");
                }
            });
        }

        let io = {
            let terminal = terminal.clone();
            let response_writer = response_tx.clone();
            let render_notify = render_notify.clone();
            let render_dirty = render_dirty.clone();
            let content_seq = content_seq.clone();
            let content_write_lock = content_write_lock.clone();
            let detection_content_seq = detection_content_seq.clone();
            let child_pid = child_pid.clone();
            let events = events.clone();
            let reported_cwd = reported_cwd.clone();
            let compression_wake = compression.notifier();
            let rt = tokio::runtime::Handle::current();
            let on_read = Box::new(move |bytes: &[u8]| {
                let _content_write_guard = match content_write_lock.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                content_seq.fetch_add(1, Ordering::AcqRel);
                let shell_pid = child_pid.load(Ordering::Acquire);
                let result =
                    terminal.process_pty_bytes(pane_id, shell_pid, bytes, &response_writer);
                content_seq.fetch_add(1, Ordering::Release);
                drop(_content_write_guard);
                compression_wake.wake();
                publish_terminal_bells(pane_id, result.terminal_bells, &events);
                if agent_detection == AgentDetection::Enabled {
                    observe_detection_content_change(bytes, &detection_content_seq);
                }
                let title_requested =
                    result.terminal_title_changed && render_dirty.request_terminal_title(pane_id);
                let render_requested = result.request_render && render_dirty.request_pty(pane_id);
                if title_requested || render_requested {
                    render_notify.notify_one();
                }
                if let Some(delay) = result.render_delay {
                    let render_notify = render_notify.clone();
                    let render_dirty = render_dirty.clone();
                    rt.spawn(async move {
                        tokio::time::sleep(delay).await;
                        if render_dirty.request_pty(pane_id) {
                            render_notify.notify_one();
                        }
                    });
                }
                if let Some(cwd) = result.reported_cwd.clone() {
                    publish_reported_cwd(pane_id, cwd, &reported_cwd, &events);
                }
                for content in result.clipboard_writes {
                    if let Err(err) = events.try_send(AppEvent::ClipboardWrite { pane_id, content })
                    {
                        warn!(
                            pane = pane_id.raw(),
                            err = %err,
                            "failed to send OSC 52 clipboard write"
                        );
                    }
                }
                PtyReadResult {
                    terminal_responses: result.terminal_responses,
                }
            });
            PaneRuntimeIo::Actor(PtyIoActor::spawn(PtyIoActorConfig {
                pane_id: pane_id.raw(),
                master_fd: spawned.master_fd,
                initially_quiesced: false,
                on_read,
                on_reader_exit: None,
            })?)
        };

        // --- Detection task ---
        let (detect_handle, detect_reset_notify, pending_release) = if agent_detection
            == AgentDetection::Enabled
        {
            use crate::detect;
            use std::time::{Duration, Instant};

            const TICK_UNIDENTIFIED: Duration = Duration::from_millis(500);
            const TICK_IDENTIFIED: Duration = Duration::from_millis(300);
            const TICK_PENDING_RELEASE: Duration = Duration::from_millis(50);

            let child_pid = child_pid.clone();
            let terminal = terminal.clone();
            let state_events = events.clone();
            let detection_content_seq = detection_content_seq.clone();
            let full_lifecycle_authority_active_for_task = full_lifecycle_authority_active.clone();
            let render_notify = render_notify.clone();
            let render_dirty = render_dirty.clone();
            let detect_reset_notify = Arc::new(Notify::new());
            let detect_reset = detect_reset_notify.clone();
            let pending_release = Arc::new(Mutex::new(None));
            let pending_release_for_task = pending_release.clone();

            let handle = tokio::spawn(async move {
                let mut agent_presence =
                    AgentDetectionPresence::from_agent(initial_state.detected_agent);
                let mut state = AgentState::Idle;
                let mut last_visible_idle = initial_state.detected_agent.is_some();
                let mut last_process_check = Instant::now();
                #[cfg(windows)]
                let mut last_observation = (Instant::now(), Some(0));
                let mut last_foreground_pgid = None;
                let mut has_process_probe = false;
                let mut acquisition_started_at = None;
                let mut last_content_change_at = None;
                let mut pending_foreground_shell_clear = false;
                let mut foreground_shell_exit_reported = false;
                let mut release_was_active = false;
                let mut pending_restore_probe = initial_state.detected_agent.is_some();
                let mut last_visible_blocker = false;
                let mut last_visible_working = false;
                let mut last_visible_signal_refresh = None;
                let mut last_detection_text = String::new();
                let mut last_screen_scan_detection_content_seq = None;
                let mut agent_startup_grace_until = None;
                let mut pending_idle = PendingIdleConfirmation::default();

                tokio::time::sleep(Duration::from_millis(50)).await;

                loop {
                    let now_for_tick = Instant::now();
                    let tick = if active_pending_release(&pending_release_for_task, now_for_tick)
                        .is_some()
                        || terminal.has_transient_default_color_override()
                    {
                        TICK_PENDING_RELEASE
                    } else if pending_idle.active() {
                        AGENT_PENDING_IDLE_RECHECK
                    } else if agent_presence.current_agent().is_none() {
                        TICK_UNIDENTIFIED
                    } else {
                        TICK_IDENTIFIED
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(tick) => {}
                        _ = detect_reset.notified() => {
                            agent_presence = AgentDetectionPresence::from_agent(None);
                            state = AgentState::Unknown;
                            last_visible_idle = false;
                            last_foreground_pgid = None;
                            has_process_probe = false;
                            acquisition_started_at = None;
                            last_content_change_at = None;
                            pending_foreground_shell_clear = false;
                            foreground_shell_exit_reported = false;
                            release_was_active = false;
                            pending_restore_probe = false;
                            last_visible_blocker = false;
                            last_visible_working = false;
                            last_visible_signal_refresh = None;
                            last_detection_text.clear();
                            last_screen_scan_detection_content_seq = None;
                            agent_startup_grace_until = None;
                            pending_idle.clear();
                        }
                    }

                    let now = Instant::now();
                    let suppressed_agent = active_pending_release(&pending_release_for_task, now);
                    if suppressed_agent.is_none() && release_was_active {
                        has_process_probe = false;
                        acquisition_started_at = None;
                        last_content_change_at = None;
                    }
                    release_was_active = suppressed_agent.is_some();
                    let pid = child_pid.load(Ordering::Acquire);
                    let mut agent = agent_presence.current_agent();
                    let lifecycle_authority_active =
                        full_lifecycle_authority_active_for_task.load(Ordering::Acquire);
                    let process_probe_input = ProcessProbeInput {
                        current_agent: agent,
                        suppressed_agent,
                        foreground_pgid: last_foreground_pgid,
                        last_foreground_pgid,
                        has_process_probe,
                        acquisition_age: acquisition_started_at
                            .map(|started| now.duration_since(started)),
                        pending_foreground_shell_clear,
                        pending_restore_probe,
                        elapsed_since_process_check: now.duration_since(last_process_check),
                    };
                    #[cfg(windows)]
                    let last_content_seq = last_observation.1;
                    #[cfg(windows)]
                    let foreground_observation_due = should_observe_foreground_process_group(
                        lifecycle_authority_active,
                        last_content_seq != Some(content_seq)
                            && (last_content_seq.is_some()
                                || now.duration_since(last_observation.0) >= TICK_IDENTIFIED),
                        now.duration_since(last_observation.0),
                        process_probe_input,
                    );
                    #[cfg(not(windows))]
                    let foreground_observation_due = true;
                    let foreground_pgid = match (pid, foreground_observation_due) {
                        (0, _) => None,
                        (_, true) => detect::foreground_process_group_id(pid),
                        _ => last_foreground_pgid,
                    };
                    let process_group_changed =
                        foreground_group_changed(foreground_pgid, last_foreground_pgid);
                    let should_check_process = pid > 0 && {
                        let process_probe_input = ProcessProbeInput {
                            foreground_pgid,
                            ..process_probe_input
                        };
                        !should_skip_process_probe_for_lifecycle_authority(
                            lifecycle_authority_active,
                            process_probe_input,
                        ) && should_probe_foreground_job(process_probe_input)
                    };

                    let mut agent_changed = false;
                    if should_check_process {
                        last_process_check = now;
                        let had_process_probe = has_process_probe;
                        has_process_probe = true;
                        if pid > 0 {
                            let probe = probe_foreground_process(pid, foreground_pgid);
                            let process_name = probe.process_name;
                            let process_group_id = probe.process_group_id;
                            let tracked_process_group_id = process_group_for_change_tracking(
                                foreground_pgid,
                                process_group_id,
                            );
                            let foreground_is_pane_shell = probe.foreground_is_pane_shell;
                            let mut new_agent = probe.agent;

                            if let Some(suppressed_agent) = suppressed_agent {
                                if new_agent == Some(suppressed_agent) {
                                    new_agent = None;
                                } else if let Ok(mut pending_release) =
                                    pending_release_for_task.lock()
                                {
                                    *pending_release = None;
                                }
                            }

                            let previous_agent = agent_presence.current_agent();
                            let foreground_action = foreground_shell_agent_action(
                                previous_agent,
                                new_agent,
                                foreground_is_pane_shell,
                                foreground_shell_exit_reported,
                            );
                            let changed = apply_foreground_shell_agent_action(
                                &mut agent_presence,
                                foreground_action,
                                previous_agent,
                                new_agent,
                                &mut pending_foreground_shell_clear,
                                &mut foreground_shell_exit_reported,
                            );
                            last_foreground_pgid = tracked_process_group_id;
                            if new_agent.is_some() {
                                acquisition_started_at = None;
                                last_content_change_at = None;
                            } else if agent_presence.current_agent().is_none()
                                && had_process_probe
                                && process_group_changed
                            {
                                acquisition_started_at = Some(now);
                            }
                            pending_restore_probe = false;
                            if changed {
                                agent = agent_presence.current_agent();
                                if agent != previous_agent
                                    || foreground_action
                                        == ForegroundShellAgentAction::ReportReplacementProcess
                                {
                                    pending_idle.clear();
                                    last_screen_scan_detection_content_seq = None;
                                    // A replacement agent must not inherit OSC
                                    // evidence from the previous process; a first
                                    // acquisition keeps the evidence its own
                                    // process already emitted.
                                    clear_osc_evidence_for_agent_transition(
                                        &terminal,
                                        previous_agent,
                                    );
                                    if let Some(agent) = agent {
                                        agent_startup_grace_until =
                                            Some(now + AGENT_STARTUP_GRACE_WINDOW);
                                        state = AgentState::Unknown;
                                        last_visible_idle = false;
                                        last_visible_blocker = false;
                                        last_visible_working = false;
                                        last_visible_signal_refresh = None;
                                        publish_agent_process_detected_event(
                                            state_events.clone(),
                                            pane_id,
                                            agent,
                                            now,
                                        )
                                        .await;
                                    } else {
                                        agent_startup_grace_until = None;
                                    }
                                }
                                if let Some(process_name) = process_name {
                                    info!(
                                        pane = pane_id.raw(),
                                        previous_agent = ?previous_agent,
                                        ?agent,
                                        process = %process_name,
                                        pgid = ?process_group_id,
                                        "agent changed"
                                    );
                                } else {
                                    info!(
                                        pane = pane_id.raw(),
                                        previous_agent = ?previous_agent,
                                        ?agent,
                                        pgid = ?process_group_id,
                                        "agent changed"
                                    );
                                }
                                agent_changed = true;
                            }
                        }
                    }

                    let pid = child_pid.load(Ordering::Acquire);
                    // Keep the terminal restore side effect separate from render notification state.
                    #[allow(clippy::collapsible_if)]
                    if pid > 0 && terminal.maybe_restore_host_terminal_theme(pane_id, pid) {
                        if render_dirty.request_pty(pane_id) {
                            render_notify.notify_one();
                        }
                    }

                    let process_exited = pending_foreground_shell_clear
                        && agent.is_some()
                        && !foreground_shell_exit_reported;

                    if lifecycle_authority_active && !process_exited {
                        pending_idle.clear();
                        continue;
                    }

                    if let Some(until) = agent_startup_grace_until {
                        if process_exited {
                            agent_startup_grace_until = None;
                            last_screen_scan_detection_content_seq = None;
                            pending_idle.clear();
                        } else {
                            if now < until {
                                pending_idle.clear();
                                continue;
                            }
                            agent_startup_grace_until = None;
                            pending_idle.clear();
                            continue;
                        }
                    }

                    let current_detection_content_seq = if agent.is_some() {
                        Some(detection_content_seq.load(Ordering::Relaxed))
                    } else {
                        None
                    };
                    match decide_detection_screen_read(DetectionScreenReadInput {
                        state,
                        agent,
                        pending_idle_active: pending_idle.active(),
                        agent_changed,
                        process_exited,
                        current_detection_content_seq,
                        last_screen_scan_detection_content_seq,
                    }) {
                        DetectionScreenReadDecision::Read => {}
                        DetectionScreenReadDecision::Skip => continue,
                    }

                    let content = terminal.detection_text();
                    last_screen_scan_detection_content_seq = current_detection_content_seq;
                    let content_changed = content != last_detection_text;
                    last_detection_text.clone_from(&content);
                    if detect::should_skip_state_update(agent, &content) {
                        pending_idle.clear();
                        continue;
                    }
                    sync_content_change_acquisition(
                        agent_presence.current_agent(),
                        suppressed_agent,
                        process_group_changed,
                        content_changed,
                        now,
                        &mut acquisition_started_at,
                        &mut last_content_change_at,
                    );

                    let osc_title = terminal.agent_osc_title();
                    let osc_progress = terminal.agent_osc_progress();
                    let osc_status = terminal.agent_osc_status();
                    let Some(screen_detection) = detection_update_for_publish_with_osc(
                        agent,
                        &content,
                        &osc_title,
                        &osc_progress,
                        &osc_status,
                        process_exited,
                    ) else {
                        pending_idle.clear();
                        continue;
                    };
                    match decide_screen_detection_publish(
                        ScreenDetectionPublishInput {
                            screen_detection,
                            current_state: state,
                            last_visible_idle,
                            last_visible_blocker,
                            last_visible_working,
                            last_visible_signal_refresh,
                            process_exited,
                            agent_changed,
                            now,
                        },
                        &mut pending_idle,
                    ) {
                        DetectionPublishDecision::NoPublish => {}
                        DetectionPublishDecision::Publish {
                            state: new_state,
                            visible_idle,
                            visible_blocker,
                            visible_working,
                            process_exited: publish_process_exited,
                        } => {
                            apply_agent_detection_publish_update(
                                state_events.clone(),
                                pane_id,
                                agent,
                                AgentDetectionPublishUpdate {
                                    state: new_state,
                                    visible_idle,
                                    visible_blocker,
                                    visible_working,
                                    process_exited: publish_process_exited,
                                },
                                now,
                                &mut state,
                                &mut last_visible_idle,
                                &mut last_visible_blocker,
                                &mut last_visible_working,
                                &mut last_visible_signal_refresh,
                                &mut foreground_shell_exit_reported,
                            )
                            .await;
                        }
                    }
                }
            });
            (
                Some(handle.abort_handle()),
                detect_reset_notify,
                pending_release,
            )
        } else {
            (None, Arc::new(Notify::new()), Arc::new(Mutex::new(None)))
        };

        Ok(Self {
            pane_id,
            terminal,
            io,
            current_size: Cell::new((rows, cols, 0, 0)),
            child_pid,
            reported_cwd,
            persistence_cwd: Mutex::new(None),
            cwd_process_exited: child_wait_completed.clone(),
            child_wait_completed: Some(child_wait_completed),
            kitty_keyboard_flags,
            content_seq,
            content_write_lock,
            detection_content_seq,
            full_lifecycle_authority_active,
            detect_reset_notify,
            pending_release,
            preserve_processes_on_drop: false,
            compression,
            detect_handle,
        })
    }

    pub fn begin_graceful_release(&self, agent: Agent) {
        if let Ok(mut pending_release) = self.pending_release.lock() {
            *pending_release = Some(PendingAgentRelease {
                agent,
                until: std::time::Instant::now() + RELEASE_REACQUIRE_SUPPRESSION,
            });
        }
        self.detect_reset_notify.notify_one();
    }

    pub fn set_full_lifecycle_authority_active(&self, active: bool) {
        let previous = self
            .full_lifecycle_authority_active
            .swap(active, Ordering::AcqRel);
        if active && !previous {
            self.detect_reset_notify.notify_one();
        }
    }

    pub(crate) fn current_size(&self) -> (u16, u16) {
        let (rows, cols, _, _) = self.current_size.get();
        (rows, cols)
    }

    pub(crate) fn content_seq(&self) -> u64 {
        self.content_seq.load(Ordering::Acquire)
    }

    /// Resize if the dimensions actually changed.
    pub fn resize(&self, rows: u16, cols: u16, cell_width_px: u32, cell_height_px: u32) {
        let rows = rows.max(2);
        let cols = cols.max(4);
        let size = (rows, cols, cell_width_px, cell_height_px);
        if self.current_size.get() == size {
            return;
        }
        self.current_size.set(size);
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.content_seq.fetch_add(1, Ordering::AcqRel);
        let terminal_responses = self
            .terminal
            .resize(rows, cols, cell_width_px, cell_height_px);
        self.content_seq.fetch_add(1, Ordering::Release);
        drop(_content_write_guard);
        self.compression.wake();
        mark_detection_content_changed(&self.detection_content_seq);
        self.io.resize(
            rows,
            cols,
            cell_width_px,
            cell_height_px,
            terminal_responses,
        );
    }

    pub fn nudge_child_redraw_after_handoff(&self) {
        let (rows, cols, cell_width_px, cell_height_px) = self.current_size.get();
        self.io
            .nudge_child_redraw_after_handoff(rows, cols, cell_width_px, cell_height_px);
    }

    /// Scroll up by N lines (into scrollback history).
    pub fn scroll_up(&self, lines: usize) {
        self.terminal.scroll_up(lines);
        self.compression.wake();
    }

    /// Scroll down by N lines (toward live output).
    pub fn scroll_down(&self, lines: usize) {
        self.terminal.scroll_down(lines);
        self.compression.wake();
    }

    /// Reset scroll to live view (offset = 0).
    pub fn scroll_reset(&self) {
        self.terminal.scroll_reset();
        self.compression.wake();
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        self.terminal.scroll_metrics()
    }

    pub fn input_state(&self) -> Option<InputState> {
        self.terminal.input_state()
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.terminal.bracketed_paste_enabled()
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.terminal.mouse_reporting_enabled()
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.terminal.sgr_pixel_mouse_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        self.terminal.plain_page_keys_use_host_scrollback()
    }

    pub fn cursor_state(&self, area: Rect, show_cursor: bool) -> Option<TerminalCursorState> {
        if !show_cursor {
            return None;
        }
        let cursor = self.terminal.cursor_state()?;
        if cursor.x >= area.width || cursor.y >= area.height {
            return None;
        }
        Some(TerminalCursorState {
            x: area.x + cursor.x,
            y: area.y + cursor.y,
            visible: cursor.visible,
            shape: cursor.shape,
        })
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.terminal.synchronized_output_active()
    }

    pub fn visible_text(&self) -> String {
        self.terminal.visible_text()
    }

    pub fn visible_ansi(&self) -> String {
        self.terminal.visible_ansi()
    }

    pub fn detection_text(&self) -> String {
        self.terminal.detection_text()
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.terminal.terminal_title()
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_text_snapshot(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_ansi_snapshot(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_unwrapped_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_unwrapped_text_snapshot(lines);
        self.compression.wake();
        result
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        let result = self.terminal.recent_unwrapped_ansi(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_unwrapped_ansi_snapshot(lines);
        self.compression.wake();
        result
    }

    pub fn snapshot_history(&self) -> Option<String> {
        let ansi = self.recent_unwrapped_ansi(usize::MAX);
        (!ansi.trim().is_empty()).then_some(ansi)
    }

    pub fn extract_selection<P>(
        &self,
        selection: &crate::selection::Selection<P>,
    ) -> Option<String> {
        let result = self.terminal.extract_selection(selection);
        self.compression.wake();
        result
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        self.terminal.render(frame, area, show_cursor);
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.terminal.visible_hyperlinks(area)
    }

    pub fn keyboard_protocol(&self) -> crate::input::KeyboardProtocol {
        let fallback = crate::input::KeyboardProtocol::from_kitty_flags(
            self.kitty_keyboard_flags.load(Ordering::Relaxed),
        );
        self.terminal.keyboard_protocol(fallback)
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.terminal.modify_other_keys_level()
    }

    pub fn encode_terminal_key(&self, key: crate::input::TerminalKey) -> Vec<u8> {
        self.terminal
            .encode_terminal_key(key, self.keyboard_protocol())
    }

    pub fn try_send_bytes(&self, bytes: Bytes) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.io.try_send_bytes(bytes)
    }

    pub fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<std::sync::mpsc::Receiver<std::io::Result<()>>> {
        self.io
            .queue_user_input_submission(text, enter, delay, deadline)
    }

    pub fn try_send_paste(&self, text: String) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.try_send_bytes(self.paste_payload(text))
    }

    fn paste_payload(&self, text: String) -> Bytes {
        let text = crate::platform::prepare_paste_text_for_pty(text);
        let bracketed = self.bracketed_paste_enabled();
        let payload = if bracketed {
            format!("\x1b[200~{text}\x1b[201~")
        } else {
            text
        };
        Bytes::from(payload)
    }

    pub fn wheel_routing(&self) -> Option<WheelRouting> {
        self.terminal.wheel_routing()
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        u16,
        Vec<crate::ghostty::ScreenTextRow>,
    )> {
        let result = self.terminal.screen_text_snapshot();
        self.compression.wake();
        result
    }

    pub fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        if !self.mouse_reporting_enabled() {
            return None;
        }
        self.terminal.encode_mouse_button(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.terminal.encode_mouse_motion(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        if self.wheel_routing()? != WheelRouting::MouseReport {
            return None;
        }
        self.terminal.encode_mouse_wheel(kind, position, modifiers)
    }

    pub(crate) fn pixel_size(&self) -> Option<(u32, u32)> {
        let (rows, cols, cell_width_px, cell_height_px) = self.current_size.get();
        let width = u32::from(cols).checked_mul(cell_width_px)?;
        let height = u32::from(rows).checked_mul(cell_height_px)?;
        (width > 0 && height > 0).then_some((width, height))
    }

    pub fn encode_alternate_scroll(
        &self,
        kind: crossterm::event::MouseEventKind,
    ) -> Option<Vec<u8>> {
        if self.wheel_routing()? != WheelRouting::AlternateScroll {
            return None;
        }
        let key = match kind {
            crossterm::event::MouseEventKind::ScrollUp => crossterm::event::KeyCode::Up,
            crossterm::event::MouseEventKind::ScrollDown => crossterm::event::KeyCode::Down,
            _ => return None,
        };
        Some(self.encode_terminal_key(crate::input::TerminalKey::new(
            key,
            crossterm::event::KeyModifiers::empty(),
        )))
    }

    /// Get the current working directory of the child shell process.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        if let Some(cwd) = self
            .reported_cwd
            .lock()
            .ok()
            .and_then(|reported_cwd| reported_cwd.clone())
        {
            return Some(cwd);
        }

        let pid = self.child_pid.load(Ordering::Relaxed);
        crate::platform::process_cwd(pid)
    }

    pub fn cwd_for_persistence(&self) -> Option<std::path::PathBuf> {
        let pid = self.child_pid.load(Ordering::Acquire);
        let exited = self.cwd_process_exited.load(Ordering::Acquire);
        if let Some(cwd) = (!exited)
            .then(|| crate::platform::process_cwd(pid))
            .flatten()
            .filter(|cwd| cwd.is_absolute())
        {
            // Persistence observations must not change OSC authority or follow-cwd behavior.
            if let Ok(mut known) = self.persistence_cwd.lock() {
                *known = Some(cwd.clone());
            }
            return Some(cwd);
        }
        self.persistence_cwd
            .lock()
            .ok()
            .and_then(|cwd| cwd.clone())
            .or_else(|| self.reported_cwd.lock().ok().and_then(|cwd| cwd.clone()))
    }

    pub fn child_pid(&self) -> Option<u32> {
        let pid = self.child_pid.load(Ordering::Acquire);
        (pid > 0).then_some(pid)
    }

    pub fn follow_cwd(&self) -> Option<std::path::PathBuf> {
        {
            let leader_cwd = self
                .io
                .foreground_process_group_id()
                .and_then(usable_process_cwd);
            leader_cwd.or_else(|| self.cwd())
        }

        #[cfg(not(unix))]
        {
            self.cwd()
        }
    }

    /// Get the current working directory of the process group controlling the pane PTY.
    pub fn foreground_cwd(&self) -> Option<std::path::PathBuf> {
        {
            let pid = self.child_pid.load(Ordering::Acquire);
            let shell_cwd = absolute_process_cwd(pid);
            let foreground_pgid = self
                .io
                .foreground_process_group_id()
                .or_else(|| crate::platform::foreground_process_group_id(pid));
            let leader_cwd = foreground_pgid.and_then(absolute_process_cwd);

            // The group leader's cwd is authoritative (issue #3270): a helper
            // process that chdirs elsewhere inside the same foreground group
            // must not override it. Scan other members only when the leader's
            // cwd cannot be read at all.
            leader_cwd
                .or_else(|| foreground_member_cwd_different_from_shell(pid, shell_cwd.as_ref()))
        }

        #[cfg(not(unix))]
        {
            None
        }
    }
}

#[cfg(test)]
impl PaneRuntime {
    pub(crate) fn test_with_channel(cols: u16, rows: u16) -> (Self, mpsc::Receiver<Bytes>) {
        Self::test_with_channel_and_scrollback_bytes(cols, rows, 0, &[], 4)
    }

    pub(crate) fn test_with_channel_capacity(
        cols: u16,
        rows: u16,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<Bytes>) {
        Self::test_with_channel_and_scrollback_bytes(cols, rows, 0, &[], capacity)
    }

    pub(crate) fn test_with_screen_bytes(cols: u16, rows: u16, bytes: &[u8]) -> Self {
        Self::test_with_scrollback_bytes(cols, rows, 0, bytes)
    }

    pub(crate) fn test_process_pty_bytes(&self, bytes: &[u8]) {
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.content_seq.fetch_add(1, Ordering::AcqRel);
        let (tx, _rx) = mpsc::channel(1);
        let _ = self.terminal.process_pty_bytes(self.pane_id, 0, bytes, &tx);
        self.content_seq.fetch_add(1, Ordering::Release);
        self.compression.wake();
    }

    pub(crate) fn test_with_scrollback_bytes(
        cols: u16,
        rows: u16,
        scrollback_limit_bytes: usize,
        bytes: &[u8],
    ) -> Self {
        Self::test_with_channel_and_scrollback_bytes(cols, rows, scrollback_limit_bytes, bytes, 4).0
    }

    pub(crate) fn test_with_channel_and_scrollback_bytes(
        cols: u16,
        rows: u16,
        scrollback_limit_bytes: usize,
        bytes: &[u8],
        channel_capacity: usize,
    ) -> (Self, mpsc::Receiver<Bytes>) {
        let (tx, rx) = mpsc::channel(channel_capacity);
        let (resize_tx, _resize_rx) = watch::channel((rows, cols, 0, 0));
        let mut terminal =
            crate::ghostty::Terminal::new(cols, rows, scrollback_limit_bytes).unwrap();
        terminal.write(bytes);
        let pane_id = PaneId::from_raw(0);
        let terminal = Arc::new(PaneTerminal::new(
            GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap(),
        ));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());

        (
            Self {
                pane_id,
                terminal,
                io: PaneRuntimeIo::TestChannel {
                    sender: tx,
                    resize_tx,
                },
                current_size: Cell::new((rows, cols, 0, 0)),
                child_pid: Arc::new(AtomicU32::new(0)),
                reported_cwd: Arc::new(Mutex::new(None)),
                persistence_cwd: Mutex::new(None),
                cwd_process_exited: Arc::new(AtomicBool::new(false)),
                child_wait_completed: None,
                kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
                content_seq: Arc::new(AtomicU64::new(0)),
                content_write_lock: Arc::new(Mutex::new(())),
                detection_content_seq: Arc::new(AtomicU64::new(0)),
                full_lifecycle_authority_active: Arc::new(AtomicBool::new(false)),
                detect_reset_notify: Arc::new(Notify::new()),
                pending_release: Arc::new(Mutex::new(None)),
                preserve_processes_on_drop: true,
                compression,
                detect_handle: Some(tokio::spawn(async {}).abort_handle()),
            },
            rx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_liveness_treats_reaped_direct_child_as_gone() {
        assert!(!process_alive_for_shutdown(42, 42, true, |_| true));
    }

    #[test]
    fn shutdown_liveness_keeps_unreaped_direct_child_alive() {
        assert!(process_alive_for_shutdown(42, 42, false, |_| true));
    }

    #[test]
    fn pane_shell_prefers_configured_shell() {
        assert_eq!(
            pane_shell_from("/usr/bin/nu", Some("/bin/bash".to_string())),
            "/usr/bin/nu"
        );
    }

    #[test]
    fn pane_shell_falls_back_to_shell_env() {
        assert_eq!(
            pane_shell_from("", Some("/bin/bash".to_string())),
            "/bin/bash"
        );
    }

    #[test]
    fn pane_shell_ignores_empty_values() {
        assert_eq!(
            pane_shell_from("   ", Some("  ".to_string())),
            default_pane_shell()
        );
        assert_eq!(pane_shell_from("", None), default_pane_shell());
    }

    #[test]
    fn windows_non_powershell_builder_launches_plain_shell() {
        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("cmd.exe", crate::config::ShellModeConfig::NonLogin),
            ShellLaunchTarget::Windows,
        )
        .unwrap();

        assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("cmd.exe")]);
    }

    #[test]
    fn login_shell_builder_rejects_missing_shell_instead_of_falling_back() {
        let err = pane_shell_command_builder_for_target(
            PaneShellConfig::new(
                "/__sesh_mux_missing_shell__",
                crate::config::ShellModeConfig::Login,
            ),
            ShellLaunchTarget::OtherUnix,
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn login_shell_builder_resolves_bare_shell_names_from_path() {
        let _lock = crate::integration::integration_env_lock();
        let base = std::env::temp_dir().join(format!(
            "sesh-mux-login-shell-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let shell = bin.join("fake-shell");
        std::fs::write(&shell, "#!/bin/sh\nexit 0\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let original_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &bin);

        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("fake-shell", crate::config::ShellModeConfig::Login),
            ShellLaunchTarget::OtherUnix,
        )
        .unwrap();

        assert!(cmd.is_default_prog());
        assert_eq!(
            cmd.get_env("SHELL").and_then(std::ffi::OsStr::to_str),
            shell.to_str()
        );
        match original_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn login_shell_resolution_preserves_shell_paths() {
        assert_eq!(resolve_shell_for_login_mode("/bin/sh").unwrap(), "/bin/sh");
    }

    #[test]
    fn non_login_shell_builder_execs_resolved_shell_directly() {
        let cmd = pane_shell_command_builder(PaneShellConfig::new(
            "/bin/sh",
            crate::config::ShellModeConfig::NonLogin,
        ))
        .unwrap();
        assert!(!cmd.is_default_prog());
        assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("/bin/sh")]);
    }

    #[tokio::test]
    async fn handoff_history_ansi_captures_primary_screen() {
        let runtime =
            PaneRuntime::test_with_scrollback_bytes(40, 5, 4096, b"handoff-primary-history\r\n");

        let history = runtime.handoff_history_ansi().unwrap();

        assert!(history.contains("handoff-primary-history"));
    }

    #[tokio::test]
    async fn handoff_history_ansi_skips_alternate_screen() {
        let runtime = PaneRuntime::test_with_scrollback_bytes(
            40,
            5,
            4096,
            b"primary\r\n\x1b[?1049halt-screen",
        );

        assert!(runtime.handoff_history_ansi().is_none());
    }

    #[tokio::test]
    async fn ended_handoff_keeps_persistence_cwd_when_pid_is_reused() {
        let (runtime, _rx) = PaneRuntime::test_with_channel(80, 24);
        assert!(runtime.child_wait_completed.is_none());
        let saved = std::env::temp_dir().join("saved-handoff-cwd");
        *runtime.persistence_cwd.lock().unwrap() = Some(saved.clone());
        // A different live process now owns the imported shell's numeric PID.
        runtime
            .child_pid
            .store(std::process::id(), Ordering::Release);
        runtime.cwd_process_exited.store(true, Ordering::Release);
        assert_eq!(runtime.cwd_for_persistence(), Some(saved));
        *runtime.persistence_cwd.lock().unwrap() = None;
        assert_eq!(runtime.cwd_for_persistence(), None);
    }

    #[test]
    fn truncate_handoff_history_keeps_recent_utf8_boundary() {
        let history = format!("old\n{}\nrecent\n", "é".repeat(8));

        let truncated = truncate_handoff_history(history, 20);

        assert_eq!(truncated, "recent\n");
        assert!(truncated.is_char_boundary(0));
    }

    #[test]
    fn truncate_handoff_history_drops_partial_long_line() {
        let history = format!("old\n{}", "x".repeat(64));

        let truncated = truncate_handoff_history(history, 12);

        assert!(truncated.is_empty());
    }

    #[tokio::test]
    async fn compression_permit_survives_an_aborted_async_waiter() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = spawn_blocking_with_compression_permit(permit, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.await.unwrap();

        handle.abort();
        assert!(semaphore.clone().try_acquire_owned().is_err());

        release_tx.send(()).unwrap();
        handle.await.unwrap();
        assert!(semaphore.try_acquire_owned().is_ok());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn compression_task_rechecks_history_after_a_read() {
        let suffix = "x".repeat(66);
        let history = (1..=2_000)
            .map(|line| format!("{line:05} {suffix}\r\n"))
            .collect::<String>();
        let runtime =
            PaneRuntime::test_with_scrollback_bytes(80, 24, 20_000_000, history.as_bytes());

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while runtime.compression.completed_passes() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let completed_before_read = runtime.compression.completed_passes();

        let snapshot = runtime.recent_unwrapped_text_snapshot(usize::MAX);
        assert!(snapshot.text.contains("00001 "));
        assert!(snapshot.text.contains("02000 "));

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while runtime.compression.completed_passes() == completed_before_read {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn same_agent_after_reported_exit_is_a_replacement_process() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Pi), Some(Agent::Pi), false, true),
            ForegroundShellAgentAction::ReportReplacementProcess
        );
    }

    #[test]
    fn unknown_non_shell_foreground_job_is_not_immediate_clear_signal() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Claude), None, false, false),
            ForegroundShellAgentAction::ObserveProbe
        );
    }

    #[test]
    fn reported_process_exit_clears_before_unknown_foreground_probe() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Claude), None, false, true),
            ForegroundShellAgentAction::ClearAgent
        );
    }
}
