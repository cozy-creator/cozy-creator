// Torch's pool filter and exception ABI stay here; allocation policy stays in Rust.
#include <ATen/cuda/CUDAContextLight.h>
#include <c10/core/Allocator.h>
#include <c10/core/StorageImpl.h>
#include <c10/util/Exception.h>
#include <c10/util/ApproximateClock.h>
#include <torch/csrc/cuda/CUDAPluggableAllocator.h>
#include <torch/headeronly/version.h>
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <functional>
#include <cstdio>
#include <map>
#include <memory>
#include <mutex>
#include <thread>
#include <utility>
#include <vector>

#if defined(_WIN32)
#define VR_EXPORT __declspec(dllexport)
#else
#include <unistd.h>
#define VR_EXPORT __attribute__((visibility("default")))
#endif

using Stream = struct CUstream_st*;
using Pool = c10::MempoolId_t;
using Filter = std::function<bool(Stream)>;

namespace {
struct Failure {
    int32_t kind;
    int32_t cuda;
    char message[512];
};
struct Segment {
    int32_t device;
    uint64_t address;
    size_t bytes;
    size_t size_class;
    uint64_t pool_a;
    uint64_t pool_b;
};
struct SnapshotBlock {
    size_t bytes;
    size_t requested;
    int32_t allocated;
    int32_t active;
};
using Observe = int32_t (*)(int32_t, const Segment*, size_t, Failure*);
using Visit = int32_t (*)(void*, const Segment*, const SnapshotBlock*, size_t, Failure*);
struct Bindings {
    uint32_t abi;
    uint32_t size;
    uint64_t (*alloc)(size_t, int32_t, uint64_t, int32_t, uint64_t, uint64_t, Failure*);
    int32_t (*free)(int32_t, uint64_t, uint64_t, Failure*);
    int32_t (*record_stream)(uint64_t, uint64_t, Failure*);
    int32_t (*trim)(Failure*);
    int32_t (*pool_begin)(int32_t, uint64_t, uint64_t, uint64_t, Failure*);
    int32_t (*pool_end)(int32_t, uint64_t, uint64_t, Failure*);
    int32_t (*pool_release)(int32_t, uint64_t, uint64_t, Failure*);
    int32_t (*observe)(Observe, Failure*);
    int32_t (*snapshot)(void*, Visit, Failure*);
};
Bindings core{};
#if defined(_WIN32)
int process_id() { return 0; }
#else
int process_id() { return getpid(); }
#endif
int bound_process = 0;
struct Route {
    int device;
    Pool pool;
    Filter filter;
    std::vector<Stream> streams;
    std::thread::id thread;
};
std::mutex mutex;
std::vector<Route> routes;
std::atomic<size_t> routing{0};

void check(const Failure& error) {
    if (error.kind == 1) {
        TORCH_CHECK_WITH(OutOfMemoryError, false, error.message);
    }
    if (error.kind == 2) {
        throw c10::AcceleratorError({__func__, __FILE__, __LINE__}, error.cuda, error.message);
    }
    TORCH_CHECK(error.kind == 0, error.message);
}

// Before `mutex`, which a forked child may inherit held by a thread it doesn't have.
void check_process() {
    TORCH_CHECK(process_id() == bound_process, "varena: the Torch allocator cannot be used after fork");
}

uint64_t stream_id(Stream stream) {
    TORCH_CHECK(reinterpret_cast<uintptr_t>(stream) != 2,
                "varena: cudaStreamPerThread is unsupported; use an explicit CUDA stream");
    return reinterpret_cast<uintptr_t>(stream);
}
} // namespace

