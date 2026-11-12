//! The lock that serializes host access to the device's queue with code outside wgpu-hal.

use core::{fmt, marker::PhantomData};

use wgpu_sync::{Condvar, CondvarMutex};

/// A lock that serializes host access to a Vulkan queue, which Vulkan requires to be externally
/// synchronized: no two commands that take the queue, such as `vkQueueSubmit`,
/// `vkQueuePresentKHR` and `vkQueueWaitIdle`, may overlap.
///
/// wgpu-hal takes its device's lock around each such command it makes, for the length of that
/// call ([`Queue::queue_lock`](super::Queue::queue_lock)). Code outside wgpu-hal that accesses the
/// same queue takes it around each of its own.
///
/// # Safety
///
/// An implementation must:
/// - return from [`lock`](Self::lock) only once no other acquisition is held, the last
///   [`unlock`](Self::unlock) happening before it (Release/Acquire, as any mutex provides);
/// - allow `unlock` on the thread that called `lock`;
/// - not unwind out of `lock` or `unlock`, which C callbacks may call.
pub unsafe trait QueueLock: Send + Sync {
    /// Blocks until the calling thread holds the lock.
    ///
    /// A thread holds at most one acquisition, and calls no function of wgpu or wgpu-hal while it
    /// holds one: they take the lock themselves.
    fn lock(&self);

    /// Releases the lock.
    ///
    /// # Safety
    ///
    /// The calling thread holds an acquisition of this lock, taken with [`lock`](Self::lock).
    unsafe fn unlock(&self);
}

/// wgpu-hal's own [`QueueLock`]: a binary semaphore, so that `lock` and `unlock` need no guard.
pub(super) struct DeviceQueueLock {
    locked: CondvarMutex<bool>,
    unlocked: Condvar,
}

impl DeviceQueueLock {
    pub(super) fn new() -> Self {
        Self {
            locked: CondvarMutex::new(false),
            unlocked: Condvar::new(),
        }
    }
}

// SAFETY: `lock` returns once it has set `locked`, which `unlock` clears, both under the mutex,
// whose release and acquisition order them. Neither unwinds.
unsafe impl QueueLock for DeviceQueueLock {
    fn lock(&self) {
        let mut locked = self.locked.lock();
        while *locked {
            self.unlocked.wait(&mut locked);
        }
        *locked = true;
    }

    unsafe fn unlock(&self) {
        *self.locked.lock() = false;
        self.unlocked.notify_one();
    }
}

impl fmt::Debug for DeviceQueueLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceQueueLock")
            .field("locked", &*self.locked.lock())
            .finish()
    }
}

/// An acquisition of a [`QueueLock`], released when it drops, on the thread that took it.
pub struct QueueLockGuard<'a> {
    lock: &'a dyn QueueLock,
    _not_send: PhantomData<*const ()>,
}

impl<'a> QueueLockGuard<'a> {
    /// Takes `lock` until the guard drops.
    pub fn new(lock: &'a dyn QueueLock) -> Self {
        lock.lock();
        Self {
            lock,
            _not_send: PhantomData,
        }
    }
}

impl Drop for QueueLockGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: This guard's thread took the lock in `new`, and has not released it.
        unsafe { self.lock.unlock() };
    }
}

impl fmt::Debug for QueueLockGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueLockGuard").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
