# AdrenoMetal

Metal frontend through metal2vulkan and Turnip.

The frontend uses metal2vulkan and Turnip with the kernel GPU interface and selected OS ABI. GPU composition has been verified on sheng with A740.

All clients use single-sample rendering by default. `NATIVE_METAL_SINGLE_SAMPLE=0` preserves a process's requested sample counts; `1` forces single-sample rendering. Texture types, pipeline sample counts and resolve outputs follow the selected policy. Same-user clients share writable shader caches. Resource aliasing, swizzles, range copies, function constants, specialized link names and allocation queries reach the runtime. Queue priorities aggregate current live requests without a process-name policy.

Build and interface details: [documentation](https://unixnu.com/drivers/adreno-metal). Component sources and dependencies: [catalog](https://unixnu.com/reference/components).

## Memory reuse

Unused owned image backing is reused by pooled and dedicated images. Extra spares have no byte limit, expire after ten seconds, and go sooner under memory pressure or when a new block cannot be made. Blocks are freed off the releasing thread on the runtime's free thread, and on the releasing thread once its queue is full. An allocation failure releases kept spares and waits for posted frees before retrying. Imported memory and owner callbacks keep their release ordering.

## Shader caches and priming

Each process keeps its pipeline cache in the first directory its sandbox lets it write: the same-user shared directory, then Metal's own cache directory. Compiler replies for source and stitched libraries are kept beside it. A cache entry's access time is refreshed on a hit until the platform refuses the change once.

The render server primes Metal libraries in the background while it draws nothing. On macOS, where the render server and the apps are different users with separate caches, it primes only the libraries of bundles loaded in its own process, and any command buffer in the last quarter second counts as drawing.

## Telemetry table

With `diag=1`, the render server sends its telemetry table to the kernel log (`[metal table]`, shown with `log-gpu=1`) every six seconds and to the system log every thirty (`NativeAdrenoMetal table:`). The kernel keeps the first 48 rows of at most 90 characters. Memory rows come first: `MEM` (MiB by kind and Turnip's BO cache), `MK`, `MKb`, `MKc` (blocks by size class: pooled, empty, spare, dedicated images, dedicated buffers), `ML` (lazily allocated blocks and the MiB the driver backed), `PG` (declared purgeable), `PE` (memory pressure events and held surface imports), `RM`, `PS` and `MA` (allocation call stacks). Compile rows follow: `T` (pipeline, library, compiler and function creation), `VP` (driver pipelines made at creation and at draws, with the driver's time), `V`, `SC`, `PR` (priming) and `FL`. `TH` rows list the threads that used the most CPU since the last table. Other processes write one `NativeAdrenoMetal work:` line to the system log every thirty seconds while their compile counts change.

## Frame diagnostics

With `gpu-trace=1`, the render server writes command phases, encoding batches, attachments and Turnip pass timings to its frame trace. Kernel event 63 identifies the render, encode and completion threads. In `iconservicesagent`, event 65 records the Metal device and queue, then up to eight completed command buffers containing draws or dispatches. Each work record includes the command buffer status and draw and dispatch counts. [GPU frame diagnostics](https://unixnu.com/porting/gpu-diagnostics)

`wallpaper-freeze=1` enables wallpaper isolation. A Poster renderer freezes GPU submission after a successful command draws into and stores an IOSurface of at least two million pixels. The completed output stays retained. Later commands remain committed and unscheduled, with their resources and queue leases retained. Already submitted work completes normally. PosterBoard and other processes keep submitting. The switch defaults to off; `NATIVE_METAL_FREEZE_WALLPAPER=1` selects the same mode directly in a renderer process.

`metal-flow=1` enables the runtime's lowering thread, commit submission and pipeline warm pass for testing. The switch defaults to off. Either value of `metal-flow=` logs the `FL` counts to os_log when they change.

Slow encoding operations retain source lines and enqueue times. Completion handlers and resource destructors carry individual timing and identities. Kernel sampling of the render, encode and completion threads and blocked-thread waits provide the cross-thread wait record.

`python3 scripts/check-wallpaper-freeze-host.py` in the resolved tree checks real pixels, callbacks, concurrent parked submissions and process selection.

The macOS plugin entry (the IOGPUMetalDevice subclass), the layout of block-compressed textures and WindowServer's frame pacing options follow [macOS-DroidVM](https://github.com/Droid-VM/macOS-DroidVM), the DroidVM port booted with UniXNU.
