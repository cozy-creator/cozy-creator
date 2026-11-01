//! Default-off allocator measurement. Only reset/read/calibrate allocate host memory.
//! Markers use fixed thread-local storage and a monotonic clock; they never call CUDA.
use std::{cell::RefCell, time::Instant};

pub const STRIDE: u64 = 1021;
pub const CAPACITY: usize = 1024;
pub const POINTS: usize = 13;
pub const NAMES: [&str; POINTS] = [
    "bridge_enter",
    "core_enter",
    "pid_enter",
    "pid_exit",
    "capture_enter",
    "capture_acquired",
    "registry_enter",
    "registry_exit",
    "state_enter",
    "state_acquired",
    "bookkeeping_end",
    "core_exit",
    "bridge_exit",
];

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Point {
    BridgeEnter,
    CoreEnter,
    PidEnter,
    PidExit,
    CaptureEnter,
    CaptureAcquired,
    RegistryEnter,
    RegistryExit,
    StateEnter,
    StateAcquired,
    BookkeepingEnd,
    CoreExit,
    BridgeExit,
}

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Operation {
    Alloc,
    Free,
}

#[derive(Clone, Copy)]
pub struct Record {
    pub operation: usize,
    pub sequence: u64,
    pub requested: usize,
    pub capacity: usize,
    pub success: bool,
    pub slow: bool,
    pub nested: bool,
    pub present: u16,
    pub duplicate: u16,
    pub ns: [u64; POINTS],
    callback_failed: bool,
}

impl Record {
    const EMPTY: Self = Self {
        operation: 0,
        sequence: 0,
        requested: 0,
        capacity: 0,
        success: false,
        slow: false,
        nested: false,
        present: 0,
        duplicate: 0,
        ns: [0; POINTS],
        callback_failed: false,
    };

    fn mark(&mut self, point: Point, origin: Instant) {
        let bit = 1 << point as usize;
        if self.present & bit != 0 {
            self.duplicate |= bit;
            return;
        }
        self.ns[point as usize] = origin.elapsed().as_nanos() as u64;
        self.present |= bit;
    }
}

struct State {
    enabled: bool,
    origin: Option<Instant>,
    seen: [u64; 2],
    active: Option<Record>,
    records: [Record; CAPACITY],
    len: usize,
    dropped: u64,
    nested: u64,
}

impl State {
    const fn new() -> Self {
        Self {
            enabled: false,
            origin: None,
            seen: [0; 2],
            active: None,
            records: [Record::EMPTY; CAPACITY],
            len: 0,
            dropped: 0,
            nested: 0,
        }
    }

    fn reset(&mut self, enabled: bool) -> crate::Result<()> {
        if self.active.is_some() {
            return Err(crate::Error::Invalid(
                "cannot reset an active allocator probe",
            ));
        }
        // Touch the entire bounded storage outside the measured allocator loop.
        self.records.fill(Record::EMPTY);
        self.len = 0;
        self.seen = [0; 2];
        self.dropped = 0;
        self.nested = 0;
        self.origin = Some(Instant::now());
        self.enabled = enabled;
        Ok(())
    }

    fn start(&mut self, operation: Operation, requested: usize) -> bool {
        if self.active.is_some() {
            self.nested += 1;
            self.active.as_mut().expect("active").nested = true;
            return false;
        }
        if self.len == CAPACITY {
            self.dropped += 1;
            return false;
        }
        let mut record = Record {
            operation: operation as usize,
            sequence: self.seen[operation as usize],
            requested,
            ..Record::EMPTY
        };
        record.mark(
            Point::BridgeEnter,
            self.origin.expect("reset outside timing"),
        );
        self.active = Some(record);
        true
    }
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State::new()) };
}

/// Enable this thread only; call before the existing host start timestamp.
pub fn reset() -> crate::Result<()> {
    STATE.with(|state| state.borrow_mut().reset(true))
}

#[inline(never)]
pub fn enter(operation: Operation, requested: usize) -> bool {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if !state.enabled {
            return false;
        }
        state.seen[operation as usize] += 1;
        if state.active.is_some() {
            state.nested += 1;
            state.active.as_mut().expect("active").nested = true;
            return false;
        }
        if !state.seen[operation as usize].is_multiple_of(STRIDE) {
            return false;
        }
        state.start(operation, requested)
    })
}

#[inline(never)]
pub fn exit(sampled: bool, success: bool) {
    if !sampled {
        return;
    }
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(mut record) = state.active.take() else {
            return;
        };
        record.mark(
            Point::BridgeExit,
            state.origin.expect("reset outside timing"),
        );
        record.success = success && !record.callback_failed;
        let at = state.len;
        state.records[at] = record;
        state.len += 1;
    });
}

