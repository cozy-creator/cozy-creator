//! Real-process CPU proof peer. Run through release-gate/suites/varena/ledger-cpu/prove.py.
//! No CUDA initialization, model, daemon or mocked kernel backend.
#![allow(unsafe_code)]

#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
mod proof {
    use serde_json::{Value, json};
    use std::fs::{self, OpenOptions};
    use std::io::{self, BufRead, Write};
    use std::os::fd::AsRawFd;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use varena::ledger::{Answer, Bytes, Device, Ledger, Participant, Physical, Sharing};

    fn emit(value: Value) {
        println!("{value}");
        io::stdout().flush().unwrap();
    }
    fn open(directory: &Path) -> varena::ledger::Result<Ledger> {
        Ledger::open(
            directory,
            Device::parse("GPU-13800000-0000-0000-0000-000000000003")?,
        )
    }
    fn snapshot(ledger: &Ledger) -> varena::ledger::Result<Value> {
        let snapshot = ledger.snapshot()?;
        let rows: Vec<_> = snapshot
            .rows
            .iter()
            .map(|row| {
                json!({
                    "slot": row.id.slot, "incarnation": row.id.incarnation, "pid": row.pid,
                    "start_ticks": row.start_ticks, "priority": row.priority,
                    "sharing": format!("{:?}", row.sharing), "want": row.want,
                    "live": row.physical.weights.live, "idle": row.physical.weights.idle,
                    "pending": row.physical.weights.pending, "reclaimable": row.reclaimable,
                })
            })
            .collect();
        let round = snapshot.round.as_ref().map(|round| json!({
        "id": round.generation, "sample": round.sample_generation, "requested": round.requested,
        "aborted": round.aborted, "finished": round.finished(), "completed": round.completed().unwrap(),
        "donors": round.donations.iter().map(|d| json!({"slot":d.owner.slot,"incarnation":d.owner.incarnation,
            "requested":d.requested,"completed":d.completed,"pending":d.pending,"answer":format!("{:?}",d.answer)})).collect::<Vec<_>>(),
    }));
        Ok(
            json!({"generation":snapshot.generation,"capacity_epoch":snapshot.capacity_epoch,
        "device":snapshot.backing_device,"inode":snapshot.backing_inode,
        "rows":rows,"round":round,"sample":snapshot.sample.map(|s|json!({"id":s.generation,
            "capacity_epoch":s.capacity_epoch,"used":s.driver_used,"total":s.total}))}),
        )
    }
    fn published(
        peer: &Participant,
        live: u64,
        idle: u64,
        pending: u64,
        reclaimable: u64,
        used: u64,
    ) -> varena::ledger::Result<()> {
        peer.publish(
            Physical {
                weights: Bytes {
                    live,
                    idle,
                    pending,
                },
                ..Physical::default()
            },
            reclaimable,
            used,
        )
    }
    fn command(ledger: &Ledger, peer: &Participant, line: &str) -> varena::ledger::Result<Value> {
        let parts: Vec<_> = line.split_whitespace().collect();
        let n = |at: usize| parts[at].parse::<u64>().unwrap();
        match parts.first().copied().unwrap_or("") {
            "configure" => {
                peer.configure(if parts[1] == "shared" { Sharing::Shared } else { Sharing::Dedicated }, parts[2].parse().unwrap())?;
                Ok(json!({"ok":true}))
            }
            "withdraw" => Ok(json!({"changed":peer.withdraw(n(1))?})),
            "clear-want" => { peer.clear_want()?; Ok(json!({"ok":true})) }
            "publish" => {
                published(peer, n(1), n(2), n(3), n(4), n(5))?;
                Ok(json!({"ok":true}))
            }
            "sample" => {
                // Coordinator sampling commits exact departures before its optimistic read.
                ledger.reap_dead()?;
                let expected = ledger.snapshot()?.generation;
                let s = peer.sample(expected, n(1), n(2), n(3), None, false)?;
                Ok(json!({"sample":s.generation}))
            }
            "sample-at" => {
                let s = peer.sample(n(1), n(2), n(3), n(4), None, false)?;
                Ok(json!({"sample":s.generation}))
            }
            "ask" => {
                let r = peer.ask(n(1), n(2))?;
                Ok(json!({"round":r.generation,"requested":r.requested,"donors":r.donations.len()}))
            }
            "ack" => {
                let answer = match parts[4] {
                    "pending" => Answer::Pending,
                    "complete" => Answer::Complete,
                    "refused" => Answer::Refused,
                    _ => panic!("unknown acknowledgement"),
                };
                peer.acknowledge(n(1), n(2), n(3), answer)?;
                Ok(json!({"ok":true}))
            }
            "snapshot" => snapshot(ledger),
            "exec-check" => {
                let child = std::process::Command::new(std::env::current_exe()?)
                    .arg("inspect")
                    .arg(ledger.path().parent().unwrap())
                    .output()?;
                assert!(
                    child.status.success(),
                    "{}",
                    String::from_utf8_lossy(&child.stderr)
                );
                let observed: Value = serde_json::from_slice(&child.stdout).unwrap();
                assert_eq!(observed["ledger_descriptions"], 1);
                Ok(json!({"close_on_exec":true,"child_descriptions":1}))
            }
            "descriptor-limit" => {
                let directory = ledger.path().parent().unwrap();
                let mut opened = Vec::new();
                loop {
                    match open(directory) {
                        Ok(connection) => opened.push(connection),
                        Err(varena::ledger::Error::Full("process ledger descriptor registry")) => {
                            break;
                        }
                        Err(error) => return Err(error),
                    }
                }
                let count = opened.len();
                assert_eq!(
                    count, 254,
                    "the participant owns two of the 256 bounded descriptions"
                );
                drop(opened);
                let recovered = open(directory)?;
                assert!(
                    recovered
                        .snapshot()?
                        .rows
                        .iter()
                        .any(|row| row.id == peer.id())
                );
                Ok(json!({"bounded":256,"extra_connections":count,"recovered":true}))
            }
            "threads" => {
                published(peer, 1, 8, 0, 8, 1)?;
                std::thread::scope(|scope| {
                    for lane in 0..2 {
                        scope.spawn(move || {
                            for index in 1..33 {
                                let value = lane * 1000 + index;
                                published(peer, value, value + 7, 0, value + 7, value).unwrap();
                                let observed = ledger.snapshot().unwrap();
                                let mine = observed
                                    .rows
                                    .iter()
                                    .find(|row| row.id == peer.id())
                                    .unwrap();
                                assert_eq!(
                                    mine.physical.weights.idle,
                                    mine.physical.weights.live + 7
                                );
                            }
                        });
                    }
                });
                Ok(json!({"threads":2,"publications":64,"consistent":true}))
            }
            "reap" => Ok(
                json!({"reaped":ledger.reap_dead()?.iter().map(|row|row.pid).collect::<Vec<_>>()}),
            ),
            "burst" => {
                emit(json!({"burst":true,"pid":std::process::id(),"slot":peer.id().slot}));
                for value in 1..u64::MAX {
                    published(peer, value, 0, 0, 0, value)?;
                }
                unreachable!()
            }
            _ => Err(varena::ledger::Error::Invalid("unknown proof command")),
        }
    }

