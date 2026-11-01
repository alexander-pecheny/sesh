//! Agent state detection via terminal tail pattern matching.
//!
//! Each pane's live bottom-of-buffer text is read periodically and matched
//! against known agent output patterns to determine state.

pub mod manifest;
mod manifest_version;

/// The detected state of a terminal pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Agent finished, prompt visible, nothing happening.
    Idle,
    /// Agent is actively working/processing.
    Working,
    /// Agent needs human input and is blocked on a response.
    Blocked,
    /// Plain shell or unrecognized program.
    Unknown,
}

/// Screen-derived agent state plus confidence metadata used for source arbitration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDetection {
    pub state: AgentState,
    /// True when the current screen is an agent-owned viewer that shows
    /// transcript/history instead of the live prompt state.
    pub skip_state_update: bool,
    /// True when the current screen visibly shows live idle chrome.
    pub visible_idle: bool,
    /// True when the current screen visibly shows live UI chrome that needs
    /// human input. This is stronger than arbitrary prompt-like text in the
    /// scrollback and may override a non-blocked integration state.
    pub visible_blocker: bool,
    /// True when the current screen visibly shows live working chrome. PTY
    /// activity is the normal working authority; this remains diagnostic
    /// metadata and for non-PTY fallback paths.
    pub visible_working: bool,
}

/// Which agent we detected running in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Pi,
    Claude,
    Codex,
}

impl Agent {
    pub const ALL: [Self; 3] = [Self::Pi, Self::Claude, Self::Codex];
    pub const SCREEN_MANIFEST_AGENTS: [Self; 3] = Self::ALL;
}

pub fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::Pi => "pi",
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    }
}

pub fn interactive_agent_executable(agent: Agent) -> &'static str {
    match agent {
        Agent::Pi => "pi",
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    }
}

pub fn parse_agent_label(agent: &str) -> Option<Agent> {
    let name = normalized_agent_lookup_name(agent);
    parse_canonical_agent_label(&name).or_else(|| lookup_agent(&name))
}

pub(crate) fn parse_canonical_agent_label(label: &str) -> Option<Agent> {
    let agent = lookup_agent(label)?;
    (agent_label(agent) == label).then_some(agent)
}

fn lookup_agent(name: &str) -> Option<Agent> {
    let name = path_basename(name);
    match name {
        "pi" => Some(Agent::Pi),
        "claude" | "claude-code" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        _ => None,
    }
}

/// Identify which agent is running from the process name.
/// Returns `None` for plain shells or unrecognized programs.
pub fn identify_agent(process_name: &str) -> Option<Agent> {
    parse_agent_label(process_name)
}

pub fn identify_agent_in_job(job: &crate::platform::ForegroundJob) -> Option<(Agent, String)> {
    if let Some(process) = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group_id)
    {
        let candidate = normalized_process_name(process);
        if let Some(agent) = identify_agent(&candidate) {
            return Some((agent, candidate));
        }
    }

    let mut best: Option<(u8, Agent, String)> = None;

    for process in &job.processes {
        let candidate = normalized_process_name(process);
        let Some(agent) = identify_agent(&candidate) else {
            continue;
        };
        let score = process_priority(process, &candidate);

        match &best {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => best = Some((score, agent, candidate)),
        }
    }

    best.map(|(_, agent, name)| (agent, name))
}

/// Detect state and whether a visible blocker is present on the current screen.
#[allow(dead_code)] // shim for existing callers; detect_agent_with_osc is the real path
pub fn detect_agent(agent: Option<Agent>, screen_content: &str) -> AgentDetection {
    detect_agent_with_osc(agent, screen_content, "", "")
}

/// Detect state using screen content plus OSC title/progress strings.
pub fn detect_agent_with_osc(
    agent: Option<Agent>,
    screen_content: &str,
    osc_title: &str,
    osc_progress: &str,
) -> AgentDetection {
    let Some(agent) = agent else {
        return AgentDetection {
            state: AgentState::Unknown,
            skip_state_update: false,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
        };
    };
    manifest::detect_with_osc(
        agent,
        manifest::DetectionInput {
            screen: screen_content,
            osc_title,
            osc_progress,
        },
    )
}

pub fn should_skip_state_update(agent: Option<Agent>, screen_content: &str) -> bool {
    agent.is_some_and(|agent| manifest::should_skip_state_update(agent, screen_content))
}

pub(crate) fn full_lifecycle_hook_authority(source: &str, agent_label: &str) -> bool {
    matches!(
        (source, agent_label),
        ("sesh-mux:pi", "pi")
            | ("sesh-mux:omp", "omp")
            | ("sesh-mux:mastracode", "mastracode")
            | ("sesh-mux:opencode", "opencode")
            | ("sesh-mux:kilo", "kilo")
            | ("sesh-mux:kimi", "kimi")
    )
}

