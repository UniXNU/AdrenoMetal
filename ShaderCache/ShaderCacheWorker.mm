// SPDX-License-Identifier: MIT
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include "ShaderCacheState.h"
#include "ShaderCacheActivity.h"
#include "../SharedShaderCache.h"
#include "../PrimeShaders.h"
#include <signal.h>
#include <sys/resource.h>

static void *pluginHandle;
static volatile sig_atomic_t interrupted;
static void interruptWorker(int) {interrupted = 1;}

extern "C" const char *nmb_runtime_build() {
    auto build = reinterpret_cast<const char *(*)()>(dlsym(pluginHandle, "nmb_runtime_build"));
    return build ? build() : "unknown";
}
extern "C" void native_bridge_metal_quiet_problems(bool quiet) {
    auto set = reinterpret_cast<void (*)(bool)>(dlsym(pluginHandle, "native_bridge_metal_quiet_problems"));
    if (set) set(quiet);
}

static void saveState(NSString *state, const std::string &library = {}, const std::string &function = {}, NSString *error = @"") {
    NSDictionary *value = @{
        @"state":state, @"build":SCInstalledBuild(), @"updated":NSDate.date,
        @"libraries":@(native_prime_libraries_done), @"total":@(native_prime_libraries_total),
        @"functions":@(native_prime_made + native_prime_functions_resumed),
        @"failedLibraries":@(native_prime_libraries_failed), @"failedFunctions":@(native_prime_functions_refused),
        @"missing":@(native_prime_inventory_failed), @"library":@(library.c_str()), @"function":@(function.c_str()), @"error":error
    };
    [value writeToFile:SCPath(@"shader-cache.plist") atomically:YES];
}

static void startState() {
    NSDictionary *saved = [NSDictionary dictionaryWithContentsOfFile:SCPath(@"shader-cache.plist")];
    if (![saved[@"build"] isEqual:SCInstalledBuild()]) {saveState(@"running"); return;}
    NSMutableDictionary *state = saved.mutableCopy;
    state[@"state"] = @"running";
    state[@"updated"] = NSDate.date;
    state[@"error"] = @"";
    [state writeToFile:SCPath(@"shader-cache.plist") atomically:YES];
}

int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 2 || (strcmp(argv[1], "--run") && strcmp(argv[1], "--stop"))) return 64;
        if (geteuid() != 0 || setuid(0) != 0 || systemcache::writerDirectory().empty()) return 77;
        umask(0022);
        if (!strcmp(argv[1], "--stop")) {
            if (!SCWorkerRunning()) return 0;
            return [NSData.data writeToFile:SCPath(@"shader-cache.stop") atomically:YES] ? 0 : 74;
        }
        int lock = open(SCPath(@"shader-cache.lock").fileSystemRepresentation, O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0644);
        if (lock < 0 || flock(lock, LOCK_EX | LOCK_NB)) {if (lock >= 0) close(lock); return 75;}
        freopen("/dev/null", "w", stdout);
        freopen(SCPath(@"shader-cache-worker.log").fileSystemRepresentation, "a", stderr);
        unlink(SCPath(@"shader-cache.stop").fileSystemRepresentation);
        signal(SIGTERM, interruptWorker);
        signal(SIGINT, interruptWorker);
        SCCompilationActivity activity;
        const IOReturn powerStatus = activity.begin();
        if (powerStatus != kIOReturnSuccess) {
            fprintf(stderr, "Shader Cache - power assertion creation failed - 0x%x\n", (unsigned)powerStatus);
            saveState(@"failed", {}, {}, @"Could not prevent idle sleep during compilation");
            return 71;
        }
        fprintf(stderr, "Shader Cache - preventing display and idle system sleep\n");
        startState();
        setpriority(PRIO_PROCESS, 0, 10);
        setenv("NATIVE_METAL_PRIMER", "1", 1);
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        NSString *executable = [SCPlugin stringByAppendingPathComponent:@"Contents/MacOS/NativeAdrenoMetal"];
        pluginHandle = dlopen(executable.fileSystemRepresentation, RTLD_NOW | RTLD_LOCAL);
        if (!device || !pluginHandle || !dlsym(pluginHandle, "nmb_runtime_build") || ![NSStringFromClass([device class]) isEqual:@"NativeBridgeMetalDevice"]) {
            saveState(@"failed", {}, {}, @"AdrenoMetal is unavailable");
            return 69;
        }
        prime::cancelled = [] {return interrupted || access(SCPath(@"shader-cache.stop").fileSystemRepresentation, F_OK) == 0;};
        prime::progress = [](const std::string &library, const std::string &function) {
            saveState(prime::stopping() ? @"stopping" : @"running", library, function);
        };
        prime::run(device, SCStorage.fileSystemRepresentation, 0,
                   "/Library/GPUBundles/NativeAdrenoMetal.primelist", true);
        activity.end();
        fprintf(stderr, "Shader Cache - restored normal idle sleep policy\n");
        saveState(prime::stopping() ? @"stopped" : native_prime_pass_complete == 1 ? @"complete" : @"incomplete");
        unlink(SCPath(@"shader-cache.stop").fileSystemRepresentation);
        close(lock);
        return native_prime_pass_complete == 1 ? 0 : prime::stopping() ? 130 : 1;
    }
}
