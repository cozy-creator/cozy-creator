//! A folder moved to the OS trash (cl-1097), as a file manager's Delete does: the freedesktop Trash on Linux and the other Unixes, the
//! person's Trash on macOS, the Recycle Bin on Windows. It's moved whole, never copied or deleted: put back, it's as it was. Where no
//! trash can take it, it stays where it is and that's the error.
#![allow(unsafe_code)]
use std::path::{Path, PathBuf};

/// `path` in the trash: where it is now, when the OS says (the Recycle Bin doesn't).
pub fn put(path: &Path) -> crate::Result<Option<PathBuf>> {
    Ok(imp::put(&std::path::absolute(path)?)?)
}

/// `name`, else `name 2`, `name 3`, …: the first `free` takes.
#[cfg(unix)]
fn first_free(name: &std::ffi::OsStr, mut free: impl FnMut(&std::ffi::OsStr) -> std::io::Result<bool>) -> std::io::Result<std::ffi::OsString> {
    for n in 1.. {
        let mut named = name.to_owned();
        if n > 1 { named.push(format!(" {n}")); }
        if free(&named)? { return Ok(named); }
    }
    unreachable!("names go on")
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::{
        ffi::OsStr,
        fs, io,
        io::Write as _,
        os::unix::{ffi::OsStrExt as _, fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _}},
        path::{Path, PathBuf},
    };

    /// The freedesktop.org Trash spec 1.0: the home trash when `path` is on its device, else its device's own top-level one.
    pub(super) fn put(path: &Path) -> io::Result<Option<PathBuf>> {
        let device = fs::symlink_metadata(path)?.dev();
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|dir| dir.is_absolute())
            .or_else(|| crate::paths::login_home().map(|home| home.join(".local").join("share")))
            .ok_or_else(|| io::Error::other("no home folder for its trash"))?;
        let home = data.join("Trash");
        let reach = home.ancestors().find(|dir| dir.exists()).map(fs::metadata).transpose()?.map(|meta| meta.dev());
        if reach == Some(device) {
            return trash(path, &home, path).map(Some);
        }
        let top = path.ancestors().skip(1).take_while(|dir| fs::metadata(dir).is_ok_and(|meta| meta.dev() == device)).last()
            .ok_or_else(|| io::Error::other("no top folder on its drive"))?;
        let uid = rustix::process::getuid().as_raw();
        let shared = top.join(".Trash");
        let shared_ok = fs::symlink_metadata(&shared).is_ok_and(|meta| meta.is_dir() && meta.permissions().mode() & 0o1000 != 0);
        let trash_dir = if shared_ok { shared.join(uid.to_string()) } else { top.join(format!(".Trash-{uid}")) };
        trash(path, &trash_dir, path.strip_prefix(top).unwrap_or(path)).map(Some)
    }

    /// Its `.trashinfo` made first, which reserves the name, then it's moved into `files/`.
    fn trash(path: &Path, trash: &Path, said: &Path) -> io::Result<PathBuf> {
        let (files, info) = (trash.join("files"), trash.join("info"));
        for dir in [&files, &info] { fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?; }
        let name = path.file_name().ok_or_else(|| io::Error::other("no folder name"))?;
        let mut kept = None;
        let name = super::first_free(name, |named| {
            if files.join(named).exists() { return Ok(false); }
            let mut file = named.to_owned();
            file.push(".trashinfo");
            match fs::OpenOptions::new().write(true).create_new(true).open(info.join(&file)) {
                Ok(written) => { kept = Some((written, info.join(file))); Ok(true) }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
                Err(e) => Err(e),
            }
        })?;
        let (mut written, at) = kept.expect("reserved");
        let wrote = writeln!(written, "[Trash Info]\nPath={}\nDeletionDate={}", encoded(said.as_os_str()), now()).and_then(|()| written.sync_all());
        let landed = files.join(&name);
        if let Err(e) = wrote.and_then(|()| fs::rename(path, &landed)) {
            let _ = fs::remove_file(at);
            return Err(e);
        }
        Ok(landed)
    }

    /// A path as the spec's `Path=` holds it: each byte but an unreserved one and `/` as `%XX`.
    fn encoded(path: &OsStr) -> String {
        path.as_bytes().iter().map(|&b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (b as char).to_string(),
            b => format!("%{b:02X}"),
        }).collect()
    }

    /// Now, in local time: `YYYY-MM-DDThh:mm:ss`.
    fn now() -> String {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs()) as libc::time_t;
        // SAFETY: plain-old-data the call fills.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: both pointers are to locals that outlive the call.
        unsafe { libc::localtime_r(&at, &mut tm) };
        format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::{fs, io, os::unix::fs::MetadataExt as _, path::{Path, PathBuf}};

    /// `~/.Trash` for what's on the home folder's volume, else that volume's `.Trashes/<uid>`, as the Finder keeps them.
    pub(super) fn put(path: &Path) -> io::Result<Option<PathBuf>> {
        let device = fs::symlink_metadata(path)?.dev();
        let home = crate::paths::login_home().ok_or_else(|| io::Error::other("no home folder for its trash"))?;
        let trash = if fs::metadata(&home)?.dev() == device {
            home.join(".Trash")
        } else {
            let top = path.ancestors().skip(1).take_while(|dir| fs::metadata(dir).is_ok_and(|meta| meta.dev() == device)).last()
                .ok_or_else(|| io::Error::other("no top folder on its volume"))?;
            top.join(".Trashes").join(rustix::process::getuid().as_raw().to_string())
        };
        fs::create_dir_all(&trash)?;
        let name = path.file_name().ok_or_else(|| io::Error::other("no folder name"))?;
        let name = super::first_free(name, |named| Ok(fs::symlink_metadata(trash.join(named)).is_err()))?;
        let landed = trash.join(name);
        fs::rename(path, &landed)?;
        Ok(Some(landed))
    }
}

#[cfg(windows)]
mod imp {
    use std::{io, os::windows::ffi::OsStrExt as _, path::{Path, PathBuf}};
    use windows_sys::Win32::{
        Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW},
        UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW},
    };

    /// A fixed drive's: the Recycle Bin. Only a fixed drive has one; elsewhere the shell would delete it for good, so it's refused.
    const DRIVE_FIXED: u32 = 3;

    pub(super) fn put(path: &Path) -> io::Result<Option<PathBuf>> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut volume = vec![0u16; 1024];
        // SAFETY: a NUL-terminated path and a buffer of the length given.
        if unsafe { GetVolumePathNameW(wide.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the NUL-terminated volume root the call above wrote.
        if unsafe { GetDriveTypeW(volume.as_ptr()) } != DRIVE_FIXED {
            return Err(io::Error::other("its drive has no Recycle Bin"));
        }
        // The shell takes a list: each path NUL-terminated, the list ended by another NUL.
        let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
        let mut operation = SHFILEOPSTRUCTW {
            hwnd: std::ptr::null_mut(),
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            pTo: std::ptr::null(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
            fAnyOperationsAborted: 0,
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: std::ptr::null(),
        };
        // SAFETY: the operation and its double-NUL-terminated list outlive the call.
        let failed = unsafe { SHFileOperationW(&mut operation) };
        if failed != 0 || operation.fAnyOperationsAborted != 0 {
            return Err(io::Error::other(format!("the shell couldn't move it to the Recycle Bin (error {failed:#x})")));
        }
        Ok(None)
    }
}
