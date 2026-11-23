// SPDX-License-Identifier: MIT
//! The bridge's Metal objects behind a C ABI (NativeMetalObjects.h): the metal2vulkan runtime's device, resources, states, queues,
//! command buffers and encoders as boxed handles for the Objective-C frontend. Raw Metal enumerations go through the runtime's own
//! checked conversions; a value it does not know fails the call (never a substitute). Failures leave their reason in a per-thread
//! string, and no call unwinds into C.

use metal2vulkan::bind::SamplerDescriptor;
use metal2vulkan::metal::state::{DepthStencilDescriptor, StencilDescriptor};
use metal2vulkan::library::{FunctionConstantValue, FunctionConstantValues};
use metal2vulkan::metal::{
    BlendFactor, BlendOperation, ColorWriteMask, CompareFunction, CullMode, DataType, IndexType, PixelFormat, PrimitiveTopologyClass,
    PrimitiveType, RenderStages, SamplerAddressMode, SamplerBorderColor, SamplerMinMagFilter, SamplerMipFilter, StencilOperation, TextureType,
    TessellationControlPointIndexType, TessellationFactorFormat, TessellationFactorStepFunction, TessellationPartitionMode, TextureUsage,
    TextureSwizzle, TextureSwizzleChannels, TriangleFillMode, VertexFormat, VertexStepFunction, Winding,
};
use metal2vulkan::pipeline::{
    ColorAttachmentDescriptor, ComputePipelineDescriptor, LinkedFunction, LinkedFunctions, RenderPipelineDescriptor, StageSource,
    TessellationDescriptor, TileRenderPipelineDescriptor, VertexAttributeDescriptor, VertexBufferLayoutDescriptor, VertexDescriptor,
};
use metal2vulkan_runtime::{
    BlitCommandEncoder, ClearColor, CommandBuffer, CommandQueue, ComputeCommandEncoder,
    DepthStencilState, Device, Event, Fence, LoadAction,
    Region, RenderCommandEncoder, RenderPassDescriptor, Resource as RuntimeResource, ResourceUsage,
    SamplerState, ScissorRect, SharedEvent, StoreAction, Viewport, VisibilityResultMode,
};
use crate::purge::{Cause, Purge, Resource};
use crate::library::{Function, Library};
use crate::resources::{Buffer, PurgeableState, ResourceOptions};
use crate::archive::BinaryArchive;
use crate::pipelines::{ComputePipelineState, RenderPipelineState};
use crate::ownership::{Owner, Ownership};
use std::sync::{Arc, Mutex};
use crate::texture::{Texture, TextureDescriptor};
use crate::heap::{Heap, HeapDescriptor, HeapType};
type Deallocator = Box<dyn FnOnce(NonNull<u8>, u64) + Send>;
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::time::Duration;
use metal2vulkan_runtime::{CommandBufferError, MultisampleDepthResolveFilter, MultisampleStencilResolveFilter};

thread_local! {
    static LAST: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static COMPLETING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SCHEDULING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn set_error(why: String) {
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        last.clear();
        last.extend(why.bytes().filter(|b| *b != 0));
        last.push(0);
    });
}

/// Runs `body`, turning an error or a panic into the per-thread reason and `fallback`.
fn guard<T>(fallback: T, body: impl FnOnce() -> Result<T, String>) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(Ok(value)) => value,
        Ok(Err(why)) => {
            set_error(why);
            fallback
        }
        Err(payload) => {
            let why = payload.downcast_ref::<String>().map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied()).unwrap_or("unknown panic payload");
            set_error(format!("panic in the Metal runtime: {why}"));
            fallback
        }
    }
}

fn boxed<T>(value: T) -> *mut T {
    Box::into_raw(Box::new(value))
}

/// The object a handle points at.
fn get<'a, T>(handle: *const T) -> Result<&'a T, String> {
    // SAFETY: handles come from `boxed` and live until their release call (the header's contract).
    unsafe { handle.as_ref() }.ok_or_else(|| "null handle".to_string())
}

fn get_mut<'a, T>(handle: *mut T) -> Result<&'a mut T, String> {
    // SAFETY: as in `get`; an encoder is used by one thread at a time (Metal's rule).
    unsafe { handle.as_mut() }.ok_or_else(|| "null handle".to_string())
}

fn release<T>(handle: *mut T) {
    if !handle.is_null() {
        // SAFETY: from `boxed`, released once.
        drop(unsafe { Box::from_raw(handle) });
    }
}

fn copy_out(text: &str, out: *mut c_char, size: usize) -> usize {
    if !out.is_null() && size > 0 {
        let n = text.len().min(size - 1);
        // SAFETY: the caller's buffer holds `size` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), out as *mut u8, n);
            *out.add(n) = 0;
        }
    }
    text.len()
}

fn raw<T: TryFrom<u32>>(value: u64, what: &str) -> Result<T, String>
where
    T::Error: std::fmt::Debug,
{
    u32::try_from(value).ok().and_then(|v| T::try_from(v).ok()).ok_or_else(|| format!("{what} {value} is not a Metal value"))
}

fn e<E: std::fmt::Debug>(what: &'static str) -> impl FnOnce(E) -> String {
    move |error| format!("{what}: {error:?}")
}

fn narrow(value: u64) -> Result<u32, String> { u32::try_from(value).map_err(|_| format!("Metal integer {value} exceeds u32")) }

fn options(value: u64) -> Result<ResourceOptions, String> {
    ResourceOptions::from_raw(value).map_err(e("resource options"))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_last_error() -> *const c_char {
    LAST.with(|last| {
        let last = last.borrow();
        if last.is_empty() { c"".as_ptr() } else { last.as_ptr() as *const c_char }
    })
}

/// The GPU scanout's half-float conversion kernel (shaders/scanout_convert.ll), assembled to AIR at build time.
static SCANOUT_CONVERT_AIR: &[u8] = include_bytes!(env!("NMB_SCANOUT_AIR"));

/// # Safety
/// `length` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_scanout_convert_air(length: *mut usize) -> *const c_void {
    // SAFETY: the caller's contract.
    if let Some(length) = unsafe { length.as_mut() } {
        *length = SCANOUT_CONVERT_AIR.len();
    }
    SCANOUT_CONVERT_AIR.as_ptr() as *const c_void
}

/// The runtime's kernel_function_constant_gate_fold fixture, assembled to AIR at build time.
static FUNCTION_CONSTANT_AIR: &[u8] = include_bytes!(env!("NMB_FC_AIR"));

/// # Safety
/// `length` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_fixture_air(length: *mut usize) -> *const c_void {
    // SAFETY: the caller's contract.
    if let Some(length) = unsafe { length.as_mut() } {
        *length = crate::FIXTURE_AIR.len();
    }
    crate::FIXTURE_AIR.as_ptr() as *const c_void
}

/// # Safety
/// `length` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_constant_air(length: *mut usize) -> *const c_void {
    // SAFETY: the caller's contract.
    if let Some(length) = unsafe { length.as_mut() } {
        *length = FUNCTION_CONSTANT_AIR.len();
    }
    FUNCTION_CONSTANT_AIR.as_ptr() as *const c_void
}

// ---- device ----

pub struct NmbDevice(Device, std::sync::Arc<crate::archive::Registry>, std::sync::Mutex<NoCopyPages>, std::sync::Arc<Purge>);

/// # Safety
/// `loader` and `cache` are C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_device_open(loader: *const c_char, cache: *const c_char) -> *mut NmbDevice {
    guard(std::ptr::null_mut(), || {
        if loader.is_null() || cache.is_null() {
            return Err("loader and cache are required".into());
        }
        // SAFETY: the caller's contract.
        let (loader, cache) = unsafe { (CStr::from_ptr(loader), CStr::from_ptr(cache)) };
        open_runtime_diagnostics(cache.to_string_lossy().as_ref());
        let (device, registry) = match crate::open(loader, cache) {
            Ok(opened) => opened,
            Err(error) => {
                if std::env::var("SHENG_DIAG").as_deref() == Ok("1") {
                    eprintln!("{} device-open refused: {error}", diagnostic_stamp());
                    note_encoder_timing(&format!("device-open refused: {error}"));
                }
                return Err(error);
            }
        };
        write_runtime_policy(&device);
        let purge = Purge::new(&device);
        Ok(boxed(NmbDevice(device, registry, std::sync::Mutex::default(), purge)))
    })
}

static LAST_WARNING: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

const TIMING_LOG_LIMIT: usize = 512 * 1024;
struct TimingLog { file: std::fs::File, bytes: usize }
static TIMING_LOG: std::sync::OnceLock<std::sync::Mutex<TimingLog>> = std::sync::OnceLock::new();
static POLICY_LOG: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

fn open_runtime_diagnostics(cache: &str) {
    if std::env::var_os("SHENG_DIAG").is_none() { return; }
    let root = PathBuf::from(cache);
    let _ = POLICY_LOG.set(root.join("runtime-policy.log"));
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(root.join("timing.log")) {
        let bytes = file.metadata().map_or(0, |m| m.len() as usize);
        let _ = TIMING_LOG.set(std::sync::Mutex::new(TimingLog { file, bytes }));
    }
}

fn diagnostic_stamp() -> String {
    let boot = std::env::var("NATIVE_METAL_BOOT_SESSION").unwrap_or_else(|_| "unknown".into());
    let build = option_env!("NMB_COMPONENT_ID").unwrap_or("host-working-tree");
    let uptime = metal2vulkan_runtime::host_time() / 1000;
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros());
    format!("boot={boot} build={build} runtime={} uptime_us={uptime} time_us={time}", env!("NMB_RUNTIME_ID"))
}

fn write_runtime_policy(device: &Device) {
    let Some(path) = POLICY_LOG.get() else { return; };
    let text = format!("{}\nunified={}\nextensions={:#?}\nmemory_types={:#?}\nprofile={:#?}\n",
        diagnostic_stamp(), device.has_unified_memory(), device.extensions(),
        device.memory_types(), device.translator().profile());
    let text: String = text.chars().take(64 * 1024).collect();
    let _ = std::fs::write(path, text);
}

fn note_encoder_timing(text: &str) {
    use std::io::Write;
    let Some(log) = TIMING_LOG.get() else { return; };
    let Ok(mut log) = log.lock() else { return; };
    let text: String = text.chars().take(4096).collect();
    let line = format!("{} {text}\n", diagnostic_stamp());
    if log.bytes + line.len() > TIMING_LOG_LIMIT {
        if log.file.set_len(0).is_err() { return; }
        log.bytes = 0;
    }
    if log.file.write_all(line.as_bytes()).is_ok() { log.bytes += line.len(); }
}

/// The translator source the runtime was built from (its pin and recorded patches, `build-native-metal-bridge.py`), which keys the shader
/// cache, as a C string.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_runtime_build() -> *const c_char {
    concat!(env!("NMB_RUNTIME_ID"), "\0").as_ptr().cast()
}

/// The sink's counts into the caller's ten words: pipeline makings (count, total us, longest us, waited us, failed), messages (info,
/// warning or worse), and the device's shader cache hits, misses and writes.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_work_stats(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// The device's shader cache counts into the caller's 24 words (`nmb_device_shader_cache_detail` in the header): everything
/// `ShaderCacheStatistics` holds that tells whether the cache worked and why not.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_shader_cache_detail(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// The device's texture copy counts into the caller's eight words: textures over host pages in place, of them linear images at Metal's
/// row pitch, host-page textures shadowed for their pitch and for no image the driver makes, textures given a shadow image, shadow
/// copies in and texels copied back, and host-page textures shadowed as render targets.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_texture_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// The warm pass and where work ran, into the caller's eight words: records warmed, failed and unread, driver bytes the pass added; scheduler
/// passes and batches submitted on the committing thread; render log waits, and encoder ends the lowering thread lowered.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_flow_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// Submission grouping, command-pool reuse, in-place passes and commit costs, into the header's fifteen words.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_fast_path_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// Where render passes cost the GPU more than Metal's single pass in tile memory, into the caller's ten words: passes begun as
/// instances, passes split inline, then instance splits for a barrier, tile memory, a memoryless write and a pending pipeline, stand-ins
/// made and their bytes, and stand-in copies made and those made at draws.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_pass_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// Runtime v2 memory counters. Fields overlap; each counter is sampled independently.
#[repr(C)]
pub struct NmbRuntimeMemory {
    pub version: u64,
    pub sample_ns: u64,
    pub shared_allocation_bytes: u64,
    pub private_allocation_bytes: u64,
    pub shared_placed_bytes: u64,
    pub private_placed_bytes: u64,
    pub shared_idle_bytes: u64,
    pub private_idle_bytes: u64,
    pub shared_reserve_bytes: u64,
    pub private_reserve_bytes: u64,
    pub shared_idle_age_ns: u64,
    pub private_idle_age_ns: u64,
    pub import_bytes: u64,
    pub export_bytes: u64,
    pub surface_import_bytes: u64,
    pub imports: u64,
    pub command_buffers: u64,
    pub releases_waiting: u64,
    pub allocations_freed: u64,
    pub texture_block_bytes: u64,
    pub texture_block_held_bytes: u64,
    pub shared_buffer_bytes: u64,
    pub private_buffer_bytes: u64,
}

/// # Safety
/// `out` points to one writable `NmbRuntimeMemory`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_device_runtime_memory(device: *const NmbDevice, out: *mut NmbRuntimeMemory) -> bool {
    guard(false, || {
        if std::env::var("SHENG_DIAG").as_deref() != Ok("1") {
            return Ok(false);
        }
        // SAFETY: the caller supplies one writable record.
        let out = unsafe { out.as_mut() }.ok_or("null memory snapshot")?;
        let counters = get(device)?.0.counters();
        let g = counters.gauges();
        let m = counters.metrics();
        *out = NmbRuntimeMemory {
            version: 2,
            sample_ns: metal2vulkan_runtime::host_time(),
            shared_allocation_bytes: g.shared_allocation_bytes,
            private_allocation_bytes: g.private_allocation_bytes,
            shared_placed_bytes: g.shared_placed_bytes,
            private_placed_bytes: g.private_placed_bytes,
            shared_idle_bytes: g.shared_idle_bytes,
            private_idle_bytes: g.private_idle_bytes,
            shared_reserve_bytes: g.shared_reserve_bytes,
            private_reserve_bytes: g.private_reserve_bytes,
            shared_idle_age_ns: g.shared_idle_age_nanos,
            private_idle_age_ns: g.private_idle_age_nanos,
            import_bytes: g.import_bytes,
            export_bytes: g.export_bytes,
            surface_import_bytes: g.surface_import_bytes,
            imports: g.imports,
            command_buffers: g.command_buffers,
            releases_waiting: g.releases_waiting,
            allocations_freed: m.allocations_freed,
            texture_block_bytes: g.texture_block_bytes,
            texture_block_held_bytes: g.texture_block_held_bytes,
            shared_buffer_bytes: g.shared_buffer_bytes,
            private_buffer_bytes: g.private_buffer_bytes,
        };
        Ok(true)
    })
}

/// Where the runtime's device memory is, into the caller's ten words: bytes of its pooled blocks with a placement and of those the
/// bytes placed, of its empty kept pooled blocks, of its dedicated blocks (GPU pools, large resources) and of its host imports; then
/// the bytes it allocated so far, its live pooled and dedicated block counts, and the GPU pool blocks made and released.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_memory_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// The runtime's live blocks of its own device memory by what holds them and by size, into the caller's 48 words:
/// seven size classes each (at most 16, 32, 40, 48, 56 and 64 MiB, and larger) of the pooled blocks with a placement, the pooled
/// blocks kept empty, the spare image blocks, the dedicated blocks of an image and those of a buffer; then the bytes of the spare
/// blocks and of the dedicated blocks of images and of buffers; then, of the blocks of a lazily allocated memory type, those with a
/// placement and those kept without one, each as blocks, bytes, blocks the driver committed and bytes it committed; then the blocks
/// and bytes of buffers' own imported pages.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_memory_kinds(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// The driver pipelines the runtime made, into the caller's eight words: `VkPipeline`s its pipeline states' variants made, of them
/// those made at a state's creation and how many of those a draw or dispatch took, those a host step made at a draw or dispatch,
/// the nanoseconds its graphics and its compute pipeline makings spent inside the driver, the functions translated and the shader
/// modules made. Counters the runtime keeps at all times: the creation log need not be on.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_pipeline_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// The runtime's command buffer stages (bridge patch 0022, `stage_trace.rs`) from sequence `from` on, at most `max` records of
/// {u64 host uptime ns, u64 id, u32 sequence, u16 stage, u16 arg}: how many were written; `*next` the sequence to ask for next.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_stage_ring_read(from: u64, out: *mut crate::trace::StageRecord, max: usize,
                                      next: *mut u64) -> usize {
    if out.is_null() || next.is_null() {
        return 0;
    }
    // SAFETY: the caller's `max` records and its word.
    let records = unsafe { std::slice::from_raw_parts_mut(out, max) };
    let (count, following) = crate::trace::read(from, records);
    unsafe { *next = following };
    count
}

/// Name the read-only system cache layer for the device opened after this call (bridge patch 0036): the directory at `root`, read
/// only where it, every directory on the way to an entry and the entry are owned by one of the `count` user ids at `owners` and
/// not writable by group or others, and never through a link. False with the reason in `nmb_last_error` where the directory fails
/// that test; no layer is named then. A null `root` names none.
///
/// # Safety
/// `root` is null or a C string; `owners` points at `count` user ids.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_system_cache_open(root: *const c_char, owners: *const u32, count: usize) -> bool {
    guard(false, || {
        crate::system_cache::set_system_layer(None);
        if root.is_null() {
            return Ok(true);
        }
        if owners.is_null() || count == 0 {
            return Err("system cache: no owner".into());
        }
        // SAFETY: the caller's contract.
        let (root, owners) = unsafe { (CStr::from_ptr(root), std::slice::from_raw_parts(owners, count)) };
        let layer = crate::system_cache::SystemLayer::open(&PathBuf::from(root.to_string_lossy().into_owned()), owners)
            .map_err(|why| format!("system cache: {why}"))?;
        crate::system_cache::set_system_layer(Some(std::sync::Arc::new(layer)));
        Ok(true)
    })
}

/// Whether the cache directory of the device opened after this call makes its directories and entries readable by everyone
/// (0755, 0644) instead of by their owner alone: the directory of the process that fills the system layer.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_cache_readable_by_all(on: bool) {
    crate::system_cache::set_cache_readable_by_all(on);
}

/// The system layer's counts into the caller's four words: entries read from it and accepted, entries whose contents were refused,
/// levels refused for their kind, owner or mode, and the accepted entries' bytes. False without a layer.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_system_cache_counts(out: *mut u64) -> bool {
    guard(false, || {
        if out.is_null() {
            return Err("null counts".into());
        }
        let Some(layer) = crate::system_cache::system_layer() else {
            return Ok(false);
        };
        let c = layer.counts();
        // SAFETY: the header's four words.
        unsafe { std::slice::from_raw_parts_mut(out, 4) }.copy_from_slice(&[c.accepted, c.rejected, c.refused, c.accepted_bytes]);
        Ok(true)
    })
}

