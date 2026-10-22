// SPDX-License-Identifier: MIT
// Adreno Metal plugin, reached through the original discovery (IOAcceleratorES -> MetalPluginName -> NSBundle -> principal class ->
// initWithAcceleratorPort:). Default (stage 1): it runs the shared userspace GPU test (NativeAdrenoUserTest.h) from inside a real system
// process, under that process's sandbox, and then returns nil. No device is registered, so the original software rendering keeps running unchanged.
// Published (the image carries the file NativeAdrenoMetal.publish beside the bundle): backboardd, the one process that owns the GPU, returns the native
// Metal frontend's device (the metal2vulkan runtime on Turnip, on the Adreno 740), which the original discovery registers; the CARenderServer then
// renders the system UI through it, and the display driver's frames are written to the scanout by the GPU (GpuScanoutClient.h). Another process
// gets its own native device (its own driver connection, GPU address window and shader cache) when its name is listed in the file
// NativeAdrenoMetal.clients beside the bundle ("*" lists every process); an unlisted one returns nil and keeps the software renderer.
#import <Foundation/Foundation.h>
#include <pthread.h>
#include <dlfcn.h>
#include <unistd.h>
#include <pwd.h>
#include <mach/mach.h>
#include <objc/message.h>
#include "NativeAdrenoUserTest.h"
#include "NativeMetalBridgePhase.h"
#include "CacheExport.h"
#include "PrimeShaders.h"
#include "ExceptionDiag.h"

@interface _MTLDevice : NSObject
@end
@interface NativeAdrenoMetalDeviceBase : NSObject   // only a lock object for the published mode's one-time setup
@end
@implementation NativeAdrenoMetalDeviceBase
@end
@interface NativeAdrenoMetalDevice : _MTLDevice
- (id)initWithAcceleratorPort:(unsigned)port;
@end

