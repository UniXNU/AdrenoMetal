# AdrenoMetal

Metal frontend through metal2vulkan and Turnip.

The frontend uses metal2vulkan and Turnip with the kernel GPU interface and selected OS ABI. GPU composition has been verified on sheng with A740.

All clients use single-sample rendering by default. `NATIVE_METAL_SINGLE_SAMPLE=0` preserves a process's requested sample counts; `1` forces single-sample rendering. Texture types, pipeline sample counts and resolve outputs follow the selected policy. Same-user clients share writable shader caches. Resource aliasing, swizzles, range copies, function constants, specialized link names and allocation queries reach the runtime. Queue priorities aggregate current live requests without a process-name policy.

Build and interface details: [documentation](https://unixnu.com/drivers/adreno-metal). Component sources and dependencies: [catalog](https://unixnu.com/reference/components).

## Memory reuse

Unused owned image backing is reused by pooled and dedicated images. Extra spares have no byte limit, expire after ten seconds, and go sooner under memory pressure or when a new block cannot be made. Blocks are freed off the releasing thread on the runtime's free thread, and on the releasing thread once its queue is full. An allocation failure releases kept spares and waits for posted frees before retrying. Imported memory and owner callbacks keep their release ordering.

## Frame diagnostics

With `sheng-gpu-trace=1`, the render server writes command phases, encoding batches, attachments and Turnip pass timings to its frame trace. Kernel event 63 identifies the render, encode and completion threads. In `iconservicesagent`, event 65 records the Metal device and queue, then up to eight completed command buffers containing draws or dispatches. Each work record includes the command buffer status and draw and dispatch counts. [GPU frame diagnostics](https://unixnu.com/porting/gpu-diagnostics)

`sheng-wallpaper-freeze=1` enables wallpaper isolation. A Poster renderer freezes GPU submission after a successful command draws into and stores an IOSurface of at least two million pixels. The completed output stays retained. Later commands remain committed and unscheduled, with their resources and queue leases retained. Already submitted work completes normally. PosterBoard and other processes keep submitting. The switch defaults to off; `NATIVE_METAL_FREEZE_WALLPAPER=1` selects the same mode directly in a renderer process.

`sheng-metal-flow=1` enables the runtime's lowering thread, commit submission and pipeline warm pass for testing. The switch defaults to off. Either value of `sheng-metal-flow=` logs the `FL` counts to os_log when they change.

Slow encoding operations retain source lines and enqueue times. Completion handlers and resource destructors carry individual timing and identities. Kernel sampling of the render, encode and completion threads and blocked-thread waits provide the cross-thread wait record.

`python3 scripts/check-wallpaper-freeze-host.py` in the resolved tree checks real pixels, callbacks, concurrent parked submissions and process selection.
