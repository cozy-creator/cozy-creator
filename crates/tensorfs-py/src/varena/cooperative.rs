//! Explicit Linux cooperative owner; no allocator callback invokes Python or this API.
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Mutex};
use varena::{
    cooperative::{self, Accounting, Peer},
    ledger,
};

pyo3::create_exception!(tensorfs, CooperationDeferredError, PyRuntimeError);
pyo3::create_exception!(
    tensorfs,
    CooperationStaleSampleError,
    CooperationDeferredError
);

fn transient(py: Python<'_>, reason: &str, error: PyErr) -> PyErr {
    match error.value(py).setattr("reason", reason) {
        Ok(()) => error,
        Err(error) => error,
    }
}

fn raise(py: Python<'_>, error: cooperative::Error) -> PyErr {
    match error {
        cooperative::Error::Native(error @ varena::Error::CaptureActive) => transient(
            py,
            "capture",
            CooperationDeferredError::new_err(error.to_string()),
        ),
        cooperative::Error::Native(error @ varena::Error::Busy) => transient(
            py,
            "busy",
            CooperationDeferredError::new_err(error.to_string()),
        ),
        cooperative::Error::Ledger(error @ ledger::Error::StaleSample) => transient(
            py,
            "stale_sample",
            CooperationStaleSampleError::new_err(error.to_string()),
        ),
        cooperative::Error::Ledger(error @ ledger::Error::Stale) => transient(
            py,
            "stale",
            CooperationDeferredError::new_err(error.to_string()),
        ),
        cooperative::Error::Ledger(error @ ledger::Error::Busy) => transient(
            py,
            "publication",
            CooperationDeferredError::new_err(error.to_string()),
        ),
        cooperative::Error::Native(error) => super::raise(py, error),
        cooperative::Error::Ledger(error) => PyRuntimeError::new_err(error.to_string()),
    }
}

fn physical(value: ledger::Physical) -> Value {
    let bytes = |b: ledger::Bytes| json!({"live":b.live,"idle":b.idle,"pending":b.pending});
    json!({"weights":bytes(value.weights),"working":bytes(value.working),
        "graph_pools":bytes(value.graph_pools),"caches":bytes(value.caches),
        "total":value.total().expect("validated native physical accounting")})
}

fn accounting(value: Accounting) -> Value {
    json!({"physical":physical(value.physical),"reclaimable":value.reclaimable,
        "pending_release":value.pending_release,"refreshed":value.refreshed})
}

fn row_id(value: ledger::RowId) -> Value {
    json!({"slot":value.slot,"incarnation":value.incarnation})
}

fn round(value: ledger::Round) -> Value {
    json!({"generation":value.generation,"sample_generation":value.sample_generation,
        "requester":row_id(value.requester),"requested":value.requested,"aborted":value.aborted,
        "finished":value.finished(),"completed":value.completed().expect("bounded donations"),
        "donations":value.donations.iter().map(|d|json!({"owner":row_id(d.owner),
            "requested":d.requested,"held_at_ask":d.held_at_ask,"completed":d.completed,
            "pending":d.pending,"answer":format!("{:?}",d.answer)})).collect::<Vec<_>>()})
}

fn sample(value: ledger::Sample) -> Value {
    json!({"generation":value.generation,"capacity_epoch":value.capacity_epoch,
        "total":value.total,"driver_used":value.driver_used,"observed_at":value.observed_at,
        "reserve":value.reserve,"attributable":value.attributable})
}

fn service(value: cooperative::Service) -> Value {
    json!({"round":value.round,"released":value.released,"completed":value.completed,
        "pending":value.pending,"deferred":value.deferred,
        "accounting":value.accounting.map(accounting)})
}

fn snapshot(owner: ledger::RowId, value: ledger::Snapshot) -> Value {
    json!({"owner":row_id(owner),"generation":value.generation,
        "capacity_epoch":value.capacity_epoch,"backing_device":value.backing_device,
        "backing_inode":value.backing_inode,"scope":{"boot":value.scope.boot,
            "uid":value.scope.uid,"user_namespace":value.scope.user_namespace,
            "pid_namespace":value.scope.pid_namespace,"mount_namespace":value.scope.mount_namespace},
        "sample":value.sample.map(sample),"round":value.round.map(round),
        "rows":value.rows.into_iter().map(|r|json!({"id":row_id(r.id),"pid":r.pid,
            "start_ticks":r.start_ticks,"priority":r.priority,"sharing":format!("{:?}",r.sharing),
            "last_use":r.last_use,"want":r.want,"physical":physical(r.physical),"reclaimable":r.reclaimable})).collect::<Vec<_>>()})
}

#[pyclass(name = "_VarenaPeer", module = "tensorfs._ext", frozen)]
pub(super) struct PyPeer {
    // Python methods detach before this lock. Close takes the owner out, unlocks, then drops it.
    peer: Mutex<Option<Peer>>,
    pid: u32,
}

