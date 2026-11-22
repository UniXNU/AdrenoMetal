// SPDX-License-Identifier: MIT
//! Consumer indices and placement leases retained by commands through GPU completion.
use metal2vulkan_runtime as rt;
use std::sync::{Arc, Mutex};

pub type Owner = Arc<dyn Send + Sync>;
#[derive(Default)]
pub struct Ownership(pub Vec<Owner>);
impl Ownership {
    pub fn new(owners: Vec<Owner>) -> Arc<Self> {
        Arc::new(Self(owners))
    }
}

pub struct IndexPool {
    free: Mutex<Vec<u32>>,
}
impl IndexPool {
    pub fn new(class: rt::IndexClass) -> Arc<Self> {
        Arc::new(Self {
            free: Mutex::new(class.range().rev().collect()),
        })
    }
    pub fn reserve(self: &Arc<Self>) -> Option<Reservation> {
        self.free
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop()
            .map(|index| {
                Reservation(Arc::new(IndexLease {
                    pool: self.clone(),
                    index,
                }))
            })
    }
}
struct IndexLease {
    pool: Arc<IndexPool>,
    index: u32,
}
impl Drop for IndexLease {
    fn drop(&mut self) {
        self.pool
            .free
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(self.index);
    }
}
pub struct Reservation(Arc<IndexLease>);
impl Reservation {
    pub fn index(&self) -> u32 {
        self.0.index
    }
    pub fn owner(&self) -> Owner {
        self.0.clone()
    }
}
pub fn reserve(device: &rt::Device, class: rt::IndexClass) -> Result<Reservation, String> {
    crate::archive::Registry::for_device(device)?
        .reserve(class)
        .ok_or_else(|| format!("{class:?} index space exhausted"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloned_lease_prevents_reuse_until_command_retires() {
        let pool = IndexPool::new(rt::IndexClass::Sampler);
        let first = pool.reserve().unwrap();
        let index = first.index();
        let command = first.owner();
        drop(first);
        assert_ne!(pool.reserve().unwrap().index(), index);
        drop(command);
        assert_eq!(pool.reserve().unwrap().index(), index);
    }
    #[test]
    fn exhaustion_does_not_displace_a_live_index() {
        let pool = IndexPool::new(rt::IndexClass::Sampler);
        let leases: Vec<_> = (0..rt::IndexClass::Sampler.range().len())
            .map(|_| pool.reserve().unwrap())
            .collect();
        assert!(pool.reserve().is_none());
        drop(leases);
        assert!(pool.reserve().is_some());
    }
}
