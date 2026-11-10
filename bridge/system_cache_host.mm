// SPDX-License-Identifier: MIT
// The system shader cache (SharedShaderCache.h, bridge patch 0036) between processes on the host.
// system_cache_host <loader> <own cache directory> fill
//     opens the device in that directory as the process that fills the layer (entries readable by everyone), makes a render and a
//     compute pipeline and a library from source, draws, and leaves the entries there.
// system_cache_host <loader> <own cache directory> read <layer directory> <owner uid>
//     names the layer with that one trusted owner, opens the device in its own (empty) directory, makes the same pipelines and
//     library, checks the drawn pixels, and prints what its own cache missed and what the layer answered:
//     `RESULT layer=<0|1> accepted=<n> rejected=<n> refused=<n> translated=<functions translated here> replies=<n> pixels=ok`.
// The cases to run it for: a second process hitting what the first published (translated=0, accepted above 0), a layer of
// another owner or writable by its group (layer=0), a directory or an entry others may write and a link in a directory's place
// (refused above 0, those entries translated here), and entries with changed bytes (rejected above 0, left in place).
#include "FrontendChecks.h"
#include "NativeMetalObjects.h"
#include "../NativeBridgeMetalFrontend.h"
#include <string>

int main(int argc, char **argv) {
    if (argc < 4) return 2;
    const bool fill = !strcmp(argv[3], "fill");
    if (!fill && argc != 6) return 2;
    @autoreleasepool {
        bool layer = false;
        if (fill) native_bridge_metal_system_cache(nullptr, nullptr, 0, true);
        else {
            const uint32_t owner = (uint32_t)strtoul(argv[5], nullptr, 10);
            layer = native_bridge_metal_system_cache(argv[4], &owner, 1, false);
            if (!layer) std::printf("layer refused: %s\n", nmb_last_error());
        }
        id<MTLDevice> device = native_bridge_metal_open(argv[1], argv[2], 0x6e6d62, nullptr);
        if (!device) {std::printf("no device: %s\n", nmb_last_error()); return 2;}
        size_t length = 0;
        const void *air = nmb_fixture_air(&length);
        id<MTLLibrary> library = native_bridge_metal_library_with_air(air, length);
        frontendchecks::Report report = {};
        frontendchecks::gReport = &report;
        id<MTLRenderPipelineState> pipeline = frontendchecks::pipeline(device, library, @"colored_fragment", MTLPixelFormatRGBA8Unorm);
        NSError *error = nil;
        id<MTLComputePipelineState> compute = [device newComputePipelineStateWithFunction:[library newFunctionWithName:@"scale_and_offset"] error:&error];
        if (!library || !pipeline || !compute) {std::puts("FAIL a pipeline was refused"); return 1;}
        // A library from source: its compiler reply is kept beside the pipeline cache, and read from the layer by a reader.
        id<MTLLibrary> source = [device newLibraryWithSource:@"#include <metal_stdlib>\nusing namespace metal;\n"
            "kernel void system_cache_probe(device float *data [[buffer(0)]], uint id [[thread_position_in_grid]]) {data[id] = 7.0f * float(id);}\n"
            options:nil error:&error];
        if (!source) std::printf("source library not made on this host: %s\n", error.localizedDescription.UTF8String);
        auto td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:16 height:16 mipmapped:NO];
        td.usage = MTLTextureUsageRenderTarget;
        id<MTLTexture> texture = [device newTextureWithDescriptor:td];
        id<MTLCommandBuffer> render = [[device newCommandQueue] commandBuffer];
        frontendchecks::drawQuad(render, pipeline, texture, nil, frontendchecks::kQuarter, 6);
        [render commit];
        if (!frontendchecks::completed(render, "render")) return 1;
        uint8_t pixels[16 * 16 * 4] = {};
        [texture getBytes:pixels bytesPerRow:64 fromRegion:MTLRegionMake2D(0, 0, 16, 16) mipmapLevel:0];
        if (frontendchecks::wrongTexels(pixels, 64, 16)) {std::puts("FAIL wrong pixels"); return 1;}
        // The cache thread writes what the creations queued.
        usleep(1500000);
        NmbShaderCacheStats stats = {};
        native_bridge_metal_shader_cache_stats(&stats);
        uint64_t sy[4] = {};
        (void)nmb_system_cache_counts(sy);
        char text[8192] = {};
        native_bridge_metal_telemetry(text, sizeof text);
        unsigned long long translated = 0, found = 0, replies = 0;
        for (char *line = strtok(text, "\n"); line; line = strtok(nullptr, "\n")) {
            if (!strncmp(line, "SC ", 3) || !strncmp(line, "PRb ", 4) || !strncmp(line, "SY ", 3)) std::printf("%s\n", line);
            sscanf(line, "SC fn %llu/%llu", &found, &translated);
            if (const char *at = strstr(line, "replies ")) if (!strncmp(line, "SY ", 3)) sscanf(at, "replies %llu", &replies);
        }
        std::printf("RESULT layer=%d accepted=%llu rejected=%llu refused=%llu translated=%llu found=%llu misses=%llu replies=%llu pixels=ok\n",
                    layer, (unsigned long long)sy[0], (unsigned long long)sy[1], (unsigned long long)sy[2], translated, found,
                    (unsigned long long)stats.misses, replies);
    }
}
