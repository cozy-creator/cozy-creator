//! Host RAM is varena's (Paul 11-07, REVIEW #3): one number for what a machine's processes may
//! hold, sized here from what the kernel accounts, with the computer's reserve for programs
//! outside cozy left out. Its clients draw on it and keep no budget of their own: weight pages,
//! spilled retained state and spares (a tier's [`Budget::room`]), the page cache a read or a
//! read-ahead fills ([`Budget::cache`]), and staging and every process's own memory, which the
//! kernel charges and [`Budget::held`] counts. A contained machine's `memory.max` is this number
//! ([`machine`]): TensorD writes it and sizes nothing. Idle clients give pages back through the
//! host's ledger page, and every client looks again as the kernel's figures move.
use platform::contain::{MachineFacts, host_memory};

const GIB: u64 = 1 << 30;
/// What the machine's own processes may grow by under the tightest number.
pub const MACHINE_ROOM: u64 = GIB;
/// The least a contained machine's processes keep free for growth between looks.
const LEAST_HEADROOM: u64 = 512 << 20;

/// What stays free for programs outside cozy to grow into: a quarter of the memory in play for
/// cozy (what it holds plus what is available), from 4 to 16 GiB, plus what free swap is short of a
/// quarter of the swap (a full swap takes nothing those programs push out). What they hold now is
/// theirs already and is never counted again: a desktop holding half of a 62 GiB laptop leaves the
/// margin of a 31 GiB one. Live: both move.
pub fn reserve(in_play: u64) -> u64 {
    let (swap, free) = host_memory::swap();
    (in_play / 4).clamp(4 * GIB, 16 * GIB) + (swap / 4).saturating_sub(free)
}

/// A contained machine's number and the reserve kept outside it: what it holds that the kernel
/// cannot take back plus what the computer has available outside it, less the reserve. Never more
/// than the computer less the reserve, never less than its own processes plus [`MACHINE_ROOM`].
/// When programs outside grow into the reserve the number falls below what the executors hold:
/// their tiers give pages back as they look, then the kernel takes page cache, then kills inside
/// the machine. A run may fail; the computer stays up.
pub fn machine(facts: &MachineFacts) -> (u64, u64) {
    let in_play = facts.held + facts.outside;
    let reserve = reserve(in_play);
    let max = in_play.saturating_sub(reserve).min(facts.computer.saturating_sub(reserve));
    (max.max(facts.own + MACHINE_ROOM), reserve)
}

/// What cozy's executors hold that the kernel cannot take back, as each publishes it in the host's
/// ledger page; 0 with no page.
fn cozy_held() -> u64 {
    #[cfg(target_os = "linux")]
    {
        static TABLE: std::sync::OnceLock<Option<crate::Table>> = std::sync::OnceLock::new();
        TABLE.get_or_init(|| crate::Table::open_host().ok()).as_ref().map_or(0, crate::Table::held)
    }
    #[cfg(not(target_os = "linux"))]
    0
}

/// The number as one process sees it, from the tightest limit on its path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    /// What that limit's processes may hold in all. Contained, the limit is the number; elsewhere
    /// it is the computer less the reserve.
    pub total: i64,
    /// What they hold that the kernel cannot take back: all it charges less clean page cache.
    pub held: i64,
    /// All it charges, page cache included.
    pub charged: i64,
    /// Kept free past pinned pages where the kernel enforces the number, for what a call grows by
    /// before a client looks again: a sixteenth of it (varena's GPU rule), at least 512 MiB.
    /// Elsewhere the reserve is the slack, and nothing kills at the number.
    pub headroom: i64,
    pub reserve: i64,
}

impl Budget {
    /// What pinned pages may still take, past `headroom`. Negative: give some back.
    pub fn room(&self) -> i64 {
        self.total - self.held - self.headroom
    }

    /// What the page cache may still fill: memory nothing holds within the number. Past it a
    /// cached read displaces other cache, a store larger than memory its own earlier bytes.
    pub fn cache(&self) -> i64 {
        self.total - self.charged
    }
}

/// The page cache a read or a read-ahead may still fill now (`Budget::cache`): every reader of
/// the store stays within it, and keeps no bound of its own.
pub fn cache_budget() -> i64 {
    budget().cache()
}

/// The number now, for this process. Unreadable: no room at all. Uncontained, the memory in play is
/// what the host has available plus what cozy's executors hold (the host's ledger page).
pub fn budget() -> Budget {
    let memory = host_memory::read();
    if memory.available < 0 || memory.bound <= 0 {
        return Budget::default();
    }
    let in_play = u64::try_from(memory.available + memory.active).unwrap_or(0) + cozy_held();
    let reserve = i64::try_from(reserve(in_play)).unwrap_or(i64::MAX);
    let held = (memory.bound - memory.available - memory.active).max(0);
    let computer = host_memory::computer().and_then(|computer| i64::try_from(computer).ok()).unwrap_or(memory.bound);
    let (total, headroom) = if memory.bound < computer {
        (memory.bound, (memory.bound / 16).max(LEAST_HEADROOM as i64))
    } else {
        (computer - reserve, 0)
    };
    Budget { total, held, charged: memory.charged, headroom, reserve }
}
