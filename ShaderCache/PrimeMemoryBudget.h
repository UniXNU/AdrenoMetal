// SPDX-License-Identifier: MIT
#pragma once

#include <cstdint>

namespace prime {
struct MemorySample {
    uint64_t footprint, resident, headroom;
    int regions;
};

inline constexpr unsigned kBatchAttempts = 96;

inline const char *rotationReason(unsigned attempts, const MemorySample *current, const MemorySample *baseline) {
    constexpr uint64_t MiB = 1ull << 20;
    if (current) {
        // The worker has no headroom report when XNU classifies it as a daemon.
        if (current->headroom && current->headroom < 160 * MiB) return "headroom";
        if (current->footprint >= 1536 * MiB) return "footprint-ceiling";
        // Cache seeding and library parsing can cross the growth budget on every
        // restart. A worker gets one completed attempt below the hard limits.
        if (attempts && baseline) {
            if (current->resident > baseline->resident + 96 * MiB) return "resident";
            if (current->footprint > baseline->footprint + 128 * MiB) return "footprint";
            if (current->regions > baseline->regions + 512) return "regions";
        }
    }
    return attempts >= kBatchAttempts ? "batch" : nullptr;
}
}  // namespace prime
