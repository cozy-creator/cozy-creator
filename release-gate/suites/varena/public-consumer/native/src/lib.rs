//! Qualification-only access to existing native metadata and fixture-owned Working.
//! Build in the exact TensorFS/private-Rust/std cohort; never embed another allocator.
#![allow(unsafe_code)]
#![cfg(all(target_os = "linux", target_has_atomic = "64"))]
use serde_json::{Value, json};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::sync::{Mutex, OnceLock};
use varena::{cooperative::Owner, ledger};

type Result<T> = std::result::Result<T, String>;
thread_local! { static TEXT: RefCell<CString> = RefCell::new(CString::default()); }
fn text(value: String) -> *const c_char {
    TEXT.with(|slot| {
        *slot.borrow_mut() = CString::new(value.replace('\0', "?")).expect("no NUL");
        slot.borrow().as_ptr()
    })
}
fn boundary(operation: impl FnOnce() -> Result<()>) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            text(error);
            1
        }
        Err(_) => {
            text("fixture helper panic".into());
            2
        }
    }
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
unsafe fn string(pointer: *const c_char) -> Result<String> {
    if pointer.is_null() {
        return Err("null string".into());
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map(str::to_owned)
        .map_err(error)
}
#[unsafe(no_mangle)]
pub extern "C" fn probe_last_error() -> *const c_char {
    TEXT.with(|slot| slot.borrow().as_ptr())
}
#[unsafe(no_mangle)]
pub extern "C" fn probe_core_identity() -> u64 {
    varena::abi::vr_v1_core_identity()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn probe_inventory(device: i32, out: *mut u64) -> i32 {
    boundary(|| {
        if out.is_null() {
            return Err("null inventory output".into());
        }
        unsafe {
            *out = varena::plan::inventory_generation(device).map_err(error)?;
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn probe_alloc(device: i32, bytes: usize, stream: u64, out: *mut u64) -> i32 {
    boundary(|| {
        if out.is_null() || stream <= 2 {
            return Err("explicit stream and output required".into());
        }
        unsafe {
            *out = 0;
            *out = varena::working::alloc(device, bytes, stream, None).map_err(error)?;
        }
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn probe_free(device: i32, pointer: u64, stream: u64) -> i32 {
    boundary(|| varena::working::free(device, pointer, stream).map_err(error))
}
#[unsafe(no_mangle)]
pub extern "C" fn probe_retire_stream(device: i32, stream: u64) -> i32 {
    boundary(|| varena::working::retire_stream(device, stream).map_err(error))
}
#[unsafe(no_mangle)]
pub extern "C" fn probe_trim(device: i32) -> i32 {
    boundary(|| varena::working::trim(device).map(|_| ()).map_err(error))
}
struct Observer {
    owner: Owner,
    table: ledger::Ledger,
}
static OBSERVER: OnceLock<Mutex<Option<Observer>>> = OnceLock::new();
#[unsafe(no_mangle)]
pub unsafe extern "C" fn probe_observer(device: i32, uuid: *const c_char) -> i32 {
    let mut deferred = false;
    let status = boundary(|| {
        let uuid = unsafe { string(uuid)? };
        let directory = varena::cooperative::default_directory().map_err(error)?;
        let mut observer = OBSERVER.get_or_init(Mutex::default).lock().map_err(error)?;
        if observer.is_some() {
            return Err("observer already installed".into());
        }
        // The first public Gpu already created the service. This temporary execution
        // client is closed before any custody assertions; the retained Control is host-only.
        let owner = Owner::get_or_open_default(device).map_err(error)?;
        let table = match ledger::Ledger::open_nonblocking(
            &directory,
            ledger::Device::parse(&uuid).map_err(error)?,
        ) {
            Ok(table) => table,
            Err(ledger::Error::Busy) => {
                deferred = true;
                return Err("ledger publication is busy".into());
            }
            Err(value) => return Err(error(value)),
        };
        if owner.facts().map_err(error)?.clients < 2 {
            return Err("no public client retained the observer's service".into());
        }
        // This is a nonfinal client close before submitting the gate, outside every
        // allocation/capture guard; it cannot wait for final GPU service retirement.
        owner.close().map_err(error)?;
        *observer = Some(Observer { owner, table });
        Ok(())
    });
    if deferred { 3 } else { status }
}
fn bytes(value: ledger::Bytes) -> Value {
    json!({"live": value.live, "idle": value.idle, "pending": value.pending})
}
#[unsafe(no_mangle)]
pub extern "C" fn probe_observer_facts() -> *const c_char {
    let result = (|| -> Result<Value> {
        let held = OBSERVER.get_or_init(Mutex::default).lock().map_err(error)?;
        let observer = held.as_ref().ok_or("no observer")?;
        let facts = observer.owner.facts().map_err(error)?;
        let id = observer.owner.id();
        let table = match observer.table.snapshot() {
            Ok(table) => table,
            Err(ledger::Error::Busy) => return Ok(json!({"deferred":true})),
            Err(value) => return Err(error(value)),
        };
        let row = table.rows.iter().find(|row| row.id == id).map(|row| json!({
            "id": {"slot": row.id.slot, "incarnation": row.id.incarnation},
            "pid": row.pid, "start_ticks": row.start_ticks,
            "activity": format!("{:?}", row.activity),
            "working": bytes(row.physical.working), "weights": bytes(row.physical.weights),
            "caches": bytes(row.physical.caches), "graph_pools": bytes(row.physical.graph_pools),
        }));
        Ok(
            json!({"owner": {"slot": id.slot, "incarnation": id.incarnation},
            "clients": facts.clients, "passive_holds": facts.passive_holds,
            "closed": facts.closed, "active": facts.active, "stopping": facts.stopping,
            "retired": facts.retired, "activity": format!("{:?}", facts.activity),
            "published": facts.published, "measured_requests": facts.pressure.measured_requests,
            "failed": facts.failed, "row": row,
            "table_generation": table.generation}),
        )
    })();
    text(match result {
        Ok(value) => value.to_string(),
        Err(error) => json!({"error":error}).to_string(),
    })
}
