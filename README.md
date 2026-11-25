# AdrenoMetal upstream-minimal experiment

This branch uses metal2vulkan development `0c9db4976504d3eba5898506098e7ec00b7418dc`. Runtime patch `0073` selects native presentation without requiring Vulkan surfaces or swapchains. Patch `0077` delays native display scheduling until the final driver submission, so the display driver captures the current frame's fence. Patch `0078` supports read-only host-memory imports. Patch `0079` enables zero-copy in-place linear textures (with attachment feedback loop support) over imported host memory. Rendering and device capability requirements remain upstream implementations.

Indirect buffer and texture declarations, heap declarations, encoder barriers, concurrent compute and queue enqueueing use the upstream public APIs. Commands retain both Objective-C resource owners and consumer placement leases until real GPU completion. Native IOSurface writes are marked across render attachments, resolves, blits and shader-write bindings; ordinary commands keep upstream scheduling behavior.

AdrenoMetal owns descriptor-index grants, heap and buffer placement leases, and bounded completion waits through public runtime APIs. Commands retain consumer ownership tokens through GPU retirement. Texture views retain backing leases separately from descriptor indices.

Binary archives and the read-only system cache populate the upstream cache directory, isolated by runtime build identity so removed translator patches cannot leave reusable artifacts. Imports preserve its generation lock, byte accounting and version record. A system-cache writer publishes readable files; ordinary applications create no publication worker. Pipeline creation and cache decoding remain upstream operations.

The removed runtime patches `0066`–`0072` and local shader priming workarounds are not selected. Device-loss diagnostics use the upstream Vulkan call, result and fault report.

Host regression tests cover index exhaustion and retirement, completion timeouts, concurrent cache imports, cache ownership checks and heap placement. The public-API lifetime test also covers indirect declarations, texture views and heap retention behind an unsignaled event. GPU compilation is separate from sheng rendering validation. The retained Darwin clock and coherent-memory capability wiring previously reached the desktop with rendering corruption and still requires device testing.
