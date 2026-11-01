//! Shared helpers for integration tests.

use std::path::Path;

/// Create a file symlink, returning `false` when Windows denied the symlink
/// privilege. Callers skip symlink-only assertions on `false` instead of
/// failing an ordinary non-elevated local test run.
pub(super) fn symlink_file(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).expect("create symlink");
    true
}
