use std::path::PathBuf;

use crate::workspace::GitSpaceMetadata;

use super::discovery::canonicalize_best_effort_path;

pub fn git_status_cache_key_for_space(space: &GitSpaceMetadata) -> PathBuf {
    canonicalize_best_effort_path(&space.repo_root)
}

#[cfg(test)]
mod tests {}
