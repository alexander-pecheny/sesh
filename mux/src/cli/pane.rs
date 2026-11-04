fn caller_pane_id() -> Option<String> {
    std::env::var("SESH_MUX_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
}
use crate::api::schema::{
    Method, OutputMatch, PaneCurrentParams, PaneListParams, PaneProcessInfoParams, PaneReadParams,
    PaneReleaseAgentParams, PaneRenameParams, PaneReportAgentParams, PaneReportAgentSessionParams,
    PaneReportMetadataParams, PaneSendInputParams, PaneSendKeysParams, PaneSendTextParams,
    PaneTarget, PaneWaitForOutputParams, ReadFormat, ReadSource, Request,
};

pub(super) fn run_pane_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(|arg| arg.as_str()) else {
        print_pane_help();
        return Ok(2);
    };

    match subcommand {
        "list" => pane_list(&args[1..]),
        "current" => pane_current(&args[1..]),
        "get" => pane_get(&args[1..]),
        "process-info" => pane_process_info(&args[1..]),
        "read" => pane_read(&args[1..]),
        "rename" => pane_rename(&args[1..]),
        "close" => pane_close(&args[1..]),
        "send-text" => pane_send_text(&args[1..]),
        "send-keys" => pane_send_keys(&args[1..]),
        "wait-output" => pane_wait_output(&args[1..]),
        "report-agent" => pane_report_agent(&args[1..]),
        "report-agent-session" => pane_report_agent_session(&args[1..]),
        "release-agent" => pane_release_agent(&args[1..]),
        "report-metadata" => pane_report_metadata(&args[1..]),
        "run" => pane_run(&args[1..]),
        "help" | "--help" | "-h" => {
            print_pane_help();
            Ok(0)
        }
        _ => {
            print_pane_help();
            Ok(2)
        }
    }
}

fn pane_list(args: &[String]) -> std::io::Result<i32> {
    let mut workspace_id = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --workspace");
                    return Ok(2);
                };
                workspace_id = Some(super::normalize_workspace_id(value));
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    super::print_response(&super::send_request(&Request {
        id: "cli:pane:list".into(),
        method: Method::PaneList(PaneListParams { workspace_id }),
    })?)
}

fn pane_get(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_pane_id) = args.first() else {
        eprintln!("usage: sesh-mux pane get <pane_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: sesh-mux pane get <pane_id>");
        return Ok(2);
    }

    super::print_response(&super::send_request(&Request {
        id: "cli:pane:get".into(),
        method: Method::PaneGet(PaneTarget {
            pane_id: super::normalize_pane_id(raw_pane_id),
        }),
    })?)
}

