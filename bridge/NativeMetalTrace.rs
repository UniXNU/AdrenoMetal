// SPDX-License-Identifier: MIT
//! Bounded bridge diagnostics using the existing C stage-record layout.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

pub type DiagnosticHook = extern "C" fn(u32, u64, u64, u64);
static HOOK: OnceLock<DiagnosticHook> = OnceLock::new();
static RING: OnceLock<Mutex<Ring>> = OnceLock::new();
const CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct StageRecord {
    pub when: u64,
    pub id: u64,
    pub sequence: u32,
    pub stage: u16,
    pub arg: u16,
}
struct Ring {
    next: u64,
    records: VecDeque<(u64, StageRecord)>,
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("SHENG_DIAG").as_deref() == Ok("1"))
}

pub fn set_diagnostic_hook(hook: DiagnosticHook) {
    let _ = HOOK.set(hook);
}

pub fn mark(id: u64, stage: u16, arg: u16) {
    if !enabled() {
        return;
    }
    let ring = RING.get_or_init(|| {
        Mutex::new(Ring {
            next: 1,
            records: VecDeque::with_capacity(CAPACITY),
        })
    });
    {
        let mut ring = ring.lock().unwrap_or_else(|e| e.into_inner());
        let sequence = ring.next;
        ring.next = sequence.saturating_add(1);
        if ring.records.len() == CAPACITY {
            ring.records.pop_front();
        }
        ring.records.push_back((
            sequence,
            StageRecord {
                when: metal2vulkan_runtime::host_time(),
                id,
                sequence: sequence as u32,
                stage,
                arg,
            },
        ));
    }
    // The external transport never runs under the ring's lock.
    if let Some(hook) = HOOK.get() {
        hook(39, id, stage as u64, arg as u64);
    }
}

pub fn read(from: u64, out: &mut [StageRecord]) -> (usize, u64) {
    let Some(ring) = RING.get() else {
        return (0, from.max(1));
    };
    let ring = ring.lock().unwrap_or_else(|e| e.into_inner());
    let oldest = ring
        .records
        .front()
        .map_or(ring.next, |(sequence, _)| *sequence);
    let mut next = from.max(oldest).max(1);
    let mut count = 0;
    for (sequence, record) in &ring.records {
        if *sequence < next {
            continue;
        }
        if count == out.len() {
            break;
        }
        out[count] = *record;
        count += 1;
        next = sequence.saturating_add(1);
    }
    (count, next)
}
