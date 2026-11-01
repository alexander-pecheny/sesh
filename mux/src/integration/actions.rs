use std::io;

use super::registry::{integration_target_label, integration_target_supported};
use super::targets::{
    install_claude, install_codex, install_pi, uninstall_claude, uninstall_codex, uninstall_pi,
};
use super::version::{agent_version_requirement, enforce_agent_version};

pub(crate) fn install_target(
    target: crate::api::schema::IntegrationTarget,
) -> io::Result<Vec<String>> {
    let result = install_target_inner(target);
    let outcome = if result.is_ok() { "ok" } else { "error" };
    crate::logging::integration_action("install", integration_target_label(target), outcome);
    result
}

fn install_target_inner(target: crate::api::schema::IntegrationTarget) -> io::Result<Vec<String>> {
    if !integration_target_supported(target) {
        return Err(io::Error::other(format!(
            "{} integration is not supported on Windows",
            integration_target_label(target)
        )));
    }

    let version_warning = match agent_version_requirement(target) {
        Some(requirement) => enforce_agent_version(&requirement)?,
        None => None,
    };

    let mut messages = match target {
        crate::api::schema::IntegrationTarget::Pi => {
            let path = install_pi()?;
            vec![format!("installed pi integration to {}", path.display())]
        }
        crate::api::schema::IntegrationTarget::Claude => {
            let installed = install_claude()?;
            vec![
                format!(
                    "installed claude integration hook to {}",
                    installed.hook_path.display()
                ),
                format!(
                    "ensured claude settings at {}",
                    installed.settings_path.display()
                ),
            ]
        }
        crate::api::schema::IntegrationTarget::Codex => {
            let installed = install_codex()?;
            vec![
                format!(
                    "installed codex integration hook to {}",
                    installed.hook_path.display()
                ),
                format!("ensured codex hooks at {}", installed.hooks_path.display()),
                format!(
                    "ensured codex config at {}",
                    installed.config_path.display()
                ),
            ]
        }
    };

    if let Some(warning) = version_warning {
        messages.push(warning);
    }

    Ok(messages)
}

pub(crate) fn uninstall_target(
    target: crate::api::schema::IntegrationTarget,
) -> io::Result<Vec<String>> {
    let messages = match target {
        crate::api::schema::IntegrationTarget::Pi => {
            let result = uninstall_pi()?;
            if result.removed_extension {
                vec![format!(
                    "removed pi integration extension at {}",
                    result.extension_path.display()
                )]
            } else {
                vec![format!(
                    "no pi integration extension found at {}",
                    result.extension_path.display()
                )]
            }
        }
        crate::api::schema::IntegrationTarget::Claude => {
            let result = uninstall_claude()?;
            let mut messages = Vec::new();
            if result.removed_hook_file {
                messages.push(format!(
                    "removed claude hook at {}",
                    result.hook_path.display()
                ));
            } else {
                messages.push(format!(
                    "no claude hook found at {}",
                    result.hook_path.display()
                ));
            }
            if result.updated_settings {
                messages.push(format!(
                    "removed sesh-mux claude hook entries from {}",
                    result.settings_path.display()
                ));
            } else {
                messages.push(format!(
                    "no sesh-mux claude hook entries found in {}",
                    result.settings_path.display()
                ));
            }
            messages
        }
        crate::api::schema::IntegrationTarget::Codex => {
            let result = uninstall_codex()?;
            let mut messages = Vec::new();
            if result.removed_hook_file {
                messages.push(format!(
                    "removed codex hook at {}",
                    result.hook_path.display()
                ));
            } else {
                messages.push(format!(
                    "no codex hook found at {}",
                    result.hook_path.display()
                ));
            }
            if result.updated_hooks {
                messages.push(format!(
                    "removed sesh-mux codex hook entries from {}",
                    result.hooks_path.display()
                ));
            } else {
                messages.push(format!(
                    "no sesh-mux codex hook entries found in {}",
                    result.hooks_path.display()
                ));
            }
            messages.push(format!(
                "left codex config unchanged at {}",
                result.config_path.display()
            ));
            messages
        }
    };

    crate::logging::integration_action("uninstall", integration_target_label(target), "ok");
    Ok(messages)
}
