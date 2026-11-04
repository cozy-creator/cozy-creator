//! Package environments, built one way for a release (`published`) and for local code
//! (`local_source`): uv's Python, the locked (or resolved) dependencies, wheels from files, the
//! project, this machine's Runtime/TensorFS pair over them, then this machine's PyTorch layer
//! (`layer`), then the environment's description by its own Runtime and its record; its bytecode
//! compiles once it is installed. uv runs under one environment.
use super::{canceled, io_failure, layer::Layer, Failure, Job, Locked, Publisher, SDK};
use crate::{catalog::normalized, machine::{pypi, update}, process::Supervised};
use pep440_rs::{Version, VersionSpecifiers};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    process::{Command, Stdio},
    str::FromStr,
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
    /// `machine`: this machine's pair; `own`: newer ones its floor needs, in this environment alone;
    /// `locked`: the package's own, on a machine with no pair.
    pub sdk: String,
    /// What the run is told of the pair (uv's findings, a development Runtime): empty for nothing.
    pub fallback: String,
    /// The package's floors on the pair that a development Runtime missed and ran regardless.
    #[serde(default)]
    pub floors_missed: Vec<String>,
}

/// The pair's step: refused for a floor the pair misses, with what it misses.
enum Pair {
    Floor(Vec<String>),
    Failed(Failure),
}

/// A requirement on a member of the pair (`cozy-runtime[media]>=0.39.0`).
struct Floor {
    name: String,
    extras: String,
    bounds: String,
    specifiers: VersionSpecifiers,
}

impl Floor {
    fn parse(requirement: &str) -> Option<Self> {
        let requirement: String = requirement.split(';').next()?.chars().filter(|c| !" ()".contains(*c)).collect();
        let end = requirement.find(|c: char| "<>=!~".contains(c))?;
        let (named, bounds) = requirement.split_at(end);
        let (name, extras) = named.split_once('[').map_or((named, ""), |(name, extras)| (name, extras.trim_end_matches(']')));
        let specifiers = VersionSpecifiers::from_str(bounds).ok()?;
        Some(Self { name: normalized(name), extras: extras.to_string(), bounds: bounds.to_string(), specifiers })
    }

    fn meets(&self, version: &str) -> bool {
        Version::from_str(version).is_ok_and(|version| self.specifiers.contains(&version))
    }

    /// `name[extras]==version`, as uv installs it.
    fn pinned(&self, version: &str) -> String {
        match self.extras.is_empty() {
            true => format!("{}=={version}", self.name),
            false => format!("{}[{}]=={version}", self.name, self.extras),
        }
    }
}

impl Publisher {
    /// Builds `env` for `project` (an interrupted build there is gone first), holding the uv
    /// cache throughout, on this machine's PyTorch layer. Only when the package (or a dependency)
    /// declares a Torch range the layer misses does it install its own, with a warning: rare by
    /// design, since packages declare a minimum and machines run the newest Torch.
    pub(crate) fn build(&self, env: &Path, project: &Project, job: &Job) -> Result<Built, Failure> {
        let layer = self.layer(job, None)?;
        let missed = match self.build_on(env, project, job, layer.as_deref(), &[]) {
            Err((TORCH_MISSED, missed)) => missed,
            built => return built,
        };
        let torch = layer.as_ref().and_then(|l| l.version("torch")).unwrap_or_default();
        let warning = format!("requires {missed}; this machine's torch is {torch}: its environment installs its own");
        tracing::warn!("package {} {warning}", project.label);
        job.stage(format!("warning: {warning}"));
        let own: Vec<String> = missed.split(", ").filter(|r| on_root(r)).map(String::from).collect();
        let mut built = self.build_on(env, project, job, None, &own)?;
        built.fallback = [built.fallback, warning].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join("; ");
        Ok(built)
    }

