// SPDX-License-Identifier: MIT
//! Purgeable memory: what `setPurgeableState:` promises of a buffer, a texture or a heap, kept by the bridge above the runtime.
//!
//! Metal's contract. A resource whose state is Volatile may lose its contents. `setPurgeableState:` answers the state before the
//! call, and Empty where the contents were discarded; the caller sets NonVolatile before it uses the resource again and makes the
//! contents again when the answer is Empty. Apple's drivers do not discard at the call: the kernel purges volatile memory when it
//! needs pages. RenderBox, QuartzCore, SkyLight and Core Image keep their recycled buffers, textures and heaps Volatile on that
//! promise, so on a GPU whose memory is wired for life those caches cost their whole size.
//!
//! Where the discard is made. The runtime keeps a resource's state and never discards (its attributes.rs): a buffer's `VkBuffer`
//! and placement and an image's `VkImage` and placement are the resource's own for its life (buffer.rs `FixedPages`, texture.rs
//! `Owner::Runtime`; Vulkan binds memory once), and its views, copies and first moves rely on that. So the bridge discards with
//! the two operations the runtime performs for every resource: it releases the runtime object and later makes one again from the
//! same description. A release is safe by the runtime's own rules: a batch in flight and an encoder being recorded hold what they
//! use, so the placement is returned only once nothing of the runtime's reads it, and a block with no placement left follows the
//! pools' aging (kept empty blocks, spare blocks; bridge patches 0021, 0030, 0034). The handle the frontend holds stays the same
//! box for the resource's life, so a call that still names it never reaches freed memory: it finds the resource placed, or places
//! it again.
//!
//! What may be discarded ([`Resource::kept`]): a texture the device made of its own memory (`nmb_texture_new`, not a drawable), a
//! buffer the device made (`nmb_buffer_new`), a heap (`nmb_heap_new`), where the runtime keeps a purgeable state for it. Never: a
//! texture over a buffer or a surface, a no-copy buffer, a view, a resource placed in a heap, a drawable, and what the runtime
//! makes for itself (it has no handle here and no caller that could declare it Volatile). Those are [`Resource::Fixed`], with no
//! cell: their calls borrow the runtime object as before.
//!
//! When ([`Cell::sweep`]). A resource is discarded only while all of these hold, read under its lock:
//! - its state is Volatile or Empty, by the runtime's own answer (which keeps the state only of a resource with an allocation of
//!   its own: at least 16 KiB, not memoryless);
//! - no command buffer that named it is being recorded or is in flight (`holds`: the frontend holds it from the call that names
//!   it until the command buffer completed);
//! - nothing made of it lives (`ties`: a view of a texture, a texture over a buffer, a resource placed in a heap), since those
//!   share its memory and would not follow a new placement;
//! - neither its CPU pointer nor its GPU address was ever handed out (`pinned`): Metal keeps both for a resource's life, and a
//!   resource made again lies elsewhere;
//! - no call is inside it (its lock is free).
//! The triggers are the system's memory pressure signal ([`Cause::Pressure`]) and age ([`Cause::Age`]: Volatile or Empty and idle
//! for the runtime's spare age), because pressure events are rare on this system while memory is short.
//!
//! After a discard the state is Empty. NonVolatile places the resource again before it answers Empty; a query answers Empty and
//! places nothing (a query changes nothing in Metal, and placing for it would cost the memory again for a caller that only asks);
//! Volatile leaves it Empty, as Metal does. Any other use of a discarded resource is the caller's error under the contract (it did
//! not set NonVolatile first): the resource is placed again with new, zeroed memory, its state still Empty, as a GPU whose pages
//! were purged shows emptied pages, and it may be discarded again once idle. Placing again fails as an allocation fails: the call
//! that needed the resource fails with the allocation's error, `setPurgeableState:` still answers Empty, and the next use tries
//! again.
//!
//! `NATIVE_METAL_PURGE=0` (the boot argument `gpu-purge=0`) makes every resource `Fixed`: the state is kept by the runtime alone
//! and nothing is ever discarded, for a comparison on the same image.

