// SPDX-License-Identifier: MIT
//! Native Metal bridge: the metal2vulkan runtime (the Metal API on Vulkan) over Turnip, behind a C ABI the Objective-C
//! Metal frontend and the plugin's self-test call. Turnip is opened as the runtime's Vulkan "loader" (it exports
//! vkGetInstanceProcAddr; iPadOS has no Khronos loader). The pinned runtime is GPL-2.0-or-later OR LGPL-3.0-or-later and is
//! linked from its selected source with recorded consumer patches. The copied cache reader retains its license notice.
//!
//! The self-test runs real Metal-level work and checks the GPU's results: a compute kernel, a render pass, a render into
//! host pages imported without a copy (the IOSurface path), and translation of the target's own QuartzCore library into
//! A740 pipelines. Each step reports a marker before it starts so a hang names the step.

#[path = "NativeMetalSystemCache.rs"]
pub mod system_cache;

#[path = "NativeMetalTrace.rs"]
pub mod trace;

#[path = "NativeMetalHostMapping.rs"]
pub mod host_mapping;

#[path = "NativeMetalBufferArena.rs"]
pub mod buffer_arena;

#[path = "NativeMetalArchive.rs"]
pub mod archive;

#[path = "NativeMetalPipelines.rs"]
pub mod pipelines;

#[path = "NativeMetalCapabilities.rs"]
pub mod capabilities;

#[path = "NativeMetalHeap.rs"]
pub mod heap;

#[path = "NativeMetalTexture.rs"]
pub mod texture;

#[path = "NativeMetalResources.rs"]
pub mod resources;

#[path = "NativeMetalLibrary.rs"]
pub mod library;

#[path = "NativeMetalObjects.rs"]
pub mod objects;
#[path = "NativeMetalPurge.rs"]
pub mod purge;

// The system allocator: mimalloc reserved its arenas with large mmaps, and on iPadOS 27 the kernel stopped the render server and
// SpringBoard with an exception inside that mmap (unix_mmap_prim_aligned from mi_arena_reserve), so the shell resprung.

use metal2vulkan::metal::{PixelFormat, PrimitiveType, TextureUsage};
use metal2vulkan::pipeline::{ColorAttachmentDescriptor, RenderPipelineDescriptor, StageSource};
use metal2vulkan_runtime::{
    ClearColor, CommandBuffer, CommandBufferStatus, Device, DeviceOptions, LoadAction,
    RenderPassDescriptor, StoreAction,
};
use std::ffi::{c_char, c_void, CStr};
use crate::resources::{Buffer, ResourceOptions};
use crate::texture::{Texture, TextureDescriptor};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::time::Duration;

/// The runtime's own example library (validation/fixtures/public/examples/runtime_examples.ll), assembled to AIR at build time.
static FIXTURE_AIR: &[u8] = include_bytes!(env!("NMB_FIXTURE_AIR"));

unsafe extern "C" {
    /// libkern (libSystem): clean and invalidate the data cache over a range, from user space.
    fn sys_dcache_flush(start: *mut c_void, len: usize);
}

/// Result of nmb_selftest; mirrors NativeMetalBridge.h.
#[repr(C)]
pub struct NmbSelftest {
    /// 0 when every step ran; otherwise the marker of the step that failed.
    pub stage: u32,
    pub compute_bad: u32,
    pub render_bad: u32,
    pub import_bad: u32,
    pub qc_functions: u32,
    pub qc_kernels_ok: u32,
    pub qc_kernels_failed: u32,
    pub qc_pairs_ok: u32,
    pub qc_pairs_failed: u32,
    pub reserved: u32,
    pub message: [u8; 96],
}

type Mark = Option<unsafe extern "C" fn(u32)>;

struct Run<'a> {
    out: &'a mut NmbSelftest,
    mark: Mark,
}