// ---- published mode ----
namespace publish {
// The image's marker: a file beside the bundle in the union view of /System/Library/Extensions (the bundle's own seal stays untouched).
// A file beside the bundle: its path in path, true when it is readable.
static bool requestedPath(const char *name, char *path, size_t size) {
    Dl_info info = {};
    if (!dladdr(reinterpret_cast<const void *>(&requestedPath), &info) || !info.dli_fname) return false;
    snprintf(path, size, "%s", info.dli_fname);
    char *slash = strrchr(path, '/');          // the bundle's executable: <dir>/NativeAdrenoMetal.bundle/NativeAdrenoMetal
    if (!slash) return false;
    *slash = 0;
    slash = strrchr(path, '/');
    if (!slash) return false;
    snprintf(slash + 1, size - size_t(slash + 1 - path), "%s", name);
    return access(path, R_OK) == 0;
}
static bool requested() {
    char path[1024];
    return requestedPath("NativeAdrenoMetal.publish", path, sizeof path);
}
static Gpu *makeGpu(unsigned port) {
    void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL);
    if (!iokit) return nullptr;
    Gpu *gpu = new Gpu;
    gpu->call = (CallFn)dlsym(iokit, "IOConnectCallStructMethod");
    gpu->map = (MapFn)dlsym(iokit, "IOConnectMapMemory64");
    gpu->unmap = (UnmapFn)dlsym(iokit, "IOConnectUnmapMemory64");
    gpu->openFn = (IOServiceOpenFn)dlsym(iokit, "IOServiceOpen");
    gpu->closeFn = (IOServiceCloseFn)dlsym(iokit, "IOServiceClose");
    gpu->port = port;
    return gpu;
}
static std::string gCacheDirectory;   // the runtime's cache directory (the one that opened the device), exported by CacheExport.h
// Progress markers 0x68xx (GPU6): 6800 entered, 6801 Turnip found, 6802 device made, 6803 returned to the discovery, 68f1 no Turnip, 68f2 no device.
static unsigned gDiagBits;
// Reads of the render server's own state for the framebuffer-format question (diagnostic only, for the exact QuartzCore of the target image; a read
// that fails or a build that differs reports zeros). Each value is one marker, 0x7Kvv: K picks the item, vv the value.
static bool readTask(uintptr_t address, void *out, size_t size) {
    vm_size_t got = 0;
    return vm_read_overwrite(mach_task_self(), address, size, (vm_address_t) out, &got) == KERN_SUCCESS && got == size;
}
static bool writeTask(uintptr_t address, const void *data, size_t size) {
    return vm_write(mach_task_self(), address, (vm_offset_t) data, (mach_msg_type_number_t) size) == KERN_SUCCESS;
}
// EXPERIMENT (second report onward): this display path takes plain 8-bit BGRA only, so tell the render server's display that it has neither the deep
// (10-bit) framebuffer nor universal buffer compression. Whether the state it reads those from is the whole story is what the markers show.
static unsigned gReports;
struct DisplaySample {unsigned flags = 0, mode = 0, behaviour = 0, globals = 0; uint16_t display = 0; uintptr_t object = 0;};
static DisplaySample sampleDisplay(id first, intptr_t slide) {
    DisplaySample out;
    uintptr_t a = first ? (uintptr_t) *(void **) ((char *) (__bridge void *) first + 8) : 0, b = 0, d = 0;
    if (a && readTask(a + 0x40, &b, 8) && b && readTask(b + 0x60, &d, 8) && d) {
        uint32_t f = 0; uint64_t m = 0; uint8_t byte[4] = {};
        readTask(d + 0x238, &f, 4);
        readTask(d + 0x258, &out.display, 2);
        readTask(d + 0x248, &m, 8);
        readTask(d + 0x6c17, &byte[0], 1);
        readTask(d + 0x6658 + 0x57d, &byte[1], 1);
        readTask(d + 0x6658 + 0x58a, &byte[2], 1);
        readTask(d + 0x6658 + 0x59b, &byte[3], 1);
        out.object = d;
        out.flags = f & 0xff;
        out.mode = (unsigned) ((m >> 55) & 31) | (m ? 0x20 : 0);
        out.behaviour = (byte[0] ? 1 : 0) | (byte[1] ? 2 : 0) | (byte[2] ? 4 : 0) | (byte[3] ? 8 : 0);
    }
    uint8_t v[4] = {};
    readTask(0x1f491ae10 + slide, &v[0], 1);    // the render server's deep-framebuffer flag
    readTask(0x1ed7abf10 + slide, &v[1], 1);    // framebuffer compression in effect
    readTask(0x1f4919dc0 + slide, &v[2], 1);    // its default
    readTask(0x1f4919c20 + slide, &v[3], 1);    // universal buffer compression
    out.globals = (v[0] ? 1 : 0) | (v[1] ? 2 : 0) | (v[2] ? 4 : 0) | (v[3] ? 8 : 0);
    return out;
}
static void markSample(Gpu &gpu, const DisplaySample &x, unsigned base) {
    gpu.mark(base | (x.flags << 4) | x.globals);                  // display flags (bit 3 deep), globals: deep/compression/default/universal
    gpu.mark((base + 0x1000) | (x.display >> 4));                 // display limits word: 0x200 compression support, 0x1c00 type
    gpu.mark((base + 0x2000) | (x.mode << 4) | x.behaviour);     // mode colour field, ICC/other behaviour bytes
}
static void reportDisplayState(Gpu &gpu) {
    ++gReports;
    static DisplaySample before;
    static bool changed;
    Class serverClass = NSClassFromString(@"CAWindowServer");
    void *start = dlsym(RTLD_DEFAULT, "CARenderServerStart");
    if (!serverClass || !start) return;
    const intptr_t slide = (intptr_t) start - 0x189651efc;
    @try {
        id server = ((id (*)(id, SEL)) objc_msgSend)(serverClass, NSSelectorFromString(@"serverIfRunning"));
        NSArray *displays = server ? ((id (*)(id, SEL)) objc_msgSend)(server, NSSelectorFromString(@"displays")) : nil;
        id first = displays.count ? displays[0] : nil;
        DisplaySample now = sampleDisplay(first, slide);
        if (!changed) {
            before = now;
            if (gReports >= 2 && now.object) {
                uint32_t f = 0;
                if (readTask(now.object + 0x238, &f, 4) && (f & 8)) {f &= ~8u; writeTask(now.object + 0x238, &f, 4);}
                uint8_t zero = 0;
                writeTask(0x1f4919c20 + slide, &zero, 1);
                changed = true;
                now = sampleDisplay(first, slide);
            }
        }
        markSample(gpu, before, 0x7000);
        markSample(gpu, now, 0xA000);
    } @catch (NSException *) {}
}
// Whether NativeAdrenoMetal.clients names this process (names separated by blanks, commas or new lines; "*" names every process).
static bool listedClient() {
    char path[1024];
    if (!requestedPath("NativeAdrenoMetal.clients", path, sizeof path)) return false;
    FILE *file = fopen(path, "r");
    if (!file) return false;
    char text[1024] = {};
    const size_t length = fread(text, 1, sizeof text - 1, file);
    fclose(file);
    text[length] = 0;
    const char *self = getprogname();
    for (char *save = nullptr, *name = strtok_r(text, " ,\t\r\n", &save); name; name = strtok_r(nullptr, " ,\t\r\n", &save))
        if (!strcmp(name, "*") || !strcmp(name, self)) return true;
    return false;
}
// A listed process other than the render server: the same runtime on its own connection, with none of the render server's threads (no
// display hand-off, telemetry, benchmark or cache export). Its shader cache lives in its own cache or temp directory.
// The reason a client's device failed, as a text line the driver keeps per process (the render server's report row is rewritten every ten seconds):
// "<step> <detail>", through the marker stream (a connection is not needed). The Gpu object is made only on this path.
static void clientFailure(unsigned port, const char *step, const char *detail) {
    if (Gpu *gpu = makeGpu(port)) {
        char text[64];
        snprintf(text, sizeof text, "%.14s %.46s", step, detail ? detail : "");
        streamText(*gpu, text);
    }
}
static id clientDevice(unsigned port) {
    if (!listedClient()) return nil;
    char turnip[1024];
    if (!turnip::libraryPath(turnip, sizeof turnip) || access(turnip, R_OK) != 0) {clientFailure(port, "no-turnip", ""); return nil;}
    uint64_t registryID = 0;
    if (void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL)) {
        if (auto retain = (int (*)(unsigned))dlsym(iokit, "IOObjectRetain")) retain(port);
        if (auto entryID = (int (*)(unsigned, uint64_t *))dlsym(iokit, "IORegistryEntryGetRegistryEntryID")) entryID(port, &registryID);
    }
    // IOSurfaces this GPU reads or writes must be plain rows, as in the render server.
    CFPreferencesSetAppValue(CFSTR("CAEnableIOSurfaceCompression"), kCFBooleanFalse, kCFPreferencesCurrentApplication);
    char candidates[2][512] = {};
    if (!confstr(_CS_DARWIN_USER_CACHE_DIR, candidates[0], sizeof candidates[0] - 16)) candidates[0][0] = 0;
    if (!confstr(_CS_DARWIN_USER_TEMP_DIR, candidates[1], sizeof candidates[1] - 16)) candidates[1][0] = 0;
    id device = nil;
    const char *lastError = "";
    unsigned usable = 0;
    for (unsigned i = 0; i < 2 && !device; ++i) {
        if (!candidates[i][0]) continue;
        strlcat(candidates[i], "m2v", sizeof candidates[i]);
        mkdir(candidates[i], 0700);
        if (access(candidates[i], W_OK) != 0) continue;
        ++usable;
        @try {
            device = native_bridge_metal_open(turnip, candidates[i], registryID, nullptr);
        } @catch (NSException *exception) {
            device = nil;
            lastError = "exception";
        }
        if (!device && !lastError[0]) lastError = nmb_last_error();
    }
    if (!device) clientFailure(port, usable ? "no-device" : "no-cachedir", lastError);
    return device;
}
static id deviceForPort(unsigned port) {
    static id device;
    static bool tried;
    @synchronized([NativeAdrenoMetalDeviceBase class]) {
        if (tried) return device;
        tried = true;
        exceptiondiag::start(); // the throw site of an uncaught exception, for the kernel's THROW row (ExceptionDiag.h)
        if (strcmp(getprogname(), "backboardd") != 0) return device = clientDevice(port);
        // The discovery releases the accelerator port when it is done; the monitor and the scanout client keep using it.
        if (void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL)) {
            if (auto retain = (int (*)(unsigned))dlsym(iokit, "IOObjectRetain")) retain(port);
        }
        Gpu *gpu = makeGpu(port);
        if (!gpu) return nil;
        gpu->mark(0x6800);
        char turnip[1024];
        if (!turnip::libraryPath(turnip, sizeof turnip) || access(turnip, R_OK) != 0) {gpu->mark(0x68f1); return nil;}
        gpu->mark(0x6801);
        // The runtime refuses to open without a writable shader-cache directory, and which directories backboardd's sandbox lets it write is not
        // known from here: try the user cache and temp directories, then /tmp and /var/tmp, and use the first that opens a device.
        uint64_t registryID = 0;
        if (void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL)) {
            if (auto entryID = (int (*)(unsigned, uint64_t *))dlsym(iokit, "IORegistryEntryGetRegistryEntryID")) entryID(port, &registryID);
        }
        // What the render server sees for its framebuffer choice (marker 0x6Axx): bit0 the deep-framebuffer preference is set, bit1 it is false,
        // bit2 MobileGestalt's answer behind the render server's deep flag is true, bit3 it exists, bit4 the compression capability dictionary exists,
        // bit5 the process is not root.
        {
            unsigned bits = 0;
            if (CFTypeRef value = CFPreferencesCopyAppValue(CFSTR("CAEnableDeepFramebuffer"), kCFPreferencesCurrentApplication)) {
                bits |= 1;
                if (CFGetTypeID(value) == CFBooleanGetTypeID() && !CFBooleanGetValue((CFBooleanRef) value)) bits |= 2;
                CFRelease(value);
            }
            if (auto copyAnswer = (CFTypeRef (*)(CFStringRef)) dlsym(RTLD_DEFAULT, "MGCopyAnswer")) {
                if (CFTypeRef deep = copyAnswer(CFSTR("Aixt/MEN2O2B7f+8m4TxUA"))) {
                    bits |= 8;
                    if (CFGetTypeID(deep) == CFBooleanGetTypeID() && CFBooleanGetValue((CFBooleanRef) deep)) bits |= 4;
                    CFRelease(deep);
                }
                if (CFTypeRef caps = copyAnswer(CFSTR("AoKnINTLPoKML3ctoP0AZg"))) {bits |= 16; CFRelease(caps);}
            }
            if (getuid() != 0) bits |= 32;
            gDiagBits = bits;
            gpu->mark(0x6A00 | bits);
        }
        // The render server compresses its framebuffer (an Apple/AGX layout this GPU does not write) unless told not to; the display driver takes plain rows.
        setenv("CA_DISABLE_FRAMEBUFFER_COMPRESSION", "1", 1);
        // The runtime's creation log reports each VkPipeline making's time to the device's sink, which the telemetry's V row sums.
        setenv("METAL2VULKAN_LOG_PIPELINES", "1", 0);
        CFPreferencesSetAppValue(CFSTR("CAEnableFramebufferCompression"), kCFBooleanFalse, kCFPreferencesCurrentApplication);
        CFPreferencesSetAppValue(CFSTR("CAEnableIOSurfaceCompression"), kCFBooleanFalse, kCFPreferencesCurrentApplication);
        char candidates[5][512] = {};
        if (!confstr(_CS_DARWIN_USER_CACHE_DIR, candidates[0], sizeof candidates[0] - 16)) candidates[0][0] = 0;
        if (!confstr(_CS_DARWIN_USER_TEMP_DIR, candidates[1], sizeof candidates[1] - 16)) candidates[1][0] = 0;
        strlcpy(candidates[2], "/tmp/", sizeof candidates[2]);
        strlcpy(candidates[3], "/private/var/tmp/", sizeof candidates[3]);
        // The user temp directory of the mobile user is under /var/mobile/tmp, which launchd's boot task dirs_cleaner empties at every boot,
        // so with a persistent Data store the pipelines were still compiled again each boot (V row hit0). Try the user's Library/Caches first
        // (marker 0x6814); the earlier candidates keep their markers.
        if (const passwd *user = getpwuid(getuid()); user && user->pw_dir && user->pw_dir[0]) {
            snprintf(candidates[4], sizeof candidates[4] - 16, "%s/Library/Caches/com.apple.backboardd", user->pw_dir);
            mkdir(candidates[4], 0700);
            strlcat(candidates[4], "/", sizeof candidates[4]);
        }
        static const unsigned order[] = {4, 0, 1, 2, 3};
        for (unsigned i : order) {
            if (device) break;
            if (!candidates[i][0]) continue;
            strlcat(candidates[i], "m2v", sizeof candidates[i]);
            mkdir(candidates[i], 0700);
            if (access(candidates[i], W_OK) != 0) continue;
            // The image's pipeline pack (an earlier boot's export), into this directory before the runtime reads it. Marker 0x6Cnn: files written
            // (saturated at 0xfe), 0x6Cff the pack was refused.
            if (char pack[1024]; requestedPath("NativeAdrenoMetal.m2vpack", pack, sizeof pack)) {
                const int imported = cacheexport::importPack(pack, candidates[i]);
                if (imported) gpu->mark(0x6C00 | (imported < 0 ? 0xff : imported > 0xfe ? 0xfe : imported));
            }
            @try {
                device = native_bridge_metal_open(turnip, candidates[i], registryID, nullptr);
            } @catch (NSException *exception) {
                device = nil;
            }
            if (device) {gpu->mark(0x6810 + i); gCacheDirectory = candidates[i];}   // which directory served
        }
        if (!device) {
            gpu->mark(0x68f2);
            char text[64];
            snprintf(text, sizeof text, "PUB no device: %.40s", nmb_last_error());
            streamText(*gpu, text);
            return nil;
        }
        gpu->mark(0x6802);
        // The status line (GPU8) every ten seconds, and the display hand-off client on its own thread: both only read or serve, neither is the discovery's.
        pthread_t monitor, scanout;
        pthread_attr_t attributes;
        pthread_attr_init(&attributes);
        pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED);
        pthread_attr_setstacksize(&attributes, 1u << 20);
        pthread_create(&monitor, &attributes, [](void *arg) -> void * {
            Gpu *g = static_cast<Gpu *>(arg);
            for (;;) {
                sleep(10);
                char status[64], text[64];
                static unsigned phase;
                // status (counters, then the last two refusals' texts), formats; the stream carries 63 characters per line, so a long status goes
                // out in 57-character pieces on consecutive lines (the UFS log keeps every line).
                char full[420];
                native_bridge_metal_status(full, sizeof full);
                const size_t length = strlen(full);
                const unsigned pieces = (unsigned)((length + 56) / 57);
                const unsigned step = phase++ % (1 + (pieces ? (pieces > 7 ? 7 : pieces) : 1));
                if (step == 0) {
                    native_bridge_metal_formats(status, sizeof status);
                    snprintf(text, sizeof text, "PUB %.58s", status);
                } else {
                    snprintf(text, sizeof text, "ST%u %.57s", step - 1, full + (size_t)(step - 1) * 57);
                }
                streamText(*g, text);
                reportDisplayState(*g);
                g->mark(0x6A00 | gDiagBits);   // the diagnostics page lists the last few markers only
            }
            return nullptr;
        }, gpu);
        pthread_t telemetry;
        pthread_create(&telemetry, &attributes, [](void *arg) -> void * {
            Gpu *g = static_cast<Gpu *>(arg);
            sleep(5);
            // The bridge's telemetry table, resent whole every four seconds as markers (two characters each): 0xC000|row starts a row of the
            // kernel's table, 0x8000|(a<<7)|b appends two characters. Rows are at most 90 characters; 48 rows.
            for (;;) {
                static char table[6144];
                native_bridge_metal_telemetry(table, sizeof table);
                unsigned row = 0;
                for (char *line = table; *line && row < 48;) {
                    char *end = strchr(line, '\n');
                    if (end) *end = 0;
                    g->mark(0xC000u | row);
                    const size_t length = strlen(line) < 90 ? strlen(line) : 90;
                    for (size_t i = 0; i < length; i += 2) {
                        const unsigned a = (unsigned char)line[i] & 0x7f, b = i + 1 < length ? (unsigned char)line[i + 1] & 0x7f : 0;
                        g->mark(0x8000u | (a << 7) | b);
                    }
                    ++row;
                    if (!end) break;
                    line = end + 1;
                }
                sleep(6);
            }
            return nullptr;
        }, gpu);
        pthread_create(&scanout, &attributes, [](void *arg) -> void * {
            Gpu *g = static_cast<Gpu *>(arg);
            sleep(3);
            @try {
                bridgephase::serveScanout(*g, device);
            } @catch (NSException *exception) {
                g->mark(0x68f3);
            }
            return nullptr;
        }, gpu);
        pthread_t exporter;
        pthread_create(&exporter, &attributes, [](void *arg) -> void * {
            cacheexport::run(*static_cast<Gpu *>(arg), gCacheDirectory);
            return nullptr;
        }, gpu);
        // Every Metal library of the OS through the runtime, 90 s after the render server opened the device (PrimeShaders.h); the boots that froze
        // or crashed before three minutes never primed anything. Progress is kept in the Data cache directory, so later boots resume.
        // The translator recurses deeply on large functions: give it the stack of a main thread.
        pthread_attr_setstacksize(&attributes, 8u << 20);
        pthread_t primer;
        pthread_create(&primer, &attributes, [](void *) -> void * {
            char list[1024];
            prime::run((id<MTLDevice>)device, gCacheDirectory, 90,
                       requestedPath("NativeAdrenoMetal.primelist", list, sizeof list) ? std::string(list) : std::string());
            return nullptr;
        }, nullptr);
        pthread_attr_destroy(&attributes);
        gpu->mark(0x6803);
        return device;
    }
}
}  // namespace publish

