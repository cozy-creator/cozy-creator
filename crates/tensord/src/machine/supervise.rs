//! The machine's stable parent, forked before any thread exists. It reaps orphans (a pod's
//! PID 1 must), forwards stop signals and starts the service again after an abnormal exit, as
//! the Go guardian restarts its request plane. A clean exit (a stop or an accepted release)
//! ends it, and so does a second consecutive exit before readiness: measured progress, no timer.
//! The service is this executable, forked; an activated Runtime update is a new service on the
//! activated SDK. An activated binary (`super::binary`) is exec'd in place, same PID and arguments,
//! and its own parent forks the service. A candidate that never proves readiness is rolled back by
//! the parent then running: a binary by renaming the previous one back and exec'ing it.
//!
//! A service that says it is leaving (`leave`) or is told to stop has `stuck::GRACE_S` to exit, then is killed; one still there
//! with no progress `stuck::STALL_S` later is in the kernel (cl-977: an RC follow's restart waited forever on a TensorD whose last
//! thread sat in the NVIDIA driver). It is recorded (`stuck`) and the next service starts beside it: this parent holds the
//! listeners (`net::Listeners`), so a port it still holds is the next one's too.
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
    process::{getpid, kill_process, wait, Pid, Signal, WaitOptions, WaitStatus},
};
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::io::{self, Read, Write};

#[cfg(unix)]
use std::fs::File;
#[cfg(windows)]
use platform::{ipc::{LocalListener, LocalStream}, process::{Birth, Exact}};

use super::{net::Listeners, provider::ProviderSelf, stuck::Stuck, update::Recovered};

/// The write end of the readiness pipe, kept in the service for its word that it is leaving.
#[cfg(unix)]
static PARENT: OnceLock<File> = OnceLock::new();

