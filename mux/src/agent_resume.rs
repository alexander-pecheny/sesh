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

pub fn plan(source: &str, agent: &str, session_ref: &AgentSessionRef) -> Option<AgentResumePlan> {
    if !is_official_agent_source(source, agent) {
        return None;
    }

    let argv = match (source, agent, session_ref.kind) {
        ("sesh-mux:claude", "claude", AgentSessionRefKind::Id) => {
            vec![
                "claude".into(),
                "--resume".into(),
                session_ref.value.clone(),
            ]
        }
        ("sesh-mux:codex", "codex", AgentSessionRefKind::Id) => {
            vec!["codex".into(), "resume".into(), session_ref.value.clone()]
        }
        ("sesh-mux:pi", "pi", AgentSessionRefKind::Path | AgentSessionRefKind::Id) => {
            vec!["pi".into(), "--session".into(), session_ref.value.clone()]
        }
        _ => return None,
    };

    Some(AgentResumePlan {
        agent: agent.to_string(),
        argv,
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
        assert!(plan(
            &snapshot.source,
            "pi",
            &AgentSessionRef::path("/tmp/s.jsonl").unwrap()
        )
        .is_some());
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
    fn planner_rejects_custom_and_unsupported_path_refs() {
        let claude_session = absolute_test_path("claude-session");
        assert!(plan(
            "custom:claude",
            "claude",
            &AgentSessionRef::id("session").unwrap()
        )
        .is_none());
        assert!(plan(
            "sesh-mux:claude",
            "claude",
            &AgentSessionRef::path(&claude_session).unwrap()
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
            plan("sesh-mux:claude", "claude", &reported).unwrap().argv,
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
