// SPDX-License-Identifier: MIT
// Pipeline-cache export (int-147). The render server's runtime keeps its translated shaders and the Vulkan driver's pipeline records in its
// cache directory, but that directory lives on the RAM Data disk: every boot compiled the whole UI again (int-146: 98 pipelines, 63 s,
// up to 1.6 s each, also while the user opened App Library). This thread sends the directory to the kernel's UFS log (method 0x7100 on a
// hand-off connection, kits/sheng/BootLog.c export region) whenever it changed and then stayed unchanged for one period; the host reads it
// back with the log and builds the next image's Data with it, so those pipelines start as cache hits.
// A boot whose image carried a pack exports only the files the pack did not have (see baked()).
// Stream: "M2VCACH1", u32 directory length, directory, u32 file count, then per file u32 path length, u32 mode, u32 uid, u32 gid, u64 size,
// path (relative to the directory), contents. Export blob: "SHEXR001", u32 stream bytes, u32 0, the stream as it is: it goes out in pieces
// while the files are read, so the render server never holds more than a piece (int-149: one boot's new variants were about 100 MB).
// Image pack: "SHEXP001", u32 stream bytes, u32 compressed bytes, the stream zlib-compressed (scripts/add-gpu-plugin-data.py merges exports
// into one). The export only reads.
#pragma once
#include <dirent.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zlib.h>
#include <functional>
#include <set>
#include <string>
#include <vector>
#ifndef CACHEEXPORT_FORMAT_ONLY   // the format alone (host check, bridge/cache_export_host.cpp); the plugin also gets the sender
#include "NativeAdrenoUserTest.h"
#endif

namespace cacheexport {

// kLimit: the largest export blob, one half of the kernel's 256 MiB export region.
constexpr uint32_t kMethod = 0x7100, kPiece = 3968, kLimit = 128u << 20;
struct File {std::string path; struct stat st;};

// A pack in the image is the union of several boots' exports (scripts/add-gpu-plugin-data.py); it is read as a stream, so only its own
// file and the stream bounds limit it.
constexpr uint64_t kPackLimit = 1ull << 30;

// Files the image's pack carried (path and size). The export leaves them out: the host has them already and merges each boot's export
// with the earlier ones, so a boot sends only what it compiled itself (int-148: the whole directory outgrew the export area once the UI's
// own variants were added, and the export refused the build, marker 0x69f2).
static std::set<std::string> &baked() {static std::set<std::string> files; return files;}
static std::string bakedKey(const std::string &path, uint64_t size) {return path + '\n' + std::to_string(size);}
static void dropBaked(std::vector<File> &files) {
    if (baked().empty()) return;
    std::vector<File> fresh;
    for (File &f : files) if (!baked().count(bakedKey(f.path, (uint64_t)f.st.st_size))) fresh.push_back(std::move(f));
    files.swap(fresh);
}

static void scan(const std::string &root, const std::string &relative, std::vector<File> &out, unsigned depth) {
    if (depth > 8) return;
    DIR *dir = opendir((root + "/" + relative).c_str());
    if (!dir) return;
    while (dirent *entry = readdir(dir)) {
        if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, "..")) continue;
        const std::string path = relative.empty() ? std::string(entry->d_name) : relative + "/" + entry->d_name;
        struct stat st = {};
        if (lstat((root + "/" + path).c_str(), &st) != 0) continue;
        if (S_ISDIR(st.st_mode)) scan(root, path, out, depth + 1);
        else if (S_ISREG(st.st_mode)) out.push_back({path, st});
    }
    closedir(dir);
}

// Count, bytes and newest modification of the directory: equal on two scans = nothing is being written.
static uint64_t signature(const std::vector<File> &files) {
    uint64_t s = files.size() * 0x9e3779b97f4a7c15ull;
    for (const File &f : files) s = s * 31 + (uint64_t)f.st.st_size * 1000003 + (uint64_t)f.st.st_mtimespec.tv_sec * 7 + (uint64_t)f.st.st_mtimespec.tv_nsec;
    return s;
}

template <typename T> static void put(std::vector<uint8_t> &b, T v) {const uint8_t *p = (const uint8_t *)&v; b.insert(b.end(), p, p + sizeof v);}

// The stream's length for these files, from their sizes as scanned.
static uint64_t streamBytes(const std::string &root, const std::vector<File> &files) {
    uint64_t n = 8 + 4 + root.size() + 4;
    for (const File &f : files) n += 24 + f.path.size() + (uint64_t)f.st.st_size;
    return n;
}

