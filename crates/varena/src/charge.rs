//! The machine's GPU charge page (design varena-local): with `--gpu-memory-limit` set, TensorD makes
//! one sealed page holding a section per GPU (its uuid, its limit, the bytes cozy's executors hold
//! there) and hands it to every executor at Start. An executor charges each byte it creates on a GPU
//! before creating it and refunds it on release, so together they never pass the limit; TensorD
//! frees an exited executor's slots. Atomics only: the page is untrusted input to either side.
//!
//! Write order, so a kill between two steps only over-counts: a charge raises `charged` (CAS, never
//! past `limit`) and then its slot's `bytes`; a refund lowers the slot's first. A slot is freed
//! bytes and birth first, then `pid` (Release), so a claimer that wins the pid CAS finds it clean.

use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicU32, AtomicU64};

use crate::os::{self, Map, Owned};
use crate::{Error, Result};

pub const MAGIC: u64 = u64::from_le_bytes(*b"COZYCHG1");
/// Raised only for an incompatible layout; additive fields take reserved space.
pub const VERSION: u32 = 1;
/// Slots per GPU: one per process holding memory there.
pub const SLOTS: u32 = 64;

#[repr(C)]
struct Header {
    magic: AtomicU64,
    version: AtomicU32,
    gpus: AtomicU32,
    slots: AtomicU32,
    _pad: AtomicU32,
    _reserved: AtomicU64,
}

/// One GPU's books: its uuid (the driver's 16 raw bytes, as NVML and `cuDeviceGetUuid` give them),
/// what cozy may hold there, and what its slots hold now.
#[repr(C)]
pub struct Section {
    uuid: [AtomicU64; 2],
    limit: AtomicU64,
    charged: AtomicU64,
    _reserved: AtomicU64,
}

/// One process's charge on one GPU.
#[repr(C)]
pub struct Slot {
    pub pid: AtomicU32,
    _pad: AtomicU32,
    pub start_ticks: AtomicU64,
    pub bytes: AtomicU64,
    _reserved: AtomicU64,
}

const HEAD: usize = std::mem::size_of::<Header>();
const SECTION: usize = std::mem::size_of::<Section>();
const SLOT: usize = std::mem::size_of::<Slot>();
const _: () = assert!(HEAD == 32 && SECTION == 40 && SLOT == 32);

/// The bytes a page of `gpus` sections of `slots` slots takes.
pub fn bytes(gpus: u32, slots: u32) -> usize {
    HEAD + gpus as usize * (SECTION + SLOT * slots as usize)
}

