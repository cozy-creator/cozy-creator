//! Controller tools receive the same native filesystem boundary as project workers.
//! Their captured stdio and every writable tool cache belong to the project.
use std::{collections::HashMap, fs::{self,File}, io, path::{Path,PathBuf}, process::{Command,Output,Stdio}, sync::{LazyLock,Mutex}, time::Duration};
use platform::private_exec::{PrivateDescriptor,PrivateResources,prepare_project_writes};

static ACTIVE:LazyLock<Mutex<HashMap<PathBuf,usize>>>=LazyLock::new(Mutex::default);
pub(crate) fn active(root:&Path)->bool {
    let root=crate::project::vault::logical(root);
    ACTIVE.lock().expect("private controller tools").get(&root).is_some_and(|count|*count>0)
}
struct Running {root:PathBuf, release:bool}
impl Running {
    fn new(root:&Path)->Self {
        let root=crate::project::vault::logical(root);
        *ACTIVE.lock().expect("private controller tools").entry(root.clone()).or_default()+=1;
        Self{root,release:true}
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if !self.release { return; }
        let mut active=ACTIVE.lock().expect("private controller tools");
        if let Some(count)=active.get_mut(&self.root) { *count-=1; if *count==0 { active.remove(&self.root); } }
    }
}
fn error(why:impl std::fmt::Display)->io::Error { io::Error::new(io::ErrorKind::PermissionDenied,format!("project tool confinement unavailable: {why}")) }

fn executable(command:&Command)->io::Result<PathBuf> {
    let program=Path::new(command.get_program());
    if program.is_absolute() { return fs::canonicalize(program); }
    if program.components().count()!=1 { return Err(error("the tool executable is not absolute")); }
    let search=command.get_envs().find_map(|(key,value)|(key=="PATH").then_some(value)).flatten().map(std::ffi::OsStr::to_owned).or_else(||std::env::var_os("PATH")).unwrap_or_default();
    std::env::split_paths(&search).filter(|dir|dir.is_absolute()).find_map(|dir|fs::canonicalize(dir.join(program)).ok()).ok_or_else(||error("the tool executable is unavailable"))
}

