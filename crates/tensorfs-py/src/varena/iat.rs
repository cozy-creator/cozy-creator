//! Windows PE64 import-address-table rewriting, using documented loader and page APIs.
use super::Functions;
use std::ffi::{c_void, CStr};
use std::sync::atomic::{AtomicUsize, Ordering};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleExA(flags: u32, name: *const i8, module: *mut *mut c_void) -> i32;
    fn VirtualQuery(address: *const c_void, information: *mut MemoryInfo, bytes: usize) -> usize;
    fn GetProcAddress(module: *mut c_void, name: *const i8) -> *mut c_void;
    fn VirtualProtect(address: *mut c_void, bytes: usize, protection: u32, old: *mut u32) -> i32;
}
#[repr(C)]
struct MemoryInfo {
    base: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}
fn readable(address: usize, bytes: usize) -> Result<usize, String> {
    let end = address.checked_add(bytes).ok_or("PE range overflow")?;
    let mut at = address;
    let mut mapped_end = address;
    while at < end {
        let mut info = std::mem::MaybeUninit::<MemoryInfo>::uninit();
        // SAFETY: VirtualQuery writes a complete memory information record to this stack cell.
        let written = unsafe {
            VirtualQuery(
                at as *const c_void,
                info.as_mut_ptr(),
                std::mem::size_of::<MemoryInfo>(),
            )
        };
        if written != std::mem::size_of::<MemoryInfo>() {
            return Err("cannot inspect Torch PE mapping".into());
        }
        // SAFETY: VirtualQuery initialized the complete record, checked immediately above.
        let info = unsafe { info.assume_init() };
        if info.state != 0x1000
            || info.protect & 0x100 != 0
            || !matches!(info.protect & 0xff, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80)
        {
            return Err("Torch PE import is not readable committed memory".into());
        }
        mapped_end = (info.base as usize)
            .checked_add(info.region_size)
            .ok_or("PE mapping overflow")?;
        if mapped_end <= at {
            return Err("empty Torch PE mapping".into());
        }
        at = mapped_end;
    }
    Ok(mapped_end)
}
struct Slot {
    address: usize,
    original: usize,
    allocate: bool,
}
pub(super) struct Plan {
    slots: Vec<Slot>,
    device: usize,
}

