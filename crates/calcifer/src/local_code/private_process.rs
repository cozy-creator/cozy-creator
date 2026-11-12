//! Controller-owned tool storage and captured diagnostics stay inside the project.
//! Their captured stdio and every writable tool cache belong to the project.
use std::{collections::HashMap, fs::{self,File}, io::{self,Read as _,Write as _}, path::{Path,PathBuf}, process::{Command,Output,Stdio}, sync::{Arc,LazyLock,Mutex,atomic::{AtomicBool,Ordering}}, time::Duration};

static ACTIVE:LazyLock<Mutex<HashMap<PathBuf,Vec<Arc<AtomicBool>>>>>=LazyLock::new(Mutex::default);
static NAMESPACE_FILE:Mutex<()>=Mutex::new(());
pub(crate) fn active(root:&Path)->bool {
    let root=crate::project::vault::logical(root);
    ACTIVE.lock().expect("private controller tools").get(&root).is_some_and(|tools|!tools.is_empty())
}
pub(crate) fn cancel(root:&Path) {
    let root=crate::project::vault::logical(root);
    if let Some(tools)=ACTIVE.lock().expect("private controller tools").get(&root) {
        for tool in tools {tool.store(true,Ordering::Release);}
    }
}
struct Running {root:PathBuf, cancelled:Arc<AtomicBool>, release:bool}
impl Running {
    fn new(root:&Path)->Self {
        let root=crate::project::vault::logical(root);
        let cancelled=Arc::new(AtomicBool::new(false));
        ACTIVE.lock().expect("private controller tools").entry(root.clone()).or_default().push(cancelled.clone());
        Self{root,cancelled,release:true}
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if !self.release { return; }
        let mut active=ACTIVE.lock().expect("private controller tools");
        if let Some(tools)=active.get_mut(&self.root) {
            tools.retain(|tool|!Arc::ptr_eq(tool,&self.cancelled));
            if tools.is_empty() {active.remove(&self.root);}
        }
    }
}
fn error(why:impl std::fmt::Display)->io::Error { io::Error::new(io::ErrorKind::PermissionDenied,format!("project tool custody unavailable: {why}")) }

/// Stable across provider remounts and folder moves, with no outside ledger.
fn namespace(root:&Path,create:bool)->io::Result<Option<String>> {
    let _serial=NAMESPACE_FILE.lock().expect("project tool namespace");
    let scratch=root.join(".vault-calcifer");
    let path=scratch.join("process-namespace");
    match fs::symlink_metadata(&path) {
        Ok(_)=>{},
        Err(why) if why.kind()==io::ErrorKind::NotFound && !create=>return Ok(None),
        Err(why) if why.kind()==io::ErrorKind::NotFound=>{
            let mut random=[0;8];getrandom::fill(&mut random).map_err(error)?;
            let value=format!("{}\n",hex::encode(random));
            let mut file=platform::durable::create_private_new(&path).map_err(error)?;
            file.write_all(value.as_bytes())?;file.sync_all()?;
            platform::durable::sync_dir(&scratch).map_err(error)?;
        },
        Err(why)=>return Err(why),
    }
    let metadata=fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len()!=17 {
        return Err(error("the retained process namespace has an unknown format"));
    }
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid()!=rustix::process::geteuid().as_raw() || metadata.mode()&0o077!=0 {
            return Err(error("the retained process namespace is not owner-only"));
        }
    }
    let file=File::open(&path)?;
    if platform::durable::FileId::of(&file).map_err(error)?!=platform::durable::FileId::at(&path).map_err(error)? {
        return Err(error("the retained process namespace changed"));
    }
    let mut value=String::new();file.take(18).read_to_string(&mut value)?;
    let Some(value)=value.strip_suffix('\n').filter(|value|value.len()==16&&value.bytes().all(|byte|byte.is_ascii_digit()||(b'a'..=b'f').contains(&byte))) else {
        return Err(error("the retained process namespace has an unknown format"));
    };
    Ok(Some(value.to_owned()))
}

