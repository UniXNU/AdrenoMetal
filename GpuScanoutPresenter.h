// SPDX-License-Identifier: MIT
// GPU presentation: the GPU copies a finished BGRA frame (the compositor's IOSurface pages) into the firmware scanout memory. Both are
// imported without a copy (newBufferWithBytesNoCopy: pinned by the kernel and mapped into the GPU's 4 KiB context), the copy is a blit on
// the GPU's queue, and the call returns after the GPU retired it. A half-float frame (RGBA 16F, what iPadOS 27's render server draws
// once a Metal device renders) is converted by a compute kernel (shaders/scanout_convert.ll) through the display driver's own table of
// codes (NativeHalfCodes.h), so the bytes match its CPU path. A BGRA frame goes through the kernel scanout_opaque, which copies it with
// alpha 0xff, as the half-float conversion stores it: the display blends the scanout by its per-pixel alpha (SSPP SRC_FORMAT 0x237ff),
// and the compositor's framebuffer alpha is not opacity (translucent tinted blocks around Liquid Glass, i23c80d087); a device without
// the kernel blits it. No CPU touches a pixel; there is no staging copy and no software fallback here. Used through the public Metal API, on any device (the native frontend on the tablet, the Mac's device for the host check).
#pragma once
#import <Metal/Metal.h>
#include <unistd.h>
#include <stdint.h>
#include <string.h>
#include <atomic>
#include "../GopFramebuffer/NativeGOPWire.h"
#include "../GopFramebuffer/NativeHalfCodes.h"

// The conversion kernel's AIR and the frontend's loader for bare AIR (both in the plugin; weak, so a device without them only blits).
extern "C" const void *nmb_scanout_convert_air(size_t *length) __attribute__((weak_import));
extern "C" id native_bridge_metal_library_with_air(const void *bytes, size_t length) __attribute__((weak_import));

