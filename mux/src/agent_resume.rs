use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

const MAX_SESSION_ID_LEN: usize = 512;
const MAX_SESSION_PATH_LEN: usize = 4096;

/// `path` is the Agent's Transcript. It rides along but is not identity, so two
/// refs to one session stay equal whether or not a report carried the path.
#[derive(Debug, Clone, Eq)]
pub struct AgentSessionRef {
    pub kind: AgentSessionRefKind,
    pub value: String,
    pub path: Option<String>,
}

impl PartialEq for AgentSessionRef {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.value == other.value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionRefKind {
    Id,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResumePlan {
    pub agent: String,
    pub argv: Vec<String>,
    pub dedupe_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedAgentSession {
    pub source: String,
    pub agent: String,
    pub session_ref: AgentSessionRef,
}

/// What `agent.start` ran an Agent with, minus the session it picked, so a resume runs it the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentLaunch {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl AgentLaunch {
    pub fn from_start(agent: crate::detect::Agent, args: &[String], env: &BTreeMap<String, String>) -> Self {
        let set_on_every_start = [crate::integration::SESH_MUX_PANE_ID_ENV_VAR, crate::api::SOCKET_PATH_ENV_VAR];
        Self {
            args: args_without_session(agent, args),
            env: env
                .iter()
                .filter(|(name, _)| !set_on_every_start.contains(&name.as_str()))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        }
    }
}

fn args_without_session(agent: crate::detect::Agent, args: &[String]) -> Vec<String> {
    use crate::detect::Agent;
    let flags: &[&str] = match agent {
        Agent::Claude => &["--session-id", "--resume", "-r"],
        Agent::Codex => &["resume"],
        Agent::Pi => &["--session"],
    };
    let mut kept = Vec::new();
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if !flags.contains(&flag) {
            kept.push(arg.clone());
        } else if flag == arg && args.peek().is_some_and(|value| !value.starts_with('-')) {
            args.next();
        }
    }
    kept
}

pub fn launch_argv(executable: &str, env: &BTreeMap<String, String>, args: Vec<String>) -> Option<Vec<String>> {
    let valid = |name: &String| !name.is_empty() && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if !env.iter().all(|(name, value)| valid(name) && !value.chars().any(char::is_control)) {
        return None;
    }
    let assignments = env.iter().map(|(name, value)| format!("{name}={value}"));
    let head = (!env.is_empty()).then(|| "/usr/bin/env".to_string());
    Some(head.into_iter().chain(assignments).chain([executable.to_string()]).chain(args).collect())
}

impl AgentSessionRef {
    pub fn id(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        valid_session_id(&value).then_some(Self {
            kind: AgentSessionRefKind::Id,
            value,
            path: None,
        })
    }

    pub fn path(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        valid_session_path(&value).then_some(Self {
            kind: AgentSessionRefKind::Path,
            path: Some(value.clone()),
            value,
        })
    }