@implementation NativeAdrenoMetalDevice
- (id)initWithAcceleratorPort:(unsigned)port {
    self = [super init];
    if (!self || !port) return nil;
    if (publish::requested()) return publish::deviceForPort(port);
    static bool attempted;
    if (attempted) return nil;
    attempted = true;
    // The test runs on its own thread: a kernel call that never returns (int-042: BoFree) then stalls that thread only, not SpringBoard's
    // main thread (the UI froze with the test running inline).
    // The discovery code releases the accelerator port as soon as this returns nil, which is before the test thread has started: the first
    // marker (and every later open) then went to a dead name and never reached the kernel (K0 in int-043..052, with the plugin file opened by
    // SpringBoard). Keep one extra user reference on the port, and let the thread look the service up itself as well.
    if (void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL)) {
        if (auto retain = (int (*)(unsigned))dlsym(iokit, "IOObjectRetain")) retain(port);
    }
    struct Job {unsigned port; char name[32];};
    Job *job = new Job{port, {}};
    strlcpy(job->name, getprogname(), sizeof job->name);
    pthread_t thread;
    pthread_attr_t attributes;
    pthread_attr_init(&attributes);
    pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED);
    if (pthread_create(&thread, &attributes, [](void *arg) -> void * {
            Job *j = static_cast<Job *>(arg);
            unsigned port = j->port;
            if (void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL)) {
                auto matching = (void *(*)(const char *))dlsym(iokit, "IOServiceMatching");
                auto getService = (unsigned (*)(unsigned, void *))dlsym(iokit, "IOServiceGetMatchingService");
                if (matching && getService) {
                    const unsigned own = getService(0, matching("IOAcceleratorES"));
                    if (own) port = own;
                }
            }
            gAfterTurnip = bridgephase::run;
            adrenoUserTest(port, j->name);
            delete j;
            return nullptr;
        }, job) != 0) delete job;
    pthread_attr_destroy(&attributes);
    return nil;
}
@end