/// Root calls this under its exclusive transition while work_view still exists.
/// A prior Calcifer's helpers are native scope members even after ACTIVE is lost.
pub(crate) fn retire(root:&Path)->Result<(),api::error::Error> {
    use api::error::{Error,Kind};
    if active(root) {return Err(Error::new(Kind::Conflict,"project_busy","owned controller tools are still retiring"));}
    let pending=||Error::new(Kind::Conflict,"project_tool_retirement_pending","the project's retained controller tool custody is not yet verified");
    let view=crate::project::vault::resolve_path(root)?;
    let Some(namespace)=namespace(&view,false).map_err(|_|pending())? else {
        match fs::read_dir(view.join(".vault-calcifer/processes")) {
            Ok(mut entries)=>{if entries.next().is_some() {return Err(pending());}return Ok(());},
            Err(why) if why.kind()!=io::ErrorKind::NotFound=>return Err(pending()),
            _=>return Ok(()),
        }
    };
    platform::contain::Scope::sweep(&namespace).map_err(|_|pending())?;
    if platform::contain::Scope::namespace_processes(&namespace).map_err(|_|pending())?!=0 {return Err(pending());}
    Ok(())
}

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
    let namespace=namespace(&root,true)?.ok_or_else(||error("the process namespace is unavailable"))?;
    let temporary=tempfile::Builder::new().prefix("tool-").tempdir_in(scratch.join("processes"))?;
    let input_path=temporary.path().join("stdin"); fs::write(&input_path,[])?;
    let stdin=File::open(input_path)?;
    let stdout=tempfile::NamedTempFile::new_in(temporary.path())?;
    let stderr=tempfile::NamedTempFile::new_in(temporary.path())?;
    let project_handle=File::open(&root)?;
    for file in [&stdin,stdout.as_file(),stderr.as_file()] {
        platform::private_exec::validate_project_descriptor(&project_handle,file).map_err(error)?;
    }
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
    // These are direct tools, without the worker's cgroup trampoline. Give
    // Linux descendants their ownership token before their very first exec.
    #[cfg(target_os="linux")]
    let process_scope=platform::contain::Scope::inherited(&namespace).map_err(error)?;
    #[cfg(not(target_os="linux"))]
    let process_scope=platform::contain::Scope::create(&namespace).map_err(error)?;
    if let Some((name,value))=process_scope.environment() {owned.env(name,value);}
    // Only 0/1/2 cross exec, each backed by the verified project files above.
    // The native helper suppresses requested cores and unrelated inherited FDs;
    // it makes no OS filesystem or arbitrary custom-Python sandbox claim.
    platform::private_exec::prepare_managed_child(&mut owned,Vec::new()).map_err(error)?;
    if running.cancelled.load(Ordering::Acquire) {
        process_scope.end().map_err(error)?;
        return Err(io::Error::new(io::ErrorKind::Interrupted,"project tool cancelled for lock"));
    }
    let mut tree=platform::process::spawn(owned).map_err(error)?;
    running.release=false;
    #[cfg(not(target_os="linux"))]
    process_scope.adopt(tree.pid()).map_err(error)?;
    let birth=platform::process::Birth::of(tree.pid()).map_err(error)?;
    // Preserve exact process births while the leader is alive. Reap only after
    // its group is ended; a reused PID can never authorize another process kill.
    let mut followers=HashMap::new();
    while !birth.ended().map_err(error)? {
        for member in platform::process::group_members(&birth) {
            if let Some(exact)=platform::process::Exact::open(&member).map_err(error)? { followers.entry(member).or_insert(exact); }
        }
        if running.cancelled.load(Ordering::Acquire) {break;}
        std::thread::sleep(Duration::from_millis(20));
    }
    for member in platform::process::group_members(&birth) {
        if let Some(exact)=platform::process::Exact::open(&member).map_err(error)? { followers.entry(member).or_insert(exact); }
    }
    let closed=(|| {
        tree.kill().map_err(error)?;
        process_scope.end().map_err(error)?;
        if !platform::private_exec::project_scope_quiescent(&process_scope).map_err(error)? {
            return Err(error("tool descendants still hold the project"));
        }
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
    drop(tree); drop(stdout); drop(stderr); temporary.close()?;
    let cancelled=running.cancelled.load(Ordering::Acquire);
    running.release=true;
    drop(running);
    if cancelled {Err(io::Error::new(io::ErrorKind::Interrupted,"project tool cancelled for lock"))} else {closed}
}
