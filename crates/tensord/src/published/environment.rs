//! Package environments, built one way for a release (`published`) and for local code
//! (`local_source`): uv's Python, the locked (or resolved) dependencies, wheels from files, the
//! project, this machine's Runtime/TensorFS pair over them with its bytecode compiled, then the
//! environment's description by its own Runtime and its record. uv runs under one environment.
use super::{canceled, io_failure, Failure, Job, Locked, Publisher, SDK};
use crate::{catalog::normalized, machine::update, process::Supervised};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    process::{Command, Stdio},
};

/// What one environment is built from.
pub(crate) struct Project<'a> {
    /// Its pyproject.toml's directory (and its uv.lock's, when `locked`).
    pub dir: &'a Path,
    pub python: &'a str,
    pub locked: Option<&'a Locked>,
    /// Wheels installed from these files at the versions their names say, before the project: a
    /// release's Hub-hosted ones (its own included), a local package's carried ones.
    pub wheels: Option<&'a Path>,
    /// Lock rows left to `wheels` besides their own (a release's git rows its Hub carries).
    pub skip: &'a [String],
    /// Local code: the project installed editable, read where it is.
    pub editable: bool,
    /// Indexes an unlocked project's sources name without declaring (name → URL).
    pub indexes: &'a BTreeMap<String, String>,
    /// `{package} {release}`, as a person reads it.
    pub label: &'a str,
    /// What the person sees while it is built; its steps are counted after it.
    pub stage: &'a str,
    /// A first step of the caller's (a release's Hub files, fetched into `wheels`).
    pub fetch: Option<&'a dyn Fn() -> Result<(), Failure>>,
}

/// What its record says of the Runtime pair it holds.
#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct Built {
    /// `machine`: this machine's pair; `locked`: the package's own.
    pub sdk: String,
    /// What the run is told of the pair (uv's findings, a development Runtime): empty for nothing.
    pub fallback: String,
}

/// The pair's step: refused for a floor the pair misses, with what it misses.
enum Pair {
    Floor(String),
    Failed(Failure),
}