extern "C" {
VR_EXPORT int varena_torch_bind(const Bindings* bindings) noexcept {
    if (!bindings || bindings->abi != 2 || bindings->size != sizeof(Bindings) || core.abi != 0) {
        return -1;
    }
    core = *bindings;
    bound_process = process_id();
    return 0;
}

VR_EXPORT void* varena_torch_malloc(size_t bytes, int device, Stream stream) {
    const uint64_t id = stream_id(stream);
    Pool pool{0, 0};
    bool routed = false;
    if (routing.load(std::memory_order_acquire)) {
        check_process();
        std::lock_guard<std::mutex> lock(mutex);
        Route* selected = nullptr;
        for (auto& route : routes) {
            if (route.device == device && route.filter(stream)) {
                selected = &route;
                break;
            }
        }
        // Another stream's allocation during capture runs eagerly: what it writes would not
        // be replayed, so it never lands in a pool. It takes a free eager block or fails capture.
        if (selected) {
            pool = selected->pool;
            routed = true;
            if (std::find(selected->streams.begin(), selected->streams.end(), stream) == selected->streams.end()) {
                Failure error{};
                core.pool_begin(device, pool.first, pool.second, id, &error);
                check(error);
                selected->streams.push_back(stream);
            }
        }
    }
    Failure error{};
    const uint64_t ptr = core.alloc(bytes, device, id, routed, pool.first, pool.second, &error);
    // cuDNN catches OutOfMemoryError and can switch and cache engines. A short prepared
    // pool must abort capture instead; the owning Python scope translates only after cleanup.
    if (routed && error.kind == 1) {
        TORCH_CHECK(false, "[tensorfs.varena.capture_capacity.v1] ", error.message);
    }
    check(error);
    TORCH_CHECK(ptr != 0 || bytes == 0, "varena: allocator returned a null pointer without an error");
    return reinterpret_cast<void*>(ptr);
}

VR_EXPORT void varena_torch_free(void* ptr, size_t, int device, Stream stream) noexcept {
    Failure error{};
    // Rust retains a destructor failure and raises it at the next allocator/Python boundary.
    core.free(device, reinterpret_cast<uintptr_t>(ptr), reinterpret_cast<uintptr_t>(stream), &error);
}

VR_EXPORT void varena_torch_record_stream(void* ptr, Stream stream) {
    Failure error{};
    core.record_stream(reinterpret_cast<uintptr_t>(ptr), stream_id(stream), &error);
    check(error);
}

VR_EXPORT void varena_torch_reset() {
    Failure error{};
    core.trim(&error);
    check(error);
}

VR_EXPORT void varena_torch_begin_pool(int device, Pool pool, Filter filter) {
    check_process();
    std::lock_guard<std::mutex> lock(mutex);
    for (const auto& route : routes) {
        TORCH_CHECK(route.device != device ||
                    (route.pool != pool && route.thread != std::this_thread::get_id()),
                    "varena: captures cannot overlap on one pool or one device/thread");
    }
    // Reserve routing storage before the native transition so bad_alloc cannot strand a capture.
    routes.reserve(routes.size() + 1);
    Failure error{};
    core.pool_begin(device, pool.first, pool.second, 0, &error);
    check(error);
    routes.push_back({device, pool, std::move(filter), {}, std::this_thread::get_id()});
    routing.store(routes.size(), std::memory_order_release);
}

VR_EXPORT void varena_torch_end_pool(int device, Pool pool) {
    check_process();
    std::lock_guard<std::mutex> lock(mutex);
    bool found = false;
    for (auto route = routes.begin(); route != routes.end(); ++route) {
        if (route->device == device && route->pool == pool) {
            routes.erase(route);
            found = true;
            break;
        }
    }
    routing.store(routes.size(), std::memory_order_release);
    if (!found) return; // An explicitly ended failed capture may still receive Torch's reset hook.
    Failure error{};
    core.pool_end(device, pool.first, pool.second, &error);
    check(error);
}

// A storage's owners, for a graph owner to prove none escaped before it declares a pool idle.
VR_EXPORT uint64_t varena_torch_storage_owners(void* storage) noexcept {
    return c10::raw::intrusive_ptr::use_count(static_cast<c10::StorageImpl*>(storage));
}

// Owner calls only after confirming CUDA stream capture ended. Graph reset owns reference release.
VR_EXPORT int varena_torch_abort_routing(int device, uint64_t a, uint64_t b) noexcept {
    if (process_id() != bound_process) return -1;
    std::lock_guard<std::mutex> lock(mutex);
    for (auto route = routes.begin(); route != routes.end(); ++route) {
        if (route->device == device && route->pool == Pool{a, b}
            && route->thread == std::this_thread::get_id()) {
            routes.erase(route);
            routing.store(routes.size(), std::memory_order_release);
            Failure error{};
            core.pool_end(device, a, b, &error);
            return error.kind == 0 ? 1 : -1;
        }
    }
    return 0;
}

VR_EXPORT void varena_torch_release_pool(int device, Pool pool) noexcept {
    Failure error{};
    core.pool_release(device, pool.first, pool.second, &error);
}
} // extern "C"

