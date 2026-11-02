// SPDX-License-Identifier: MIT
#include "../QueuePriority.h"
#include <cassert>
#include <cstdio>
int main() {
    NBQueuePriorities p;
    using R=NBQueuePriorities::Request;
    p.add(R{});p.add({0,2});
    assert(p.aggregate().foreground==0 && p.aggregate().background==2);
    p.remove({0,2});p.add({2,3});
    assert(p.aggregate().foreground==1 && p.aggregate().background==3);
    p.remove(R{});
    assert(p.aggregate().foreground==2);
    p.add({5,4});assert(p.aggregate().foreground==5);
    p.remove({5,4});assert(p.aggregate().foreground==2);
    p.remove({2,3});assert(p.aggregate().foreground==1 && p.aggregate().background==3);
    assert(!p.valid(3,false) && p.valid(3,true) && !p.valid(6,true));
    std::puts("PASS current queue priorities: promote, lower, destroy, defaults, invalid values");
}
