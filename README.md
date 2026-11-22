# AdrenoMetal upstream-minimal experiment

This branch uses metal2vulkan development `4e9aad67d5af604a64725e26f3070968dd15a618`. Its only runtime patch is `0073`, which selects native presentation without requiring Vulkan surfaces or swapchains. Rendering and device capability requirements remain upstream implementations.

AdrenoMetal owns descriptor-index grants, heap and buffer placement leases, and bounded completion waits through public runtime APIs. Commands retain consumer ownership tokens through GPU retirement. Texture views retain backing leases separately from descriptor indices.

Binary archives and the read-only system cache populate the upstream cache directory, isolated by runtime build identity so removed translator patches cannot leave reusable artifacts. Imports preserve its generation lock, byte accounting and version record. A system-cache writer publishes readable files; ordinary applications create no publication worker. Pipeline creation and cache decoding remain upstream operations.

The removed runtime patches `0066`–`0072` and local shader priming workarounds are not selected. Device-loss diagnostics use the upstream Vulkan call, result and fault report.

Host regression tests cover index exhaustion and retirement, completion timeouts, concurrent cache imports, cache ownership checks and heap placement. GPU compilation is separate from sheng rendering validation. The retained Darwin clock and coherent-memory capability wiring previously reached the desktop with rendering corruption and still requires device testing.
