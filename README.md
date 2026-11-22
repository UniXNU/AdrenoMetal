# AdrenoMetal upstream-minimal experiment

This branch targets unmodified metal2vulkan development `4e9aad67d5af604a64725e26f3070968dd15a618`, except for native presentation selection (`0073`). The native display driver owns scanout; the Vulkan surface and swapchain requirement previously prevented device creation on sheng.

The earlier shader-correction, loss-location, bounded-wait, descriptor-reservation, placement-lease, external-cache and cache-publication patches are omitted. Local shader priming workarounds are also removed.

The consumer still needs porting to the upstream public API. Static inspection currently finds missing `CacheLookup` and `IndexReservation` exports, in addition to patched method calls. Do not treat this branch as build-qualified or rendering-qualified.

Darwin kernel and Turnip platform support remains separate. The recent native clock and coherent-memory capability wiring is retained as a test candidate: it reached the desktop but showed severe rendering corruption. Its correctness is not qualified.