fn pane_current(args: &[String]) -> std::io::Result<i32> {
    let env_pane_id = caller_pane_id();
    let caller_pane_id = match parse_pane_current_args(args, env_pane_id.as_deref()) {
        Ok(caller_pane_id) => caller_pane_id,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    super::print_response(&super::send_request(&Request {
        id: "cli:pane:current".into(),
        method: Method::PaneCurrent(PaneCurrentParams { caller_pane_id }),
    })?)
}

fn parse_pane_current_args(
    args: &[String],
    env_pane_id: Option<&str>,
) -> Result<Option<String>, String> {
    let mut caller_pane_id = env_pane_id.map(super::normalize_pane_id);
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                caller_pane_id = Some(super::normalize_pane_id(value));
                index += 2;
            }
            "--current" => {
                caller_pane_id = env_pane_id.map(super::normalize_pane_id);
                index += 1;
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(caller_pane_id)
}

fn pane_process_info(args: &[String]) -> std::io::Result<i32> {
    let pane_id = match parse_optional_current_pane_args_from_env(args) {
        Ok(pane_id) => pane_id,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    super::print_response(&super::send_request(&Request {
        id: "cli:pane:process_info".into(),
        method: Method::PaneProcessInfo(PaneProcessInfoParams { pane_id }),
    })?)
}

fn pane_rename(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_pane_id) = args.first() else {
        eprintln!("usage: sesh-mux pane rename <pane_id> <label>|--clear");
        return Ok(2);
    };
    if args.len() < 2 {
        eprintln!("usage: sesh-mux pane rename <pane_id> <label>|--clear");
        return Ok(2);
    }
    let label = if args.len() == 2 && args[1] == "--clear" {
        None
    } else {
        Some(args[1..].join(" "))
    };

    super::runtime::pane_rename(PaneRenameParams {
        pane_id: super::normalize_pane_id(raw_pane_id),
        label,
    })
}

fn pane_read(args: &[String]) -> std::io::Result<i32> {
    let params = match parse_pane_read_args(args) {
        Ok(params) => params,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    let response = super::send_request(&Request {
        id: "cli:pane:read".into(),
        method: Method::PaneRead(params),
    })?;

    super::print_read_response(&response)
}

fn parse_pane_read_args(args: &[String]) -> Result<PaneReadParams, String> {
    const USAGE: &str = "usage: sesh-mux pane read <pane_id> [--source visible|recent|recent-unwrapped|detection] [--lines N] [--format text|ansi] [--ansi] [--raw]";

    let args = super::expand_equals_args(args, &["--source", "--lines", "--format"]);
    let mut pane_id = None;
    let mut source = ReadSource::Recent;
    let mut lines = None;
    let mut format = ReadFormat::Text;
    let mut strip_ansi = true;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --source".into());
                };
                source = super::parse_read_source(value).map_err(|err| err.to_string())?;
                index += 2;
            }
            "--lines" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --lines".into());
                };
                lines =
                    Some(super::parse_u32_flag("--lines", value).map_err(|err| err.to_string())?);
                index += 2;
            }
            "--format" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --format".into());
                };
                format = super::parse_read_format(value).map_err(|err| err.to_string())?;
                index += 2;
            }
            "--ansi" => {
                format = ReadFormat::Ansi;
                index += 1;
            }
            "--raw" => {
                format = ReadFormat::Ansi;
                strip_ansi = false;
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option: {option}"));
            }
            positional => {
                if pane_id.is_some() {
                    return Err(format!("unexpected argument: {positional}"));
                }
                pane_id = Some(super::normalize_pane_id(positional));
                index += 1;
            }
        }
    }

    let Some(pane_id) = pane_id else {
        return Err(USAGE.into());
    };

    Ok(PaneReadParams {
        pane_id,
        source,
        lines,
        format,
        strip_ansi,
        intent: crate::api::schema::ReadIntent::Interactive,
    })
}

fn pane_close(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_pane_id) = args.first() else {
        eprintln!("usage: sesh-mux pane close <pane_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: sesh-mux pane close <pane_id>");
        return Ok(2);
    }

    super::runtime::pane_close(super::normalize_pane_id(raw_pane_id))
}

fn pane_send_text(args: &[String]) -> std::io::Result<i32> {
    if args.len() < 2 {
        eprintln!("usage: sesh-mux pane send-text <pane_id> <text>");
        return Ok(2);
    }

    let pane_id = super::normalize_pane_id(&args[0]);
    let text = args[1..].join(" ");
    super::send_ok_request(Method::PaneSendText(PaneSendTextParams { pane_id, text }))
}

fn pane_send_keys(args: &[String]) -> std::io::Result<i32> {
    if args.len() < 2 {
        eprintln!("usage: sesh-mux pane send-keys <pane_id> <key> [key ...]");
        return Ok(2);
    }

    let pane_id = super::normalize_pane_id(&args[0]);
    let keys = args[1..].to_vec();
    super::send_ok_request(Method::PaneSendKeys(PaneSendKeysParams { pane_id, keys }))
}

fn pane_run(args: &[String]) -> std::io::Result<i32> {
    if args.len() < 2 {
        eprintln!("usage: sesh-mux pane run <pane_id> <command>");
        return Ok(2);
    }

    let pane_id = super::normalize_pane_id(&args[0]);
    let text = args[1..].join(" ");
    super::send_ok_request(Method::PaneSendInput(PaneSendInputParams {
        pane_id,
        text,
        keys: vec!["Enter".into()],
    }))
}

