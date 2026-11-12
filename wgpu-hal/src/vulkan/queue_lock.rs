//! The lock that serializes host access to the device's queue with code outside wgpu-hal.

use alloc::sync::Arc;
use core::{fmt, marker::PhantomData};

use wgpu_sync::{Condvar, CondvarMutex};

/// Which queues an acquisition of a [`QueueLock`] covers.
///
/// A scope names a queue by family and index, so it does not tell apart queues of one family
/// created with different flags (`VkDeviceQueueCreateInfo::flags`): they share a scope, which only
/// over-excludes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueueLockScope {
    /// One queue of the device.
    Queue { family_index: u32, queue_index: u32 },
    /// Every queue of the device, as code outside wgpu-hal that calls `vkDeviceWaitIdle` needs.
    AllQueues,
}

impl QueueLockScope {
    /// Whether an acquisition of `self` and one of `other` must exclude each other.
    #[must_use]
    pub const fn overlaps(self, other: Self) -> bool {
        match (self, other) {
            (
                Self::Queue {
                    family_index,
                    queue_index,
                },
                Self::Queue {
                    family_index: other_family_index,
                    queue_index: other_queue_index,
                },
            ) => family_index == other_family_index && queue_index == other_queue_index,
            (Self::AllQueues, _) | (_, Self::AllQueues) => true,
        }
    }
}

/// A lock that serializes host access to a device's queues, which Vulkan requires to be
/// externally synchronized: no two commands that take a queue, such as `vkQueueSubmit`,
/// `vkQueuePresentKHR` and `vkQueueWaitIdle`, may overlap on it.
///
/// wgpu-hal takes its device's lock, with its queue's scope, around each such command it makes,
/// for the length of that call ([`Queue::queue_lock`](super::Queue::queue_lock)). Code outside
/// wgpu-hal that accesses the device's queues takes it, with the scope of the queues it accesses,
/// around each of its own. An application can give wgpu-hal its lock
/// ([`DeviceQueue::lock`](super::DeviceQueue::lock)), which wgpu-hal then always takes.
///
/// # Safety
///
/// An implementation must:
/// - return from [`lock`](Self::lock) only once no acquisition of an overlapping scope
///   ([`QueueLockScope::overlaps`]) is held, the last `unlock` of one happening before it
///   (Release/Acquire, as any mutex provides);
/// - allow `unlock` on the thread that called `lock`;
/// - not unwind out of `lock` or `unlock`, which C callbacks may call.
///
/// A caller holds at most one acquisition of a lock at a time, and calls no function of wgpu or
/// wgpu-hal while it holds one: they take the lock themselves. Nor may code that runs inside a
/// Vulkan call, such as a `log` logger or a debug messenger, take it.
pub unsafe trait QueueLock: Send + Sync {
    /// Blocks until the calling thread holds the lock for `scope`.
    fn lock(&self, scope: QueueLockScope);

    /// Releases an acquisition.
    ///
    /// # Safety
    ///
    /// The calling thread holds an acquisition of `scope`, taken with [`lock`](Self::lock), that
    /// it has not released.
    unsafe fn unlock(&self, scope: QueueLockScope);

    /// What tells this lock apart, for wgpu-hal's debug checks: its address. A lock that adapts a
    /// lock shared with others, such as a process-wide one, returns the shared lock's.
    #[doc(hidden)]
    fn identity(&self) -> *const () {
        let this: *const Self = self;
        this.cast()
    }
}

/// wgpu-hal's own [`QueueLock`]: one binary semaphore for every scope, so that `lock` and
/// `unlock` need no guard.
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

// SAFETY: Every scope sets `locked`, which `unlock` clears, both under the mutex, whose release
// and acquisition order them, so any two acquisitions exclude each other. Neither unwinds.
unsafe impl QueueLock for DeviceQueueLock {
    fn lock(&self, _scope: QueueLockScope) {
        let mut locked = self.locked.lock();
        while *locked {
            self.unlocked.wait(&mut locked);
        }
        *locked = true;
    }

