//! Real processes on one real table, no GPU: `cargo run -p varena-ledger-cells`. Each donor is a
//! child process holding pretend bytes and trimming them at a set speed, so what is under test is
//! the table, the kernel's lock and futex behaviour, and the protocol. The GPU run of the same
//! protocol is `cell.sh` beside this.
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use varena_ledger::{Donor, Recovery, Row, Table};

const GIB: u64 = 1 << 30;

struct Pretend {
    held: u64,
    trim_ms: u64,
}
impl Donor for Pretend {
    fn holdings(&mut self) -> (u64, u64) { (self.held, self.held) }
    fn trim(&mut self, bytes: u64) -> u64 {
        std::thread::sleep(Duration::from_millis(self.trim_ms));
        let released = bytes.min(self.held);
        self.held -= released;
        println!("trimmed {released}");
        released
    }
    fn tick(&mut self, _: &Table, _: Row, _: u64) -> bool { false }
}

/// Idle memory it can't give.
struct Stuck(u64);
impl Donor for Stuck {
    fn holdings(&mut self) -> (u64, u64) { (self.0, self.0) }
    fn trim(&mut self, _: u64) -> u64 { 0 }
    fn tick(&mut self, _: &Table, _: Row, _: u64) -> bool { false }
}

/// Child: hold `gib` GiB as a donor. `age` orders it: lower trims first.
fn donor(uuid: [u8; 16], gib: u64, trim_ms: u64, age_ms: u64) {
    // SAFETY: a plain prctl: a failed cell leaves no donor behind.
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
    let table = Table::open(&uuid).expect("open");
    let row = table.claim().expect("row");
    table.request_begin(row);
    std::thread::sleep(Duration::from_millis(age_ms));
    table.request_end(row);
    table.publish(row, gib * GIB, gib * GIB);
    println!("ready");
    if trim_ms == u64::MAX { table.serve(row, &mut Stuck(gib * GIB)); }
    table.serve(row, &mut Pretend { held: gib * GIB, trim_ms });
}

fn spawn(uuid: [u8; 16], gib: u64, trim_ms: u64, age_ms: u64) -> Child {
    let hex: String = uuid.iter().map(|b| format!("{b:02x}")).collect();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["donor", &hex, &gib.to_string(), &trim_ms.to_string(), &age_ms.to_string()])
        .stdout(Stdio::piped()).spawn().unwrap();
    let mut out = child.stdout.take().unwrap();
    let mut seen = [0u8; 6];
    std::io::Read::read_exact(&mut out, &mut seen).unwrap();
    assert_eq!(&seen, b"ready\n");
    std::thread::spawn(move || std::io::copy(&mut out, &mut std::io::sink()));
    std::thread::sleep(Duration::from_millis(10));
    child
}

/// A table with this process as an asker: a row, and its donor thread holding nothing.
fn fresh(tag: u8) -> ([u8; 16], &'static Table, Row) {
    let mut uuid = [tag; 16];
    uuid[..4].copy_from_slice(&std::process::id().to_le_bytes());
    let (table, row) = claimed(&uuid);
    (uuid, table, row)
}

fn claimed(uuid: &[u8; 16]) -> (&'static Table, Row) {
    let table: &'static Table = Box::leak(Box::new(Table::open(uuid).unwrap()));
    let row = table.claim().unwrap();
    std::thread::spawn(move || table.serve(row, &mut Pretend { held: 0, trim_ms: 0 }));
    (table, row)
}

fn signal(child: &Child, signal: i32) {
    // SAFETY: a live child's pid.
    assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
}

fn recover(table: &Table, row: Row, bytes: u64, patience: Duration) -> (Recovery, Duration) {
    let began = Instant::now();
    (table.recover(row, bytes, began + patience), began.elapsed())
}