namespace {
namespace torch_allocator = torch::cuda::CUDAPluggableAllocator;
namespace caching = c10::cuda::CUDACachingAllocator;

template <class F>
int32_t contain(Failure* failure, F&& operation) noexcept {
    try {
        operation();
        return 1;
    } catch (const std::exception& error) {
        failure->kind = 3;
        std::snprintf(failure->message, sizeof(failure->message), "%s", error.what());
    } catch (...) {
        failure->kind = 3;
        std::snprintf(failure->message, sizeof(failure->message), "unknown Working observer exception");
    }
    return 0;
}

class VarenaAllocator final : public torch_allocator::CUDAPluggableAllocator {
    std::mutex observer_mutex_;
    std::map<std::pair<int32_t, uint64_t>, Segment> segments_;
    std::vector<caching::AllocatorTraceTracker> trackers_;
public:
    VarenaAllocator() : CUDAPluggableAllocator(varena_torch_malloc, varena_torch_free) {
        set_reset_fn(varena_torch_reset);
        set_record_stream_fn(varena_torch_record_stream);
        set_begin_allocate_to_pool(varena_torch_begin_pool);
        set_end_allocate_to_pool_fn(varena_torch_end_pool);
        set_release_pool(varena_torch_release_pool);
    }

    void observe(int32_t action, const Segment* segments, size_t count) {
        // Native Working state is already held. No callback may acquire the GIL or
        // re-enter Varena. NCCL's callback takes only hook/communicator locks and
        // calls NCCL registration, never this allocator's allocation entrypoints.
        std::lock_guard<std::mutex> lock(observer_mutex_);
        if (action == 0) {
            std::map<std::pair<int32_t, uint64_t>, Segment> initial;
            for (size_t i = 0; i < count; ++i) {
                const auto& segment = segments[i];
                TORCH_CHECK(initial.emplace(std::make_pair(segment.device, segment.address), segment).second,
                            "Varena duplicate initial Working segment");
            }
            TORCH_CHECK(segments_.empty() && trackers_.empty(), "Varena observer already initialized");
            segments_.swap(initial);
            return;
        }
        TORCH_CHECK(count == 1 && (action == 1 || action == 2), "invalid Varena Working observation");
        const auto& segment = segments[0];
        const auto key = std::make_pair(segment.device, segment.address);
        if (action == 1) {
            TORCH_CHECK(segments_.emplace(key, segment).second, "Varena duplicate Working map");
        } else {
            const auto found = segments_.find(key);
            TORCH_CHECK(found != segments_.end() && found->second.bytes == segment.bytes &&
                        found->second.pool_a == segment.pool_a && found->second.pool_b == segment.pool_b,
                        "Varena Working retirement identity mismatch");
        }
        caching::TraceEntry event(action == 1 ? caching::TraceEntry::SEGMENT_ALLOC : caching::TraceEntry::SEGMENT_FREE,
            static_cast<c10::DeviceIndex>(segment.device), segment.address, segment.bytes, nullptr,
            {segment.pool_a, segment.pool_b}, c10::getApproximateTime());
        for (const auto& tracker : trackers_) tracker(event);
        // A throwing tracker leaves the range retained. Rust poisons its owner
        // before returning; it must neither unmap nor reuse that physical backing.
        if (action == 2) segments_.erase(key);
    }

