//! Windows: a synchronization waitable timer at an absolute system time, which follows clock
//! changes and fires once a resume passed it, and an event for a wake.
#![allow(unsafe_code)]
use crate::{Error, Result};
use std::{io, ptr, time::SystemTime};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::Threading::{
        CancelWaitableTimer, CreateEventW, CreateWaitableTimerW, INFINITE, ResetEvent, SetEvent, SetWaitableTimer,
        WaitForMultipleObjects,
    },
};

/// 100 ns intervals from 1601 to 1970.
const EPOCH: u64 = 116_444_736_000_000_000;

pub(super) struct Alarm {
    wake: HANDLE,
    timer: HANDLE,
}

// SAFETY: an event and a waitable timer may be set, waited on and closed from any thread.
unsafe impl Send for Alarm {}
// SAFETY: as above.
unsafe impl Sync for Alarm {}

impl Alarm {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: no attributes or name: an unnamed manual-reset event, unsignaled.
        let wake = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if wake.is_null() {
            return Err(Error::os("CreateEventW")(io::Error::last_os_error()));
        }
        // SAFETY: no attributes or name; a synchronization timer resets when a wait sees it.
        let timer = unsafe { CreateWaitableTimerW(ptr::null(), 0, ptr::null()) };
        if timer.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: the event just made, closed once.
            unsafe { CloseHandle(wake) };
            return Err(Error::os("CreateWaitableTimerW")(error));
        }
        Ok(Self { wake, timer })
    }

    pub(super) fn wait(&self, at: Option<SystemTime>) -> Result<bool> {
        match at {
            Some(at) => {
                let since = at.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
                // Positive: an absolute UTC time.
                let due = (EPOCH + (since.as_nanos() / 100) as u64).min(i64::MAX as u64) as i64;
                // SAFETY: a live timer and its due time; no period or completion routine.
                if unsafe { SetWaitableTimer(self.timer, &due, 0, None, ptr::null(), 0) } == 0 {
                    return Err(Error::os("SetWaitableTimer")(io::Error::last_os_error()));
                }
            }
            // SAFETY: a live timer.
            None => unsafe {
                CancelWaitableTimer(self.timer);
            },
        }
        let handles = [self.wake, self.timer];
        // SAFETY: two live handles, waited on for either.
        match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } {
            WAIT_OBJECT_0 => {
                // SAFETY: the event this owns.
                unsafe { ResetEvent(self.wake) };
                Ok(false)
            }
            w if w == WAIT_OBJECT_0 + 1 => Ok(true),
            _ => Err(Error::os("WaitForMultipleObjects")(io::Error::last_os_error())),
        }
    }

    pub(super) fn wake(&self) {
        // SAFETY: the event this owns.
        unsafe { SetEvent(self.wake) };
    }
}

impl Drop for Alarm {
    fn drop(&mut self) {
        // SAFETY: the handles this owns, each closed once.
        unsafe {
            CloseHandle(self.timer);
            CloseHandle(self.wake);
        }
    }
}
