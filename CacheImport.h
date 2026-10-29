// SPDX-License-Identifier: MIT
// The pipeline caches compiled ahead on the host (UniXNU scripts/make-pipeline-cache.py), read on demand from the userdata import region
// (UniXNU-sheng scripts/make-pipeline-import.py) through the GPU driver's method 0x7101, which hands out 4096-byte blocks. The runtime asks
// for an entry only when its cache directory lacks it (nmb_set_shader_cache_fallback); nothing is copied into the directory, which lives on
// the RAM Data disk (int 590aaee2: unpacking the whole set there took 850 MB and starved the GPU of memory). What the render server
// compiles itself still goes to the directory. prime-done is read the same way, so priming skips every library the region covers.
// Layout: block 0 "SHNGIMP2", u32 version 2, u32 entry count, u64 index bytes, u64 data bytes, the SHA-256 of the index; the index
// from block 1 (per entry u64 data offset, u32 stored bytes, u32 bytes, u16 path bytes, the path; sorted by path); the data from the block
// after it, each entry zlib-compressed on its own. The index is checked against its digest once and kept in memory (a few MB); each entry's zlib stream checks its own bytes.
#pragma once
#include <dlfcn.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <zlib.h>
#include <CommonCrypto/CommonDigest.h>
#include <algorithm>
#include <string>
#include <vector>
#include "NativeAdrenoUserTest.h"

