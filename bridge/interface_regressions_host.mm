// SPDX-License-Identifier: MIT
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include "../NativeBridgeMetalFrontend.h"
#include "NativeMetalObjects.h"
#include <cstdio>
#include <vector>
#include <atomic>
#include <cstring>
#include <unistd.h>
#include <thread>
#include <chrono>

@interface NSObject (InterfaceChecks)
- (void)newFunctionWithDescriptor:(MTLFunctionDescriptor *)d completionHandler:(void (^)(id<MTLFunction>,NSError *))handler;
- (void)newComputePipelineStateWithDescriptor:(MTLComputePipelineDescriptor *)d completionHandler:(void (^)(id<MTLComputePipelineState>,NSError *))handler;
- (id<MTLTexture>)newTextureWithBytesNoCopy:(void *)bytes length:(NSUInteger)length descriptor:(MTLTextureDescriptor *)d deallocator:(void (^)(void *,NSUInteger))deallocator;
@end
static unsigned missing;
static void absent(const char *c,const char *s) {fprintf(stderr,"missing %s %s\n",c,s);++missing;}
#define CHECK(x) do {if(!(x)){fprintf(stderr,"FAIL %d: %s\n",__LINE__,#x);return 1;}}while(0)
static bool finish(id<MTLCommandBuffer> cb) {
 [cb commit];[cb waitUntilCompleted];
 if(cb.status!=MTLCommandBufferStatusCompleted)fprintf(stderr,"command failed: %s\n",cb.error.description.UTF8String);
 return cb.status==MTLCommandBufferStatusCompleted;
}
static id<MTLTexture> texture(id<MTLDevice> d,NSUInteger samples=1) {
 auto t=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:8 height:8 mipmapped:NO];
 t.usage=MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead|MTLTextureUsagePixelFormatView;
 t.storageMode=MTLStorageModePrivate;t.sampleCount=samples;
 if(samples>1)t.textureType=MTLTextureType2DMultisample;
 return [d newTextureWithDescriptor:t];
}
static id<MTLBuffer> readback(id<MTLDevice> dev,id<MTLCommandBuffer> cb,id<MTLTexture> t) {
 auto b=[dev newBufferWithLength:256 options:MTLResourceStorageModeShared];
 auto e=[cb blitCommandEncoder];
 [e copyFromTexture:t sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(8,8,1)
  toBuffer:b destinationOffset:0 destinationBytesPerRow:32 destinationBytesPerImage:256];[e endEncoding];return b;
}
int main(int argc,char **argv) {@autoreleasepool {
 CHECK(argc==4);const bool defaults=!strcmp(argv[3],"default");
 const bool single=defaults || !strcmp(argv[3],"single");
 if(defaults)unsetenv("NATIVE_METAL_SINGLE_SAMPLE");
 else setenv("NATIVE_METAL_SINGLE_SAMPLE",single?"1":"0",1);
 id<MTLDevice> dev=native_bridge_metal_open(argv[1],argv[2],0,absent);CHECK(dev);
 id<MTLCommandQueue> queue=[dev newCommandQueue];CHECK(queue);
 // Automatic heaps reclaim their ranges while aliasable objects remain alive.
 auto hd=[MTLHeapDescriptor new];hd.size=65536;hd.storageMode=MTLStorageModePrivate;
 auto heap=[dev newHeapWithDescriptor:hd];CHECK(heap);
 auto b=[heap newBufferWithLength:4096 options:MTLResourceStorageModePrivate];CHECK(b);
 NSUInteger used=heap.usedSize;CHECK(used>=4096);[b makeAliasable];CHECK(b.isAliasable && heap.usedSize<used);
 auto td=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:8 height:8 mipmapped:NO];
 td.storageMode=MTLStorageModePrivate;td.usage=MTLTextureUsageShaderRead|MTLTextureUsagePixelFormatView;
 auto ht=[heap newTextureWithDescriptor:td];CHECK(ht);used=heap.usedSize;
 [ht makeAliasable];CHECK(ht.isAliasable && heap.usedSize<used);
 // Zero is a real swizzle, not a marker for the identity.
 td.swizzle=MTLTextureSwizzleChannelsMake(MTLTextureSwizzleZero,MTLTextureSwizzleZero,MTLTextureSwizzleZero,MTLTextureSwizzleZero);
 auto zero=[dev newTextureWithDescriptor:td];CHECK(zero && zero.swizzle.red==MTLTextureSwizzleZero && zero.swizzle.alpha==MTLTextureSwizzleZero);
 auto view=[zero newTextureViewWithPixelFormat:zero.pixelFormat textureType:zero.textureType levels:NSMakeRange(0,1) slices:NSMakeRange(0,1)
  swizzle:MTLTextureSwizzleChannelsMake(MTLTextureSwizzleBlue,MTLTextureSwizzleGreen,MTLTextureSwizzleRed,MTLTextureSwizzleOne)];
 CHECK(view && view.swizzle.red==MTLTextureSwizzleBlue && view.swizzle.alpha==MTLTextureSwizzleOne);
 // Completion selectors deliver successes and errors; constants expose their actual names and indices.
 size_t length=0;const void *air=nmb_function_constant_air(&length);
 id<MTLLibrary> library=native_bridge_metal_library_with_air(air,length);CHECK(library);
 auto function=[library newFunctionWithName:@"fcfold"];CHECK(function && function.functionConstantsDictionary.count);
 for(NSString *name in function.functionConstantsDictionary) {
  MTLFunctionConstant *c=function.functionConstantsDictionary[name];CHECK([name isEqual:c.name]);CHECK(c.type!=MTLDataTypeNone);
 }
 auto fd=[MTLFunctionDescriptor new];fd.name=@"absent_function";
 __block unsigned callbacks=0;
 [(id)library newFunctionWithDescriptor:fd completionHandler:^(id<MTLFunction> f,NSError *error){if(!f && error)callbacks++;}];CHECK(callbacks==1);
 fd.name=@"fcfold";fd.specializedName=@"aliased_fcfold";fd.constantValues=[MTLFunctionConstantValues new];int mode=0;
 [fd.constantValues setConstantValue:&mode type:MTLDataTypeInt atIndex:0];
 __block id<MTLFunction> specialized;
 [(id)library newFunctionWithDescriptor:fd completionHandler:^(id<MTLFunction> f,NSError *error){if(f && !error){callbacks++;specialized=f;}}];CHECK(callbacks==2 && specialized && [specialized.name isEqualToString:@"aliased_fcfold"]);
 auto cd=[MTLComputePipelineDescriptor new];cd.computeFunction=specialized;
 [(id)dev newComputePipelineStateWithDescriptor:cd completionHandler:^(id<MTLComputePipelineState> p,NSError *error){if(p && !error)callbacks++;}];CHECK(callbacks==3);
 // Clear/resolve and load/store-and-resolve preserve both outputs in single-sample mode.
 if(single)for(unsigned variant=0;variant<3;variant++) {
  auto source=texture(dev,4),dest=texture(dev);CHECK(source && source.sampleCount==1 && dest);
  auto cb=[queue commandBuffer];auto pass=[MTLRenderPassDescriptor renderPassDescriptor];
  pass.colorAttachments[0].texture=source;pass.colorAttachments[0].loadAction=MTLLoadActionClear;
  pass.colorAttachments[0].clearColor=MTLClearColorMake(1,0,0,1);pass.colorAttachments[0].storeAction=MTLStoreActionStore;
  if(variant==2){auto e=[cb renderCommandEncoderWithDescriptor:pass];[e endEncoding];pass.colorAttachments[0].loadAction=MTLLoadActionLoad;}
  pass.colorAttachments[0].resolveTexture=dest;
  pass.colorAttachments[0].storeAction=variant==0?MTLStoreActionMultisampleResolve:MTLStoreActionStoreAndMultisampleResolve;
  auto e=[cb renderCommandEncoderWithDescriptor:pass];CHECK(e);
  MTLViewport vp[2]={{0,0,8,8,0,1},{0,0,4,4,0,1}};MTLScissorRect sc[2]={{0,0,8,8},{0,0,4,4}};
  [e setViewports:vp count:2];[e setScissorRects:sc count:2];[e endEncoding];
  auto output=readback(dev,cb,dest);id<MTLBuffer> original=variant?readback(dev,cb,source):nil;
  CHECK(finish(cb));for(unsigned i=0;i<64;i++){
   CHECK(((const uint32_t *)output.contents)[i]==0xff0000ffu);
   if(original)CHECK(((const uint32_t *)original.contents)[i]==0xff0000ffu);
  }
 }
 // A real draw uses the same sample count as its attachment and resolve target.
 air=nmb_fixture_air(&length);id<MTLLibrary> drawLibrary=native_bridge_metal_library_with_air(air,length);CHECK(drawLibrary);
 const float positions[]={-1,-1,3,-1,-1,3},colors[]={0,1,0,1,0,1,0,1,0,1,0,1};
 auto vertices=[dev newBufferWithBytes:positions length:sizeof positions options:MTLResourceStorageModeShared];
 auto colorBuffer=[dev newBufferWithBytes:colors length:sizeof colors options:MTLResourceStorageModeShared];
 for(unsigned sampled=0;sampled<2;sampled++) {
  auto pd=[MTLRenderPipelineDescriptor new];pd.vertexFunction=[drawLibrary newFunctionWithName:@"colored_vertex"];
  pd.fragmentFunction=[drawLibrary newFunctionWithName:sampled?@"sampled_fragment":@"colored_fragment"];
  pd.colorAttachments[0].pixelFormat=MTLPixelFormatRGBA8Unorm;pd.rasterSampleCount=single?4:1;
  NSError *error=nil;auto pipeline=[dev newRenderPipelineStateWithDescriptor:pd error:&error];
  if(!pipeline)fprintf(stderr,"draw pipeline: %s\n",error.description.UTF8String);CHECK(pipeline);
  auto source=texture(dev,single?4:1),target=texture(dev);CHECK(source && target && source.allocatedSize>0 && dev.currentAllocatedSize>0);
  auto cb=[queue commandBuffer];auto pass=[MTLRenderPassDescriptor renderPassDescriptor];
  pass.colorAttachments[0].texture=source;pass.colorAttachments[0].loadAction=MTLLoadActionClear;
  pass.colorAttachments[0].storeAction=single?MTLStoreActionMultisampleResolve:MTLStoreActionStore;
  if(single)pass.colorAttachments[0].resolveTexture=target;
  auto encoder=[cb renderCommandEncoderWithDescriptor:pass];CHECK(encoder);[encoder setRenderPipelineState:pipeline];
  [encoder setVertexBuffer:vertices offset:0 atIndex:0];[encoder setVertexBuffer:colorBuffer offset:0 atIndex:1];
  if(sampled) {
   auto sd=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:8 height:8 mipmapped:NO];
   sd.storageMode=MTLStorageModeShared;sd.usage=MTLTextureUsageShaderRead|MTLTextureUsagePixelFormatView;
   auto input=[dev newTextureWithDescriptor:sd];CHECK(input);uint32_t pixels[64];for(auto &p:pixels)p=0x40302010;
   [input replaceRegion:MTLRegionMake2D(0,0,8,8) mipmapLevel:0 withBytes:pixels bytesPerRow:32];
   auto swizzled=[input newTextureViewWithPixelFormat:input.pixelFormat textureType:input.textureType levels:NSMakeRange(0,1) slices:NSMakeRange(0,1)
    swizzle:MTLTextureSwizzleChannelsMake(MTLTextureSwizzleBlue,MTLTextureSwizzleGreen,MTLTextureSwizzleRed,MTLTextureSwizzleOne)];
   CHECK(swizzled && swizzled.allocatedSize==input.allocatedSize);[encoder setFragmentTexture:swizzled atIndex:0];
  }
  [encoder drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[encoder endEncoding];
  auto pixels=readback(dev,cb,single?target:source);CHECK(finish(cb));
  for(unsigned i=0;i<64;i++)CHECK(((const uint32_t *)pixels.contents)[i]==(sampled?0xff102030u:0xff00ff00u));
 }
 // Shader-selected viewport 1 must use the second viewport and scissor, not entry zero.
 {
  NSError *error=nil;
  NSString *source=@"#include <metal_stdlib>\nusing namespace metal;\n"
   "struct V {float4 p [[position]]; uint viewport [[viewport_array_index]];};\n"
   "vertex V indexed_vertex(uint id [[vertex_id]]) {float2 p[3]={float2(-1,-1),float2(3,-1),float2(-1,3)}; return {float4(p[id],0,1),1};}\n"
   "fragment float4 red_fragment() {return float4(1,0,0,1);}";
  auto lib=[dev newLibraryWithSource:source options:nil error:&error];
  if(!lib)fprintf(stderr,"viewport library: %s\n",error.description.UTF8String);CHECK(lib);
  auto pd=[MTLRenderPipelineDescriptor new];pd.vertexFunction=[lib newFunctionWithName:@"indexed_vertex"];
  pd.fragmentFunction=[lib newFunctionWithName:@"red_fragment"];pd.colorAttachments[0].pixelFormat=MTLPixelFormatRGBA8Unorm;
  auto pipeline=[dev newRenderPipelineStateWithDescriptor:pd error:&error];CHECK(pipeline);
  auto target=texture(dev);auto cb=[queue commandBuffer];auto pass=[MTLRenderPassDescriptor renderPassDescriptor];
  pass.colorAttachments[0].texture=target;pass.colorAttachments[0].loadAction=MTLLoadActionClear;pass.colorAttachments[0].storeAction=MTLStoreActionStore;
  pass.colorAttachments[0].clearColor=MTLClearColorMake(0,0,0,0);
  auto e=[cb renderCommandEncoderWithDescriptor:pass];[e setRenderPipelineState:pipeline];
  MTLViewport vp[2]={{0,0,8,8,0,1},{4,0,4,8,0,1}};MTLScissorRect sc[2]={{0,0,8,8},{4,2,4,4}};
  [e setViewports:vp count:2];[e setScissorRects:sc count:2];[e drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[e endEncoding];
  auto pixels=readback(dev,cb,target);CHECK(finish(cb));
  for(unsigned y=0;y<8;y++)for(unsigned x=0;x<8;x++)CHECK(((const uint32_t *)pixels.contents)[y*8+x]==(x>=4 && y>=2 && y<6?0xff0000ffu:0));
 }
 // A zero-length compressed input infers its layout size and retains the deallocator.
 auto compressed=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatASTC_4x4_LDR width:4 height:4 mipmapped:NO];
 compressed.storageMode=MTLStorageModeShared;compressed.usage=MTLTextureUsageShaderRead;
 __block bool released=false;uint8_t block[16]={};const void *blockAddress=block;
 @autoreleasepool {
  id<MTLTexture> input=[(id)dev newTextureWithBytesNoCopy:block length:0 descriptor:compressed deallocator:^(void *bytes,NSUInteger n){released=bytes==blockAddress && n==16;}];
  CHECK(input);uint8_t copied[16]={};[input getBytes:copied bytesPerRow:16 fromRegion:MTLRegionMake2D(0,0,4,4) mipmapLevel:0];CHECK(!memcmp(block,copied,16));input=nil;
 }
 CHECK(released);
 // Range copies use the requested source/destination mip and slice, including array layers.
 auto ad=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA8Unorm width:8 height:8 mipmapped:YES];
 ad.textureType=MTLTextureType2DArray;ad.arrayLength=2;ad.storageMode=MTLStorageModeShared;ad.usage=MTLTextureUsageShaderRead;
 auto a=[dev newTextureWithDescriptor:ad],dest=[dev newTextureWithDescriptor:ad];CHECK(a && dest);
 for(NSUInteger slice=0;slice<2;slice++)for(NSUInteger level=0;level<4;level++) {
  NSUInteger w=8>>level;std::vector<uint32_t> pixels(w*w,0xff000000u|uint32_t(slice*32+level));
  [a replaceRegion:MTLRegionMake2D(0,0,w,w) mipmapLevel:level slice:slice withBytes:pixels.data() bytesPerRow:w*4 bytesPerImage:w*w*4];
 }
 auto cb=[queue commandBuffer];auto blit=[cb blitCommandEncoder];
 [blit copyFromTexture:a sourceSlice:0 sourceLevel:1 toTexture:dest destinationSlice:0 destinationLevel:1 sliceCount:2 levelCount:3];
 [blit endEncoding];CHECK(finish(cb));
 for(NSUInteger slice=0;slice<2;slice++)for(NSUInteger level=1;level<4;level++){
  NSUInteger w=8>>level;std::vector<uint32_t> pixels(w*w);
  [dest getBytes:pixels.data() bytesPerRow:w*4 bytesPerImage:w*w*4 fromRegion:MTLRegionMake2D(0,0,w,w) mipmapLevel:level slice:slice];
  for(uint32_t pixel:pixels)CHECK(pixel==(0xff000000u|uint32_t(slice*32+level)));
 }
 // A completed object can remain retained while its queue reuses the slot after its handlers return.
 auto limited=[dev newCommandQueueWithMaxCommandBufferCount:1];CHECK(limited);
 auto first=[limited commandBuffer];auto done=dispatch_semaphore_create(0);
 [first addCompletedHandler:^(id<MTLCommandBuffer>){dispatch_semaphore_signal(done);}];
 [first commit];CHECK(dispatch_semaphore_wait(done,dispatch_time(DISPATCH_TIME_NOW,10*NSEC_PER_SEC))==0);[first waitUntilCompleted];
 @autoreleasepool{auto next=[limited commandBuffer];CHECK(next);}
 // Refused work completes and releases its slot even while the caller retains the object.
 auto refused=[limited commandBuffer];auto failedBlit=[refused blitCommandEncoder];
 id<MTLBuffer> absentBuffer=nil;[failedBlit fillBuffer:absentBuffer range:NSMakeRange(0,4) value:1];[failedBlit endEncoding];
 auto recovered=dispatch_semaphore_create(0);
 [refused addCompletedHandler:^(id<MTLCommandBuffer> b){if(b.status==MTLCommandBufferStatusError)dispatch_semaphore_signal(recovered);}];
 [refused commit];CHECK(dispatch_semaphore_wait(recovered,dispatch_time(DISPATCH_TIME_NOW,10*NSEC_PER_SEC))==0);[refused waitUntilCompleted];
 @autoreleasepool{auto next=[limited commandBuffer];CHECK(next);}
 // A requested limit above the default is passed through too.
 @autoreleasepool {
  auto larger=[dev newCommandQueueWithMaxCommandBufferCount:96];CHECK(larger);
  NSMutableArray *buffers=[NSMutableArray array];for(unsigned i=0;i<65;i++){auto b=[larger commandBuffer];CHECK(b);[buffers addObject:b];}
 }
 // An uncommitted buffer returns its slot when abandoned; a full queue waits until a slot is free.
 id<MTLCommandBuffer> held;@autoreleasepool{held=[limited commandBuffer];}CHECK(held);
 auto started=dispatch_semaphore_create(0),acquired=dispatch_semaphore_create(0);
 std::thread waiter([&]{@autoreleasepool{dispatch_semaphore_signal(started);auto pending=[limited commandBuffer];if(pending)dispatch_semaphore_signal(acquired);}});
 CHECK(dispatch_semaphore_wait(started,dispatch_time(DISPATCH_TIME_NOW,10*NSEC_PER_SEC))==0);
 CHECK(dispatch_semaphore_wait(acquired,dispatch_time(DISPATCH_TIME_NOW,50*NSEC_PER_MSEC))!=0);
 held=nil;
 CHECK(dispatch_semaphore_wait(acquired,dispatch_time(DISPATCH_TIME_NOW,10*NSEC_PER_SEC))==0);waiter.join();
 CHECK(!missing);printf("PASS: interfaces, heap reclaim, range copies, callbacks, queue reuse%s\n",single?", single-sample resolve pixels":"");
 return 0;
}}