    unsafe fn unlock(&self, _scope: QueueLockScope) {
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

/// A [`QueueLock`] as wgpu-hal stores and hands out: the application's lock or its own, and, in
/// debug builds, a record of the acquisitions each thread holds of it, so that wgpu-hal can catch
/// a holder that calls into it, or takes the lock again.
pub(super) struct TrackedQueueLock {
    inner: Arc<dyn QueueLock>,
}

impl TrackedQueueLock {
    pub(super) fn new(inner: Arc<dyn QueueLock>) -> Self {
        Self { inner }
    }
}

// SAFETY: Delegates to `inner`, which meets the requirements.
unsafe impl QueueLock for TrackedQueueLock {
    fn lock(&self, scope: QueueLockScope) {
        #[cfg(debug_assertions)]
        {
            assert!(
                !held::by_current_thread(self.identity()),
                "this thread holds the queue lock already: a holder must not take it again, nor \
                 call into wgpu, wgpu-core or wgpu-hal"
            );
            // Nothing allocates while the lock is held.
            held::reserve();
        }
        self.inner.lock(scope);
        #[cfg(debug_assertions)]
        held::insert(self.identity(), scope);
    }

    unsafe fn unlock(&self, scope: QueueLockScope) {
        // SAFETY: The calling thread holds the acquisition, as our caller guarantees.
        unsafe { self.inner.unlock(scope) };
        #[cfg(debug_assertions)]
        held::remove(self.identity(), scope);
    }

    fn identity(&self) -> *const () {
        self.inner.identity()
    }
}

impl fmt::Debug for TrackedQueueLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrackedQueueLock").finish_non_exhaustive()
    }
}

/// An acquisition of a [`QueueLock`], released when it drops, on the thread that took it.
#[must_use = "the lock is released when the guard drops"]
pub struct QueueLockGuard<'a> {
    lock: &'a dyn QueueLock,
    scope: QueueLockScope,
    _not_send: PhantomData<*const ()>,
}

impl<'a> QueueLockGuard<'a> {
    /// Takes `lock` for `scope` until the guard drops.
    pub fn new(lock: &'a dyn QueueLock, scope: QueueLockScope) -> Self {
        lock.lock(scope);
        Self {
            lock,
            scope,
            _not_send: PhantomData,
        }
    }
}

impl Drop for QueueLockGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: This guard's thread took the lock for `scope` in `new`, and has not released it.
        unsafe { self.lock.unlock(self.scope) };
    }
}

impl fmt::Debug for QueueLockGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueLockGuard")
            .field("scope", &self.scope)
            .finish()
    }
}

/// Whether the calling thread holds a queue lock through wgpu-hal: taken with
/// [`super::Queue::lock_queue`] or the lock that [`super::Queue::queue_lock`] hands out, or by
/// wgpu-hal itself. A lock that the application took directly is not seen. Always `false` in
/// release builds, which track nothing.
///
/// wgpu-core checks it, with lock validation, whenever it takes a lock of its own.
#[doc(hidden)]
pub fn queue_lock_held_by_current_thread() -> bool {
    #[cfg(debug_assertions)]
    {
        held::any()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// The tracked queue locks that each thread holds, in debug builds.
#[cfg(debug_assertions)]
mod held {
    use super::QueueLockScope;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    std::thread_local! {
        /// The tracked locks the thread holds, by identity, with their scopes.
        static HELD: RefCell<Vec<(*const (), QueueLockScope)>> = const { RefCell::new(Vec::new()) };
    }

    /// Makes room for one more acquisition, before the lock is taken.
    pub(super) fn reserve() {
        let _ = HELD.try_with(|held| held.borrow_mut().reserve(1));
    }

    pub(super) fn insert(lock: *const (), scope: QueueLockScope) {
        let _ = HELD.try_with(|held| held.borrow_mut().push((lock, scope)));
    }

    pub(super) fn remove(lock: *const (), scope: QueueLockScope) {
        let _ = HELD.try_with(|held| {
            let mut held = held.borrow_mut();
            if let Some(index) = held.iter().rposition(|&entry| entry == (lock, scope)) {
                held.remove(index);
            }
        });
    }

    pub(super) fn by_current_thread(lock: *const ()) -> bool {
        HELD.try_with(|held| held.borrow().iter().any(|&(id, _)| id == lock))
            .unwrap_or(false)
    }

    pub(super) fn any() -> bool {
        HELD.try_with(|held| !held.borrow().is_empty())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests;
