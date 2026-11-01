use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::env::*;

pub(crate) fn integration_target_label(
    target: crate::api::schema::IntegrationTarget,
) -> &'static str {
    match target {
        crate::api::schema::IntegrationTarget::Pi => "pi",
        crate::api::schema::IntegrationTarget::Claude => "claude",
        crate::api::schema::IntegrationTarget::Codex => "codex",
    }
}

pub(crate) fn integration_target_command(
    target: crate::api::schema::IntegrationTarget,
) -> &'static str {
    integration_target_command_names(target)[0]
}

pub(crate) fn integration_target_command_names(
    target: crate::api::schema::IntegrationTarget,
) -> &'static [&'static str] {
    match target {
        crate::api::schema::IntegrationTarget::Pi => &["pi"],
        crate::api::schema::IntegrationTarget::Claude => &["claude"],
        crate::api::schema::IntegrationTarget::Codex => &["codex"],
    }
}

pub(crate) fn integration_target_supported(target: crate::api::schema::IntegrationTarget) -> bool {
    {
        let _ = target;
        true
    }
}

pub(crate) fn integration_target_available(target: crate::api::schema::IntegrationTarget) -> bool {
    if !integration_target_supported(target) {
        return false;
    }

    integration_target_command_names(target)
        .iter()
        .any(|command| command_available(command))
        || integration_target_install_layout_available(target)
}

pub(crate) fn integration_target_install_layout_available(
    target: crate::api::schema::IntegrationTarget,
) -> bool {
    match target {
        crate::api::schema::IntegrationTarget::Codex => codex_standalone_binary_available(),
        _ => false,
    }
}

pub(crate) fn command_available(command: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        command_path_candidates(&dir, command)
            .into_iter()
            .any(|path| executable_file_exists(&path))
    })
}

pub(crate) fn command_path_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    let base = dir.join(command);

    {
        vec![base]
    }
}

pub(crate) fn executable_file_exists(path: &Path) -> bool {
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

pub(crate) fn codex_standalone_binary_available() -> bool {
    let Ok(releases_dir) =
        codex_dir().map(|dir| dir.join("packages").join("standalone").join("releases"))
    else {
        return false;
    };
    let Ok(entries) = fs::read_dir(releases_dir) else {
        return false;
    };

    entries.filter_map(Result::ok).any(|entry| {
        executable_file_exists(&entry.path().join("bin").join(codex_executable_name()))
    })
}

pub(crate) fn codex_executable_name() -> &'static str {
    if cfg!(windows) {
        "codex.exe"
    } else {
        "codex"
    }
}

pub(crate) fn installed_integration_statuses() -> Vec<super::IntegrationStatus> {
    integration_specs()
        .into_iter()
        .filter_map(|(target, path, expected_version)| {
            if !integration_target_supported(target) {
                return None;
            }
            Some(integration_status_at(target, path.ok()?, expected_version))
        })
        .collect()
}

pub(crate) fn integration_recommendations() -> Vec<super::IntegrationRecommendation> {
    integration_specs()
        .into_iter()
        .filter_map(|(target, path, expected_version)| {
            if !integration_target_supported(target) {
                return None;
            }
            let path = path.ok()?;
            let status = integration_status_at(target, path.clone(), expected_version);
            Some(super::IntegrationRecommendation {
                target,
                label: integration_target_label(target),
                command: integration_target_command(target),
                available: integration_target_available(target)
                    || status.state != super::IntegrationStatusKind::NotInstalled,
                path,
                state: status.state,
            })
        })
        .collect()
}

pub(crate) fn outdated_installed_integrations() -> Vec<super::IntegrationStatus> {
    installed_integration_statuses()
        .into_iter()
        .filter(|status| status.state == super::IntegrationStatusKind::Outdated)
        .collect()
}

fn integration_specs() -> [(
    crate::api::schema::IntegrationTarget,
    io::Result<PathBuf>,
    u32,
); 3] {
    [
        (
            crate::api::schema::IntegrationTarget::Pi,
            pi_extension_dir().map(|dir| dir.join(super::PI_EXTENSION_INSTALL_NAME)),
            super::PI_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Claude,
            claude_dir().map(|dir| dir.join("hooks").join(super::CLAUDE_HOOK_INSTALL_NAME)),
            super::CLAUDE_INTEGRATION_VERSION,
        ),
        (
            crate::api::schema::IntegrationTarget::Codex,
            codex_dir().map(|dir| dir.join(super::CODEX_HOOK_INSTALL_NAME)),
            super::CODEX_INTEGRATION_VERSION,
        ),
    ]
}

pub(crate) fn integration_update_instructions(
    targets: &[crate::api::schema::IntegrationTarget],
) -> String {
    let commands: Vec<String> = targets
        .iter()
        .map(|target| {
            format!(
                "`sesh-mux integration install {}`",
                integration_target_label(*target)
            )
        })
        .collect();

    match commands.as_slice() {
        [] => String::new(),
        [command] => format!("run {command}"),
        [rest @ .., last] => format!("run {} and {last}", rest.join(", ")),
    }
}

pub(crate) fn print_outdated_update_notice() -> bool {
    let outdated = outdated_installed_integrations();
    if outdated.is_empty() {
        return false;
    }

    let targets = outdated
        .iter()
        .map(|integration| integration.target)
        .collect::<Vec<_>>();
    eprintln!(
        "installed sesh-mux integrations need updating; {}.",
        integration_update_instructions(&targets).replace('`', "")
    );
    true
}

fn integration_state_for_path(
    path: &Path,
    expected_version: u32,
) -> (super::IntegrationStatusKind, Option<u32>) {
    if !path.is_file() {
        return (super::IntegrationStatusKind::NotInstalled, None);
    }

    let installed_version = fs::read_to_string(path)
        .ok()
        .and_then(|content| parse_integration_version(&content));
    let state = if installed_version.is_some_and(|version| version >= expected_version) {
        super::IntegrationStatusKind::Current
    } else {
        super::IntegrationStatusKind::Outdated
    };

    (state, installed_version)
}

pub(crate) fn integration_status_at(
    target: crate::api::schema::IntegrationTarget,
    path: PathBuf,
    expected_version: u32,
) -> super::IntegrationStatus {
    let (state, installed_version) = integration_state_for_path(&path, expected_version);

    super::IntegrationStatus {
        target,
        path,
        state,
        installed_version,
        expected_version,
    }
}

pub(crate) fn parse_integration_version(content: &str) -> Option<u32> {
    content.lines().find_map(|line| {
        let marker_line = line
            .trim()
            .trim_start_matches('/')
            .trim_start_matches('#')
            .trim();
        marker_line
            .strip_prefix(super::INTEGRATION_VERSION_MARKER)?
            .trim()
            .parse()
            .ok()
    })
}
