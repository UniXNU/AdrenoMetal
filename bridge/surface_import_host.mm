// SPDX-License-Identifier: MIT
// Count real frontend no-copy imports/releases, forwarding every call to the
// unmodified bridge. Use actual GPU pixels and bounded pending-work lifetimes.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <IOSurface/IOSurfaceRef.h>
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <mutex>
#include <set>
#include <vector>
#include <unistd.h>
#include "../NativeBridgeMetalFrontend.h"
#include "NativeMetalObjects.h"

static std::atomic<unsigned> imports, releases;
static std::mutex handlesLock;
static std::set<NmbBuffer *> handles;
static uint64_t (*vkImports)();
extern "C" NmbBuffer *tracked_surface_import(NmbDevice *d, void *p, uint64_t n, uint64_t options) {
    NmbBuffer *buffer = nmb_buffer_new_no_copy(d,p,n,options);
    if (buffer) {std::lock_guard<std::mutex> lock(handlesLock);handles.insert(buffer);imports++;}
    return buffer;
}
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
static bool drained(unsigned expected) {
    for(unsigned i=0;i<300 && releases.load()!=expected;++i)usleep(1000);
    return releases.load()==expected;
}
static IOSurfaceRef makeSurface(unsigned byte) {
    IOSurfaceRef surface=IOSurfaceCreate((__bridge CFDictionaryRef)@{
        (id)kIOSurfaceWidth:@64,(id)kIOSurfaceHeight:@16,(id)kIOSurfaceBytesPerElement:@4,
        (id)kIOSurfaceBytesPerRow:@256,(id)kIOSurfacePixelFormat:@((uint32_t)0x52474241)});
    if(!surface || IOSurfaceLock(surface,0,nullptr))return nullptr;
    memset(IOSurfaceGetBaseAddress(surface),byte,IOSurfaceGetAllocSize(surface));
    if(IOSurfaceUnlock(surface,0,nullptr)){CFRelease(surface);return nullptr;}
    return surface;
}
static MTLTextureDescriptor *descriptor() {
    auto d=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:64 height:16 mipmapped:NO];
    d.storageMode=MTLStorageModeShared;
    d.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite|MTLTextureUsagePixelFormatView;
    return d;
}

static bool aliases(id<MTLDevice> device,unsigned count,unsigned allocations,bool parallel) {
    const unsigned before=imports.load(),released=releases.load();bool ok=true;
    const uint64_t vkBefore=vkImports?vkImports():0;
    @autoreleasepool {
        std::vector<IOSurfaceRef> surfaces;
        for(unsigned i=0;i<allocations;++i){auto s=makeSurface(i?0xa7:0xcc);if(!s)return false;surfaces.push_back(s);}
        auto textures=[NSMutableArray array];auto d=descriptor();
        if(parallel) {
            std::atomic<unsigned> failures{0};auto *failed=&failures;
            const IOSurfaceRef surface=surfaces[0];
            dispatch_apply(count,dispatch_get_global_queue(QOS_CLASS_DEFAULT,0),^(size_t i){
                (void)i;
                @autoreleasepool {
                    id<MTLTexture> texture=[device newTextureWithDescriptor:d iosurface:surface plane:0];
                    if(!texture){(*failed)++;return;}
                    @synchronized(textures){[textures addObject:texture];}
                }
            });
            ok=failures.load()==0;
        } else for(unsigned i=0;i<count;++i) {
            id<MTLTexture> texture=[device newTextureWithDescriptor:d iosurface:surfaces[i%allocations] plane:0];
            if(i&1)texture=[[texture newTextureViewWithPixelFormat:MTLPixelFormatRGBA8Unorm] newTextureViewWithPixelFormat:MTLPixelFormatRGBA8Unorm];
            if(!texture)return false;[textures addObject:texture];
        }
        if(textures.count!=count)return false;
        std::vector<unsigned char> data(4096,0x5a);
        id<MTLBuffer> input=[device newBufferWithBytes:data.data() length:data.size() options:MTLResourceStorageModeShared];
        id<MTLBuffer> output=[device newBufferWithLength:count*4096 options:MTLResourceStorageModeShared];
        id<MTLCommandBuffer> command=[[device newCommandQueue] commandBuffer];id<MTLBlitCommandEncoder> copy=[command blitCommandEncoder];
        if(!input || !output || !copy)return false;
        [copy copyFromBuffer:input sourceOffset:0 sourceBytesPerRow:256 sourceBytesPerImage:4096 sourceSize:MTLSizeMake(64,16,1)
                  toTexture:textures[0] destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0,0,0)];
        for(unsigned i=0;i<count;++i)
            [copy copyFromTexture:textures[i] sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(64,16,1)
                        toBuffer:output destinationOffset:i*4096 destinationBytesPerRow:256 destinationBytesPerImage:4096];
        [copy endEncoding];[command commit];[command waitUntilCompleted];
        ok &= command.status==MTLCommandBufferStatusCompleted;
        bool mismatch=false;
        for(unsigned i=0;i<count;++i)for(unsigned j=0;j<4096;++j)
            if(((const unsigned char *)output.contents)[i*4096+j]!=(i%allocations?0xa7:0x5a)) {
                if(!mismatch)std::printf("first mismatch alias=%u byte=%u actual=%02x expected=%02x\n",i,j,
                    ((const unsigned char *)output.contents)[i*4096+j],i%allocations?0xa7:0x5a);
                mismatch=true;ok=false;
            }
        for(IOSurfaceRef surface:surfaces)CFRelease(surface);
    }
    // The surfaces are gone: their held imports go at the next trim (a completion, an import or memory pressure).
    native_bridge_metal_trim_surface_imports();
    const unsigned made=imports.load()-before;
    const bool retired=drained(released+made);
    // Every live surface is imported once, however many textures alias it.
    const bool reuse=made==allocations;
    const uint64_t actualVk=vkImports?vkImports()-vkBefore:0;
    std::printf("aliases=%u surfaces=%u parallel=%d imports=%u releases=%u vk_imports=%llu gpu=%s retired=%s reuse=%s\n",
        count,allocations,parallel,made,releases.load()-released,(unsigned long long)actualVk,ok?"pass":"FAIL",retired?"pass":"FAIL",reuse?"pass":"FAIL");
    return ok && retired && reuse && (!vkImports || actualVk==allocations);
}

