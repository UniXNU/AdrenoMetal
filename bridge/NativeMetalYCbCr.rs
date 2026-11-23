// SPDX-License-Identifier: MIT
//! Two-plane YCbCr IOSurface texture conversion over runtime v2 compute pipelines.

use crate::library::Library;
use crate::pipelines::ComputePipelineState;
use crate::resources::Buffer;
use crate::texture::{Texture, TextureDescriptor};
use metal2vulkan::metal::format::{ChromaSubsampling, YCbCrReads, ycbcr_format};
use metal2vulkan::metal::{PixelFormat, TextureSwizzleChannels, TextureType};
use metal2vulkan::pipeline::{ComputePipelineDescriptor, StageSource};
use metal2vulkan_runtime as rt;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The two-plane YCbCr conversion compute kernel (shaders/ycbcr_convert.ll), assembled to AIR at build time.
static YCBCR_CONVERT_AIR: &[u8] = include_bytes!(env!("NMB_YCBCR_AIR"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YCbCrMatrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl YCbCrMatrix {
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Bt709,
            2 => Self::Bt2020,
            _ => Self::Bt601,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YCbCrRange {
    Video,
    Full,
}

impl YCbCrRange {
    pub fn from_surface_format(ostype: u32) -> Self {
        match ostype {
            // CoreVideo full-range two-plane OSTypes: 420f, 422f, 444f, xf20, xf22, xf44, pf20, pf22, pf44, &f20, &f22, &f44.
            0x34323066 | 0x34323266 | 0x34343466
            | 0x78663230 | 0x78663232 | 0x78663434
            | 0x70663230 | 0x70663232 | 0x70663434
            | 0x26663230 | 0x26663232 | 0x26663434 => Self::Full,
            _ => Self::Video,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct YCbCrSurfaceLayout {
    pub luma_offset: u64,
    pub luma_bytes_per_row: u64,
    pub chroma_offset: u64,
    pub chroma_bytes_per_row: u64,
    pub ostype: u32,
    pub plane: u64,
    pub surface_width: u64,
    pub surface_height: u64,
    pub matrix: YCbCrMatrix,
}

#[derive(Clone)]
struct ConverterState {
    queue: rt::CommandQueue,
    pipeline: ComputePipelineState,
}

/// Lazily initialized compute queue and pipeline for converting two-plane YCbCr IOSurfaces.
pub struct YCbCrConverter {
    state: Mutex<Option<ConverterState>>,
    converted: AtomicU64,
}

impl YCbCrConverter {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(None),
            converted: AtomicU64::new(0),
        }
    }

    pub fn converted_count(&self) -> u64 {
        self.converted.load(Ordering::Relaxed)
    }

    fn state(&self, device: &rt::Device) -> Result<ConverterState, String> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = guard.as_ref() {
            return Ok(state.clone());
        }
        let library = Library::from_air(YCBCR_CONVERT_AIR)
            .map_err(|e| format!("ycbcr library: {e:?}"))?;
        let function = library
            .new_function_with_name("ycbcr_convert")
            .map_err(|e| format!("ycbcr function: {e:?}"))?;
        let pipeline = crate::pipelines::compute(
            device,
            &ComputePipelineDescriptor::new(StageSource::Function(function.function_descriptor())),
        )
        .map_err(|e| format!("ycbcr pipeline: {e:?}"))?;
        let queue = device
            .new_command_queue(64)
            .map_err(|e| format!("ycbcr queue: {e:?}"))?;
        let state = ConverterState { queue, pipeline };
        *guard = Some(state.clone());
        Ok(state)
    }
}

pub type SurfaceSeedFn = unsafe extern "C" fn(*const c_void) -> u32;

/// Callback reading the underlying IOSurface seed while the owning imported buffer stays alive.
#[derive(Clone, Copy)]
pub struct SurfaceSeed {
    func: SurfaceSeedFn,
    context: usize,
}

impl SurfaceSeed {
    /// # Safety
    /// `context` must stay valid for `func` for the entire lifetime of the owning `Buffer`.
    pub unsafe fn new(func: SurfaceSeedFn, context: *const c_void) -> Option<Self> {
        (!context.is_null()).then_some(Self {
            func,
            context: context as usize,
        })
    }

    pub fn read(self) -> u32 {
        // SAFETY: `YCbCrSource` retains the owning `Buffer` whose release callback retains the IOSurface.
        unsafe { (self.func)(self.context as *const c_void) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RefreshState {
    last_seed: Option<u32>,
    last_use_tag: u64,
    fresh_from_create: bool,
}

/// Pure refresh decision for a reused YCbCr texture backing.
///
/// - When an IOSurface seed is tracked (`last_seed` and `current_seed` are both `Some`),
///   reconversion runs only when the seed changes, avoiding redundant full-frame 4K compute
///   dispatches across command buffers on static video/wallpaper surfaces.
/// - When no seed callback is available (`None`), falls back to per-command-buffer refresh
///   (skipping the initial post-create bind/read and deduplicating within the same CB).
fn decide_refresh(state: &RefreshState, current_seed: Option<u32>, use_tag: u64) -> (bool, RefreshState) {
    let mut next = *state;
    if let (Some(prev), Some(curr)) = (state.last_seed, current_seed) {
        next.fresh_from_create = false;
        next.last_use_tag = use_tag;
        if prev != curr {
            next.last_seed = Some(curr);
            return (true, next);
        }
        return (false, next);
    }
    if use_tag == 0 {
        if state.fresh_from_create {
            next.fresh_from_create = false;
            return (false, next);
        }
        return (true, next);
    }
    if state.last_use_tag == use_tag {
        return (false, next);
    }
    if state.fresh_from_create {
        next.fresh_from_create = false;
        next.last_use_tag = use_tag;
        return (false, next);
    }
    next.last_use_tag = use_tag;
    (true, next)
}

/// Refresh handle shared by a converted YCbCr backing texture and its same-format views.
pub struct YCbCrRefresh {
    source: Arc<YCbCrSource>,
    backing_raw: rt::Texture,
    backing_ownership: Arc<crate::ownership::Ownership>,
    params: YCbCrParams,
    state: Mutex<RefreshState>,
}

impl YCbCrRefresh {
    pub fn source(&self) -> &Arc<YCbCrSource> {
        &self.source
    }

    /// Re-dispatches YCbCr->RGBA conversion into the existing backing image when the surface
    /// seed changed or the texture is reused in a new command buffer / CPU readback.
    pub fn refresh_for_use(&self, use_tag: u64) -> Result<(), String> {
        let mut guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let current_seed = self.source.seed.map(SurfaceSeed::read);
        let (needs_refresh, next_state) = decide_refresh(&guard, current_seed, use_tag);
        if needs_refresh {
            let converter_state = self.source.converter.state(self.source.buffer.device())?;
            self.source.dispatch_into(
                &converter_state,
                &self.backing_raw,
                &self.backing_ownership,
                self.params,
            )?;
        }
        *guard = next_state;
        Ok(())
    }
}

/// Source surface metadata retained by converted YCbCr textures so views in sibling YCbCr formats can reconvert.
#[derive(Clone)]
pub struct YCbCrSource {
    converter: Arc<YCbCrConverter>,
    buffer: Buffer,
    layout: YCbCrSurfaceLayout,
    seed: Option<SurfaceSeed>,
}

impl YCbCrSource {
    pub fn new(
        converter: Arc<YCbCrConverter>,
        buffer: Buffer,
        layout: YCbCrSurfaceLayout,
        seed: Option<SurfaceSeed>,
    ) -> Arc<Self> {
        Arc::new(Self {
            converter,
            buffer,
            layout,
            seed,
        })
    }

    fn dispatch_into(
        &self,
        state: &ConverterState,
        target: &rt::Texture,
        target_ownership: &Arc<crate::ownership::Ownership>,
        params: YCbCrParams,
    ) -> Result<(), String> {
        let command_buffer = state
            .queue
            .command_buffer()
            .map_err(|e| format!("ycbcr command buffer: {e:?}"))?;
        let mut encoder = command_buffer
            .compute_command_encoder(rt::DispatchType::Serial)
            .map_err(|e| format!("ycbcr compute encoder: {e:?}"))?;
        encoder
            .set_compute_pipeline_state(&state.pipeline)
            .map_err(|e| format!("ycbcr pipeline bind: {e:?}"))?;
        encoder
            .set_buffer(Some(&*self.buffer), 0, 0)
            .map_err(|e| format!("ycbcr source bind: {e:?}"))?;
        encoder
            .set_bytes(&params.to_le_bytes(), 1)
            .map_err(|e| format!("ycbcr params bind: {e:?}"))?;
        encoder
            .set_texture(Some(target), 0)
            .map_err(|e| format!("ycbcr target bind: {e:?}"))?;
        encoder
            .dispatch_threads([params.width, params.height, 1], [16, 16, 1])
            .map_err(|e| format!("ycbcr dispatch: {e:?}"))?;
        encoder.end_encoding();

        // Keep source pages, target image, and pipeline alive until the GPU conversion finishes.
        // Runtime v2 orders submissions across queues on the same device by commit order and inserts
        // the image write-to-read barrier automatically before any subsequent draw reads `target`.
        let owners = vec![
            self.buffer.ownership.clone(),
            target_ownership.clone(),
            state.pipeline.ownership.clone(),
        ];
        command_buffer
            .add_completed_handler(move |_| drop(owners))
            .map_err(|e| format!("ycbcr completion handler: {e:?}"))?;
        command_buffer
            .commit()
            .map_err(|e| format!("ycbcr commit: {e:?}"))?;
        self.converter.converted.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn convert(self: &Arc<Self>, descriptor: &TextureDescriptor) -> Result<Texture, String> {
        let params = build_params(descriptor, &self.layout, self.buffer.length())?;
        let device = self.buffer.device();
        let state = self.converter.state(device)?;
        let (raw, ownership, backing_size) = Texture::allocate_ycbcr_target(device, descriptor)?;
        let initial_seed = self.seed.map(SurfaceSeed::read);
        self.dispatch_into(&state, &raw, &ownership, params)?;

        let refresh = Arc::new(YCbCrRefresh {
            source: Arc::clone(self),
            backing_raw: raw.clone(),
            backing_ownership: ownership.clone(),
            params,
            state: Mutex::new(RefreshState {
                last_seed: initial_seed,
                last_use_tag: 0,
                fresh_from_create: true,
            }),
        });
        let backing = Texture::from_ycbcr_backing(
            device,
            descriptor,
            raw,
            ownership,
            backing_size,
            refresh,
        );

        if descriptor.swizzle != TextureSwizzleChannels::DEFAULT {
            backing.new_texture_view_with_swizzle(
                descriptor.pixel_format,
                descriptor.texture_type,
                0..descriptor.mipmap_level_count,
                0..descriptor.array_length,
                descriptor.swizzle,
            )
        } else {
            Ok(backing)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct YCbCrParams {
    width: u32,
    height: u32,
    luma_offset: u32,
    luma_row_bytes: u32,
    chroma_offset: u32,
    chroma_row_bytes: u32,
    chroma_w: u32,
    chroma_h: u32,
    flags: u32,
    kb_r: i32,
    kr_r: i32,
    kb_g: i32,
    kr_g: i32,
    kb_b: i32,
    kr_b: i32,
}

impl YCbCrParams {
    fn to_le_bytes(self) -> [u8; 64] {
        let mut out = [0u8; 64];
        let words: [u32; 16] = [
            self.width,
            self.height,
            self.luma_offset,
            self.luma_row_bytes,
            self.chroma_offset,
            self.chroma_row_bytes,
            self.chroma_w,
            self.chroma_h,
            self.flags,
            self.kb_r as u32,
            self.kr_r as u32,
            self.kb_g as u32,
            self.kr_g as u32,
            self.kb_b as u32,
            self.kr_b as u32,
            0,
        ];
        for (i, w) in words.into_iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

/// Exact Metal fixed-point `(kb, kr)` coefficients matching `metal2vulkan::bind::ycbcr_convert`.
fn coefficients(range: YCbCrRange, matrix: YCbCrMatrix) -> [(i32, i32); 3] {
    match (range, matrix) {
        (YCbCrRange::Video, YCbCrMatrix::Bt601) => [(-9, 13123), (-3218, -6686), (16591, 9)],
        (YCbCrRange::Video, YCbCrMatrix::Bt709) => [(-1, 14743), (-1753, -4383), (17373, 1)],
        (YCbCrRange::Full, YCbCrMatrix::Bt601) => [(-8, 11483), (-2816, -5850), (14518, 8)],
        (YCbCrRange::Full, YCbCrMatrix::Bt709) => [(-1, 12900), (-1534, -3835), (15201, 1)],
        (YCbCrRange::Video, YCbCrMatrix::Bt2020) => [(0, 13806), (-1540, -5349), (17614, -1)],
        (YCbCrRange::Full, YCbCrMatrix::Bt2020) => [(0, 12080), (-1348, -4680), (15412, -1)],
    }
}

fn row_used_bytes(codes: u64, bits_mode: u32) -> Result<u64, String> {
    match bits_mode {
        0 => Ok(codes),
        1 => codes.checked_mul(2).ok_or_else(|| "ycbcr row overflow".into()),
        2 => codes
            .div_ceil(3)
            .checked_mul(4)
            .ok_or_else(|| "ycbcr packed row overflow".into()),
        _ => Err("unsupported ycbcr bit mode".into()),
    }
}

fn check_plane_bounds(
    name: &str,
    offset: u64,
    row_bytes: u64,
    rows: u64,
    used_row_bytes: u64,
    buffer_len: u64,
) -> Result<(), String> {
    if rows == 0 || used_row_bytes == 0 || row_bytes < used_row_bytes {
        return Err(format!(
            "ycbcr {name} plane layout invalid: rows={rows} row_bytes={row_bytes} used={used_row_bytes}"
        ));
    }
    let span = (rows - 1)
        .checked_mul(row_bytes)
        .and_then(|v| v.checked_add(used_row_bytes))
        .and_then(|v| offset.checked_add(v))
        .ok_or_else(|| format!("ycbcr {name} plane range overflow"))?;
    let aligned_end = span
        .checked_next_multiple_of(4)
        .ok_or_else(|| format!("ycbcr {name} plane alignment overflow"))?;
    if aligned_end > buffer_len {
        return Err(format!(
            "ycbcr {name} plane exceeds buffer: end={aligned_end} buffer_len={buffer_len}"
        ));
    }
    Ok(())
}

fn build_params(
    descriptor: &TextureDescriptor,
    layout: &YCbCrSurfaceLayout,
    buffer_len: u64,
) -> Result<YCbCrParams, String> {
    if descriptor.texture_type != TextureType::Type2D {
        return Err("two-plane YCbCr textures require TextureType::Type2D".into());
    }
    if layout.plane != 0 {
        return Err(format!(
            "two-plane YCbCr surface textures require plane 0 (got plane {})",
            layout.plane
        ));
    }
    if descriptor.width == 0
        || descriptor.height == 0
        || descriptor.depth != 1
        || descriptor.mipmap_level_count != 1
        || descriptor.array_length != 1
        || descriptor.sample_count != 1
    {
        return Err("two-plane YCbCr textures require a single-level 2D single-sample descriptor".into());
    }
    if layout.chroma_bytes_per_row == 0 {
        return Err(format!(
            "two-plane YCbCr surface missing chroma plane: format={:#x} surface={}x{}",
            layout.ostype, layout.surface_width, layout.surface_height
        ));
    }
    let fmt = ycbcr_format(descriptor.pixel_format).ok_or_else(|| {
        format!(
            "unsupported YCbCr pixel format {:?}",
            descriptor.pixel_format
        )
    })?;
    let bits_mode: u32 = match (fmt.bits, fmt.packed) {
        (8, false) => 0,
        (10, false) => 1,
        (10, true) => 2,
        _ => {
            return Err(format!(
                "unsupported YCbCr bit layout: bits={} packed={}",
                fmt.bits, fmt.packed
            ));
        }
    };
    let (subsampling_code, halved): (u32, [bool; 2]) = match fmt.subsampling {
        ChromaSubsampling::Chroma420 => (0, [true, true]),
        ChromaSubsampling::Chroma422 => (1, [true, false]),
        ChromaSubsampling::Chroma444 => (2, [false, false]),
        _ => return Err("unsupported YCbCr chroma subsampling".into()),
    };
    let output_code: u32 = match fmt.reads {
        YCbCrReads::Converted => 0,
        YCbCrReads::Srgb => 1,
        YCbCrReads::Codes => 2,
        YCbCrReads::ExtendedRange => 3,
        YCbCrReads::Pq => 4,
        YCbCrReads::Hlg => 5,
        _ => return Err("unsupported YCbCr output transfer".into()),
    };

    let chroma_w = if halved[0] {
        descriptor.width.div_ceil(2)
    } else {
        descriptor.width
    };
    let chroma_h = if halved[1] {
        descriptor.height.div_ceil(2)
    } else {
        descriptor.height
    };

    let luma_used = row_used_bytes(descriptor.width, bits_mode)?;
    let chroma_codes = chroma_w
        .checked_mul(2)
        .ok_or_else(|| "ycbcr chroma code count overflow".to_string())?;
    let chroma_used = row_used_bytes(chroma_codes, bits_mode)?;

    check_plane_bounds(
        "luma",
        layout.luma_offset,
        layout.luma_bytes_per_row,
        descriptor.height,
        luma_used,
        buffer_len,
    )?;
    check_plane_bounds(
        "chroma",
        layout.chroma_offset,
        layout.chroma_bytes_per_row,
        chroma_h,
        chroma_used,
        buffer_len,
    )?;

    let to_u32 = |v: u64, what: &str| {
        u32::try_from(v).map_err(|_| format!("ycbcr {what} ({v}) exceeds u32"))
    };
    let range = YCbCrRange::from_surface_format(layout.ostype);
    let range_bit: u32 = match range {
        YCbCrRange::Video => 0,
        YCbCrRange::Full => 1,
    };
    let [(kb_r, kr_r), (kb_g, kr_g), (kb_b, kr_b)] = coefficients(range, layout.matrix);
    let flags = bits_mode | (subsampling_code << 2) | (range_bit << 4) | (output_code << 5);

    Ok(YCbCrParams {
        width: to_u32(descriptor.width, "width")?,
        height: to_u32(descriptor.height, "height")?,
        luma_offset: to_u32(layout.luma_offset, "luma_offset")?,
        luma_row_bytes: to_u32(layout.luma_bytes_per_row, "luma_bytes_per_row")?,
        chroma_offset: to_u32(layout.chroma_offset, "chroma_offset")?,
        chroma_row_bytes: to_u32(layout.chroma_bytes_per_row, "chroma_bytes_per_row")?,
        chroma_w: to_u32(chroma_w, "chroma_w")?,
        chroma_h: to_u32(chroma_h, "chroma_h")?,
        flags,
        kb_r,
        kr_r,
        kb_g,
        kr_g,
        kb_b,
        kr_b,
    })
}

pub fn is_ycbcr_pixel_format(format: PixelFormat) -> bool {
    metal2vulkan::metal::format::ycbcr(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p010_x420_4k_surface_builds_valid_params() {
        let mut desc = TextureDescriptor::texture_2d(PixelFormat::YCBCR104202P, 3840, 2160, false);
        desc.usage = metal2vulkan::metal::TextureUsage::SHADER_READ;
        let layout = YCbCrSurfaceLayout {
            luma_offset: 0,
            luma_bytes_per_row: 7680,
            chroma_offset: 16_715_776,
            chroma_bytes_per_row: 7680,
            ostype: 0x78343230, // 'x420'
            plane: 0,
            surface_width: 3840,
            surface_height: 2160,
            matrix: YCbCrMatrix::Bt709,
        };
        let buffer_len = 16_715_776 + 7680 * 1080;
        let params = build_params(&desc, &layout, buffer_len).unwrap();
        assert_eq!(params.width, 3840);
        assert_eq!(params.height, 2160);
        assert_eq!(params.chroma_w, 1920);
        assert_eq!(params.chroma_h, 1080);
        assert_eq!(params.flags & 3, 1); // 10-bit unpacked
        assert_eq!((params.flags >> 2) & 3, 0); // 4:2:0
        assert_eq!((params.flags >> 4) & 1, 0); // Video range
        assert_eq!((params.flags >> 5) & 7, 0); // Converted
        assert_eq!((params.kb_r, params.kr_r), (-1, 14743));
        assert_eq!(params.to_le_bytes().len(), 64);
    }

    #[test]
    fn rejects_truncated_chroma_plane() {
        let desc = TextureDescriptor::texture_2d(PixelFormat::YCBCR84202P, 64, 32, false);
        let layout = YCbCrSurfaceLayout {
            luma_offset: 0,
            luma_bytes_per_row: 64,
            chroma_offset: 64 * 32,
            chroma_bytes_per_row: 64,
            ostype: 0x34323076, // '420v'
            plane: 0,
            surface_width: 64,
            surface_height: 32,
            matrix: YCbCrMatrix::Bt601,
        };
        let exact = 64 * 32 + 64 * 16;
        assert!(build_params(&desc, &layout, exact).is_ok());
        assert!(build_params(&desc, &layout, exact - 4).is_err());
    }

    #[test]
    fn full_range_ostypes_map_to_full() {
        assert_eq!(YCbCrRange::from_surface_format(0x34323066), YCbCrRange::Full); // 420f
        assert_eq!(YCbCrRange::from_surface_format(0x78663230), YCbCrRange::Full); // xf20
        assert_eq!(YCbCrRange::from_surface_format(0x78343230), YCbCrRange::Video); // x420
        assert_eq!(YCbCrRange::from_surface_format(0x34323076), YCbCrRange::Video); // 420v
    }

    #[test]
    fn refresh_policy_handles_create_dedup_seed_bump_and_reuse() {
        let s0 = RefreshState {
            last_seed: Some(10),
            last_use_tag: 0,
            fresh_from_create: true,
        };
        // Unchanged IOSurface seed across CB1 -> CB2 -> CB3 triggers zero redundant conversions.
        let (go, s1) = decide_refresh(&s0, Some(10), 1);
        assert!(!go);
        assert!(!s1.fresh_from_create);
        assert_eq!(s1.last_use_tag, 1);
        let (go, s2) = decide_refresh(&s1, Some(10), 2);
        assert!(!go);
        assert_eq!(s2.last_use_tag, 2);
        let (go, s3) = decide_refresh(&s2, Some(10), 3);
        assert!(!go);
        assert_eq!(s3.last_use_tag, 3);
        // Seed bump triggers immediate reconversion even within the same CB.
        let (go, s4) = decide_refresh(&s3, Some(11), 3);
        assert!(go);
        assert_eq!(s4.last_seed, Some(11));
        // Subsequent CB with same new seed stays clean.
        let (go, s5) = decide_refresh(&s4, Some(11), 4);
        assert!(!go);
        assert_eq!(s5.last_seed, Some(11));
        // CPU get_bytes skips unchanged seed and refreshes immediately on seed bump.
        let (go, g1) = decide_refresh(&s0, Some(10), 0);
        assert!(!go);
        let (go, _) = decide_refresh(&g1, Some(10), 0);
        assert!(!go);
        let (go, _) = decide_refresh(&s0, Some(12), 0);
        assert!(go);

        // Unseeded fallback keeps per-CB refresh and same-CB deduplication.
        let u0 = RefreshState {
            last_seed: None,
            last_use_tag: 0,
            fresh_from_create: true,
        };
        let (go, u1) = decide_refresh(&u0, None, 1);
        assert!(!go);
        let (go, u2) = decide_refresh(&u1, None, 1);
        assert!(!go);
        let (go, u3) = decide_refresh(&u2, None, 2);
        assert!(go);
        let (go, _) = decide_refresh(&u3, None, 2);
        assert!(!go);
    }
}
