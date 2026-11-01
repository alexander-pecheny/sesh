use serde::{Deserialize, Serialize};

use super::agents::AgentInfo;
use super::events::EventEnvelope;
use super::integrations::{
    IntegrationInstallResult, IntegrationTarget, IntegrationUninstallResult,
};
use super::panes::{PaneInfo, PaneProcessInfo, PaneReadResult};
use super::server::ServerCapabilities;
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;
use super::worktrees::{WorktreeInfo, WorktreeSourceInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SuccessResponse {
    pub id: String,
    pub result: ResponseResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorResponse {
    pub id: String,
    pub error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseResult {
    Pong {
        version: String,
        protocol: u32,
        #[serde(default)]
        capabilities: Option<ServerCapabilities>,
    },
    WorkspaceInfo {
        workspace: WorkspaceInfo,
    },
    WorkspaceCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    WorkspaceList {
        workspaces: Vec<WorkspaceInfo>,
    },
    WorktreeList {
        source: WorktreeSourceInfo,
        worktrees: Vec<WorktreeInfo>,
    },
    WorktreeCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
    },
    WorktreeOpened {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
        already_open: bool,
    },
    WorktreeRemoved {
        workspace_id: String,
        path: String,
        forced: bool,
    },
    TabInfo {
        tab: TabInfo,
    },
    TabCreated {
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    TabList {
        tabs: Vec<TabInfo>,
    },
    AgentInfo {
        agent: AgentInfo,
    },
    AgentStarted {
        agent: AgentInfo,
        argv: Vec<String>,
    },
    AgentPrompted {
        agent: AgentInfo,
    },
    AgentList {
        agents: Vec<AgentInfo>,
    },
    PaneInfo {
        pane: PaneInfo,
    },
    PaneList {
        panes: Vec<PaneInfo>,
    },
    PaneCurrent {
        pane: PaneInfo,
    },
    PaneProcessInfo {
        process_info: PaneProcessInfo,
    },
    PaneRead {
        read: PaneReadResult,
    },
    SubscriptionStarted {},
    WaitMatched {
        event: EventEnvelope,
    },
    OutputMatched {
        pane_id: String,
        revision: u64,
        matched_line: Option<String>,
        read: PaneReadResult,
    },
    IntegrationList {
        integrations: Vec<super::integrations::IntegrationInfo>,
    },
    IntegrationInstall {
        target: IntegrationTarget,
        details: IntegrationInstallResult,
    },
    IntegrationUninstall {
        target: IntegrationTarget,
        details: IntegrationUninstallResult,
    },
    Ok {},
}
