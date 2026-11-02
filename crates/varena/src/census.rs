//! Private same-cohort scalar diagnostics. No CUDA, completion inference, or reclamation.
use crate::{Contents, Result, State, arena_inventory, plan};
use std::sync::atomic::Ordering;

#[derive(Debug)]
pub struct RegionEntry {
    pub id: u64,
    pub requested_bytes: usize,
    pub reserved_bytes: usize,
    pub mapped_bytes: usize,
    pub contents: &'static str,
    pub pins: usize,
    pub unjoined: bool,
}

#[derive(Debug)]
pub struct ArenaSnapshot {
    pub id: u64,
    pub granularity: usize,
    pub inventory_generation_before: u64,
    pub inventory_generation_after: u64,
    pub fills: u64,
    pub hits: u64,
    pub evictions: u64,
    pub entries: Vec<RegionEntry>,
}

#[derive(Debug)]
pub struct FinalizingArena {
    pub id: u64,
    pub physical_bytes: usize,
}

#[derive(Debug)]
pub struct RegionHandle {
    pub handle: u64,
    pub arena_id: u64,
    pub region_id: u64,
}

#[derive(Debug)]
pub struct Snapshot {
    pub device: i32,
    pub anchor_arena_id: u64,
    pub inventory_generation_before: u64,
    pub inventory_generation_after: u64,
    pub arenas: Vec<ArenaSnapshot>,
    pub finalizing: Vec<FinalizingArena>,
    pub region_handles: Vec<RegionHandle>,
}

fn entries(state: &State) -> Vec<RegionEntry> {
    state
        .entries
        .iter()
        .map(|(&id, entry)| RegionEntry {
            id,
            requested_bytes: entry.requested_bytes,
            reserved_bytes: entry.bytes,
            mapped_bytes: if entry.handle.is_some() {
                entry.bytes
            } else {
                0
            },
            contents: match entry.uses.state() {
                Contents::Cold => "cold",
                Contents::Loading => "loading",
                Contents::Published => "published",
                Contents::Ready => "ready",
                Contents::Retired => "retired",
            },
            pins: entry.uses.pins.load(Ordering::Acquire),
            unjoined: entry.uses.unjoined.load(Ordering::Acquire),
        })
        .collect()
}

pub(crate) fn snapshot(device: i32, anchor_arena_id: u64) -> Result<Snapshot> {
    crate::check_process()?;
    let before = plan::inventory_generation(device)?;
    // Own the inventory before taking any arena lock. All guards drop before these
    // temporary Arcs, including on error. Existing Arena::drop transfers resources
    // to native retirement; it never tears down CUDA on this observer's stack.
    let inventory = arena_inventory(device)?;
    let mut arenas = Vec::with_capacity(inventory.arenas.len());
    for arena in &inventory.arenas {
        let arena_before = plan::inventory_generation(device)?;
        let (fills, hits, evictions, entries) = {
            let state = arena.lock()?;
            (state.fills, state.hits, state.evictions, entries(&state))
        };
        arenas.push(ArenaSnapshot {
            id: arena.id(),
            granularity: arena.granularity(),
            inventory_generation_before: arena_before,
            inventory_generation_after: plan::inventory_generation(device)?,
            fills,
            hits,
            evictions,
            entries,
        });
    }
    let finalizing = inventory
        .finalizing
        .iter()
        .map(|metadata| FinalizingArena {
            id: metadata.id,
            physical_bytes: metadata.physical.load(Ordering::Acquire),
        })
        .collect();
    // A last temporary Arc may publish a finalizing handoff. Observe that epoch
    // after releasing inventory, outside every arena/registry lock.
    drop(inventory);
    Ok(Snapshot {
        device,
        anchor_arena_id,
        inventory_generation_before: before,
        inventory_generation_after: plan::inventory_generation(device)?,
        arenas,
        finalizing,
        region_handles: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entry, Uses};
    use std::sync::Arc;

    #[test]
    fn retired_mapping_keeps_logical_size_without_a_region_owner() {
        let mut state = State::empty();
        let uses = Arc::new(Uses::new());
        uses.set(Contents::Retired);
        state.entries.insert(
            7,
            Entry {
                ptr: 0,
                requested_bytes: 13,
                bytes: 2 << 20,
                handle: Some(1),
                uses,
                preferred: false,
                last_use: 0,
            },
        );
        let rows = entries(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 7);
        assert_eq!(rows[0].requested_bytes, 13);
        assert_eq!(rows[0].reserved_bytes, 2 << 20);
        assert_eq!(rows[0].mapped_bytes, 2 << 20);
        assert_eq!(rows[0].contents, "retired");
        assert_eq!(rows[0].pins, 0);
        assert!(!rows[0].unjoined);
        state.entries.get_mut(&7).unwrap().handle = None;
        let rows = entries(&state);
        assert_eq!(rows[0].mapped_bytes, 0);
        assert_eq!(rows[0].requested_bytes, 13);
        assert_eq!(rows[0].reserved_bytes, 2 << 20);
    }
}
