//! The executor's launch: its trampoline, inherited environment and seal.
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};
#[cfg(unix)]
use std::os::{
    fd::AsRawFd,
    unix::{fs::MetadataExt, process::CommandExt},
};

/// This process's user: the effective uid, or 0 where there is none (Windows).
pub(crate) fn euid() -> u32 {
    #[cfg(unix)]
    return nix::unistd::geteuid().as_raw();
    #[cfg(windows)]
    0
}

/// This process's group: the effective gid, or 0 where there is none (Windows).
pub(crate) fn egid() -> u32 {
    #[cfg(unix)]
    return nix::unistd::getegid().as_raw();
    #[cfg(windows)]
    0
}

/// Reuse Runtime's pre-CUDA seal: expected parent, parent-death SIGKILL, no_new_privs and
/// OOM score. Every child gets its own process group, so a kill
/// reaches the descendants that stayed in it and a terminal signal to the machine does not.
#[cfg(unix)]
pub fn trampoline(python: &Path, scope: Option<&platform::contain::Scope>) -> io::Result<Command> {
    let mut command = Command::new(python);
    platform::process::inherit_nothing(&mut command);
    command
        .args([
            "-I",
            "-m",
            "cozy_runtime.internal.trampoline",
            "--expect-parent",
        ])
        .arg(std::process::id().to_string())
        .args(["--oom-adj", "1000"]);
    // The trampoline joins the executor's own cgroup before anything is imported; a token
    // scope and the process group (set below) need nothing of it.
    match scope {
        Some(scope) => command.args(scope.trampoline_args()),
        None => command.args(["--scope-backend", "inherit"]),
    };
    command.process_group(0).arg("--").arg(python);
    Ok(command)
}

/// Windows: no trampoline. The executor starts as its Python, its tree held by the machine's
/// handle on it; there is no parent-death signal and no seal of privileges.
/// A venv's python.exe is a launcher that starts the interpreter as its child, which would make
/// the executor a stranger's child: the interpreter starts itself, told its venv as the launcher
/// tells it ([`interpreter_environment`]).
#[cfg(windows)]
pub fn trampoline(python: &Path, _scope: Option<&platform::contain::Scope>) -> io::Result<Command> {
    let mut command = Command::new(venv_home(python)?.map_or_else(|| python.into(), |home| home.join("python.exe")));
    platform::process::inherit_nothing(&mut command);
    Ok(command)
}

/// The environment `trampoline`'s interpreter needs beside the executor's own: on Windows, the
/// venv python it stands in for (`__PYVENV_LAUNCHER__`, CPython's own handoff). None elsewhere.
pub fn interpreter_environment(python: &Path) -> io::Result<Option<(&'static str, PathBuf)>> {
    #[cfg(windows)]
    if venv_home(python)?.is_some() {
        return Ok(Some(("__PYVENV_LAUNCHER__", python.to_path_buf())));
    }
    let _ = python;
    Ok(None)
}

/// The base interpreter's directory a venv's `pyvenv.cfg` names (`home`); None for a Python that
/// is no venv's.
#[cfg(windows)]
fn venv_home(python: &Path) -> io::Result<Option<PathBuf>> {
    let Some(venv) = python.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    let text = match fs::read_to_string(venv.join("pyvenv.cfg")) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let home = text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "home").then(|| PathBuf::from(value.trim()))
    });
    home.map(Some).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "pyvenv.cfg names no home"))
}

/// Runtime `child_env.ERASED_PREFIXES`: inherited names no executor sees. The seal imposes
/// the ones it needs after this erase, so no image or operator export can redirect them.
const ERASED_PREFIXES: [&str; 18] = [
    "COZY_",
    "CUDA_",
    "PYTORCH_",
    "PYTHON",
    "TORCH_",
    "TORCHINDUCTOR_",
    "TRITON_",
    "NCCL_",
    "HF_",
    "HUGGINGFACE_",
    "TENSORHUB_",
    "CIVITAI_",
    "COMFY_",
    "OMP_",
    "MKL_",
    "FLASH_ATTENTION_",
    "TMPDIR",
    "TENSORFS_",
];

