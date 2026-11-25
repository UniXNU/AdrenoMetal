// SPDX-License-Identifier: MIT
//! Stable aliases keep imported VM objects alive when a caller reuses its address.

use std::ptr::NonNull;

pub struct HostMapping {
    address: usize,
    length: usize,
    offset: usize,
    writable: bool,
}

impl HostMapping {
    /// Alias the enclosing page-aligned range covering `(pointer, length)` without copying pages.
    /// `min_alignment` is the Vulkan host-pointer import granule (power of two, e.g. 4096); the
    /// remapped alias is aligned to `max(vm_page_size, min_alignment)`.
    /// Read-only mappings stay read-only; the kernel pins and maps them read-only in SMMU.
    /// The caller must own and keep the range valid until this method returns.
    #[cfg(target_vendor = "apple")]
    pub unsafe fn alias(
        pointer: NonNull<u8>,
        length: u64,
        min_alignment: u64,
    ) -> Result<Self, String> {
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn mach_vm_remap(
                target: u32,
                address: *mut u64,
                length: u64,
                mask: u64,
                flags: i32,
                source: u32,
                source_address: u64,
                copy: i32,
                current: *mut i32,
                maximum: *mut i32,
                inheritance: i32,
            ) -> i32;
            fn mach_vm_deallocate(task: u32, address: u64, length: u64) -> i32;
        }
        let length_usize = usize::try_from(length).map_err(|_| "host import is too long")?;
        if length_usize == 0 {
            return Err("host import requires non-zero length".into());
        }
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page <= 0 {
            return Err("invalid system page size".into());
        }
        let page = page as usize;
        let min_align = usize::try_from(min_alignment.max(1))
            .map_err(|_| "host import alignment overflow")?;
        let granule = if min_align.is_power_of_two() {
            page.max(min_align)
        } else {
            return Err("host import alignment must be a power of two".into());
        };
        if !granule.is_power_of_two() || !granule.is_multiple_of(page) {
            return Err("host import alignment is incompatible with system page size".into());
        }
        let start = pointer.as_ptr() as usize;
        let aligned_start = start & !(granule - 1);
        let offset = start - aligned_start;
        let end = start
            .checked_add(length_usize)
            .ok_or("host import range overflow")?;
        let aligned_end = end
            .checked_next_multiple_of(granule)
            .ok_or("host import alignment overflow")?;
        let aligned_length = aligned_end - aligned_start;
        let (mut address, mut current, mut maximum) = (0u64, 0i32, 0i32);
        // SAFETY: out parameters are local; copy=false shares the source objects in a fresh mapping.
        let status = unsafe {
            mach_vm_remap(
                mach_task_self_,
                &mut address,
                aligned_length as u64,
                (granule - 1) as u64,
                1,
                mach_task_self_,
                aligned_start as u64,
                0,
                &mut current,
                &mut maximum,
                2,
            )
        };
        if status != 0 {
            return Err(format!("host import remap failed: {status}"));
        }
        if current & 1 == 0 || address == 0 || !(address as usize).is_multiple_of(granule) {
            // SAFETY: release only the alias this call created; never alter caller protection.
            unsafe {
                mach_vm_deallocate(mach_task_self_, address, aligned_length as u64);
            }
            return Err("host import requires readable pages".into());
        }
        Ok(Self {
            address: address as usize,
            length: aligned_length,
            offset,
            writable: current & 2 != 0,
        })
    }

    #[cfg(not(target_vendor = "apple"))]
    pub unsafe fn alias(
        _pointer: NonNull<u8>,
        _length: u64,
        _min_alignment: u64,
    ) -> Result<Self, String> {
        Err("stable host aliases require the Darwin VM API".into())
    }

    /// Granule-aligned base address of the remapped alias for Vulkan host-pointer import.
    pub fn page_pointer(&self) -> NonNull<u8> {
        // SAFETY: construction rejects address zero, and this value owns the mapping.
        unsafe { NonNull::new_unchecked(self.address as *mut u8) }
    }

    /// Granule-multiple byte length of the remapped alias for Vulkan host-pointer import.
    pub fn page_length(&self) -> u64 {
        self.length as u64
    }

    /// Byte offset from `page_pointer()` to the start of the requested caller range.
    pub fn offset(&self) -> u64 {
        self.offset as u64
    }

    /// Exact aliased pointer corresponding to the requested caller range.
    pub fn pointer(&self) -> NonNull<u8> {
        // SAFETY: `offset < length` and `address != 0`.
        unsafe { NonNull::new_unchecked((self.address + self.offset) as *mut u8) }
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }
}

