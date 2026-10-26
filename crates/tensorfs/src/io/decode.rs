//! Checked format plans executed by the prebuilt catalog through the read engine's driver.

use std::collections::BTreeMap;
use std::ffi::{c_void, CString};
use std::sync::{Arc, Mutex, OnceLock};

use super::cuda::{self, CtxGuard};
use super::{Copy, DeviceBuffer, Dst, ReadOpts, Src};
use crate::failure::{Failure, Kind, Result};
use crate::format::decode::plan::{Core, Plan};
use crate::format::{decode, Dtype, Encoding, PartBytes};
use crate::kernels::{self, Binary, Factors, Launch, Prepared};

/// Declared format metadata and an existing contiguous byte span. No store is consulted.
/// `src` supports Host or Device; packed carriers retain their format dtype/shape separately.
pub struct Part<'a> {
    pub role: &'a str,
    pub dtype: &'a Dtype,
    pub shape: &'a [u64],
    pub src: Src<'a>,
    pub len: usize,
}

/// Actual output metadata must match the explicit logical descriptor exactly.
pub struct Output<'a> {
    pub dtype: &'a Dtype,
    pub shape: &'a [u64],
    pub dst: Dst,
}

/// Validate declared roles, dtypes, shapes, lengths and params without reading data or
/// initializing CUDA. The ordinary format registry remains the only scheme schema.
pub fn check(
    encoding: &Encoding,
    dtype: &Dtype,
    shape: &[u64],
    parts: &[decode::PartMetadata<'_>],
) -> Result<()> {
    Ok(decode::check_metadata(dtype, shape, encoding, parts)?)
}

fn invalid<T>(detail: impl Into<String>) -> Result<T> {
    crate::failure::fail(Kind::Invalid, detail)
}

/// Check the existing resident plan/catalog before source I/O, and bound the bytes
/// requested by its private CUDA allocations. Excludes the output, supplied parts,
/// shared read staging, driver/module memory and allocation-granularity overhead.
pub fn device_scratch_bytes(
    device: i32,
    encoding: &Encoding,
    dtype: &Dtype,
    shape: &[u64],
    parts: &[decode::PartMetadata<'_>],
) -> Result<usize> {
    check(encoding, dtype, shape, parts)?;
    if !decode::supports_sources(encoding) {
        return crate::failure::fail(
            Kind::Unsupported,
            "this scheme has no resident device-input plan",
        );
    }
    let family = (cuda::attribute(device, 75)? * 10 + cuda::attribute(device, 76)?) as u32;
    decode::planned_sources(dtype, shape, encoding, parts, |plan, geometry| {
        let count = geometry
            .rows
            .checked_mul(geometry.cols)
            .ok_or_else(|| crate::format::Error::Decode("decode size overflow".into()))?;
        Ok(device_plan(plan, geometry, dtype, 0, count as u64)
            .and_then(|launch| scratch_bytes(&launch, family)))
    })?
}

fn scratch_bytes(launch: &Launch<'_>, family: u32) -> Result<usize> {
    let binary = |stage: &str| -> Result<()> {
        if launch.stage_binary(family, stage).is_none() {
            return crate::failure::fail(
                Kind::Unsupported,
                format!(
                    "no prebuilt CUDA sm{family} {stage}/{} kernel in this build",
                    launch.dtype
                ),
            );
        }
        Ok(())
    };
    let overflow = || Failure {
        kind: Kind::Invalid,
        detail: "decode scratch size overflow".into(),
    };
    let mut bytes = 0usize;
    let mut add = |count: usize, width: usize| -> Result<()> {
        bytes = count
            .checked_mul(width)
            .and_then(|n| bytes.checked_add(n))
            .ok_or_else(overflow)?;
        Ok(())
    };
    binary(launch.kind)?;
    if launch.words.binding().is_none() {
        add(launch.words.host()?.len(), 1)?;
    }
    add(launch.slots.len(), 4)?;
    add(launch.index.len(), 4)?;
    add(1, 4)?; // Device error word.
    add(launch.params.len(), 8)?;
    for factor in [
        &launch.scale,
        &launch.term,
        &launch.table,
        &launch.global,
        &launch.post,
    ] {
        match factor {
            Factors::Host(values) => add(values.len(), 4)?,
            Factors::Slot {
                dtype: Dtype::F32, ..
            } => {}
            Factors::Slot { dtype, count, .. } => {
                let table = decode::elem::table(dtype);
                if !matches!(dtype, Dtype::BF16 | Dtype::F16) && table.is_none() {
                    return crate::failure::fail(
                        Kind::Unsupported,
                        format!("resident factor dtype {dtype} is unsupported"),
                    );
                }
                binary("cast_metadata")?;
                add(*count, 4)?;
                add(table.map_or(1, |v| v.len()), 4)?;
                add(2, 8)?; // Cast count and mode.
            }
        }
    }
    if matches!(launch.kind, "rotated" | "grouped_rotated") {
        binary("regular_h4")?;
        binary(if launch.kind == "grouped_rotated" {
            "requantize"
        } else {
            "unpack"
        })?;
        add(
            usize::try_from(launch.params[0]).map_err(|_| overflow())?,
            8,
        )?; // Two I32 buffers.
        let mut stride = 1u64;
        while stride < launch.params[28] {
            add(launch.params.len(), 8)?; // Each queued rotation has immutable parameters.
            stride = stride.checked_mul(4).ok_or_else(overflow)?;
        }
    }
    Ok(bytes)
}

fn span(ptr: u64, len: usize) -> Result<std::ops::Range<u64>> {
    if len > isize::MAX as usize || (ptr == 0 && len != 0) {
        return invalid("decode buffer has an invalid address or length");
    }
    let end = ptr.checked_add(len as u64).ok_or_else(|| Failure {
        kind: Kind::Invalid,
        detail: "decode buffer address overflow".into(),
    })?;
    Ok(ptr..end)
}

fn alignment(dtype: &Dtype) -> usize {
    match dtype {
        Dtype::F32 | Dtype::I32 => 4,
        Dtype::F16 | Dtype::BF16 => 2,
        _ => 1,
    }
}

/// Decode existing parts into an existing destination. Host parts/output use the reference
/// CPU decoder; supported all-Device parts/output use the SAME typed plan and prebuilt kernels as
/// checkpoint reads. Mixed placement, files and implicit transfers are unsupported.
///
/// Metadata, placement, address, alignment and overlap refusals precede output writes. After
/// a runtime/value failure output contents are unspecified. Success proves completion.
///
/// # Safety
/// Every span must be valid and retained, output exclusively writable, for this call. Device
/// streams must belong to that device's primary context and be valid on the calling thread
/// (including PTDS). On error callers must retain ALL parts/output owners until io::drain for
/// that device AND their producer streams report checked completion; failure is not proof of
/// quiescence. This includes owners shared through DLPack and views of larger allocations.
pub unsafe fn into(
    encoding: &Encoding,
    dtype: &Dtype,
    shape: &[u64],
    parts: &[Part<'_>],
    out: Output<'_>,
) -> Result<()> {
    let (metadata, device) = validate_into(encoding, dtype, shape, parts, &out)?;
    match device {
        None => {
            let host: BTreeMap<_, _> = parts
                .iter()
                .map(|p| {
                    let Src::Host(ptr) = p.src else {
                        unreachable!("placement checked")
                    };
                    // SAFETY: caller owns each checked Host span. Empty null spans are permitted.
                    let bytes = if p.len == 0 {
                        &[]
                    } else {
                        // SAFETY: caller owns each checked host span (contract above); empty spans are never read.
                        unsafe { std::slice::from_raw_parts(ptr, p.len) }
                    };
                    (
                        p.role,
                        PartBytes {
                            dtype: p.dtype,
                            shape: p.shape,
                            bytes,
                        },
                    )
                })
                .collect();
            let decoded = decode::decode(dtype, shape, encoding, &host)?;
            let Dst::Host { ptr, .. } = out.dst else {
                unreachable!("placement checked")
            };
            if !decoded.is_empty() {
                // SAFETY: the output span was checked to hold `decoded.len()` bytes and is the caller's exclusive destination.
                unsafe { std::ptr::copy_nonoverlapping(decoded.as_ptr(), ptr, decoded.len()) };
            }
            Ok(())
        }
        Some(device) => {
            decode::planned_sources(dtype, shape, encoding, &metadata, |plan, geometry| {
                let count = geometry
                    .rows
                    .checked_mul(geometry.cols)
                    .ok_or_else(|| crate::format::Error::Decode("decode size overflow".into()))?;
                Ok(
                    device_plan(plan, geometry, dtype, 0, count as u64).and_then(|launch| {
                        execute_into(device, &launch, parts, Some(out.dst)).map(|_| ())
                    }),
                )
            })?
        }
    }
}

fn validate_into<'a>(
    encoding: &Encoding,
    dtype: &Dtype,
    shape: &[u64],
    parts: &[Part<'a>],
    out: &Output<'_>,
) -> Result<(Vec<decode::PartMetadata<'a>>, Option<i32>)> {
    let metadata: Vec<_> = parts
        .iter()
        .map(|p| decode::PartMetadata {
            role: p.role,
            dtype: p.dtype,
            shape: p.shape,
            len: p.len,
        })
        .collect();
    check(encoding, dtype, shape, &metadata)?;
    if out.dtype != dtype || out.shape != shape {
        return invalid("decode output dtype/shape differs from the logical descriptor");
    }
    if dtype.byte_len(shape) != Some(out.dst.len() as u64) {
        return invalid("decode output byte length differs from the logical descriptor");
    }
    let (output, device) = match out.dst {
        Dst::Host { ptr, len } => (span(ptr as u64, len)?, None),
        Dst::Device {
            device, ptr, len, ..
        } => (span(ptr, len)?, Some(device)),
    };
    if device.is_some() && !(output.start as usize).is_multiple_of(alignment(dtype)) {
        return invalid("decode output address is not aligned for its dtype");
    }
    if device.is_some() && !decode::supports_sources(encoding) {
        return crate::failure::fail(
            Kind::Unsupported,
            "this scheme has no resident device-input plan",
        );
    }
    for part in parts {
        let (pointer, placement) = match part.src {
            Src::Host(ptr) => (ptr as u64, None),
            Src::Device { ptr, device, .. } => (ptr, Some(device)),
            _ => {
                return crate::failure::fail(
                    Kind::Unsupported,
                    "decode inputs must be Host or Device byte spans",
                );
            }
        };
        if placement != device {
            return crate::failure::fail(
                Kind::Unsupported,
                "resident decode requires all inputs and output on the same device or all on Host",
            );
        }
        let input = span(pointer, part.len)?;
        if device.is_some() && !(pointer as usize).is_multiple_of(alignment(part.dtype)) {
            return invalid(format!(
                "decode part {:?} address is not aligned for its format dtype",
                part.role
            ));
        }
        if !input.is_empty()
            && !output.is_empty()
            && input.start < output.end
            && output.start < input.end
        {
            return invalid("decode output overlaps an input part");
        }
    }
    Ok((metadata, device))
}

/// Prepare the existing device decoder without reading input values or submitting its kernels.
///
/// # Safety
/// Parts and output satisfy `into`'s span contract and remain retained through the decoder
/// AND every dependent consumer. Enqueue supplies native producer streams or a prior proof
/// of completion; this object never owns the caller's input/output allocations.
pub unsafe fn prepare_into(
    encoding: &Encoding,
    dtype: &Dtype,
    shape: &[u64],
    parts: &[Part<'_>],
    out: Output<'_>,
) -> Result<DeviceDecode> {
    let (metadata, device) = validate_into(encoding, dtype, shape, parts, &out)?;
    let Some(device) = device else {
        return invalid("prepared decode requires existing device buffers");
    };
    decode::planned_sources(dtype, shape, encoding, &metadata, |plan, geometry| {
        let count = geometry
            .rows
            .checked_mul(geometry.cols)
            .ok_or_else(|| crate::format::Error::Decode("decode size overflow".into()))?;
        Ok(device_plan(plan, geometry, dtype, 0, count as u64)
            .and_then(|launch| prepare_execute(device, &launch, parts, Some(out.dst))))
    })?
}

/// Decode a logical element window through the prebuilt CUDA catalog. Host-only plans are
/// unsupported; logical CPU expansion and upload are never a GPU decode fallback.
pub fn read(
    dtype: &Dtype,
    shape: &[u64],
    encoding: &Encoding,
    parts: &BTreeMap<&str, PartBytes>,
    device: i32,
    begin: u64,
    count: u64,
) -> Result<DeviceBuffer> {
    decode::planned(dtype, shape, encoding, parts, true, |plan, geometry| {
        Ok(device_plan(plan, geometry, dtype, begin, count)
            .and_then(|launch| execute(device, &launch)))
    })?
}

fn device_plan<'a>(
    plan: Plan<'a>,
    geometry: decode::Geometry,
    dtype: &Dtype,
    begin: u64,
    count: u64,
) -> Result<Box<Launch<'a>>> {
    // Refuse before prepare can apply host factors, transpose or round a Values payload.
    if matches!(&plan.core, Core::Values(_)) {
        return crate::failure::fail(
            Kind::Unsupported,
            "GPU decode requires a device core; host value plans are unsupported",
        );
    }
    match kernels::prepare(plan, geometry, dtype, begin, count)? {
        Prepared::Kernel(launch) => Ok(launch),
        Prepared::Host(_) => crate::failure::fail(
            Kind::Unsupported,
            "GPU decode has no prebuilt device core for this plan",
        ),
    }
}

fn io_error(error: super::Error) -> Failure {
    error.source.into()
}

struct UploadHold(Option<DeviceBuffer>);
impl Drop for UploadHold {
    fn drop(&mut self) {
        if let Some(buffer) = self.0.take() {
            super::device::retain_decode(None, vec![buffer]);
        }
    }
}

fn upload(device: i32, bytes: &[u8]) -> Result<DeviceBuffer> {
    let mut hold = UploadHold(Some(DeviceBuffer::new(device, bytes.len())?));
    write(hold.0.as_ref().expect("upload buffer live"), bytes, 0)?;
    Ok(hold.0.take().expect("completed upload buffer"))
}

struct DecodeHold {
    output: Option<DeviceBuffer>,
    inputs: Vec<DeviceBuffer>,
    scratch: Vec<DeviceBuffer>,
    stream: Option<cuda::Stream>,
    complete: bool,
}
impl Drop for DecodeHold {
    fn drop(&mut self) {
        if !self.complete {
            let mut buffers = std::mem::take(&mut self.inputs);
            buffers.append(&mut self.scratch);
            buffers.extend(self.output.take());
            // An external output has no owned allocation until its first argument upload.
            // No private work can precede that registration: casts register their output
            // scratch before launch, and every decode kernel needs the owned slots buffer.
            // Early catalog/context/producer errors therefore have nothing to quarantine.
            // The caller still retains external owners when a producer drain failed.
            if !buffers.is_empty() {
                super::device::retain_decode(self.stream.take(), buffers);
            }
        }
    }
}

fn write(buffer: &DeviceBuffer, bytes: &[u8], stream: usize) -> Result<()> {
    let copy = Copy {
        src: Src::Host(bytes.as_ptr()),
        dst: Dst::Device {
            device: buffer.device(),
            ptr: buffer.ptr(),
            len: bytes.len(),
            stream,
        },
    };
    // SAFETY: source and exclusive destination remain live until the engine has drained.
    unsafe { super::read_staged(&[copy], &ReadOpts::default(), None) }.map_err(io_error)?;
    Ok(())
}

fn bytes<T>(values: &[T]) -> &[u8] {
    // SAFETY: used only for initialized fixed-width integer/float arrays in the kernel ABI.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), std::mem::size_of_val(values)) }
}

struct Module {
    context: usize,
    raw: usize,
    function: usize,
}

impl Drop for Module {
    fn drop(&mut self) {
        if let (Ok(driver), Ok(_context)) = (cuda::driver(), CtxGuard::enter(self.context as _)) {
            // SAFETY: the module was loaded in this context, which is current; it is unloaded once, here.
            unsafe { (driver.module_unload)(self.raw as _) };
        }
    }
}

fn module(context: usize, binary: &'static Binary) -> Result<Arc<Module>> {
    type Key = (usize, u32, &'static str, &'static str);
    static MODULES: OnceLock<Mutex<BTreeMap<Key, Arc<Module>>>> = OnceLock::new();
    let mut modules = MODULES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = (context, binary.family, binary.kind, binary.dtype);
    if let Some(module) = modules.get(&key) {
        return Ok(module.clone());
    }
    let driver = cuda::driver()?;
    let _context = CtxGuard::enter(context as _)?;
    let mut raw = std::ptr::null_mut();
    // SAFETY: `raw` is a writable out-pointer and `binary.image` a live, complete module image.
    cuda::check("cuModuleLoadData", unsafe {
        (driver.module_load_data)(&mut raw, binary.image.as_ptr().cast())
    })?;
    let mut module = Module {
        context,
        raw: raw as usize,
        function: 0,
    };
    let name = CString::new(binary.symbol).map_err(|_| Failure {
        kind: Kind::Invalid,
        detail: "kernel symbol contains NUL".into(),
    })?;
    let mut function = std::ptr::null_mut();
    // SAFETY: `function` is a writable out-pointer, `raw` a loaded module and `name` NUL-terminated.
    cuda::check("cuModuleGetFunction", unsafe {
        (driver.module_get_function)(&mut function, raw, name.as_ptr())
    })?;
    module.function = function as usize;
    let module = Arc::new(module);
    modules.insert(key, module.clone());
    Ok(module)
}

/// Triton3.8 appends global/profile scratch parameters to the eleven source arguments.
/// AOT refuses nonzero scratch requirements, but CUDA still reads BOTH parameter slots.
fn launch_values(values: [u64; 11]) -> [u64; 13] {
    let mut abi = [0; 13];
    abi[..11].copy_from_slice(&values);
    abi
}

fn argument_pointers(values: &mut [u64; 13]) -> [*mut c_void; 13] {
    std::array::from_fn(|i| (&mut values[i] as *mut u64).cast())
}

struct KernelLaunch {
    module: Arc<Module>,
    blocks: u32,
    threads: u32,
    shared: u32,
    values: [u64; 13],
}
impl KernelLaunch {
    fn prepare(context: usize, binary: &'static Binary, n: u64, values: [u64; 11]) -> Result<Self> {
        Ok(Self {
            module: module(context, binary)?,
            blocks: u32::try_from(n.div_ceil(256)).map_err(|_| Failure {
                kind: Kind::Invalid,
                detail: "CUDA decode grid exceeds u32".into(),
            })?,
            threads: binary.warps * 32,
            shared: binary.shared,
            values: launch_values(values),
        })
    }
    fn enqueue(&self, stream: &cuda::Stream) -> Result<()> {
        if self.blocks == 0 {
            return Ok(());
        }
        let mut values = self.values;
        let mut args = argument_pointers(&mut values);
        // SAFETY: the prepared arguments retain all owned buffers; the caller retains
        // external spans. CUDA copies these live scalar arguments during this call.
        cuda::check("cuLaunchKernel", unsafe {
            (cuda::driver()?.launch_kernel)(
                self.module.function as _,
                self.blocks,
                1,
                1,
                self.threads,
                1,
                1,
                self.shared,
                stream.raw() as _,
                args.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        })?;
        Ok(())
    }
}

fn owned_input(hold: &mut DecodeHold, device: i32, data: &[u8]) -> Result<u64> {
    let input = upload(device, data)?;
    let ptr = input.ptr();
    hold.inputs.push(input);
    Ok(ptr)
}

fn source_pointer(parts: &[Part<'_>], index: usize) -> Result<u64> {
    match parts.get(index).map(|p| p.src) {
        Some(Src::Device { ptr, .. }) => Ok(ptr),
        _ => crate::failure::fail(
            Kind::Invalid,
            "decode input slot is not a resident device part",
        ),
    }
}

fn factor_input(
    hold: &mut DecodeHold,
    stages: &mut Vec<KernelLaunch>,
    device: i32,
    launch: &Launch<'_>,
    family: u32,
    factors: &Factors,
    parts: &[Part<'_>],
) -> Result<u64> {
    let (index, dtype, count) = match factors {
        Factors::Host(values) => return owned_input(hold, device, bytes(values)),
        Factors::Slot {
            index,
            dtype,
            count,
        } => (*index, dtype, *count),
    };
    let src = source_pointer(parts, index)?;
    if *dtype == Dtype::F32 {
        return Ok(src);
    }
    let mode = match dtype {
        Dtype::BF16 => 0,
        Dtype::F16 => 1,
        _ if decode::elem::table(dtype).is_some() => 2,
        _ => {
            return crate::failure::fail(
                Kind::Unsupported,
                format!("resident factor dtype {dtype} is unsupported"),
            );
        }
    };
    let len = count.checked_mul(4).ok_or_else(|| Failure {
        kind: Kind::Invalid,
        detail: "factor allocation overflow".into(),
    })?;
    let buffer = DeviceBuffer::new(device, len)?;
    let out = buffer.ptr();
    hold.scratch.push(buffer);
    let table = decode::elem::table(dtype).unwrap_or_else(|| vec![0.0]);
    let table = owned_input(hold, device, bytes(&table))?;
    let params = owned_input(hold, device, bytes(&[count as u64, mode]))?;
    let binary = launch
        .stage_binary(family, "cast_metadata")
        .ok_or_else(|| Failure {
            kind: Kind::Unsupported,
            detail: format!(
                "no prebuilt CUDA sm{family} cast_metadata/{} kernel in this build",
                launch.dtype
            ),
        })?;
    stages.push(KernelLaunch::prepare(
        hold.stream.as_ref().expect("decode stream live").context() as usize,
        binary,
        count as u64,
        [out, src, 0, 0, 0, table, 0, 0, 0, 0, params],
    )?);
    Ok(out)
}

fn execute(device: i32, launch: &Launch<'_>) -> Result<DeviceBuffer> {
    execute_into(device, launch, &[], None)?.ok_or_else(|| Failure {
        kind: Kind::Invalid,
        detail: "decode output missing".into(),
    })
}

/// External parts/output remain owned by the unsafe caller. Unknown completion retains this
/// private stream and ALL engine allocations; io::drain is the checked external-owner boundary.
fn execute_into(
    device: i32,
    launch: &Launch<'_>,
    parts: &[Part<'_>],
    destination: Option<Dst>,
) -> Result<Option<DeviceBuffer>> {
    if launch.params[5] == 0 {
        // Preserve the legacy empty path: no metadata upload, factor cast or catalog
        // module is required. An external output still joins its supplied producers.
        if destination.is_none() {
            return Ok(Some(DeviceBuffer::new(device, 0)?));
        }
        let stream = cuda::Stream::new(device)?;
        let _context = CtxGuard::enter(stream.context())?;
        join_producers(parts, destination)?;
        return Ok(None);
    }
    let mut prepared = prepare_execute(device, launch, parts, destination)?;
    let _context = CtxGuard::enter(
        prepared
            .hold
            .stream
            .as_ref()
            .expect("prepared stream")
            .context(),
    )?;
    join_producers(parts, destination)?;
    // SAFETY: the producer streams just proved completion and all spans stay owned.
    unsafe { prepared.enqueue_after(&[]) }?;
    prepared.finish()?;
    Ok(prepared.hold.output.take())
}

fn join_producers(parts: &[Part<'_>], destination: Option<Dst>) -> Result<()> {
    // Legacy synchronous API accepts caller-thread default streams. Event enqueue
    // accepts only explicit native streams; both paths preserve actual producer order.
    let mut streams = std::collections::BTreeSet::new();
    for part in parts {
        if let Src::Device { stream, .. } = part.src {
            streams.insert(stream);
        }
    }
    if let Some(Dst::Device { stream, .. }) = destination {
        streams.insert(stream);
    }
    for stream in streams {
        // SAFETY: the caller supplied these live producer streams on this OS thread.
        cuda::check("cuStreamSynchronize(decode producer)", unsafe {
            (cuda::driver()?.stream_synchronize)(stream as _)
        })?;
    }
    Ok(())
}

/// One prepared instance of the shared decoder, retaining metadata, scratch and modules.
/// External source/output owners remain the unsafe caller's responsibility.
pub struct DeviceDecode {
    hold: DecodeHold,
    stages: Vec<KernelLaunch>,
    dependencies: Vec<super::device::Event>,
    error: u64,
    device: i32,
    submitted: bool,
    invalid: bool,
    released: bool,
    release_failed: bool,
}
// SAFETY: the sole owner serializes enqueue/finish; every CUDA operation enters the
// stream's retained primary context. No borrowed Rust source or descriptor is stored.
unsafe impl Send for DeviceDecode {}
impl DeviceDecode {
    /// Enqueue only: no allocation, filesystem access or host completion wait.
    ///
    /// # Safety
    /// `streams` names every producer of the retained external spans; each is an explicit
    /// native stream in this device's primary context. Empty means already checked complete.
    /// Retain this instance AND external owners through dependent consumer completion.
    pub unsafe fn enqueue_after(&mut self, streams: &[usize]) -> Result<(usize, u64)> {
        if self.submitted
            || self.released
            || streams.len() > self.dependencies.len()
            || streams.iter().any(|s| *s <= 2)
        {
            return invalid("prepared decode needs one submission and explicit native streams");
        }
        self.submitted = true;
        self.hold.complete = false;
        let stream = self.hold.stream.as_ref().expect("prepared stream");
        let _context = CtxGuard::enter(stream.context())?;
        for (event, producer) in self.dependencies.iter().zip(streams) {
            event.record(*producer as _)?;
            event.wait_on(stream.raw() as _)?;
        }
        // SAFETY: error is this instance's owned i32 device buffer; clear precedes every stage.
        cuda::check("cuMemsetD8Async(decode error)", unsafe {
            (cuda::driver()?.memset_d8_async)(self.error, 0, 4, stream.raw() as _)
        })?;
        for stage in &self.stages {
            stage.enqueue(stream)?;
        }
        Ok((stream.raw(), self.error))
    }
    pub fn submitted(&self) -> bool {
        self.submitted
    }
    /// Release only this decoder's private owners after checked composed completion.
    ///
    /// # Safety
    /// Every dependent consumer is checked complete and actual capture is excluded.
    /// On failure retain this instance until process retirement; partially released
    /// handles are inert and an ambiguous driver release must never be retried.
    pub unsafe fn release_completed(&mut self) -> Result<()> {
        if self.release_failed {
            return crate::failure::fail(Kind::Io, "decoder release was already unproven");
        }
        if self.released {
            return Ok(());
        }
        if !self.hold.complete {
            return crate::failure::fail(Kind::Io, "decoder completion is unproven before release");
        }
        self.released = true;
        self.release_failed = true;
        for event in &mut self.dependencies {
            event.release_checked()?;
        }
        for buffer in self
            .hold
            .inputs
            .iter_mut()
            .chain(&mut self.hold.scratch)
            .chain(self.hold.output.iter_mut())
        {
            // SAFETY: decoder and consumer completion are proven and this owns each allocation.
            unsafe { buffer.release_completed() }?;
        }
        if let Some(stream) = &mut self.hold.stream {
            // SAFETY: finish checked this private stream and all consumers are complete.
            unsafe { stream.release_completed() }?;
        }
        self.release_failed = false;
        Ok(())
    }
    /// Check decoder completion and values. This does not certify a dependent consumer.
    pub fn finish(&mut self) -> Result<()> {
        if !self.submitted {
            return invalid("enqueue prepared decode before finish");
        }
        if !self.hold.complete {
            let stream = self.hold.stream.as_ref().expect("prepared stream");
            stream.drain()?;
            let mut value = [0u8; 4];
            let copy = Copy {
                src: Src::Device {
                    device: self.device,
                    ptr: self.error,
                    stream: stream.raw(),
                },
                dst: Dst::host(&mut value),
            };
            // SAFETY: this object owns the completed error word and the stack destination is live.
            unsafe { super::read(&[copy], &ReadOpts::default()) }.map_err(io_error)?;
            self.invalid = i32::from_le_bytes(value) != 0;
            self.hold.complete = true;
        }
        if self.invalid {
            return crate::failure::fail(
                Kind::Decode,
                "CUDA decode encountered an invalid index or a NaN requantized value",
            );
        }
        Ok(())
    }
}

fn prepare_execute(
    device: i32,
    launch: &Launch<'_>,
    parts: &[Part<'_>],
    destination: Option<Dst>,
) -> Result<DeviceDecode> {
    let width = if launch.dtype == Dtype::F32 { 4 } else { 2 };
    let size = launch.params[5]
        .checked_mul(width)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| Failure {
            kind: Kind::Invalid,
            detail: "CUDA decode size overflow".into(),
        })?;
    let output = if destination.is_none() {
        Some(DeviceBuffer::new(device, size)?)
    } else {
        None
    };
    let mut hold = DecodeHold {
        output,
        inputs: Vec::new(),
        scratch: Vec::new(),
        stream: Some(cuda::Stream::new(device)?),
        complete: false,
    };
    let context = hold.stream.as_ref().expect("decode stream").context();
    let _context = CtxGuard::enter(context)?;
    let family = (cuda::attribute(device, 75)? * 10 + cuda::attribute(device, 76)?) as u32;
    let binary = |stage: &str| {
        launch.stage_binary(family, stage).ok_or_else(|| Failure {
            kind: Kind::Unsupported,
            detail: format!(
                "no prebuilt CUDA sm{family} {stage}/{} kernel in this build",
                launch.dtype
            ),
        })
    };
    let words = match launch.words.binding() {
        Some((index, _)) => source_pointer(parts, index)?,
        None => owned_input(&mut hold, device, launch.words.host()?)?,
    };
    let mut stages = Vec::new();
    let slots = owned_input(&mut hold, device, bytes(&launch.slots))?;
    let scale = factor_input(
        &mut hold,
        &mut stages,
        device,
        launch,
        family,
        &launch.scale,
        parts,
    )?;
    let term = factor_input(
        &mut hold,
        &mut stages,
        device,
        launch,
        family,
        &launch.term,
        parts,
    )?;
    let table = factor_input(
        &mut hold,
        &mut stages,
        device,
        launch,
        family,
        &launch.table,
        parts,
    )?;
    let global = factor_input(
        &mut hold,
        &mut stages,
        device,
        launch,
        family,
        &launch.global,
        parts,
    )?;
    let post = factor_input(
        &mut hold,
        &mut stages,
        device,
        launch,
        family,
        &launch.post,
        parts,
    )?;
    let index = owned_input(&mut hold, device, bytes(&launch.index))?;
    let error = owned_input(&mut hold, device, bytes(&[0i32]))?;
    let params = owned_input(&mut hold, device, bytes(&launch.params))?;
    let output = match destination {
        Some(Dst::Device { ptr, .. }) => ptr,
        None => hold.output.as_ref().expect("decode output").ptr(),
        _ => unreachable!("checked device destination"),
    };
    let call = |stage: &str, out: u64, input: u64, n: u64, parameters: u64| {
        KernelLaunch::prepare(
            context as usize,
            binary(stage)?,
            n,
            [
                out, input, slots, scale, term, table, global, post, index, error, parameters,
            ],
        )
    };
    if size != 0 {
        if matches!(launch.kind, "rotated" | "grouped_rotated") {
            let total = launch.params[0];
            let len = total
                .checked_mul(4)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| Failure {
                    kind: Kind::Invalid,
                    detail: "rotation scratch overflow".into(),
                })?;
            let start = hold.scratch.len();
            hold.scratch.push(DeviceBuffer::new(device, len)?);
            hold.scratch.push(DeviceBuffer::new(device, len)?);
            stages.push(call(
                if launch.kind == "grouped_rotated" {
                    "requantize"
                } else {
                    "unpack"
                },
                hold.scratch[start].ptr(),
                words,
                total,
                params,
            )?);
            let mut current = 0;
            let mut stride = 1u64;
            while stride < launch.params[28] {
                let mut next = launch.params;
                next[31] = stride;
                let parameters = owned_input(&mut hold, device, bytes(&next))?;
                stages.push(call(
                    "regular_h4",
                    hold.scratch[start + 1 - current].ptr(),
                    hold.scratch[start + current].ptr(),
                    total,
                    parameters,
                )?);
                current = 1 - current;
                stride = stride.checked_mul(4).ok_or_else(|| Failure {
                    kind: Kind::Invalid,
                    detail: "rotation stride overflow".into(),
                })?;
            }
            stages.push(call(
                launch.kind,
                output,
                hold.scratch[start + current].ptr(),
                launch.params[5],
                params,
            )?);
        } else {
            stages.push(call(launch.kind, output, words, launch.params[5], params)?);
        }
    }
    let dependencies = (0..4)
        .map(|_| super::device::Event::new(context, false))
        .collect::<cuda::Result<_>>()?;
    // Preparation's own metadata uploads are checked; no decoder input was read yet.
    hold.complete = true;
    Ok(DeviceDecode {
        hold,
        stages,
        dependencies,
        error,
        device,
        submitted: false,
        invalid: false,
        released: false,
        release_failed: false,
    })
}