impl Run<'_> {
    fn step(&mut self, marker: u32) {
        if let Some(mark) = self.mark {
            // SAFETY: the caller's callback, valid for the call.
            unsafe { mark(marker) };
        }
    }
    fn fail(&mut self, marker: u32, why: impl std::fmt::Display) -> u32 {
        self.out.stage = marker;
        let text = format!("{why}");
        let n = text.len().min(self.out.message.len() - 1);
        self.out.message[..n].copy_from_slice(&text.as_bytes()[..n]);
        self.out.message[n] = 0;
        marker
    }
}

const WAIT: Duration = Duration::from_secs(10);

fn finish(command_buffer: &CommandBuffer) -> Result<(), String> {
    command_buffer.commit().map_err(|e| format!("commit: {e:?}"))?;
    if !command_buffer.wait_until_completed_for(WAIT).map_err(|e| format!("completion wait: {e:?}"))? {
        return Err("no completion in 10 s".into());
    }
    match command_buffer.status() {
        CommandBufferStatus::Completed => Ok(()),
        status => Err(format!("{status:?} {:?}", command_buffer.error())),
    }
}

pub(crate) fn open(loader: &CStr, cache: &CStr) -> Result<(Device, std::sync::Arc<crate::archive::Registry>), String> {
    let loader = PathBuf::from(loader.to_str().map_err(|e| format!("loader path: {e}"))?);
    // SAFETY: the frontend gives the bundled Turnip library, which exports the Vulkan
    // instance entry points. The runtime keeps its dynamic-library handle alive.
    let instance = unsafe {
        metal2vulkan_runtime::Instance::with_drivers(Some(&loader), &[], &[])
    }.map_err(|e| format!("Vulkan instance: {e:?}"))?;
    let physicals = instance.physical_devices().map_err(|e| format!("Vulkan devices: {e:?}"))?;
    let mut refusals = Vec::new();
    for physical in physicals {
        if let Err(why) = physical.check() {
            refusals.push(format!("runtime floor: {why:?}"));
            continue;
        }
        let mut options = DeviceOptions::default();
        let registry = crate::archive::Registry::new(PathBuf::from(cache.to_str().map_err(|e| format!("cache path: {e}"))?));
        options.cache_lookup = Some(registry.clone());
        options.pipeline_cache_dir = Some(PathBuf::from(cache.to_str().map_err(|e| format!("cache path: {e}"))?));
        match Device::new(&physical, options) {
            Ok(device) => {
                // This frontend owns the full resource-ID namespace of its device.
                for class in [metal2vulkan_runtime::IndexClass::Texture, metal2vulkan_runtime::IndexClass::Sampler] {
                    let range = class.range();
                    device.grant_indices(class, range.start, range.end - range.start)
                        .map_err(|e| format!("resource-ID grant: {e:?}"))?;
                }
                registry.bind(&device);
                return Ok((device, registry));
            }
            Err(why) => refusals.push(format!("device creation: {why:?}")),
        }
    }
    Err(format!("no bundled Vulkan device meets the runtime contract: {}", refusals.join("; ")))
}

