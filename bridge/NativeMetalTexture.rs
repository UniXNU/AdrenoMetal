// SPDX-License-Identifier: MIT
//! Metal texture metadata and ownership over runtime-v2 images and buffer views.

use crate::resources::{Buffer, PurgeableState, ResourceOptions, StorageMode};
use metal2vulkan::metal::{PixelFormat, TextureSwizzleChannels, TextureType, TextureUsage};
use metal2vulkan_runtime as rt;
use std::ops::{Deref, Range};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Debug)]
pub struct TextureDescriptor {
    pub texture_type: TextureType,
    pub pixel_format: PixelFormat,
    pub width: u64,
    pub height: u64,
    pub depth: u64,
    pub mipmap_level_count: u64,
    pub sample_count: u64,
    pub array_length: u64,
    pub usage: TextureUsage,
    pub resource_options: ResourceOptions,
    pub rotation: u64,
    pub framebuffer_only: bool,
    pub is_drawable: bool,
    pub swizzle: TextureSwizzleChannels,
    pub allow_gpu_optimized_contents: bool,
}
impl Default for TextureDescriptor {
    fn default() -> Self {
        Self {
            texture_type: TextureType::Type2D,
            pixel_format: PixelFormat::RGBA8Unorm,
            width: 1,
            height: 1,
            depth: 1,
            mipmap_level_count: 1,
            sample_count: 1,
            array_length: 1,
            usage: TextureUsage::SHADER_READ,
            resource_options: ResourceOptions::shared(),
            rotation: 0,
            framebuffer_only: false,
            is_drawable: false,
            swizzle: TextureSwizzleChannels {
                red: metal2vulkan::metal::TextureSwizzle::Red,
                green: metal2vulkan::metal::TextureSwizzle::Green,
                blue: metal2vulkan::metal::TextureSwizzle::Blue,
                alpha: metal2vulkan::metal::TextureSwizzle::Alpha,
            },
            allow_gpu_optimized_contents: true,
        }
    }
}
impl TextureDescriptor {
    pub fn texture_2d(pixel_format: PixelFormat, width: u64, height: u64, mipmapped: bool) -> Self {
        let levels = if mipmapped {
            u64::BITS - width.max(height).leading_zeros()
        } else {
            1
        };
        Self {
            pixel_format,
            width,
            height,
            mipmap_level_count: levels as u64,
            ..Self::default()
        }
    }
    pub fn runtime(&self) -> Result<rt::TextureDescriptor, String> {
        if self.rotation != 0 {
            return Err("texture rotation needs the consumer's coordinate transform".into());
        }
        let narrow =
            |value| u32::try_from(value).map_err(|_| "texture dimension exceeds u32".to_owned());
        let mut d = rt::TextureDescriptor::new(
            self.texture_type,
            self.pixel_format,
            narrow(self.width)?,
            narrow(self.height)?,
        );
        d.depth = narrow(self.depth)?;
        d.mipmap_level_count = narrow(self.mipmap_level_count)?;
        d.sample_count = narrow(self.sample_count)?;
        d.array_length = narrow(self.array_length)?;
        d.usage = self.usage;
        d.swizzle = self.swizzle;
        // Upstream runtime v2 refuses StorageMode::Memoryless (Gap::MemorylessTexture).
        // Back memoryless descriptors with pooled Private storage while preserving Memoryless
        // in Metal metadata; Private still refuses CPU get_bytes/replace_region access.
        d.storage_mode = match self.resource_options.storage_mode() {
            StorageMode::Shared => rt::StorageMode::Shared,
            StorageMode::Managed => rt::StorageMode::Managed,
            StorageMode::Private | StorageMode::Memoryless => rt::StorageMode::Private,
        };
        Ok(d)
    }
}

