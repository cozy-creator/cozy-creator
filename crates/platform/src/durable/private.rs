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
