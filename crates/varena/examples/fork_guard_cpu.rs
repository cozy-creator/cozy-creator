//! Real CPU-only fork/initialization proof. No nonzero allocation or CUDA API is invoked.
#![allow(unsafe_code)]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod proof {
    use serde_json::{Value, json};
    #[cfg(feature = "kernel-fork-guard")]
    use std::sync::{Arc, Barrier};
    use std::{
        fs,
        io::Read,
        os::fd::FromRawFd,
        process::Command,
        time::{Duration, Instant},
    };
    use varena::{Error, working};
    const ALL_REJECTED: u32 = 63;
    const WORDS: usize = 4;

    fn inherited<T>(result: varena::Result<T>) -> bool {
        matches!(
            result,
            Err(Error::Invalid("CUDA residency cannot be used after fork"))
        )
    }
    fn rejection_mask() -> u32 {
        [
            inherited(working::bind_process()),
            inherited(working::alloc(0, 0, 0, None)),
            inherited(working::free(0, 0, 0)),
            inherited(working::record_stream_any(0, 0)),
            inherited(working::trim_all()),
            inherited(working::stats(0)),
        ]
        .into_iter()
        .enumerate()
        .fold(0, |mask, (i, ok)| mask | ((ok as u32) << i))
    }
    fn marker() -> u32 {
        #[cfg(feature = "kernel-fork-guard")]
        {
            varena::fork_guard::status().marker
        }
        #[cfg(not(feature = "kernel-fork-guard"))]
        {
            0
        }
    }
    fn authenticated() -> bool {
        #[cfg(feature = "kernel-fork-guard")]
        {
            varena::fork_guard::status().thread_authenticated
        }
        #[cfg(not(feature = "kernel-fork-guard"))]
        {
            false
        }
    }
    fn status_json() -> Value {
        #[cfg(feature = "kernel-fork-guard")]
        {
            let s = varena::fork_guard::status();
            json!({"backend":s.backend,"marker":s.marker,"thread_authenticated":s.thread_authenticated,
                "page_bytes":s.page_bytes,"failure_stage":s.failure_stage,"failure_errno":s.failure_errno})
        }
        #[cfg(not(feature = "kernel-fork-guard"))]
        {
            json!({"backend":"default_actual_pid"})
        }
    }
    fn emit_child(fd: i32, mask: u32) -> ! {
        // SAFETY: getpid has no pointers. Only fixed stack storage is used before raw exit.
        let pid = unsafe { libc::syscall(libc::SYS_getpid) as u32 };
        let words = [pid, mask, marker(), authenticated() as u32];
        let mut bytes = [0u8; WORDS * 4];
        for (at, word) in words.iter().enumerate() {
            bytes[at * 4..at * 4 + 4].copy_from_slice(&word.to_ne_bytes());
        }
        // SAFETY: one tiny record cannot fill the empty pipe; raw exit avoids copied owner drops.
        unsafe {
            libc::write(fd, bytes.as_ptr().cast(), bytes.len());
            libc::_exit(0);
        }
    }
    fn pipe() -> [i32; 2] {
        let mut fds = [-1; 2];
        // SAFETY: valid two-int output storage; each branch closes or owns its exact FDs.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: pre-resolve write before raw fork; a zero-byte write dereferences no buffer.
        assert_eq!(unsafe { libc::write(fds[1], std::ptr::null(), 0) }, 0);
        fds
    }
    fn finish(pid: i32, fds: [i32; 2]) -> [u32; WORDS] {
        // SAFETY: parent owns this write descriptor and no longer writes it.
        unsafe {
            libc::close(fds[1]);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut status = 0;
        loop {
            // SAFETY: exact sole child and valid status output.
            let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if result == pid {
                break;
            }
            assert_eq!(result, 0);
            if Instant::now() >= deadline {
                // SAFETY: this child has not exited/reaped; its PID cannot be reused.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    libc::waitpid(pid, &mut status, 0);
                }
                panic!("child exceeded the explicit five-second CPU proof bound");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "child status {status}"
        );
        let mut bytes = [0u8; WORDS * 4];
        // SAFETY: ownership of the parent's read descriptor transfers to File exactly once.
        let mut read = unsafe { fs::File::from_raw_fd(fds[0]) };
        read.read_exact(&mut bytes).unwrap();
        std::array::from_fn(|i| u32::from_ne_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()))
    }
    fn fork_case(raw: bool, descendant: bool) -> Value {
        let fds = pipe();
        let parent = std::process::id();
        // SAFETY: child performs only CPU rejection checks, fixed pipe write and raw exit.
        // SYS_fork intentionally bypasses libc handlers; no CUDA is initialized.
        let pid = unsafe {
            if raw {
                libc::syscall(libc::SYS_fork) as i32
            } else {
                libc::fork()
            }
        };
        assert!(pid >= 0);
        if pid == 0 {
            // SAFETY: child owns its descriptor copy.
            unsafe {
                libc::close(fds[0]);
            }
            if descendant {
                // SAFETY: same fixed child path, no libc handlers.
                let next = unsafe { libc::syscall(libc::SYS_fork) as i32 };
                if next == 0 {
                    emit_child(fds[1], rejection_mask());
                }
                if next < 0 {
                    // SAFETY: immediate raw exit on fork failure.
                    unsafe {
                        libc::_exit(2);
                    }
                }
                let mut status = 0;
                // SAFETY: exact sole child; wait before this intermediate child exits.
                unsafe {
                    libc::waitpid(next, &mut status, 0);
                    libc::_exit(if libc::WIFEXITED(status) {
                        libc::WEXITSTATUS(status)
                    } else {
                        3
                    });
                }
            }
            emit_child(fds[1], rejection_mask());
        }
        let row = finish(pid, fds);
        assert_ne!(row[0], parent);
        assert_eq!(row[1], ALL_REJECTED, "{row:?}");
        assert_eq!(row[2], 0);
        json!({"raw_sys_fork":raw,"descendant":descendant,"rejection_mask":row[1],"marker":row[2],"child_pid":row[0]})
    }
    fn healthy() {
        working::bind_process().unwrap();
        let s = working::stats(0).unwrap();
        assert_eq!(s.allocated_bytes, 0);
        assert_eq!(s.mapped_bytes, 0);
        assert_eq!(s.pending_bytes, 0);
        assert_eq!(s.pools, 0);
        assert_eq!(s.event_handles, 0);
        assert_eq!(s.driver_memory_calls, 0);
        assert!(
            !fs::read_to_string("/proc/self/maps")
                .unwrap()
                .contains("libcuda.so")
        );
    }
    fn protection() -> String {
        #[cfg(feature = "kernel-fork-guard")]
        {
            let s = varena::fork_guard::status();
            if s.page_address == 0 {
                return s.backend.into();
            }
            let maps = fs::read_to_string("/proc/self/maps").unwrap();
            for line in maps.lines() {
                let mut f = line.split_whitespace();
                let (lo, hi) = f.next().unwrap().split_once('-').unwrap();
                let (lo, hi) = (
                    usize::from_str_radix(lo, 16).unwrap(),
                    usize::from_str_radix(hi, 16).unwrap(),
                );
                if lo <= s.page_address && hi >= s.page_address + s.page_bytes {
                    let flags = f.next().unwrap();
                    assert_eq!(flags, "r--p");
                    return flags.into();
                }
            }
            panic!("published read-only VMA absent");
        }
        #[cfg(not(feature = "kernel-fork-guard"))]
        {
            "default_actual_pid".into()
        }
    }
    #[repr(C)]
    struct Shared {
        fd: i32,
    }
    extern "C" fn shared_child(argument: *mut libc::c_void) -> i32 {
        // SAFETY: CLONE_VFORK suspends the parent; Shared stays live/read-only. The child has a
        // separate stack but deliberately copied TLS, and exits without Rust owner destruction.
        let fd = unsafe { (*argument.cast::<Shared>()).fd };
        emit_child(fd, rejection_mask())
    }
    fn shared_vm_counterexample() -> Value {
        let fds = pipe();
        let parent = std::process::id();
        let mut stack = vec![0u8; 1 << 20];
        let mut shared = Shared { fd: fds[1] };
        // SAFETY: Linux clone, not a POSIX vfork-library-call test. Separate child stack and
        // suspended sole parent prevent concurrent TLS/owner access. Only zero-sized checks run.
        let child = unsafe {
            libc::clone(
                shared_child,
                stack.as_mut_ptr().add(stack.len()).cast(),
                libc::CLONE_VM | libc::CLONE_VFORK | libc::SIGCHLD,
                (&mut shared as *mut Shared).cast(),
            )
        };
        assert!(child >= 0);
        let row = finish(child, fds);
        assert_ne!(row[0], parent);
        let armed = protection() == "r--p";
        assert_eq!(row[1], if armed { 0 } else { ALL_REJECTED }, "{row:?}");
        json!({"separate_tgid":true,"copied_tls":true,"shared_address_space":true,
        "accepted":row[1]==0,"counterexample_demonstrated":armed,"qualified":false})
    }
    fn run(mode: &str) -> Value {
        assert!(
            !fs::read_to_string("/proc/self/maps")
                .unwrap()
                .contains("libcuda.so")
        );
        #[cfg(feature = "kernel-fork-guard")]
        if mode == "unpublished" {
            let prepared = varena::fork_guard::Prepared::new().unwrap();
            assert_eq!(varena::fork_guard::status().backend, "unarmed_pid");
            let before = std::thread::spawn(|| fork_case(true, false))
                .join()
                .unwrap();
            assert!(prepared.publish().unwrap());
            healthy();
            return json!({"passed":true,"case":mode,"before_publication":before,"after_publication":fork_case(true,false),"protection":protection(),"cuda_initialized":false});
        }
        #[cfg(feature = "kernel-fork-guard")]
        if mode == "concurrent" {
            let ready = Arc::new(Barrier::new(5));
            let publish = Arc::new(Barrier::new(5));
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let ready = ready.clone();
                    let publish = publish.clone();
                    std::thread::spawn(move || {
                        let page = varena::fork_guard::Prepared::new().unwrap();
                        ready.wait();
                        publish.wait();
                        page.publish().unwrap()
                    })
                })
                .collect();
            ready.wait();
            let before = fork_case(true, false);
            publish.wait();
            let winners = threads
                .into_iter()
                .map(|t| usize::from(t.join().unwrap()))
                .sum::<usize>();
            assert_eq!(winners, 1);
            healthy();
            return json!({"passed":true,"case":mode,"publication_winners":winners,"unpublished_fork":before,"after_publication":fork_case(true,false),"protection":protection(),"cuda_initialized":false});
        }
        if mode == "fallback" {
            let mut old = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: read this process's own limits into valid stack storage.
            assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut old) }, 0);
            let limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: old.rlim_max,
            };
            // SAFETY: only this process's soft limit changes; direct bind allocates no Rust heap.
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) }, 0);
            let bound = working::bind_process();
            // SAFETY: immediately restore original limits before formatting or allocating.
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_AS, &old) }, 0);
            bound.unwrap();
            #[cfg(feature = "kernel-fork-guard")]
            {
                let s = varena::fork_guard::status();
                assert_eq!(s.backend, "fallback_pid");
                assert_eq!(s.failure_errno, libc::ENOMEM);
                assert_eq!(s.failure_stage, varena::fork_guard::Stage::Map as u32);
            }
        } else {
            working::bind_process().unwrap();
        }
        healthy();
        let ordinary = fork_case(false, false);
        let raw = fork_case(true, false);
        let descendant = fork_case(true, true);
        let thread = std::thread::spawn(|| {
            let before = authenticated();
            healthy();
            let after = authenticated();
            (before, after, fork_case(true, false))
        })
        .join()
        .unwrap();
        #[cfg(feature = "kernel-fork-guard")]
        if mode != "fallback" {
            assert!(!thread.0 && thread.1);
        }
        let unauthed = std::thread::spawn(|| fork_case(true, false))
            .join()
            .unwrap();
        let counterexample = shared_vm_counterexample();
        healthy();
        json!({"passed":true,"case":mode,"guard":status_json(),"protection":protection(),"ordinary":ordinary,"raw":raw,"descendant":descendant,
        "thread_auth_before":thread.0,"thread_auth_after":thread.1,"thread_fork":thread.2,"unauthenticated_thread_fork":unauthed,
        "shared_vm":counterexample,"cuda_initialized":false,"production_qualified":false})
    }
    pub(super) fn main() {
        let mode = std::env::args().nth(1).unwrap_or_else(|| "matrix".into());
        if mode != "matrix" {
            println!("{}", run(&mode));
            return;
        }
        let mut cases = vec!["armed", "fallback"];
        if cfg!(feature = "kernel-fork-guard") {
            cases.extend(["unpublished", "concurrent"]);
        }
        let mut rows = vec![];
        for mode in cases {
            let out = Command::new(std::env::current_exe().unwrap())
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{mode}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            rows.push(serde_json::from_slice::<Value>(&out.stdout).unwrap());
        }
        println!(
            "{}",
            json!({"passed":true,"experimental_feature":cfg!(feature="kernel-fork-guard"),"production_qualified":false,"cuda_initialized":false,"cases":rows})
        );
    }
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() {
    proof::main();
}
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() {
    eprintln!("CPU fork proof is currently Linux x86_64 only; no qualification");
    std::process::exit(77);
}