pub(crate) fn session_identity_only_integration(source: &str, agent_label: &str) -> bool {
    matches!(
        (source, agent_label),
        ("sesh-mux:hermes", "hermes")
            | ("sesh-mux:qwen", "qwen")
            | ("sesh-mux:letta", "letta")
            | ("sesh-mux:antigravity_cli", "agy")
    )
}

// ---------------------------------------------------------------------------
// Process identification (platform-specific)
// ---------------------------------------------------------------------------

/// Get the foreground job for a given child PID.
/// Delegates to platform-specific implementation.
pub fn foreground_job(child_pid: u32) -> Option<crate::platform::ForegroundJob> {
    crate::platform::foreground_job(child_pid)
}

/// Get the foreground process group leader as a one-process job.
/// This is cheaper than collecting every process in the foreground job.
pub fn foreground_group_leader_job(
    process_group_id: u32,
) -> Option<crate::platform::ForegroundJob> {
    crate::platform::foreground_group_leader_job(process_group_id)
}

/// Get the foreground process group for a pane shell PID.
/// This is cheaper than collecting every process in the foreground job.
pub fn foreground_process_group_id(child_pid: u32) -> Option<u32> {
    crate::platform::foreground_process_group_id(child_pid)
}

fn normalized_process_name(process: &crate::platform::ForegroundProcess) -> String {
    let effective = process.argv0.as_deref().unwrap_or(&process.name);
    let lower_effective = effective.to_lowercase();

    if is_generic_runtime_or_shell(&lower_effective) {
        if let Some(wrapped_agent) =
            wrapped_agent_name_from_runtime_argv(&lower_effective, process.argv.as_deref())
        {
            return wrapped_agent;
        }
    }

    if identify_agent(effective).is_some() {
        return effective.to_string();
    }

    if let Some(wrapped_agent) = argv0_agent_name(process.argv.as_deref())
        .or_else(|| cmdline_argv0_agent_name(process.cmdline.as_deref().unwrap_or_default()))
    {
        return wrapped_agent;
    }

    effective.to_string()
}

fn wrapped_agent_name_from_runtime_argv(runtime: &str, argv: Option<&[String]>) -> Option<String> {
    let argv = argv?;
    let runtime_name = normalized_agent_lookup_name(path_basename(runtime));

    match runtime_name.as_str() {
        "node" | "bun" => script_arg_agent_name(argv, &["-e", "--eval", "-p", "--print"], &[]),
        name if is_python_runtime(name) => script_arg_agent_name(argv, &["-c"], &["-m"]),
        "sh" | "bash" | "zsh" | "fish" => script_arg_agent_name(argv, &["-c"], &[]),
        "cmd" => windows_cmd_arg_agent_name(argv),
        "powershell" | "pwsh" => powershell_arg_agent_name(argv),
        "tmux" => None,
        _ => None,
    }
}

fn windows_cmd_arg_agent_name(argv: &[String]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.trim_matches('"').to_lowercase();
        match flag.as_str() {
            "/c" | "/k" => {
                return args
                    .next()
                    .and_then(|command| command_text_agent_name(command))
            }
            "/d" | "/s" | "/q" | "/a" | "/u" | "/e:on" | "/e:off" | "/f:on" | "/f:off"
            | "/v:on" | "/v:off" => continue,
            _ => {}
        }
    }
    None
}

fn powershell_arg_agent_name(argv: &[String]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.trim_matches('"').to_lowercase();
        match flag.as_str() {
            "-file" | "-f" | "/file" => {
                return args
                    .next()
                    .and_then(|path| agent_name_from_path_token(path));
            }
            "-command" | "-c" | "/command" | "/c" => {
                return args
                    .next()
                    .and_then(|command| command_text_agent_name(command));
            }
            "-encodedcommand" | "-enc" | "/encodedcommand" | "/enc" => return None,
            "-configurationname" | "-executionpolicy" | "-outputformat" | "-psconsolefile"
            | "-version" | "-windowstyle" | "-workingdirectory" => {
                let _ = args.next();
            }
            _ if flag.starts_with('-') || flag.starts_with('/') => {}
            _ => return agent_name_from_path_token(arg),
        }
    }
    None
}

fn command_text_agent_name(command: &str) -> Option<String> {
    let mut rest = command;
    while let Some((token, next)) = command_text_token(rest) {
        let token = token.trim();
        if token.eq_ignore_ascii_case("&")
            || token.eq_ignore_ascii_case(".")
            || token.eq_ignore_ascii_case("call")
        {
            rest = next;
            continue;
        }
        return agent_name_from_path_token(token);
    }
    None
}