#[derive(Clone)]
pub struct Texture {
    raw: rt::Texture,
    descriptor: TextureDescriptor,
    device: rt::Device,
    state: Arc<AtomicU64>,
    lease: Option<Arc<crate::heap::Lease>>,
    pub(crate) ownership: Arc<crate::ownership::Ownership>,
    backing_size: u64,
    backing: Arc<crate::ownership::Ownership>,
    ycbcr: Option<Arc<crate::ycbcr::YCbCrRefresh>>,
}
impl Deref for Texture {
    type Target = rt::Texture;
    fn deref(&self) -> &rt::Texture {
        &self.raw
    }
}
impl Texture {
    pub(crate) fn placed(
        raw: rt::Texture,
        descriptor: TextureDescriptor,
        device: rt::Device,
        lease: Arc<crate::heap::Lease>,
        index: Option<crate::ownership::Reservation>,
        backing_size: u64,
    ) -> Self {
        let backing = crate::ownership::Ownership::new(vec![lease.clone()]);
        let mut owners: Vec<crate::ownership::Owner> = vec![backing.clone()];
        if let Some(index) = index {
            owners.push(index.owner());
        }
        Self {
            raw,
            descriptor,
            device,
            state: Arc::new(AtomicU64::new(2)),
            ownership: crate::ownership::Ownership::new(owners),
            backing,
            backing_size,
            lease: Some(lease),
            ycbcr: None,
        }
    }
    pub fn make_aliasable(&self) -> Result<(), String> {
        self.lease
            .as_ref()
            .ok_or("only heap resources can be made aliasable")?
            .make_aliasable();
        Ok(())
    }
    pub fn is_aliasable(&self) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| lease.is_aliasable())
    }
    pub fn allocate(device: &rt::Device, descriptor: &TextureDescriptor) -> Result<Self, String> {
        let runtime = descriptor.runtime()?;
        let index = if texture_needs_index(runtime.pixel_format, runtime.usage) {
            Some(crate::ownership::reserve(device, rt::IndexClass::Texture)?)
        } else {
            None
        };
        let raw = device
            .new_texture(&runtime, index.as_ref().map(|r| r.index()))
            .map_err(|e| format!("texture: {e:?}"))?;
        let owners = index.into_iter().map(|r| r.owner()).collect();
        Ok(Self {
            raw,
            descriptor: descriptor.clone(),
            device: device.clone(),
            state: Arc::new(AtomicU64::new(2)),
            ownership: crate::ownership::Ownership::new(owners),
            backing: crate::ownership::Ownership::new(vec![]),
            backing_size: allocation_size(device, descriptor),
            lease: None,
            ycbcr: None,
        })
    }
    pub(crate) fn allocate_ycbcr_target(
        device: &rt::Device,
        descriptor: &TextureDescriptor,
    ) -> Result<(rt::Texture, Arc<crate::ownership::Ownership>, u64), String> {
        let mut backing_desc = TextureDescriptor::texture_2d(
            PixelFormat::RGBA16Float,
            descriptor.width,
            descriptor.height,
            false,
        );
        backing_desc.resource_options = ResourceOptions::private();
        backing_desc.usage = TextureUsage::SHADER_READ
            | TextureUsage::SHADER_WRITE
            | (descriptor.usage & TextureUsage::PIXEL_FORMAT_VIEW);
        let index = crate::ownership::reserve(device, rt::IndexClass::Texture)?;
        let raw = device
            .new_texture(&backing_desc.runtime()?, Some(index.index()))
            .map_err(|e| format!("ycbcr backing texture: {e:?}"))?;
        let backing_size = allocation_size(device, &backing_desc);
        let ownership = crate::ownership::Ownership::new(vec![index.owner()]);
        Ok((raw, ownership, backing_size))
    }

    pub(crate) fn from_ycbcr_backing(
        device: &rt::Device,
        descriptor: &TextureDescriptor,
        raw: rt::Texture,
        ownership: Arc<crate::ownership::Ownership>,
        backing_size: u64,
        ycbcr: Arc<crate::ycbcr::YCbCrRefresh>,
    ) -> Self {
        let mut reported = descriptor.clone();
        reported.swizzle = TextureSwizzleChannels::DEFAULT;
        Self {
            raw,
            descriptor: reported,
            device: device.clone(),
            state: Arc::new(AtomicU64::new(2)),
            backing: ownership.clone(),
            ownership,
            backing_size,
            lease: None,
            ycbcr: Some(ycbcr),
        }
    }

    pub fn refresh_ycbcr_for_use(&self, use_tag: u64) -> Result<(), String> {
        if let Some(ref ycbcr) = self.ycbcr {
            ycbcr.refresh_for_use(use_tag)?;
        }
        Ok(())
    }
    pub fn over_buffer(
        buffer: &Buffer,
        descriptor: &TextureDescriptor,
        offset: u64,
        bytes_per_row: u64,
    ) -> Result<Self, String> {
        let device = buffer.device();
        let runtime = descriptor.runtime()?;
        let index = if texture_needs_index(runtime.pixel_format, runtime.usage) {
            Some(crate::ownership::reserve(device, rt::IndexClass::Texture)?)
        } else {
            None
        };
        let raw = (**buffer)
            .new_texture(
                &runtime,
                index.as_ref().map(|r| r.index()),
                offset,
                bytes_per_row,
            )
            .map_err(|e| format!("texture over buffer: {e:?}"))?;
        let mut owners: Vec<crate::ownership::Owner> = Vec::with_capacity(2);
        if let Some(index) = index {
            owners.push(index.owner());
        }
        owners.push(buffer.ownership.clone());
        Ok(Self {
            raw,
            descriptor: descriptor.clone(),
            device: device.clone(),
            state: Arc::new(AtomicU64::new(2)),
            ownership: crate::ownership::Ownership::new(owners),
            backing_size: buffer.length(),
            backing: crate::ownership::Ownership::new(vec![buffer.ownership.clone()]),
            lease: None,
            ycbcr: None,
        })
    }
    pub fn new_texture_view(&self, format: PixelFormat) -> Result<Self, String> {
        let d = self.raw.descriptor();
        let layers = if matches!(d.texture_type, TextureType::Cube | TextureType::CubeArray) {
            d.array_length
                .checked_mul(6)
                .ok_or("texture slice count overflow")?
        } else {
            d.array_length
        };
        self.new_texture_view_with_levels_slices(
            format,
            d.texture_type,
            0..d.mipmap_level_count as u64,
            0..layers as u64,
        )
    }
    pub fn new_texture_view_with_levels_slices(
        &self,
        format: PixelFormat,
        kind: TextureType,
        levels: Range<u64>,
        slices: Range<u64>,
    ) -> Result<Self, String> {
        self.new_texture_view_with_swizzle(format, kind, levels, slices, self.descriptor.swizzle)
    }
    pub fn new_texture_view_with_swizzle(
        &self,
        format: PixelFormat,
        kind: TextureType,
        levels: Range<u64>,
        slices: Range<u64>,
        swizzle: TextureSwizzleChannels,
    ) -> Result<Self, String> {
        if let Some(ref ycbcr) = self.ycbcr {
            if format != self.descriptor.pixel_format && crate::ycbcr::is_ycbcr_pixel_format(format)
            {
                if kind != TextureType::Type2D || levels != (0..1) || slices != (0..1) {
                    return Err("YCbCr sibling views require whole single-level 2D range".into());
                }
                let mut view_desc = self.descriptor.clone();
                view_desc.pixel_format = format;
                view_desc.swizzle = swizzle;
                return ycbcr.source().convert(&view_desc);
            }
        }
        let runtime_format = if self.ycbcr.is_some() && format == self.descriptor.pixel_format {
            PixelFormat::RGBA16Float
        } else {
            format
        };
        let narrow = |v| u32::try_from(v).map_err(|_| "texture view range exceeds u32".to_owned());
        let index = if texture_needs_index(runtime_format, self.raw.descriptor().usage) {
            Some(crate::ownership::reserve(&self.device, rt::IndexClass::Texture)?)
        } else {
            None
        };
        let raw = self
            .raw
            .new_texture_view_with_swizzle(
                runtime_format,
                kind,
                narrow(levels.start)?..narrow(levels.end)?,
                narrow(slices.start)?..narrow(slices.end)?,
                swizzle,
                index.as_ref().map(|r| r.index()),
            )
            .map_err(|e| format!("texture view: {e:?}"))?;
        let runtime = raw.descriptor();
        let mut descriptor = self.descriptor.clone();
        descriptor.texture_type = runtime.texture_type;
        descriptor.pixel_format = if self.ycbcr.is_some() && format == self.descriptor.pixel_format
        {
            format
        } else {
            runtime.pixel_format
        };
        descriptor.width = runtime.width as u64;
        descriptor.height = runtime.height as u64;
        descriptor.depth = runtime.depth as u64;
        descriptor.array_length = runtime.array_length as u64;
        descriptor.mipmap_level_count = runtime.mipmap_level_count as u64;
        descriptor.swizzle = runtime.swizzle;
        let mut owners: Vec<crate::ownership::Owner> = Vec::with_capacity(2);
        if let Some(index) = index {
            owners.push(index.owner());
        }
        owners.push(self.backing.clone());
        Ok(Self {
            raw,
            descriptor,
            device: self.device.clone(),
            state: self.state.clone(),
            ownership: crate::ownership::Ownership::new(owners),
            backing: self.backing.clone(),
            backing_size: self.backing_size,
            lease: self.lease.clone(),
            ycbcr: self.ycbcr.clone(),
        })
    }
    pub fn texture_type(&self) -> TextureType {
        self.descriptor.texture_type
    }
    pub fn pixel_format(&self) -> PixelFormat {
        self.descriptor.pixel_format
    }
    pub fn width(&self) -> u64 {
        self.descriptor.width
    }
    pub fn height(&self) -> u64 {
        self.descriptor.height
    }
    pub fn depth(&self) -> u64 {
        self.descriptor.depth
    }
    pub fn mipmap_level_count(&self) -> u64 {
        self.descriptor.mipmap_level_count
    }
    pub fn sample_count(&self) -> u64 {
        self.descriptor.sample_count
    }
    pub fn array_length(&self) -> u64 {
        self.descriptor.array_length
    }
    pub fn usage(&self) -> TextureUsage {
        self.descriptor.usage
    }
    pub fn resource_options(&self) -> ResourceOptions {
        self.descriptor.resource_options
    }
    pub fn rotation(&self) -> u64 {
        self.descriptor.rotation
    }
    pub fn framebuffer_only(&self) -> bool {
        self.descriptor.framebuffer_only
    }
    pub fn is_drawable(&self) -> bool {
        self.descriptor.is_drawable
    }
    pub fn swizzle(&self) -> TextureSwizzleChannels {
        self.descriptor.swizzle
    }
    pub fn allow_gpu_optimized_contents(&self) -> bool {
        self.descriptor.allow_gpu_optimized_contents
    }
    pub fn set_purgeable_state(&self, state: PurgeableState) -> Result<PurgeableState, String> {
        let previous = self.state.load(Ordering::Relaxed);
        if state != PurgeableState::KeepCurrent {
            self.state.store(state.raw(), Ordering::Relaxed);
        }
        PurgeableState::from_raw(previous)
    }
    /// Placement requirement for purge accounting; physical pool residency comes from runtime gauges.
    pub fn backing_allocation_size(&self) -> u64 {
        self.backing_size
    }
}
impl Buffer {
    pub fn new_texture(
        &self,
        descriptor: &TextureDescriptor,
        offset: u64,
        bytes_per_row: u64,
    ) -> Result<Texture, String> {
        Texture::over_buffer(self, descriptor, offset, bytes_per_row)
    }
}

