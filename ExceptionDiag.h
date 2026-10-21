// SPDX-License-Identifier: MIT
// The throw site of an uncaught Objective-C exception (int-201). MobileSafari aborts with "-[_NSPlaceholderData initWithContentsOfURL:...]: nil URL
// argument" on a dispatch worker, and the crash record's backtrace has only the dispatch frames: the unwinder gave up inside the throw, so which
// caller passed the nil URL is lost. Foundation records the return addresses when it throws (callStackReturnAddresses); this handler copies them,
// with the exception's name and reason, into a malloc'd C string that the kernel's abort-reason scan finds in the dying process's heap
// (kits/sheng/BootFaultProbe.c, "SHENGTHROW ") and the log prints as the THROW row. The original handler (the app's own, or Foundation's) runs
// after it, unchanged; a process that never throws is untouched.
#pragma once
#import <Foundation/Foundation.h>
#include <stdlib.h>
#include <string.h>

namespace exceptiondiag {

static NSUncaughtExceptionHandler *gPrevious;
static char *gText;   // kept referenced: the text must stay in the heap until the process is gone

static void handler(NSException *exception) {
    @autoreleasepool {
        NSMutableString *text = [NSMutableString stringWithFormat:@"SHENGTHROW %@ | ", exception.name];
        NSString *reason = exception.reason ?: @"";
        [text appendString:reason.length > 96 ? [reason substringToIndex:96] : reason];
        [text appendString:@" |"];
        NSArray<NSNumber *> *frames = exception.callStackReturnAddresses;
        for (NSUInteger i = 0; i < frames.count && i < 20; ++i) [text appendFormat:@" %llx", (unsigned long long)frames[i].unsignedLongLongValue];
        for (NSUInteger i = 0; i < text.length; ++i) if ([text characterAtIndex:i] < 32 || [text characterAtIndex:i] > 126) [text replaceCharactersInRange:NSMakeRange(i, 1) withString:@"?"];
        if (const char *utf8 = text.UTF8String) gText = strdup(utf8);
    }
    if (gPrevious) gPrevious(exception);
}
// Every process that loads the plugin, once: the handler is installed in front of whatever the process already had.
static void start() {
    static bool started;
    if (started) return;
    started = true;
    gPrevious = NSGetUncaughtExceptionHandler();
    NSSetUncaughtExceptionHandler(handler);
}

}  // namespace exceptiondiag
