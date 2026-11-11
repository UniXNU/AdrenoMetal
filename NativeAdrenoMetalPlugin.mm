// SPDX-License-Identifier: MIT
// Adreno Metal plugin, reached through the original discovery (IOAcceleratorES -> MetalPluginName -> NSBundle -> principal class ->
// initWithAcceleratorPort:). Default (stage 1): it runs the shared userspace GPU test (NativeAdrenoUserTest.h) from inside a real system
// process, under that process's sandbox, and then returns nil. No device is registered, so the original software rendering keeps running unchanged.
// Published (the image carries the file NativeAdrenoMetal.publish beside the bundle): backboardd, the one process that owns the GPU, returns the native
// Metal frontend's device (the metal2vulkan runtime on Turnip, on the Adreno 740), which the original discovery registers; the CARenderServer then
// renders the system UI through it, and the display driver's frames are written to the scanout by the GPU (GpuScanoutClient.h). Another process
// gets its own native device too (its own driver connection, GPU address space and shader cache), as every process does with Apple's
// own GPU plugin.
#import <Foundation/Foundation.h>
#include <string>
#include <sys/stat.h>
#include <dirent.h>
#include <sys/mount.h>
#include <os/log.h>
#include "NativeRenderServer.h"
#include "SharedShaderCache.h"
#include <pthread.h>
#include <thread>
#include <dlfcn.h>
#if __has_feature(ptrauth_calls)
#include <ptrauth.h>
#endif
#include <unistd.h>
#include <pwd.h>
#include <fts.h>
#include <mach/mach.h>
#include <sys/sysctl.h>
#include <libkern/OSCacheControl.h>
#include <objc/message.h>
#include "NativeAdrenoUserTest.h"
#include "NativeTurnipSwapTest.h"
#include "NativeGpuTrace.h"
#include "NativeMetalBridgePhase.h"
#include "CacheExport.h"
#include "PrimeShaders.h"
#include "ExceptionDiag.h"