/// The service's exit: its parent hears the code first, so it waits for the exit only so long (cl-977).
pub(crate) fn leave(code: i32) -> ! {
    #[cfg(unix)]
    if let Some(mut pipe) = PARENT.get() {
        if pipe.write_all(&[b'x', code as u8]).is_ok() {
            platform::process::leaving();
        }
    }
    std::process::exit(code)
}

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
            let mut pipe = pipe;
            let mut acknowledged = [0];
            pipe.read_exact(&mut acknowledged)?;
            if acknowledged != [b'a'] {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid supervisor acknowledgment"));
            }
            return Ok(Ready(Some(pipe)));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let program = std::env::current_exe()?;
    let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut failures_before_ready = 0;
    loop {
        // A published binary activation needs nothing more here: the service starts from the
        // launch path, which holds it.
        super::update::recover_activation(paths)?;
        let listener = LocalListener::bind(&service_pipe(paths, std::process::id()))?;
        let mut command = std::process::Command::new(&program);
        command.args(&arguments);
        let mut service = platform::process::spawn(command)?;
        #[cfg(test)]
        if tests::wait_before_open() {
            service.wait()?;
        }
        let mut pipe = match Exact::open(&Birth::of(service.pid())?)? {
            Some(process) => loop {
                let Some(pipe) = listener.accept_unless(&process)? else { break None };
                match platform::handles::peer(&pipe) {
                    Ok(peer) if peer.pid == service.pid() => {
                        let mut pipe = pipe;
                        match pipe.write_all(b"a") {
                            Ok(()) => break Some(pipe),
                            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                            Err(error) => return Err(error),
                        }
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(error) => return Err(error),
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
/// on a rental's `provider` credential and the `listeners`.
#[cfg(unix)]
pub(crate) fn supervise(paths: &super::update::Paths, provider: Option<ProviderSelf>, listeners: &Listeners) -> io::Result<Ready> {
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
            exec(paths, &launch, provider.as_ref(), listeners, &signals)?;
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
                let pipe = File::from(write);
                let _ = PARENT.set(pipe.try_clone()?);
                return Ok(Ready(Some(pipe)));
            }
            platform::process::Forked::Parent(child) => {
                Pid::from_raw(child as i32).ok_or_else(|| io::Error::other("fork answered no child"))?
            }
        };
        drop(write);
        let mut said = Said::new(File::from(read))?;
        let (ended, stopping) = wait_service(child, &signals, &mut said)?;
        said.read();
        let code = match ended {
            Ended::Reaped(status) => (status.exit_status()).or(status.terminating_signal().map(|signal| 128 + signal)).unwrap_or(1),
            // It stays a child here, reaped whenever the kernel lets it exit; the next service takes over beside it.
            Ended::Stuck(stuck) => {
                let code = stuck.code.unwrap_or(128 + libc::SIGKILL);
                tracing::error!("{}, {} s after its kill; it stays until the kernel lets it go, and the next service starts", stuck.line(),
                    (now_ms() - stuck.killed_unix_ms) / 1000);
                if let Err(error) = super::stuck::record(paths, stuck) {
                    tracing::warn!("recording the stuck service: {error}");
                }
                code
            }
        };
        let ready = said.ready();
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
                    exec(paths, &launch, provider.as_ref(), listeners, &signals)?;
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
fn exec(paths: &super::update::Paths, launch: &std::path::Path, provider: Option<&ProviderSelf>, listeners: &Listeners, signals: &SupervisorSignals) -> io::Result<()> {
    tracing::info!("running {}", launch.display());
    let error = match listeners.hand_on() {
        true => super::binary::exec(launch, provider),
        false => io::Error::other("cannot hand on its listeners"),
    };
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

/// How the service ended: reaped, or stuck in the kernel past its kill.
#[cfg(unix)]
enum Ended {
    Reaped(WaitStatus),
    Stuck(Stuck),
}

/// What the service wrote on its readiness pipe: `r` once ready, `x<code>` as it leaves.
#[cfg(unix)]
struct Said {
    pipe: File,
    bytes: Vec<u8>,
}

#[cfg(unix)]
impl Said {
    fn new(pipe: File) -> io::Result<Self> {
        rustix::io::ioctl_fionbio(&pipe, true)?;
        Ok(Self { pipe, bytes: vec![] })
    }

    /// What it wrote so far, without waiting.
    fn read(&mut self) {
        let mut chunk = [0u8; 64];
        while let Ok(n @ 1..) = self.pipe.read(&mut chunk) {
            self.bytes.extend_from_slice(&chunk[..n]);
        }
    }

    fn ready(&self) -> bool {
        self.bytes.contains(&b'r')
    }

    fn code(&self) -> Option<i32> {
        let at = self.bytes.iter().position(|b| *b == b'x')?;
        self.bytes.get(at + 1).map(|code| i32::from(*code))
    }
}

/// Waits for the service to end, reaping every orphan meanwhile and forwarding stop signals. Once it says it is leaving or is
/// told to stop, it has `GRACE_S`; then it is killed, and a killed one that shows no progress for `STALL_S` is stuck.
#[cfg(unix)]
fn wait_service(child: Pid, signals: &SupervisorSignals, said: &mut Said) -> io::Result<(Ended, bool)> {
    use super::stuck::{GRACE_S, STALLS, STALL_S};
    let pid = child.as_raw_pid() as u32;
    let progress = || (platform::process::meter(pid), platform::process::threads(pid).len());
    let (mut stopping, mut leaving) = (false, false);
    // When it was killed, what it showed then, and the stalls waited since.
    let mut killed: Option<(i64, _, u32)> = None;
    loop {
        match signals.wait()? {
            Woken::Stop(signal) => {
                stopping = true;
                let _ = kill_process(child, signal);
                if !std::mem::replace(&mut leaving, true) {
                    signals.alarm(GRACE_S);
                }
            }
            // Its word is on its own pipe: a signal left from the service before it says nothing of this one.
            Woken::Leaving => {
                said.read();
                if said.code().is_some() && !std::mem::replace(&mut leaving, true) {
                    signals.alarm(GRACE_S);
                }
            }
            Woken::Alarm if leaving => match &mut killed {
                None => {
                    tracing::warn!("the service (pid {pid}) did not exit within {GRACE_S} s; killing it");
                    let _ = kill_process(child, Signal::KILL);
                    killed = Some((now_ms(), progress(), 0));
                    signals.alarm(STALL_S);
                }
                Some((at, seen, stalls)) => {
                    let now = progress();
                    *stalls += 1;
                    if now != *seen && *stalls < STALLS {
                        *seen = now;
                        signals.alarm(STALL_S);
                        continue;
                    }
                    let birth = platform::process::Birth::of(pid).map_err(io::Error::other)?;
                    let stuck = Stuck { birth, killed_unix_ms: *at, threads: platform::process::threads(pid), code: said.code() };
                    return Ok((Ended::Stuck(stuck), stopping));
                }
            },
            Woken::Alarm => {}
            Woken::Child => loop {
                // Any child: `waitpid(None, ..)` would take only this process's group.
                match wait(WaitOptions::NOHANG) {
                    Ok(None) | Err(Errno::CHILD) => break,
                    Ok(Some((reaped, status))) if reaped == child => {
                        signals.alarm(0);
                        if let Some((at, _, _)) = killed {
                            tracing::info!("the service (pid {pid}) exited {} s after its kill", (now_ms() - at) / 1000);
                        }
                        return Ok((Ended::Reaped(status), stopping));
                    }
                    Ok(Some(_)) | Err(Errno::INTR) => continue,
                    Err(error) => return Err(error.into()),
                }
            },
        }
    }
}

#[cfg(unix)]
fn now_ms() -> i64 {
    super::lifecycle::now_ms()
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

    // Force the first child to leave a closed pipe before its parent can open Exact.
    // This hook is absent from production builds.
    pub(super) fn wait_before_open() -> bool {
        use std::sync::atomic::{AtomicBool, Ordering};
        static FIRST: AtomicBool = AtomicBool::new(true);
        matches!(std::env::var("COZY_SUPERVISOR_TEST_CASE").as_deref(), Ok("exited-before-open"))
            && FIRST.swap(false, Ordering::Relaxed)
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
    fn a_child_that_connects_then_exits_is_counted_as_unready() {
        run_case("connect-exit", 7, 2);
    }

    #[test]
    fn a_child_exited_before_open_does_not_poison_its_successor() {
        run_case("exited-before-open", 0, 3);
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
        if case == "connect-exit" || (case == "exited-before-open" && !root.join("starts").exists()) {
            let parent = platform::process::parent_of(std::process::id()).unwrap();
            match LocalStream::connect(&service_pipe(&paths, parent)).map_err(io::Error::from) {
                Ok(pipe) => {
                    let starts = root.join("starts");
                    let count = std::fs::read_to_string(&starts).ok().and_then(|text| text.parse::<u32>().ok()).unwrap_or(0) + 1;
                    std::fs::write(starts, count.to_string()).unwrap();
                    drop(pipe);
                    std::process::exit(7);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("{error}"),
            }
        }
        let mut ready = supervise(&paths, None).unwrap();
        let starts = root.join("starts");
        let count = std::fs::read_to_string(&starts).ok().and_then(|text| text.parse::<u32>().ok()).unwrap_or(0) + 1;
        std::fs::write(&starts, count.to_string()).unwrap();
        match case.as_str() {
            "early-exit" => std::process::exit(7),
            "ready-restart" | "exited-before-open" => {
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
