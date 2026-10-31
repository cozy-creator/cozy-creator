// Torch's pool filter and exception ABI stay here; allocation policy stays in Rust.
#include <c10/util/Exception.h>
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <functional>
#include <mutex>
#include <thread>
#include <utility>
#include <vector>

#if defined(_WIN32)
#define VR_EXPORT __declspec(dllexport)
#else
#define VR_EXPORT __attribute__((visibility("default")))
#endif

using Stream = struct CUstream_st*;
using Pool = std::pair<unsigned long long, unsigned long long>;
using Filter = std::function<bool(Stream)>;

namespace {
struct Failure {
    int32_t kind;
    int32_t cuda;
    char message[512];
};
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
};
Bindings core{};
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

uint64_t stream_id(Stream stream) {
    TORCH_CHECK(reinterpret_cast<uintptr_t>(stream) != 2,
                "varena: cudaStreamPerThread is unsupported; use an explicit CUDA stream");
    return reinterpret_cast<uintptr_t>(stream);
}
} // namespace

extern "C" {
VR_EXPORT int varena_torch_bind(const Bindings* bindings) noexcept {
    if (!bindings || bindings->abi != 1 || bindings->size != sizeof(Bindings) || core.abi != 0) {
        return -1;
    }
    core = *bindings;
    return 0;
}

VR_EXPORT void* varena_torch_malloc(size_t bytes, int device, Stream stream) {
    const uint64_t id = stream_id(stream);
    Pool pool{0, 0};
    bool routed = false;
    if (routing.load(std::memory_order_acquire)) {
        std::lock_guard<std::mutex> lock(mutex);
        for (auto& route : routes) {
            if (route.device != device || !route.filter(stream)) {
                continue;
            }
            pool = route.pool;
            routed = true;
            if (std::find(route.streams.begin(), route.streams.end(), stream) == route.streams.end()) {
                Failure error{};
                core.pool_begin(device, pool.first, pool.second, id, &error);
                check(error);
                route.streams.push_back(stream);
            }
            break;
        }
    }
    Failure error{};
    const uint64_t ptr = core.alloc(bytes, device, id, routed, pool.first, pool.second, &error);
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
    std::lock_guard<std::mutex> lock(mutex);
    for (const auto& route : routes) {
        TORCH_CHECK(route.device != device || route.pool != pool,
                    "varena: overlapping captures cannot share one pool");
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

// Owner calls only after confirming CUDA stream capture ended. Graph reset owns reference release.
VR_EXPORT int varena_torch_abort_routing(int device, uint64_t a, uint64_t b) noexcept {
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
