//! New secret files: restrictive access is applied at creation, before the first write.
//! Windows file security follows CreateFileW's explicit descriptor contract:
//! https://learn.microsoft.com/windows/win32/fileio/file-security-and-access-rights
#![allow(unsafe_code)]
use std::{fs::File, io, path::Path};

/// Creates a new owner-only file and never opens or replaces an existing name.
/// The caller writes and flushes its secret only after this succeeds. Windows volumes
/// without persistent ACLs are refused; a failed check may leave an empty new file.
pub fn create_private_new(path: &Path) -> crate::Result<File> {
    create(path).map_err(crate::Error::io("create private file", path))
}

#[cfg(unix)]
fn create(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(windows)]
fn create(path: &Path) -> io::Result<File> {
    use std::{
        mem,
        os::windows::{
            ffi::OsStrExt as _,
            io::{AsRawHandle as _, FromRawHandle as _},
        },
        path::{Component, Prefix},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{GENERIC_WRITE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, GetVolumeInformationByHandleW,
        },
        System::SystemServices::FILE_PERSISTENT_ACLS,
    };
    // An alternate data stream shares its existing file's security descriptor; it is
    // not a fresh independently protected file. Device namespaces are not files either.
    for part in path.components() {
        match part {
            Component::Normal(name) if name.encode_wide().any(|unit| unit == b':' as u16) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a private file cannot be an alternate data stream",
                ));
            }
            Component::Prefix(prefix)
                if !matches!(
                    prefix.kind(),
                    Prefix::Disk(_)
                        | Prefix::VerbatimDisk(_)
                        | Prefix::UNC(_, _)
                        | Prefix::VerbatimUNC(_, _)
                ) =>
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a private file needs a filesystem path",
                ));
            }
            _ => {}
        }
    }
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file path contains NUL",
        ));
    }
    name.push(0);
    let sid = crate::ipc::this_user()?;
    // Protected DACL: no inherited entries, and only the current user is allowed.
    let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
        .encode_utf16()
        .chain([0])
        .collect();
    struct Descriptor(PSECURITY_DESCRIPTOR);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            // SAFETY: one descriptor returned by the conversion below, freed once.
            unsafe {
                LocalFree(self.0 as HLOCAL);
            }
        }
    }
    let mut descriptor = ptr::null_mut();
    // SAFETY: a terminated UTF-16 descriptor and an owned writable output pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = Descriptor(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: path and descriptor remain alive through this call; CREATE_NEW never
    // follows an existing file into a secret write, and the returned handle is owned.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call transferred one live file handle to this scope.
    let file = unsafe { File::from_raw_handle(handle) };
    let mut flags = 0;
    // SAFETY: our live file handle and a writable flags field; all other outputs omitted.
    if unsafe {
        GetVolumeInformationByHandleW(
            file.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut flags,
            ptr::null_mut(),
            0,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if flags & FILE_PERSISTENT_ACLS == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this filesystem cannot enforce owner-only access; choose a protected filesystem",
        ));
    }
    Ok(file)
}

/// Makes `dir` this account's alone: mode 0700 on Unix; on Windows a protected DACL whose one entry gives this account everything,
/// inherited by all that's made in it.
pub fn make_private_dir(dir: &Path) -> crate::Result<()> {
    private_dir(dir).map_err(crate::Error::io("make a folder private", dir))
}

/// Whether another account on this computer may open `dir`. On Unix: its other bits, or its group bits while its group is this
/// account's own primary one (`staff`, `users`: every account's); a folder given to another group is a share its owner chose. On
/// Windows: an entry allowing anyone but this account, SYSTEM, Administrators or the owner.
pub fn open_to_others(dir: &Path) -> crate::Result<bool> {
    others_bits(dir).map(|(mode, shared)| mode & shared != 0).map_err(crate::Error::io("read a folder's access", dir))
}

/// Shuts `dir` to the accounts `open_to_others` names: on Unix those bits cleared, a chosen group's kept; on Windows, owner-only, as
/// `make_private_dir` (which reaches into what's in it: a caller does it once, off any request's path).
pub fn shut_to_others(dir: &Path) -> crate::Result<()> {
    shut_others(dir).map_err(crate::Error::io("make a folder private", dir))
}

