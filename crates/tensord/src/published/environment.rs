//! Package environments, built one way for a release (`published`) and for local code
//! (`local_source`): uv's Python, the locked (or resolved) dependencies with the Runtime/TensorFS
//! pair the lock names at its newest released patch (this machine's where it is a later patch of
//! it or the lock's can't speak this machine's executor protocol, `stands_in`; a pair that can't is
//! refused, `speaks`), wheels from files, the project, then this machine's PyTorch layer (`layer`),
//! then the environment's description by its own Runtime and its record; its bytecode compiles
//! once it is installed. uv runs under one environment.
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
    /// Local code: the project installed editable from this tree (its directory or one above it),
    /// which each generation links as its own `SOURCE`: one environment runs every tree built on it.
    pub editable: Option<&'a Path>,
    /// Indexes an unlocked project's sources name without declaring (name → URL).
    pub indexes: &'a BTreeMap<String, String>,
    /// Its package, which the machine lists installing (`activity`).
    pub package: &'a str,
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
    /// `machine`: this machine's pair; `locked`: the one its lock names; `resolved`: the one uv
    /// resolved for an unlocked project whose bounds this machine's misses.
    pub sdk: String,
    /// What the run is told of the pair (uv's findings, a pair not taken): empty for nothing.
    pub fallback: String,
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
        let _install = job.within(crate::activity::Kind::Install, project.package);
        let layer = self.layer(job, None)?;
        let missed = match self.build_on(env, project, job, layer.as_deref(), &[]) {
            Err((TORCH_MISSED, missed)) => missed,
            built => return built,
        };
        let torch = layer.as_ref().and_then(|l| l.version("torch")).unwrap_or_default();
        let warning = format!("requires {missed}; this machine's torch is {torch}: its environment installs its own");
        if crate::private_io::current().is_none() {
            tracing::warn!("package {} {warning}", project.label);
        }
        job.stage(format!("warning: {warning}"));
        let own: Vec<String> = missed.split(", ").filter(|r| on_root(r)).map(String::from).collect();
        let mut built = self.build_on(env, project, job, None, &own)?;
        built.fallback = [built.fallback, warning].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join("; ");
        Ok(built)
    }

    /// `build` on `layer`, or with no layer and the Torch requirements `own` installed for itself.
    fn build_on(&self, env: &Path, project: &Project, job: &Job, layer: Option<&Layer>, own: &[String]) -> Result<Built, Failure> {
        let _cache = self.uv_cache_hold().map_err(io_failure)?;
        let over = project.locked.is_some_and(|locked| self.stands_in(locked));
        let editable_after = project.editable.is_some() && project.locked.is_some();
        let steps = [true, project.locked.is_some() && project.wheels.is_some(), editable_after, project.locked.is_some()];
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
        self.uv(&["venv", "--no-project", "--python", minor(project.python), &envs], &[], job)?;
        // uv never installs what the layer provides: a lock's Torch rows stay out (`Locked::torch`).
        let provided: Vec<&str> = layer.map(|l| l.provided().collect()).unwrap_or_default();
        let excludes = env.join("cozy-excludes.txt");
        fs::write(&excludes, provided.iter().map(|name| format!("{name}\n")).collect::<String>()).map_err(io_failure)?;
        // The lock's rows, its pair's too unless this machine's goes over them (`over`).
        let sync = |locked: &Locked, over: bool| -> Result<(), Failure> {
            let mut args = vec!["sync", "--frozen", "--inexact", "--no-dev", "--no-editable", "--no-install-project",
                "--project", &dir, "--python", &py];
            let held = carried.iter().map(|(name, _)| name.as_str()).chain(project.skip.iter().map(String::as_str))
                .chain(SDK.into_iter().filter(|name| over && locked.packages.contains_key(*name)))
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
            Ok(())
        };
        next();
        let mut sdk = if over { "machine" } else { "locked" };
        let mut fallback = vec![];
        // Why this machine's own pair isn't what it runs on.
        let mut passed = match self.sdk.requirements.is_empty() {
            true => "this machine has no pair of its own".to_string(),
            false => String::new(),
        };
        match project.locked {
            Some(locked) => sync(locked, over)?,
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
                sdk = "machine";
                if passed.is_empty() {
                    if let Err((_, refused)) = self.uv(&offered, &[], job) {
                        passed = format!("this machine's doesn't install with it: {}", crate::triage::ends(&refused, 300));
                    }
                }
                if !passed.is_empty() {
                    sdk = "resolved";
                    if let Err(failure) = self.uv(&args, &[], job) {
                        // A floor only a GitHub prerelease meets: uv's index can't resolve the project.
                        let declared = declared_pair(project.dir);
                        if failure.0 == super::CANCELED || failure.0 == super::UNREACHABLE || declared.is_empty() {
                            return Err(failure);
                        }
                        let chosen = self.published_pair(env, &declared, project.label, job)?;
                        job.stage(format!("{} needs {}: its environment takes {}", project.label, declared.join(" and "), chosen.join(" and ")));
                        let wheels = env.join(PAIR_WHEELS);
                        let links = wheels.to_string_lossy();
                        self.uv(&[args.clone(), vec!["--find-links", &links]].concat(), &[], job)?;
                        let _ = fs::remove_dir_all(&wheels);
                    }
                    if !self.sdk.requirements.is_empty() {
                        fallback.push(format!("it doesn't install with this machine's {}: it runs on {}", shown(&self.machine_pair()), shown(&pair_of(&installed(env)))));
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
        if let Some(tree) = project.editable {
            live_paths(env, tree).map_err(io_failure)?;
        }
        // Torch for itself, the rare fallback: at what the package requires, from the index for
        // this machine's accelerator.
        if !own.is_empty() && !installed(env).contains_key("torch") {
            let mut args = vec!["pip", "install", "--python", &py, "--torch-backend", "auto"];
            args.extend(own.iter().map(String::as_str));
            self.uv(&args, &[], job)?;
        }
        if let Some(locked) = project.locked {
            let build = &self.sdk.runtime_build;
            next();
            // A lock's pair of another series can't speak this machine's executor protocol: this
            // machine's goes over it where every declared range admits it, else nothing runs.
            let theirs = pair_of(&locked.packages);
            let foreign = !self.speaks(&theirs);
            if over {
                match self.pair(env, &py, &self.sdk.requirements, job)? {
                    Ok(found) if foreign => {
                        let ranges: Vec<&str> = found.iter().map(String::as_str).filter(|line| on_pair(line)).collect();
                        if !ranges.is_empty() {
                            let why = format!("its requirements refuse this machine's ({})", ranges.join("; "));
                            return Err(self.unspoken(env, project, &theirs, &why, true));
                        }
                        fallback.push(format!("its lock's {} can't speak this machine's executor protocol: it runs on this machine's {}; {}",
                            shown(&theirs), shown(&self.machine_pair()), relock(project, false)));
                        fallback.extend(checked(&found));
                    }
                    Ok(found) => fallback.extend(checked(&found)),
                    Err(conflict) if foreign => {
                        let why = format!("this machine's doesn't install over its lock ({conflict})");
                        return Err(self.unspoken(env, project, &theirs, &why, false));
                    }
                    Err(conflict) => {
                        sync(locked, false)?;
                        sdk = "locked";
                        fallback.push(format!("this machine's {} wants versions its lock does not hold ({conflict}): it runs on its lock's {}",
                            shown(&self.machine_pair()), shown(&pair_of(&locked.packages))));
                    }
                }
            } else {
                // The lock's own series at its newest released patch: a patch is fixes, never a
                // change in what a package calls. One that doesn't install here leaves the lock's.
                let newest: Vec<String> = SDK.iter()
                    .filter_map(|name| Some(newest_patch(name, locked.packages.get(*name)?))).collect();
                match self.pair(env, &py, &newest, job) {
                    Ok(Ok(found)) => fallback.extend(checked(&found)),
                    Ok(Err(why)) | Err((super::UNREACHABLE, why)) => {
                        sync(locked, false)?;
                        fallback.push(format!("no newer patch of its lock's {} installs here ({why}): it runs on that", shown(&pair_of(&locked.packages))));
                    }
                    Err(failure) => return Err(failure),
                }
                if !build.is_empty() {
                    fallback.push(format!("this machine's development Runtime {build} (`--runtime <path>`) is no later patch of its lock's {}: it runs on that series",
                        shown(&pair_of(&locked.packages))));
                }
            }
        }
        // However it resolved, it runs only on a pair that speaks this machine's protocol.
        let held = pair_of(&installed(env));
        if !self.speaks(&held) {
            return Err(self.unspoken(env, project, &held, &passed, false));
        }
        let fallback = fallback.join("; ");
        if !fallback.is_empty() {
            tracing::warn!("package {}: {fallback}", project.label);
            job.stage(format!("warning: {fallback}"));
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
        Ok(Built { sdk: sdk.into(), fallback })
    }

    /// Whether this machine's own Runtime/TensorFS pair goes over the one `locked` names: each of
    /// it the lock's or a later patch of it (before 1.0 a minor may change what a package calls, a
    /// patch doesn't), so a release runs what its lock resolved, with a Runtime's fixes; or a pair
    /// that can't speak this machine's executor protocol (`speaks`), which only this machine's can.
    /// A member the lock doesn't name is this machine's.
    pub(crate) fn stands_in(&self, locked: &Locked) -> bool {
        let ours = self.machine_pair();
        !ours.is_empty() && (!self.speaks(&locked.packages) || SDK.iter().all(|name| match (ours.get(*name), locked.packages.get(*name)) {
            (Some(ours), Some(locked)) => patch_of(ours, locked),
            (ours, _) => ours.is_some(),
        }))
    }

    /// The pair whose series speaks this machine's executor protocol: its own, else the TensorFS
    /// of its own release, whose `tensorfs.executor` is compiled from the executor-api it is
    /// built with (a Runtime of another series requires another TensorFS).
    fn protocol(&self) -> BTreeMap<String, String> {
        match self.machine_pair() {
            ours if ours.is_empty() => BTreeMap::from([("tensorfs".to_string(), env!("CARGO_PKG_VERSION").to_string())]),
            ours => ours,
        }
    }

    /// Whether a pair (a lock's, or what an environment holds) speaks this machine's executor
    /// protocol: each member of the protocol's series. One it doesn't name says nothing.
    pub(crate) fn speaks(&self, pair: &BTreeMap<String, String>) -> bool {
        self.protocol().iter().all(|(name, ours)| pair.get(name).is_none_or(|theirs| same_series(theirs, ours)))
    }

    /// `project` refused, `env` gone: its pair can't speak this machine's executor protocol, and
    /// this machine's isn't what it runs on (`why`; `ranged`: its declared range refuses it). The
    /// next command: an updated machine for a newer pair (or none of its own), else a newer lock
    /// or release.
    fn unspoken(&self, env: &Path, project: &Project, pair: &BTreeMap<String, String>, why: &str, ranged: bool) -> Failure {
        let _ = fs::remove_dir_all(env);
        let ours = self.protocol();
        let newer = pair.iter().any(|(name, theirs)| ours.get(name).is_some_and(|ours| later_series(theirs, ours)));
        let next = match newer || self.sdk.requirements.is_empty() {
            true => "update this machine: `cozy machine update <machine>`".to_string(),
            false => relock(project, ranged),
        };
        // A newer pair is a floor this machine misses, as an unpublished one is.
        let code = if newer { "runtime_floor_unmet" } else { "runtime_protocol_unspoken" };
        (code, format!("{} runs on {}, which can't speak this machine's executor protocol ({}'s): {why}; {next}",
            project.label, shown(pair), shown(&ours)))
    }

    /// This machine's own pair (an update's, else the image's) by its wheels' names: empty without one.
    pub(crate) fn machine_pair(&self) -> BTreeMap<String, String> {
        let named = self.sdk.requirements.iter().filter_map(|wheel| {
            let mut parts = Path::new(wheel).file_name()?.to_str()?.split('-');
            Some((normalized(parts.next()?), parts.next()?.to_string()))
        });
        pair_of(&named.collect())
    }

    /// `requirements` of the pair (this machine's, or the lock's newest patch) over a locked
    /// environment, every other installed version held exact. One the lock's pins refuse leaves
    /// the lock's (`Err`: why); what uv's check finds that it adds (a ceiling of the package's).
    fn pair(&self, env: &Path, py: &str, requirements: &[String], job: &Job) -> Result<Result<Vec<String>, String>, Failure> {
        let constraints = env.join("cozy-constraints.txt");
        let pins: String = installed(env).iter().filter(|(name, _)| !SDK.contains(&name.as_str()))
            .map(|(name, version)| format!("{name}=={version}\n")).collect();
        fs::write(&constraints, pins).map_err(io_failure)?;
        let (constraints, excludes) = (constraints.to_string_lossy(), env.join("cozy-excludes.txt"));
        let links = self.sdk.find_links.as_ref().map(|p| p.to_string_lossy().to_string());
        let mut args = vec!["pip", "install", "--python", py, "--excludes", excludes.to_str().unwrap_or_default(), "--constraints", &constraints];
        if let Some(links) = &links {
            args.extend(["--find-links", links]);
        }
        args.extend(platform::tools::uv_upgrade(SDK));
        args.extend(requirements.iter().map(String::as_str));
        // What the package's own closure reports already stands (NVIDIA's aarch64 cusparseLt
        // wheel is tagged `sbsa`, so uv flags it on every Linux arm64 machine).
        let before = self.check(py, job)?;
        match self.uv(&args, &[], job) {
            Err(("package_installation_uv_failed", detail)) => return Ok(Err(crate::triage::ends(&detail, 1000))),
            done => done?,
        }
        Ok(Ok(self.check(py, job)?.difference(&before).cloned().collect()))
    }

    /// Finds a published version for each of `floors` and for what a prerelease's wheel requires of
    /// the pair beyond what `env` holds: uv's index, else a GitHub prerelease, landed in `env`'s
    /// `PAIR_WHEELS` for uv to find. Each as uv installs it.
    fn published_pair(&self, env: &Path, floors: &[String], label: &str, job: &Job) -> Result<Vec<String>, Failure> {
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
            let input = format!("{}{}\n", floor.name, floor.bounds);
            let resolved = update::compile_sdk(&self.sdk.uv, &self.sdk.python, &input, &stop, false).map_err(io_failure)?;
            let prefix = format!("{}==", floor.name);
            // A failed lookup says nothing of what the index holds.
            let unread = resolved.as_ref().err().filter(|said| network(said)).cloned();
            let on_index = resolved.ok().and_then(|plan| plan.lines().find_map(|line| line.trim().strip_prefix(&prefix).map(str::to_owned)));
            let version = match on_index {
                Some(version) => version,
                None => {
                    let meets = |version: &str| floor.meets(version);
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
    /// Linux tags the builder and its descendants before exec, so a new machine sweeps an
    /// interrupted builder before reusing its environment, even after SIGKILL.
    pub(crate) fn uv(&self, args: &[&str], env: &[(&str, &std::ffi::OsStr)], job: &Job) -> Result<(), Failure> {
        let mut command = self.sdk.uv.command().map_err(|e| ("uv_unavailable", e.to_string()))?;
        command.args(args).envs(env.iter().copied()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        crate::private_io::protect_command(&mut command).map_err(io_failure)?;
        let (ended, stderr) = run_builder(&mut command, &self.scope_namespace, job.stop.as_ref(), &|read| job.bytes(read, 0))?;
        let step = args[..2.min(args.len())].join(" ");
        match ended {
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

/// A builder's own group and inherited ownership, supervised while its stderr is drained.
fn run_builder(command: &mut Command, namespace: &str, stop: Option<&crate::stop::Stop>, wrote: &dyn Fn(u64)) -> Result<(Supervised, Vec<u8>), Failure> {
    let scope = match platform::contain::Scope::inherited(namespace) {
        Ok(scope) => Some(scope),
        Err(error) if error.is_unsupported() => None,
        Err(error) => return Err(io_failure(io::Error::other(error))),
    };
    // After the caller's environment: no override can accidentally erase the owner token.
    command.envs(scope.as_ref().and_then(platform::contain::Scope::environment));
    platform::process::own_group(command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Some(scope) = &scope {
                scope.end().map_err(|error| io_failure(io::Error::other(error)))?;
            }
            return Err(("package_installation_uv_absent", format!("cannot run uv: {error}")));
        }
    };
    let mut stderr = child.stderr.take().expect("piped builder stderr");
    std::thread::scope(|threads| {
        let drained = threads.spawn(move || {
            let mut kept = Vec::new();
            let _ = stderr.read_to_end(&mut kept);
            kept
        });
        let ended = crate::process::supervise(&mut child, stop, wrote)
            .map_err(|error| ("package_installation_uv_failed", format!("uv: {error}")));
        // A build backend can outlive uv and keep its stderr open. End it before waiting on
        // that pipe, including when supervision failed or the run was canceled.
        let cleaned = scope.as_ref().map(platform::contain::Scope::end).transpose()
            .map_err(|error| io_failure(io::Error::other(error)));
        let stderr = drained.join().unwrap_or_default();
        let ended = ended?;
        cleaned?;
        Ok((ended, stderr))
    })
}

#[cfg(all(test, target_os = "linux"))]
#[path = "environment_process_tests.rs"]
mod process_tests;

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
    /// A description's `failure` said for what it is when the Runtime its lock pinned refused to
    /// read the code (its `static_*` codes): that Runtime is older than the code, and `next`
    /// re-locks it on a newer one. Any other failure stands.
    pub(crate) fn unread(&self, failure: Failure, env: &Path, package: &str, next: &str) -> Failure {
        // `code: code: why` (the Runtime's error says its code too).
        let refused = failure.1.split_once(": ").filter(|(code, _)| code.starts_with("static_"))
            .map(|(code, why)| why.strip_prefix(code).and_then(|why| why.strip_prefix(": ")).unwrap_or(why));
        let runtime = installed(env).remove("cozy-runtime");
        let (Some(why), Some(runtime), "locked") = (refused, runtime, self.sdk.as_str()) else { return failure };
        // `file:line: `construct` — why; the static interface reads …`: the place and the why.
        let why = why.split("; the static interface").next().unwrap_or(why);
        let why = match why.split_once(" — ") {
            Some((at, what)) => format!("{what} ({})", at.split(": `").next().unwrap_or(at)),
            None => why.to_owned(),
        };
        ("package_lock_runtime_old", format!("{package}'s lock pins cozy-runtime {runtime}, which can't read it: {why}: re-lock it on a newer Runtime, `{next}`"))
    }

    /// The record of the environment at `env` (a generation's): the package (distribution and
    /// version), its App and interface, the pair it holds, its description.
    pub(crate) fn record(&self, (identity, environment): (&str, &str), (distribution, version): (&str, &str), env: &Path, interface: &Value,
        runtime_build: &str, described: Described) -> Result<Value, Failure> {
        let Described { source_digest, callees, closures } = described;
        let application = api::interface::application(interface)
            .ok_or(("package_prepare_interface_invalid", "the package interface names no application".to_string()))?;
        let dependencies: Vec<Value> = installed(env).into_iter().filter(|(name, _)| SDK.contains(&name.as_str()))
            .map(|(name, version)| json!({"name": name, "version": version})).collect();
        let runtime_build = if self.sdk == "machine" { runtime_build } else { "" };
        Ok(json!({"identity":identity,"package":distribution,"version":version,"application":application,
            "python":platform::paths::venv_python(env),"dependencies":dependencies,"interface":interface,"sdk":self.sdk,
            "sdk_fallback":self.fallback,"runtime_build":runtime_build,"environment":environment,"source_digest":source_digest,"callees":callees,"closures":closures}))
    }
}

/// One question to the environment's own Runtime (`cozy-runtime machine-describe`), which reads
/// sources and installed metadata without importing them.
fn ask_runtime(python: &Path, request: &Value, stop: Option<&crate::stop::Stop>) -> Result<Value, Failure> {
    // -S keeps authored .pth startup code out; only the venv's own site-packages are added.
    const BOOTSTRAP: &str = "import pathlib,runpy,sys;\
        root=pathlib.Path(sys.executable).absolute().parent.parent;\
        sys.path[:0]=[str(p) for p in (*(root/'lib').glob('python*/site-packages'),root/'Lib'/'site-packages') if p.is_dir()];\
        sys.argv=['cozy-runtime','machine-describe'];runpy.run_module('cozy_runtime.cli.main',run_name='__main__')";
    let failed = |why: String| ("package_prepare_description_failed", why);
    let mut command = Command::new(python);
    command.args(["-I", "-B", "-S", "-c", BOOTSTRAP]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    if crate::private_io::protect_command(&mut command).map_err(io_failure)? {
        command.stderr(Stdio::null());
    }
    platform::process::own_group(&mut command);
    let mut child = command.spawn().map_err(io_failure)?;
    // Its own exit says why, should it stop before reading the request.
    let _ = child.stdin.take().expect("piped").write_all(request.to_string().as_bytes());
    let mut stdout = child.stdout.take().expect("piped");
    // Supervised, as uv is: a cancel ends it, so a description that does not end holds no install.
    let (ended, read) = std::thread::scope(|scope| {
        let drained = scope.spawn(move || {
            let mut kept = Vec::new();
            let _ = stdout.read_to_end(&mut kept);
            kept
        });
        let ended = crate::process::supervise(&mut child, stop, &|_| ());
        (ended, drained.join().unwrap_or_default())
    });
    let status = match ended.map_err(io_failure)? {
        Supervised::Exited(status) => status,
        Supervised::Canceled => return Err(canceled()),
    };
    let reply: Value = serde_json::from_slice(&read).map_err(|e| failed(format!("the description failed: {e}")))?;
    match reply["kind"].as_str() {
        Some("describe_failed") => Err(failed(format!("{}: {}", reply["code"].as_str().unwrap_or_default(), reply["detail"].as_str().unwrap_or_default()))),
        Some(_) if status.success() => Ok(reply),
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
pub(crate) fn describe_environment(python: &Path, root: &str, packages: &BTreeMap<String, String>, closures: bool, stop: Option<&crate::stop::Stop>)
    -> Result<Described, Failure> {
    let reply = ask_runtime(python, &json!({"kind": "describe_environment", "root": root, "packages": packages, "closures": closures}), stop)?;
    let callees = serde_json::from_value(reply["callees"].clone())
        .map_err(|e| ("package_prepare_description_failed", format!("the environment's callees: {e}")))?;
    Ok(Described { source_digest: reply["source_digest"].as_str().unwrap_or_default().to_string(), callees, closures: closures_of(&reply) })
}

/// A live project's interface and code closures, read from its source directory by the
/// environment's Runtime.
pub(crate) fn describe_project(python: &Path, project: &Path, stop: Option<&crate::stop::Stop>) -> Result<(Value, BTreeMap<String, String>), Failure> {
    let reply = ask_runtime(python, &json!({"kind": "describe", "project": project, "environment_python": python}), stop)?;
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

/// What a local generation links its tree as, beside its `env`.
pub(crate) const SOURCE: &str = "source";

/// An editable install's source paths spelled as `import` lines, each under `tree` named from the
/// `SOURCE` of the generation Python runs as (`sys.prefix` is its `env`, the link unresolved), so
/// each generation reads its own tree. Python adds them at startup all the same, and the Runtime's
/// static reader, which never runs such a line, reads the environment's installed packages without
/// following the project out of it: the project is described from its own directory.
fn live_paths(env: &Path, tree: &Path) -> io::Result<()> {
    let Some(site) = platform::paths::venv_site_packages(env) else {
        return Ok(());
    };
    let (env, tree) = (platform::durable::canonical(env)?, platform::durable::canonical(tree)?);
    for entry in fs::read_dir(&site)? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "pth") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let target = |line: &str| platform::durable::canonical(&site.join(line.trim_end())).ok();
        let live = |line: &str| !line.is_empty() && !line.starts_with('#') && !line.starts_with("import ") && !line.starts_with("import\t")
            && !target(line).is_some_and(|p| p.starts_with(&env));
        if !text.lines().any(live) {
            continue;
        }
        let under = |line: &str| target(line).and_then(|p| p.strip_prefix(&tree).ok().map(|rest| {
            rest.components().map(|c| Value::from(c.as_os_str().to_string_lossy().into_owned())).collect::<Vec<_>>()
        }));
        let spelled: String = text.lines().map(|line| match (live(line), under(line)) {
            (true, Some(parts)) => format!("import os, sys; sys.path.append(os.path.join(os.path.dirname(sys.prefix), {}, *{}))\n",
                Value::from(SOURCE), Value::from(parts)),
            (true, None) => format!("import sys; sys.path.append({})\n", Value::from(line.trim_end())),
            (false, _) => format!("{line}\n"),
        }).collect();
        // A new file: uv links the one it installed from its cache.
        let staged = path.with_extension("pth.new");
        fs::write(&staged, spelled)?;
        fs::rename(&staged, &path)?;
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
                true => format!("Ask its author to support this platform; browse available packages with `cozy package ls {}`", label.split(' ').next().unwrap_or(label)),
                false => "`--on new` rents a Linux machine, or its author adds this platform and locks again".to_string(),
            }),
        ),
        failure => failure,
    }
}

/// `3.12` of the `3.12.15` a release was published with: it runs on this machine's own Python of
/// that minor (its ABI), which may be another patch, one this machine's uv cannot fetch.
fn minor(python: &str) -> &str {
    python.match_indices('.').nth(1).map_or(python, |(at, _)| &python[..at])
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

/// A release's series: its major, or before 1.0 its minor, within which a release only fixes.
fn series(version: &Version) -> Vec<u64> {
    let release = version.release();
    let parts = if release.first().is_some_and(|major| *major > 0) { 1 } else { 2 };
    (0..parts).map(|at| release.get(at).copied().unwrap_or(0)).collect()
}

/// Whether release `ours` is `locked` or a later patch of it: the same series and not older.
fn patch_of(ours: &str, locked: &str) -> bool {
    let (Ok(ours), Ok(locked)) = (Version::from_str(ours), Version::from_str(locked)) else {
        return false;
    };
    series(&ours) == series(&locked) && ours >= locked
}

/// Whether releases `a` and `b` are of one series.
fn same_series(a: &str, b: &str) -> bool {
    matches!((Version::from_str(a), Version::from_str(b)), (Ok(a), Ok(b)) if series(&a) == series(&b))
}

/// Whether release `a` is of a later series than `b`.
fn later_series(a: &str, b: &str) -> bool {
    matches!((Version::from_str(a), Version::from_str(b)), (Ok(a), Ok(b)) if series(&a) > series(&b))
}

/// What uv's check found that the pair added, as the run's warning.
fn checked(found: &[String]) -> Option<String> {
    (!found.is_empty()).then(|| format!("uv pip check: {}", found.join("; ")))
}

/// Whether uv's finding `line` (`The package `x` requires `cozy-runtime<0.44`, …`) is a range on
/// a member of the pair.
fn on_pair(line: &str) -> bool {
    let required = line.split('`').nth(3).and_then(|r| r.split(|c: char| " ;<>=!~[(".contains(c)).next()).unwrap_or_default();
    SDK.contains(&normalized(required).as_str())
}

/// How an older pair takes this machine's: update the refusing SDK requirement or its
/// dependency owner, then re-lock local code; a published package needs a newer release.
fn relock(project: &Project, ranged: bool) -> String {
    const LOCK: &str = "`uv lock --upgrade-package cozy-runtime`";
    match (project.package.starts_with("local/"), project.locked.is_some(), ranged) {
        (true, true, false) => format!("re-lock it on this machine's: {LOCK}"),
        (true, true, true) => format!("update the refusing cozy-runtime or tensorfs requirement in pyproject.toml, or upgrade the dependency that declares it, then re-lock it: {LOCK}"),
        (true, false, _) => "update the refusing cozy-runtime or tensorfs requirement in pyproject.toml, or upgrade the dependency that declares it, to admit this machine's pair".into(),
        (false, ..) => newest_hint(project.package),
    }
}

/// A published release's next where nothing better is known: its newest. Its install names the one that speaks this
/// machine's protocol in its place (`Publisher::onward`).
pub(crate) fn newest_hint(package: &str) -> String {
    format!("try its newest release: `cozy package install {package}`")
}

/// `name` at the newest release of `locked`'s series: `cozy-runtime>=0.41.0,==0.41.*`.
fn newest_patch(name: &str, locked: &str) -> String {
    match Version::from_str(locked) {
        Ok(version) => {
            let series: Vec<String> = series(&version).iter().map(u64::to_string).collect();
            format!("{name}>={locked},=={}.*", series.join("."))
        }
        Err(_) => format!("{name}=={locked}"),
    }
}

/// The Runtime and TensorFS of `versions`, by name.
pub(crate) fn pair_of(versions: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    versions.iter().filter(|(name, _)| SDK.contains(&name.as_str())).map(|(name, version)| (name.clone(), version.clone())).collect()
}

/// `Runtime 0.41.0 and TensorFS 0.20.0`.
fn shown(pair: &BTreeMap<String, String>) -> String {
    let label = |name: &str| if name == "cozy-runtime" { "Runtime" } else { "TensorFS" };
    pair.iter().map(|(name, version)| format!("{} {version}", label(name))).collect::<Vec<_>>().join(" and ")
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

/// What a person does about a floor nothing published meets.
fn floor_remedy(label: &str) -> String {
    let own = "or run it on a Runtime you built: `cozy machine update <machine> --runtime <path>`";
    match label.split(' ').next().filter(|package| !package.starts_with("local/")) {
        Some(package) => format!("try a compatible release with `cozy run {package}@<release>`, {own}"),
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
/// Its own process tree with no parent-death signal: it outlives the thread that asked (a run's
/// preparation, whose end killed it half done), and a PyTorch base, which has no Runtime's
/// trampoline, compiles too. compileall's forkserver and workers outlive a killed leader, so the
/// tree is killed once the leader exits.
pub(crate) fn compile_bytecode(python: &Path) {
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let script = format!(
        "import compileall, os, sysconfig\ngetattr(os, 'nice', lambda _: 0)(19)\n\
         compileall.compile_dir(sysconfig.get_paths()['purelib'], quiet=2, workers={workers})"
    );
    let started = || -> io::Result<_> {
        let mut command = crate::launch_identity::interpreter(python)?;
        command.envs(crate::launch_identity::interpreter_environment(python)?).args(["-I", "-c", &script]);
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let tree = platform::process::spawn(command)?;
        let leader = platform::process::Exact::open(&platform::process::Birth::of(tree.pid())?)?;
        Ok((tree, leader))
    };
    let reaped = started().and_then(|(mut tree, leader)| {
        std::thread::Builder::new().name("bytecode".into()).spawn(move || {
            // Unreaped, the leader's zombie keeps its group's number for the kill.
            let _ = leader.map(|leader| leader.wait());
            let _ = tree.kill();
            match tree.wait() {
                Ok(status) if !status.success() => tracing::warn!("environment bytecode: compileall exited {status}"),
                Err(error) => tracing::warn!("environment bytecode: {error}"),
                Ok(_) => {}
            }
        })
    });
    if let Err(error) = reaped {
        tracing::warn!("environment bytecode: {error}");
    }
}
