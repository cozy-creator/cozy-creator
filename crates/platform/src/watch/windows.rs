//! Windows: a change notification on each directory for names (and, where writes count, sizes
//! and last writes), and an event for a wake. NTFS reports a write through a handle held open
//! only when its cache flushes: at once after a sync or a close (measured, microseconds), not
//! within 10 s otherwise. Names aren't reported, so any counted write wakes.
#![allow(unsafe_code)]
use super::{Dir, Writes};
use crate::{Error, Result};
use std::{io, os::windows::ffi::OsStrExt, ptr, sync::Arc, time::Duration};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FindCloseChangeNotification, FindFirstChangeNotificationW,
        FindNextChangeNotification,
    },
    System::Threading::{CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects},
};

pub(super) struct Watch {
    wake: Arc<Wake>,
    /// Each directory and its notification, while it has one.
    dirs: Vec<(Dir, Option<Change>)>,
}

/// A manual-reset event: a wait that sees it resets it.
pub(super) struct Wake(HANDLE);

struct Change(HANDLE);

// SAFETY: an event or a change-notification handle may be signaled, waited on and closed from any thread.
unsafe impl Send for Wake {}
// SAFETY: as above.
unsafe impl Sync for Wake {}
// SAFETY: as above.
unsafe impl Send for Change {}

impl Watch {
    pub(super) fn new(dirs: Vec<Dir>) -> Result<Self> {
        // SAFETY: no attributes or name: an unnamed manual-reset event, unsignaled.
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(Error::os("CreateEventW")(io::Error::last_os_error()));
        }
        let mut watch = Self { wake: Arc::new(Wake(event)), dirs: dirs.into_iter().map(|dir| (dir, None)).collect() };
        if let Err(error) = watch.arm() {
            return Err(Error::io("watch", &watch.dirs[0].0.path)(error));
        }
        Ok(watch)
    }

    /// Asks a notification of each directory that has none; the first's failure is said.
    fn arm(&mut self) -> io::Result<()> {
        let mut first = Ok(());
        for (at, (dir, change)) in self.dirs.iter_mut().enumerate() {
            if change.is_some() {
                continue;
            }
            let mut changes = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME;
            if !matches!(dir.writes, Writes::None) {
                changes |= FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;
            }
            let wide: Vec<u16> = dir.path.as_os_str().encode_wide().chain([0]).collect();
            // SAFETY: a NUL-terminated wide path; the handle is closed on drop.
            let handle = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 0, changes) };
            if handle == INVALID_HANDLE_VALUE {
                if at == 0 {
                    first = Err(io::Error::last_os_error());
                }
                continue;
            }
            *change = Some(Change(handle));
        }
        first
    }

    pub(super) fn wait(&mut self, most: Option<Duration>) -> Result<bool> {
        let milliseconds = most.map_or(INFINITE, |most| most.as_millis().min(u128::from(INFINITE - 1)) as u32);
        let mut changed = false;
        let mut period = milliseconds;
        loop {
            let watched: Vec<usize> = (0..self.dirs.len()).filter(|&i| self.dirs[i].1.is_some()).collect();
            let mut handles = vec![self.wake.0];
            handles.extend(watched.iter().map(|&i| self.dirs[i].1.as_ref().expect("watched").0));
            // SAFETY: live handles, at most 64, waited on for any one.
            let signaled = unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, period) };
            if signaled == WAIT_TIMEOUT {
                break;
            }
            let index = signaled.wrapping_sub(WAIT_OBJECT_0) as usize;
            if index >= handles.len() {
                return Err(Error::os("WaitForMultipleObjects")(io::Error::last_os_error()));
            }
            if index == 0 {
                self.wake.reset();
                return Ok(changed);
            }
            changed = true;
            let i = watched[index - 1];
            // SAFETY: a live change-notification handle; a directory gone fails and is asked again.
            if unsafe { FindNextChangeNotification(handles[index]) } == 0 {
                self.dirs[i].1 = None;
            }
            // Drain what is already signaled: one wake per wait.
            period = 0;
        }
        let _ = self.arm();
        Ok(changed)
    }

    pub(super) fn wake(&self) -> Arc<Wake> {
        self.wake.clone()
    }
}

impl Wake {
    pub(super) fn wake(&self) {
        // SAFETY: the event this owns.
        unsafe { SetEvent(self.0) };
    }

    fn reset(&self) {
        // SAFETY: as above.
        unsafe { ResetEvent(self.0) };
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        // SAFETY: the event this owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Drop for Change {
    fn drop(&mut self) {
        // SAFETY: the handle this owns, closed once.
        unsafe { FindCloseChangeNotification(self.0) };
    }
}
