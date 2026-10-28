// SPDX-License-Identifier: MIT
// Userspace side of the GPU hand-off (boot-arg sheng-fb-gpu=1): serves the display driver's frames with the GPU. For every offered frame the
// GPU copies the compositor's surface pages into the scanout mapping (GpuScanoutPresenter.h), and the swap is completed with the GPU's
// verdict. A failed copy is reported as failed; the display driver then falls back to its own path and counts it.
// The calls reach the kernel through a Transport. In the plugin that is the Adreno accelerator's connection (the original sandbox profiles
// admit that class, not the presenter's own service): methods 0x7000 + NGP_METHOD_* through the tunnel selector, scanout as memory type 0x5343.
#pragma once
#import <Metal/Metal.h>
#include <mach/mach_time.h>
#include <stdint.h>
#include <unistd.h>
#include "GpuScanoutPresenter.h"

extern "C" void NBWaitInFlight(uint64_t beforeUs) __attribute__((weak_import));
extern "C" void NBTraceSwap(uint64_t offered,uint64_t copied,uint64_t completed,uint32_t sequence,uint32_t result) __attribute__((weak_import));

namespace gpuscanout {

struct Transport {
    void *context;
    // NGP_METHOD_MODE / WAIT_SWAP / COMPLETE_SWAP with the wire records; returns the IOReturn (0 = success).
    int (*call)(void *context, uint32_t method, const void *in, size_t inSize, void *out, size_t outSize);
    // The scanout, writable, mapped into this task.
    int (*mapScanout)(void *context, uint64_t *address, uint64_t *length);
    // Progress marker (the plugin shows the latest ones on the boot page, which survives a panic that parks the calling core).
    void (*mark)(void *context, uint32_t value);
};

struct Stats {
    uint64_t frames, failures, timeouts;
    int lastResult;     // gpupresent::Result of the last frame
    int step;           // 0 idle, 2 mode refused, 3 scanout not mappable/importable, 4 serving
    int error;          // the failing call's return, when there is one
    uint64_t verified;  // frames whose scanout was read back and compared with the surface
    uint64_t mismatchRows;   // rows of those frames that differ (CPU reads only: nothing is rendered or copied by the CPU)
    uint32_t lastMs;    // the GPU copy's time for the last frame
    uint64_t totalMs;   // and for all frames (the mean is totalMs / (frames + failures))
};

class Client {
public:
    // Reads the mode and maps the scanout; false with stats.step telling where it stopped.
    bool open(id<MTLDevice> device, const Transport &transport, Stats *stats) {
        transport_ = transport;
        stats_ = stats;
        NGPMode mode = {};
        note(0x6710);
        int status = transport_.call(transport_.context, NGP_METHOD_MODE, nullptr, 0, &mode, sizeof mode);
        note(0x6711);
        if (status || mode.version != NGP_VERSION || mode.format != NGP_BGRX8888 || !mode.width || !mode.height ||
            mode.stride < mode.width * 4 || mode.bytes < mode.stride * mode.height) return fail(2, status);
        const size_t page = (size_t)getpagesize();
        uint64_t address = 0, length = 0;
        note(0x6712);
        status = transport_.mapScanout(transport_.context, &address, &length);
        note(0x6713);
        if (status || !address || length < mode.bytes) return fail(3, status);
        // The mapping covers whole pages; the framebuffer allocation ends on a page boundary, the copy never writes past the last row.
        gpupresent::Scanout scanout = {(void *)(uintptr_t)address, (size_t)((mode.bytes + page - 1) & ~(page - 1)), (size_t)mode.stride,
                                       (uint32_t)mode.width, (uint32_t)mode.height};
        note(0x6714);
        scanout_ = scanout;
        presenter_ = new gpupresent::Presenter(device, scanout);
        note(0x6715);
        if (!presenter_->ready()) return fail(3, 0);
        stats_->step = 4;
        return true;
    }
    // One wait (at most timeoutMs) and, when the display driver offered a frame, one GPU copy and its completion. Returns false on a
    // protocol error that makes further serving pointless.
    bool serveOne(uint64_t timeoutMs) {
        NGPWait wait = {timeoutMs};
        NGPSwap swap = {};
        if (!stats_->timeouts && !stats_->frames) note(0x6720);   // first wait
        const int status = transport_.call(transport_.context, NGP_METHOD_WAIT_SWAP, &wait, sizeof wait, &swap, sizeof swap);
        if (status) {
            if ((unsigned)status == 0xe00002d6u /* kIOReturnTimeout */) {++stats_->timeouts; return true;}
            ++stats_->failures;
            stats_->error = status;
            return false;
        }
        const size_t page = (size_t)getpagesize();
        {   // measurement: the second half of the run copies half the rows
            static uint64_t first = 0;
            const uint64_t nowS = clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1000000000ull;
            if (!first) first = nowS;
            (void)first; gpupresent::gHalfRows.store(false, std::memory_order_relaxed);
        }
        note(0x6730);   // a frame was offered
        const uint64_t offeredUs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1000;
        if (NBWaitInFlight) NBWaitInFlight(offeredUs);
        const uint64_t began = mach_absolute_time();
        const gpupresent::Result r = presenter_->present({(const void *)(uintptr_t)swap.address, (size_t)((swap.bytes + page - 1) & ~(page - 1)),
                                                          (size_t)swap.stride, swap.width, swap.height, swap.reserved, swap.format});
        {
            static mach_timebase_info_data_t base;
            if (!base.denom) mach_timebase_info(&base);
            stats_->lastMs = (uint32_t)(((mach_absolute_time() - began) * base.numer / base.denom) / 1000000);
            stats_->totalMs += stats_->lastMs;
        }
        stats_->lastResult = (int)r;
        if (r == gpupresent::kOk && swap.format == NGP_BGRX8888 && stats_->verified < 4) verify(swap);
        note(0x6731);   // the GPU copy returned
        const uint64_t copiedUs = clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1000;
        NGPComplete done = {swap.sequence, r == gpupresent::kOk ? 0u : 0xe00002bcu /* kIOReturnError */, 0};
        const int completed = transport_.call(transport_.context, NGP_METHOD_COMPLETE_SWAP, &done, sizeof done, nullptr, 0);
        if (NBTraceSwap) NBTraceSwap(offeredUs, copiedUs, clock_gettime_nsec_np(CLOCK_UPTIME_RAW) / 1000, (uint32_t)swap.sequence, (uint32_t)r);
        if (completed) {++stats_->failures; stats_->error = completed; return false;}
        if (r == gpupresent::kOk) ++stats_->frames; else ++stats_->failures;
        return true;
    }
private:
    // Reads the scanout back with aligned 32-bit loads (the framebuffer is device memory; wider loads can fault) and compares the top rows with the
    // surface the GPU copied. The lower part of the panel is skipped: the diagnostic renderer paints there. Evidence only; nothing is written.
    void verify(const NGPSwap &swap) {
        const uint32_t rows = swap.height < 400 ? swap.height : 400, words = swap.width;
        const volatile uint32_t *shown = (const volatile uint32_t *)scanout_.base;
        const volatile uint32_t *surface = (const volatile uint32_t *)(uintptr_t)swap.address;
        for (uint32_t y = 0; y < rows; ++y) {
            const volatile uint32_t *a = shown + (size_t)y * (scanout_.stride / 4), *b = surface + (size_t)y * (swap.stride / 4);
            for (uint32_t x = 0; x < words; ++x)
                if (a[x] != b[x]) {++stats_->mismatchRows; break;}
        }
        ++stats_->verified;
    }
    void note(uint32_t value) {if (transport_.mark) transport_.mark(transport_.context, value);}
    bool fail(int step, int error) {stats_->step = step; stats_->error = error; return false;}
    Transport transport_ = {};
    gpupresent::Presenter *presenter_ = nullptr;
    gpupresent::Scanout scanout_ = {};
    Stats *stats_ = nullptr;
};

}  // namespace gpuscanout