// PE offsets are specified by Microsoft's PE/COFF format. Every subsequent RVA is checked
// against SizeOfImage before dereferencing. Only already-loaded Torch CUDA DLLs are considered.
pub(super) fn discover() -> Result<Plan, String> {
    if !cfg!(target_arch = "x86_64") {
        return Err("Windows CUDA import hook requires x86_64".into());
    }
    let mut plan = Plan {
        slots: Vec::new(),
        device: 0,
    };
    for name in [c"c10_cuda.dll", c"torch_cuda.dll"] {
        // SAFETY: the DLL name is terminated; PIN retains an already-loaded Torch module for process lifetime.
        let mut module = std::ptr::null_mut();
        // SAFETY: the terminated loaded DLL name is pinned for process lifetime.
        unsafe {
            GetModuleHandleExA(1, name.as_ptr(), &mut module);
        }
        let module = module as usize;
        if module == 0 {
            return Err("import CUDA Torch first: its CUDA DLLs are not loaded".into());
        }
        readable(module, 0x40)?;
        // SAFETY: the loader returned a live PE image with its fixed DOS header.
        let pe_offset = unsafe { std::ptr::read_unaligned((module + 0x3c) as *const u32) } as usize;
        if pe_offset > 4096 {
            return Err("unsupported Torch PE header offset".into());
        }
        readable(module + pe_offset, 24 + 128)?;
        // SAFETY: the complete fixed NT header range was validated readable above.
        let (signature, machine, magic, image_bytes, import_rva, import_bytes) = unsafe {
            (
                std::ptr::read_unaligned((module + pe_offset) as *const u32),
                std::ptr::read_unaligned((module + pe_offset + 4) as *const u16),
                std::ptr::read_unaligned((module + pe_offset + 24) as *const u16),
                std::ptr::read_unaligned((module + pe_offset + 24 + 56) as *const u32) as usize,
                std::ptr::read_unaligned((module + pe_offset + 24 + 120) as *const u32) as usize,
                std::ptr::read_unaligned((module + pe_offset + 24 + 124) as *const u32) as usize,
            )
        };
        if signature != 0x4550 || machine != 0x8664 || magic != 0x20b {
            return Err("unsupported Torch PE64 image".into());
        }
        let image = Image {
            base: module,
            bytes: image_bytes,
        };
        image.range(import_rva, import_bytes)?;
        let mut found = Vec::new();
        for offset in (import_rva..import_rva + import_bytes).step_by(20) {
            image.range(offset, 20)?;
            let (names, library_name, slots) = (
                image.u32(offset)? as usize,
                image.u32(offset + 12)? as usize,
                image.u32(offset + 16)? as usize,
            );
            if names == 0 && library_name == 0 && slots == 0 {
                break;
            }
            if names == 0 {
                return Err("Torch PE lacks original import names".into());
            }
            let library_name = image.string(library_name)?;
            // SAFETY: the bounded terminated DLL name belongs to the loaded image; PIN retains its code.
            let mut library = std::ptr::null_mut();
            // SAFETY: the validated terminated import name identifies a loaded dependency.
            unsafe {
                GetModuleHandleExA(1, library_name.as_ptr(), &mut library);
            }
            for index in 0..image_bytes / 8 {
                let thunk = image.u64(names + index * 8)?;
                if thunk == 0 {
                    break;
                }
                if thunk & (1 << 63) != 0 {
                    continue;
                }
                let symbol =
                    image.string((thunk as usize).checked_add(2).ok_or("PE name overflow")?)?;
                let allocate = match symbol.to_bytes() {
                    b"cudaMalloc" => true,
                    b"cudaFree" => false,
                    _ => continue,
                };
                if library.is_null() {
                    return Err("CUDA runtime DLL is not loaded".into());
                }
                // SAFETY: named CUDA runtime function from a live loader-owned DLL.
                let original = unsafe { GetProcAddress(library, symbol.as_ptr()) } as usize;
                let slot_rva = slots + index * 8;
                image.range(slot_rva, 8)?;
                if original == 0 || image.u64(slot_rva)? as usize != original {
                    return Err("Torch CUDA import is already redirected".into());
                }
                if (module + slot_rva) % 8 != 0 {
                    return Err("unaligned Torch CUDA import".into());
                }
                found.push(Slot {
                    address: module + slot_rva,
                    original,
                    allocate,
                });
                // SAFETY: CUDA runtime exports the documented device-query function.
                let device = unsafe { GetProcAddress(library, c"cudaGetDevice".as_ptr()) } as usize;
                if device == 0 || (plan.device != 0 && plan.device != device) {
                    return Err("inconsistent CUDA runtime DLLs".into());
                }
                plan.device = device;
            }
        }
        if found.iter().filter(|s| s.allocate).count() != 1
            || found.iter().filter(|s| !s.allocate).count() != 1
        {
            return Err("expected one cudaMalloc/cudaFree import in each Torch CUDA DLL".into());
        }
        plan.slots.extend(found);
    }
    Ok(plan)
}
struct Image {
    base: usize,
    bytes: usize,
}
impl Image {
    fn range(&self, offset: usize, bytes: usize) -> Result<(), String> {
        if !offset
            .checked_add(bytes)
            .is_some_and(|end| end <= self.bytes)
        {
            return Err("Torch PE import outside image".into());
        }
        readable(self.base + offset, bytes)?;
        Ok(())
    }
    fn u32(&self, offset: usize) -> Result<u32, String> {
        self.range(offset, 4)?;
        // SAFETY: full read range is inside the loaded PE image.
        Ok(unsafe { std::ptr::read_unaligned((self.base + offset) as *const u32) })
    }
    fn u64(&self, offset: usize) -> Result<u64, String> {
        self.range(offset, 8)?;
        // SAFETY: full read range is inside the loaded PE image.
        Ok(unsafe { std::ptr::read_unaligned((self.base + offset) as *const u64) })
    }
    fn string(&self, offset: usize) -> Result<&CStr, String> {
        self.range(offset, 1)?;
        let mapped = readable(self.base + offset, 1)?;
        let length = (self.bytes - offset).min(mapped - (self.base + offset));
        // SAFETY: the bounded scan remains inside one validated readable mapping.
        let bytes =
            unsafe { std::slice::from_raw_parts((self.base + offset) as *const u8, length) };
        CStr::from_bytes_until_nul(bytes).map_err(|_| "unterminated PE import name".into())
    }
}
impl Plan {
    pub(super) fn functions(&self) -> Result<Functions, String> {
        let malloc = self
            .slots
            .iter()
            .find(|s| s.allocate)
            .ok_or("missing cudaMalloc import")?
            .original;
        let free = self
            .slots
            .iter()
            .find(|s| !s.allocate)
            .ok_or("missing cudaFree import")?
            .original;
        if self
            .slots
            .iter()
            .any(|s| s.original != if s.allocate { malloc } else { free })
        {
            return Err("Torch DLLs use different CUDA runtime instances".into());
        }
        let mut runtime = std::ptr::null_mut();
        // SAFETY: FROM_ADDRESS locates the original allocation's loaded DLL; PIN retains it.
        if unsafe { GetModuleHandleExA(0x05, malloc as *const i8, &mut runtime) } == 0 {
            return Err("cannot retain the original CUDA runtime".into());
        }
        // SAFETY: runtime is the original allocation's live DLL; the export names are terminated.
        let (runtime_malloc, last_error) = unsafe {
            (
                GetProcAddress(runtime, c"cudaMalloc".as_ptr()),
                GetProcAddress(runtime, c"cudaGetLastError".as_ptr()),
            )
        };
        if runtime_malloc as usize != malloc || last_error.is_null() {
            return Err("cannot resolve error handling in the original CUDA runtime".into());
        }
        // SAFETY: names were resolved from CUDA runtime exports with these stable C signatures.
        Ok(unsafe {
            Functions {
                malloc: std::mem::transmute::<usize, super::Malloc>(malloc),
                free: std::mem::transmute::<usize, super::Free>(free),
                device: std::mem::transmute::<usize, super::GetDevice>(self.device),
                last_error: std::mem::transmute::<*mut c_void, super::GetLastError>(last_error),
            }
        })
    }
    pub(super) fn install(self, malloc: usize, free: usize) -> Result<usize, String> {
        let mut hook_module = std::ptr::null_mut();
        // SAFETY: FROM_ADDRESS interprets the name parameter as a live function address; PIN
        // keeps its containing extension loaded for the lifetime of these permanent IAT hooks.
        if unsafe { GetModuleHandleExA(0x05, malloc as *const i8, &mut hook_module) } == 0 {
            return Err("cannot pin the native hook module".into());
        }
        let mut changed: Vec<(&Slot, u32)> = Vec::new();
        for slot in &self.slots {
            let mut protection = 0;
            // SAFETY: alter only a loader-owned IAT data slot, preserving its original protection.
            if unsafe { VirtualProtect(slot.address as *mut c_void, 8, 0x04, &mut protection) } == 0
            {
                for (slot, old) in changed.into_iter().rev() {
                    // SAFETY: these slots are writable from previous successful VirtualProtect.
                    unsafe { &*(slot.address as *const AtomicUsize) }
                        .store(slot.original, Ordering::SeqCst);
                    let mut unused = 0;
                    // SAFETY: restore the original data-page protection.
                    unsafe {
                        VirtualProtect(slot.address as *mut c_void, 8, old, &mut unused);
                    }
                }
                return Err("cannot make Torch IAT writable".into());
            }
            // SAFETY: naturally aligned IAT pointer, made writable immediately above.
            unsafe { &*(slot.address as *const AtomicUsize) }
                .store(if slot.allocate { malloc } else { free }, Ordering::SeqCst);
            changed.push((slot, protection));
        }
        for (slot, protection) in changed.into_iter().rev() {
            let mut unused = 0;
            // SAFETY: restore original protection of the same IAT data address.
            if unsafe { VirtualProtect(slot.address as *mut c_void, 8, protection, &mut unused) }
                == 0
            {
                return Err("Torch IAT installed but page protection restoration failed".into());
            }
        }
        Ok(self.slots.len())
    }
}