namespace cacheimport {
constexpr uint32_t kMethod = 0x7101, kBlock = 4096;
// Read counters for the telemetry: entries served, bytes served (stored), misses, failed reads; and the first path the region lacked.
extern "C" {volatile uint64_t native_cache_import_stats[4]; char native_cache_import_first_miss[160];}

struct Entry {uint64_t offset; uint32_t stored, bytes; std::string path;};
static Gpu gLink;
static std::vector<Entry> gEntries;
static uint64_t gDataFirst;   // byte offset of the data in the region
static pthread_mutex_t gLock = PTHREAD_MUTEX_INITIALIZER;
// The last data block read: entries lie next to each other in path order, which is often the order they are asked in.
static uint64_t gCachedBlock = UINT64_MAX;
static uint8_t gCached[kBlock];
// The region's size and its 4096-byte blocks: through the GPU driver on the tablet; a host check reads an image file (openFile).
static bool (*gRegionBlock)(uint64_t offset, uint8_t *out) = [](uint64_t offset, uint8_t *out) {
    return gLink.method(kMethod, &offset, sizeof offset, out, kBlock) == 0;
};
static FILE *gFile;

static bool readBlock(uint64_t offset, uint8_t *out) {
    if (offset == gCachedBlock) {memcpy(out, gCached, kBlock); return true;}
    if (!gRegionBlock(offset, out)) return false;
    gCachedBlock = offset;
    memcpy(gCached, out, kBlock);
    return true;
}
// `bytes` bytes of the region from `at` (any alignment) into out.
static bool readBytes(uint64_t at, size_t bytes, uint8_t *out) {
    uint8_t block[kBlock];
    while (bytes) {
        const uint64_t base = at & ~uint64_t(kBlock - 1);
        const size_t skip = (size_t)(at - base), take = std::min<size_t>(bytes, kBlock - skip);
        if (!readBlock(base, block)) return false;
        memcpy(out, block + skip, take);
        out += take; at += take; bytes -= take;
    }
    return true;
}

// The entry at `path` (relative to the cache directory), inflated into a malloc'd buffer the caller frees.
static bool read(const char *path, uint8_t **out, size_t *length) {
    if (!path || !out || !length) return false;
    const auto found = std::lower_bound(gEntries.begin(), gEntries.end(), path, [](const Entry &e, const char *p) {return e.path < p;});
    if (found == gEntries.end() || found->path != path) {
        if (!native_cache_import_stats[2]++) strlcpy(native_cache_import_first_miss, path, sizeof native_cache_import_first_miss);
        return false;
    }
    std::vector<uint8_t> stored(found->stored);
    pthread_mutex_lock(&gLock);
    const bool got = readBytes(gDataFirst + found->offset, found->stored, stored.data());
    pthread_mutex_unlock(&gLock);
    // zlib through dlopen, as CacheExport.h takes it: the plugin links no libz.
    using UncompressFn = int (*)(Bytef *, uLongf *, const Bytef *, uLong);
    static const UncompressFn inflateAll = [] {
        void *z = dlopen("/usr/lib/libz.1.dylib", RTLD_NOW | RTLD_LOCAL);
        return z ? (UncompressFn)dlsym(z, "uncompress") : nullptr;
    }();
    uint8_t *data = static_cast<uint8_t *>(malloc(found->bytes ? found->bytes : 1));
    uLongf made = found->bytes;
    if (!got || !data || !inflateAll || inflateAll(data, &made, stored.data(), found->stored) != Z_OK || made != found->bytes) {
        free(data);
        native_cache_import_stats[3] = native_cache_import_stats[3] + 1;
        return false;
    }
    *out = data;
    *length = made;
    native_cache_import_stats[0] = native_cache_import_stats[0] + 1;
    native_cache_import_stats[1] = native_cache_import_stats[1] + found->stored;
    return true;
}

static bool readIndex(uint64_t region, Gpu *marker);
// The host check's region: the payload of an import image (make-pipeline-import.py) written raw at `path`.
[[maybe_unused]] static bool openFile(const char *path) {
    if (!(gFile = fopen(path, "rb"))) return false;
    gRegionBlock = [](uint64_t offset, uint8_t *out) {return fseeko(gFile, (off_t)offset, SEEK_SET) == 0 && fread(out, 1, kBlock, gFile) == kBlock;};
    fseeko(gFile, 0, SEEK_END);
    return readIndex((uint64_t)ftello(gFile), nullptr);
}

// Opens the region and reads its index; false (nothing installed) without a region or with one that does not check.
// Markers 0x63nn: 00 started, 01 index ready (then 0x64nn: entries in thousands, saturated), f0 no connection, f1 no selector,
// f2 no region (or not this layout), f3 the index does not match its digest.
static bool open(Gpu &marker) {
    marker.mark(0x6300);
    gLink.call = marker.call; gLink.openFn = marker.openFn; gLink.closeFn = marker.closeFn; gLink.port = marker.port;
    if (!gLink.openFn || gLink.openFn(gLink.port, mach_task_self(), 0x4148, &gLink.conn) != 0 || !gLink.conn) {marker.mark(0x63f0); return false;}
    if (!gLink.probe()) {marker.mark(0x63f1); return false;}
    uint64_t region = 0;
    if (gLink.method(kMethod, &region, sizeof region, &region, sizeof region) != 0) {marker.mark(0x63f2); return false;}
    return readIndex(region, &marker);
}
static bool readIndex(uint64_t region, Gpu *gpu) {
    auto mark = [gpu](uint32_t value) {if (gpu) gpu->mark(value);};
    uint8_t block[kBlock];
    if (region < 2 * kBlock || !readBlock(0, block) || memcmp(block, "SHNGIMP2", 8)) {mark(0x63f2); return false;}
    uint32_t version = 0, count = 0;
    uint64_t indexBytes = 0, dataBytes = 0;
    memcpy(&version, block + 8, 4); memcpy(&count, block + 12, 4); memcpy(&indexBytes, block + 16, 8); memcpy(&dataBytes, block + 24, 8);
    if (version != 2 || !count || indexBytes % kBlock || kBlock + indexBytes + dataBytes > region) {mark(0x63f2); return false;}
    uint8_t digest[32];
    memcpy(digest, block + 32, 32);
    // The index, checked against its digest; an entry's own zlib stream checks its bytes when it is read.
    std::vector<uint8_t> index(indexBytes);
    if (!readBytes(kBlock, indexBytes, index.data())) {mark(0x63f2); return false;}
    CC_SHA256_CTX sha;
    CC_SHA256_Init(&sha);
    CC_SHA256_Update(&sha, index.data(), (CC_LONG)index.size());
    uint8_t got[32];
    CC_SHA256_Final(got, &sha);
    if (memcmp(got, digest, 32)) {mark(0x63f3); return false;}
    gEntries.reserve(count);
    size_t at = 0;
    for (uint32_t i = 0; i < count; ++i) {
        if (at + 18 > index.size()) {mark(0x63f3); gEntries.clear(); return false;}
        Entry e;
        uint16_t pathBytes = 0;
        memcpy(&e.offset, &index[at], 8); memcpy(&e.stored, &index[at + 8], 4); memcpy(&e.bytes, &index[at + 12], 4);
        memcpy(&pathBytes, &index[at + 16], 2);
        at += 18;
        if (at + pathBytes > index.size() || e.offset + e.stored > dataBytes) {mark(0x63f3); gEntries.clear(); return false;}
        e.path.assign(reinterpret_cast<const char *>(&index[at]), pathBytes);
        at += pathBytes;
        gEntries.push_back(std::move(e));
    }
    gDataFirst = kBlock + indexBytes;
    mark(0x6301);
    mark(0x6400 | (count / 1000 > 0xff ? 0xff : count / 1000));
    return true;
}
}  // namespace cacheimport