use metal2vulkan_runtime::{Buffer, Device, Heap, HeapDescriptor, HeapType, PurgeableState, ResourceOptions, Texture, TextureDescriptor};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed, Ordering::SeqCst};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError, Weak};
use std::time::{Duration, Instant};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// False where `NATIVE_METAL_PURGE=0` (the boot argument `gpu-purge=0`): read once, as the device opens.
fn purge_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NATIVE_METAL_PURGE").is_none_or(|value| value != "0"))
}

/// Why a sweep runs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// The system reported memory pressure: every idle Volatile or Empty resource goes.
    Pressure = 0,
    /// The age timer: those Volatile or Empty and idle for the given age go.
    Age = 1,
}

/// Bit 8 of a `setPurgeableState:` answer: the contents were discarded and placing the resource again failed (the reason is the
/// call's error text). The low byte is still Metal's answer, Empty.
pub const AGAIN_FAILED: u64 = 1 << 8;

/// What a device counts of its discards, for the frontend's PG rows.
#[derive(Default)]
struct Counts {
    /// Resources discarded and their bytes, by [`Cause`].
    discarded: [AtomicU64; 2],
    discarded_bytes: [AtomicU64; 2],
    /// Resources placed again and their bytes: by a NonVolatile request, by another use.
    again: [AtomicU64; 2],
    again_bytes: [AtomicU64; 2],
    /// `setPurgeableState:` answers of Empty for contents a sweep discarded.
    empty_answers: AtomicU64,
    /// Bytes discarded and not placed again, now.
    out_bytes: AtomicU64,
    /// Bytes that became Volatile or Empty so far, and of them those of resources that can never go: their CPU pointer or GPU
    /// address was handed out.
    declared_bytes: AtomicU64,
    declared_pinned_bytes: AtomicU64,
    /// Bytes the last sweep found Volatile or Empty and left: named by a command buffer not yet completed or inside a call, tied
    /// to a view or a placed resource, not yet idle for the age.
    left_bytes: [AtomicU64; 3],
    /// Placements again that failed.
    failed: AtomicU64,
}

/// A device's purge state: whether it discards at all, the resources that may go, and the counts.
pub struct Purge {
    device: Device,
    on: bool,
    /// The resources Volatile or Empty with memory, for the sweeps.
    listed: Mutex<Vec<Weak<dyn Candidate>>>,
    counts: Counts,
    /// The origin of the cells' idle clocks.
    start: Instant,
}

impl Purge {
    /// The purge state of `device`, on unless the process was started with `NATIVE_METAL_PURGE=0`.
    pub fn new(device: &Device) -> Arc<Purge> {
        Arc::new(Purge { device: device.clone(), on: purge_on(), listed: Mutex::new(Vec::new()), counts: Counts::default(), start: Instant::now() })
    }

