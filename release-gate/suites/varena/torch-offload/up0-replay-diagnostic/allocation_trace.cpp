// Diagnostic-only: delegate the installed allocator, recording actual requests.
#include "diagnostic_markers.h"
#include <ATen/record_function.h>
#include <ATen/core/Tensor.h>
#include <ATen/cuda/CUDAContextLight.h>
#include <torch/csrc/cuda/CUDAPluggableAllocator.h>
#include <atomic>
#include <algorithm>
#include <array>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <memory>
#include <mutex>
#include <sstream>
#include <thread>
#include <vector>

namespace {
namespace ca = c10::cuda::CUDACachingAllocator;
namespace plug = torch::cuda::CUDAPluggableAllocator;
struct Row {
    int kind{}, stage{}, device{-1}, dims{}, dtype{}, argument{-1}, nesting{};
    uint64_t pointer{}, bytes{}, stream{}, thread{}, a{}, b{};
    int64_t offset{}, shape[8]{}, stride[8]{};
    char op[96]{};
    uint64_t monotonic_ns{}, graph_identity{}, plan_handle{};
    uint32_t marker{}, invocation{};
};
std::array<Row, 131072> rows;
std::mutex mutex;
size_t used{};
std::atomic<bool> lost{false};
std::atomic<int> stage{0};
std::atomic<bool> native_window{false}, observer_attached{false};
std::atomic<uint64_t> unsupported_observer_events{0}, inactive_markers{0}, invalid_markers{0};
std::atomic<uint64_t> dropped_rows{0}, snapshot_failures{0};
thread_local const char* names[128]{};
thread_local size_t depth{};
std::string output;
std::array<char, 512> failure{};
std::atomic_flag failure_claimed = ATOMIC_FLAG_INIT;
std::atomic<bool> failure_ready{false};
void record_failure(const char* message) noexcept {
    lost.store(true);
    if (!failure_claimed.test_and_set(std::memory_order_acq_rel)) {
        std::snprintf(failure.data(), failure.size(), "%s", message);
        failure_ready.store(true, std::memory_order_release);
    }
}
at::CallbackHandle callback{};
struct Checkpoint {
    int marker{}, stage{}, device{};
    size_t recorded_rows_before_snapshot{}, request_row{}, exception_row{};
    uint64_t a{}, b{}, requested_bytes{}, stream{}, thread{};
    ca::SnapshotInfo snapshot;
};
std::array<Checkpoint, 24> checkpoints;
size_t checkpoint_count{};
std::mutex checkpoint_mutex;
thread_local c10::MempoolId_t routing_pool{};

int checkpoint(const std::shared_ptr<ca::CUDAAllocator>& allocator, int marker,
               int device, uint64_t a, uint64_t b, size_t request_row = 0,
               size_t exception_row = 0, uint64_t requested_bytes = 0,
               uint64_t stream = 0) noexcept {
    try {
        std::lock_guard<std::mutex> checkpoint_lock(checkpoint_mutex);
        if (checkpoint_count == checkpoints.size()) { lost.store(true); return -1; }
        Checkpoint point{};
        point.marker = marker; point.stage = stage.load(); point.device = device;
        point.a = a; point.b = b;
        point.request_row = request_row; point.exception_row = exception_row;
        point.requested_bytes = requested_bytes; point.stream = stream;
        point.thread = std::hash<std::thread::id>{}(std::this_thread::get_id());
        {
            std::lock_guard<std::mutex> lock(mutex);
            point.recorded_rows_before_snapshot = used;
            if (exception_row) point.stage = rows.at(exception_row - 1).stage;
        }
        // Other allocations may progress before this later snapshot acquires native state.
        point.snapshot = allocator->snapshot({0, 0}, false);
        checkpoints[checkpoint_count++] = std::move(point);
        return 0;
    } catch (const std::exception& error) {
        snapshot_failures.fetch_add(1); record_failure(error.what()); return -1;
    } catch (...) { snapshot_failures.fetch_add(1); lost.store(true); return -1; }
}

size_t record(Row row) noexcept {
    if (!native_window.load(std::memory_order_acquire)) return 0;
    try {
        row.nesting = static_cast<int>(depth);
        row.thread = std::hash<std::thread::id>{}(std::this_thread::get_id());
        if (depth && depth <= 128) std::snprintf(row.op, sizeof(row.op), "%s", names[depth - 1]);
        std::lock_guard<std::mutex> lock(mutex);
        if (!native_window.load(std::memory_order_relaxed)) return 0;
        if (used < rows.size()) {
            row.stage = stage.load(std::memory_order_relaxed);
            row.monotonic_ns = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(
                std::chrono::steady_clock::now().time_since_epoch()).count());
            rows[used] = row;
            return ++used; // One-based ordinal committed under the row mutex.
        }
        dropped_rows.fetch_add(1);
        lost = true;
    } catch (...) { lost = true; }
    return 0;
}

