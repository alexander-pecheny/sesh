use super::*;

fn protocol_schema_entry<T: schemars::JsonSchema>(name: &str) -> serde_json::Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    rewrite_schema_refs(&mut schema, name);
    schema
}

fn rewrite_schema_refs(value: &mut serde_json::Value, schema_name: &str) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(serde_json::Value::String(reference)) = object.get_mut("$ref") {
                if let Some(path) = reference.strip_prefix("#/") {
                    *reference = format!("#/schemas/{schema_name}/{path}");
                }
            }
            for child in object.values_mut() {
                rewrite_schema_refs(child, schema_name);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_schema_refs(item, schema_name);
            }
        }
        _ => {}
    }
}

fn protocol_schema_document() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "sesh-mux API",
        "schema_version": 1,
        "protocol": crate::protocol::PROTOCOL_VERSION,
        "schemas": {
            "request": protocol_schema_entry::<Request>("request"),
            "success_response": protocol_schema_entry::<SuccessResponse>("success_response"),
            "error_response": protocol_schema_entry::<ErrorResponse>("error_response"),
            "event": protocol_schema_entry::<EventEnvelope>("event"),
            "subscription_event": protocol_schema_entry::<SubscriptionEventEnvelope>("subscription_event"),
        },
    })
}

#[test]
fn request_uses_dot_method_names() {
    let request = Request {
        id: "req_1".into(),
        method: Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: Some("/tmp".into()),
            focus: true,
            label: Some("api".into()),
            env: Default::default(),
        }),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "workspace.create");
}

#[test]
fn workspace_close_group_intent_defaults_false_and_round_trips() {
    let request: Request = serde_json::from_value(serde_json::json!({
        "id": "close",
        "method": "workspace.close",
        "params": { "workspace_id": "w1" }
    }))
    .unwrap();
    assert!(matches!(
        request.method,
        Method::WorkspaceClose(WorkspaceCloseParams {
            close_group: false,
            ..
        })
    ));

    let explicit = Request {
        id: "close-group".into(),
        method: Method::WorkspaceClose(WorkspaceCloseParams {
            workspace_id: "w1".into(),
            close_group: true,
        }),
    };
    let json = serde_json::to_value(&explicit).unwrap();
    assert_eq!(json["params"]["close_group"], true);
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), explicit);
}

#[test]
fn agent_start_and_prompt_requests_round_trip() {
    let start = Request {
        id: "start".into(),
        method: Method::AgentStart(AgentStartParams {
            name: "reviewer".into(),
            kind: "pi".into(),
            pane_id: "w1:p2".into(),
            args: vec!["--no-session".into()],
            env: Default::default(),
            timeout_ms: Some(30_000),
        }),
    };
    let start_json = serde_json::to_value(&start).unwrap();
    assert_eq!(start_json["method"], "agent.start");
    assert_eq!(start_json["params"]["pane_id"], "w1:p2");
    assert_eq!(
        serde_json::from_value::<Request>(start_json).unwrap(),
        start
    );

    let prompt = Request {
        id: "prompt".into(),
        method: Method::AgentPrompt(AgentPromptParams {
            target: "reviewer".into(),
            text: "review this".into(),
            wait: None,
        }),
    };
    let prompt_json = serde_json::to_value(&prompt).unwrap();
    assert_eq!(prompt_json["method"], "agent.prompt");
    assert_eq!(
        serde_json::from_value::<Request>(prompt_json).unwrap(),
        prompt
    );

    let prompt_and_wait = Request {
        id: "prompt-and-wait".into(),
        method: Method::AgentPrompt(AgentPromptParams {
            target: "reviewer".into(),
            text: "review this".into(),
            wait: Some(AgentPromptWaitOptions {
                until: vec![AgentStatus::Idle, AgentStatus::Done],
                timeout_ms: Some(120_000),
                submission_deadline: None,
            }),
        }),
    };
    let prompt_and_wait_json = serde_json::to_value(&prompt_and_wait).unwrap();
    assert_eq!(
        prompt_and_wait_json["params"]["wait"]["until"],
        serde_json::json!(["idle", "done"])
    );
    assert_eq!(
        prompt_and_wait_json["params"]["wait"]["timeout_ms"],
        120_000
    );
    assert_eq!(
        serde_json::from_value::<Request>(prompt_and_wait_json).unwrap(),
        prompt_and_wait
    );
}

#[test]
fn bundled_protocol_schema_refs_resolve_inside_bundle() {
    fn assert_no_standalone_refs(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(serde_json::Value::String(reference)) = object.get("$ref") {
                    assert!(
                        !reference.starts_with("#/$defs/"),
                        "schema bundle contains standalone ref {reference}"
                    );
                }
                for child in object.values() {
                    assert_no_standalone_refs(child);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    assert_no_standalone_refs(item);
                }
            }
            _ => {}
        }
    }

    assert_no_standalone_refs(&protocol_schema_document());
}

#[test]
fn request_round_trips_for_server_stop() {
    let request = Request {
        id: "req_stop".into(),
        method: Method::ServerStop(EmptyParams::default()),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "server.stop");
    let restored: Request = serde_json::from_value(json).unwrap();
    assert_eq!(restored, request);
}