fn pane_wait_output(args: &[String]) -> std::io::Result<i32> {
    let params = match parse_pane_wait_output_args(args) {
        Ok(params) => params,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    super::print_response(&super::send_request(&Request {
        id: "cli:pane:wait-output".into(),
        method: Method::PaneWaitForOutput(params),
    })?)
}

fn parse_pane_wait_output_args(args: &[String]) -> Result<PaneWaitForOutputParams, String> {
    const USAGE: &str = "usage: sesh-mux pane wait-output <pane_id> (--match TEXT | --regex PATTERN) [--source visible|recent|recent-unwrapped] [--lines N] [--timeout MS] [--raw]";

    let args = super::expand_equals_args(
        args,
        &["--match", "--regex", "--source", "--lines", "--timeout"],
    );
    let mut pane_id = None;
    let mut source = ReadSource::Recent;
    let mut lines = None;
    let mut timeout_ms = None;
    let mut strip_ansi = true;
    let mut matcher = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            option @ ("--match" | "--regex") => {
                let Some(value) = args.get(index + 1) else {
                    return Err(format!("missing value for {option}"));
                };
                if matcher.is_some() {
                    return Err("--match and --regex are mutually exclusive".into());
                }
                matcher = Some(if option == "--regex" {
                    OutputMatch::Regex {
                        value: value.clone(),
                    }
                } else {
                    OutputMatch::Substring {
                        value: value.clone(),
                    }
                });
                index += 2;
            }
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --source".into());
                };
                source = super::parse_read_source(value).map_err(|err| err.to_string())?;
                index += 2;
            }
            "--lines" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --lines".into());
                };
                lines =
                    Some(super::parse_u32_flag("--lines", value).map_err(|err| err.to_string())?);
                index += 2;
            }
            "--timeout" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --timeout".into());
                };
                timeout_ms =
                    Some(super::parse_u64_flag("--timeout", value).map_err(|err| err.to_string())?);
                index += 2;
            }
            "--raw" => {
                strip_ansi = false;
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option: {option}"));
            }
            positional => {
                if pane_id.is_some() {
                    return Err(format!("unexpected argument: {positional}"));
                }
                pane_id = Some(super::normalize_pane_id(positional));
                index += 1;
            }
        }
    }
    let Some(pane_id) = pane_id else {
        return Err(USAGE.into());
    };
    let Some(matcher) = matcher else {
        return Err("missing required --match or --regex".into());
    };
    Ok(PaneWaitForOutputParams {
        pane_id,
        source,
        lines,
        r#match: matcher,
        timeout_ms,
        strip_ansi,
    })
}

fn pane_report_agent(args: &[String]) -> std::io::Result<i32> {
    const USAGE: &str = "usage: sesh-mux pane report-agent <pane_id> --source ID --agent LABEL --state idle|working|blocked|unknown [--message TEXT] [--seq N] [--agent-session-id ID] [--agent-session-path PATH]";

    let args = super::expand_equals_args(
        args,
        &[
            "--source",
            "--agent",
            "--state",
            "--message",
            "--seq",
            "--agent-session-id",
            "--agent-session-path",
        ],
    );
    let mut pane_id = None;
    let mut source = None;
    let mut agent = None;
    let mut state = None;
    let mut message = None;
    let mut seq = None;
    let mut agent_session_id = None;
    let mut agent_session_path = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --source");
                    return Ok(2);
                };
                source = Some(value.clone());
                index += 2;
            }
            "--agent" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent");
                    return Ok(2);
                };
                agent = Some(value.clone());
                index += 2;
            }
            "--state" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --state");
                    return Ok(2);
                };
                state = Some(super::parse_pane_agent_state(value)?);
                index += 2;
            }
            "--message" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --message");
                    return Ok(2);
                };
                message = Some(value.clone());
                index += 2;
            }
            "--seq" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --seq");
                    return Ok(2);
                };
                seq = Some(super::parse_u64_flag("--seq", value)?);
                index += 2;
            }
            "--agent-session-id" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent-session-id");
                    return Ok(2);
                };
                agent_session_id = Some(value.clone());
                index += 2;
            }
            "--agent-session-path" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent-session-path");
                    return Ok(2);
                };
                agent_session_path = Some(value.clone());
                index += 2;
            }
            option if option.starts_with('-') => {
                eprintln!("unknown option: {option}");
                return Ok(2);
            }
            positional => {
                if pane_id.is_some() {
                    eprintln!("unexpected argument: {positional}");
                    return Ok(2);
                }
                pane_id = Some(super::normalize_pane_id(positional));
                index += 1;
            }
        }
    }

    let Some(pane_id) = pane_id else {
        eprintln!("{USAGE}");
        return Ok(2);
    };
    let Some(source) = source.and_then(|source| {
        let source = source.trim().to_string();
        (!source.is_empty()).then_some(source)
    }) else {
        eprintln!("missing required --source");
        return Ok(2);
    };
    let Some(agent) = agent else {
        eprintln!("missing required --agent");
        return Ok(2);
    };
    let Some(state) = state else {
        eprintln!("missing required --state");
        return Ok(2);
    };

    super::send_ok_request(Method::PaneReportAgent(PaneReportAgentParams {
        pane_id,
        source,
        agent,
        state,
        message,
        seq,
        agent_session_id,
        agent_session_path,
    }))
}

