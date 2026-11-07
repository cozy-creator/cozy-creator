//! Typed public Use exports. DLPack storage owns the native export, never a Python wrapper.
use pyo3::{exceptions::PyValueError, ffi, prelude::*};
use std::ffi::c_void;

#[repr(C)]
struct Device {
    kind: i32,
    ordinal: i32,
}
#[repr(C)]
struct Dtype {
    code: u8,
    bits: u8,
    lanes: u16,
}
#[repr(C)]
struct Tensor {
    data: *mut c_void,
    device: Device,
    ndim: i32,
    dtype: Dtype,
    shape: *mut i64,
    strides: *mut i64,
    byte_offset: u64,
}
#[repr(C)]
struct Managed {
    tensor: Tensor,
    context: *mut c_void,
    deleter: Option<unsafe extern "C" fn(*mut Managed)>,
}

struct Held {
    _storage: varena::abi::TensorExport,
    shape: Vec<i64>,
    strides: Option<Vec<i64>>,
    pid: u32,
}

fn invalid(message: &'static str) -> PyErr {
    PyValueError::new_err(message)
}

/// Return the number of addressed bytes relative to the layout's byte offset.
/// This is a span, rather than numel * itemsize: views may overlap or contain holes.
fn layout_span(code: u8, bits: u8, shape: &[i64], strides: Option<&[i64]>) -> PyResult<u64> {
    let supported = match code {
        0 | 1 => matches!(bits, 8 | 16 | 32 | 64),
        2 => matches!(bits, 16 | 32 | 64),
        4 => bits == 16,
        5 => matches!(bits, 64 | 128),
        6 => bits == 8,
        _ => false,
    };
    if !supported {
        return Err(invalid("unsupported DLPack dtype"));
    }
    i32::try_from(shape.len()).map_err(|_| invalid("tensor rank exceeds DLPack int32"))?;
    if shape.iter().any(|&dimension| dimension < 0) {
        return Err(invalid("tensor shape must be nonnegative"));
    }
    if let Some(strides) = strides {
        if strides.len() != shape.len() {
            return Err(invalid("tensor shape and stride ranks differ"));
        }
        if strides.iter().any(|&stride| stride < 0) {
            return Err(invalid("negative tensor strides are unsupported"));
        }
    }
    if shape.contains(&0) {
        return Ok(0);
    }
    let items = if let Some(strides) = strides {
        let mut last = 0_u64;
        for (&dimension, &stride) in shape.iter().zip(strides) {
            let extent = (dimension as u64 - 1)
                .checked_mul(stride as u64)
                .ok_or_else(|| invalid("tensor stride span overflow"))?;
            last = last
                .checked_add(extent)
                .ok_or_else(|| invalid("tensor stride span overflow"))?;
        }
        last.checked_add(1)
            .ok_or_else(|| invalid("tensor stride span overflow"))?
    } else {
        shape.iter().try_fold(1_u64, |items, &dimension| {
            items
                .checked_mul(dimension as u64)
                .ok_or_else(|| invalid("tensor element count overflow"))
        })?
    };
    items
        .checked_mul(u64::from(bits / 8))
        .ok_or_else(|| invalid("tensor byte span overflow"))
}

unsafe extern "C" fn delete(tensor: *mut Managed) {
    if tensor.is_null() {
        return;
    }
    // SAFETY: DLPack transfers this allocation and its context to exactly one consumer.
    let tensor = unsafe { Box::from_raw(tensor) };
    // SAFETY: export creates this context from one Held allocation.
    let held = unsafe { Box::from_raw(tensor.context.cast::<Held>()) };
    if held.pid != std::process::id() {
        // Forked children must not enter inherited native registries or owner mutexes.
        std::mem::forget(held);
    }
    // On the creating process, TensorExport records final consumer tails and hands
    // remaining Source/GPU owners to native retirement. No Python attachment or wait.
}

unsafe extern "C" fn capsule_delete(capsule: *mut ffi::PyObject) {
    // SAFETY: Python calls with a live capsule; a consumed capsule has a different name.
    unsafe {
        if ffi::PyCapsule_IsValid(capsule, c"dltensor".as_ptr()) == 1 {
            delete(ffi::PyCapsule_GetPointer(capsule, c"dltensor".as_ptr()).cast::<Managed>());
        }
    }
}

#[pyfunction]
#[pyo3(signature = (handle, index, code, bits, shape, strides, byte_offset, stream))]
fn _varena_public_use_dlpack(
    py: Python<'_>,
    handle: u64,
    index: usize,
    code: u8,
    bits: u8,
    shape: Vec<i64>,
    strides: Option<Vec<i64>>,
    byte_offset: u64,
    stream: u64,
) -> PyResult<Bound<'_, PyAny>> {
    let span = layout_span(code, bits, &shape, strides.as_deref())?;
    if byte_offset % u64::from(bits / 8) != 0 {
        return Err(invalid("tensor byte offset is not aligned to its dtype"));
    }
    let end = byte_offset
        .checked_add(span)
        .ok_or_else(|| invalid("tensor byte offset overflow"))?;
    if !super::working::installed() {
        return Err(invalid(
            "install tensorfs.varena.torch before exporting Torch storage",
        ));
    }
    let storage = py
        .detach(|| varena::abi::export_use(handle, index, stream))
        .map_err(|error| super::raise(py, error))?;
    if end > storage.len() as u64 {
        return Err(invalid("tensor layout exceeds the logical Region bytes"));
    }
    // Torch requires a zero DLPack byte_offset. Normalize the admitted interior
    // view into its data pointer; the native storage owner still pins the full
    // Region. Empty views may end exactly at its logical boundary, but Torch's
    // record_stream uses the storage DataPtr even when numel is zero. Keep that
    // pointer inside the Region after validating the original offset/span above.
    let pointer = if span == 0 {
        storage.ptr()
    } else {
        storage
            .ptr()
            .checked_add(byte_offset)
            .ok_or_else(|| invalid("tensor interior pointer overflow"))?
    };
    let pointer = usize::try_from(pointer)
        .map_err(|_| invalid("tensor pointer exceeds the host address width"))?;
    let device = storage.device();
    let ndim = shape.len() as i32; // Checked by layout_span.
    let mut held = Box::new(Held {
        _storage: storage,
        shape,
        strides,
        pid: std::process::id(),
    });
    let shape = if ndim == 0 {
        std::ptr::null_mut()
    } else {
        held.shape.as_mut_ptr()
    };
    let strides = held
        .strides
        .as_mut()
        .filter(|_| ndim != 0)
        .map_or(std::ptr::null_mut(), Vec::as_mut_ptr);
    let tensor = Box::into_raw(Box::new(Managed {
        tensor: Tensor {
            data: pointer as *mut c_void,
            device: Device {
                kind: 2,
                ordinal: device,
            },
            ndim,
            dtype: Dtype {
                code,
                bits,
                lanes: 1,
            },
            shape,
            strides,
            byte_offset: 0,
        },
        context: Box::into_raw(held).cast(),
        deleter: Some(delete),
    }));
    // SAFETY: the capsule uniquely owns tensor until consumption. Failure releases
    // storage through the same native deleter, including unconsumed capsule teardown.
    unsafe {
        let capsule = ffi::PyCapsule_New(tensor.cast(), c"dltensor".as_ptr(), Some(capsule_delete));
        if capsule.is_null() {
            delete(tensor);
            return Err(PyErr::fetch(py));
        }
        Ok(Bound::from_owned_ptr(py, capsule))
    }
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(_varena_public_use_dlpack, module)?)
}
