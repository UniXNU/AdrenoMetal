// SPDX-License-Identifier: MIT
// Real GPU pixels: only a successful large IOSurface draw arms the freeze.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <IOSurface/IOSurfaceRef.h>
#include "../NativeBridgeMetalFrontend.h"
#include "NativeMetalObjects.h"
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <thread>
#include <vector>
#include <unistd.h>

#define CHECK(x) do {if(!(x)){fprintf(stderr,"FAIL %d: %s\n",__LINE__,#x);return 1;}}while(0)
static IOSurfaceRef surface(unsigned width,unsigned height) {
 return IOSurfaceCreate((__bridge CFDictionaryRef)@{
  (id)kIOSurfaceWidth:@(width),(id)kIOSurfaceHeight:@(height),
  (id)kIOSurfaceBytesPerElement:@4,(id)kIOSurfaceBytesPerRow:@(width*4),
  (id)kIOSurfacePixelFormat:@((uint32_t)0x52474241)});
}
static bool finish(id<MTLCommandBuffer> cb) {
 [cb commit];[cb waitUntilCompleted];return cb.status==MTLCommandBufferStatusCompleted;
}
int main(int argc,char **argv) {@autoreleasepool {
 CHECK(argc==4);const bool freeze=!strcmp(argv[3],"freeze");
 if(!strcmp(argv[3],"absent"))unsetenv("NATIVE_METAL_FREEZE_WALLPAPER");
 else setenv("NATIVE_METAL_FREEZE_WALLPAPER",strcmp(argv[3],"disabled")?"1":"0",1);
 id<MTLDevice> dev=native_bridge_metal_open(argv[1],argv[2],0,nullptr);CHECK(dev);
 id<MTLCommandQueue> queue=[dev newCommandQueueWithMaxCommandBufferCount:8];CHECK(queue);
 // A failed command does not count as a completed frame.
 auto refused=[queue commandBuffer];[refused blitCommandEncoder];[refused commit];[refused waitUntilCompleted];
 CHECK(refused.status==MTLCommandBufferStatusError);
 auto normal=[queue commandBuffer];CHECK(finish(normal));
 size_t bytes=0;const void *air=nmb_fixture_air(&bytes);
 id<MTLLibrary> lib=native_bridge_metal_library_with_air(air,bytes);CHECK(lib);
 auto pd=[MTLRenderPipelineDescriptor new];pd.vertexFunction=[lib newFunctionWithName:@"colored_vertex"];
 pd.fragmentFunction=[lib newFunctionWithName:@"colored_fragment"];pd.colorAttachments[0].pixelFormat=MTLPixelFormatRGBA8Unorm;
 NSError *error=nil;auto pipeline=[dev newRenderPipelineStateWithDescriptor:pd error:&error];CHECK(pipeline);
 const float points[]={-1,-1,3,-1,-1,3},green[]={0,1,0,1,0,1,0,1,0,1,0,1},red[]={1,0,0,1,1,0,0,1,1,0,0,1};
 auto vertices=[dev newBufferWithBytes:points length:sizeof points options:MTLResourceStorageModeShared];
 auto greenBuffer=[dev newBufferWithBytes:green length:sizeof green options:MTLResourceStorageModeShared];
 auto redBuffer=[dev newBufferWithBytes:red length:sizeof red options:MTLResourceStorageModeShared];
 auto draw=^(id<MTLTexture> target,id<MTLBuffer> colors) {
  id<MTLCommandBuffer> cb=[queue commandBuffer];
  auto pass=[MTLRenderPassDescriptor renderPassDescriptor];pass.colorAttachments[0].texture=target;
  pass.colorAttachments[0].loadAction=MTLLoadActionClear;pass.colorAttachments[0].storeAction=MTLStoreActionStore;
  auto encoder=[cb renderCommandEncoderWithDescriptor:pass];
  [encoder setRenderPipelineState:pipeline];[encoder setVertexBuffer:vertices offset:0 atIndex:0];
  [encoder setVertexBuffer:colors offset:0 atIndex:1];[encoder drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
  [encoder endEncoding];return cb;
 };
 auto desc=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:2048 height:1024 mipmapped:NO];
 desc.storageMode=MTLStorageModeShared;desc.usage=MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead;
 // A large offscreen render and a small shared surface are not the wallpaper output.
 auto privateTexture=[dev newTextureWithDescriptor:desc];CHECK(privateTexture);
 CHECK(finish(draw(privateTexture,greenBuffer)));
 IOSurfaceRef small=surface(8,8);CHECK(small);
 MTLTextureDescriptor *smallDesc=[desc copy];smallDesc.width=8;smallDesc.height=8;
 auto smallTexture=[dev newTextureWithDescriptor:smallDesc iosurface:small plane:0];CHECK(smallTexture);
 CHECK(finish(draw(smallTexture,greenBuffer)));CFRelease(small);
 IOSurfaceRef output=surface(2048,1024);CHECK(output);
 auto target=[dev newTextureWithDescriptor:desc iosurface:output plane:0];CHECK(target);
 auto first=draw(target,greenBuffer);std::atomic<unsigned> scheduled{0},completed{0};
 auto *s=&scheduled,*c=&completed;
 [first addScheduledHandler:^(id<MTLCommandBuffer>){++*s;}];
 [first addCompletedHandler:^(id<MTLCommandBuffer>){++*c;}];
 CHECK(finish(first) && scheduled==1 && completed==1);
 const uint32_t *pixels=(const uint32_t *)IOSurfaceGetBaseAddress(output);
 CHECK(pixels[0]==0xff00ff00u && pixels[2048*512+1024]==0xff00ff00u);
 if(freeze) {
  // Concurrent attempts stay committed. They neither paint red nor invent callbacks.
  auto pending=[NSMutableArray array];
  for(unsigned i=0;i<4;++i) {
   auto cb=draw(target,redBuffer);
   [cb addScheduledHandler:^(id<MTLCommandBuffer>){++*s;}];
   [cb addCompletedHandler:^(id<MTLCommandBuffer>){++*c;}];[pending addObject:cb];
  }
  std::vector<std::thread> threads;
  for(id<MTLCommandBuffer> cb in pending)threads.emplace_back([cb]{@autoreleasepool{[cb commit];}});
  for(auto &thread:threads)thread.join();
  usleep(100000);
  for(id<MTLCommandBuffer> cb in pending)CHECK(cb.status==MTLCommandBufferStatusCommitted && !cb.error);
  CHECK(scheduled==1 && completed==1 && pixels[0]==0xff00ff00u && pixels[2048*512+1024]==0xff00ff00u);
 } else {
  CHECK(finish(draw(target,redBuffer)));CHECK(pixels[0]==0xff0000ffu);
 }
 CFRelease(output);
 std::printf("PASS: %s first-frame pixels, genuine callbacks, pending submissions and process scope\n",argv[3]);
 return 0;
}}
