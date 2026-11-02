# AdrenoMetal

Metal frontend through metal2vulkan and Turnip.

The frontend uses metal2vulkan and Turnip with the kernel GPU interface and selected OS ABI. GPU composition has been verified on sheng with A740.

All clients use single-sample rendering by default. `NATIVE_METAL_SINGLE_SAMPLE=0` preserves a process's requested sample counts; `1` forces single-sample rendering. Texture types, pipeline sample counts and resolve outputs follow the selected policy. Same-user clients share writable shader caches. Resource aliasing, swizzles, range copies, function constants, specialized link names and allocation queries reach the runtime. Queue priorities aggregate current live requests without a process-name policy.

Build and interface details: [documentation](https://unixnu.com/drivers/adreno-metal). Component sources and dependencies: [catalog](https://unixnu.com/reference/components).

## Memory reuse

Unused owned image backing can serve later pooled or dedicated images. Extra image spares share a 256 MiB device budget and expire after ten seconds. Plain unused backing of at least 256 KiB can be released by the existing room worker, with at most 128 MiB queued. Allocation failure reclaims queued backing before retrying. Imported memory and owner callbacks retain their release ordering.

## Frame diagnostics

`sheng-gpu-trace=1` records frame identities, synchronization and display decisions. `sheng-gpu-pixels=1` retains bounded raw samples after GPU completion. The sheng log exporter carries both streams. Events 61 through 63 identify encoding batches, frontend phases and render threads. The persistent frame trace includes memory decisions, completion stages and attachment conditions. See [GPU frame diagnostics](https://unixnu.com/porting/gpu-diagnostics) for fields and decoding.

`sheng-wallpaper-freeze=1` enables wallpaper isolation. A Poster renderer freezes GPU submission after a successful command draws into and stores an IOSurface of at least two million pixels. The completed output stays retained. Later commands remain committed and unscheduled, with their resources and queue leases retained. Already submitted work completes normally. PosterBoard and other processes keep submitting. The switch defaults to off; `NATIVE_METAL_FREEZE_WALLPAPER=1` selects the same mode directly in a renderer process. Trace event 60 records admitted commands, the completion transition and parked commands.

Slow encoding operations retain source lines and enqueue times. Completion handlers and resource destructors carry individual timing and identities. Dispatch introspection records its availability and, with ABI v2, task enqueue/dequeue/completion and parent links. Kernel blocked-thread sampling and Mach request/reply ports provide the cross-process wait record.

`python3 scripts/check-wallpaper-freeze-host.py` in the resolved tree checks real pixels, callbacks, concurrent parked submissions and process selection.