fn compute(device: &Device, library: &crate::library::Library) -> Result<u32, String> {
    const COUNT: usize = 1000;
    let function = library.new_function_with_name("scale_and_offset").map_err(|e| format!("{e:?}"))?;
    let state = crate::pipelines::compute(device, &metal2vulkan::pipeline::ComputePipelineDescriptor::new(StageSource::Function(function.function_descriptor())))
        .map_err(|e| format!("pipeline: {e:?}"))?;
    let input: Vec<u8> = (0..COUNT).flat_map(|i| (i as f32).to_le_bytes()).collect();
    let input = Buffer::allocate(device, (&input).len() as u64, ResourceOptions::shared(), Some(&input)).map_err(|e| format!("{e:?}"))?;
    let output = Buffer::allocate(device, (COUNT * 4) as u64, ResourceOptions::shared(), None).map_err(|e| format!("{e:?}"))?;
    let scale = Buffer::allocate(device, (&2.0f32.to_le_bytes()).len() as u64, ResourceOptions::shared(), Some(&2.0f32.to_le_bytes())).map_err(|e| format!("{e:?}"))?;
    let queue = device.new_command_queue(64).map_err(|e| format!("{e:?}"))?;
    let command_buffer = queue.command_buffer().map_err(|e| format!("{e:?}"))?;
    let mut encoder = command_buffer.compute_command_encoder(metal2vulkan_runtime::DispatchType::Serial).map_err(|e| format!("{e:?}"))?;
    encoder.set_compute_pipeline_state(&state).map_err(|e| format!("{e:?}"))?;
    encoder.set_buffer(Some(&input), 0, 0).map_err(|e| format!("{e:?}"))?;
    encoder.set_buffer(Some(&output), 0, 1).map_err(|e| format!("{e:?}"))?;
    encoder.set_buffer(Some(&scale), 0, 2).map_err(|e| format!("{e:?}"))?;
    encoder.dispatch_threads([COUNT as u32, 1, 1], [64, 1, 1]).map_err(|e| format!("{e:?}"))?;
    encoder.end_encoding();
    finish(&command_buffer)?;
    let contents = output.contents().ok_or("no contents")?;
    // SAFETY: the buffer holds COUNT floats and no command buffer writes it now.
    let values = unsafe { std::slice::from_raw_parts(contents.as_ptr() as *const f32, COUNT) };
    Ok(values.iter().enumerate().filter(|(i, v)| **v != 3.0 * *i as f32).count() as u32)
}

/// Draws the fixture's top-left-quarter quad into `target` (cleared blue) and returns the encoded command buffer's result.
fn draw_quad(device: &Device, library: &crate::library::Library, target: &metal2vulkan_runtime::Texture,
             readback: Option<(&metal2vulkan_runtime::Buffer, u64, u64)>) -> Result<(), String> {
    let vertex = library.new_function_with_name("colored_vertex").map_err(|e| format!("{e:?}"))?;
    let fragment = library.new_function_with_name("colored_fragment").map_err(|e| format!("{e:?}"))?;
    let mut descriptor = RenderPipelineDescriptor::new(StageSource::Function(vertex.function_descriptor()));
    descriptor.fragment = Some(StageSource::Function(fragment.function_descriptor()));
    descriptor.color_attachments[0] = Some(ColorAttachmentDescriptor::new(PixelFormat::RGBA8Unorm));
    let state = crate::pipelines::render(device, &descriptor).map_err(|e| format!("pipeline: {e:?}"))?;
    let positions: [[f32; 2]; 6] = [[-1.0, 1.0], [0.0, 1.0], [-1.0, 0.0], [0.0, 1.0], [0.0, 0.0], [-1.0, 0.0]];
    let positions: Vec<u8> = positions.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let colors: Vec<u8> = (0..6).flat_map(|_| [1.0f32, 0.2, 0.0, 1.0]).flat_map(|v| v.to_le_bytes()).collect();
    let positions = Buffer::allocate(device, (&positions).len() as u64, ResourceOptions::shared(), Some(&positions)).map_err(|e| format!("{e:?}"))?;
    let colors = Buffer::allocate(device, (&colors).len() as u64, ResourceOptions::shared(), Some(&colors)).map_err(|e| format!("{e:?}"))?;
    let mut pass = RenderPassDescriptor::default();
    let color = &mut pass.color_attachments[0];
    color.texture = Some(target.clone());
    color.load_action = LoadAction::Clear;
    color.store_action = StoreAction::Store;
    color.clear_color = ClearColor::new(0.0, 0.0, 1.0, 1.0);
    let queue = device.new_command_queue(64).map_err(|e| format!("{e:?}"))?;
    let command_buffer = queue.command_buffer().map_err(|e| format!("{e:?}"))?;
    let mut encoder = command_buffer.render_command_encoder(&pass).map_err(|e| format!("{e:?}"))?;
    encoder.set_render_pipeline_state(&state).map_err(|e| format!("{e:?}"))?;
    encoder.set_vertex_buffer(Some(&positions), 0, 0).map_err(|e| format!("{e:?}"))?;
    encoder.set_vertex_buffer(Some(&colors), 0, 1).map_err(|e| format!("{e:?}"))?;
    encoder.draw_primitives(PrimitiveType::Triangle, 0, 6, 1, 0).map_err(|e| format!("{e:?}"))?;
    encoder.end_encoding();
    if let Some((buffer, row, side)) = readback {
        let mut blit = command_buffer.blit_command_encoder().map_err(|e| format!("{e:?}"))?;
        blit.copy_from_texture_to_buffer(target, metal2vulkan_runtime::TexturePlace::default(),
            [u32::try_from(side).map_err(|_| "readback side exceeds u32")?, u32::try_from(side).map_err(|_| "readback side exceeds u32")?, 1], buffer,
            metal2vulkan_runtime::BufferLayout { offset: 0, bytes_per_row: row, bytes_per_image: row.checked_mul(side).ok_or("readback image pitch overflow")? }).map_err(|e| format!("{e:?}"))?;
        blit.end_encoding();
    }
    finish(&command_buffer)
}

