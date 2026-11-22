# AdrenoMetal

Metal frontend through metal2vulkan and Turnip. UniXNU selects the exact
metal2vulkan `development` revision in
`scripts/data/gpu-metal-vulkan-inputs.json` and applies the recorded patches in
`scripts/build-adreno-metal-bridge.py`.

This integration branch uses the rewritten runtime v2. Hardware qualification of
an earlier runtime does not qualify this version. Device creation checks the
upstream Vulkan floor and reports every missing requirement.

The native display driver presents completed images. The bridge creates the
runtime instance without Vulkan presentation, so surface and swapchain
extensions are omitted while all rendering and memory requirements remain.
Device-open failures are written to DEBUG stderr and the bounded timing log.

## Build checks

From the resolved UniXNU tree:

```sh
python3 scripts/check-metal2vulkan-pin.py --latest
python3 scripts/build-adreno-metal-bridge.py --target macos27 --check-only
```

The target check applies the complete patch sequence before checking public
interfaces. It type-checks arm64e without linking a plugin or packaging an image.
Source identities cover every Rust bridge module, the translator tree and the
embedded authored fixtures. Cached archives with another identity are rejected.

## Resources

The frontend owns Metal resource options, texture metadata, heap placement,
purgeable state and binary archives. Runtime objects retain placement leases
until encoded and submitted GPU work releases its references.

Small buffers share 64 KiB allocation blocks with 256-byte placement alignment.
The consumer keeps no idle buffer block: its last placement releases the block.
The runtime frees idle underlying allocations after 250 ms and retains at most
one 16 MiB reserve per memory class. Imported memory keeps a stable Darwin VM
alias until the Vulkan import is released, so reuse of the caller's virtual
address does not redirect a live import to different pages.

Automatic heaps allocate free ranges. Placement heaps validate explicit offsets.
`makeAliasable` releases a heap range; subsequent overlapping use requires the
caller's Metal synchronization.

`setPurgeableState:` can discard idle owned buffers, textures and heaps. Views,
imports, drawables, resources held by unfinished commands, and resources whose
CPU pointer or GPU address was handed out remain held. Pressure signals and a
250 ms idle timer trigger sweeps. `gpu-purge=0` keeps resources without discarding.

## Shader caches

Each process uses a writable cache directory. The runtime checks lowering keys,
profile fingerprints, versions and checksums before accepting cached functions.
Consumer binary archives retain compiled functions and pipeline membership;
`FailOnBinaryArchiveMiss` rejects a pipeline absent from its supplied archives.
Archives do not replace the runtime's partial-grid or SIMD variant selection.

The system cache reader checks allowed owners and permissions and traverses
without following symlinks. It supports complete and chunked entries and the new
runtime's two-generation layout. Both source-library replies and runtime
lowerings use the read-only layer after a writable-cache miss.

The macOS addon installs `/Applications/Shader Cache.app` and registers it with
LaunchServices for each Aqua session. Registration does not start compilation.
Start requests an administrator-authorized worker. Stop finishes the current
compilation and saves progress. The worker prevents automatic display and system
sleep while compiling and releases its assertions on completion, stop or failure.
`--status` prints its current state as JSON.

## Diagnostics

DEBUG sampling requires `diag=1`, exposed to the process as `SHENG_DIAG=1`.
`MEM2` reports upstream live memory gauges. `RT2` reports submissions, copied
bytes, pipeline creation, cache activity, allocation categories and first-use
waits. These snapshots have their own versioned layouts; removed runtime-v1
statistics queries report their schema as unavailable.

The bounded bridge stage ring records commit calls and completion callbacks.
These are bridge events, not Vulkan submission timestamps. The upstream metrics
supply submission and completion timing. Device-loss errors retain the first
runtime source location and fault report. Kernel and Turnip GPU logs remain
available for correlation.

`gpu-trace=1` retains frontend command phases and encoding batches. Detailed
sampling stays off in the ordinary release configuration.

## Current upstream limits

Runtime v2 explicitly refuses memoryless textures, YCbCr and extended-range
texture conversions, tile pipelines and tile dispatch. The frontend does not
advertise those capabilities. Ordinary color IOSurface planes use the runtime's
buffer-texture path. Nonzero texture rotations, padded 3D buffer-texture image
pitches, indexed control-point patch draws and tessellation factor scaling also
report explicit errors.

Read-only no-copy imports require an upstream import contract beyond the current
writable-host-memory API. They report an error rather than dropping their
protection flag. Kernels that need the upstream external stage helper report
`StageUnfilled` until that helper is integrated.

The rewritten runtime exposes no resource-overlap query. Such queries report an
unknown dependency, preserving the frontend's conservative resource wait.

Build checks establish source and interface compatibility. Rendering, memory
use, cache reuse and recovery require separate sheng device tests.