impl PyPeer {
    fn call<T: Send>(
        &self,
        py: Python<'_>,
        operation: impl FnOnce(&Peer) -> cooperative::Result<T> + Send,
    ) -> PyResult<T> {
        if self.pid != std::process::id() {
            return Err(PyRuntimeError::new_err("inherited GPU participant"));
        }
        py.detach(|| {
            let owner = self
                .peer
                .lock()
                .map_err(|_| varena::Error::Poisoned("Python GPU participant mutex".into()))?;
            operation(
                owner
                    .as_ref()
                    .ok_or(varena::Error::Invalid("GPU participant is closed"))?,
            )
        })
        .map_err(|error| raise(py, error))
    }
}

#[pymethods]
impl PyPeer {
    /// Typed transient outcomes and categorical configuration, safe for an automatic owner.
    #[classattr]
    const OWNER_PROTOCOL: u32 = 1;

    #[staticmethod]
    fn initialize() -> PyResult<()> {
        ledger::initialize().map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    #[new]
    fn new(
        py: Python<'_>,
        device: i32,
        uuid: &str,
        directory: PathBuf,
        shared: bool,
        priority: i32,
    ) -> PyResult<Self> {
        let descriptor =
            ledger::Device::parse(uuid).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let sharing = if shared {
            ledger::Sharing::Shared
        } else {
            ledger::Sharing::Dedicated
        };
        let peer = py
            .detach(|| Peer::open(device, descriptor, &directory, sharing, priority))
            .map_err(|error| raise(py, error))?;
        Ok(Self {
            peer: Mutex::new(Some(peer)),
            pid: std::process::id(),
        })
    }

    fn publish(&self, py: Python<'_>, last_use: u64) -> PyResult<String> {
        self.call(py, |peer| {
            Ok(accounting(peer.publish(last_use)?).to_string())
        })
    }

    fn configure(&self, py: Python<'_>, shared: bool, priority: i32) -> PyResult<()> {
        self.call(py, |peer| {
            peer.configure(
                if shared {
                    ledger::Sharing::Shared
                } else {
                    ledger::Sharing::Dedicated
                },
                priority,
            )
        })
    }

    fn sample(&self, py: Python<'_>, observed_at: u64) -> PyResult<String> {
        self.call(py, |peer| Ok(sample(peer.sample(observed_at)?).to_string()))
    }

    fn ask(&self, py: Python<'_>, generation: u64, wanted: u64) -> PyResult<String> {
        self.call(py, |peer| {
            Ok(round(peer.ask(generation, wanted)?).to_string())
        })
    }

    fn service(&self, py: Python<'_>) -> PyResult<String> {
        self.call(py, |peer| Ok(service(peer.service()?).to_string()))
    }

    fn snapshot(&self, py: Python<'_>) -> PyResult<String> {
        self.call(py, |peer| {
            Ok(snapshot(peer.id(), peer.snapshot()?).to_string())
        })
    }

    fn cuda_calls(&self, py: Python<'_>) -> PyResult<u64> {
        self.call(py, Peer::cuda_calls)
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        if self.pid != std::process::id() {
            return Err(PyRuntimeError::new_err("inherited GPU participant"));
        }
        let retired = py
            .detach(|| {
                let mut slot = self
                    .peer
                    .lock()
                    .map_err(|_| varena::Error::Poisoned("Python GPU participant mutex".into()))?;
                let Some(owner) = slot.take() else {
                    return Ok(None);
                };
                match owner.retire() {
                    Ok(retired) => Ok(Some(retired)),
                    Err((owner, error)) => {
                        *slot = Some(owner);
                        Err(error)
                    }
                }
            })
            .map_err(|error| raise(py, error))?;
        // Explicit final close can join GPU work. Neither Python nor this mutex is held.
        py.detach(|| drop(retired));
        Ok(())
    }
}

/// Its handle contains host control only. Native retirement never attaches to Python.
#[pyclass(name = "_VarenaOwner", module = "tensorfs._ext", frozen)]
pub(super) struct PyOwner {
    owner: cooperative::Owner,
    row: ledger::RowId,
}

#[pymethods]
impl PyOwner {
    #[classattr]
    const OWNER_PROTOCOL: u32 = 1;

    #[classattr]
    const PRESSURE_PROTOCOL: u32 = 1;

    #[new]
    fn new(
        py: Python<'_>,
        device: i32,
        uuid: &str,
        directory: PathBuf,
        shared: bool,
        priority: i32,
    ) -> PyResult<Self> {
        let descriptor =
            ledger::Device::parse(uuid).map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        let owner = py
            .detach(|| {
                cooperative::Owner::open(
                    device,
                    descriptor,
                    &directory,
                    if shared {
                        ledger::Sharing::Shared
                    } else {
                        ledger::Sharing::Dedicated
                    },
                    priority,
                )
            })
            .map_err(|e| raise(py, e))?;
        let row = owner.id();
        Ok(Self { owner, row })
    }