impl Drop for HostMapping {
    fn drop(&mut self) {
        #[cfg(target_vendor = "apple")]
        unsafe {
            unsafe extern "C" {
                static mach_task_self_: u32;
                fn mach_vm_deallocate(task: u32, address: u64, length: u64) -> i32;
            }
            // SAFETY: this alias stays owned until the Vulkan import's release callback runs.
            mach_vm_deallocate(mach_task_self_, self.address as u64, self.length as u64);
        }
    }
}

#[cfg(all(test, target_vendor = "apple"))]
mod tests {
    use super::HostMapping;
    use std::ptr::NonNull;

    #[test]
    fn alias_keeps_original_pages_after_the_caller_reuses_its_address() {
        unsafe {
            let length = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let original = libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            );
            assert_ne!(original, libc::MAP_FAILED);
            std::ptr::write_bytes(original, 0x31, length);
            let alias =
                HostMapping::alias(NonNull::new(original.cast::<u8>()).unwrap(), length as u64, 1)
                    .unwrap();
            assert_eq!(libc::munmap(original, length), 0);
            let replacement = libc::mmap(
                original,
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE | libc::MAP_FIXED,
                -1,
                0,
            );
            assert_eq!(replacement, original);
            std::ptr::write_bytes(replacement, 0x62, length);
            assert_eq!(std::ptr::read_volatile(alias.pointer().as_ptr()), 0x31);
            assert_eq!(std::ptr::read_volatile(replacement.cast::<u8>()), 0x62);
            drop(alias);
            assert_eq!(libc::munmap(replacement, length), 0);
        }
    }
    #[test]
    fn checked_copy_rejects_a_deallocated_source_without_dereferencing_it() {
        unsafe {
            let length = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let source = libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            );
            assert_ne!(source, libc::MAP_FAILED);
            std::ptr::write_bytes(source, 0x43, length);
            let mut output = vec![0u8; length];
            let destination = NonNull::new(output.as_mut_ptr()).unwrap();
            super::copy_readable(source.cast(), destination, length as u64).unwrap();
            assert!(output.iter().all(|byte| *byte == 0x43));
            assert_eq!(libc::munmap(source, length), 0);
            assert!(super::copy_readable(source.cast(), destination, length as u64).is_err());
        }
    }
    #[test]
    fn read_only_alias_keeps_original_pages_without_upgrading_permissions() {
        unsafe {
            let length = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let original = libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            );
            assert_ne!(original, libc::MAP_FAILED);
            std::ptr::write_bytes(original, 0x5a, length);
            assert_eq!(libc::mprotect(original, length, libc::PROT_READ), 0);
            let pointer = NonNull::new(original.cast::<u8>()).unwrap();
            let alias = HostMapping::alias(pointer, length as u64, 1).unwrap();
            assert!(!alias.is_writable());
            assert_eq!(std::ptr::read_volatile(alias.pointer().as_ptr()), 0x5a);
            assert_eq!(libc::munmap(original, length), 0);
            assert_eq!(std::ptr::read_volatile(alias.pointer().as_ptr()), 0x5a);
            drop(alias);
        }
    }
    #[test]
    fn invalid_imports_reject_unreadable_empty_and_overflowing_ranges() {
        unsafe {
            let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let region = libc::mmap(
                std::ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            );
            assert_ne!(region, libc::MAP_FAILED);
            assert_eq!(libc::mprotect(region.cast::<u8>().add(page).cast(), page, libc::PROT_NONE), 0);
            let base = NonNull::new(region.cast::<u8>()).unwrap();
            let guard_page = NonNull::new(region.cast::<u8>().add(page)).unwrap();
            // Unreadable page rejected.
            assert!(HostMapping::alias(guard_page, page as u64, 1).is_err());
            // Unaligned range crossing from readable page into PROT_NONE page rejected.
            assert!(HostMapping::alias(NonNull::new(base.as_ptr().add(page - 16)).unwrap(), 64, 1).is_err());
            // Zero length and range overflow rejected.
            assert!(HostMapping::alias(base, 0, 1).is_err());
            assert!(HostMapping::alias(base, u64::MAX, 1).is_err());
            assert_eq!(libc::munmap(region, page * 2), 0);
        }
    }

    #[test]
    fn unaligned_and_sub_page_alias_preserves_exact_bytes_and_offset() {
        unsafe {
            let page = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let original = libc::mmap(
                std::ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            );
            assert_ne!(original, libc::MAP_FAILED);
            let bytes = std::slice::from_raw_parts_mut(original.cast::<u8>(), page * 2);
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = (i % 251) as u8;
            }
            // Sub-page length at page-aligned base.
            let sub = HostMapping::alias(NonNull::new(original.cast::<u8>()).unwrap(), 256, 4096).unwrap();
            assert_eq!(sub.offset(), 0);
            assert_eq!(sub.page_length(), page as u64);
            assert_eq!(sub.pointer().as_ptr(), sub.page_pointer().as_ptr());

            // Unaligned pointer crossing page boundary.
            let off = page - 64;
            let len = 192usize;
            let cross = HostMapping::alias(
                NonNull::new(original.cast::<u8>().add(off)).unwrap(),
                len as u64,
                4096,
            )
            .unwrap();
            assert_eq!(cross.offset(), off as u64);
            assert_eq!(cross.page_length(), (page * 2) as u64);
            assert_eq!((cross.page_pointer().as_ptr() as usize) % page, 0);

            // Replace original mapping; alias keeps original bytes intact.
            assert_eq!(libc::munmap(original, page * 2), 0);
            let replacement = libc::mmap(
                original,
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANON | libc::MAP_PRIVATE | libc::MAP_FIXED,
                -1,
                0,
            );
            assert_eq!(replacement, original);
            std::ptr::write_bytes(replacement, 0xff, page * 2);

            let aliased = std::slice::from_raw_parts(cross.pointer().as_ptr(), len);
            for (i, &b) in aliased.iter().enumerate() {
                assert_eq!(b, ((off + i) % 251) as u8);
            }
            drop(sub);
            drop(cross);
            assert_eq!(libc::munmap(replacement, page * 2), 0);
        }
    }
}

