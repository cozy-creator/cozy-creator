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

/// Whether another account on this computer may open `dir`: group or other bits on Unix; on Windows, an entry allowing anyone but
/// this account, SYSTEM, Administrators or the owner.
pub fn others_may_open(dir: &Path) -> crate::Result<bool> {
    opened_by_others(dir).map_err(crate::Error::io("read a folder's access", dir))
}

#[cfg(unix)]
fn private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
fn opened_by_others(dir: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt as _;
    Ok(std::fs::metadata(dir)?.permissions().mode() & 0o077 != 0)
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
            Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION,
        },
    };
    let name = wide(dir)?;
    let (mut dacl, mut descriptor) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: a terminated path and writable outputs; the descriptor is freed below.
    let code = unsafe {
        GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(), &mut dacl, ptr::null_mut(), &mut descriptor)
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: the live descriptor and a writable string pointer; the length is not asked.
    let converted = unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, ptr::null_mut()) };
    let sddl = (converted != 0).then(|| {
        // SAFETY: a NUL-terminated string the conversion allocated.
        let sddl = unsafe {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(text, len))
        };
        // SAFETY: the string the conversion allocated, freed once.
        unsafe { LocalFree(text as HLOCAL) };
        sddl
    });
    let no_dacl = dacl.is_null();
    // SAFETY: the descriptor GetNamedSecurityInfoW allocated, freed once; `dacl` pointed into it and isn't read again.
    unsafe { LocalFree(descriptor as HLOCAL) };
    let sddl = sddl.ok_or_else(io::Error::last_os_error)?;
    let own = crate::ipc::this_user()?;
    Ok(no_dacl || others_allowed(&sddl, &own))
}

/// Whether a DACL in SDDL (`D:PAI(A;OICI;FA;;;SY)(A;;FA;;;S-1-5-21-…)`) allows anyone but `own`, SYSTEM, Administrators or the
/// owner. An inherit-only entry (`IO`) doesn't apply to the folder itself.
#[cfg(windows)]
fn others_allowed(sddl: &str, own: &str) -> bool {
    let Some(dacl) = sddl.split_once("D:").map(|(_, dacl)| dacl) else { return true };
    if dacl.starts_with("NO_ACCESS_CONTROL") {
        return true;
    }
    dacl.split('(').skip(1).filter_map(|ace| ace.split(')').next()).any(|ace| {
        let fields: Vec<&str> = ace.split(';').collect();
        let (kind, flags, sid) = (fields.first().copied().unwrap_or(""), fields.get(1).copied().unwrap_or(""), fields.get(5).copied().unwrap_or(""));
        let inherit_only = flags.as_bytes().chunks(2).any(|flag| flag == b"IO");
        matches!(kind, "A" | "OA" | "XA" | "ZA") && !inherit_only && !["SY", "BA", "OW", "CO", own].contains(&sid)
    })
}