impl Publisher {
    /// Builds `env` for `project` (an interrupted build there is gone first).
    pub(crate) fn build(&self, env: &Path, project: &Project, job: &Job) -> Result<Built, Failure> {
        let own_sdk = !self.sdk.requirements.is_empty();
        let editable_after = project.editable && project.locked.is_some();
        // The last uv step compiles the whole environment's bytecode: executors run `-I` from an
        // environment they may not write, so uncompiled modules compile again in every executor
        // (SDXL's imports: 10.7 s uncompiled, 4.5 s compiled; J/BREAKDOWN.md).
        let steps = [true, project.locked.is_some() && project.wheels.is_some(), editable_after, own_sdk];
        let last = steps.iter().rposition(|s| *s);
        let compile = |step: usize| if Some(step) == last { "--compile-bytecode" } else { "--no-compile-bytecode" };
        let total = steps.iter().filter(|s| **s).count() + 1 + usize::from(project.fetch.is_some());
        let step = std::cell::Cell::new(0);
        let next = || {
            step.set(step.get() + 1);
            job.stage(format!("{} {}/{total}", project.stage, step.get()));
        };
        if let Some(fetch) = project.fetch {
            next();
            fetch()?;
        }
        let carried = carried(project.wheels).map_err(io_failure)?;
        if env.exists() {
            fs::remove_dir_all(env).map_err(io_failure)?;
        }
        let (dir, envs, py) = (project.dir.to_string_lossy(), env.to_string_lossy(), platform::paths::venv_python(env));
        let py = py.to_string_lossy();
        next();
        self.uv(&["venv", "--no-project", "--python", project.python, &envs], &[], job)?;
        next();
        let pair: Vec<&str> = SDK.into_iter().filter(|_| own_sdk).collect();
        match project.locked {
            Some(locked) => {
                let mut args = vec!["sync", "--frozen", "--inexact", "--no-dev", "--no-editable", "--no-install-project",
                    "--project", &dir, "--python", &py, compile(0)];
                let held = carried.iter().map(|(name, _)| name.as_str()).chain(project.skip.iter().map(String::as_str))
                    .chain(pair.iter().copied().filter(|name| locked.packages.contains_key(*name)));
                for name in held {
                    args.extend(["--no-install-package", name]);
                }
                // uv keys a directory it built by its metadata alone: changed code builds again.
                for name in &locked.local {
                    args.extend(["--refresh-package", name]);
                }
                let placed = [("UV_PROJECT_ENVIRONMENT", env.as_os_str())];
                self.uv(&args, &placed, job).map_err(|failure| platform(failure, project.label))?;
                // An App the lock holds is described from its installed files, which the Runtime's
                // static reader refuses as links into an image's seeded cache: those are copied.
                let linked = linked_applications(env);
                if self.sdk.uv.seeded() && !linked.is_empty() {
                    args.extend(["--link-mode", "copy"]);
                    for name in &linked {
                        args.extend(["--reinstall-package", name]);
                    }
                    self.uv(&args, &placed, job)?;
                }
            }
            None => {
                let mut args = vec!["pip", "install", "--python", &py, compile(0)];
                if let Some(wheels) = project.wheels {
                    args.extend(["--find-links", wheels.to_str().unwrap_or_default()]);
                }
                let named: Vec<String> = project.indexes.iter().map(|(name, url)| format!("--index={name}={url}")).collect();
                args.extend(named.iter().map(String::as_str));
                args.extend(["--editable", &dir]);
                // The pair's wheels are offered where the project's bounds admit them: PyPI's may
                // not install here (no macOS TensorFS wheel).
                let offered = [args.clone(), self.sdk.requirements.iter().map(String::as_str).collect()].concat();
                if !(own_sdk && self.uv(&offered, &[], job).is_ok()) {
                    self.uv(&args, &[], job)?;
                }
            }
        }
        if let Some(wheels) = project.wheels.filter(|_| steps[1]) {
            next();
            let pins: Vec<String> = carried.iter().map(|(name, version)| format!("{name}=={version}")).collect();
            let mut args = vec!["pip", "install", "--python", &py, "--no-deps", "--no-index", "--link-mode", "copy",
                "--find-links", wheels.to_str().unwrap_or_default(), compile(1)];
            args.extend(pins.iter().map(String::as_str));
            self.uv(&args, &[], job)?;
        }
        if editable_after {
            next();
            // Its dependencies are in place: the project alone, whatever indexes its sources name.
            self.uv(&["pip", "install", "--python", &py, "--no-deps", "--no-sources", "--editable", &dir, compile(2)], &[], job)?;
        }
        if project.editable {
            live_paths(env).map_err(io_failure)?;
        }
        let mut fallback = String::new();
        if own_sdk {
            next();
            // The pair the package itself selects: its lock's, else what its resolution installed.
            let wanted = match project.locked {
                Some(locked) => locked.packages.clone(),
                None => installed(env),
            };
            fallback = match self.pair(env, &py, compile(3), job) {
                Ok(warning) => warning,
                Err(Pair::Failed(failure)) => return Err(failure),
                Err(Pair::Floor(floors)) => {
                    let _ = fs::remove_dir_all(env);
                    return Err(self.meet_floor(project.label, &floors, &wanted, job));
                }
            };
            if !fallback.is_empty() {
                tracing::warn!("package {}: {fallback}", project.label);
                job.stage(format!("warning: {fallback}"));
            }
        }
        Ok(Built { sdk: if own_sdk { "machine" } else { "locked" }.into(), fallback })
    }

