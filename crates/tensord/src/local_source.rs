//! A run's local source: unpublished code, named by the digest of its manifest object and sent
//! by content. No machine reads a host path: a manifest that names one is refused.
//!
//! Each manifest's files are their own tree (`source-<manifest>/tree`), so a later install cannot
//! rewrite code or data an accepted run still reads. The environment is keyed on what it is built
//! from (lock, project metadata, path dependencies, carried wheels, this machine's Python, pair
//! and PyTorch), never the project's own code, and installs the project editable with its paths
//! named from each generation's `source`: one environment runs every tree built on it, so an edit
//! to the code alone builds nothing. It uses the shared dependency cache.
use crate::{
    journal::Installation,
    published::{self, environment, Job, Publisher},
    objects::{Objects, Refused},
    service::Service,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};
use crate::stop::Stop;
use tensorfs::format::ObjectRef;
use tensorfs::sha256;

/// The manifest object (JSON). Members are objects the same signer wrote.
#[derive(Deserialize)]
pub(crate) struct Manifest {
    pub package: String,
    #[serde(default)]
    pub release: String,
    /// Wheels the project vendors, by file name.
    #[serde(default)]
    pub wheels: Vec<Member>,
    /// Installed dependency distribution -> the package whose invocables it owns.
    #[serde(default)]
    pub callees: std::collections::BTreeMap<String, String>,
    /// A live project and the local path dependencies its lock names, file by file under
    /// their common directory; `project` is the project's own directory there.
    #[serde(default)]
    pub files: Vec<Member>,
    #[serde(default)]
    pub project: String,
    /// The local path dependencies its lock installs, as directories or files under the tree:
    /// their code is the environment's, not the project's own.
    #[serde(default)]
    pub locals: Vec<String>,
}
#[derive(Deserialize)]
pub(crate) struct Member {
    #[serde(default)]
    pub name: String,
    pub digest: String,
    pub length: u64,
    #[serde(default)]
    pub executable: bool,
}
impl Manifest {
    /// The project's directory under the tree, as a prefix of its files' names.
    fn own(&self) -> String {
        match self.project.is_empty() {
            true => String::new(),
            false => format!("{}/", self.project),
        }
    }
    /// Whether `name` is a local path dependency's file, or outside the project.
    fn beside(&self, name: &str) -> bool {
        !name.starts_with(&self.own()) || self.locals.iter().any(|dir| name == dir || name.starts_with(&format!("{dir}/")))
    }
    /// The project's own code by content, the same wherever it is installed: what keys its own
    /// memoized calls (an editable install's files say nothing of the code they point at).
    fn source_digest(&self) -> String {
        let own = self.own();
        let code: Vec<_> = self.files.iter().filter(|f| !self.beside(&f.name)).map(|f| (&f.name[own.len()..], &f.digest)).collect();
        format!("sha256:{}", sha256::hex_digest(serde_json::json!(code).to_string().as_bytes()))
    }
}
impl Member {
    /// What a tree holds for this file: its bytes and whether it is executable.
    fn held(&self) -> String {
        format!("{}{}", self.digest, if self.executable { "+x" } else { "" })
    }
}

/// One package's installer.
#[derive(Default)]
struct Installing {
    turn: Mutex<()>,
    /// The run whose install has the turn: what a second run waits for.
    holder: Mutex<Option<String>>,
}

/// The installer's holder, forgotten however its install ends.
struct Holder<'a>(&'a Mutex<Option<String>>);
impl Drop for Holder<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap() = None;
    }
}

pub(crate) struct LocalSources {
    pub objects: Arc<Objects>,
    /// Local code's environments are built as releases' are.
    publisher: Arc<Publisher>,
    /// The machine's generations (`catalog`).
    generations: PathBuf,
    /// Each signer's packages: `env-<inputs>` environments and `source-<manifest>/tree` trees.
    editable: PathBuf,
    /// By package: one package's installs go in turn, and another's go beside them.
    installing: Mutex<HashMap<String, Arc<Installing>>>,
    /// The generation each run still preparing was handed: no other install of its package takes
    /// it away before the run binds and the journal names it (cl-773).
    preparing: Mutex<HashMap<String, String>>,
}

