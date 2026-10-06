//! Stamps the helper with a hash of its sources, which Sesh compares to decide whether the
//! copy on a Host is stale.

fn main() {
    let mut files: Vec<_> = std::fs::read_dir("src")
        .expect("src")
        .map(|entry| entry.expect("src entry").path())
        .collect();
    files.sort();
    files.push("Cargo.toml".into());
    let hash = files
        .iter()
        .flat_map(|file| std::fs::read(file).expect("source"))
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
        });
    println!("cargo:rustc-env=SOURCE_HASH={hash:016x}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