/// Texels off the expected picture: the top-left quarter drawn (255, 51, 0, 255), the rest cleared blue.
fn wrong_texels(bytes: &[u8], row: usize, side: usize) -> u32 {
    let mut bad = 0;
    for y in 0..side {
        for x in 0..side {
            let at = y * row + x * 4;
            let inside = x < side / 2 && y < side / 2;
            let expected: [u8; 4] = if inside { [255, 51, 0, 255] } else { [0, 0, 255, 255] };
            if bytes[at..at + 4] != expected {
                bad += 1;
            }
        }
    }
    bad
}

fn render(device: &Device, library: &crate::library::Library) -> Result<u32, String> {
    const SIDE: u64 = 8;
    let mut texture = TextureDescriptor::texture_2d(PixelFormat::RGBA8Unorm, SIDE, SIDE, false);
    texture.usage = TextureUsage::RENDER_TARGET;
    texture.resource_options = ResourceOptions::private();
    let target = Texture::allocate(device, &texture).map_err(|e| format!("{e:?}"))?;
    let readback = Buffer::allocate(device, SIDE * 4 * SIDE, ResourceOptions::shared(), None).map_err(|e| format!("{e:?}"))?;
    draw_quad(device, library, &target, Some((&readback, SIDE * 4, SIDE)))?;
    let contents = readback.contents().ok_or("no contents")?;
    // SAFETY: SIDE rows of SIDE * 4 bytes, no longer written.
    let bytes = unsafe { std::slice::from_raw_parts(contents.as_ptr(), (SIDE * 4 * SIDE) as usize) };
    Ok(wrong_texels(bytes, (SIDE * 4) as usize, SIDE as usize))
}

