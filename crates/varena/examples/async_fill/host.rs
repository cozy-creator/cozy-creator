//! Bounded HOST VMM fixture resource, matching native host Source ownership stages.
use super::*;
#[repr(C)]
struct Location { kind: i32, id: i32 }
#[repr(C)]
struct Properties { kind: i32, handles: i32, location: Location, windows: Obj, flags: [u8; 8] }
#[repr(C)]
struct Access { location: Location, flags: i32 }
#[derive(Default)]
pub(super) struct Allocation { pointer: u64, handle: u64, bytes: usize, mapped: bool }
impl Allocation { pub(super) fn pointer(&self) -> usize { self.pointer as usize } }
pub(super) struct Api {
    granularity: unsafe extern "system" fn(*mut usize, *const Properties, i32) -> i32,
    create: unsafe extern "system" fn(*mut u64, usize, *const Properties, u64) -> i32,
    reserve: unsafe extern "system" fn(*mut u64, usize, usize, u64, u64) -> i32,
    map: unsafe extern "system" fn(u64, usize, usize, u64, u64) -> i32,
    access: unsafe extern "system" fn(u64, usize, *const Access, usize) -> i32,
    unmap: unsafe extern "system" fn(u64, usize) -> i32,
    release: unsafe extern "system" fn(u64) -> i32,
    address_free: unsafe extern "system" fn(u64, usize) -> i32,
    device: i32,
}
impl Api {
    pub(super) unsafe fn load(library: &Library, device: i32) -> Result<Self> {
        unsafe { Ok(Self {
            granularity: *library.get(b"cuMemGetAllocationGranularity\0")?,
            create: *library.get(b"cuMemCreate\0")?, reserve: *library.get(b"cuMemAddressReserve\0")?,
            map: *library.get(b"cuMemMap\0")?, access: *library.get(b"cuMemSetAccess\0")?,
            unmap: *library.get(b"cuMemUnmap\0")?, release: *library.get(b"cuMemRelease\0")?,
            address_free: *library.get(b"cuMemAddressFree\0")?, device,
        }) }
    }
    pub(super) fn prepare(&self, a: &mut Allocation) -> Result<()> {
        const BYTES: usize = 2 << 20;
        let p = Properties { kind: 1, handles: 0, location: Location { kind: 2, id: 0 }, windows: std::ptr::null_mut(), flags: [0; 8] };
        let mut granularity = 0;
        unsafe { check("fixture HOST granularity", (self.granularity)(&mut granularity, &p, 0))?; }
        require(granularity > 0 && granularity <= BYTES && BYTES.is_multiple_of(granularity), "fixture HOST backing exceeds two MiB")?;
        let mut handle = 0;
        unsafe { check("fixture HOST create", (self.create)(&mut handle, BYTES, &p, 0))?; }
        a.handle = handle; a.bytes = BYTES;
        let mut pointer = 0;
        unsafe { check("fixture HOST reserve", (self.reserve)(&mut pointer, BYTES, 0, 0, 0))?; }
        a.pointer = pointer;
        unsafe { check("fixture HOST map", (self.map)(pointer, BYTES, 0, handle, 0))?; }
        a.mapped = true;
        let access = [Access { location: Location { kind: 2, id: 0 }, flags: 3 }, Access { location: Location { kind: 1, id: self.device }, flags: 3 }];
        unsafe { check("fixture HOST CPU+GPU access", (self.access)(pointer, BYTES, access.as_ptr(), access.len()))?; }
        Ok(())
    }
    pub(super) fn release(&self, a: &mut Allocation) -> varena::Result<()> {
        let status = |operation, code| if code == 0 { Ok(()) } else { Err(varena::Error::Cuda { operation, code }) };
        if a.mapped { unsafe { status("fixture HOST unmap", (self.unmap)(a.pointer, a.bytes))?; } a.mapped = false; }
        if a.handle != 0 { unsafe { status("fixture HOST release", (self.release)(a.handle))?; } a.handle = 0; }
        if a.pointer != 0 { unsafe { status("fixture HOST address free", (self.address_free)(a.pointer, a.bytes))?; } a.pointer = 0; }
        a.bytes = 0;
        Ok(())
    }
}
