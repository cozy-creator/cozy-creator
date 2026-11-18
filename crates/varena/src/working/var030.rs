//! PROOF ONLY (var-030; never merge): what a refused working allocation found. The last refusal's
//! figures ride varena's working stats into the run record (`stats`, names `var030_*`); stderr gets
//! the same as a few short lines, the head last (TensorD keeps a failed call's last 64 KiB of it):
//! the reclaim steps tried for the allocation in order (`reclaim::var030_take`), every working block
//! by life, the free bytes that could not go back because a live or pending block shares their chunk,
//! the free holes, and the resident weight regions.
use super::{Life, State};
use crate::Device;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static LAST: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());
const MOST: u64 = 200;
const STEPS: [&str; 10] = [
    "CompletedWorking", "Contentless", "IdlePools", "PendingWorking", "IdlePeers", "Weights", "Join", "OwnerCaches",
    "Peers", "HighWeights",
];
const RELEASED: [&str; 10] = ["var030_released_completed_working", "var030_released_contentless", "var030_released_idle_pools", "var030_released_pending_working", "var030_released_idle_peers", "var030_released_weights", "var030_released_join", "var030_released_owner_caches", "var030_released_peers", "var030_released_high_weights"];
const EVICTED: [&str; 10] = ["var030_evicted_completed_working", "var030_evicted_contentless", "var030_evicted_idle_pools", "var030_evicted_pending_working", "var030_evicted_idle_peers", "var030_evicted_weights", "var030_evicted_join", "var030_evicted_owner_caches", "var030_evicted_peers", "var030_evicted_high_weights"];
const RUNS: [&str; 10] = ["var030_runs_completed_working", "var030_runs_contentless", "var030_runs_idle_pools", "var030_runs_pending_working", "var030_runs_idle_peers", "var030_runs_weights", "var030_runs_join", "var030_runs_owner_caches", "var030_runs_peers", "var030_runs_high_weights"];
const MICROS: [&str; 10] = ["var030_us_completed_working", "var030_us_contentless", "var030_us_idle_pools", "var030_us_pending_working", "var030_us_idle_peers", "var030_us_weights", "var030_us_join", "var030_us_owner_caches", "var030_us_peers", "var030_us_high_weights"];

/// Each working allocation asked of this process: the largest is kept.
pub(super) fn asked(bytes: usize) {
    LARGEST.fetch_max(bytes, Ordering::Relaxed);
}

/// The largest allocation asked, the refusals so far, and the last refusal's figures.
pub(super) fn stats() -> Vec<(&'static str, u64)> {
    let mut out = LAST.lock().map(|last| last.clone()).unwrap_or_default();
    out.push(("var030_largest_allocation", LARGEST.load(Ordering::Relaxed) as u64));
    out.push(("var030_refusals", SEQUENCE.load(Ordering::Relaxed)));
    out
}

#[derive(Default)]
struct Seg {
    live: usize,
    pending: usize,
    free: usize,
    blocks: usize,
    used: Vec<usize>,
    freed: Vec<usize>,
}

