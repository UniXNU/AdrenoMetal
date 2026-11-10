// SPDX-License-Identifier: MIT
// Interleave encoding on two queues, then reverse commit order. Global deltas assign
// the first buffer's render work to the second buffer's blits; per-buffer counts must not.
#include "FrontendChecks.h"
#include "NativeMetalObjects.h"
#include "../NativeBridgeMetalFrontend.h"
#include <map>
#include <string>

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    @autoreleasepool {
        id<MTLDevice> device = native_bridge_metal_open(argv[1], argv[2], 0x6e6d62, nullptr);
        if (!device) return 2;
        size_t length = 0;
        const void *air = nmb_fixture_air(&length);
        id<MTLLibrary> library = native_bridge_metal_library_with_air(air, length);
        frontendchecks::Report report = {};
        frontendchecks::gReport = &report;
        id<MTLRenderPipelineState> pipeline = frontendchecks::pipeline(device, library, @"colored_fragment", MTLPixelFormatRGBA8Unorm);
        auto td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:16 height:16 mipmapped:NO];
        td.usage = MTLTextureUsageRenderTarget;
        id<MTLTexture> texture = [device newTextureWithDescriptor:td];
        id<MTLBuffer> buffer = [device newBufferWithLength:256 options:MTLResourceStorageModeShared];
        id<MTLCommandBuffer> render = [[device newCommandQueue] commandBuffer];
        id<MTLCommandBuffer> copy = [[device newCommandQueue] commandBuffer];
        if (!library || !pipeline || !texture || !buffer || !render || !copy) return 2;
        frontendchecks::drawQuad(render, pipeline, texture, nil, frontendchecks::kQuarter, 6);
        for (unsigned i = 0; i < 3; ++i) {
            id<MTLBlitCommandEncoder> blit = [copy blitCommandEncoder];
            [blit fillBuffer:buffer range:NSMakeRange(0, 256) value:0x5a];
            [blit endEncoding];
        }
        frontendchecks::drawQuad(render, pipeline, texture, nil, frontendchecks::kQuarter, 6);
        [copy commit];[render commit];
        if (!frontendchecks::completed(copy, "blit") || !frontendchecks::completed(render, "render")) return 1;
        char text[8192] = {};
        native_bridge_metal_telemetry(text, sizeof text);
        // The table's memory, compile and thread rows are there, within the first 48 rows (what the kernel keeps), each at most 90 characters.
        {
            const std::string table = text;
            unsigned row = 0;
            std::map<std::string, unsigned> at;
            unsigned long long blocks = 0;
            for (size_t begin = 0; begin < table.size(); ++row) {
                size_t end = table.find('\n', begin);
                if (end == std::string::npos) end = table.size();
                if (end - begin > 90) {std::puts("FAIL a telemetry row longer than 90 characters");return 1;}
                const std::string line = table.substr(begin, end - begin);
                at.emplace(line.substr(0, line.find(' ')), row);
                // The blocks the size rows count: the pooled blocks in use and kept empty of the MK row.
                unsigned long long c[14] = {};
                if (sscanf(line.c_str(), "MK 16/32/40/48/56/64/+ pool %llu/%llu/%llu/%llu/%llu/%llu/%llu empty %llu/%llu/%llu/%llu/%llu/%llu/%llu",
                           &c[0], &c[1], &c[2], &c[3], &c[4], &c[5], &c[6], &c[7], &c[8], &c[9], &c[10], &c[11], &c[12], &c[13]) == 14)
                    for (unsigned long long n : c) blocks += n;
                if (!line.compare(0, 3, "MK ") || !line.compare(0, 3, "ML ") || !line.compare(0, 3, "VP ") || !line.compare(0, 3, "TH ")) std::printf("%s\n", line.c_str());
                begin = end + 1;
            }
            for (const char *name : {"MEM", "MK", "MKb", "MKc", "ML", "PG", "PE", "PS", "T", "VP", "V", "SC", "PR", "TH", "P0", "S"}) {
                const auto found = at.find(name);
                if (found == at.end() || found->second >= 48) {std::printf("FAIL telemetry row %s missing from the first 48 rows\n", name);return 1;}
            }
            if (!blocks) {std::puts("FAIL the size rows count no pooled block of a device that holds a texture and a buffer");return 1;}
        }
        std::map<unsigned, std::pair<unsigned, unsigned>> counts;
        for (char *line = strtok(text, "\n"); line; line = strtok(nullptr, "\n")) {
            unsigned index, sec, ms, passes, draws, kpx, dispatches, blits;
            if (sscanf(line, "K%u t=%u.%us rp%u dr%u kpx%u cs%u bl%u", &index, &sec, &ms, &passes, &draws, &kpx, &dispatches, &blits) != 8) continue;
            std::printf("%s\n", line);
            counts[index] = {passes, blits};
            if (draws != (index == 2 ? 2u : 0u)) {std::puts("FAIL cross-buffer draw attribution");return 1;}
        }
        if (counts.size() != 2 || counts[1] != std::make_pair(0u, 3u) || counts[2] != std::make_pair(2u, 0u)) {
            std::puts("FAIL interleaved render/blit attribution");return 1;
        }
        uint8_t pixels[16 * 16 * 4] = {};
        [texture getBytes:pixels bytesPerRow:64 fromRegion:MTLRegionMake2D(0, 0, 16, 16) mipmapLevel:0];
        if (frontendchecks::wrongTexels(pixels, 64, 16)) return 1;
        for (unsigned i = 0; i < 256; ++i) if (((const uint8_t *)buffer.contents)[i] != 0x5a) return 1;
        std::puts("PASS: opposite encode/commit orders retain per-buffer render/blit counts and correct host GPU pixels");
    }
}