    /// The age an idle Volatile or Empty resource goes at: the runtime's spare block age, so a resource and the block it emptied
    /// go on one clock.
    pub fn age() -> Duration {
        Device::spare_age()
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Discard what may go for `cause`, those idle for `idle` where the cause is age: the bytes discarded.
    pub fn sweep(&self, cause: Cause, idle: Duration) -> u64 {
        if !self.on {
            return 0;
        }
        // Out of the list while they are looked at: a cell listed meanwhile is added to the new list.
        let listed = std::mem::take(&mut *lock(&self.listed));
        let now = Instant::now();
        let (mut bytes, mut left, mut stay) = (0, [0u64; 3], Vec::with_capacity(listed.len()));
        for weak in listed {
            // Dropped outside every lock: this may be the last holder of a resource its caller released meanwhile.
            let Some(cell) = weak.upgrade() else {
                continue;
            };
            match cell.sweep(cause, now, idle) {
                Swept::Discarded(size) => {
                    bytes += size;
                    self.counts.discarded[cause as usize].fetch_add(1, Relaxed);
                    self.counts.discarded_bytes[cause as usize].fetch_add(size, Relaxed);
                    self.counts.out_bytes.fetch_add(size, Relaxed);
                }
                Swept::Left(why, size) => {
                    left[why as usize] += size;
                    // A tied resource is listed again when its last tie goes; the others may go at a later sweep.
                    if why != Left::Tied {
                        stay.push(weak);
                    }
                }
                Swept::Not => {}
            }
        }
        for (count, bytes) in self.counts.left_bytes.iter().zip(left) {
            count.store(bytes, Relaxed);
        }
        lock(&self.listed).extend(stay);
        bytes
    }

    /// How many resources a later sweep may still discard.
    pub fn pending(&self) -> usize {
        lock(&self.listed).len()
    }

    /// The counts, into the caller's sixteen words: resources and bytes discarded by pressure, then by age; resources and bytes
    /// placed again by a NonVolatile request, then by another use; Empty answers for discarded contents; bytes discarded now;
    /// bytes that became Volatile or Empty so far and those of them pinned; the bytes the last sweep left because they were in
    /// use, tied, or not idle long enough; placements again that failed.
    pub fn counts(&self, out: &mut [u64]) {
        let c = &self.counts;
        let words = [
            c.discarded[0].load(Relaxed), c.discarded_bytes[0].load(Relaxed), c.discarded[1].load(Relaxed), c.discarded_bytes[1].load(Relaxed),
            c.again[0].load(Relaxed), c.again_bytes[0].load(Relaxed), c.again[1].load(Relaxed), c.again_bytes[1].load(Relaxed),
            c.empty_answers.load(Relaxed), c.out_bytes.load(Relaxed), c.declared_bytes.load(Relaxed), c.declared_pinned_bytes.load(Relaxed),
            c.left_bytes[0].load(Relaxed), c.left_bytes[1].load(Relaxed), c.left_bytes[2].load(Relaxed), c.failed.load(Relaxed),
        ];
        for (out, word) in out.iter_mut().zip(words) {
            *out = word;
        }
    }
}

/// Why a sweep left a Volatile or Empty resource.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Left {
    /// A command buffer not yet completed names it, or a call is inside it.
    Used = 0,
    /// A view of it, a texture over it or a resource placed in it lives.
    Tied = 1,
    /// It was not Volatile or Empty and idle for the age yet.
    Young = 2,
}

enum Swept {
    Discarded(u64),
    Left(Left, u64),
    /// No longer a candidate: placed NonVolatile again, discarded already, or pinned for good.
    Not,
}

trait Candidate: Send + Sync {
    fn sweep(&self, cause: Cause, now: Instant, idle: Duration) -> Swept;
}

/// A kind of resource the bridge can make again from its description.
pub trait Kind: Clone + Send + Sync + 'static {
    /// What the resource was made of.
    type Recipe: Send + Sync + 'static;
    fn make(device: &Device, recipe: &Self::Recipe) -> Result<Self, String>;
    /// The runtime's `setPurgeableState:`.
    fn state(&self, state: PurgeableState) -> Result<PurgeableState, String>;
    /// The bytes of device memory it holds.
    fn bytes(&self) -> u64;
}

impl Kind for Texture {
    type Recipe = TextureDescriptor;
    fn make(device: &Device, recipe: &TextureDescriptor) -> Result<Texture, String> {
        device.new_texture(recipe).map_err(|error| format!("texture: {error:?}"))
    }
    fn state(&self, state: PurgeableState) -> Result<PurgeableState, String> {
        self.set_purgeable_state(state).map_err(|error| format!("purgeable state: {error:?}"))
    }
    fn bytes(&self) -> u64 {
        self.backing_allocation_size()
    }
}

