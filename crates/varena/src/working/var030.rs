//! PROOF ONLY (var-030; never merge): what a refused working allocation found, as stderr lines:
//! the reclaim steps tried for it in order (`reclaim::var030_take`), the card's accounting, every
//! working block by life, each eager segment's chunks (free bytes that could not go back because a
//! live or pending block shares their chunk), the free holes, and the resident weight regions.
use super::{Life, State};
use crate::Device;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MOST: u64 = 200;
const STEPS: [&str; 10] = [
    "CompletedWorking", "Contentless", "IdlePools", "PendingWorking", "IdlePeers", "Weights", "Join", "OwnerCaches",
    "Peers", "HighWeights",
];

#[derive(Default)]
struct Seg {
    live: usize,
    pending: usize,
    free: usize,
    largest: usize,
    blocks: usize,
    used: Vec<usize>,
    freed: Vec<usize>,
}

pub(super) fn dump(owner: &Device, state: &State, requested: usize, class: usize, exact: usize, stream: u64, looping: bool) -> Vec<String> {
    let steps = crate::reclaim::var030_take();
    let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    if n >= MOST {
        return Vec::new();
    }
    let tag = format!("varena-var030 #{n}");
    let mut out = Vec::new();
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

    let shown = &steps[steps.len().saturating_sub(240)..];
    let mut line = String::new();
    for (index, (step, want, released, evicted, ns, progress)) in shown.iter().enumerate() {
        if *step == 251 {
            line.push_str(&format!(" Try:need={released}:want={want}:{}", if *progress { "physical" } else { "capped" }));
        } else {
            let name = if *step == 250 { "StopAhead" } else { STEPS.get(*step as usize).copied().unwrap_or("?") };
            line.push_str(&format!(
                " {name}:want={want}:released={released}:evicted={evicted}:us={}:{}",
                ns / 1000,
                if *progress { "progress" } else { "none" },
            ));
        }
        if (index + 1) % 10 == 0 {
            out.push(format!("{tag} steps{line}"));
            line.clear();
        }
    }
    out.push(format!("{tag} steps{line} ({} shown of {})", shown.len(), steps.len()));

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
            Life::Ready => {
                seg.free += block.capacity;
                seg.largest = seg.largest.max(block.capacity);
            }
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
    let names = ["live", "scratch", "free", "free_one_stream", "pending", "captured", "unjoined"];
    let mut line = String::new();
    for (name, (count, capacity, asked)) in names.iter().zip(lives.iter()) {
        line.push_str(&format!(" {name}={count}/{capacity}/{asked}"));
    }
    out.push(format!("{tag} blocks (count/capacity/requested){line}"));
    largest_live.sort_unstable_by(|a, b| b.cmp(a));
    let mut line = String::new();
    for (capacity, asked, on) in largest_live.iter().take(40) {
        line.push_str(&format!(" {capacity}/{asked}/s{on}"));
    }
    out.push(format!("{tag} live_largest (capacity/requested/stream){line} ({} live)", largest_live.len()));

    let (mut backed_bytes, mut returnable, mut stuck, mut stuck_chunks, mut pool_bytes) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut line = String::new();
    let mut printed = 0usize;
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
        if mapped == 0 {
            continue;
        }
        if printed < 400 {
            line.push_str(&format!(
                " [{:x} bytes={} class={} mapped={} live={} pending={} free={} largest_free={} blocks={} free_whole_chunks={} free_beside_used={}]",
                address, segment.bytes, segment.size_class, mapped, seg.live, seg.pending, seg.free, seg.largest, seg.blocks, whole, shared,
            ));
            printed += 1;
            if printed % 6 == 0 {
                out.push(format!("{tag} segments{line}"));
                line.clear();
            }
        }
    }
    if !line.is_empty() {
        out.push(format!("{tag} segments{line}"));
    }
    holes.sort_unstable_by(|a, b| b.cmp(a));
    let mut line = String::new();
    for hole in holes.iter().take(24) {
        line.push_str(&format!(" {hole}"));
    }
    out.push(format!(
        "{tag} fragmentation eager_mapped={backed_bytes} graph_pools={pool_bytes} free_in_wholly_free_chunks={returnable} \
         free_beside_live_or_pending={stuck} in_chunks={stuck_chunks} holes={} hole_bytes={} largest:{line}",
        holes.len(),
        holes.iter().sum::<usize>(),
    ));

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
    let mut line = String::new();
    for (bytes, id, pins, free, prefetched, high, retained, packed) in resident.iter().take(32) {
        line.push_str(&format!(
            " {bytes}:id{id}:pins{pins}:{}{}{}{}{}",
            if *free { "idle" } else { "held" },
            if *prefetched { ":prefetched" } else { "" },
            if *high { ":high" } else { "" },
            if *retained { ":retained" } else { "" },
            if *packed { ":packed" } else { "" },
        ));
    }
    out.push(format!(
        "{tag} weights resident_idle={idle}/{idle_bytes} resident_held={busy}/{busy_bytes} cold={cold} largest:{line}"
    ));
    out
}