pub(super) fn dump(owner: &Device, state: &State, requested: usize, class: usize, exact: usize, stream: u64, looping: bool) -> Vec<String> {
    let steps = crate::reclaim::var030_take();
    let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let tag = format!("varena-var030 #{n}");
    let mut out: Vec<String> = Vec::new();
    let mut last: Vec<(&'static str, u64)> = Vec::new();

    // count, capacity, requested: live, live scratch, free, free on one stream only, pending, captured, unjoined
    let mut lives = [(0usize, 0usize, 0usize); 7];
    let mut segs: BTreeMap<u64, Seg> = BTreeMap::new();
    let mut holes: Vec<usize> = Vec::new();
    let mut largest_live: Vec<(usize, usize, u64)> = Vec::new();
    for (&ptr, block) in &state.blocks {
        let at = match block.life {
            Life::Live if block.scratch => 1,
            Life::Live => 0,
            Life::Ready if block.reusable_on.is_none() => 2,
            Life::Ready => 3,
            Life::Pending => 4,
            Life::Captured => 5,
            Life::Unjoined(_) => 6,
        };
        lives[at].0 += 1;
        lives[at].1 += block.capacity;
        lives[at].2 += block.requested;
        if at == 0 {
            largest_live.push((block.capacity, block.requested, block.stream));
        }
        let Some(segment) = state.segments.get(&block.segment) else { continue };
        if segment.pool.is_some() {
            continue;
        }
        let seg = segs.entry(block.segment).or_insert_with(|| Seg {
            used: vec![0; segment.chunks.len()],
            freed: vec![0; segment.chunks.len()],
            ..Seg::default()
        });
        seg.blocks += 1;
        let free = block.life == Life::Ready;
        match block.life {
            Life::Live => seg.live += block.capacity,
            Life::Ready => seg.free += block.capacity,
            _ => seg.pending += block.capacity,
        }
        let start = ptr.saturating_sub(block.segment) as usize;
        let end = start + block.capacity;
        let mut backed = 0usize;
        for (index, chunk) in segment.chunks.iter().enumerate() {
            let overlap = end.min(chunk.offset + chunk.bytes).saturating_sub(start.max(chunk.offset));
            if overlap == 0 {
                continue;
            }
            if free {
                seg.freed[index] += overlap;
                if chunk.handle.is_some() {
                    backed += overlap;
                }
            } else {
                seg.used[index] += overlap;
            }
        }
        if free && backed != 0 {
            holes.push(backed);
        }
    }

    let (mut backed_bytes, mut returnable, mut stuck, mut stuck_chunks, mut pool_bytes) = (0usize, 0usize, 0usize, 0usize, 0usize);
    // mapped, live, pending, free, free beside used, blocks, bytes, address
    let mut rows: Vec<(usize, usize, usize, usize, usize, usize, usize, u64)> = Vec::new();
    for (address, segment) in &state.segments {
        if segment.pool.is_some() {
            if segment.backed() {
                pool_bytes += segment.bytes;
            }
            continue;
        }
        let empty = Seg { used: vec![0; segment.chunks.len()], freed: vec![0; segment.chunks.len()], ..Seg::default() };
        let seg = segs.get(address).unwrap_or(&empty);
        let (mut mapped, mut whole, mut shared) = (0usize, 0usize, 0usize);
        for (index, chunk) in segment.chunks.iter().enumerate() {
            if chunk.handle.is_none() {
                continue;
            }
            mapped += chunk.bytes;
            if seg.used[index] == 0 {
                whole += chunk.bytes;
            } else if seg.freed[index] != 0 {
                shared += seg.freed[index];
                stuck_chunks += 1;
            }
        }
        backed_bytes += mapped;
        returnable += whole;
        stuck += shared;
        if mapped != 0 {
            rows.push((mapped, seg.live, seg.pending, seg.free, shared, seg.blocks, segment.bytes, *address));
        }
    }
    holes.sort_unstable_by(|a, b| b.cmp(a));
    largest_live.sort_unstable_by(|a, b| b.cmp(a));

    let (mut idle, mut idle_bytes, mut busy, mut busy_bytes, mut cold) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut resident: Vec<(usize, u64, usize, bool, bool, bool, bool, bool)> = Vec::new();
    for (id, entry) in &state.entries {
        if entry.handle.is_none() {
            cold += 1;
            continue;
        }
        let free = entry.idle();
        if free {
            idle += 1;
            idle_bytes += entry.bytes;
        } else {
            busy += 1;
            busy_bytes += entry.bytes;
        }
        resident.push((entry.bytes, *id, entry.uses.pins(), free, entry.prefetched, entry.high, entry.retained, entry.packed.is_some()));
    }
    resident.sort_unstable_by(|a, b| b.cmp(a));

    // The record's figures.
    let (mut released, mut evicted, mut runs, mut micros, mut tries) = ([0u64; 10], [0u64; 10], [0u64; 10], [0u64; 10], 0u64);
    for (step, _, gave, went, ns, _) in &steps {
        if *step == 251 {
            tries += 1;
        } else if (*step as usize) < 10 {
            let at = *step as usize;
            released[at] += *gave as u64;
            evicted[at] += *went as u64;
            runs[at] += 1;
            micros[at] += *ns / 1000;
        }
    }
    for at in 0..10 {
        last.push((RELEASED[at], released[at]));
        last.push((EVICTED[at], evicted[at]));
        last.push((RUNS[at], runs[at]));
        last.push((MICROS[at], micros[at]));
    }
    last.push(("var030_requested", requested as u64));
    last.push(("var030_exact", exact as u64));
    last.push(("var030_tries", tries));
    last.push(("var030_step_rows", steps.len() as u64));
    last.push(("var030_capacity", owner.capacity() as u64));
    last.push(("var030_owned", owner.owned() as u64));
    last.push(("var030_weight_pages", owner.pages.load(Ordering::Acquire) as u64));
    last.push(("var030_working_mapped", owner.working.load(Ordering::Acquire) as u64));
    last.push(("var030_pinned", owner.held.load(Ordering::Acquire) as u64));
    last.push(("var030_outside", owner.outside.load(Ordering::Acquire) as u64));
    last.push(("var030_over", owner.over(exact) as u64));
    last.push(("var030_live_blocks", lives[0].0 as u64));
    last.push(("var030_live_bytes", lives[0].1 as u64));
    last.push(("var030_live_requested", lives[0].2 as u64));
    last.push(("var030_scratch_bytes", lives[1].1 as u64));
    last.push(("var030_free_blocks", lives[2].0 as u64));
    last.push(("var030_free_bytes", lives[2].1 as u64));
    last.push(("var030_free_one_stream_bytes", lives[3].1 as u64));
    last.push(("var030_pending_blocks", lives[4].0 as u64));
    last.push(("var030_pending_bytes", lives[4].1 as u64));
    last.push(("var030_captured_bytes", lives[5].1 as u64));
    last.push(("var030_unjoined_bytes", lives[6].1 as u64));
    last.push(("var030_eager_mapped", backed_bytes as u64));
    last.push(("var030_graph_pools", pool_bytes as u64));
    last.push(("var030_free_in_whole_chunks", returnable as u64));
    last.push(("var030_free_beside_used", stuck as u64));
    last.push(("var030_free_beside_used_chunks", stuck_chunks as u64));
    last.push(("var030_holes", holes.len() as u64));
    last.push(("var030_hole_bytes", holes.iter().sum::<usize>() as u64));
    last.push(("var030_largest_hole", holes.first().copied().unwrap_or(0) as u64));
    last.push(("var030_largest_live_block", largest_live.first().map_or(0, |row| row.0) as u64));
    last.push(("var030_weights_idle", idle as u64));
    last.push(("var030_weights_idle_bytes", idle_bytes as u64));
    last.push(("var030_weights_held", busy as u64));
    last.push(("var030_weights_held_bytes", busy_bytes as u64));
    last.push(("var030_weights_cold", cold as u64));
    if let Ok(mut kept) = LAST.lock() {
        *kept = last;
    }
    if n >= MOST {
        return out;
    }

    // stderr: the long lines first, the head last.
    rows.sort_unstable_by(|a, b| b.cmp(a));
    let mut line = String::new();
    for (mapped, live, pending, free, shared, blocks, bytes, address) in rows.iter().take(16) {
        line.push_str(&format!(" [{address:x} bytes={bytes} mapped={mapped} live={live} pending={pending} free={free} free_beside_used={shared} blocks={blocks}]"));
    }
    out.push(format!("{tag} segments ({} mapped, the largest){line}", rows.len()));
    let mut line = String::new();
    for (bytes, id, pins, free, prefetched, high, retained, packed) in resident.iter().take(20) {
        line.push_str(&format!(
            " {bytes}:id{id}:pins{pins}:{}{}{}{}{}",
            if *free { "idle" } else { "held" },
            if *prefetched { ":prefetched" } else { "" },
            if *high { ":high" } else { "" },
            if *retained { ":retained" } else { "" },
            if *packed { ":packed" } else { "" },
        ));
    }
    out.push(format!("{tag} weights resident_idle={idle}/{idle_bytes} resident_held={busy}/{busy_bytes} cold={cold} largest:{line}"));
    let mut line = String::new();
    for (capacity, asked, on) in largest_live.iter().take(24) {
        line.push_str(&format!(" {capacity}/{asked}/s{on}"));
    }
    out.push(format!("{tag} live_largest (capacity/requested/stream){line} ({} live)", largest_live.len()));
    let names = ["live", "scratch", "free", "free_one_stream", "pending", "captured", "unjoined"];
    let mut line = String::new();
    for (name, (count, capacity, asked)) in names.iter().zip(lives.iter()) {
        line.push_str(&format!(" {name}={count}/{capacity}/{asked}"));
    }
    out.push(format!("{tag} blocks (count/capacity/requested){line}"));
    let mut line = String::new();
    for hole in holes.iter().take(16) {
        line.push_str(&format!(" {hole}"));
    }
    out.push(format!(
        "{tag} fragmentation eager_mapped={backed_bytes} graph_pools={pool_bytes} free_in_wholly_free_chunks={returnable} \
         free_beside_live_or_pending={stuck} in_chunks={stuck_chunks} holes={} hole_bytes={} largest:{line}",
        holes.len(),
        holes.iter().sum::<usize>(),
    ));
    let shown = &steps[steps.len().saturating_sub(60)..];
    let mut line = String::new();
    for (step, want, gave, went, ns, progress) in shown.iter() {
        if *step == 251 {
            line.push_str(&format!(" Try:need={gave}:want={want}:{}", if *progress { "physical" } else { "capped" }));
        } else {
            let name = if *step == 250 { "StopAhead" } else { STEPS.get(*step as usize).copied().unwrap_or("?") };
            line.push_str(&format!(" {name}:released={gave}:evicted={went}:us={}:{}", ns / 1000, if *progress { "progress" } else { "none" }));
        }
    }
    out.push(format!("{tag} steps ({} shown of {}){line}", shown.len(), steps.len()));
    out.push(format!(
        "{tag} head requested={requested} class={class} exact={exact} stream={stream} looping={looping} capacity={} \
         total={} ceiling={} reservation={} outside={} owned={} weight_pages={} working_mapped={} pinned={} over={}",
        owner.capacity(),
        owner.total.load(Ordering::Acquire),
        owner.ceiling.load(Ordering::Acquire),
        owner.reservation.get().copied().unwrap_or(0),
        owner.outside.load(Ordering::Acquire),
        owner.owned(),
        owner.pages.load(Ordering::Acquire),
        owner.working.load(Ordering::Acquire),
        owner.held.load(Ordering::Acquire),
        owner.over(exact),
    ));
    out
}
