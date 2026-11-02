//! Example-owned CUDA stream. The library client itself has no CUDA SDK dependency.
#![allow(unsafe_code)]
use libloading::Library;
use std::ffi::c_void;
type Obj = *mut c_void;
type Op = unsafe extern "system" fn(Obj) -> i32;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn check(code: i32) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(format!("CUDA error {code}").into())
    }
}

pub struct Stream {
    _driver: Library,
    raw: Obj,
    event: Obj,
    record: unsafe extern "system" fn(Obj, Obj) -> i32,
    wait: Op,
    destroy_event: Op,
    destroy_stream: Op,
    copy: unsafe extern "system" fn(u64, u64, usize, Obj) -> i32,
    read: unsafe extern "system" fn(*mut c_void, u64, usize) -> i32,
    pop: unsafe extern "system" fn(*mut Obj) -> i32,
    release: unsafe extern "system" fn(i32) -> i32,
}
impl Stream {
    pub fn new() -> Result<Self> {
        // SAFETY: current CUDA driver signatures; all handles stay owned until close.
        unsafe {
            let driver = Library::new(if cfg!(windows) {
                "nvcuda.dll"
            } else {
                "libcuda.so.1"
            })?;
            check(driver
                .get::<unsafe extern "system" fn(u32) -> i32>(b"cuInit\0")?(
                0
            ))?;
            let mut context = std::ptr::null_mut();
            check(driver
                .get::<unsafe extern "system" fn(*mut Obj, i32) -> i32>(
                    b"cuDevicePrimaryCtxRetain\0",
                )?(&mut context, 0))?;
            check(driver.get::<Op>(b"cuCtxPushCurrent_v2\0")?(context))?;
            let mut raw = std::ptr::null_mut();
            let mut event = std::ptr::null_mut();
            check(driver
                .get::<unsafe extern "system" fn(*mut Obj, u32) -> i32>(
                    b"cuStreamCreate\0",
                )?(&mut raw, 1))?;
            check(driver
                .get::<unsafe extern "system" fn(*mut Obj, u32) -> i32>(
                    b"cuEventCreate\0",
                )?(&mut event, 2))?;
            Ok(Self {
                raw,
                event,
                record: *driver.get(b"cuEventRecord\0")?,
                wait: *driver.get(b"cuEventSynchronize\0")?,
                destroy_event: *driver.get(b"cuEventDestroy_v2\0")?,
                destroy_stream: *driver.get(b"cuStreamDestroy_v2\0")?,
                copy: *driver.get(b"cuMemcpyDtoDAsync_v2\0")?,
                read: *driver.get(b"cuMemcpyDtoH_v2\0")?,
                pop: *driver.get(b"cuCtxPopCurrent_v2\0")?,
                release: *driver.get(b"cuDevicePrimaryCtxRelease_v2\0")?,
                _driver: driver,
            })
        }
    }
    pub fn raw(&self) -> u64 {
        self.raw as u64
    }
    pub fn synchronize(&self) -> Result<()> {
        // SAFETY: both handles remain owned in the current primary context.
        unsafe {
            check((self.record)(self.event, self.raw))?;
            check((self.wait)(self.event))
        }
    }
    /// The caller retains both live Varena allocations through checked completion.
    pub unsafe fn verify_copy(&self, source: u64, destination: u64, expected: &[u8]) -> Result<()> {
        let mut output = vec![0; expected.len()];
        unsafe {
            check((self.copy)(destination, source, output.len(), self.raw))?;
        }
        self.synchronize()?;
        // D2H returns only after the copied host bytes are complete.
        unsafe {
            check((self.read)(
                output.as_mut_ptr().cast(),
                destination,
                output.len(),
            ))?;
        }
        if output != expected {
            return Err("native copy changed bytes".into());
        }
        Ok(())
    }
    /// Call only after all borrowed Stream users and deferred Varena retirements finish.
    pub fn close(self) -> Result<()> {
        self.synchronize()?;
        // SAFETY: the example checked every dependent operation and retirement first.
        unsafe {
            check((self.destroy_event)(self.event))?;
            check((self.destroy_stream)(self.raw))?;
            let mut context = std::ptr::null_mut();
            check((self.pop)(&mut context))?;
            check((self.release)(0))?;
        }
        Ok(())
    }
    // There is deliberately no unchecked CUDA Drop. On a failed proof the example
    // exits with the diagnostic and retains uncertain handles for process teardown.
}
