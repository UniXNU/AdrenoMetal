/* SPDX-License-Identifier: MIT */
#ifndef NATIVE_GPU_TRACE_H
#define NATIVE_GPU_TRACE_H
#include <stdint.h>
#include <stddef.h>

/* Diagnostic ABI, independent of rendering policy. All times are uptime us.
 * Event fields and joins are documented by scripts/decode-gpu-trace.py. */
enum { NativeGpuTraceVersion=1, NativeGpuTraceNote=3,
       NativeGpuTraceMethod=0x7102, NativeGpuPixelMethod=0x7103,
       NativeGpuTraceCapacity=16384, NativeGpuTraceMetadata=4096, NativeGpuCaptureRecords=4096, NativeGpuPixelSlots=24,
       NativeGpuPixelBytes=65536, NativeGpuPixelChunk=1024 };
enum {
    GpuTraceConfig=1, GpuTraceProcess=2, GpuTraceBuild=3,
    GpuTraceSubmit=10, GpuTraceRetire=11, GpuTraceSwitch=12, GpuTraceRecovery=13,
    GpuTraceFenceQuery=14, GpuTraceImport=15,
    GpuTraceDisplay=20, GpuTraceDisplaySurface=21, GpuTraceDisplayFence=22,
    GpuTraceDisplayLatch=23, GpuTraceDisplayPixel=24,
    GpuTraceCommand=30, GpuTraceSurface=31, GpuTraceEvent=32,
    GpuTraceAttachment=33, GpuTraceAttachmentFormat=34, GpuTraceTexture=35,
    GpuTracePass=36, GpuTraceGmem=37, GpuTraceVkSubmit=38, GpuTraceRuntime=39,
    GpuTracePixel=40, GpuTraceDropped=41
};
struct NativeGpuTraceRecord {
    uint64_t sequence,us;
    uint32_t event,pid;
    uint64_t value[12];
};
struct NativeGpuTraceHeader {
    uint32_t magic,version,record_bytes,count;
    uint64_t issued,dropped;
};
/* A sampled grid of raw texels, never colour converted. source_stride describes
 * the producer; row_bytes describes the grid. coordinates are floor(x*width/w).
 * colour_space: 0 unknown, 1 linear format, 2 sRGB format. flags: 1 GPU complete,
 * 2 displayed, 4 content changed during the CPU read. */
struct NativeGpuPixelHeader {
    uint32_t magic,version,pid,stage;
    uint64_t capture,us,command,surface,physical;
    uint32_t width,height,format,colour_space;
    uint64_t source_stride;
    uint32_t grid_width,grid_height,row_bytes,bytes_per_pixel;
    uint32_t flags,bytes;
    uint64_t fence;
};
struct NativeGpuPixelPiece {
    struct NativeGpuPixelHeader header;
    uint32_t offset,bytes;
    uint8_t data[NativeGpuPixelChunk];
};
#ifdef __cplusplus
static_assert(sizeof(NativeGpuTraceRecord)==120,"GPU trace record ABI");
static_assert(sizeof(NativeGpuPixelHeader)==112,"GPU pixel header ABI");
extern "C" {
#endif
#ifdef NATIVE_GPU_TRACE_OPTIONAL
#define NATIVE_GPU_TRACE_LINK __attribute__((weak_import))
#else
#define NATIVE_GPU_TRACE_LINK
#endif
int native_gpu_trace_enabled(void) NATIVE_GPU_TRACE_LINK;
void native_gpu_trace_record(uint32_t event,uint32_t pid,const uint64_t *values,uint32_t count) NATIVE_GPU_TRACE_LINK;
uint32_t native_gpu_trace_snapshot(void *out,uint32_t bytes);
int native_gpu_pixel_put(const struct NativeGpuPixelPiece *piece,uint32_t pid) NATIVE_GPU_TRACE_LINK;
uint32_t native_gpu_pixel_snapshot(void *out,uint32_t bytes);
#ifdef __cplusplus
}
template<typename... T> static inline void gpuTraceEvent(uint32_t event,uint32_t pid,T... args) {
#ifdef NATIVE_GPU_TRACE_OPTIONAL
    if(!&native_gpu_trace_record)return;
#endif
    const uint64_t words[]={uint64_t(args)...};
    native_gpu_trace_record(event,pid,words,sizeof...(args));
}
#endif
#undef NATIVE_GPU_TRACE_LINK
#endif
