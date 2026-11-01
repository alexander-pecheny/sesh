use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::claude_settings::{
    install as install_claude_settings, uninstall as uninstall_claude_settings,
};
use super::command::hook_command;
use super::config_edit::{
    build_codex_config_with_hooks, ensure_command_hook, ensure_hooks_object,
    hooks_object_if_present, remove_hook_commands,
};
use super::config_file::{check_config_targets, write_config};
use super::env::{claude_dir, codex_dir, pi_extension_dir};
use super::file_ops::{make_executable, remove_file_if_exists, remove_legacy_bash_hook_file};
use super::types::{
    ClaudeInstallPaths, ClaudeUninstallResult, CodexInstallPaths, CodexUninstallResult,
    PiUninstallResult,
};
use super::{
    CLAUDE_HOOK_ASSET, CLAUDE_HOOK_INSTALL_NAME, CODEX_HOOK_ASSET, CODEX_HOOK_INSTALL_NAME,
    PI_EXTENSION_ASSET, PI_EXTENSION_INSTALL_NAME,
};

fn ensure_extension_dir(dir: &Path, agent: &str) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    if dir.parent().is_some_and(|parent| parent.is_dir()) {
        return fs::create_dir_all(dir);
    }
    Err(io::Error::other(format!(
        "{agent} extension directory not found at {}. install {agent} first",
        dir.display()
    )))
}

pub(crate) fn install_pi() -> io::Result<PathBuf> {
    let dir = pi_extension_dir()?;
    ensure_extension_dir(&dir, "pi")?;

    let path = dir.join(PI_EXTENSION_INSTALL_NAME);
    fs::write(&path, PI_EXTENSION_ASSET)?;
    Ok(path)
}

pub(crate) fn install_claude() -> io::Result<ClaudeInstallPaths> {
    let dir = claude_dir()?;
    check_config_targets(&dir, &["settings.json"])?;
    if !dir.is_dir() {
        return Err(io::Error::other(format!(
            "claude directory not found at {}. install claude code first",
            dir.display()
        )));
    }

    let hooks_dir = dir.join("hooks");
    fs::create_dir_all(&hooks_dir)?;

    let hook_path = hooks_dir.join(CLAUDE_HOOK_INSTALL_NAME);
    fs::write(&hook_path, CLAUDE_HOOK_ASSET)?;
    make_executable(&hook_path)?;

    let settings_path = dir.join("settings.json");
    let existing_settings = if settings_path.is_file() {
        fs::read_to_string(&settings_path)?
    } else {
        "{}".to_string()
    };
    let updated_settings = install_claude_settings(&existing_settings, &settings_path, &hook_path)?;
    remove_legacy_bash_hook_file(&hook_path)?;

    if updated_settings != existing_settings {
        write_config(&settings_path, updated_settings)?;
    }

    Ok(ClaudeInstallPaths {
        hook_path,
        settings_path,
    })
}

pub(crate) fn install_codex() -> io::Result<CodexInstallPaths> {
    let dir = codex_dir()?;
    check_config_targets(&dir, &["hooks.json", "config.toml"])?;
    if !dir.is_dir() {
        return Err(io::Error::other(format!(
            "codex config directory not found at {}. install codex first",
            dir.display()
        )));
    }

    let hook_path = dir.join(CODEX_HOOK_INSTALL_NAME);
    fs::write(&hook_path, CODEX_HOOK_ASSET)?;
    make_executable(&hook_path)?;

    let hooks_path = dir.join("hooks.json");
    let mut hooks_file = if hooks_path.is_file() {
        serde_json::from_str::<Value>(&fs::read_to_string(&hooks_path)?).map_err(|err| {
            io::Error::other(format!("failed to parse {}: {err}", hooks_path.display()))
        })?
    } else {
        json!({})
    };

    let hooks = ensure_hooks_object(
        &mut hooks_file,
        &hooks_path,
        "codex hooks file",
        "codex hooks file hooks",
    )?;
    remove_hook_commands(hooks, "PermissionRequest", &hook_path, Some("blocked"))?;
    remove_hook_commands(hooks, "SessionStart", &hook_path, Some("idle"))?;
    remove_hook_commands(hooks, "UserPromptSubmit", &hook_path, Some("working"))?;
    remove_hook_commands(hooks, "PreToolUse", &hook_path, Some("working"))?;
    remove_hook_commands(hooks, "Stop", &hook_path, Some("idle"))?;
    remove_hook_commands(hooks, "SessionStart", &hook_path, Some("session"))?;
    ensure_command_hook(
        hooks,
        "SessionStart",
        hook_command(&hook_path, Some("session")),
        10,
        None,
    )?;
    ensure_command_hook(
        hooks,
        "PermissionRequest",
        hook_command(&hook_path, Some("permission")),
        10,
        None,
    )?;
    remove_legacy_bash_hook_file(&hook_path)?;

    write_config(&hooks_path, serde_json::to_string_pretty(&hooks_file)?)?;

    let config_path = dir.join("config.toml");
    let existing_config = if config_path.is_file() {
        fs::read_to_string(&config_path)?
    } else {
        String::new()
    };
    let new_config = build_codex_config_with_hooks(&existing_config);
    if new_config != existing_config {
        write_config(&config_path, new_config)?;
    }

    Ok(CodexInstallPaths {
        hook_path,
        hooks_path,
        config_path,
    })
}