    caching::SnapshotInfo snapshot(Pool pool, bool /*include_traces*/) override {
        struct VisitContext { caching::SnapshotInfo snapshot; Pool pool; } context{{}, pool};
        // Do not hold observer_mutex_ across native state traversal: mutations
        // acquire those locks in the opposite direction before notifying us.
        const auto visit = +[](void* raw, const Segment* segment, const SnapshotBlock* blocks,
                               size_t count, Failure* failure) -> int32_t {
            return contain(failure, [&] {
                auto& context = *static_cast<VisitContext*>(raw);
                const Pool identity{segment->pool_a, segment->pool_b};
                if (context.pool != Pool{0, 0} && context.pool != identity) return;
                caching::SegmentInfo value{};
                value.device = static_cast<c10::DeviceIndex>(segment->device);
                value.address = segment->address;
                value.total_size = segment->bytes;
                value.is_large = segment->size_class > (1 << 20);
                value.owner_private_pool_id = identity;
                for (size_t i = 0; i < count; ++i) {
                    caching::BlockInfo block{};
                    block.size = blocks[i].bytes;
                    block.requested_size = blocks[i].requested;
                    block.allocated = blocks[i].allocated != 0;
                    block.active = blocks[i].active != 0;
                    if (block.allocated) {
                        value.allocated_size += block.size;
                    }
                    if (block.active) {
                        value.active_size += block.size;
                        value.requested_size += block.requested_size;
                    }
                    value.blocks.push_back(std::move(block));
                }
                context.snapshot.segments.push_back(std::move(value));
            });
        };
        Failure failure{};
        core.snapshot(&context, visit, &failure);
        check(failure);
        return std::move(context.snapshot);
    }

    void attachAllocatorTraceTracker(caching::AllocatorTraceTracker tracker) override {
        std::lock_guard<std::mutex> lock(observer_mutex_);
        trackers_.push_back(std::move(tracker));
    }
};

// Process-lifetime ownership: native callbacks remain valid after Python drops
// its installation handles and throughout Torch/global destructor ordering.
auto& installed_allocator = *new std::shared_ptr<VarenaAllocator>();

int32_t observe_working(int32_t action, const Segment* segments, size_t count, Failure* failure) noexcept {
    return contain(failure, [&] { installed_allocator->observe(action, segments, count); });
}
} // namespace

extern "C" VR_EXPORT int32_t varena_torch_install(Failure* failure) noexcept {
    return contain(failure, [&] {
        TORCH_CHECK(core.abi == 2 && !installed_allocator, "Varena allocator installation identity mismatch");
        installed_allocator = std::make_shared<VarenaAllocator>();
        Failure observed{};
        core.observe(observe_working, &observed);
        check(observed);
        torch_allocator::changeCurrentAllocator(installed_allocator);
    });
}

extern "C" VR_EXPORT int32_t varena_torch_workspace_cache_enabled(
        int32_t* enabled, Failure* failure) noexcept {
    return contain(failure, [&] {
        TORCH_CHECK(enabled, "Varena workspace-cache query requires an output");
        // This host-only accessor returns Torch's first-call-latched state. It
        // neither initializes CUDA nor creates a library handle or workspace.
#if TORCH_VERSION_MAJOR == 2 && TORCH_VERSION_MINOR < 15
        *enabled = 1; // Before 2.15 Torch has no switch: it always caches workspaces per stream.
#else
        *enabled = at::cuda::isCUDABlasWorkspaceCachingEnabled() ? 1 : 0;
#endif
    });
}
