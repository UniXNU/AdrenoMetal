// SPDX-License-Identifier: MIT
// Replay the captured macOS 27.3 RenderBox alpha stage with controlled framebuffer values.
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <dlfcn.h>
#include <cstdint>
#include <cstring>
#include <cstdio>
#include <vector>
static id (*libraryWithAir)(const void *,size_t);
#include <cmath>
static id<MTLLibrary> air(const char *path) {
 NSData *d=[NSData dataWithContentsOfFile:@(path)];uint32_t offset=0,length=0;if(d.length<20)return nil;
 memcpy(&offset,(const uint8_t *)d.bytes+8,4);memcpy(&length,(const uint8_t *)d.bytes+12,4);
 if(offset>d.length || length>d.length-offset)return nil;
 return libraryWithAir((const uint8_t *)d.bytes+offset,length);
}
int main(int argc,char **argv) {@autoreleasepool {
 if(argc!=5)return 2;id<MTLDevice> device=MTLCreateSystemDefaultDevice();if(!device)return 2;
 void *plugin=dlopen("/Library/GPUBundles/NativeAdrenoMetal.bundle/Contents/MacOS/NativeAdrenoMetal",RTLD_LAZY|RTLD_NOLOAD);
 libraryWithAir=(id (*)(const void *,size_t))dlsym(plugin?plugin:RTLD_DEFAULT,"native_bridge_metal_library_with_air");if(!libraryWithAir){fprintf(stderr,"Metal bridge AIR entry unavailable\n");return 2;}
 printf("RenderBox alpha control device=%s\n",device.name.UTF8String);
 auto vl=air(argv[1]),fl=air(argv[2]);if(!vl || !fl){fprintf(stderr,"AIR creation failed\n");return 1;}
 NSData *globals=[NSData dataWithContentsOfFile:@(argv[3])],*gradient=[NSData dataWithContentsOfFile:@(argv[4])];
 uint32_t state[4]={0xe80524,0x24000,0xf473,0x6c};auto fc=[MTLFunctionConstantValues new];[fc setConstantValue:state type:MTLDataTypeUInt4 atIndex:0];
 NSError *error=nil;auto pd=[MTLRenderPipelineDescriptor new];pd.vertexFunction=[vl newFunctionWithName:@"alpha_effect_vertex"];pd.fragmentFunction=[fl newFunctionWithName:@"alpha_effect_fragment" constantValues:fc error:&error];pd.colorAttachments[0].pixelFormat=MTLPixelFormatRGBA16Float;pd.colorAttachments[1].pixelFormat=MTLPixelFormatRG32Float;pd.colorAttachments[1].writeMask=MTLColorWriteMaskNone;
 auto pipeline=[device newRenderPipelineStateWithDescriptor:pd error:&error];if(!pipeline){fprintf(stderr,"pipeline: %s | %s\n",error.description.UTF8String,"");return 1;}
 auto td=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA16Float width:230 height:124 mipmapped:NO];td.usage=MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead;auto color=[device newTextureWithDescriptor:td];td.pixelFormat=MTLPixelFormatRG32Float;auto coverage=[device newTextureWithDescriptor:td];
 td.width=td.height=32;td.pixelFormat=MTLPixelFormatRGBA8Unorm;auto noise=[device newTextureWithDescriptor:td];uint8_t n[32*32*4];memset(n,127,sizeof n);[noise replaceRegion:MTLRegionMake2D(0,0,32,32) mipmapLevel:0 withBytes:n bytesPerRow:128];
 uint16_t indices[4]={1,0,2,3};auto index=[device newBufferWithBytes:indices length:sizeof indices options:MTLResourceStorageModeShared];auto queue=[device newCommandQueue];
 unsigned failed=0;
 for(double red : {-0.25,0.,0.05,0.15,0.5,1.,1.5}) {
  auto cb=[queue commandBuffer];auto pass=[MTLRenderPassDescriptor renderPassDescriptor];pass.colorAttachments[0].texture=color;pass.colorAttachments[0].loadAction=MTLLoadActionClear;pass.colorAttachments[0].storeAction=MTLStoreActionStore;pass.colorAttachments[0].clearColor=MTLClearColorMake(red,.2,.3,1);pass.colorAttachments[1].texture=coverage;pass.colorAttachments[1].loadAction=MTLLoadActionClear;pass.colorAttachments[1].storeAction=MTLStoreActionStore;pass.colorAttachments[1].clearColor=MTLClearColorMake(1,.5,0,0);
  auto e=[cb renderCommandEncoderWithDescriptor:pass];[e setRenderPipelineState:pipeline];[e setScissorRect:(MTLScissorRect){124,0,106,106}];[e setVertexBytes:globals.bytes length:globals.length atIndex:0];[e setFragmentBytes:globals.bytes length:globals.length atIndex:0];[e setFragmentBytes:gradient.bytes length:gradient.length atIndex:3];[e setFragmentTexture:noise atIndex:3];[e drawIndexedPrimitives:MTLPrimitiveTypeTriangleStrip indexCount:4 indexType:MTLIndexTypeUInt16 indexBuffer:index indexBufferOffset:0];[e endEncoding];[cb commit];[cb waitUntilCompleted];if(cb.status!=MTLCommandBufferStatusCompleted){fprintf(stderr,"render: %s\n",cb.error.description.UTF8String);return 1;}
  std::vector<__fp16> pixels(230*124*4);[color getBytes:pixels.data() bytesPerRow:230*8 fromRegion:MTLRegionMake2D(0,0,230,124) mipmapLevel:0];unsigned bad=0;for(__fp16 v:pixels)bad+=!std::isfinite(float(v));failed+=bad!=0;size_t at=(50*230+150)*4;printf("source=%g nonfinite=%u sample=%g,%g,%g,%g\n",red,bad,float(pixels[at]),float(pixels[at+1]),float(pixels[at+2]),float(pixels[at+3]));
 }
 return failed?1:0;
}}