#if TARGET_OS_OSX
#include "IOGPUMetalDeviceShim.h"
// macOS: +[IOGPUMetalDevice registerAcceleratorService:] takes the bundle's MetalPluginClassName only if it subclasses IOGPUMetalDevice.
@interface IOGPUMetalDevice : NSObject
@end
#else
@interface _MTLDevice : NSObject
@end
#endif
@interface NativeAdrenoMetalDeviceBase : NSObject   // only a lock object for the published mode's one-time setup
@end
@implementation NativeAdrenoMetalDeviceBase
@end
#if TARGET_OS_OSX
@interface NativeAdrenoMetalDevice : IOGPUMetalDevice
#else
@interface NativeAdrenoMetalDevice : _MTLDevice
#endif
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
    // The bundle's executable: <dir>/NativeAdrenoMetal.bundle/NativeAdrenoMetal (iOS) or
    // <dir>/NativeAdrenoMetal.bundle/Contents/MacOS/NativeAdrenoMetal (macOS).
    char *bundle = strstr(path, "/NativeAdrenoMetal.bundle/");
    if (!bundle) return false;
    snprintf(bundle + 1, size - size_t(bundle + 1 - path), "%s", name);
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
static bool gFillsSystemCache;        // the render server opened its device in the system shader cache's directory (SharedShaderCache.h)
// Progress markers 0x68xx (GPU6): 6800 entered, 6801 Turnip found, 6802 device made, 6803 returned to the discovery, 68f1 no Turnip, 68f2 no device.
static unsigned gDiagBits;
// Reads of the render server's own state for the framebuffer-format question (diagnostic only, for the exact QuartzCore of the target image; a read
// that fails or a build that differs reports zeros). Each value is one marker, 0x7Kvv: K picks the item, vv the value.
static bool readTask(uintptr_t address, void *out, size_t size) {
    vm_size_t got = 0;
    return vm_read_overwrite(mach_task_self(), address, size, (vm_address_t) out, &got) == KERN_SUCCESS && got == size;
}
[[maybe_unused]] static bool writeTask(uintptr_t address, const void *data, size_t size) {   // iPadOS only (24A446 QuartzCore)
    return vm_write(mach_task_self(), address, (vm_offset_t) data, (mach_msg_type_number_t) size) == KERN_SUCCESS;
}
// EXPERIMENT (second report onward): this display path takes plain 8-bit BGRA only, so tell the render server's display that it has neither the deep
// (10-bit) framebuffer nor universal buffer compression. Whether the state it reads those from is the whole story is what the markers show.
// EXPERIMENT (boot-arg fb-bgra=1): the render server's framebuffer format with every other virtualized answer left as it is.
// framebuffer_pixel_format (24A446 0x184461838) takes RGhA on `tbnz w8, #0` at 0x184461928 when CADeviceIsVirtualized (byte 0x1e701bdd9) is 1;
// 40 other places in QuartzCore read the same byte (XR formats, backdrop compute, ASTC ...). Holding the byte at 0 turned all of them, and the
// tablet drew light blue blocks. Here the byte stays 1 and only this branch is made a NOP, so the framebuffer is chosen as on a real device and
// nothing else changes. Marks 0x6E40 unreadable, 0x6E41 other instruction, 0x6E42 patched, 0x6E50|stage failures.
// The boot argument token that starts with prefix (a whole name, or name=value), or nullptr: a name is matched only at the start of the
// command line or after a space, so "log=1" does not find "cache-log=1".
static const char *bootArgToken(const char *args, const char *prefix) {
    const size_t length = strlen(prefix);
    for (const char *at = args; (at = strstr(at, prefix)); at += length)
        if (at == args || at[-1] == ' ') return at;
    return nullptr;
}
static bool bootArgSet(const char *key) {
    char args[1100] = {};
    size_t size = sizeof args - 1;
    if (sysctlbyname("kern.bootargs", args, &size, nullptr, 0) != 0) return false;
    const char *at = bootArgToken(args, key);
    const size_t length = strlen(key);
    return at && (at[length] == 0 || at[length] == ' ');
}
// `site` is the slid address of that branch in the running QuartzCore; any other word there is left alone (0x6E41).
[[maybe_unused]] static unsigned patchFramebufferBranch(uintptr_t site) {
    uint32_t word = 0;
    if (!readTask(site, &word, sizeof word)) return 0x6E40;
    if (word == 0xd503201f) return 0x6E42;
    if (word != 0x37000608) return 0x6E41;
    const vm_address_t page = site & ~(uintptr_t)(vm_page_size - 1);
    if (vm_protect(mach_task_self(), page, vm_page_size, FALSE, VM_PROT_READ | VM_PROT_WRITE | VM_PROT_COPY) != KERN_SUCCESS) return 0x6E51;
    const uint32_t nop = 0xd503201f;
    const bool wrote = vm_write(mach_task_self(), site, (vm_offset_t)&nop, sizeof nop) == KERN_SUCCESS;
    const bool restored = vm_protect(mach_task_self(), page, vm_page_size, FALSE, VM_PROT_READ | VM_PROT_EXECUTE) == KERN_SUCCESS;
    sys_icache_invalidate((void *)site, sizeof nop);
    if (!wrote) return 0x6E52;
    if (!restored) return 0x6E53;
    return 0x6E42;
}
// Every process other than the render server: the same runtime on its own connection, with none of the render server's threads (no
// display hand-off, telemetry, benchmark or cache export). Its shader cache lives in its own cache or temp directory.
// The reason a client's device failed, as a text line the driver keeps per process (the render server's report row is rewritten every ten seconds):
// "<step> <detail>", through the marker stream (a connection is not needed). The Gpu object is made only on this path.
// Deletes every "*.import" file under `root`: the copying import of an earlier image wrote each file beside its place and renamed it only
// once the whole pack was read; a boot that ended first left them all there.
static void removeHalfImported(const char *root) {
    char *paths[] = {const_cast<char *>(root), nullptr};
    FTS *fts = fts_open(paths, FTS_PHYSICAL | FTS_NOCHDIR | FTS_XDEV, nullptr);
    if (!fts) return;
    while (FTSENT *e = fts_read(fts))
        if (e->fts_info == FTS_F && e->fts_namelen > 7 && !strcmp(e->fts_name + e->fts_namelen - 7, ".import")) unlink(e->fts_path);
    fts_close(fts);
}
// diag=1 in the render server: the bridge's telemetry table (native_bridge_metal_telemetry) to the kernel's board log every six
// seconds and to the system log every thirty. The kernel keeps 48 rows of at most 95 characters (its [metal table] section): the
// marker 0xC000 | row starts a row, 0x8000 | a << 7 | b appends two characters. A row that reads as it did when last sent is not
// sent again, and a row the table no longer has is cleared. The system log has every row, past the 48th too.
static void *streamTelemetry(void *arg) {
    Gpu *g = static_cast<Gpu *>(arg);
    pthread_setname_np("native-metal-telemetry");
    pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);   // a row is up to 46 marker calls: behind the render server's own work
    sleep(5);
    static char table[8192], sent[48][92];
    for (unsigned pass = 0;; ++pass) {
        native_bridge_metal_telemetry(table, sizeof table);
        unsigned row = 0;
        for (char *line = table; *line; ++row) {
            char *end = strchr(line, '\n');
            if (end) *end = 0;
            if (pass % 5 == 0) os_log(OS_LOG_DEFAULT, "NativeAdrenoMetal table: %{public}s", line);
            if (row < 48 && strncmp(sent[row], line, sizeof sent[row] - 1)) {
                strlcpy(sent[row], line, sizeof sent[row]);
                g->mark(0xC000u | row);
                const size_t length = strlen(sent[row]);
                for (size_t i = 0; i < length; i += 2) {
                    const unsigned a = (unsigned char)sent[row][i] & 0x7f, b = i + 1 < length ? (unsigned char)sent[row][i + 1] & 0x7f : 0;
                    g->mark(0x8000u | (a << 7) | b);
                }
            }
            if (!end) {++row; break;}
            line = end + 1;
        }
        for (; row < 48; ++row)
            if (sent[row][0]) {sent[row][0] = 0; g->mark(0xC000u | row);}
        sleep(6);
    }
    return nullptr;
}
static unsigned gClientPort;
static void clientFailure(unsigned port, const char *step, const char *detail) {
    if (Gpu *gpu = makeGpu(port)) {
        // The row holds 63 characters: the common prefix goes, and a long reason keeps its start (what was refused) and its end (why).
        if (!detail) detail = "";
        if (!strncmp(detail, "native-metal: ", 14)) detail += 14;
        char shown[48];
        const size_t length = strlen(detail);
        if (length <= 46) strlcpy(shown, detail, sizeof shown);
        else snprintf(shown, sizeof shown, "%.18s~%s", detail, detail + length - 27);
        char text[64];
        snprintf(text, sizeof text, "%.14s %s", step, shown);
        streamText(*gpu, text);
        // A refusal's text is what says why a client ends (iconservicesagent's pipeline refusal showed 18 characters of it): the rest of it
        // follows in rows of its own, up to six, each 46 characters.
        if (!strcmp(step, "problem") && length > 46)
            for (unsigned part = 1, at = 46; part <= 6 && at < length; ++part, at += 46) {
                // The log keeps the latest text row only: each part follows the one before it by 4 s so a sample catches every one.
                std::string row = "problem" + std::to_string(part) + " " + std::string(detail + at).substr(0, 46);
                auto *target = gpu;
                dispatch_after(dispatch_time(DISPATCH_TIME_NOW, int64_t(part) * 4 * NSEC_PER_SEC),
                               dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{streamText(*target, row.c_str());});
            }
    }
}
static id clientDevice(unsigned port) {
    char turnip[1024];
    if (!turnip::libraryPath(turnip, sizeof turnip) || access(turnip, R_OK) != 0) {clientFailure(port, "no-turnip", ""); return nil;}
    uint64_t registryID = 0;
    if (void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL)) {
        if (auto retain = (int (*)(unsigned))dlsym(iokit, "IOObjectRetain")) retain(port);
        if (auto entryID = (int (*)(unsigned, uint64_t *))dlsym(iokit, "IORegistryEntryGetRegistryEntryID")) entryID(port, &registryID);
    }
    // IOSurfaces this GPU reads or writes must be plain rows, as in the render server.
    CFPreferencesSetAppValue(CFSTR("CAEnableIOSurfaceCompression"), kCFBooleanFalse, kCFPreferencesCurrentApplication);
    char candidates[3][512] = {};
    if (!confstr(_CS_DARWIN_USER_CACHE_DIR, candidates[0], sizeof candidates[0] - 16)) candidates[0][0] = 0;
    if (!confstr(_CS_DARWIN_USER_TEMP_DIR, candidates[1], sizeof candidates[1] - 16)) candidates[1][0] = 0;
    strlcpy(candidates[2], sharedShaderCacheDirectory().c_str(), sizeof candidates[2]);
    id device = nil;
    const char *lastError = "";
    unsigned usable = 0;
    for (unsigned i : {2u, 0u, 1u}) {
        if (device) break;
        if (!candidates[i][0]) continue;
        // In the user cache directory the cache sits inside Metal's own shader cache directory (com.apple.metal): a sandboxed process may
        // write there whenever its profile lets Metal cache shaders at all (macOS iconservicesagent: its profile allows METAL_CACHE_PATH
        // and denied every write to <cache>/m2v, so each of its launches compiled every pipeline again and it was killed at its memory
        // limit within seconds).
        if (i == 0) {strlcat(candidates[i], "com.apple.metal", sizeof candidates[i]); mkdir(candidates[i], 0700); strlcat(candidates[i], "/", sizeof candidates[i]);}
        if (i != 2) strlcat(candidates[i], "m2v", sizeof candidates[i]);
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
    if (!device && !usable) {
        setenv("METAL2VULKAN_SHADER_CACHE_OPTIONAL", "1", 1);
        const char *path = candidates[0][0] ? candidates[0] : candidates[1][0] ? candidates[1] : "/var/mobile/tmp/m2v";
        @try {
            device = native_bridge_metal_open(turnip, path, registryID, nullptr);
        } @catch (NSException *exception) {
            device = nil;
            lastError = "exception";
        }
        if (!device && !lastError[0]) lastError = nmb_last_error();
    }
    if (!device) clientFailure(port, usable ? "no-device" : "no-cachedir", lastError);
    // The client's first problem, once, on the same text line, sent from the thread that hit it: WidgetRenderer crashed on a nil render
    // encoder with no reason anywhere readable (its own log lines need the plugin's format strings, which the device does not keep), and a
    // once-a-second reader never ran before the crash (i568).
    if (device) {
        gClientPort = port;
        native_bridge_metal_problem_hook = [](const char *problem) {clientFailure(gClientPort, "problem", problem);};
        char problem[200] = {};
        if (native_bridge_metal_first_problem(problem, sizeof problem) && problem[0]) clientFailure(port, "problem", problem);
    }
    return device;
}
#if TARGET_OS_OSX
// The compositor's QuartzCore asserts metallib_url != nil (CA::OGL::new_metal_context: [[NSBundle bundleForClass:[CALayer class]]
// URLForResource:@"default" withExtension:@"metallib"]) on this layout (macOS run32), though both the root and the SystemOS volume hold
// QuartzCore.framework/Versions/A/Resources/default.metallib. Measured here, in the render server, with the same calls, before QuartzCore
// makes its Metal context: what the bundle resolves to, and what readdir and NSFileManager list at the bundle's root and in Versions.
static void quartzCoreBundleReport() {
    @autoreleasepool {
        Class layer = NSClassFromString(@"CALayer");
        NSBundle *bundle = layer ? [NSBundle bundleForClass:layer] : nil;
        NSURL *url = [bundle URLForResource:@"default" withExtension:@"metallib"];
        os_log(OS_LOG_DEFAULT, "NativeAdrenoMetal qc: bundle %{public}s resources %{public}s exec %{public}s url %{public}s",
               bundle.bundlePath.UTF8String ?: "-", bundle.resourcePath.UTF8String ?: "-", bundle.executablePath.UTF8String ?: "-",
               url.path.UTF8String ?: "nil");
        NSString *root = bundle.bundlePath ?: @"/System/Library/Frameworks/QuartzCore.framework";
        for (NSString *sub in @[@"", @"/Versions", @"/Versions/A", @"/Versions/A/Resources", @"/Resources"]) {
            NSString *path = [root stringByAppendingString:sub];
            std::string listing;
            if (DIR *d = opendir(path.fileSystemRepresentation)) {
                while (struct dirent *e = readdir(d)) {listing += e->d_name; listing += ' ';}
                closedir(d);
            } else listing = "(opendir " + std::to_string(errno) + ")";
            NSArray *items = [NSFileManager.defaultManager contentsOfDirectoryAtPath:path error:nil];
            struct stat st = {};
            const int sr = lstat(path.fileSystemRepresentation, &st);
            // opendir reads a whole union directory (both layers) only when fstatfs reports MNT_UNION (0x20) for it.
            struct statfs sf = {};
            const int fr = statfs(path.fileSystemRepresentation, &sf);
            os_log(OS_LOG_DEFAULT, "NativeAdrenoMetal qc: %{public}s statfs %d flags 0x%x %{public}s on %{public}s",
                   path.UTF8String, fr, sf.f_flags, sf.f_fstypename, sf.f_mntonname);
            os_log(OS_LOG_DEFAULT, "NativeAdrenoMetal qc: %{public}s lstat %d mode %o dev %d readdir [%{public}s] fm [%{public}s]",
                   path.UTF8String, sr, st.st_mode, st.st_dev, listing.c_str(),
                   [[items componentsJoinedByString:@" "] UTF8String] ?: "nil");
        }
    }
}
#endif
static id deviceForPort(unsigned port) {
    static id device;
    static bool tried;
    @synchronized([NativeAdrenoMetalDeviceBase class]) {
        if (tried) return device;
        tried = true;
        exceptiondiag::start(); // the throw site of an uncaught exception, for the kernel's THROW row (ExceptionDiag.h)
        // No concurrent binning in any Metal process: with it BV ran ahead of BR and the two stopped waiting on each other inside a pass
        // (int-261p, int-262, and in system memory int-265). Without it Turnip's autotune picks tile memory (GMEM) or system memory per
        // pass: i553 forced system memory for every pass, the GPU busy 19 % at 680 MHz. A tile store writes whole 16x4 blocks, which the
        // runtime keeps within a texture's rows (bridge patch 0016).
        if (FILE *flags = fopen("/private/var/ngpu/tu_debug", "r")) {
            char line[128] = {};
            if (fgets(line, sizeof line, flags)) {line[strcspn(line, "\n")] = 0; setenv("TU_DEBUG", line, 1);}
            fclose(flags);
        }
        // tu-debug=<flags>: TU_DEBUG for every Metal process (an experiment: sysmem renders without GMEM load and store, gmem always with them).
        {
            char args[1100] = {};
            size_t size = sizeof args - 1;
            if (sysctlbyname("kern.bootargs", args, &size, nullptr, 0) == 0)
                if (const char *at = bootArgToken(args, "tu-debug=")) {
                    char value[64] = {};
                    strlcpy(value, at + strlen("tu-debug="), sizeof value);
                    value[strcspn(value, " ")] = 0;
                    setenv("TU_DEBUG", value, 1);
                }
        }
        // Every pass renders in tile memory (GMEM) wherever Turnip can tile it: attachments are loaded and stored once per tile instead of
        // every draw going to system memory, and memoryless attachments take no memory at all. The colour order of GMEM loads and stores is
        // the image's own swap on the memory side (patches/mesa/turnip-gmem-memory-side-swap.patch: 0.0 % of lock screen tiles exchanged).
        // A pass Turnip cannot tile still renders in system memory; the RM row of the memory log counts both. tu-debug= overrides this.
        setenv("TU_DEBUG", "gmem,nocb", 0);
        // diag=1 turns on every diagnostic of the GPU stack in this process: the frontend's frame trace, Turnip's pass stamps and
        // submission ring, the runtime's stage ring, and in the render server the status and telemetry lines. Without it none of them runs.
        if (bootArgSet("diag=1") || bootArgSet("gpu-trace=1")) setenv("SHENG_DIAG", "1", 1);
        else unsetenv("SHENG_DIAG");
        if (bootArgSet("gpu-trace=1")) setenv("SHENG_GPU_TRACE", "1", 1);
        else unsetenv("SHENG_GPU_TRACE");
        // metal-flow=1 enables the lowering thread, commit submission and the warm pass; =0 states the default. Either logs the FL counts
        // (native_bridge_metal_open), so two runs compare without diag=1's own cost.
        // gpu-purge=0: the bridge discards nothing of what callers declare Volatile (NativeMetalPurge.rs), for a comparison on the
        // same image. Read by the bridge as the device opens.
        if (bootArgSet("gpu-purge=0")) setenv("NATIVE_METAL_PURGE", "0", 1);
        else unsetenv("NATIVE_METAL_PURGE");
        // gpu-ycbcr-planes=0: every two-plane YCbCr surface texture is made with its converted image, as before bridge patch 0035,
        // for a comparison on the same image. Read by the bridge at the first such texture.
        if (bootArgSet("gpu-ycbcr-planes=0")) setenv("NATIVE_METAL_YCBCR_PLANES", "0", 1);
        else unsetenv("NATIVE_METAL_YCBCR_PLANES");
        {
            // metal-flow=<bits>: 1 the lowering thread, 2 commit submission, 4 the warm pass; all three without the argument.
            char args[1100] = {}, flow[16] = "";
            size_t size = sizeof args - 1;
            if (sysctlbyname("kern.bootargs", args, &size, nullptr, 0) == 0)
                if (const char *at = bootArgToken(args, "metal-flow=")) {
                    strlcpy(flow, at + strlen("metal-flow="), sizeof flow);
                    flow[strcspn(flow, " ")] = 0;
                }
            if (flow[0]) setenv("NATIVE_METAL_FLOW", flow, 1);
            else unsetenv("NATIVE_METAL_FLOW");
        }
        if (bootArgSet("wallpaper-freeze=1")) setenv("NATIVE_METAL_FREEZE_WALLPAPER", "1", 1);
        // log-gpu=1: the GPU stack's log lines (NSLog, the pipeline journal); off without it.
        if (bootArgSet("log-gpu=1")) setenv("SHENG_LOG_GPU", "1", 1);
        else unsetenv("SHENG_LOG_GPU");
#if TARGET_OS_OSX
        // macOS: Turnip opens the accelerator by class (AGXAccelerator here); the display takes plain rows (no Apple framebuffer or IOSurface
        // compression). The runtime keeps its default block size: the virtual-machine board shrank it for a 512 MiB GPU pool, which this
        // GPU's window does not have. The render server (WindowServer) takes the same path as backboardd on iPadOS below (display hand-off
        // client, shader cache directory, pipeline pack, priming), without the iPadOS QuartzCore addresses; every device gets IOGPU's
        // answers (IOGPUMetalDeviceShim.h).
        setenv("TU_DARWIN_SERVICE", "AGXAccelerator", 1);
        setenv("CA_DISABLE_FRAMEBUFFER_COMPRESSION", "1", 1);
        CFPreferencesSetAppValue(CFSTR("CAEnableFramebufferCompression"), kCFBooleanFalse, kCFPreferencesCurrentApplication);
        if (!nativeIsRenderServer()) {
            // The system shader cache the render server fills (SharedShaderCache.h), read for what this process's own cache lacks.
            // gpu-prime-system=0: no process reads it and the render server keeps its cache and its priming to itself, as before, for
            // a comparison on the same image.
            if (!bootArgSet("gpu-prime-system=0"))
                native_bridge_metal_system_cache(systemcache::kReaderPath, systemcache::kOwners, 2, false);
            device = clientDevice(port);
            if (device) iogpu::install(device, port);
            return device;
        }
#else
        if (!nativeIsRenderServer()) return device = clientDevice(port);
#endif
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
#if defined(__arm64e__)
        // iPadOS 27 picks a 64-bit half-float framebuffer ('RGhA') whenever the display's EDR headroom is above 1 (this panel reports XDR
        // headroom), which the display path can only show converted, slowly and greyed. As on iPadOS 17, where the render server drew plain
        // 8-bit BGRA, the headroom is held at 1. IOMFBDisplay::framebuffer_pixel_format (24A446 0x184461838, asked on every update) takes
        // RGhA when the headroom (Display::edr_headroom, CA_DISPLAY_EDR_HEADROOM first when nonzero) is above CA_FP16_MIN_HEADROOM and
        // the panel's own above 8, or when CA_FORCE_FP16_FRAMEBUFFER, CA_ALLOW_EDR_MIRRORING or the display's own flag says so. Both
        // options are QuartzCore's float options: init_debug reads them from the environment into debug_values (index 21 at
        // __DATA_DIRTY 0x1e7021014, index 81 at 0x1e7021104), so they are set in the environment for a read after this and written for
        // one before it. i84edf75a1 wrote only the stored values (marks 6E01, 6E02) and the framebuffer stayed RGhA: an option read after
        // the plugin loaded put the threshold back to 0. CADefaultsGetFloat takes the environment first (getenv, atof), then the defaults:
        // a threshold of 1e30 is above any headroom.
        // id02fe1633 read back 0x6E19 in one process: CA_ALLOW_EDR_MIRRORING set and the threshold not raised (a process whose options
        // were read after the write). The mirroring option is held off the same two ways (bool option 156 at _ca_debug_options
        // 0x1e702116c + 156; init_debug stores value != 0).
        setenv("CA_DISPLAY_EDR_HEADROOM", "1", 1);
        setenv("CA_FP16_MIN_HEADROOM", "1e30", 1);
        setenv("CA_ALLOW_EDR_MIRRORING", "0", 1);
#if !TARGET_OS_OSX
        // The stored option values and the framebuffer branch are addresses of iPadOS 27's QuartzCore (24A446).
        if (void *start = dlsym(RTLD_DEFAULT, "CARenderServerStart")) {
            const uintptr_t slide = (uintptr_t)ptrauth_strip(start, ptrauth_key_function_pointer) - 0x1846dce88;
            const float one = 1.0f, never = 1e30f;
            gpu->mark(writeTask(0x1e7021014 + slide, &one, sizeof one) ? 0x6E01 : 0x6EF1);
            gpu->mark(writeTask(0x1e7021104 + slide, &never, sizeof never) ? 0x6E02 : 0x6EF2);
            const uint8_t off = 0;
            gpu->mark(writeTask(0x1e702116c + 156 + slide, &off, sizeof off) ? 0x6E03 : 0x6EF3);
            if (!strcmp(getprogname(), "backboardd") && bootArgSet("fb-bgra=1")) gpu->mark(patchFramebufferBranch(0x184461928 + slide));
            // What framebuffer_pixel_format reads in the render server, once the desktop has started (other processes draw no
            // framebuffer): 0x6E20 | 1 headroom override 1, 2 threshold above 1, 4 CA_FORCE_FP16_FRAMEBUFFER (_ca_debug_options
            // 0x1e702116c + 128), 8 CA_ALLOW_EDR_MIRRORING (+ 156), 0x10 the device-is-virtualized byte (0x1e701bdd9).
            // i35abaf83 read 0x6E23 and the framebuffer stayed RGhA (48388K swap surfaces, 8 bytes a pixel). The function has two more
            // ways to RGhA: the display's own flag (+0x6840 of the display) and, when no compression format was picked, the virtualized
            // byte, which sub_184541f10 stores once from a call with the constant CFString at 0x1e9021680; the string's text is noted
            // ("ca virtualized key ...") so the answer behind it can be found.
            static uintptr_t gSlide;
            gSlide = slide;
            pthread_t reader;
            if (!strcmp(getprogname(), "backboardd") && pthread_create(&reader, nullptr, [](void *arg) -> void * {
                sleep(30);
                const float headroom = *reinterpret_cast<volatile const float *>(0x1e7021014 + gSlide);
                const float threshold = *reinterpret_cast<volatile const float *>(0x1e7021104 + gSlide);
                const uint8_t force = *reinterpret_cast<volatile const uint8_t *>(0x1e702116c + 128 + gSlide);
                const uint8_t mirroring = *reinterpret_cast<volatile const uint8_t *>(0x1e702116c + 156 + gSlide);
                const uint8_t virtualized = *reinterpret_cast<volatile const uint8_t *>(0x1e701bdd9 + gSlide);
                static_cast<Gpu *>(arg)->mark(0x6E20 | (headroom == 1.0f ? 1 : 0) | (threshold > 1.0f ? 2 : 0) | (force ? 4 : 0) | (mirroring ? 8 : 0) |
                                              (virtualized ? 0x10 : 0));
                // A constant CFString: isa, flags, bytes, length. Only an 8-bit constant (flags 0x7c8) with a short length is read.
                const uint64_t *cf = reinterpret_cast<const uint64_t *>(0x1e9021680 + gSlide);
                char note[96];
                if ((uint32_t)cf[1] == 0x7c8 && cf[3] > 0 && cf[3] < 64) {
                    const char *text = reinterpret_cast<const char *>(ptrauth_strip(reinterpret_cast<void *>(cf[2]), ptrauth_key_process_independent_data));
                    snprintf(note, sizeof note, "ca virtualized key %.*s = %u", (int)cf[3], text, virtualized);
                } else
                    snprintf(note, sizeof note, "ca virtualized key ? flags %llx = %u", (unsigned long long)cf[1], virtualized);
                native_bridge_metal_note(note);
                return nullptr;
            }, gpu) == 0)
                pthread_detach(reader);
        }
#else
        // macOS: the framebuffer format is changed as a fixup of signed text (the board's payload text_fixups), applied when the page
        // passes code signing validation. A write into the mapped page from here made the page invalid, and code signing killed
        // WindowServer at its next validation of that page.
#endif  // !TARGET_OS_OSX
#endif
        // The runtime's creation log reports each VkPipeline making's time to the device's sink, which the telemetry's V row sums
        // (diag=1).
        unsetenv("METAL2VULKAN_LOG_PIPELINES");
        CFPreferencesSetAppValue(CFSTR("CAEnableFramebufferCompression"), kCFBooleanFalse, kCFPreferencesCurrentApplication);
        CFPreferencesSetAppValue(CFSTR("CAEnableIOSurfaceCompression"), kCFBooleanFalse, kCFPreferencesCurrentApplication);
        char candidates[7][512] = {};
        if (!confstr(_CS_DARWIN_USER_CACHE_DIR, candidates[0], sizeof candidates[0] - 16)) candidates[0][0] = 0;
        if (!confstr(_CS_DARWIN_USER_TEMP_DIR, candidates[1], sizeof candidates[1] - 16)) candidates[1][0] = 0;
        strlcpy(candidates[2], "/tmp/", sizeof candidates[2]);
        strlcpy(candidates[3], "/private/var/tmp/", sizeof candidates[3]);
        // The user temp directory of the mobile user is under /var/mobile/tmp, which launchd's boot task dirs_cleaner empties at every boot,
        // so with a persistent Data store the pipelines were still compiled again each boot (V row hit0). Try the user's Library/Caches first
        // (marker 0x6814); the earlier candidates keep their markers.
        if (const passwd *user = getpwuid(getuid()); user && user->pw_dir && user->pw_dir[0]) {
            snprintf(candidates[4], sizeof candidates[4] - 16, "%s/Library/Caches/com.apple." NATIVE_RENDER_SERVER, user->pw_dir);
            mkdir(candidates[4], 0700);
            strlcat(candidates[4], "/", sizeof candidates[4]);
        }
        strlcpy(candidates[5], sharedShaderCacheDirectory().c_str(), sizeof candidates[5]);
#if TARGET_OS_OSX
        // First the system shader cache's directory, where the kernel made it for this process (marker 0x6816): what the render
        // server compiles and primes there, every process reads (SharedShaderCache.h). Not with gpu-prime-system=0.
        if (!bootArgSet("gpu-prime-system=0")) strlcpy(candidates[6], systemcache::writerDirectory().c_str(), sizeof candidates[6]);
#endif
        static const unsigned order[] = {6, 5, 4, 0, 1, 2, 3};
        for (unsigned i : order) {
            if (device) break;
            if (!candidates[i][0]) continue;
            if (i == 0) {strlcat(candidates[i], "com.apple.metal", sizeof candidates[i]); mkdir(candidates[i], 0700); strlcat(candidates[i], "/", sizeof candidates[i]);}   // as for the clients below
            if (i < 5) strlcat(candidates[i], "m2v", sizeof candidates[i]);
            if (i != 6) mkdir(candidates[i], 0700);
            if (access(candidates[i], W_OK) != 0) continue;
            // Files an earlier image's copying import left half written here (int 590aaee2), which hold memory of the Data disk.
            {
                static bool once;
                if (!once && i < 5) {once = true; removeHalfImported(candidates[i]);}
            }
            // The image's pipeline pack (an earlier boot's export), into this directory before the runtime reads it. Marker 0x6Cnn: files written
            // (saturated at 0xfe), 0x6Cff the pack was refused.
            if (char pack[1024]; requestedPath("NativeAdrenoMetal.m2vpack", pack, sizeof pack)) {
                const int imported = cacheexport::importPack(pack, candidates[i]);
                if (imported) gpu->mark(0x6C00 | (imported < 0 ? 0xff : imported > 0xfe ? 0xfe : imported));
            }
            // What the device writes in the system cache's directory is readable by everyone; in any other, by this user alone.
            native_bridge_metal_system_cache(nullptr, nullptr, 0, i == 6);
            @try {
                device = native_bridge_metal_open(turnip, candidates[i], registryID, nullptr);
            } @catch (NSException *exception) {
                device = nil;
            }
            if (device) {gpu->mark(0x6810 + i); gCacheDirectory = candidates[i]; gFillsSystemCache = i == 6;}   // which directory served
        }
        if (!device) {
            gpu->mark(0x68f2);
            char text[64];
            snprintf(text, sizeof text, "PUB no device: %.40s", nmb_last_error());
            streamText(*gpu, text);
            return nil;
        }
        gpu->mark(0x6802);
        // The display hand-off client on its own thread: it only serves, not the discovery.
        pthread_t scanout;
        pthread_attr_t attributes;
        pthread_attr_init(&attributes);
        pthread_attr_setdetachstate(&attributes, PTHREAD_CREATE_DETACHED);
        pthread_attr_setstacksize(&attributes, 1u << 20);
        // The telemetry table with diag=1 (streamTelemetry); without it the thread does not exist.
        if (getenv("SHENG_DIAG")) {
            pthread_t telemetry;
            pthread_create(&telemetry, &attributes, streamTelemetry, gpu);
        }
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
        // The cache directory's export to the UFS log only when NativeAdrenoMetal.cacheexport sits beside the bundle: Data persists across
        // boots now, so the caches no longer need the trip through the host, and once priming has filled the directory, every rescan and
        // export competed with the render server (int-245: the scanning thread at 45% CPU, the sending one at 48%, frames twice as slow).
        char exportPath[1024];
        if (requestedPath("NativeAdrenoMetal.cacheexport", exportPath, sizeof exportPath)) {
            pthread_t exporter;
            pthread_create(&exporter, &attributes, [](void *arg) -> void * {
                cacheexport::run(*static_cast<Gpu *>(arg), gCacheDirectory);
                return nullptr;
            }, gpu);
        }
        // Every Metal library of the OS through the runtime, 90 s after the render server opened the device (PrimeShaders.h); the boots that froze
        // or crashed before three minutes never primed anything. Progress is kept in the Data cache directory, so later boots resume.
        // The translator recurses deeply on large functions: give it the stack of a main thread.
        pthread_attr_setstacksize(&attributes, 8u << 20);
        pthread_t primer;
        pthread_create(&primer, &attributes, [](void *) -> void * {
                pthread_setname_np("native-metal-prime");   // a process sample and the telemetry's TH rows name its CPU time
                char list[1024];
                prime::run((id<MTLDevice>)device, gCacheDirectory, 90,
                           requestedPath("NativeAdrenoMetal.primelist", list, sizeof list) ? std::string(list) : std::string(),
                           gFillsSystemCache);
                return nullptr;
            }, nullptr);
        pthread_attr_destroy(&attributes);
        // The GMEM colour order test (NativeTurnipSwapTest.h), once, 20 s after the device is published: each result line goes to the
        // kernel's GPU trace as a problem record (event 65).
        if (bootArgSet("swaptest=1") && nativeIsRenderServer()) {
            pthread_t tester;
            pthread_create(&tester, nullptr, [](void *) -> void * {
                    sleep(20);
                    char path[1024];
                    if (!turnip::libraryPath(path, sizeof path)) return nullptr;
                    void *lib = dlopen(path, RTLD_NOW | RTLD_LOCAL);
                    auto send = lib ? reinterpret_cast<bool (*)(uint32_t, const void *, uint32_t)>(dlsym(lib, "tu_darwin_frame_diagnostic"))
                                    : nullptr;
                    turnip::swaptest::run(path, [&](const char *text) {
                        if (!send) return;
                        NativeGpuTraceRecord record = {};
                        record.event = GpuTraceProblem;
                        strlcpy(reinterpret_cast<char *>(&record.value[1]), text, 11 * sizeof(uint64_t));
                        send(NativeGpuTraceMethod, &record, sizeof record);
                    });
                    return nullptr;
                }, nullptr);
            pthread_detach(tester);
        }
        gpu->mark(0x6803);
#if TARGET_OS_OSX
        iogpu::install(device, port);
        quartzCoreBundleReport();
#endif
        return device;
    }
}
}  // namespace publish

