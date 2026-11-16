// SPDX-License-Identifier: MIT
// The render server primes the OS inventory on a background thread while the desktop is idle. Kernel entry points
// use default function constants; vertex entry points use a matching fragment function with BGRA8 and RGBA16Float targets.
// prime-done records fully compiled libraries. prime-fn2 and prime-refused2 record successful and refused entry points
// for the current runtime build; prime-current identifies an interrupted compile and prime-bad records that entry point.
// The next process resumes successful work and retains refusal counts. PRc and prime.log report the pass's completion.
#pragma once
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <TargetConditionals.h>
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
#include <sys/sysctl.h>
#include <unistd.h>
#include <set>
#include <string>
#include <vector>

extern "C" unsigned native_bridge_metal_recent_commits(uint64_t windowUs);

@interface NSObject (NBPrimeZeroConstants)
- (id<MTLFunction>)nb_newFunctionWithZeroConstants:(NSString *)name;
- (NSArray<NSString *> *)nb_primeFunctionNames;
@end

// Priming's progress for the telemetry table (the PR row): what the thread does (1 a pipeline compiles, 2 it waits for the desktop to
// settle, 3 it opens a library and its functions), libraries done of the list, pipelines made, and the most command buffers the last wait
// saw in 250 ms.
extern "C" {volatile uint32_t native_prime_compiling, native_prime_libraries_done, native_prime_libraries_total, native_prime_made, native_prime_rate,
               native_prime_functions_resumed;
// For the PRb row: the libraries this process's passes compiled, those a pass leaves out, and the pass's scope (1 the bundles loaded
// in this process, 2 the whole system).
volatile uint32_t native_prime_libraries_primed, native_prime_libraries_skipped, native_prime_scope;
volatile uint32_t native_prime_pass_complete, native_prime_libraries_failed, native_prime_functions_refused,
                  native_prime_functions_skipped, native_prime_inventory_failed;}

extern "C" void native_bridge_metal_quiet_problems(bool);
namespace prime {
#if TARGET_OS_OSX
// macOS: the system apps are in /System/Applications; the OS cryptex is reached through /System/Cryptexes/OS as on iPadOS.
static const char *const kRoots[] = {"/System/Library", "/System/Applications", "/Applications", "/System/Cryptexes/OS/System/Library"};
#else
static const char *const kRoots[] = {"/System/Library", "/Applications", "/System/Cryptexes/OS/System/Library"};
#endif

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

static std::string bootSession() {
    char uuid[64] = {};
    size_t size = sizeof uuid;
    if (sysctlbyname("kern.bootsessionuuid", uuid, &size, nullptr, 0) != 0) return "unknown";
    uuid[sizeof uuid - 1] = 0;
    return uuid;
}

// The libraries: first the paths of the list file beside the bundle (NativeAdrenoMetal.primelist, every .metallib of the OS image, one
// absolute path per line), because the render server's sandbox refuses to list most directories (a scan alone found 8 of 166). Only
// without a list does the scan run: it walks every directory it can reach and held a core for 8 s with frames stalling (i557u, 120 s
// into the boot). A path is taken once, by its file identity.
// `note` gets the list's outcome: its path, the lines read, the libraries kept and the stat failures with the last errno.
static std::vector<std::string> libraries(const std::string &listPath, std::string &note) {
    std::vector<std::string> found;
    std::set<std::pair<dev_t, ino_t>> seen;
    unsigned lines = 0, failed = 0, invalid = 0;
    int lastErrno = 0;
    std::set<std::string> list = listPath.empty() ? std::set<std::string>() : readLines(listPath);
    const char *source = listPath.empty() ? "(none)" : listPath.c_str();
    if (list.empty()) {list = embeddedLines(); source = "embedded";}
    const bool inventory = !list.empty();
    for (const std::string &path : list) {
        ++lines;
        struct stat st = {};
        if (path.empty() || path[0] != '/') {++invalid; continue;}
        if (stat(path.c_str(), &st) != 0) {++failed; lastErrno = errno; continue;}
        if (!S_ISREG(st.st_mode)) {++invalid; continue;}
        if (seen.insert({st.st_dev, st.st_ino}).second) found.push_back(path);
    }
    char line[200];
    snprintf(line, sizeof line, "list %s lines=%u kept=%zu stat-failed=%u errno=%d invalid=%u inventory=%u", source, lines,
             found.size(), failed, lastErrno, invalid, inventory ? 1u : 0u);
    note = line;
    native_prime_inventory_failed = failed + invalid + (inventory ? 0u : 1u);
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
// macOS's WindowServer composes only when something on screen changes: without input the whole system submitted nothing to the GPU in
// a quarter of a run's two-second windows, and a desktop in use ran at 18 to 20 submissions a second, under the bar above, so priming
// compiled through it (in four runs' process samples the priming thread was compiling or writing its progress in 88 to 94 % of the
// samples, in two of them beside the encoding thread's draws). There any command buffer in the window is the render server at work.
#if TARGET_OS_OSX
static constexpr unsigned kQuietCommits = 0;
#else
static constexpr unsigned kQuietCommits = 8;
#endif
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
        if (commits > kQuietCommits) {busy = true; quietSince = 0; native_prime_compiling = 2;}
        else if (!busy) return 0;
        else if (!quietSince) quietSince = now;
        // After animation, two seconds without it before the next compile, so a pause between two swipes does not let one in.
        else if (now - quietSince >= 2000) return now - began;
        usleep(100000);
    }
}