// Existing Varena Working observer. No new allocator/owner, CUDA call, GIL or
// serialization here. SEGMENT_FREE is its PRE-unmap retirement notification.
void observe_segment(const ca::TraceEntry& event) noexcept {
    Row row{};
    if (event.action_ == ca::TraceEntry::SEGMENT_ALLOC) row.kind = 15;
    else if (event.action_ == ca::TraceEntry::SEGMENT_FREE) row.kind = 16;
    else {
        if (native_window.load()) {
            unsupported_observer_events.fetch_add(1);
            lost.store(true);
        }
        return;
    }
    row.device = event.device_;
    row.pointer = event.addr_;
    row.bytes = event.size_;
    row.stream = reinterpret_cast<uintptr_t>(event.stream_);
    row.a = event.mempool_.first;
    row.b = event.mempool_.second;
    record(row);
}

const char* marker_name(uint32_t marker) noexcept {
#define MARKER(name) case XS138_##name: return #name
    switch (marker) {
        MARKER(DISPATCH_ENTER); MARKER(DISPATCH_RETURN); MARKER(DISPATCH_ERROR);
        MARKER(USE_PREPARE_ENTER); MARKER(USE_PREPARE_RETURN); MARKER(USE_PREPARE_ERROR);
        MARKER(WAIT_READY_ENTER); MARKER(WAIT_READY_RETURN); MARKER(WAIT_READY_ERROR);
        MARKER(INPUT_COPY_ENTER); MARKER(INPUT_COPY_RETURN); MARKER(INPUT_COPY_ERROR);
        MARKER(REPLAY_ENTER); MARKER(REPLAY_RETURN); MARKER(REPLAY_ERROR);
        MARKER(OUTPUT_CLONE_ENTER); MARKER(OUTPUT_CLONE_RETURN); MARKER(OUTPUT_CLONE_ERROR);
        MARKER(EPOCH_RELEASE_ENTER); MARKER(EPOCH_RELEASE_RETURN); MARKER(EPOCH_RELEASE_ERROR);
        MARKER(FOLLOWING_BOUNDARY_ENTER); MARKER(FOLLOWING_BOUNDARY_RETURN); MARKER(FOLLOWING_BOUNDARY_ERROR);
        MARKER(SELECT_INTENT_ENTER); MARKER(SELECT_INTENT_RETURN); MARKER(SELECT_INTENT_ERROR);
        MARKER(RESTORE_INTENT_ENTER); MARKER(RESTORE_INTENT_RETURN); MARKER(RESTORE_INTENT_ERROR);
        MARKER(CACHE_RETIRE_ENTER); MARKER(CACHE_RETIRE_RETURN); MARKER(CACHE_RETIRE_ERROR);
        MARKER(CACHE_RETIRE_DECLINED);
        default: return nullptr;
    }
#undef MARKER
}

void tensor_metadata(const c10::IValue& value, int kind, int argument) {
    if (value.isTensor()) {
        const auto& tensor = value.toTensor();
        if (!tensor.defined() || !tensor.is_cuda() || !tensor.has_storage()) return;
        Row row{}; row.kind = kind; row.device = tensor.get_device();
        row.pointer = reinterpret_cast<uintptr_t>(tensor.const_data_ptr());
        row.bytes = tensor.nbytes(); row.argument = argument;
        row.offset = tensor.storage_offset(); row.dtype = static_cast<int>(tensor.scalar_type());
        row.dims = static_cast<int>(tensor.dim());
        if (row.dims > 8) lost.store(true);
        for (int i = 0; i < std::min(row.dims, 8); ++i) {
            row.shape[i] = tensor.size(i); row.stride[i] = tensor.stride(i);
        }
        record(row);
    } else if (value.isTensorList()) {
        for (const auto& tensor : value.toTensorVector()) tensor_metadata(tensor, kind, argument);
    } else if (value.isTuple()) {
        for (const auto& child : value.toTupleRef().elements()) tensor_metadata(child, kind, argument);
    } else if (value.isList()) {
        for (const auto& child : value.toListRef()) tensor_metadata(child, kind, argument);
    }
}