impl Kind for Buffer {
    /// The length and the options. A buffer made with bytes is made again without them: its contents were discarded.
    type Recipe = (u64, ResourceOptions);
    fn make(device: &Device, recipe: &(u64, ResourceOptions)) -> Result<Buffer, String> {
        device.new_buffer(recipe.0, recipe.1).map_err(|error| format!("buffer: {error:?}"))
    }
    fn state(&self, state: PurgeableState) -> Result<PurgeableState, String> {
        self.set_purgeable_state(state).map_err(|error| format!("purgeable state: {error:?}"))
    }
    fn bytes(&self) -> u64 {
        self.length()
    }
}

impl Kind for Heap {
    /// The type, the size asked and the options.
    type Recipe = (HeapType, u64, ResourceOptions);
    fn make(device: &Device, recipe: &(HeapType, u64, ResourceOptions)) -> Result<Heap, String> {
        device.new_heap(&HeapDescriptor::new(recipe.0, recipe.1, recipe.2)).map_err(|error| format!("heap: {error:?}"))
    }
    fn state(&self, state: PurgeableState) -> Result<PurgeableState, String> {
        self.set_purgeable_state(state).map_err(|error| format!("heap purgeable state: {error:?}"))
    }
    fn bytes(&self) -> u64 {
        self.size()
    }
}

/// Whether the runtime keeps `object`'s purgeable state, asked of a new object: Volatile is read back, then NonVolatile put back.
/// Neither request changes the object's contents.
fn keeps_state<T: Kind>(object: &T) -> bool {
    let kept = object.state(PurgeableState::Volatile).is_ok() && matches!(object.state(PurgeableState::KeepCurrent), Ok(PurgeableState::Volatile));
    let _ = object.state(PurgeableState::NonVolatile);
    kept
}

fn discardable(state: PurgeableState) -> bool {
    matches!(state, PurgeableState::Volatile | PurgeableState::Empty)
}

struct Slot<T> {
    /// The runtime object; `None` while the resource is discarded.
    object: Option<T>,
    /// The state Metal answers: the runtime's own, but Empty while `lost`.
    state: PurgeableState,
    /// When the state last changed, or the resource was last placed again by a use.
    since: Instant,
    /// Its contents were discarded since the caller last set NonVolatile. Always set while `object` is `None`; set with an object
    /// once a use placed the resource again, whose runtime state is then a new object's and not asked.
    lost: bool,
}

/// A resource that may be discarded: its runtime object or none, how to make it again, and who uses it.
pub struct Cell<T: Kind> {
    purge: Arc<Purge>,
    recipe: T::Recipe,
    /// The bytes of device memory it holds while placed.
    bytes: u64,
    slot: Mutex<Slot<T>>,
    /// The command buffers being recorded or in flight that name it.
    holds: AtomicU32,
    /// The views of it, textures over it and resources placed in it that live.
    ties: AtomicU32,
    /// Its CPU pointer or GPU address was handed out.
    pinned: AtomicBool,
    /// When its last hold went, in milliseconds of the device's purge clock.
    idle_ms: AtomicU64,
    /// It is in the device's list of candidates. Changed under `slot`'s lock.
    listed: AtomicBool,
    me: Weak<Cell<T>>,
}

impl<T: Kind> Cell<T> {
    /// In the device's list from now on, once. Called with `slot`'s lock held.
    fn list(&self) {
        if self.pinned.load(SeqCst) || self.listed.swap(true, SeqCst) {
            return;
        }
        let me: Weak<dyn Candidate> = self.me.clone();
        lock(&self.purge.listed).push(me);
    }

    /// The object, placed again where it was discarded: the use of a resource whose caller did not set NonVolatile first.
    fn live(&self) -> Result<T, String> {
        if let Some(object) = &lock(&self.slot).object {
            return Ok(object.clone());
        }
        let slot = self.place(1)?;
        Ok(slot.object.clone().expect("a placed resource has its object"))
    }

    /// The object where the resource has one, placing nothing.
    fn peek(&self) -> Option<T> {
        lock(&self.slot).object.clone()
    }

