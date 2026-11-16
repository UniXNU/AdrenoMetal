// SPDX-License-Identifier: MIT
#pragma once
#include <IOKit/pwr_mgt/IOPMLib.h>
#include <stdio.h>

// The worker owns these assertions so closing the app does not end protection.
class SCCompilationActivity {
    IOPMAssertionID system = kIOPMNullAssertionID;
    IOPMAssertionID display = kIOPMNullAssertionID;

    static void release(IOPMAssertionID &assertion) {
        if (assertion == kIOPMNullAssertionID) return;
        const IOReturn status = IOPMAssertionRelease(assertion);
        if (status != kIOReturnSuccess)
            fprintf(stderr, "Shader Cache - power assertion release failed - 0x%x\n", (unsigned)status);
        assertion = kIOPMNullAssertionID;
    }
public:
    SCCompilationActivity() = default;
    SCCompilationActivity(const SCCompilationActivity &) = delete;
    SCCompilationActivity &operator=(const SCCompilationActivity &) = delete;
    ~SCCompilationActivity() {end();}

    IOReturn begin() {
        if (system != kIOPMNullAssertionID) return kIOReturnSuccess;
        const CFStringRef reason = CFSTR("Shader Cache compilation");
        IOReturn status = IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleSystemSleep,
            kIOPMAssertionLevelOn, reason, &system);
        if (status != kIOReturnSuccess) return status;
        status = IOPMAssertionCreateWithName(kIOPMAssertionTypePreventUserIdleDisplaySleep,
            kIOPMAssertionLevelOn, reason, &display);
        if (status != kIOReturnSuccess) end();
        return status;
    }

    void end() {
        release(display);
        release(system);
    }
};
