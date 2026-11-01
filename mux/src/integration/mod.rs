mod actions;
mod claude_settings;
mod command;
mod config_edit;
mod config_file;
mod env;
mod file_ops;
mod registry;
mod targets;
mod types;
mod version;

pub(crate) use actions::{install_target, uninstall_target};
#[cfg(test)]
pub(crate) use env::integration_env_lock;
pub(crate) use env::{
    apply_pane_base_env, SESH_MUX_PANE_ID_ENV_VAR, SESH_MUX_TAB_ID_ENV_VAR,
    SESH_MUX_WORKSPACE_ID_ENV_VAR,
};
pub(crate) use registry::{
    installed_integration_statuses, integration_recommendations, integration_target_label,
    print_outdated_update_notice,
};
pub(crate) use types::{IntegrationRecommendation, IntegrationStatus, IntegrationStatusKind};

const PI_EXTENSION_INSTALL_NAME: &str = "sesh-mux-agent-state.ts";
const PI_EXTENSION_ASSET: &str = include_str!("assets/pi/sesh-mux-agent-state.ts");
const PI_INTEGRATION_VERSION: u32 = 9;
const CLAUDE_HOOK_INSTALL_NAME: &str = "sesh-mux-agent-state.sh";
const CLAUDE_HOOK_ASSET: &str = include_str!("assets/claude/sesh-mux-agent-state.sh");
const CLAUDE_INTEGRATION_VERSION: u32 = 11;
const CODEX_HOOK_INSTALL_NAME: &str = "sesh-mux-agent-state.sh";
const CODEX_HOOK_ASSET: &str = include_str!("assets/codex/sesh-mux-agent-state.sh");
const CODEX_INTEGRATION_VERSION: u32 = 9;

const INTEGRATION_VERSION_MARKER: &str = "SESH_MUX_INTEGRATION_VERSION=";

pub(crate) const INSTALL_WARNING_PREFIX: &str = "warning:";

#[cfg(test)]
mod test_support;
