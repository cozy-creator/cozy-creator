//! The one standalone/TensorD domain and authoritative device identity discovery.
#![allow(unsafe_code)]

use super::Result;
use crate::{Error, ledger};
use nvml_wrapper::{
    Nvml,
    error::{NvmlError, nvml_sym, nvml_try},
};
use nvml_wrapper_sys::bindings::{
    nvmlUUID_v1_t, nvmlUUIDType_t_NVML_UUID_TYPE_BINARY, nvmlUUIDValue_t,
};
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Prepare the existing TensorD domain without loading CUDA/NVML or opening a table.
/// Existing paths are validated, never repaired, reset or assigned another owner.
pub fn default_directory() -> io::Result<PathBuf> {
    // SAFETY: geteuid takes no memory arguments and performs no GPU initialization.
    let uid = unsafe { libc::geteuid() };
    let path = PathBuf::from(format!("/dev/shm/cozy-varena-{uid}"));
    prepare_directory(&path, uid)?;
    Ok(path)
}

fn prepare_directory(path: &Path, uid: u32) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cooperation directory must be this effective UID's private 0700 directory",
        ));
    }
    Ok(())
}

fn nvml_error(operation: &str, error: NvmlError) -> super::Error {
    let reason = format!("{operation} during device identity discovery: {error}");
    match error {
        NvmlError::GpuLost => Error::Poisoned(reason).into(),
        _ => Error::Unsupported(reason).into(),
    }
}

/// Discover this CUDA ordinal's actual GPU/MIG descriptor, never an NVML ordinal or
/// a guessed prefix. Only the first standalone opener needs this GPU-context query.
pub fn device_descriptor(device: i32) -> Result<ledger::Device> {
    crate::check_process()?;
    if device < 0 {
        return Err(Error::Invalid("device must be nonnegative").into());
    }
    // The token excludes capture/replay; no capture/registry mutex spans NVML work.
    // It must be gone before Peer::open, which refuses outstanding retirements.
    let _exclusion = crate::enter_retirement(device)?;
    crate::working::initialize(device)?;
    let uuid = crate::working::peer::uuid(device)?;
    let nvml = Nvml::init().map_err(|error| nvml_error("nvmlInit_v2", error))?;
    let query = nvml_sym(nvml.lib().nvmlDeviceGetHandleByUUIDV.as_ref())
        .map_err(|error| nvml_error("nvmlDeviceGetHandleByUUIDV", error))?;
    let input = nvmlUUID_v1_t {
        // NVIDIA's NVML_STRUCT_VERSION(UUID, 1), not a guessed structure layout.
        version: (std::mem::size_of::<nvmlUUID_v1_t>() as u32) | (1 << 24),
        type_: nvmlUUIDType_t_NVML_UUID_TYPE_BINARY,
        value: nvmlUUIDValue_t { bytes: uuid },
    };
    let mut handle = std::ptr::null_mut();
    // SAFETY: the generated UUID layout/version and writable output are valid; Nvml
    // owns the loaded library and initialization through all uses of this handle.
    nvml_try(unsafe { query(&input, &mut handle) })
        .map_err(|error| nvml_error("nvmlDeviceGetHandleByUUIDV", error))?;
    if handle.is_null() {
        return Err(
            Error::Unsupported("NVML returned a null device identity handle".into()).into(),
        );
    }
    // SAFETY: the successful lookup returned a live handle under this Nvml instance.
    let found = unsafe { nvml_wrapper::Device::new(handle, &nvml) };
    let canonical = found
        .uuid()
        .map_err(|error| nvml_error("nvmlDeviceGetUUID", error))?;
    let descriptor = ledger::Device::parse(&canonical)?;
    if descriptor.uuid != uuid {
        return Err(Error::Invalid("NVML identity differs from the actual CUDA UUID").into());
    }
    Ok(descriptor)
}