#[inline(never)]
pub fn mark(point: Point) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let origin = state.origin;
        if let Some(record) = &mut state.active {
            record.mark(point, origin.expect("reset outside timing"));
        }
    });
}

#[inline(never)]
pub fn allocation(requested: usize, capacity: usize) {
    STATE.with(|state| {
        if let Some(record) = &mut state.borrow_mut().active {
            record.requested = requested;
            record.capacity = capacity;
        }
    });
}

pub fn failure() {
    STATE.with(|state| {
        if let Some(record) = &mut state.borrow_mut().active {
            record.callback_failed = true;
        }
    });
}

pub fn slow() {
    STATE.with(|state| {
        if let Some(record) = &mut state.borrow_mut().active {
            record.slow = true;
        }
    });
}

pub struct Snapshot {
    pub seen: [u64; 2],
    pub dropped: u64,
    pub nested: u64,
    pub records: Vec<Record>,
}

/// Disable before cloning/serializing. No sampled callback performs allocation or growth.
pub fn read() -> crate::Result<Snapshot> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.active.is_some() {
            return Err(crate::Error::Invalid(
                "cannot read an active allocator probe",
            ));
        }
        state.enabled = false;
        Ok(Snapshot {
            seen: state.seen,
            dropped: state.dropped,
            nested: state.nested,
            records: state.records[..state.len].to_vec(),
        })
    })
}

pub struct Calibration {
    pub clock_pairs_ns: Vec<u64>,
    pub empty_markers: Snapshot,
    pub dispatch_iterations: usize,
    pub disabled_dispatch_ns: u64,
    pub enabled_dispatch_ns: u64,
    pub bundle_pairs: usize,
    pub disabled_bundle_ns: u64,
    pub enabled_bundle_ns: u64,
}

fn empty_marks(operation: Operation) {
    mark(Point::CoreEnter);
    mark(Point::PidEnter);
    mark(Point::PidExit);
    if matches!(operation, Operation::Free) {
        mark(Point::CaptureEnter);
        mark(Point::CaptureAcquired);
    }
    mark(Point::RegistryEnter);
    mark(Point::RegistryExit);
    if matches!(operation, Operation::Alloc) {
        allocation(0, 0);
    }
    mark(Point::StateEnter);
    mark(Point::StateAcquired);
    if matches!(operation, Operation::Free) {
        allocation(0, 0);
    }
    mark(Point::BookkeepingEnd);
    mark(Point::CoreExit);
}

/// CPU-only empty spans, not simulated allocator operations. Run outside GPU timing.
pub fn calibrate() -> crate::Result<Calibration> {
    STATE.with(|state| state.borrow_mut().reset(false))?;
    let mut clocks = Vec::with_capacity(256);
    for _ in 0..256 {
        let now = Instant::now();
        clocks.push(now.elapsed().as_nanos() as u64);
    }
    for operation in [Operation::Alloc, Operation::Free] {
        for _ in 0..128 {
            let sampled = STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.seen[operation as usize] += 1;
                state.start(operation, 0)
            });
            empty_marks(operation);
            exit(sampled, true);
        }
    }
    let empty_markers = read()?;
    let dispatch_iterations = 100_000;
    let mut elapsed = [0; 2];
    for (at, enabled) in [false, true].into_iter().enumerate() {
        STATE.with(|state| state.borrow_mut().reset(enabled))?;
        let start = Instant::now();
        for _ in 0..dispatch_iterations {
            let sampled = enter(Operation::Alloc, 0);
            exit(sampled, true);
        }
        elapsed[at] = start.elapsed().as_nanos() as u64;
        let _ = read()?;
    }
    let bundle_pairs = 50_000;
    let mut bundle = [0; 2];
    for (at, enabled) in [false, true].into_iter().enumerate() {
        STATE.with(|state| state.borrow_mut().reset(enabled))?;
        let start = Instant::now();
        for _ in 0..bundle_pairs {
            for operation in [Operation::Alloc, Operation::Free] {
                let sampled = enter(operation, 0);
                // Unsampled calls still execute every marker/metadata helper, as they do
                // in the real diagnostic build. This is not an allocator simulation.
                empty_marks(operation);
                exit(sampled, true);
            }
        }
        bundle[at] = start.elapsed().as_nanos() as u64;
        let _ = read()?;
    }
    Ok(Calibration {
        clock_pairs_ns: clocks,
        empty_markers,
        dispatch_iterations,
        disabled_dispatch_ns: elapsed[0],
        enabled_dispatch_ns: elapsed[1],
        bundle_pairs,
        disabled_bundle_ns: bundle[0],
        enabled_bundle_ns: bundle[1],
    })
}