// As Metal keeps a surface's GPU mapping while the surface lives: textures made and dropped over a surface the caller still holds share
// one import, kept; once the caller lets go of the surface, a trim releases it.
static bool expires(id<MTLDevice> device) {
    const unsigned before=imports.load(),released=releases.load();IOSurfaceRef surface=makeSurface(0x35);if(!surface)return false;
    bool ok=true;
    for(unsigned i=0;i<2;++i) {
        @autoreleasepool {id<MTLTexture> texture=[device newTextureWithDescriptor:descriptor() iosurface:surface plane:0];if(!texture)ok=false;}
        native_bridge_metal_trim_surface_imports();
        usleep(20000);
        ok &= imports.load()==before+1 && releases.load()==released;
    }
    CFRelease(surface);
    native_bridge_metal_trim_surface_imports();
    ok &= drained(released+1);
    std::printf("held owners imports=%u releases=%u result=%s\n",imports.load()-before,releases.load()-released,ok?"pass":"FAIL");
    return ok;
}

static bool pending(id<MTLDevice> device) {
    const unsigned before=imports.load(),released=releases.load();bool ok=true;
    @autoreleasepool {
        IOSurfaceRef surface=makeSurface(0x79);if(!surface)return false;
        id<MTLSharedEvent> event=[device newSharedEvent];id<MTLCommandBuffer> command=[[device newCommandQueue] commandBuffer];
        id<MTLBuffer> output=[device newBufferWithLength:4096 options:MTLResourceStorageModeShared];
        if(!event || !command || !output)return false;
        @autoreleasepool {
            id<MTLTexture> texture=[device newTextureWithDescriptor:descriptor() iosurface:surface plane:0];if(!texture)return false;
            [command encodeWaitForEvent:event value:1];id<MTLBlitCommandEncoder> copy=[command blitCommandEncoder];
            [copy copyFromTexture:texture sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(64,16,1)
                        toBuffer:output destinationOffset:0 destinationBytesPerRow:256 destinationBytesPerImage:4096];
            [copy endEncoding];[command commit];
        }
        auto finished=dispatch_semaphore_create(0);
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT,0),^{@autoreleasepool{[command waitUntilCompleted];}dispatch_semaphore_signal(finished);});
        if(!dispatch_semaphore_wait(finished,dispatch_time(DISPATCH_TIME_NOW,100*NSEC_PER_MSEC)))ok=false;
        @autoreleasepool {
            id<MTLTexture> alias=[device newTextureWithDescriptor:descriptor() iosurface:surface plane:0];
            if(!alias || imports.load()!=before+1 || releases.load()!=released)ok=false;
            // Drop the caller's last surface reference before the GPU reads it.
            CFRelease(surface);surface=nullptr;
        }
        event.signaledValue=1;
        if(dispatch_semaphore_wait(finished,dispatch_time(DISPATCH_TIME_NOW,3*NSEC_PER_SEC)))return false;
        if(command.status!=MTLCommandBufferStatusCompleted)ok=false;
        for(unsigned i=0;i<4096;++i)if(((const unsigned char *)output.contents)[i]!=0x79)ok=false;
        // The completion lets go of the held import: the caller dropped the surface before the GPU read it.
        ok &= drained(released+1);
    }
    std::printf("pending owners imports=%u releases=%u result=%s\n",imports.load()-before,releases.load()-released,ok?"pass":"FAIL");
    return ok;
}

int main(int argc,char **argv) {
    if(argc!=3 && !(argc==4 && !strcmp(argv[3],"parallel")))return 2;
    @autoreleasepool {
        id<MTLDevice> device=native_bridge_metal_open(argv[1],argv[2],0x6e6d62,nullptr);if(!device)return 2;
        void *loader=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);if(!loader)return 2;
        vkImports=reinterpret_cast<uint64_t (*)()>(dlsym(loader,"nmb_test_vk_host_import_count"));
        if(getenv("NMB_TEST_REAL_VULKAN_LOADER") && !vkImports)return 2;
        if(argc==4)return aliases(device,64,1,true)?0:1;
        bool ok=aliases(device,1,1,false);
        ok &= aliases(device,128,1,false);
        ok &= aliases(device,128,2,false);
        ok &= aliases(device,64,1,true);
        ok &= expires(device);
        ok &= pending(device);
        ok &= aliases(device,514,257,false);
        if(!ok || imports.load()!=releases.load()){std::puts("FAIL: live IOSurface import reuse");return 1;}
        std::puts("PASS: live IOSurface aliases reuse one held import with real GPU bytes; held while the surface lives, released after it, pending work keeps it");
    }
}
