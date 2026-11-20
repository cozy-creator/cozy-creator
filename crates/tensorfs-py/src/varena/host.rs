//! This process's row in the host's ledger page (`varena_ledger::Table::open_host`): one owner of
//! host bytes across every executor of the machine, by the GPU page's protocol. What a process
//! holds there is its tiers' pinned pages. In no request all of it is idle: a process in a request
//! that is short asks, idle tiers give pages back, and TensorD ends an idle process when that is
//! not enough, as v1 ended every idle neighbour. Advisory: with no table a tier decides alone.
use super::pages::Tier;
use std::sync::Weak;

#[cfg(target_os = "linux")]
mod imp {
    use super::{Tier, Weak};
    use platform::contain::host_pressure::{HostMode, Pressure};
    use std::sync::{Arc, Condvar, Mutex, Once};
    use std::time::{Duration, Instant};
    use varena_ledger::{Donor, Look, Recovery, Row, Table};

    /// How long an ask waits on peers that have stopped answering.
    const PEER_WAIT: Duration = Duration::from_secs(2);

    pub(super) struct Host {
        table: Table,
        row: Row,
        tiers: Mutex<Vec<Weak<Tier>>>,
        asking: Mutex<Ask>,
        ask: Condvar,
        watching: Once,
    }

    /// What tiers in a request are short of, for the asking thread. `refused`: the last ask
    /// came back final, so none goes again before a request ends.
    #[derive(Default)]
    struct Ask {
        bytes: u64,
        refused: bool,
    }

    /// The process's host, made once per pid: a forked child makes its own.
    static HOST: Mutex<Option<(u32, Option<&'static Host>)>> = Mutex::new(None);

    pub(super) fn host() -> Option<&'static Host> {
        let pid = std::process::id();
        let mut slot = HOST.lock().unwrap_or_else(|error| error.into_inner());
        if let Some((owner, host)) = *slot {
            if owner == pid {
                return host;
            }
        }
        let host = open();
        *slot = Some((pid, host));
        host
    }

