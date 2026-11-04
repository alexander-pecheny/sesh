use super::harness::*;

#[test]
fn workspace_and_pane_management_commands_work() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("sesh-mux.sock");

    let server = spawn_sesh_mux(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let listed = run_cli(&socket_path, &["workspace", "list"]);
    assert!(listed.status.success());
    let listed_json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed_json["result"]["type"], "workspace_list");
    assert_eq!(
        listed_json["result"]["workspaces"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    let created = run_cli(
        &socket_path,
        &["workspace", "create", "--cwd", base.to_str().unwrap()],
    );
    assert!(created.status.success());
    let created_json: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let workspace_id = created_json["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();

    let panes = run_cli(&socket_path, &["pane", "list", "--workspace", "1"]);
    assert!(panes.status.success());
    let panes_json: serde_json::Value = serde_json::from_slice(&panes.stdout).unwrap();
    assert_eq!(panes_json["result"]["panes"].as_array().unwrap().len(), 1);

    let tab = run_cli_json(&socket_path, &["tab", "create", "--workspace", &workspace_id]);
    let tab_pane_id = tab["result"]["root_pane"]["pane_id"].as_str().unwrap();

    let fetched = run_cli(&socket_path, &["pane", "get", tab_pane_id]);
    assert!(fetched.status.success());
    let fetched_json: serde_json::Value = serde_json::from_slice(&fetched.stdout).unwrap();
    assert_eq!(fetched_json["result"]["pane"]["pane_id"], tab_pane_id);

    let closed = run_cli(&socket_path, &["pane", "close", tab_pane_id]);
    assert!(closed.status.success());
    let closed_json: serde_json::Value = serde_json::from_slice(&closed.stdout).unwrap();
    assert_eq!(closed_json["result"]["type"], "ok");

    let renamed = run_cli(
        &socket_path,
        &["workspace", "rename", &workspace_id, "demo"],
    );
    assert!(renamed.status.success());
    let renamed_json: serde_json::Value = serde_json::from_slice(&renamed.stdout).unwrap();
    assert_eq!(renamed_json["result"]["workspace"]["label"], "demo");

    let closed_workspace = run_cli(&socket_path, &["workspace", "close", &workspace_id]);
    assert!(closed_workspace.status.success());
    let closed_workspace_json: serde_json::Value =
        serde_json::from_slice(&closed_workspace.stdout).unwrap();
    assert_eq!(closed_workspace_json["result"]["type"], "ok");

    cleanup_spawned_sesh_mux(server, base);
}

#[test]
fn worktree_management_commands_work() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("sesh-mux.sock");
    let repo = base.join("repo");
    let checkout = base.join("checkout");
    create_committed_repo(&repo);

    let server = spawn_sesh_mux(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let branch = "worktree/cli-wrapper";
    let created = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "create",
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            branch,
            "--path",
            checkout.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(created["result"]["type"], "worktree_created");
    assert_eq!(created["result"]["worktree"]["branch"], branch);
    let child_workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(checkout.join("README.md").exists());

    let workspaces = run_cli_json(&socket_path, &["workspace", "list"]);
    let workspace_list = workspaces["result"]["workspaces"].as_array().unwrap();
    let parent_workspace_id = workspace_list
        .iter()
        .find(|workspace| workspace["worktree"]["is_linked_worktree"].as_bool() == Some(false))
        .and_then(|workspace| workspace["workspace_id"].as_str())
        .unwrap()
        .to_string();
    assert!(workspace_list.iter().any(|workspace| {
        workspace["workspace_id"].as_str() == Some(child_workspace_id.as_str())
            && workspace["worktree"]["is_linked_worktree"].as_bool() == Some(true)
    }));

    let listed = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "list",
            "--workspace",
            &parent_workspace_id,
            "--json",
        ],
    );
    let listed_entry = listed["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["branch"].as_str() == Some(branch))
        .unwrap();
    assert_eq!(
        listed_entry["open_workspace_id"].as_str(),
        Some(child_workspace_id.as_str())
    );

    let opened = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "open",
            "--workspace",
            &parent_workspace_id,
            "--branch",
            branch,
            "--json",
        ],
    );
    assert_eq!(opened["result"]["type"], "worktree_opened");
    assert_eq!(opened["result"]["already_open"], true);
    assert_eq!(
        opened["result"]["workspace"]["workspace_id"].as_str(),
        Some(child_workspace_id.as_str())
    );

    fs::write(checkout.join("README.md"), "dirty\n").unwrap();
    let safe_remove = run_cli(
        &socket_path,
        &[
            "worktree",
            "remove",
            "--workspace",
            &child_workspace_id,
            "--json",
        ],
    );
    assert_eq!(safe_remove.status.code(), Some(1));
    let safe_remove_json: serde_json::Value = serde_json::from_slice(&safe_remove.stderr).unwrap();
    assert_eq!(
        safe_remove_json["error"]["code"],
        "dirty_worktree_requires_force"
    );
    assert!(checkout.exists());

    let force_removed = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "remove",
            "--workspace",
            &child_workspace_id,
            "--force",
            "--json",
        ],
    );
    assert_eq!(force_removed["result"]["type"], "worktree_removed");
    assert_eq!(force_removed["result"]["forced"], true);
    assert!(!checkout.exists());

    cleanup_spawned_sesh_mux(server, base);
}

