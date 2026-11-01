use crate::api::schema::IntegrationTarget;

const USAGE: &str = "usage: sesh-mux integration install|uninstall <pi|claude|codex>\n       sesh-mux integration status [--outdated-only]";

pub(super) fn run_integration_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("install") => integration_change(&args[1..], crate::integration::install_target),
        Some("uninstall") => integration_change(&args[1..], crate::integration::uninstall_target),
        Some("status") => integration_status(&args[1..]),
        Some("help" | "--help" | "-h") => {
            eprintln!("{USAGE}");
            Ok(0)
        }
        _ => {
            eprintln!("{USAGE}");
            Ok(2)
        }
    }
}

fn integration_status(args: &[String]) -> std::io::Result<i32> {
    let outdated_only = match args {
        [] => false,
        [flag] if flag == "--outdated-only" => true,
        _ => {
            eprintln!("{USAGE}");
            return Ok(2);
        }
    };
    if outdated_only {
        crate::integration::print_outdated_update_notice();
        return Ok(0);
    }
    for status in crate::integration::installed_integration_statuses() {
        let target = crate::integration::integration_target_label(status.target);
        let state = describe_integration_state(
            status.state,
            status.installed_version,
            status.expected_version,
        );
        println!("{target}: {state} ({})", status.path.display());
    }
    Ok(0)
}

fn describe_integration_state(
    state: crate::integration::IntegrationStatusKind,
    installed_version: Option<u32>,
    expected_version: u32,
) -> String {
    let version = match installed_version {
        Some(version) => format!("v{version}"),
        None => "legacy".to_string(),
    };
    match state {
        crate::integration::IntegrationStatusKind::NotInstalled => "not installed".to_string(),
        crate::integration::IntegrationStatusKind::Current => format!("current ({version})"),
        crate::integration::IntegrationStatusKind::Outdated
            if installed_version.is_some_and(|installed| installed >= expected_version) =>
        {
            format!("needs repair ({version})")
        }
        crate::integration::IntegrationStatusKind::Outdated => {
            format!("outdated ({version} < v{expected_version})")
        }
    }
}

fn integration_change(
    args: &[String],
    change: fn(IntegrationTarget) -> std::io::Result<Vec<String>>,
) -> std::io::Result<i32> {
    let target = match args {
        [target] if target == "pi" => IntegrationTarget::Pi,
        [target] if target == "claude" => IntegrationTarget::Claude,
        [target] if target == "codex" => IntegrationTarget::Codex,
        _ => {
            eprintln!("{USAGE}");
            return Ok(2);
        }
    };
    match change(target) {
        Ok(messages) => {
            for message in messages {
                println!("{message}");
            }
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}