// The stream, handed to emit(bytes, count) in order (emit returns false to stop); false when a file vanished or no longer has the size
// it was scanned with, or emit stopped.
template <typename Emit> static bool stream(const std::string &root, const std::vector<File> &files, Emit &&emit) {
    std::vector<uint8_t> head;
    {static const char magic[] = "M2VCACH1"; head.insert(head.end(), magic, magic + 8);}
    put<uint32_t>(head, (uint32_t)root.size());
    head.insert(head.end(), root.begin(), root.end());
    put<uint32_t>(head, (uint32_t)files.size());
    if (!emit(head.data(), head.size())) return false;
    std::vector<uint8_t> chunk(65536);
    for (const File &f : files) {
        FILE *in = fopen((root + "/" + f.path).c_str(), "rb");
        if (!in) return false;
        head.clear();
        put<uint32_t>(head, (uint32_t)f.path.size());
        put<uint32_t>(head, (uint32_t)f.st.st_mode);
        put<uint32_t>(head, (uint32_t)f.st.st_uid);
        put<uint32_t>(head, (uint32_t)f.st.st_gid);
        put<uint64_t>(head, (uint64_t)f.st.st_size);
        head.insert(head.end(), f.path.begin(), f.path.end());
        bool ok = emit(head.data(), head.size());
        uint64_t left = (uint64_t)f.st.st_size;
        while (ok && left) {
            const size_t want = left < chunk.size() ? (size_t)left : chunk.size();
            ok = fread(chunk.data(), 1, want, in) == want && emit(chunk.data(), want);
            left -= want;
        }
        ok = ok && fgetc(in) == EOF;   // grown since the scan: refuse rather than send a short copy
        fclose(in);
        if (!ok) return false;
    }
    return true;
}

#ifdef CACHEEXPORT_FORMAT_ONLY
// The image pack format (SHEXP001, compressed), for the host check; scripts/add-gpu-plugin-data.py builds the same. False when a file
// vanished or zlib is missing.
static bool build(const std::string &root, const std::vector<File> &files, std::vector<uint8_t> &out) {
    using BoundFn = unsigned long (*)(unsigned long);
    using CompressFn = int (*)(uint8_t *, unsigned long *, const uint8_t *, unsigned long, int);
    static void *z = dlopen("/usr/lib/libz.1.dylib", RTLD_NOW | RTLD_LOCAL);
    if (!z) return false;
    auto bound = (BoundFn)dlsym(z, "compressBound");
    auto compress = (CompressFn)dlsym(z, "compress2");
    if (!bound || !compress) return false;
    std::vector<uint8_t> raw;
    if (!stream(root, files, [&](const uint8_t *p, size_t n) {raw.insert(raw.end(), p, p + n); return true;})) return false;
    unsigned long zbytes = bound(raw.size());
    std::vector<uint8_t> zdata(zbytes);
    if (compress(zdata.data(), &zbytes, raw.data(), raw.size(), 6) != 0) return false;
    out.clear();
    {static const char magic[] = "SHEXP001"; out.insert(out.end(), magic, magic + 8);}
    put<uint32_t>(out, (uint32_t)raw.size());
    put<uint32_t>(out, (uint32_t)zbytes);
    out.insert(out.end(), zdata.begin(), zdata.begin() + (long)zbytes);
    return true;
}
#endif

