// SPDX-License-Identifier: MIT
#pragma once
#include <cstddef>
#include <cstdint>
// Logical Metal queues share one Vulkan queue. Count current requests, including
// defaults, so lowering or destroying a queue releases its priority boost.
class NBQueuePriorities {
    size_t counts_[2][6]{};
public:
    struct Request {uint32_t foreground=1,background=3;};
    static bool valid(uint32_t p,bool background) {return p<6 && (background || p!=3);}
    void add(Request r) {++counts_[0][r.foreground];++counts_[1][r.background];}
    void remove(Request r) {--counts_[0][r.foreground];--counts_[1][r.background];}
    Request aggregate() const {
        const uint32_t order[]={5,4,0,1,2,3};
        Request out;
        for(auto p:order)if(counts_[0][p]){out.foreground=p;break;}
        for(auto p:order)if(counts_[1][p]){out.background=p;break;}
        return out;
    }
};