    /// The slot with its object, made again where the resource was discarded and counted under `how` (0 a NonVolatile request,
    /// 1 another use). The object is made with no lock held: making it is a creation call of the runtime's, which may wait, and
    /// nothing of this resource is locked while the runtime works. Where another call placed the resource meanwhile, that object
    /// stays and this call's goes. The contents are still the discarded ones (`lost`): the state stays Empty, and the resource may
    /// go again once idle.
    fn place(&self, how: usize) -> Result<MutexGuard<'_, Slot<T>>, String> {
        let counts = &self.purge.counts;
        loop {
            let slot = lock(&self.slot);
            if slot.object.is_some() {
                return Ok(slot);
            }
            drop(slot);
            let made = T::make(&self.purge.device, &self.recipe).map_err(|why| {
                counts.failed.fetch_add(1, Relaxed);
                format!("placing a discarded resource again: {why}")
            })?;
            let mut slot = lock(&self.slot);
            if slot.object.is_none() {
                counts.again[how].fetch_add(1, Relaxed);
                counts.again_bytes[how].fetch_add(self.bytes, Relaxed);
                counts.out_bytes.fetch_sub(self.bytes, Relaxed);
                slot.object = Some(made);
                slot.since = Instant::now();
                self.list();
                return Ok(slot);
            }
            // Released with no lock held.
            drop(slot);
            drop(made);
        }
    }

    /// `setPurgeableState:`: Metal's raw answer, with [`AGAIN_FAILED`] where placing again failed.
    fn set_state(&self, requested: PurgeableState) -> Result<u64, String> {
        let mut slot = lock(&self.slot);
        if slot.lost {
            // Its contents were discarded: it is Empty, with memory or without, until NonVolatile brings it back. A query and
            // Empty change nothing, and Volatile asked of an Empty object leaves it Empty, as Metal does.
            if requested != PurgeableState::NonVolatile {
                return Ok(PurgeableState::Empty.raw());
            }
            if slot.object.is_none() {
                drop(slot);
                slot = match self.place(0) {
                    Ok(slot) => slot,
                    Err(why) => {
                        crate::objects::set_error(why);
                        return Ok(PurgeableState::Empty.raw() | AGAIN_FAILED);
                    }
                };
            }
            // Still lost unless another NonVolatile request brought it back while this one placed it.
            if slot.lost {
                slot.lost = false;
                slot.state = PurgeableState::NonVolatile;
                slot.since = Instant::now();
                self.purge.counts.empty_answers.fetch_add(1, Relaxed);
                return Ok(PurgeableState::Empty.raw());
            }
        }
        // Not lost: the resource has its object, and its state is the runtime's.
        let object = slot.object.clone().expect("a resource whose contents were not discarded has its object");
        let answer = object.state(requested)?;
        // What the runtime keeps: Volatile asked of an Empty object stays Empty.
        let now = match requested {
            PurgeableState::KeepCurrent => answer,
            _ => object.state(PurgeableState::KeepCurrent)?,
        };
        if now != slot.state {
            if discardable(now) && !discardable(slot.state) {
                let counts = &self.purge.counts;
                counts.declared_bytes.fetch_add(self.bytes, Relaxed);
                if self.pinned.load(SeqCst) {
                    counts.declared_pinned_bytes.fetch_add(self.bytes, Relaxed);
                }
            }
            slot.state = now;
            slot.since = Instant::now();
        }
        if discardable(now) {
            self.list();
        }
        Ok(answer.raw())
    }
}