/// `name` as a path inside a tree: relative, with no step out of it.
fn inside(name: &str) -> Result<&Path, Refused> {
    let path = Path::new(name);
    if path.components().all(|c| matches!(c, Component::Normal(_))) {
        return Ok(path);
    }
    Err(refused("local_source_invalid", format!("{name:?} is not a path inside the package's tree")))
}

/// The files the tree under `home` holds, by name; none while it is absent or half written.
fn held_tree(home: &Path) -> BTreeMap<String, String> {
    fs::read(home.join("tree.json")).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()
}

fn keyed(name: &str, prefix: &str, length: usize) -> bool {
    name.strip_prefix(prefix).is_some_and(|key| key.len() == length && key.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Only this layout's environments leave. Unknown entries in a package home are preserved.
fn collect_environments(home: &Path, named: &HashSet<PathBuf>) {
    for entry in fs::read_dir(home).into_iter().flatten().flatten() {
        let path = entry.path();
        if !keyed(&entry.file_name().to_string_lossy(), "env-", 16) || !path.is_dir() {
            continue;
        }
        if !named.contains(&platform::durable::canonical(&path).unwrap_or_else(|_| path.clone())) {
            removed("editable environment", &path);
            removed("carried wheels", &path.with_extension("wheels"));
        }
    }
}

/// Removes `path` (`what`), logging the bytes it freed.
fn removed(what: &str, path: &Path) {
    let bytes = crate::reclaim::freeable(path);
    if fs::remove_dir_all(path).is_ok() {
        if !crate::private_io::is_private_path(path) {
            tracing::info!("removed {what} {}: {}", path.display(), crate::reclaim::size(bytes));
        }
    }
}

fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        code,
        message: message.into(),
    }
}

/// One member as the object this signer wrote.
fn written_member(objects: &Objects, actor: &str, m: &Member) -> Result<ObjectRef, Refused> {
    match objects.path(actor, &m.digest)? {
        Some((_, length)) if length == m.length => Ok(ObjectRef {
            sha256: m.digest.trim_start_matches("sha256:").to_string(),
            length,
        }),
        _ => Err(refused(
            "local_source_incomplete",
            format!("{} {} was not written to this machine", m.name, m.digest),
        )),
    }
}

/// The manifest `digest` (`sha256:<hex>`) as this signer wrote it.
fn open_manifest(
    objects: &Objects,
    actor: &str,
    digest: &str,
) -> Result<(Manifest, ObjectRef), Refused> {
    let (path, length) = objects.path(actor, digest)?.ok_or_else(|| {
        refused(
            "local_source_incomplete",
            format!("the local package manifest {digest} was not written to this machine"),
        )
    })?;
    if length > 1 << 20 {
        return Err(refused(
            "local_source_invalid",
            "a local package manifest is at most 1 MiB",
        ));
    }
    let invalid = |e: serde_json::Error| refused("local_source_invalid", format!("the local package manifest is invalid: {e}"));
    let document: serde_json::Value = serde_json::from_slice(&fs::read(path)?).map_err(invalid)?;
    if document.get("path").is_some() {
        return Err(refused(
            "local_source_path_refused",
            "a local package manifest names its files by content: no machine reads a host path (`path`)",
        ));
    }
    let manifest: Manifest = serde_json::from_value(document).map_err(invalid)?;
    if manifest.files.is_empty() {
        return Err(refused("local_source_invalid", "a local package manifest names its files"));
    }
    let sha256 = digest.trim_start_matches("sha256:").to_string();
    Ok((manifest, ObjectRef { sha256, length }))
}

impl LocalSources {
    pub(crate) fn new(objects: Arc<Objects>, publisher: Arc<Publisher>, generations: &Path) -> Self {
        Self {
            editable: generations.with_file_name("editable"),
            generations: generations.to_path_buf(),
            objects,
            publisher,
            installing: Mutex::new(HashMap::new()),
            preparing: Mutex::default(),
        }
    }

    /// Where `actor`'s live `package` is kept.
    fn home(&self, actor: &str, package: &str) -> Result<PathBuf, Refused> {
        let name = crate::catalog::normalized(package.strip_prefix("local/").unwrap_or_default());
        if name.is_empty() || name.contains('/') {
            return Err(refused("local_source_invalid", "a local package manifest names local/<name>"));
        }
        let root = crate::private_io::cache_root(self.editable.clone(), "editable")?;
        Ok(root.join(&sha256::hex_digest(actor.as_bytes())[..16]).join(name))
    }

