# AdrenoMetal

Metal frontend through `UniXNU/metal2vulkan` and Turnip.

## Translator and runtime

This repository targets the `UniXNU/metal2vulkan` fork (`development` branch, pinned in `UniXNU/scripts/data/gpu-metal-vulkan-inputs.json` at `87a869f7fc4b73265d4812c40d8cccb6a83c7b17`, based on upstream `steelbrain/metal2vulkan` `0c9db4976504d3eba5898506098e7ec00b7418dc`).

The five UniXNU runtime and translator changes are maintained directly in `UniXNU/metal2vulkan` rather than as carried files under `bridge/patches/`:

- Nonpresenting Vulkan instance creation without surface or swapchain requirements for native scanout.
- Deferred native display scheduling after the final driver submission so display fence capture includes the current frame.
- Read-only host-memory buffer imports.
- Zero-copy in-place linear textures (with `VK_EXT_attachment_feedback_loop_layout`) over imported host memory.
- Entry-return floor tail guard across translator shader stages.

## Bridge architecture

Indirect buffer and texture declarations, heap declarations, encoder barriers, concurrent compute and queue enqueueing use the runtime public APIs. Commands retain both Objective-C resource owners and consumer placement leases until real GPU completion. Native IOSurface writes are marked across render attachments, resolves, blits and shader-write bindings; ordinary commands keep upstream scheduling behavior.

AdrenoMetal owns descriptor-index grants, heap and buffer placement leases, bounded completion waits, pooled private storage for memoryless descriptors, and compute-based two-plane YCbCr IOSurface conversion with seed-tracked refresh. Commands retain consumer ownership tokens through GPU retirement. Texture views retain backing leases separately from descriptor indices.

Binary archives and the read-only system cache populate the runtime cache directory, isolated by runtime build identity. Imports preserve the generation lock, byte accounting and version record. A system-cache writer publishes readable files; ordinary applications create no publication worker. Pipeline creation and cache decoding remain runtime operations.

## Validation

Host regression tests cover descriptor-index exhaustion and retirement, completion timeouts, concurrent cache imports, cache ownership checks, heap placement, indirect declarations, texture views and heap retention behind an unsignaled event.

End-to-end macOS 27.3 (`26C5035r`) rendering, live window dragging, zero-copy host-import linear textures and fault-free SMMU/GPU execution have been validated on `sheng` (Adreno 740).
