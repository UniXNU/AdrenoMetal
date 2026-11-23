// SPDX-License-Identifier: MIT
//! Consumer heap placements over runtime blocks, with leases held through GPU retirement.
use crate::resources::{Buffer, PurgeableState, ResourceOptions, StorageMode};
use crate::texture::{Texture, TextureDescriptor};
use metal2vulkan_runtime as rt;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeapType {
    Automatic,
    Placement,
    Sparse,
}
impl HeapType {
    pub fn from_raw(raw: u64) -> Result<Self, String> {
        match raw {
            0 => Ok(Self::Automatic),
            1 => Ok(Self::Placement),
            2 => Ok(Self::Sparse),
            _ => Err(format!("invalid heap type {raw}")),
        }
    }
}
pub struct HeapDescriptor {
    kind: HeapType,
    size: u64,
    options: ResourceOptions,
}
impl HeapDescriptor {
    pub fn new(kind: HeapType, size: u64, options: ResourceOptions) -> Self {
        Self {
            kind,
            size,
            options,
        }
    }
}
#[derive(Default)]
pub(crate) struct Placements {
    next: u64,
    ranges: BTreeMap<u64, (u64, u64)>,
}
impl Placements {
    pub(crate) fn release(&mut self, id: u64) {
        self.ranges.remove(&id);
    }
    fn gaps(&self, size: u64) -> Vec<(u64, u64)> {
        let mut ranges: Vec<_> = self.ranges.values().copied().collect();
        ranges.sort_unstable();
        let mut gaps = Vec::new();
        let mut at = 0;
        for (start, end) in ranges {
            if start > at {
                gaps.push((at, start));
            }
            at = at.max(end);
        }
        if at < size {
            gaps.push((at, size));
        }
        gaps
    }
    pub(crate) fn reserve(
        &mut self,
        size: u64,
        align: u64,
        offset: Option<u64>,
        capacity: u64,
    ) -> Result<(u64, u64), String> {
        if size == 0 || !align.is_power_of_two() {
            return Err("invalid heap allocation size or alignment".into());
        }
        let start = match offset {
            Some(start) if start.is_multiple_of(align) => start,
            Some(_) => return Err("heap offset does not meet resource alignment".into()),
            None => self
                .gaps(capacity)
                .into_iter()
                .find_map(|(start, end)| {
                    let aligned = start.checked_add(align - 1)? & !(align - 1);
                    (aligned.checked_add(size)? <= end).then_some(aligned)
                })
                .ok_or("heap has no room for the resource")?,
        };
        let end = start
            .checked_add(size)
            .filter(|end| *end <= capacity)
            .ok_or("resource exceeds heap")?;
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or("heap placement IDs exhausted")?;
        self.ranges.insert(id, (start, end));
        Ok((id, start))
    }
}
struct Inner {
    device: rt::Device,
    block: rt::Block,
    kind: HeapType,
    size: u64,
    options: ResourceOptions,
    placements: Mutex<Placements>,
    state: AtomicU64,
}
/// Does not keep the heap or device alive. Runtime resources retain this lease while GPU work refers to them.
pub struct Lease {
    heap: Weak<Inner>,
    id: u64,
    aliasable: AtomicBool,
}
impl Lease {
    pub fn make_aliasable(&self) {
        if !self.aliasable.swap(true, Ordering::AcqRel) {
            self.release();
        }
    }
    pub fn is_aliasable(&self) -> bool {
        self.aliasable.load(Ordering::Acquire)
    }
    fn release(&self) {
        if let Some(heap) = self.heap.upgrade() {
            heap.placements
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ranges
                .remove(&self.id);
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.aliasable.load(Ordering::Relaxed) {
            self.release();
        }
    }
}
#[derive(Clone)]
pub struct Heap(Arc<Inner>);
impl Heap {
    pub(crate) fn block_id_on(&self, device: &rt::Device) -> Result<rt::BlockId, String> {
        if !std::ptr::eq(self.0.device.raw().device, device.raw().device) {
            return Err("heap belongs to another runtime device".into());
        }
        Ok(self.0.block.id)
    }

