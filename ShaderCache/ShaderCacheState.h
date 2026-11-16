// SPDX-License-Identifier: MIT
#pragma once
#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <sys/file.h>
#include <unistd.h>

static NSString *const SCStorage = @"/private/var/tmp/NativeAdrenoMetal.cache";
static NSString *const SCPlugin = @"/Library/GPUBundles/NativeAdrenoMetal.bundle";
static NSString *const SCApplication = @"/Library/UniXNU/Shader Cache.app";

static NSString *SCPath(NSString *name) {return [SCStorage stringByAppendingPathComponent:name];}
static NSString *SCInstalledBuild() {
    NSData *data = [NSData dataWithContentsOfFile:[SCPlugin stringByAppendingPathComponent:@"Contents/Resources/gpu-build.json"]];
    if (!data) return @"";
    NSDictionary *build = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
    return [build[@"metal_source_sha256"] isKindOfClass:NSString.class] ? build[@"metal_source_sha256"] : @"";
}

static BOOL SCWorkerRunning() {
    int fd = open(SCPath(@"shader-cache.lock").fileSystemRepresentation, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) return NO;
    const BOOL locked = flock(fd, LOCK_SH | LOCK_NB) != 0 && errno == EWOULDBLOCK;
    close(fd);
    return locked;
}

static NSDictionary *SCReadState() {
    NSDictionary *saved = [NSDictionary dictionaryWithContentsOfFile:SCPath(@"shader-cache.plist")];
    NSMutableDictionary *state = saved ? saved.mutableCopy : [NSMutableDictionary dictionary];
    const BOOL running = SCWorkerRunning();
    state[@"running"] = @(running);
    if (running) {
        state[@"state"] = access(SCPath(@"shader-cache.stop").fileSystemRepresentation, F_OK) == 0 ? @"stopping" : @"running";
    } else if ([state[@"state"] isEqual:@"running"] || [state[@"state"] isEqual:@"stopping"]) {
        state[@"state"] = @"interrupted";
    } else if (!state[@"state"]) state[@"state"] = @"idle";
    if (!running && saved && ![saved[@"build"] isEqual:SCInstalledBuild()]) state[@"state"] = @"outdated";
    return state;
}
