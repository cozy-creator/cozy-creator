//! Where a Runtime or TensorFS version PyPI lacks lives: a GitHub prerelease (cozy-runtime
//! `v<version>`, cozy-creator `tensorfs-v<version>`), whose `SHA256SUMS` names its wheels.

fn repo_and_prefix(distribution: &str) -> (&'static str, &'static str) {
    match distribution.replace('-', "_").as_str() {
        "cozy_runtime" => ("cozy-runtime", "v"),
        _ => ("cozy-creator", "tensorfs-v"),
    }
}

/// The release `version` of `distribution`: its files are under this.
pub fn base(distribution: &str, version: &str) -> String {
    let (repo, prefix) = repo_and_prefix(distribution);
    format!("https://github.com/cozy-creator/{repo}/releases/download/{prefix}{version}")
}

/// The GitHub API's list of `distribution`'s repository's releases, newest first.
pub fn releases(distribution: &str) -> String {
    let (repo, _) = repo_and_prefix(distribution);
    format!("https://api.github.com/repos/cozy-creator/{repo}/releases?per_page=100")
}

/// The version of `distribution` a release tag names, if it names one (cozy-creator's tags name
/// `cozy` too).
pub fn version_of<'a>(distribution: &str, tag: &'a str) -> Option<&'a str> {
    tag.strip_prefix(repo_and_prefix(distribution).1)
}

/// `distribution` `version`'s wheel for `platform` (`linux/x86_64`) in a release's `SHA256SUMS`, as
/// (file, sha256): the platform's own first, else the pure one.
pub fn wheel<'a>(sums: &'a str, distribution: &str, version: &str, platform: &str) -> Option<(&'a str, &'a str)> {
    let prefix = format!("{}-{version}-", distribution.replace('-', "_"));
    let wheels: Vec<(&str, &str)> = sums
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .map(|(sha256, file)| (file.trim().trim_start_matches('*'), sha256.trim()))
        .filter(|(file, sha256)| sha256.len() == 64 && file.starts_with(&prefix) && file.ends_with(".whl"))
        .collect();
    let (os, arch) = platform.split_once('/').unwrap_or((platform, ""));
    // A version-specific (`cp310-cp310`) wheel fits one Python only; the stable-ABI one fits all.
    let native = |file: &str| file.contains("-abi3-") && match (os, arch) {
        ("linux", arch) => file.contains("manylinux") && file.contains(arch),
        ("macos", "aarch64") => file.contains("macosx") && file.contains("arm64"),
        ("macos", arch) => file.contains("macosx") && file.contains(arch),
        ("windows", "x86_64") => file.contains("win_amd64"),
        ("windows", _) => file.contains("win_arm64"),
        _ => false,
    };
    (wheels.iter().find(|(file, _)| native(file)))
        .or_else(|| wheels.iter().find(|(file, _)| file.ends_with("-none-any.whl")))
        .copied()
}
