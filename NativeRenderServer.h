// SPDX-License-Identifier: MIT
// The system's render server, the process that composes the display and gets the render-server path of the plugin (display hand-off client,
// shader cache directory, pipeline priming), the IOSurface use counts and the frame trace: backboardd on iPadOS, WindowServer on macOS. The
// kernel's NativeAdrenoPublish.cpp names the same process (row 0, its mailbox page).
#pragma once
#include <TargetConditionals.h>
#include <stdlib.h>
#include <string.h>

#if TARGET_OS_OSX
#define NATIVE_RENDER_SERVER "WindowServer"
#else
#define NATIVE_RENDER_SERVER "backboardd"
#endif

inline bool nativeIsRenderServer() {
    static const bool renderServer = !strcmp(getprogname(), NATIVE_RENDER_SERVER);
    return renderServer;
}