    /// This machine's own Runtime and TensorFS over the environment. Every other installed version
    /// stays exact: they constrain the install, and a pin the pair's own requirements reject keeps
    /// its version while the pair goes in without its dependencies. A floor the package puts on the
    /// pair that it misses (Fibo's `cozy-runtime>=0.30.16` for #1700's BatchNorm fix) is `Floor`,
    /// never a run on a Runtime too old for it. What else uv's check finds that the pair adds (a
    /// ceiling a newer pair passes: version skew is allowed) is the run's warning.
    fn pair(&self, env: &Path, py: &str, compile: &str, job: &Job) -> Result<String, Pair> {
        let failed = Pair::Failed;
        let constraints = env.join("cozy-constraints.txt");
        let pins: String = installed(env).iter().filter(|(name, _)| !SDK.contains(&name.as_str()))
            .map(|(name, version)| format!("{name}=={version}\n")).collect();
        fs::write(&constraints, pins).map_err(|e| failed(io_failure(e)))?;
        let constraints = constraints.to_string_lossy();
        let links = self.sdk.find_links.as_ref().map(|p| p.to_string_lossy().to_string());
        let install = |how: &[&str]| {
            let mut args = vec!["pip", "install", "--python", py, compile];
            args.extend(how);
            if let Some(links) = &links {
                args.extend(["--find-links", links]);
            }
            args.extend(self.sdk.requirements.iter().map(String::as_str));
            self.uv(&args, &[], job)
        };
        // What the package's own closure reports already stands (NVIDIA's aarch64 cusparseLt
        // wheel is tagged `sbsa`, so uv flags it on every Linux arm64 machine).
        let before = self.check(py, job).map_err(failed)?;
        let mut warnings = vec![];
        if let Err((code, detail)) = install(&["--constraints", &constraints]) {
            if code == super::CANCELED {
                return Err(failed((code, detail)));
            }
            install(&["--no-deps"]).map_err(failed)?;
            warnings.push(format!(
                "this machine's Runtime and TensorFS want versions the package's own do not hold, which it keeps ({})",
                crate::triage::ends(&detail, 1000)
            ));
        }
        let added: Vec<String> = self.check(py, job).map_err(failed)?.difference(&before).cloned().collect();
        let floors: Vec<&str> = added.iter().filter_map(|line| floor(line)).collect();
        if !floors.is_empty() {
            return Err(Pair::Floor(floors.join(" and ")));
        }
        if !added.is_empty() {
            warnings.push(format!("uv pip check: {}", added.join("; ")));
        }
        let build = &self.sdk.runtime_build;
        if !warnings.is_empty() && !build.is_empty() {
            warnings.push(format!("this machine's development Runtime {build} (`--runtime <path>`) runs it regardless"));
        }
        Ok(warnings.join("; "))
    }