    fn fork_owner(directory: &Path) -> varena::ledger::Result<()> {
        let ledger = open(directory)?;
        let peer = ledger.register(Sharing::Shared, 0)?;
        published(&peer, 0, 123, 0, 123, 1)?;
        let fds: Vec<i32> = fs::read_dir("/proc/self/fd")?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                (fs::read_link(entry.path()).ok()?.as_path() == ledger.path())
                    .then(|| entry.file_name().to_str().unwrap().parse().unwrap())
            })
            .collect();
        assert_eq!(
            fds.len(),
            2,
            "one independent publication and row descriptor"
        );
        // SAFETY: this single-threaded proof process calls libc fork, exercising the registered
        // ledger atfork handlers. No CUDA context exists; the child does not touch parent locks.
        let child = unsafe { libc::fork() };
        if child < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if child == 0 {
            assert!(matches!(
                ledger.snapshot(),
                Err(varena::ledger::Error::Inherited)
            ));
            let replacement = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/null")?;
            for &fd in &fds {
                // SAFETY: all listed inherited ledger descriptors were closed by the atfork hook.
                // Reuse their numbers deliberately to expose any later inherited double close.
                assert_eq!(unsafe { libc::dup2(replacement.as_raw_fd(), fd) }, fd);
            }
            // Also reuse registry slots before dropping inherited descriptor objects.
            let fresh = open(directory)?;
            let own = fresh.register(Sharing::Shared, 3)?;
            drop(peer);
            drop(ledger);
            for &fd in &fds {
                // SAFETY: each numeric descriptor now owns our /dev/null duplicate.
                let still_open = unsafe { libc::fcntl(fd, libc::F_GETFD) };
                assert!(still_open >= 0, "inherited destructor closed reused fd");
            }
            emit(
                json!({"fork_child":std::process::id(),"fd_reuse":true,"inherited_rejected":true,
                    "slot":own.id().slot,"incarnation":own.id().incarnation,"snapshot":snapshot(&fresh)?}),
            );
            for fd in fds {
                if fd != replacement.as_raw_fd() {
                    // SAFETY: close only the proof's explicit dup2 descriptors, once.
                    unsafe { libc::close(fd) };
                }
            }
            while !directory.join("child-stop").exists() {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(own);
            drop(fresh);
            drop(replacement);
            std::process::exit(0);
        }
        emit(
            json!({"fork_parent":std::process::id(),"child":child,"slot":peer.id().slot,"incarnation":peer.id().incarnation}),
        );
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn fork_race(directory: &Path) -> varena::ledger::Result<()> {
        let ledger = open(directory)?;
        let peer = ledger.register(Sharing::Shared, 0)?;
        let parent = std::process::id();
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let path = directory.to_path_buf();
        let worker = std::thread::spawn(move || {
            let mut opens = 0;
            while !flag.load(Ordering::Acquire) {
                let observation = open(&path).unwrap();
                observation.snapshot().unwrap();
                drop(observation);
                opens += 1;
            }
            opens
        });
        for _ in 0..32 {
            // SAFETY: hooks were installed by initialize() before this worker started. This
            // Linux proof deliberately overlaps libc fork with actual ledger open/close.
            let child = unsafe { libc::fork() };
            if child < 0 {
                return Err(io::Error::last_os_error().into());
            }
            if child == 0 {
                assert!(matches!(
                    ledger.snapshot(),
                    Err(varena::ledger::Error::Inherited)
                ));
                let inherited = fs::read_dir("/proc/self/fd")?
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        (fs::read_link(entry.path()).ok()?.as_path() == ledger.path()).then_some(())
                    })
                    .count();
                assert_eq!(
                    inherited, 0,
                    "fork inherited an unregistered ledger description"
                );
                let fresh = open(directory)?;
                let mine = fresh.register(Sharing::Shared, 1)?;
                assert!(fresh.snapshot()?.rows.iter().any(|row| row.pid == parent));
                drop(mine);
                drop(fresh);
                // SAFETY: do not run inherited thread/runtime destructors in this proof child.
                unsafe { libc::_exit(0) };
            }
            let mut status = 0;
            // SAFETY: wait only for the exact child this loop just created.
            assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
            assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
        }
        stopped.store(true, Ordering::Release);
        let opens = worker.join().expect("open/close worker");
        assert!(opens > 0);
        // Best-effort Drop may leave metadata when the observer held publication. Every
        // child has been exactly waited above; reap its released row lock before asserting
        // the live set. A read-only snapshot is not an implicit liveness/cleanup operation.
        let before = ledger.snapshot()?;
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(ledger.path())?;
        for row in before.rows.iter().filter(|row| row.pid != parent) {
            let mut range = libc::flock {
                l_type: libc::F_WRLCK as _,
                l_whence: libc::SEEK_SET as _,
                l_start: (row.id.slot + 1) as _,
                l_len: 1,
                l_pid: 0,
            };
            // SAFETY: independent live descriptor and valid Linux flock; each child was
            // exactly waited above. Taking its row range proves the ownership FD is gone.
            assert_eq!(
                unsafe { libc::fcntl(probe.as_raw_fd(), libc::F_OFD_SETLK, &range) },
                0
            );
            range.l_type = libc::F_UNLCK as _;
            // SAFETY: release only the exact row range this proof just acquired.
            assert_eq!(
                unsafe { libc::fcntl(probe.as_raw_fd(), libc::F_OFD_SETLK, &range) },
                0
            );
        }
        let reaped = ledger.reap_dead()?;
        assert!(reaped.iter().all(|row| row.pid != parent));
        let after = ledger.snapshot()?;
        assert_eq!(after.rows.len(), 1);
        assert_eq!(
            after.sample, before.sample,
            "reaping never changes observed driver capacity"
        );
        drop(peer);
        emit(
            json!({"fork_open_close_iterations":32,"concurrent_opens":opens,
            "departures_reaped":reaped.len(),"passed":true}),
        );
        Ok(())
    }

    fn run() -> varena::ledger::Result<()> {
        // This proof establishes the first-use boundary while still single-threaded.
        varena::ledger::initialize()?;
        let args: Vec<_> = std::env::args().collect();
        let role = &args[1];
        let directory = Path::new(&args[2]);
        if role == "fork" {
            return fork_owner(directory);
        }
        if role == "fork-race" {
            return fork_race(directory);
        }
        let ledger = if role == "nonblocking" {
            Ledger::open_nonblocking(
                directory,
                Device::parse("GPU-13800000-0000-0000-0000-000000000003")?,
            )?
        } else {
            open(directory)?
        };
        if role == "inspect" {
            let descriptions = fs::read_dir("/proc/self/fd")?
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    (fs::read_link(entry.path()).ok()?.as_path() == ledger.path()).then_some(())
                })
                .count();
            assert_eq!(descriptions, 1, "exec inherited a ledger descriptor");
            let mut observed = snapshot(&ledger)?;
            observed["ledger_descriptions"] = json!(descriptions);
            emit(observed);
            return Ok(());
        }
        let priority = args.get(3).map_or(0, |n| n.parse().unwrap());
        let peer = ledger.register(Sharing::Shared, priority)?;
        emit(
            json!({"ready":true,"pid":std::process::id(),"slot":peer.id().slot,
                "incarnation":peer.id().incarnation,"path":ledger.path(),"snapshot":snapshot(&ledger)?}),
        );
        let mut peer = Some(peer);
        for line in io::stdin().lock().lines() {
            let line = line?;
            if line == "close" {
                break;
            }
            if line == "leave" {
                drop(peer.take());
                emit(json!({"left":true,"pid":std::process::id()}));
                continue;
            }
            if line == "alive" {
                emit(json!({"pid":std::process::id(),"participant":peer.is_some()}));
                continue;
            }
            let Some(peer) = &peer else {
                emit(json!({"error":"participant already left"}));
                continue;
            };
            match command(&ledger, peer, &line) {
                Ok(value) => emit(value),
                Err(error) => emit(json!({"error":error.to_string()})),
            }
        }
        Ok(())
    }
    pub fn main() {
        if let Err(error) = run() {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[cfg(all(target_os = "linux", target_has_atomic = "64"))]
fn main() {
    proof::main();
}

#[cfg(not(all(target_os = "linux", target_has_atomic = "64")))]
fn main() {
    eprintln!("ledger proof requires Linux with 64-bit atomics");
    std::process::exit(2);
}