impl<T: Kind> Candidate for Cell<T> {
    fn sweep(&self, cause: Cause, now: Instant, idle: Duration) -> Swept {
        let mut slot = match self.slot.try_lock() {
            Ok(slot) => slot,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            // A call is inside the resource.
            Err(TryLockError::WouldBlock) => return Swept::Left(Left::Used, self.bytes),
        };
        if slot.object.is_none() || !discardable(slot.state) || self.pinned.load(SeqCst) {
            self.listed.store(false, SeqCst);
            return Swept::Not;
        }
        // Read after the lock is taken: a use counts itself before it takes the lock (`Resource::hold`, `tie`, `pin`), so
        // either it is seen here or it finds the resource discarded and places it again.
        if self.holds.load(SeqCst) != 0 {
            return Swept::Left(Left::Used, self.bytes);
        }
        if self.ties.load(SeqCst) != 0 {
            self.listed.store(false, SeqCst);
            return Swept::Left(Left::Tied, self.bytes);
        }
        if cause == Cause::Age {
            let idle_since = self.purge.start + Duration::from_millis(self.idle_ms.load(SeqCst));
            if now.saturating_duration_since(slot.since.max(idle_since)) < idle {
                return Swept::Left(Left::Young, self.bytes);
            }
        }
        let object = slot.object.take();
        slot.state = PurgeableState::Empty;
        slot.since = now;
        slot.lost = true;
        self.listed.store(false, SeqCst);
        drop(slot);
        // The runtime's release: the placement is returned once no batch in flight uses it.
        drop(object);
        Swept::Discarded(self.bytes)
    }
}

impl<T: Kind> Drop for Cell<T> {
    fn drop(&mut self) {
        let slot = self.slot.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.object.is_none() {
            self.purge.counts.out_bytes.fetch_sub(self.bytes, Relaxed);
        }
    }
}

/// What a tie is made on: a cell of any kind.
trait Tied: Send + Sync {
    fn tie(&self);
    fn untie(&self);
}

impl<T: Kind> Tied for Cell<T> {
    fn tie(&self) {
        self.ties.fetch_add(1, SeqCst);
    }

    fn untie(&self) {
        if self.ties.fetch_sub(1, SeqCst) != 1 {
            return;
        }
        // The last tie went: a candidate again where it is Volatile or Empty with memory.
        let slot = lock(&self.slot);
        if slot.object.is_some() && discardable(slot.state) {
            self.list();
        }
    }
}

/// A view of a resource, a texture over a buffer, or a resource placed in a heap, held by what was made: the resource it was
/// made of is not discarded while it lives.
pub struct Tie(Arc<dyn Tied>);

impl Tie {
    /// A tie on `cell`, counted before the caller reads the cell's object ([`Cell::sweep`]).
    fn on(cell: Arc<dyn Tied>) -> Tie {
        cell.tie();
        Tie(cell)
    }
}

impl Drop for Tie {
    fn drop(&mut self) {
        self.0.untie();
    }
}

/// A resource's runtime object as a call uses it: borrowed from a fixed resource, or a handle of its own to a kept one's (a
/// retain, so a discard made while the call runs leaves the call's object whole).
pub enum Live<'a, T> {
    Fixed(&'a T),
    Placed(T),
}

impl<T> std::ops::Deref for Live<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Live::Fixed(object) => object,
            Live::Placed(object) => object,
        }
    }
}

/// A buffer, texture or heap behind a frontend handle.
pub enum Resource<T: Kind> {
    /// Never discarded: its object, and its tie on the resource it was made of where that one may be.
    Fixed(T, Option<Tie>),
    /// May be discarded while Volatile or Empty and idle.
    Kept(Arc<Cell<T>>),
}

impl<T: Kind> Resource<T> {
    /// `object`, never discarded.
    pub fn fixed(object: T) -> Resource<T> {
        Resource::Fixed(object, None)
    }

    /// `object`, made of the resource `tie` is on, never discarded.
    pub fn tied(object: T, tie: Option<Tie>) -> Resource<T> {
        Resource::Fixed(object, tie)
    }