    /// A package needing a newer Runtime than this machine's: the machine updates its pair to the
    /// package's own versions (never older than it runs), and the run prepares again on it after
    /// the restart the update ends with. The refusal when it cannot; a cancel ends the wait.
    fn meet_floor(&self, label: &str, floors: &str, wanted: &BTreeMap<String, String>, job: &Job) -> Failure {
        let refuse = |why: String| ("runtime_floor_unmet", format!("{label} needs {floors}, and {why}"));
        let build = &self.sdk.runtime_build;
        if !build.is_empty() {
            return refuse(format!("this machine runs development Runtime {build} (`cozy machine update --runtime <path>`)"));
        }
        let Some(updates) = &self.updates else {
            return refuse("this machine does not update itself".into());
        };
        let running = updates.software();
        let newer = |name: &str, running: &str| {
            wanted.get(name).filter(|version| older(running, version)).map(|version| update::Choice { version: version.clone(), ..Default::default() })
        };
        let (runtime, tensorfs) = (newer("cozy-runtime", &running.runtime), newer("tensorfs", &running.tensorfs));
        if runtime.is_none() && tensorfs.is_none() {
            return refuse(format!("it names no Runtime newer than this machine's {}", running.runtime));
        }
        let to = |choice: &Option<update::Choice>, running: &str| choice.as_ref().map_or(running.to_string(), |c| c.version.clone());
        let (to_runtime, to_tensorfs) = (to(&runtime, &running.runtime), to(&tensorfs, &running.tensorfs));
        let operation = format!("floor-{to_runtime}-{to_tensorfs}").replace('+', ".");
        let request = update::Request {
            operation: operation.clone(),
            pin: None,
            runtime,
            tensorfs,
            cozy: None,
            gpu_memory_limit: None,
            binary: None,
            toolchain: false,
        };
        let stop = job.stop.clone().unwrap_or_default();
        let pause = || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            stop.is_cancelled()
        };
        loop {
            match updates.request(request.clone(), |code| std::process::exit(code)) {
                Ok(_) => break,
                // Another update runs: this one asks once it ends (its restart prepares this run again).
                Err((409, _)) => {
                    job.waiting(Some(format!("waiting for this machine's update in progress ({label} needs {floors})")));
                    if pause() {
                        return canceled();
                    }
                }
                Err((_, why)) => return refuse(format!("this machine's update to Runtime {to_runtime} was refused: {why}")),
            }
        }
        tracing::info!("package {label} needs {floors}: updating to Runtime {to_runtime}, TensorFS {to_tensorfs}");
        job.waiting(Some(format!("waiting for this machine's update to Runtime {to_runtime} ({label} needs {floors})")));
        loop {
            if let Some(status) = updates.update(&operation).filter(|status| status.terminal()) {
                return refuse(format!("this machine's update to Runtime {to_runtime} ended {}: {}", status.state, status.error));
            }
            if pause() {
                return canceled();
            }
        }
    }

    /// `uv pip check`'s findings in an environment, one line each (`The package …`).
    fn check(&self, py: &str, job: &Job) -> Result<BTreeSet<String>, Failure> {
        match self.uv(&["pip", "check", "--python", py], &[], job) {
            Ok(()) => Ok(BTreeSet::new()),
            Err((code, detail)) => {
                let found: BTreeSet<String> = detail.lines().map(str::trim).filter(|l| l.starts_with("The package ")).map(String::from).collect();
                if found.is_empty() { Err((code, detail)) } else { Ok(found) }
            }
        }
    }

    /// uv as the machine runs it (`machine::uv`). `job` hears the bytes it writes about once a
    /// second; its process group is killed when the run is canceled (`process::supervise`).
    pub(crate) fn uv(&self, args: &[&str], env: &[(&str, &std::ffi::OsStr)], job: &Job) -> Result<(), Failure> {
        let mut command = self.sdk.uv.command().map_err(|e| ("uv_unavailable", e.to_string()))?;
        command.args(args).envs(env.iter().copied()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        platform::process::own_group(&mut command);
        let mut child = command.spawn().map_err(|e| ("package_installation_uv_absent", format!("cannot run uv: {e}")))?;
        let mut stderr = child.stderr.take().expect("piped uv stderr");
        let (ended, stderr) = std::thread::scope(|scope| {
            let drained = scope.spawn(move || {
                let mut kept = Vec::new();
                let _ = stderr.read_to_end(&mut kept);
                kept
            });
            let ended = crate::process::supervise(&mut child, job.stop.as_ref(), &|read| job.bytes(read, 0));
            (ended, drained.join().unwrap_or_default())
        });
        let step = args[..2.min(args.len())].join(" ");
        match ended.map_err(|e| ("package_installation_uv_failed", format!("uv: {e}")))? {
            Supervised::Exited(status) if status.success() => Ok(()),
            Supervised::Exited(_) => Err(("package_installation_uv_failed", format!("uv {step}:\n{}", stderr_tail(&stderr)))),
            Supervised::Canceled => Err(canceled()),
        }
    }
}

/// The generation at `dir` published: its record written last, by rename, so only a whole
/// environment ever resolves (`catalog`).
pub(crate) fn publish(dir: &Path, record: &Value) -> Result<(), Failure> {
    let staged = dir.join(".generation.json.new");
    let mut file = OpenOptions::new().create_new(true).write(true).open(&staged).map_err(io_failure)?;
    file.write_all(record.to_string().as_bytes()).and_then(|_| file.sync_all()).map_err(io_failure)?;
    fs::rename(&staged, dir.join("generation.json")).map_err(io_failure)?;
    platform::durable::sync_dir(dir)
        .and_then(|()| platform::durable::sync_dir(dir.parent().unwrap_or(dir)))
        .map_err(|error| io_failure(error.into()))
}

impl Built {
    /// The record of the environment at `env` (a generation's): the package (distribution and
    /// version), its App and interface, the pair it holds, its description.
    pub(crate) fn record(&self, identity: &str, (distribution, version): (&str, &str), env: &Path, interface: &Value,
        runtime_build: &str, (source_digest, callees): (String, Vec<crate::catalog::Callee>)) -> Result<Value, Failure> {
        let application = protocol::interface::application(interface)
            .ok_or(("package_prepare_interface_invalid", "the package interface names no application".to_string()))?;
        let dependencies: Vec<Value> = installed(env).into_iter().filter(|(name, _)| SDK.contains(&name.as_str()))
            .map(|(name, version)| json!({"name": name, "version": version})).collect();
        let runtime_build = if self.sdk == "machine" { runtime_build } else { "" };
        Ok(json!({"identity":identity,"package":distribution,"version":version,"application":application,
            "python":platform::paths::venv_python(env),"dependencies":dependencies,"interface":interface,"sdk":self.sdk,
            "sdk_fallback":self.fallback,"runtime_build":runtime_build,"source_digest":source_digest,"callees":callees}))
    }
}

