//! A private window of the user's own browser (cl-1067; Paul, 11-16: "always open an incognito browser window, no matter what", so a
//! project's pages stay out of the browser's history and its files out of its cache). Started, then left alone.
use std::{
    ffi::OsString,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Each family's private-window flag, by a word in its program, app or handler name.
const FAMILIES: &[(&str, &str)] = &[("firefox", "--private-window"), ("edge", "--inprivate"), ("brave", "--incognito"), ("chrom", "--incognito")];

fn family(name: &str) -> Option<usize> {
    let name = name.to_lowercase();
    FAMILIES.iter().position(|(word, _)| name.contains(word))
}

fn flag(name: &str) -> Option<&'static str> {
    family(name).map(|at| FAMILIES[at].1)
}

/// `url` in a private window: of `$BROWSER`'s first browser that has one, else of the system's default browser, else of the first known
/// one installed. Which it opened, or why none did.
pub(super) async fn open(url: &str) -> Result<String, String> {
    let found = candidates(url);
    let mut why = Vec::new();
    for (name, words) in &found {
        match start(words).await {
            Ok(()) => return Ok(name.clone()),
            Err(failed) => why.push(failed),
        }
    }
    if found.is_empty() {
        why.push(if cfg!(any(target_os = "macos", windows)) || display() { "none found" } else { "no display" }.into());
    }
    Err(why.join("; "))
}

/// Each browser to try, by name, as the words that start its private window at `url`.
fn candidates(url: &str) -> Vec<(String, Vec<String>)> {
    let separator = if cfg!(windows) { ';' } else { ':' };
    let mut found = Vec::new();
    for command in std::env::var("BROWSER").unwrap_or_default().split(separator) {
        let mut words: Vec<String> = command.split_whitespace().map(|word| word.replace("%s", url)).collect();
        let Some(program) = words.first() else { continue };
        let name = Path::new(program).file_stem().map_or(program.clone(), |stem| stem.to_string_lossy().into_owned());
        let Some(flag) = flag(&name) else { continue };
        words.insert(1, flag.into());
        if !command.contains("%s") { words.push(url.into()); }
        found.push((name, words));
    }
    found.extend(system(url));
    found
}

fn display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// What `program` prints, when it answers.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let done = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    done.status.success().then(|| String::from_utf8_lossy(&done.stdout).into_owned())
}

/// The default browser's family first (`xdg-settings`), then the others on PATH. None without a display.
#[cfg(not(any(target_os = "macos", windows)))]
fn system(url: &str) -> Vec<(String, Vec<String>)> {
    const KNOWN: &[&str] = &["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "brave-browser", "microsoft-edge", "microsoft-edge-stable", "firefox"];
    if !display() { return Vec::new(); }
    let default = output("xdg-settings", &["get", "default-web-browser"]).and_then(|name| family(&name));
    let path: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    let mut known: Vec<&str> = KNOWN.iter().copied().filter(|name| platform::tools::on_path(name, &path).is_some()).collect();
    known.sort_by_key(|name| default.is_none() || family(name) != default);
    known.into_iter().filter_map(|name| Some((name.to_owned(), vec![name.to_owned(), flag(name)?.to_owned(), url.to_owned()]))).collect()
}

/// The default browser (LaunchServices' https handler), then the others: each by its bundle id, a new instance of it (`open -n`), which
/// hands the window to the one running.
#[cfg(target_os = "macos")]
fn system(url: &str) -> Vec<(String, Vec<String>)> {
    const KNOWN: &[&str] = &["com.google.Chrome", "com.brave.Browser", "com.microsoft.edgemac", "org.mozilla.firefox", "org.chromium.Chromium"];
    let default = default_handler().filter(|id| flag(id).is_some());
    let others = KNOWN.iter().filter(|id| !default.as_ref().is_some_and(|held| held.eq_ignore_ascii_case(id))).map(|id| (*id).to_owned());
    let ids: Vec<String> = default.clone().into_iter().chain(others).collect();
    ids.into_iter().filter_map(|id| {
        let words = ["open", "-nb", id.as_str(), "--args", flag(&id)?, url].map(str::to_owned).to_vec();
        Some((id, words))
    }).collect()
}

/// `defaults` lists each handler's keys in order, so its `LSHandlerRoleAll` comes just before its `LSHandlerURLScheme = https`.
#[cfg(target_os = "macos")]
fn default_handler() -> Option<String> {
    let said = output("defaults", &["read", "com.apple.LaunchServices/com.apple.launchservices.secure", "LSHandlers"])?;
    let before = &said[..said.find("LSHandlerURLScheme = https;")?];
    let role = &before[before.rfind("LSHandlerRoleAll = ")? + "LSHandlerRoleAll = ".len()..];
    Some(role.split(';').next()?.trim().trim_matches('"').to_owned())
}

/// The default browser (the https handler the user chose), then the others where they install.
#[cfg(windows)]
fn system(url: &str) -> Vec<(String, Vec<String>)> {
    const KNOWN: &[&str] = &[r"Google\Chrome\Application\chrome.exe", r"BraveSoftware\Brave-Browser\Application\brave.exe", r"Microsoft\Edge\Application\msedge.exe", r"Mozilla Firefox\firefox.exe"];
    let mut exes: Vec<std::path::PathBuf> = default_handler().into_iter().collect();
    for root in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].into_iter().filter_map(std::env::var_os) {
        for known in KNOWN {
            let exe = Path::new(&root).join(known);
            if exe.is_file() && !exes.contains(&exe) { exes.push(exe); }
        }
    }
    exes.into_iter().filter_map(|exe| {
        let name = exe.file_stem()?.to_string_lossy().into_owned();
        let words = vec![exe.to_string_lossy().into_owned(), flag(&name)?.to_owned(), url.to_owned()];
        Some((name, words))
    }).collect()
}

/// The program of the https handler's open command (`"C:\…\chrome.exe" --single-argument %1`).
#[cfg(windows)]
fn default_handler() -> Option<std::path::PathBuf> {
    let value = |key: &str, name: &[&str]| -> Option<String> {
        let said = output("reg", &[&["query", key][..], name].concat())?;
        said.lines().find_map(|line| line.split_once("REG_SZ").or_else(|| line.split_once("REG_EXPAND_SZ")).map(|(_, value)| value.trim().to_owned()))
    };
    let id = value(r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice", &["/v", "ProgId"])?;
    let command = value(&format!(r"HKCR\{id}\shell\open\command"), &["/ve"])?;
    let exe = match command.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next()?,
        None => command.split_whitespace().next()?,
    };
    Some(std::path::PathBuf::from(exe))
}

/// `words` started on their own, given a moment to fail: one that exits well, or is still going, opened the window.
async fn start(words: &[String]) -> Result<(), String> {
    let program = &words[0];
    let args: Vec<OsString> = words[1..].iter().map(OsString::from).collect();
    let mut started = platform::process::detach(Path::new(program), &args, None).map_err(|e| format!("{program}: {e}"))?;
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        match started.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} {}", platform::process::ended(&status).0)),
            Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(e) => return Err(format!("{program}: {e}")),
        }
    }
    Ok(())
}