fn pane_report_agent_session(args: &[String]) -> std::io::Result<i32> {
    const USAGE: &str = "usage: sesh-mux pane report-agent-session <pane_id> --source ID --agent LABEL [--seq N] [--agent-session-id ID] [--agent-session-path PATH] [--session-start-source SOURCE]";

    let args = super::expand_equals_args(
        args,
        &[
            "--source",
            "--agent",
            "--seq",
            "--agent-session-id",
            "--agent-session-path",
            "--session-start-source",
        ],
    );
    let mut pane_id = None;
    let mut source = None;
    let mut agent = None;
    let mut seq = None;
    let mut agent_session_id = None;
    let mut agent_session_path = None;
    let mut session_start_source = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --source");
                    return Ok(2);
                };
                source = Some(value.clone());
                index += 2;
            }
            "--agent" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent");
                    return Ok(2);
                };
                agent = Some(value.clone());
                index += 2;
            }
            "--seq" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --seq");
                    return Ok(2);
                };
                seq = Some(super::parse_u64_flag("--seq", value)?);
                index += 2;
            }
            "--agent-session-id" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent-session-id");
                    return Ok(2);
                };
                agent_session_id = Some(value.clone());
                index += 2;
            }
            "--agent-session-path" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent-session-path");
                    return Ok(2);
                };
                agent_session_path = Some(value.clone());
                index += 2;
            }
            "--session-start-source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --session-start-source");
                    return Ok(2);
                };
                session_start_source = Some(value.clone());
                index += 2;
            }
            option if option.starts_with('-') => {
                eprintln!("unknown option: {option}");
                return Ok(2);
            }
            positional => {
                if pane_id.is_some() {
                    eprintln!("unexpected argument: {positional}");
                    return Ok(2);
                }
                pane_id = Some(super::normalize_pane_id(positional));
                index += 1;
            }
        }
    }

    let Some(pane_id) = pane_id else {
        eprintln!("{USAGE}");
        return Ok(2);
    };
    let Some(source) = source.and_then(|source| {
        let source = source.trim().to_string();
        (!source.is_empty()).then_some(source)
    }) else {
        eprintln!("missing required --source");
        return Ok(2);
    };
    let Some(agent) = agent else {
        eprintln!("missing required --agent");
        return Ok(2);
    };

    super::send_ok_request(Method::PaneReportAgentSession(
        PaneReportAgentSessionParams {
            pane_id,
            source,
            agent,
            seq,
            agent_session_id,
            agent_session_path,
            session_start_source,
        },
    ))
}