    /// A local source's objects, its manifest and every member: what a run takes custody of when
    /// it is accepted. Every captured member is retained: a cached generation may be collected
    /// while this run waits to install. The caller's object cache still avoids reuploading
    /// unchanged bytes. One the signer did not write is `local_source_incomplete`, naming it.
    pub(crate) fn written(&self, actor: &str, digest: &str) -> Result<Vec<ObjectRef>, Refused> {
        let (manifest, own) = open_manifest(&self.objects, actor, digest)?;
        self.home(actor, &manifest.package)?;
        let mut written = vec![own];
        for m in manifest.wheels.iter().chain(&manifest.files) {
            written.push(written_member(&self.objects, actor, m)?);
        }
        Ok(written)
    }

    /// The identity of the environment a manifest runs in (`env-<identity[..16]>`): one per lock,
    /// project metadata, local path dependencies, carried wheels, the project's place in its tree,
    /// and this machine's interpreter, the SDK pair it takes (`Publisher::pair_key`) and
    /// PyTorch. The project's own code is none of it: each generation names its tree.
    fn environment(&self, manifest: &Manifest, locked: Option<&published::Locked>) -> String {
        let own = manifest.own();
        let held = |name: &str| {
            let path = format!("{own}{name}");
            manifest.files.iter().find(|f| f.name == path).map(|f| f.digest.as_str()).unwrap_or_default()
        };
        let beside: Vec<_> = manifest.files.iter().filter(|f| manifest.beside(&f.name)).map(|f| (&f.name, f.held())).collect();
        let carried: Vec<_> = manifest.wheels.iter().map(|w| (&w.name, &w.digest)).collect();
        let key = serde_json::json!({
            "lock": held("uv.lock"), "project": held("pyproject.toml"), "beside": beside, "wheels": carried,
            "own": manifest.project,
            "pair": self.publisher.pair_key(locked), "python": self.publisher.sdk.python,
            "torch": self.publisher.torch_pins(None),
            "private_view": crate::private_io::current().map(|scope| scope.view_root),
        });
        sha256::hex_digest(key.to_string().as_bytes())
    }

    /// Make the tree under `home` the manifest's files: changed ones are replaced from the
    /// objects the signer wrote, and ones it no longer names leave.
    fn sync(&self, actor: &str, home: &Path, files: &[Member]) -> Result<PathBuf, Refused> {
        let tree = home.join("tree");
        let held = held_tree(home);
        let next: BTreeMap<String, String> = files.iter().map(|f| (f.name.clone(), f.held())).collect();
        if next != held {
            // Half synced, the tree holds what neither manifest names: forget it until done.
            let _ = fs::remove_file(home.join("tree.json"));
        }
        for file in files {
            let target = tree.join(inside(&file.name)?);
            if held.get(&file.name) == Some(&file.held()) && target.is_file() {
                continue;
            }
            let object = written_member(&self.objects, actor, file)?;
            fs::create_dir_all(target.parent().expect("a file inside the tree"))?;
            let incoming = home.join(".incoming");
            fs::copy(self.objects.store()?.path(&object).map_err(crate::storage::io_error)?, &incoming)?;
            platform::durable::set_mode(&incoming, if file.executable { 0o755 } else { 0o644 }).map_err(std::io::Error::from)?;
            fs::rename(&incoming, &target)?;
        }
        for gone in held.keys().filter(|name| !next.contains_key(*name)) {
            let path = tree.join(gone);
            let _ = fs::remove_file(&path);
            // Directories it emptied leave with it.
            let mut parent = path.parent();
            while let Some(directory) = parent.filter(|d| *d != tree && fs::remove_dir(d).is_ok()) {
                parent = directory.parent();
            }
        }
        if next != held {
            fs::create_dir_all(&tree)?;
            fs::write(home.join(".tree.json"), serde_json::to_vec(&next).map_err(std::io::Error::other)?)?;
            fs::rename(home.join(".tree.json"), home.join("tree.json"))?;
        }
        Ok(tree)
    }

