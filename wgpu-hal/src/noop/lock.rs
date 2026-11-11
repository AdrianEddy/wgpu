//! The lock of the noop backend's state.

use core::{
    cell::UnsafeCell,
    fmt,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use wgpu_sync::atomic::{AtomicBool, Ordering};

/// A spin lock.
///
/// The noop backend builds without `std` too, where `wgpu_sync::Mutex` is not `Sync`, while the
/// HAL's types must be `Sync` on native targets. Its state is locked only for a few instructions
/// at a time, and no code from outside the backend runs under the lock, so spinning is enough.
pub(super) struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: `lock` gives one thread at a time access to `value`, so sharing the lock between
// threads only moves `T` between them, as `std::sync::Mutex` does.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub(super) fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub(super) fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinLockGuard {
            lock: self,
            _value: PhantomData,
        }
    }
}

impl<T: Default> Default for SpinLock<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> fmt::Debug for SpinLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpinLock").finish_non_exhaustive()
    }
}

pub(super) struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    /// The guard hands out `&mut T`, so it is `Send` only if `T` is, and `Sync` only if `T` is,
    /// whatever the lock is.
    _value: PhantomData<&'a mut T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the lock.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}