    fn open() -> Option<&'static Host> {
        let table = Table::open_host().map_err(|error| tracing::warn!("host pages: no host ledger: {error}")).ok()?;
        let Some(row) = table.claim() else {
            tracing::warn!("host pages: the host ledger is full; this process gives nothing back");
            return None;
        };
        let host: &'static Host = Box::leak(Box::new(Host {
            table, row, tiers: Mutex::default(), asking: Mutex::default(), ask: Condvar::new(), watching: Once::new(),
        }));
        let serving = std::thread::Builder::new().name("tensorfs-host-owner".into())
            .spawn(move || host.table.serve(host.row, &mut Giving(host)));
        let asking = std::thread::Builder::new().name("tensorfs-host-ask".into()).spawn(move || host.asks());
        if let Err(error) = serving.and(asking) {
            tracing::warn!("host pages: {error}");
        }
        Some(host)
    }

    impl Host {
        fn tiers(&self) -> Vec<Arc<Tier>> {
            let mut tiers = self.tiers.lock().unwrap_or_else(|error| error.into_inner());
            tiers.retain(|tier| tier.strong_count() != 0);
            tiers.iter().filter_map(Weak::upgrade).collect()
        }

        fn asks(&self) {
            loop {
                let bytes = {
                    let mut ask = self.asking.lock().unwrap_or_else(|error| error.into_inner());
                    while ask.bytes == 0 || ask.refused {
                        ask = self.ask.wait(ask).unwrap_or_else(|error| error.into_inner());
                    }
                    std::mem::take(&mut ask.bytes)
                };
                let recovered = self.table.recover(self.row, bytes, Instant::now() + PEER_WAIT);
                if recovered == Recovery::Final {
                    self.asking.lock().unwrap_or_else(|error| error.into_inner()).refused = true;
                }
                for tier in self.tiers() {
                    tier.relook();
                }
            }
        }

        /// Out of a request its tiers look again only when asked or when the host stalls on memory or
        /// meets a limit: what grew elsewhere takes pages back. No timer.
        fn watch(&'static self, mode: HostMode) {
            let mut pressure = match Pressure::arm(mode) {
                Ok(pressure) => pressure,
                Err(error) => return tracing::warn!("host pages: idle tiers look again only when asked: {error}"),
            };
            let watching = std::thread::Builder::new().name("tensorfs-host-pressure".into()).spawn(move || {
                while pressure.wait().is_ok() {
                    for tier in self.tiers() {
                        tier.relook();
                    }
                }
            });
            if let Err(error) = watching {
                tracing::warn!("host pages: idle tiers look again only when asked: {error}");
            }
        }
    }

    struct Giving(&'static Host);

    impl Donor for Giving {
        /// What this process holds that the kernel cannot take back (its anonymous bytes, pinned
        /// pages among them, and its staging, which is shared memory), for varena's host number;
        /// idle, its pinned pages.
        fn holdings(&mut self) -> (u64, u64) {
            let tiers = self.0.tiers();
            let pinned: u64 = tiers.iter().map(|tier| tier.givable()).sum();
            let staged = tiers.iter().map(|tier| tier.held()).sum::<u64>().saturating_sub(pinned);
            let anon = platform::contain::host_memory::resident(std::process::id()).map_or(pinned, |now| now.anon.max(pinned));
            (anon + staged, if self.0.table.running(self.0.row) { 0 } else { pinned })
        }

        fn trim(&mut self, bytes: u64) -> u64 {
            if self.0.table.running(self.0.row) {
                return 0;
            }
            let mut given = 0;
            for tier in self.0.tiers() {
                given += tier.give(bytes.saturating_sub(given));
                if given >= bytes {
                    break;
                }
            }
            given
        }

        /// Each tier looks again: other programs and processes grew, and its pages give way.
        fn tick(&mut self, _: &Table, _: Row, _: u64) -> Look {
            for tier in self.0.tiers() {
                tier.relook();
            }
            Look::Nothing
        }
    }

    pub(super) fn join(tier: Weak<Tier>, shared: bool) {
        if let Some(host) = host() {
            host.tiers.lock().unwrap_or_else(|error| error.into_inner()).push(tier);
            host.watching.call_once(|| host.watch(if shared { HostMode::Shared } else { HostMode::Dedicated }));
        }
    }

    pub(super) fn boundary(begin: bool) {
        if let Some(host) = host() {
            if begin {
                host.table.request_begin(host.row);
            } else {
                host.table.request_end(host.row);
                host.asking.lock().unwrap_or_else(|error| error.into_inner()).refused = false;
            }
        }
    }

    pub(super) fn short(bytes: u64) -> u64 {
        let Some(host) = host() else { return 0 };
        if !host.table.running(host.row) {
            return 0;
        }
        let mut ask = host.asking.lock().unwrap_or_else(|error| error.into_inner());
        if ask.refused {
            return 0;
        }
        ask.bytes = ask.bytes.max(bytes);
        host.ask.notify_one();
        host.table.idle_elsewhere(host.row)
    }

    pub(super) fn ask_idle(bytes: u64) -> bool {
        let Some(host) = host() else { return false };
        let gave = host.table.ask_idle_peers(host.row, bytes, Instant::now() + PEER_WAIT);
        for tier in host.tiers() {
            tier.relook();
        }
        gave
    }

    pub(super) fn peers_running() -> bool {
        host().is_some_and(|host| host.table.peers_running(host.row))
    }

    /// This process's row says what it holds now, not at its donor's last look.
    pub(super) fn republish() {
        if let Some(host) = host() {
            let (held, idle) = Giving(host).holdings();
            host.table.publish(host.row, held, idle);
        }
    }

    /// This process's own tiers give about `bytes` back, spares and the pages worth least first.
    pub(super) fn give_own(bytes: u64) -> u64 {
        let Some(host) = host() else { return 0 };
        let mut given = 0;
        for tier in host.tiers() {
            if given >= bytes {
                break;
            }
            given += tier.give(bytes - given);
        }
        given
    }
}

