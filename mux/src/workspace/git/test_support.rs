use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn temp_test_dir(name: &str) -> PathBuf {
    let unique = format!(
        "sesh-mux-workspace-tests-{}-{}-{}",
        name,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let path = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn init_repo_with_commit(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    run_git(repo, &["init", "--quiet"]);
    run_git(repo, &["config", "user.email", "sesh-mux@example.invalid"]);
    run_git(repo, &["config", "user.name", "sesh-mux Test"]);
    run_git(
        repo,
        &["commit", "--quiet", "--allow-empty", "-m", "initial"],
    );
}

pub(crate) fn create_bare_repo_with_linked_worktree(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = temp_test_dir(name);
    let seed = base.join("seed");
    let bare = base.join(".bare");
    let checkout = base.join("feature");
    init_repo_with_commit(&seed);
    run_git(
        &base,
        &[
            "clone",
            "--quiet",
            "--bare",
            seed.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    run_git(
        &bare,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            checkout.to_str().unwrap(),
            "HEAD",
        ],
    );
    (base, bare, checkout)
}

pub(super) fn run_git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}
