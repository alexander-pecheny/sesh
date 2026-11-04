use std::io;

// musl's allocator unmaps freed blocks at once, and every unmap flushes the TLB on all CPUs.
#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod agent_resume;
mod api;
mod app;
mod build_info;
mod cli;
mod client;
mod config;
mod detect;
mod events;
mod ghostty;
mod handoff_runtime;
mod input;
mod integration;
mod ipc;
mod layout;
mod logging;
mod metadata_tokens;
mod noninteractive_process;
mod pane;
mod persist;
mod platform;
mod protocol;
mod pty;
mod raw_input;
mod render_signal;
mod selection;
mod server;
mod session;
mod terminal;
mod terminal_modes;
mod terminal_theme;
mod workspace;
mod worktree;

const USAGE: &str = "\
sesh-mux: a headless, scriptable terminal server for Sesh

usage: sesh-mux server                       run the server in the foreground
       sesh-mux server stop                  stop the running server
       sesh-mux server live-handoff [--import-exe PATH]
                                             hand panes over to a new server process
       sesh-mux workspace|tab|pane|agent|worktree <subcommand> ...
       sesh-mux terminal attach TERMINAL_ID [--takeover]
       sesh-mux integration install|uninstall|status [claude|codex|pi]
       sesh-mux --version

Run a group without a subcommand for its usage. Files live in ~/.sesh/mux;
SESH_MUX_SOCKET_PATH overrides the API socket.";

fn args_as_utf8<I>(args: I) -> Result<Vec<String>, String>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    args.into_iter()
        .enumerate()
        .map(|(index, arg)| {
            arg.into_string()
                .map_err(|_| format!("argument {index} is not valid UTF-8"))
        })
        .collect()
}

fn finish_cli(outcome: io::Result<cli::CommandOutcome>) -> io::Result<()> {
    match outcome {
        Ok(cli::CommandOutcome::Handled(code)) => std::process::exit(code),
        Ok(cli::CommandOutcome::NotCli) => Ok(()),
        Err(err) if cli::protocol_mismatch_was_reported(&err) => std::process::exit(1),
        Err(err) if cli::server_not_running_was_reported(&err) => {
            if let Some(response) = cli::server_not_running_reported_response(&err) {
                if let Ok(json) = serde_json::to_string(response) {
                    eprintln!("{json}");
                }
            }
            std::process::exit(1);
        }
        Err(err) => Err(err),
    }
}

fn main() -> io::Result<()> {
    let raw_args = match args_as_utf8(std::env::args_os()) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };
    let args = session::strip_session_args(&raw_args);
    finish_cli(cli::maybe_run(&args))?;

    match args.get(1).map(String::as_str) {
        Some("server") => server::headless::run_server(&args),
        Some("--version" | "-V") => {
            platform::begin_cli_output();
            println!("sesh-mux {} api {} build {}", build_info::version(), build_info::API, build_info::build());
            Ok(())
        }
        Some("--help" | "-h" | "help") | None => {
            platform::begin_cli_output();
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_as_utf8_reports_the_offending_argument_instead_of_panicking() {
        use std::os::unix::ffi::OsStringExt;
        let args = vec![
            std::ffi::OsString::from("sesh-mux"),
            std::ffi::OsString::from("pane"),
            std::ffi::OsString::from_vec(vec![0xff]),
        ];
        assert_eq!(
            args_as_utf8(args).unwrap_err(),
            "argument 2 is not valid UTF-8"
        );
    }
}