fn pane_release_agent(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_pane_id) = args.first() else {
        eprintln!(
            "usage: sesh-mux pane release-agent <pane_id> --source ID --agent LABEL [--seq N]"
        );
        return Ok(2);
    };

    let pane_id = super::normalize_pane_id(raw_pane_id);
    let mut source = None;
    let mut agent = None;
    let mut seq = None;

    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --source");
                    return Ok(2);
                };
                source = Some(value.clone());
                index += 2;
            }
            "--agent" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent");
                    return Ok(2);
                };
                agent = Some(value.clone());
                index += 2;
            }
            "--seq" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --seq");
                    return Ok(2);
                };
                seq = Some(super::parse_u64_flag("--seq", value)?);
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let Some(source) = source.and_then(|source| {
        let source = source.trim().to_string();
        (!source.is_empty()).then_some(source)
    }) else {
        eprintln!("missing required --source");
        return Ok(2);
    };
    let Some(agent) = agent else {
        eprintln!("missing required --agent");
        return Ok(2);
    };

    super::send_ok_request(Method::PaneReleaseAgent(PaneReleaseAgentParams {
        pane_id,
        source,
        agent,
        seq,
    }))
}

fn pane_report_metadata(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_pane_id) = args.first() else {
        eprintln!("usage: sesh-mux pane report-metadata <pane_id> --source ID [--agent LABEL] [--applies-to-source ID] [--title TEXT|--clear-title] [--display-agent TEXT|--clear-display-agent] [--state-label STATUS=TEXT] [--clear-state-labels] [--token NAME=VALUE] [--clear-token NAME] [--seq N] [--ttl-ms N]");
        return Ok(2);
    };

    let pane_id = super::normalize_pane_id(raw_pane_id);
    let mut source = None;
    let mut agent = None;
    let mut applies_to_source = None;
    let mut title = None;
    let mut display_agent = None;
    let mut state_labels = std::collections::HashMap::new();
    let mut tokens = std::collections::HashMap::new();
    let mut clear_title = false;
    let mut clear_display_agent = false;
    let mut clear_state_labels = false;
    let mut seq = None;
    let mut ttl_ms = None;

    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --source");
                    return Ok(2);
                };
                source = Some(value.clone());
                index += 2;
            }
            "--agent" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent");
                    return Ok(2);
                };
                agent = Some(value.clone());
                index += 2;
            }
            "--applies-to-source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --applies-to-source");
                    return Ok(2);
                };
                applies_to_source = Some(value.clone());
                index += 2;
            }
            "--title" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --title");
                    return Ok(2);
                };
                title = Some(value.clone());
                index += 2;
            }
            "--clear-title" => {
                clear_title = true;
                index += 1;
            }
            "--display-agent" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --display-agent");
                    return Ok(2);
                };
                display_agent = Some(value.clone());
                index += 2;
            }
            "--clear-display-agent" => {
                clear_display_agent = true;
                index += 1;
            }
            "--state-label" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --state-label");
                    return Ok(2);
                };
                let Some((status, label)) = value.split_once('=') else {
                    eprintln!("expected --state-label STATUS=TEXT");
                    return Ok(2);
                };
                let status = status.trim().to_ascii_lowercase();
                if !matches!(
                    status.as_str(),
                    "idle" | "working" | "blocked" | "done" | "unknown"
                ) {
                    eprintln!("unknown state label: {status}");
                    return Ok(2);
                }
                state_labels.insert(status, label.to_string());
                index += 2;
            }
            "--clear-state-labels" => {
                clear_state_labels = true;
                index += 1;
            }
            "--token" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --token");
                    return Ok(2);
                };
                let (key, value) = match super::parse_token_assignment(value) {
                    Ok(token) => token,
                    Err(message) => {
                        eprintln!("{message}");
                        return Ok(2);
                    }
                };
                tokens.insert(key, value);
                index += 2;
            }
            "--clear-token" => {
                let Some(key) = args.get(index + 1) else {
                    eprintln!("missing value for --clear-token");
                    return Ok(2);
                };
                tokens.insert(key.clone(), None);
                index += 2;
            }
            "--seq" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --seq");
                    return Ok(2);
                };
                seq = Some(super::parse_u64_flag("--seq", value)?);
                index += 2;
            }
            "--ttl-ms" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --ttl-ms");
                    return Ok(2);
                };
                ttl_ms = Some(super::parse_u64_flag("--ttl-ms", value)?);
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let Some(source) = source.and_then(|source| {
        let source = source.trim().to_string();
        (!source.is_empty()).then_some(source)
    }) else {
        eprintln!("missing required --source");
        return Ok(2);
    };
    if applies_to_source
        .as_deref()
        .is_some_and(|source| source.trim().is_empty())
    {
        eprintln!("missing value for --applies-to-source");
        return Ok(2);
    }
    if title.is_some() && clear_title
        || display_agent.is_some() && clear_display_agent
        || !state_labels.is_empty() && clear_state_labels
    {
        eprintln!("cannot set and clear the same metadata field");
        return Ok(2);
    }
    if title.is_none()
        && display_agent.is_none()
        && state_labels.is_empty()
        && tokens.is_empty()
        && !clear_title
        && !clear_display_agent
        && !clear_state_labels
    {
        eprintln!("missing metadata field to set or clear");
        return Ok(2);
    }

    super::send_ok_request(Method::PaneReportMetadata(PaneReportMetadataParams {
        pane_id,
        source,
        agent,
        applies_to_source,
        title,
        display_agent,
        state_labels,
        tokens,
        clear_title,
        clear_display_agent,
        clear_state_labels,
        seq,
        ttl_ms,
    }))
}