pub(super) fn output(command:&mut Command)->io::Result<Output> {
    let scope=match crate::private_io::current() {
        Some(scope)=>Some(scope),
        None=>match command.get_current_dir() { Some(dir)=>crate::private_io::of_path(dir).map_err(error)?,None=>None },
    };
    let Some(scope)=scope else { return command.output(); };
    let root=scope.storage.view_root;
    let admission=crate::project::vault::try_admit_root(&crate::project::vault::logical(&root)).map_err(error)?;
    let cwd=fs::canonicalize(command.get_current_dir().ok_or_else(||error("a project tool needs an owned working directory"))?)?;
    if !cwd.starts_with(&root) { return Err(error("the tool working directory is outside the project")); }
    // This guard covers processes, captured output, and every policy/native handle.
    // A failed cleanup retains the guard below so storage cannot report closed.
    let mut running=Running::new(&root);
    drop(admission);
    let scratch=root.join(".vault-calcifer");
    for dir in ["processes","tmp","uv-cache","uv-tools","python","cache","data","config","state","bytecode"] {
        platform::durable::private_dir(&scratch.join(dir)).map_err(error)?;
    }
    let temporary=tempfile::Builder::new().prefix("tool-").tempdir_in(scratch.join("processes"))?;
    let input_path=temporary.path().join("stdin"); fs::write(&input_path,[])?;
    let stdin=File::open(input_path)?;
    let stdout=tempfile::NamedTempFile::new_in(temporary.path())?;
    let stderr=tempfile::NamedTempFile::new_in(temporary.path())?;
    let program=executable(command)?;
    let mut owned=Command::new(&program);
    owned.args(command.get_args()).current_dir(&cwd);
    for (key,value) in command.get_envs() { if let Some(value)=value { owned.env(key,value); } else { owned.env_remove(key); } }
    owned.env("HOME",&root).env("XDG_CACHE_HOME",scratch.join("cache")).env("XDG_CONFIG_HOME",scratch.join("config"))
        .env("XDG_DATA_HOME",scratch.join("data")).env("XDG_STATE_HOME",scratch.join("state"))
        .env("UV_CACHE_DIR",scratch.join("uv-cache")).env("UV_TOOL_DIR",scratch.join("uv-tools"))
        .env("UV_PYTHON_INSTALL_DIR",scratch.join("python")).env("TMPDIR",scratch.join("tmp"))
        .env("TEMP",scratch.join("tmp")).env("TMP",scratch.join("tmp"))
        .env("PYTHONPYCACHEPREFIX",scratch.join("bytecode")).env("PYTHONDONTWRITEBYTECODE","1")
        .stdin(Stdio::from(stdin.try_clone()?)).stdout(Stdio::from(stdout.as_file().try_clone()?)).stderr(Stdio::from(stderr.as_file().try_clone()?));
    let mut resources=PrivateResources {project_root:File::open(&root)?,read_only_trees:vec![],read_only_files:vec![],executables:vec![File::open(&program)?],devices:vec![],inherited:vec![PrivateDescriptor::ReadOnly(stdin),PrivateDescriptor::ProjectWritable(stdout.as_file().try_clone()?),PrivateDescriptor::ProjectWritable(stderr.as_file().try_clone()?)]};
    for path in ["/usr/lib","/usr/lib64","/usr/local/lib","/lib","/lib64","/usr/share","/usr/bin","/usr/local/bin","/bin","/etc/ssl/certs"] {
        if Path::new(path).is_dir() { resources.read_only_trees.push(File::open(path)?); }
    }
    for path in ["/etc/resolv.conf","/etc/hosts","/etc/nsswitch.conf","/etc/ld.so.cache","/etc/ssl/cert.pem"] {
        if Path::new(path).is_file() { resources.read_only_files.push(File::open(path)?); }
    }
    for path in ["/bin/sh","/usr/bin/env","/usr/bin/git","/usr/bin/python3","/usr/bin/python","/usr/local/bin/python3","/usr/local/bin/python"] {
        if Path::new(path).is_file() { resources.executables.push(File::open(fs::canonicalize(path)?)?); }
    }
    for path in ["/dev/null","/dev/urandom","/dev/random"] {
        resources.devices.push(fs::OpenOptions::new().read(true).write(path=="/dev/null").open(path)?);
    }
    let policy=prepare_project_writes(resources).map_err(error)?;
    let process_scope=platform::contain::Scope::create(&platform::contain::namespace(&scratch)).map_err(error)?;
    policy.apply_in_scope(&mut owned,&process_scope).map_err(error)?;
    let mut tree=platform::process::spawn(owned).map_err(error)?;
    running.release=false;
    let birth=platform::process::Birth::of(tree.pid()).map_err(error)?;
    // Preserve exact process births while the leader is alive. Reap only after
    // its group is ended; a reused PID can never authorize another process kill.
    let mut followers=HashMap::new();
    while !birth.ended().map_err(error)? {
        for member in platform::process::group_members(&birth) {
            if let Some(exact)=platform::process::Exact::open(&member).map_err(error)? { followers.entry(member).or_insert(exact); }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for member in platform::process::group_members(&birth) {
        if let Some(exact)=platform::process::Exact::open(&member).map_err(error)? { followers.entry(member).or_insert(exact); }
    }
    let closed=(|| {
        tree.kill().map_err(error)?;
        if process_scope.end().map_err(error)?!=0 || !platform::private_exec::project_scope_quiescent(&process_scope).map_err(error)? {return Err(error("tool descendants still hold the project"));}
        let followers=followers.into_values().collect::<Vec<_>>();
        platform::process::end_tree(&followers).map_err(error)?;
        let status=tree.wait().map_err(error)?;
        if !platform::process::group_members(&birth).is_empty() { return Err(error("tool descendants still hold the project")); }
        Ok(Output {status,stdout:fs::read(stdout.path())?,stderr:fs::read(stderr.path())?})
    })();
    if closed.is_err() {
        // Keep unresolved process custody visible to the lock gate. It must not
        // claim success after a failed native process-tree retirement.
        std::mem::forget(running);
        return closed;
    }
    drop(tree); drop(policy); drop(stdout); drop(stderr); temporary.close()?;
    running.release=true;
    drop(running);
    closed
}
