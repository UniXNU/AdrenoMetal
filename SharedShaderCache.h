// SPDX-License-Identifier: MIT
#pragma once
#include <string>
#include <pwd.h>
#include <unistd.h>
#include <sys/stat.h>
#include <stdlib.h>
#include <vector>

// Processes of one user share immutable, version-keyed shader entries. A sandbox
// that cannot write this directory continues with its container's cache.
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
        std::string probe = root + "/.write-XXXXXX";
        int fd = mkstemp(probe.data());
        if (fd < 0) return {};
        close(fd); unlink(probe.c_str());
        return root;
    }();
    return path;
}

// Call before the first Vulkan device is opened, including the optional smoke path.
inline void configureShaderDiskCache(const std::string &root) {
    if (root.empty()) return;
    const std::string ir3 = root + "/ir3";
    mkdir(ir3.c_str(), 0700);
    if (access(ir3.c_str(), W_OK) == 0) setenv("MESA_SHADER_CACHE_DIR", ir3.c_str(), 0);
}
