//! Optional real native lifetime probes, separate from pressure/progress qualification.
use super::*;
use std::os::unix::fs::DirBuilderExt;

// Only constructed in the parent for its unreaped direct child. In particular,
// every error after fork closes the pipe and terminates/reaps this child only.
struct ForkChild {
    pid: i32,
    pidfd: i32,
    read: i32,
    reaped: bool,
}
impl Drop for ForkChild {
    fn drop(&mut self) {
        unsafe {
            if !self.reaped {
                if self.pidfd >= 0 {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        self.pidfd,
                        libc::SIGKILL,
                        std::ptr::null::<c_void>(),
                        0,
                    );
                } else {
                    // Still our unreaped child; its PID cannot have been reused.
                    libc::kill(self.pid, libc::SIGKILL);
                }
                while libc::waitpid(self.pid, std::ptr::null_mut(), 0) < 0
                    && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
                {
                }
            }
            libc::close(self.read);
            if self.pidfd >= 0 {
                libc::close(self.pidfd);
            }
        }
    }
}

pub(super) fn inherited(owner: &cooperative::Owner) -> Result<Value> {
    let before = owner.facts()?;
    if !before.active {
        return Err(io::Error::other("fork probe needs a live native owner").into());
    }
    let mut pipe = [-1; 2];
    // SAFETY: this fixture owns both pipe descriptors and its fork child exclusively.
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let parent = std::process::id() as i32;
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // No CUDA, heap formatting, stdio, or Rust teardown in the private-VM child.
        // Parent death must not leave a child blocked in a broken inherited guard.
        unsafe {
            libc::close(pipe[0]);
            let armed = libc::syscall(
                libc::SYS_prctl,
                libc::PR_SET_PDEATHSIG,
                libc::SIGKILL,
                0,
                0,
                0,
            );
            if armed != 0 || libc::getppid() != parent {
                libc::_exit(125);
            }
        }
        let facts = matches!(
            owner.facts(),
            Err(cooperative::Error::Ledger(ledger::Error::Inherited))
        );
        let close = matches!(
            owner.close(),
            Err(cooperative::Error::Ledger(ledger::Error::Inherited))
        );
        let marker = u8::from(facts) | (u8::from(close) << 1);
        unsafe {
            let written = libc::write(pipe[1], (&marker as *const u8).cast(), 1);
            libc::_exit(if marker == 3 && written == 1 { 0 } else { 2 });
        }
    }
    if pid < 0 {
        let error = io::Error::last_os_error();
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
        }
        return Err(error.into());
    }
    unsafe {
        libc::close(pipe[1]);
    }
    let mut child = ForkChild {
        pid,
        pidfd: -1,
        read: pipe[0],
        reaped: false,
    };
    child.pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
    if child.pidfd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let birth = std::fs::read_to_string(format!("/proc/{pid}/stat"))?
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("invalid child stat"))?
        .1
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| io::Error::other("missing child birth"))?
        .parse::<u64>()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut status = 0;
    let mut marker = 0u8;
    let mut observed = false;
    while Instant::now() < deadline {
        let mut poll = libc::pollfd {
            fd: pipe[0],
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 10) } > 0 && poll.revents & libc::POLLIN != 0 {
            observed |= unsafe { libc::read(pipe[0], (&mut marker as *mut u8).cast(), 1) } == 1;
        }
        if unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } == pid {
            child.reaped = true;
            // The child may have written between the poll and waitpid.
            if !observed {
                observed = unsafe { libc::read(pipe[0], (&mut marker as *mut u8).cast(), 1) } == 1;
            }
            break;
        }
    }
    let exited = child.reaped;
    if !exited
        || !observed
        || marker != 3
        || !libc::WIFEXITED(status)
        || libc::WEXITSTATUS(status) != 0
    {
        return Err(io::Error::other(format!(
            "inherited owner guard failed: exited={exited} marker={marker} status={status}"
        ))
        .into());
    }
    if !owner.facts()?.active {
        return Err(io::Error::other("child changed the parent owner").into());
    }
    Ok(
        json!({"child_pid":pid,"child_start_ticks":birth,"facts_inherited_rejected":true,
        "close_inherited_rejected":true,"child_exit":0,"parent_owner_active":true,
        "boundary":"actual post-CUDA fork; child uses exact Inherited guards, fixed pipe byte and _exit only"}),
    )
}

type StreamCreate = unsafe extern "C" fn(*mut Ctx, u32) -> i32;
type StreamOp = unsafe extern "C" fn(Ctx) -> i32;
type Begin = unsafe extern "C" fn(Ctx, i32) -> i32;
type End = unsafe extern "C" fn(Ctx, *mut Ctx) -> i32;

