use super::*;

// Codex is only a registry key here; behavior tests supply synthetic rules.

#[test]
fn all_bundled_manifests_parse_and_validate() {
    for agent in [Agent::Claude, Agent::Codex, Agent::Pi] {
        assert!(
            bundled_manifest(agent).is_some(),
            "missing bundled manifest for {}",
            agent_label(agent)
        );
    }
}

#[test]
fn top_non_empty_lines_requires_a_canonical_positive_bounded_count() {
    let name = "top_non_empty_lines";
    assert!(validate_region_name(&format!("{name}(1)")).is_ok());
    assert!(validate_region_name(&format!("{name}({})", u16::MAX)).is_ok());
    for count in ["0", "01", "+1", "65536", "999999999999999999999999"] {
        assert!(
            validate_region_name(&format!("{name}({count})")).is_err(),
            "{name} accepted invalid count {count}"
        );
    }
}

#[test]
fn top_non_empty_lines_requires_engine_three_when_declared() {
    let manifest = r#"
id = "codex"
version = "1"
min_engine_version = 2

[[rules]]
id = "background"
state = "working"
region = " top_non_empty_lines(1) "
contains = ["active"]
"#;
    assert!(parse_manifest(manifest).is_err());
}

#[test]
fn claude_waiting_for_a_login_code_is_blocked() {
    let screen = " Browser didn't open? Use the url below to sign in (c to copy)\n\nhttps://claude.com/cai/oauth/authorize?code=true\n\n Paste code here if prompted >\n";
    let detection = detect_with_osc(
        Agent::Claude,
        DetectionInput {
            screen,
            osc_title: "",
            osc_progress: "",
            osc_status: "",
        },
    );
    assert_eq!(detection.state, AgentState::Blocked);
}

#[test]
fn claude_program_status_outranks_the_screen() {
    let screen = "❯ \n  ? for shortcuts\n";
    for (status, state) in [
        ("working", AgentState::Working),
        ("blocked", AgentState::Blocked),
        ("done", AgentState::Idle),
        ("error", AgentState::Idle),
    ] {
        let input = DetectionInput {
            screen,
            osc_title: "",
            osc_progress: "",
            osc_status: status,
        };
        assert_eq!(
            detect_with_osc(Agent::Claude, input).state,
            state,
            "{status}"
        );
    }
}

#[test]
fn an_added_agents_screen_rules_come_from_its_file() {
    let file = "name = \"walrus\"\ncommand = [\"walrus\"]\n[detect]\nid = \"walrus\"\nversion = \"1\"\nmin_engine_version = 1\nupdated_at = \"2026-11-24T00:00:00Z\"\n[[detect.rules]]\nid = \"ready\"\nstate = \"idle\"\npriority = 90\nregion = \"bottom_non_empty_lines(4)\"\nline_regex = ['^ready']\n";
    let spec: crate::detect::agents::Spec = toml::from_str(file).unwrap();
    let manifest = parse_manifest(spec.detect.as_deref().unwrap()).unwrap();
    assert_eq!(manifest.id, "walrus");
    assert_eq!(compile_manifest(&manifest).unwrap().len(), 1);
}
