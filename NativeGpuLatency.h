/* SPDX-License-Identifier: MIT */
#ifndef NATIVE_GPU_LATENCY_H
#define NATIVE_GPU_LATENCY_H
#include <stdint.h>

/* Keep command, memory, completion, scheduling and display causality. */
static inline bool native_gpu_latency_stage(uint32_t stage) {
    return (stage >= 1 && stage <= 16) || (stage >= 40 && stage <= 57);
}
static inline bool native_gpu_latency_event(uint32_t event,const uint64_t *v,uint32_t count) {
    switch(event) {
    case 39:return count >= 3 && native_gpu_latency_stage((uint32_t)v[1]);
    case 1:case 3:case 10:case 11:case 12:case 13:case 15:
    case 20:case 21:case 22:case 23:case 30:case 32:case 33:case 34:
    case 36:case 38:case 41:case 42:case 43:case 45:case 46:case 47:
    case 48:case 50:case 51:case 52:case 53:case 55:case 58:
    case 60:case 61:case 62:case 63:case 64:case 67:case 68:return true;
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
