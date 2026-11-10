// SPDX-License-Identifier: BSD-2-Clause-Patent
// macOS: what +[IOGPUMetalDevice registerAcceleratorService:] and its clients take from an IOGPUMetalDevice, which the native frontend's
// device (an _MTLDevice) is not. From macOS-DroidVM's macOS Metal plugin (overlay/metal/NativeAdrenoMetalPlugin.mm), on the same frontend
// classes (NativeBridgeMetalFrontend.mm: NBBuffer, NBQueue, ...):
//   - registration hands the device the accelerator's service object (-_setAcceleratorService:, kept like IOGPU's ivar); Metal's wrappers
//     set themselves (-_setDeviceWrapper:, weak; -_deviceWrapper is the device itself without one: _MTLDevice's nil made
//     MTLCopyDeviceForRegistryID answer nil);
//   - -acceleratorPort replaces _MTLDevice's 0: SkyLight composites with Metal only when the first device has a port
//     (WSGetDefaultMetalAccelerator), else in software (CompositorSW);
//   - IOGPU's answers about features this device has not got, its resource-pool housekeeping, queue priorities, per-command-buffer
//     userDictionary (MPS), gpuCoreCount (MPS), bitCodeHash (stitched libraries), and the Metal protocol conformances clients check;
//   - any other IOGPU selector a client sends gets a stub answering 0, named on stderr once per class (a survey, not an abort).
#pragma once
#if TARGET_OS_OSX
#import <Foundation/Foundation.h>
#include <CommonCrypto/CommonDigest.h>
#include <IOKit/IOKitLib.h>
#include <atomic>
#include <objc/runtime.h>
#include <stdio.h>
#include <string.h>

