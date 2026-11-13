//! Where a Runtime or TensorFS version PyPI lacks lives: a GitHub prerelease (cozy-runtime
//! `v<version>`, cozy-creator `tensorfs-v<version>`), whose `SHA256SUMS` names its wheels. The open
//! RC's newest builds (cl-922) are on its rolling prerelease `rc-<x.y.z>` in each repository, with the
//! `cozy` a machine runs (`cozy-<version>-linux-<arch>`): its Runtime as a dev release of the minor it makes
//! (`0.47.0.dev2260+g1a2b3c4d`, cl-952), so a package's floor can name the minor (`>=0.47.0.dev0`), and
//! each wheel kept an hour after the push that replaced it; TensorFS as `<version>+dev.g<sha8>`; cozy as the dev version of that minor with its commit (`0.26.0-dev+g1a2b3c4d`,
//! cl-976).

fn repo_and_prefix(distribution: &str) -> (&'static str, &'static str) {
    let repo = repo(distribution);
    (repo, if repo == "cozy-runtime" { "v" } else { "tensorfs-v" })
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
    let wheels: Vec<(&str, &str)> = listed(sums).filter(|(file, _)| file.starts_with(&prefix) && file.ends_with(".whl")).collect();
    (wheels.iter().find(|(file, _)| native(file, platform)))
        .or_else(|| wheels.iter().find(|(file, _)| file.ends_with("-none-any.whl")))
        .copied()
}

/// Whether wheel `file` is the stable-ABI one built for `platform`: a version-specific
/// (`cp310-cp310`) wheel fits one Python only.
fn native(file: &str, platform: &str) -> bool {
    let (os, arch) = platform.split_once('/').unwrap_or((platform, ""));
    file.contains("-abi3-") && match (os, arch) {
        ("linux", arch) => file.contains("manylinux") && file.contains(arch),
        ("macos", "aarch64") => file.contains("macosx") && file.contains("arm64"),
        ("macos", arch) => file.contains("macosx") && file.contains(arch),
        ("windows", "x86_64") => file.contains("win_amd64"),
        ("windows", _) => file.contains("win_arm64"),
        _ => false,
    }
}

/// Each `(file, sha256)` a `SHA256SUMS` names.
fn listed(sums: &str) -> impl Iterator<Item = (&str, &str)> {
    (sums.lines())
        .filter_map(|line| line.split_once(char::is_whitespace))
        .map(|(sha256, file)| (file.trim().trim_start_matches('*'), sha256.trim()))
        .filter(|(_, sha256)| sha256.len() == 64)
}

/// The repository whose releases hold `distribution` (`cozy` for the machine's binary).
pub fn repo(distribution: &str) -> &'static str {
    match distribution.replace('-', "_").as_str() {
        "cozy_runtime" => "cozy-runtime",
        _ => "cozy-creator",
    }
}

/// `repo`'s rolling prerelease of the open RC `rc` (`0.25.0`): its newest builds are under this.
pub fn rc(repo: &str, rc: &str) -> String {
    format!("https://github.com/cozy-creator/{repo}/releases/download/rc-{rc}")
}

/// One build a `SHA256SUMS` names: its file, sha256 and version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Build {
    pub file: String,
    pub sha256: String,
    pub version: String,
}

/// The build of `distribution` for `platform` (`linux/x86_64`) on an RC's prerelease: a native
/// wheel, never the pure one (it carries no kernels), or for `cozy` the machine's binary
/// (`cozy-<version>-<os>-<arch>`).
pub fn rc_build(sums: &str, distribution: &str, platform: &str) -> Option<Build> {
    let (os, arch) = platform.split_once('/').unwrap_or((platform, ""));
    let (file, sha256, version) = match distribution {
        // Its version may hold a `-` (`0.26.0-dev+g1a2b3c4d`).
        "cozy" => listed(sums).find_map(|(file, sha256)| {
            let version = file.strip_prefix("cozy-")?.strip_suffix(&format!("-{os}-{arch}"))?;
            Some((file, sha256, version))
        })?,
        _ => {
            let prefix = format!("{}-", distribution.replace('-', "_"));
            let (file, sha256) = listed(sums).find(|(file, _)| file.starts_with(&prefix) && file.ends_with(".whl") && native(file, platform))?;
            (file, sha256, file.split('-').nth(1)?)
        }
    };
    (!version.is_empty()).then(|| Build { file: file.to_owned(), sha256: sha256.to_owned(), version: version.to_owned() })
}

/// The pure wheel (`-py3-none-any.whl`) of `distribution` an RC's `SHA256SUMS` names: what a lock and a publish's floor check
/// read, on any platform.
pub fn rc_pure(sums: &str, distribution: &str) -> Option<Build> {
    let prefix = format!("{}-", distribution.replace('-', "_"));
    let (file, sha256) = listed(sums).find(|(file, _)| file.starts_with(&prefix) && file.ends_with("-py3-none-any.whl"))?;
    let version = file.split('-').nth(1).filter(|version| !version.is_empty())?;
    Some(Build { file: file.to_owned(), sha256: sha256.to_owned(), version: version.to_owned() })
}

/// Whether `version` is a dev build: the open RC's Runtime (`0.47.0.dev2260+g1a2b3c4d`) or cozy (`0.26.0-dev+g1a2b3c4d`), or
/// one built from a commit (`0.45.0+dev.gb976b092`), the RC's TensorFS or one by hand.
pub fn dev(version: &str) -> bool {
    version.contains("+dev.g") || version.contains("-dev") || version.parse::<pep440_rs::Version>().is_ok_and(|version| version.is_dev())
}

/// What a dev build is called in a line: the RC's Runtime by its version (`0.47.0.dev2260`), another by its commit
/// (`b976b092` of `0.45.0+dev.gb976b092`, `1a2b3c4d` of the RC's cozy `0.26.0-dev+g1a2b3c4d`); a release as it is.
pub fn shown(version: &str) -> &str {
    if let Some((_, commit)) = version.split_once("+dev.g") {
        return commit;
    }
    match version.split_once('+') {
        Some((public, local)) if public.contains("-dev") => local.strip_prefix('g').unwrap_or(local),
        Some((public, _)) if dev(public) => public,
        _ => version,
    }
}

