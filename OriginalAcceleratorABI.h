// SPDX-License-Identifier: MIT
#pragma once
// Exact supplied iOS17 base, independently audited against its complete
// original vtable and protected constructor/destructor bodies. No extra fields
// or operational overrides beyond IOService. This declares a real ancestry;
// no original class, method, vtable or metaclass is replaced.
#include <IOKit/IOService.h>
#if XNU_TARGET_OS_OSX
// macOS: the accelerator base is IOGraphicsFamily's IOAccelerator, of the same shape (136 bytes, no overrides beyond the
// destructors and getMetaClass); IOGPU's Metal discovery matches IOServiceMatching("IOAccelerator").
class IOAccelerator : public IOService {
 OSDeclareAbstractStructors(IOAccelerator)
};
#define NATIVE_ACCELERATOR_BASE IOAccelerator
#else
class IOAcceleratorES : public IOService {
 OSDeclareAbstractStructors(IOAcceleratorES)
};
#define NATIVE_ACCELERATOR_BASE IOAcceleratorES
#endif
static_assert(sizeof(IOService)==136 && sizeof(NATIVE_ACCELERATOR_BASE)==136,"Qualified original accelerator base extent");
