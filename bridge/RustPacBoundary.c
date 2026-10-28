// SPDX-License-Identifier: MIT
// The boundary between the Rust bridge and the rest of an arm64e process (iPadOS 27's render server). rustc compiles arm64e code without
// pointer authentication: it calls every function pointer with a plain blr/br and hands out unsigned function pointers. The rest of the
// process signs function pointers (IA key, discriminator 0) and authenticates them at each indirect call, so neither side's pointers work
// in the other's calls. Two fixes, both only in the arm64e plugin:
//  - Rust -> C: scripts/rust-pac-rewrite.py turns every blr/br Xn in the linked Rust code into bl/b nmb_pac_call_xN below, which strips
//    the signature (xpaci) and branches; an unsigned pointer passes unchanged. TLS thunks, dlsym results, Vulkan entry points and the
//    plugin's callbacks are all called this way.
//  - C -> Rust: the imports that take a Rust callback are renamed in the Rust objects (build-adreno-metal-plugin.py) to the nmb_pac_*
//    wrappers below, which sign the callback before handing it on. Vulkan's debug messenger callback is signed in the create calls,
//    reached through the wrapped vkGetInstanceProcAddr.
#include <ptrauth.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unwind.h>
#include <mach-o/dyld.h>
#include <vulkan/vulkan.h>

#define NMB_STRIP(f) ptrauth_strip((void *)(f), ptrauth_key_function_pointer)
#define NMB_SIGN(f) ptrauth_sign_unauthenticated(NMB_STRIP(f), ptrauth_key_function_pointer, 0)

// ---- Rust -> C: one trampoline per register (x30 is the link register and is never a target here) ----
#define NMB_CALL(n) __attribute__((naked, used, visibility("default"))) void nmb_pac_call_x##n(void) \
    {__asm__ volatile("xpaci x" #n "\n\tbr x" #n);}
NMB_CALL(0) NMB_CALL(1) NMB_CALL(2) NMB_CALL(3) NMB_CALL(4) NMB_CALL(5) NMB_CALL(6) NMB_CALL(7) NMB_CALL(8) NMB_CALL(9)
NMB_CALL(10) NMB_CALL(11) NMB_CALL(12) NMB_CALL(13) NMB_CALL(14) NMB_CALL(15) NMB_CALL(16) NMB_CALL(17) NMB_CALL(18) NMB_CALL(19)
NMB_CALL(20) NMB_CALL(21) NMB_CALL(22) NMB_CALL(23) NMB_CALL(24) NMB_CALL(25) NMB_CALL(26) NMB_CALL(27) NMB_CALL(28) NMB_CALL(29)

// ---- C -> Rust: imports that take a Rust callback ----
// The callback parameters are taken as void * so the compiler neither authenticates nor re-signs the unsigned bits Rust passed.
int nmb_pac_pthread_create(pthread_t *thread, const pthread_attr_t *attr, void *start, void *arg)
{
    return pthread_create(thread, attr, (void *(*)(void *))NMB_SIGN(start), arg);
}
void nmb_pac_tlv_atexit(void *destructor, void *object)
{
    _tlv_atexit((void (*)(void *))NMB_SIGN(destructor), object);
}
int nmb_pac_atexit(void *function)
{
    return atexit((void (*)(void))NMB_SIGN(function));
}
void nmb_pac_dyld_register_func_for_add_image(void *function)
{
    _dyld_register_func_for_add_image((void (*)(const struct mach_header *, intptr_t))NMB_SIGN(function));
}
_Unwind_Reason_Code nmb_pac_Unwind_Backtrace(void *trace, void *context)
{
    return _Unwind_Backtrace((_Unwind_Trace_Fn)NMB_SIGN(trace), context);
}

// ---- Vulkan: the debug messenger callback the runtime installs ----
static PFN_vkGetInstanceProcAddr gGetInstanceProcAddr;
static PFN_vkCreateInstance gCreateInstance;
static PFN_vkCreateDebugUtilsMessengerEXT gCreateMessenger;
// Signs every messenger callback in a pNext chain in place; returns the original values for restore (at most 8 messengers).
static unsigned signChain(const void *next, void **saved, unsigned cap)
{
    unsigned count = 0;
    for (VkBaseOutStructure *s = (VkBaseOutStructure *)next; s && count < cap; s = s->pNext) {
        if (s->sType != VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT) continue;
        VkDebugUtilsMessengerCreateInfoEXT *info = (VkDebugUtilsMessengerCreateInfoEXT *)s;
        saved[count++] = (void *)info->pfnUserCallback;
        if (info->pfnUserCallback) info->pfnUserCallback = (PFN_vkDebugUtilsMessengerCallbackEXT)NMB_SIGN(info->pfnUserCallback);
    }
    return count;
}
static void restoreChain(const void *next, void **saved, unsigned count)
{
    unsigned at = 0;
    for (VkBaseOutStructure *s = (VkBaseOutStructure *)next; s && at < count; s = s->pNext)
        if (s->sType == VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT)
            ((VkDebugUtilsMessengerCreateInfoEXT *)s)->pfnUserCallback = (PFN_vkDebugUtilsMessengerCallbackEXT)saved[at++];
}
static VkResult wrapCreateInstance(const VkInstanceCreateInfo *info, const VkAllocationCallbacks *allocator, VkInstance *instance)
{
    void *saved[8];
    const unsigned count = info ? signChain(info->pNext, saved, 8) : 0;
    const VkResult result = gCreateInstance(info, allocator, instance);
    if (info) restoreChain(info->pNext, saved, count);
    return result;
}
static VkResult wrapCreateMessenger(VkInstance instance, const VkDebugUtilsMessengerCreateInfoEXT *info,
                                    const VkAllocationCallbacks *allocator, VkDebugUtilsMessengerEXT *messenger)
{
    if (!info) return gCreateMessenger(instance, info, allocator, messenger);
    VkDebugUtilsMessengerCreateInfoEXT copy = *info;
    if (copy.pfnUserCallback) copy.pfnUserCallback = (PFN_vkDebugUtilsMessengerCallbackEXT)NMB_SIGN(copy.pfnUserCallback);
    return gCreateMessenger(instance, &copy, allocator, messenger);
}
static PFN_vkVoidFunction wrapGetInstanceProcAddr(VkInstance instance, const char *name)
{
    if (!name) return NULL;
    if (!strcmp(name, "vkGetInstanceProcAddr")) return (PFN_vkVoidFunction)wrapGetInstanceProcAddr;
    PFN_vkVoidFunction real = gGetInstanceProcAddr(instance, name);
    if (!real) return NULL;
    if (!strcmp(name, "vkCreateInstance")) {gCreateInstance = (PFN_vkCreateInstance)real; return (PFN_vkVoidFunction)wrapCreateInstance;}
    if (!strcmp(name, "vkCreateDebugUtilsMessengerEXT")) {
        gCreateMessenger = (PFN_vkCreateDebugUtilsMessengerEXT)real;
        return (PFN_vkVoidFunction)wrapCreateMessenger;
    }
    return real;
}
void *nmb_pac_dlsym(void *handle, const char *name)
{
    void *symbol = dlsym(handle, name);
    if (symbol && name && (!strcmp(name, "vkGetInstanceProcAddr") || !strcmp(name, "vk_icdGetInstanceProcAddr"))) {
        gGetInstanceProcAddr = (PFN_vkGetInstanceProcAddr)symbol;
        return (void *)wrapGetInstanceProcAddr;
    }
    return symbol;
}