pub fn credential_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "CREDENTIAL",
        "API_KEY",
        "AUTH",
    ]
    .iter()
    .any(|word| upper.contains(word))
}

/// The process environment every machine-launched package process starts from: the
/// machine's own environment without credentials or names the seal owns.
pub fn inherited() -> BTreeMap<String, String> {
    protocol::config::environment()
        .iter()
        .filter(|(name, _)| {
            !credential_name(name) && !ERASED_PREFIXES.iter().any(|p| name.starts_with(p))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// What the machine imposes on an executor before CUDA can initialize (Runtime
/// `Worker.imposed` plus its JIT/kernel scopes). The executor reports what it received in
/// Hello and the machine compares every name both know.
#[derive(Clone, Debug)]
pub struct Seal {
    /// `CUDA_VISIBLE_DEVICES`; empty means no GPU.
    pub devices: String,
    /// The shared TensorFS store, separate from the executor's home.
    pub tensorfs_home: Option<PathBuf>,
    /// Degree above one: NVLS off and GPU peer memory only over NVLink (Runtime cr-068).
    pub group: bool,
    /// `COZY_HOME`: attention/kernel qualification is kept here between executors.
    pub home: PathBuf,
    /// Upstream JIT caches and `TMPDIR`, scoped to one machine incarnation and generation.
    pub jit: PathBuf,
    /// This identity's persistent kernel-store namespace.
    pub kernels: PathBuf,
    /// The namespace's lock (`reclaim::kernel_hold`), shared: the process launched under
    /// this seal inherits it, so no sweep deletes a kernel while it lives.
    pub kernel_hold: Option<Arc<File>>,
    pub generation: String,
}

/// The launched process keeps these shared locks open (not close-on-exec) until it exits.
/// Windows inherits none: there the holds last while the machine does.
#[cfg(windows)]
pub fn inherit(_: &mut Command, _: &[Option<&Arc<File>>]) {}

/// The launched process keeps these shared locks open (not close-on-exec) until it exits.
#[cfg(unix)]
pub fn inherit(command: &mut Command, holds: &[Option<&Arc<File>>]) {
    platform::process::keep_open(command, holds.iter().flatten().map(|hold| hold.as_raw_fd()).collect());
}

/// `OMP_NUM_THREADS` the seal imposes. torch's allocator is the Runtime's, set in code.
const THREADS: u32 = 4;

impl Seal {
    /// Create the owner's directories under `root` for one generation.
    pub fn prepare(
        root: &Path,
        incarnation: &str,
        generation: &str,
        devices: &str,
    ) -> io::Result<Self> {
        if incarnation.is_empty() || !incarnation.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a machine run is named by an alphanumeric incarnation",
            ));
        }
        // Directory names never carry a caller's spelling of the generation.
        let generation = &tensorfs::sha256::hex_digest(generation.as_bytes())[..32];
        let namespace = format!("u{}", euid());
        let owned = |path: PathBuf| -> io::Result<PathBuf> {
            match fs::create_dir(&path) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
            platform::durable::set_mode(&path, 0o700)?;
            Ok(path)
        };
        fs::create_dir_all(root)?;
        for boundary in ["home", "kernels", "jit"] {
            boundary_directory(&root.join(boundary))?;
        }
        boundary_directory(&root.join("jit").join(incarnation))?;
        let jit = owned(root.join("jit").join(incarnation).join(generation))?;
        // On a low disk the persistent kernel store is an optional write: compiled kernels
        // then live in this run's JIT scope, removed with it.
        let (kernels, kernel_hold) = if crate::reclaim::measure(root)?.short().is_some() {
            (owned(jit.join("kernels"))?, None)
        } else {
            let hold = crate::reclaim::kernel_hold(&root.join("kernels"), &namespace)?;
            hold.lock_shared()?; // waits out a sweep deleting in this namespace
            let kernels = owned(root.join("kernels").join(&namespace))?;
            (kernels, Some(Arc::new(hold)))
        };
        owned(kernels.join(format!("torch-kernels.{generation}")))?;
        Ok(Self {
            devices: devices.into(),
            tensorfs_home: None,
            group: false,
            home: owned(root.join("home").join(&namespace))?,
            jit,
            kernels,
            kernel_hold,
            generation: generation.into(),
        })
    }

    pub fn imposed(&self) -> BTreeMap<String, String> {
        let path = |path: PathBuf| path.to_string_lossy().into_owned();
        let mut imposed = BTreeMap::from([
            ("CUDA_VISIBLE_DEVICES".to_string(), self.devices.clone()),
            ("OMP_NUM_THREADS".into(), THREADS.to_string()),
            ("COZY_HOME".into(), path(self.home.clone())),
            ("TMPDIR".into(), path(self.jit.clone())),
            ("CUDA_CACHE_PATH".into(), path(self.jit.join("cuda"))),
            (
                "PYTHONPYCACHEPREFIX".into(),
                path(self.jit.join("bytecode")),
            ),
            (
                "TORCH_EXTENSIONS_DIR".into(),
                path(self.jit.join("extensions")),
            ),
            (
                "TORCHINDUCTOR_CACHE_DIR".into(),
                path(self.jit.join("inductor")),
            ),
            ("COZY_KERNEL_CACHE".into(), path(self.kernels.clone())),
            ("TRITON_CACHE_DIR".into(), path(self.kernels.join("triton"))),
            (
                "PYTORCH_KERNEL_CACHE_PATH".into(),
                path(
                    self.kernels
                        .join(format!("torch-kernels.{}", self.generation)),
                ),
            ),
            (
                "FLASH_ATTENTION_CUTE_DSL_CACHE_DIR".into(),
                path(self.kernels.join("flash-attn4")),
            ),
            ("FLASH_ATTENTION_CUTE_DSL_CACHE_ENABLED".into(), "1".into()),
        ]);
        // NVLink SHARP needs a privilege containers lack. The peer route is the Runtime's: it
        // measures the group's links and sets NCCL's P2P level before the group forms.
        if let Some(root) = &self.tensorfs_home {
            imposed.insert("TENSORFS_HOME".into(), path(root.clone()));
        }
        if self.group {
            imposed.insert("NCCL_NVLS_ENABLE".into(), "0".into());
        }
        imposed
    }

    /// Inherited, then explicitly configured locations, then the seal, which always wins.
    pub fn environment(&self, configured: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut environment = inherited();
        environment.extend(
            configured
                .iter()
                .filter(|(name, _)| !credential_name(name))
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        environment.extend(self.imposed());
        environment
    }

    /// Imposed names the executor did not report receiving exactly. The machine and its
    /// Runtime ship together (hard cut), so every imposed name is in its allowlist.
    pub fn mismatches(&self, reported: &BTreeMap<String, String>) -> Vec<String> {
        self.imposed()
            .into_iter()
            .filter(|(name, value)| reported.get(name) != Some(value))
            .map(|(name, _)| name)
            .collect()
    }
}

/// JIT scopes belong to one machine run (Runtime: one worker boot); the executors of
/// earlier runs are gone before a new one starts, so their scopes are unowned.
pub fn remove_stale_jit(root: &Path, incarnation: &str) {
    let Ok(entries) = fs::read_dir(root.join("jit")) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != incarnation {
            if let Err(error) = fs::remove_dir_all(entry.path()) {
                tracing::warn!("stale JIT scope {}: {error}", entry.path().display());
            }
        }
    }
}

/// Owner-held directory that executors may traverse but not list.
fn boundary_directory(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    // Windows has no POSIX owner: the profile's ACL keeps the directory the user's.
    #[cfg(unix)]
    let owned = metadata.uid() == euid();
    #[cfg(windows)]
    let owned = true;
    if !metadata.is_dir() || !owned {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "seal boundary is not an owner directory",
        ));
    }
    Ok(platform::durable::set_mode(path, 0o711)?)
}