namespace gpupresent {

// Measurement only: when set, the copy covers the upper half of the rows (the GPU cost of the copy is what is being measured).
inline std::atomic<bool> gHalfRows{false};

struct Scanout {
    void *base;       // writable mapping of the firmware framebuffer (page aligned)
    size_t bytes;     // whole extent, a multiple of the page size
    size_t stride;    // bytes per row
    uint32_t width, height;
};

struct Frame {
    const void *base; // page-aligned mapping of the surface pages
    size_t bytes;     // allocation size, a multiple of the page size
    size_t stride;
    uint32_t width, height;
    uint32_t mapping; // the kernel's id for this mapping of the surface (0: none, imported for this frame only)
    uint32_t format;  // NGP_BGRX8888, NGP_RGBA16F_LINEAR or NGP_RGBA16F_ENCODED
};

enum Result { kOk = 0, kBadFrame, kBadScanout, kNoResource, kGpuFailed };

class Presenter {
public:
    Presenter(id<MTLDevice> device, const Scanout &scanout) : device_(device), scanout_(scanout) {
        queue_ = [device newCommandQueue];
        if (!validScanout() || !queue_) return;
        target_ = [device newBufferWithBytesNoCopy:scanout.base length:scanout.bytes options:MTLResourceStorageModeShared deallocator:nil];
    }
    bool ready() const { return target_ != nil; }
    // One frame: encodes the GPU copy, commits, waits for the GPU. Rows are copied as whole rows of width*4 bytes.
    Result present(const Frame &frame) {
        if (!ready()) return kBadScanout;
        if (frame.format == NGP_RGBA16F_LINEAR || frame.format == NGP_RGBA16F_ENCODED) return convert(frame);
        if (frame.format != NGP_BGRX8888) return kBadFrame;
        const size_t page = (size_t)getpagesize(), row = (size_t)frame.width * 4;
        if (!frame.base || ((uintptr_t)frame.base & (page - 1)) || !frame.bytes || (frame.bytes & (page - 1)) ||
            frame.width != scanout_.width || frame.height != scanout_.height || frame.stride < row ||
            (size_t)frame.height > frame.bytes / frame.stride || frame.stride * (frame.height - 1) + row > frame.bytes) return kBadFrame;
        const uint32_t rows = gHalfRows.load(std::memory_order_relaxed) ? frame.height / 2 : frame.height;
        id<MTLBuffer> source = sourceFor(frame);
        if (!source) return kNoResource;
        if (!(frame.stride & 3) && !(scanout_.stride & 3) && pipeline(opaque_, opaqueFailed_, @"scanout_opaque")) {
            id<MTLCommandBuffer> commands = [queue_ commandBuffer];
            id<MTLComputeCommandEncoder> encoder = commands ? [commands computeCommandEncoder] : nil;
            if (!encoder) return kNoResource;
            const uint32_t params[4] = {frame.width, rows, (uint32_t)(frame.stride / 4), (uint32_t)(scanout_.stride / 4)};
            [encoder setComputePipelineState:opaque_];
            [encoder setBuffer:source offset:0 atIndex:0];
            [encoder setBuffer:target_ offset:0 atIndex:1];
            [encoder setBytes:params length:sizeof params atIndex:3];
            [encoder dispatchThreadgroups:MTLSizeMake((frame.width + 15) / 16, (rows + 7) / 8, 1) threadsPerThreadgroup:MTLSizeMake(16, 8, 1)];
            [encoder endEncoding];
            [commands commit];
            [commands waitUntilCompleted];
            return commands.status == MTLCommandBufferStatusCompleted ? kOk : kGpuFailed;
        }
        id<MTLCommandBuffer> commands = [queue_ commandBuffer];
        id<MTLBlitCommandEncoder> blit = commands ? [commands blitCommandEncoder] : nil;
        if (!blit) return kNoResource;
        if (frame.stride == scanout_.stride) {
            // Equal pitch: the rows are one contiguous run.
            [blit copyFromBuffer:source sourceOffset:0 toBuffer:target_ destinationOffset:0 size:frame.stride * (rows - 1) + row];
        } else {
            // A linear texture over the surface pages reads them with the surface's pitch and writes the scanout's pitch.
            MTLTextureDescriptor *d = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm width:frame.width
                                                                                       height:frame.height mipmapped:NO];
            d.storageMode = MTLStorageModeShared;
            d.usage = MTLTextureUsageShaderRead;
            if (frame.stride % [device_ minimumLinearTextureAlignmentForPixelFormat:MTLPixelFormatBGRA8Unorm]) {
                for (uint32_t y = 0; y < rows; ++y)
                    [blit copyFromBuffer:source sourceOffset:y * frame.stride toBuffer:target_ destinationOffset:y * scanout_.stride size:row];
            } else {
                id<MTLTexture> texture = [source newTextureWithDescriptor:d offset:0 bytesPerRow:frame.stride];
                if (!texture) {[blit endEncoding]; return kNoResource;}
                [blit copyFromTexture:texture sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0, 0, 0)
                           sourceSize:MTLSizeMake(frame.width, rows, 1) toBuffer:target_ destinationOffset:0
                    destinationBytesPerRow:scanout_.stride destinationBytesPerImage:scanout_.stride * frame.height];
            }
        }
        [blit endEncoding];
        [commands commit];
        [commands waitUntilCompleted];
        return commands.status == MTLCommandBufferStatusCompleted ? kOk : kGpuFailed;
    }
    // A half-float frame: one thread per pixel, 16 x 8 threads a group (the kernel skips threads past the edges).
    Result convert(const Frame &frame) {
        const size_t page = (size_t)getpagesize(), row = (size_t)frame.width * 8;
        if (!frame.base || ((uintptr_t)frame.base & (page - 1)) || !frame.bytes || (frame.bytes & (page - 1)) ||
            frame.width != scanout_.width || frame.height != scanout_.height || frame.stride < row || (frame.stride & 3) ||
            (scanout_.stride & 3) || frame.stride * (frame.height - 1) + row > frame.bytes) return kBadFrame;
        const bool encoded = frame.format == NGP_RGBA16F_ENCODED;
        if (!pipeline(convert_, convertFailed_, @"scanout_convert") || !table(encoded)) return kNoResource;
        id<MTLBuffer> source = sourceFor(frame);
        if (!source) return kNoResource;
        id<MTLCommandBuffer> commands = [queue_ commandBuffer];
        id<MTLComputeCommandEncoder> encoder = commands ? [commands computeCommandEncoder] : nil;
        if (!encoder) return kNoResource;
        const uint32_t params[4] = {frame.width, frame.height, (uint32_t)(frame.stride / 4), (uint32_t)(scanout_.stride / 4)};
        [encoder setComputePipelineState:convert_];
        [encoder setBuffer:source offset:0 atIndex:0];
        [encoder setBuffer:target_ offset:0 atIndex:1];
        [encoder setBuffer:tables_[encoded] offset:0 atIndex:2];
        [encoder setBytes:params length:sizeof params atIndex:3];
        [encoder dispatchThreadgroups:MTLSizeMake((frame.width + 15) / 16, (frame.height + 7) / 8, 1)
                threadsPerThreadgroup:MTLSizeMake(16, 8, 1)];
        [encoder endEncoding];
        [commands commit];
        [commands waitUntilCompleted];
        return commands.status == MTLCommandBufferStatusCompleted ? kOk : kGpuFailed;
    }
    // A kernel of the scanout library, made once; a failure is not retried (every later half-float frame then goes back to the display
    // driver's own path, every BGRA frame to the blit).
    bool pipeline(__strong id<MTLComputePipelineState> &state, bool &failed, NSString *name) {
        if (state || failed) return state != nil;
        failed = true;
        if (!nmb_scanout_convert_air || !native_bridge_metal_library_with_air) return false;
        size_t length = 0;
        const void *air = nmb_scanout_convert_air(&length);
        id<MTLLibrary> library = air && length ? native_bridge_metal_library_with_air(air, length) : nil;
        id<MTLFunction> function = [library newFunctionWithName:name];
        NSError *error = nil;
        state = function ? [device_ newComputePipelineStateWithFunction:function error:&error] : nil;
        if (state && state.maxTotalThreadsPerThreadgroup < 128) state = nil;
        failed = state == nil;
        return state != nil;
    }
    // The 8-bit code of every 16-bit half pattern, as 32-bit words (65536 of them, 256 KiB).
    id<MTLBuffer> table(bool encoded) {
        if (tables_[encoded]) return tables_[encoded];
        id<MTLBuffer> buffer = [device_ newBufferWithLength:65536 * sizeof(uint32_t) options:MTLResourceStorageModeShared];
        if (!buffer) return nil;
        uint32_t *codes = (uint32_t *)buffer.contents;
        for (uint32_t bits = 0; bits < 65536; ++bits) codes[bits] = native_half::code((uint16_t)bits, encoded);
        tables_[encoded] = buffer;
        return buffer;
    }
