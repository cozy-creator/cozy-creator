//! Diagnostic-only pool scan/report. No CUDA work or allocation-policy decision.
use super::{Block, Life, PoolId, State};
use std::fmt::Write as _;
use std::io::Write as _;

pub(super) fn eligibility(block: &Block, stream: u64) -> (bool, bool) {
    let ready = block.life == Life::Ready && block.reusable_on.is_none();
    let ordered = block.reusable_on == Some(stream)
        && (matches!(block.life, Life::Ready | Life::Captured) || block.deferred_default());
    (ready, ordered)
}

/// The original fit scan, in its original segment/block order. The actual selector
/// and the failed-fit report both use this exact predicate and contiguous-run walk.
pub(super) fn runs(state: &State, pool: PoolId, stream: u64, mut consider: impl FnMut(u64, usize, u64, bool)) {
    for (base, segment) in &state.segments {
        if segment.pool != Some(pool) { continue; }
        let (mut start, mut length, mut complete) = (*base, 0usize, true);
        for (ptr, block) in state.blocks.range(*base..*base + segment.bytes as u64) {
            let (ready, ordered) = eligibility(block, stream);
            if !ready && !ordered {
                consider(start, length, *base, complete);
                length = 0;
                continue;
            }
            if length != 0 && *ptr != start + length as u64 {
                consider(start, length, *base, complete);
                length = 0;
            }
            if length == 0 { start = *ptr; complete = true; }
            length += block.capacity;
            complete &= ready;
        }
        consider(start, length, *base, complete);
    }
}

fn life(value: Life) -> &'static str {
    match value {
        Life::Live => "live", Life::Ready => "ready", Life::Pending => "pending",
        Life::Deferred => "deferred", Life::Captured => "captured", Life::Unjoined => "unjoined",
    }
}

pub(super) fn emit(state: &State, device: i32, pool: PoolId, class_bytes: usize, requested: usize, stream: u64) {
    let (mut eligible, mut largest) = (0usize, 0usize);
    runs(state, pool, stream, |_, bytes, _, _| { eligible += bytes; largest = largest.max(bytes); });
    let (mut mapped, mut live, mut blocked, mut count) = (0usize, 0usize, 0usize, 0usize);
    let mut ranges = String::new();
    const MAX_RANGES: usize = 64;
    for (base, segment) in &state.segments {
        if segment.pool != Some(pool) { continue; }
        mapped += segment.bytes;
        for (ptr, block) in state.blocks.range(*base..*base + segment.bytes as u64) {
            let (ready, ordered) = eligibility(block, stream);
            if block.life == Life::Live { live += block.capacity; }
            if !ready && !ordered { blocked += block.capacity; }
            if count < MAX_RANGES {
                if count != 0 { ranges.push(','); }
                let reuse = block.reusable_on.map_or_else(|| "null".to_owned(), |value| value.to_string());
                let _ = write!(ranges, "{{\"address\":{ptr},\"segment\":{base},\"capacity\":{},\"requested\":{},\"life\":\"{}\",\"ready\":{ready},\"ordered\":{ordered},\"origin_stream\":{},\"origin_retired\":{},\"reusable_on\":{reuse},\"consumer_streams\":[",
                    block.capacity, block.requested, life(block.life), block.stream, block.origin_retired);
                for (index, consumer) in block.streams.iter().take(16).enumerate() {
                    if index != 0 { ranges.push(','); }
                    let _ = write!(ranges, "{consumer}");
                }
                let _ = write!(ranges, "],\"consumer_stream_count\":{},\"event_count\":{}}}", block.streams.len(), block.events.len());
            }
            count += 1;
        }
    }
    let session = &state.pools[&pool];
    let mut output = String::new();
    let _ = write!(output, "VARENA_POOL_FIT_FAILURE_V1 {{\"device\":{device},\"pool\":[{},{}],\"request_stream\":{stream},\"requested_bytes\":{requested},\"class_bytes\":{class_bytes},\"mapped_pool_bytes\":{mapped},\"eligible_total_bytes\":{eligible},\"largest_eligible_run_bytes\":{largest},\"live_bytes\":{live},\"blocked_including_live_bytes\":{blocked},\"routing\":{},\"prepared\":{},\"references\":{},\"native_capture_guard\":{},\"pool_streams\":[",
        pool.0, pool.1, session.routing, session.prepared, session.references, session.capture.is_some());
    for (index, consumer) in session.streams.iter().take(16).enumerate() {
        if index != 0 { output.push(','); }
        let _ = write!(output, "{consumer}");
    }
    let _ = write!(output, "],\"pool_stream_count\":{},\"ranges_total\":{count},\"ranges_truncated\":{},\"ranges\":[{ranges}]}}\n",
        session.streams.len(), count > MAX_RANGES);
    // Preserve the original allocator failure even if the diagnostic sink refuses.
    // The caller still holds the same Working lock; no driver query or retry occurs.
    let _ = std::io::stderr().lock().write_all(output.as_bytes());
}