fn print_pane_help() {
    eprintln!("sesh-mux pane commands:");
    eprintln!("  sesh-mux pane list [--workspace <workspace_id>]");
    eprintln!("  sesh-mux pane current [--pane ID|--current]");
    eprintln!("  sesh-mux pane get <pane_id>");
    eprintln!("  sesh-mux pane process-info [--pane ID|--current]");
    eprintln!("  sesh-mux pane rename <pane_id> <label>|--clear");
    eprintln!("  sesh-mux pane read <pane_id> [--source visible|recent|recent-unwrapped] [--lines N] [--format text|ansi] [--ansi]");
    eprintln!("  sesh-mux pane close <pane_id>");
    eprintln!("  sesh-mux pane send-text <pane_id> <text>");
    eprintln!("  sesh-mux pane send-keys <pane_id> <key> [key ...]");
    eprintln!("  sesh-mux pane wait-output <pane_id> (--match TEXT | --regex PATTERN) [--source visible|recent|recent-unwrapped] [--lines N] [--timeout MS] [--raw]");
    eprintln!("  sesh-mux pane report-agent <pane_id> --source ID --agent LABEL --state idle|working|blocked|unknown [--message TEXT] [--seq N] [--agent-session-id ID] [--agent-session-path PATH]");
    eprintln!("  sesh-mux pane report-agent-session <pane_id> --source ID --agent LABEL [--seq N] [--agent-session-id ID] [--agent-session-path PATH]");
    eprintln!("  sesh-mux pane release-agent <pane_id> --source ID --agent LABEL [--seq N]");
    eprintln!("  sesh-mux pane report-metadata <pane_id> --source ID [--agent LABEL] [--applies-to-source ID] [--title TEXT|--clear-title] [--display-agent TEXT|--clear-display-agent] [--state-label STATUS=TEXT] [--clear-state-labels] [--token NAME=VALUE] [--clear-token NAME] [--seq N] [--ttl-ms N]");
    eprintln!("  sesh-mux pane run <pane_id> <command>");
}

fn parse_optional_current_pane_args_from_env(args: &[String]) -> Result<Option<String>, String> {
    let env_pane_id = caller_pane_id();
    parse_optional_current_pane_args(args, env_pane_id.as_deref())
}

fn parse_optional_current_pane_args(
    args: &[String],
    env_pane_id: Option<&str>,
) -> Result<Option<String>, String> {
    let mut pane_id = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                pane_id = Some(super::normalize_pane_id(value));
                index += 2;
            }
            "--current" => {
                pane_id = env_pane_id.map(super::normalize_pane_id);
                index += 1;
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(pane_id)
}

#[cfg(test)]
mod tests {}