/// The 16 raw bytes of a `GPU-xxxxxxxx-xxxx-…` uuid; None for any other name.
pub fn uuid_bytes(uuid: &str) -> Option<[u8; 16]> {
    let hex: String = uuid.strip_prefix("GPU-")?.chars().filter(|c| *c != '-').collect();
    let mut out = [0u8; 16];
    if hex.len() != 32 {
        return None;
    }
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

/// A mapped charge page.
pub struct Page {
    map: Map,
    gpus: u32,
    slots: u32,
}

impl Page {
    /// TensorD's: a new sealed page with a section per GPU (its uuid and limit), and the handle
    /// every executor's Start carries.
    pub fn create(limits: &[([u8; 16], u64)]) -> Result<(Page, Owned)> {
        let gpus = u32::try_from(limits.len()).map_err(|_| Error::Invalid("too many GPUs for a charge page".into()))?;
        let len = bytes(gpus, SLOTS);
        let file = os::sealed_page(len as u64)?;
        let page = Page { map: os::map(&file, len, true)?, gpus, slots: SLOTS };
        let h = page.header();
        h.version.store(VERSION, Relaxed);
        h.gpus.store(gpus, Relaxed);
        h.slots.store(SLOTS, Relaxed);
        for (i, (uuid, limit)) in limits.iter().enumerate() {
            let Some(s) = page.section_at(i as u32) else { continue };
            let (lo, hi) = uuid.split_at(8);
            s.uuid[0].store(u64::from_le_bytes(lo.try_into().unwrap_or_default()), Relaxed);
            s.uuid[1].store(u64::from_le_bytes(hi.try_into().unwrap_or_default()), Relaxed);
            s.limit.store(*limit, Relaxed);
        }
        h.magic.store(MAGIC, Release);
        Ok((page, file))
    }

    /// Either side's: an existing page. A version above this side's is refused (its caller runs
    /// uncharged, warning); a lower one is read as this side's, its new fields reserved there.
    pub fn attach(file: &Owned) -> Result<Page> {
        let len = os::size(file)? as usize;
        if len < HEAD {
            return Err(Error::Invalid("a charge page shorter than its header".into()));
        }
        let map = os::map(file, len, true)?;
        // SAFETY: the mapping is page-aligned, at least the header long, and the header is atomics.
        let h = unsafe { &*(map.base as *const Header) };
        if h.magic.load(Acquire) != MAGIC {
            return Err(Error::Invalid("not a charge page".into()));
        }
        let version = h.version.load(Relaxed);
        if version > VERSION {
            return Err(Error::Invalid(format!("charge page version {version}, this side reads {VERSION}")));
        }
        let (gpus, slots) = (h.gpus.load(Relaxed), h.slots.load(Relaxed));
        if len < bytes(gpus, slots) {
            return Err(Error::Invalid("a charge page shorter than its sections".into()));
        }
        Ok(Page { map, gpus, slots })
    }

    fn header(&self) -> &Header {
        // SAFETY: checked at create/attach: the mapping holds the header, all atomics.
        unsafe {
            &*(self.map.base as *const Header)
        }
    }

    fn section_at(&self, i: u32) -> Option<&Section> {
        if i >= self.gpus {
            return None;
        }
        let at = HEAD + i as usize * (SECTION + SLOT * self.slots as usize);
        // SAFETY: in bounds (`bytes(gpus, slots)` checked at create/attach), 8-aligned, atomics.
        Some(unsafe { &*(self.map.base.add(at) as *const Section) })
    }

    /// The section of the GPU with these raw uuid bytes.
    pub fn section(&self, uuid: &[u8; 16]) -> Option<&Section> {
        (0..self.gpus).filter_map(|i| self.section_at(i)).find(|s| s.uuid() == *uuid)
    }

    /// Every GPU's section.
    pub fn sections(&self) -> impl Iterator<Item = &Section> {
        (0..self.gpus).filter_map(|i| self.section_at(i))
    }

    /// The slots of `section` (one of this page's).
    pub fn slots<'a>(&'a self, section: &'a Section) -> impl Iterator<Item = &'a Slot> {
        let base = section as *const Section as *const u8;
        (0..self.slots as usize).map(move |k| {
            // SAFETY: `section` is one of this page's, followed by its `slots` slots in bounds.
            unsafe {
                &*(base.add(SECTION + k * SLOT) as *const Slot)
            }
        })
    }

    /// TensorD's, once `pid` of `start_ticks` has exited (proven): its slots in every section are
    /// freed and their bytes refunded; a section left with no claimed slot is healed to 0 (what a
    /// kill between a charge's two steps left over). The bytes refunded.
    pub fn reap(&self, pid: u32, start_ticks: u64) -> u64 {
        let mut refunded = 0;
        for section in self.sections() {
            for slot in self.slots(section) {
                if slot.pid.load(Acquire) != pid || slot.start_ticks.load(Acquire) != start_ticks {
                    continue;
                }
                let bytes = slot.bytes.swap(0, AcqRel);
                slot.start_ticks.store(0, Release);
                section.refund(bytes);
                slot.pid.store(0, Release);
                refunded += bytes;
            }
            let charged = section.charged();
            if charged > 0 && self.slots(section).all(|s| s.pid.load(Acquire) == 0) {
                let _ = section.charged.compare_exchange(charged, 0, AcqRel, Relaxed);
            }
        }
        refunded
    }
}

impl Section {
    pub fn uuid(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        let (lo, hi) = out.split_at_mut(8);
        lo.copy_from_slice(&self.uuid[0].load(Relaxed).to_le_bytes());
        hi.copy_from_slice(&self.uuid[1].load(Relaxed).to_le_bytes());
        out
    }

    pub fn limit(&self) -> u64 {
        self.limit.load(Relaxed)
    }

    /// What cozy's executors hold on this GPU now.
    pub fn charged(&self) -> u64 {
        self.charged.load(Acquire)
    }

    /// What is left of its limit.
    pub fn left(&self) -> u64 {
        self.limit().saturating_sub(self.charged())
    }

    /// Raises `charged` by `bytes` unless that passes the limit; whether it did.
    pub fn charge(&self, bytes: u64) -> bool {
        let limit = self.limit();
        self.charged
            .try_update(AcqRel, Acquire, |c| c.checked_add(bytes).filter(|n| *n <= limit))
            .is_ok()
    }

    /// Lowers `charged` by `bytes`, never below 0.
    pub fn refund(&self, bytes: u64) {
        let _ = self.charged.try_update(AcqRel, Acquire, |c| Some(c.saturating_sub(bytes)));
    }
}

/// This process's slot in one GPU's section: each byte it creates there is charged first and
/// refunded once destroyed. Freed on drop once it holds nothing (else TensorD's reap frees it).
pub struct Claim {
    page: Page,
    /// The section's and the slot's offsets in the mapping.
    section: usize,
    slot: usize,
}

impl Claim {
    /// A free slot of `uuid`'s section, taken for this process (`pid` of `start_ticks`). None
    /// when the page has no section for this GPU or no free slot; an error for a page this side
    /// cannot read.
    pub fn take(file: &Owned, uuid: &[u8; 16], pid: u32, start_ticks: u64) -> Result<Option<Claim>> {
        let page = Page::attach(file)?;
        let at = |p: *const u8| p as usize - page.map.base as usize;
        let Some(section) = page.section(uuid) else { return Ok(None) };
        let Some(slot) = page.slots(section).find(|s| s.pid.compare_exchange(0, pid, AcqRel, Relaxed).is_ok()) else { return Ok(None) };
        slot.start_ticks.store(start_ticks, Release);
        let (section, slot) = (at(section as *const Section as *const u8), at(slot as *const Slot as *const u8));
        Ok(Some(Claim { page, section, slot }))
    }

    fn section(&self) -> &Section {
        // SAFETY: an offset `take` found in this mapping, which the claim holds.
        unsafe { &*(self.page.map.base.add(self.section) as *const Section) }
    }

    fn slot(&self) -> &Slot {
        // SAFETY: as `section`.
        unsafe { &*(self.page.map.base.add(self.slot) as *const Slot) }
    }

    /// Charges `bytes` unless that passes the GPU's limit; whether it did.
    pub fn charge(&self, bytes: u64) -> bool {
        let charged = self.section().charge(bytes);
        if charged {
            self.slot().bytes.fetch_add(bytes, AcqRel);
        }
        charged
    }

    pub fn refund(&self, bytes: u64) {
        let _ = self.slot().bytes.try_update(AcqRel, Acquire, |b| Some(b.saturating_sub(bytes)));
        self.section().refund(bytes);
    }

    /// What cozy's executors hold on this GPU now.
    pub fn charged(&self) -> u64 {
        self.section().charged()
    }

    pub fn left(&self) -> u64 {
        self.section().left()
    }

    pub fn limit(&self) -> u64 {
        self.section().limit()
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let slot = self.slot();
        if slot.bytes.load(Acquire) == 0 {
            slot.start_ticks.store(0, Release);
            slot.pid.store(0, Release);
        }
    }
}