std::unique_ptr<at::ObserverContext> begin_op(const at::RecordFunction& fn) {
    if (depth < 128) names[depth] = fn.name();
    else lost.store(true);
    ++depth;
    if (!stage.load()) return nullptr;
    Row started{}; started.kind = 9; started.a = depth; record(started);
    try {
        int argument = 0;
        for (const auto& value : fn.inputs()) tensor_metadata(value, 7, argument++);
    } catch (const std::exception& error) {
        record_failure(error.what());
        Row row{}; row.kind = 8; record(row); // Metadata unavailable, never a fake zero pointer.
    } catch (...) {
        lost.store(true); Row row{}; row.kind = 8; record(row);
    }
    return nullptr;
}
void end_op(const at::RecordFunction& fn, at::ObserverContext*) {
    if (stage.load()) {
        try {
            int argument = 0;
            for (const auto& value : fn.outputs()) tensor_metadata(value, 11, argument++);
        } catch (const std::exception& error) {
            record_failure(error.what()); Row row{}; row.kind = 8; record(row);
        } catch (...) {
            lost.store(true); Row row{}; row.kind = 8; record(row);
        }
        Row ended{}; ended.kind = 10; ended.a = depth; record(ended);
    }
    if (depth) --depth;
}

class Traced final : public plug::CUDAPluggableAllocator {
    std::shared_ptr<ca::CUDAAllocator> inner_;
public:
    explicit Traced(std::shared_ptr<ca::CUDAAllocator> inner)
        : CUDAPluggableAllocator(dynamic_cast<plug::CUDAPluggableAllocator&>(*inner)), inner_(std::move(inner)) {
        const auto alloc = alloc_fn_;
        const auto observed = inner_;
        alloc_fn_ = [alloc, observed](size_t bytes, int device, cudaStream_t stream) -> void* {
            Row row{}; row.kind = 1; row.bytes = bytes; row.device = device; row.stream = reinterpret_cast<uintptr_t>(stream);
            row.a = routing_pool.first; row.b = routing_pool.second;
            const auto request_row = record(row);
            try {
                auto ptr = alloc(bytes, device, stream);
                row.kind = 2; row.pointer = reinterpret_cast<uintptr_t>(ptr); record(row); return ptr;
            } catch (...) {
                row.kind = 3;
                const auto exception_row = record(row);
                if (exception_row) checkpoint(observed, 8, device, row.a, row.b,
                    request_row, exception_row, row.bytes, row.stream);
                throw;
            }
        };
        const auto free = free_fn_;
        free_fn_ = [free](void* ptr, size_t bytes, int device, cudaStream_t stream) {
            Row row{}; row.kind = 4; row.pointer = reinterpret_cast<uintptr_t>(ptr); row.bytes = bytes;
            row.device = device; row.stream = reinterpret_cast<uintptr_t>(stream); record(row);
            try { free(ptr, bytes, device, stream); row.kind = 12; record(row); }
            catch (...) { row.kind = 13; record(row); throw; }
        };
        const auto record_stream = record_stream_fn_;
        record_stream_fn_ = [record_stream](void* ptr, cudaStream_t stream) {
            Row row{}; row.kind = 5; row.pointer = reinterpret_cast<uintptr_t>(ptr);
            row.stream = reinterpret_cast<uintptr_t>(stream); record(row); record_stream(ptr, stream);
        };
        const auto begin = begin_allocate_to_pool_fn_;
        begin_allocate_to_pool_fn_ = [begin](int device, c10::MempoolId_t pool, std::function<bool(cudaStream_t)> filter) {
            Row row{}; row.kind = 6; row.device = device; row.a = pool.first; row.b = pool.second; record(row);
            begin(device, pool, std::move(filter));
            routing_pool = pool;
        };
        const auto end = end_allocate_to_pool_fn_;
        end_allocate_to_pool_fn_ = [end](int device, c10::MempoolId_t pool) {
            Row row{}; row.kind = 14; row.device = device; row.a = pool.first; row.b = pool.second; record(row);
            end(device, pool);
            routing_pool = {0, 0};
        };
    }
    ca::SnapshotInfo snapshot(c10::MempoolId_t pool, bool traces) override { return inner_->snapshot(pool, traces); }
    void attachAllocatorTraceTracker(ca::AllocatorTraceTracker tracker) override {
        inner_->attachAllocatorTraceTracker(std::move(tracker));
    }
};
auto& holder = *new std::shared_ptr<Traced>();
}

