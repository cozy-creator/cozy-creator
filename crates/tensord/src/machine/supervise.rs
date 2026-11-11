//! The machine's stable parent, forked before any thread exists. It reaps orphans (a pod's
//! PID 1 must), forwards stop signals and starts the service again after an abnormal exit, as
//! the Go guardian restarts its request plane. A clean exit (a stop or an accepted release)
//! ends it, and so does a second consecutive exit before readiness: measured progress, no timer.
//! The service is this executable, forked; an activated Runtime update is a new service on the
//! activated SDK. An activated binary (`super::binary`) is exec'd in place, same PID and arguments,
//! and its own parent forks the service. A candidate that never proves readiness is rolled back by
//! the parent then running: a binary by renaming the previous one back and exec'ing it.
//!
//! Windows forks nothing: the stable parent starts the service from the launch path, with its
//! arguments. A private local pipe identifies the service and carries its readiness report. An
//! activated or restored binary is simply the next service it starts. The service's Job Object
//! ends its tree with the parent; Windows has no orphans to reap.
#[cfg(unix)]
use platform::process::{SupervisorSignals, Woken};
#[cfg(unix)]
use rustix::{
    io::Errno,
    process::{getpid, kill_process, wait, Pid, WaitOptions, WaitStatus},
};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::io::{self, Read, Write};

#[cfg(unix)]
use std::fs::File;
#[cfg(windows)]
use platform::{ipc::{LocalListener, LocalStream}, process::{Birth, Exact}};

use super::{provider::ProviderSelf, update::Recovered};

/// A child connects to its parent's private pipe. A new parent listens before starting
/// the service and retains the existing restart and rollback behavior.
#[cfg(windows)]
pub(crate) fn supervise(
    paths: &super::update::Paths,
    _provider: Option<ProviderSelf>,
) -> io::Result<Ready> {
    let parent = platform::process::parent_of(std::process::id())?;
    match LocalStream::connect(&service_pipe(paths, parent)).map_err(io::Error::from) {
        Ok(pipe) => {
            if platform::handles::peer(&pipe)?.pid != parent {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "readiness pipe belongs to another process"));
            }
            return Ok(Ready(Some(pipe)));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = LocalListener::bind(&service_pipe(paths, std::process::id()))?;
    let program = std::env::current_exe()?;
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut failures_before_ready = 0;
    loop {
        // A published binary activation needs nothing more here: the service starts from the
        // launch path, which holds it.
        super::update::recover_activation(paths)?;
        let mut command = std::process::Command::new(&program);
        command.args(&arguments);
        let mut service = platform::process::spawn(command)?;
        let mut pipe = match Exact::open(&Birth::of(service.pid())?)? {
            Some(process) => loop {
                let Some(pipe) = listener.accept_unless(&process)? else { break None };
                if platform::handles::peer(&pipe)?.pid == service.pid() {
                    break Some(pipe);
                }
            },
            None => None,
        };
        let code = service.wait()?.code().unwrap_or(1);
        // What the service left running ends with it.
        drop(service);
        let ready = pipe.as_mut().is_some_and(|pipe| pipe.read(&mut [0u8; 1]).unwrap_or(0) == 1);
        if code == 0 {
            std::process::exit(0);
        }
        if code == super::update::REPLACE_EXIT || code == super::binary::EXEC_EXIT {
            tracing::info!("starting the service on its updated software");
            failures_before_ready = 0;
            continue;
        }
        failures_before_ready = if ready { 0 } else { failures_before_ready + 1 };
        if failures_before_ready >= 2 {
            let said = super::update::service_error(paths).map(|line| format!(": {line}")).unwrap_or_default();
            let cause = format!("the service exited twice before readiness ({code}){said}");
            if let Recovered::Nothing = super::update::rollback_pending(paths, &cause)? {
                tracing::error!("{cause}; stopping");
                std::process::exit(code);
            }
            tracing::warn!("{cause}; update rolled back");
            failures_before_ready = 0;
            continue;
        }
        tracing::warn!("the service exited ({code}); starting it again");
    }
}

#[cfg(windows)]
fn service_pipe(paths: &super::update::Paths, parent: u32) -> std::path::PathBuf {
    paths.update(&format!("service-{parent}.pipe"))
}

