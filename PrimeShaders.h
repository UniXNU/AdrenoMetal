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
#include <dlfcn.h>
#include <fts.h>
#include <mach-o/dyld.h>
#include <mach-o/getsect.h>
#include <mach-o/loader.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <sys/stat.h>
#include <unistd.h>
#include <set>
#include <string>
#include <vector>

extern "C" unsigned native_bridge_metal_recent_commits(uint64_t windowUs);

@interface NSObject (NBPrimeZeroConstants)
- (id<MTLFunction>)nb_newFunctionWithZeroConstants:(NSString *)name;
@end

// Priming's progress for the telemetry table (the PR row): what the thread does (1 a pipeline compiles, 2 it waits for the desktop to
// settle, 3 it opens a library and its functions), libraries done of the list, pipelines made, and the most command buffers the last wait
// saw in 250 ms.
extern "C" {volatile uint32_t native_prime_compiling, native_prime_libraries_done, native_prime_libraries_total, native_prime_made, native_prime_rate;}

extern "C" void native_bridge_metal_quiet_problems(bool);
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
// The same list carried in this executable's __DATA,__primelist section (build-adreno-metal-plugin.py), for a render server whose sandbox
// refuses to read the file beside the bundle.
static std::set<std::string> embeddedLines() {
    std::set<std::string> lines;
    Dl_info info = {};
    if (!dladdr(reinterpret_cast<const void *>(&embeddedLines), &info) || !info.dli_fbase) return lines;
    unsigned long size = 0;
    const uint8_t *data = getsectiondata(static_cast<const struct mach_header_64 *>(info.dli_fbase), "__DATA", "__primelist", &size);
    if (!data) return lines;
    const std::string text(reinterpret_cast<const char *>(data), size);
    for (size_t at = 0; at < text.size();) {
        size_t end = text.find('\n', at);
        if (end == std::string::npos) end = text.size();
        if (end > at) lines.insert(text.substr(at, end - at));
        at = end + 1;
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
// absolute path per line), because the render server's sandbox refuses to list most directories (a scan alone found 8 of 166). Only
// without a list does the scan run: it walks every directory it can reach and held a core for 8 s with frames stalling (i557u, 120 s
// into the boot). A path is taken once, by its file identity.
// `note` gets the list's outcome: its path, the lines read, the libraries kept and the stat failures with the last errno.
static std::vector<std::string> libraries(const std::string &listPath, std::string &note) {
    std::vector<std::string> found;
    std::set<std::pair<dev_t, ino_t>> seen;
    unsigned lines = 0, failed = 0;
    int lastErrno = 0;
    std::set<std::string> list = listPath.empty() ? std::set<std::string>() : readLines(listPath);
    const char *source = listPath.empty() ? "(none)" : listPath.c_str();
    if (list.empty()) {list = embeddedLines(); source = "embedded";}
    for (const std::string &path : list) {
        ++lines;
        struct stat st = {};
        if (path.empty() || path[0] != '/') continue;
        if (stat(path.c_str(), &st) != 0) {++failed; lastErrno = errno; continue;}
        if (S_ISREG(st.st_mode) && seen.insert({st.st_dev, st.st_ino}).second) found.push_back(path);
    }
    char line[200];
    snprintf(line, sizeof line, "list %s lines=%u kept=%zu stat-failed=%u errno=%d", source, lines,
             found.size(), failed, lastErrno);
    note = line;
    for (const char *root : kRoots) {
        if (!found.empty()) break;
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

// Waits until the render server is not animating: more than 4 command buffers in 250 ms (16 fps; an idle desktop redraws at about 12 fps)
// means frames are being drawn, and a pipeline compile then competes with them for the CPU and the driver (int-244: priming during the
// scripted swipes raised the median frame interval from 19 to 30 ms). A higher bar let compiles in once they had slowed the frames under
// it (int-250: 12 per 250 ms, 460 pipelines made during the swipes at a 48 ms median). With a fixed 120 Hz vsync the idle desktop draws 6
// per 250 ms (i516), so the bar is 8: still under any animation, which draws 15 to 30. A desktop that never drops under it still gets one
// pipeline every 10 s (i513-i515: priming sat at 15/162 for the whole run without making a pipeline). Returns the milliseconds it waited.
static uint64_t waitForQuiet() {
    const uint64_t began = nowMs();
    uint64_t quietSince = 0;
    bool busy = false;
    native_prime_rate = 0;
    for (;;) {
        const uint64_t now = nowMs();
        const unsigned commits = native_bridge_metal_recent_commits(250000);
        if (commits > native_prime_rate) native_prime_rate = commits;
        if (now - began >= 10000) return now - began;
        if (commits > 8) {busy = true; quietSince = 0; native_prime_compiling = 2;}
        else if (!busy) return 0;
        else if (!quietSince) quietSince = now;
        // After animation, two seconds without it before the next compile, so a pause between two swipes does not let one in.
        else if (now - quietSince >= 2000) return now - began;
        usleep(100000);
    }
}

extern "C" const char *nmb_runtime_build(void);

// The LC_UUID of the loaded image whose path ends in `suffix`, "unknown" without one.
static std::string imageUuid(const char *suffix) {
    for (uint32_t index = 0; index < _dyld_image_count(); ++index) {
        const char *name = _dyld_get_image_name(index);
        const size_t length = strlen(name), want = strlen(suffix);
        if (length < want || strcmp(name + length - want, suffix) != 0) continue;
        const auto *header = reinterpret_cast<const mach_header_64 *>(_dyld_get_image_header(index));
        if (!header || header->magic != MH_MAGIC_64) break;
        const auto *command = reinterpret_cast<const load_command *>(header + 1);
        for (uint32_t i = 0; i < header->ncmds; ++i) {
            if (command->cmd == LC_UUID) {
                const uint8_t *uuid = reinterpret_cast<const uuid_command *>(command)->uuid;
                char text[33];
                for (int k = 0; k < 16; ++k) snprintf(text + 2 * k, 3, "%02x", uuid[k]);
                return text;
            }
            command = reinterpret_cast<const load_command *>(reinterpret_cast<const uint8_t *>(command) + command->cmdsize);
        }
        break;
    }
    return "unknown";
}

// What the cached pipelines were made by, which ends each finished library's id: the runtime's translator source, which keys the shader
// cache (int-246: after a runtime change the Vulkan cache had 0 hits and 214 misses, while prime-done still listed every library), and
// Turnip's build, which keys its pipeline cache (int-249: 4099 misses after a Turnip rebuild). A change of either primes every library
// again; a plugin rebuild alone keeps what was finished.
static std::string buildId() {
    return std::string(nmb_runtime_build()) + "-" + imageUuid("/libvulkan_freedreno.dylib");
}

// A kernel that calls visible functions it does not define (`NeedsInput(LinkedFunction { symbol: "..." })`): link the library's function of
// each symbol the refusals name and make the pipeline again, until it is made or a symbol names no function of the library.
static bool linkedKernel(id<MTLDevice> device, id<MTLLibrary> lib, id<MTLFunction> kernel, NSError **error) {
    NSMutableArray<id<MTLFunction>> *linked = [NSMutableArray array];
    NSMutableSet<NSString *> *symbols = [NSMutableSet set];
    for (unsigned round = 0; round < 16; ++round) {
        NSString *why = (*error).localizedDescription;
        NSRange at = [why rangeOfString:@"LinkedFunction { symbol: \""];
        if (at.location == NSNotFound) return false;
        NSString *rest = [why substringFromIndex:NSMaxRange(at)];
        NSRange end = [rest rangeOfString:@"\""];
        if (end.location == NSNotFound) return false;
        NSString *symbol = [rest substringToIndex:end.location];
        if ([symbols containsObject:symbol]) return false;
        [symbols addObject:symbol];
        id<MTLFunction> callee = [lib newFunctionWithName:symbol];
        if (!callee) return false;
        [linked addObject:callee];
        MTLComputePipelineDescriptor *d = [MTLComputePipelineDescriptor new];
        d.computeFunction = kernel;
        MTLLinkedFunctions *functions = [MTLLinkedFunctions linkedFunctions];
        functions.functions = linked;
        d.linkedFunctions = functions;
        *error = nil;
        if ([device newComputePipelineStateWithDescriptor:d options:MTLPipelineOptionNone reflection:nil error:error]) return true;
    }
    return false;
}

// One library: every function it names, in its order; returns the log line.
static std::string library(id<MTLDevice> device, const std::string &path, const std::string &current, const std::set<std::string> &bad) {
    unsigned kernels = 0, renders = 0, made = 0, refused = 0, skipped = 0, zeroed = 0, linkedMade = 0;
    uint64_t waited = 0;
    std::string firstRefusal;   // the first refused function and why, so the log tells translator gaps from unspecialized constants
    const uint64_t began = nowMs();
    @autoreleasepool {
        NSError *error = nil;
        // Reading and parsing a library is work too: not during an animation either.
        waited += waitForQuiet();
        native_prime_compiling = 3;
        id<MTLLibrary> lib = [device newLibraryWithURL:[NSURL fileURLWithPath:@(path.c_str())] error:&error];
        native_prime_compiling = 0;
        if (!lib) return path + " library refused: " + (error ? error.localizedDescription.UTF8String : "no error");
        NSArray<NSString *> *names = lib.functionNames;
        for (NSString *name in names) {
            @autoreleasepool {
                const std::string key = path + "\t" + name.UTF8String;
                native_prime_compiling = 3;
                if (bad.count(key)) {++skipped; continue;}
                id<MTLFunction> function = [lib newFunctionWithName:name];
                if (!function) {++refused; continue;}
                const MTLFunctionType type = function.functionType;
                if (type != MTLFunctionTypeKernel && type != MTLFunctionTypeVertex) continue;
                waited += waitForQuiet();
                writeCurrent(current, key);
                native_prime_compiling = 1;
                bool ok = false;
                @try {
                    if (type == MTLFunctionTypeKernel) {
                        ++kernels;
                        ok = [device newComputePipelineStateWithFunction:function error:&error] != nil;
                        // A kernel that uses a function constant's value needs one: prime the variant with every constant zero, which
                        // still takes the module through translation and the driver (int-245: 2901 kernels refused for want of one).
                        if (!ok && [error.localizedDescription containsString:@"FunctionConstant"] &&
                            [lib respondsToSelector:@selector(nb_newFunctionWithZeroConstants:)]) {
                            id<MTLFunction> zero = [(NSObject *)lib nb_newFunctionWithZeroConstants:name];
                            if (zero) {error = nil; ok = [device newComputePipelineStateWithFunction:zero error:&error] != nil; if (ok) ++zeroed;}
                        }
                        if (!ok && [error.localizedDescription containsString:@"LinkedFunction"]) {
                            ok = linkedKernel(device, lib, function, &error);
                            if (ok) ++linkedMade;
                        }
                    } else {
                        ++renders;
                        MTLRenderPipelineDescriptor *d = [MTLRenderPipelineDescriptor new];
                        d.vertexFunction = function;
                        d.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
                        // The fragment function of the same stem under the libraries' usual spellings; without one the pipeline still
                        // rasterizes (Metal refuses an unrasterized pipeline whose vertex function returns a position, int-245: 732 refusals).
                        static NSString *const kPairs[][2] = {{@"_vert_", @"_frag_"}, {@"_vertex", @"_fragment"}, {@"Vertex", @"Fragment"},
                                                              {@"vertex", @"fragment"}, {@"_vs", @"_fs"}, {@"_vert", @"_frag"}, {@"VS", @"PS"}};
                        for (const auto &pair : kPairs) {
                            NSString *fragment = [name stringByReplacingOccurrencesOfString:pair[0] withString:pair[1]];
                            if (![fragment isEqualToString:name] && [names containsObject:fragment]) {
                                d.fragmentFunction = [lib newFunctionWithName:fragment];
                                if (d.fragmentFunction) break;
                            }
                        }
                        ok = [device newRenderPipelineStateWithDescriptor:d error:&error] != nil;
                    }
                } @catch (NSException *exception) {
                    ok = false;
                }
                native_prime_compiling = 0;
                ok ? ++made : ++refused;
                if (ok) native_prime_made = native_prime_made + 1;
                if (!ok && firstRefusal.empty()) {
                    NSString *why = error.localizedDescription ?: @"no error";
                    firstRefusal = std::string(" first-refused=") + name.UTF8String + ": " + why.UTF8String;
                    if (firstRefusal.size() > 220) firstRefusal.resize(220);
                    for (char &c : firstRefusal) if (c == '\n' || c == '\t') c = ' ';
                }
                error = nil;
                // Low priority already; a short pause between pipelines keeps the render server's own work ahead.
                usleep(20000);
            }
        }
    }
    unlink(current.c_str());
    char line[180];
    snprintf(line, sizeof line, " kernels=%u renders=%u made=%u zeroed=%u linked=%u refused=%u skipped=%u ms=%llu waited=%llu", kernels, renders,
             made, zeroed, linkedMade, refused, skipped, (unsigned long long)(nowMs() - began), (unsigned long long)waited);
    return path + line + firstRefusal;
}

// Libraries finished elsewhere (the pipeline caches compiled ahead on the host, CacheImport.h), in prime-done's line form.
static std::set<std::string> &extraDone() {static std::set<std::string> lines; return lines;}

// The thread: waits for the desktop to settle, then works through the libraries not finished yet.
static void run(id<MTLDevice> device, std::string root, unsigned delaySeconds, const std::string &listPath) {
    pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0);
    native_bridge_metal_quiet_problems(true);
    while (root.size() > 1 && root.back() == '/') root.pop_back();
    const std::string donePath = root + "/prime-done", badPath = root + "/prime-bad", current = root + "/prime-current",
                      logPath = root + "/prime.log";
    // A function was being compiled when the process ended: it is not tried again.
    for (const std::string &line : readLines(current)) appendLine(badPath, line);
    unlink(current.c_str());
    sleep(delaySeconds);
    const std::set<std::string> bad = readLines(badPath);
    std::set<std::string> done = readLines(donePath);
    done.insert(extraDone().begin(), extraDone().end());
    const uint64_t began = nowMs();
    unsigned libs = 0, finished = 0;
    std::string note;
    const std::vector<std::string> paths = libraries(listPath, note);
    const std::string build = buildId();
    appendLine(logPath, note + " build=" + build);
    native_prime_libraries_total = (uint32_t)paths.size();
    for (const std::string &path : paths) {
        ++libs;
        native_prime_libraries_done = finished;
        struct stat st = {};
        if (stat(path.c_str(), &st) != 0) continue;
        // The runtime's and Turnip's builds end the id: a newer one primes every library again (buildId()).
        const std::string id = path + "\t" + std::to_string((long long)st.st_size) + "\t" + std::to_string((long long)st.st_mtimespec.tv_sec) + "\t" + build;
        if (done.count(id)) {++finished; continue;}
        appendLine(logPath, library(device, path, current, bad));
        appendLine(donePath, id);
        done.insert(id);
        ++finished;
    }
    native_prime_libraries_done = finished;
    native_prime_compiling = 0;
    char line[120];
    snprintf(line, sizeof line, "pass libraries=%u finished=%u ms=%llu", libs, finished, (unsigned long long)(nowMs() - began));
    appendLine(logPath, line);
}
}  // namespace prime