fn allocation_size(device: &rt::Device, descriptor: &TextureDescriptor) -> u64 {
    if let Ok(size) = descriptor.runtime().and_then(|d| device.heap_texture_size_and_align(&d).map_err(|e| format!("{e:?}"))) { return size.size; }
    // Dedicated-only images remain supported; estimate purge bytes from public level metadata.
    let layers = descriptor.array_length.saturating_mul(if matches!(descriptor.texture_type, TextureType::Cube | TextureType::CubeArray) { 6 } else { 1 });
    (0..descriptor.mipmap_level_count).map(|level| device.texture_level_info(&rt::TextureLevelQuery {
        pixel_format: descriptor.pixel_format.as_raw() as u64, width: descriptor.width, height: descriptor.height,
        depth: descriptor.depth, array_length: layers, level, planes: 0,
    }).bytes.saturating_mul(descriptor.sample_count)).fold(0, u64::saturating_add)
}

/// Matches `metal2vulkan-runtime` texture reachability (`reached(pf, effective_usage)`):
/// empty usage (`MTLTextureUsageUnknown`) expands to `SHADER_READ | SHADER_WRITE | RENDER_TARGET | PIXEL_FORMAT_VIEW`,
/// and depth/stencil render-target-only textures take no descriptor-set index.
pub(crate) fn texture_needs_index(pixel_format: PixelFormat, usage: TextureUsage) -> bool {
    let effective = if usage.is_empty() {
        TextureUsage::SHADER_READ
            | TextureUsage::SHADER_WRITE
            | TextureUsage::RENDER_TARGET
            | TextureUsage::PIXEL_FORMAT_VIEW
    } else {
        usage
    };
    effective.intersects(TextureUsage::SHADER_READ | TextureUsage::SHADER_WRITE)
        || (effective.contains(TextureUsage::RENDER_TARGET)
            && !metal2vulkan::metal::format::is_depth_stencil(pixel_format))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memoryless_descriptor_maps_to_private_runtime_storage() {
        let mut desc = TextureDescriptor::texture_2d(PixelFormat::RG16Float, 46, 46, false);
        desc.resource_options = ResourceOptions::from_raw(3 << 4).unwrap();
        desc.usage = TextureUsage::RENDER_TARGET;
        assert_eq!(desc.resource_options.storage_mode(), StorageMode::Memoryless);
        let runtime = desc.runtime().unwrap();
        assert_eq!(runtime.storage_mode, rt::StorageMode::Private);
    }

    #[test]
    fn depth_stencil_render_target_only_takes_no_texture_index() {
        for pf in [
            PixelFormat::Depth32FloatStencil8,
            PixelFormat::Depth32Float,
            PixelFormat::Depth16Unorm,
            PixelFormat::Stencil8,
            PixelFormat::Depth24UnormStencil8,
            PixelFormat::X32Stencil8,
            PixelFormat::X24Stencil8,
        ] {
            assert!(!texture_needs_index(pf, TextureUsage::RENDER_TARGET), "{pf:?}");
            assert!(
                !texture_needs_index(pf, TextureUsage::RENDER_TARGET | TextureUsage::PIXEL_FORMAT_VIEW),
                "{pf:?}"
            );
            assert!(
                texture_needs_index(pf, TextureUsage::RENDER_TARGET | TextureUsage::SHADER_READ),
                "{pf:?}"
            );
            assert!(texture_needs_index(pf, TextureUsage::empty()), "{pf:?}");
        }
    }

    #[test]
    fn colour_render_target_and_shader_usages_require_texture_index() {
        for pf in [PixelFormat::BGRA8Unorm, PixelFormat::RGBA16Float, PixelFormat::RG16Float] {
            assert!(texture_needs_index(pf, TextureUsage::RENDER_TARGET), "{pf:?}");
            assert!(texture_needs_index(pf, TextureUsage::SHADER_READ), "{pf:?}");
            assert!(texture_needs_index(pf, TextureUsage::SHADER_WRITE), "{pf:?}");
            assert!(texture_needs_index(pf, TextureUsage::empty()), "{pf:?}");
            assert!(!texture_needs_index(pf, TextureUsage::PIXEL_FORMAT_VIEW), "{pf:?}");
        }
    }
}
