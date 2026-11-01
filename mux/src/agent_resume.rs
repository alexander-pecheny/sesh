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

pub fn is_reserved_native_state_source(source: &str, agent: &str) -> bool {
    matches!(
        (source, agent),
        ("sesh-mux:claude", "claude")
            | ("sesh-mux:codex", "codex")
            | ("sesh-mux:copilot", "copilot")
            | ("sesh-mux:devin", "devin")
            | ("sesh-mux:droid", "droid")
            | ("sesh-mux:qodercli", "qodercli")
            | ("sesh-mux:qwen", "qwen")
            | ("sesh-mux:cursor", "cursor")
            | ("sesh-mux:grok", "grok")
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
        ("pi" | "omp", AgentSessionRefKind::Path) => AgentSessionRef::path(value)?,
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
        ("sesh-mux:copilot", "copilot", AgentSessionRefKind::Id) => {
            vec!["copilot".into(), format!("--resume={}", session_ref.value)]
        }
        ("sesh-mux:devin", "devin", AgentSessionRefKind::Id) => {
            vec!["devin".into(), "--resume".into(), session_ref.value.clone()]
        }
        ("sesh-mux:droid", "droid", AgentSessionRefKind::Id) => {
            vec!["droid".into(), "--resume".into(), session_ref.value.clone()]
        }
        ("sesh-mux:kimi", "kimi", AgentSessionRefKind::Id) => {
            vec!["kimi".into(), "--session".into(), session_ref.value.clone()]
        }
        ("sesh-mux:mastracode", "mastracode", AgentSessionRefKind::Id) => {
            vec![
                "mastracode".into(),
                "--thread".into(),
                session_ref.value.clone(),
            ]
        }
        ("sesh-mux:pi", "pi", AgentSessionRefKind::Path | AgentSessionRefKind::Id) => {
            vec!["pi".into(), "--session".into(), session_ref.value.clone()]
        }
        ("sesh-mux:omp", "omp", AgentSessionRefKind::Path | AgentSessionRefKind::Id) => {
            // omp resume is `-r, --resume=<value>` (ID prefix or path); it has no
            // `--session` flag, unlike pi.
            vec!["omp".into(), format!("--resume={}", session_ref.value)]
        }
        ("sesh-mux:hermes", "hermes", AgentSessionRefKind::Id) => {
            vec![
                "hermes".into(),
                "--resume".into(),
                session_ref.value.clone(),
            ]
        }
        ("sesh-mux:opencode", "opencode", AgentSessionRefKind::Id) => {
            vec![
                "opencode".into(),
                "--session".into(),
                session_ref.value.clone(),
            ]
        }
        ("sesh-mux:qodercli", "qodercli", AgentSessionRefKind::Id) => {
            vec![
                "qodercli".into(),
                "--resume".into(),
                session_ref.value.clone(),
            ]
        }
        ("sesh-mux:qwen", "qwen", AgentSessionRefKind::Id) => {
            vec!["qwen".into(), "--resume".into(), session_ref.value.clone()]
        }
        ("sesh-mux:kilo", "kilo", AgentSessionRefKind::Id) => {
            vec!["kilo".into(), "--session".into(), session_ref.value.clone()]
        }
        ("sesh-mux:cursor", "cursor", AgentSessionRefKind::Id) => {
            vec![
                if cfg!(windows) {
                    "cursor-agent.cmd"
                } else {
                    "cursor-agent"
                }
                .into(),
                "--resume".into(),
                session_ref.value.clone(),
            ]
        }
        ("sesh-mux:antigravity_cli", "agy", AgentSessionRefKind::Id) => {
            vec![
                "agy".into(),
                "--conversation".into(),
                session_ref.value.clone(),
            ]
        }
        ("sesh-mux:grok", "grok", AgentSessionRefKind::Id) => {
            vec!["grok".into(), "--resume".into(), session_ref.value.clone()]
        }
        ("sesh-mux:letta", "letta", AgentSessionRefKind::Id) => {
            if let Some(agent_id) = session_ref.value.strip_prefix("default:") {
                if agent_id.is_empty() {
                    return None;
                }
                vec![
                    "letta".into(),
                    "--conversation".into(),
                    "default".into(),
                    "--agent".into(),
                    agent_id.into(),
                ]
            } else {
                vec![
                    "letta".into(),
                    "--conversation".into(),
                    session_ref.value.clone(),
                ]
            }
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
        ("sesh-mux:claude", "claude")
            | ("sesh-mux:codex", "codex")
            | ("sesh-mux:copilot", "copilot")
            | ("sesh-mux:devin", "devin")
            | ("sesh-mux:droid", "droid")
            | ("sesh-mux:kimi", "kimi")
            | ("sesh-mux:omp", "omp")
            | ("sesh-mux:mastracode", "mastracode")
            | ("sesh-mux:pi", "pi")
            | ("sesh-mux:hermes", "hermes")
            | ("sesh-mux:opencode", "opencode")
            | ("sesh-mux:qodercli", "qodercli")
            | ("sesh-mux:qwen", "qwen")
            | ("sesh-mux:kilo", "kilo")
            | ("sesh-mux:cursor", "cursor")
            | ("sesh-mux:antigravity_cli", "agy")
            | ("sesh-mux:grok", "grok")
            | ("sesh-mux:letta", "letta")
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

    fn absolute_test_path(name: &str) -> String {
        std::env::current_dir()
            .unwrap()
            .join(name)
            .display()
            .to_string()
    }

    #[test]
    fn native_state_reservation_excludes_full_lifecycle_sources() {
        assert!(is_reserved_native_state_source("sesh-mux:claude", "claude"));
        assert!(is_reserved_native_state_source("sesh-mux:codex", "codex"));
        assert!(is_reserved_native_state_source("sesh-mux:devin", "devin"));
        assert!(!is_reserved_native_state_source("sesh-mux:kimi", "kimi"));
        assert!(!is_reserved_native_state_source(
            "sesh-mux:opencode",
            "opencode"
        ));
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
    fn planner_allows_supported_agents() {
        let pi_session = absolute_test_path("pi-session.jsonl");
        let omp_session = absolute_test_path("omp-session.jsonl");
        assert_eq!(
            plan(
                "sesh-mux:claude",
                "claude",
                &AgentSessionRef::id("claude-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["claude", "--resume", "claude-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:codex",
                "codex",
                &AgentSessionRef::id("codex-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["codex", "resume", "codex-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:copilot",
                "copilot",
                &AgentSessionRef::id("copilot-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["copilot", "--resume=copilot-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:devin",
                "devin",
                &AgentSessionRef::id("devin-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["devin", "--resume", "devin-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:droid",
                "droid",
                &AgentSessionRef::id("droid-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["droid", "--resume", "droid-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:kimi",
                "kimi",
                &AgentSessionRef::id("kimi-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["kimi", "--session", "kimi-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:mastracode",
                "mastracode",
                &AgentSessionRef::id("mastracode-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["mastracode", "--thread", "mastracode-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:pi",
                "pi",
                &AgentSessionRef::path(&pi_session).unwrap()
            )
            .unwrap()
            .argv,
            vec!["pi", "--session", pi_session.as_str()]
        );
        assert_eq!(
            plan(
                "sesh-mux:omp",
                "omp",
                &AgentSessionRef::path(&omp_session).unwrap()
            )
            .unwrap()
            .argv,
            vec!["omp", format!("--resume={omp_session}").as_str()]
        );
        assert_eq!(
            plan(
                "sesh-mux:hermes",
                "hermes",
                &AgentSessionRef::id("hermes-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["hermes", "--resume", "hermes-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:opencode",
                "opencode",
                &AgentSessionRef::id("opencode-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["opencode", "--session", "opencode-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:qodercli",
                "qodercli",
                &AgentSessionRef::id("qoder-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["qodercli", "--resume", "qoder-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:qwen",
                "qwen",
                &AgentSessionRef::id("qwen-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["qwen", "--resume", "qwen-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:kilo",
                "kilo",
                &AgentSessionRef::id("kilo-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["kilo", "--session", "kilo-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:cursor",
                "cursor",
                &AgentSessionRef::id("cursor-session").unwrap()
            )
            .unwrap()
            .argv,
            vec![
                if cfg!(windows) {
                    "cursor-agent.cmd"
                } else {
                    "cursor-agent"
                },
                "--resume",
                "cursor-session",
            ]
        );
        assert_eq!(
            plan(
                "sesh-mux:antigravity_cli",
                "agy",
                &AgentSessionRef::id("agy-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["agy", "--conversation", "agy-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:grok",
                "grok",
                &AgentSessionRef::id("grok-session").unwrap()
            )
            .unwrap()
            .argv,
            vec!["grok", "--resume", "grok-session"]
        );
        assert_eq!(
            plan(
                "sesh-mux:letta",
                "letta",
                &AgentSessionRef::id("conversation-123").unwrap()
            )
            .unwrap()
            .argv,
            vec!["letta", "--conversation", "conversation-123"]
        );
        assert_eq!(
            plan(
                "sesh-mux:letta",
                "letta",
                &AgentSessionRef::id("default:agent-123").unwrap()
            )
            .unwrap()
            .argv,
            vec!["letta", "--conversation", "default", "--agent", "agent-123"]
        );
        assert!(plan(
            "sesh-mux:letta",
            "letta",
            &AgentSessionRef::id("default:").unwrap()
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
    fn report_ref_prefers_pi_and_omp_paths_and_validates_values() {
        let pi_session = absolute_test_path("pi-session.jsonl");
        let omp_session = absolute_test_path("omp-session.jsonl");
        let claude_session = absolute_test_path("claude-session");
        let copilot_session = absolute_test_path("copilot-session");
        let session_ref = session_ref_from_report(
            "sesh-mux:pi",
            "pi",
            Some("pi-id".into()),
            Some(pi_session.clone()),
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Path);
        assert_eq!(session_ref.value, pi_session);

        assert!(
            session_ref_from_report("sesh-mux:pi", "pi", Some("bad\nid".into()), None).is_none()
        );
        assert!(
            session_ref_from_report("sesh-mux:pi", "pi", None, Some("relative.jsonl".into()))
                .is_none()
        );
        assert!(session_ref_from_report("custom:pi", "pi", Some("pi-id".into()), None).is_none());

        let session_ref = session_ref_from_report(
            "sesh-mux:omp",
            "omp",
            Some("omp-id".into()),
            Some(omp_session.clone()),
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Path);
        assert_eq!(session_ref.value, omp_session);

        let session_ref =
            session_ref_from_report("sesh-mux:omp", "omp", Some("omp-id".into()), None).unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "omp-id");
        let session_ref = session_ref_from_report(
            "sesh-mux:omp",
            "omp",
            Some("omp-id".into()),
            Some("relative.jsonl".into()),
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "omp-id");
        assert!(session_ref_from_report(
            "sesh-mux:omp",
            "omp",
            None,
            Some("relative.jsonl".into())
        )
        .is_none());

        assert!(
            session_ref_from_report("sesh-mux:claude", "claude", None, Some(claude_session))
                .is_none()
        );

        let session_ref = session_ref_from_report(
            "sesh-mux:copilot",
            "copilot",
            Some("copilot-id".into()),
            None,
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "copilot-id");
        assert!(session_ref_from_report(
            "sesh-mux:copilot",
            "copilot",
            None,
            Some(copilot_session)
        )
        .is_none());

        let session_ref =
            session_ref_from_report("sesh-mux:devin", "devin", Some("devin-id".into()), None)
                .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "devin-id");

        let session_ref =
            session_ref_from_report("sesh-mux:droid", "droid", Some("droid-id".into()), None)
                .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "droid-id");
        assert!(session_ref_from_report(
            "sesh-mux:droid",
            "droid",
            None,
            Some("/tmp/droid-session".into())
        )
        .is_none());

        let session_ref =
            session_ref_from_report("sesh-mux:kimi", "kimi", Some("kimi-id".into()), None).unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "kimi-id");

        let session_ref = session_ref_from_report(
            "sesh-mux:mastracode",
            "mastracode",
            Some("mastracode-id".into()),
            None,
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "mastracode-id");

        let session_ref =
            session_ref_from_report("sesh-mux:kilo", "kilo", Some("kilo-id".into()), None).unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "kilo-id");

        let session_ref = session_ref_from_report(
            "sesh-mux:qodercli",
            "qodercli",
            Some("qoder-id".into()),
            None,
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "qoder-id");

        let session_ref =
            session_ref_from_report("sesh-mux:qwen", "qwen", Some("qwen-id".into()), None).unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "qwen-id");

        let session_ref = session_ref_from_report(
            "sesh-mux:antigravity_cli",
            "agy",
            Some("agy-id".into()),
            None,
        )
        .unwrap();
        assert_eq!(session_ref.kind, AgentSessionRefKind::Id);
        assert_eq!(session_ref.value, "agy-id");
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

    #[test]
    fn ids_are_data_not_shell_text() {
        let id = "abc; rm -rf /";
        let codex_plan =
            plan("sesh-mux:codex", "codex", &AgentSessionRef::id(id).unwrap()).unwrap();
        assert_eq!(codex_plan.argv, vec!["codex", "resume", id]);

        let copilot_plan = plan(
            "sesh-mux:copilot",
            "copilot",
            &AgentSessionRef::id(id).unwrap(),
        )
        .unwrap();
        assert_eq!(copilot_plan.argv, vec!["copilot", "--resume=abc; rm -rf /"]);

        let devin_plan =
            plan("sesh-mux:devin", "devin", &AgentSessionRef::id(id).unwrap()).unwrap();
        assert_eq!(devin_plan.argv, vec!["devin", "--resume", id]);
    }

    #[test]
    fn planner_rejects_path_refs_for_id_only_agents() {
        let hermes_session = absolute_test_path("hermes-session");
        let opencode_session = absolute_test_path("opencode-session");
        let kilo_session = absolute_test_path("kilo-session");
        let copilot_session = absolute_test_path("copilot-session");
        let devin_session = absolute_test_path("devin-session");
        assert!(plan(
            "sesh-mux:hermes",
            "hermes",
            &AgentSessionRef::path(&hermes_session).unwrap()
        )
        .is_none());
        assert!(plan(
            "sesh-mux:opencode",
            "opencode",
            &AgentSessionRef::path(&opencode_session).unwrap()
        )
        .is_none());
        assert!(plan(
            "sesh-mux:kilo",
            "kilo",
            &AgentSessionRef::path(&kilo_session).unwrap()
        )
        .is_none());
        assert!(plan(
            "sesh-mux:copilot",
            "copilot",
            &AgentSessionRef::path(&copilot_session).unwrap()
        )
        .is_none());
        assert!(plan(
            "sesh-mux:devin",
            "devin",
            &AgentSessionRef::path(&devin_session).unwrap()
        )
        .is_none());
        assert!(session_ref_from_snapshot(
            "sesh-mux:mastracode",
            "mastracode",
            AgentSessionRefKind::Id,
            "mastracode-session",
            None
        )
        .is_some());
        assert!(session_ref_from_snapshot(
            "sesh-mux:hermes",
            "hermes",
            AgentSessionRefKind::Id,
            "hermes-session",
            None
        )
        .is_some());
        assert!(session_ref_from_snapshot(
            "sesh-mux:opencode",
            "opencode",
            AgentSessionRefKind::Id,
            "opencode-session",
            None
        )
        .is_some());
        assert!(session_ref_from_snapshot(
            "sesh-mux:kilo",
            "kilo",
            AgentSessionRefKind::Id,
            "kilo-session",
            None
        )
        .is_some());
        assert!(session_ref_from_snapshot(
            "sesh-mux:copilot",
            "copilot",
            AgentSessionRefKind::Id,
            "copilot-session",
            None
        )
        .is_some());
        assert!(session_ref_from_snapshot(
            "sesh-mux:devin",
            "devin",
            AgentSessionRefKind::Id,
            "devin-session",
            None
        )
        .is_some());
        assert!(session_ref_from_snapshot(
            "sesh-mux:antigravity_cli",
            "agy",
            AgentSessionRefKind::Id,
            "agy-session",
            None
        )
        .is_some());
        let agy_session = absolute_test_path("agy-session");
        assert!(plan(
            "sesh-mux:antigravity_cli",
            "agy",
            &AgentSessionRef::path(&agy_session).unwrap()
        )
        .is_none());
    }
}