/// One question to the environment's own Runtime (`cozy-runtime machine-describe`), which reads
/// sources and installed metadata without importing them.
fn ask_runtime(python: &Path, request: &Value) -> Result<Value, Failure> {
    // -S keeps authored .pth startup code out; only the venv's own site-packages are added.
    const BOOTSTRAP: &str = "import pathlib,runpy,sys;\
        root=pathlib.Path(sys.executable).absolute().parent.parent;\
        sys.path[:0]=[str(p) for p in (*(root/'lib').glob('python*/site-packages'),root/'Lib'/'site-packages') if p.is_dir()];\
        sys.argv=['cozy-runtime','machine-describe'];runpy.run_module('cozy_runtime.cli.main',run_name='__main__')";
    let failed = |why: String| ("package_prepare_description_failed", why);
    let output = Command::new(python)
        .args(["-I", "-S", "-c", BOOTSTRAP])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().expect("piped").write_all(request.to_string().as_bytes())?;
            child.wait_with_output()
        })
        .map_err(io_failure)?;
    let reply: Value = serde_json::from_slice(&output.stdout).map_err(|e| failed(format!("the description failed: {e}")))?;
    match reply["kind"].as_str() {
        Some("describe_failed") => Err(failed(format!("{}: {}", reply["code"].as_str().unwrap_or_default(), reply["detail"].as_str().unwrap_or_default()))),
        Some(_) if output.status.success() => Ok(reply),
        _ => Err(failed("the description did not complete".into())),
    }
}

/// The environment's root digest and the other Apps it holds (`packages`: their distributions'
/// packages), described inside it. A failed description fails the install: an environment never
/// publishes with silently missing callees.
pub(crate) fn describe_environment(python: &Path, root: &str, packages: &BTreeMap<String, String>)
    -> Result<(String, Vec<crate::catalog::Callee>), Failure> {
    let reply = ask_runtime(python, &json!({"kind": "describe_environment", "root": root, "packages": packages}))?;
    let callees = serde_json::from_value(reply["callees"].clone())
        .map_err(|e| ("package_prepare_description_failed", format!("the environment's callees: {e}")))?;
    Ok((reply["source_digest"].as_str().unwrap_or_default().to_string(), callees))
}

/// A live project's interface, read from its source directory by the environment's Runtime.
pub(crate) fn describe_project(python: &Path, project: &Path) -> Result<Value, Failure> {
    let reply = ask_runtime(python, &json!({"kind": "describe", "project": project, "environment_python": python}))?;
    Some(reply["interface"].clone()).filter(Value::is_object)
        .ok_or(("package_prepare_description_failed", "the project's description holds no interface".into()))
}

/// Every distribution installed in `env`, by normalized name, at its version.
fn installed(env: &Path) -> BTreeMap<String, String> {
    platform::paths::venv_site_packages(env)
        .into_iter()
        .flat_map(|site| fs::read_dir(site).into_iter().flatten().flatten())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let (distribution, version) = name.strip_suffix(".dist-info")?.split_once('-')?;
            Some((normalized(distribution), version.to_string()))
        })
        .collect()
}

/// The distributions the wheels in `dir` install, each at the version its file names.
fn carried(dir: Option<&Path>) -> io::Result<Vec<(String, String)>> {
    let Some(dir) = dir else {
        return Ok(vec![]);
    };
    let mut found = BTreeSet::new();
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let mut parts = name.strip_suffix(".whl").unwrap_or_default().split('-');
        if let (Some(distribution), Some(version)) = (parts.next(), parts.next()) {
            found.insert((normalized(distribution), version.to_string()));
        }
    }
    Ok(found.into_iter().collect())
}

