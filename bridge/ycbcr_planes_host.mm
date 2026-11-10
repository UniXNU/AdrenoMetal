// SPDX-License-Identifier: MIT
// A two-plane YCbCr surface texture (a video frame, MTLPixelFormat 500 over a '420v' IOSurface) that is only sampled is made over
// the surface's planes alone: no converted image is made for it. Real GPU pixels of the planes path are held against the converted
// path's; a binding that cannot read the planes in place and a call other than a slot binding still get the image; a surface whose
// texture and owner went stays imported while a command buffer in flight samples it. Mode `converted` runs with
// NATIVE_METAL_YCBCR_PLANES=0 (the boot argument gpu-ycbcr-planes=0): every texture is made with its image, as before.
// Built as surface_import_host is: the frontend's surface imports and buffer releases are counted through the tracked_ wrappers.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <IOSurface/IOSurfaceRef.h>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <set>
#include <string>
#include <unistd.h>
#include <vector>
#include "../NativeBridgeMetalFrontend.h"
#include "NativeMetalObjects.h"

static std::atomic<unsigned> imports, releases;
static std::mutex handlesLock;
static std::set<NmbBuffer *> handles;
extern "C" NmbBuffer *tracked_surface_import_releasing(NmbDevice *d, void *p, uint64_t n, uint64_t options,
                                                      void (*release)(void *, void *, uint64_t), void *context) {
    NmbBuffer *buffer = nmb_buffer_new_no_copy_releasing(d,p,n,options,release,context);
    if (buffer) {std::lock_guard<std::mutex> lock(handlesLock);handles.insert(buffer);imports++;}
    return buffer;
}
extern "C" void tracked_buffer_release(NmbBuffer *buffer) {
    {std::lock_guard<std::mutex> lock(handlesLock);if(handles.erase(buffer))releases++;}
    nmb_buffer_release(buffer);
}

