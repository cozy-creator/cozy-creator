// Fixture-owned real Torch trace listener. Never invokes a callback synthetically.
#include <c10/cuda/CUDACachingAllocator.h>
#include <c10/util/Exception.h>
#include <cuda_runtime_api.h>
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <vector>

namespace {
namespace caching = c10::cuda::CUDACachingAllocator;
std::atomic<uint64_t> maps{0}, retiring{0}, rejected{0}, reject_address{0};
}

extern "C" int observer_probe_attach() noexcept {
    try {
        caching::attachAllocatorTraceTracker([](const caching::TraceEntry& event) {
            if (event.action_ == caching::TraceEntry::SEGMENT_ALLOC) ++maps;
            if (event.action_ == caching::TraceEntry::SEGMENT_FREE) {
                ++retiring;
                if (event.addr_ == reject_address.load() && event.addr_ != 0) {
                    ++rejected;
                    TORCH_CHECK(false, "owned Working observer refusal");
                }
            }
        });
        return 0;
    } catch (...) {
        return -1;
    }
}

extern "C" void observer_probe_reject(uint64_t address) noexcept {
    reject_address.store(address);
}

extern "C" uint64_t observer_probe_count(int kind) noexcept {
    return kind == 0 ? maps.load() : kind == 1 ? retiring.load() : rejected.load();
}

extern "C" int observer_probe_read(uint64_t address, size_t bytes, uint8_t expected) noexcept {
    try {
        std::vector<uint8_t> host(bytes);
        const auto status = cudaMemcpy(host.data(), reinterpret_cast<void*>(address), bytes, cudaMemcpyDeviceToHost);
        if (status != cudaSuccess) return -static_cast<int>(status);
        return std::all_of(host.begin(), host.end(), [=](uint8_t value) { return value == expected; }) ? 0 : 1;
    } catch (...) {
        return 2;
    }
}