    fn wake(&self, py: Python<'_>, used_at: u64) -> PyResult<()> {
        py.detach(|| self.owner.wake(used_at))
            .map_err(|e| raise(py, e))
    }

    fn request_boundary(&self, py: Python<'_>, begin: bool) -> PyResult<()> {
        let boundary = if begin {
            cooperative::Boundary::RequestBegin
        } else {
            cooperative::Boundary::RequestEnd
        };
        py.detach(|| self.owner.boundary(boundary))
            .map_err(|e| raise(py, e))
    }

    fn facts(&self, py: Python<'_>) -> PyResult<String> {
        let facts = py.detach(|| self.owner.facts()).map_err(|e| raise(py, e))?;
        let pressure = &facts.pressure;
        Ok(
            json!({"active":facts.active,"stopping":facts.stopping,"retired":facts.retired,
                "pressure":{"samples":pressure.samples,"observed_at":pressure.observed_at,
                    "free":pressure.free,"total":pressure.total,"own_opaque":pressure.own_opaque,
                    "foreign":pressure.foreign,"attribution":pressure.attribution,
                    "reserve":pressure.reserve,"measured_requests":pressure.measured_requests,
                    "local_reserve":pressure.local_reserve,"process_signals":pressure.process_signals,
                    "measurement":pressure.measurement,
                    "process_signals_served":pressure.process_signals_served,
                    "rejected_requests":pressure.rejected_requests,"new_processes":pressure.new_processes,
                    "suppressed":pressure.suppressed,"round":pressure.round,"completed":pressure.completed,
                    "budget_exhausted":pressure.budget_exhausted},
                "published":facts.published,"serviced":facts.serviced,
            "deferred":facts.deferred,"physical_bytes":facts.physical_bytes,
                "last_donation":facts.last_donation.map(service),"failed":facts.failed,
                "recoveries":facts.recoveries,"active_recoveries":facts.active_recoveries,
                "pending_withdrawals":facts.pending_withdrawals,"withdrawn_rounds":facts.withdrawn_rounds,
                "demand_clear_pending":facts.demand_clear_pending,
                "last_withdrawn_round":facts.last_withdrawn_round,"last_recovery":facts.last_recovery.map(|value|
                    json!({"outcome":value.outcome.as_str(),"requested_bytes":value.requested_bytes,
                        "released_bytes":value.released_bytes,"round":value.round,
                        "completed_bytes":value.completed_bytes,"elapsed_ns":value.elapsed_ns}))})
            .to_string(),
        )
    }

    fn configure(&self, py: Python<'_>, shared: bool, priority: i32) -> PyResult<()> {
        py.detach(|| {
            self.owner.configure(
                if shared {
                    ledger::Sharing::Shared
                } else {
                    ledger::Sharing::Dedicated
                },
                priority,
            )
        })
        .map_err(|e| raise(py, e))
    }

    fn snapshot(&self, py: Python<'_>) -> PyResult<String> {
        py.detach(|| self.owner.snapshot())
            .map(|value| snapshot(self.row, value).to_string())
            .map_err(|e| raise(py, e))
    }

    fn cuda_calls(&self, py: Python<'_>) -> PyResult<u64> {
        py.detach(|| self.owner.cuda_calls())
            .map_err(|e| raise(py, e))
    }

    fn close(&self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| self.owner.close()).map_err(|e| raise(py, e))
    }
}

pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyPeer>()?;
    module.add_class::<PyOwner>()?;
    let deferred = module.py().get_type::<CooperationDeferredError>();
    deferred.setattr("__module__", "tensorfs._ext")?;
    module.add("CooperationDeferredError", deferred)?;
    let stale = module.py().get_type::<CooperationStaleSampleError>();
    stale.setattr("__module__", "tensorfs._ext")?;
    module.add("CooperationStaleSampleError", stale)?;
    Ok(())
}

impl Drop for PyPeer {
    fn drop(&mut self) {
        if let Some(owner) = self
            .peer
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            if self.pid == std::process::id() {
                crate::cleanup::defer(Retire(Some(owner)));
            } else {
                std::mem::forget(owner);
            }
        }
    }
}

struct Retire(Option<Peer>);
impl Drop for Retire {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            if let Err((owner, error)) = owner.try_close() {
                if matches!(
                    error,
                    cooperative::Error::Native(varena::Error::CaptureActive)
                ) {
                    // Same queue/retry pattern as Unready and Abandoned. Do not block the
                    // cleanup queue behind a capture that may itself need other releases.
                    std::thread::sleep(std::time::Duration::from_millis(1));
                    crate::cleanup::defer(Self(Some(owner)));
                } else {
                    // Unknown ownership/fault state cannot authorize final GPU destruction.
                    std::mem::forget(owner);
                }
            }
        }
    }
}
