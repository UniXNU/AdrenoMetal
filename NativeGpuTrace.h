/* SPDX-License-Identifier: MIT */
#ifndef NATIVE_GPU_TRACE_H
#define NATIVE_GPU_TRACE_H
#include <stdint.h>
#include <stddef.h>

/* Diagnostic ABI, independent of rendering policy. All times are uptime us.
 * Event fields and joins are documented by scripts/decode-gpu-trace.py. */
enum { NativeGpuTraceVersion=1, NativeGpuTraceNote=3, NativeGpuTraceMethod=0x7102,
       NativeGpuTraceCapacity=65536, NativeGpuTraceMetadata=4096 };
enum {
    GpuTraceConfig=1, GpuTraceBuild=3,
    GpuTraceSubmit=10, GpuTraceRetire=11,
    GpuTraceDisplay=20, GpuTraceDisplaySurface=21, GpuTraceDisplayFence=22,
    GpuTraceDisplayLatch=23, GpuTraceTurnipPass=36, GpuTraceTurnipGmemSide=37, GpuTraceTurnipAttachment=44,
    GpuTraceThread=63, GpuTraceProblem=65
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
#ifdef __cplusplus
static_assert(sizeof(NativeGpuTraceRecord)==120,"GPU trace record ABI");
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