// Reads a pack's stream: inflated as it is consumed, so neither the pack nor the stream is held in memory.
struct PackReader {
    using InitFn = int (*)(z_streamp, const char *, int);
    using InflateFn = int (*)(z_streamp, int);
    using EndFn = int (*)(z_streamp);
    FILE *in = nullptr;
    // Where the compressed bytes come from: the pack file, or the import region (importRegion).
    std::function<bool(void *, size_t)> source;
    z_stream z = {};
    InflateFn inflateFn = nullptr;
    EndFn endFn = nullptr;
    bool open = false, ended = false;
    uint64_t left = 0, produced = 0, expect = 0;   // compressed bytes not yet read from the file, stream bytes produced, stream bytes announced
    uint8_t input[65536];
    ~PackReader() {if (open) endFn(&z); if (in) fclose(in);}
    bool start(const char *path) {
        if (!(in = fopen(path, "rb"))) return false;
        struct stat st = {};
        if (fstat(fileno(in), &st) != 0 || (uint64_t)st.st_size > kPackLimit) return false;
        FILE *file = in;
        return startWith([file](void *out, size_t n) {return fread(out, 1, n, file) == n;}, (uint64_t)st.st_size);
    }
    // A pack of `bytes` bytes (its 16-byte head included) that `read` hands out in order.
    bool startWith(std::function<bool(void *, size_t)> read, uint64_t bytes) {
        static void *lib = dlopen("/usr/lib/libz.1.dylib", RTLD_NOW | RTLD_LOCAL);
        auto init = lib ? (InitFn)dlsym(lib, "inflateInit_") : nullptr;
        inflateFn = lib ? (InflateFn)dlsym(lib, "inflate") : nullptr;
        endFn = lib ? (EndFn)dlsym(lib, "inflateEnd") : nullptr;
        source = std::move(read);
        uint8_t head[16];
        uint32_t raw = 0, zbytes = 0;
        if (!init || !inflateFn || !endFn || !source(head, 16) || memcmp(head, "SHEXP001", 8)) return false;
        memcpy(&raw, head + 8, 4);
        memcpy(&zbytes, head + 12, 4);
        if ((uint64_t)zbytes != bytes - 16) return false;
        left = zbytes; expect = raw;
        open = init(&z, ZLIB_VERSION, (int)sizeof z) == Z_OK;
        return open;
    }
    bool read(void *out, size_t n) {
        z.next_out = static_cast<Bytef *>(out);
        z.avail_out = (uInt)n;
        while (z.avail_out) {
            if (ended) return false;
            if (!z.avail_in && left) {
                const size_t want = left < sizeof input ? (size_t)left : sizeof input;
                if (!source(input, want)) return false;
                left -= want;
                z.next_in = input;
                z.avail_in = (uInt)want;
            }
            const uInt before = z.avail_out;
            const int r = inflateFn(&z, Z_NO_FLUSH);
            produced += before - z.avail_out;
            if (r == Z_STREAM_END) ended = true;
            else if (r != Z_OK) return false;
            if (produced > expect) return false;
        }
        return true;
    }
    bool skip(uint64_t n) {
        uint8_t scratch[16384];
        while (n) {const size_t k = n < sizeof scratch ? (size_t)n : sizeof scratch; if (!read(scratch, k)) return false; n -= k;}
        return true;
    }
    // The whole stream was consumed: zlib's check passed at its end and nothing follows it.
    bool finished() {
        if (!ended) {uint8_t b; if (read(&b, 1)) return false;}
        return ended && produced == expect && !left && !z.avail_in;
    }
};

// Import, before the device opens: the image may carry a pack beside the bundle (NativeAdrenoMetal.m2vpack, put there by
// scripts/add-gpu-plugin-data.py --pipeline-pack). Its files go into this process's own cache directory, so they carry this process's owner
// and mode; a file that is already there with the same size is kept. Files are written beside their place and renamed only once the
// whole pack has been read and checked: a damaged pack imports nothing. Returns the number of files written, or -1.
static int importStream(PackReader &r, const std::string &directory);
static int importPack(const char *pack, const std::string &directory) {
    if (access(pack, R_OK) != 0) return 0;
    PackReader r;
    if (!r.start(pack)) return -1;
    return importStream(r, directory);
}
static int importStream(PackReader &r, const std::string &directory) {
    std::vector<std::string> written;
    auto fail = [&]() {for (const std::string &f : written) unlink((f + ".import").c_str()); return -1;};
    char magic[8];
    uint32_t rootLength = 0, count = 0;
    if (!r.read(magic, 8) || memcmp(magic, "M2VCACH1", 8) || !r.read(&rootLength, 4) || rootLength > 4096 || !r.skip(rootLength) ||
        !r.read(&count, 4) || count > 1u << 20) return fail();
    std::set<std::string> seen;
    std::vector<uint8_t> chunk(65536);
    for (uint32_t i = 0; i < count; ++i) {
        uint32_t pathLength = 0, mode = 0, uid = 0, gid = 0;
        uint64_t size = 0;
        if (!r.read(&pathLength, 4) || !r.read(&mode, 4) || !r.read(&uid, 4) || !r.read(&gid, 4) || !r.read(&size, 8)) return fail();
        if (pathLength == 0 || pathLength > 1024 || size > r.expect) return fail();
        std::string path(pathLength, '\0');
        if (!r.read(&path[0], pathLength)) return fail();
        if (path[0] == '/' || path.find("..") != std::string::npos || path.find('\0') != std::string::npos) return fail();
        seen.insert(bakedKey(path, size));
        const std::string full = directory + "/" + path;
        struct stat st = {};
        if (stat(full.c_str(), &st) == 0 && (uint64_t)st.st_size == size) {if (!r.skip(size)) return fail(); continue;}
        for (size_t slash = full.find('/', directory.size() + 1); slash != std::string::npos; slash = full.find('/', slash + 1))
            mkdir(full.substr(0, slash).c_str(), 0700);
        written.push_back(full);
        FILE *out = fopen((full + ".import").c_str(), "wb");
        if (!out) return fail();
        bool ok = true;
        for (uint64_t left = size; ok && left;) {
            const size_t k = left < chunk.size() ? (size_t)left : chunk.size();
            ok = r.read(chunk.data(), k) && fwrite(chunk.data(), 1, k, out) == k;
            left -= k;
        }
        ok = fclose(out) == 0 && ok;
        if (!ok) return fail();
    }
    if (!r.finished()) return fail();
    int renamed = 0;
    for (const std::string &f : written) if (rename((f + ".import").c_str(), f.c_str()) == 0) ++renamed; else unlink((f + ".import").c_str());
    baked().insert(seen.begin(), seen.end());   // only a pack read to its end counts
    return renamed;
}

