// SPDX-License-Identifier: MIT
// Host instrument: the Mac's Core Image renders a filter chain through a CIContext over the native frontend's device (bridge over a host
// Vulkan driver, MoltenVK on the Mac), as iconservicesagent does on the target. Core Image builds its programs through Metal (stitched
// function graphs, run-time source, its kernel libraries); a program it cannot make ends in a null program. Prints PASS with the pixel it
// read, or FAIL with the frontend's last problem. NMB_SYSTEM_DEVICE=1 runs the Mac's own device as the baseline. The Mac's Core Image is not
// the target's: this names gaps, it qualifies nothing on iPadOS.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <CoreImage/CoreImage.h>
#include <cstdio>
#include <cstring>
#include "../NativeBridgeMetalFrontend.h"
extern "C" size_t native_bridge_metal_telemetry(char *, size_t);

static void onMissing(const char *cls, const char *sel) {std::fprintf(stderr, "missing selector: %s %s\n", cls, sel);}

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc < 3) return 2;
        id<MTLDevice> device = getenv("NMB_SYSTEM_DEVICE") ? MTLCreateSystemDefaultDevice() : native_bridge_metal_open(argv[1], argv[2], 0x6e6d63, onMissing);
        if (!device) {std::printf("FAIL device\n"); return 1;}
        // Core Image's DAG path on iPadOS 27 (CI::MetalDAGHelper::get_precompiled_kernel): a kernel of the Core Image kernel library by
        // name, though the library names none. argv[3]: such a library (this Mac's ci_stdlib.metallib).
        if (argc > 3) {
            NSError *error = nil;
            id<MTLLibrary> kernels = [device newLibraryWithURL:[NSURL fileURLWithPath:@(argv[3])] error:&error];
            id<MTLFunction> add = [kernels newFunctionWithName:@"_add" constantValues:[MTLFunctionConstantValues new] error:&error];
            std::printf("KERNELS library %s names %lu _add %s type %lu\n", kernels ? "yes" : "no", (unsigned long)kernels.functionNames.count,
                        add ? "yes" : "no", (unsigned long)add.functionType);
            if (!add) {std::printf("FAIL kernel by name\n"); return 1;}
            // iPadOS 27's Core Image links a kernel only when its name is among the library's externFunctionNames (its TYPE 5 entries;
            // Apple's Metal answers 228 for macOS 26.5.1's ci_stdlib).
            NSArray *externs = [(id)kernels respondsToSelector:@selector(externFunctionNames)] ? [(id)kernels performSelector:@selector(externFunctionNames)] : nil;
            std::printf("EXTERNS %lu\n", (unsigned long)[externs count]);
            if (![externs containsObject:@"_add"]) {std::printf("FAIL extern function names\n"); return 1;}
            // As Apple's Metal answers (macOS 26.5.1): no names, the kernel by name, function type 4.
            if (kernels.functionNames.count || add.functionType != 4) {std::printf("FAIL kernel library answers\n"); return 1;}
        }
        id<MTLCommandQueue> queue = [device newCommandQueue];
        CIContext *context = [CIContext contextWithMTLCommandQueue:queue options:@{kCIContextCacheIntermediates: @NO}];
        if (!context) {std::printf("FAIL CIContext\n"); return 1;}
        const NSUInteger side = 64;
        MTLTextureDescriptor *d = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm width:side height:side mipmapped:NO];
        d.usage = MTLTextureUsageRenderTarget | MTLTextureUsageShaderRead | MTLTextureUsageShaderWrite;
        d.storageMode = MTLStorageModeShared;
        id<MTLTexture> target = [device newTextureWithDescriptor:d];
        // A solid colour through colour controls and a blur: a chain of stitchable kernels, then a separable blur.
        CIImage *image = [[CIImage imageWithColor:[CIColor colorWithRed:0.8 green:0.2 blue:0.1]] imageByCroppingToRect:CGRectMake(0, 0, side, side)];
        CIFilter *controls = [CIFilter filterWithName:@"CIColorControls"];
        [controls setValue:image forKey:kCIInputImageKey];
        [controls setValue:@1.2 forKey:kCIInputSaturationKey];
        CIFilter *blur = [CIFilter filterWithName:@"CIGaussianBlur"];
        [blur setValue:controls.outputImage forKey:kCIInputImageKey];
        [blur setValue:@2 forKey:kCIInputRadiusKey];
        CIImage *out = [blur.outputImage imageByCroppingToRect:CGRectMake(0, 0, side, side)];
        id<MTLCommandBuffer> commands = [queue commandBuffer];
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        [context render:out toMTLTexture:target commandBuffer:commands bounds:CGRectMake(0, 0, side, side) colorSpace:space];
        CGColorSpaceRelease(space);
        [commands commit];
        [commands waitUntilCompleted];
        uint8_t pixel[4] = {};
        [target getBytes:pixel bytesPerRow:side * 4 fromRegion:MTLRegionMake2D(side / 2, side / 2, 1, 1) mipmapLevel:0];
        static char table[16384];
        if (!getenv("NMB_SYSTEM_DEVICE")) {
            native_bridge_metal_telemetry(table, sizeof table);
            for (char *line = std::strtok(table, "\n"); line; line = std::strtok(nullptr, "\n")) std::printf("TELEMETRY %s\n", line);
        }
        const bool red = pixel[2] > 150 && pixel[1] < 120 && pixel[0] < 120;
        std::printf("%s pixel bgra %u %u %u %u status %lu\n", red ? "PASS" : "FAIL", pixel[0], pixel[1], pixel[2], pixel[3], (unsigned long)commands.status);
        return red ? 0 : 1;
    }
}