/// Copy caller-owned bytes through the VM API, which reports a hole or remap as an error.
/// No Rust slice borrows memory that another process or thread can replace.
#[cfg(target_vendor = "apple")]
pub unsafe fn copy_readable(
    source: *const u8,
    destination: NonNull<u8>,
    length: u64,
) -> Result<(), String> {
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn mach_vm_read_overwrite(
            task: u32,
            source: u64,
            length: u64,
            destination: u64,
            copied: *mut u64,
        ) -> i32;
    }
    if length == 0 {
        return Ok(());
    }
    if source.is_null()
        || (source as usize)
            .checked_add(usize::try_from(length).map_err(|_| "host copy length overflow")?)
            .is_none()
    {
        return Err("host copy source range overflows".into());
    }
    let mut copied = 0;
    let status = unsafe {
        mach_vm_read_overwrite(
            mach_task_self_,
            source as u64,
            length,
            destination.as_ptr() as u64,
            &mut copied,
        )
    };
    if status != 0 || copied != length {
        return Err(format!(
            "host bytes unavailable: VM status {status}, copied {copied}/{length}"
        ));
    }
    Ok(())
}
#[cfg(not(target_vendor = "apple"))]
pub unsafe fn copy_readable(
    _source: *const u8,
    _destination: NonNull<u8>,
    _length: u64,
) -> Result<(), String> {
    Err("checked host copies require the Darwin VM API".into())
}
