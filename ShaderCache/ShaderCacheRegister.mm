// SPDX-License-Identifier: MIT
#import <Foundation/Foundation.h>
#import <CoreServices/CoreServices.h>
#include "ShaderCacheState.h"

static BOOL registeredAt(NSString *path) {
    CFArrayRef urls = LSCopyApplicationURLsForBundleIdentifier(CFSTR("org.unixnu.ShaderCache"), nullptr);
    if (!urls) return NO;
    NSArray<NSURL *> *applications = CFBridgingRelease(urls);
    for (NSURL *url in applications) {
        if ([url.path.stringByStandardizingPath isEqual:path.stringByStandardizingPath]) return YES;
    }
    return NO;
}

static void record(NSString *path, OSStatus status) {
    NSString *message = [NSString stringWithFormat:@"%@ - %@ - status %d - %@\n", NSDate.date,
        status == noErr ? @"Registered" : @"Registration failed", int(status), path];
    fputs(message.UTF8String, status == noErr ? stdout : stderr);
    NSString *directory = [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Logs/UniXNU"];
    if ([NSFileManager.defaultManager createDirectoryAtPath:directory withIntermediateDirectories:YES attributes:nil error:nullptr]) {
        [message writeToFile:[directory stringByAppendingPathComponent:@"ShaderCacheRegister.log"]
                 atomically:YES encoding:NSUTF8StringEncoding error:nullptr];
    }
}

int main(int argc, const char **argv) {
    @autoreleasepool {
        NSString *path = SCApplication;
        if (argc == 3 && !strcmp(argv[1], "--application")) path = @(argv[2]);
        else if (argc != 1) return 64;
        NSBundle *app = [NSBundle bundleWithPath:path];
        if (![app.bundleIdentifier isEqual:@"org.unixnu.ShaderCache"] || !app.executableURL) {
            record(path, kLSNotAnApplicationErr);
            return 66;
        }
        // Registration belongs to the logged-in user's LaunchServices domain.
        // A finite retry window covers login while the service is still starting.
        OSStatus status = kLSUnknownErr;
        for (unsigned attempt = 0; attempt < 10; ++attempt) {
            status = LSRegisterURL((__bridge CFURLRef)[NSURL fileURLWithPath:path isDirectory:YES], true);
            if (status == noErr && registeredAt(path)) break;
            if (status == noErr) status = kLSUnknownErr;
            if (attempt != 9) [NSThread sleepForTimeInterval:2];
        }
        record(path, status);
        return status == noErr ? 0 : 1;
    }
}
