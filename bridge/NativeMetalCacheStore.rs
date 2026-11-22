// SPDX-License-Identifier: GPL-2.0-or-later OR LGPL-3.0-or-later
//! Imports into the pinned upstream cache file format, without changing the runtime.
//! State framing/checksum follow steelbrain/metal2vulkan 4e9aad67 cache.rs.
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::FileExt;
use std::path::Path;
const HALF_CAP: u64 = 64 << 20;
fn sum(bytes: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let step = |lane: u64, word: u64| (lane ^ word).wrapping_mul(K).rotate_left(29);
    let mut lanes = [K, K ^ 1, K ^ 2, K ^ 3];
    let (chunks, tail) = bytes.as_chunks::<32>();
    for chunk in chunks {
        for (lane, word) in lanes.iter_mut().zip(chunk.as_chunks::<8>().0) {
            *lane = step(*lane, u64::from_le_bytes(*word));
        }
    }
    let hash = lanes.into_iter().fold(bytes.len() as u64, step);
    tail.iter()
        .fold(hash, |hash, &byte| step(hash, u64::from(byte)))
}
fn opened(parent: i32, name: &str, flags: i32) -> io::Result<File> {
    let name = CString::new(name)?;
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn directory(parent: i32, name: &str) -> io::Result<File> {
    let name_c = CString::new(name)?;
    if unsafe { libc::mkdirat(parent, name_c.as_ptr(), 0o700) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(error);
        }
    }
    opened(parent, name, libc::O_RDONLY | libc::O_DIRECTORY)
}
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
/// Import only into the existing young generation under the same flock as upstream writers.
/// Cache misses, a full generation and I/O failures never change pipeline creation behavior.
pub fn import(root: &Path, name: &str, bytes: &[u8]) -> io::Result<bool> {
    let Some((kind, digest)) = name.split_once('/') else {
        return Ok(false);
    };
    if !matches!(kind, "translations" | "renders")
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Ok(false);
    }
    let root = opened(
        libc::AT_FDCWD,
        root.as_os_str()
            .to_str()
            .ok_or_else(|| io::Error::other("cache path is not UTF-8"))?,
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    let state = opened(root.as_raw_fd(), "state", libc::O_RDWR)?;
    if unsafe { libc::flock(state.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let state = Lock(state);
    let mut record = [0u8; 24];
    state.0.read_exact_at(&mut record, 0)?;
    let young = u64::from_le_bytes(record[..8].try_into().unwrap());
    let size = u64::from_le_bytes(record[8..16].try_into().unwrap());
    if young == 0
        || u64::from_le_bytes(record[16..].try_into().unwrap()) != sum(&record[..16])
        || size.saturating_add(bytes.len() as u64) > HALF_CAP
    {
        return Ok(false);
    }
    let generation = directory(root.as_raw_fd(), &format!("g{young}"))?;
    let directory = directory(generation.as_raw_fd(), kind)?;
    if opened(directory.as_raw_fd(), digest, libc::O_RDONLY).is_ok() {
        return Ok(false);
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temp = format!(
        ".consumer-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let file = opened(
        directory.as_raw_fd(),
        &temp,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    )?;
    let result = (|| {
        record[8..16].copy_from_slice(&size.saturating_add(bytes.len() as u64).to_le_bytes());
        let hash = sum(&record[..16]);
        record[16..].copy_from_slice(&hash.to_le_bytes());
        state.0.write_all_at(&record, 0)?;
        file.write_all_at(bytes, 0)?;
        // Runtime readers may run without the state lock, so publish complete files only.
        let from = CString::new(temp.as_bytes())?;
        let to = CString::new(digest)?;
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                from.as_ptr(),
                directory.as_raw_fd(),
                to.as_ptr(),
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }

        Ok(true)
    })();
    if let Ok(temp) = CString::new(temp) {
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temp.as_ptr(), 0);
        }
    }
    result
}
/// Driver blobs are copied before Device::new starts its asynchronous cache seed.
pub fn import_blob(root: &Path, name: &str, bytes: &[u8]) -> io::Result<bool> {
    let Some(digest) = name.strip_prefix("pipelines-") else {
        return Ok(false);
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || bytes.len() > 32 << 20
    {
        return Ok(false);
    }
    std::fs::create_dir_all(root)?;
    let directory = opened(
        libc::AT_FDCWD,
        root.to_str()
            .ok_or_else(|| io::Error::other("cache path is not UTF-8"))?,
        libc::O_RDONLY | libc::O_DIRECTORY,
    )?;
    let file = match opened(
        directory.as_raw_fd(),
        name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    ) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        result => result?,
    };
    // No Device uses this new file until this call returns. Failed writes are cache misses.
    file.write_all_at(bytes, 0)?;
    Ok(true)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_preserves_newest_version_and_accounts_bytes() {
        let root = std::env::temp_dir().join(format!("nmb-cache-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let mut state = vec![0u8; 40];
        state[..8].copy_from_slice(&1u64.to_le_bytes());
        let hash = sum(&state[..16]);
        state[16..24].copy_from_slice(&hash.to_le_bytes());
        state[24..].fill(91);
        std::fs::write(root.join("state"), &state).unwrap();
        let name = format!("translations/{}", "a".repeat(64));
        assert!(import(&root, &name, b"artifact").unwrap());
        assert!(!import(&root, &name, b"replacement").unwrap());
        let current = std::fs::read(root.join("state")).unwrap();
        assert_eq!(&current[24..], &state[24..]);
        assert_eq!(u64::from_le_bytes(current[8..16].try_into().unwrap()), 8);
        assert_eq!(
            std::fs::read(root.join("g1").join(name)).unwrap(),
            b"artifact"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn concurrent_imports_serialize_with_upstream_file_locks() {
        let root =
            std::env::temp_dir().join(format!("nmb-cache-concurrent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let mut state = [0u8; 40];
        state[..8].copy_from_slice(&1u64.to_le_bytes());
        let hash = sum(&state[..16]);
        state[16..24].copy_from_slice(&hash.to_le_bytes());
        std::fs::write(root.join("state"), state).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    import(&root, &format!("translations/{i:064x}"), b"bytes").unwrap()
                })
            })
            .collect();
        for thread in threads {
            assert!(thread.join().unwrap());
        }
        let record = std::fs::read(root.join("state")).unwrap();
        assert_eq!(u64::from_le_bytes(record[8..16].try_into().unwrap()), 40);
        assert_eq!(
            u64::from_le_bytes(record[16..24].try_into().unwrap()),
            sum(&record[..16])
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn imports_reject_symlinked_state_without_touching_target() {
        let root = std::env::temp_dir().join(format!("nmb-cache-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let target = root.join("outside");
        std::fs::write(&target, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&target, root.join("state")).unwrap();
        assert!(import(&root, &format!("translations/{}", "a".repeat(64)), b"bytes").is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"unchanged");
        std::fs::remove_dir_all(root).unwrap();
    }
}
