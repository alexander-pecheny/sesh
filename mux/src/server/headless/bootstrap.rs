use super::*;

/// `sesh-mux server [--handoff-import SOCKET TOKEN]`.
pub fn run_server(args: &[String]) -> io::Result<()> {
    let handoff_import = args.get(2).map(String::as_str) == Some("--handoff-import");
    if let Err(err) = crate::platform::prepare_server_process(handoff_import) {
        eprintln!("sesh-mux: could not select a persistent service context: {err}");
    }
    crate::logging::init_file_logging("sesh-mux-server.log");
    crate::platform::raise_server_nofile_limit();

    if handoff_import {
        let socket_path = args
            .get(3)
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing handoff socket"))?;
        let token = args
            .get(4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing handoff token"))?;
        return run_handoff_import_server(&socket_path, token);
    }

    let loaded_config = config::Config::load();
    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_hub = api::EventHub::default();
    let should_quit = Arc::new(AtomicBool::new(false));
    let api_server = match api::start_server_with_stop_control(
        api_tx.clone(),
        event_hub.clone(),
        should_quit.clone(),
    ) {
        Ok(server) => server,
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            eprintln!(
                "error: sesh-mux server is already running at {}",
                api::socket_path().display()
            );
            std::process::exit(1);
        }
        Err(err) => return Err(err),
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;
    let result = rt.block_on(async {
        let app = app::App::new(
            &loaded_config.config,
            app::AppPolicy::PRODUCTION,
            api_rx,
            event_hub,
        );
        let mut server =
            HeadlessServer::new(app, Some(api_tx.clone()), Some(api_server), should_quit)?;
        info!(api_socket = %api::socket_path().display(), "sesh-mux server started");
        print_ready_message();
        server.run().await
    });
    rt.shutdown_timeout(Duration::from_millis(100));
    crate::logging::shutdown("server");
    result
}

fn run_handoff_import_server(socket_path: &Path, token: &str) -> io::Result<()> {
    let loaded_config = config::Config::load();
    let mut received = crate::server::handoff::receive(socket_path, token)?;
    crate::server::handoff::log_import_result(received.manifest.panes.len());
    let from_legacy = crate::server::handoff::is_legacy_manifest(&received.manifest);
    let legacy = if from_legacy {
        legacy::LegacyListener::beside_handoff_socket(socket_path)
    } else {
        legacy::LegacyListener::at(received.manifest.legacy_socket.take())
    };

    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_hub = api::EventHub::default();
    let should_quit = Arc::new(AtomicBool::new(false));
    let mut imports = HashMap::new();
    for (pane, fd) in std::mem::take(&mut received.manifest.panes)
        .into_iter()
        .zip(received.fds)
    {
        imports.insert(
            pane.pane_id,
            crate::handoff_runtime::ImportedHandoffRuntime {
                master_fd: fd,
                state: pane,
            },
        );
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;
    let result = rt.block_on(async {
        let mut app = app::App::new_from_handoff(
            &loaded_config.config,
            api_rx,
            event_hub.clone(),
            &received.manifest.snapshot,
            &mut imports,
        )?;
        if from_legacy {
            app.mark_imported_agents_legacy();
        }
        crate::server::handoff::report_restored(&mut received.stream)?;
        if std::env::var("SESH_MUX_TEST_HANDOFF_IMPORT_FAIL").as_deref() == Ok("after_restored") {
            return Err(io::Error::other(
                "test handoff import failure after restored",
            ));
        }
        wait_for_old_public_sockets_to_close(Duration::from_secs(5))?;
        let api_server = api::start_server_with_stop_control(
            api_tx.clone(),
            event_hub.clone(),
            should_quit.clone(),
        )?;
        let mut server =
            HeadlessServer::new(app, Some(api_tx.clone()), Some(api_server), should_quit)?;
        server.legacy = legacy;
        crate::server::handoff::report_ready(&mut received.stream)?;
        crate::server::handoff::wait_committed(&mut received.stream)?;
        server.app.assume_handoff_ownership();
        server.app.state.mark_session_dirty();
        server.app.unpause_handoff_readers();
        server.pending_handoff_repaint_nudge = true;
        if let Err(err) = crate::server::handoff::report_owned(&mut received.stream) {
            warn!(err = %err, "failed to report handoff ownership; continuing as owner");
        }
        info!(from_legacy, "handoff import server started");
        print_ready_message();
        server.run().await
    });
    rt.shutdown_timeout(Duration::from_millis(100));
    crate::logging::shutdown("server");
    result
}

fn print_ready_message() {
    eprintln!("sesh-mux server running");
    eprintln!("api socket: {}", api::socket_path().display());
    eprintln!("client socket: {}", client_socket_path().display());
    eprintln!(
        "logs: {}",
        crate::session::data_dir()
            .join("sesh-mux-server.log")
            .display()
    );
}