static const unsigned W=64,H=32;
// A '420v' surface: plane 0 the luma bytes, plane 1 the Cb,Cr pairs of each 2 x 2 block, rows 64 bytes apart, filled from `seed`.
static IOSurfaceRef makeSurface(unsigned seed) {
    NSDictionary *luma=@{(id)kIOSurfacePlaneWidth:@(W),(id)kIOSurfacePlaneHeight:@(H),(id)kIOSurfacePlaneBytesPerRow:@64,
        (id)kIOSurfacePlaneOffset:@0,(id)kIOSurfacePlaneSize:@(64*H),(id)kIOSurfacePlaneBytesPerElement:@1};
    NSDictionary *chroma=@{(id)kIOSurfacePlaneWidth:@(W/2),(id)kIOSurfacePlaneHeight:@(H/2),(id)kIOSurfacePlaneBytesPerRow:@64,
        (id)kIOSurfacePlaneOffset:@(64*H),(id)kIOSurfacePlaneSize:@(64*H/2),(id)kIOSurfacePlaneBytesPerElement:@2};
    IOSurfaceRef surface=IOSurfaceCreate((__bridge CFDictionaryRef)@{(id)kIOSurfaceWidth:@(W),(id)kIOSurfaceHeight:@(H),
        (id)kIOSurfacePixelFormat:@((uint32_t)0x34323076),(id)kIOSurfaceAllocSize:@(64*H*3/2),(id)kIOSurfacePlaneInfo:@[luma,chroma]});
    if(!surface || IOSurfaceLock(surface,0,nullptr))return nullptr;
    uint8_t *bytes=(uint8_t *)IOSurfaceGetBaseAddress(surface);uint32_t state=seed*2654435761u+1;
    for(unsigned i=0;i<64*H*3/2;++i){state=state*1664525u+1013904223u;bytes[i]=(uint8_t)(16+(state>>24)%220);}
    IOSurfaceUnlock(surface,0,nullptr);
    return surface;
}
static id<MTLTexture> frame(id<MTLDevice> device,IOSurfaceRef surface,MTLTextureUsage usage) {
    auto d=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:(MTLPixelFormat)500 width:W height:H mipmapped:NO];
    d.usage=usage;
    return [device newTextureWithDescriptor:d iosurface:surface plane:0];
}
// planes, conv, later, whole, images alive, their MiB: the telemetry's YC row (absent before the first such texture: zeros).
struct Counts {unsigned long long planes,conv,later,whole,images,mib;};
static Counts counts() {
    char text[8192]={};native_bridge_metal_telemetry(text,sizeof text);Counts c={};
    const char *row=strstr(text,"YC planes ");
    if(row)sscanf(row,"YC planes %llu conv %llu later %llu whole %llu | img %llu %lluM",&c.planes,&c.conv,&c.later,&c.whole,&c.images,&c.mib);
    return c;
}
struct Rig {
    id<MTLDevice> device;id<MTLCommandQueue> queue;id<MTLRenderPipelineState> sampled,fetched;id<MTLSamplerState> nearest;
    // Draws `texture` over a W x H half-float target with `pipeline` into `commands`, then copies the target into `out`.
    void draw(id<MTLCommandBuffer> commands,id<MTLRenderPipelineState> pipeline,id<MTLTexture> texture,id<MTLBuffer> out) {
        auto td=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA16Float width:W height:H mipmapped:NO];
        td.usage=MTLTextureUsageRenderTarget;td.storageMode=MTLStorageModePrivate;
        id<MTLTexture> target=[device newTextureWithDescriptor:td];
        auto pass=[MTLRenderPassDescriptor renderPassDescriptor];
        pass.colorAttachments[0].texture=target;pass.colorAttachments[0].loadAction=MTLLoadActionClear;
        pass.colorAttachments[0].storeAction=MTLStoreActionStore;
        id<MTLRenderCommandEncoder> e=[commands renderCommandEncoderWithDescriptor:pass];
        [e setRenderPipelineState:pipeline];[e setFragmentTexture:texture atIndex:0];[e setFragmentSamplerState:nearest atIndex:0];
        [e drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[e endEncoding];
        id<MTLBlitCommandEncoder> blit=[commands blitCommandEncoder];
        [blit copyFromTexture:target sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(W,H,1)
                     toBuffer:out destinationOffset:0 destinationBytesPerRow:W*8 destinationBytesPerImage:W*H*8];
        [blit endEncoding];
    }
    // The halves a draw of `texture` with `pipeline` leaves; empty where the command buffer did not complete.
    std::vector<uint16_t> pixels(id<MTLRenderPipelineState> pipeline,id<MTLTexture> texture) {
        id<MTLBuffer> out=[device newBufferWithLength:W*H*8 options:MTLResourceStorageModeShared];
        id<MTLCommandBuffer> commands=[queue commandBuffer];
        draw(commands,pipeline,texture,out);[commands commit];[commands waitUntilCompleted];
        if(commands.status!=MTLCommandBufferStatusCompleted){std::printf("command buffer: %s\n",commands.error.description.UTF8String);return {};}
        const uint16_t *halves=(const uint16_t *)out.contents;
        return std::vector<uint16_t>(halves,halves+W*H*4);
    }
};
static float half(uint16_t h) {
    const int e=(h>>10)&31,m=h&1023;const float v=e==0?std::ldexp((float)m,-24):std::ldexp((float)(m+1024),e-25);
    return h&0x8000?-v:v;
}
// The largest difference between two pictures' channels; 9 where either is missing.
static float apart(const std::vector<uint16_t> &a,const std::vector<uint16_t> &b) {
    if(a.empty() || a.size()!=b.size())return 9;
    float most=0;for(size_t i=0;i<a.size();++i)most=std::max(most,std::fabs(half(a[i])-half(b[i])));
    return most;
}
static bool drained(unsigned expected) {
    for(unsigned i=0;i<500 && releases.load()!=expected;++i){native_bridge_metal_trim_surface_imports();usleep(2000);}
    return releases.load()==expected;
}

