// SPDX-License-Identifier: MIT
// Shader priming ahead of the tablet: the plugin's own priming (PrimeShaders.h) over the OS image's libraries on a local mount, through the
// frontend and the runtime over the tablet's driver (Turnip for the Adreno 740, its kernel answered by the host copy of the kernel stack,
// tests/hostsim). Turnip compiles for the GPU without running anything on it, so the cache directory this fills is what the render server's
// own priming would have written; scripts/make-pipeline-cache.py packs it into the image. Each finished library is listed in prime-done under
// the tablet's id for it (its path there, size and mtime, the runtime's build and the tablet Turnip's LC_UUID), so the render server does not
// prime it again.
// argv: loader (host Turnip), cache directory, library list (tablet paths), OS root, OS cryptex root, the tablet's Turnip dylib, [first, count]
#include "../PrimeShaders.h"
#include "../NativeBridgeMetalFrontend.h"
#include "NativeMetalObjects.h"
#include <fstream>

static void onMissing(const char *, const char *) {}

// The LC_UUID of the Mach-O file at `path` (thin), "unknown" without one: the tablet's Turnip, which ends the tablet's prime-done ids.
static std::string fileUuid(const char *path) {
    std::ifstream in(path, std::ios::binary);
    std::vector<char> bytes((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
    if (bytes.size() < sizeof(mach_header_64)) return "unknown";
    const auto *header = reinterpret_cast<const mach_header_64 *>(bytes.data());
    if (header->magic != MH_MAGIC_64) return "unknown";
    size_t at = sizeof(mach_header_64);
    for (uint32_t i = 0; i < header->ncmds && at + sizeof(load_command) <= bytes.size(); ++i) {
        const auto *command = reinterpret_cast<const load_command *>(bytes.data() + at);
        if (command->cmd == LC_UUID && at + sizeof(uuid_command) <= bytes.size()) {
            const uint8_t *uuid = reinterpret_cast<const uuid_command *>(command)->uuid;
            char text[33];
            for (int k = 0; k < 16; ++k) snprintf(text + 2 * k, 3, "%02x", uuid[k]);
            return text;
        }
        at += command->cmdsize;
    }
    return "unknown";
}

int main(int argc, char **argv) {
    @autoreleasepool {
        if (argc != 7 && argc != 9) return 2;
        id<MTLDevice> device = native_bridge_metal_open(argv[1], argv[2], 0x6e6d70, onMissing);
        if (!device) {std::printf("FAIL device\n"); return 1;}
        native_bridge_metal_quiet_problems(true);
        const std::string root = argv[2], osRoot = argv[4], cryptexRoot = argv[5], cryptex = "/System/Cryptexes/OS";
        const std::string build = std::string(prime::nmb_runtime_build()) + "-" + fileUuid(argv[6]);
        std::vector<std::string> paths;
        {std::ifstream list(argv[3]); for (std::string line; std::getline(list, line);) if (!line.empty() && line[0] == '/') paths.push_back(line);}
        size_t first = 0, count = paths.size();
        if (argc == 9) {first = strtoul(argv[7], nullptr, 10); count = strtoul(argv[8], nullptr, 10);}
        const std::set<std::string> bad = prime::readLines(root + "/prime-bad");
        const std::string current = root + "/prime-current", log = root + "/prime.log", done = root + "/prime-done";
        unsigned primed = 0, absent = 0;
        for (size_t i = first; i < paths.size() && i < first + count; ++i) {
            const std::string &path = paths[i];
            const std::string local = path.compare(0, cryptex.size(), cryptex) == 0 ? cryptexRoot + path.substr(cryptex.size()) : osRoot + path;
            struct stat st = {};
            if (stat(local.c_str(), &st) != 0) {++absent; prime::appendLine(log, path + " absent from the image mount"); continue;}
            prime::appendLine(log, prime::library(device, local, current, bad));
            prime::appendLine(done, path + "\t" + std::to_string((long long)st.st_size) + "\t" +
                                    std::to_string((long long)st.st_mtimespec.tv_sec) + "\t" + build);
            ++primed;
        }
        // The cache thread writes the entries after the calls returned: wait until its write count has stood still for 10 s.
        NmbShaderCacheStats stats = {};
        uint64_t last = UINT64_MAX;
        for (unsigned still = 0; still < 20; usleep(500000)) {
            native_bridge_metal_shader_cache_stats(&stats);
            const uint64_t now = stats.writes + stats.driver_saves;
            still = now == last ? still + 1 : 0;
            last = now;
        }
        std::printf("PASS: primed %u libraries (%u absent) made=%u writes=%llu driver_saves=%llu build=%s\n", primed, absent,
                    (unsigned)native_prime_made, (unsigned long long)stats.writes, (unsigned long long)stats.driver_saves, build.c_str());
        return 0;
    }
}