#[test]
fn integration_list_request_and_response_round_trip() {
    let request = Request {
        id: "req_integrations".into(),
        method: Method::IntegrationList(EmptyParams::default()),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["method"], "integration.list");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let response = SuccessResponse {
        id: "req_integrations".into(),
        result: ResponseResult::IntegrationList {
            integrations: vec![IntegrationInfo {
                target: IntegrationTarget::Codex,
                label: "codex".into(),
                command: "codex".into(),
                available: true,
                state: IntegrationState::Outdated,
            }],
        },
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["result"]["type"], "integration_list");
    assert_eq!(json["result"]["integrations"][0]["state"], "outdated");
    assert_eq!(
        serde_json::from_value::<SuccessResponse>(json).unwrap(),
        response
    );
}

#[test]
fn event_envelope_round_trips() {
    let events = [
        EventEnvelope {
            event: EventKind::PaneOutputChanged,
            data: EventData::PaneOutputChanged {
                pane_id: "p_1".into(),
                workspace_id: "w_1".into(),
                revision: 42,
            },
        },
        EventEnvelope {
            event: EventKind::WorkspaceMoved,
            data: EventData::WorkspaceMoved {
                workspace_id: "w_1".into(),
                insert_index: 2,
                workspaces: vec![],
            },
        },
        EventEnvelope {
            event: EventKind::WorkspaceReordered,
            data: EventData::WorkspaceReordered {
                workspace_ids: vec!["w_1".into(), "w_2".into()],
                before_workspace_id: Some("w_3".into()),
                workspaces: vec![],
            },
        },
        EventEnvelope {
            event: EventKind::TabMoved,
            data: EventData::TabMoved {
                tab_id: "w_1:1".into(),
                workspace_id: "w_1".into(),
                insert_index: 1,
                tabs: vec![],
            },
        },
        EventEnvelope {
            event: EventKind::LayoutUpdated,
            data: EventData::LayoutUpdated {
                layout: PaneLayoutSnapshot {
                    workspace_id: "w_1".into(),
                    tab_id: "w_1:1".into(),
                    zoomed: false,
                    area: PaneLayoutRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 24,
                    },
                    focused_pane_id: "w_1-1".into(),
                    panes: vec![PaneLayoutPane {
                        pane_id: "w_1-1".into(),
                        focused: true,
                        rect: PaneLayoutRect {
                            x: 0,
                            y: 0,
                            width: 100,
                            height: 24,
                        },
                    }],
                    splits: vec![],
                },
            },
        },
    ];

    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        let restored: EventEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, event);
    }
}

#[test]
fn subscribe_request_parses_parameterized_subscriptions() {
    let json = r#"
    {
        "id": "sub_1",
        "method": "events.subscribe",
        "params": {
            "subscriptions": [
                {
                    "type": "pane.output_matched",
                    "pane_id": "p_1_1",
                    "source": "recent",
                    "lines": 200,
                    "match": { "type": "substring", "value": "auth: received" }
                },
                {
                    "type": "pane.agent_status_changed",
                    "pane_id": "p_1_1",
                    "agent_status": "done"
                },
                {
                    "type": "pane.scroll_changed",
                    "pane_id": "p_1_1"
                }
            ]
        }
    }
    "#;

    let request: Request = serde_json::from_str(json).unwrap();
    let Method::EventsSubscribe(params) = request.method else {
        panic!("wrong method parsed");
    };
    assert_eq!(params.subscriptions.len(), 3);
    assert!(matches!(
        &params.subscriptions[0],
        Subscription::PaneOutputMatched {
            pane_id,
            source: ReadSource::Recent,
            lines: Some(200),
            r#match: OutputMatch::Substring { value },
            strip_ansi: true,
        } if pane_id == "p_1_1" && value == "auth: received"
    ));
    assert!(matches!(
        &params.subscriptions[1],
        Subscription::PaneAgentStatusChanged {
            pane_id,
            agent_status: Some(AgentStatus::Done),
        } if pane_id == "p_1_1"
    ));
    assert!(matches!(
        &params.subscriptions[2],
        Subscription::PaneScrollChanged { pane_id } if pane_id == "p_1_1"
    ));
}

#[test]
fn subscription_event_envelope_round_trips() {
    let event = SubscriptionEventEnvelope {
        event: SubscriptionEventKind::PaneOutputMatched,
        data: SubscriptionEventData::PaneOutputMatched(PaneOutputMatchedEvent {
            pane_id: "p_1_1".into(),
            matched_line: "auth: received".into(),
            read: PaneReadResult {
                pane_id: "p_1_1".into(),
                workspace_id: "w_1".into(),
                tab_id: "t_1_1".into(),
                source: ReadSource::Recent,
                format: ReadFormat::Text,
                text: "auth: received\n".into(),
                revision: 0,
                truncated: false,
            },
        }),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"event\":\"pane.output_matched\""));
    let restored: SubscriptionEventEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, event);
}

#[test]
fn scroll_changed_subscription_event_round_trips() {
    let event = SubscriptionEventEnvelope {
        event: SubscriptionEventKind::ScrollChanged,
        data: SubscriptionEventData::ScrollChanged(PaneScrollChangedEvent {
            pane_id: "p_1_1".into(),
            workspace_id: "w_1".into(),
            scroll: PaneScrollInfo {
                offset_from_bottom: 12,
                max_offset_from_bottom: 240,
                viewport_rows: 30,
            },
        }),
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"event\":\"pane.scroll_changed\""));
    let restored: SubscriptionEventEnvelope = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, event);
}

#[test]
fn agent_status_request_values_remain_strict() {
    assert!(serde_json::from_str::<AgentStatus>(r#""working""#).is_ok());
    assert!(serde_json::from_str::<AgentStatus>(r#""future_status""#).is_err());
}
