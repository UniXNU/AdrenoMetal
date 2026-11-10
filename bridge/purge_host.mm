// SPDX-License-Identifier: MIT
// Purgeable memory through the public Metal API (bridge/NativeMetalPurge.rs): a resource declared Volatile is discarded only while
// nothing uses it, answers Empty afterwards, and is usable again; every case reads back what the GPU wrote.
// Arguments: the Vulkan loader, a cache directory, and the mode: "on" (the default behaviour), "off" (run with NATIVE_METAL_PURGE=0 in
// the environment, the boot argument gpu-purge=0: nothing is discarded), "timer" (the age timer discards by itself; waits two ages),
// "stress" (four threads that follow the contract and one that does not, against two threads that discard without pause).
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <IOSurface/IOSurfaceRef.h>
#include <cstdio>
#include <cstring>
#include <atomic>
#include <string>
#include <thread>
#include <unistd.h>
#include <vector>
#include "NativeMetalObjects.h"
#include "../NativeBridgeMetalFrontend.h"

static id<MTLDevice> gDeviceUnderTest;
// One queue a thread, kept for the thread's life: the helpers below take the calling thread's.
static thread_local void *tQueue;
static id<MTLCommandQueue> queue() {
    if (!tQueue) tQueue = (__bridge_retained void *)[gDeviceUnderTest newCommandQueue];
    return (__bridge id<MTLCommandQueue>)tQueue;
}
static std::atomic<unsigned> gFailed;
#define CHECK(c, ...) do { if (!(c)) { std::printf("FAIL line %d: ", __LINE__); std::printf(__VA_ARGS__); std::printf("\n"); gFailed++; } } while (0)

struct Counts {
    uint64_t w[16];
    uint64_t discarded() const {return w[0] + w[2];}
    uint64_t discardedBytes() const {return w[1] + w[3];}
    uint64_t again() const {return w[4];}
    uint64_t byUse() const {return w[6];}
    uint64_t emptyAnswers() const {return w[8];}
    uint64_t outBytes() const {return w[9];}
    uint64_t pinnedBytes() const {return w[11];}
    uint64_t leftUse() const {return w[12];}
    uint64_t leftTie() const {return w[13];}
    uint64_t leftYoung() const {return w[14];}
    uint64_t failed() const {return w[15];}
};
static Counts counts() {
    Counts c = {};
    native_bridge_metal_purge_counts(c.w);
    return c;
}
static uint64_t pressure() {return native_bridge_metal_purge(0, 0);}

static bool finish(id<MTLCommandBuffer> commands, const char *what) {
    [commands commit];
    [commands waitUntilCompleted];
    if (commands.status == MTLCommandBufferStatusCompleted) return true;
    std::printf("FAIL %s: status %lu %s\n", what, (unsigned long)commands.status, commands.error.localizedDescription.UTF8String ?: "");
    gFailed++;
    return false;
}
// A side x side RGBA8 texture's texels filled with `value` through a blit from a shared buffer.
static void fill(id<MTLTexture> texture, unsigned side, uint8_t value) {
    id<MTLBuffer> source = [gDeviceUnderTest newBufferWithLength:side * side * 4 options:MTLResourceStorageModeShared];
    memset(source.contents, value, side * side * 4);
    id<MTLCommandBuffer> commands = [queue() commandBuffer];
    id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
    [blit copyFromBuffer:source sourceOffset:0 sourceBytesPerRow:side * 4 sourceBytesPerImage:side * side * 4 sourceSize:MTLSizeMake(side, side, 1)
               toTexture:texture destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
    [blit endEncoding];
    finish(commands, "fill");
}
// How many of the texture's bytes differ from `value`, read back through a blit into a shared buffer.
static unsigned wrong(id<MTLTexture> texture, unsigned side, uint8_t value) {
    id<MTLBuffer> out = [gDeviceUnderTest newBufferWithLength:side * side * 4 options:MTLResourceStorageModeShared];
    memset(out.contents, value ^ 0xff, side * side * 4);
    id<MTLCommandBuffer> commands = [queue() commandBuffer];
    id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
    [blit copyFromTexture:texture sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0, 0, 0) sourceSize:MTLSizeMake(side, side, 1)
                 toBuffer:out destinationOffset:0 destinationBytesPerRow:side * 4 destinationBytesPerImage:side * side * 4];
    [blit endEncoding];
    if (!finish(commands, "readback")) return side * side * 4;
    unsigned bad = 0;
    const uint8_t *bytes = (const uint8_t *)out.contents;
    for (unsigned i = 0; i < side * side * 4; ++i) bad += bytes[i] != value;
    return bad;
}
static unsigned wrongBuffer(id<MTLBuffer> buffer, uint8_t value) {
    const NSUInteger length = buffer.length;
    id<MTLBuffer> out = [gDeviceUnderTest newBufferWithLength:length options:MTLResourceStorageModeShared];
    memset(out.contents, value ^ 0xff, length);
    id<MTLCommandBuffer> commands = [queue() commandBuffer];
    id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
    [blit copyFromBuffer:buffer sourceOffset:0 toBuffer:out destinationOffset:0 size:length];
    [blit endEncoding];
    if (!finish(commands, "buffer readback")) return (unsigned)length;
    unsigned bad = 0;
    const uint8_t *bytes = (const uint8_t *)out.contents;
    for (NSUInteger i = 0; i < length; ++i) bad += bytes[i] != value;
    return bad;
}
static void fillBuffer(id<MTLBuffer> buffer, uint8_t value) {
    id<MTLCommandBuffer> commands = [queue() commandBuffer];
    id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
    [blit fillBuffer:buffer range:NSMakeRange(0, buffer.length) value:value];
    [blit endEncoding];
    finish(commands, "buffer fill");
}
static id<MTLTexture> texture(unsigned side, MTLStorageMode mode = MTLStorageModePrivate) {
    auto d = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:side height:side mipmapped:NO];
    d.storageMode = mode;
    d.usage = MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite | MTLTextureUsagePixelFormatView;
    return [gDeviceUnderTest newTextureWithDescriptor:d];
}
// The MiB placed in the runtime's pooled blocks (the telemetry's MEM row).
static unsigned long long placedMiB() {
    static char text[16384];
    native_bridge_metal_telemetry(text, sizeof text);
    unsigned long long pool = 0, placed = 0;
    const char *row = strstr(text, "MEM pool ");
    if (!row || sscanf(row, "MEM pool %llu(%llu)", &pool, &placed) != 2) {std::puts("FAIL no MEM row");gFailed++;}
    return placed;
}

