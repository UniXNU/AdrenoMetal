// SPDX-License-Identifier: MIT
#import <Foundation/Foundation.h>
#include <atomic>
#include <dlfcn.h>
#include <cassert>
#include <cstring>
#include <vector>
#include <array>
static thread_local uint64_t tDispatchItem;
static bool diagOn(){return true;}
static std::vector<std::array<uint64_t,7>> rows;
template<class... T> static void frameEvent(uint32_t event,T... v){rows.push_back({event,(uint64_t)v...});}
struct dispatch_introspection_hooks_s;
static void *mockDlsym(void *,const char *);
#define dlsym mockDlsym
#include "../NativeMetalDispatchTrace.h"
#undef dlsym
static dispatch_introspection_hooks_s active{};
static NBDispatchVersions version{1,2,sizeof(active),1,64,1,56,1,56};
static unsigned oldEnqueue;
static void previousEnqueue(dispatch_queue_s *,dispatch_introspection_queue_item_s *){++oldEnqueue;}
static void previousCreate(dispatch_introspection_queue_s *){}
static void mockInstall(dispatch_introspection_hooks_s *hooks){auto previous=active;active=*hooks;*hooks=previous;}
static void *mockDlsym(void *,const char *name){if(!strcmp(name,"dispatch_introspection_hooks_install"))return (void *)mockInstall;return &version;}
int main(int argc,char **argv) {
 if(argc>1){if(!strcmp(argv[1],"item-version"))version.item_version=2;else version.hooks_version=3;}
 active.queue_item_enqueue=previousEnqueue;active.queue_create=previousCreate;
 installDispatchTrace();
 if(argc>1){assert(active.queue_item_enqueue==previousEnqueue);assert(rows.back()[3]==2);return 0;}
 assert(active.queue_create==previousCreate);
 dispatch_queue_s *queue=(dispatch_queue_s *)(__bridge void *)dispatch_get_main_queue();
 uint64_t a[8]={0x11,11,0,101,201,0,0,0},b[8]={0x12,22,0,102,202,0,0,3};
 active.queue_item_enqueue(queue,(dispatch_introspection_queue_item_s *)a);assert(oldEnqueue==1);
 active.queue_item_dequeue(queue,(dispatch_introspection_queue_item_s *)a);assert(tDispatchItem==11);
 active.queue_item_enqueue(queue,(dispatch_introspection_queue_item_s *)b);assert(rows.back()[5]==11);
 active.queue_item_dequeue(queue,(dispatch_introspection_queue_item_s *)b);assert(tDispatchItem==22);
 active.queue_item_complete((dispatch_continuation_s *)22);assert(tDispatchItem==11);
 active.queue_item_complete((dispatch_continuation_s *)11);assert(tDispatchItem==0);
 assert(oldEnqueue==2);
}
