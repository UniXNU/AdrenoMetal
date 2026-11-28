// SPDX-License-Identifier: MIT
#pragma once

#import <Metal/Metal.h>

static constexpr char NBShaderCacheService[] = "org.unixnu.shader-cache.worker";
static constexpr char NBShaderCacheWorker[] = "NativeAdrenoShaderCacheWorker";

#ifdef NB_SHADER_CACHE_XPC
#include <dlfcn.h>
#include <xpc/xpc.h>
static inline xpc_connection_t NBShaderCacheConnection(uint64_t flags) {
    using Create = xpc_connection_t (*)(const char *, dispatch_queue_t, uint64_t);
    auto create = reinterpret_cast<Create>(dlsym(RTLD_DEFAULT, "xpc_connection_create_mach_service"));
    return create ? create(NBShaderCacheService, dispatch_get_main_queue(), flags) : nullptr;
}
#endif

enum class NBPrimeResult : int {
    Complete,
    Incomplete,
    Stopped,
    Unavailable,
    CacheUnavailable,
    Busy,
    MemoryLimit,
    PublicationFailed,
    MemoryLimitNoProgress,
    NextBatch,
    WorkerInterrupted,
};

struct NBPrimeCounts {
    unsigned completed, total, functions, refused, skipped, unopened, missing, deferred;
};
using NBPrimeProgress = void (*)(void *, const NBPrimeCounts &, const char *, const char *);
using NBPrimeCancelled = bool (*)(void *);
using NBPrimeRun = NBPrimeResult (*)(id<MTLDevice>, NBPrimeProgress, NBPrimeCancelled, void *);

extern "C" NBPrimeResult native_adreno_run_shader_cache(
    id<MTLDevice>, NBPrimeProgress, NBPrimeCancelled, void *);