    /// `object`, made of `recipe` by the device of `purge`, which may be discarded and made again of it; fixed where the device
    /// does not discard, and where the runtime keeps no purgeable state for the object (one with no allocation of its own: under
    /// 16 KiB, or memoryless), which can then never be Volatile and whose uses pay for no cell.
    pub fn kept(purge: &Arc<Purge>, object: T, recipe: T::Recipe) -> Resource<T> {
        if !purge.on || !keeps_state(&object) {
            return Resource::Fixed(object, None);
        }
        let bytes = object.bytes();
        let slot = Slot { object: Some(object), state: PurgeableState::NonVolatile, since: Instant::now(), lost: false };
        Resource::Kept(Arc::new_cyclic(|me| Cell {
            purge: Arc::clone(purge),
            recipe,
            bytes,
            slot: Mutex::new(slot),
            holds: AtomicU32::new(0),
            ties: AtomicU32::new(0),
            pinned: AtomicBool::new(false),
            idle_ms: AtomicU64::new(0),
            listed: AtomicBool::new(false),
            me: me.clone(),
        }))
    }

    /// The object for a call that uses the resource, placed again where it was discarded.
    pub fn live(&self) -> Result<Live<'_, T>, String> {
        match self {
            Resource::Fixed(object, _) => Ok(Live::Fixed(object)),
            Resource::Kept(cell) => cell.live().map(Live::Placed),
        }
    }

    /// The object where the resource has one, for a question that places nothing: `None` while it is discarded.
    pub fn peek(&self) -> Option<Live<'_, T>> {
        match self {
            Resource::Fixed(object, _) => Some(Live::Fixed(object)),
            Resource::Kept(cell) => cell.peek().map(Live::Placed),
        }
    }

    /// What a kept resource was made of.
    pub fn recipe(&self) -> Option<&T::Recipe> {
        match self {
            Resource::Fixed(..) => None,
            Resource::Kept(cell) => Some(&cell.recipe),
        }
    }

    /// The bytes of device memory the resource holds while placed.
    pub fn bytes(&self) -> u64 {
        match self {
            Resource::Fixed(object, _) => object.bytes(),
            Resource::Kept(cell) => cell.bytes,
        }
    }

    /// `setPurgeableState:` with Metal's state: Metal's raw answer, with [`AGAIN_FAILED`] where the contents were discarded and
    /// placing the resource again failed.
    pub fn set_state(&self, requested: PurgeableState) -> Result<u64, String> {
        match self {
            Resource::Fixed(object, _) => Ok(object.state(requested)?.raw()),
            Resource::Kept(cell) => cell.set_state(requested),
        }
    }

    /// A tie for what is about to be made of the resource (a view, a texture over it, a resource placed in it), taken before the
    /// resource's object is read: what a fixed resource is itself tied to, since that is whose memory the new one shares.
    pub fn tie(&self) -> Option<Tie> {
        match self {
            Resource::Fixed(_, tie) => tie.as_ref().map(|tie| Tie::on(Arc::clone(&tie.0))),
            Resource::Kept(cell) => {
                let cell: Arc<dyn Tied> = cell.clone();
                Some(Tie::on(cell))
            }
        }
    }

    /// A command buffer names the resource from now until [`unhold`](Self::unhold): it is not discarded meanwhile. Counted before
    /// the command's own use reads the resource's object.
    pub fn hold(&self) {
        if let Resource::Kept(cell) = self {
            cell.holds.fetch_add(1, SeqCst);
        }
    }

    /// The command buffer that held the resource completed, or went without a commit.
    pub fn unhold(&self) {
        if let Resource::Kept(cell) = self {
            cell.idle_ms.store(cell.purge.now_ms(), SeqCst);
            cell.holds.fetch_sub(1, SeqCst);
        }
    }

    /// The resource's CPU pointer or GPU address is about to be handed out: it is never discarded from now on. Set before the
    /// object is read, so a pointer handed out is the pointer of the placement the resource keeps.
    pub fn pin(&self) {
        if let Resource::Kept(cell) = self {
            cell.pinned.store(true, SeqCst);
        }
    }
}
