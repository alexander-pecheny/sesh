use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const GHOSTTY_VERSION: &str = "1.3.2-HEAD-+44f2a44df";
const GHOSTTY_PARTS: &[&str] = &["build.zig", "build.zig.zon", "include", "pkg", "src", "dist"];

fn zig_target(target: &str) -> &str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-linux-musl" => "x86_64-linux-musl",
        "aarch64-unknown-linux-musl" => "aarch64-linux-musl",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        other => panic!("unsupported target for libghostty-vt build: {other}"),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    if from.is_dir() {
        fs::create_dir_all(to).expect("create ghostty source dir");
        for entry in fs::read_dir(from).expect("read ghostty source dir") {
            let entry = entry.expect("ghostty source entry");
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        fs::copy(from, to).unwrap_or_else(|err| panic!("copy {}: {err}", from.display()));
    }
}

// The submodule stays pristine upstream ghostty; our patches go onto a copy in OUT_DIR.
fn patched_ghostty(manifest_dir: &Path, out_dir: &Path) -> PathBuf {
    let upstream = manifest_dir.join("vendor/ghostty");
    assert!(
        upstream.join("build.zig").exists(),
        "vendor/ghostty is empty; run `git submodule update --init mux/vendor/ghostty`"
    );
    let source = out_dir.join("ghostty-src");
    if source.exists() {
        fs::remove_dir_all(&source).expect("clear ghostty source copy");
    }
    fs::create_dir_all(&source).expect("create ghostty source copy");
    for part in GHOSTTY_PARTS {
        copy_tree(&upstream.join(part), &source.join(part));
    }
    let mut patches: Vec<PathBuf> = fs::read_dir(manifest_dir.join("vendor/ghostty-patches"))
        .expect("read vendor/ghostty-patches")
        .map(|entry| entry.expect("patch entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "patch"))
        .collect();
    patches.sort();
    for patch in patches {
        let status = Command::new("patch")
            .args(["-p3", "--quiet", "--forward", "-d"])
            .arg(&source)
            .arg("-i")
            .arg(&patch)
            .status()
            .expect("run patch");
        assert!(status.success(), "applying {} failed", patch.display());
    }
    source
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/ghostty-patches");
    println!("cargo:rerun-if-changed=vendor/ghostty/build.zig.zon");
    println!("cargo:rerun-if-changed=vendor/ghostty/src");
    println!("cargo:rerun-if-changed=vendor/ghostty/pkg");
    println!("cargo:rerun-if-changed=vendor/ghostty/include");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SIMD");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_ZIG_SYSTEM_DIR");
    println!("cargo:rerun-if-env-changed=ZIG");
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let optimize = env::var("LIBGHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    let simd = !matches!(
        env::var("LIBGHOSTTY_VT_SIMD").as_deref(),
        Ok("0" | "false" | "no" | "off")
    );
    let target = env::var("TARGET").expect("TARGET");
    let source = patched_ghostty(&manifest_dir, &out_dir);

    let prefix = out_dir.join("zig-out");
    let zig = env::var("ZIG").unwrap_or_else(|_| "zig".into());
    let mut command = Command::new(&zig);
    command
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg(format!("-Dsimd={simd}"))
        .arg(format!("-Dtarget={}", zig_target(&target)))
        .arg(format!("-Dversion-string={GHOSTTY_VERSION}"))
        .arg("-Demit-xcframework=false")
        .arg("--cache-dir")
        .arg(out_dir.join("zig-cache"))
        .arg("--prefix")
        .arg(&prefix);
    if let Ok(system_dir) = env::var("LIBGHOSTTY_VT_ZIG_SYSTEM_DIR") {
        command.arg("--system").arg(system_dir);
    }
    let status = command.current_dir(&source).status().unwrap_or_else(|err| {
        panic!("failed to run {zig:?} (libghostty-vt needs Zig 0.16.0; set ZIG): {err}")
    });
    assert!(status.success(), "zig build for libghostty-vt failed: {status}");

    let lib_dir = prefix.join("lib");
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    if target.contains("apple-darwin") {
        println!(
            "cargo:rustc-link-arg={}",
            lib_dir.join("libghostty-vt.a").display()
        );
    } else {
        println!("cargo:rustc-link-lib=static=ghostty-vt");
    }
}