fn command_text_token(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    let first = input.chars().next()?;
    if first == '"' || first == '\'' {
        let start = first.len_utf8();
        if let Some(end) = input[start..].find(first) {
            let end = start + end;
            return Some((&input[start..end], &input[end + first.len_utf8()..]));
        }
        return Some((&input[start..], ""));
    }

    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    Some((&input[..end], &input[end..]))
}

fn script_arg_agent_name(
    argv: &[String],
    eval_flags: &[&str],
    module_flags: &[&str],
) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args
                .next()
                .and_then(|token| agent_name_from_path_token(token));
        }

        if flag_matches(arg, eval_flags) || flag_matches(arg, module_flags) {
            return None;
        }

        if arg.starts_with('-') {
            if option_takes_value(arg) {
                let _ = args.next();
            }
            continue;
        }

        return agent_name_from_path_token(arg);
    }

    None
}

fn flag_matches(arg: &str, flags: &[&str]) -> bool {
    flags
        .iter()
        .any(|flag| arg == *flag || short_flag_payload(arg, flag) || long_flag_value(arg, flag))
}

fn short_flag_payload(arg: &str, flag: &str) -> bool {
    flag.starts_with('-')
        && !flag.starts_with("--")
        && arg.starts_with(flag)
        && arg.len() > flag.len()
}

fn long_flag_value(arg: &str, flag: &str) -> bool {
    flag.starts_with("--")
        && arg
            .strip_prefix(flag)
            .is_some_and(|rest| rest.starts_with('='))
}

fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-r" | "--require"
            | "--loader"
            | "--import"
            | "--experimental-loader"
            | "--inspect-port"
            | "-W"
            | "-X"
            | "-S"
            | "-L"
            | "-o"
    )
}

fn argv0_agent_name(argv: Option<&[String]>) -> Option<String> {
    agent_name_from_path_token(argv?.first()?)
}

fn cmdline_argv0_agent_name(cmdline: &str) -> Option<String> {
    agent_name_from_path_token(cmdline.split_whitespace().next()?)
}

fn agent_name_from_path_token(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(|c| matches!(c, '"' | '\''));
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }

    agent_name_from_basename(path_basename(trimmed))
        .or_else(|| agent_name_from_known_package_path(trimmed))
        .or_else(|| resolved_agent_name_from_path_token(trimmed))
}

fn agent_name_from_known_package_path(path: &str) -> Option<String> {
    let raw_components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect();
    let ends_with = |suffix: &[&str]| {
        raw_components.len() >= suffix.len()
            && raw_components[raw_components.len() - suffix.len()..]
                .iter()
                .zip(suffix)
                .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
    };
    if ends_with(&[
        "node_modules",
        "@earendil-works",
        "pi-coding-agent",
        "dist",
        "cli.js",
    ]) || ends_with(&[
        "node_modules",
        "@earendil-works",
        "pi-coding-agent",
        "dist",
        "bundle",
        "cli.js",
    ]) {
        return Some(agent_label(Agent::Pi).to_string());
    }

    None
}

fn resolved_agent_name_from_path_token(token: &str) -> Option<String> {
    let path = std::path::Path::new(token);
    if path.components().count() < 2 {
        return None;
    }

    let resolved = std::fs::canonicalize(path).ok()?;
    let basename = resolved.file_name()?.to_str()?;
    agent_name_from_basename(basename)
}

fn agent_name_from_basename(basename: &str) -> Option<String> {
    let agent = parse_agent_label(basename)?;
    Some(agent_label(agent).to_string())
}

fn normalized_agent_lookup_name(name: &str) -> String {
    let mut name = name.trim().to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if name.ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(path)
}

fn process_priority(process: &crate::platform::ForegroundProcess, normalized_name: &str) -> u8 {
    let lower_name = normalized_name.to_lowercase();
    if lower_name != process.name.to_lowercase() {
        return 3;
    }
    if !is_generic_runtime_or_shell(&lower_name) {
        return 2;
    }
    1
}

fn is_generic_runtime_or_shell(name: &str) -> bool {
    let name = normalized_agent_lookup_name(path_basename(name));
    is_python_runtime(&name)
        || matches!(
            name.as_str(),
            "sh" | "bash"
                | "zsh"
                | "fish"
                | "tmux"
                | "node"
                | "bun"
                | "cmd"
                | "powershell"
                | "pwsh"
        )
}

fn is_python_runtime(name: &str) -> bool {
    name == "python"
        || name.strip_prefix("python").is_some_and(|version| {
            !version.is_empty()
                && version
                    .split('.')
                    .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    // ---- Agent identification ----

    // A plain shell pane launched with sesh-mux's injected prompt integration
    // must still classify as a shell, not an agent, even though its argv now
    // carries a -Command payload.

    // ---- Screen detection routing ----

    // ---- Process identification (real PTY) ----
}
