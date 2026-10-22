// SPDX-License-Identifier: MIT
// Adreno probe daemon: started by launchd from an owner job (a union view over /System/Library/LaunchDaemons, see the boot-time addon
// mount), so it runs outside every sandbox profile. It waits for the Adreno accelerator service to appear, runs the shared userspace GPU
// test once, then writes the system log for the UFS log export (NativeOsLogExport.mm) and exits. The test result reaches the kernel through
// the driver's progress markers and SetMetadata.
#include <stdlib.h>
#include "NativeAdrenoUserTest.h"

extern "C" void nativeOsLogExport();

using MatchingFn = void *(*)(const char *);
using GetServiceFn = unsigned (*)(unsigned, void *);

int main() {
    void *iokit = dlopen("/System/Library/Frameworks/IOKit.framework/IOKit", RTLD_NOW | RTLD_LOCAL);
    if (!iokit) return 1;
    auto matching = (MatchingFn)dlsym(iokit, "IOServiceMatching");
    auto getService = (GetServiceFn)dlsym(iokit, "IOServiceGetMatchingService");
    if (!matching || !getService) return 2;
    // The service is published a few seconds into boot (after the GPU is powered); poll for up to two minutes.
    unsigned port = 0;
    for (unsigned i = 0; i < 240 && !port; ++i) {
        port = getService(0, matching("IOAcceleratorES"));
        if (!port) usleep(500000);
    }
    const int status = !port ? 3 : adrenoUserTest(port, "adrenoprobed") >= 0 ? 0 : 4;
    nativeOsLogExport();
    return status;
}