pub fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("donor") {
        let mut uuid = [0; 16];
        for (byte, pair) in uuid.iter_mut().zip(args[1].as_bytes().chunks(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
        }
        return donor(uuid, args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap());
    }
    let patience = Duration::from_secs(2);

    // The oldest idle donor gives; a wake is far under the 500 ms old cadence.
    let (uuid, table, row) = fresh(1);
    let (mut old, mut young) = (spawn(uuid, 8, 0, 0), spawn(uuid, 8, 0, 20));
    let (result, took) = recover(table, row, 2 * GIB, patience);
    println!("ask: {result:?} in {took:?}, idle left {} GiB", table.idle() / GIB);
    assert_eq!(result, Recovery::Retry);
    assert!(took < Duration::from_millis(100), "{took:?}");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(table.idle(), 14 * GIB, "only the oldest trims");

    // Every release ends a wait: asking again takes the rest. With nothing idle anywhere every
    // peer answers, and the answer is final at once.
    let mut asks = 0;
    let (result, took) = loop {
        let (result, took) = recover(table, row, 32 * GIB, patience);
        asks += 1;
        if result == Recovery::Final { break (result, took); }
    };
    println!("empty after {asks} asks: {result:?} in {took:?}");
    assert_eq!(table.idle(), 0);
    assert_eq!(result, Recovery::Final);
    assert!(took < Duration::from_millis(500), "{took:?}");
    let _ = (old.kill(), young.kill(), old.wait(), young.wait());

    // Before an asker evicts its own weights: peers in no request give first, and one in a request
    // does not count as a reason to ask.
    let (uuid, table, row) = fresh(6);
    table.request_begin(row);
    assert!(!table.peers_idle(row));
    let mut idle = spawn(uuid, 4, 0, 0);
    assert!(table.peers_idle(row));
    let gave = table.ask_idle_peers(row, GIB, Instant::now() + patience);
    std::thread::sleep(Duration::from_millis(50));
    println!("idle first: gave {gave}, {} GiB left idle", table.idle() / GIB);
    assert!(gave);
    assert_eq!(table.idle(), 3 * GIB);
    let _ = (idle.kill(), idle.wait());

    // A donor killed mid-trim wakes the asker, and the other donor carries on.
    let (uuid, table, row) = fresh(2);
    let (mut dying, mut other) = (spawn(uuid, 8, 10_000, 0), spawn(uuid, 8, 0, 20));
    let waiter = std::thread::spawn(move || {
        let (table, row) = claimed(&uuid);
        recover(table, row, 16 * GIB, Duration::from_secs(30))
    });
    std::thread::sleep(Duration::from_millis(200));
    let killed = Instant::now();
    dying.kill().unwrap();
    dying.wait().unwrap();
    let (result, _) = waiter.join().unwrap();
    println!("killed: {result:?} {:?} after the kill", killed.elapsed());
    assert_eq!(result, Recovery::Retry);
    assert!(killed.elapsed() < Duration::from_millis(500), "{:?}", killed.elapsed());
    // The other donor is still there.
    let (result, _) = recover(table, row, GIB, patience);
    assert_eq!(result, Recovery::Retry);
    table.reap();
    assert_eq!(table.pids().len(), 3, "this process's two rows and the living donor");
    let _ = (other.kill(), other.wait());
    table.reap();
    // With no parent to report an exit, a dead peer means no wait: the asker takes what's free.
    let (result, took) = recover(table, row, GIB, patience);
    println!("dead peer, no parent: {result:?} in {took:?}");
    assert_eq!(result, Recovery::Final);
    assert!(took < Duration::from_millis(100), "{took:?}");

    // A stopped donor first in line does not block the one behind it.
    let (uuid, table, row) = fresh(3);
    let (mut stopped, mut behind) = (spawn(uuid, 8, 0, 0), spawn(uuid, 8, 0, 20));
    signal(&stopped, libc::SIGSTOP);
    std::thread::sleep(Duration::from_millis(50));
    let (result, took) = recover(table, row, 12 * GIB, patience);
    println!("stopped: {result:?} in {took:?}");
    assert_eq!(result, Recovery::Retry, "the donor behind takes the rest of the want");
    assert!(took < patience);
    // Now only the stopped one has anything: the deadline bounds it, never a hang.
    let (result, took) = recover(table, row, GIB, patience);
    println!("stopped alone: {result:?} in {took:?}");
    assert_eq!(result, Recovery::Final);
    assert!(took >= patience && took < patience * 2, "{took:?}");
    // A process's exit, as its parent reports it, wakes an asker at once.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        table.exited();
    });
    let (result, took) = recover(table, row, GIB, patience);
    println!("exit reported: {result:?} in {took:?}");
    assert_eq!(result, Recovery::Retry);
    assert!(took < Duration::from_millis(500), "{took:?}");
    signal(&stopped, libc::SIGCONT);
    let _ = (stopped.kill(), behind.kill(), stopped.wait(), behind.wait());

    // A forked child never keeps its dead parent's row alive.
    let (uuid, table, _) = fresh(4);
    let hex: String = uuid.iter().map(|b| format!("{b:02x}")).collect();
    let mut parent = Command::new(std::env::current_exe().unwrap());
    // SAFETY: fork and sleep in pre_exec's single thread, with the child exiting by itself.
    unsafe {
        parent.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
    let mut parent = parent.args(["donor", &hex, "1", "0", "0"]).stdout(Stdio::null()).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(table.pids().len(), 2);
    let _ = (parent.kill(), parent.wait());
    table.reap();
    assert_eq!(table.pids().len(), 1, "its row went with it");
    forked_child_holds_nothing(&uuid);

    // A peer whose idle memory can't be had: the asker doesn't wait out its patience on it.
    let (uuid, table, row) = fresh(7);
    let mut stuck = spawn(uuid, 2, u64::MAX, 0);
    let (result, took) = recover(table, row, GIB, patience);
    println!("idle it can't give: {result:?} in {took:?}");
    assert_eq!(result, Recovery::Final);
    assert!(took < Duration::from_millis(500), "{took:?}");
    let _ = (stuck.kill(), stuck.wait());

    // TensorD's turn: an asker peers could not help waits for an exit, or for a no.
    let (uuid, table, row) = fresh(5);
    let supervisor = Table::open(&uuid).unwrap();
    let waiting = std::thread::spawn(move || {
        let (table, row) = claimed(&uuid);
        let first = recover(table, row, GIB, Duration::from_secs(2)).0;
        (first, recover(table, row, GIB, Duration::from_secs(2)).0)
    });
    let ended = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    std::thread::spawn({
        let ended = ended.clone();
        move || supervisor.supervise(|_| Some(ended.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0))
    });
    let (first, second) = waiting.join().unwrap();
    println!("supervised: {first:?} then {second:?}, asked {}", ended.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!((first, second), (Recovery::Retry, Recovery::Final));
    assert_eq!(ended.load(std::sync::atomic::Ordering::SeqCst), 2);
    let _ = (table, row);

    for tag in [1u8, 2, 3, 4, 5, 6, 7] {
        let mut uuid = [tag; 16];
        uuid[..4].copy_from_slice(&std::process::id().to_le_bytes());
        let name: String = uuid.iter().map(|b| format!("{b:02x}")).collect();
        // SAFETY: geteuid has no arguments.
        let uid = unsafe { libc::geteuid() };
        let _ = std::fs::remove_file(format!("/dev/shm/cozy-varena-{uid}/v6-{name}"));
    }
    println!("all cells passed");
}

/// Fork from a row's owner, then have the owner die while the child sleeps: the row must go.
fn forked_child_holds_nothing(uuid: &[u8; 16]) {
    let watcher = Table::open(uuid).unwrap();
    // SAFETY: fork in a process whose other threads this cell does not rely on in the children.
    let owner = unsafe { libc::fork() };
    if owner == 0 {
        let table = Table::open(uuid).unwrap();
        let _row = table.claim().unwrap();
        // SAFETY: as above; the grandchild only sleeps and exits.
        if unsafe { libc::fork() } == 0 {
            unsafe { libc::sleep(3); libc::_exit(0) };
        }
        unsafe { libc::sleep(1); libc::_exit(0) };
    }
    std::thread::sleep(Duration::from_millis(300));
    let before = watcher.pids().len();
    std::thread::sleep(Duration::from_millis(1200));
    let mut status = 0;
    // SAFETY: our own child.
    unsafe { libc::waitpid(owner, &mut status, 0) };
    watcher.reap();
    let after = watcher.pids().len();
    println!("fork: {before} rows with the owner, {after} after it died with its child alive");
    assert_eq!(after, before - 1);
}
