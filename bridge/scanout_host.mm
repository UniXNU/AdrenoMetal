// SPDX-License-Identifier: MIT
// Host check of the GPU scanout (GpuScanoutPresenter.h): a half-float frame converted by the compute kernel stores exactly the bytes of the
// display driver's table (NativeHalfCodes.h), for linear and encoded frames, with padded rows on both sides; a BGRA frame is copied with
// its alpha opaque.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <initializer_list>
#include "../NativeBridgeMetalFrontend.h"
#include "../GpuScanoutPresenter.h"

// Shader priming's progress lives in the plugin (PrimeShaders.h); the frontend's telemetry reads it.
extern "C" {volatile uint32_t native_prime_compiling, native_prime_libraries_done, native_prime_libraries_total, native_prime_made, native_prime_rate;}

static void *pages(size_t bytes) {
    void *p = nullptr;
    return posix_memalign(&p, (size_t)getpagesize(), bytes) ? nullptr : p;
}

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc < 3) return 2;
        id<MTLDevice> device = native_bridge_metal_open(argv[1], argv[2], 0x6e6d62, nullptr);
        if (!device) {std::printf("FAIL device\n"); return 1;}
        const size_t page = (size_t)getpagesize();
        const uint32_t width = 1003, height = 517;
        const size_t outStride = width * 4 + 96, outBytes = (outStride * height + page - 1) & ~(page - 1);
        const size_t inStride = width * 8 + 64, inBytes = (inStride * height + page - 1) & ~(page - 1);
        uint8_t *out = (uint8_t *)pages(outBytes), *in = (uint8_t *)pages(inBytes);
        if (!out || !in) return 1;
        gpupresent::Presenter presenter(device, {out, outBytes, outStride, width, height});
        if (!presenter.ready()) {std::printf("FAIL presenter\n"); return 1;}
        srandom(7);
        int failed = 0;
        for (uint32_t format : {(uint32_t)NGP_RGBA16F_LINEAR, (uint32_t)NGP_RGBA16F_ENCODED}) {
            for (size_t i = 0; i < inBytes / 2; ++i) {
                // every pattern class: random bits, small positives, around 1.0, negatives, infinities and NaNs
                uint16_t v = (uint16_t)random();
                if (i % 7 == 0) v = (uint16_t)(0x3c00 - (random() & 0x3ff));
                ((uint16_t *)in)[i] = v;
            }
            memset(out, 0x5a, outBytes);
            const gpupresent::Result r = presenter.present({in, inBytes, inStride, width, height, 0, format});
            if (r != gpupresent::kOk) {std::printf("FAIL format %u: result %d\n", format, (int)r); ++failed; continue;}
            uint64_t wrong = 0;
            for (uint32_t y = 0; y < height; ++y) {
                const uint16_t *src = (const uint16_t *)(in + y * inStride);
                const uint32_t *dst = (const uint32_t *)(out + y * outStride);
                for (uint32_t x = 0; x < width; ++x) {
                    const bool encoded = format == NGP_RGBA16F_ENCODED;
                    const uint32_t want = 0xff000000u | (uint32_t)native_half::code(src[4 * x], encoded) << 16 |
                                          (uint32_t)native_half::code(src[4 * x + 1], encoded) << 8 | native_half::code(src[4 * x + 2], encoded);
                    if (dst[x] != want && wrong++ < 4) std::printf("  format %u (%u,%u): %08x want %08x\n", format, x, y, dst[x], want);
                }
                // the padding after each row is left alone
                for (size_t b = width * 4; b < outStride; ++b) if (out[y * outStride + b] != 0x5a && wrong++ < 4) std::printf("  padding written\n");
            }
            if (wrong) {std::printf("FAIL format %u: %llu wrong\n", format, (unsigned long long)wrong); ++failed;}
        }
        // BGRA is copied with its alpha made opaque (the display blends the scanout by its alpha), every row, the padding left alone
        for (size_t i = 0; i < inBytes; ++i) in[i] = (uint8_t)random();
        memset(out, 0x5a, outBytes);
        if (presenter.present({in, inBytes, width * 4 + 64, width, height, 0, NGP_BGRX8888}) != gpupresent::kOk) {std::printf("FAIL BGRA copy\n"); ++failed;}
        uint64_t bgraWrong = 0;
        for (uint32_t y = 0; y < height; ++y) {
            const uint32_t *src = (const uint32_t *)(in + y * (width * 4 + 64)), *dst = (const uint32_t *)(out + y * outStride);
            for (uint32_t x = 0; x < width; ++x) if (dst[x] != (src[x] | 0xff000000u) && bgraWrong++ < 4) std::printf("  BGRA (%u,%u): %08x\n", x, y, dst[x]);
            for (size_t b = width * 4; b < outStride; ++b) if (out[y * outStride + b] != 0x5a && bgraWrong++ < 4) std::printf("  BGRA padding written\n");
        }
        if (bgraWrong) {std::printf("FAIL BGRA copy: %llu wrong\n", (unsigned long long)bgraWrong); ++failed;}
        if (presenter.present({in, inBytes, inStride, width, height, 0, 77}) != gpupresent::kBadFrame) {std::printf("FAIL unknown format taken\n"); ++failed;}
        if (failed) return 1;
        std::printf("PASS: half-float frames convert to the display driver's bytes on the GPU (linear and encoded), BGRA copies opaque, unknown formats refused\n");
    }
    return 0;
}