/// The IOSurface path without an IOSurface: page-aligned host pages imported without a copy (the kernel pins them, Turnip maps
/// them into the GPU), a linear render target over them, CPU caches cleaned before and invalidated after the GPU's use.
fn import(device: &Device, library: &crate::library::Library) -> Result<u32, String> {
    const SIDE: u64 = 64;
    const ROW: u64 = 256;
    let bytes = (ROW * SIDE) as usize;
    // SAFETY: an anonymous private mapping of whole pages, unmapped below after every GPU use completed.
    let pages = unsafe {
        libc::mmap(std::ptr::null_mut(), bytes, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0)
    };
    if pages == libc::MAP_FAILED {
        return Err("mmap".into());
    }
    let result = (|| {
        // SAFETY: the pages were just mapped; nothing else uses them.
        unsafe { std::ptr::write_bytes(pages as *mut u8, 0x5a, bytes) };
        // SAFETY: the mapping is writable, page aligned and outlives the buffer (dropped before the unmap).
        // SAFETY: as above, for `bytes` bytes.
        let buffer = unsafe { Buffer::import(device, NonNull::new(pages as *mut u8).ok_or("null")?, bytes as u64, ResourceOptions::shared(), None) }
            .map_err(|e| format!("no-copy buffer: {e:?}"))?;
        let mut texture = TextureDescriptor::texture_2d(PixelFormat::RGBA8Unorm, SIDE, SIDE, false);
        texture.usage = TextureUsage::RENDER_TARGET | TextureUsage::SHADER_READ;
        texture.resource_options = ResourceOptions::shared();
        let target = buffer.new_texture(&texture, 0, ROW).map_err(|e| format!("texture over pages: {e:?}"))?;
        // SAFETY: the range is this mapping.
        unsafe { sys_dcache_flush(pages, bytes) };
        draw_quad(device, library, &target, None)?;
        // SAFETY: the GPU finished writing; drop any line the CPU fetched meanwhile.
        unsafe { sys_dcache_flush(pages, bytes) };
        // SAFETY: the pages hold `bytes` bytes and no command buffer writes them now.
        let view = unsafe { std::slice::from_raw_parts(pages as *const u8, bytes) };
        let bad = wrong_texels(view, ROW as usize, SIDE as usize);
        drop(target);
        drop(buffer);
        Ok(bad)
    })();
    // SAFETY: the buffer and texture over the pages are gone.
    unsafe { libc::munmap(pages, bytes) };
    result
}

