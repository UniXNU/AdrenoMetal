# AdrenoMetal

Metal frontend through metal2vulkan and Turnip.

The frontend uses metal2vulkan and Turnip with the kernel GPU interface and selected OS ABI. GPU composition has been verified on sheng with A740.

Poster processes use single-sample rendering. Same-user clients share writable shader caches. Resource aliasing, swizzles, range copies, function constants, specialized link names and allocation queries reach the runtime. Queue priorities aggregate current live requests without a process-name policy.

Build and interface details: [documentation](https://unixnu.com/drivers/adreno-metal). Component sources and dependencies: [catalog](https://unixnu.com/reference/components).

## Frame diagnostics

`sheng-gpu-trace=1` records frame identities, synchronization and display decisions. `sheng-gpu-pixels=1` retains bounded raw samples after GPU completion. The sheng log exporter carries both streams. See [GPU frame diagnostics](https://unixnu.com/porting/gpu-diagnostics) for fields and decoding.