#if TARGET_OS_OSX
// macOS: whether a library belongs to code loaded in this process. The render server runs as a system user (launchd starts
// WindowServer with no user name) and the apps as the console user, each user's pipeline cache in a directory of its own that
// another user's processes are refused (SharedShaderCache.h), so what the render server primes serves the render server alone.
// A library is its bundle's: the nearest directory above it whose name has an extension (QuartzCore.framework, a .bundle, an
// .app), which has an image loaded here when this process may ask for the library. Of the 400 libraries of a macOS 27 image, 57 are
// of frameworks WindowServer loads; 165 are other GPUs' driver bundles, which nothing loads on this GPU. Paths compare without the
// OS cryptex's prefix. Asked when priming reaches the library: one left out is not listed as finished, so a later start asks again.
static bool libraryOfLoadedBundle(const std::string &path) {
    static const char kCryptex[] = "/System/Cryptexes/OS";
    auto plain = [](const char *text) {
        return std::string(strncmp(text, kCryptex, sizeof kCryptex - 1) == 0 && text[sizeof kCryptex - 1] == '/' ? text + sizeof kCryptex - 1 : text);
    };
    std::string bundle = plain(path.c_str());
    for (;;) {
        const size_t slash = bundle.rfind('/');
        if (slash == std::string::npos || slash == 0) return false;
        bundle.resize(slash);
        if (bundle.find('.', bundle.rfind('/') + 1) != std::string::npos) break;
    }
    bundle += '/';
    for (uint32_t index = 0, count = _dyld_image_count(); index < count; ++index) {
        const char *name = _dyld_get_image_name(index);
        if (name && plain(name).compare(0, bundle.size(), bundle) == 0) return true;
    }
    return false;
}
// macOS: whether a library is in another GPU's driver bundle (/System/Library/Extensions/<name>.bundle: AGXMetal* and the like, 167 of
// the 400 libraries of a macOS 27 image). IOGPU loads the bundle of the accelerator found, which here is this plugin, so nothing
// asks for those libraries on this GPU.
static bool libraryOfDriverBundle(const std::string &path) {
    static const char kCryptex[] = "/System/Cryptexes/OS", kExtensions[] = "/System/Library/Extensions/";
    const char *plain = path.c_str();
    if (strncmp(plain, kCryptex, sizeof kCryptex - 1) == 0 && plain[sizeof kCryptex - 1] == '/') plain += sizeof kCryptex - 1;
    if (strncmp(plain, kExtensions, sizeof kExtensions - 1) != 0) return false;
    const char *slash = strchr(plain + sizeof kExtensions - 1, '/');
    return slash && slash - plain > 7 && strncmp(slash - 7, ".bundle", 7) == 0;
}
// A universal archive whose slices all contain Apple GPU machine code.
static bool appleGpuArchive(const std::string &path) {
    FILE *file = fopen(path.c_str(), "rb");
    if (!file) return false;
    struct stat st = {};
    uint8_t header[8];
    auto word = [](const uint8_t *p) {return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3];};
    bool archive = fstat(fileno(file), &st) == 0 && fread(header, 1, sizeof header, file) == sizeof header &&
                   !memcmp(header, "\xca\xfe\xba\xbe", 4);
    const uint32_t count = archive ? word(header + 4) : 0;
    archive = archive && count && st.st_size >= 8 && count <= (uint64_t)(st.st_size - 8) / 20;
    for (uint32_t i = 0; archive && i < count; ++i) {
        uint8_t slice[20];
        if (fread(slice, 1, sizeof slice, file) != sizeof slice) {archive = false; break;}
        const uint64_t offset = word(slice + 8), size = word(slice + 12);
        archive = word(slice) == 0x01000013 && size && offset <= (uint64_t)st.st_size && size <= (uint64_t)st.st_size - offset;
    }
    fclose(file);
    return archive;
}
#endif

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
        if (callee.functionConstantsDictionary.count && [lib respondsToSelector:@selector(nb_newFunctionWithZeroConstants:)])
            callee = [(NSObject *)lib nb_newFunctionWithZeroConstants:symbol];
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