/// The bytes of the file at `relative` under the system layer, read by the layer's rules, at most `limit`: a block of `malloc`
/// the caller frees, its length in `length`; null where there is no layer or the file is absent, refused or longer. The caller
/// checks the contents and reports the verdict (`nmb_system_cache_verdict`).
///
/// # Safety
/// `relative` is a C string and `length` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_system_cache_read(relative: *const c_char, limit: u64, length: *mut usize) -> *mut u8 {
    guard(std::ptr::null_mut(), || {
        if relative.is_null() || length.is_null() {
            return Err("system cache read: null argument".into());
        }
        let Some(layer) = crate::system_cache::system_layer() else {
            return Ok(std::ptr::null_mut());
        };
        // SAFETY: the caller's contract.
        let relative = unsafe { CStr::from_ptr(relative) }.to_string_lossy().into_owned();
        let Some(bytes) = layer.read(std::path::Path::new(&relative), limit) else {
            return Ok(std::ptr::null_mut());
        };
        // SAFETY: a block of the bytes' length (at least one byte), filled from them; `length` is writable.
        unsafe {
            let block = libc::malloc(bytes.len().max(1)).cast::<u8>();
            if block.is_null() {
                return Err("system cache read: no memory".into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), block, bytes.len());
            *length = bytes.len();
            Ok(block)
        }
    })
}

/// Count what the caller made of `bytes` bytes `nmb_system_cache_read` gave: accepted, or refused by its own check.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_system_cache_verdict(accepted: bool, bytes: u64) {
    if let Some(layer) = crate::system_cache::system_layer() {
        if accepted {
            layer.accepted(bytes);
        } else {
            layer.rejected();
        }
    }
}

/// Free the runtime's spare dedicated blocks and kept empty pooled blocks (bridge patch 0021), as Metal gives its resource cache's
/// memory back when the system reports memory pressure: how many blocks went.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_release_kept_memory(device: *const NmbDevice) -> usize {
    guard(0, || {
        get(device)?;
        Err("runtime-v2 frees idle allocations on its own 250 ms clock; no synchronous trim API".into())
    })
}

/// Discard the device's resources that are Volatile or Empty and idle (NativeMetalPurge.rs): `cause` 0 for the system's memory
/// pressure signal, which takes every such resource, 1 for the age timer, which takes those idle for `idle_ms`. The bytes discarded.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_purge(device: *const NmbDevice, cause: u32, idle_ms: u64) -> u64 {
    guard(0, || {
        let cause = match cause {
            0 => Cause::Pressure,
            1 => Cause::Age,
            _ => return Err(format!("purge cause {cause}")),
        };
        Ok(get(device)?.3.sweep(cause, Duration::from_millis(idle_ms)))
    })
}

/// How many of the device's resources a later sweep may still discard: Volatile or Empty with memory, in use or not idle long enough.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_purge_pending(device: *const NmbDevice) -> u64 {
    guard(0, || Ok(get(device)?.3.pending() as u64))
}

/// The age an idle Volatile or Empty resource is discarded at, in milliseconds: the runtime's spare block age.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_purge_age_ms() -> u64 {
    Purge::age().as_millis() as u64
}

/// What the device discarded and placed again, into the caller's sixteen words (`Purge::counts`).
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_purge_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        if out.is_null() {
            return Err("null counts".into());
        }
        // SAFETY: the header's sixteen words.
        get(device)?.3.counts(unsafe { std::slice::from_raw_parts_mut(out, 16) });
        Ok(true)
    })
}

/// A command buffer names the buffer from now until `nmb_buffer_unhold`: it is not discarded meanwhile. Called before the command
/// that names it is encoded. True where the buffer is discarded now: its caller names it without having set it NonVolatile first.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_hold(buffer: *const NmbBuffer) -> bool {
    guard(false, || Ok(get(buffer)?.0.hold()))
}

/// The command buffer that held the buffer completed, or went without a commit.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_unhold(buffer: *const NmbBuffer) {
    guard((), || Ok(get(buffer)?.0.unhold()))
}

/// `nmb_buffer_hold` for a texture.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_hold(texture: *const NmbTexture) -> bool {
    guard(false, || Ok(get(texture)?.0.hold()))
}

/// `nmb_buffer_unhold` for a texture.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_unhold(texture: *const NmbTexture) {
    guard((), || Ok(get(texture)?.0.unhold()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_hold(heap: *const NmbHeap) -> bool {
    guard(false, || Ok(get(heap)?.0.hold()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_unhold(heap: *const NmbHeap) {
    guard((), || Ok(get(heap)?.0.unhold()))
}

/// The call stacks that allocated device memory of 4 MiB or more (bridge patch 0020), most bytes first, into the caller's `max` records
/// of twelve words: bytes, allocations, then ten return addresses (0 past the stack's end). Returns the records written.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_allocation_sites(out: *mut u64, max: usize) -> usize {
    guard(0, || {
        if out.is_null() || max == 0 { return Err("empty allocation-site output".into()); }
        Err("runtime-v1 address-stack schema is unavailable; runtime-v2 exposes allocation categories in its metrics".into())
    })
}

/// Separate artifact and driver-record counters, mirroring the C ABI.
#[repr(C)]
pub struct NmbShaderCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub rejected: u64,
    pub read_failures: u64,
    pub writes: u64,
    pub write_failures: u64,
    pub evictions: u64,
    pub driver_loads: u64,
    pub driver_hits: u64,
    pub driver_misses: u64,
    pub driver_saves: u64,
    pub driver_truncated: u64,
    pub driver_mismatched: u64,
    pub driver_unchanged: u64,
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_shader_cache_stats(device: *const NmbDevice, out: *mut NmbShaderCacheStats) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// The last warning the device's sink received, as a C string; returns its length.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_last_warning(out: *mut c_char, size: usize) -> usize {
    guard(0, || Ok(copy_out(&LAST_WARNING.lock().map(|s| s.clone()).unwrap_or_default(), out, size)))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_release(device: *mut NmbDevice) {
    release(device)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_name(device: *const NmbDevice, out: *mut c_char, size: usize) -> usize {
    guard(0, || {
        let raw = get(device)?.0.raw();
        // SAFETY: the runtime owns the live instance and physical device throughout this call.
        let properties = unsafe { raw.instance.get_physical_device_properties(raw.physical_device) };
        let name = properties.device_name_as_c_str().map_err(e("Vulkan device name"))?.to_string_lossy();
        Ok(copy_out(&name, out, size))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_max_buffer_length(device: *const NmbDevice) -> u64 {
    guard(0, || Ok(get(device)?.0.translator().metal_properties().max_buffer_length.min(metal2vulkan_runtime::MAX_BUFFER_LENGTH)))
}

/// `maxThreadsPerThreadgroup` into the caller's three words.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_max_threads_per_threadgroup(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        if out.is_null() {
            return Err("null size".into());
        }
        let s = get(device)?.0.translator().metal_properties().max_threads_per_threadgroup;
        // SAFETY: the header's three words.
        unsafe { std::slice::from_raw_parts_mut(out, 3) }.copy_from_slice(&s.map(u64::from));
        Ok(true)
    })
}

/// The device selectors `nmb_device_answer` takes beyond Apple's private counts and flags: the selector and its kind
/// (the header's NMB_ANSWER_* values).
const PUBLIC_ANSWERS: &[(&str, u32)] = &[
    ("maxThreadgroupMemoryLength", 0),
    ("argumentBuffersSupport", 0),
    ("readWriteTextureSupport", 0),
    ("maxColorAttachments", 0),
    ("minConstantBufferAlignmentBytes", 0),
    ("deviceLinearReadOnlyTextureAlignmentBytes", 0),
    ("areRasterOrderGroupsSupported", 1),
    ("supportsRasterOrderGroups", 1),
    ("isDepth24Stencil8PixelFormatSupported", 1),
    ("areProgrammableSamplePositionsSupported", 1),
    ("supports32BitFloatFiltering", 1),
    ("supportsMemorylessRenderTargets", 1),
    ("supportsSamplerReductionMode", 1),
    ("supportsYCBCRFormats12", 1),
    ("supportsYCBCRPackedFormats12", 1),
    ("supportsSIMDGroup", 1),
    ("minimumLinearTextureAlignmentForPixelFormat:", 2),
    ("supportsTextureSampleCount:", 3),
    ("supportsVertexAmplificationCount:", 3),
    ("supportsRasterizationRateMapWithLayerCount:", 3),
    ("supportsFamily:", 3),
    ("supportsPrimitiveType:", 3),
    ("supportsTextureWriteRoundingMode:", 3),
];

/// Every selector `nmb_device_answer` takes, with its kind, as C strings.
fn answer_selectors() -> &'static [(std::ffi::CString, u32)] {
    static ALL: std::sync::OnceLock<Vec<(std::ffi::CString, u32)>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        crate::capabilities::PRIVATE_COUNTS.iter().map(|s| (*s, 0))
            .chain(crate::capabilities::PRIVATE_FLAGS.iter().map(|s| (*s, 1)))
            .chain(PUBLIC_ANSWERS.iter().copied())
            .map(|(s, kind)| (std::ffi::CString::new(s).expect("selector"), kind)).collect()
    })
}

/// The selector at `index` of the ones `nmb_device_answer` takes and its kind; null past the last.
///
/// # Safety
/// `kind` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_device_answer_selector(index: usize, kind: *mut u32) -> *const c_char {
    let Some((selector, k)) = answer_selectors().get(index) else { return std::ptr::null() };
    // SAFETY: the caller's contract.
    if let Some(kind) = unsafe { kind.as_mut() } {
        *kind = *k;
    }
    selector.as_ptr()
}

/// The rows of Apple's tables the device does not meet in the target families `supportsFamily:` answers true (`TARGET_FAMILIES`), one
/// family per line ("Apple2 4: <row> ..."), into `out` (NUL-terminated); the length the whole text needs.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_family_waivers(device: *const NmbDevice, out: *mut c_char, size: usize) -> usize {
    guard(0, || {
        let text = format!("runtime-v2 unsupported features: {:#?}", get(device)?.0.translator().metal_properties().unsupported_features);
        Ok(copy_out(&text, out, size))
    })
}

fn device_answer(device: &NmbDevice, selector: &str, arg: u64) -> Result<u64, String> {
    crate::capabilities::answer(&device.0, selector, arg)
}

/// The runtime's answer on the device to `selector` (one `nmb_device_answer_selector` lists), with `arg` for a selector that takes one.
/// False, with the reason, where the device has no answer of its own.
///
/// # Safety
/// `selector` is a C string; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_device_answer(device: *const NmbDevice, selector: *const c_char, arg: u64, out: *mut u64) -> bool {
    guard(false, || {
        if selector.is_null() || out.is_null() {
            return Err("null selector or answer".into());
        }
        // SAFETY: the caller's contract.
        let selector = unsafe { CStr::from_ptr(selector) }.to_str().map_err(e("selector"))?;
        let answer = device_answer(get(device)?, selector, arg)?;
        // SAFETY: as above.
        unsafe { *out = answer };
        Ok(true)
    })
}

/// `requiredLinearTextureBytesPerRowForDescriptor:` (Apple SPI): a row of `width` texels of `format` rounded up to the format's
/// linear alignment, from the runtime's format description; 0, with the reason, for a format no buffer can back.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_linear_bytes_per_row(device: *const NmbDevice, format: u64, width: u64) -> u64 {
    guard(0, || {
        let d = &get(device)?.0;
        let alignment = d
            .minimum_linear_texture_alignment(raw::<PixelFormat>(format, "pixel format")?)
            .map_err(e("linear texture"))?;
        let info = d.pixel_format_info(format);
        // Flags bit 0x400: a format of blocks, which the alignment query has refused already.
        if info.flags & 0x400 != 0 || info.words[0] == 0 {
            return Err(format!("pixel format {format} has no texel size"));
        }
        Ok((width * info.words[0]).div_ceil(alignment) * alignment)
    })
}

/// `vendorName`: the Vulkan device's PCI vendor, named where the ID is a known GPU vendor's, else the ID itself.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_vendor_name(device: *const NmbDevice, out: *mut c_char, size: usize) -> usize {
    guard(0, || {
        let raw = get(device)?.0.raw();
        // SAFETY: the runtime owns both handles.
        let id = unsafe { raw.instance.get_physical_device_properties(raw.physical_device) }.vendor_id;
        let name = match id {
            0x5143 => "Qualcomm".to_string(),
            0x106b => "Apple".to_string(),
            0x10de => "NVIDIA".to_string(),
            0x1002 => "AMD".to_string(),
            0x8086 => "Intel".to_string(),
            0x13b5 => "ARM".to_string(),
            _ => format!("0x{id:04x}"),
        };
        Ok(copy_out(&name, out, size))
    })
}

// ---- buffers ----

/// A buffer: the runtime's, behind what keeps its purgeable state (NativeMetalPurge.rs), and whether it has a CPU pointer.
pub struct NmbBuffer(Resource<Buffer>, bool);

impl NmbBuffer {
    fn of(buffer: Resource<Buffer>) -> Result<NmbBuffer, String> {
        let cpu = buffer.live()?.contents().is_some();
        Ok(NmbBuffer(buffer, cpu))
    }
}

/// # Safety
/// `bytes` is null or holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_buffer_new(device: *mut NmbDevice, length: u64, options_word: u64, bytes: *const c_void) -> *mut NmbBuffer {
    guard(std::ptr::null_mut(), || {
        let device = get(device)?;
        let options = options(options_word)?;
        let buffer = if bytes.is_null() {
            Buffer::allocate(&device.0, length, options, None)
        } else {
            // SAFETY: the C caller provides the address. The VM API validates it while copying.
            unsafe { Buffer::allocate_from_host(&device.0, length, options, bytes.cast()) }
        };
        Ok(boxed(NmbBuffer::of(Resource::kept(&device.3, buffer.map_err(e("buffer"))?, (length, options)))?))
    })
}

/// # Safety
/// `pointer` is page aligned, holds `length` bytes and stays mapped until the buffer is released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_buffer_new_no_copy(device: *mut NmbDevice, pointer: *mut c_void, length: u64, options_word: u64)
                                                -> *mut NmbBuffer {
    // SAFETY: the caller's contract.
    unsafe { no_copy(device, pointer, length, options_word, None) }
}

/// The C side of a no-copy deallocator: `release(context, pointer, length)`.
pub type NmbRelease = unsafe extern "C" fn(*mut c_void, *mut c_void, u64);

struct ReleaseContext(*mut c_void);
// SAFETY: the caller hands the context over for a call on any thread (Metal's deallocator contract).
unsafe impl Send for ReleaseContext {}

/// `nmb_buffer_new_no_copy` with Metal's deallocator: `release` runs once, on any thread, when the runtime frees its import of the pages
/// (the last buffer over them and every command buffer using them are gone), so the pages stay pinned until then and are never unmapped
/// under the GPU. A refused call never runs it: the caller still owns `context`.
///
/// # Safety
/// `pointer` is page aligned, holds `length` bytes and stays mapped until `release` runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_buffer_new_no_copy_releasing(device: *mut NmbDevice, pointer: *mut c_void, length: u64, options_word: u64,
                                                          release: Option<NmbRelease>, context: *mut c_void) -> *mut NmbBuffer {
    let deallocator = release.map(|release| {
        let context = ReleaseContext(context);
        Box::new(move |pointer: NonNull<u8>, length: u64| {
            let context = context;
            // SAFETY: the caller's contract.
            unsafe { release(context.0, pointer.as_ptr() as *mut c_void, length) }
        }) as Deallocator
    });
    // SAFETY: the caller's contract.
    unsafe { no_copy(device, pointer, length, options_word, deallocator) }
}

/// The pages each no-copy range had when a buffer was last made over exactly it (`pages_identity`), under the device's no-copy gate.
/// The runtime puts a no-copy buffer over exactly the bytes a live one was made over in that one's import, so over the pages that
/// were there then (`host_aliases.rs`); Metal wires the pages at the address when it makes each buffer. CoreAnimation's render
/// server maps a client's new shared memory (and IOSurfaces) where an earlier one was while its no-copy buffers still live in its
/// caches: the GPU drew the earlier pages, another text mask in a label's place (GFXBench's test list; Metal Check case 40).
#[derive(Default)]
pub struct NoCopyPages {
    seen: std::collections::HashMap<(u64, u64), u64>,
    /// Whether `seen` was ever emptied to bound it: a range without a note may then have a live buffer over it.
    forgot: bool,
}

const NO_COPY_RANGES_NOTED: usize = 8192;
/// Ranges checked, and ranges whose pages had changed (reported to the runtime as remapped).
static NO_COPY_REMAPS: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// What the pages under `length` bytes at `start` are, as the kernel names them: for each map entry over the range, the leaf VM
/// object's (hashed) id and the object offset at the range's first byte in it. A remap puts other objects there, and the object of
/// pages a live import pinned outlives the remap (the import holds it), so no new object takes its id while it lives. `None` where
/// a page has no object yet (memory never touched, so not what a live import pinned) or the range is not all mapped.
#[cfg(target_vendor = "apple")]
fn pages_identity(start: u64, length: u64) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    // vm_region_submap_short_info_64 (mach/vm_region.h, packed to 4).
    #[repr(C, packed(4))]
    #[derive(Default)]
    struct Region {
        protection: i32,
        max_protection: i32,
        inheritance: u32,
        offset: u64,
        user_tag: u32,
        ref_count: u32,
        shadow_depth: u16,
        external_pager: u8,
        share_mode: u8,
        is_submap: i32,
        behavior: i32,
        object_id: u32,
        user_wired_count: u16,
    }
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn mach_vm_region_recurse(task: u32, address: *mut u64, size: *mut u64, depth: *mut u32, info: *mut i32, count: *mut u32) -> i32;
    }
    let end = start.checked_add(length)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut at = start;
    while at < end {
        let (mut address, mut size, mut depth) = (at, 0u64, 64u32);
        let mut region = Region::default();
        let mut count = (std::mem::size_of::<Region>() / 4) as u32;
        // SAFETY: the out-parameters are this frame's, `count` words of `region`; the call reads no memory of the range.
        let status = unsafe {
            mach_vm_region_recurse(mach_task_self_, &mut address, &mut size, &mut depth, (&raw mut region).cast(), &mut count)
        };
        let (object, offset) = (region.object_id, region.offset);
        if status != 0 || address > at || object == 0 {
            return None;
        }
        (at, object, offset + (at - address)).hash(&mut hash);
        at = address.checked_add(size)?;
    }
    Some(hash.finish())
}

