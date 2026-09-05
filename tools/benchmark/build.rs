use std::{path::PathBuf, process::Command};
fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()
            .filter(|r| r.status.success())
            .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string())
    };
    let revision = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"]).is_none_or(|s| !s.is_empty());
    println!("cargo:rustc-env=USHAS_BENCH_SOURCE_REVISION={revision}");
    println!("cargo:rustc-env=USHAS_BENCH_SOURCE_DIRTY={dirty}");
    for path in [".git/HEAD", ".git/refs/heads/main", ".git/index"] {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../claude-model/src");
}
