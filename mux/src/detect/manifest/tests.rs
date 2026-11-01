use super::*;

// Codex is only a registry key here; behavior tests supply synthetic rules.

#[test]
fn all_bundled_manifests_parse_and_validate() {
    for agent in Agent::SCREEN_MANIFEST_AGENTS {
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
