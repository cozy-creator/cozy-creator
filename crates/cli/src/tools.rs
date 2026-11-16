//! The tools this computer has of those cozy runs and never bundles, as `cozy doctor` shows them.
//! Calcifer fetches the ones cozy installs itself, on first need.
use std::path::PathBuf;

use platform::tools::{self, Class, Found, Install, Side, TOOLS, Tool, install_hint};
use serde::Serialize;

use crate::Context;

/// One tool as `cozy doctor` shows it.
#[derive(Serialize)]
pub(crate) struct Row {
    pub name: &'static str,
    pub side: &'static str,
    pub class: &'static str,
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    pub cost: &'static str,
    /// What installs it here: a cozy verb, the OS's own command, or what to install.
    pub install: String,
}

/// Every tool this computer uses, probed now.
pub(crate) fn rows(ctx: &Context, side: Side) -> Vec<Row> {
    let path = path();
    TOOLS
        .iter()
        .filter(|tool| tool.side == side && tools::used_here(tool))
        .map(|tool| {
            let found: Option<Found> = (tool.probe)(&ctx.home.tools(), &path);
            Row {
                name: tool.name,
                side: match tool.side {
                    Side::Computer => "computer",
                    Side::Machine => "machine",
                },
                class: match tool.class {
                    Class::Essential => "essential",
                    Class::Optional => "optional",
                },
                found: found.is_some(),
                version: found
                    .as_ref()
                    .map(|f| f.version.clone())
                    .unwrap_or_default(),
                path: found.map(|f| f.path),
                cost: tool.cost,
                install: install_hint(tool, &path),
            }
        })
        .collect()
}

/// A row as a line of `cozy doctor`.
pub(crate) fn cells(row: &Row) -> [String; 3] {
    match row.found {
        true => [
            row.name.into(),
            match row.version.is_empty() {
                true => "found".into(),
                false => row.version.clone(),
            },
            row.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ],
        false => [
            row.name.into(),
            format!("missing: {}", row.cost),
            row.install.clone(),
        ],
    }
}

fn path() -> Vec<PathBuf> {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default()
}


/// What this computer lacks of what cozy needs, installed now (`cozy doctor --fix`, which the installers run, cl-1030): the system
/// packages in one go through its package manager, asked for once before `sudo` (or as root or an administrator, without asking), else
/// the exact command said; then what cozy otherwise fetches at its first need. Nothing here fails: what isn't installed stays as
/// `cozy doctor` lists it, and a feature refuses with its install named, as before.
pub(crate) async fn fix(ctx: &Context) {
    let say = |line: &str| match ctx.json {
        true => eprintln!("{line}"),
        false => println!("{line}"),
    };
    let path = path();
    let missing = || -> Vec<&'static Tool> {
        TOOLS
            .iter()
            .filter(|tool| tools::used_here(tool) && matches!(tool.install, Install::System(_)))
            // A machine's own tools are its image's or its toolchain's, but for the C compiler Triton needs on an NVIDIA computer.
            .filter(|tool| tool.side == Side::Computer || (tool.name == "cc" && nvidia()))
            .filter(|tool| (tool.probe)(&ctx.home.tools(), &path).is_none())
            .collect()
    };
    let wanted = missing();
    if !wanted.is_empty() {
        system(&wanted, &path, &say).await;
        for tool in missing() {
            say(&format!("{} is still missing: {}; {}", tool.name, tool.cost, install_hint(tool, &path)));
        }
    }
    for (name, fetched) in calcifer::prefetch(&ctx.home, &say).await {
        match fetched {
            Ok(build) => say(&format!("{name}: ready ({build})")),
            Err(why) => say(&format!("{name}: not fetched now ({why}); cozy fetches it when a feature first needs it")),
        }
    }
}

/// Why `tool` is wanted, as the one question says it.
fn why(tool: &Tool) -> String {
    match tool.name {
        "cc" => "Triton kernels need a C compiler".into(),
        _ => format!("without it, {}", tool.cost),
    }
}