pub(super) fn capture_close(
    owner: &cooperative::Owner,
    cuda: &Cuda,
    directory: &std::path::Path,
    descriptor: ledger::Device,
    device: i32,
) -> Result<Value> {
    let path = directory.join("capture-counter");
    std::fs::DirBuilder::new().mode(0o700).create(&path)?;
    // Real counter owner, opened before capture and retired after every guard.
    // This helper creates no Arena weights: Peer.cuda_calls covers the global
    // product working-driver counter, not our fixture's CUDA stream/graph calls.
    let observer =
        cooperative::Peer::open(device, descriptor, &path, ledger::Sharing::Dedicated, 0)?;
    let (create, destroy_stream, begin, end, destroy_graph) = unsafe {
        (
            *cuda._library.get::<StreamCreate>(b"cuStreamCreate\0")?,
            *cuda._library.get::<StreamOp>(b"cuStreamDestroy_v2\0")?,
            // v1 has a different signature; no unversioned fallback is permitted.
            *cuda._library.get::<Begin>(b"cuStreamBeginCapture_v2\0")?,
            *cuda._library.get::<End>(b"cuStreamEndCapture\0")?,
            *cuda._library.get::<StreamOp>(b"cuGraphDestroy\0")?,
        )
    };
    cuda.synchronize()?;
    let mut stream = std::ptr::null_mut();
    check("cuStreamCreate", unsafe { create(&mut stream, 1) })?;
    let guard = match varena::enter_capture(device) {
        Ok(guard) => guard,
        Err(error) => {
            unsafe {
                destroy_stream(stream);
            }
            return Err(error.into());
        }
    };
    if let Err(error) = check("cuStreamBeginCapture_v2", unsafe { begin(stream, 0) }) {
        drop(guard);
        unsafe {
            destroy_stream(stream);
        }
        return Err(error);
    }
    let measurement = (|| -> Result<Value> {
        let before = observer.cuda_calls()?;
        let initial = working::stats(device)?;
        let refusal = owner.close();
        if !matches!(
            refusal,
            Err(cooperative::Error::Native(varena::Error::CaptureActive))
        ) {
            return Err(
                io::Error::other("owner close did not refuse during actual capture").into(),
            );
        }
        // A defined observation interval, not an inference of process death or release.
        std::thread::sleep(Duration::from_millis(600));
        let during = owner.facts()?;
        let after = observer.cuda_calls()?;
        let final_stats = working::stats(device)?;
        if before != after
            || during.active
            || !during.stopping
            || during.retired
            || initial.allocated_bytes != final_stats.allocated_bytes
            || initial.mapped_bytes != final_stats.mapped_bytes
        {
            return Err(io::Error::other(
                "capture-close boundary changed native driver/owner/storage state",
            )
            .into());
        }
        Ok(
            json!({"actual_cuda_capture":true,"close_refused":true,"active":during.active,
            "stopping":during.stopping,"retired_during_capture":during.retired,
            "working_driver_calls_before":before,"working_driver_calls_after":after,
            "product_working_driver_call_delta":0,"live_before":initial.allocated_bytes,
            "live_after":final_stats.allocated_bytes,"mapped_before":initial.mapped_bytes,
            "mapped_after":final_stats.mapped_bytes,"observation_ms":600}),
        )
    })();
    let mut graph = std::ptr::null_mut();
    let finished = unsafe { end(stream, &mut graph) };
    if finished != 0 {
        // An uncertain CUDA capture state must not unwind GPU-owning values.
        // This isolated helper exits without Rust teardown; the controller
        // retains this failure receipt and waits for its exact owned process.
        let _ = emit(json!({"event":"capture_end_failed","cuda_status":finished,
            "native_retirement_quarantined":true,"pid":std::process::id()}));
        unsafe {
            libc::_exit(3);
        }
    }
    // Actual CUDA capture ends before native retirement exclusion is released.
    drop(guard);
    let graph_cleanup = if graph.is_null() {
        0
    } else {
        unsafe { destroy_graph(graph) }
    };
    let stream_cleanup = unsafe { destroy_stream(stream) };
    check("cuGraphDestroy", graph_cleanup)?;
    check("cuStreamDestroy", stream_cleanup)?;
    let mut result = measurement?;
    owner.close()?;
    if !owner.facts()?.retired {
        return Err(io::Error::other("owner did not retire after capture").into());
    }
    result["retired_after_capture"] = json!(true);
    result["boundary"] = json!(
        "Owner close deferral and product working-driver calls only; no Arena weights, no last-Arena teardown or all-external-CUDA claim"
    );
    if let Err((observer, error)) = observer.try_close() {
        // Refusal transfers the complete owner back; never turn it into an
        // unguarded last-owner drop while reporting a failed fixture check.
        std::mem::forget(observer);
        return Err(error.into());
    }
    Ok(result)
}
