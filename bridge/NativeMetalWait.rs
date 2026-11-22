// SPDX-License-Identifier: MIT
//! Bounded consumer waits using the upstream completion callback and queue flush.
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
#[derive(Default)]
pub struct Completion {
    done: Mutex<bool>,
    changed: Condvar,
}
impl Completion {
    pub fn attach(
        command: &metal2vulkan_runtime::CommandBuffer,
    ) -> Result<Arc<Self>, metal2vulkan_runtime::Error> {
        let completion = Arc::new(Self::default());
        let signal = completion.clone();
        command.add_completed_handler(move |_| signal.signal())?;
        Ok(completion)
    }
    pub fn signal(&self) {
        *self.done.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.changed.notify_all();
    }
    pub fn wait(&self, timeout: Duration) -> bool {
        let began = Instant::now();
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        while !*done {
            let Some(left) = timeout.checked_sub(began.elapsed()) else {
                return false;
            };
            let result = self
                .changed
                .wait_timeout(done, left)
                .unwrap_or_else(|e| e.into_inner());
            done = result.0;
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_does_not_invalidate_late_completion() {
        let completion = Arc::new(Completion::default());
        assert!(!completion.wait(Duration::ZERO));
        completion.signal();
        assert!(completion.wait(Duration::ZERO));
    }
}
