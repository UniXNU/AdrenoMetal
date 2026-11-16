// SPDX-License-Identifier: MIT
// Link with Foundation and IOKit; define SC_ACTIVITY_LIVE to check powerd.
#import <Foundation/Foundation.h>
#include <IOKit/pwr_mgt/IOPMLib.h>
#include <assert.h>
#include <unistd.h>

#ifndef SC_ACTIVITY_LIVE
#include <set>
static unsigned creates, releases, failAt;
static std::set<IOPMAssertionID> held;
static IOReturn createAssertion(CFStringRef type, IOPMAssertionLevel level,
                               CFStringRef reason, IOPMAssertionID *assertion) {
    ++creates;
    assert(CFEqual(type, creates == 1 ? kIOPMAssertionTypePreventUserIdleSystemSleep :
                                      kIOPMAssertionTypePreventUserIdleDisplaySleep));
    assert(level == kIOPMAssertionLevelOn);
    assert(CFEqual(reason, CFSTR("Shader Cache compilation")));
    if (creates == failAt) return kIOReturnNoMemory;
    *assertion = creates;
    assert(held.insert(*assertion).second);
    return kIOReturnSuccess;
}
static IOReturn releaseAssertion(IOPMAssertionID assertion) {
    assert(held.erase(assertion) == 1);
    ++releases;
    return kIOReturnSuccess;
}
#define IOPMAssertionCreateWithName createAssertion
#define IOPMAssertionRelease releaseAssertion
#endif
#include "../ShaderCache/ShaderCacheActivity.h"

#ifdef SC_ACTIVITY_LIVE
static unsigned activeAssertions() {
    CFDictionaryRef raw = nullptr;
    assert(IOPMCopyAssertionsByProcess(&raw) == kIOReturnSuccess);
    NSDictionary *processes = CFBridgingRelease(raw);
    unsigned count = 0;
    for (NSDictionary *value in processes[@(getpid())]) {
        if ([value[(__bridge NSString *)kIOPMAssertionNameKey] isEqual:@"Shader Cache compilation"] &&
            [value[(__bridge NSString *)kIOPMAssertionLevelKey] unsignedIntValue] == kIOPMAssertionLevelOn)
            ++count;
    }
    return count;
}
#else
static void reset(unsigned failure = 0) {
    assert(held.empty());
    creates = releases = 0;
    failAt = failure;
}
static IOReturn workerFailure() {
    SCCompilationActivity activity;
    assert(activity.begin() == kIOReturnSuccess);
    return kIOReturnError;
}
#endif

int main() {
    @autoreleasepool {
#ifdef SC_ACTIVITY_LIVE
        assert(activeAssertions() == 0);
        {
            SCCompilationActivity activity;
            assert(activity.begin() == kIOReturnSuccess);
            assert(activity.begin() == kIOReturnSuccess);
            assert(activeAssertions() == 2);
            activity.end();
            activity.end();
            assert(activeAssertions() == 0);
        }
        {
            SCCompilationActivity activity;
            assert(activity.begin() == kIOReturnSuccess);
            assert(activeAssertions() == 2);
        }
        assert(activeAssertions() == 0);
        puts("PASS - powerd holds and releases both compilation assertions");
#else
        reset();
        {
            SCCompilationActivity activity;
            assert(activity.begin() == kIOReturnSuccess);
            assert(activity.begin() == kIOReturnSuccess);
            assert(creates == 2 && held.size() == 2);
            activity.end();
            activity.end();
        }
        assert(releases == 2 && held.empty());
        reset();
        assert(workerFailure() == kIOReturnError);
        assert(releases == 2 && held.empty());
        for (unsigned failure : {1u, 2u}) {
            reset(failure);
            {
                SCCompilationActivity activity;
                assert(activity.begin() == kIOReturnNoMemory);
                assert(held.empty());
            }
            assert(releases == failure - 1);
        }
        puts("PASS - completion, early failure, partial acquisition and repeated cleanup");
#endif
    }
}