int main(int argc,char **argv) {
    if(argc!=4)return 2;
    const bool planes=!strcmp(argv[3],"planes");
    if(!planes)setenv("NATIVE_METAL_YCBCR_PLANES","0",1);
    @autoreleasepool {
        Rig rig;rig.device=native_bridge_metal_open(argv[1],argv[2],0x6e6d62,nullptr);if(!rig.device)return 2;
        rig.queue=[rig.device newCommandQueue];
        NSError *error=nil;
        auto library=[rig.device newLibraryWithSource:@"#include <metal_stdlib>\nusing namespace metal;\n"
            "struct V {float4 p [[position]]; float2 uv;};\n"
            "vertex V full(uint id [[vertex_id]]) {float2 p[3]={float2(-1,-1),float2(3,-1),float2(-1,3)};"
            " V v; v.p=float4(p[id],0,1); v.uv=float2(p[id].x*0.5+0.5,0.5-p[id].y*0.5); return v;}\n"
            "fragment float4 sampled(V v [[stage_in]], texture2d<float> t [[texture(0)]], sampler s [[sampler(0)]]) {return t.sample(s,v.uv);}\n"
            "fragment float4 fetched(V v [[stage_in]], texture2d<float> t [[texture(0)]]) {return t.read(uint2(v.p.xy));}\n" options:nil error:&error];
        if(!library){std::printf("library: %s\n",error.description.UTF8String);return 2;}
        auto pd=[MTLRenderPipelineDescriptor new];pd.vertexFunction=[library newFunctionWithName:@"full"];
        pd.colorAttachments[0].pixelFormat=MTLPixelFormatRGBA16Float;
        pd.fragmentFunction=[library newFunctionWithName:@"sampled"];rig.sampled=[rig.device newRenderPipelineStateWithDescriptor:pd error:&error];
        pd.fragmentFunction=[library newFunctionWithName:@"fetched"];rig.fetched=[rig.device newRenderPipelineStateWithDescriptor:pd error:&error];
        rig.nearest=[rig.device newSamplerStateWithDescriptor:[MTLSamplerDescriptor new]];
        if(!rig.sampled || !rig.fetched || !rig.nearest){std::printf("pipeline: %s\n",error.description.UTF8String);return 2;}
        unsigned failures=0;
        auto check=[&](bool ok,const char *what){if(!ok){++failures;std::printf("FAILED %s\n",what);}};
        IOSurfaceRef surface=makeSurface(1);if(!surface)return 2;

        // 1. A frame that is only sampled: over its planes alone in `planes` mode, with its image otherwise.
        id<MTLTexture> light=frame(rig.device,surface,MTLTextureUsageShaderRead);if(!light)return 2;
        const auto sampled=rig.pixels(rig.sampled,light);
        Counts c=counts();
        std::printf("sampled: planes %llu conv %llu later %llu whole %llu images %llu\n",c.planes,c.conv,c.later,c.whole,c.images);
        check(planes?(c.planes==1 && c.conv==0 && c.later==0 && c.images==0):(c.planes==0 && c.conv==1 && c.images==1),"the sampled frame's path");

        // 2. The converted path's pixels: a frame whose usage needs its image, read texel by texel from that image.
        id<MTLTexture> whole=frame(rig.device,surface,MTLTextureUsageShaderRead|MTLTextureUsagePixelFormatView);if(!whole)return 2;
        const auto converted=rig.pixels(rig.fetched,whole);
        c=counts();
        check(c.planes==(planes?1:0) && c.conv==(planes?1:2) && c.images==(planes?1:2),"a frame of another usage keeps its image");
        const float difference=apart(sampled,converted);
        std::printf("planes path against converted path: largest difference %.6f\n",difference);
        check(difference<=1.0f/1024,"the sampled frame's pixels are the converted image's");
        bool flat=true;for(size_t i=4;i<converted.size();i+=4)flat&=converted[i]==converted[0] && converted[i+1]==converted[1];
        check(!converted.empty() && !flat,"the picture is not flat");

        // 3. A binding that cannot read the planes in place makes the image then, and reads the same texels.
        const auto later=rig.pixels(rig.fetched,light);
        c=counts();
        std::printf("fetched: later %llu images %llu (%lluM)\n",c.later,c.images,c.mib);
        check(later==converted,"a fetch of the sampled frame reads the converted texels");
        check(planes?(c.later==1 && c.images==2 && c.whole==0):(c.later==0 && c.images==2),"the image is made at the binding that needs it");
        // Sampled again after the image exists: still the same picture.
        check(apart(rig.pixels(rig.sampled,light),converted)<=1.0f/1024,"sampled again");

        // 4. A call other than a slot binding makes the texture whole: a copy of its luma rows into a buffer.
        {
            IOSurfaceRef other=makeSurface(2);id<MTLTexture> copied=frame(rig.device,other,MTLTextureUsageShaderRead);
            id<MTLBuffer> out=[rig.device newBufferWithLength:W*H options:MTLResourceStorageModeShared];
            id<MTLCommandBuffer> commands=[rig.queue commandBuffer];id<MTLBlitCommandEncoder> blit=[commands blitCommandEncoder];
            [blit copyFromTexture:copied sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(W,H,1)
                         toBuffer:out destinationOffset:0 destinationBytesPerRow:W destinationBytesPerImage:W*H];
            [blit endEncoding];[commands commit];[commands waitUntilCompleted];
            check(commands.status==MTLCommandBufferStatusCompleted,"a copy out of the frame completes");
            check(!memcmp(out.contents,IOSurfaceGetBaseAddress(other),W*H),"the copy moved the luma rows");
            const Counts after=counts();
            check(planes?(after.planes==c.planes+1 && after.whole==1):(after.whole==0 && after.conv==c.conv+1),"the copy made the frame whole");
            check(!rig.pixels(rig.sampled,copied).empty(),"the whole frame still draws");
            CFRelease(other);
        }

        // 5. In flight: the texture and every owner's reference to its surface go while the command buffer that samples it waits.
        {
            // The earlier frames' imports whose surfaces went are let go first.
            for(unsigned i=0;i<20;++i){native_bridge_metal_trim_surface_imports();usleep(5000);}
            const unsigned importsBefore=imports.load(),releasesBefore=releases.load();
            id<MTLSharedEvent> event=[rig.device newSharedEvent];id<MTLCommandBuffer> commands=[rig.queue commandBuffer];
            id<MTLBuffer> out=[rig.device newBufferWithLength:W*H*8 options:MTLResourceStorageModeShared];
            @autoreleasepool {
                IOSurfaceRef held=makeSurface(1);id<MTLTexture> texture=frame(rig.device,held,MTLTextureUsageShaderRead);
                [commands encodeWaitForEvent:event value:1];
                rig.draw(commands,rig.sampled,texture,out);
                texture=nil;CFRelease(held);
            }
            [commands commit];
            native_bridge_metal_trim_surface_imports();usleep(100000);native_bridge_metal_trim_surface_imports();
            check(imports.load()==importsBefore+1 && releases.load()==releasesBefore,"the surface stays imported while its frame is in flight");
            event.signaledValue=1;[commands waitUntilCompleted];
            check(commands.status==MTLCommandBufferStatusCompleted,"the gated draw completes");
            const uint16_t *halves=(const uint16_t *)out.contents;
            check(apart(std::vector<uint16_t>(halves,halves+W*H*4),converted)<=1.0f/1024,"the frame in flight drew its picture");
            commands=nil;
            check(drained(releasesBefore+1),"the import goes once the command buffer completed");
        }
        light=nil;whole=nil;CFRelease(surface);
        c=counts();
        std::printf("end: planes %llu conv %llu later %llu whole %llu images %llu\n",c.planes,c.conv,c.later,c.whole,c.images);
        if(failures){std::printf("FAIL: two-plane YCbCr surface textures, %s mode (%u checks)\n",argv[3],failures);return 1;}
        std::printf("PASS: two-plane YCbCr surface textures, %s mode: %s\n",argv[3],planes?
            "sampled over the planes alone with the converted path's pixels, image made only where needed, surface kept in flight":
            "every frame made with its converted image, same pixels");
    }
}
