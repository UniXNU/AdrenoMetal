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
        d.storage_mode = match self.resource_options.storage_mode() {
            StorageMode::Shared => rt::StorageMode::Shared,
            StorageMode::Managed => rt::StorageMode::Managed,
            StorageMode::Private => rt::StorageMode::Private,
            StorageMode::Memoryless => rt::StorageMode::Memoryless,
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
    ) -> Self {
        Self {
            raw,
            descriptor,
            device,
            state: Arc::new(AtomicU64::new(2)),
            lease: Some(lease),
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
        let index = device
            .reserve_index(rt::IndexClass::Texture)
            .ok_or("texture index space exhausted")?;
        let raw = device
            .new_texture(&descriptor.runtime()?, Some(index.index()))
            .map_err(|e| format!("texture: {e:?}"))?;
        Ok(Self {
            raw,
            descriptor: descriptor.clone(),
            device: device.clone(),
            state: Arc::new(AtomicU64::new(2)),
            lease: None,
        })
    }
    pub fn over_buffer(
        buffer: &Buffer,
        descriptor: &TextureDescriptor,
        offset: u64,
        bytes_per_row: u64,
    ) -> Result<Self, String> {
        let device = buffer.device();
        let index = device
            .reserve_index(rt::IndexClass::Texture)
            .ok_or("texture index space exhausted")?;
        let raw = (**buffer)
            .new_texture(
                &descriptor.runtime()?,
                Some(index.index()),
                offset,
                bytes_per_row,
            )
            .map_err(|e| format!("texture over buffer: {e:?}"))?;
        Ok(Self {
            raw,
            descriptor: descriptor.clone(),
            device: device.clone(),
            state: Arc::new(AtomicU64::new(2)),
            lease: None,
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
        let narrow = |v| u32::try_from(v).map_err(|_| "texture view range exceeds u32".to_owned());
        let index = self
            .device
            .reserve_index(rt::IndexClass::Texture)
            .ok_or("texture index space exhausted")?;
        let raw = self
            .raw
            .new_texture_view_with_swizzle(
                format,
                kind,
                narrow(levels.start)?..narrow(levels.end)?,
                narrow(slices.start)?..narrow(slices.end)?,
                swizzle,
                Some(index.index()),
            )
            .map_err(|e| format!("texture view: {e:?}"))?;
        let runtime = raw.descriptor();
        let mut descriptor = self.descriptor.clone();
        descriptor.texture_type = runtime.texture_type;
        descriptor.pixel_format = runtime.pixel_format;
        descriptor.width = runtime.width as u64;
        descriptor.height = runtime.height as u64;
        descriptor.depth = runtime.depth as u64;
        descriptor.array_length = runtime.array_length as u64;
        descriptor.mipmap_level_count = runtime.mipmap_level_count as u64;
        descriptor.swizzle = runtime.swizzle;
        Ok(Self {
            raw,
            descriptor,
            device: self.device.clone(),
            state: self.state.clone(),
            lease: self.lease.clone(),
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
    pub fn backing_allocation_size(&self) -> u64 {
        self.raw.backing_allocation_size()
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