    /// Install editable against this manifest's tree: its environment built as a release's is
    /// (`published::environment`) when none built from the same inputs is ready, then a generation
    /// of it and the tree describing the project as it is now.
    #[allow(clippy::too_many_arguments)]
    fn install_live(
        &self,
        service: &Service,
        actor: &str,
        manifest_digest: &str,
        manifest: Manifest,
        digest: &str,
        job: &Job,
    ) -> Result<Installation, Refused> {
        let home = self.home(actor, &manifest.package)?;
        let source = home.join(format!("source-{digest}"));
        fs::create_dir_all(&source)?;
        let tree = self.sync(actor, &source, &manifest.files)?;
        let project = match manifest.project.is_empty() {
            true => tree.clone(),
            false => tree.join(inside(&manifest.project)?),
        };
        if !project.join("pyproject.toml").is_file() {
            return Err(refused("local_source_invalid", format!("the manifest's files hold no {}pyproject.toml", manifest.own())));
        }
        // This machine's PyTorch first (its own, the first time): the environment and alias are keyed on it.
        self.publisher.layer(job, None).map_err(failed)?;
        let alias = alias(&self.publisher, manifest_digest);
        let pyproject = fs::read_to_string(project.join("pyproject.toml"))?;
        let locked = match fs::read_to_string(project.join("uv.lock")) {
            Ok(lock) => Some(published::read_lock(&pyproject, &lock, None).map_err(failed)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let built_from = self.environment(&manifest, locked.as_ref());
        let environment = home.join(format!("env-{}", &built_from[..16]));
        let ready = environment.join(".ready");
        let label = format!("{} {}", manifest.package, manifest.release);
        // Reused, it is in use again: reclaim drops only an environment no generation names that
        // has been left alone a while (`reclaim::IDLE`).
        let _ = platform::durable::set_times(&environment, fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
        if !ready.is_file() {
            let stage = format!("installing {} on this machine", manifest.package);
            job.stage(stage.clone());
            // Wheels the CLI carries for locked dependencies this machine cannot fetch itself (a
            // Hub's on the caller's computer, a private git source): beside the environment they
            // install into, and needed only while it is built.
            let carried = environment.with_extension("wheels");
            let _ = fs::remove_dir_all(&carried);
            fs::create_dir_all(&carried)?;
            for wheel in &manifest.wheels {
                if !wheel.name.ends_with(".whl") || wheel.name.contains('/') {
                    return Err(refused("local_source_invalid", format!("{:?} is not a wheel file name", wheel.name)));
                }
                let object = written_member(&self.objects, actor, wheel)?;
                fs::copy(self.objects.store()?.path(&object).map_err(crate::storage::io_error)?, carried.join(&wheel.name))?;
            }
            let built = published::patiently(job, || self.publisher.build(&environment, &environment::Project {
                dir: &project,
                python: &self.publisher.sdk.python,
                locked: locked.as_ref(),
                wheels: (!manifest.wheels.is_empty()).then_some(carried.as_path()),
                skip: &[],
                editable: Some(&tree),
                package: &manifest.package,
                label: &label,
                stage: &stage,
                fetch: None,
            }, job)).map_err(failed)?;
            let _ = fs::remove_dir_all(&carried);
            fs::write(&ready, serde_json::to_vec(&built).map_err(std::io::Error::other)?)?;
        }
        let built: environment::Built = serde_json::from_slice(&fs::read(&ready)?).map_err(std::io::Error::other)?;
        // One generation per environment and tree: a run of code unchanged since reads it as it is.
        let identity = sha256::hex_digest(serde_json::json!([environment, digest]).to_string().as_bytes())[..32].to_string();
        if service.catalog.resolve(&identity).is_err() {
            job.stage(format!("describing {}", manifest.package));
            let dir = crate::private_io::cache_root(service.catalog.root().to_owned(), "generations")?.join(&identity);
            let _ = fs::remove_dir_all(&dir); // an interrupted, never-published one
            fs::create_dir_all(&dir)?;
            let published = || -> Result<(), published::Failure> {
                environment::hold(&dir)?;
                let link = |target: &Path, name| platform::durable::symlink(target, &dir.join(name)).map_err(|e| published::io_failure(e.into()));
                link(&environment, "env")?;
                link(&tree, environment::SOURCE)?;
                let (env, distribution) = (dir.join("env"), published::distribution(&pyproject)?);
                let python = platform::paths::venv_python(&env);
                let stop = job.stop.as_ref();
                // Its lock's Runtime too old for its code: re-locked in its folder.
                let unread = |failure: published::Failure| built.unread(failure, &env, &manifest.package, "uv lock --upgrade-package cozy-runtime");
                let (interface, closures) = environment::describe_project(&python, &project, stop).map_err(unread)?;
                let callees = callees(&environment, &python, &distribution, &manifest.callees, stop).map_err(unread)?;
                let described = environment::Described { source_digest: manifest.source_digest(), callees, closures };
                let record = built.record((&identity, &built_from), (&distribution, &manifest.release), &env, &interface,
                    &self.publisher.sdk.runtime_build, described)?;
                environment::publish(&dir, &record)
            };
            if let Err(failure) = published() {
                let _ = fs::remove_dir_all(&dir);
                return Err(failed(failure));
            }
        }
        let held = service.catalog.resolve(&identity)?;
        let installed = service.engine.bind_installation(Installation {
            actor: actor.to_string(),
            alias,
            generation: identity,
            package: manifest.package,
            release: manifest.release,
            interface: serde_json::to_vec(&held.record.interface).map_err(std::io::Error::other)?,
            hub: String::new(),
        })?;
        service.installed(&held);
        Ok(installed)
    }

    /// `run` ended its preparation: bound, the journal names its generation; else nothing does.
    pub(crate) fn prepared(&self, run: &str) {
        self.preparing.lock().unwrap().remove(run);
    }

    /// The generations of runs still preparing.
    pub(crate) fn preparing(&self) -> HashSet<String> {
        self.preparing.lock().unwrap().values().cloned().collect()
    }

    fn installer(&self, package: &str) -> Arc<Installing> {
        self.installing.lock().unwrap().entry(crate::private_io::resource_key(package)).or_default().clone()
    }

    /// Keep remote source homes while a generation names their environment. Only completed,
    /// recognized source homes leave; partial or unknown homes are preserved for recovery.
    pub(crate) fn collect(&self, actor: &str, package: &str) {
        let Ok(home) = self.home(actor, package) else {
            return;
        };
        let installer = self.installer(package);
        let _installing = installer.turn.lock().unwrap();
        // An unavailable or partial inventory says nothing about which sources are unused.
        let Ok(generation_root) = crate::private_io::cache_root(self.generations.clone(), "generations") else { return };
        let Ok(generations) = fs::read_dir(generation_root)
            .and_then(|entries| entries.collect::<std::io::Result<Vec<_>>>()) else { return; };
        // What generations link, canonical as each home's entries are read.
        let mut named = HashSet::new();
        for generation in generations {
            for name in ["env", environment::SOURCE] {
                let link = generation.path().join(name);
                match fs::read_link(&link) {
                    Ok(target) => { named.insert(platform::durable::canonical(&target).unwrap_or(target)); }
                    // Published generations own an ordinary environment directory, and no tree.
                    Err(error) if error.kind() == std::io::ErrorKind::InvalidInput && link.is_dir() => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound && name == environment::SOURCE => {}
                    Err(_) => return,
                }
            }
        }
        collect_environments(&home, &named);
        for entry in fs::read_dir(&home).into_iter().flatten().flatten() {
            let path = entry.path();
            let root = platform::durable::canonical(&path).unwrap_or_else(|_| path.clone());
            // A half-synced tree is kept for recovery, and one a generation links (or holds the environment of).
            if keyed(&entry.file_name().to_string_lossy(), "source-", 64)
                && !held_tree(&path).is_empty() && !named.iter().any(|linked| linked.starts_with(&root))
            {
                removed("editable source", &path);
            }
        }
        let _ = fs::remove_dir(&home); // Only an empty home; never erase unknown entries.
    }

    /// This signer's installation of the manifest `digest` (`sha256:<hex>`).
    /// `stage` hears what is built: nothing for an environment this machine holds. While
    /// another run's install holds the installer, run `run` is told it waits for that run. Its
    /// build takes its turn in the machine's install line at `order` (`line`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install(
        &self,
        service: &Service,
        actor: &str,
        digest: &str,
        stop: Option<&Stop>,
        run: &str,
        order: u64,
        observer: published::Observer,
    ) -> Result<Installation, Refused> {
        let package = open_manifest(&self.objects, actor, digest).map(|(m, _)| m.package).unwrap_or_default();
        let installer = self.installer(&package);
        let _installing = match installer.turn.try_lock() {
            Ok(installing) => installing,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                let reason = match installer.holder.lock().unwrap().clone() {
                    Some(holder) => format!("waiting for run {holder} to install {package}"),
                    None => "waiting for another run's install".into(),
                };
                let tell = |reason| match run.is_empty() {
                    true => Ok(()),
                    false => service.engine.waiting(run, reason),
                };
                tell(Some(reason))?;
                let installing = installer.turn.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                tell(None)?;
                installing
            }
        };
        *installer.holder.lock().unwrap() = Some(service.engine.caller_id(run));
        let _held = Holder(&installer.holder);
        let alias = alias(&self.publisher, digest);
        if let Some(held) = service.engine.installation(actor, &alias)? {
            if service.catalog.resolve(&held.generation).is_ok() {
                self.preparing.lock().unwrap().insert(run.to_string(), held.generation.clone());
                return Ok(held);
            }
        }
        let (manifest, object) = open_manifest(&self.objects, actor, digest)?;
        let (engine, waiter) = (service.engine.clone(), run.to_string());
        let job = Job {
            observer: Some(observer),
            stop: stop.cloned(),
            order,
            caller: service.engine.caller_id(run),
            waiting: (!run.is_empty()).then(|| {
                Box::new(move |reason| {
                    if let Err(error) = engine.waiting(&waiter, reason) {
                        tracing::warn!("run {waiter}: its waiting reason was not shown: {error}");
                    }
                }) as Box<dyn Fn(Option<String>) + Send + Sync>
            }),
            ..Default::default()
        };
        let installed = self.install_live(service, actor, digest, manifest, &object.sha256, &job)?;
        self.preparing.lock().unwrap().insert(run.to_string(), installed.generation.clone());
        Ok(installed)
    }
}

/// The installation alias of the manifest `digest` (`sha256:<hex>`) on this machine. Like a published installation's, it
/// includes the selected SDK: the digest alone cannot identify the environment after an update.
pub(crate) fn alias(publisher: &Publisher, digest: &str) -> String {
    let sdk = &publisher.sdk;
    let mut key = serde_json::json!({"manifest": digest, "sdk": sdk.requirements, "links": sdk.find_links, "python": sdk.python, "torch": publisher.torch_pins(None)});
    // Private environments embed their physical interpreter/source paths. A remount at a
    // different view needs a new cache binding; the old immutable installation remains
    // available to historical runs. This cache key is not the authored RunSpec digest.
    if let Some(scope) = crate::private_io::current() {
        key["private_view"] = serde_json::json!(scope.view_root);
    }
    format!("local-{}", &tensorfs::sha256::hex_digest(key.to_string().as_bytes())[..32])
}

/// The other Apps `environment` holds, by `packages` (distribution -> package): described once
/// per environment, as its files are the same for every tree it runs.
fn callees(environment: &Path, python: &Path, root: &str, packages: &BTreeMap<String, String>, stop: Option<&Stop>)
    -> Result<Vec<crate::catalog::Callee>, published::Failure> {
    let key = sha256::hex_digest(serde_json::json!([root, packages]).to_string().as_bytes());
    let kept = environment.join(format!(".callees-{}.json", &key[..16]));
    if let Some(callees) = fs::read(&kept).ok().and_then(|raw| serde_json::from_slice(&raw).ok()) {
        return Ok(callees);
    }
    let callees = environment::describe_environment(python, root, packages, false, stop)?.callees;
    let staged = kept.with_extension("new");
    let wrote = serde_json::to_vec(&callees).map_err(std::io::Error::other).and_then(|raw| fs::write(&staged, raw)).and_then(|()| fs::rename(&staged, &kept));
    if let Err(error) = wrote {
        if !crate::private_io::is_private_path(environment) {
            tracing::warn!("{}: its callees were not kept: {error}", environment.display());
        }
    }
    Ok(callees)
}

/// A build's failure as the run's reason.
fn failed((code, message): published::Failure) -> Refused {
    refused(code, message)
}
