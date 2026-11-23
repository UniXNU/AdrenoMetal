// SPDX-License-Identifier: MIT
//! Stable aliases keep imported VM objects alive when a caller reuses its address.

use std::ptr::NonNull;

pub struct HostMapping {
    address: usize,
    length: usize,
    writable: bool,
}

impl HostMapping {
    /// Alias the caller's readable, page-aligned range without copying its pages.
    /// Read-only mappings stay read-only; the kernel pins and maps them read-only in SMMU.
    /// The caller must own and keep the range valid until this method returns.
    #[cfg(target_vendor = "apple")]
    pub unsafe fn alias(pointer: NonNull<u8>, length: u64) -> Result<Self, String> {
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
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page <= 0
            || length == 0
            || !(pointer.as_ptr() as usize).is_multiple_of(page as usize)
            || !length_usize.is_multiple_of(page as usize)
        {
            return Err("host import must span whole aligned pages".into());
        }
        let (mut address, mut current, mut maximum) = (0u64, 0i32, 0i32);
        // SAFETY: out parameters are local; copy=false shares the source objects in a fresh mapping.
        let status = unsafe {
            mach_vm_remap(
                mach_task_self_,
                &mut address,
                length,
                0,
                1,
                mach_task_self_,
                pointer.as_ptr() as u64,
                0,
                &mut current,
                &mut maximum,
                2,
            )
        };
        if status != 0 {
            return Err(format!("host import remap failed: {status}"));
        }
        if current & 1 == 0 || address == 0 {
            // SAFETY: release only the alias this call created; never alter the caller's protection.
            unsafe {
                mach_vm_deallocate(mach_task_self_, address, length);
            }
            return Err("host import requires readable pages".into());
        }
        Ok(Self {
            address: address as usize,
            length: length_usize,
            writable: current & 2 != 0,
        })
    }

    #[cfg(not(target_vendor = "apple"))]
    pub unsafe fn alias(_pointer: NonNull<u8>, _length: u64) -> Result<Self, String> {
        Err("stable host aliases require the Darwin VM API".into())
    }

    pub fn pointer(&self) -> NonNull<u8> {
        // SAFETY: construction rejects address zero, and this value owns the mapping.
        unsafe { NonNull::new_unchecked(self.address as *mut u8) }
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
                HostMapping::alias(NonNull::new(original.cast::<u8>()).unwrap(), length as u64)
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
            let alias = HostMapping::alias(pointer, length as u64).unwrap();
            assert!(!alias.is_writable());
            assert_eq!(std::ptr::read_volatile(alias.pointer().as_ptr()), 0x5a);
            assert_eq!(libc::munmap(original, length), 0);
            assert_eq!(std::ptr::read_volatile(alias.pointer().as_ptr()), 0x5a);
            drop(alias);
        }
    }
    #[test]
    fn invalid_imports_reject_unreadable_and_unaligned_ranges() {
        unsafe {
            let length = libc::sysconf(libc::_SC_PAGESIZE) as usize;
            let original = libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_NONE,
                libc::MAP_ANON | libc::MAP_PRIVATE,
                -1,
                0,
            );
            assert_ne!(original, libc::MAP_FAILED);
            let pointer = NonNull::new(original.cast::<u8>()).unwrap();
            assert!(HostMapping::alias(pointer, length as u64).is_err());
            assert!(
                HostMapping::alias(
                    NonNull::new(pointer.as_ptr().add(1)).unwrap(),
                    length as u64
                )
                .is_err()
            );
            assert_eq!(libc::munmap(original, length), 0);
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
