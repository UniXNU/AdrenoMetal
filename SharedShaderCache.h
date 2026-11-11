// SPDX-License-Identifier: MIT
#pragma once
#include <string>
#include <pwd.h>
#include <unistd.h>
#include <sys/stat.h>
#include <stdlib.h>
#include <vector>

// Processes of one user share immutable, version-keyed shader entries. A sandbox
// that cannot write this directory continues with its container's cache. The
// sandbox is asked first without a report: a refused probe file made the sandbox
// write a violation report (with a backtrace) at every launch of every sandboxed
// Metal process (iconservicesagent, widgets).
extern "C" int sandbox_check(pid_t pid, const char *operation, int type, ...);
inline std::string sharedShaderCacheDirectory() {
    static const std::string path = []() -> std::string {
        std::string root;
        if (const char *override = getenv("NATIVE_METAL_SHARED_CACHE_DIR")) {
            if (!*override) return {};
            root = override;
        } else {
            passwd user{}, *found=nullptr;
            std::vector<char> bytes(16384);
            if (getpwuid_r(getuid(), &user, bytes.data(), bytes.size(), &found) || !found || !user.pw_dir) return {};
            root = std::string(user.pw_dir) + "/Library/Caches/org.unixnu.metal";
        }
        mkdir(root.c_str(), 0700);
        struct stat st = {};
        if (lstat(root.c_str(), &st) || !S_ISDIR(st.st_mode) || st.st_uid != getuid() || (st.st_mode & 0022)) return {};
        // A sandboxed process (sandbox_check with no operation answers 1) asks for the write first: SANDBOX_FILTER_PATH (1) |
        // SANDBOX_CHECK_NO_REPORT (0x40000000), nonzero is a refusal.
        if (sandbox_check(getpid(), nullptr, 0) == 1 &&
            sandbox_check(getpid(), "file-write-create", 1 | 0x40000000, (root + "/.write-probe").c_str()) != 0) return {};
        std::string probe = root + "/.write-XXXXXX";
        int fd = mkstemp(probe.data());
        if (fd < 0) return {};
        close(fd); unlink(probe.c_str());
        return root;
    }();
    return path;
}

// The system shader cache (macOS): one directory the render server fills and every process reads.
// The render server (WindowServer, the user _windowserver) is not the console user, so the per-user directory above serves it alone,
// and its sandbox lets it write only its own cache and temporary directories, /private/tmp and /private/var/tmp, while the one
// directory every sandbox that may use graphics can read is /Library/GPUBundles (system.sb's system-graphics, the WebKit GPU
// process's profile). No path is both, so the kernel makes the directory at boot where the render server may write it
// (kWriterPath: owner _windowserver, mode 0755) and shows it read-only where everyone may read it (kReaderPath, a read-only bind
// mount beside the plugin). The render server opens its device in the first (its cache directory, made readable by everyone) and
// primes the system's Metal libraries into it; every other process asks the second for what its own cache lacks.
// A reader trusts the directory only by who can write it: it, every directory on the way to an entry and the entry must be owned by
// the superuser or the render server's user and not writable by group or others, each opened without following a link (the
// runtime's system layer, bridge patch 0036). An entry is then checked as one of the process's own cache is (name, build, key,
// profile, checksum). gpu-prime-system=0 turns all of it off.
namespace systemcache {
static const char kWriterPath[] = "/private/var/tmp/NativeAdrenoMetal.cache";
static const char kReaderPath[] = "/Library/GPUBundles/NativeAdrenoMetal.cache";
static const uint32_t kOwners[] = {0, 88};   // the superuser and _windowserver
// The directory this process fills, when the kernel made it for this process's user (or this process is the superuser): never
// made here, since /private/var/tmp takes anyone's directories.
inline std::string writerDirectory() {
    struct stat st = {};
    if (lstat(kWriterPath, &st) || !S_ISDIR(st.st_mode) || (st.st_mode & 0022)) return {};
    if (st.st_uid != kOwners[0] && st.st_uid != kOwners[1]) return {};
    if (st.st_uid != getuid() && getuid() != 0) return {};
    return access(kWriterPath, W_OK) == 0 ? std::string(kWriterPath) : std::string();
}
}  // namespace systemcache

// Call before the first Vulkan device is opened, including the optional smoke path.
inline void configureShaderDiskCache(const std::string &root) {
    if (root.empty()) return;
    const std::string ir3 = root + "/ir3";
    mkdir(ir3.c_str(), 0700);
    if (access(ir3.c_str(), W_OK) == 0) setenv("MESA_SHADER_CACHE_DIR", ir3.c_str(), 0);
}
