//! Linux ELF data-import rewriting. This touches relocation slots, never executable code.
//! ELF64 layouts follow the System V ABI; dl_iterate_phdr/dlsym/mprotect follow glibc APIs.
use super::Functions;
use std::ffi::{c_void, CStr, CString};
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C)]
struct Dynamic {
    tag: i64,
    value: u64,
}
#[repr(C)]
struct Rela {
    offset: u64,
    info: u64,
    addend: i64,
}
#[repr(C)]
struct Symbol {
    name: u32,
    info: u8,
    other: u8,
    section: u16,
    value: u64,
    size: u64,
}
struct Slot {
    address: usize,
    original: usize,
    allocate: bool,
    protection: i32,
}
pub(super) struct Plan {
    slots: Vec<Slot>,
    libraries: Vec<usize>,
}
struct Discovery {
    plan: Plan,
    error: Option<String>,
}

pub(super) fn discover() -> Result<Plan, String> {
    if !cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        return Err("native Torch import hook requires ELF64 x86_64 or aarch64".into());
    }
    let mut discovery = Discovery {
        plan: Plan {
            slots: Vec::new(),
            libraries: Vec::new(),
        },
        error: None,
    };
    // SAFETY: glibc calls visit synchronously with live loaded-object program headers. The
    // caller-owned Discovery lives until iteration returns and is accessed only by this thread.
    unsafe {
        libc::dl_iterate_phdr(Some(visit), (&mut discovery as *mut Discovery).cast());
    }
    if let Some(error) = discovery.error {
        return Err(error);
    }
    if discovery.plan.libraries.len() != 2 {
        return Err("import CUDA Torch first: expected libc10_cuda.so and libtorch_cuda.so".into());
    }
    Ok(discovery.plan)
}

unsafe extern "C" fn visit(info: *mut libc::dl_phdr_info, _size: usize, data: *mut c_void) -> i32 {
    // SAFETY: dl_iterate_phdr's callback contract and discover's live Discovery pointer.
    let (info, discovery) = unsafe { (&*info, &mut *data.cast::<Discovery>()) };
    // SAFETY: glibc guarantees a terminated dlpi_name for each loaded object.
    let name = unsafe { CStr::from_ptr(info.dlpi_name) }.to_string_lossy();
    if !matches!(
        name.rsplit('/').next(),
        Some("libc10_cuda.so" | "libtorch_cuda.so")
    ) {
        return 0;
    }
    match inspect(info, &name) {
        Ok((slots, library)) => {
            discovery.plan.slots.extend(slots);
            discovery.plan.libraries.push(library);
            0
        }
        Err(error) => {
            discovery.error = Some(error);
            1
        }
    }
}