static id<MTLFunction> defaultFunction(id<MTLLibrary> lib, NSString *name, unsigned &zeroed) {
    id<MTLFunction> function = [lib newFunctionWithName:name];
    if (function.functionConstantsDictionary.count && [lib respondsToSelector:@selector(nb_newFunctionWithZeroConstants:)]) {
        function = [(NSObject *)lib nb_newFunctionWithZeroConstants:name];
        if (function) ++zeroed;
    }
    return function;
}

// Canonical packed inputs follow the function's reflected scalar type and lane count.
static MTLVertexDescriptor *vertexInputs(id<MTLFunction> function) {
    MTLVertexDescriptor *descriptor = [MTLVertexDescriptor vertexDescriptor];
    for (MTLVertexAttribute *attribute in function.vertexAttributes) {
        const NSUInteger index = attribute.attributeIndex, type = attribute.attributeType;
        if (index >= 31 || !attribute.isActive) continue;
        MTLVertexFormat format = MTLVertexFormatInvalid;
        NSUInteger bytes = 0;
        struct Family {NSUInteger type, bytes; MTLVertexFormat scalar, vector2;};
        static const Family families[] = {
            {MTLDataTypeFloat,4,MTLVertexFormatFloat,MTLVertexFormatFloat2},
            {MTLDataTypeHalf,2,MTLVertexFormatHalf,MTLVertexFormatHalf2},
            {MTLDataTypeInt,4,MTLVertexFormatInt,MTLVertexFormatInt2},
            {MTLDataTypeUInt,4,MTLVertexFormatUInt,MTLVertexFormatUInt2},
            {MTLDataTypeShort,2,MTLVertexFormatShort,MTLVertexFormatShort2},
            {MTLDataTypeUShort,2,MTLVertexFormatUShort,MTLVertexFormatUShort2},
            {MTLDataTypeChar,1,MTLVertexFormatChar,MTLVertexFormatChar2},
            {MTLDataTypeUChar,1,MTLVertexFormatUChar,MTLVertexFormatUChar2},
        };
        for (const Family &family : families) if (type >= family.type && type < family.type + 4) {
            const NSUInteger lanes = type - family.type + 1;
            format = lanes == 1 ? family.scalar : (MTLVertexFormat)(family.vector2 + lanes - 2);
            bytes = family.bytes * lanes;
            break;
        }
        if (format == MTLVertexFormatInvalid) continue;
        descriptor.attributes[index].format = format;
        descriptor.attributes[index].bufferIndex = index;
        descriptor.layouts[index].stride = bytes;
        descriptor.layouts[index].stepFunction = MTLVertexStepFunctionPerVertex;
    }
    return descriptor;
}