extern "C" int allocation_trace_install() noexcept {
    try {
        auto inner = plug::getCurrentAllocator();
        TORCH_CHECK(inner && !inner->initialized(), "trace installation must precede CUDA initialization");
        TORCH_CHECK(!holder, "trace allocator is already installed");
        holder = std::make_shared<Traced>(inner);
        inner->attachAllocatorTraceTracker(observe_segment);
        observer_attached.store(true, std::memory_order_release);
        plug::changeCurrentAllocator(holder);
        callback = at::addThreadLocalCallback(at::RecordFunctionCallback(begin_op, end_op).needsInputs(true).needsOutputs(true));
        at::disableCallback(callback);
        return 0;
    } catch (const std::exception& e) { record_failure(e.what()); return -1; }
}
extern "C" const char* allocation_trace_error() noexcept {
    return failure_ready.load(std::memory_order_acquire) ? failure.data() : "";
}
extern "C" int allocation_trace_workspace_cache_enabled() noexcept {
    try {
        return at::cuda::isCUDABlasWorkspaceCachingEnabled() ? 1 : 0;
    } catch (const std::exception& error) {
        record_failure(error.what());
    } catch (...) {
        record_failure("workspace-cache state query failed");
    }
    return -1;
}
extern "C" int allocation_trace_window(int enabled) noexcept {
    if (enabled != 0 && enabled != 1) { record_failure("invalid native trace window"); return -1; }
    try {
        // Close is linearized against committed rows. A callback already waiting
        // for this lock rechecks the window before committing anything.
        std::lock_guard<std::mutex> lock(mutex);
        native_window.store(enabled != 0, std::memory_order_release);
        return 0;
    } catch (...) { lost.store(true); return -1; }
}
extern "C" uint64_t allocation_trace_marker(uint32_t marker, uint32_t invocation,
        int32_t device, uint64_t stream, uint64_t graph_identity, uint64_t plan_handle,
        uint64_t pool_a, uint64_t pool_b) noexcept {
    if (!marker_name(marker)) {
        invalid_markers.fetch_add(1); record_failure("invalid host trace marker"); return 0;
    }
    if (!native_window.load(std::memory_order_acquire)) { inactive_markers.fetch_add(1); return 0; }
    Row row{}; row.kind = 17; row.marker = marker; row.invocation = invocation;
    row.device = device; row.stream = stream; row.graph_identity = graph_identity;
    row.plan_handle = plan_handle; row.a = pool_a; row.b = pool_b;
    return record(row);
}
extern "C" int allocation_trace_checkpoint(int marker, int device, uint64_t a, uint64_t b) noexcept {
    // Host-only native snapshot; no Python objects, CUDA calls, or serialization.
    if (!holder) { record_failure("snapshot before trace allocator installation"); return -1; }
    return checkpoint(holder, marker, device, a, b);
}
extern "C" void allocation_trace_stage(int value) noexcept {
    try {
        if (value < 0) { record_failure("negative ATen trace stage"); return; }
        if (value) at::reenableCallback(callback);
        else at::disableCallback(callback);
        stage.store(value);
    } catch (...) { lost.store(true); }
}
extern "C" const char* allocation_trace_dump() noexcept {
    try {
        std::vector<Row> saved_rows;
        std::vector<Checkpoint> saved_checkpoints;
        bool saved_window{};
        {
            std::lock_guard<std::mutex> checkpoint_lock(checkpoint_mutex);
            std::lock_guard<std::mutex> lock(mutex);
            saved_rows.assign(rows.begin(), rows.begin() + used);
            saved_checkpoints.assign(checkpoints.begin(), checkpoints.begin() + checkpoint_count);
            saved_window = native_window.load(std::memory_order_relaxed);
        }
        // Serialization never holds a mutex acquired by the allocator tracker.
        std::ostringstream out;
        out << "{\"format_version\":3,\"ordinal_base\":1,\"lost\":" << (lost.load() ? "true" : "false")
            << ",\"observer_attached\":" << (observer_attached.load() ? "true" : "false")
            << ",\"native_window_enabled\":" << (saved_window ? "true" : "false")
            << ",\"unsupported_observer_events\":" << unsupported_observer_events.load()
            << ",\"inactive_markers\":" << inactive_markers.load() << ",\"invalid_markers\":" << invalid_markers.load()
            << ",\"dropped_rows\":" << dropped_rows.load() << ",\"snapshot_failures\":" << snapshot_failures.load()
            << ",\"row_capacity\":" << rows.size()
            << ",\"snapshot_cut_ordinal\":" << saved_rows.size()
            << ",\"row_fields\":[\"kind\",\"stage\",\"device\",\"pointer\",\"bytes\",\"stream\",\"thread\",\"pool_a\",\"pool_b\",\"storage_offset\",\"dtype\",\"ndim\",\"op\",\"shape\",\"stride\",\"argument\",\"depth\",\"monotonic_ns\",\"marker\",\"invocation\",\"graph_identity\",\"plan_handle\"]"
            << ",\"kind\":{\"1\":\"allocation_request\",\"2\":\"allocation_return\",\"3\":\"allocation_exception\",\"4\":\"free_request\",\"5\":\"record_stream\",\"6\":\"pool_begin\",\"7\":\"operator_input\",\"8\":\"metadata_unavailable\",\"9\":\"operator_begin\",\"10\":\"operator_end\",\"11\":\"operator_output\",\"12\":\"free_return\",\"13\":\"free_exception\",\"14\":\"pool_end\",\"15\":\"working_segment_mapped\",\"16\":\"working_segment_retire_intent\",\"17\":\"host_stage_marker\"}"
            << ",\"segment_free_semantics\":\"pre_unmap_retirement_intent\""
            << ",\"segment_stream_semantics\":\"unavailable_observer_supplies_null\""
            << ",\"segment_zero_pool_semantics\":\"ordinary_eager_not_private_pool\""
            << ",\"graph_identity_semantics\":\"diagnostic_python_control_identity_not_cuda_graph_handle\""
            << ",\"plan_handle_semantics\":\"abi_handle_not_native_plan_id\""
            << ",\"marker_zero_identity_semantics\":\"unavailable_reason_supplied_by_frontend\""
            << ",\"marker_names\":{";
        bool first_marker = true;
        for (uint32_t marker = 100; marker <= 203; ++marker) {
            if (const char* name = marker_name(marker)) {
                if (!first_marker) out << ',';
                first_marker = false;
                out << '\"' << marker << "\":\"" << name << '\"';
            }
        }
        out << "},\"rows\":[";
        for (size_t i = 0; i < saved_rows.size(); ++i) {
            const auto& r = saved_rows[i]; if (i) out << ',';
            out << '[' << r.kind << ',' << r.stage << ',' << r.device << ',' << r.pointer << ',' << r.bytes << ',' << r.stream << ',' << r.thread << ',' << r.a << ',' << r.b << ',' << r.offset << ',' << r.dtype << ',' << r.dims << ",\"";
            for (const char* p = r.op; *p; ++p) { if (*p == '"' || *p == '\\') out << '\\'; if (static_cast<unsigned char>(*p) >= 32) out << *p; }
            out << "\",[";
            for (int d = 0; d < std::min(r.dims, 8); ++d) { if (d) out << ','; out << r.shape[d]; }
            out << "],[";
            for (int d = 0; d < std::min(r.dims, 8); ++d) { if (d) out << ','; out << r.stride[d]; }
            out << "]," << r.argument << ',' << r.nesting << ',' << r.monotonic_ns << ','
                << r.marker << ',' << r.invocation << ',' << r.graph_identity << ',' << r.plan_handle << ']';
        }
        out << "],\"checkpoints\":[";
        for (size_t i = 0; i < saved_checkpoints.size(); ++i) {
            if (i) out << ',';
            const auto& p = saved_checkpoints[i];
            out << "{\"marker\":" << p.marker << ",\"stage\":" << p.stage << ",\"device\":" << p.device
                << ",\"recorded_rows_before_snapshot\":" << p.recorded_rows_before_snapshot
                << ",\"request_row\":" << p.request_row << ",\"exception_row\":" << p.exception_row
                << ",\"requested_bytes\":" << p.requested_bytes << ",\"stream\":" << p.stream
                << ",\"thread\":" << p.thread
                << ",\"pool\":[" << p.a << ',' << p.b << "],\"segments\":[";
            bool first = true;
            for (const auto& s : p.snapshot.segments) {
                if (!first) out << ',';
                first = false;
                out << "{\"device\":" << static_cast<int>(s.device) << ",\"address\":" << s.address
                    << ",\"total_size\":" << s.total_size << ",\"allocated_size\":" << s.allocated_size
                    << ",\"active_size\":" << s.active_size << ",\"pool\":[" << s.owner_private_pool_id.first
                    << ',' << s.owner_private_pool_id.second << "],\"blocks\":[";
                bool first_block = true;
                for (const auto& b : s.blocks) {
                    if (!first_block) out << ',';
                    first_block = false;
                    out << '[' << b.size << ',' << b.requested_size << ',' << b.allocated << ',' << b.active << ']';
                }
                out << "]}";
            }
            out << "]}";
        }
        out << "]}"; output = out.str(); return output.c_str();
    } catch (...) { return nullptr; }
}