private:
    // The compositor cycles through a few surfaces and the kernel keeps each one's mapping under one id, so the GPU import (pinning and
    // mapping the pages, milliseconds per frame otherwise) is made once per surface. The kernel keeps more mappings (6) than kSources, so
    // every import cached here still has its mapping.
    static constexpr unsigned kSources = 4;
    struct Source { uint32_t mapping; const void *base; size_t bytes; uint64_t used; id<MTLBuffer> buffer; };
    id<MTLBuffer> sourceFor(const Frame &frame) {
        if (!frame.mapping)
            return [device_ newBufferWithBytesNoCopy:(void *)frame.base length:frame.bytes options:MTLResourceStorageModeShared deallocator:nil];
        unsigned victim = 0;
        for (unsigned i = 0; i < kSources; ++i) {
            Source &s = sources_[i];
            if (s.buffer && s.mapping == frame.mapping && s.base == frame.base && s.bytes == frame.bytes) {s.used = ++uses_; return s.buffer;}
            if (!sources_[i].buffer || sources_[i].used < sources_[victim].used) victim = i;
        }
        id<MTLBuffer> buffer = [device_ newBufferWithBytesNoCopy:(void *)frame.base length:frame.bytes options:MTLResourceStorageModeShared deallocator:nil];
        if (buffer) sources_[victim] = {frame.mapping, frame.base, frame.bytes, ++uses_, buffer};
        return buffer;
    }
    bool validScanout() const {
        const size_t page = (size_t)getpagesize();
        return scanout_.base && !((uintptr_t)scanout_.base & (page - 1)) && scanout_.bytes && !(scanout_.bytes & (page - 1)) &&
               scanout_.width && scanout_.height && scanout_.stride >= (size_t)scanout_.width * 4 &&
               scanout_.stride * scanout_.height <= scanout_.bytes;
    }
    id<MTLDevice> device_;
    Scanout scanout_;
    id<MTLCommandQueue> queue_;
    id<MTLBuffer> target_;
    id<MTLComputePipelineState> convert_ = nil, opaque_ = nil;
    bool convertFailed_ = false, opaqueFailed_ = false;
    id<MTLBuffer> tables_[2] = {nil, nil};
    Source sources_[kSources] = {};
    uint64_t uses_ = 0;
};

}  // namespace gpupresent
