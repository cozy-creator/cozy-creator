//! `COZY_COMMIT`: the commit this build is from, which `machine show` reports. A build input
//! `COZY_COMMIT` wins (a container build without the repository), then git's HEAD, then CI's
//! `GITHUB_SHA`. `COZY_VERSION`: the version every part of cozy says (cl-976), the workspace's,
//! with the commit for the open RC's dev version (`0.26.0-dev+g1a2b3c4d`).
use std::{path::Path, process::Command};

fn main() {
    // Any owner: a container build reads a checkout mounted from another user.
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "safe.directory=*"])
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    // Rebuilt when HEAD moves: HEAD, the branch it names, and packed refs.
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    for name in ["HEAD", "packed-refs"].into_iter().chain(branch.as_deref()) {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            // Packed branches have no loose ref file. A missing watched input rebuilds
            // every time, so watch its existing ancestor for a newly written ref.
            let path = Path::new(&path);
            let watched = path
                .ancestors()
                .find(|candidate| candidate.exists())
                .unwrap_or(path);
            println!("cargo:rerun-if-changed={}", watched.display());
        }
    }
    println!("cargo:rerun-if-env-changed=COZY_COMMIT");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    let commit = std::env::var("COZY_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| git(&["rev-parse", "HEAD"]))
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=COZY_COMMIT={commit}");
    // A release, or one dev-build.yaml stamped already (`0.25.0+dev.g1a2b3c4d`), stays as it is.
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets it");
    let hex = commit.len() >= 8 && commit.bytes().all(|c| c.is_ascii_hexdigit());
    let version = match version.contains('-') && !version.contains('+') && hex {
        true => format!("{version}+g{}", &commit[..8]),
        false => version,
    };
    println!("cargo:rustc-env=COZY_VERSION={version}");
}