    fn with_path(mut self, path: Option<String>) -> Self {
        if self.path.is_none() {
            self.path = path.filter(|path| valid_session_path(path));
        }
        self
    }
}

pub fn session_ref_from_report(
    source: &str,
    agent: &str,
    agent_session_id: Option<String>,
    agent_session_path: Option<String>,
) -> Option<AgentSessionRef> {
    if !is_official_agent_source(source, agent) {
        return None;
    }

    if agent == "pi" || agent == "omp" {
        return agent_session_path
            .and_then(AgentSessionRef::path)
            .or_else(|| agent_session_id.and_then(AgentSessionRef::id));
    }

    agent_session_id
        .and_then(AgentSessionRef::id)
        .map(|session_ref| session_ref.with_path(agent_session_path))
}

pub fn persisted_session_from_launch_args(
    agent: crate::detect::Agent,
    args: &[String],
) -> Option<PersistedAgentSession> {
    if agent == crate::detect::Agent::Claude {
        let at = args.iter().position(|arg| arg == "--session-id")?;
        return Some(PersistedAgentSession {
            source: "sesh-mux:claude".into(),
            agent: "claude".into(),
            session_ref: AgentSessionRef::id(args.get(at + 1)?.clone())?,
        });
    }
    let [command, session_id] = args else {
        return None;
    };
    if agent != crate::detect::Agent::Codex || command != "resume" || session_id.starts_with('-') {
        return None;
    }

    Some(PersistedAgentSession {
        source: "sesh-mux:codex".into(),
        agent: "codex".into(),
        session_ref: AgentSessionRef::id(session_id.clone())?,
    })
}

pub fn normalize_session_start_source(value: Option<String>) -> Option<String> {
    match value.as_deref().map(str::trim) {
        Some(
            source @ ("startup" | "resume" | "clear" | "compact" | "branch" | "new" | "fork"
            | "select"),
        ) => Some(source.to_string()),
        _ => None,
    }
}

/// Sources from hooks installed by the server Sesh ran before sesh-mux, which agents that
/// outlived the handoff still report with.
const LEGACY_SOURCE_PREFIX: &str = "herdr:";

/// Maps a legacy hook source onto the sesh-mux source of the same integration.
pub fn canonical_source(source: &str) -> std::borrow::Cow<'_, str> {
    match source.strip_prefix(LEGACY_SOURCE_PREFIX) {
        Some(agent) => format!("sesh-mux:{agent}").into(),
        None => source.into(),
    }
}

pub(crate) fn deserialize_canonical_source<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    let source = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(canonical_source(&source).into_owned())
}

pub fn is_reserved_native_state_source(source: &str, agent: &str) -> bool {
    matches!(
        (source, agent),
        ("sesh-mux:claude", "claude") | ("sesh-mux:codex", "codex")
    )
}

pub fn session_ref_from_snapshot(
    source: &str,
    agent: &str,
    kind: AgentSessionRefKind,
    value: &str,
    path: Option<&str>,
) -> Option<PersistedAgentSession> {
    if !is_official_agent_source(source, agent) {
        return None;
    }
    let session_ref = match (agent, kind) {
        ("pi", AgentSessionRefKind::Path) => AgentSessionRef::path(value)?,
        (_, AgentSessionRefKind::Id) => {
            AgentSessionRef::id(value)?.with_path(path.map(str::to_string))
        }
        _ => return None,
    };
    Some(PersistedAgentSession {
        source: source.to_string(),
        agent: agent.to_string(),
        session_ref,
    })
}

pub fn plan(
    source: &str,
    agent: &str,
    session_ref: &AgentSessionRef,
    launch: Option<&AgentLaunch>,
) -> Option<AgentResumePlan> {
    if !is_official_agent_source(source, agent) {
        return None;
    }

    let resume_args = match (source, agent, session_ref.kind) {
        ("sesh-mux:claude", "claude", AgentSessionRefKind::Id) => ["--resume", &session_ref.value],
        ("sesh-mux:codex", "codex", AgentSessionRefKind::Id) => ["resume", &session_ref.value],
        ("sesh-mux:pi", "pi", AgentSessionRefKind::Path | AgentSessionRefKind::Id) => {
            ["--session", &session_ref.value]
        }
        _ => return None,
    };
    let launch = launch.cloned().unwrap_or_default();
    let args = launch.args.into_iter().chain(resume_args.map(str::to_string)).collect();

    Some(AgentResumePlan {
        agent: agent.to_string(),
        argv: launch_argv(agent, &launch.env, args)?,
        dedupe_key: dedupe_key(source, agent, session_ref),
    })
}

pub fn dedupe_key(source: &str, agent: &str, session_ref: &AgentSessionRef) -> String {
    format!(
        "{source}\u{0}{agent}\u{0}{:?}\u{0}{}",
        session_ref.kind, session_ref.value
    )
}

pub(crate) fn is_official_agent_source(source: &str, agent: &str) -> bool {
    matches!(
        (source, agent),
        ("sesh-mux:claude", "claude") | ("sesh-mux:codex", "codex") | ("sesh-mux:pi", "pi")
    )
}

fn valid_session_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_SESSION_ID_LEN && !value.chars().any(char::is_control)
}