/// `dir`'s mode, and the bits of it that reach other accounts.
#[cfg(unix)]
fn others_bits(dir: &Path) -> io::Result<(u32, u32)> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let meta = std::fs::metadata(dir)?;
    let own_group = meta.gid() == rustix::process::getgid().as_raw();
    Ok((meta.permissions().mode(), if own_group { 0o077 } else { 0o007 }))
}

#[cfg(unix)]
fn shut_others(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let (mode, shared) = others_bits(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode & 0o7777 & !shared))
}

#[cfg(windows)]
fn others_bits(dir: &Path) -> io::Result<(u32, u32)> {
    opened_by_others(dir).map(|open| (u32::from(open), 1))
}

#[cfg(windows)]
fn shut_others(dir: &Path) -> io::Result<()> {
    private_dir(dir)
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn wide(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt as _;
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    if name[..name.len() - 1].contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"));
    }
    Ok(name)
}

#[cfg(windows)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{
            Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW},
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };
    let name = wide(dir)?;
    let sid = crate::ipc::this_user()?;
    // Protected: nothing inherited from above. Object and container inherit: what's made inside is as private.
    let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{sid})").encode_utf16().chain([0]).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: a terminated UTF-16 descriptor and an owned writable output pointer.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let (mut present, mut dacl, mut defaulted) = (0, ptr::null_mut(), 0);
    // SAFETY: the live descriptor just made and writable outputs; the DACL points into it.
    let read = unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) };
    let result = if read == 0 || present == 0 {
        Err(io::Error::last_os_error())
    } else {
        let information = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
        // SAFETY: a terminated path and the descriptor's DACL, alive through the call; no owner, group or SACL changes.
        match unsafe { SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, information, ptr::null_mut(), ptr::null_mut(), dacl, ptr::null()) } {
            0 => Ok(()),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    };
    // SAFETY: the descriptor the conversion allocated, freed once, after its DACL's last use.
    unsafe { LocalFree(descriptor as HLOCAL) };
    result
}

#[cfg(windows)]
fn opened_by_others(dir: &Path) -> io::Result<bool> {
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACL, Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE,
        },
        System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE},
    };
    let name = wide(dir)?;
    let (mut dacl, mut descriptor): (*mut ACL, _) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: a terminated path and writable outputs; the descriptor is freed below.
    let code = unsafe {
        GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), &mut dacl, ptr::null_mut(), &mut descriptor)
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    // This account, SYSTEM, Administrators, OWNER RIGHTS and CREATOR OWNER: no other account. Compared in full `S-1-…` form, as SDDL
    // would shorten some to aliases (the built-in Administrator's to `LA`).
    let own = crate::ipc::this_user()?;
    let mine = |sid: &str| [own.as_str(), "S-1-5-18", "S-1-5-32-544", "S-1-3-4", "S-1-3-0"].contains(&sid);
    // No DACL at all lets everyone in.
    let mut others = dacl.is_null();
    // SAFETY: a live DACL inside the descriptor, read for its count.
    let count = if dacl.is_null() { 0 } else { unsafe { (*dacl).AceCount } };
    for at in 0..u32::from(count) {
        let mut ace = ptr::null_mut();
        // SAFETY: the live DACL, an index under its count and a writable pointer into it.
        if unsafe { GetAce(dacl, at, &mut ace) } == 0 {
            continue;
        }
        // SAFETY: every ACE starts with its header.
        let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let kind = u32::from(allowed.Header.AceType);
        if !matches!(kind, ACCESS_ALLOWED_ACE_TYPE | ACCESS_ALLOWED_CALLBACK_ACE_TYPE) || u32::from(allowed.Header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        let mut text: *mut u16 = ptr::null_mut();
        // SAFETY: an allowed ACE's SID starts at its SidStart; a writable string pointer.
        if unsafe { ConvertSidToStringSidW(ptr::addr_of!(allowed.SidStart).cast_mut().cast(), &mut text) } == 0 {
            continue;
        }
        // SAFETY: a NUL-terminated string the conversion allocated.
        let sid = unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
        };
        // SAFETY: the string the conversion allocated, freed once.
        unsafe { LocalFree(text as HLOCAL) };
        others |= !mine(&sid);
    }
    // SAFETY: the descriptor GetNamedSecurityInfoW allocated, freed once; `dacl` pointed into it and isn't read again.
    unsafe { LocalFree(descriptor as HLOCAL) };
    Ok(others)
}
