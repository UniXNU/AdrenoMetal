// SPDX-License-Identifier: MIT
//! Small Metal buffers share a runtime granule; leases survive all GPU references.
use crate::heap::Placements;
use metal2vulkan_runtime as rt;
use std::sync::{Arc, Mutex, Weak};
const GRANULE: u64 = 64 << 10;
// Match the runtime's heap-buffer guarantee, including texel-buffer views.
const ALIGNMENT: u64 = 256;
struct Chunk {
    block: rt::Block,
    class: rt::MemoryClass,
    capacity: u64,
    places: Mutex<Placements>,
}
/// Owns no Device handle, so retaining it from a runtime buffer cannot form a cycle.
pub struct Allocation {
    chunk: Arc<Chunk>,
    id: u64,
    offset: u64,
}
impl Allocation {
    pub fn place(&self) -> (rt::BlockId, u64) {
        (self.chunk.block.id, self.offset)
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        self.chunk
            .places
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .release(self.id);
    }
}
#[derive(Default)]
pub struct BufferArena {
    chunks: Mutex<Vec<Weak<Chunk>>>,
}
impl BufferArena {
    pub fn take(
        &self,
        device: &rt::Device,
        class: rt::MemoryClass,
        length: u64,
    ) -> Result<Arc<Allocation>, String> {
        if length == 0 {
            return Err("empty buffer allocation".into());
        }
        if length <= GRANULE / 2 {
            let mut chunks = self.chunks.lock().unwrap_or_else(|e| e.into_inner());
            chunks.retain(|chunk| chunk.strong_count() != 0);
            for chunk in chunks
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|chunk| chunk.class == class)
            {
                let place = chunk
                    .places
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .reserve(length, ALIGNMENT, None, chunk.capacity);
                if let Ok((id, offset)) = place {
                    return Ok(Arc::new(Allocation { chunk, id, offset }));
                }
            }
            let chunk = make_chunk(device, class, GRANULE)?;
            let (id, offset) = chunk
                .places
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .reserve(length, ALIGNMENT, None, chunk.capacity)?;
            chunks.push(Arc::downgrade(&chunk));
            return Ok(Arc::new(Allocation { chunk, id, offset }));
        }
        let chunk = make_chunk(device, class, length)?;
        let (id, offset) = chunk
            .places
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reserve(length, ALIGNMENT, None, chunk.capacity)?;
        Ok(Arc::new(Allocation { chunk, id, offset }))
    }
}
fn make_chunk(
    device: &rt::Device,
    class: rt::MemoryClass,
    length: u64,
) -> Result<Arc<Chunk>, String> {
    let block = device
        .take_block(class, length)
        .map_err(|e| format!("buffer arena: {e:?}"))?;
    Ok(Arc::new(Chunk {
        block,
        class,
        capacity: length,
        places: Mutex::default(),
    }))
}
