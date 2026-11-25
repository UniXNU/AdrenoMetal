// SPDX-License-Identifier: MIT
//! Consumer-owned Metal buffer options and allocation handles for runtime v2.

use metal2vulkan_runtime as rt;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageMode {
    Shared,
    Managed,
    Private,
    Memoryless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceOptions(u64);

impl ResourceOptions {
    pub fn from_raw(raw: u64) -> Result<Self, String> {
        if raw & 0xf > 1 || (raw >> 4) & 0xf > 3 || (raw >> 8) & 0x3 > 2 {
            return Err(format!("invalid Metal resource options {raw:#x}"));
        }
        Ok(Self(raw))
    }
    pub fn raw(self) -> u64 {
        self.0
    }
    pub fn shared() -> Self {
        Self(0)
    }
    pub fn private() -> Self {
        Self(2 << 4)
    }
    pub fn storage_mode(self) -> StorageMode {
        match (self.0 >> 4) & 0xf {
            0 => StorageMode::Shared,
            1 => StorageMode::Managed,
            2 => StorageMode::Private,
            _ => StorageMode::Memoryless,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum PurgeableState {
    KeepCurrent = 1,
    NonVolatile = 2,
    Volatile = 3,
    Empty = 4,
}
impl PurgeableState {
    pub fn from_raw(raw: u64) -> Result<Self, String> {
        match raw {
            1 => Ok(Self::KeepCurrent),
            2 => Ok(Self::NonVolatile),
            3 => Ok(Self::Volatile),
            4 => Ok(Self::Empty),
            _ => Err(format!("invalid purgeable state {raw}")),
        }
    }
    pub fn raw(self) -> u64 {
        self as u64
    }
}

#[derive(Clone)]
pub struct Buffer {
    raw: rt::Buffer,
    device: rt::Device,
    options: ResourceOptions,
    state: Arc<AtomicU64>,
    // Claims keep placement IDs valid for views constructed after this call.
    _blocks: Arc<Vec<rt::Block>>,
    imported_pointer: Option<usize>,
    lease: Option<Arc<crate::heap::Lease>>,
    pub(crate) ownership: Arc<crate::ownership::Ownership>,
}

impl Deref for Buffer {
    type Target = rt::Buffer;
    fn deref(&self) -> &rt::Buffer {
        &self.raw
    }
}

impl Buffer {
    pub(crate) fn placed(
        raw: rt::Buffer,
        device: rt::Device,
        options: ResourceOptions,
        lease: Arc<crate::heap::Lease>,
    ) -> Self {
        Self {
            raw,
            device,
            options,
            state: Arc::new(AtomicU64::new(2)),
            _blocks: Arc::new(Vec::new()),
            imported_pointer: None,
            ownership: crate::ownership::Ownership::new(vec![lease.clone()]),
            lease: Some(lease),
        }
    }
    pub fn make_aliasable(&self) -> Result<(), String> {
        if let Some(lease) = &self.lease {
            lease.make_aliasable();
        }
        Ok(())
    }
    pub fn is_aliasable(&self) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| lease.is_aliasable())
    }
    pub fn allocate(
        device: &rt::Device,
        length: u64,
        options: ResourceOptions,
        bytes: Option<&[u8]>,
    ) -> Result<Self, String> {
        if length == 0 {
            return Err("a Metal buffer must have nonzero length".into());
        }
        if bytes.is_some_and(|b| b.len() as u64 != length) {
            return Err("initial buffer byte count differs from length".into());
        }
        let managed = options.storage_mode() == StorageMode::Managed;
        let class = match options.storage_mode() {
            StorageMode::Shared | StorageMode::Managed => rt::MemoryClass::Shared,
            StorageMode::Private => rt::MemoryClass::Private,
            StorageMode::Memoryless => return Err("Metal buffers cannot be memoryless".into()),
        };
        let context = crate::archive::Registry::for_device(device)?;
        let allocation = context.buffers.take(device, class, length)?;
        let mut placements = vec![allocation];
        let raw = if managed {
            let copy = if device.has_unified_memory() {
                None
            } else {
                placements.push(
                    context
                        .buffers
                        .take(device, rt::MemoryClass::Private, length)?,
                );
                Some(placements[1].place())
            };
            device.new_managed_buffer(placements[0].place(), copy, length, 0)
        } else {
            let (block, offset) = placements[0].place();
            device.new_buffer(block, offset, length, Some(0))
        }
        .map_err(|e| format!("buffer placement: {e:?}"))?;

        let blocks = Vec::new();
        let buffer = Self {
            raw,
            device: device.clone(),
            options,
            state: Arc::new(AtomicU64::new(2)),
            _blocks: Arc::new(blocks),
            imported_pointer: None,
            ownership: crate::ownership::Ownership::new(placements.into_iter().map(|p| p as crate::ownership::Owner).collect()),
            lease: None,
        };
        if let Some(bytes) = bytes {
            if options.storage_mode() == StorageMode::Private {
                let staging =
                    Self::allocate(device, length, ResourceOptions::shared(), Some(bytes))?;
                buffer.upload(&staging)?;
                return Ok(buffer);
            }
            let pointer = buffer
                .contents()
                .ok_or("buffer allocation has no CPU mapping")?;
            // SAFETY: the newly allocated range is writable, disjoint from the input and not submitted.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.as_ptr(), bytes.len());
            }
            if managed {
                buffer
                    .did_modify_range(0, length)
                    .map_err(|e| format!("initial managed bytes: {e:?}"))?;
            }
        }
        Ok(buffer)
    }

    /// Construct a buffer from a C caller's bytes without borrowing its virtual mapping.
    pub unsafe fn allocate_from_host(
        device: &rt::Device,
        length: u64,
        options: ResourceOptions,
        source: *const u8,
    ) -> Result<Self, String> {
        if options.storage_mode() == StorageMode::Private {
            let staging = Self::allocate(device, length, ResourceOptions::shared(), None)?;
            unsafe {
                crate::host_mapping::copy_readable(
                    source,
                    staging
                        .contents()
                        .ok_or("staging buffer has no host mapping")?,
                    length,
                )
            }?;
            let buffer = Self::allocate(device, length, options, None)?;
            buffer.upload(&staging)?;
            return Ok(buffer);
        }
        let buffer = Self::allocate(device, length, options, None)?;
        unsafe {
            crate::host_mapping::copy_readable(
                source,
                buffer.contents().ok_or("buffer has no host mapping")?,
                length,
            )
        }?;
        if options.storage_mode() == StorageMode::Managed {
            buffer
                .did_modify_range(0, length)
                .map_err(|e| format!("initial managed bytes: {e:?}"))?;
        }
        Ok(buffer)
    }
    fn upload(&self, staging: &Self) -> Result<(), String> {
        let queue = self
            .device
            .new_command_queue(1)
            .map_err(|e| format!("buffer upload queue: {e:?}"))?;
        let command = queue
            .command_buffer()
            .map_err(|e| format!("buffer upload command: {e:?}"))?;
        let mut blit = command
            .blit_command_encoder()
            .map_err(|e| format!("buffer upload encoder: {e:?}"))?;
        blit.copy_from_buffer_to_buffer(staging, 0, self, 0, self.length())
            .map_err(|e| format!("buffer upload: {e:?}"))?;
        blit.end_encoding();
        command
            .commit()
            .map_err(|e| format!("buffer upload commit: {e:?}"))?;
        command
            .wait_until_completed()
            .map_err(|e| format!("buffer upload wait: {e:?}"))?;
        if let Some(error) = command.error() {
            return Err(format!("buffer upload failed: {error:?}"));
        }
        Ok(())
    }

    /// The caller keeps these pages mapped until the release callback runs.
    pub unsafe fn import(
        device: &rt::Device,
        pointer: NonNull<u8>,
        length: u64,
        options: ResourceOptions,
        release: Option<Box<dyn FnOnce(NonNull<u8>, u64) + Send>>,
    ) -> Result<Self, String> {
        if !matches!(
            options.storage_mode(),
            StorageMode::Shared | StorageMode::Managed
        ) {
            return Err("no-copy buffers require Shared or Managed storage".into());
        }
        let requested_read_only = options.raw() & (1 << 24) != 0;
        if requested_read_only && options.storage_mode() == StorageMode::Managed {
            return Err("read-only no-copy buffers require Shared storage".into());
        }
        // Arm the caller callback only after all fallible placement operations succeed.
        // Failed construction leaves ownership with the C caller.
        let pending = Arc::new(Mutex::new(None::<Box<dyn FnOnce() + Send>>));
        let freed = pending.clone();
        // The GPU imports a granule-aligned alias covering the caller range, not the reusable
        // virtual address the client gave. Read-only aliases stay read-only and are pinned
        // read-only by the kernel driver. This mapping is released after Vulkan frees the import.
        let mut min_align = 4096u64;
        let (block, offset) = loop {
            let mapping =
                unsafe { crate::host_mapping::HostMapping::alias(pointer, length, min_align) }?;
            if !mapping.is_writable() && options.storage_mode() == StorageMode::Managed {
                return Err("read-only host pages require Shared storage".into());
            }
            let imported = mapping.page_pointer();
            let imported_len = mapping.page_length();
            let offset = mapping.offset();
            let read_only = !mapping.is_writable() || requested_read_only;
            let freed_cb = freed.clone();
            let callback = Box::new(move || {
                let callback = freed_cb.lock().unwrap_or_else(|e| e.into_inner()).take();
                drop(mapping);
                if let Some(callback) = callback {
                    callback();
                }
            });
            let attempt = if read_only {
                // SAFETY: enclosing readable pages stay aliased until release runs;
                // runtime write paths refuse writes and the kernel pins the pages read-only.
                unsafe {
                    device.import_read_only_host_memory(imported, imported_len, Some(callback))
                }
            } else {
                // SAFETY: enclosing readable and writable pages stay aliased until release runs.
                unsafe { device.import_host_memory(imported, imported_len, Some(callback)) }
            };
            match attempt {
                Ok((block, _)) => break (block, offset),
                Err(rt::Error::Limit(rt::Limit::ImportAlignment { alignment, .. }))
                    if alignment > min_align && alignment.is_power_of_two() =>
                {
                    min_align = alignment;
                }
                Err(e) => return Err(format!("host-page import: {e:?}")),
            }
        };
        let mut blocks = vec![block];
        let raw = if options.storage_mode() == StorageMode::Managed {
            let copy = if device.has_unified_memory() {
                None
            } else {
                blocks.push(
                    device
                        .take_block(rt::MemoryClass::Private, length)
                        .map_err(|e| format!("managed import copy: {e:?}"))?,
                );
                Some((blocks[1].id, 0))
            };
            device.new_managed_buffer((blocks[0].id, offset), copy, length, length)
        } else {
            device.new_buffer(blocks[0].id, offset, length, None)
        }
        .map_err(|e| format!("import placement: {e:?}"))?;
        if let Some(release) = release {
            let address = pointer.as_ptr() as usize;
            *pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(move || {
                // SAFETY: an address captured from the non-null pointer above, returned only after GPU release.
                release(
                    unsafe { NonNull::new_unchecked(address as *mut u8) },
                    length,
                );
            }));
        }
        Ok(Self {
            raw,
            device: device.clone(),
            options,
            state: Arc::new(AtomicU64::new(2)),
            _blocks: Arc::new(blocks),
            imported_pointer: Some(pointer.as_ptr() as usize),
            ownership: crate::ownership::Ownership::new(vec![]),
            lease: None,
        })
    }

    pub fn contents(&self) -> Option<NonNull<u8>> {
        if self.options.storage_mode() == StorageMode::Private {
            return None;
        }
        self.imported_pointer
            .and_then(|p| NonNull::new(p as *mut u8))
            .or_else(|| self.raw.contents())
    }
    pub fn resource_options(&self) -> ResourceOptions {
        self.options
    }
    pub fn device(&self) -> &rt::Device {
        &self.device
    }
    pub fn set_purgeable_state(&self, state: PurgeableState) -> Result<PurgeableState, String> {
        let previous = self.state.load(Ordering::Relaxed);
        if state != PurgeableState::KeepCurrent {
            self.state.store(state.raw(), Ordering::Relaxed);
        }
        PurgeableState::from_raw(previous)
    }
}
