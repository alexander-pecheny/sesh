//! The app knows the built-in Agents before it reaches any machine from `Resources/agents.json`,
//! which must say what the agent files say.

#[test]
fn the_apps_copy_of_the_built_in_agents_matches_their_files() {
    std::env::set_var("SESH_AGENTS", "/nonexistent");
    let specs: Vec<&serde_json::Value> = sesh_transcript::agent::specs().iter().map(|spec| &spec.raw).collect();
    let copy: serde_json::Value = serde_json::from_str(include_str!("../../../Resources/agents.json")).unwrap();
    assert_eq!(serde_json::json!(specs), copy, "regenerate it with `just agents`");
}