/// Forks the service. Returns only in the service child, with the pipe it reports readiness
/// on; the parent exits with the service's final status, or execs an activated binary, handing
/// on a rental's `provider` credential.
#[cfg(unix)]
pub(crate) fn supervise(paths: &super::update::Paths, provider: Option<ProviderSelf>) -> io::Result<Ready> {
    let signals = SupervisorSignals::new();
    platform::process::keep_sigchld()?;
    signals.block()?;
    // Orphans of the service reparent here, not to a PID 1 that may not reap them. Elsewhere
    // (macOS) they reparent to launchd, which reaps them, and executor trees end by group.
    match platform::process::become_subreaper() {
        Err(error) if error.is_unsupported() => {}
        other => other?,
    }
    let parent = getpid();
    let mut failures_before_ready = 0;
    loop {
        if let Recovered::Exec(launch) = super::update::recover_activation(paths)? {
            exec(paths, &launch, provider.as_ref(), &signals)?;
        }
        let (read, write) = cloexec_pipe()?;
        // Refused unless this process still has one thread: the child continues with no lock
        // another thread held.
        let child = match platform::process::fork()? {
            platform::process::Forked::Child => {
                drop(read);
                // The service never outlives its supervisor.
                if !platform::process::die_with_parent(parent.as_raw_pid() as u32)? {
                    std::process::exit(1);
                }
                platform::process::unblock_signals()?;
                return Ok(Ready(Some(File::from(write))));
            }
            platform::process::Forked::Parent(child) => {
                Pid::from_raw(child as i32).ok_or_else(|| io::Error::other("fork answered no child"))?
            }
        };
        drop(write);
        let (status, stopping) = wait_service(child, &signals)?;
        let mut byte = [0u8; 1];
        let ready = File::from(read).read(&mut byte).unwrap_or(0) == 1;
        let code = (status.exit_status())
            .or(status.terminating_signal().map(|signal| 128 + signal))
            .unwrap_or(1);
        if stopping || code == 0 {
            std::process::exit(code);
        }
        // An activated binary is exec'd as the loop recovers its published activation.
        if code == super::update::REPLACE_EXIT || code == super::binary::EXEC_EXIT {
            tracing::info!("starting the service on its updated software");
            failures_before_ready = 0;
            continue;
        }
        failures_before_ready = if ready { 0 } else { failures_before_ready + 1 };
        if failures_before_ready >= 2 {
            let said = super::update::service_error(paths).map(|line| format!(": {line}")).unwrap_or_default();
            let cause = format!("the service exited twice before readiness ({code}){said}");
            match super::update::rollback_pending(paths, &cause)? {
                Recovered::Nothing => {
                    tracing::error!("{cause}; stopping");
                    std::process::exit(code);
                }
                Recovered::Restored => tracing::warn!("{cause}; update rolled back"),
                Recovered::Exec(launch) => {
                    tracing::warn!("{cause}; update rolled back to the previous binary");
                    exec(paths, &launch, provider.as_ref(), &signals)?;
                }
            }
            failures_before_ready = 0;
            continue;
        }
        tracing::warn!("the service exited ({code}); starting it again");
    }
}

/// Replaces this parent with the binary at `launch`. One that will not exec is rolled back, and
/// this image serves on.
#[cfg(unix)]
fn exec(paths: &super::update::Paths, launch: &std::path::Path, provider: Option<&ProviderSelf>, signals: &SupervisorSignals) -> io::Result<()> {
    tracing::info!("running {}", launch.display());
    let error = super::binary::exec(launch, provider);
    signals.block()?;
    let cause = format!("cannot run {}: {error}", launch.display());
    tracing::error!("{cause}");
    super::update::rollback_pending(paths, &cause).map(drop)
}

/// Places `fd` at `at` for the re-executed machine to inherit.
#[cfg(target_os = "linux")]
pub(super) fn inherit(fd: std::os::fd::OwnedFd, at: i32) -> bool {
    let placed = platform::process::hand_on_exec(&fd, at).is_ok();
    if fd.as_raw_fd() == at {
        std::mem::forget(fd);
    }
    placed
}

/// Waits for the service to end, reaping every orphan meanwhile and forwarding stop signals.
#[cfg(unix)]
fn wait_service(child: Pid, signals: &SupervisorSignals) -> io::Result<(WaitStatus, bool)> {
    let mut stopping = false;
    loop {
        match signals.wait()? {
            Woken::Stop(signal) => {
                stopping = true;
                let _ = kill_process(child, signal);
            }
            Woken::Child => loop {
                // Any child: `waitpid(None, ..)` would take only this process's group.
                match wait(WaitOptions::NOHANG) {
                    Ok(None) | Err(Errno::CHILD) => break,
                    Ok(Some((pid, status))) if pid == child => return Ok((status, stopping)),
                    Ok(Some(_)) | Err(Errno::INTR) => continue,
                    Err(error) => return Err(error.into()),
                }
            },
        }
    }
}

