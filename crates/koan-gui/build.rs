//! The build the About panel names: the commit the binary was built from,
//! or `KOAN_BUILD` where a release sets one.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=KOAN_BUILD");
    // A commit or a checkout moves the worktree's HEAD log.
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
    let build = std::env::var("KOAN_BUILD")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| {
            git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into())
        });
    println!("cargo:rustc-env=KOAN_BUILD={build}");
}

fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}