#[test]
fn workspace_close_requires_explicit_worktree_group_intent() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("sesh-mux.sock");
    let repo = base.join("repo");
    let checkout = base.join("checkout");
    create_committed_repo(&repo);

    let server = spawn_sesh_mux(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "create",
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            "worktree/group-close",
            "--path",
            checkout.to_str().unwrap(),
        ],
    );
    assert_eq!(created["result"]["type"], "worktree_created");
    let workspaces = run_cli_json(&socket_path, &["workspace", "list"]);
    let workspace_list = workspaces["result"]["workspaces"].as_array().unwrap();
    let parent_workspace_id = workspace_list
        .iter()
        .find(|workspace| workspace["worktree"]["is_linked_worktree"] == false)
        .and_then(|workspace| workspace["workspace_id"].as_str())
        .unwrap()
        .to_string();
    let workspace_ids = workspace_list
        .iter()
        .map(|workspace| workspace["workspace_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    let rejected = run_cli(&socket_path, &["workspace", "close", &parent_workspace_id]);
    assert_eq!(rejected.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&rejected.stderr).unwrap();
    assert_eq!(error["error"]["code"], "workspace_group_close_required");
    let after_rejection = run_cli_json(&socket_path, &["workspace", "list"]);
    assert_eq!(
        after_rejection["result"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|workspace| workspace["workspace_id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        workspace_ids
    );

    let closed = run_cli(
        &socket_path,
        &["workspace", "close", &parent_workspace_id, "--group"],
    );
    assert!(
        closed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&closed.stderr)
    );
    let after_close = run_cli_json(&socket_path, &["workspace", "list"]);
    assert!(after_close["result"]["workspaces"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        checkout.exists(),
        "workspace close must not remove the checkout"
    );

    cleanup_spawned_sesh_mux(server, base);
}

#[test]
fn forced_worktree_remove_terminates_processes_inside_checkout() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("sesh-mux.sock");
    let repo = base.join("repo");
    let checkout = base.join("checkout-with-process");
    create_committed_repo(&repo);

    let server = spawn_sesh_mux(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "create",
            "--cwd",
            repo.to_str().unwrap(),
            "--branch",
            "worktree/force-process",
            "--path",
            checkout.to_str().unwrap(),
            "--json",
        ],
    );
    let child_workspace_id = created["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let pid_file = base.join("worktree-remove-force.pid");
    let command = format!(
        "python3 -c 'import os,time,pathlib; pathlib.Path(r\"{}\").write_text(str(os.getpid())); time.sleep(1000)'",
        pid_file.display()
    );
    let ran = run_cli(&socket_path, &["pane", "run", &pane_id, &command]);
    assert!(
        ran.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let pid = wait_for_pid_file(&pid_file, Duration::from_secs(5)).unwrap_or_else(|err| {
        panic!("failed to read pane child pid: {err}");
    });
    assert!(process_exists(pid), "child process was not running");

    let removed = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "remove",
            "--workspace",
            &child_workspace_id,
            "--force",
            "--json",
        ],
    );
    assert_eq!(removed["result"]["type"], "worktree_removed");
    assert!(wait_for_pid_exit(pid, Duration::from_secs(3)));
    assert!(!checkout.exists());

    cleanup_spawned_sesh_mux(server, base);
}

#[test]
fn worktree_open_existing_checkout_by_path_and_branch() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("sesh-mux.sock");
    let repo = base.join("repo");
    let checkout = base.join("external-checkout");
    create_committed_repo(&repo);
    let branch = "worktree/cli-open-existing";
    run_git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            checkout.to_str().unwrap(),
            "HEAD",
        ],
    );

    let server = spawn_sesh_mux(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let opened = run_cli_json_in_dir(
        &socket_path,
        &[
            "worktree",
            "open",
            "--cwd",
            "repo",
            "--path",
            "external-checkout",
            "--json",
        ],
        &base,
    );
    assert_eq!(opened["result"]["type"], "worktree_opened");
    assert_eq!(opened["result"]["already_open"], false);
    assert_eq!(opened["result"]["worktree"]["branch"], branch);
    assert_eq!(
        opened["result"]["workspace"]["worktree"]["is_linked_worktree"],
        true
    );
    let child_workspace_id = opened["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();

    let workspaces = run_cli_json(&socket_path, &["workspace", "list"]);
    let workspace_list = workspaces["result"]["workspaces"].as_array().unwrap();
    let parent_workspace_id = workspace_list
        .iter()
        .find(|workspace| workspace["worktree"]["is_linked_worktree"].as_bool() == Some(false))
        .and_then(|workspace| workspace["workspace_id"].as_str())
        .unwrap()
        .to_string();

    let listed = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "list",
            "--workspace",
            &parent_workspace_id,
            "--json",
        ],
    );
    let listed_entry = listed["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["branch"].as_str() == Some(branch))
        .unwrap();
    assert_eq!(
        listed_entry["open_workspace_id"].as_str(),
        Some(child_workspace_id.as_str())
    );

    let reopened = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "open",
            "--workspace",
            &parent_workspace_id,
            "--branch",
            branch,
            "--json",
        ],
    );
    assert_eq!(reopened["result"]["type"], "worktree_opened");
    assert_eq!(reopened["result"]["already_open"], true);
    assert_eq!(
        reopened["result"]["workspace"]["workspace_id"].as_str(),
        Some(child_workspace_id.as_str())
    );

    let removed = run_cli_json(
        &socket_path,
        &[
            "worktree",
            "remove",
            "--workspace",
            &child_workspace_id,
            "--force",
            "--json",
        ],
    );
    assert_eq!(removed["result"]["type"], "worktree_removed");

    cleanup_spawned_sesh_mux(server, base);
}