    pub fn new(device: &rt::Device, descriptor: &HeapDescriptor) -> Result<Self, String> {
        if descriptor.kind == HeapType::Sparse || descriptor.size == 0 {
            return Err("sparse or empty heap is not supported".into());
        }
        let class = match descriptor.options.storage_mode() {
            StorageMode::Shared => rt::MemoryClass::Shared,
            StorageMode::Managed if device.has_unified_memory() => rt::MemoryClass::Shared,
            StorageMode::Private => rt::MemoryClass::Private,
            _ => return Err("upstream heap placement does not support this storage mode".into()),
        };
        let block = device
            .take_block(class, descriptor.size)
            .map_err(|e| format!("heap memory: {e:?}"))?;
        Ok(Self(Arc::new(Inner {
            device: device.clone(),
            block,
            kind: descriptor.kind,
            size: descriptor.size,
            options: descriptor.options,
            placements: Mutex::default(),
            state: AtomicU64::new(2),
        })))
    }
    pub fn size(&self) -> u64 {
        self.0.size
    }
    pub fn used_size(&self) -> u64 {
        let gaps: u64 = self
            .0
            .placements
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .gaps(self.0.size)
            .iter()
            .map(|(a, b)| b - a)
            .sum();
        self.0.size - gaps
    }
    pub fn max_available_size(&self, alignment: u64) -> Result<u64, String> {
        let align = alignment.max(1);
        if !align.is_power_of_two() {
            return Err("heap alignment is not a power of two".into());
        }
        if self.0.kind != HeapType::Automatic {
            return Ok(0);
        }
        Ok(self
            .0
            .placements
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .gaps(self.0.size)
            .iter()
            .filter_map(|(start, end)| {
                let start = start.checked_add(align - 1)? & !(align - 1);
                Some(end.saturating_sub(start))
            })
            .max()
            .unwrap_or(0))
    }
    fn place(
        &self,
        requirement: rt::SizeAndAlign,
        offset: Option<u64>,
    ) -> Result<(Arc<Lease>, u64), String> {
        if (self.0.kind == HeapType::Placement) != offset.is_some() {
            return Err("heap allocation call does not match heap type".into());
        }
        let (id, start) = self
            .0
            .placements
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reserve(requirement.size, requirement.align, offset, self.0.size)?;
        Ok((
            Arc::new(Lease {
                heap: Arc::downgrade(&self.0),
                id,
                aliasable: AtomicBool::new(false),
            }),
            start,
        ))
    }
    fn range(&self) -> rt::HeapRange {
        rt::HeapRange {
            block: self.0.block.id,
            offset: 0,
            len: self.0.size,
        }
    }
    pub fn new_buffer(&self, length: u64, options: ResourceOptions) -> Result<Buffer, String> {
        self.buffer(length, options, None)
    }
    pub fn new_buffer_at_offset(
        &self,
        length: u64,
        options: ResourceOptions,
        offset: u64,
    ) -> Result<Buffer, String> {
        self.buffer(length, options, Some(offset))
    }
    fn buffer(
        &self,
        length: u64,
        options: ResourceOptions,
        offset: Option<u64>,
    ) -> Result<Buffer, String> {
        if options.storage_mode() != self.0.options.storage_mode() {
            return Err("buffer storage mode differs from its heap".into());
        }
        let requirement = self
            .0
            .device
            .heap_buffer_size_and_align(length, options.raw());
        let (lease, offset) = self.place(requirement, offset)?;
        let raw = self
            .0
            .device
            .new_heap_buffer(self.range(), offset, length)
            .map_err(|e| format!("heap buffer: {e:?}"))?;
        Ok(Buffer::placed(raw, self.0.device.clone(), options, lease))
    }
    pub fn new_texture(&self, descriptor: &TextureDescriptor) -> Result<Texture, String> {
        self.texture(descriptor, None)
    }
    pub fn new_texture_at_offset(
        &self,
        descriptor: &TextureDescriptor,
        offset: u64,
    ) -> Result<Texture, String> {
        self.texture(descriptor, Some(offset))
    }
    fn texture(
        &self,
        descriptor: &TextureDescriptor,
        offset: Option<u64>,
    ) -> Result<Texture, String> {
        if descriptor.resource_options.storage_mode() != self.0.options.storage_mode() {
            return Err("texture storage mode differs from its heap".into());
        }
        let d = descriptor.runtime()?;
        let requirement = self
            .0
            .device
            .heap_texture_size_and_align(&d)
            .map_err(|e| format!("heap texture size: {e:?}"))?;
        let (lease, offset) = self.place(requirement, offset)?;
        let index = crate::ownership::reserve(&self.0.device, rt::IndexClass::Texture)?;
        let raw = self
            .0
            .device
            .new_heap_texture(&d, Some(index.index()), self.range(), offset)
            .map_err(|e| format!("heap texture: {e:?}"))?;
        Ok(Texture::placed(
            raw,
            descriptor.clone(),
            self.0.device.clone(),
            lease,
            index,
            requirement.size,
        ))
    }
    pub fn set_purgeable_state(&self, state: PurgeableState) -> Result<PurgeableState, String> {
        let previous = self.0.state.load(Ordering::Relaxed);
        if state != PurgeableState::KeepCurrent {
            self.0.state.store(state.raw(), Ordering::Relaxed);
        }
        PurgeableState::from_raw(previous)
    }
}

#[cfg(test)]
mod tests {
    use super::Placements;
    #[test]
    fn automatic_placements_hold_ranges_until_released() {
        let mut placements = Placements::default();
        let (first, _) = placements.reserve(128, 64, None, 512).unwrap();
        assert_eq!(placements.reserve(128, 256, None, 512).unwrap().1, 256);
        assert!(placements.reserve(300, 64, None, 512).is_err());
        placements.ranges.remove(&first);
        assert_eq!(placements.reserve(256, 64, None, 512).unwrap().1, 0);
    }
    #[test]
    fn explicit_placement_checks_overflow_and_alignment() {
        let mut placements = Placements::default();
        assert!(placements.reserve(128, 64, Some(1), 512).is_err());
        assert!(
            placements
                .reserve(128, 64, Some(u64::MAX - 63), u64::MAX)
                .is_err()
        );
        assert_eq!(placements.reserve(128, 64, Some(64), 512).unwrap().1, 64);
    }
}
