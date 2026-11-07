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
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};
    use varena_ledger::{Donor, Recovery, Row, Table};

    /// How long an ask waits on peers that have stopped answering.
    const PEER_WAIT: Duration = Duration::from_secs(2);

    pub(super) struct Host {
        table: Table,
        row: Row,
        tiers: Mutex<Vec<Weak<Tier>>>,
        asking: Mutex<Ask>,
        ask: Condvar,
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
            table, row, tiers: Mutex::default(), asking: Mutex::default(), ask: Condvar::new(),
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
    }

    struct Giving(&'static Host);

    impl Donor for Giving {
        fn holdings(&mut self) -> (u64, u64) {
            let held = self.0.tiers().iter().map(|tier| tier.held()).sum();
            (held, if self.0.table.running(self.0.row) { 0 } else { held })
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
        fn tick(&mut self, _: &Table, _: Row, _: u64) -> bool {
            for tier in self.0.tiers() {
                tier.relook();
            }
            false
        }
    }

    pub(super) fn join(tier: Weak<Tier>) {
        if let Some(host) = host() {
            host.tiers.lock().unwrap_or_else(|error| error.into_inner()).push(tier);
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

    pub(super) fn short(bytes: u64) {
        let Some(host) = host() else { return };
        if !host.table.running(host.row) {
            return;
        }
        let mut ask = host.asking.lock().unwrap_or_else(|error| error.into_inner());
        if !ask.refused {
            ask.bytes = ask.bytes.max(bytes);
            host.ask.notify_one();
        }
    }

    pub(super) fn peers_running() -> bool {
        host().is_some_and(|host| host.table.peers_running(host.row))
    }
}

/// `tier` gives and asks through this process's row.
pub(super) fn join(tier: Weak<Tier>) {
    #[cfg(target_os = "linux")]
    imp::join(tier);
    #[cfg(not(target_os = "linux"))]
    drop(tier);
}

/// A GPU request of this process begins or ends.
pub(super) fn boundary(begin: bool) {
    #[cfg(target_os = "linux")]
    imp::boundary(begin);
    #[cfg(not(target_os = "linux"))]
    let _ = begin;
}

/// A tier of a process in a request is `bytes` short: idle peers give pages back, then TensorD
/// ends an idle process. Never waits: the asking thread does, and the tiers look again after.
pub(super) fn short(bytes: u64) {
    #[cfg(target_os = "linux")]
    imp::short(bytes);
    #[cfg(not(target_os = "linux"))]
    let _ = bytes;
}

/// Another process on this machine is in a request: an idle tier pins nothing meanwhile.
pub(super) fn peers_running() -> bool {
    #[cfg(target_os = "linux")]
    return imp::peers_running();
    #[cfg(not(target_os = "linux"))]
    false
}