@implementation NativeAdrenoMetalDevice
- (id)initWithAcceleratorPort:(unsigned)port {
#if TARGET_OS_OSX
    // The registered device is the frontend's object, not this one: IOGPUMetalDevice's own initializer would open an IOGPU user client.
    // This object is kept, because -[IOGPUMetalDevice dealloc] tears down state its initializer never made.
    static NSMutableArray *kept;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ kept = [NSMutableArray array]; });
    @synchronized(kept) {[kept addObject:self];}
    if (!port) return nil;
#else
    self = [super init];
    if (!self || !port) return nil;
#endif
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
#if TARGET_OS_OSX
                    const unsigned own = getService(0, matching("AGXAccelerator"));
#else
                    const unsigned own = getService(0, matching("IOAcceleratorES"));
#endif
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

#if TARGET_OS_OSX
#include <objc/runtime.h>
#include <mach/mach_time.h>
#include <atomic>
// WindowServer's frame pacing, as macOS-DroidVM's plugin (24af265, kc253-kc292), each behind its boot argument.
//
// ca-triple-buffer=1 sets QuartzCore's debug option CA_FORCE_TRIPLE_BUFFERED_LATENCY (read by IOMFBDisplay::can_update_):
// CoreAnimation renders a frame ahead instead of starting each frame only after the last one's buffer came back, which it otherwise
// does whenever a frame took longer than a refresh (DroidVM: a heavy window drag from 56-60 to 61-67 frames a second).
__attribute__((constructor)) static void tripleBufferFromBootArgs() {
    if (strcmp(getprogname(), "WindowServer") || !publish::bootArgSet("ca-triple-buffer=1")) return;
    auto setOption = reinterpret_cast<void (*)(unsigned, bool)>(dlsym(RTLD_DEFAULT, "CASetDebugOption"));
    auto optionName = reinterpret_cast<const char *(*)(unsigned)>(dlsym(RTLD_DEFAULT, "CAGetDebugOptionEnvVariableName"));
    // The option's index differs between QuartzCore builds, so it is found by its name.
    unsigned index = ~0u;
    for (unsigned i = 0; optionName && i < 1024; ++i) {
        const char *name = optionName(i);
        if (!name) break;
        if (!strcmp(name, "CA_FORCE_TRIPLE_BUFFERED_LATENCY")) { index = i; break; }
    }
    const bool set = setOption && index != ~0u;
    if (set) setOption(index, true);
    os_log(OS_LOG_DEFAULT, "NativeAdrenoMetal: CA_FORCE_TRIPLE_BUFFERED_LATENCY %{public}s (option %u)", set ? "set" : "not found", index);
}