fn valid_session_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SESSION_PATH_LEN
        && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_sources_map_onto_sesh_mux_sources() {
        assert_eq!(canonical_source("herdr:claude"), "sesh-mux:claude");
        assert_eq!(canonical_source("sesh-mux:codex"), "sesh-mux:codex");
        assert_eq!(canonical_source("custom:pi"), "custom:pi");
        let snapshot: crate::persist::PaneAgentSessionSnapshot = serde_json::from_str(
            r#"{"source":"herdr:pi","agent":"pi","kind":"path","value":"/tmp/s.jsonl"}"#,
        )
        .unwrap();
        assert_eq!(snapshot.source, "sesh-mux:pi");
        let session = AgentSessionRef::path("/tmp/s.jsonl").unwrap();
        assert!(plan(&snapshot.source, "pi", &session, None).is_some());
    }

    fn absolute_test_path(name: &str) -> String {
        std::env::current_dir()
            .unwrap()
            .join(name)
            .display()
            .to_string()
    }

    #[test]
    fn codex_noncanonical_resume_launch_has_no_explicit_session() {
        assert_eq!(
            persisted_session_from_launch_args(
                crate::detect::Agent::Codex,
                &["resume".into(), "codex-session".into()]
            )
            .unwrap()
            .session_ref
            .value,
            "codex-session"
        );
        assert!(persisted_session_from_launch_args(
            crate::detect::Agent::Codex,
            &["resume".into(), "--last".into()]
        )
        .is_none());
        assert!(persisted_session_from_launch_args(
            crate::detect::Agent::Codex,
            &["resume".into(), "not-a-session".into(), "--last".into()]
        )
        .is_none());
        assert!(persisted_session_from_launch_args(
            crate::detect::Agent::Codex,
            &[
                "--remote".into(),
                "ws://example.test".into(),
                "resume".into(),
                "remote-session".into(),
            ]
        )
        .is_none());
    }

    #[test]
    fn claude_launch_with_a_session_id_names_its_session() {
        let args = ["--remote-control".into(), "x".into(), "--session-id".into(), "5c91096a-1a44-4e89-93ba-28c19f080748".into()];
        let session = persisted_session_from_launch_args(crate::detect::Agent::Claude, &args).unwrap();
        assert_eq!((session.source.as_str(), session.session_ref.value.as_str()), ("sesh-mux:claude", "5c91096a-1a44-4e89-93ba-28c19f080748"));
        assert!(persisted_session_from_launch_args(crate::detect::Agent::Claude, &["--resume".into(), "x".into()]).is_none());
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn launch_keeps_what_the_agent_was_started_with_except_its_session() {
        use crate::detect::Agent;
        let env = [
            ("CLAUDE_CONFIG_DIR", "/c"),
            (crate::integration::SESH_MUX_PANE_ID_ENV_VAR, "w1:p1"),
            (crate::api::SOCKET_PATH_ENV_VAR, "/old.sock"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .into();
        let claude = AgentLaunch::from_start(
            Agent::Claude,
            &strings(&["--remote-control", "x", "--session-id", "id", "--dangerously-skip-permissions"]),
            &env,
        );
        assert_eq!(claude.args, strings(&["--remote-control", "x", "--dangerously-skip-permissions"]));
        assert_eq!(claude.env, [("CLAUDE_CONFIG_DIR".to_string(), "/c".to_string())].into());
        let none = BTreeMap::new();
        let args = |agent, args: &[&str]| AgentLaunch::from_start(agent, &strings(args), &none).args;
        assert_eq!(args(Agent::Claude, &["--resume=id", "-r", "--verbose"]), strings(&["--verbose"]));
        assert_eq!(args(Agent::Codex, &["resume", "id", "--yolo"]), strings(&["--yolo"]));
        assert_eq!(args(Agent::Codex, &["--remote", "ws://x", "resume", "id"]), strings(&["--remote", "ws://x"]));
        assert_eq!(args(Agent::Pi, &["--session", "/s.jsonl", "--model", "m"]), strings(&["--model", "m"]));
    }

    #[test]
    fn resume_runs_the_agent_with_its_launch_args_and_env() {
        let launch = AgentLaunch {
            args: strings(&["--dangerously-skip-permissions"]),
            env: [("CLAUDE_CONFIG_DIR".to_string(), "/c".to_string())].into(),
        };
        let session = AgentSessionRef::id("id").unwrap();
        assert_eq!(
            plan("sesh-mux:claude", "claude", &session, Some(&launch)).unwrap().argv,
            strings(&["/usr/bin/env", "CLAUDE_CONFIG_DIR=/c", "claude", "--dangerously-skip-permissions", "--resume", "id"])
        );
        let codex = AgentLaunch { args: strings(&["--yolo"]), env: BTreeMap::new() };
        assert_eq!(
            plan("sesh-mux:codex", "codex", &session, Some(&codex)).unwrap().argv,
            strings(&["codex", "--yolo", "resume", "id"])
        );
        assert_eq!(
            plan("sesh-mux:codex", "codex", &session, None).unwrap().argv,
            strings(&["codex", "resume", "id"])
        );
    }

    #[test]
    fn planner_rejects_custom_and_unsupported_path_refs() {
        let claude_session = absolute_test_path("claude-session");
        assert!(plan(
            "custom:claude",
            "claude",
            &AgentSessionRef::id("session").unwrap(),
            None
        )
        .is_none());
        assert!(plan(
            "sesh-mux:claude",
            "claude",
            &AgentSessionRef::path(&claude_session).unwrap(),
            None
        )
        .is_none());
    }

    #[test]
    fn report_and_snapshot_refs_keep_transcript_paths_without_changing_identity() {
        let transcript = absolute_test_path("claude-session.jsonl");
        let reported = session_ref_from_report(
            "sesh-mux:claude",
            "claude",
            Some("claude-id".into()),
            Some(transcript.clone()),
        )
        .unwrap();
        assert_eq!(reported.kind, AgentSessionRefKind::Id);
        assert_eq!(reported.path.as_deref(), Some(transcript.as_str()));
        assert_eq!(reported, AgentSessionRef::id("claude-id").unwrap());
        assert_eq!(
            plan("sesh-mux:claude", "claude", &reported, None).unwrap().argv,
            vec!["claude", "--resume", "claude-id"]
        );

        let relative = session_ref_from_report(
            "sesh-mux:codex",
            "codex",
            Some("codex-id".into()),
            Some("relative.jsonl".into()),
        )
        .unwrap();
        assert_eq!(relative.path, None);

        let restored = session_ref_from_snapshot(
            "sesh-mux:codex",
            "codex",
            AgentSessionRefKind::Id,
            "codex-id",
            Some(&transcript),
        )
        .unwrap();
        assert_eq!(
            restored.session_ref.path.as_deref(),
            Some(transcript.as_str())
        );
    }

    #[test]
    fn normalize_session_start_source_allows_known_values() {
        assert_eq!(
            normalize_session_start_source(Some("startup".into())),
            Some("startup".into())
        );
        assert_eq!(
            normalize_session_start_source(Some("resume".into())),
            Some("resume".into())
        );
        assert_eq!(
            normalize_session_start_source(Some("clear".into())),
            Some("clear".into())
        );
        assert_eq!(
            normalize_session_start_source(Some("compact".into())),
            Some("compact".into())
        );
        assert_eq!(
            normalize_session_start_source(Some("branch".into())),
            Some("branch".into())
        );
        assert_eq!(
            normalize_session_start_source(Some("new".into())),
            Some("new".into())
        );
        assert_eq!(
            normalize_session_start_source(Some("fork".into())),
            Some("fork".into())
        );
        assert_eq!(
            normalize_session_start_source(Some("select".into())),
            Some("select".into())
        );
        assert_eq!(
            normalize_session_start_source(Some(" resume ".into())),
            Some("resume".into())
        );
        assert_eq!(normalize_session_start_source(Some("other".into())), None);
        assert_eq!(normalize_session_start_source(None), None);
    }
}