// One library: every function it names, in its order; returns the log line.
static std::string library(id<MTLDevice> device, const std::string &path, const std::string &current, const std::set<std::string> &bad,
                           const std::string &functionsPath, const std::set<std::string> &functionsDone,
                           const std::string &refusedPath, const std::set<std::string> &refusedDone,
                           const std::string &build, bool &complete) {
    complete = false;
    unsigned kernels = 0, renders = 0, made = 0, refused = 0, skipped = 0, zeroed = 0, linkedMade = 0, halfMade = 0;
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
        NSArray<NSString *> *entries = [lib respondsToSelector:@selector(nb_primeFunctionNames)] ? [(NSObject *)lib nb_primeFunctionNames] : names;
        for (NSString *name in entries) {
            @autoreleasepool {
                const std::string key = path + "\t" + name.UTF8String;
                const std::string doneKey = key + "\t" + build;
                native_prime_compiling = 3;
                if (bad.count(doneKey)) {++skipped; continue;}
                const std::string attemptKey = doneKey + "-inputs3";
                if (refusedDone.count(attemptKey)) {++refused; continue;}
                if (functionsDone.count(doneKey)) {++made; native_prime_functions_resumed = native_prime_functions_resumed + 1; continue;}
                id<MTLFunction> function = defaultFunction(lib, name, zeroed);
                if (!function) {++refused; appendLine(refusedPath, attemptKey); continue;}
                const MTLFunctionType type = function.functionType;
                if (type != MTLFunctionTypeKernel && type != MTLFunctionTypeVertex) continue;
                waited += waitForQuiet();
                writeCurrent(current, doneKey);
                native_prime_compiling = 1;
                bool ok = false;
                @try {
                    if (type == MTLFunctionTypeKernel) {
                        ++kernels;
                        ok = [device newComputePipelineStateWithFunction:function error:&error] != nil;
                        if (!ok && [error.localizedDescription containsString:@"LinkedFunction"]) {
                            ok = linkedKernel(device, lib, function, &error);
                            if (ok) ++linkedMade;
                        }
                    } else {
                        ++renders;
                        MTLRenderPipelineDescriptor *d = [MTLRenderPipelineDescriptor new];
                        d.vertexFunction = function;
                        d.vertexDescriptor = vertexInputs(function);
                        d.colorAttachments[0].pixelFormat = MTLPixelFormatBGRA8Unorm;
                        // The fragment function of the same stem under the libraries' usual spellings; without one the pipeline still
                        // rasterizes (Metal refuses an unrasterized pipeline whose vertex function returns a position, int-245: 732 refusals).
                        static NSString *const kPairs[][2] = {{@"_vert_", @"_frag_"}, {@"_vertex", @"_fragment"}, {@"Vertex", @"Fragment"},
                                                              {@"vertex", @"fragment"}, {@"_vs", @"_fs"}, {@"_vert", @"_frag"}, {@"VS", @"PS"}};
                        for (const auto &pair : kPairs) {
                            NSString *fragment = [name stringByReplacingOccurrencesOfString:pair[0] withString:pair[1]];
                            if (![fragment isEqualToString:name] && [names containsObject:fragment]) {
                                d.fragmentFunction = defaultFunction(lib, fragment, zeroed);
                                if (d.fragmentFunction) break;
                            }
                        }
                        ok = [device newRenderPipelineStateWithDescriptor:d error:&error] != nil;
                        if (!ok && [error.localizedDescription containsString:@"RasterizedVoidVertex"]) {
                            d.rasterizationEnabled = NO;
                            error = nil;
                            ok = [device newRenderPipelineStateWithDescriptor:d error:&error] != nil;
                        }
                        // The render server's framebuffer is half-float (RGhA) on this board: its pipelines draw into RGBA16Float, a state
                        // of its own to Turnip, so the same pair is made for it too.
                        if (ok) {
                            d.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA16Float;
                            NSError *half = nil;
                            if ([device newRenderPipelineStateWithDescriptor:d error:&half]) ++halfMade;
                        }
                    }
                } @catch (NSException *exception) {
                    ok = false;
                }
                native_prime_compiling = 0;
                appendLine(ok ? functionsPath : refusedPath, ok ? doneKey : attemptKey);
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
    native_prime_functions_refused = native_prime_functions_refused + refused;
    native_prime_functions_skipped = native_prime_functions_skipped + skipped;
    complete = refused == 0 && skipped == 0;
    char line[200];
    snprintf(line, sizeof line, " kernels=%u renders=%u made=%u half=%u zeroed=%u linked=%u refused=%u skipped=%u ms=%llu waited=%llu", kernels, renders,
             made, halfMade, zeroed, linkedMade, refused, skipped, (unsigned long long)(nowMs() - began), (unsigned long long)waited);
    return path + line + firstRefusal;
}

// The thread: waits for the desktop to settle, then works through the libraries not finished yet.
// `wholeSystem` (macOS): this process fills the system shader cache (SharedShaderCache.h), which every process reads, so every
// library of the list is primed but those of other GPUs' driver bundles; without it only the libraries of bundles loaded here are,
// since the cache then serves this process alone.
static void run(id<MTLDevice> device, std::string root, unsigned delaySeconds, const std::string &listPath, bool wholeSystem) {
    pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0);
    native_bridge_metal_quiet_problems(true);
    while (root.size() > 1 && root.back() == '/') root.pop_back();
    const std::string donePath = root + "/prime-done", badPath = root + "/prime-bad", current = root + "/prime-current",
                      logPath = root + "/prime.log";
    const std::string resume = root + "/prime-resume";
    const bool planned = access(resume.c_str(), F_OK) == 0;
    if (planned) unlink(resume.c_str());
    else for (const std::string &line : readLines(current)) appendLine(badPath, line);
    unlink(current.c_str());
    sleep(delaySeconds);
    const std::set<std::string> bad = readLines(badPath);
    std::set<std::string> done = readLines(donePath);
    const std::string functionsPath = root + "/prime-fn2", refusedPath = root + "/prime-refused2";
    const std::set<std::string> functionsDone = readLines(functionsPath);
    const std::set<std::string> refusedDone = readLines(refusedPath);
    const uint64_t began = nowMs();
    unsigned libs = 0, finished = 0, foreign = 0;
    std::string note;
    const std::vector<std::string> paths = libraries(listPath, note);
    const std::string build = buildId() + "-prime2";
    const std::string boot = bootSession();
#if TARGET_OS_OSX
    const unsigned scope = wholeSystem ? 2 : 1;
#else
    const unsigned scope = 2;
#endif
    appendLine(logPath, note + " inputs=3 boot=" + boot + " scope=" + std::to_string(scope) + " build=" + build);
    std::set<std::string> binaryArchives;
#if TARGET_OS_OSX
    if (wholeSystem) for (const std::string &path : paths) if (appleGpuArchive(path)) {
        binaryArchives.insert(path);
        appendLine(logPath, "archive " + path + " cpu=0x01000013");
    }
#endif
    auto selected = [&](const std::string &path) {
#if TARGET_OS_OSX
        return wholeSystem ? !libraryOfDriverBundle(path) && !binaryArchives.count(path) : libraryOfLoadedBundle(path);
#else
        (void)path;
        (void)wholeSystem;
        return true;
#endif
    };
    unsigned eligible = 0;
    for (const std::string &path : paths) if (selected(path)) ++eligible;
    native_prime_libraries_total = eligible;
    native_prime_scope = scope;
    for (const std::string &path : paths) {
        ++libs;
        native_prime_libraries_done = finished;
        struct stat st = {};
        if (stat(path.c_str(), &st) != 0) {native_prime_libraries_failed = native_prime_libraries_failed + 1; continue;}
        if (!selected(path)) {
            ++foreign;
            native_prime_libraries_skipped = foreign;
            continue;
        }
        // The runtime's and Turnip's builds end the id: a newer one primes every library again (buildId()).
        const std::string id = path + "\t" + std::to_string((long long)st.st_size) + "\t" + std::to_string((long long)st.st_mtimespec.tv_sec) + "\t" + build;
        if (done.count(id)) {++finished; continue;}
        bool complete = false;
        appendLine(logPath, library(device, path, current, bad, functionsPath, functionsDone, refusedPath, refusedDone, build, complete));
        native_prime_libraries_primed = native_prime_libraries_primed + 1;
        if (complete) {
            appendLine(donePath, id);
            done.insert(id);
            ++finished;
        } else {
            native_prime_libraries_failed = native_prime_libraries_failed + 1;
        }
    }
    native_prime_libraries_done = finished;
    native_prime_compiling = 0;
    native_prime_pass_complete = eligible && finished == eligible && !native_prime_libraries_failed && !native_prime_inventory_failed ? 1 : 2;
    char line[512];
    snprintf(line, sizeof line, "pass boot=%s scope=%u build=%s inputs=3 libraries=%u eligible=%u finished=%u failed=%u refused=%u bad=%u missing=%u excluded=%u binary=%zu complete=%u ms=%llu",
             boot.c_str(), scope, build.c_str(), libs, eligible, finished, native_prime_libraries_failed, native_prime_functions_refused, native_prime_functions_skipped,
             native_prime_inventory_failed, foreign, binaryArchives.size(), native_prime_pass_complete, (unsigned long long)(nowMs() - began));
    appendLine(logPath, line);
}
}  // namespace prime