/// `tier` gives and asks through this process's row; `shared`, on a person's computer.
pub(super) fn join(tier: Weak<Tier>, shared: bool) {
    #[cfg(target_os = "linux")]
    imp::join(tier, shared);
    #[cfg(not(target_os = "linux"))]
    let _ = (tier, shared);
}

/// A GPU request of this process begins or ends.
pub(super) fn boundary(begin: bool) {
    #[cfg(target_os = "linux")]
    imp::boundary(begin);
    #[cfg(not(target_os = "linux"))]
    let _ = begin;
}

/// A tier of a process in a request is `bytes` short: idle peers give pages back, then TensorD
/// ends an idle process. Never waits: the asking thread does, and the tiers look again after. What
/// idle peers hold that the ask may bring back: 0 out of a request, or once an ask came back final.
pub(super) fn short(bytes: u64) -> u64 {
    #[cfg(target_os = "linux")]
    return imp::short(bytes);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = bytes;
        0
    }
}

/// Ask idle peers for `bytes` and wait for their answers (a few seconds at most): whether any came
/// back. For a copy that cannot wait for the asking thread, a retained region's.
pub(super) fn ask_idle(bytes: u64) -> bool {
    #[cfg(target_os = "linux")]
    return imp::ask_idle(bytes);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = bytes;
        false
    }
}

/// What `admit` asks against, for one reading of the number: `varena_ledger::host::admission`, the
/// number TensorFS's reads keep their page cache within too. A CPU executor's weights draw on the
/// same room (`varena.host_room`, var-027).
pub(super) fn room(now: &varena_ledger::host::Budget) -> i64 {
    varena_ledger::host::admission(now)
}

/// The machine's one host admission (var-030 W5), for `bytes` more host memory this process will pin
/// (varena's last resort, the Runtime's pinned wholes, a shared result mapping): what the number leaves
/// (`varena_ledger::host::budget`: the tightest limit on this process's path, as TensorD enforces it)
/// past every executor's holdings (the kernel's charge, or the host page's sum where that is more) and
/// the headroom. Short of it, idle executors give pages back, then this process's own tiers, whose
/// pages the store fills again; still short, what it lacked, before the kernel would end anything.
/// Unreadable figures admit it: only the pin itself can refuse then.
pub(super) fn admit(bytes: u64) -> Result<(), varena::HostShort> {
    use varena_ledger::host::{Budget, budget};
    let want = i64::try_from(bytes).unwrap_or(i64::MAX);
    let short = |now: &Budget| u64::try_from(want - room(now)).unwrap_or(0);
    let mut now = budget();
    if room(&now) >= want {
        return Ok(());
    }
    ask_idle(short(&now));
    now = budget();
    // This process's own pages, until there is room or nothing is left to give. Its row in the host page
    // is published again after each give: the number takes the larger of the kernel's charge and the
    // page's rows, and a row the donor thread last published still counts what was just given.
    #[cfg(target_os = "linux")]
    while room(&now) < want {
        let given = imp::give_own(short(&now));
        imp::republish();
        now = budget();
        if given == 0 {
            break;
        }
    }
    if room(&now) >= want {
        return Ok(());
    }
    Err(varena::HostShort {
        wanted: bytes,
        free: u64::try_from(room(&now)).unwrap_or(0),
        limit: u64::try_from(now.total).unwrap_or(0),
    })
}

/// Another process on this machine is in a request: an idle tier pins nothing meanwhile.
pub(super) fn peers_running() -> bool {
    #[cfg(target_os = "linux")]
    return imp::peers_running();
    #[cfg(not(target_os = "linux"))]
    false
}