// ca-free-wakeup=1: SkyLight starts each WindowServer update at -[CAWindowServerDisplay nextWakeupTime], which QuartzCore puts on the
// refresh grid (the last VBL plus a frame), so an update that took 13 ms is followed by one at 16.7 ms. This answers the earlier of that
// and a refresh after the last update's start (-beginExternalUpdate:usingSoftwareRenderer:): the display shows a swap when it comes,
// not at a VBL. SkyLight adds its updater delay (0.5 ms) and updates at once only when the last update is more than 1.25 frames before
// the answer, so before the due time the answer is the due time less that delay, after it the least time 1.25 frames past the last
// start. Without an update in the last second the grid's answer stands. The refresh is vsync='s (120 by default).
// DroidVM: a heavy window drag from 60 to 96 frames a second.
static std::atomic<uint64_t> gUpdateBegan{0}, gLastAsk{0};
static double hostSeconds(uint64_t t) {
    static mach_timebase_info_data_t base;
    if (!base.denom) mach_timebase_info(&base);
    return (double)t * base.numer / base.denom / 1e9;
}
static uint64_t hostTicks(double seconds) {
    static mach_timebase_info_data_t base;
    if (!base.denom) mach_timebase_info(&base);
    return (uint64_t)(seconds * 1e9 * base.denom / base.numer);
}
__attribute__((constructor)) static void freeWakeupFromBootArgs() {
    if (strcmp(getprogname(), "WindowServer") || !publish::bootArgSet("ca-free-wakeup=1")) return;
    char args[1100] = {};
    size_t size = sizeof args - 1;
    unsigned hz = 120;
    if (sysctlbyname("kern.bootargs", args, &size, nullptr, 0) == 0)
        if (const char *at = publish::bootArgToken(args, "vsync=")) {
            const int value = atoi(at + 6);
            if (value >= 30 && value <= 240) hz = (unsigned)value;
        }
    const double refresh = 1.0 / hz;
    Class display = objc_getClass("CAWindowServerDisplay");
    static SEL wakeSel = sel_registerName("nextWakeupTime"), beginSel = sel_registerName("beginExternalUpdate:usingSoftwareRenderer:");
    Method wake = display ? class_getInstanceMethod(display, wakeSel) : nullptr;
    Method begin = display ? class_getInstanceMethod(display, beginSel) : nullptr;
    if (!wake || !begin || method_getTypeEncoding(wake)[0] != 'd' || method_getNumberOfArguments(begin) != 4) {
        os_log(OS_LOG_DEFAULT, "NativeAdrenoMetal: free wakeup: CAWindowServerDisplay lacks %{public}s",
               !wake ? "nextWakeupTime" : "beginExternalUpdate:usingSoftwareRenderer:");
        return;
    }
    static auto wakeGrid = reinterpret_cast<double (*)(id, SEL)>(method_getImplementation(wake));
    static auto beginOriginal = reinterpret_cast<uint64_t (*)(id, SEL, void *, uint64_t)>(method_getImplementation(begin));
    method_setImplementation(begin, imp_implementationWithBlock(^uint64_t(id self, void *update, uint64_t software) {
        // The update's start is SkyLight's last ask before it, where its update callback began.
        const uint64_t now = mach_absolute_time(), asked = gLastAsk.load(std::memory_order_relaxed);
        gUpdateBegan.store(asked && now - asked < hostTicks(0.003) ? asked : now, std::memory_order_relaxed);
        return beginOriginal(self, beginSel, update, software);
    }));
    method_setImplementation(wake, imp_implementationWithBlock(^double(id self) {
        const double grid = wakeGrid(self, wakeSel);
        gLastAsk.store(mach_absolute_time(), std::memory_order_relaxed);
        const uint64_t began = gUpdateBegan.load(std::memory_order_relaxed);
        if (grid <= 0 || !began) return grid;
        const double now = hostSeconds(mach_absolute_time()), start = hostSeconds(began);
        if (now - start > 1.0) return grid;
        const double due = start + refresh - 0.0005, ahead = start + 1.25 * refresh + 0.001;
        if (now < due) return due < grid ? due : grid;
        return now > ahead ? now : ahead;
    }));
    os_log(OS_LOG_DEFAULT, "NativeAdrenoMetal: free wakeup on (%.2f ms)", refresh * 1e3);
}
#endif
