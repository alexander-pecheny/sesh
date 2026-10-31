use super::*;

#[test]
fn startup_onboarding_is_client_rendered_and_modal() {
    let config = ClientShellConfig::from_config(&Config::default()).with_startup_onboarding(true);
    let mut state = ClientShellState::new(config);
    let early = state.handle_input_bytes(b"\r");
    assert!(early.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Onboarding)
    ));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    let frame = state.compose(106, 20).expect("onboarding frame");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("terminal workspace manager for coding agents"));
    assert!(text.contains("this is a mouse-first terminal"));
    assert!(text.contains("ctrl+b enters prefix mode"));
    assert!(text.contains("install optional agent integrations"));
    assert_eq!(state.hits.overlay_primary.width, 12);

    let ignored = state.handle_input_bytes(b"x");
    assert!(ignored.requests.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Onboarding)
    ));
    let outside =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })]);
    assert!(outside.actions.is_empty());
    assert!(outside.requests.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Onboarding)
    ));

    state.set_pane_surface(surface_with_popup());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Onboarding)
    ));
    let popup_input = state.handle_input_bytes(b"hidden-popup-input");
    assert!(popup_input.requests.is_empty());
    let popup_paste = state.handle_raw_events(vec![RawInputEvent::Paste("secret".into())]);
    assert!(popup_paste.requests.is_empty());
}

#[test]
fn onboarding_completion_persists_and_opens_endpoint_integrations() {
    let path = std::env::temp_dir().join(format!(
        "herdr-client-onboarding-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::write(&path, "[terminal]\ndefault_shell = \"fish\"\n")
        .expect("write onboarding config");
    let onboarding_config = || {
        let mut config =
            ClientShellConfig::from_config(&Config::default()).with_startup_onboarding(true);
        config.local_config_path = path.clone();
        config
    };

    let config = onboarding_config();
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let outcome = state.handle_input_bytes(b"\r");

    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Integrations,
            loading_integrations: true,
            ..
        }))
    ));
    assert!(matches!(
        &outcome.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(request.method, crate::api::schema::Method::IntegrationList(_))
    ));
    let persisted = std::fs::read_to_string(&path).expect("read onboarding config");
    assert!(persisted.contains("onboarding = false"));
    assert!(persisted.contains("default_shell = \"fish\""));

    for input in [b"\x1b[C".as_slice(), b"l".as_slice()] {
        let config = onboarding_config();
        let mut state = ClientShellState::new(config);
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(surface());
        let outcome = state.handle_input_bytes(input);
        assert!(matches!(
            state.overlay,
            Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
                section: ClientSettingsSection::Integrations,
                ..
            }))
        ));
        assert_eq!(outcome.actions.len(), 1);
    }

    let config = onboarding_config();
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("onboarding mouse frame");
    let button = state.hits.overlay_primary;
    let click = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: button.x,
        row: button.y,
        modifiers: KeyModifiers::NONE,
    })]);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Integrations,
            ..
        }))
    ));
    assert_eq!(click.actions.len(), 1);

    let unreadable_path = path.with_extension("dir");
    std::fs::create_dir(&unreadable_path).expect("create unreadable config path");
    let mut config = onboarding_config();
    config.local_config_path = unreadable_path.clone();
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let failed_write = state.handle_input_bytes(b"\r");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Integrations,
            ..
        }))
    ));
    assert_eq!(failed_write.actions.len(), 1);
    assert!(state
        .config_diagnostic
        .as_deref()
        .is_some_and(|diagnostic| diagnostic.contains("failed to read config")));
    assert!(unreadable_path.is_dir());
    std::fs::remove_dir(&unreadable_path).expect("remove unreadable config path");

    std::fs::remove_file(path).expect("remove onboarding config");
}

#[test]
fn unavailable_integration_list_does_not_wedge_settings() {
    let mut config =
        ClientShellConfig::from_config(&Config::default()).with_startup_onboarding(true);
    config.local_config_path = std::env::temp_dir().join(format!(
        "herdr-client-onboarding-unavailable-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(Vec::new()));

    let outcome = state.handle_input_bytes(b"\r");

    assert!(outcome.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Integrations,
            loading_integrations: false,
            ..
        }))
    ));
    assert!(state
        .visible_endpoint_notice
        .as_ref()
        .is_some_and(|notice| notice.key.code == "integration.list"));
    let _ = std::fs::remove_file(&state.config.local_config_path);
}