fn inspect(info: &libc::dl_phdr_info, name: &str) -> Result<(Vec<Slot>, usize), String> {
    // SAFETY: glibc provides dlpi_phnum live ELF program headers for this loaded DSO.
    let headers = unsafe { std::slice::from_raw_parts(info.dlpi_phdr, info.dlpi_phnum as usize) };
    let base = info.dlpi_addr as usize;
    let mapped: Vec<_> = headers
        .iter()
        .filter(|h| h.p_type == libc::PT_LOAD)
        .map(|h| (base + h.p_vaddr as usize)..(base + h.p_vaddr as usize + h.p_memsz as usize))
        .collect();
    let contains = |address: usize, bytes: usize| {
        mapped.iter().any(|range| {
            address >= range.start
                && address
                    .checked_add(bytes)
                    .is_some_and(|end| end <= range.end)
        })
    };
    let dynamic = headers
        .iter()
        .find(|h| h.p_type == libc::PT_DYNAMIC)
        .ok_or("Torch ELF lacks dynamic section")?;
    let address = base + dynamic.p_vaddr as usize;
    let count = dynamic.p_memsz as usize / std::mem::size_of::<Dynamic>();
    if !contains(address, count * std::mem::size_of::<Dynamic>()) {
        return Err("invalid Torch dynamic range".into());
    }
    // SAFETY: the program-header bounds above cover this complete dynamic table.
    let table = unsafe { std::slice::from_raw_parts(address as *const Dynamic, count) };
    let value = |tag: i64| {
        table
            .iter()
            .take_while(|v| v.tag != 0)
            .find(|v| v.tag == tag)
            .map(|v| v.value as usize)
            .ok_or_else(|| format!("Torch ELF lacks dynamic tag {tag}"))
    };
    let (relocations, length, symbols, strings, string_bytes) =
        (value(23)?, value(2)?, value(6)?, value(5)?, value(10)?);
    if value(20)? != 7
        || value(11)? != std::mem::size_of::<Symbol>()
        || length % std::mem::size_of::<Rela>() != 0
    {
        return Err("unsupported Torch ELF relocation layout".into());
    }
    if !contains(relocations, length) || !contains(strings, string_bytes) {
        return Err("invalid Torch relocation/string bounds".into());
    }
    // SAFETY: validated ELF relocation bounds and expected ELF64 RELA stride.
    let relas = unsafe {
        std::slice::from_raw_parts(
            relocations as *const Rela,
            length / std::mem::size_of::<Rela>(),
        )
    };
    let maps = std::fs::read_to_string("/proc/self/maps").map_err(|e| e.to_string())?;
    let mut slots = Vec::new();
    for relocation in relas {
        let kind = (relocation.info & 0xffff_ffff) as u32;
        let jump_slot = if cfg!(target_arch = "x86_64") {
            7
        } else {
            1026
        };
        if kind != jump_slot {
            continue;
        }
        let symbol_address = symbols
            .checked_add(
                ((relocation.info >> 32) as usize)
                    .checked_mul(std::mem::size_of::<Symbol>())
                    .ok_or("symbol overflow")?,
            )
            .ok_or("symbol overflow")?;
        if !contains(symbol_address, std::mem::size_of::<Symbol>()) {
            return Err("invalid Torch symbol address".into());
        }
        // SAFETY: the complete symbol resides in a loaded ELF segment, checked above.
        let symbol = unsafe { &*(symbol_address as *const Symbol) };
        if symbol.name as usize >= string_bytes {
            return Err("invalid Torch symbol name offset".into());
        }
        // SAFETY: the string table and offset bounds are checked, and scanning stays in the table.
        let remaining = unsafe {
            std::slice::from_raw_parts(
                (strings + symbol.name as usize) as *const u8,
                string_bytes - symbol.name as usize,
            )
        };
        let end = remaining
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("unterminated Torch symbol")?;
        let allocate = match &remaining[..end] {
            b"cudaMalloc" => true,
            b"cudaFree" => false,
            _ => continue,
        };
        let address = base
            .checked_add(relocation.offset as usize)
            .ok_or("slot address overflow")?;
        if address % std::mem::align_of::<AtomicUsize>() != 0
            || !contains(address, std::mem::size_of::<usize>())
        {
            return Err("invalid Torch import slot".into());
        }
        let protection = protection(&maps, address)?;
        slots.push(Slot {
            address,
            original: 0,
            allocate,
            protection,
        });
    }
    if slots.iter().filter(|s| s.allocate).count() != 1
        || slots.iter().filter(|s| !s.allocate).count() != 1
    {
        return Err(format!(
            "{name}: expected one cudaMalloc and one cudaFree import"
        ));
    }
    let name = CString::new(name).map_err(|e| e.to_string())?;
    // SAFETY: dlopen on an already loaded named DSO resolves lazy imports before we save them.
    // Intentionally retain the handle for process lifetime because the hooks outlive Python refs.
    let library = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD) };
    if library.is_null() {
        return Err("cannot retain loaded Torch CUDA library".into());
    }
    for slot in &mut slots {
        let symbol = if slot.allocate {
            c"cudaMalloc"
        } else {
            c"cudaFree"
        };
        // SAFETY: the handle is live and the symbol name is terminated.
        let original = unsafe { libc::dlsym(library, symbol.as_ptr()) } as usize;
        if original == 0 {
            return Err("cannot resolve original CUDA runtime function".into());
        }
        // SAFETY: the aligned relocation slot lies in the checked live mapping.
        let current = unsafe { *(slot.address as *const usize) };
        if current != original {
            return Err("Torch CUDA import is unresolved or already redirected".into());
        }
        slot.original = original;
    }
    Ok((slots, library as usize))
}

fn protection(maps: &str, address: usize) -> Result<i32, String> {
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let Some((start, end)) = fields.next().and_then(|range| range.split_once('-')) else {
            continue;
        };
        let (Ok(start), Ok(end)) = (
            usize::from_str_radix(start, 16),
            usize::from_str_radix(end, 16),
        ) else {
            continue;
        };
        if !(start..end).contains(&address) {
            continue;
        }
        let flags = fields
            .next()
            .ok_or("missing ELF page permissions")?
            .as_bytes();
        if flags.len() < 3 || flags[0] != b'r' || flags[2] == b'x' {
            return Err("refusing non-data Torch import page".into());
        }
        return Ok(libc::PROT_READ
            | if flags[1] == b'w' {
                libc::PROT_WRITE
            } else {
                0
            });
    }
    Err("Torch import slot is not mapped".into())
}

