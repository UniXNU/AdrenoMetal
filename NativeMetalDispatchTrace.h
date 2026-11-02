/* SPDX-License-Identifier: MIT */
/* dispatch/private/introspection_private.h, hooks ABI v2. No queue layout is read. */
struct dispatch_introspection_queue_s;
struct dispatch_queue_s;
struct dispatch_introspection_queue_item_s;
struct dispatch_continuation_s;
enum dispatch_introspection_runtime_event {NBDispatchEventPlaceholder=0};
struct dispatch_introspection_hooks_s {
 void (*queue_create)(dispatch_introspection_queue_s *);
 void (*queue_dispose)(dispatch_introspection_queue_s *);
 void (*queue_item_enqueue)(dispatch_queue_s *,dispatch_introspection_queue_item_s *);
 void (*queue_item_dequeue)(dispatch_queue_s *,dispatch_introspection_queue_item_s *);
 void (*queue_item_complete)(dispatch_continuation_s *);
 void (*runtime_event)(dispatch_introspection_runtime_event,void *,unsigned long long);
 void *reserved[4];
};
struct NBDispatchVersions {
 unsigned long version,hooks_version;size_t hooks_size;
 unsigned long item_version;size_t item_size;
 unsigned long block_version;size_t block_size;
 unsigned long function_version;size_t function_size;
};
static dispatch_introspection_hooks_s gPreviousDispatchHooks{};
static std::atomic<bool> gDispatchPreviousReady{false};
static thread_local bool tDispatchTracing;
static thread_local uint64_t tDispatchParents[32];
static thread_local unsigned tDispatchDepth;
static void dispatchQueueDefinition(dispatch_queue_t queue) {
 static std::atomic<uint64_t> seen[256];const uint64_t key=(uintptr_t)(__bridge void *)queue;
 if(!key)return;
 for(auto &slot:seen) {
  uint64_t old=slot.load(std::memory_order_relaxed);if(old==key)return;
  if(!old && slot.compare_exchange_strong(old,key)) {
   const char *label=dispatch_queue_get_label(queue);char text[48]={};if(label)strncpy(text,label,sizeof text-1);
   for(unsigned part=0;part<2;++part){uint64_t words[3]={};memcpy(words,text+24*part,24);frameEvent(68,key,5,part,words[0],words[1],words[2]);}
   return;
  }
 }
}
static void dispatchItemPoint(dispatch_queue_s *rawQueue,dispatch_introspection_queue_item_s *item,unsigned phase) {
 dispatch_queue_t queue=(__bridge dispatch_queue_t)(void *)rawQueue;
 if(tDispatchTracing || !item)return;
 tDispatchTracing=true;
 uint64_t type=0,fields[6]={},flags=0;const char *raw=(const char *)item;
 memcpy(&type,raw,8);
 if(type==0x11 || type==0x12) {
  memcpy(fields,raw+8,sizeof fields);memcpy(&flags,raw+56,8);
  dispatchQueueDefinition(queue);
  frameEvent(68,fields[0],phase|((flags&7)<<8),(uintptr_t)(__bridge void *)queue,fields[3],tDispatchItem,fields[2]);
  if(phase==2 && tDispatchDepth<32){tDispatchParents[tDispatchDepth++]=tDispatchItem;tDispatchItem=fields[0];}
 }
 tDispatchTracing=false;
}
static void dispatchItemEnqueue(dispatch_queue_s *queue,dispatch_introspection_queue_item_s *item) {
 dispatchItemPoint(queue,item,1);
 if(gDispatchPreviousReady.load(std::memory_order_acquire) && gPreviousDispatchHooks.queue_item_enqueue)gPreviousDispatchHooks.queue_item_enqueue(queue,item);
}
static void dispatchItemDequeue(dispatch_queue_s *queue,dispatch_introspection_queue_item_s *item) {
 dispatchItemPoint(queue,item,2);
 if(gDispatchPreviousReady.load(std::memory_order_acquire) && gPreviousDispatchHooks.queue_item_dequeue)gPreviousDispatchHooks.queue_item_dequeue(queue,item);
}
static void dispatchItemComplete(dispatch_continuation_s *item) {
 if(!tDispatchTracing && (uintptr_t)item==tDispatchItem) {
  tDispatchTracing=true;frameEvent(68,(uintptr_t)item,3,0,0,tDispatchDepth?tDispatchParents[tDispatchDepth-1]:0,0);
  tDispatchItem=tDispatchDepth?tDispatchParents[--tDispatchDepth]:0;tDispatchTracing=false;
 }
 if(gDispatchPreviousReady.load(std::memory_order_acquire) && gPreviousDispatchHooks.queue_item_complete)gPreviousDispatchHooks.queue_item_complete(item);
}
static void dispatchRuntimeEvent(dispatch_introspection_runtime_event event,void *pointer,unsigned long long value) {
 if(!tDispatchTracing){tDispatchTracing=true;frameEvent(68,(uintptr_t)pointer,4|((uint64_t)event<<8),0,0,tDispatchItem,value);tDispatchTracing=false;}
 if(gDispatchPreviousReady.load(std::memory_order_acquire) && gPreviousDispatchHooks.runtime_event)gPreviousDispatchHooks.runtime_event(event,pointer,value);
}
static void installDispatchTrace() {
 if(!diagOn())return;
 static std::atomic<bool> installed{false};if(installed.exchange(true))return;
 using Install=void (*)(dispatch_introspection_hooks_s *);
 auto install=(Install)dlsym(RTLD_DEFAULT,"dispatch_introspection_hooks_install");
 const auto *version=(const NBDispatchVersions *)dlsym(RTLD_DEFAULT,"dispatch_introspection_versions");
 if(!install || !version){frameEvent(68,0,0,1,0,0,0);return;}
 if(version->version!=1 || version->hooks_version!=2 || version->hooks_size!=sizeof(dispatch_introspection_hooks_s) ||
    version->item_version!=1 || version->block_version!=1 || version->function_version!=1 ||
    version->item_size<64 || version->block_size<56 || version->function_size<56) {
  frameEvent(68,0,0,2,version->hooks_version,version->hooks_size,version->item_size);return;
 }
 dispatch_introspection_hooks_s hooks{};
 hooks.queue_item_enqueue=dispatchItemEnqueue;hooks.queue_item_dequeue=dispatchItemDequeue;
 hooks.queue_item_complete=dispatchItemComplete;hooks.runtime_event=dispatchRuntimeEvent;
 install(&hooks);gPreviousDispatchHooks=hooks;gDispatchPreviousReady.store(true,std::memory_order_release);
 hooks.queue_create=gPreviousDispatchHooks.queue_create;hooks.queue_dispose=gPreviousDispatchHooks.queue_dispose;
 hooks.queue_item_enqueue=dispatchItemEnqueue;hooks.queue_item_dequeue=dispatchItemDequeue;
 hooks.queue_item_complete=dispatchItemComplete;hooks.runtime_event=dispatchRuntimeEvent;
 memcpy(hooks.reserved,gPreviousDispatchHooks.reserved,sizeof hooks.reserved);install(&hooks);
 frameEvent(68,0,0,0,version->hooks_version,version->hooks_size,version->item_size);
}
