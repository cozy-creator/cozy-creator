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

// mimalloc's own, which the mimalloc crate doesn't wrap. Nothing here names libmimalloc-sys's Rust items, so it's named to
// link its C library.
#[cfg(feature = "allocator")]
use libmimalloc_sys as _;
#[cfg(feature = "allocator")]
unsafe extern "C" {
    fn mi_collect(force: bool);
    fn mi_process_info(
        elapsed_msecs: *mut usize,
        user_msecs: *mut usize,
        system_msecs: *mut usize,
        current_rss: *mut usize,
        peak_rss: *mut usize,
        current_commit: *mut usize,
        peak_commit: *mut usize,
        page_faults: *mut usize,
    );
}

/// Gives the OS back what the allocators keep of freed memory: mimalloc's empty pages of this thread and every arena's freed
/// pages, at once, and glibc's free pages, where C code (SQLite) allocates. Left alone, mimalloc purges a second after a free
/// and only when some thread next allocates, and glibc never trims its threads' arenas, so an idle process stays as large as
/// its last peak (cl-870). For when much was just freed.
#[cfg(feature = "allocator")]
pub fn give_back() {
    // SAFETY: takes no pointer; any thread may call it.
    unsafe { mi_collect(true) }
    trim();
}

/// glibc's free pages back to the OS, from every arena.
#[cfg(all(feature = "allocator", target_os = "linux", target_env = "gnu"))]
fn trim() {
    // SAFETY: takes no pointer; any thread may call it.
    unsafe { libc::malloc_trim(0) };
}
#[cfg(all(feature = "allocator", not(all(target_os = "linux", target_env = "gnu"))))]
fn trim() {}

/// For a thread going idle (a tokio worker parking): its own freed pages back, which no other thread can collect for it, at
/// most once a second, so a busy runtime parking thousands of times a second pays a clock read (cl-870).
#[cfg(feature = "allocator")]
pub fn parked() {
    thread_local! {
        static LAST: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
    }
    let now = std::time::Instant::now();
    let due = LAST
        .try_with(|last| {
            let due = last.get().is_none_or(|at| now.duration_since(at) >= std::time::Duration::from_secs(1));
            if due {
                last.set(Some(now));
            }
            due
        })
        .unwrap_or(false);
    if due {
        // SAFETY: takes no pointer; it collects the calling thread's own pages.
        unsafe { mi_collect(false) }
    }
}

/// Bytes mimalloc holds of the OS's memory: what's in use, and what's freed and not given back.
#[cfg(feature = "allocator")]
pub fn committed() -> u64 {
    let [mut elapsed, mut user, mut system, mut rss, mut peak_rss, mut commit, mut peak_commit, mut faults] = [0usize; 8];
    // SAFETY: each pointer is to a local, which the call writes once.
    unsafe {
        mi_process_info(&mut elapsed, &mut user, &mut system, &mut rss, &mut peak_rss, &mut commit, &mut peak_commit, &mut faults)
    };
    commit as u64
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