#[cfg(not(target_vendor = "apple"))]
fn pages_identity(_: u64, _: u64) -> Option<u64> {
    None
}

/// Ranges checked and ranges found remapped since the device opened (`NoCopyPages`).
#[unsafe(no_mangle)]
pub extern "C" fn nmb_no_copy_remaps(out: *mut u64) -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    if out.is_null() {
        return false;
    }
    // SAFETY: the header's two words.
    let out = unsafe { std::slice::from_raw_parts_mut(out, 2) };
    out[0] = NO_COPY_REMAPS[0].load(Relaxed);
    out[1] = NO_COPY_REMAPS[1].load(Relaxed);
    true
}

unsafe fn no_copy(device: *mut NmbDevice, pointer: *mut c_void, length: u64, options_word: u64, deallocator: Option<Deallocator>)
                  -> *mut NmbBuffer {
    guard(std::ptr::null_mut(), || {
        let device = get(device)?;
        // SAFETY: the caller's contract.
        let base = NonNull::new(pointer as *mut u8).ok_or("null memory")?;
        let modes = options(options_word)?;
        // The runtime's live-import lookup and subsequent join are separate:
        // simultaneous first constructors can allocate distinct imports of the
        // same pages. Cover every native ABI no-copy call, including mixtures
        // of raw buffers and IOSurface textures. Each call still gets its own
        // buffer object. This ABI passes no dma-buf source; a deallocator runs
        // only when an import is freed, never under this gate. Drop it before
        // returning/formatting a failure; GPU work and resource release never
        // take it. A device's unrelated APIs and other devices remain separate.
        // The gate also holds each range's pages when a buffer was last made over it: pages other than those now (a remap) are
        // reported to the runtime first, so the buffer imports the pages at the address instead of joining a live import of the
        // earlier ones (`NoCopyPages`).
        let buffer = {
            use std::sync::atomic::Ordering::Relaxed;
            let mut gate = device.2.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let range = (pointer as u64, length);
            let now = pages_identity(range.0, range.1);
            let changed = match gate.seen.get(&range) {
                Some(&noted) => now != Some(noted),
                None => gate.forgot || now.is_none(),
            };
            NO_COPY_REMAPS[0].fetch_add(1, Relaxed);
            if changed {
                NO_COPY_REMAPS[1].fetch_add(1, Relaxed);
            }
            // SAFETY: as above.
            let buffer = unsafe { Buffer::import(&device.0, base, length, modes, deallocator) };
            // Noted as they are once the buffer pinned them (pinning for writing may give the range objects of its own).
            match (&buffer, pages_identity(range.0, range.1)) {
                (Ok(_), Some(pages)) => {
                    if gate.seen.len() >= NO_COPY_RANGES_NOTED && !gate.seen.contains_key(&range) {
                        gate.seen.clear();
                        gate.forgot = true;
                    }
                    gate.seen.insert(range, pages);
                }
                _ => {
                    gate.seen.remove(&range);
                }
            }
            buffer
        };
        Ok(boxed(NmbBuffer::of(Resource::fixed(buffer.map_err(e("no-copy buffer"))?))?))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_release(buffer: *mut NmbBuffer) {
    release(buffer)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_contents(buffer: *const NmbBuffer) -> *mut c_void {
    guard(std::ptr::null_mut(), || {
        let buffer = get(buffer)?;
        // The pointer is the buffer's for its life, as Metal's is: one handed out keeps the buffer's memory where it is.
        if buffer.1 {
            buffer.0.pin();
        }
        Ok(buffer.0.live()?.contents().map_or(std::ptr::null_mut(), |p| p.as_ptr() as *mut c_void))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_length(buffer: *const NmbBuffer) -> u64 {
    guard(0, || Ok(get(buffer)?.0.bytes()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_gpu_address(buffer: *const NmbBuffer) -> u64 {
    guard(0, || {
        let buffer = &get(buffer)?.0;
        // The address is the buffer's for its life too.
        buffer.pin();
        Ok(buffer.live()?.gpu_address())
    })
}

// ---- heaps ----

pub struct NmbHeap(Resource<Heap>);

/// `newHeapWithDescriptor:`: type (0 automatic, 1 placement), size, resource options. Sparse heaps are not made here.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_new(device: *mut NmbDevice, heap_type: u64, size: u64, options_word: u64) -> *mut NmbHeap {
    guard(std::ptr::null_mut(), || {
        let device = get(device)?;
        let heap_type = HeapType::from_raw(heap_type).map_err(e("heap type"))?;
        if heap_type == HeapType::Sparse {
            return Err("sparse heaps are not implemented".into());
        }
        let options = options(options_word)?;
        let heap = Heap::new(&device.0, &HeapDescriptor::new(heap_type, size, options)).map_err(e("heap"))?;
        Ok(boxed(NmbHeap(Resource::kept(&device.3, heap, (heap_type, size, options)))))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_release(heap: *mut NmbHeap) {
    release(heap)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_size(heap: *const NmbHeap) -> u64 {
    guard(0, || Ok(get(heap)?.0.bytes()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_used_size(heap: *const NmbHeap) -> u64 {
    guard(0, || Ok(get(heap)?.0.peek().map_or(0, |heap| heap.used_size())))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_allocated_size(heap: *const NmbHeap) -> u64 {
    guard(0, || Ok(get(heap)?.0.bytes()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_max_available_size(heap: *const NmbHeap, alignment: u64) -> u64 {
    guard(0, || {
        let heap = &get(heap)?.0;
        if let Some(heap) = heap.peek() {
            return heap.max_available_size(alignment).map_err(e("heap available size"));
        }
        // A discarded heap holds nothing (no resource placed in it lives): an empty heap's answer, placing nothing.
        if alignment != 0 && !alignment.is_power_of_two() {
            return Err(format!("heap available size: alignment {alignment} is not a power of two"));
        }
        Ok(match heap.recipe() {
            Some((HeapType::Automatic, ..)) => heap.bytes(),
            _ => 0,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_set_purgeable_state(heap: *const NmbHeap, state: u64) -> u64 {
    guard(0, || {
        let state = PurgeableState::from_raw(state).map_err(e("purgeable state"))?;
        get(heap)?.0.set_state(state)
    })
}

/// A buffer in the heap: automatic heaps place it, a placement heap takes `offset` (`u64::MAX` asks for automatic placement).
#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_new_buffer(heap: *const NmbHeap, length: u64, options_word: u64, offset: u64) -> *mut NmbBuffer {
    guard(std::ptr::null_mut(), || {
        // The tie comes first: a heap that holds a resource is not discarded.
        let (tie, heap) = (get(heap)?.0.tie(), get(heap)?.0.live()?);
        let options = options(options_word)?;
        let buffer = if offset == u64::MAX { heap.new_buffer(length, options) } else { heap.new_buffer_at_offset(length, options, offset) };
        Ok(boxed(NmbBuffer::of(Resource::tied(buffer.map_err(e("heap buffer"))?, tie))?))
    })
}

/// A texture in the heap, placed as `nmb_heap_new_buffer` places a buffer.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_heap_new_texture(heap: *const NmbHeap, descriptor: *const NmbTextureDescriptor, offset: u64) -> *mut NmbTexture {
    guard(std::ptr::null_mut(), || {
        let (tie, heap) = (get(heap)?.0.tie(), get(heap)?.0.live()?);
        let descriptor = texture_descriptor(descriptor)?;
        let texture = if offset == u64::MAX { heap.new_texture(&descriptor) } else { heap.new_texture_at_offset(&descriptor, offset) };
        Ok(boxed(NmbTexture(Resource::tied(texture.map_err(e("heap texture"))?, tie))))
    })
}

/// `heapTextureSizeAndAlignWithDescriptor:`: the bytes the texture takes of a heap and the alignment of its offset, into `out[0]` and
/// `out[1]`; false, with the reason, for a descriptor the runtime refuses.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_heap_texture_size_and_align(device: *const NmbDevice, descriptor: *const NmbTextureDescriptor, out: *mut u64) -> bool {
    guard(false, || {
        let answer = get(device)?.0.heap_texture_size_and_align(&texture_descriptor(descriptor)?.runtime()?).map_err(e("heap texture size"))?;
        let out = unsafe { std::slice::from_raw_parts_mut(out, 2) };
        (out[0], out[1]) = (answer.size, answer.align);
        Ok(true)
    })
}

/// `heapBufferSizeAndAlignWithLength:options:`, as `nmb_device_heap_texture_size_and_align` answers a texture.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_heap_buffer_size_and_align(device: *const NmbDevice, length: u64, options_word: u64, out: *mut u64) -> bool {
    guard(false, || {
        let answer = get(device)?.0.heap_buffer_size_and_align(length, options(options_word)?.raw());
        let out = unsafe { std::slice::from_raw_parts_mut(out, 2) };
        (out[0], out[1]) = (answer.size, answer.align);
        Ok(true)
    })
}

// ---- textures ----

#[repr(C)]
pub struct NmbTextureDescriptor {
    pub kind: u64,
    pub format: u64,
    pub width: u64,
    pub height: u64,
    pub depth: u64,
    pub levels: u64,
    pub samples: u64,
    pub array_length: u64,
    pub usage: u64,
    pub options: u64,
    pub rotation: u64,
    pub framebuffer_only: u64,
    pub is_drawable: u64,
    pub swizzle: u64,
    pub allow_gpu_optimized_contents: u64,
}

// Bit 32 distinguishes an explicit all-zero swizzle from an omitted descriptor field.
fn swizzle_of(packed: u64) -> Result<Option<TextureSwizzleChannels>, String> {
    if packed == 0 { return Ok(None); }
    let channel = |shift: u32| raw::<TextureSwizzle>((packed >> shift) & 0xff, "texture swizzle");
    Ok(Some(TextureSwizzleChannels { red: channel(0)?, green: channel(8)?, blue: channel(16)?, alpha: channel(24)? }))
}
fn swizzle_packed(s: TextureSwizzleChannels) -> u64 {
    (1u64 << 32) | s.red.as_raw() as u64 | (s.green.as_raw() as u64) << 8 | (s.blue.as_raw() as u64) << 16 | (s.alpha.as_raw() as u64) << 24
}
pub struct NmbTexture(Resource<Texture>);

fn texture_descriptor(d: *const NmbTextureDescriptor) -> Result<TextureDescriptor, String> {
    let d = get(d)?;
    let mut out = TextureDescriptor::default();
    out.texture_type = raw::<TextureType>(d.kind, "texture type")?;
    out.pixel_format = raw::<PixelFormat>(d.format, "pixel format")?;
    out.width = d.width;
    out.height = d.height;
    out.depth = d.depth;
    out.mipmap_level_count = d.levels;
    out.sample_count = d.samples;
    out.array_length = d.array_length;
    out.usage = raw::<TextureUsage>(d.usage, "texture usage")?;
    out.resource_options = options(d.options)?;
    out.rotation = d.rotation;
    out.framebuffer_only = d.framebuffer_only != 0;
    out.is_drawable = d.is_drawable != 0;
    out.allow_gpu_optimized_contents = d.allow_gpu_optimized_contents != 0;
    if let Some(swizzle) = swizzle_of(d.swizzle)? { out.swizzle = swizzle; }
    Ok(out)
}

#[cfg(test)]
mod texture_descriptor_tests {
    use super::*;

    #[test]
    fn heap_layout_preserves_the_callers_optimization_policy() {
        let mut descriptor = NmbTextureDescriptor {
            kind: TextureType::Type2D.as_raw() as u64,
            format: PixelFormat::RGBA16Float.as_raw() as u64,
            width: 242, height: 242, depth: 1, levels: 1, samples: 1, array_length: 1,
            usage: TextureUsage::SHADER_READ.as_raw() as u64,
            options: ResourceOptions::private().raw(),
            rotation: 0, framebuffer_only: 0, is_drawable: 0, swizzle: 0,
            allow_gpu_optimized_contents: 0,
        };
        assert!(!texture_descriptor(&descriptor).unwrap().allow_gpu_optimized_contents);
        descriptor.allow_gpu_optimized_contents = 1;
        assert!(texture_descriptor(&descriptor).unwrap().allow_gpu_optimized_contents);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_new(device: *mut NmbDevice, descriptor: *const NmbTextureDescriptor) -> *mut NmbTexture {
    guard(std::ptr::null_mut(), || {
        let descriptor = texture_descriptor(descriptor)?;
        let device = get(device)?;
        let texture = Texture::allocate(&device.0, &descriptor)?;
        // A drawable's texture is the display's: never discarded.
        Ok(boxed(NmbTexture(match descriptor.is_drawable {
            true => Resource::fixed(texture),
            false => Resource::kept(&device.3, texture, descriptor),
        })))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_new_on_buffer(buffer: *mut NmbBuffer, descriptor: *const NmbTextureDescriptor, offset: u64,
                                            bytes_per_row: u64, iosurface: bool) -> *mut NmbTexture {
    guard(std::ptr::null_mut(), || {
        let descriptor = texture_descriptor(descriptor)?;
        let (tie, buffer) = (get(buffer)?.0.tie(), get(buffer)?.0.live()?);
        // IOSurface pages are imported once by the frontend. Runtime v2's ordinary
        // buffer texture handles the image-in-place or synchronized-copy form.
        let _ = iosurface;
        let texture = buffer.new_texture(&descriptor, offset, bytes_per_row)?;
        Ok(boxed(NmbTexture(Resource::tied(texture, tie))))
    })
}

/// `newTextureWithDescriptor:iosurface:plane:` over a surface with planes: `offset` and `bytes_per_row` are the asked plane's, and a
/// two-plane surface also gives its OSType, the asked plane, its size and plane 1's offset and rows (`chroma`, 0 and 0 for one plane),
/// which a two-plane texture reads its second plane from.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_new_on_surface(buffer: *mut NmbBuffer, descriptor: *const NmbTextureDescriptor, offset: u64,
                                             bytes_per_row: u64, pixel_format: u32, plane: u64, width: u64, height: u64,
                                             chroma_offset: u64, chroma_bytes_per_row: u64) -> *mut NmbTexture {
    guard(std::ptr::null_mut(), || {
        let descriptor = texture_descriptor(descriptor)?;
        if metal2vulkan::metal::format::ycbcr(descriptor.pixel_format) {
            return Err(format!("upstream runtime-v2 has no YCbCr surface conversion: format={pixel_format:#x} plane={plane} surface={width}x{height} chroma={chroma_offset}/{chroma_bytes_per_row}"));
        }
        let (tie, buffer) = (get(buffer)?.0.tie(), get(buffer)?.0.live()?);
        let texture = buffer.new_texture(&descriptor, offset, bytes_per_row)?;
        Ok(boxed(NmbTexture(Resource::tied(texture, tie))))
    })
}

/// The process's two-plane YCbCr surface textures, into the caller's fourteen words: made over their planes alone, made with their
/// converted image, images made later for a texture over its planes at a slot binding that needed one, textures over their planes
/// that another call made whole, the converted images alive now and the bytes they and their staging copies hold; then the
/// textures whose planes a function reads in place, and those that keep their image alone by the reason: the device, the layout
/// (another format, plane 1), a swizzle, the planes' offset, their element count, planes past the surface, no texel buffer view.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_ycbcr_surface_counts(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        get(device)?;
        if out.is_null() { return Err("null legacy statistics output".into()); }
        Err("runtime-v1 statistics schema is unavailable; use runtime-v2 memory and metrics snapshots".into())
    })
}

/// `newLinearTextureWithDescriptor:offset:bytesPerRow:bytesPerImage:` (Apple SPI).
#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_new_linear(buffer: *mut NmbBuffer, descriptor: *const NmbTextureDescriptor, offset: u64, bytes_per_row: u64,
                                         bytes_per_image: u64) -> *mut NmbTexture {
    guard(std::ptr::null_mut(), || {
        let descriptor = texture_descriptor(descriptor)?;
        if descriptor.depth > 1 && bytes_per_image != bytes_per_row.checked_mul(descriptor.height).ok_or("linear image pitch overflow")? {
            return Err("upstream runtime-v2 has no padded 3D texture-over-buffer layout".into());
        }
        let (tie, buffer) = (get(buffer)?.0.tie(), get(buffer)?.0.live()?);
        let texture = buffer.new_texture(&descriptor, offset, bytes_per_row)?;
        Ok(boxed(NmbTexture(Resource::tied(texture, tie))))
    })
}

/// A view of `texture`: of the whole texture as `format` where `whole`, else of `levels` and `slices` (start, count) as `format` and `kind`.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_new_view(texture: *mut NmbTexture, format: u64, kind: u64, levels: *const u64, slices: *const u64, whole: bool)
                                       -> *mut NmbTexture {
    guard(std::ptr::null_mut(), || {
        // The tie comes first: a texture with a view of it is not discarded, since the view shares its memory.
        let (tie, t) = (get(texture)?.0.tie(), get(texture)?.0.live()?);
        let format = raw::<PixelFormat>(format, "pixel format")?;
        let view = if whole {
            t.new_texture_view(format)
        } else {
            if levels.is_null() || slices.is_null() {
                return Err("null levels or slices".into());
            }
            // SAFETY: the header's two words each.
            let (l, s) = unsafe { (std::slice::from_raw_parts(levels, 2), std::slice::from_raw_parts(slices, 2)) };
            let ranges = (l[0]..l[0].checked_add(l[1]).ok_or("levels overflow")?, s[0]..s[0].checked_add(s[1]).ok_or("slices overflow")?);
            t.new_texture_view_with_levels_slices(format, raw::<TextureType>(kind, "texture type")?, ranges.0, ranges.1)
        };
        Ok(boxed(NmbTexture(Resource::tied(view.map_err(e("texture view"))?, tie))))
    })
}

/// `setPurgeableState:` on a texture: the answer as Metal's raw state, 0 where it fails. Bit 8 is set where the texture's contents
/// were discarded and placing it again failed: the answer's low byte is still Empty, and the reason is the last error.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_set_purgeable_state(texture: *const NmbTexture, state: u64) -> u64 {
    guard(0, || {
        let state = PurgeableState::from_raw(state).map_err(e("purgeable state"))?;
        get(texture)?.0.set_state(state)
    })
}

/// `setPurgeableState:` on a buffer, as on a texture.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_set_purgeable_state(buffer: *const NmbBuffer, state: u64) -> u64 {
    guard(0, || {
        let state = PurgeableState::from_raw(state).map_err(e("purgeable state"))?;
        get(buffer)?.0.set_state(state)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_release(texture: *mut NmbTexture) {
    release(texture)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_describe(texture: *const NmbTexture, out: *mut NmbTextureDescriptor) {
    guard((), || {
        let t = get(texture)?.0.slot()?;
        let out = get_mut(out)?;
        *out = NmbTextureDescriptor {
            kind: t.texture_type().as_raw() as u64,
            format: t.pixel_format().as_raw() as u64,
            width: t.width(),
            height: t.height(),
            depth: t.depth(),
            levels: t.mipmap_level_count(),
            samples: t.sample_count(),
            array_length: t.array_length(),
            usage: t.usage().as_raw() as u64,
            options: t.resource_options().raw(),
            rotation: t.rotation(),
            framebuffer_only: u64::from(t.framebuffer_only()),
            is_drawable: u64::from(t.is_drawable()),
            swizzle: swizzle_packed(t.swizzle()),
            allow_gpu_optimized_contents: u64::from(t.allow_gpu_optimized_contents()),
        };
        Ok(())
    })
}

fn region(r: *const u64) -> Result<Region, String> {
    if r.is_null() {
        return Err("null region".into());
    }
    // SAFETY: the header's six words.
    let r = unsafe { std::slice::from_raw_parts(r, 6) };
    Ok(Region { origin: [narrow(r[0])?, narrow(r[1])?, narrow(r[2])?], size: [narrow(r[3])?, narrow(r[4])?, narrow(r[5])?] })
}

fn triple(v: *const u64) -> Result<[u32; 3], String> {
    if v.is_null() {
        return Err("null origin or size".into());
    }
    // SAFETY: the header's three words.
    let v = unsafe { std::slice::from_raw_parts(v, 3) };
    Ok([narrow(v[0])?, narrow(v[1])?, narrow(v[2])?])
}

/// # Safety
/// `bytes` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_texture_replace(texture: *mut NmbTexture, region_words: *const u64, level: u64, slice: u64,
                                             bytes: *const c_void, bytes_per_row: u64, bytes_per_image: u64, length: u64) -> bool {
    guard(false, || {
        if bytes.is_null() {
            return Err("null bytes".into());
        }
        // SAFETY: the caller's contract.
        let bytes = unsafe { metal2vulkan_runtime::HostBytes::from_raw(bytes.cast(), usize::try_from(length).map_err(e("texture byte count"))?) };
        get(texture)?.0.live()?.replace_region(region(region_words)?, narrow(level)?, narrow(slice)?, bytes, bytes_per_row, bytes_per_image)
            .map_err(e("replace region"))?;
        Ok(true)
    })
}

/// # Safety
/// `bytes` holds `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_texture_get_bytes(texture: *const NmbTexture, bytes: *mut c_void, bytes_per_row: u64, bytes_per_image: u64,
                                               length: u64, region_words: *const u64, level: u64, slice: u64) -> bool {
    guard(false, || {
        if bytes.is_null() {
            return Err("null bytes".into());
        }
        // SAFETY: the caller's contract.
        let bytes = unsafe { std::slice::from_raw_parts_mut(bytes as *mut u8, length as usize) };
        get(texture)?.0.live()?.get_bytes(bytes, bytes_per_row, bytes_per_image, region(region_words)?, narrow(level)?, narrow(slice)?)
            .map_err(e("get bytes"))?;
        Ok(true)
    })
}

// ---- libraries and functions ----

pub struct NmbLibrary(Library, Vec<String>);
pub struct NmbFunction(Function);

/// # Safety
/// `bytes` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_library_new(device: *mut NmbDevice, bytes: *const c_void, length: usize, air: bool) -> *mut NmbLibrary {
    guard(std::ptr::null_mut(), || {
        get(device)?;
        if bytes.is_null() || length == 0 {
            return Err("no library bytes".into());
        }
        // SAFETY: the caller's contract.
        let bytes = unsafe { std::slice::from_raw_parts(bytes as *const u8, length) };
        let library = if air {
            Library::from_air(bytes).map_err(e("AIR"))?
        } else {
            Library::from_metallib(crate::air_slice(bytes).ok_or("metallib without an AIR64 slice")?).map_err(e("metallib"))?
        };
        let names = library.function_names();
        Ok(boxed(NmbLibrary(library, names)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_library_release(library: *mut NmbLibrary) {
    release(library)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_library_function_count(library: *const NmbLibrary) -> u32 {
    guard(0, || Ok(get(library)?.1.len() as u32))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_library_function_name(library: *const NmbLibrary, index: u32, out: *mut c_char, size: usize) -> usize {
    guard(0, || {
        let names = &get(library)?.1;
        Ok(copy_out(names.get(index as usize).ok_or("function index")?, out, size))
    })
}

/// # Safety
/// `name` is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_new(library: *mut NmbLibrary, name: *const c_char) -> *mut NmbFunction {
    guard(std::ptr::null_mut(), || {
        if name.is_null() {
            return Err("null function name".into());
        }
        // SAFETY: the caller's contract.
        let name = unsafe { CStr::from_ptr(name) }.to_str().map_err(e("function name"))?;
        Ok(boxed(NmbFunction(get(library)?.0.new_function_with_name(name).map_err(e("function"))?)))
    })
}

#[repr(C)]
pub struct NmbConstant {
    pub kind: u64,
    pub index: u64,
    pub name: *const c_char,
    pub bytes: *const c_void,
    pub length: u64,
}

/// # Safety
/// `name` is a C string; `constants` holds `count` entries whose names are C strings (or null) and whose bytes hold `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_new_specialized(library: *mut NmbLibrary, name: *const c_char, constants: *const NmbConstant,
                                                      count: u32) -> *mut NmbFunction {
    // SAFETY: the caller's contract.
    unsafe { specialized(library, name, constants, count, false) }
}

/// The same with every declared constant the caller did not give a value set to zero (false for a bool). Metal accepts a function
/// whose code reads a constant nobody set; the translator needs a value (`NeedsInput(FunctionConstant)`), and zero is what such a
/// read yields there. The frontend retries a pipeline with this variant only after the specialized one was refused for that reason.
///
/// # Safety
/// As for `nmb_function_new_specialized`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_new_specialized_zero_rest(library: *mut NmbLibrary, name: *const c_char,
                                                                constants: *const NmbConstant, count: u32) -> *mut NmbFunction {
    // SAFETY: the caller's contract.
    unsafe { specialized(library, name, constants, count, true) }
}

unsafe fn specialized(library: *mut NmbLibrary, name: *const c_char, constants: *const NmbConstant, count: u32,
                      zero_rest: bool) -> *mut NmbFunction {
    guard(std::ptr::null_mut(), || {
        if name.is_null() || (constants.is_null() && count > 0) {
            return Err("null function name or constants".into());
        }
        // SAFETY: the caller's contract.
        let name = unsafe { CStr::from_ptr(name) }.to_str().map_err(e("function name"))?;
        let constants = if count == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(constants, count as usize) } };
        let mut values = FunctionConstantValues::defaults();
        for c in constants {
            if c.bytes.is_null() {
                return Err("constant without bytes".into());
            }
            // SAFETY: the caller's contract.
            let bytes = unsafe { std::slice::from_raw_parts(c.bytes as *const u8, c.length as usize) };
            let value = FunctionConstantValue::new(raw::<DataType>(c.kind, "constant type")?, bytes.to_vec());
            if c.name.is_null() {
                values.by_index.insert(u32::try_from(c.index).map_err(e("constant index"))?, value);
            } else {
                // SAFETY: the caller's contract.
                let key = unsafe { CStr::from_ptr(c.name) }.to_str().map_err(e("constant name"))?;
                values.by_name.insert(key.to_string(), value);
            }
        }
        if zero_rest {
            let generic = get(library)?.0.new_function_with_name(name).map_err(e("function"))?;
            for (key, constant) in generic.function_constants_dictionary().map_err(e("function constants"))? {
                if values.by_name.contains_key(&key) || values.by_index.contains_key(&constant.index()) {
                    continue;
                }
                let ty = constant.data_type();
                let bytes = constant_bytes(ty.as_raw());
                if bytes == 0 {
                    return Err(format!("function constant {key} of type {}", ty.as_raw()));
                }
                values.by_index.insert(constant.index(), FunctionConstantValue::new(ty, vec![0; bytes]));
            }
        }
        let function = get(library)?.0.new_function_with_name_constant_values(name, &values).map_err(e("specialized function"))?;
        Ok(boxed(NmbFunction(function)))
    })
}

/// Packed bytes of a function constant's `MTLDataType` (scalars and vectors), 0 for a type that is not given a value.
fn constant_bytes(raw: u32) -> usize {
    // MTLDataType numbers each scalar family's 1-, 2-, 3- and 4-lane types consecutively; a value is packed, lanes times the scalar.
    let (first, scalar) = match raw {
        3..=6 => (3, 4),       // float
        16..=19 => (16, 2),    // half
        29..=32 => (29, 4),    // int
        33..=36 => (33, 4),    // uint
        37..=40 => (37, 2),    // short
        41..=44 => (41, 2),    // ushort
        45..=48 => (45, 1),    // char
        49..=52 => (49, 1),    // uchar
        53..=56 => (53, 1),    // bool
        81..=84 => (81, 8),    // long
        85..=88 => (85, 8),    // ulong
        121..=124 => (121, 2), // bfloat
        _ => return 0,
    };
    (raw - first + 1) as usize * scalar
}

/// Specialize every declared constant to zero for shader priming.
///
/// # Safety
/// `name` is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_new_zero_constants(library: *mut NmbLibrary, name: *const c_char) -> *mut NmbFunction {
    // SAFETY: The caller supplies the C string.
    unsafe { specialize_prime_function(library, name, false, u32::MAX) }
}

/// Specialize required constants and leave optional values undefined.
/// `one_index` is `u32::MAX` for all-zero specialization.
///
/// # Safety
/// `name` is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_new_required_constants(library: *mut NmbLibrary, name: *const c_char, one_index: u32) -> *mut NmbFunction {
    // SAFETY: The caller supplies the C string.
    unsafe { specialize_prime_function(library, name, true, one_index) }
}

unsafe fn specialize_prime_function(library: *mut NmbLibrary, name: *const c_char, required_only: bool, one_index: u32) -> *mut NmbFunction {
    guard(std::ptr::null_mut(), || {
        if name.is_null() {
            return Err("null function name".into());
        }
        // SAFETY: the caller's contract.
        let name = unsafe { CStr::from_ptr(name) }.to_str().map_err(e("function name"))?;
        let library = &get(library)?.0;
        let generic = library.new_function_with_name(name).map_err(e("function"))?;
        let mut values = FunctionConstantValues::defaults();
        let mut selected = one_index == u32::MAX;
        for (key, constant) in generic.function_constants_dictionary().map_err(e("function constants"))? {
            if required_only && !constant.is_required() {
                continue;
            }
            let ty = constant.data_type();
            let bytes = constant_bytes(ty.as_raw());
            if bytes == 0 {
                return Err(format!("function constant {key} of type {}", ty.as_raw()));
            }
            let mut value = vec![0; bytes];
            if one_index != u32::MAX && constant.index() == one_index {
                if !required_only || !constant.is_required() ||
                    !matches!(ty.as_raw(), 29 | 33 | 37 | 41 | 45 | 49 | 81 | 85) {
                    return Err("priming may set one only for a required scalar integer".into());
                }
                value[0] = 1;
                selected = true;
            }
            values.by_index.insert(constant.index(), FunctionConstantValue::new(ty, value));
        }
        if !selected {
            return Err("priming constant index was not found".into());
        }
        let function = library.new_function_with_name_constant_values(name, &values).map_err(e("zero-specialized function"))?;
        Ok(boxed(NmbFunction(function)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_release(function: *mut NmbFunction) {
    release(function)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_type(function: *const NmbFunction) -> u64 {
    guard(0, || Ok(get(function)?.0.function_type().map_err(e("function type"))?.as_raw() as u64))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_attribute_count(function: *const NmbFunction) -> i32 {
    guard(-1, || Ok(get(function)?.0.stage_input_attributes().map_err(e("function attributes"))?.map_or(0, |a| a.len() as i32)))
}

/// # Safety
/// `name` has `capacity` writable bytes and `facts` holds five writable words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_attribute_at(function: *const NmbFunction, at: u32, name: *mut c_char,
                                                capacity: usize, facts: *mut u64) -> usize {
    guard(0, || {
        let attributes = get(function)?.0.stage_input_attributes().map_err(e("function attributes"))?.unwrap_or_default();
        let a = attributes.get(at as usize).ok_or("function attribute index")?;
        if facts.is_null() { return Err("null attribute facts".into()); }
        let values = [a.attribute_index() as u64, a.attribute_type().as_raw() as u64, a.is_active() as u64,
                      a.is_patch_data() as u64, a.is_patch_control_point_data() as u64];
        // SAFETY: the caller supplies five writable words.
        unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), facts, values.len()); }
        Ok(copy_out(a.name(), name, capacity))
    })
}

/// `patchType`: 0 (none) for a function other than a post-tessellation vertex function.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_patch_type(function: *const NmbFunction) -> u64 {
    guard(0, || Ok(get(function)?.0.patch_type().map_err(e("patch type"))?.as_raw() as u64))
}

/// `patchControlPointCount`: -1 for no patch, or a patch that states no count.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_patch_control_point_count(function: *const NmbFunction) -> i64 {
    guard(-1, || Ok(get(function)?.0.patch_control_point_count().map_err(e("patch control point count"))?.map_or(-1, i64::from)))
}

// ---- pipelines and states ----

pub struct NmbComputePipeline(ComputePipelineState);
pub struct NmbRenderPipeline(RenderPipelineState);
pub struct NmbSampler(SamplerState, Arc<Ownership>);
pub struct NmbDepthStencil(DepthStencilState);

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_pipeline_new(device: *mut NmbDevice, function: *mut NmbFunction) -> *mut NmbComputePipeline {
    // SAFETY: null options names no array, the existing handles follow this ABI.
    unsafe { nmb_compute_pipeline_new_with_options(device, function, std::ptr::null()) }
}

/// # Safety
/// `options` is null or a valid record with a counted array of live archive handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_compute_pipeline_new_with_options(device: *mut NmbDevice, function: *mut NmbFunction,
                                                              options: *const NmbPipelineOptions) -> *mut NmbComputePipeline {
    guard(std::ptr::null_mut(), || {
        let resolved = unsafe { pipeline_options(options)? };
        let descriptor = ComputePipelineDescriptor::new(StageSource::Function(get(function)?.0.function_descriptor()));
        let device = &get(device)?.0;
        resolved.check_compute(device, &descriptor)?;
        let state = crate::pipelines::compute(device, &descriptor);
        Ok(boxed(NmbComputePipeline(state.map_err(e("compute pipeline"))?)))
    })
}

#[repr(C)]
pub struct NmbComputePipelineDescriptor {
    pub function: *mut NmbFunction,
    pub max_threads: u64,
    pub multiple_of_execution_width: u64,
    pub indirect: u64,
    pub max_call_stack_depth: u64,
    pub adding_binary_functions: u64,
    pub mutability: [u64; 31],
    pub linked: *const *mut NmbFunction,
    pub linked_count: u64,
    pub private_linked: *const *mut NmbFunction,
    pub private_linked_count: u64,
}

/// The linked functions of a counted array of live function handles.
fn linked_list(functions: *const *mut NmbFunction, count: u64) -> Result<Vec<LinkedFunction>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if functions.is_null() {
        return Err("null linked functions".into());
    }
    // SAFETY: the caller's contract: `count` live handles.
    let handles = unsafe { std::slice::from_raw_parts(functions, usize::try_from(count).map_err(e("linked function count"))?) };
    handles.iter().map(|&function| Ok(get(function)?.0.linked_function())).collect()
}

fn compute_descriptor(descriptor: *const NmbComputePipelineDescriptor) -> Result<ComputePipelineDescriptor, String> {
    let d = get(descriptor)?;
    let mut linked = LinkedFunctions::new();
    linked.functions = linked_list(d.linked, d.linked_count)?;
    linked.private_functions = linked_list(d.private_linked, d.private_linked_count)?;
    let kernel = get(d.function)?.0.function_descriptor().with_linked(linked);
    let mut out = ComputePipelineDescriptor::new(StageSource::Function(kernel));
    out.max_total_threads_per_threadgroup = if d.max_threads == 0 { None } else { Some(u32::try_from(d.max_threads).map_err(e("compute max threads"))?) };
    out.threadgroup_size_is_multiple_of_execution_width = d.multiple_of_execution_width != 0;
    out.support_indirect_command_buffers = d.indirect != 0;
    out.max_call_stack_depth = u32::try_from(d.max_call_stack_depth).map_err(e("compute stack depth"))?;
    out.support_adding_binary_functions = d.adding_binary_functions != 0;
    for (index, &mutability) in d.mutability.iter().enumerate() {
        if mutability != 0 {
            out.buffers.insert(index as u32, raw::<metal2vulkan::metal::Mutability>(mutability, "buffer mutability")?);
        }
    }
    Ok(out)
}

/// # Safety
/// `options` is null or a valid record with a counted array of live archive handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_compute_pipeline_new_with_descriptor(device: *mut NmbDevice, descriptor: *const NmbComputePipelineDescriptor,
                                                                 options: *const NmbPipelineOptions) -> *mut NmbComputePipeline {
    guard(std::ptr::null_mut(), || {
        let resolved = unsafe { pipeline_options(options)? };
        let device = &get(device)?.0;
        let descriptor = compute_descriptor(descriptor)?;
        resolved.check_compute(device, &descriptor)?;
        let state = crate::pipelines::compute(device, &descriptor)?;
        Ok(boxed(NmbComputePipeline(state)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_pipeline_release(pipeline: *mut NmbComputePipeline) {
    release(pipeline)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_pipeline_max_threads(pipeline: *const NmbComputePipeline) -> u64 {
    guard(0, || Ok(get(pipeline)?.0.max_total_threads_per_threadgroup() as u64))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_pipeline_execution_width(pipeline: *const NmbComputePipeline) -> u64 {
    guard(0, || Ok(get(pipeline)?.0.thread_execution_width() as u64))
}

#[repr(C)]
pub struct NmbColorAttachment {
    pub format: u64,
    pub write_mask: u64,
    pub blending: u64,
    pub source_rgb: u64,
    pub destination_rgb: u64,
    pub rgb_operation: u64,
    pub source_alpha: u64,
    pub destination_alpha: u64,
    pub alpha_operation: u64,
}

#[repr(C)]
pub struct NmbVertexAttribute {
    pub format: u64,
    pub offset: u64,
    pub buffer: u64,
}

#[repr(C)]
pub struct NmbVertexLayout {
    pub stride: u64,
    pub step_function: u64,
    pub step_rate: u64,
}

#[repr(C)]
pub struct NmbRenderPipelineDescriptor {
    pub vertex: *mut NmbFunction,
    pub fragment: *mut NmbFunction,
    pub colors: [NmbColorAttachment; 8],
    pub depth_format: u64,
    pub stencil_format: u64,
    pub samples: u64,
    pub topology: u64,
    pub rasterization: u64,
    pub vertex_inputs: u64,
    pub attributes: [NmbVertexAttribute; 31],
    pub layouts: [NmbVertexLayout; 31],
    pub tessellation: NmbTessellation,
    pub vertex_linked: *const *mut NmbFunction,
    pub vertex_linked_count: u64,
    pub vertex_private_linked: *const *mut NmbFunction,
    pub vertex_private_linked_count: u64,
    pub fragment_linked: *const *mut NmbFunction,
    pub fragment_linked_count: u64,
    pub fragment_private_linked: *const *mut NmbFunction,
    pub fragment_private_linked_count: u64,
}

/// A post-tessellation vertex function's state; `tessellates` 0 leaves the descriptor's tessellation unset.
#[repr(C)]
pub struct NmbTessellation {
    pub tessellates: u64,
    pub max_factor: u64,
    pub partition_mode: u64,
    pub factor_format: u64,
    pub factor_step_function: u64,
    pub output_winding: u64,
    pub control_point_index_type: u64,
    pub factor_scale_enabled: u64,
}

fn optional_format(value: u64) -> Result<Option<PixelFormat>, String> {
    if value == 0 { Ok(None) } else { raw::<PixelFormat>(value, "pixel format").map(Some) }
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_pipeline_new(device: *mut NmbDevice, descriptor: *const NmbRenderPipelineDescriptor) -> *mut NmbRenderPipeline {
    // SAFETY: null options names no array; the device/descriptor handles follow the existing ABI.
    unsafe { nmb_render_pipeline_new_with_options(device, descriptor, std::ptr::null()) }
}

#[repr(C)]
pub struct NmbPipelineOptions {
    pub flags: u64,
    pub archives: *const *mut NmbBinaryArchive,
    pub archive_count: usize,
}

/// # Safety
/// The options record and its counted archive array are valid for this call.
unsafe fn pipeline_options(options: *const NmbPipelineOptions) -> Result<PipelineOptions, String> {
    let mut resolved = PipelineOptions::default();
    if let Some(options) = unsafe { options.as_ref() } {
        resolved.flags = options.flags;
        if options.archive_count > 0 {
            if options.archives.is_null() || options.archive_count > isize::MAX as usize / std::mem::size_of::<*mut NmbBinaryArchive>() {
                return Err("invalid binary archive array".into());
            }
            let archives = unsafe { std::slice::from_raw_parts(options.archives, options.archive_count) };
            for &archive in archives {
                resolved.binary_archives.push(get(archive)?.0.clone());
            }
        }
    }
    Ok(resolved)
}

/// # Safety
/// `options` is null or a valid record; its array holds `archive_count` live archive handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_render_pipeline_new_with_options(device: *mut NmbDevice, descriptor: *const NmbRenderPipelineDescriptor,
                                                              options: *const NmbPipelineOptions) -> *mut NmbRenderPipeline {
    guard(std::ptr::null_mut(), || {
        let device = &get(device)?.0;
        let resolved = unsafe { pipeline_options(options)? };
        let descriptor = render_descriptor(descriptor)?;
        resolved.check_render(device, &descriptor)?;
        let state = crate::pipelines::render(device, &descriptor)?;
        Ok(boxed(NmbRenderPipeline(state)))
    })
}

fn render_descriptor(descriptor: *const NmbRenderPipelineDescriptor) -> Result<RenderPipelineDescriptor, String> {
        let d = get(descriptor)?;
        // Each stage's linked functions ride on its function descriptor, as the compute pipeline's do (compute_descriptor).
        let stage_linked = |functions, count, private, private_count| -> Result<LinkedFunctions, String> {
            let mut linked = LinkedFunctions::new();
            linked.functions = linked_list(functions, count)?;
            linked.private_functions = linked_list(private, private_count)?;
            Ok(linked)
        };
        let vertex = get(d.vertex)?.0.function_descriptor()
            .with_linked(stage_linked(d.vertex_linked, d.vertex_linked_count, d.vertex_private_linked, d.vertex_private_linked_count)?);
        let mut out = RenderPipelineDescriptor::new(StageSource::Function(vertex));
        if !d.fragment.is_null() {
            let fragment = get(d.fragment)?.0.function_descriptor().with_linked(stage_linked(
                d.fragment_linked, d.fragment_linked_count, d.fragment_private_linked, d.fragment_private_linked_count)?);
            out.fragment = Some(StageSource::Function(fragment));
        }
        for (slot, c) in out.color_attachments.iter_mut().zip(d.colors.iter()) {
            if c.format == 0 {
                continue;
            }
            let mut a = ColorAttachmentDescriptor::new(raw::<PixelFormat>(c.format, "color format")?);
            a.write_mask = raw::<ColorWriteMask>(c.write_mask, "write mask")?;
            a.blending_enabled = c.blending != 0;
            a.source_rgb_blend_factor = raw::<BlendFactor>(c.source_rgb, "blend factor")?;
            a.destination_rgb_blend_factor = raw::<BlendFactor>(c.destination_rgb, "blend factor")?;
            a.rgb_blend_operation = raw::<BlendOperation>(c.rgb_operation, "blend operation")?;
            a.source_alpha_blend_factor = raw::<BlendFactor>(c.source_alpha, "blend factor")?;
            a.destination_alpha_blend_factor = raw::<BlendFactor>(c.destination_alpha, "blend factor")?;
            a.alpha_blend_operation = raw::<BlendOperation>(c.alpha_operation, "blend operation")?;
            *slot = Some(a);
        }
        out.depth_format = optional_format(d.depth_format)?;
        out.stencil_format = optional_format(d.stencil_format)?;
        out.raster_sample_count = d.samples.max(1) as u32;
        out.input_primitive_topology = raw::<PrimitiveTopologyClass>(d.topology, "topology")?;
        out.rasterization_enabled = d.rasterization != 0;
        if d.vertex_inputs != 0 {
            let mut inputs = VertexDescriptor::new();
            for (i, a) in d.attributes.iter().enumerate() {
                if a.format == 0 {
                    continue;
                }
                inputs.attributes.insert(i as u32, VertexAttributeDescriptor::new(raw::<VertexFormat>(a.format, "vertex format")?,
                                                                                    a.offset as u32, a.buffer as u32));
                if let Some(l) = d.layouts.get(a.buffer as usize) {
                    let mut layout = VertexBufferLayoutDescriptor::new(Some(l.stride as u32));
                    layout.step_function = raw::<VertexStepFunction>(l.step_function, "step function")?;
                    layout.step_rate = l.step_rate as u32;
                    inputs.layouts.insert(a.buffer as u32, layout);
                }
            }
            out.vertex_descriptor = Some(inputs);
        }
        let t = &d.tessellation;
        if t.tessellates != 0 {
            let mut tessellation = TessellationDescriptor::default();
            tessellation.max_tessellation_factor = u32::try_from(t.max_factor).map_err(|_| "tessellation factor".to_string())?;
            tessellation.partition_mode = raw::<TessellationPartitionMode>(t.partition_mode, "partition mode")?;
            tessellation.factor_format = raw::<TessellationFactorFormat>(t.factor_format, "factor format")?;
            tessellation.factor_step_function = raw::<TessellationFactorStepFunction>(t.factor_step_function, "factor step function")?;
            tessellation.output_winding_order = raw::<Winding>(t.output_winding, "output winding")?;
            tessellation.control_point_index_type = raw::<TessellationControlPointIndexType>(t.control_point_index_type,
                                                                                             "control point index type")?;
            tessellation.factor_scale_enabled = t.factor_scale_enabled != 0;
            out.tessellation = Some(tessellation);
        }
        Ok(out)
}

/// `MTLTileRenderPipelineDescriptor`: the tile function, the colour attachment formats (0 for none), the sample count, whether
/// the threadgroup is the tile, the threadgroup limit (0 for the pipeline's own) and the buffers' mutability.
#[repr(C)]
pub struct NmbTilePipelineDescriptor {
    pub tile: *mut NmbFunction,
    pub formats: [u64; 8],
    pub samples: u64,
    pub threadgroup_matches_tile: u64,
    pub max_threads: u64,
    pub max_call_stack_depth: u64,
    pub adding_binary_functions: u64,
    pub mutability: [u64; 31],
    pub linked: *const *mut NmbFunction,
    pub linked_count: u64,
    pub private_linked: *const *mut NmbFunction,
    pub private_linked_count: u64,
}

fn tile_descriptor(descriptor: *const NmbTilePipelineDescriptor) -> Result<TileRenderPipelineDescriptor, String> {
    let d = get(descriptor)?;
    let mut linked = LinkedFunctions::new();
    linked.functions = linked_list(d.linked, d.linked_count)?;
    linked.private_functions = linked_list(d.private_linked, d.private_linked_count)?;
    let tile = get(d.tile)?.0.function_descriptor().with_linked(linked);
    let mut out = TileRenderPipelineDescriptor::new(StageSource::Function(tile));
    for (index, &format) in d.formats.iter().enumerate() {
        out.color_attachment_formats[index] = optional_format(format)?;
    }
    out.raster_sample_count = u32::try_from(d.samples.max(1)).map_err(e("tile sample count"))?;
    out.threadgroup_size_matches_tile_size = d.threadgroup_matches_tile != 0;
    out.max_total_threads_per_threadgroup = if d.max_threads == 0 { None } else { Some(u32::try_from(d.max_threads).map_err(e("tile max threads"))?) };
    out.max_call_stack_depth = u32::try_from(d.max_call_stack_depth.max(1)).map_err(e("tile stack depth"))?;
    out.support_adding_binary_functions = d.adding_binary_functions != 0;
    for (index, &mutability) in d.mutability.iter().enumerate() {
        if mutability != 0 {
            out.buffers.insert(index as u32, raw::<metal2vulkan::metal::Mutability>(mutability, "buffer mutability")?);
        }
    }
    Ok(out)
}

/// `newRenderPipelineStateWithTileDescriptor:options:reflection:error:` without reflection: a render pipeline state whose
/// tile function a render encoder dispatches per tile.
///
/// # Safety
/// `options` is null or a valid record with a counted array of live archive handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_tile_pipeline_new(device: *mut NmbDevice, descriptor: *const NmbTilePipelineDescriptor,
                                              options: *const NmbPipelineOptions) -> *mut NmbRenderPipeline {
    guard(std::ptr::null_mut(), || {
        get(device)?;
        unsafe { pipeline_options(options)?; }
        tile_descriptor(descriptor)?;
        Err("upstream runtime-v2 does not implement tile pipeline states".into())
    })
}

/// `tileWidth` and `tileHeight` of the encoder's pass, into the caller's two words.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_tile_size(encoder: *mut NmbEncoder, out: *mut u64) -> bool {
    guard(false, || {
        render(encoder)?;
        if out.is_null() { return Err("null tile size".into()); }
        Err("upstream runtime-v2 does not implement tile render encoders".into())
    })
}

/// `dispatchThreadsPerTile:`: `threads` is width, height, depth.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_dispatch_threads_per_tile(encoder: *mut NmbEncoder, threads: *const u64) -> bool {
    guard(false, || {
        render(encoder)?; triple(threads)?;
        Err("upstream runtime-v2 does not implement tile dispatch".into())
    })
}

/// `dispatchThreadsPerTile:inRegion:`: `region` is the header's six words.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_dispatch_threads_per_tile_in_region(encoder: *mut NmbEncoder, threads: *const u64, region_words: *const u64)
                                                                 -> bool {
    guard(false, || {
        render(encoder)?; triple(threads)?; region(region_words)?;
        Err("upstream runtime-v2 does not implement regional tile dispatch".into())
    })
}

/// `setThreadgroupMemoryLength:offset:atIndex:` of a render encoder (the tile stage's threadgroup memory).
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_threadgroup_memory_length(encoder: *mut NmbEncoder, length: u64, offset: u64, index: u32) -> bool {
    guard(false, || {
        render(encoder)?;
        Err(format!("upstream runtime-v2 does not implement tile threadgroup memory: length={length} offset={offset} index={index}"))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_pipeline_release(pipeline: *mut NmbRenderPipeline) {
    release(pipeline)
}

// ---- pipeline reflection ----

/// A pipeline's reflection, as the `reflection:` out-parameter of a pipeline's creation answers it, in JSON for the frontend's
/// MTLArgument/MTLBinding objects: Metal's raw enumeration values, and a buffer's struct, array, pointer and texture reference types
/// nested as Metal nests them. GFXBench's NGL engine (Aztec Ruins) finds every uniform's buffer offset and every texture's index in it.
pub struct NmbReflection(std::ffi::CString);

mod reflection_json {
    use metal2vulkan::reflection::{ArrayType, Binding, PointerType, StructType, TextureReferenceType};
    use std::fmt::Write;

    fn string(out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }

    fn structure(out: &mut String, s: &StructType) {
        out.push_str("{\"members\":[");
        for (i, m) in s.members().iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"name\":");
            string(out, m.name());
            let _ = write!(out, ",\"offset\":{},\"dataType\":{},\"argumentIndex\":{}", m.offset(), m.data_type().as_raw(), m.argument_index());
            if let Some(t) = m.struct_type() {
                out.push_str(",\"struct\":");
                structure(out, t);
            }
            if let Some(t) = m.array_type() {
                out.push_str(",\"array\":");
                array(out, t);
            }
            if let Some(t) = m.pointer_type() {
                out.push_str(",\"pointer\":");
                pointer(out, t);
            }
            if let Some(t) = m.texture_reference_type() {
                out.push_str(",\"texture\":");
                texture_reference(out, t);
            }
            out.push('}');
        }
        out.push_str("]}");
    }

    fn array(out: &mut String, a: &ArrayType) {
        let _ = write!(out, "{{\"elementType\":{},\"length\":{},\"stride\":{},\"argumentIndexStride\":{}", a.element_type().as_raw(),
                       a.array_length(), a.stride(), a.argument_index_stride());
        if let Some(t) = a.element_struct_type() {
            out.push_str(",\"struct\":");
            structure(out, t);
        }
        if let Some(t) = a.element_array_type() {
            out.push_str(",\"array\":");
            array(out, t);
        }
        if let Some(t) = a.element_pointer_type() {
            out.push_str(",\"pointer\":");
            pointer(out, t);
        }
        if let Some(t) = a.element_texture_reference_type() {
            out.push_str(",\"texture\":");
            texture_reference(out, t);
        }
        out.push('}');
    }

    fn pointer(out: &mut String, p: &PointerType) {
        let _ = write!(out, "{{\"elementType\":{},\"access\":{},\"alignment\":{},\"size\":{},\"argumentBuffer\":{}", p.element_type().as_raw(),
                       p.access().as_raw(), p.alignment(), p.data_size(), p.element_is_argument_buffer());
        if let Some(t) = p.element_struct_type() {
            out.push_str(",\"struct\":");
            structure(out, t);
        }
        if let Some(t) = p.element_array_type() {
            out.push_str(",\"array\":");
            array(out, t);
        }
        out.push('}');
    }

    fn texture_reference(out: &mut String, t: &TextureReferenceType) {
        let _ = write!(out, "{{\"dataType\":{},\"textureType\":{},\"access\":{},\"depth\":{}}}", t.texture_data_type().as_raw(),
                       t.texture_type().as_raw(), t.access().as_raw(), t.is_depth_texture());
    }

    fn binding(out: &mut String, b: &Binding) {
        out.push_str("{\"name\":");
        string(out, b.name());
        let _ = write!(out, ",\"type\":{},\"access\":{},\"index\":{},\"used\":{},\"argument\":{}", b.binding_type().as_raw(), b.access().as_raw(),
                       b.index(), b.is_used(), b.is_argument());
        if let Some(buffer) = b.buffer() {
            let _ = write!(out, ",\"buffer\":{{\"alignment\":{},\"size\":{},\"dataType\":{}", buffer.buffer_alignment(), buffer.buffer_data_size(),
                           buffer.buffer_data_type().as_raw());
            if let Some(t) = buffer.buffer_struct_type() {
                out.push_str(",\"struct\":");
                structure(out, t);
            }
            if let Some(t) = buffer.buffer_pointer_type() {
                out.push_str(",\"pointer\":");
                pointer(out, t);
            }
            out.push('}');
        }
        if let Some(t) = b.threadgroup() {
            let _ = write!(out, ",\"threadgroup\":{{\"alignment\":{},\"size\":{}}}", t.threadgroup_memory_alignment(), t.threadgroup_memory_data_size());
        }
        if let Some(t) = b.texture() {
            let _ = write!(out, ",\"texture\":{{\"textureType\":{},\"dataType\":{},\"depth\":{},\"arrayLength\":{}}}", t.texture_type().as_raw(),
                           t.texture_data_type().as_raw(), t.is_depth_texture(), t.array_length());
        }
        out.push('}');
    }

    /// `"name":[binding,...]`.
    pub(super) fn stage(out: &mut String, name: &str, bindings: &[Binding]) {
        let _ = write!(out, "\"{name}\":[");
        for (i, b) in bindings.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            binding(out, b);
        }
        out.push(']');
    }
}

fn reflection_handle(json: String) -> Result<*mut NmbReflection, String> {
    Ok(boxed(NmbReflection(std::ffi::CString::new(json).map_err(|_| "a NUL in the reflection".to_string())?)))
}

/// The reflection of the render pipeline `descriptor` describes: `{"vertex":[...],"fragment":[...]}`.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_pipeline_reflection(descriptor: *const NmbRenderPipelineDescriptor) -> *mut NmbReflection {
    guard(std::ptr::null_mut(), || {
        let budget = metal2vulkan::Budget::with_deadline(None);
        let reflection = render_descriptor(descriptor)?.reflection(&budget).map_err(e("render pipeline reflection"))?;
        let mut json = String::from("{");
        reflection_json::stage(&mut json, "vertex", reflection.vertex_bindings());
        json.push(',');
        reflection_json::stage(&mut json, "fragment", reflection.fragment_bindings());
        json.push('}');
        reflection_handle(json)
    })
}

/// The reflection of the compute pipeline `descriptor` describes: `{"bindings":[...]}`.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_pipeline_reflection(descriptor: *const NmbComputePipelineDescriptor) -> *mut NmbReflection {
    guard(std::ptr::null_mut(), || {
        let budget = metal2vulkan::Budget::with_deadline(None);
        let reflection = compute_descriptor(descriptor)?.reflection(&budget).map_err(e("compute pipeline reflection"))?;
        let mut json = String::from("{");
        reflection_json::stage(&mut json, "bindings", reflection.bindings());
        json.push('}');
        reflection_handle(json)
    })
}

/// The reflection's JSON, valid until its release.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_reflection_json(reflection: *const NmbReflection) -> *const c_char {
    guard(std::ptr::null(), || Ok(get(reflection)?.0.as_ptr()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_reflection_release(reflection: *mut NmbReflection) {
    release(reflection)
}

// ---- binary archives ----

pub struct NmbBinaryArchive(BinaryArchive);

/// # Safety
/// `path` is null (an empty archive) or a C string naming the archive's file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_binary_archive_new(device: *mut NmbDevice, path: *const c_char) -> *mut NmbBinaryArchive {
    guard(std::ptr::null_mut(), || {
        // SAFETY: the caller's contract.
        let url = (!path.is_null()).then(|| PathBuf::from(unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned()));
        let device = get(device)?;
        let archive = BinaryArchive::new(&device.0, &device.1, url.as_deref())?;
        Ok(boxed(NmbBinaryArchive(archive)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_binary_archive_release(archive: *mut NmbBinaryArchive) {
    release(archive)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_binary_archive_add_render(archive: *mut NmbBinaryArchive, descriptor: *const NmbRenderPipelineDescriptor) -> bool {
    guard(false, || {
        get(archive)?.0.add_render_pipeline_functions(&render_descriptor(descriptor)?).map_err(e("archive render pipeline"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_binary_archive_add_compute(archive: *mut NmbBinaryArchive, function: *mut NmbFunction) -> bool {
    guard(false, || {
        let descriptor = ComputePipelineDescriptor::new(StageSource::Function(get(function)?.0.function_descriptor()));
        get(archive)?.0.add_compute_pipeline_functions(&descriptor).map_err(e("archive compute pipeline"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_binary_archive_add_compute_descriptor(archive: *mut NmbBinaryArchive, descriptor: *const NmbComputePipelineDescriptor) -> bool {
    guard(false, || {
        get(archive)?.0.add_compute_pipeline_functions(&compute_descriptor(descriptor)?).map_err(e("archive compute pipeline"))?;
        Ok(true)
    })
}

/// # Safety
/// `path` is a C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_binary_archive_serialize(archive: *mut NmbBinaryArchive, path: *const c_char) -> bool {
    guard(false, || {
        if path.is_null() {
            return Err("null path".into());
        }
        // SAFETY: the caller's contract.
        let path = PathBuf::from(unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned());
        get(archive)?.0.serialize_to_url(&path).map_err(e("archive serialize"))?;
        Ok(true)
    })
}

#[repr(C)]
pub struct NmbSamplerDescriptor {
    pub min_filter: u64,
    pub mag_filter: u64,
    pub mip_filter: u64,
    pub s_address: u64,
    pub t_address: u64,
    pub r_address: u64,
    pub border_color: u64,
    pub normalized: u64,
    pub max_anisotropy: u64,
    pub compare: u64,
    pub lod_average: u64,
    pub lod_min: f32,
    pub lod_max: f32,
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_sampler_new(device: *mut NmbDevice, descriptor: *const NmbSamplerDescriptor) -> *mut NmbSampler {
    guard(std::ptr::null_mut(), || {
        let d = get(descriptor)?;
        let mut out = SamplerDescriptor::new();
        out.min_filter = raw::<SamplerMinMagFilter>(d.min_filter, "min filter")?;
        out.mag_filter = raw::<SamplerMinMagFilter>(d.mag_filter, "mag filter")?;
        out.mip_filter = raw::<SamplerMipFilter>(d.mip_filter, "mip filter")?;
        out.s_address_mode = raw::<SamplerAddressMode>(d.s_address, "address mode")?;
        out.t_address_mode = raw::<SamplerAddressMode>(d.t_address, "address mode")?;
        out.r_address_mode = raw::<SamplerAddressMode>(d.r_address, "address mode")?;
        out.border_color = raw::<SamplerBorderColor>(d.border_color, "border color")?;
        out.normalized_coordinates = d.normalized != 0;
        out.max_anisotropy = d.max_anisotropy.max(1) as u32;
        out.compare_function = raw::<CompareFunction>(d.compare, "compare function")?;
        out.lod_average = d.lod_average != 0;
        out.lod_min_clamp = d.lod_min;
        out.lod_max_clamp = d.lod_max;
        let device = &get(device)?.0;
        let index = crate::ownership::reserve(device, metal2vulkan_runtime::IndexClass::Sampler)?;
        Ok(boxed(NmbSampler(device.new_sampler_state(&out, index.index()).map_err(e("sampler"))?, Ownership::new(vec![index.owner()]))))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_sampler_release(sampler: *mut NmbSampler) {
    release(sampler)
}

#[repr(C)]
pub struct NmbStencil {
    pub compare: u64,
    pub failure: u64,
    pub depth_failure: u64,
    pub pass: u64,
    pub read_mask: u64,
    pub write_mask: u64,
}

#[repr(C)]
pub struct NmbDepthStencilDescriptor {
    pub compare: u64,
    pub write: u64,
    pub front: NmbStencil,
    pub back: NmbStencil,
}

fn stencil(s: &NmbStencil) -> Result<StencilDescriptor, String> {
    let mut out = StencilDescriptor::default();
    out.stencil_compare_function = raw::<CompareFunction>(s.compare, "stencil compare")?;
    out.stencil_failure_operation = raw::<StencilOperation>(s.failure, "stencil operation")?;
    out.depth_failure_operation = raw::<StencilOperation>(s.depth_failure, "stencil operation")?;
    out.depth_stencil_pass_operation = raw::<StencilOperation>(s.pass, "stencil operation")?;
    out.read_mask = s.read_mask as u32;
    out.write_mask = s.write_mask as u32;
    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_depth_stencil_new(device: *mut NmbDevice, descriptor: *const NmbDepthStencilDescriptor) -> *mut NmbDepthStencil {
    guard(std::ptr::null_mut(), || {
        let d = get(descriptor)?;
        let mut out = DepthStencilDescriptor::default();
        out.depth_compare_function = raw::<CompareFunction>(d.compare, "depth compare")?;
        out.depth_write_enabled = d.write != 0;
        out.front_face_stencil = stencil(&d.front)?;
        out.back_face_stencil = stencil(&d.back)?;
        Ok(boxed(NmbDepthStencil(get(device)?.0.new_depth_stencil_state(&out))))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_depth_stencil_release(state: *mut NmbDepthStencil) {
    release(state)
}

// ---- queues and command buffers ----

pub struct NmbQueue(CommandQueue);
pub struct NmbCommandBuffer(CommandBuffer, Device, u64, Arc<Mutex<Vec<Owner>>>, CommandQueue, Mutex<Option<Arc<crate::completion::Completion>>>);
static COMMAND_BUFFER_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[unsafe(no_mangle)]
pub extern "C" fn nmb_queue_new(device: *mut NmbDevice) -> *mut NmbQueue {
    guard(std::ptr::null_mut(), || Ok(boxed(NmbQueue(get(device)?.0.new_command_queue(64).map_err(e("queue"))?))))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_queue_release(queue: *mut NmbQueue) {
    release(queue)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_new(queue: *mut NmbQueue) -> *mut NmbCommandBuffer {
    guard(std::ptr::null_mut(), || {
        let queue = &get(queue)?.0;
        let command = queue.command_buffer().map_err(e("command buffer"))?;
        let owners = Arc::new(Mutex::new(Vec::<Owner>::new()));
        let held = owners.clone();
        command.add_completed_handler(move |_| held.lock().unwrap_or_else(|e| e.into_inner()).clear()).map_err(e("resource retirement"))?;
        Ok(boxed(NmbCommandBuffer(command, queue.device().clone(),
            COMMAND_BUFFER_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed), owners, queue.clone(), Mutex::new(None))))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_release(command_buffer: *mut NmbCommandBuffer) {
    release(command_buffer)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_uses_buffer(command_buffer: *const NmbCommandBuffer, buffer: *const NmbBuffer) -> i32 {
    guard(-1, || {
        get(command_buffer)?; get(buffer)?;
        Err("runtime-v2 exposes no resource-overlap query; dependency remains unknown".into())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_uses_texture(command_buffer: *const NmbCommandBuffer, texture: *const NmbTexture) -> i32 {
    guard(-1, || {
        get(command_buffer)?; get(texture)?;
        Err("runtime-v2 exposes no resource-overlap query; dependency remains unknown".into())
    })
}

struct Context(*mut c_void);
// SAFETY: the context is the caller's, handed back on the runtime's thread as the header states.
unsafe impl Send for Context {}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_on_completed(command_buffer: *mut NmbCommandBuffer, handler: Option<extern "C" fn(*mut c_void)>,
                                                  context: *mut c_void) -> bool {
    guard(false, || {
        let handler = handler.ok_or("null handler")?;
        let context = Context(context);
        let id = get(command_buffer)?.2;
        get(command_buffer)?.0.add_completed_handler(move |_: &CommandBuffer| {
            crate::trace::mark(id, 14, 0);
            let context = context;
            COMPLETING.with(|flag| { let previous=flag.replace(true); handler(context.0); flag.set(previous); });
            crate::trace::mark(id, 55, 0);
        }).map_err(e("completed handler"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_on_scheduled(command_buffer: *mut NmbCommandBuffer, handler: Option<extern "C" fn(*mut c_void)>,
                                                  context: *mut c_void) -> bool {
    guard(false, || {
        let handler = handler.ok_or("null handler")?;
        let context = Context(context);
        get(command_buffer)?.0.add_scheduled_handler(move |_: &CommandBuffer| {
            let context = context;
            SCHEDULING.with(|flag| { let previous=flag.replace(true); handler(context.0); flag.set(previous); });
        }).map_err(e("scheduled handler"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_wait_scheduled(command_buffer: *mut NmbCommandBuffer) -> bool {
    guard(false, || {
        get(command_buffer)?.0.wait_until_scheduled().map_err(e("wait scheduled"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_enqueue(command_buffer: *mut NmbCommandBuffer) -> bool {
    guard(false, || {
        get(command_buffer)?.0.enqueue().map_err(e("enqueue"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_commit(command_buffer: *mut NmbCommandBuffer) -> bool {
    let began = TIMING_LOG.get().map(|_| std::time::Instant::now());
    let result = guard(false, || {
        // These marks describe the bridge call, not the runtime's Vulkan submission.
        let buffer = get(command_buffer)?;
        crate::trace::mark(buffer.2, 1, 0);
        crate::trace::mark(buffer.2, 3, 0);
        let mut completion = buffer.5.lock().unwrap_or_else(|e| e.into_inner());
        if completion.is_none() { *completion = Some(crate::completion::Completion::attach(&buffer.0).map_err(e("completion handler"))?); }
        drop(completion);
        buffer.0.commit().map_err(e("commit"))?;
        Ok(true)
    });
    if let Some(began) = began {
        let nanos = began.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        if nanos >= 8_000_000 {
            note_encoder_timing(&format!("command_buffer={command_buffer:p} commit_us={} accepted={result}", nanos / 1000));
        }
    }
    result
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_wait(command_buffer: *mut NmbCommandBuffer, timeout_ms: u64) -> i32 {
    guard(-1, || {
        let buffer = get(command_buffer)?;
        let command_buffer = &buffer.0;
        if timeout_ms == 0 {
            command_buffer.wait_until_completed().map_err(e("wait"))?;
            return Ok(0);
        }
        if COMPLETING.with(|flag| flag.get()) || SCHEDULING.with(|flag| flag.get()) {
            command_buffer.wait_until_completed().map_err(e("wait"))?;
            return Ok(0);
        }
        buffer.4.send_now(command_buffer);
        let completion = buffer.5.lock().unwrap_or_else(|e| e.into_inner()).clone().ok_or("command buffer not committed")?;
        Ok(if completion.wait(Duration::from_millis(timeout_ms)) { 0 } else { 1 })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_status(command_buffer: *const NmbCommandBuffer) -> u64 {
    guard(5, || {
        use metal2vulkan_runtime::CommandBufferStatus;
        Ok(match get(command_buffer)?.0.status() {
            CommandBufferStatus::NotEnqueued => 0,
            CommandBufferStatus::Enqueued => 1,
            CommandBufferStatus::Committed => 2,
            CommandBufferStatus::Scheduled => 3,
            CommandBufferStatus::Completed => 4,
            CommandBufferStatus::Error => 5,
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_error(command_buffer: *const NmbCommandBuffer, out: *mut c_char, size: usize) -> usize {
    guard(0, || {
        let cb = get(command_buffer)?;
        let loss = cb.1.loss();
        let description = cb.0.error().map(|error| format!("{error:?}; first device loss={loss:?}"));
        Ok(description.map_or(0, |text| copy_out(&text, out, size)))
    })
}

// A device loss invalidates every command queue, including its recovery victims.
fn metal_command_buffer_error_code(error: Option<CommandBufferError>) -> u64 {
    match error {
        Some(CommandBufferError::DeviceLost { .. }) => 11,
        Some(CommandBufferError::NotRun) => 1,
        Some(_) => 1,
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_metal_error_code(command_buffer: *const NmbCommandBuffer) -> u64 {
    guard(0, || Ok(metal_command_buffer_error_code(get(command_buffer)?.0.error())))
}

#[cfg(test)]
mod command_buffer_error_tests {
    use super::*;

    #[test]
    fn device_loss_and_abandoned_work_keep_distinct_codes() {
        assert_eq!(metal_command_buffer_error_code(Some(CommandBufferError::DeviceLost { in_flight: true })), 11);
        assert_eq!(metal_command_buffer_error_code(Some(CommandBufferError::DeviceLost { in_flight: false })), 11);
        assert_eq!(metal_command_buffer_error_code(Some(CommandBufferError::NotRun)), 1);
        assert_eq!(metal_command_buffer_error_code(None), 0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_gpu_time(command_buffer: *const NmbCommandBuffer, end: bool) -> f64 {
    guard(0.0, || {
        let command_buffer = &get(command_buffer)?.0;
        let time = if end { command_buffer.gpu_end_time() } else { command_buffer.gpu_start_time() };
        Ok(time as f64 / 1_000_000_000.0)
    })
}

// ---- events and fences ----

pub enum NmbEvent {
    Event(Event),
    Shared(SharedEvent),
}
pub struct NmbFence(Fence);

impl NmbEvent {
    fn as_ref(&self) -> &Event {
        match self {
            NmbEvent::Event(e) => e,
            NmbEvent::Shared(s) => s.as_event(),
        }
    }
}

fn shared<'a>(event: *const NmbEvent) -> Result<&'a SharedEvent, String> {
    match get(event)? {
        NmbEvent::Shared(s) => Ok(s),
        NmbEvent::Event(_) => Err("not a shared event".into()),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_event_new(device: *mut NmbDevice, is_shared: bool) -> *mut NmbEvent {
    guard(std::ptr::null_mut(), || {
        let device = &get(device)?.0;
        Ok(boxed(if is_shared {
            NmbEvent::Shared(device.new_shared_event().map_err(e("shared event"))?)
        } else {
            NmbEvent::Event(device.new_event().map_err(e("event"))?)
        }))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_event_release(event: *mut NmbEvent) {
    release(event)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_shared_event_value(event: *const NmbEvent) -> u64 {
    guard(0, || Ok(shared(event)?.signaled_value()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_shared_event_set_value(event: *mut NmbEvent, value: u64) -> bool {
    guard(false, || {
        shared(event)?.set_signaled_value(value);
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_shared_event_wait(event: *mut NmbEvent, value: u64, timeout_ms: u64) -> bool {
    guard(false, || shared(event)?.wait_until_signaled_value(value, Duration::from_millis(timeout_ms)).map_err(e("shared event wait")))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_shared_event_notify(event: *mut NmbEvent, value: u64, handler: Option<extern "C" fn(*mut c_void, u64)>,
                                          context: *mut c_void) -> bool {
    guard(false, || {
        let handler = handler.ok_or("null handler")?;
        let context = Context(context);
        // The runtime completion thread detects the value; the frontend dispatches
        // the notification to the caller's Metal listener queue.
        shared(event)?.notify_listener(value, move |reached| {
            let context = context;
            handler(context.0, reached)
        }).map_err(e("shared event notification"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_signal_event(command_buffer: *mut NmbCommandBuffer, event: *mut NmbEvent, value: u64) -> bool {
    guard(false, || {
        get(command_buffer)?.0.encode_signal_event(get(event)?.as_ref(), value).map_err(e("signal event"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_wait_event(command_buffer: *mut NmbCommandBuffer, event: *mut NmbEvent, value: u64) -> bool {
    guard(false, || {
        get(command_buffer)?.0.encode_wait_for_event(get(event)?.as_ref(), value).map_err(e("wait for event"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_fence_new(device: *mut NmbDevice) -> *mut NmbFence {
    guard(std::ptr::null_mut(), || Ok(boxed(NmbFence(get(device)?.0.new_fence()))))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_fence_release(fence: *mut NmbFence) {
    release(fence)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_update_fence(encoder: *mut NmbEncoder, fence: *mut NmbFence, stages: u64) -> bool {
    guard(false, || {
        let fence = &get(fence)?.0;
        match get_mut(encoder)? {
            NmbEncoder::Compute(c) => c.update_fence(fence),
            NmbEncoder::Blit(b) => b.update_fence(fence),
            NmbEncoder::Render(r) => {
                raw::<RenderStages>(stages, "render stages")?;
                r.update_fence(fence)
            },
        }
        .map_err(e("update fence"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_wait_fence(encoder: *mut NmbEncoder, fence: *mut NmbFence, stages: u64) -> bool {
    guard(false, || {
        let fence = &get(fence)?.0;
        match get_mut(encoder)? {
            NmbEncoder::Compute(c) => c.wait_for_fence(fence),
            NmbEncoder::Blit(b) => b.wait_for_fence(fence),
            NmbEncoder::Render(r) => {
                raw::<RenderStages>(stages, "render stages")?;
                r.wait_for_fence(fence)
            },
        }
        .map_err(e("wait for fence"))?;
        Ok(true)
    })
}

// ---- encoders ----

pub enum NmbEncoder {
    Compute(ComputeCommandEncoder),
    Render(RenderCommandEncoder),
    Blit(BlitCommandEncoder),
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_encoder_new(command_buffer: *mut NmbCommandBuffer) -> *mut NmbEncoder {
    nmb_compute_encoder_with_dispatch(command_buffer, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_encoder_with_dispatch(command_buffer: *mut NmbCommandBuffer, dispatch: u64) -> *mut NmbEncoder {
    guard(std::ptr::null_mut(), || {
        let dispatch = match dispatch {
            0 => metal2vulkan_runtime::DispatchType::Serial,
            1 => metal2vulkan_runtime::DispatchType::Concurrent,
            _ => return Err("invalid compute dispatch type".into()),
        };
        Ok(boxed(NmbEncoder::Compute(get(command_buffer)?.0.compute_command_encoder(dispatch).map_err(e("compute encoder"))?)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_blit_encoder_new(command_buffer: *mut NmbCommandBuffer) -> *mut NmbEncoder {
    guard(std::ptr::null_mut(), || Ok(boxed(NmbEncoder::Blit(get(command_buffer)?.0.blit_command_encoder().map_err(e("blit encoder"))?))))
}

#[repr(C)]
pub struct NmbPassAttachment {
    pub texture: *mut NmbTexture,
    pub level: u64,
    pub slice: u64,
    pub depth_plane: u64,
    pub load: u64,
    pub store: u64,
    pub clear: [f64; 4],
    pub resolve_texture: *mut NmbTexture,
    pub resolve_level: u64,
    pub resolve_slice: u64,
    pub resolve_depth_plane: u64,
    pub resolve_filter: u64,
}

#[repr(C)]
pub struct NmbRenderPassDescriptor {
    pub colors: [NmbPassAttachment; 8],
    pub depth: NmbPassAttachment,
    pub stencil: NmbPassAttachment,
    pub width: u64,
    pub height: u64,
    pub array_length: u64,
    pub samples: u64,
    pub visibility: *const NmbBuffer,
    /// `tileWidth`, `tileHeight`, `imageblockSampleLength` and `threadgroupMemoryLength` of the pass (0 for Metal's defaults).
    pub tile_width: u64,
    pub tile_height: u64,
    pub imageblock_sample_length: u64,
    pub threadgroup_memory_length: u64,
}

fn texture_of(a: &NmbPassAttachment) -> Result<Option<metal2vulkan_runtime::Texture>, String> {
    if a.texture.is_null() { Ok(None) } else { Ok(Some((**get(a.texture)?.0.live()?).clone())) }
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_encoder_new(command_buffer: *mut NmbCommandBuffer, descriptor: *const NmbRenderPassDescriptor)
                                         -> *mut NmbEncoder {
    guard(std::ptr::null_mut(), || {
        let d = get(descriptor)?;
        let mut pass = RenderPassDescriptor::default();
        for (slot, a) in pass.color_attachments.iter_mut().zip(d.colors.iter()) {
            slot.texture = texture_of(a)?;
            slot.resolve_texture = optional(a.resolve_texture)?.map(|t| t.0.live().map(|t| (**t).clone())).transpose()?;
            slot.resolve_level = narrow(a.resolve_level)?;
            slot.resolve_slice = narrow(a.resolve_slice)?;
            slot.resolve_depth_plane = narrow(a.resolve_depth_plane)?;
            slot.level = narrow(a.level)?;
            slot.slice = narrow(a.slice)?;
            if a.depth_plane != 0 && slot.texture.as_ref().is_some_and(|texture| texture.descriptor().texture_type == TextureType::Type3D) { return Err("upstream runtime does not render into a 3D depth plane".into()); }
            slot.load_action = LoadAction::from_raw(a.load).map_err(e("load action"))?;
            slot.store_action = StoreAction::from_raw(a.store).map_err(e("store action"))?;
            slot.clear_color = ClearColor::new(a.clear[0], a.clear[1], a.clear[2], a.clear[3]);
        }
        let depth = &mut pass.depth_attachment;
        depth.texture = texture_of(&d.depth)?;
        depth.resolve_texture = optional(d.depth.resolve_texture)?.map(|t| t.0.live().map(|t| (**t).clone())).transpose()?;
        depth.resolve_level = narrow(d.depth.resolve_level)?;
        depth.resolve_slice = narrow(d.depth.resolve_slice)?;
        depth.resolve_depth_plane = narrow(d.depth.resolve_depth_plane)?;
        depth.depth_resolve_filter = MultisampleDepthResolveFilter::from_raw(d.depth.resolve_filter).map_err(e("depth resolve filter"))?;
        depth.level = narrow(d.depth.level)?;
        depth.slice = narrow(d.depth.slice)?;
        if d.depth.depth_plane != 0 && depth.texture.as_ref().is_some_and(|texture| texture.descriptor().texture_type == TextureType::Type3D) { return Err("upstream runtime does not render into a 3D depth plane".into()); }
        depth.load_action = LoadAction::from_raw(d.depth.load).map_err(e("load action"))?;
        depth.store_action = StoreAction::from_raw(d.depth.store).map_err(e("store action"))?;
        depth.clear_depth = d.depth.clear[0];
        let stencil = &mut pass.stencil_attachment;
        stencil.texture = texture_of(&d.stencil)?;
        stencil.resolve_texture = optional(d.stencil.resolve_texture)?.map(|t| t.0.live().map(|t| (**t).clone())).transpose()?;
        stencil.resolve_level = narrow(d.stencil.resolve_level)?;
        stencil.resolve_slice = narrow(d.stencil.resolve_slice)?;
        stencil.resolve_depth_plane = narrow(d.stencil.resolve_depth_plane)?;
        stencil.stencil_resolve_filter = MultisampleStencilResolveFilter::from_raw(d.stencil.resolve_filter).map_err(e("stencil resolve filter"))?;
        stencil.level = narrow(d.stencil.level)?;
        stencil.slice = narrow(d.stencil.slice)?;
        if d.stencil.depth_plane != 0 && stencil.texture.as_ref().is_some_and(|texture| texture.descriptor().texture_type == TextureType::Type3D) { return Err("upstream runtime does not render into a 3D depth plane".into()); }
        stencil.load_action = LoadAction::from_raw(d.stencil.load).map_err(e("load action"))?;
        stencil.store_action = StoreAction::from_raw(d.stencil.store).map_err(e("store action"))?;
        stencil.clear_stencil = d.stencil.clear[0] as u32;
        pass.render_target_width = narrow(d.width)?;
        pass.render_target_height = narrow(d.height)?;
        pass.render_target_array_length = narrow(d.array_length)?;
        pass.default_raster_sample_count = narrow(d.samples)?;
        if d.tile_width != 0 || d.tile_height != 0 || d.imageblock_sample_length != 0 || d.threadgroup_memory_length != 0 {
            return Err("upstream runtime does not implement tile/imageblock render-pass fields".into());
        }
        if !d.visibility.is_null() {
            pass.visibility_result_buffer = Some((**get(d.visibility)?.0.live()?).clone());
        }
        let encoder = get(command_buffer)?.0.render_command_encoder(&pass).map_err(e("render encoder"))?;
        Ok(boxed(NmbEncoder::Render(encoder)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_end(encoder: *mut NmbEncoder) -> bool {
    guard(false, || {
        if encoder.is_null() {
            return Err("null encoder".into());
        }
        // SAFETY: from `boxed`; ending releases it.
        let encoder = unsafe { Box::from_raw(encoder) };
        match *encoder {
            NmbEncoder::Compute(c) => c.end_encoding(),
            NmbEncoder::Render(r) => r.end_encoding(),
            NmbEncoder::Blit(b) => b.end_encoding(),
        };
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_discard(encoder: *mut NmbEncoder) {
    release(encoder)
}

fn compute<'a>(encoder: *mut NmbEncoder) -> Result<&'a mut ComputeCommandEncoder, String> {
    match get_mut(encoder)? {
        NmbEncoder::Compute(c) => Ok(c),
        _ => Err("not a compute encoder".into()),
    }
}

fn render<'a>(encoder: *mut NmbEncoder) -> Result<&'a mut RenderCommandEncoder, String> {
    match get_mut(encoder)? {
        NmbEncoder::Render(r) => Ok(r),
        _ => Err("not a render encoder".into()),
    }
}

fn blit<'a>(encoder: *mut NmbEncoder) -> Result<&'a mut BlitCommandEncoder, String> {
    match get_mut(encoder)? {
        NmbEncoder::Blit(b) => Ok(b),
        _ => Err("not a blit encoder".into()),
    }
}

fn optional<'a, T>(handle: *const T) -> Result<Option<&'a T>, String> {
    if handle.is_null() { Ok(None) } else { get(handle).map(Some) }
}

fn resource_usage(word: u64) -> Result<Option<ResourceUsage>, String> {
    let bits = raw::<metal2vulkan::metal::ResourceUsage>(word, "resource usage")?.as_raw();
    Ok([(1, ResourceUsage::READ), (2, ResourceUsage::WRITE), (4, ResourceUsage::SAMPLE)]
        .into_iter().filter_map(|(bit, usage)| (bits & bit != 0).then_some(usage))
        .reduce(|a, b| a | b))
}

#[cfg(test)]
mod indirect_resource_usage_tests {
    use super::*;
    #[test]
    fn combined_usage_preserves_sampling_and_writes() {
        assert_eq!(resource_usage(0).unwrap(), None);
        assert_eq!(resource_usage(7).unwrap(), Some(ResourceUsage::READ | ResourceUsage::WRITE | ResourceUsage::SAMPLE));
        assert_eq!(resource_usage(6).unwrap(), Some(ResourceUsage::WRITE | ResourceUsage::SAMPLE));
    }
    #[test]
    fn unknown_usage_bits_are_refused() {
        assert!(resource_usage(8).is_err());
        assert!(resource_usage(u64::MAX).is_err());
    }
}

/// Declare indirect resources through the runtime's hazard tracking and retirement ownership.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_use_resource(encoder: *mut NmbEncoder, buffer: *const NmbBuffer,
                                           texture: *const NmbTexture, usage: u64, stages: u64) -> bool {
    guard(false, || {
        let buffer = optional(buffer)?.map(|b| b.0.live()).transpose()?;
        let texture = optional(texture)?.map(|t| t.0.live()).transpose()?;
        let resource = match (buffer.as_deref(), texture.as_deref()) {
            (Some(b), None) => RuntimeResource::Buffer(b),
            (None, Some(t)) => RuntimeResource::Texture(t),
            _ => return Err("expected one indirect resource".into()),
        };
        let usage = resource_usage(usage)?;
        match get_mut(encoder)? {
            NmbEncoder::Compute(c) => {
                if let Some(usage) = usage { c.use_resource(resource, usage).map_err(e("use resource"))?; }
            },
            NmbEncoder::Render(r) => {
                raw::<RenderStages>(stages, "render stages")?;
                if let Some(usage) = usage { r.use_resource(resource, usage).map_err(e("use resource"))?; }
            },
            _ => return Err("indirect resources require a compute or render encoder".into()),
        }
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_use_heap(encoder: *mut NmbEncoder, command: *const NmbCommandBuffer,
                                      heap: *const NmbHeap, stages: u64) -> bool {
    guard(false, || {
        let heap = get(heap)?.0.live()?;
        let block = heap.block_id_on(&get(command)?.1)?;
        match get_mut(encoder)? {
            NmbEncoder::Compute(c) => c.use_heap(block),
            NmbEncoder::Render(r) => {
                raw::<RenderStages>(stages, "render stages")?;
                r.use_heap(block)
            },
            _ => return Err("heap declarations require a compute or render encoder".into()),
        }.map_err(e("use heap"))?;
        Ok(true)
    })
}

/// Both Metal barrier forms map to the upstream global encoder barrier.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_memory_barrier(encoder: *mut NmbEncoder, after: u64, before: u64) -> bool {
    guard(false, || {
        match get_mut(encoder)? {
            NmbEncoder::Compute(c) => c.memory_barrier(),
            NmbEncoder::Render(r) => {
                raw::<RenderStages>(after, "render stages")?;
                raw::<RenderStages>(before, "render stages")?;
                r.memory_barrier().map_err(e("memory barrier"))?;
            },
            _ => return Err("memory barriers require a compute or render encoder".into()),
        }
        Ok(true)
    })
}

const COMPUTE: u32 = 0;
const VERTEX: u32 = 1;
const FRAGMENT: u32 = 2;
/// The tile stage of a render encoder (`setTileBuffer:` and its kin).
const TILE: u32 = 3;

fn bad_stage(stage: u32) -> String {
    format!("stage {stage} does not fit this encoder")
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_set_buffer(encoder: *mut NmbEncoder, stage: u32, buffer: *mut NmbBuffer, offset: u64, index: u32) -> bool {
    guard(false, || {
        let buffer = optional(buffer)?.map(|b| b.0.live()).transpose()?;
        let buffer = buffer.as_deref().map(|buffer| &**buffer);
        match (get_mut(encoder)?, stage) {
            (NmbEncoder::Compute(c), COMPUTE) => c.set_buffer(buffer, offset, index),
            (NmbEncoder::Render(r), VERTEX) => r.set_vertex_buffer(buffer, offset, index),
            (NmbEncoder::Render(r), FRAGMENT) => r.set_fragment_buffer(buffer, offset, index),
            (NmbEncoder::Render(_), TILE) => return Err("upstream runtime does not implement tile shaders".into()),
            _ => return Err(bad_stage(stage)),
        }
        .map_err(e("set buffer"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_set_buffer_offset(encoder: *mut NmbEncoder, stage: u32, offset: u64, index: u32) -> bool {
    guard(false, || {
        match (get_mut(encoder)?, stage) {
            (NmbEncoder::Compute(c), COMPUTE) => c.set_buffer_offset(offset, index),
            (NmbEncoder::Render(r), VERTEX) => r.set_vertex_buffer_offset(offset, index),
            (NmbEncoder::Render(r), FRAGMENT) => r.set_fragment_buffer_offset(offset, index),
            (NmbEncoder::Render(_), TILE) => return Err("upstream runtime does not implement tile shaders".into()),
            _ => return Err(bad_stage(stage)),
        }
        .map_err(e("set buffer offset"))?;
        Ok(true)
    })
}

/// # Safety
/// `bytes` holds `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_encoder_set_bytes(encoder: *mut NmbEncoder, stage: u32, bytes: *const c_void, length: usize, index: u32)
                                               -> bool {
    guard(false, || {
        if bytes.is_null() {
            return Err("null bytes".into());
        }
        // SAFETY: the caller's contract.
        let bytes = unsafe { std::slice::from_raw_parts(bytes as *const u8, length) };
        match (get_mut(encoder)?, stage) {
            (NmbEncoder::Compute(c), COMPUTE) => c.set_bytes(bytes, index),
            (NmbEncoder::Render(r), VERTEX) => r.set_vertex_bytes(bytes, index),
            (NmbEncoder::Render(r), FRAGMENT) => r.set_fragment_bytes(bytes, index),
            (NmbEncoder::Render(_), TILE) => return Err("upstream runtime does not implement tile shaders".into()),
            _ => return Err(bad_stage(stage)),
        }
        .map_err(e("set bytes"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_set_texture(encoder: *mut NmbEncoder, stage: u32, texture: *mut NmbTexture, index: u32) -> bool {
    guard(false, || {
        let texture = optional(texture)?.map(|t| t.0.slot()).transpose()?;
        let texture = texture.as_deref().map(|texture| &**texture);
        match (get_mut(encoder)?, stage) {
            (NmbEncoder::Compute(c), COMPUTE) => c.set_texture(texture, index),
            (NmbEncoder::Render(r), VERTEX) => r.set_vertex_texture(texture, index),
            (NmbEncoder::Render(r), FRAGMENT) => r.set_fragment_texture(texture, index),
            (NmbEncoder::Render(_), TILE) => return Err("upstream runtime does not implement tile shaders".into()),
            _ => return Err(bad_stage(stage)),
        }
        .map_err(e("set texture"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_encoder_set_sampler(encoder: *mut NmbEncoder, stage: u32, sampler: *mut NmbSampler, index: u32) -> bool {
    guard(false, || {
        let sampler = optional(sampler)?.map(|s| &s.0);
        match (get_mut(encoder)?, stage) {
            (NmbEncoder::Compute(c), COMPUTE) => c.set_sampler_state(sampler, index),
            (NmbEncoder::Render(r), VERTEX) => r.set_vertex_sampler_state(sampler, index),
            (NmbEncoder::Render(r), FRAGMENT) => r.set_fragment_sampler_state(sampler, index),
            (NmbEncoder::Render(_), TILE) => return Err("upstream runtime does not implement tile shaders".into()),
            _ => return Err(bad_stage(stage)),
        }
        .map_err(e("set sampler"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_set_pipeline(encoder: *mut NmbEncoder, pipeline: *mut NmbComputePipeline) -> bool {
    guard(false, || {
        compute(encoder)?.set_compute_pipeline_state(&get(pipeline)?.0).map_err(e("compute pipeline"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_set_threadgroup_memory(encoder: *mut NmbEncoder, length: u32, index: u32) -> bool {
    guard(false, || {
        compute(encoder)?.set_threadgroup_memory_length(length, index).map_err(e("threadgroup memory"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_dispatch(encoder: *mut NmbEncoder, size: *const u64, per_group: *const u64, threads: bool) -> bool {
    guard(false, || {
        let (s, g) = (triple(size)?, triple(per_group)?);
        let c = compute(encoder)?;
        if threads { c.dispatch_threads(s, g) } else { c.dispatch_threadgroups(s, g) }.map_err(e("dispatch"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_compute_dispatch_indirect(encoder: *mut NmbEncoder, buffer: *mut NmbBuffer, offset: u64, per_group: *const u64) -> bool {
    guard(false, || {
        let g = triple(per_group)?;
        compute(encoder)?.dispatch_threadgroups_with_indirect_buffer(&*get(buffer)?.0.live()?, offset, g)
            .map_err(e("indirect dispatch"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_pipeline(encoder: *mut NmbEncoder, pipeline: *mut NmbRenderPipeline) -> bool {
    guard(false, || {
        render(encoder)?.set_render_pipeline_state(&get(pipeline)?.0).map_err(e("render pipeline"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_depth_stencil(encoder: *mut NmbEncoder, state: *mut NmbDepthStencil) -> bool {
    guard(false, || {
        render(encoder)?.set_depth_stencil_state(optional(state)?.map(|s| &s.0)).map_err(e("depth/stencil state"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_viewport(encoder: *mut NmbEncoder, viewport: *const f64) -> bool {
    guard(false, || {
        if viewport.is_null() {
            return Err("null viewport".into());
        }
        // SAFETY: the header's six values.
        let v = unsafe { std::slice::from_raw_parts(viewport, 6) };
        render(encoder)?.set_viewport(Viewport { origin_x: v[0], origin_y: v[1], width: v[2], height: v[3], znear: v[4], zfar: v[5] }).map_err(e("viewport"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_scissor(encoder: *mut NmbEncoder, rect: *const u64) -> bool {
    guard(false, || {
        if rect.is_null() {
            return Err("null scissor".into());
        }
        // SAFETY: the header's four values.
        let r = unsafe { std::slice::from_raw_parts(rect, 4) };
        render(encoder)?.set_scissor_rect(scissor(r)?);
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_raster(encoder: *mut NmbEncoder, cull_mode: u64, winding: u64, fill_mode: u64) -> bool {
    guard(false, || {
        let r = render(encoder)?;
        r.set_cull_mode(raw::<CullMode>(cull_mode, "cull mode")?);
        r.set_front_facing_winding(raw::<Winding>(winding, "winding")?);
        r.set_triangle_fill_mode(raw::<TriangleFillMode>(fill_mode, "fill mode")?).map_err(e("fill mode"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_stencil_reference(encoder: *mut NmbEncoder, front: u32, back: u32) -> bool {
    guard(false, || {
        render(encoder)?.set_stencil_front_back_reference_value(front, back);
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_depth_bias(encoder: *mut NmbEncoder, bias: f32, slope_scale: f32, clamp: f32) -> bool {
    guard(false, || {
        render(encoder)?.set_depth_bias(bias, slope_scale, clamp);
        Ok(true)
    })
}

/// SPI `setLineWidth:`: the runtime accepts Metal's default width of 1 and refuses any other (Apple's wide-line rule is unmeasured).
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_line_width(encoder: *mut NmbEncoder, width: f32) -> bool {
    guard(false, || {
        render(encoder)?.set_line_width(width);
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_visibility_mode(encoder: *mut NmbEncoder, mode: u64, offset: u64) -> bool {
    guard(false, || {
        let mode = VisibilityResultMode::from_raw(mode).map_err(e("visibility result mode"))?;
        render(encoder)?.set_visibility_result_mode(mode, offset).map_err(e("visibility result mode"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_blend_color(encoder: *mut NmbEncoder, color: *const f32) -> bool {
    guard(false, || {
        if color.is_null() {
            return Err("null color".into());
        }
        // SAFETY: the header's four values.
        let c = unsafe { std::slice::from_raw_parts(color, 4) };
        render(encoder)?.set_blend_color(c[0], c[1], c[2], c[3]);
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_draw(encoder: *mut NmbEncoder, primitive: u64, start: u32, count: u32, instances: u32, base_instance: u32)
                                  -> bool {
    guard(false, || {
        render(encoder)?.draw_primitives(raw::<PrimitiveType>(primitive, "primitive")?, start, count, instances, base_instance)
            .map_err(e("draw"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn nmb_render_draw_indexed(encoder: *mut NmbEncoder, primitive: u64, count: u32, index_type: u64, indices: *mut NmbBuffer,
                                          offset: u64, instances: u32, base_vertex: i32, base_instance: u32) -> bool {
    guard(false, || {
        render(encoder)?.draw_indexed_primitives(raw::<PrimitiveType>(primitive, "primitive")?, count,
                                                 metal2vulkan_runtime::IndexBuffer { index_type: raw::<IndexType>(index_type, "index type")?, buffer: &*get(indices)?.0.live()?, offset }, instances,
                                                 base_vertex, base_instance).map_err(e("indexed draw"))?;
        Ok(true)
    })
}

/// `setTessellationFactorBuffer:offset:instanceStride:`; a null buffer unsets it.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_tessellation_factor_buffer(encoder: *mut NmbEncoder, buffer: *mut NmbBuffer, offset: u64, instance_stride: u64)
                                                            -> bool {
    guard(false, || {
        let buffer = optional_buffer(buffer)?;
        render(encoder)?.set_tessellation_factor_buffer(buffer.as_deref().map(|buffer| &**buffer), offset, instance_stride).map_err(e("tessellation factor buffer"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_tessellation_factor_scale(encoder: *mut NmbEncoder, scale: f32) -> bool {
    guard(false, || {
        render(encoder)?;
        Err(format!("upstream runtime does not implement tessellation factor scale ({scale})"))
    })
}

fn optional_buffer<'a>(buffer: *mut NmbBuffer) -> Result<Option<crate::purge::Live<'a, Buffer>>, String> {
    if buffer.is_null() { Ok(None) } else { get(buffer)?.0.live().map(Some) }
}

/// The four patch draws: `indirect` null takes `patch_start`, `patch_count`, `instances` and `base_instance`, otherwise reads them from
/// it at `indirect_offset`; `control_points_index` null draws the patches' own control points, otherwise indexes them through it.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn nmb_render_draw_patches(encoder: *mut NmbEncoder, control_points: u32, patch_start: u32, patch_count: u32,
                                          patch_index: *mut NmbBuffer, patch_index_offset: u64, control_point_index: *mut NmbBuffer,
                                          _control_point_index_offset: u64, instances: u32, base_instance: u32, indirect: *mut NmbBuffer,
                                          indirect_offset: u64) -> bool {
    guard(false, || {
        let r = render(encoder)?;
        let (patch_index, points, indirect) = (optional_buffer(patch_index)?, optional_buffer(control_point_index)?, optional_buffer(indirect)?);
        if points.is_some() {
            return Err("upstream runtime does not implement indexed control-point patch draws".into());
        }
        let patch_index = patch_index.as_deref().map(|buffer| (&**buffer, patch_index_offset));
        let result = match indirect.as_deref() {
            None => r.draw_patches(control_points, patch_start, patch_count, patch_index, instances, base_instance),
            Some(indirect) => r.draw_patches_indirect(control_points, patch_index, indirect, indirect_offset),
        };
        result.map_err(e("patch draw"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_blit_copy_buffer(encoder: *mut NmbEncoder, source: *mut NmbBuffer, source_offset: u64, destination: *mut NmbBuffer,
                                       destination_offset: u64, size: u64) -> bool {
    guard(false, || {
        blit(encoder)?.copy_from_buffer_to_buffer(&*get(source)?.0.live()?, source_offset, &*get(destination)?.0.live()?, destination_offset, size)
            .map_err(e("buffer copy"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_blit_fill_buffer(encoder: *mut NmbEncoder, buffer: *mut NmbBuffer, start: u64, end: u64, value: u8) -> bool {
    guard(false, || {
        blit(encoder)?.fill_buffer(&*get(buffer)?.0.live()?, start..end, value).map_err(e("fill"))?;
        Ok(true)
    })
}

fn origin(v: *const u64) -> Result<[u32; 3], String> {
    triple(v)
}

fn size(v: *const u64) -> Result<[u32; 3], String> {
    triple(v)
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn nmb_blit_copy_texture(encoder: *mut NmbEncoder, source: *mut NmbTexture, source_slice: u64, source_level: u64,
                                        source_origin: *const u64, extent: *const u64, destination: *mut NmbTexture,
                                        destination_slice: u64, destination_level: u64, destination_origin: *const u64) -> bool {
    guard(false, || {
        blit(encoder)?.copy_from_texture(&*get(source)?.0.live()?,
            metal2vulkan_runtime::TexturePlace { slice: narrow(source_slice)?, level: narrow(source_level)?, origin: origin(source_origin)? },
            size(extent)?, &*get(destination)?.0.live()?,
            metal2vulkan_runtime::TexturePlace { slice: narrow(destination_slice)?, level: narrow(destination_level)?, origin: origin(destination_origin)? })
            .map_err(e("texture copy"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn nmb_blit_buffer_to_texture(encoder: *mut NmbEncoder, source: *mut NmbBuffer, offset: u64, bytes_per_row: u64,
                                             bytes_per_image: u64, extent: *const u64, destination: *mut NmbTexture, slice: u64,
                                             level: u64, destination_origin: *const u64) -> bool {
    guard(false, || {
        blit(encoder)?.copy_from_buffer(&*get(source)?.0.live()?,
            metal2vulkan_runtime::BufferLayout { offset, bytes_per_row, bytes_per_image }, size(extent)?, &*get(destination)?.0.live()?,
            metal2vulkan_runtime::TexturePlace { slice: narrow(slice)?, level: narrow(level)?, origin: origin(destination_origin)? })
            .map_err(e("buffer to texture"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn nmb_blit_texture_to_buffer(encoder: *mut NmbEncoder, source: *mut NmbTexture, slice: u64, level: u64,
                                             source_origin: *const u64, extent: *const u64, destination: *mut NmbBuffer, offset: u64,
                                             bytes_per_row: u64, bytes_per_image: u64) -> bool {
    guard(false, || {
        blit(encoder)?.copy_from_texture_to_buffer(&*get(source)?.0.live()?,
            metal2vulkan_runtime::TexturePlace { slice: narrow(slice)?, level: narrow(level)?, origin: origin(source_origin)? },
            size(extent)?, &*get(destination)?.0.live()?, metal2vulkan_runtime::BufferLayout { offset, bytes_per_row, bytes_per_image })
            .map_err(e("texture to buffer"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_blit_generate_mipmaps(encoder: *mut NmbEncoder, texture: *mut NmbTexture) -> bool {
    guard(false, || {
        blit(encoder)?.generate_mipmaps(&*get(texture)?.0.live()?).map_err(e("mipmaps"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_new_view_swizzle(texture: *mut NmbTexture, format: u64, kind: u64, levels: *const u64, slices: *const u64,
                                               swizzle: u64) -> *mut NmbTexture {
    guard(std::ptr::null_mut(), || {
        let (tie, t) = (get(texture)?.0.tie(), get(texture)?.0.live()?);
        if levels.is_null() || slices.is_null() {
            return Err("null levels or slices".into());
        }
        // SAFETY: the header's two words each.
        let (l, s) = unsafe { (std::slice::from_raw_parts(levels, 2), std::slice::from_raw_parts(slices, 2)) };
        let ranges = (l[0]..l[0].checked_add(l[1]).ok_or("levels overflow")?, s[0]..s[0].checked_add(s[1]).ok_or("slices overflow")?);
        let channels = swizzle_of(swizzle)?.unwrap_or(TextureSwizzleChannels::DEFAULT);
        let view = t.new_texture_view_with_swizzle(raw::<PixelFormat>(format, "pixel format")?, raw::<TextureType>(kind, "texture type")?,
                                                   ranges.0, ranges.1, channels);
        Ok(boxed(NmbTexture(Resource::tied(view.map_err(e("texture view"))?, tie))))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_make_aliasable(texture: *const NmbTexture) -> bool {
    guard(false, || {
        get(texture)?.0.live()?.make_aliasable().map_err(e("make aliasable"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_is_aliasable(texture: *const NmbTexture) -> bool {
    guard(false, || Ok(get(texture)?.0.live()?.is_aliasable()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_make_aliasable(buffer: *const NmbBuffer) -> bool {
    guard(false, || {
        get(buffer)?.0.live()?.make_aliasable().map_err(e("make aliasable"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_buffer_is_aliasable(buffer: *const NmbBuffer) -> bool {
    guard(false, || Ok(get(buffer)?.0.live()?.is_aliasable()))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_constant_count(function: *const NmbFunction) -> u32 {
    guard(0, || Ok(get(function)?.0.function_constants_dictionary().map_err(e("function constants"))?.len() as u32))
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_constant_at(function: *const NmbFunction, at: u32, name: *mut c_char, capacity: usize, index: *mut u64,
                                           data_type: *mut u64, required: *mut bool) -> bool {
    guard(false, || {
        let constants = get(function)?.0.function_constants_dictionary().map_err(e("function constants"))?;
        let (key, constant) = constants.iter().nth(at as usize).ok_or("no such constant")?;
        if name.is_null() || capacity == 0 || index.is_null() || data_type.is_null() || required.is_null() {
            return Err("null out".into());
        }
        let bytes = key.as_bytes();
        let n = bytes.len().min(capacity - 1);
        // SAFETY: the caller gives `capacity` bytes at `name` and one word at each other pointer.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), name as *mut u8, n);
            *name.add(n) = 0;
            *index = u64::from(constant.index());
            *data_type = u64::from(constant.data_type().as_raw());
            *required = constant.is_required();
        }
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_function_constant_name_size(function: *const NmbFunction, at: u32) -> usize {
    guard(0, || {
        let constants = get(function)?.0.function_constants_dictionary().map_err(e("function constants"))?;
        Ok(constants.iter().nth(at as usize).ok_or("no such constant")?.0.len() + 1)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_viewports(encoder: *mut NmbEncoder, values: *const f64, count: usize) -> bool {
    guard(false, || {
        if count == 0 || count > 16 || values.is_null() { return Err("invalid viewport array".into()); }
        let v = unsafe { std::slice::from_raw_parts(values, count * 6) };
        let viewports: Vec<_> = v.chunks_exact(6).map(|v| Viewport { origin_x:v[0], origin_y:v[1], width:v[2], height:v[3], znear:v[4], zfar:v[5] }).collect();
        render(encoder)?.set_viewports(&viewports).map_err(e("viewports"))?;
        Ok(true)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn nmb_render_set_scissors(encoder: *mut NmbEncoder, values: *const u64, count: usize) -> bool {
    guard(false, || {
        if count == 0 || count > 16 || values.is_null() { return Err("invalid scissor array".into()); }
        let v = unsafe { std::slice::from_raw_parts(values, count * 4) };
        let rects = v.chunks_exact(4).map(scissor).collect::<Result<Vec<_>, _>>()?;
        render(encoder)?.set_scissor_rects(&rects).map_err(e("scissor rectangles"))?;
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_texture_allocated_size(texture: *const NmbTexture) -> u64 {
    guard(0, || Ok(get(texture)?.0.bytes()))
}
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_current_allocated_size(device: *const NmbDevice) -> u64 {
    guard(0, || {
        let g = get(device)?.0.counters().gauges();
        Ok(g.shared_allocation_bytes.saturating_add(g.private_allocation_bytes)
            .saturating_add(g.texture_own_bytes).saturating_add(g.import_bytes)
            .saturating_add(g.export_bytes).saturating_add(g.surface_import_bytes))
    })
}

/// Set the link symbol while retaining the source definition and its constants.
///
/// # Safety
/// `name` is null or a readable, NUL-terminated UTF-8 string; `function` is live and exclusively borrowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_function_set_specialized_name(function: *mut NmbFunction, name: *const c_char) -> bool {
    guard(false, || {
        let name = if name.is_null() { None } else { Some(unsafe { CStr::from_ptr(name) }.to_str().map_err(|_| "invalid function alias")?.to_string()) };
        let function = get_mut(function)?;
        function.0 = function.0.clone().with_specialized_name(name);
        Ok(true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_queue_new_with_max_command_buffer_count(device: *mut NmbDevice, count: usize) -> *mut NmbQueue {
    guard(std::ptr::null_mut(), || {
        let count = u32::try_from(count).map_err(e("command queue limit"))?;
        let queue = get(device)?.0.new_command_queue(count).map_err(e("command queue"))?;
        Ok(Box::into_raw(Box::new(NmbQueue(queue))))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_diagnostic_id(buffer: *const NmbCommandBuffer) -> u64 {
    guard(0, || Ok(get(buffer)?.2))
}
#[unsafe(no_mangle)]
pub extern "C" fn nmb_gpu_diagnostic_hook(hook: extern "C" fn(u32,u64,u64,u64)) {
    crate::trace::set_diagnostic_hook(hook);
}

fn scissor(r: &[u64]) -> Result<ScissorRect, String> {
    Ok(ScissorRect { x: r[0], y: r[1], width: r[2], height: r[3] })
}
trait MetalRaw: Sized { fn from_raw(raw: u64) -> Result<Self, String>; }
macro_rules! metal_raw {
    ($type:ty, {$($value:literal => $variant:ident),* $(,)?}) => {
        impl MetalRaw for $type {
            fn from_raw(raw: u64) -> Result<Self, String> {
                match raw { $($value => Ok(Self::$variant),)* _ => Err(format!("invalid {} value {raw}", stringify!($type))) }
            }
        }
    };
}
metal_raw!(LoadAction, {0 => DontCare, 1 => Load, 2 => Clear});
metal_raw!(StoreAction, {0 => DontCare, 1 => Store, 2 => MultisampleResolve, 3 => StoreAndMultisampleResolve, 4 => Unknown, 5 => CustomSampleDepthStore});
metal_raw!(MultisampleDepthResolveFilter, {0 => Sample0, 1 => Min, 2 => Max});
metal_raw!(MultisampleStencilResolveFilter, {0 => Sample0, 1 => DepthResolvedSample});
metal_raw!(VisibilityResultMode, {0 => Disabled, 1 => Boolean, 2 => Counting});

/// Version 2 followed by upstream cumulative counters, in the exact order declared in the header.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_device_runtime_metrics(device: *const NmbDevice, out: *mut u64) -> bool {
    guard(false, || {
        if out.is_null() { return Err("null runtime metrics output".into()); }
        if std::env::var_os("SHENG_DIAG").is_none_or(|value| value != "1") { return Ok(false); }
        let m = get(device)?.0.counters().metrics();
        let values = [2, m.submissions, m.submit_infos, m.submit_call_nanos, m.submit_call_max_nanos,
            m.queue_lock_wait_nanos, m.passes, m.barriers, m.bytes_copied, m.bytes_zeroed,
            m.pipelines_created, m.pipeline_creation_nanos, m.translations, m.translations_loaded,
            m.cache_bytes_written, m.cache_writes_dropped, m.over_copies_imported, m.over_copy_bytes_imported,
            m.allocations_freed, m.consumer_allocations, m.consumer_allocation_nanos,
            m.texture_allocations.bytes, m.texture_block_allocations.bytes,
            m.ring_block_allocations.bytes, m.staging_region_allocations.bytes, m.staging_buffer_allocations.bytes,
            m.shared_take_allocations.bytes, m.private_take_allocations.bytes,
            m.first_use_waits, m.first_use_wait_nanos, m.held_releases, m.held_release_bytes];
        // SAFETY: the ABI names 32 writable words.
        unsafe { std::ptr::copy_nonoverlapping(values.as_ptr(), out, values.len()); }
        Ok(true)
    })
}

#[derive(Default)]
struct PipelineOptions { flags: u64, binary_archives: Vec<BinaryArchive> }
impl PipelineOptions {
    fn check_compute(&self, device: &Device, descriptor: &ComputePipelineDescriptor) -> Result<(), String> {
        if self.flags & 4 != 0 && !self.binary_archives.iter().any(|archive| archive.contains_compute(device, descriptor)) {
            return Err("compute pipeline binary archive miss".into());
        }
        Ok(())
    }
    fn check_render(&self, device: &Device, descriptor: &RenderPipelineDescriptor) -> Result<(), String> {
        if self.flags & 4 != 0 && !self.binary_archives.iter().any(|archive| archive.contains_render(device, descriptor)) {
            return Err("render pipeline binary archive miss".into());
        }
        Ok(())
    }
}

/// Keeps consumer leases independently of Objective-C handle lifetime until GPU retirement.
#[unsafe(no_mangle)]
pub extern "C" fn nmb_command_buffer_keep(command: *const NmbCommandBuffer, resource: *const c_void, kind: u32) -> bool {
    guard(false, || {
        let owner: Owner = match kind {
            0 => get(resource.cast::<NmbBuffer>())?.0.live()?.ownership.clone(),
            1 => get(resource.cast::<NmbTexture>())?.0.live()?.ownership.clone(),
            2 => get(resource.cast::<NmbSampler>())?.1.clone(),
            3 => get(resource.cast::<NmbComputePipeline>())?.0.ownership.clone(),
            4 => get(resource.cast::<NmbRenderPipeline>())?.0.ownership.clone(),
            5 => Arc::new((*get(resource.cast::<NmbHeap>())?.0.live()?).clone()),
            _ => return Err("invalid consumer resource kind".into()),
        };
        get(command)?.3.lock().unwrap_or_else(|e| e.into_inner()).push(owner);
        Ok(true)
    })
}
