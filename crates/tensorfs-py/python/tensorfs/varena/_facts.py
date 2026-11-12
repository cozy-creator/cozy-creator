"""The records varena's native calls return, by key: `_ext.pyi` names them, and their readers type
what they read with them. Plain dicts at run time."""

from typing import Literal, TypedDict


class ReclaimedByReason(TypedDict):
    allocator: int
    room: int


class DeviceStats(TypedDict):
    reserved_bytes: int
    resident_bytes: int
    ready_bytes: int
    pinned_bytes: int
    reclaimable_bytes: int
    regions: int
    fills: int
    hits: int
    evictions: int
    reclaimed_bytes: int
    prefetch_unused_bytes: int
    context_joins: int
    driver_physical_ooms: int
    event_waits: int
    reclaimed_by_reason: ReclaimedByReason
    poisoned: bool


class PagesStats(TypedDict):
    scope: str
    pages: int
    bytes: int
    room: int
    cap: int
    headroom: int
    pressed: bool
    pinned: bool
    refused: str | None
    wanted: int
    pin_failed: int
    reserved: int
    reserved_ahead: int
    ahead_canceled: int
    ahead_failed: int
    keep_ns: int
    reserve_ns: int
    let_go_scope: str
    let_go: dict[str, int]
    making: int
    freeing: int
    store_reads_scope: str
    #: (set, block, O_DIRECT bytes, cached bytes, ns)
    store_reads: list[tuple[str, int, int, int, int]]
    read_ahead_scope: str
    read_ahead: int
    read_ahead_bytes: int
    read_ahead_refused: int
    persisted_scope: str
    persisted: int
    persisted_bytes: int
    persist_skipped: int
    persist_failed: int


class PlanPhase(TypedDict):
    name: str
    regions: int
    bytes: int


class PlanFacts(TypedDict):
    device: int
    scope: str
    phases: list[PlanPhase]


class CacheOwnerFacts(TypedDict):
    id: int
    registered: bool
    requestable: bool
    closing: bool
    running: bool
    retired: bool
    callbacks: int
    returned: int
    declined: int
    deferred: int
    failed: str | None
    retirement_failed: str | None


class CacheRequestFacts(TypedDict):
    queued: int
    running: bool
    returned: int
    declined: int
    failed: int
    finished: bool
    cancelled: bool
    observed_release_bytes: int


class Room(TypedDict):
    capacity: int
    working_live: int
    pinned: int
    weights_resident: int
    free: int
    driver_reserve: int


class DriverShort(TypedDict):
    """`torch.driver_short`'s answer: the reserve every map now leaves the driver, whether this
    call grew it, the bytes released for it, the card's free bytes after, and what other programs
    took since this process's quietest sample (what waiting may bring back)."""

    reserve: int
    grew: int
    released: int
    free: int
    others: int


class WorkingStats(TypedDict):
    weights_resident: int
    weights_filled: int
    driver_reservation: int
    allocated_bytes: int
    allocated_peak: int
    mapped_bytes: int
    mapped_peak: int
    pending_bytes: int
    pools_bytes: int
    allocated_capacity: int
    free_bytes: int
    reclaimable_bytes: int
    unassigned_bytes: int
    pools: int
    event_handles: int
    allocations: int
    frees: int
    ooms: int
    driver_memory_calls: int
    driver_physical_ooms: int
    driver_memory_ns: int
    driver_memory_longest_ns: int
    observed_mapped: int
    observed_retired: int
    observed_ns: int
    reclaim_ns: int
    reclaim_longest_ns: int
    reclaim_longest_step: int
    reclaim_completed_working_ns: int
    reclaim_contentless_ns: int
    reclaim_idle_pools_ns: int
    reclaim_pending_working_ns: int
    reclaim_idle_peers_ns: int
    reclaim_weights_ns: int
    reclaim_join_ns: int
    reclaim_owner_caches_ns: int
    reclaim_peers_ns: int
    reclaim_high_weights_ns: int
    event_queries: int
    event_records: int
    event_waits: int
    context_joins: int
    prefetch_attempts: int
    prefetch_admissions: int
    prefetch_declines: int
    split_decisions: int
    split_lookups: int
    room_moves: int
    room_outside_moves: int
    cache_reserved_bytes: int
    cache_mapped_bytes: int
    cache_live_bytes: int
    cache_pending_bytes: int


class SplitContext(TypedDict):
    eligible: bool
    reason: str


class SplitSelection(TypedDict):
    stream: int
    allocation_backing_bytes: int
    choice_id: str
    eligible: list[str]
    required_bytes: int
    available_bytes: int
    basis: str
    plan_generation: int
    phase: str | None
    phase_weight_bytes: int
    budget_bytes: int
    capacity_bytes: int
    live_bytes: int


class SplitObservation(TypedDict):
    observation_id: int
    key: str
    choice_id: str
    peak_bytes: int | None
    backing_peak_bytes: int | None
    observed: bool
    pending: bool
    published: bool
    reason: str
    successful_passes: int
    refused_bytes: int


PhaseMode = Literal["eager", "capture", "replay", "preparation"]


class PhaseObservation(TypedDict):
    split_observations: list[SplitObservation]
    observation_id: int
    mode: PhaseMode
    key: str | None
    peak_bytes: int
    total_peak_bytes: int
    backing_peak_bytes: int
    published: bool
    invalidated: bool
    reason: str
    successful_peak_bytes: int
    successful_passes: int


class PhaseControllerFacts(TypedDict):
    transitions: int
    active: str | None
    observations: list[PhaseObservation]
    active_observations: int
    policy: str


class Verdict(TypedDict):
    """A pass refused (`_varena_pass_mark`): what it needed against the room."""

    need_bytes: int
    room_bytes: int
    held_bytes: int
    escaped_bytes: int
    transient_bytes: int
    floor_bytes: int
    learned: bool
    #: (site, bytes)
    sites: list[tuple[str, int]]


class EscapeRecord(TypedDict):
    head: str
    passes: int
    carried_bytes: int
    carried_sites: list[tuple[str, int]]
    escaped_bytes: int
    escaped_peak_bytes: int
    escaped_sites: list[tuple[str, int]]
    growth_bytes: int
    transient_bytes: int | None
    refused: Verdict | None
