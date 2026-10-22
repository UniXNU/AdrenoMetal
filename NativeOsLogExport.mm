// SPDX-License-Identifier: MIT
// System log export: the probe daemon (root, outside every sandbox profile) reads the unified log since boot and writes it as gzip text into
// a subdirectory of the render server's pipeline-cache directory. CacheExport.h ships that directory with the UFS log whenever it settles,
// so the host gets every process's os_log lines (PosterBoard, WidgetRenderer, wifid, ...) without a second export channel.
// Each pass rewrites the file through a temporary name; one daemon instance writes (the job runs under several labels).
#import <Foundation/Foundation.h>
#include <fcntl.h>
#include <objc/message.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zlib.h>
#include <string>

namespace {

constexpr const char *kDirectory = "/private/var/mobile/Library/Caches/com.apple.backboardd/m2v/zz-oslog";
constexpr const char *kLock = "/private/var/tmp/sheng-oslog.lock";
constexpr unsigned kPasses = 40, kPeriodSeconds = 45;
constexpr uint64_t kLineLimit = 600000;

const char *levelName(NSInteger level) {
    switch (level) {
    case 1: return "debug";
    case 2: return "info";
    case 3: return "notice";
    case 4: return "error";
    case 5: return "fault";
    default: return "-";
    }
}

// One pass: every log entry since the latest boot, "seconds pid process subsystem:category level message".
void writePass(unsigned pass) {
    @autoreleasepool {
        const std::string directory = kDirectory, temporary = directory + "/.oslog.txt.gz", final = directory + "/oslog.txt.gz";
        gzFile out = gzopen(temporary.c_str(), "wb6");
        if (!out) return;
        gzprintf(out, "# sheng oslog pass %u\n", pass);
        Class storeClass = NSClassFromString(@"OSLogStore");
        NSError *error = nil;
        // OSLogStoreSystem (0) is unavailable in the iOS SDK headers; the class answers it at run time with the logging entitlements.
        id store = storeClass ? ((id (*)(Class, SEL, NSInteger, NSError **))objc_msgSend)(storeClass, @selector(storeWithScope:error:), 0, &error)
                              : nil;
        if (!store) {
            gzprintf(out, "# no store: %s\n", (error ? error.description : @"OSLogStore missing").UTF8String);
        } else {
            id position = ((id (*)(id, SEL, NSTimeInterval))objc_msgSend)(store, @selector(positionWithTimeIntervalSinceLatestBoot:), 0);
            id entries = ((id (*)(id, SEL, NSUInteger, id, id, NSError **))objc_msgSend)(
                store, @selector(entriesEnumeratorWithOptions:position:predicate:error:), 0, position, nil, &error);
            if (!entries) gzprintf(out, "# no entries: %s\n", (error ? error.description : @"nil").UTF8String);
            NSDate *start = nil;
            uint64_t lines = 0;
            for (id entry in entries) {
                @autoreleasepool {
                    if (++lines > kLineLimit) {
                        gzprintf(out, "# line limit\n");
                        break;
                    }
                    NSDate *date = [entry valueForKey:@"date"];
                    if (!start) start = date;
                    const double at = [date timeIntervalSinceDate:start];
                    NSString *message = [entry valueForKey:@"composedMessage"];
                    const bool isLog = [entry respondsToSelector:@selector(level)];
                    NSString *process = [entry respondsToSelector:@selector(process)] ? [entry valueForKey:@"process"] : @"?";
                    NSString *subsystem = [entry respondsToSelector:@selector(subsystem)] ? [entry valueForKey:@"subsystem"] : @"";
                    NSString *category = [entry respondsToSelector:@selector(category)] ? [entry valueForKey:@"category"] : @"";
                    const int pid = [entry respondsToSelector:@selector(processIdentifier)] ? [[entry valueForKey:@"processIdentifier"] intValue] : 0;
                    const NSInteger level = isLog ? [[entry valueForKey:@"level"] integerValue] : 0;
                    gzprintf(out, "%9.3f %5d %s %s:%s %s %s\n", at, pid, process.UTF8String ?: "?", subsystem.UTF8String ?: "",
                             category.UTF8String ?: "", levelName(level), message.UTF8String ?: "");
                }
            }
            gzprintf(out, "# %llu entries\n", (unsigned long long)lines);
        }
        if (gzclose(out) == Z_OK) {
            chmod(temporary.c_str(), 0644);
            rename(temporary.c_str(), final.c_str());
        }
    }
}

}  // namespace

// Runs the passes (blocks for kPasses * kPeriodSeconds); returns at once if another instance holds the lock or the cache directory is absent.
extern "C" void nativeOsLogExport() {
    const int lock = open(kLock, O_CREAT | O_RDWR, 0600);
    if (lock < 0 || flock(lock, LOCK_EX | LOCK_NB) != 0) return;
    for (unsigned pass = 1; pass <= kPasses; ++pass) {
        mkdir(kDirectory, 0755);
        struct stat st = {};
        if (stat(kDirectory, &st) == 0 && S_ISDIR(st.st_mode)) writePass(pass);
        sleep(kPeriodSeconds);
    }
}
