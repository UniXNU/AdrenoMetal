# AdrenoMetal

Metal frontend through metal2vulkan and Turnip.

The frontend uses metal2vulkan and Turnip with the kernel GPU interface and selected OS ABI. GPU composition has been verified on sheng with A740.

All clients use single-sample rendering by default. `NATIVE_METAL_SINGLE_SAMPLE=0` preserves a process's requested sample counts; `1` forces single-sample rendering. Texture types, pipeline sample counts and resolve outputs follow the selected policy. Same-user clients share writable shader caches. Resource aliasing, swizzles, range copies, function constants, specialized link names and allocation queries reach the runtime. Queue priorities aggregate current live requests without a process-name policy.

Build and interface details: [documentation](https://unixnu.com/drivers/adreno-metal). Component sources and dependencies: [catalog](https://unixnu.com/reference/components).

## Memory reuse

Unused owned image backing is reused by pooled and dedicated images. Extra spares have no byte limit, expire after ten seconds, and go sooner under memory pressure or when a new block cannot be made. Blocks are freed off the releasing thread on the runtime's free thread, and on the releasing thread once its queue is full. An allocation failure releases kept spares and waits for posted frees before retrying. Imported memory and owner callbacks keep their release ordering. Released images and buffers the runtime parks for reuse, and kept empty pooled blocks beyond one a pool, go when their time is up even in a process that makes and releases nothing more.

## Purgeable memory

`setPurgeableState:` follows Metal's contract: a resource whose state is Volatile or Empty may lose its contents, and the call answers Empty for contents that were discarded. The bridge discards by releasing the runtime resource and makes it again from the same description, so a batch in flight keeps what it uses and the frontend handle stays valid.

Discarded: a texture or buffer the device made of its own memory, and a heap, where the runtime keeps a purgeable state for it (at least 16 KiB, not memoryless). Never discarded: a texture over a buffer or a surface, a no-copy buffer, a view, a resource placed in a heap, a drawable, and a resource that

- a command buffer being recorded or in flight names,
- has a live view, a texture over it or a resource placed in it,
- has handed out its CPU pointer (`contents`) or GPU address (`gpuAddress`), which Metal keeps for the resource's life.

Triggers: the system's memory pressure event (every idle Volatile or Empty resource), and age (idle for the runtime's spare block age, ten seconds).

After a discard: NonVolatile places the resource again and answers Empty; a query answers Empty and places nothing; any other use places it again with zeroed memory and leaves its state Empty. A placement that fails behaves as a failed allocation.

`gpu-purge=0` keeps the state without discarding anything (`NATIVE_METAL_PURGE=0` selects the same in a process).

## Shader caches and priming

Each process keeps its pipeline cache in the first directory its sandbox lets it write: the same-user shared directory, then Metal's own cache directory. Compiler replies for source and stitched libraries are kept beside it. A cache entry's access time is refreshed on a hit until the platform refuses the change once.

The render server primes Metal libraries in the background while it draws nothing. On macOS, where the render server and the apps are different users with separate caches, it primes only the libraries of bundles loaded in its own process, and any command buffer in the last quarter second counts as drawing.

## Telemetry table

With `diag=1`, the render server sends its telemetry table to the kernel log (`[metal table]`, shown with `log-gpu=1`) every six seconds and to the system log every thirty (`NativeAdrenoMetal table:`). The kernel keeps the first 48 rows of at most 90 characters. Memory rows come first: `MEM` (MiB by kind and Turnip's BO cache), `MK`, `MKb`, `MKc` (blocks by size class: pooled, empty, spare, dedicated images, dedicated buffers), `ML` (lazily allocated blocks and the MiB the driver backed), `PG` (declared purgeable), `PGb` (MiB discarded now, by pressure and by age, placed again by request and by another use, Empty answers), `PGc` (MiB declared, MiB never discardable because a pointer or address was handed out, what the last pass left in use, tied or young, failed placements), `PU` (only when a command named a discarded resource whose caller had not set it NonVolatile: the count and the first two callers' return addresses), `PE` (memory pressure events and held surface imports), `RM`, `PS` and `MA` (allocation call stacks). Compile rows follow: `T` (pipeline, library, compiler and function creation), `VP` (driver pipelines made at creation and at draws, with the driver's time), `V`, `SC`, `PR` (priming) and `FL`. `TH` rows list the threads that used the most CPU since the last table. Other processes write one `NativeAdrenoMetal work:` line to the system log every thirty seconds while their compile counts change. They also write a `NativeAdrenoMetal memory:` line each time their runtime blocks pass the next power of two from 64 MiB (blocks, bytes placed, declared purgeable, discarded) and a `NativeAdrenoMetal purge:` line for each pass that discarded something. With `diag=1` or `log-gpu=1`, any process writes a `NativeAdrenoMetal late use:` line for each of its first four callers that name a discarded resource without setting it NonVolatile first.

## Frame diagnostics

With `gpu-trace=1`, the render server writes command phases, encoding batches, attachments and Turnip pass timings to its frame trace. Kernel event 63 identifies the render, encode and completion threads. In `iconservicesagent`, event 65 records the Metal device and queue, then up to eight completed command buffers containing draws or dispatches. Each work record includes the command buffer status and draw and dispatch counts. [GPU frame diagnostics](https://unixnu.com/porting/gpu-diagnostics)

`wallpaper-freeze=1` enables wallpaper isolation. A Poster renderer freezes GPU submission after a successful command draws into and stores an IOSurface of at least two million pixels. The completed output stays retained. Later commands remain committed and unscheduled, with their resources and queue leases retained. Already submitted work completes normally. PosterBoard and other processes keep submitting. The switch defaults to off; `NATIVE_METAL_FREEZE_WALLPAPER=1` selects the same mode directly in a renderer process.

`metal-flow=1` enables the runtime's lowering thread, commit submission and pipeline warm pass for testing. The switch defaults to off. Either value of `metal-flow=` logs the `FL` counts to os_log when they change.

Slow encoding operations retain source lines and enqueue times. Completion handlers and resource destructors carry individual timing and identities. Kernel sampling of the render, encode and completion threads and blocked-thread waits provide the cross-thread wait record.

`python3 scripts/check-wallpaper-freeze-host.py` in the resolved tree checks real pixels, callbacks, concurrent parked submissions and process selection.

The macOS plugin entry (the IOGPUMetalDevice subclass), the layout of block-compressed textures and WindowServer's frame pacing options follow [macOS-DroidVM](https://github.com/Droid-VM/macOS-DroidVM), the DroidVM port booted with UniXNU.
