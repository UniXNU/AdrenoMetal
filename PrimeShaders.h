// SPDX-License-Identifier: MIT
// Shader priming in the render server: once the desktop has settled, every Metal library of the OS volume goes through the runtime on a
// background thread, so its functions are translated and compiled into the persistent cache directory before anything asks for them.
// Each kernel function becomes a compute pipeline; each vertex function becomes a render pipeline with the library's fragment function of
// the same stem (`_vert_` -> `_frag_`, or a fragment-less pipeline) and one BGRA8 attachment. Pipelines a caller makes with other constants
// or attachments still compile then, but their functions' modules are decoded and the translator has seen every function once.
// State in the cache directory: prime-done lists finished libraries (path, size, mtime), prime-bad the functions that ended the process
// (prime-current names the function being compiled; a start that finds it moves it to prime-bad), prime.log one line per library.
// A library listed as finished is never read again, so a restart resumes where the previous run stopped.
#pragma once
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <fts.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <set>
#include <string>
#include <vector>

namespace prime {
static const char *const kRoots[] = {"/System/Library", "/Applications", "/System/Cryptexes/OS/System/Library"};

static std::set<std::string> readLines(const std::string &path) {
    std::set<std::string> lines;
    if (FILE *f = fopen(path.c_str(), "r")) {
        char line[1024];
        while (fgets(line, sizeof line, f)) {
            line[strcspn(line, "\n")] = 0;
            if (line[0]) lines.insert(line);
        }
        fclose(f);
    }
    return lines;
}
static void appendLine(const std::string &path, const std::string &line) {
    if (FILE *f = fopen(path.c_str(), "a")) {fprintf(f, "%s\n", line.c_str()); fclose(f);}
}
static void writeCurrent(const std::string &path, const std::string &line) {
    if (FILE *f = fopen(path.c_str(), "w")) {fprintf(f, "%s\n", line.c_str()); fflush(f); fsync(fileno(f)); fclose(f);}
}
static uint64_t nowMs() {return clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1000000;}

// The libraries: first the paths of the list file beside the bundle (NativeAdrenoMetal.primelist, every .metallib of the OS image, one
// absolute path per line), because the render server's sandbox refuses to list most directories (a scan alone found 8 of 166), then
// whatever the scan still reaches. A path is taken once, by its file identity.
static std::vector<std::string> libraries(const std::string &listPath) {
    std::vector<std::string> found;
    std::set<std::pair<dev_t, ino_t>> seen;
    if (!listPath.empty())
        for (const std::string &path : readLines(listPath)) {
            struct stat st = {};
            if (!path.empty() && path[0] == '/' && stat(path.c_str(), &st) == 0 && S_ISREG(st.st_mode) &&
                seen.insert({st.st_dev, st.st_ino}).second)
                found.push_back(path);
        }
    for (const char *root : kRoots) {
        char *paths[] = {const_cast<char *>(root), nullptr};
        FTS *fts = fts_open(paths, FTS_PHYSICAL | FTS_NOCHDIR | FTS_XDEV, nullptr);
        if (!fts) continue;
        while (FTSENT *e = fts_read(fts)) {
            if (e->fts_info != FTS_F || e->fts_namelen < 9 || strcmp(e->fts_name + e->fts_namelen - 9, ".metallib")) continue;
            if (seen.insert({e->fts_statp->st_dev, e->fts_statp->st_ino}).second) found.push_back(e->fts_path);
        }
        fts_close(fts);
    }
    return found;
}

// One library: every function it names, in its order; returns the log line.
static std::string library(id<MTLDevice> device, const std::string &path, const std::string &current, const std::set<std::string> &bad) {
    unsigned kernels = 0, renders = 0, made = 0, refused = 0, skipped = 0;
    const uint64_t began = nowMs();
    @autoreleasepool {
        NSError *error = nil;
        id<MTLLibrary> lib = [device newLibraryWithURL:[NSURL fileURLWithPath:@(path.c_str())] error:&error];
        if (!lib) return path + " library refused";
        NSArray<NSString *> *names = lib.functionNames;
        for (NSString *name in names) {
            @autoreleasepool {
                const std::string key = path + "\t" + name.UTF8String;
                if (bad.count(key)) {++skipped; continue;}
                id<MTLFunction> function = [lib newFunctionWithName:name];
                if (!function) {++refused; continue;}
                const MTLFunctionType type = function.functionType;
                if (type != MTLFunctionTypeKernel && type != MTLFunctionTypeVertex) continue;
                writeCurrent(current, key);
                bool ok = false;
                @try {
                    if (type == MTLFunctionTypeKernel) {
                        ++kernels;
                        ok = [device newComputePipelineStateWithFunction:function error:&error] != nil;
                    } else {
                        ++renders;
                        MTLRenderPipelineDescriptor *d = [MTLRenderPipelineDescriptor new];
                        d.vertexFunction = function;
                        NSString *fragment = [name stringByReplacingOccurrencesOfString:@"_vert_" withString:@"_frag_"];
                        if (![fragment isEqualToString:name] && [names containsObject:fragment]) d.fragmentFunction = [lib newFunctionWithName:fragment];
                        if (!d.fragmentFunction) d.rasterizationEnabled = NO;
                        else d.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
                        ok = [device newRenderPipelineStateWithDescriptor:d error:&error] != nil;
                    }
                } @catch (NSException *exception) {
                    ok = false;
                }
                ok ? ++made : ++refused;
                // Low priority already; a short pause between pipelines keeps the render server's own work ahead.
                usleep(20000);
            }
        }
    }
    unlink(current.c_str());
    char line[160];
    snprintf(line, sizeof line, " kernels=%u renders=%u made=%u refused=%u skipped=%u ms=%llu", kernels, renders, made, refused, skipped,
             (unsigned long long)(nowMs() - began));
    return path + line;
}

// The thread: waits for the desktop to settle, then works through the libraries not finished yet.
static void run(id<MTLDevice> device, std::string root, unsigned delaySeconds, const std::string &listPath) {
    pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0);
    while (root.size() > 1 && root.back() == '/') root.pop_back();
    const std::string donePath = root + "/prime-done", badPath = root + "/prime-bad", current = root + "/prime-current",
                      logPath = root + "/prime.log";
    // A function was being compiled when the process ended: it is not tried again.
    for (const std::string &line : readLines(current)) appendLine(badPath, line);
    unlink(current.c_str());
    sleep(delaySeconds);
    const std::set<std::string> bad = readLines(badPath);
    std::set<std::string> done = readLines(donePath);
    const uint64_t began = nowMs();
    unsigned libs = 0, finished = 0;
    for (const std::string &path : libraries(listPath)) {
        ++libs;
        struct stat st = {};
        if (stat(path.c_str(), &st) != 0) continue;
        const std::string id = path + "\t" + std::to_string((long long)st.st_size) + "\t" + std::to_string((long long)st.st_mtimespec.tv_sec);
        if (done.count(id)) {++finished; continue;}
        appendLine(logPath, library(device, path, current, bad));
        appendLine(donePath, id);
        done.insert(id);
        ++finished;
    }
    char line[120];
    snprintf(line, sizeof line, "pass libraries=%u finished=%u ms=%llu", libs, finished, (unsigned long long)(nowMs() - began));
    appendLine(logPath, line);
}
}  // namespace prime
