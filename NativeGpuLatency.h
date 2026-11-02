/* SPDX-License-Identifier: MIT */
#ifndef NATIVE_GPU_LATENCY_H
#define NATIVE_GPU_LATENCY_H
#include <stdint.h>

/* Runtime stages kept in the render server's frame trace: command, memory,
 * completion and scheduling. */
static inline bool native_gpu_latency_stage(uint32_t stage) {
    return (stage >= 1 && stage <= 16) || (stage >= 40 && stage <= 57);
}
/* The kernel ring keeps GPU submission and retirement, display arrival, fence
 * and latch, build identity, the render threads the CPU sampler follows and
 * each process's Metal refusals (event 65, the text of what failed). */
static inline bool native_gpu_latency_event(uint32_t event,const uint64_t *,uint32_t) {
    switch(event) {
    case 1:case 3:case 10:case 11:case 20:case 21:case 22:case 23:case 63:case 65:return true;
    default:return false;
    }
}

/* A pending display query retains transitions and a 100 ms heartbeat, plus the
 * number of identical queries omitted. Completion is always retained. */
struct NativeGpuLatencyWaits {
    struct Slot {uint32_t busy=0,pid=0,reason=0;uint64_t request=0,target=0,when=0,omitted=0;};
    Slot slots[32]{};
    bool keep(uint64_t now,uint32_t pid,uint64_t request,uint32_t reason,uint64_t target,uint64_t &omitted) {
        Slot &s=slots[(request ^ pid)&31];uint32_t free=0;
        if(!__atomic_compare_exchange_n(&s.busy,&free,1,false,__ATOMIC_ACQUIRE,__ATOMIC_RELAXED))return true;
        const bool same=s.request==request && s.pid==pid && s.target==target;
        const bool pending=reason==3 || reason==4;
        const bool emit=!pending || !same || s.reason!=reason || now-s.when>=100000;
        if(emit) {
            omitted=same?s.omitted:0;
            s.pid=pid;s.request=request;s.target=target;s.reason=reason;s.when=now;s.omitted=0;
        } else ++s.omitted;
        __atomic_store_n(&s.busy,0,__ATOMIC_RELEASE);return emit;
    }
};
#endif
