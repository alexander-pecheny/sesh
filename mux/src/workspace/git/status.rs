use std::path::PathBuf;

use crate::workspace::{GitSpaceMetadata};

use super::{
    discovery::{canonicalize_best_effort_path},
};








pub fn git_status_cache_key_for_space(space: &GitSpaceMetadata) -> PathBuf {
    canonicalize_best_effort_path(&space.repo_root)
}


#[cfg(test)]
mod tests {

    #[cfg(target_os = "linux")]
    #[test]
    fn cache_key_from_space_preserves_non_utf8_checkout_path() {
        use std::os::unix::ffi::OsStringExt;

        let base = temp_test_dir("non-utf8-key");
        let root = base.join(std::ffi::OsString::from_vec(vec![
            b'r', b'e', b'p', b'o', 0x80,
        ]));
        write_fake_tracked_repo(&root);
        let space = git_space_metadata(&root).expect("Git metadata");

        assert_eq!(
            git_status_cache_key_for_space(&space),
            std::fs::canonicalize(&root).unwrap()
        );

        std::fs::remove_dir_all(base).unwrap();
    }
}
