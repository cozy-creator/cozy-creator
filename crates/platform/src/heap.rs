//! What this process's code holds: the bytes it has allocated and not freed, counted around its allocator. Unlike resident memory, it
//! doesn't count what an allocator keeps of what was freed, or what it gives back late (cl-826).
#![allow(unsafe_code)]
use std::{
    alloc::{GlobalAlloc, Layout},
    cell::Cell,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering::Relaxed},
};

/// The allocator `A`, counting. Each thread adds to a counter of its own, one of `SHARDS`, so no allocation waits on another thread's.
pub struct Counted<A>(pub A);

const SHARDS: usize = 64;

#[repr(align(128))]
struct Shard(AtomicIsize);

static HELD: [Shard; SHARDS] = [const { Shard(AtomicIsize::new(0)) }; SHARDS];
static THREADS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // Const and with nothing to drop, so it never allocates.
    static SHARD: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// Bytes allocated through [`Counted`] and not yet freed, by every thread.
pub fn held() -> u64 {
    HELD.iter().map(|shard| shard.0.load(Relaxed)).sum::<isize>().max(0) as u64
}

fn count(bytes: isize) {
    let shard = SHARD.try_with(|shard| {
        if shard.get() == usize::MAX {
            shard.set(THREADS.fetch_add(1, Relaxed) % SHARDS);
        }
        shard.get()
    });
    HELD[shard.unwrap_or(0)].0.fetch_add(bytes, Relaxed);
}

// SAFETY: each call is `A`'s, with the caller's arguments; counting touches no memory it hands out.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Counted<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as the caller's call.
        let at = unsafe { self.0.alloc(layout) };
        if !at.is_null() {
            count(layout.size() as isize);
        }
        at
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as the caller's call.
        let at = unsafe { self.0.alloc_zeroed(layout) };
        if !at.is_null() {
            count(layout.size() as isize);
        }
        at
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as the caller's call.
        unsafe { self.0.dealloc(ptr, layout) };
        count(-(layout.size() as isize));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as the caller's call.
        let at = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !at.is_null() {
            count(new_size as isize - layout.size() as isize);
        }
        at
    }
}