pub(crate) fn uninstall_pi() -> io::Result<PiUninstallResult> {
    let extension_path = pi_extension_dir()?.join(PI_EXTENSION_INSTALL_NAME);
    let removed_extension = remove_file_if_exists(&extension_path)?;

    Ok(PiUninstallResult {
        extension_path,
        removed_extension,
    })
}

pub(crate) fn uninstall_claude() -> io::Result<ClaudeUninstallResult> {
    let dir = claude_dir()?;
    check_config_targets(&dir, &["settings.json"])?;
    let hook_path = dir.join("hooks").join(CLAUDE_HOOK_INSTALL_NAME);
    let settings_path = dir.join("settings.json");
    let mut updated_settings = false;

    if settings_path.is_file() {
        let existing_settings = fs::read_to_string(&settings_path)?;
        let new_settings =
            uninstall_claude_settings(&existing_settings, &settings_path, &hook_path)?;
        updated_settings = new_settings != existing_settings;
        if updated_settings {
            write_config(&settings_path, new_settings)?;
        }
    }

    let removed_hook_file =
        remove_file_if_exists(&hook_path)? | remove_legacy_bash_hook_file(&hook_path)?;

    Ok(ClaudeUninstallResult {
        hook_path,
        settings_path,
        removed_hook_file,
        updated_settings,
    })
}

pub(crate) fn uninstall_codex() -> io::Result<CodexUninstallResult> {
    let codex_dir = codex_dir()?;
    check_config_targets(&codex_dir, &["hooks.json"])?;
    let hook_path = codex_dir.join(CODEX_HOOK_INSTALL_NAME);
    let hooks_path = codex_dir.join("hooks.json");
    let config_path = codex_dir.join("config.toml");
    let mut updated_hooks = false;

    if hooks_path.is_file() {
        let mut hooks_file = serde_json::from_str::<Value>(&fs::read_to_string(&hooks_path)?)
            .map_err(|err| {
                io::Error::other(format!("failed to parse {}: {err}", hooks_path.display()))
            })?;

        if let Some(hooks) = hooks_object_if_present(
            &mut hooks_file,
            &hooks_path,
            "codex hooks file",
            "codex hooks file hooks",
        )? {
            updated_hooks |= remove_hook_commands(hooks, "SessionStart", &hook_path, Some("idle"))?;
            updated_hooks |=
                remove_hook_commands(hooks, "SessionStart", &hook_path, Some("session"))?;
            updated_hooks |=
                remove_hook_commands(hooks, "UserPromptSubmit", &hook_path, Some("working"))?;
            updated_hooks |=
                remove_hook_commands(hooks, "PreToolUse", &hook_path, Some("working"))?;
            updated_hooks |=
                remove_hook_commands(hooks, "PermissionRequest", &hook_path, Some("blocked"))?;
            updated_hooks |=
                remove_hook_commands(hooks, "PermissionRequest", &hook_path, Some("permission"))?;
            updated_hooks |= remove_hook_commands(hooks, "Stop", &hook_path, Some("idle"))?;
        }

        if updated_hooks {
            write_config(&hooks_path, serde_json::to_string_pretty(&hooks_file)?)?;
        }
    }

    let removed_hook_file =
        remove_file_if_exists(&hook_path)? | remove_legacy_bash_hook_file(&hook_path)?;

    Ok(CodexUninstallResult {
        hook_path,
        hooks_path,
        config_path,
        removed_hook_file,
        updated_hooks,
    })
}