/// Installed distributions that declare an App and were linked, not copied.
fn linked_applications(env: &Path) -> Vec<String> {
    platform::paths::venv_site_packages(env)
        .into_iter()
        .flat_map(|site| fs::read_dir(site).into_iter().flatten().flatten())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let distribution = name.strip_suffix(".dist-info")?.split_once('-')?.0.to_string();
            let entries = entry.path().join("entry_points.txt");
            let linked = fs::symlink_metadata(&entries).ok()?.file_type().is_symlink();
            (linked && fs::read_to_string(&entries).ok()?.contains("[cozy.application]")).then(|| normalized(&distribution))
        })
        .collect()
}

/// An editable install's source paths spelled as `import` lines. Python adds them at startup all
/// the same, and the Runtime's static reader, which never runs such a line, reads the environment's
/// installed packages without following the project out of it: the project is described from its
/// own directory.
fn live_paths(env: &Path) -> io::Result<()> {
    let Some(site) = platform::paths::venv_site_packages(env) else {
        return Ok(());
    };
    let env = platform::durable::canonical(env)?;
    for entry in fs::read_dir(&site)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "pth") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let inside = |line: &str| platform::durable::canonical(&site.join(line.trim_end())).is_ok_and(|p| p.starts_with(&env));
        let live = |line: &str| !line.is_empty() && !line.starts_with('#') && !line.starts_with("import ") && !line.starts_with("import\t") && !inside(line);
        if !text.lines().any(live) {
            continue;
        }
        let spelled: String = text.lines().map(|line| match live(line) {
            true => format!("import sys; sys.path.append({})\n", Value::from(line.trim_end())),
            false => format!("{line}\n"),
        }).collect();
        fs::write(&path, spelled)?;
    }
    Ok(())
}

/// A uv failure that says this platform is outside the lock: its author's to fix, by adding the
/// platform to `[tool.uv] environments` and locking again.
fn platform(failure: Failure, label: &str) -> Failure {
    const OUTSIDE: [&str; 3] = [
        "not compatible with the lockfile's supported environments",
        "wheel for the current platform",
        "has no wheels with a matching platform tag",
    ];
    match failure {
        (code, detail) if code == "package_installation_uv_failed" && OUTSIDE.iter().any(|s| detail.contains(s)) => (
            "release_platform_unsupported",
            format!("{label} is not locked for this machine ({}/{}): {detail}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        failure => failure,
    }
}

/// The floor uv's finding `line` (`The package `x` requires `cozy-runtime>=0.31,<0.32`, but
/// `0.30.2` is installed`) says the pair misses: a lower bound (`>=`, `>`, `~=`, `==`) above the
/// version installed. A ceiling is no floor.
fn floor(line: &str) -> Option<&str> {
    let parts: Vec<&str> = line.split('`').collect();
    let (requirement, installed) = (*parts.get(3)?, *parts.get(5)?);
    let end = requirement.find(|c: char| "<>=!~ ;".contains(c))?;
    let name = normalized(requirement[..end].split('[').next()?);
    let below = |clause: &str| [">=", "~=", "==", ">"].iter().find_map(|op| clause.trim().strip_prefix(op)).is_some_and(|v| older(installed, v.trim()));
    (SDK.contains(&name.as_str()) && requirement[end..].split(';').next()?.split(',').any(below)).then_some(requirement)
}

/// Whether release `running` is older than `wanted`, by their numeric release parts.
fn older(running: &str, wanted: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version.split('.').map_while(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        }).collect()
    };
    parts(running) < parts(wanted)
}

/// An installer's stderr as a refusal quotes it: its last [`crate::triage::MAX_STDERR`] bytes
/// from a line start, URL credentials masked. uv explains a failed resolution over many lines.
fn stderr_tail(stderr: &[u8]) -> String {
    let start = stderr.len().saturating_sub(crate::triage::MAX_STDERR as usize);
    let mut tail = &stderr[start..];
    if start > 0 {
        if let Some(line) = tail.iter().position(|&b| b == b'\n') {
            tail = &tail[line + 1..];
        }
    }
    crate::triage::masked(String::from_utf8_lossy(tail).trim())
}

/// A hold an environment's generation keeps while a run may read it (`catalog`).
pub(crate) fn hold(dir: &Path) -> Result<(), Failure> {
    File::create(dir.join(".hold")).map(drop).map_err(io_failure)
}