    /// `build` on `layer`, or with no layer and the Torch requirements `own` installed for itself.
    fn build_on(&self, env: &Path, project: &Project, job: &Job, layer: Option<&Layer>, own: &[String]) -> Result<Built, Failure> {
        let _cache = self.uv_cache_hold().map_err(io_failure)?;
        let own_sdk = !self.sdk.requirements.is_empty();
        let editable_after = project.editable && project.locked.is_some();
        let steps = [true, project.locked.is_some() && project.wheels.is_some(), editable_after, own_sdk];
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
        // uv never installs what the layer provides: a lock's Torch rows stay out (`Locked::torch`).
        let provided: Vec<&str> = layer.map(|l| l.provided().collect()).unwrap_or_default();
        let excludes = env.join("cozy-excludes.txt");
        fs::write(&excludes, provided.iter().map(|name| format!("{name}\n")).collect::<String>()).map_err(io_failure)?;
        next();
        let pair: Vec<&str> = SDK.into_iter().filter(|_| own_sdk).collect();
        match project.locked {
            Some(locked) => {
                let mut args = vec!["sync", "--frozen", "--inexact", "--no-dev", "--no-editable", "--no-install-project",
                    "--project", &dir, "--python", &py];
                let held = carried.iter().map(|(name, _)| name.as_str()).chain(project.skip.iter().map(String::as_str))
                    .chain(pair.iter().copied().filter(|name| locked.packages.contains_key(*name)))
                    .chain(locked.torch.iter().map(String::as_str).filter(|_| layer.is_some()));
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
                let mut args = vec!["pip", "install", "--python", &py, "--excludes", excludes.to_str().unwrap_or_default()];
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
                    if let Err(failure) = self.uv(&args, &[], job) {
                        // A floor only a GitHub prerelease meets: uv's index can't resolve the project.
                        let declared = declared_pair(project.dir);
                        if failure.0 == super::CANCELED || failure.0 == super::UNREACHABLE || declared.is_empty() {
                            return Err(failure);
                        }
                        self.published_pair(env, &declared, &BTreeMap::new(), project.label, job)?;
                        let wheels = env.join(PAIR_WHEELS);
                        let links = wheels.to_string_lossy();
                        self.uv(&[args.clone(), vec!["--find-links", &links]].concat(), &[], job)?;
                    }
                }
            }
        }
        if let Some(wheels) = project.wheels.filter(|_| steps[1]) {
            next();
            let pins: Vec<String> = carried.iter().map(|(name, version)| format!("{name}=={version}")).collect();
            let mut args = vec!["pip", "install", "--python", &py, "--no-deps", "--no-index", "--link-mode", "copy",
                "--find-links", wheels.to_str().unwrap_or_default()];
            args.extend(pins.iter().map(String::as_str));
            self.uv(&args, &[], job)?;
        }
        if editable_after {
            next();
            // Its dependencies are in place: the project alone, whatever indexes its sources name.
            self.uv(&["pip", "install", "--python", &py, "--no-deps", "--no-sources", "--editable", &dir], &[], job)?;
        }
        if project.editable {
            live_paths(env).map_err(io_failure)?;
        }
        // Torch for itself, the rare fallback: at what the package requires, from the index for
        // this machine's accelerator.
        if !own.is_empty() && !installed(env).contains_key("torch") {
            let mut args = vec!["pip", "install", "--python", &py, "--torch-backend", "auto"];
            args.extend(own.iter().map(String::as_str));
            self.uv(&args, &[], job)?;
        }
        let (mut fallback, mut floors_missed) = (String::new(), vec![]);
        let mut sdk = if own_sdk { "machine" } else { "locked" };
        if own_sdk {
            next();
            // The pair the package itself selects: its lock's, else what its resolution installed.
            let wanted = match project.locked {
                Some(locked) => locked.packages.clone(),
                None => installed(env),
            };
            fallback = match self.pair(env, &py, job) {
                Ok((warning, missed)) => {
                    floors_missed = missed;
                    warning
                }
                Err(Pair::Failed(failure)) => return Err(failure),
                Err(Pair::Floor(floors)) => {
                    sdk = "own";
                    self.own_pair(env, &py, &floors, &wanted, project.label, job)?
                }
            };
            let _ = fs::remove_dir_all(env.join(PAIR_WHEELS));
            if !fallback.is_empty() {
                tracing::warn!("package {}: {fallback}", project.label);
                job.stage(format!("warning: {fallback}"));
            }
        }
        // The layer last, once uv has nothing left to install here; what uv's check finds against
        // it (a range on Torch it misses, or a Torch requirement the lock's copy misses) is a miss.
        if let Some(layer) = layer {
            layer.link(env).map_err(io_failure)?;
            let missed: Vec<String> = self.check(&py, job)?.iter().filter_map(|line| torch_miss(line, layer)).collect();
            if !missed.is_empty() {
                let _ = fs::remove_dir_all(env);
                return Err((TORCH_MISSED, missed.join(", ")));
            }
        }
        Ok(Built { sdk: sdk.into(), fallback, floors_missed })
    }

    /// This machine's own Runtime and TensorFS over the environment. Every other installed version
    /// stays exact: they constrain the install, and a pin the pair's own requirements reject keeps
    /// its version while the pair goes in without its dependencies. A floor the package puts on the
    /// pair that it misses (Fibo's `cozy-runtime>=0.30.16` for #1700's BatchNorm fix) is `Floor`
    /// (`own_pair`), never a run on a Runtime too old for it. What else uv's check finds that the pair adds (a
    /// ceiling a newer pair passes: version skew is allowed) is the run's warning.
    fn pair(&self, env: &Path, py: &str, job: &Job) -> Result<(String, Vec<String>), Pair> {
        let failed = Pair::Failed;
        let constraints = env.join("cozy-constraints.txt");
        let pins: String = installed(env).iter().filter(|(name, _)| !SDK.contains(&name.as_str()))
            .map(|(name, version)| format!("{name}=={version}\n")).collect();
        fs::write(&constraints, pins).map_err(|e| failed(io_failure(e)))?;
        let constraints = constraints.to_string_lossy();
        let links = self.sdk.find_links.as_ref().map(|p| p.to_string_lossy().to_string());
        let excludes = env.join("cozy-excludes.txt");
        let install = |how: &[&str]| {
            let mut args = vec!["pip", "install", "--python", py, "--excludes", excludes.to_str().unwrap_or_default()];
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
            // Unanswered says nothing of the pins: asked again, not installed without its dependencies.
            if code == super::CANCELED || code == super::UNREACHABLE {
                return Err(failed((code, detail)));
            }
            install(&["--no-deps"]).map_err(failed)?;
            warnings.push(format!(
                "this machine's Runtime and TensorFS want versions the package's own do not hold, which it keeps ({})",
                crate::triage::ends(&detail, 1000)
            ));
        }
        let added: Vec<String> = self.check(py, job).map_err(failed)?.difference(&before).cloned().collect();
        // A development Runtime is its owner's choice: the package runs on it, warned below.
        let floors: Vec<String> = added.iter().filter_map(|line| floor(line)).map(String::from).collect();
        if !floors.is_empty() && self.sdk.runtime_build.is_empty() {
            return Err(Pair::Floor(floors));
        }
        if !added.is_empty() {
            warnings.push(format!("uv pip check: {}", added.join("; ")));
        }
        let build = &self.sdk.runtime_build;
        if !warnings.is_empty() && !build.is_empty() {
            warnings.push(format!("this machine's development Runtime {build} (`--runtime <path>`) runs it regardless"));
        }
        Ok((warnings.join("; "), floors))
    }

    /// A package needing a newer Runtime than this machine's: this environment alone takes it, and
    /// the machine, its pair and its other environments stay as they are. The version its lock names
    /// when that meets the floor, else the newest that does. Refused when none is published.
    fn own_pair(&self, env: &Path, py: &str, floors: &[String], wanted: &BTreeMap<String, String>, label: &str, job: &Job) -> Result<String, Failure> {
        let chosen = self.published_pair(env, floors, wanted, label, job)?;
        let wheels = env.join(PAIR_WHEELS);
        job.stage(format!("{label} needs {}: its environment takes {}", floors.join(" and "), chosen.join(" and ")));
        let (constraints, excludes) = (env.join("cozy-constraints.txt"), env.join("cozy-excludes.txt"));
        let (links, machine) = (wheels.to_string_lossy(), self.sdk.find_links.as_ref().map(|p| p.to_string_lossy().to_string()));
        let install = |how: &[&str]| {
            let mut args = vec!["pip", "install", "--python", py, "--excludes", excludes.to_str().unwrap_or_default(), "--find-links", &links];
            args.extend(how);
            if let Some(machine) = &machine {
                args.extend(["--find-links", machine]);
            }
            args.extend(chosen.iter().map(String::as_str));
            self.uv(&args, &[], job)
        };
        let before = self.check(py, job)?;
        let mut warnings = vec![];
        if let Err((code, detail)) = install(&["--constraints", constraints.to_str().unwrap_or_default()]) {
            if code == super::CANCELED || code == super::UNREACHABLE {
                return Err((code, detail));
            }
            install(&["--no-deps"])?;
            warnings.push(format!("{} want versions the package's own do not hold, which it keeps ({})", chosen.join(" and "), crate::triage::ends(&detail, 1000)));
        }
        let found = self.check(py, job)?;
        let missed: Vec<&str> = found.iter().filter_map(|line| floor(line)).collect();
        if !missed.is_empty() {
            return Err(("runtime_floor_unmet", format!("{label} needs {}, and {} doesn't meet it: {}", missed.join(" and "), chosen.join(" and "), floor_remedy(label))));
        }
        let added: Vec<&String> = found.difference(&before).collect();
        if !added.is_empty() {
            warnings.push(format!("uv pip check: {}", added.iter().map(|line| line.as_str()).collect::<Vec<_>>().join("; ")));
        }
        Ok(warnings.join("; "))
    }

    /// Finds a published version for each of `floors` and for what a prerelease's wheel requires of
    /// the pair beyond what `env` holds: `locked`'s when it meets the floor, else uv's index, else a
    /// GitHub prerelease, landed in `env`'s `PAIR_WHEELS` for uv to find. Each as uv installs it.
    fn published_pair(&self, env: &Path, floors: &[String], locked: &BTreeMap<String, String>, label: &str, job: &Job) -> Result<Vec<String>, Failure> {
        let stop = job.stop.clone().unwrap_or_default();
        let wheels = env.join(PAIR_WHEELS);
        fs::create_dir_all(&wheels).map_err(io_failure)?;
        let held = installed(env);
        let mut asked: Vec<String> = floors.to_vec();
        let mut chosen: BTreeMap<String, String> = BTreeMap::new();
        while let Some(requirement) = asked.pop() {
            let invalid = || ("runtime_floor_unmet", format!("{label} needs {requirement}, which this machine can't read: a newer cozy may, `cozy machine update <machine>`"));
            let floor = Floor::parse(&requirement).ok_or_else(invalid)?;
            if chosen.contains_key(&floor.name) {
                continue;
            }
            let pin = locked.get(&floor.name).filter(|version| floor.meets(version));
            let bounds = pin.map_or_else(|| floor.bounds.clone(), |version| format!("=={version}"));
            let input = format!("{}{bounds}\n", floor.name);
            let resolved = update::compile_sdk(&self.sdk.uv, &self.sdk.python, &input, &stop, false).map_err(io_failure)?;
            let prefix = format!("{}==", floor.name);
            // A failed lookup says nothing of what the index holds.
            let unread = resolved.as_ref().err().filter(|said| network(said)).cloned();
            let on_index = resolved.ok().and_then(|plan| plan.lines().find_map(|line| line.trim().strip_prefix(&prefix).map(str::to_owned)));
            let version = match on_index {
                Some(version) => version,
                None => {
                    let meets = |version: &str| match pin {
                        Some(pin) => version == pin,
                        None => floor.meets(version),
                    };
                    let index = if unread.is_some() { "uv's index can't be read now" } else { "uv's index doesn't have it" };
                    let found = pypi::prerelease(&floor.name, &meets, &wheels, &self.machine, &stop).map_err(|why| {
                        let said = format!("{label} needs {requirement}: {index}, and a GitHub prerelease couldn't be fetched ({why})");
                        (if why.kind() == io::ErrorKind::NotConnected { super::UNREACHABLE } else { "runtime_floor_unread" }, said)
                    })?;
                    let (version, wheel) = found.ok_or_else(|| match &unread {
                        Some(said) => (super::UNREACHABLE, format!("{label} needs {requirement}: no GitHub prerelease meets it, and uv's index can't be read now ({said})")),
                        None => ("runtime_floor_unmet", format!("{label} needs {requirement}, which isn't published: neither uv's index nor a GitHub prerelease has a version that meets it: {}", floor_remedy(label))),
                    })?;
                    asked.extend(wheel_requires(&wheel).map_err(io_failure)?.into_iter().filter(|required| {
                        Floor::parse(required).is_some_and(|needed| !held.get(&needed.name).is_some_and(|version| needed.meets(version)))
                    }));
                    version
                }
            };
            chosen.insert(floor.name.clone(), floor.pinned(&version));
        }
        Ok(chosen.into_values().collect())
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
            Supervised::Exited(_) => {
                let said = stderr_tail(&stderr);
                Err(match (network(&said), full(&said)) {
                    (true, _) => (super::UNREACHABLE, format!("uv {step}: {}", said.split_whitespace().collect::<Vec<_>>().join(" "))),
                    (_, true) => ("machine_disk_full", format!("uv {step} ran out of disk{}", crate::reclaim::advice(self.store.root(), 0))),
                    _ => ("package_installation_uv_failed", format!("uv {step}:\n{said}")),
                })
            }
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
    pub(crate) fn record(&self, (identity, environment): (&str, &str), (distribution, version): (&str, &str), env: &Path, interface: &Value,
        runtime_build: &str, described: Described) -> Result<Value, Failure> {
        let Described { source_digest, callees, closures } = described;
        let application = protocol::interface::application(interface)
            .ok_or(("package_prepare_interface_invalid", "the package interface names no application".to_string()))?;
        let dependencies: Vec<Value> = installed(env).into_iter().filter(|(name, _)| SDK.contains(&name.as_str()))
            .map(|(name, version)| json!({"name": name, "version": version})).collect();
        let runtime_build = if self.sdk == "machine" { runtime_build } else { "" };
        Ok(json!({"identity":identity,"package":distribution,"version":version,"application":application,
            "python":platform::paths::venv_python(env),"dependencies":dependencies,"interface":interface,"sdk":self.sdk,
            "sdk_fallback":self.fallback,"floors_missed":self.floors_missed,"runtime_build":runtime_build,"environment":environment,"source_digest":source_digest,"callees":callees,"closures":closures}))
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

/// What an environment's Runtime says of its root (`generation`'s fields of the same names).
pub(crate) struct Described {
    pub source_digest: String,
    pub callees: Vec<crate::catalog::Callee>,
    pub closures: BTreeMap<String, String>,
}

/// The environment's root digest and the other Apps it holds (`packages`: their distributions'
/// packages), described inside it, with the root's code closures when `closures` asks. A failed
/// description fails the install: an environment never publishes with silently missing callees.
pub(crate) fn describe_environment(python: &Path, root: &str, packages: &BTreeMap<String, String>, closures: bool)
    -> Result<Described, Failure> {
    let reply = ask_runtime(python, &json!({"kind": "describe_environment", "root": root, "packages": packages, "closures": closures}))?;
    let callees = serde_json::from_value(reply["callees"].clone())
        .map_err(|e| ("package_prepare_description_failed", format!("the environment's callees: {e}")))?;
    Ok(Described { source_digest: reply["source_digest"].as_str().unwrap_or_default().to_string(), callees, closures: closures_of(&reply) })
}

/// A live project's interface and code closures, read from its source directory by the
/// environment's Runtime.
pub(crate) fn describe_project(python: &Path, project: &Path) -> Result<(Value, BTreeMap<String, String>), Failure> {
    let reply = ask_runtime(python, &json!({"kind": "describe", "project": project, "environment_python": python}))?;
    let interface = Some(reply["interface"].clone()).filter(Value::is_object)
        .ok_or(("package_prepare_description_failed", "the project's description holds no interface".into()))?;
    Ok((interface, closures_of(&reply)))
}

/// A reply's code closures: none from a Runtime that reads none, whose memo keys on the whole
/// source.
fn closures_of(reply: &Value) -> BTreeMap<String, String> {
    serde_json::from_value(reply["closures"].clone()).unwrap_or_default()
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
            format!("{label} is not locked for this machine ({}/{}): {detail}\n{}", std::env::consts::OS, std::env::consts::ARCH, match cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                true => format!("Its author adds this platform and locks again; another release may hold it: `cozy package info {}`", label.split(' ').next().unwrap_or(label)),
                false => "`--on new` rents a Linux machine, or its author adds this platform and locks again".to_string(),
            }),
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

/// Where an environment's own pair's wheels wait for uv, while it is built.
const PAIR_WHEELS: &str = "cozy-pair";

/// What the wheel requires of the Runtime pair, outside any extra.
fn wheel_requires(wheel: &Path) -> io::Result<Vec<String>> {
    let mut archive = zip::ZipArchive::new(File::open(wheel)?).map_err(io::Error::other)?;
    // `<distribution>-<version>-…whl` holds `<distribution>-<version>.dist-info/METADATA`.
    let file = wheel.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    let mut named = file.split('-');
    let name = format!("{}-{}.dist-info/METADATA", named.next().unwrap_or_default(), named.next().unwrap_or_default());
    let mut text = String::new();
    archive.by_name(&name).map_err(io::Error::other)?.read_to_string(&mut text)?;
    Ok(text.lines().filter_map(|line| line.strip_prefix("Requires-Dist:").map(str::trim))
        .filter(|required| !required.contains("extra =="))
        .filter(|required| SDK.contains(&normalized(required.split(|c: char| " ;<>=!~[(".contains(c)).next().unwrap_or_default()).as_str()))
        .map(String::from).collect())
}

/// What an unlocked project's pyproject.toml requires of the Runtime pair.
fn declared_pair(dir: &Path) -> Vec<String> {
    let read = fs::read_to_string(dir.join("pyproject.toml")).ok().and_then(|text| toml::from_str::<toml::Value>(&text).ok());
    let dependencies = read.as_ref().and_then(|document| document.get("project")?.get("dependencies")?.as_array().cloned()).unwrap_or_default();
    dependencies.iter().filter_map(toml::Value::as_str)
        .filter(|required| SDK.contains(&normalized(required.split(|c: char| " ;<>=!~[(".contains(c)).next().unwrap_or_default()).as_str()))
        .filter(|required| Floor::parse(required).is_some())
        .map(String::from).collect()
}

/// A build the layer cannot serve: the requirements it misses (`Publisher::build`).
const TORCH_MISSED: &str = "torch_range_missed";

/// Whether requirement `spec` names one of the layer's roots (torch, torchvision, …).
fn on_root(spec: &str) -> bool {
    let name = normalized(spec.split(|c: char| " ;<>=!~[(".contains(c)).next().unwrap_or_default());
    super::layer::ROOTS.contains(&name.as_str())
}

/// The requirement uv's finding `line` (`The package `x` requires `torch>=2.15`, but `2.14.0` is
/// installed`) says the layer misses: one on a layer distribution, or one a layer distribution
/// puts on the package's own copy of a dependency.
fn torch_miss(line: &str, layer: &Layer) -> Option<String> {
    let parts: Vec<&str> = line.split('`').collect();
    let (requirer, requirement) = (normalized(parts.get(1)?), *parts.get(3)?);
    let required = normalized(requirement.split(|c: char| " ;<>=!~[(".contains(c)).next()?);
    (layer.holds(&required) || layer.holds(&requirer)).then(|| requirement.to_string())
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

/// Whether uv's words are about the network rather than what it was asked for: nothing about the
/// package is settled, and asking again may be answered (`patiently`).
fn network(said: &str) -> bool {
    ["error sending request", "Request failed after", "operation timed out", "HTTP status server error"].iter().any(|word| said.contains(word))
}

/// Whether uv stopped because the disk filled.
fn full(said: &str) -> bool {
    said.contains("No space left on device")
}

/// `cozy machine update`'s flags for what `floors` asks of the pair (`cozy-runtime>=0.39.4`).
pub(crate) fn update_flags(floors: &[String]) -> String {
    let flag = |floor: &String| {
        let parsed = Floor::parse(floor)?;
        let lowest = parsed.bounds.split(',').find_map(|clause| [">=", "~=", "=="].iter().find_map(|op| clause.strip_prefix(op)))?;
        Some(format!("--{} {lowest}", parsed.name.trim_start_matches("cozy-")))
    };
    floors.iter().filter_map(flag).collect::<Vec<_>>().join(" ")
}

/// What a person does about a floor nothing published meets.
fn floor_remedy(label: &str) -> String {
    let own = "or run it on a Runtime you built: `cozy machine update <machine> --runtime <path>`";
    match label.split(' ').next().filter(|package| !package.starts_with("local/")) {
        Some(package) => format!("an older release may not need it (`cozy package info {package}` lists them; `cozy run {package}@<release>`), {own}"),
        None => format!("lower that bound in its pyproject.toml, {own}"),
    }
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

/// Compiles `python`'s environment's bytecode beside its first import, at the lowest priority on
/// the cores the machine may use; already compiled files are skipped. Uncompiled, an import
/// compiles each module it reaches on one core (SDXL's: 17-19 s, 6.5-8.5 s compiled), and again
/// in every executor that may not write the environment. uv's `--compile-bytecode` compiles only
/// what its own step installs. A failure is a warning.
pub(crate) fn compile_bytecode(python: &Path) {
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let script = format!(
        "import compileall, os, sysconfig\ngetattr(os, 'nice', lambda _: 0)(19)\n\
         compileall.compile_dir(sysconfig.get_paths()['purelib'], quiet=2, workers={workers})"
    );
    let started = crate::launch_identity::trampoline(python, None).and_then(|mut command| {
        command.envs(crate::launch_identity::interpreter_environment(python)?).args(["-I", "-c", &script]);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        command.spawn()
    });
    let reaped = started.and_then(|mut child| {
        std::thread::Builder::new().name("bytecode".into()).spawn(move || match child.wait() {
            Ok(status) if !status.success() => tracing::warn!("environment bytecode: compileall exited {status}"),
            _ => {}
        })
    });
    if let Err(error) = reaped {
        tracing::warn!("environment bytecode: {error}");
    }
}