#[test]
fn worktree_cli_rejects_local_argument_errors_before_socket_use() {
    let base = unique_test_dir();
    fs::create_dir_all(&base).unwrap();
    let socket_path = base.join("missing.sock");
    let cases: &[&[&str]] = &[
        &["worktree", "list", "--workspace", "1", "--cwd", "/tmp"],
        &["worktree", "create", "--workspace", "1", "--cwd", "/tmp"],
        &["worktree", "open", "--workspace", "1"],
        &[
            "worktree",
            "open",
            "--workspace",
            "1",
            "--path",
            "a",
            "--branch",
            "b",
        ],
        &[
            "worktree",
            "open",
            "--workspace",
            "1",
            "--cwd",
            "/tmp",
            "--branch",
            "b",
        ],
    ];

    for args in cases {
        let output = run_cli(&socket_path, args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "sesh-mux {} should fail as local parse error; stdout={} stderr={}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    cleanup_test_base(&base);
}

#[test]
fn tab_management_commands_work() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("sesh-mux.sock");

    let server = spawn_sesh_mux(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = run_cli(
        &socket_path,
        &["workspace", "create", "--cwd", base.to_str().unwrap()],
    );
    assert!(created.status.success());
    let created_json: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
    let workspace_id = created_json["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let created_tab = run_cli(
        &socket_path,
        &["tab", "create", "--workspace", &workspace_id],
    );
    assert!(created_tab.status.success());
    let created_tab_json: serde_json::Value = serde_json::from_slice(&created_tab.stdout).unwrap();
    let second_tab_id = created_tab_json["result"]["tab"]["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(second_tab_id, format!("{workspace_id}:t2"));

    let listed_tabs = run_cli(&socket_path, &["tab", "list", "--workspace", &workspace_id]);
    assert!(listed_tabs.status.success());
    let listed_tabs_json: serde_json::Value = serde_json::from_slice(&listed_tabs.stdout).unwrap();
    assert_eq!(
        listed_tabs_json["result"]["tabs"].as_array().unwrap().len(),
        2
    );

    let renamed_tab = run_cli(&socket_path, &["tab", "rename", &second_tab_id, "logs"]);
    assert!(renamed_tab.status.success());
    let renamed_tab_json: serde_json::Value = serde_json::from_slice(&renamed_tab.stdout).unwrap();
    assert_eq!(renamed_tab_json["result"]["tab"]["label"], "logs");

    let tab_get = run_cli(&socket_path, &["tab", "get", &second_tab_id]);
    assert!(tab_get.status.success());
    let tab_get_json: serde_json::Value = serde_json::from_slice(&tab_get.stdout).unwrap();
    assert_eq!(tab_get_json["result"]["tab"]["tab_id"], second_tab_id);

    let closed_tab = run_cli(&socket_path, &["tab", "close", &second_tab_id]);
    assert!(closed_tab.status.success());
    let closed_tab_json: serde_json::Value = serde_json::from_slice(&closed_tab.stdout).unwrap();
    assert_eq!(closed_tab_json["result"]["type"], "ok");

    cleanup_spawned_sesh_mux(server, base);
}