/// Whether this computer has an NVIDIA GPU its driver serves.
fn nvidia() -> bool {
    cfg!(target_os = "linux") && std::path::Path::new("/proc/driver/nvidia/version").exists()
}

/// The system packages `tools` come in, installed through this computer's package manager in one command.
async fn system(wanted: &[&'static Tool], path: &[PathBuf], say: &(dyn Fn(&str) + Sync)) {
    let package = |tool: &Tool| match tool.install {
        Install::System(package) => package,
        _ => tool.name,
    };
    let listed = wanted.iter().map(|tool| format!("  {}: {}", tool.name, why(tool))).collect::<Vec<_>>().join("\n");
    match std::env::consts::OS {
        "linux" => linux(wanted, path, &listed, say),
        "windows" => windows(wanted, path, &listed, say).await,
        // hdiutil is built in; git comes with Apple's command line tools.
        _ => {
            for tool in wanted {
                let hint = match package(tool) {
                    "git" => "xcode-select --install".to_owned(),
                    _ => install_hint(tool, path),
                };
                say(&format!("{} is missing ({}): {hint}", tool.name, why(tool)));
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn linux(_: &[&'static Tool], _: &[PathBuf], _: &str, _: &(dyn Fn(&str) + Sync)) {}

#[cfg(target_os = "linux")]
fn linux(wanted: &[&'static Tool], path: &[PathBuf], listed: &str, say: &(dyn Fn(&str) + Sync)) {
    use std::process::{Command, Stdio};
    let Some(manager) = tools::package_manager(path) else {
        say(&format!("cozy needs these, and this computer has no package manager cozy knows (apt-get, dnf, pacman, zypper):\n{listed}"));
        return;
    };
    let packages = wanted
        .iter()
        .map(|tool| match tool.install {
            Install::System(package) => tools::package_name(manager, package),
            _ => tool.name,
        })
        .collect::<Vec<_>>()
        .join(" ");
    // As root, and as the person runs it with sudo.
    let (script, said) = match manager {
        "apt-get" => (
            format!("apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y {packages}"),
            format!("sudo apt-get update && sudo apt-get install -y {packages}"),
        ),
        "dnf" => (format!("dnf install -y {packages}"), format!("sudo dnf install -y {packages}")),
        "pacman" => (format!("pacman -S --needed --noconfirm {packages}"), format!("sudo pacman -S --needed --noconfirm {packages}")),
        _ => (format!("zypper --non-interactive install {packages}"), format!("sudo zypper --non-interactive install {packages}")),
    };
    let root = Command::new("id").arg("-u").output().is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "0");
    let mut command = if root {
        say(&format!("installing what cozy needs with {manager}:\n{listed}"));
        let mut command = Command::new("sh");
        command.args(["-c", &script]).stdin(Stdio::null());
        command
    } else {
        // Asked once, on the terminal itself: an installer's stdin is the script it's read from.
        let tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty");
        let sudo = tools::on_path("sudo", path).is_some();
        let (Ok(mut tty), true) = (tty, sudo) else {
            say(&format!("cozy needs these on this computer:\n{listed}\nTo install them, run:\n  {said}"));
            return;
        };
        use std::io::{BufRead as _, Write as _};
        let _ = write!(tty, "cozy needs these on this computer:\n{listed}\nInstall them now with sudo {manager}? [Y/n] ");
        let mut answer = String::new();
        let _ = std::io::BufReader::new(&tty).read_line(&mut answer);
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
            say(&format!("Not installed. To install them later, run:\n  {said}"));
            return;
        }
        let mut command = Command::new("sudo");
        command.args(["sh", "-c", &script]);
        if let Ok(input) = tty.try_clone() {
            command.stdin(input);
        }
        command
    };
    match command.status() {
        Ok(status) if status.success() => say("installed"),
        Ok(status) => say(&format!("{manager} said no ({status}). To install them, run:\n  {said}")),
        Err(error) => say(&format!("{manager} couldn't start ({error}). To install them, run:\n  {said}")),
    }
}

#[cfg(not(windows))]
async fn windows(_: &[&'static Tool], _: &[PathBuf], _: &str, _: &(dyn Fn(&str) + Sync)) {}

/// Through winget, but WinFsp through its own pinned installer with every feature, as the vault provider's proof installs it
/// (dev-build.yaml); as an administrator without asking, else asked once on the console.
#[cfg(windows)]
async fn windows(wanted: &[&'static Tool], path: &[PathBuf], listed: &str, say: &(dyn Fn(&str) + Sync)) {
    use std::{io::IsTerminal as _, process::Command};
    let winget = tools::package_manager(path).is_some();
    let package = |tool: &Tool| match tool.install {
        Install::System(package) => package,
        _ => tool.name,
    };
    let said = wanted
        .iter()
        .map(|tool| match (winget, package(tool)) {
            (_, "winfsp") => format!("download {} and run msiexec /i winfsp-2.1.25156.msi /qn ADDLOCAL=ALL", tools::WINFSP_MSI.0),
            (true, name) => format!("winget install --exact --id {} --silent --accept-package-agreements --accept-source-agreements", tools::package_name("winget", name)),
            (false, "vcruntime") => "download and run https://aka.ms/vs/17/release/vc_redist.x64.exe".into(),
            (false, "git") => "download and run Git for Windows from https://git-scm.com/download/win".into(),
            (false, name) => format!("install {name}"),
        })
        .collect::<Vec<_>>();
    let admin = Command::new("powershell")
        .args(["-NoProfile", "-Command", "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "True");
    if !admin {
        if !std::io::stdin().is_terminal() {
            say(&format!("cozy needs these on this computer:\n{listed}\nTo install them:\n  {}", said.join("\n  ")));
            return;
        }
        print!("cozy needs these on this computer:\n{listed}\nInstall them now? [Y/n] ");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
            say(&format!("Not installed. To install them later:\n  {}", said.join("\n  ")));
            return;
        }
    } else {
        say(&format!("installing what cozy needs:\n{listed}"));
    }
    for (tool, said) in wanted.iter().zip(&said) {
        let done = match (winget, package(tool)) {
            (_, "winfsp") => winfsp(admin).await,
            (true, name) => Command::new("winget")
                .args(["install", "--exact", "--id", tools::package_name("winget", name), "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"])
                .status()
                .map(|status| status.success() || matches!(status.code(), Some(-1978335189 | -1978335135)))
                .unwrap_or(false),
            _ => false,
        };
        if !done {
            say(&format!("{} isn't installed; {said}", tool.name));
        }
    }
}

/// WinFsp's pinned installer, held to its digest, run quietly with every feature: elevated through Windows' own prompt where this isn't
/// an administrator.
#[cfg(windows)]
async fn winfsp(admin: bool) -> bool {
    use std::process::Command;
    let (url, sha256) = tools::WINFSP_MSI;
    let Ok(dir) = tempfile::tempdir() else { return false };
    let msi = dir.path().join("winfsp.msi");
    match tensorfs::fetch::get_pinned_into(url, &msi, &mut |_: u64| {}).await {
        Ok((file, got)) if got == sha256 => drop(file),
        _ => return false,
    }
    let quiet = |status: std::process::ExitStatus| matches!(status.code(), Some(0 | 3010));
    match admin {
        true => Command::new("msiexec").arg("/i").arg(&msi).args(["/qn", "/norestart", "ADDLOCAL=ALL"]).status().is_ok_and(quiet),
        false => Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(format!("exit (Start-Process msiexec -ArgumentList '/i','\"{}\"','/qn','/norestart','ADDLOCAL=ALL' -Verb RunAs -Wait -PassThru).ExitCode", msi.display()))
            .status()
            .is_ok_and(quiet),
    }
}