#ifndef CACHEEXPORT_FORMAT_ONLY
// Markers 0x69xx: 00 started, 01 connection, 02 an export went out (then 0x6Bnn: its size in MiB, saturated), f0 no connection,
// f1 no selector, f2 a file changed or vanished while it was read, f3 a piece refused, f4 the new files exceed one export.
static void run(Gpu &marker, std::string root) {
    while (root.size() > 1 && root.back() == '/') root.pop_back();
    marker.mark(0x6900);
    Gpu link;
    link.call = marker.call; link.openFn = marker.openFn; link.closeFn = marker.closeFn; link.port = marker.port;
    if (!link.openFn || link.openFn(link.port, mach_task_self(), 0x4148, &link.conn) != 0 || !link.conn) {marker.mark(0x69f0); return;}
    if (!link.probe()) {marker.mark(0x69f1); return;}
    marker.mark(0x6901);
    uint64_t previous = 0, exported = 0;
    uint32_t generation = 0;
    std::vector<uint8_t> buffer(24 + kPiece);
    for (;;) {
        sleep(15);
        std::vector<File> files;
        scan(root, "", files, 0);
        dropBaked(files);
        const uint64_t now = signature(files);
        const bool stable = now == previous;
        previous = now;
        if (!stable || now == exported || files.empty()) continue;
        const uint64_t total = 16 + streamBytes(root, files);
        if (total > kLimit) {marker.mark(0x69f4); exported = now; continue;}
        ++generation;
        uint32_t offset = 0, filled = 0;
        bool sent = true;
        auto flush = [&]() {
            const uint32_t header[6] = {kTunnelMagic, kMethod, generation, (uint32_t)total, offset, filled};
            memcpy(buffer.data(), header, sizeof header);
            sent = link.call(link.conn, link.selector, buffer.data(), sizeof header + filled, nullptr, nullptr) == 0;
            offset += filled;
            filled = 0;
            return sent;
        };
        auto emit = [&](const uint8_t *p, size_t n) {
            while (n) {
                const size_t k = n < kPiece - filled ? n : kPiece - filled;
                memcpy(buffer.data() + 24 + filled, p, k);
                filled += (uint32_t)k; p += k; n -= k;
                if (filled == kPiece && !flush()) return false;
            }
            return true;
        };
        uint8_t head[16] = {'S', 'H', 'E', 'X', 'R', '0', '0', '1'};
        const uint32_t streamed = (uint32_t)(total - 16);
        memcpy(head + 8, &streamed, 4);
        const bool read = emit(head, sizeof head) && stream(root, files, emit);
        if (read && filled) flush();
        if (!sent) {marker.mark(0x69f3); continue;}       // retried after the next change or period
        if (!read || offset != total) {marker.mark(0x69f2); continue;}
        exported = now;
        marker.mark(0x6902);
        marker.mark(0x6B00 | (uint32_t)(total >> 20 > 0xff ? 0xff : total >> 20));
    }
}

#endif

}  // namespace cacheexport