#[cfg(unix)]
type ReadyPipe = File;
#[cfg(windows)]
type ReadyPipe = LocalStream;

/// The service's readiness report to its parent, written once this boot is ready.
pub(crate) struct Ready(Option<ReadyPipe>);
impl Ready {
    pub(crate) fn report(&mut self) {
        if let Some(mut pipe) = self.0.take() {
            let _ = pipe.write_all(b"r");
        }
    }
}

/// A pipe whose ends close on exec (std makes it so on every Unix).
#[cfg(unix)]
pub(crate) fn cloexec_pipe() -> io::Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    let (read, write) = std::io::pipe()?;
    Ok((read.into(), write.into()))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{path::Path, process::{Command, Stdio}};

    fn command(root: &Path, case: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "machine::supervise::tests::service_fixture", "--nocapture"])
            .env("COZY_SUPERVISOR_TEST_ROOT", root)
            .env("COZY_SUPERVISOR_TEST_CASE", case);
        command
    }

    fn run_case(case: &str, exit: i32, starts: u32) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let output = command(root.path(), case).output().unwrap();
        assert_eq!(output.status.code(), Some(exit), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(std::fs::read_to_string(root.path().join("starts")).unwrap(), starts.to_string());
        root
    }

    #[test]
    fn readiness_survives_service_exit_and_resets_early_failures() {
        run_case("ready-restart", 0, 3);
    }

    #[test]
    fn two_early_exits_stop_the_supervisor() {
        run_case("early-exit", 7, 2);
    }

    #[test]
    fn requested_restart_uses_the_same_private_handoff() {
        run_case("updated", 0, 2);
    }

    #[test]
    fn two_early_exits_roll_back_pending_activation() {
        let root = run_case("rollback", 0, 3);
        let status: super::super::update::Status = serde_json::from_slice(
            &std::fs::read(root.path().join("engine/update/status.json")).unwrap(),
        ).unwrap();
        assert_eq!(status.state, "rolled_back");
        assert!(!root.path().join("engine/update/pending.json").exists());
    }

    #[test]
    fn parent_exit_ends_the_service_job() {
        let root = tempfile::tempdir().unwrap();
        let mut parent = command(root.path(), "parent-exit").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let file = root.path().join("service-pid");
        let pid: u32 = loop {
            if let Ok(text) = std::fs::read_to_string(&file) {
                if let Ok(pid) = text.parse() {
                    break pid;
                }
            }
            assert!(parent.try_wait().unwrap().is_none(), "supervisor exited before its service was ready");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let service = Exact::open(&Birth::of(pid).unwrap()).unwrap().unwrap();
        parent.kill().unwrap();
        parent.wait().unwrap();
        service.wait().unwrap();
        assert!(service.ended());
    }

    #[test]
    fn service_fixture() {
        let Some(root) = std::env::var_os("COZY_SUPERVISOR_TEST_ROOT").map(std::path::PathBuf::from) else { return };
        let case = std::env::var("COZY_SUPERVISOR_TEST_CASE").unwrap();
        let paths = super::super::update::Paths::new(&root.join("engine"), &root);
        std::fs::create_dir_all(paths.update("")).unwrap();
        let mut ready = supervise(&paths, None).unwrap();
        let starts = root.join("starts");
        let count = std::fs::read_to_string(&starts).ok().and_then(|text| text.parse::<u32>().ok()).unwrap_or(0) + 1;
        std::fs::write(&starts, count.to_string()).unwrap();
        match case.as_str() {
            "early-exit" => std::process::exit(7),
            "ready-restart" => {
                ready.report();
                std::process::exit(if count < 3 { 7 } else { 0 });
            }
            "updated" => {
                ready.report();
                std::process::exit(if count == 1 { super::super::update::REPLACE_EXIT } else { 0 });
            }
            "rollback" if count < 3 => {
                if count == 1 {
                    let status = super::super::update::Status { state: "starting".into(), ..Default::default() };
                    let pending = serde_json::json!({"status": status, "sdk_before": null, "binary": null});
                    std::fs::write(paths.update("pending.json"), serde_json::to_vec(&pending).unwrap()).unwrap();
                }
                std::process::exit(7);
            }
            "rollback" => {
                ready.report();
                std::process::exit(0);
            }
            "parent-exit" => {
                ready.report();
                std::fs::write(root.join("service-pid"), std::process::id().to_string()).unwrap();
                loop { std::thread::park(); }
            }
            _ => panic!("unknown supervisor fixture {case}"),
        }
    }
}
