//! Actual native service retirement while a real CUDA capture prevents its completion.
use super::*;
use cooperative::Owner;

fn observe_close(owner: &Owner) -> Result<bool> {
    let limit = Instant::now() + Duration::from_secs(10);
    loop {
        match owner.try_close() {
            Ok(completed) => return Ok(completed),
            Err(cooperative::Error::Native(varena::Error::Busy)) => {}
            Err(error) => return Err(error.into()),
        }
        require(Instant::now() < limit, "host try_close stayed busy")?;
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn complete(owner: &Owner) -> Result<()> {
    let limit = Instant::now() + Duration::from_secs(10);
    while !observe_close(owner)? {
        require(
            Instant::now() < limit,
            "native service retirement did not complete",
        )?;
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn in_capture<T>(cuda: &Cuda, device: i32, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    type Begin = unsafe extern "C" fn(Ctx, i32) -> i32;
    type End = unsafe extern "C" fn(Ctx, *mut Ctx) -> i32;
    type Destroy = unsafe extern "C" fn(Ctx) -> i32;
    let (begin, end, destroy) = unsafe {
        (
            *cuda._library.get::<Begin>(b"cuStreamBeginCapture_v2\0")?,
            *cuda._library.get::<End>(b"cuStreamEndCapture\0")?,
            *cuda._library.get::<Destroy>(b"cuGraphDestroy\0")?,
        )
    };
    let stream = probes::Stream::new(cuda)?;
    let guard = varena::enter_capture(device)?;
    check("cuStreamBeginCapture_v2", unsafe { begin(stream.ptr, 0) })?;
    let result = operation();
    let mut graph = std::ptr::null_mut();
    let status = unsafe { end(stream.ptr, &mut graph) };
    if status != 0 {
        let _ = emit(
            &json!({"event":"result","passed":false,"capture_end_status":status,
            "boundary":"no GPU-owning teardown after unproven capture end"}),
        );
        unsafe {
            libc::_exit(3);
        }
    }
    drop(guard);
    if !graph.is_null() {
        check("cuGraphDestroy", unsafe { destroy(graph) })?;
    }
    result
}

pub(super) fn poll(
    cuda: &Cuda,
    observer: &cooperative::Peer,
    device: i32,
    directory: &str,
    uuid: &str,
) -> Result<Value> {
    let descriptor = ledger::Device::parse(uuid)?;
    let directory = std::path::Path::new(directory);
    let owner = Owner::get_or_open(
        device,
        descriptor,
        directory,
        ledger::Sharing::Dedicated,
        17,
    )?;
    let other = Owner::get_or_open(device, descriptor, directory, ledger::Sharing::Shared, -99)?;
    let row = owner.id();
    require(
        other.id() == row,
        "two real clients must share the native participant",
    )?;
    require(observe_close(&other)?, "nonfinal close must finish")?;
    require(
        owner.facts()?.active,
        "nonfinal close stopped the surviving client",
    )?;
    let capture = in_capture(cuda, device, || {
        let before = observer.cuda_calls()?;
        require(
            !observe_close(&owner)?,
            "capture must keep final retirement pending",
        )?;
        require(
            !observe_close(&owner)?,
            "pending final close must remain retryable",
        )?;
        require(
            !owner.facts()?.retired,
            "pending owner falsely reported retired",
        )?;
        require(
            observer
                .snapshot()?
                .rows
                .iter()
                .any(|value| value.id == row),
            "pending native participant disappeared",
        )?;
        require(
            matches!(
                Owner::join_existing(device),
                Err(cooperative::Error::Native(varena::Error::Busy))
            ),
            "stopping service admitted a replacement client",
        )?;
        let after = observer.cuda_calls()?;
        require(before == after, "try_close/capture made native CUDA calls")?;
        Ok(json!({"actual_cuda_capture":true,"pending_twice":true,
            "row_retained":true,"native_driver_before":before,"native_driver_after":after}))
    })?;
    complete(&owner)?;
    require(
        owner.facts()?.retired,
        "completion ticket missing retired state",
    )?;
    require(
        !observer
            .snapshot()?
            .rows
            .iter()
            .any(|value| value.id == row),
        "completed participant row remains",
    )?;
    let next = Owner::get_or_open(device, descriptor, directory, ledger::Sharing::Shared, 31)?;
    require(next.id() != row, "service generation did not advance")?;
    in_capture(cuda, device, || {
        let before = observer.cuda_calls()?;
        require(
            observe_close(&owner)?,
            "old ticket consulted new capture/service",
        )?;
        require(next.facts()?.active, "old ticket stopped the new service")?;
        require(
            observer.cuda_calls()? == before,
            "old ticket made native CUDA calls",
        )
    })?;
    let next_row = next.id();
    complete(&next)?;
    require(
        !observer
            .snapshot()?
            .rows
            .iter()
            .any(|value| value.id == next_row),
        "replacement participant failed to retire",
    )?;
    Ok(
        json!({"capture":capture,"nonfinal_closed":true,"final_completed":true,
        "old_ticket_ignored_new_capture":true,
        "owner":{"slot":row.slot,"incarnation":row.incarnation},
        "next_owner":{"slot":next_row.slot,"incarnation":next_row.incarnation},
        "scope":"actual native Owner service; manual Peer is instrumentation only; no physical donation or default-discovery claim"}),
    )
}
