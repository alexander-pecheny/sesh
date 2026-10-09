//! Stamps the helper with a hash of what it ships, which Sesh compares to decide whether the
//! copy on a Host is stale. Tests are left out: the files named `*tests.rs`, and the rest of
//! any file from its `#[cfg(test)]` on, which is where every inline tests module sits.

use std::path::{Path, PathBuf};

fn shipping(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("src") {
        let path = entry.expect("src entry").path();
        if path.is_dir() {
            shipping(&path, files);
        } else if !path.to_string_lossy().ends_with("tests.rs") {
            files.push(path);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    shipping(Path::new("src"), &mut files);
    files.sort();
    files.push("Cargo.toml".into());
    let shipped: String = files
        .iter()
        .map(|file| {
            let text = std::fs::read_to_string(file).expect("source");
            text.split("#[cfg(test)]").next().unwrap_or_default().to_string()
        })
        .collect();
    let hash = shipped.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3));
    println!("cargo:rustc-env=SOURCE_HASH={hash:016x}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