#[test]
fn startup_config_diagnostics_are_client_rendered_and_persist_until_replaced() {
    let config = ClientShellConfig::from_config(&Config::default())
        .with_startup_config_diagnostic(Some("local config warning".into()));
    let mut state = ClientShellState::new(config);
    let mut shared_snapshot = snapshot();
    shared_snapshot.config_diagnostic = Some("local config warning".into());
    state.set_snapshot(Box::new(shared_snapshot));
    assert_eq!(
        state.config_diagnostic.as_deref(),
        Some("client + endpoint: local config warning")
    );

    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("endpoint config warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());

    let frame = state.compose(106, 20).expect("diagnostic frame");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("client: local config warning"));
    assert!(text.contains("endpoint: endpoint config warning"));

    state.handle_input_bytes(b"x");
    assert!(state.config_diagnostic.is_some());

    state.set_snapshot(Box::new(snapshot()));
    assert_eq!(
        state.config_diagnostic.as_deref(),
        Some("local config warning")
    );
}

#[test]
fn config_diagnostic_offsets_only_the_pane_rows_it_overlaps() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.toast_delay_seconds = 0;
    let mut state = ClientShellState::new(config);
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("one-line warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());
    state.visible_notification = Some(ClientVisibleNotification {
        endpoint_id: ClientEndpointId::Local,
        event: SemanticNotification {
            kind: SemanticNotificationKind::Custom,
            title: "notification".into(),
            body: None,
            sound: None,
            agent: None,
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            position: Some(crate::config::ToastHerdrPosition::TopRight),
        },
        deadline: std::time::Instant::now(),
    });

    state.compose(106, 20).expect("one-line frame");
    let pane_area = state.layout(106, 20).pane_surface;
    assert_eq!(state.hits.notification_toast.y, pane_area.y);
    let targetless_hit = state.hits.notification_toast;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: targetless_hit.x,
        row: targetless_hit.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(state.visible_notification.is_some());

    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("first warning\nsecond warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.compose(106, 20).expect("two-line frame");
    assert_eq!(state.hits.notification_toast.y, pane_area.y);

    state
        .visible_notification
        .as_mut()
        .expect("visible notification")
        .event
        .position = Some(crate::config::ToastHerdrPosition::BottomRight);
    state.compose(106, 20).expect("bottom notification frame");
    assert_eq!(state.hits.notification_toast.bottom(), 19);
}

#[test]
fn endpoint_reload_result_does_not_override_snapshot_diagnostic_authority() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.config_diagnostic = Some("endpoint warning".into());
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.pending_requests.insert(
        "reload-1".into(),
        PendingEndpointRequest {
            boot_id: "boot-1".into(),
            method_name: "server.reload_config".into(),
            confirmation_workspace_id: None,
            kind: PendingEndpointKind::ReloadConfig,
        },
    );

    state.handle_endpoint_result(
        "boot-1",
        "reload-1",
        Ok(crate::api::schema::ResponseResult::ConfigReload {
            status: crate::config::ConfigReloadStatus::Partial,
            diagnostics: vec!["keybinding warning".into()],
        }),
    );
    assert_eq!(state.config_diagnostic.as_deref(), Some("endpoint warning"));

    state.set_snapshot(Box::new(snapshot()));
    assert!(state.config_diagnostic.is_none());
}

#[test]
fn live_client_config_keeps_sound_diagnostics() {
    let mut shell_config = ClientShellConfig::from_config(&Config::default());
    let mut config = Config::default();
    config.ui.sound.path = Some(std::path::PathBuf::from("invalid.wav"));

    let diagnostics = shell_config.apply_live_config(&config, &[], &[]);
    assert!(diagnostics
        .iter()
        .any(|diagnostic| diagnostic.contains("expected an mp3 file")));
}

#[test]
fn outdated_integration_badges_launcher_settings_and_settings_tab() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut endpoint_snapshot = snapshot();
    endpoint_snapshot.integration_updates_available = true;
    state.set_snapshot(Box::new(endpoint_snapshot));
    state.set_pane_surface(surface());
    let shell = state.compose(106, 30).expect("integration attention shell");
    assert_eq!(state.hits.global_launcher.width, 8);
    let shell_text = shell
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(shell_text.contains("● menu"));

    state.toggle_global_menu();
    let menu = state.compose(106, 30).expect("integration attention menu");
    let menu_text = menu
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(menu_text.contains("● settings"));
    assert!(!menu_text.contains("update ready"));

    state.activate_global_menu_item(0, &mut ClientShellInput::default());
    let settings = state.compose(106, 30).expect("settings integration badge");
    let settings_text = settings
        .cells
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(settings_text.contains("● integrations"));
    let integrations_tab = state
        .hits
        .settings_tabs
        .iter()
        .find(|(_, section)| *section == ClientSettingsSection::Integrations)
        .map(|(rect, _)| *rect)
        .expect("integrations tab");
    let settings_buffer = settings.to_ratatui_buffer().expect("settings buffer");
    assert_eq!(
        settings_buffer[(integrations_tab.x + 1, integrations_tab.y)].fg,
        state.config.palette.accent
    );
    assert_eq!(
        settings_buffer[(integrations_tab.x + 3, integrations_tab.y)].fg,
        state.config.palette.overlay1
    );
}