/// The AIR slice of a universal metallib (a fat container, big endian, of an AIR64 metallib and an Apple-GPU binary, as the target's
/// frameworks ship them), as Metal itself picks it; a thin metallib is returned as it is.
pub fn air_slice(bytes: &[u8]) -> Option<&[u8]> {
    const AIR64: u32 = 0x0100_0017;
    let word = |at: usize| bytes.get(at..at + 4).map(|w| u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
    if word(0)? != 0xcafe_babe {
        return Some(bytes);
    }
    let count = word(4)? as usize;
    if count > (bytes.len() - 8) / 20 { return None; }
    for i in 0..count {
        let at = 8 + 20 * i;
        if word(at)? == AIR64 {
            let (offset, size) = (word(at + 8)? as usize, word(at + 12)? as usize);
            return bytes.get(offset..offset.checked_add(size)?);
        }
    }
    None
}

/// Translates the target's own QuartzCore library: every kernel into a compute pipeline, and the vertex/fragment pairs named
/// alike into render pipelines over a BGRA8 attachment. Counts only; refusals (function constants, tile functions) are expected.
fn quartzcore(run: &mut Run, device: &Device, bytes: &[u8]) -> Result<(), String> {
    let bytes = air_slice(bytes).ok_or("QC library: no AIR64 slice")?;
    let library = crate::library::Library::from_metallib(bytes).map_err(|e| format!("QC library: {e:?}"))?;
    let names = library.function_names();
    let list = std::env::var_os("NMB_LIST").is_some();
    run.out.qc_functions = names.len() as u32;
    let mut vertices = Vec::new();
    let mut fragments = Vec::new();
    for name in &names {
        let Ok(function) = library.new_function_with_name(name) else { continue };
        if list {
            eprintln!("fn {name} {:?}", function.function_type());
        }
        match function.function_type() {
            Ok(metal2vulkan::metal::FunctionType::Kernel) => {
                match crate::pipelines::compute(device, &metal2vulkan::pipeline::ComputePipelineDescriptor::new(StageSource::Function(function.function_descriptor()))) {
                    Ok(_) => run.out.qc_kernels_ok += 1,
                    Err(e) => {
                        run.out.qc_kernels_failed += 1;
                        if list {
                            eprintln!("kernel {name}: {e:?}");
                        }
                    }
                }
            }
            Ok(metal2vulkan::metal::FunctionType::Vertex) => vertices.push((function, name.clone())),
            Ok(metal2vulkan::metal::FunctionType::Fragment) => fragments.push((function, name.clone())),
            _ => {}
        }
    }
    // The library's own pairs: `<name>_vert_<precision>` with `<name>_frag_<precision>`.
    for (vertex, vname) in &vertices {
        let Some(fname) = [vname.replace("_vert_", "_frag_"), vname.replacen("_vert", "_frag", 1)]
            .into_iter()
            .find(|candidate| fragments.iter().any(|(_, n)| n == candidate))
        else {
            continue;
        };
        let fragment = &fragments.iter().find(|(_, n)| *n == fname).unwrap().0;
        let mut descriptor = RenderPipelineDescriptor::new(StageSource::Function(vertex.function_descriptor()));
        descriptor.fragment = Some(StageSource::Function(fragment.function_descriptor()));
        descriptor.color_attachments[0] = Some(ColorAttachmentDescriptor::new(PixelFormat::BGRA8Unorm));
        match crate::pipelines::render(device, &descriptor) {
            Ok(_) => run.out.qc_pairs_ok += 1,
            Err(e) => {
                run.out.qc_pairs_failed += 1;
                if list {
                    eprintln!("pair {vname} {fname}: {e:?}");
                }
            }
        }
    }
    Ok(())
}

fn selftest(run: &mut Run, loader: &CStr, cache: &CStr, qc: &[u8]) -> u32 {
    run.step(0x6501);
    let device = match open(loader, cache) {
        Ok((device, _registry)) => device,
        Err(why) => return run.fail(0x6501, why),
    };
    run.step(0x6502);
    let library = match crate::library::Library::from_air(FIXTURE_AIR) {
        Ok(library) => library,
        Err(why) => return run.fail(0x6502, format!("{why:?}")),
    };
    run.step(0x6503);
    match compute(&device, &library) {
        Ok(bad) => run.out.compute_bad = bad,
        Err(why) => return run.fail(0x6503, why),
    }
    run.step(0x6504);
    match render(&device, &library) {
        Ok(bad) => run.out.render_bad = bad,
        Err(why) => return run.fail(0x6504, why),
    }
    run.step(0x6505);
    match import(&device, &library) {
        Ok(bad) => run.out.import_bad = bad,
        Err(why) => return run.fail(0x6505, why),
    }
    if !qc.is_empty() {
        run.step(0x6506);
        if let Err(why) = quartzcore(run, &device, qc) {
            return run.fail(0x6506, why);
        }
    }
    run.step(0x650f);
    0
}

/// Runs the self-test; returns 0 or the marker of the failed step (also in out->stage, with out->message).
///
/// # Safety
/// `loader` and `cache` are C strings; `qc` points at `qc_len` bytes (or is null with 0); `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nmb_selftest(loader: *const c_char, cache: *const c_char, qc: *const u8, qc_len: usize, mark: Mark,
                                      out: *mut NmbSelftest) -> u32 {
    if loader.is_null() || cache.is_null() || out.is_null() {
        return 0x65ff;
    }
    // SAFETY: the caller's contract.
    let out = unsafe { &mut *out };
    // SAFETY: as above.
    let (loader, cache) = unsafe { (CStr::from_ptr(loader), CStr::from_ptr(cache)) };
    let qc: &[u8] = if qc.is_null() || qc_len == 0 { &[] } else { unsafe { std::slice::from_raw_parts(qc, qc_len) } };
    // SAFETY: plain integers and bytes.
    *out = unsafe { std::mem::zeroed() };
    let mut run = Run { out, mark };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| selftest(&mut run, loader, cache, qc))) {
        Ok(code) => code,
        Err(_) => run.fail(0x65fe, "panic"),
    }
}