// 1: volatile, discard, NonVolatile answers Empty, the texture is usable again.
static void textureCase() {
    const unsigned side = 256;
    id<MTLTexture> t = texture(side);
    fill(t, side, 0x5a);
    CHECK(wrong(t, side, 0x5a) == 0, "texture contents before the discard");
    CHECK([t setPurgeableState:MTLPurgeableStateVolatile] == MTLPurgeableStateNonVolatile, "Volatile answers the state before");
    CHECK([t setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateVolatile, "the state is Volatile");
    const Counts before = counts();
    const uint64_t bytes = pressure();
    Counts after = counts();
    CHECK(bytes >= side * side * 4 && after.discarded() == before.discarded() + 1 && after.outBytes() == before.outBytes() + bytes,
          "an idle Volatile texture is discarded (%llu bytes, %llu discards)", (unsigned long long)bytes, (unsigned long long)(after.discarded() - before.discarded()));
    // A query answers Empty and places nothing.
    CHECK([t setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty, "a discarded texture answers Empty to a query");
    CHECK(counts().outBytes() == after.outBytes(), "a query places nothing");
    // Volatile asked of an Empty object leaves it Empty.
    CHECK([t setPurgeableState:MTLPurgeableStateVolatile] == MTLPurgeableStateEmpty, "Volatile on a discarded texture answers Empty");
    CHECK([t setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty, "and leaves it Empty");
    CHECK([t setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "NonVolatile answers Empty for discarded contents");
    after = counts();
    CHECK(after.again() == before.again() + 1 && after.outBytes() == before.outBytes() && after.emptyAnswers() == before.emptyAnswers() + 1,
          "NonVolatile places the texture again");
    CHECK([t setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateNonVolatile, "the state is NonVolatile again");
    CHECK(wrong(t, side, 0) == 0, "a texture placed again reads zero");
    fill(t, side, 0xa7);
    CHECK(wrong(t, side, 0xa7) == 0, "a texture placed again is written and read");
    CHECK(pressure() == 0, "a NonVolatile texture is not discarded");
    CHECK(wrong(t, side, 0xa7) == 0, "and keeps its contents");
}

// 2: a texture a batch in flight uses, and one an encoder being recorded names, are not discarded.
static void inFlightCase() {
    const unsigned side = 128;
    id<MTLTexture> t = texture(side);
    id<MTLBuffer> source = [gDeviceUnderTest newBufferWithLength:side * side * 4 options:MTLResourceStorageModeShared];
    memset(source.contents, 0x3c, side * side * 4);
    // In flight: committed behind an event that is not signaled yet.
    id<MTLSharedEvent> event = [gDeviceUnderTest newSharedEvent];
    id<MTLCommandBuffer> commands = [queue() commandBuffer];
    [commands encodeWaitForEvent:event value:1];
    id<MTLBlitCommandEncoder> blit = [commands blitCommandEncoder];
    [blit copyFromBuffer:source sourceOffset:0 sourceBytesPerRow:side * 4 sourceBytesPerImage:side * side * 4 sourceSize:MTLSizeMake(side, side, 1)
               toTexture:t destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
    [blit endEncoding];
    [commands commit];
    [t setPurgeableState:MTLPurgeableStateVolatile];
    Counts before = counts();
    CHECK(pressure() == 0, "a texture a batch in flight writes is not discarded");
    Counts after = counts();
    CHECK(after.discarded() == before.discarded() && after.leftUse() >= side * side * 4, "it is left as in use (%llu bytes)", (unsigned long long)after.leftUse());
    event.signaledValue = 1;
    [commands waitUntilCompleted];
    CHECK(commands.status == MTLCommandBufferStatusCompleted, "the gated command buffer completed");
    // The completion handler's turn releases the hold; the texture still has what the batch wrote.
    CHECK([t setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateVolatile, "it was not discarded: NonVolatile answers Volatile");
    CHECK(wrong(t, side, 0x3c) == 0, "and has what the batch wrote");

    // Being recorded: an encoder of a command buffer that is not committed names it.
    id<MTLCommandBuffer> recording = [queue() commandBuffer];
    id<MTLBlitCommandEncoder> open = [recording blitCommandEncoder];
    memset(source.contents, 0x6d, side * side * 4);
    [open copyFromBuffer:source sourceOffset:0 sourceBytesPerRow:side * 4 sourceBytesPerImage:side * side * 4 sourceSize:MTLSizeMake(side, side, 1)
               toTexture:t destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
    [t setPurgeableState:MTLPurgeableStateVolatile];
    before = counts();
    CHECK(pressure() == 0 && counts().discarded() == before.discarded(), "a texture an encoder being recorded names is not discarded");
    [open endEncoding];
    finish(recording, "recorded copy");
    CHECK([t setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateVolatile, "it was not discarded while recorded");
    CHECK(wrong(t, side, 0x6d) == 0, "and has what the recorded copy wrote");
    // Idle now: it goes.
    [t setPurgeableState:MTLPurgeableStateVolatile];
    for (unsigned i = 0; i < 50 && !pressure(); ++i) usleep(10000);   // the completion's release of the hold may still be on its thread
    CHECK([t setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty, "once nothing uses it, it is discarded");

    // A command buffer released without a commit lets go of what it named.
    id<MTLTexture> dropped = texture(side);
    @autoreleasepool {
        id<MTLCommandBuffer> never = [queue() commandBuffer];
        id<MTLBlitCommandEncoder> unused = [never blitCommandEncoder];
        [unused generateMipmapsForTexture:dropped];
        [unused endEncoding];
        [dropped setPurgeableState:MTLPurgeableStateVolatile];
        CHECK(pressure() == 0, "named by a command buffer that is not committed");
        never = nil;unused = nil;
    }
    for (unsigned i = 0; i < 50 && !pressure(); ++i) usleep(10000);
    CHECK([dropped setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty, "a command buffer released without a commit holds nothing");
}

// 3: views. A texture with a view is not discarded; a view made of a discarded texture shares the memory it is placed in again.
static void viewCase() {
    const unsigned side = 128;
    id<MTLTexture> parent = texture(side);
    fill(parent, side, 0x11);
    @autoreleasepool {
        id<MTLTexture> view = [parent newTextureViewWithPixelFormat:MTLPixelFormatRGBA8Unorm];
        // A view keeps no state of its own, as Metal's.
        CHECK([view setPurgeableState:MTLPurgeableStateVolatile] == MTLPurgeableStateNonVolatile &&
              [view setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateNonVolatile, "a view keeps no purgeable state");
        CHECK(pressure() == 0, "a parent that is not Volatile is not discarded for its view's request");
        [parent setPurgeableState:MTLPurgeableStateVolatile];
        const Counts before = counts();
        CHECK(pressure() == 0, "a Volatile texture with a live view is not discarded");
        CHECK(counts().leftTie() >= side * side * 4 && counts().discarded() == before.discarded(), "it is left as tied");
        CHECK(wrong(view, side, 0x11) == 0, "the view reads the parent's texels");
        view = nil;
    }
    CHECK(pressure() > 0, "once its view is gone the Volatile texture is discarded");
    // A view of a discarded parent: a use without NonVolatile first. The parent is placed again (zeroed) and the view shares it.
    const Counts before = counts();
    id<MTLTexture> view = [parent newTextureViewWithPixelFormat:MTLPixelFormatRGBA8Unorm];
    CHECK(view != nil, "a view of a discarded texture is made");
    CHECK(counts().byUse() == before.byUse() + 1, "making it placed the parent again");
    CHECK(wrong(view, side, 0) == 0, "the view reads zero");
    fill(view, side, 0x77);
    CHECK(wrong(parent, side, 0x77) == 0, "what is written through the view is read through the parent");
    CHECK([parent setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty, "the parent's state is still Empty");
    CHECK([parent setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "NonVolatile answers Empty once");
    CHECK([parent setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateNonVolatile, "and NonVolatile after");
    CHECK(wrong(parent, side, 0x77) == 0, "the parent was not placed a second time");
}

// 4: heaps. A heap that holds a resource is not discarded; an empty one is, answers as an empty heap, and serves again.
static void heapCase() {
    auto hd = [MTLHeapDescriptor new];
    hd.size = 1 << 20;
    hd.storageMode = MTLStorageModePrivate;
    id<MTLHeap> heap = [gDeviceUnderTest newHeapWithDescriptor:hd];
    CHECK(heap != nil, "heap");
    const NSUInteger size = heap.size, room = [heap maxAvailableSizeWithAlignment:256];
    @autoreleasepool {
        id<MTLBuffer> placed = [heap newBufferWithLength:65536 options:MTLResourceStorageModePrivate];
        CHECK(placed != nil && heap.usedSize == 65536, "a buffer in the heap");
        fillBuffer(placed, 0x42);
        [heap setPurgeableState:MTLPurgeableStateVolatile];
        const Counts before = counts();
        CHECK(pressure() == 0 && counts().discarded() == before.discarded(), "a Volatile heap that holds a buffer is not discarded");
        CHECK(counts().leftTie() >= size, "it is left as tied");
        CHECK(wrongBuffer(placed, 0x42) == 0, "the buffer keeps its bytes");
        placed = nil;
    }
    uint64_t bytes = 0;
    for (unsigned i = 0; i < 50 && !(bytes = pressure()); ++i) usleep(10000);
    CHECK(bytes == size, "an empty Volatile heap is discarded whole (%llu of %lu)", (unsigned long long)bytes, (unsigned long)size);
    // A discarded heap answers as the empty heap it is, and places nothing for it.
    const uint64_t out = counts().outBytes();
    CHECK(heap.size == size && heap.usedSize == 0 && heap.currentAllocatedSize == size && [heap maxAvailableSizeWithAlignment:256] == room,
          "a discarded heap answers as an empty heap");
    CHECK([heap setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty && counts().outBytes() == out, "and stays discarded");
    CHECK([heap setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "NonVolatile answers Empty");
    id<MTLBuffer> again = [heap newBufferWithLength:65536 options:MTLResourceStorageModePrivate];
    CHECK(again != nil && heap.usedSize == 65536, "the heap serves a buffer again");
    CHECK(wrongBuffer(again, 0) == 0, "which reads zero");
    fillBuffer(again, 0x99);
    CHECK(wrongBuffer(again, 0x99) == 0, "and is written and read");
    auto td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:64 height:64 mipmapped:NO];
    td.storageMode = MTLStorageModePrivate;
    td.usage = MTLTextureUsageShaderRead;
    id<MTLTexture> heapTexture = [heap newTextureWithDescriptor:td];
    CHECK(heapTexture != nil, "the heap serves a texture again");
    fill(heapTexture, 64, 0x2e);
    CHECK(wrong(heapTexture, 64, 0x2e) == 0, "which is written and read");
    again = nil;heapTexture = nil;

    // A resource made in a discarded heap without NonVolatile first: the heap is placed again by the use.
    auto hd2 = [MTLHeapDescriptor new];
    hd2.size = 1 << 20;
    hd2.storageMode = MTLStorageModeShared;
    id<MTLHeap> shared = [gDeviceUnderTest newHeapWithDescriptor:hd2];
    [shared setPurgeableState:MTLPurgeableStateVolatile];
    CHECK(pressure() >= shared.size, "an empty Shared heap is discarded");
    const uint64_t uses = counts().byUse();
    id<MTLBuffer> late = [shared newBufferWithLength:4096 options:MTLResourceStorageModeShared];
    CHECK(late != nil && late.contents != nullptr && counts().byUse() == uses + 1, "a buffer made in a discarded heap places the heap again");
    memset(late.contents, 0x31, 4096);
    CHECK(wrongBuffer(late, 0x31) == 0, "and is written by the CPU and read by the GPU");
    CHECK(pressure() == 0, "the heap that holds it stays");
}

// 5: buffers. One whose CPU pointer was handed out is never discarded; one without is.
static void bufferCase() {
    id<MTLBuffer> pinned = [gDeviceUnderTest newBufferWithLength:65536 options:MTLResourceStorageModeShared];
    uint8_t *pointer = (uint8_t *)pinned.contents;
    memset(pointer, 0x21, 65536);
    Counts before = counts();
    CHECK([pinned setPurgeableState:MTLPurgeableStateVolatile] == MTLPurgeableStateNonVolatile, "a Shared buffer keeps its state");
    CHECK(pressure() == 0 && counts().discarded() == before.discarded(), "a buffer whose pointer was handed out is not discarded");
    CHECK(counts().pinnedBytes() == before.pinnedBytes() + 65536, "it is counted as never discardable");
    CHECK(pinned.contents == pointer && pointer[0] == 0x21 && pointer[65535] == 0x21, "its pointer and bytes stay");
    CHECK(wrongBuffer(pinned, 0x21) == 0, "and the GPU reads them");
    CHECK([pinned setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateVolatile, "NonVolatile answers Volatile");

    id<MTLBuffer> priv = [gDeviceUnderTest newBufferWithLength:65536 options:MTLResourceStorageModePrivate];
    fillBuffer(priv, 0x64);
    CHECK(priv.contents == nullptr, "a Private buffer has no pointer");
    [priv setPurgeableState:MTLPurgeableStateVolatile];
    uint64_t bytes = 0;
    for (unsigned i = 0; i < 50 && !(bytes = pressure()); ++i) usleep(10000);
    CHECK(bytes == 65536, "an idle Volatile Private buffer is discarded");
    CHECK(priv.length == 65536, "its length stays");
    CHECK([priv setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "NonVolatile answers Empty");
    CHECK(wrongBuffer(priv, 0) == 0, "a buffer placed again reads zero");
    fillBuffer(priv, 0x65);
    CHECK(wrongBuffer(priv, 0x65) == 0, "and is written and read");

    // A Shared buffer whose pointer was never asked for goes; the pointer asked for afterwards is the new placement's, and stays.
    std::vector<uint8_t> initial(65536, 0x17);
    id<MTLBuffer> unseen = [gDeviceUnderTest newBufferWithBytes:initial.data() length:initial.size() options:MTLResourceStorageModeShared];
    CHECK(wrongBuffer(unseen, 0x17) == 0, "a buffer made with bytes");
    [unseen setPurgeableState:MTLPurgeableStateVolatile];
    bytes = 0;
    for (unsigned i = 0; i < 50 && !(bytes = pressure()); ++i) usleep(10000);
    CHECK(bytes == 65536, "a Shared buffer whose pointer was never handed out is discarded");
    uint8_t *late = (uint8_t *)unseen.contents;
    CHECK(late != nullptr && late[0] == 0 && late[65535] == 0, "its pointer, asked for after the discard, reads zero");
    CHECK([unseen setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty, "its state is Empty");
    memset(late, 0x18, 65536);
    CHECK(pressure() == 0 && unseen.contents == late && late[0] == 0x18, "and it is never discarded again");
    CHECK([unseen setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "NonVolatile answers Empty once");
    CHECK(wrongBuffer(unseen, 0x18) == 0, "the GPU reads what the CPU wrote");

    // A buffer whose GPU address was handed out is never discarded either.
    id<MTLBuffer> addressed = [gDeviceUnderTest newBufferWithLength:65536 options:MTLResourceStorageModePrivate];
    const uint64_t address = addressed.gpuAddress;
    [addressed setPurgeableState:MTLPurgeableStateVolatile];
    CHECK(pressure() == 0 && addressed.gpuAddress == address, "a buffer whose GPU address was handed out is not discarded");
}

// 6: what never has a state: a texture too small for an allocation of its own, and a texture over a surface.
static void fixedCase() {
    id<MTLTexture> small = texture(16);
    fill(small, 16, 0x44);
    CHECK([small setPurgeableState:MTLPurgeableStateVolatile] == MTLPurgeableStateNonVolatile &&
          [small setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateNonVolatile, "a small texture keeps no state");
    NSDictionary *p = @{(id)kIOSurfaceWidth:@64, (id)kIOSurfaceHeight:@64, (id)kIOSurfaceBytesPerElement:@4, (id)kIOSurfacePixelFormat:@((uint32_t)0x52474241)};
    IOSurfaceRef surface = IOSurfaceCreate((__bridge CFDictionaryRef)p);
    auto td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:64 height:64 mipmapped:NO];
    td.storageMode = MTLStorageModeShared;
    td.usage = MTLTextureUsageShaderRead | MTLTextureUsageRenderTarget;
    id<MTLTexture> onSurface = surface ? [gDeviceUnderTest newTextureWithDescriptor:td iosurface:surface plane:0] : nil;
    if (onSurface) {
        fill(onSurface, 64, 0x58);
        CHECK([onSurface setPurgeableState:MTLPurgeableStateVolatile] == MTLPurgeableStateNonVolatile &&
              [onSurface setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateNonVolatile, "a texture over a surface keeps no state");
    } else std::puts("note: no IOSurface texture on this host; the surface case is not run");
    const Counts before = counts();
    CHECK(pressure() == 0 && counts().discarded() == before.discarded(), "neither is discarded");
    CHECK(wrong(small, 16, 0x44) == 0, "the small texture keeps its texels");
    if (onSurface) CHECK(wrong(onSurface, 64, 0x58) == 0, "the surface's texture keeps its texels");
    if (surface) CFRelease(surface);
}

// 7: age, and a discarded texture used by a command without NonVolatile first.
static void ageAndUseCase() {
    const unsigned side = 128;
    id<MTLTexture> t = texture(side);
    fill(t, side, 0x0f);
    [t setPurgeableState:MTLPurgeableStateVolatile];
    Counts before = counts();
    CHECK(native_bridge_metal_purge(1, 3600000) == 0, "a texture Volatile for less than the age is not discarded by age");
    CHECK(counts().leftYoung() >= side * side * 4 && counts().discarded() == before.discarded(), "it is left as young");
    usleep(60000);
    CHECK(native_bridge_metal_purge(1, 50) > 0, "idle for the age, it is discarded by age");
    CHECK(counts().w[2] == before.w[2] + 1, "counted under age");
    // Used by a command with no NonVolatile first (the caller's error): placed again, zeroed, and the command runs.
    before = counts();
    CHECK(wrong(t, side, 0) == 0, "a discarded texture a command reads is placed again and reads zero");
    CHECK(counts().byUse() == before.byUse() + 1 && counts().failed() == 0, "counted as placed again by a use");
    CHECK([t setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty, "its state is still Empty");
    // Empty with memory and idle: it may go again.
    uint64_t again = 0;
    for (unsigned i = 0; i < 50 && !(again = pressure()); ++i) usleep(10000);
    CHECK(again > 0, "Empty with memory and idle, it is discarded again");
    CHECK([t setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "and answers Empty to NonVolatile");
    // replaceRegion and getBytes on a discarded Shared texture.
    id<MTLTexture> shared = texture(64, MTLStorageModeShared);
    [shared setPurgeableState:MTLPurgeableStateVolatile];
    CHECK(pressure() > 0, "a Shared texture is discarded");
    std::vector<uint8_t> texels(64 * 64 * 4, 0x73), read(64 * 64 * 4, 0xff);
    [shared replaceRegion:MTLRegionMake2D(0, 0, 64, 64) mipmapLevel:0 withBytes:texels.data() bytesPerRow:64 * 4];
    [shared getBytes:read.data() bytesPerRow:64 * 4 fromRegion:MTLRegionMake2D(0, 0, 64, 64) mipmapLevel:0];
    CHECK(read == texels, "replaceRegion and getBytes work on a texture that was discarded");
}

// 8: the memory comes back: placed bytes fall once the discarded textures' placements are returned.
static void memoryCase() {
    const unsigned side = 1024, count = 6;   // 4 MiB each
    NSMutableArray<id<MTLTexture>> *textures = [NSMutableArray array];
    for (unsigned i = 0; i < count; ++i) {
        id<MTLTexture> t = texture(side);
        fill(t, side, (uint8_t)(0x80 + i));
        [textures addObject:t];
    }
    const unsigned long long before = placedMiB();
    for (id<MTLTexture> t in textures) [t setPurgeableState:MTLPurgeableStateVolatile];
    uint64_t bytes = 0;
    for (unsigned i = 0; i < 100 && bytes < (uint64_t)count * side * side * 4; ++i) {bytes += native_bridge_metal_relieve_memory();usleep(10000);}
    CHECK(bytes >= (uint64_t)count * side * side * 4, "six 4 MiB textures are discarded (%llu bytes)", (unsigned long long)bytes);
    native_bridge_metal_relieve_memory();   // the objects parked at the discard go, and the blocks they emptied
    const unsigned long long after = placedMiB();
    std::printf("placed %lluM before, %lluM after the discard of %lluM\n", before, after, (unsigned long long)(bytes >> 20));
    CHECK(after + 20 <= before, "the placed bytes fall by the discarded textures' (%lluM to %lluM)", before, after);
    for (unsigned i = 0; i < count; ++i) {
        CHECK([textures[i] setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "texture %u answers Empty", i);
        fill(textures[i], side, (uint8_t)(0x90 + i));
    }
    for (unsigned i = 0; i < count; ++i) CHECK(wrong(textures[i], side, (uint8_t)(0x90 + i)) == 0, "texture %u is written and read again", i);

    // By time alone: after a discard with no pressure event, the runtime parks the released images for a second (its recycler)
    // and its room thread lets them go when that time is up, though the process makes and releases nothing more (bridge patch
    // 0034). Nothing is made or released here between the discard and the reading.
    usleep(300000);   // the readbacks' own buffers and holds are gone
    const unsigned long long placed = placedMiB();
    for (id<MTLTexture> t in textures) [t setPurgeableState:MTLPurgeableStateVolatile];
    bytes = 0;
    for (unsigned i = 0; i < 100 && bytes < (uint64_t)count * side * side * 4; ++i) {bytes += pressure();usleep(10000);}
    CHECK(bytes >= (uint64_t)count * side * side * 4, "the six textures are discarded again (%llu bytes)", (unsigned long long)bytes);
    const unsigned long long parked = placedMiB();
    usleep(2500000);
    const unsigned long long aged = placedMiB();
    std::printf("placed %lluM before, %lluM right after the discard (parked), %lluM 2.5 s later\n", placed, parked, aged);
    CHECK(aged + 20 <= placed, "the parked images go by time, with nothing made or released (%lluM to %lluM)", placed, aged);
}

// gpu-purge=0: the state is kept, nothing is discarded, the answers are the runtime's.
static void offCase() {
    const unsigned side = 256;
    id<MTLTexture> t = texture(side);
    fill(t, side, 0x5a);
    CHECK([t setPurgeableState:MTLPurgeableStateVolatile] == MTLPurgeableStateNonVolatile, "Volatile answers NonVolatile");
    CHECK(pressure() == 0 && native_bridge_metal_purge(1, 0) == 0 && native_bridge_metal_relieve_memory() == 0, "nothing is discarded");
    CHECK([t setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateVolatile, "NonVolatile answers Volatile");
    CHECK(wrong(t, side, 0x5a) == 0, "the texture keeps its texels");
    auto hd = [MTLHeapDescriptor new];
    hd.size = 1 << 20;
    id<MTLHeap> heap = [gDeviceUnderTest newHeapWithDescriptor:hd];
    [heap setPurgeableState:MTLPurgeableStateVolatile];
    id<MTLBuffer> priv = [gDeviceUnderTest newBufferWithLength:65536 options:MTLResourceStorageModePrivate];
    fillBuffer(priv, 0x64);
    [priv setPurgeableState:MTLPurgeableStateVolatile];
    CHECK(pressure() == 0, "nor a heap or a buffer");
    CHECK([heap setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateVolatile &&
          [priv setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateVolatile, "both answer Volatile");
    CHECK(wrongBuffer(priv, 0x64) == 0, "the buffer keeps its bytes");
    const Counts c = counts();
    for (uint64_t word : c.w) CHECK(word == 0, "every purge count is zero");
}

// The age timer by itself: a Volatile texture nothing uses goes within two ages, with no call from here.
static void timerCase() {
    const unsigned side = 256;
    id<MTLTexture> t = texture(side);
    fill(t, side, 0x5a);
    [t setPurgeableState:MTLPurgeableStateVolatile];
    const uint64_t age = nmb_purge_age_ms();
    const Counts before = counts();
    for (uint64_t waited = 0; waited < age * 2 + 2000 && counts().w[2] == before.w[2]; waited += 250) usleep(250000);
    CHECK(counts().w[2] == before.w[2] + 1, "the age timer discarded the idle Volatile texture (age %llu ms)", (unsigned long long)age);
    CHECK([t setPurgeableState:MTLPurgeableStateNonVolatile] == MTLPurgeableStateEmpty, "which answers Empty");
    fill(t, side, 0x66);
    CHECK(wrong(t, side, 0x66) == 0, "and is usable again");

    // A heap declared Volatile while it holds a buffer: the timer finds it tied and stops; the buffer's release starts it again.
    auto hd = [MTLHeapDescriptor new];
    hd.size = 1 << 20;
    id<MTLHeap> heap = [gDeviceUnderTest newHeapWithDescriptor:hd];
    const Counts held = counts();
    @autoreleasepool {
        id<MTLBuffer> placed = [heap newBufferWithLength:65536 options:MTLResourceStorageModePrivate];
        CHECK(placed != nil, "a buffer in the heap");
        [heap setPurgeableState:MTLPurgeableStateVolatile];
        usleep((useconds_t)(age / 2 + 1000) * 1000);   // one pass of the timer
        CHECK(counts().w[2] == held.w[2] && [heap setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateVolatile, "the heap that holds a buffer stays");
        placed = nil;
    }
    for (uint64_t waited = 0; waited < age * 2 + 2000 && counts().w[2] == held.w[2]; waited += 250) usleep(250000);
    CHECK(counts().w[2] == held.w[2] + 1 && [heap setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty,
          "the age timer discarded the heap after its buffer went");
}

// Threads that follow the contract (NonVolatile before a use, the contents made again on Empty, Volatile after) never read
// anything but what they last wrote, or zero right after an Empty answer, while two threads discard without pause; a thread that
// uses the same resources with no NonVolatile first (the caller's error) completes every command.
static void stressCase() {
    const unsigned side = 64, workers = 4, each = 6;
    struct Item {id<MTLTexture> texture;id<MTLBuffer> buffer;std::atomic<bool> busy;uint8_t texels, bytes;};
    static Item items[workers * each];
    for (Item &item : items) {
        item.texture = texture(side);
        item.buffer = [gDeviceUnderTest newBufferWithLength:65536 options:MTLResourceStorageModePrivate];
        item.texels = item.bytes = 0;   // new resources read zero
        item.busy = false;
        [item.texture setPurgeableState:MTLPurgeableStateVolatile];
        [item.buffer setPurgeableState:MTLPurgeableStateVolatile];
    }
    static std::atomic<bool> stop;
    static std::atomic<uint64_t> rounds, empties, rogue;
    std::vector<std::thread> threads;
    for (unsigned sweeper = 0; sweeper < 2; ++sweeper)
        threads.emplace_back([sweeper] {
            while (!stop) {
                if (sweeper) native_bridge_metal_relieve_memory(); else native_bridge_metal_purge(1, 0);
                usleep(200);
            }
        });
    for (unsigned worker = 0; worker < workers; ++worker)
        threads.emplace_back([worker] {
            @autoreleasepool {
                uint8_t next = (uint8_t)(1 + worker * 50);
                for (unsigned round = 0; !stop; ++round) {
                    @autoreleasepool {
                        Item &item = items[worker * each + round % each];
                        // The rogue thread may be inside this item: the contract is one thread's, so take turns.
                        if (item.busy.exchange(true)) continue;
                        const MTLPurgeableState texture = [item.texture setPurgeableState:MTLPurgeableStateNonVolatile];
                        const MTLPurgeableState buffer = [item.buffer setPurgeableState:MTLPurgeableStateNonVolatile];
                        if (texture == MTLPurgeableStateEmpty) {item.texels = 0;empties++;}
                        if (buffer == MTLPurgeableStateEmpty) {item.bytes = 0;empties++;}
                        CHECK(wrong(item.texture, side, item.texels) == 0, "worker %u round %u: texture differs from %#x (answer %lu)", worker, round, item.texels, (unsigned long)texture);
                        CHECK(wrongBuffer(item.buffer, item.bytes) == 0, "worker %u round %u: buffer differs from %#x (answer %lu)", worker, round, item.bytes, (unsigned long)buffer);
                        if (++next == 0) next = 1;
                        fill(item.texture, side, next);item.texels = next;
                        fillBuffer(item.buffer, next);item.bytes = next;
                        CHECK(wrong(item.texture, side, next) == 0 && wrongBuffer(item.buffer, next) == 0, "worker %u round %u: what was just written", worker, round);
                        [item.texture setPurgeableState:MTLPurgeableStateVolatile];
                        [item.buffer setPurgeableState:MTLPurgeableStateVolatile];
                        item.busy = false;
                        rounds++;
                    }
                }
            }
        });
    // The rogue: reads and views resources with no NonVolatile first. Whatever it reads, its commands complete and nothing crashes.
    threads.emplace_back([] {
        @autoreleasepool {
            for (unsigned round = 0; !stop; ++round) {
                @autoreleasepool {
                    Item &item = items[round % (workers * each)];
                    if (item.busy.exchange(true)) continue;
                    // Its uses may place a discarded resource again: what the owner then finds is zero, with its state Empty.
                    const bool emptyBefore = [item.texture setPurgeableState:MTLPurgeableStateKeepCurrent] == MTLPurgeableStateEmpty;
                    id<MTLTexture> view = [item.texture newTextureViewWithPixelFormat:MTLPixelFormatRGBA8Unorm];
                    CHECK(view != nil, "rogue view");
                    const unsigned bad = wrong(view, side, item.texels), zero = wrong(view, side, 0);
                    CHECK(bad == 0 || zero == 0, "rogue round %u: a texture reads neither its last contents nor zero (was Empty %d)", round, emptyBefore);
                    (void)wrongBuffer(item.buffer, item.bytes);
                    view = nil;
                    item.busy = false;
                    rogue++;
                }
            }
        }
    });
    sleep(6);
    stop = true;
    for (std::thread &thread : threads) thread.join();
    const Counts c = counts();
    std::printf("stress: %llu contract rounds, %llu Empty answers handled, %llu rogue rounds; discarded %llu, placed again %llu by request and %llu by use\n",
                (unsigned long long)rounds.load(), (unsigned long long)empties.load(), (unsigned long long)rogue.load(),
                (unsigned long long)c.discarded(), (unsigned long long)c.again(), (unsigned long long)c.byUse());
    CHECK(rounds > 50 && empties > 10 && rogue > 10 && c.discarded() > 10 && c.failed() == 0, "the stress ran: rounds, discards and no failed placement");
    for (Item &item : items) {item.texture = nil;item.buffer = nil;}
}

int main(int argc, char **argv) {
    if (argc != 4) return 2;
    const std::string mode = argv[3];
    @autoreleasepool {
        gDeviceUnderTest = native_bridge_metal_open(argv[1], argv[2], 0x6e6d62, nullptr);
        if (!gDeviceUnderTest) return 2;
        if (mode == "off") offCase();
        else if (mode == "timer") timerCase();
        else if (mode == "stress") stressCase();
        else {
            @autoreleasepool {textureCase();}
            @autoreleasepool {inFlightCase();}
            @autoreleasepool {viewCase();}
            @autoreleasepool {heapCase();}
            @autoreleasepool {bufferCase();}
            @autoreleasepool {fixedCase();}
            @autoreleasepool {ageAndUseCase();}
            @autoreleasepool {memoryCase();}
            // The telemetry's rows of it, each within 90 characters.
            static char text[16384];
            native_bridge_metal_telemetry(text, sizeof text);
            // PU0: the caller that read a discarded texture with no NonVolatile first (ageAndUseCase).
            for (const char *name : {"\nPG req ", "\nPGb out ", "\nPGc decl ", "\nPU0 all "}) {
                const char *row = strstr(text, name);
                CHECK(row != nullptr, "telemetry row%s", name + 1);
                if (row) std::printf("%.*s\n", (int)strcspn(row + 1, "\n"), row + 1);
            }
            const Counts c = counts();
            CHECK(c.failed() == 0, "no placement again failed");
            std::printf("discarded %llu (%lluK by pressure, %lluK by age), placed again %llu by request and %llu by use, %llu Empty answers\n",
                        (unsigned long long)c.discarded(), (unsigned long long)(c.w[1] >> 10), (unsigned long long)(c.w[3] >> 10),
                        (unsigned long long)c.again(), (unsigned long long)c.byUse(), (unsigned long long)c.emptyAnswers());
        }
        if (gFailed) {std::printf("FAIL: %u checks\n", gFailed.load());return 1;}
        if (mode == "off") std::puts("PASS: with NATIVE_METAL_PURGE=0 the purgeable state is kept and nothing is discarded");
        else if (mode == "timer") std::puts("PASS: the age timer discards an idle Volatile texture, which answers Empty and is usable again");
        else if (mode == "stress") std::puts("PASS: under constant discards, callers that follow the contract read only what they wrote or zero after Empty; uses without it complete");
        else std::puts("PASS: Volatile resources are discarded only while idle, answer Empty, and are usable again; views, heaps, buffers, in-flight use, age and memory");
    }
}
