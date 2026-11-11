//! The machine's disks, watched for stalls. A device holding requests that completes none for
//! `STALL` has stopped answering: its driver times the requests out (NVMe after 30 s), aborts and
//! retries them, and every read of the store waits meanwhile. Nothing else names it: on the 0.24.0
//! bench's pod jeanne an H3 warm call spent 614 s in 21 such stalls with no sign in its record
//! (cl-861). When the device answers again, the stall is named on every run then in progress.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const LOOK: Duration = Duration::from_millis(500);
/// A healthy disk completes some request well within this, however loaded.
const STALL: Duration = Duration::from_secs(10);

/// A device's requests completed so far (reads, writes, discards, flushes) and those in flight.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Count {
    done: u64,
    in_flight: u64,
}

/// `/proc/diskstats`, by whole device: a partition is its disk's, so one stall is named once.
/// Virtual devices (loops, mappers, RAID) go: the disk under one stalls with it.
fn sample(stats: &str) -> BTreeMap<String, Count> {
    let rows: Vec<Vec<&str>> = stats.lines().map(|line| line.split_whitespace().collect()).filter(|f: &Vec<&str>| f.len() > 11).collect();
    let names: Vec<&str> = rows.iter().map(|fields| fields[2]).collect();
    rows.iter().filter(|fields| {
        let name = fields[2];
        !["loop", "ram", "zram", "sr", "fd", "dm-", "md", "nbd"].iter().any(|prefix| name.starts_with(prefix))
            && !names.iter().any(|disk| partition_of(name, disk))
    }).map(|fields| {
        let number = |at: usize| fields.get(at).and_then(|field| field.parse::<u64>().ok()).unwrap_or(0);
        (fields[2].to_owned(), Count { done: number(3) + number(7) + number(14) + number(18), in_flight: number(11) })
    }).collect()
}

/// The kernel's partition names: `sda3` of `sda`, `nvme0n1p2` of `nvme0n1`.
fn partition_of(name: &str, disk: &str) -> bool {
    let Some(rest) = name.strip_prefix(disk).filter(|rest| !rest.is_empty()) else { return false };
    let rest = match disk.ends_with(|c: char| c.is_ascii_digit()) {
        true => match rest.strip_prefix('p') { Some(rest) => rest, None => return false },
        false => rest,
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

/// One device's watch: when it last completed a request (or held none), and the most it held since.
struct Watch {
    count: Count,
    since: Instant,
    most: u64,
}

/// A stall that ended: the device, how long it completed nothing, the most requests it held.
pub(crate) struct Stall {
    pub device: String,
    pub seconds: f64,
    pub in_flight: u64,
}

impl Stall {
    pub(crate) fn warning(&self) -> String {
        format!(
            "the machine's disk {} completed nothing for {:.0} s with {} request(s) waiting: the device stalled, and reads of model weights waited on it",
            self.device, self.seconds, self.in_flight
        )
    }
}

/// Every device's watch.
#[derive(Default)]
struct Disks(BTreeMap<String, Watch>);

impl Disks {
    /// One look at `/proc/diskstats` taken at `now`: the stalls it ends.
    fn look(&mut self, stats: &str, now: Instant) -> Vec<Stall> {
        let mut ended = vec![];
        for (device, count) in sample(stats) {
            let watch = self.0.entry(device.clone()).or_insert(Watch { count, since: now, most: 0 });
            if count.in_flight != 0 && count.done == watch.count.done {
                watch.most = watch.most.max(count.in_flight);
                continue;
            }
            let (quiet, most) = (now - watch.since, watch.most);
            *watch = Watch { count, since: now, most: 0 };
            if quiet >= STALL {
                ended.push(Stall { device, seconds: quiet.as_secs_f64(), in_flight: most });
            }
        }
        ended
    }
}

/// Watches every `LOOK` until `stalled` returns false (its owner is gone).
pub(crate) fn watch(mut stalled: impl FnMut(Stall) -> bool + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new().name("disk-stalls".into()).spawn(move || {
        let mut disks = Disks::default();
        loop {
            std::thread::sleep(LOOK);
            let Ok(stats) = std::fs::read_to_string("/proc/diskstats") else { return };
            if !disks.look(&stats, Instant::now()).into_iter().all(&mut stalled) {
                return;
            }
        }
    })?;
    Ok(())
}