namespace iogpu {

inline io_service_t gAcceleratorPort;
inline id gAcceleratorService;
inline __weak id gDeviceWrapper;

static void setAcceleratorService(id, SEL, id service) { gAcceleratorService = service; }
static io_service_t acceleratorPort(id, SEL) { return gAcceleratorPort; }
static void setDeviceWrapper(id, SEL, id wrapper) { gDeviceWrapper = wrapper; }
static id deviceWrapper(id self, SEL) { id wrapper = gDeviceWrapper; return wrapper ? wrapper : self; }
static BOOL isBuiltIn(id, SEL) { return YES; }
static BOOL no(id, SEL) { return NO; }
static uint64_t zero(id, SEL) { return 0; }
static uint32_t one(id, SEL) { return 1; }
static void nothing(id, SEL) {}
static void nothingWith(id, SEL, uint64_t) {}
static BOOL amplificationCount(id, SEL, NSUInteger count) { return count == 1; }
static uint64_t commandBufferArgumentsSize(id, SEL) { return 48; }   // IOGPU's
// Unified memory: two thirds of RAM, as Metal recommends on Apple GPUs.
static uint64_t workingSetSize(id, SEL) { return NSProcessInfo.processInfo.physicalMemory / 3 * 2; }
static uint64_t stub(id, SEL) { return 0; }

// The frontend's classes and the IOGPU class each stands in for; the device is any other class.
static Class referenceClass(Class cls)
{
    static const struct { const char *frontend, *iogpu; } classes[] = {
        {"NBBuffer", "IOGPUMetalBuffer"}, {"NBTexture", "IOGPUMetalTexture"}, {"NBHeap", "IOGPUMetalHeap"},
        {"NBQueue", "IOGPUMetalCommandQueue"}, {"NBCommandBuffer", "IOGPUMetalCommandBuffer"},
        {"NBComputeEncoder", "IOGPUMetalComputeCommandEncoder"}, {"NBRenderEncoder", "IOGPUMetalRenderCommandEncoder"},
        {"NBBlitEncoder", "IOGPUMetalBlitCommandEncoder"}, {"NBEncoder", "IOGPUMetalCommandEncoder"}, {"NBFence", "IOGPUMetalFence"},
        {"NBSharedEvent", "IOGPUMetalSharedEvent"}, {"NBEvent", "IOGPUMetalEvent"}, {"NBObject", nullptr},
    };
    for (Class c = cls; c; c = class_getSuperclass(c))
        for (const auto &entry : classes)
            if (!strcmp(class_getName(c), entry.frontend))
                return entry.iogpu ? objc_getClass(entry.iogpu) : nil;
    return objc_getClass("IOGPUMetalDevice");
}

// Only the selectors of the IOGPU class the object stands in for: -respondsToSelector: stays NO for everything a real IOGPU object does
// not answer either. Silent when called: WindowServer sends some for every texture of every frame.
static BOOL resolveStub(id cls, SEL, SEL selector)
{
    Class reference = referenceClass((Class)cls);
    Method method = reference ? class_getInstanceMethod(reference, selector) : nullptr;
    if (!method)
        return NO;
    fprintf(stderr, "NativeAdrenoMetal: %s: stubbed %s %s\n", getprogname(), class_getName((Class)cls), sel_getName(selector));
    return class_addMethod((Class)cls, selector, (IMP)stub, method_getTypeEncoding(method));
}

// IOGPUMetalCommandQueue's GPU priorities (MTLGPUPriority 0-5; IOGPU starts a queue at 1, its background priority at 2), kept on the queue.
static char kPriorityKey, kBackgroundPriorityKey;
static NSUInteger priorityOf(id queue, const void *key, NSUInteger initial)
{
    NSNumber *value = objc_getAssociatedObject(queue, key);
    return value ? value.unsignedIntegerValue : initial;
}
static NSUInteger getGPUPriority(id self, SEL) { return priorityOf(self, &kPriorityKey, 1); }
static NSUInteger getBackgroundGPUPriority(id self, SEL) { return priorityOf(self, &kBackgroundPriorityKey, 2); }
static void setPriorities(id self, NSUInteger priority, NSUInteger background)
{
    objc_setAssociatedObject(self, &kPriorityKey, @(priority), OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    objc_setAssociatedObject(self, &kBackgroundPriorityKey, @(background), OBJC_ASSOCIATION_RETAIN_NONATOMIC);
}
static BOOL setGPUPriority(id self, SEL, NSUInteger priority)
{
    setPriorities(self, priority, getBackgroundGPUPriority(self, nullptr));
    return YES;
}
static BOOL setGPUPriorityOffset(id self, SEL, NSUInteger priority, NSUInteger) { return setGPUPriority(self, nullptr, priority); }
static BOOL setBothGPUPriorities(id self, SEL, NSUInteger priority, NSUInteger background)
{
    setPriorities(self, priority, background);
    return YES;
}

// IOGPUMetalCommandBuffer's userDictionary: MPS keeps its per-buffer state in it (a nil one lost it and crashed WindowServer's effects).
static char kUserDictionaryKey;
static id userDictionary(id self, SEL)
{
    @synchronized(self) {
        NSMutableDictionary *dictionary = objc_getAssociatedObject(self, &kUserDictionaryKey);
        if (!dictionary) {
            dictionary = [NSMutableDictionary dictionary];
            objc_setAssociatedObject(self, &kUserDictionaryKey, dictionary, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        }
        return dictionary;
    }
}

// MPSCore's MPSDevice::GetMPSDevice reads the core count for every Apple GPU family; the Adreno's shader processors count as eight.
static NSUInteger gpuCoreCount(id, SEL) { return 8; }

// _MTLFunction's bitCodeHash, a 32-byte digest the stitched-library path hashes: the function's name and a per-process serial.
static char kBitCodeHashKey;
static const void *bitCodeHash(id self, SEL)
{
    @synchronized(self) {
        NSData *hash = objc_getAssociatedObject(self, &kBitCodeHashKey);
        if (!hash) {
            static std::atomic<uint64_t> serial;
            NSString *text = [NSString stringWithFormat:@"%@#%llu", [self valueForKey:@"name"], (unsigned long long)++serial];
            NSData *bytes = [text dataUsingEncoding:NSUTF8StringEncoding];
            NSMutableData *digest = [NSMutableData dataWithLength:CC_SHA256_DIGEST_LENGTH];
            CC_SHA256(bytes.bytes, (CC_LONG)bytes.length, (unsigned char *)digest.mutableBytes);
            hash = digest;
            objc_setAssociatedObject(self, &kBitCodeHashKey, hash, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        }
        return hash.bytes;
    }
}

struct MethodSpec {
    const char *name;
    IMP imp;
    const char *types;
    bool override;   // also over a base class's answer
};

static void addMethods(Class cls, const MethodSpec *methods, size_t count)
{
    for (size_t i = 0; i < count; i++) {
        SEL selector = sel_registerName(methods[i].name);
        if (methods[i].override || ![cls instancesRespondToSelector:selector])
            class_addMethod(cls, selector, methods[i].imp, methods[i].types);
    }
}

// The device the frontend made for this process, made an IOGPUMetalDevice for its clients, on the accelerator port.
static void install(id device, io_service_t port)
{
    IOObjectRetain(port);
    gAcceleratorPort = port;
    static const MethodSpec deviceMethods[] = {
        {"_setAcceleratorService:", (IMP)setAcceleratorService, "v24@0:8@16", false},
        {"acceleratorPort", (IMP)acceleratorPort, "I16@0:8", true},
        {"_setDeviceWrapper:", (IMP)setDeviceWrapper, "v24@0:8@16", true},
        {"_deviceWrapper", (IMP)deviceWrapper, "@16@0:8", true},
        {"isBuiltIn", (IMP)isBuiltIn, "B16@0:8", false},
        {"supportPriorityBand", (IMP)no, "B16@0:8", false},
        {"supportsBackgroundAppRole", (IMP)no, "B16@0:8", false},
        {"isSlotted", (IMP)no, "B16@0:8", false},
        {"supportsResourceDetachBacking", (IMP)no, "B16@0:8", false},
        {"supportsVertexAmplification", (IMP)no, "B16@0:8", false},
        {"supportsVertexAmplificationCount:", (IMP)amplificationCount, "B24@0:8Q16", false},
        {"maxTransferRate", (IMP)zero, "Q16@0:8", false},
        {"location", (IMP)zero, "Q16@0:8", false},
        {"locationNumber", (IMP)zero, "Q16@0:8", false},
        {"peerGroupID", (IMP)zero, "Q16@0:8", false},
        {"peerIndex", (IMP)zero, "I16@0:8", false},
        {"peerCount", (IMP)one, "I16@0:8", false},
        {"dedicatedMemorySize", (IMP)zero, "Q16@0:8", false},
        {"hwResourcePoolCount", (IMP)zero, "I16@0:8", false},
        {"cmdBufArgsSize", (IMP)commandBufferArgumentsSize, "Q16@0:8", false},
        {"kickCleanupQueue", (IMP)nothing, "v16@0:8", false},
        {"updateGPUSelectionProperties", (IMP)nothing, "v16@0:8", false},
        {"updateResourcePoolPurgeability", (IMP)nothing, "v16@0:8", false},
        {"periodicUpdateResourcePoolPurgeability", (IMP)nothing, "v16@0:8", false},
        {"cancelPeriodicUpdateResourcePoolPurgeability", (IMP)nothing, "v16@0:8", false},
        {"recommendedMaxWorkingSetSize", (IMP)workingSetSize, "Q16@0:8", false},
        {"sharedMemorySize", (IMP)workingSetSize, "Q16@0:8", false},
        {"currentAllocatedSize", (IMP)zero, "Q16@0:8", false},
        {"gpuCoreCount", (IMP)gpuCoreCount, "Q16@0:8", false},
        // _MTLGetMTLCompilerLLVMVersionForDevice reads it for a stitched library: below 8 it takes the default compiler version.
        {"featureProfile", (IMP)zero, "Q16@0:8", false},
    };
    Class cls = object_getClass(device);
    addMethods(cls, deviceMethods, sizeof deviceMethods / sizeof *deviceMethods);
    class_addMethod(object_getClass(cls), @selector(resolveInstanceMethod:), (IMP)resolveStub, "B24@0:8:16");
    static const MethodSpec queueMethods[] = {
        {"getGPUPriority", (IMP)getGPUPriority, "Q16@0:8", false},
        {"getBackgroundGPUPriority", (IMP)getBackgroundGPUPriority, "Q16@0:8", false},
        {"setGPUPriority:", (IMP)setGPUPriority, "B24@0:8Q16", false},
        {"setGPUPriority:offset:", (IMP)setGPUPriorityOffset, "B32@0:8Q16Q24", false},
        {"_setGPUPriority:backgroundPriority:", (IMP)setBothGPUPriorities, "B32@0:8Q16Q24", false},
    };
    if (Class queue = objc_getClass("NBQueue"))
        addMethods(queue, queueMethods, sizeof queueMethods / sizeof *queueMethods);
    // IOGPUMetalResource's: the process charged for a resource's memory (one pool, charged to nobody) and aliasing (a heap's placement
    // never reuses a live resource's range).
    static const MethodSpec resourceMethods[] = {
        {"setResponsibleProcess:", (IMP)nothingWith, "v20@0:8i16", false},
        {"makeAliasable", (IMP)nothing, "v16@0:8", false},
    };
    for (const char *name : {"NBBuffer", "NBTexture"})
        if (Class resource = objc_getClass(name))
            addMethods(resource, resourceMethods, sizeof resourceMethods / sizeof *resourceMethods);
    static const MethodSpec commandBufferMethods[] = {{"userDictionary", (IMP)userDictionary, "@16@0:8", true}};
    static const MethodSpec functionMethods[] = {{"bitCodeHash", (IMP)bitCodeHash, "^v16@0:8", false}};
    if (Class function = objc_getClass("NBFunction"))
        addMethods(function, functionMethods, 1);
    if (Class commandBuffer = objc_getClass("NBCommandBuffer"))
        addMethods(commandBuffer, commandBufferMethods, 1);
    // The frontend's classes declare no Metal protocols, and clients check them (MPSCommandBufferImageCache returns nil for a command
    // buffer that does not conform to MTLCommandBuffer).
    static const char *const conformances[][2] = {
        {"NBBuffer", "MTLBuffer"}, {"NBTexture", "MTLTexture"}, {"NBLibrary", "MTLLibrary"}, {"NBFunction", "MTLFunction"},
        {"NBComputePipeline", "MTLComputePipelineState"}, {"NBRenderPipeline", "MTLRenderPipelineState"},
        {"NBSampler", "MTLSamplerState"}, {"NBDepthStencil", "MTLDepthStencilState"}, {"NBEvent", "MTLEvent"},
        {"NBSharedEvent", "MTLSharedEvent"}, {"NBFence", "MTLFence"}, {"NBQueue", "MTLCommandQueue"},
        {"NBCommandBuffer", "MTLCommandBuffer"}, {"NBEncoder", "MTLCommandEncoder"}, {"NBComputeEncoder", "MTLComputeCommandEncoder"},
        {"NBRenderEncoder", "MTLRenderCommandEncoder"}, {"NBBlitEncoder", "MTLBlitCommandEncoder"}, {"NBHeap", "MTLHeap"},
        {"NBBinaryArchive", "MTLBinaryArchive"},
    };
    for (const auto &conformance : conformances) {
        Class frontend = objc_getClass(conformance[0]);
        Protocol *protocol = objc_getProtocol(conformance[1]);
        if (frontend && protocol)
            class_addProtocol(frontend, protocol);
        else
            fprintf(stderr, "NativeAdrenoMetal: %s: no %s for %s\n", getprogname(), conformance[1], conformance[0]);
    }
    if (Class object = objc_getClass("NBObject"))
        class_addMethod(object_getClass(object), @selector(resolveInstanceMethod:), (IMP)resolveStub, "B24@0:8:16");
}

}  // namespace iogpu
#endif  // TARGET_OS_OSX