impl Plan {
    pub(super) fn functions(&self) -> Result<Functions, String> {
        let malloc = self
            .slots
            .iter()
            .find(|s| s.allocate)
            .ok_or("missing CUDA allocation import")?
            .original;
        let free = self
            .slots
            .iter()
            .find(|s| !s.allocate)
            .ok_or("missing CUDA free import")?
            .original;
        if self
            .slots
            .iter()
            .any(|s| s.original != if s.allocate { malloc } else { free })
        {
            return Err("Torch DSOs use different CUDA runtime instances".into());
        }
        let mut information = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
        // SAFETY: the original allocation function points into a retained loaded CUDA DSO.
        if unsafe { libc::dladdr(malloc as *const c_void, information.as_mut_ptr()) } == 0 {
            return Err("cannot locate the original CUDA runtime".into());
        }
        // SAFETY: successful dladdr initialized the record and its terminated loaded filename.
        let information = unsafe { information.assume_init() };
        // SAFETY: NOLOAD selects that existing runtime; retain it for the permanent hooks.
        let runtime =
            unsafe { libc::dlopen(information.dli_fname, libc::RTLD_NOW | libc::RTLD_NOLOAD) };
        if runtime.is_null() {
            return Err("cannot retain the original CUDA runtime".into());
        }
        // SAFETY: all names are terminated and runtime is the original allocation's live DSO.
        let (runtime_malloc, device, last_error) = unsafe {
            (
                libc::dlsym(runtime, c"cudaMalloc".as_ptr()),
                libc::dlsym(runtime, c"cudaGetDevice".as_ptr()),
                libc::dlsym(runtime, c"cudaGetLastError".as_ptr()),
            )
        };
        if runtime_malloc as usize != malloc || device.is_null() || last_error.is_null() {
            // SAFETY: release only this failed lookup's extra loader reference, before hooks install.
            unsafe { libc::dlclose(runtime) };
            return Err("cannot resolve error handling in the original CUDA runtime".into());
        }
        // SAFETY: verified named CUDA runtime functions have the declared stable C signatures.
        Ok(unsafe {
            Functions {
                malloc: std::mem::transmute::<usize, super::Malloc>(malloc),
                free: std::mem::transmute::<usize, super::Free>(free),
                device: std::mem::transmute::<*mut c_void, super::GetDevice>(device),
                last_error: std::mem::transmute::<*mut c_void, super::GetLastError>(last_error),
            }
        })
    }

    pub(super) fn install(self, malloc: usize, free: usize) -> Result<usize, String> {
        let mut information = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
        // SAFETY: dladdr initializes this record for an address inside the live hook module.
        if unsafe { libc::dladdr(malloc as *const c_void, information.as_mut_ptr()) } == 0 {
            return Err("cannot locate the native hook module".into());
        }
        // SAFETY: successful dladdr initialized the complete record.
        let information = unsafe { information.assume_init() };
        // SAFETY: dladdr supplies this loaded module's terminated filename. Retain its loader
        // reference forever because native Torch import slots continue pointing into its code.
        if unsafe { libc::dlopen(information.dli_fname, libc::RTLD_NOW | libc::RTLD_NOLOAD) }
            .is_null()
        {
            return Err("cannot retain the native hook module".into());
        }
        // SAFETY: sysconf has no pointer arguments and returns the process page size.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page_size <= 0 {
            return Err("cannot determine import page size".into());
        }
        let page_size = page_size as usize;
        let mut pages = std::collections::BTreeMap::new();
        for slot in &self.slots {
            pages.insert(slot.address / page_size * page_size, slot.protection);
        }
        let mut writable = Vec::new();
        for (&page, &protection) in &pages {
            // SAFETY: this is a mapped data page, validated above; preserve read permission.
            if unsafe {
                libc::mprotect(
                    page as *mut c_void,
                    page_size,
                    protection | libc::PROT_WRITE,
                )
            } != 0
            {
                for (page, protection) in writable {
                    // SAFETY: restore each page whose write permission this call changed.
                    unsafe {
                        libc::mprotect(page as *mut c_void, page_size, protection);
                    }
                }
                return Err("cannot make Torch import data writable".into());
            }
            writable.push((page, protection));
        }
        for slot in &self.slots {
            // SAFETY: the naturally aligned ELF pointer slot is on a writable data page. Atomic
            // replacement prevents tearing if an already-created native thread calls the slot.
            unsafe { &*(slot.address as *const AtomicUsize) }
                .store(if slot.allocate { malloc } else { free }, Ordering::SeqCst);
        }
        for (page, protection) in writable {
            // SAFETY: restore the exact permissions observed before installation.
            if unsafe { libc::mprotect(page as *mut c_void, page_size, protection) } != 0 {
                return Err(
                    "Torch import installed but original page protection could not be restored"
                        .into(),
                );
            }
        }
        Ok(self.slots.len())
    }
}
