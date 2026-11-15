#![allow(unsafe_code)]
use crate::{Error, Result};
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::Foundation::{HANDLE, WAIT_ABANDONED_0, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};

type Owned = OwnedHandle;

fn last(op: &'static str) -> Error {
    Error::os(op)(std::io::Error::last_os_error())
}

fn raw(h: &Owned) -> HANDLE {
    h.as_raw_handle() as HANDLE
}

/// What [`wait_any`] waits on: an event, a process, any waitable handle.
pub type Waitable<'a> = BorrowedHandle<'a>;

/// A local wake one process sets and its own threads wait on: a manual-reset event, set until
/// cleared.
pub struct Signal(Owned);

impl Signal {
    pub fn new() -> Result<Signal> {
        // SAFETY: an unnamed manual-reset event, not set, with default security.
        let h = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if h.is_null() {
            return Err(last("create a signal"));
        }
        // SAFETY: a handle we just made.
        Ok(Signal(unsafe { Owned::from_raw_handle(h as _) }))
    }

    pub fn set(&self) {
        // SAFETY: an event we hold.
        unsafe { SetEvent(raw(&self.0)) };
    }

    /// Clears it; true when it was set.
    pub fn clear(&self) -> bool {
        // SAFETY: an event we hold; a zero wait only reads it.
        let set = unsafe { WaitForSingleObject(raw(&self.0), 0) } == WAIT_OBJECT_0;
        // SAFETY: as above.
        unsafe { ResetEvent(raw(&self.0)) };
        set
    }

    pub fn waitable(&self) -> Waitable<'_> {
        self.0.as_handle()
    }
}

/// Blocks until one of `items` is signalled; the lowest such index. No timeout.
pub fn wait_any(items: &[Waitable<'_>]) -> Result<Vec<super::Ready>> {
    wait_any_for(items, None)
}

/// [`wait_any`], or none ready once `timeout` passed.
pub fn wait_any_for(items: &[Waitable<'_>], timeout: Option<std::time::Duration>) -> Result<Vec<super::Ready>> {
    let handles: Vec<HANDLE> = items.iter().map(|h| h.as_raw_handle() as HANDLE).collect();
    let ms = timeout.map_or(INFINITE, |timeout| u32::try_from(timeout.as_millis()).unwrap_or(INFINITE - 1));
    if handles.is_empty() {
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
        return Ok(vec![]);
    }
    // SAFETY: live handles the caller borrows for the call, and their count.
    let woke = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, ms) };
    if woke == WAIT_TIMEOUT {
        return Ok(vec![]);
    }
    let n = handles.len() as u32;
    let first = match woke {
        w if (WAIT_OBJECT_0..WAIT_OBJECT_0 + n).contains(&w) => (w - WAIT_OBJECT_0) as usize,
        w if (WAIT_ABANDONED_0..WAIT_ABANDONED_0 + n).contains(&w) => (w - WAIT_ABANDONED_0) as usize,
        _ => return Err(last("wait")),
    };
    // The answer names the lowest only: every later one signalled now is ready too.
    // SAFETY: as above, one handle at a time, not waiting.
    let later = (first + 1..handles.len()).filter(|&i| unsafe { WaitForSingleObject(handles[i], 0) } == WAIT_OBJECT_0);
    Ok(std::iter::once(first).chain(later).map(|index| super::Ready { index, broken: false }).collect())
}
