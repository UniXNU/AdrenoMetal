// SPDX-License-Identifier: MIT
// Host instrument for drivers/AdrenoMetal/GpuScanoutPresenter.h: anonymous pages stand for the scanout and for the compositor's surface,
// the GPU copy runs on the native frontend's device (bridge over MoltenVK) and every pixel of the scanout is compared with the surface.
// With NMB_SYSTEM_DEVICE=1 the Mac's own device runs the same code as the baseline. This is a host check of the copy's contract, not an
// Adreno run and not the kernel presenter.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <sys/mman.h>
#include <cstdio>
#include <cstdlib>
#include "../GpuScanoutPresenter.h"
#include "../NativeBridgeMetalFrontend.h"

static void *pages(size_t bytes) {
    void *p = mmap(nullptr, bytes, PROT_READ | PROT_WRITE, MAP_ANON | MAP_PRIVATE, -1, 0);
    return p == MAP_FAILED ? nullptr : p;
}
static size_t roundUp(size_t n) {const size_t page = (size_t)getpagesize(); return (n + page - 1) & ~(page - 1);}

// One frame at a given surface pitch and scanout pitch; every pixel of the scanout must equal the surface's pixel, padding must stay untouched.
static bool run(id<MTLDevice> device, uint32_t w, uint32_t h, size_t srcStride, size_t dstStride, uint32_t seed, const char *name) {
    const size_t srcBytes = roundUp(srcStride * h), dstBytes = roundUp(dstStride * h);
    uint8_t *src = (uint8_t *)pages(srcBytes), *dst = (uint8_t *)pages(dstBytes);
    if (!src || !dst) {std::printf("FAIL %s pages\n", name); return false;}
    for (size_t i = 0; i < srcBytes; ++i) src[i] = (uint8_t)((i * 131 + seed) ^ (i >> 7));
    for (size_t i = 0; i < dstBytes; ++i) dst[i] = 0xa5;
    gpupresent::Presenter presenter(device, {dst, dstBytes, dstStride, w, h});
    if (!presenter.ready()) {std::printf("FAIL %s scanout import\n", name); return false;}
    const gpupresent::Result r = presenter.present({src, srcBytes, srcStride, w, h, 0, NGP_BGRX8888});
    if (r != gpupresent::kOk) {std::printf("FAIL %s present %d\n", name, (int)r); return false;}
    for (uint32_t y = 0; y < h; ++y) {
        if (memcmp(dst + y * dstStride, src + y * srcStride, (size_t)w * 4)) {std::printf("FAIL %s row %u differs\n", name, y); return false;}
        for (size_t x = (size_t)w * 4; x < dstStride; ++x)
            if (dst[y * dstStride + x] != 0xa5) {std::printf("FAIL %s row %u padding written at %zu\n", name, y, x); return false;}
    }
    std::printf("ok %s %ux%u pitch %zu->%zu\n", name, w, h, srcStride, dstStride);
    munmap(src, srcBytes); munmap(dst, dstBytes);
    return true;
}

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc < 3) return 2;
        id<MTLDevice> device = getenv("NMB_SYSTEM_DEVICE") ? MTLCreateSystemDefaultDevice() : native_bridge_metal_open(argv[1], argv[2], 0x6e6d62, nullptr);
        if (!device) {std::printf("FAIL device\n"); return 1;}
        bool ok = true;
        ok &= run(device, 256, 64, 256 * 4, 256 * 4, 1, "equal pitch");
        ok &= run(device, 250, 61, 250 * 4 + 8, 256 * 4, 2, "surface pitch smaller");
        ok &= run(device, 250, 61, 256 * 4, 250 * 4 + 40, 3, "scanout pitch smaller");
        ok &= run(device, 254, 33, 254 * 4 + 8, 254 * 4 + 64, 4, "unaligned pitches");
        if (!ok) return 1;
        std::printf("PASS: the GPU copied every pixel of four frames into the scanout pages, padding untouched\n");
    }
    return 0;
}
