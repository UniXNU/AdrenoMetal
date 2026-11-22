// SPDX-License-Identifier: MIT
//! Consumer-owned archives imported into the upstream runtime cache directory.
use metal2vulkan::{
    Budget,
    cache::{ARTIFACT_FORMAT_VERSION, Hash256, SEMANTICS_VERSION, VariantKey},
    library::CompiledFunction,
    pipeline::{
        ComputePipelineDescriptor, KernelVariant, RenderPipelineDescriptor, StageEntries,
        StageSource,
    },
};
use metal2vulkan_runtime as rt;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

const MAGIC: &[u8; 8] = b"NMBAR002";
const MAX_BYTES: usize = 256 << 20;
const MAX_ENTRIES: usize = 65536;
#[derive(Default)]
struct Files {
    members: BTreeSet<(u8, Hash256)>,
    files: BTreeMap<String, Vec<u8>>,
    bytes: usize,
}
impl Files {
    fn insert(&mut self, name: String, bytes: Vec<u8>) -> Result<(), String> {
        let previous = self.files.get(&name).map_or(0, Vec::len);
        let total = self
            .bytes
            .checked_sub(previous)
            .and_then(|n| n.checked_add(bytes.len()))
            .filter(|n| *n <= MAX_BYTES)
            .ok_or("binary archive exceeds 256 MiB")?;
        if !self.files.contains_key(&name) && self.files.len() == MAX_ENTRIES {
            return Err("binary archive has too many functions".into());
        }
        self.files.insert(name, bytes);
        self.bytes = total;
        Ok(())
    }
    fn keep(&mut self, function: &CompiledFunction) -> Result<(), String> {
        self.insert(
            format!("translations/{}", hex(function.key().digest())),
            function.to_bytes(),
        )
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub struct Registry {
    pub buffers: crate::buffer_arena::BufferArena,
    archives: Mutex<Vec<Weak<Mutex<Files>>>>,
    indices: [Arc<crate::ownership::IndexPool>; 2],
    cache_root: PathBuf,
    shared_writer: bool,
    system: Option<Arc<crate::system_cache::SystemLayer>>,
}
fn devices() -> &'static Mutex<BTreeMap<usize, Weak<Registry>>> {
    static DEVICES: std::sync::OnceLock<Mutex<BTreeMap<usize, Weak<Registry>>>> =
        std::sync::OnceLock::new();
    DEVICES.get_or_init(Mutex::default)
}
impl Registry {
    pub fn bind(self: &Arc<Self>, device: &rt::Device) {
        let mut devices = devices().lock().unwrap_or_else(|e| e.into_inner());
        devices.retain(|_, value| value.strong_count() != 0);
        devices.insert(
            device.raw().device as *const _ as usize,
            Arc::downgrade(self),
        );
    }
    pub fn for_device(device: &rt::Device) -> Result<Arc<Self>, String> {
        devices()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(device.raw().device as *const _ as usize))
            .and_then(Weak::upgrade)
            .ok_or_else(|| "device has no consumer allocation context".into())
    }
    pub fn new(cache_root: PathBuf) -> Arc<Self> {
        let registry = Arc::new(Self {
            cache_root,
            shared_writer: crate::system_cache::cache_readable_by_all(),
            buffers: crate::buffer_arena::BufferArena::default(),
            archives: Mutex::default(),
            indices: [crate::ownership::IndexPool::new(rt::IndexClass::Texture), crate::ownership::IndexPool::new(rt::IndexClass::Sampler)],
            system: crate::system_cache::system_layer(),
        });
        if registry.shared_writer {
            let weak = Arc::downgrade(&registry);
            // Only a system-cache writer publishes. Ordinary applications create no worker.
            let _ = std::thread::Builder::new().name("nmb-cache-publish".into()).spawn(move || {
                let mut seen = BTreeMap::new();
                loop {
                    let Some(registry) = weak.upgrade() else { break; };
                    registry.publish_changed(&mut seen);
                    drop(registry);
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            });
        }
        registry
    }
    pub fn reserve(&self, class: rt::IndexClass) -> Option<crate::ownership::Reservation> {
        let slot = match class { rt::IndexClass::Texture => 0, rt::IndexClass::Sampler => 1, _ => return None };
        self.indices[slot].reserve()
    }
    fn add(&self, files: &Arc<Mutex<Files>>) {
        let mut archives = self.archives.lock().unwrap_or_else(|e| e.into_inner());
        archives.retain(|files| files.strong_count() != 0);
        archives.push(Arc::downgrade(files));
    }
}
impl Registry {
    fn read(&self, name: &str) -> Option<Vec<u8>> {
        // The runtime produces these names; archive parsing admits only this same namespace.
        if !valid_name(name) && !name.starts_with("pipelines-") {
            return None;
        }
        let archives = self
            .archives
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        for files in archives {
            if let Some(bytes) = files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .files
                .get(name)
            {
                return Some(bytes.clone());
            }
        }
        let system = self.system.as_ref()?;
        if let Some(bytes) = system.read(Path::new(name), MAX_BYTES as u64) {
            return Some(bytes);
        }
        // A newer runtime stores lowerings in two generations. Content decoding still checks
        // versions, keys and checksums; this header chooses lookup paths only.
        let state = system.read(Path::new("state"), 4096)?;
        let young = u64::from_le_bytes(state.get(..8)?.try_into().ok()?);
        for generation in [young, young.saturating_sub(1)] {
            if generation == 0 {
                continue;
            }
            if let Some(bytes) = system.read(
                &PathBuf::from(format!("g{generation}")).join(name),
                MAX_BYTES as u64,
            ) {
                return Some(bytes);
            }
        }
        None
    }
    pub fn seed_driver_blobs(&self) {
        let Some(system)=&self.system else { return; };
        let mut remaining=4;
        for entry in std::fs::read_dir(system.path()).into_iter().flatten().flatten() {
            let name=entry.file_name(); let Some(name)=name.to_str() else { continue; };
            let Some(digest)=name.strip_prefix("pipelines-") else { continue; };
            if digest.len()!=64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { continue; }
            if let Some(bytes)=system.read(Path::new(name),32<<20) { let _=crate::cache_store::import_blob(&self.cache_root,name,&bytes); }
            remaining-=1; if remaining==0 { break; }
        }
    }
    fn seed(&self, name: &str) -> Option<Vec<u8>> {
        let bytes = self.read(name)?;
        let _ = crate::cache_store::import(&self.cache_root, name, &bytes);
        Some(bytes)
    }
    pub fn prepare_compute(&self, device: &rt::Device, descriptor: &ComputePipelineDescriptor) {
        if self.system.is_none() && self.archives.lock().unwrap_or_else(|e| e.into_inner()).is_empty() { return; }
        let StageSource::Function(function) = &descriptor.kernel else { return; };
        let translator=device.translator();
        let Ok(plain)=translator.kernel_inputs_read(function,&descriptor.lowering_inputs(),&Budget::standard(),&StageEntries::new()) else { return; };
        for inputs in [plain.clone(), plain.kernel_variant(KernelVariant::PartialThreadgroups),plain.kernel_variant(KernelVariant::EmulatedSimdgroups)] {
            self.seed(&format!("translations/{}",hex(translator.function_compile_key(function,&inputs).digest())));
        }
    }
    pub fn prepare_render(&self, device: &rt::Device, descriptor: &RenderPipelineDescriptor) {
        if self.system.is_none() && self.archives.lock().unwrap_or_else(|e| e.into_inner()).is_empty() { return; }
        let translator=device.translator();
        let record=format!("renders/{}",hex(translator.queried_render_descriptor_key(descriptor).digest()));
        if let Some(bytes)=self.seed(&record) {
            // Only derive bounded digest names; the runtime checks record versions, keys and checksum.
            if bytes.len()==122 && &bytes[..8]==b"m2vrndr1" {
                for at in [48usize,81] { if bytes[at]==1 { self.seed(&format!("translations/{}",hex(&bytes[at+1..at+33]))); } }
            }
        }
        let Ok(keys)=translator.render_variant_key(descriptor,&VariantKey::new(),&Budget::standard(),&StageEntries::new()) else { return; };
        for (source,inputs) in [(Some(&descriptor.vertex), keys.vertex_inputs.as_ref()),(descriptor.fragment.as_ref(),keys.fragment_inputs.as_ref())] {
            if let (Some(StageSource::Function(function)),Some(inputs))=(source,inputs) {
                self.seed(&format!("translations/{}",hex(translator.function_compile_key(function,inputs).digest())));
            }
        }
    }
    fn publish_changed(&self, seen: &mut BTreeMap<PathBuf, std::time::SystemTime>) {
        let mut directories = vec![self.cache_root.clone()];
        for entry in std::fs::read_dir(&self.cache_root).into_iter().flatten().flatten() {
            let name=entry.file_name(); let Some(name)=name.to_str() else { continue; };
            if name.strip_prefix('g').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                directories.push(entry.path());
                directories.push(entry.path().join("translations"));
                directories.push(entry.path().join("renders"));
            }
        }
        for directory in directories {
            let Ok(metadata)=std::fs::symlink_metadata(&directory) else { continue; };
            if !metadata.is_dir() { continue; }
            let Ok(modified)=metadata.modified() else { continue; };
            if seen.get(&directory)==Some(&modified) { continue; }
            seen.insert(directory.clone(),modified);
            self.published(&directory);
            for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_file()) && !entry.file_name().to_string_lossy().starts_with('.') { self.published(&entry.path()); }
            }
        }
    }
    fn published(&self, path: &Path) {
        if self.shared_writer {
            let _ = publish_permissions(&self.cache_root, path);
        }
    }

}
fn valid_name(name: &str) -> bool {
    let Some((kind, digest)) = name.split_once('/') else {
        return false;
    };
    matches!(kind, "translations" | "renders")
        && digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[derive(Clone)]
pub struct BinaryArchive {
    device: rt::Device,
    files: Arc<Mutex<Files>>,
}
impl BinaryArchive {
    pub fn new(
        device: &rt::Device,
        registry: &Registry,
        path: Option<&Path>,
    ) -> Result<Self, String> {
        let files = match path {
            None => Files::default(),
            Some(path) => {
                let file = std::fs::File::open(path).map_err(|e| format!("archive open: {e}"))?;
                if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                    return Err("archive is not a regular file".into());
                }
                let mut bytes = Vec::new();
                use std::io::Read;
                file.take((MAX_BYTES + (8 << 20) + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                let files = decode(&bytes, &device.translator().profile_fingerprint())?;
                for (name, bytes) in &files.files {
                    if name.starts_with("translations/")
                        && !matches!(bytes.as_slice(), b"m2v-refused" | b"m2v-observes")
                    {
                        let function = CompiledFunction::from_bytes(bytes, device.translator())
                            .map_err(|e| format!("archive artifact: {e:?}"))?;
                        if name != &format!("translations/{}", hex(function.key().digest())) {
                            return Err("archive artifact name does not match its key".into());
                        }
                    }
                }
                files
            }
        };
        let files = Arc::new(Mutex::new(files));
        registry.add(&files);
        Ok(Self {
            device: device.clone(),
            files,
        })
    }
    pub fn contains_compute(
        &self,
        device: &rt::Device,
        descriptor: &ComputePipelineDescriptor,
    ) -> bool {
        self.contains(
            device,
            0,
            device
                .translator()
                .compute_pipeline_key(descriptor)
                .digest(),
        )
    }
    pub fn contains_render(
        &self,
        device: &rt::Device,
        descriptor: &RenderPipelineDescriptor,
    ) -> bool {
        self.contains(
            device,
            1,
            device
                .translator()
                .queried_render_descriptor_key(descriptor)
                .digest(),
        )
    }
    fn contains(&self, device: &rt::Device, kind: u8, key: &Hash256) -> bool {
        self.device.translator().profile_fingerprint() == device.translator().profile_fingerprint()
            && self
                .files
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .members
                .contains(&(kind, *key))
    }
    pub fn add_compute_pipeline_functions(
        &self,
        descriptor: &ComputePipelineDescriptor,
    ) -> Result<(), String> {
        let t = self.device.translator();
        let budget = Budget::standard();
        let entries = StageEntries::new();
        let mut files = Files::default();
        match &descriptor.kernel {
            StageSource::Compiled(function) => files.keep(function)?,
            StageSource::Function(function) => {
                let plain = t
                    .kernel_inputs_read(function, &descriptor.lowering_inputs(), &budget, &entries)
                    .map_err(|e| format!("archive kernel inputs: {e:?}"))?;
                let partial = plain.kernel_variant(KernelVariant::PartialThreadgroups);
                let observes = t
                    .kernel_observes_partition(function, &partial, &budget, &entries)
                    .map_err(|e| format!("archive kernel partition: {e:?}"))?;
                let whole = if observes {
                    let key = t.function_compile_key(function, &partial);
                    files.insert(
                        format!("translations/{}", hex(key.digest())),
                        b"m2v-observes".to_vec(),
                    )?;
                    t.compile_function_from_entries(function, &plain, &budget, &entries)
                } else {
                    t.compile_function_from_entries(function, &partial, &budget, &entries)
                }
                .map_err(|e| format!("archive compute function: {e:?}"))?;
                files.keep(&whole)?;
                // Archive all executable variants required by this kernel, not a replacement
                // StageSource that would erase the runtime's partial-grid/SIMD selection.
                if observes {
                    let partial = t
                        .compile_function_from_entries(function, &partial, &budget, &entries)
                        .map_err(|e| format!("archive partial-grid function: {e:?}"))?;
                    files.keep(&partial)?;
                }
                if whole.requires_full_subgroups() {
                    let emulated = plain.kernel_variant(KernelVariant::EmulatedSimdgroups);
                    let emulated = t
                        .compile_function_from_entries(function, &emulated, &budget, &entries)
                        .map_err(|e| format!("archive SIMD function: {e:?}"))?;
                    files.keep(&emulated)?;
                }
            }
        }
        self.merge(files, 0, *t.compute_pipeline_key(descriptor).digest())
    }
    pub fn add_render_pipeline_functions(
        &self,
        descriptor: &RenderPipelineDescriptor,
    ) -> Result<(), String> {
        let t = self.device.translator();
        let budget = Budget::standard();
        let entries = StageEntries::new();
        let keys = t
            .render_variant_key(descriptor, &VariantKey::new(), &budget, &entries)
            .map_err(|e| format!("archive render keys: {e:?}"))?;
        let mut files = Files::default();
        for (source, inputs) in [
            (Some(&descriptor.vertex), keys.vertex_inputs.as_ref()),
            (descriptor.fragment.as_ref(), keys.fragment_inputs.as_ref()),
        ] {
            match (source, inputs) {
                (Some(StageSource::Compiled(function)), _) => files.keep(function)?,
                (Some(StageSource::Function(function)), Some(inputs)) => {
                    let lowered = t
                        .compile_function_from_entries(function, inputs, &budget, &entries)
                        .map_err(|e| format!("archive render function: {e:?}"))?;
                    files.keep(&lowered)?;
                }
                (None, _) => {}
                _ => return Err("render archive has no lowering inputs for a function".into()),
            }
        }
        self.merge(
            files,
            1,
            *t.queried_render_descriptor_key(descriptor).digest(),
        )
    }
    fn merge(&self, additions: Files, kind: u8, key: Hash256) -> Result<(), String> {
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        // Preflight the whole addition so an allocation-limit failure adds no partial member.
        let added = additions
            .files
            .iter()
            .try_fold(0usize, |size, (name, bytes)| {
                size.checked_add(
                    bytes
                        .len()
                        .saturating_sub(files.files.get(name).map_or(0, Vec::len)),
                )
            })
            .ok_or("archive size overflow")?;
        if files.bytes.saturating_add(added) > MAX_BYTES
            || files.files.len().saturating_add(additions.files.len()) > MAX_ENTRIES
            || (files.members.len() == MAX_ENTRIES && !files.members.contains(&(kind, key)))
        {
            return Err("archive capacity exceeded".into());
        }
        for (name, bytes) in additions.files {
            files.insert(name, bytes)?;
        }
        files.members.insert((kind, key));
        Ok(())
    }
    pub fn serialize_to_url(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        let bytes = encode(
            &self.files.lock().unwrap_or_else(|e| e.into_inner()),
            &self.device.translator().profile_fingerprint(),
        );
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let temporary = parent.join(format!(
            ".nmb-archive-{}-{}",
            std::process::id(),
            rt::host_time()
        ));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            std::fs::rename(&temporary, path).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}
fn encode(files: &Files, profile: &Hash256) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&SEMANTICS_VERSION.to_le_bytes());
    bytes.extend_from_slice(&ARTIFACT_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(profile);
    bytes.extend_from_slice(&(files.members.len() as u64).to_le_bytes());
    for (kind, key) in &files.members {
        bytes.push(*kind);
        bytes.extend_from_slice(key);
    }
    bytes.extend_from_slice(&(files.files.len() as u64).to_le_bytes());
    for (name, data) in &files.files {
        bytes.extend_from_slice(&(name.len() as u64).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
        bytes.extend_from_slice(data);
    }
    let digest = Sha256::digest(&bytes);
    bytes.extend_from_slice(&digest);
    bytes
}
fn decode(bytes: &[u8], profile: &Hash256) -> Result<Files, String> {
    fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], String> {
        let (value, tail) = bytes
            .split_at_checked(length)
            .ok_or("truncated binary archive")?;
        *bytes = tail;
        Ok(value)
    }
    fn word(bytes: &mut &[u8]) -> Result<usize, String> {
        usize::try_from(u64::from_le_bytes(
            take(bytes, 8)?
                .try_into()
                .map_err(|_| "invalid archive word")?,
        ))
        .map_err(|_| "archive word exceeds usize".into())
    }
    if bytes.len() > MAX_BYTES + (8 << 20) || bytes.len() < 32 {
        return Err("invalid archive length".into());
    }
    let (body, digest) = bytes.split_at(bytes.len() - 32);
    if Sha256::digest(body).as_slice() != digest {
        return Err("archive checksum mismatch".into());
    }
    let mut bytes = body;
    if take(&mut bytes, 8)? != MAGIC
        || take(&mut bytes, 4)? != SEMANTICS_VERSION.to_le_bytes()
        || take(&mut bytes, 4)? != ARTIFACT_FORMAT_VERSION.to_le_bytes()
        || take(&mut bytes, 32)? != profile
    {
        return Err("archive runtime or GPU profile mismatch".into());
    }
    let mut files = Files::default();
    let members = word(&mut bytes)?;
    if members > MAX_ENTRIES {
        return Err("too many archive members".into());
    }
    for _ in 0..members {
        let kind = take(&mut bytes, 1)?[0];
        if kind > 1 {
            return Err("unknown archive member kind".into());
        }
        let key = take(&mut bytes, 32)?
            .try_into()
            .map_err(|_| "invalid archive key")?;
        if !files.members.insert((kind, key)) {
            return Err("duplicate archive member".into());
        }
    }
    let entries = word(&mut bytes)?;
    if entries > MAX_ENTRIES {
        return Err("too many archive functions".into());
    }
    for _ in 0..entries {
        let len = word(&mut bytes)?;
        if len > 80 {
            return Err("archive entry name is too long".into());
        }
        let name = std::str::from_utf8(take(&mut bytes, len)?)
            .map_err(|_| "invalid archive entry name")?
            .to_owned();
        if !valid_name(&name) || files.files.contains_key(&name) {
            return Err("invalid or duplicate archive entry".into());
        }
        let len = word(&mut bytes)?;
        if len > MAX_BYTES {
            return Err("archive entry is too long".into());
        }
        files.insert(name, take(&mut bytes, len)?.to_vec())?;
    }
    if !bytes.is_empty() {
        return Err("trailing bytes in archive".into());
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_publication_handles_private_modes_without_following_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!(
            "nmb-publication-{}-{}",
            std::process::id(),
            rt::host_time()
        ));
        let entry = root.join("g1/translations/entry");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, b"entry").unwrap();
        for path in [&root, &root.join("g1"), entry.parent().unwrap()] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o600)).unwrap();
        publish_permissions(&root, &entry).unwrap();
        assert_eq!(
            std::fs::metadata(&entry).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(
            std::fs::metadata(entry.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        let outside = root.join("outside");
        std::fs::write(&outside, b"private").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&outside, root.join("link")).unwrap();
        assert!(publish_permissions(&root, &root.join("link")).is_err());
        assert_eq!(
            std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(publish_permissions(&root, &root.join("../outside")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn archive_roundtrip_checks_profile_versions_digest_and_bounds() {
        let mut files = Files::default();
        files.members.insert((0, [3; 32]));
        files
            .insert(
                format!("translations/{}", hex(&[5; 32])),
                b"m2v-observes".to_vec(),
            )
            .unwrap();
        let encoded = encode(&files, &[7; 32]);
        let decoded = decode(&encoded, &[7; 32]).unwrap();
        assert_eq!(decoded.members, files.members);
        assert_eq!(decoded.files, files.files);
        assert!(decode(&encoded, &[8; 32]).is_err());
        let mut corrupt = encoded.clone();
        corrupt[16] ^= 1;
        assert!(decode(&corrupt, &[7; 32]).is_err());
        for at in [0, 8, 16, 48, encoded.len() - 1] {
            assert!(decode(&encoded[..at], &[7; 32]).is_err());
        }
        assert!(!valid_name("translations/../state"));
    }
}

fn publish_permissions(root: &Path, path: &Path) -> Result<(), String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "cache publication leaves its root")?;
    let root_path = CString::new(root.as_os_str().as_bytes()).map_err(|_| "NUL in cache root")?;
    let root_fd = unsafe {
        libc::open(
            root_path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err("cannot open cache publication root".into());
    }
    // SAFETY: the fresh descriptor is transferred once to OwnedFd.
    let mut directory = unsafe { OwnedFd::from_raw_fd(root_fd) };
    if unsafe { libc::fchmod(directory.as_raw_fd(), 0o755) } != 0 {
        return Err("cannot publish cache root permissions".into());
    }
    let parts = relative.components().collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        let std::path::Component::Normal(part) = part else {
            return Err("invalid cache publication component".into());
        };
        let part = CString::new(part.as_bytes()).map_err(|_| "NUL in cache component")?;
        let last = index + 1 == parts.len();
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                part.as_ptr(),
                libc::O_RDONLY
                    | libc::O_NOFOLLOW
                    | libc::O_CLOEXEC
                    | libc::O_NONBLOCK
                    | if last { 0 } else { libc::O_DIRECTORY },
            )
        };
        if fd < 0 {
            return Err("cannot open cache publication entry".into());
        }
        let opened = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut info: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(opened.as_raw_fd(), &mut info) } != 0 {
            return Err("cannot stat cache publication entry".into());
        }
        let mode = if info.st_mode & libc::S_IFMT == libc::S_IFDIR {
            0o755
        } else if info.st_mode & libc::S_IFMT == libc::S_IFREG {
            0o644
        } else {
            return Err("cache publication entry is not a directory or regular file".into());
        };
        if unsafe { libc::fchmod(opened.as_raw_fd(), mode) } != 0 {
            return Err("cannot publish cache entry permissions".into());
        }
        directory = opened;
    }
    Ok(())
}

impl Drop for Registry {
    fn drop(&mut self) {
        if self.shared_writer { self.publish_changed(&mut BTreeMap::new()); }
    }
}