#[test]
fn client_settings_preview_restore_and_endpoint_integrations_are_owned_by_overlay() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
        highlighted: 0,
    }));
    let open = state.handle_input_bytes(b"\r");
    assert!(open.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Theme,
            ..
        }))
    ));
    let original_theme = state.config.theme_name.clone();
    let original_palette = state.config.palette.clone();
    state.handle_input_bytes(b"j");
    assert_ne!(state.config.theme_name, original_theme);
    assert_ne!(state.config.palette.accent, original_palette.accent);
    state.handle_input_bytes(b"\x1b");
    assert!(state.overlay.is_none());
    assert_eq!(state.config.theme_name, original_theme);
    assert_eq!(state.config.palette.accent, original_palette.accent);

    state.open_settings_overlay();
    state.handle_input_bytes(b"j");
    state.handle_input_bytes(b"\t");
    state
        .compose(106, 30)
        .expect("settings outside-click geometry");
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(state.overlay.is_none());
    assert_eq!(state.config.theme_name, original_theme);
    assert_eq!(state.config.palette.accent, original_palette.accent);

    state.open_settings_overlay();
    state.compose(106, 30).expect("settings overlay");
    for _ in 0..3 {
        let next = state.handle_input_bytes(b"\t");
        assert!(next.actions.is_empty());
    }
    let integrations = state.handle_input_bytes(b"\t");
    let [ClientShellAction::Endpoint { request, .. }] = &integrations.actions[..] else {
        panic!("integration section should request endpoint status");
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::IntegrationList(_)
    ));
    let request_id = request.id.clone();
    assert!(
        state
            .handle_endpoint_result(
                "boot-1",
                &request_id,
                Ok(crate::api::schema::ResponseResult::IntegrationList {
                    integrations: vec![
                        crate::api::schema::IntegrationInfo {
                            target: crate::api::schema::IntegrationTarget::Codex,
                            label: "codex".into(),
                            command: "codex".into(),
                            available: true,
                            state: crate::api::schema::IntegrationState::Outdated,
                        },
                        crate::api::schema::IntegrationInfo {
                            target: crate::api::schema::IntegrationTarget::Claude,
                            label: "claude".into(),
                            command: "claude".into(),
                            available: false,
                            state: crate::api::schema::IntegrationState::NotInstalled,
                        },
                    ],
                }),
            )
            .0
    );
    let frame = state.compose(106, 30).expect("loaded integrations");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("update available"));
    assert!(text.contains("not found"));
    assert!(!text.contains("pane labels"));

    let popup = state.hits.settings_popup;
    let blank_click =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: popup.right().saturating_sub(2),
            row: popup.y + 3,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(!blank_click.repaint);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(_))
    ));

    let install = state.handle_input_bytes(b"\r");
    assert_eq!(install.actions.len(), 1);
    assert!(matches!(
        &install.actions[0],
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                request.method,
                crate::api::schema::Method::IntegrationInstall(
                    crate::api::schema::IntegrationInstallParams {
                        target: crate::api::schema::IntegrationTarget::Codex
                    }
                )
            )
    ));
    let escape = state.handle_input_bytes(b"\x1b");
    assert!(!escape.repaint);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(_))
    ));
    let install_request_id = match &install.actions[0] {
        ClientShellAction::Endpoint { request, .. } => request.id.clone(),
        _ => unreachable!("integration install action"),
    };
    let (repaint, refresh_actions) = state.handle_endpoint_result(
        "boot-1",
        &install_request_id,
        Ok(crate::api::schema::ResponseResult::IntegrationInstall {
            target: crate::api::schema::IntegrationTarget::Codex,
            details: crate::api::schema::IntegrationInstallResult {
                messages: vec!["installed codex".into()],
            },
        }),
    );
    assert!(repaint);
    assert!(matches!(
        refresh_actions.as_slice(),
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(request.method, crate::api::schema::Method::IntegrationList(_))
    ));
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            loading_integrations: true,
            installing_integrations: false,
            ref integration_messages,
            ..
        })) if integration_messages == &["installed codex"]
    ));
}
