use std::process::Command;

fn main() {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir("../..")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_else(|| "unknown".into())
    };
    println!(
        "cargo:rustc-env=USHAS_SOURCE_REV={}",
        git(&["rev-parse", "HEAD"])
    );
    println!(
        "cargo:rustc-env=USHAS_SOURCE_DIRTY={}",
        !git(&["status", "--porcelain"]).is_empty()
    );
    for path in [
        "../../src",
        "../../Cargo.toml",
        "../../.git/HEAD",
        "../../.git/refs/heads/main",
        "src",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
}
