use std::cell::RefCell;

use super::*;

struct Scratch(tempfile::TempDir);

impl Scratch {
    fn new() -> Self {
        let scratch = Scratch(tempfile::tempdir().unwrap());
        std::fs::create_dir_all(scratch.home().join(".ssh")).unwrap();
        scratch
    }

    fn home(&self) -> PathBuf {
        self.0.path().to_path_buf()
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.home().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn resolve(&self, alias: &str, config: &str) -> Result<Host, String> {
        self.resolve_with(alias, config, &|_| false, &|_| false)
    }

    fn resolve_with(&self, alias: &str, config: &str, exec: &dyn Fn(&str) -> bool, resolves: &dyn Fn(&str) -> bool) -> Result<Host, String> {
        let file = self.write(".ssh/config", config);
        let env = |name: &str| match name {
            "SSH_AUTH_SOCK" => Some("/tmp/agent.sock".to_string()),
            "WALRUS" => Some("tusk".to_string()),
            _ => None,
        };
        let local = Local { home: self.home(), user: "me".into(), host: "mac.local".into(), uid: 501, env: &env, exec, resolves };
        resolve_from(alias, &[file], &local)
    }
}

#[test]
fn the_first_value_obtained_wins_and_identity_files_add_up() {
    let scratch = Scratch::new();
    let host = scratch
        .resolve(
            "vps",
            "Host vps\n  HostName 10.0.0.1\n  User pecheny\n  IdentityFile ~/.ssh/a\n\nHost *\n  User root\n  Port 2222\n  IdentityFile ~/.ssh/b\n",
        )
        .unwrap();
    assert_eq!((host.host_name.as_str(), host.user.as_str(), host.port), ("10.0.0.1", "pecheny", 2222));
    assert_eq!(host.identity_files, [scratch.home().join(".ssh/a"), scratch.home().join(".ssh/b")]);
}

#[test]
fn an_unknown_alias_gets_ssh_defaults() {
    let scratch = Scratch::new();
    let host = scratch.resolve("example.com", "Host other\n  User x\n").unwrap();
    assert_eq!((host.host_name.as_str(), host.user.as_str(), host.port), ("example.com", "me", 22));
    assert_eq!(host.strict, Strict::Ask);
    assert_eq!(host.identity_agent, Some(PathBuf::from("/tmp/agent.sock")));
    assert_eq!(host.identity_files.len(), 5);
    assert_eq!(host.known_hosts[0], scratch.home().join(".ssh/known_hosts"));
    assert_eq!(host.methods, [Method::PublicKey, Method::KeyboardInteractive, Method::Password]);
    assert_eq!(host.proxy, Proxy::Direct);
}

#[test]
fn host_patterns_glob_and_a_negation_vetoes() {
    let scratch = Scratch::new();
    let config = "Host *.prod !bastion.prod\n  User deploy\nHost web?\n  Port 2200\n";
    assert_eq!(scratch.resolve("api.prod", config).unwrap().user, "deploy");
    assert_eq!(scratch.resolve("bastion.prod", config).unwrap().user, "me");
    assert_eq!(scratch.resolve("WEB1", config).unwrap().port, 2200);
    assert_eq!(scratch.resolve("web12", config).unwrap().port, 22);
}

#[test]
fn keywords_take_equals_signs_quotes_and_any_case() {
    let scratch = Scratch::new();
    let host = scratch.resolve("q", "HOST q\n\tuser=\"two words\"\n  PORT = 23 # a comment\n").unwrap();
    assert_eq!((host.user.as_str(), host.port), ("two words", 23));
}

#[test]
fn match_criteria_combine_and_exec_runs_with_tokens() {
    let scratch = Scratch::new();
    let ran = RefCell::new(Vec::new());
    let exec = |command: &str| {
        ran.borrow_mut().push(command.to_string());
        command.contains("ok")
    };
    let config = "Host box\n  HostName box.internal\nMatch host *.internal user me exec \"test ok %h %n\"\n  Port 2201\nMatch originalhost box !localuser me\n  Port 2202\n";
    let host = scratch.resolve_with("box", config, &exec, &|_| false).unwrap();
    assert_eq!(host.port, 2201);
    assert_eq!(*ran.borrow(), ["test ok box.internal box"]);
}

#[test]
fn match_final_reads_the_config_again_after_the_rest() {
    let scratch = Scratch::new();
    let config = "Match final host box\n  User late\nHost box\n  Port 2203\n";
    let host = scratch.resolve("box", config).unwrap();
    assert_eq!((host.user.as_str(), host.port), ("late", 2203));
}

#[test]
fn canonicalization_appends_a_domain_that_resolves_and_matches_again() {
    let scratch = Scratch::new();
    let config = "CanonicalizeHostname yes\nCanonicalDomains a.example b.example\nHost *.b.example\n  User canon\nMatch canonical\n  Port 2204\n";
    let host = scratch.resolve_with("web", config, &|_| false, &|name| name == "web.b.example").unwrap();
    assert_eq!((host.host_name.as_str(), host.user.as_str(), host.port), ("web.b.example", "canon", 2204));
    let dotted = scratch.resolve_with("web.x.y", config, &|_| false, &|_| true).unwrap();
    assert_eq!(dotted.host_name, "web.x.y");
}

#[test]
fn includes_glob_relative_to_dot_ssh_and_obey_the_block_they_sit_in() {
    let scratch = Scratch::new();
    scratch.write(".ssh/conf.d/b.conf", "Host box\n  Port 2205\n");
    scratch.write(".ssh/conf.d/a.conf", "Host box\n  User from-a\n");
    scratch.write(".ssh/only-other", "Port 9999\n");
    let config = "Host other\n  Include only-other\nHost *\n  Include conf.d/*.conf\n";
    let host = scratch.resolve("box", config).unwrap();
    assert_eq!((host.user.as_str(), host.port), ("from-a", 2205));
}

#[test]
fn an_include_that_includes_itself_stops() {
    let scratch = Scratch::new();
    scratch.write(".ssh/loop", "Include loop\n");
    assert!(scratch.resolve("box", "Include loop\n").unwrap_err().contains("nested too deeply"));
}

#[test]
fn proxy_command_keeps_its_line_and_expands_its_tokens() {
    let scratch = Scratch::new();
    let config = "Host ssm\n  HostName i-0abc\n  User ec2-user\n  ProxyCommand sh -c \"aws ssm start-session --target %h --parameters 'portNumber=%p'\"\n";
    let host = scratch.resolve("ssm", config).unwrap();
    assert_eq!(host.proxy, Proxy::Command("sh -c \"aws ssm start-session --target i-0abc --parameters 'portNumber=22'\"".into()));
}

#[test]
fn proxy_jump_lists_hops_and_none_turns_it_off() {
    let scratch = Scratch::new();
    let host = scratch.resolve("in", "Host in\n  ProxyJump admin@gate:2222,ssh://[fe80::1]:22,plain\n").unwrap();
    assert_eq!(
        host.proxy,
        Proxy::Jump(vec![
            Hop { user: Some("admin".into()), host: "gate".into(), port: Some(2222) },
            Hop { user: None, host: "fe80::1".into(), port: Some(22) },
            Hop { user: None, host: "plain".into(), port: None },
        ])
    );
    let off = scratch.resolve("gate", "Host gate\n  ProxyJump none\nHost *\n  ProxyJump hop\n").unwrap();
    assert_eq!(off.proxy, Proxy::Direct);
}

#[test]
fn paths_expand_tilde_environment_and_tokens() {
    let scratch = Scratch::new();
    let config = "Host box\n  User u\n  IdentityFile ~/.ssh/%r@%h-${WALRUS}\n  IdentityAgent ~/agents/%C\n  UserKnownHostsFile /dev/null\n  ForwardAgent yes\n";
    let host = scratch.resolve("box", config).unwrap();
    assert_eq!(host.identity_files, [scratch.home().join(".ssh/u@box-tusk")]);
    let agent = host.identity_agent.clone().unwrap();
    assert_eq!(agent.parent().unwrap(), scratch.home().join("agents"));
    assert_eq!(agent.file_name().unwrap().len(), 40);
    assert_eq!(host.forward_agent, host.identity_agent);
    assert_eq!(host.known_hosts, [PathBuf::from("/dev/null")]);
    assert!(scratch.resolve("box", "IdentityFile ${NOPE}/key\n").unwrap_err().contains("NOPE is not set"));
}

#[test]
fn authentication_options_shape_the_methods_tried() {
    let scratch = Scratch::new();
    let host = scratch
        .resolve("box", "PreferredAuthentications password,publickey\nPasswordAuthentication no\nStrictHostKeyChecking accept-new\nSetEnv TERM=xterm A=1\nSetEnv TERM=vt100\n")
        .unwrap();
    assert_eq!(host.methods, [Method::PublicKey]);
    assert_eq!(host.strict, Strict::AcceptNew);
    assert_eq!(host.set_env, [("TERM".to_string(), "xterm".to_string()), ("A".to_string(), "1".to_string())]);
}

#[test]
fn a_broken_line_names_its_file_and_line() {
    let scratch = Scratch::new();
    let error = scratch.resolve("box", "Host box\n  Port many\n").unwrap_err();
    assert!(error.ends_with("config:2: bad port many"), "{error}");
}
