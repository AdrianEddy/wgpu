use alloc::{sync::Arc, vec::Vec};
use core::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use std::{panic, sync::mpsc, thread};

use super::{
    queue_lock_held_by_current_thread, DeviceQueueLock, QueueLock, QueueLockGuard, QueueLockScope,
    TrackedQueueLock,
};

/// A queue's scope, as wgpu-hal takes it.
const QUEUE: QueueLockScope = QueueLockScope::Queue {
    family_index: 0,
    queue_index: 0,
};

/// How long a thread that must stay blocked is given to finish anyway.
const BLOCKED_FOR: Duration = Duration::from_millis(300);
/// How long a thread that must not block is given to finish: generous, as a loaded machine can be
/// slow to schedule it.
const FINISH_WITHIN: Duration = Duration::from_secs(10);

/// Takes `lock` on another thread, and returns a receiver that gets a message once it holds it.
fn lock_on_another_thread(lock: &Arc<DeviceQueueLock>) -> mpsc::Receiver<()> {
    let (sender, receiver) = mpsc::channel();
    let lock = Arc::clone(lock);
    thread::spawn(move || {
        let _guard = QueueLockGuard::new(&*lock, QUEUE);
        sender.send(()).unwrap();
    });
    receiver
}

#[test]
fn the_lock_excludes_other_threads_until_it_is_released() {
    let lock = Arc::new(DeviceQueueLock::new());
    lock.lock(QUEUE);
    let locked = lock_on_another_thread(&lock);
    assert!(locked.recv_timeout(BLOCKED_FOR).is_err());
    // SAFETY: This thread took the lock above.
    unsafe { lock.unlock(QUEUE) };
    locked.recv_timeout(FINISH_WITHIN).unwrap();
}

#[test]
fn a_guard_releases_the_lock_when_it_drops() {
    let lock = Arc::new(DeviceQueueLock::new());
    let guard = QueueLockGuard::new(&*lock, QUEUE);
    let locked = lock_on_another_thread(&lock);
    assert!(locked.recv_timeout(BLOCKED_FOR).is_err());
    drop(guard);
    locked.recv_timeout(FINISH_WITHIN).unwrap();
}

#[test]
fn a_guard_releases_the_lock_when_its_thread_unwinds() {
    let lock = Arc::new(DeviceQueueLock::new());
    let unwound = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let _guard = QueueLockGuard::new(&*lock, QUEUE);
        panic!("unwinding with the lock held");
    }));
    assert!(unwound.is_err());
    lock_on_another_thread(&lock)
        .recv_timeout(FINISH_WITHIN)
        .unwrap();
}

#[test]
fn threads_never_hold_the_lock_at_once() {
    const THREADS: usize = 8;
    const ROUNDS: usize = 2000;
    let lock = Arc::new(DeviceQueueLock::new());
    let holding = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let threads = (0..THREADS)
        .map(|_| {
            let (lock, holding, most) =
                (Arc::clone(&lock), Arc::clone(&holding), Arc::clone(&most));
            thread::spawn(move || {
                for _ in 0..ROUNDS {
                    let _guard = QueueLockGuard::new(&*lock, QUEUE);
                    let now = holding.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    holding.fetch_sub(1, Ordering::SeqCst);
                }
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(most.load(Ordering::SeqCst), 1);
}

#[test]
fn scopes_overlap_unless_they_name_two_different_queues() {
    let other_index = QueueLockScope::Queue {
        family_index: 0,
        queue_index: 1,
    };
    let other_family = QueueLockScope::Queue {
        family_index: 1,
        queue_index: 0,
    };
    for (a, b, overlaps) in [
        (QUEUE, QUEUE, true),
        (QUEUE, other_index, false),
        (QUEUE, other_family, false),
        (QUEUE, QueueLockScope::AllQueues, true),
        (QueueLockScope::AllQueues, QueueLockScope::AllQueues, true),
    ] {
        assert_eq!(a.overlaps(b), overlaps, "{a:?} {b:?}");
        assert_eq!(b.overlaps(a), overlaps, "{b:?} {a:?}");
    }
}

/// A lock as wgpu-hal stores it, over its own lock.
fn tracked() -> Arc<TrackedQueueLock> {
    Arc::new(TrackedQueueLock::new(Arc::new(DeviceQueueLock::new())))
}

#[test]
fn a_lock_held_through_wgpu_hal_is_seen_until_it_is_released() {
    let lock = tracked();
    assert!(!queue_lock_held_by_current_thread());
    let guard = QueueLockGuard::new(&*lock, QUEUE);
    assert_eq!(queue_lock_held_by_current_thread(), cfg!(debug_assertions));
    drop(guard);
    assert!(!queue_lock_held_by_current_thread());
}

#[test]
fn a_tracked_lock_excludes_like_the_lock_it_wraps() {
    let inner = Arc::new(DeviceQueueLock::new());
    let lock = TrackedQueueLock::new(inner.clone());
    let guard = QueueLockGuard::new(&lock, QUEUE);
    let locked = lock_on_another_thread(&inner);
    assert!(locked.recv_timeout(BLOCKED_FOR).is_err());
    drop(guard);
    locked.recv_timeout(FINISH_WITHIN).unwrap();
}

#[test]
fn sequential_acquisitions_of_different_scopes_go_through() {
    let lock = tracked();
    drop(QueueLockGuard::new(&*lock, QueueLockScope::AllQueues));
    drop(QueueLockGuard::new(&*lock, QUEUE));
    drop(QueueLockGuard::new(
        &*lock,
        QueueLockScope::Queue {
            family_index: 1,
            queue_index: 0,
        },
    ));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "holds the queue lock already")]
fn taking_the_lock_again_panics_instead_of_waiting_for_itself() {
    let lock = tracked();
    let _held = QueueLockGuard::new(&*lock, QUEUE);
    let _again = QueueLockGuard::new(&*lock, QUEUE);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "holds the queue lock already")]
fn nesting_another_scope_panics() {
    let lock = tracked();
    let _all = QueueLockGuard::new(&*lock, QueueLockScope::AllQueues);
    let _queue = QueueLockGuard::new(&*lock, QUEUE);
}

/// A lock that one device hands out and another is given keeps its identity: taking it through
/// the second while it is held through the first panics.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "holds the queue lock already")]
fn a_lock_handed_out_and_given_again_keeps_its_identity() {
    let first = tracked();
    let handed_out: Arc<dyn QueueLock> = first.clone();
    let second = TrackedQueueLock::new(handed_out);
    assert_eq!(first.identity(), second.identity());
    let _held = QueueLockGuard::new(&*first, QUEUE);
    let _again = QueueLockGuard::new(&second, QUEUE);
}

/// An adapter over one process-wide lock, as an application might give each of its devices.
struct ProcessWide;

static PROCESS_WIDE: DeviceQueueLock = DeviceQueueLock {
    locked: wgpu_sync::CondvarMutex::new(false),
    unlocked: wgpu_sync::Condvar::new(),
};

// SAFETY: Delegates to `PROCESS_WIDE`.
unsafe impl QueueLock for ProcessWide {
    fn lock(&self, scope: QueueLockScope) {
        PROCESS_WIDE.lock(scope);
    }

    unsafe fn unlock(&self, scope: QueueLockScope) {
        // SAFETY: Our caller's.
        unsafe { PROCESS_WIDE.unlock(scope) };
    }

    fn identity(&self) -> *const () {
        let lock: *const DeviceQueueLock = &PROCESS_WIDE;
        lock.cast()
    }
}

/// One lock behind two adapters, given to two devices, is one lock: taking it through the second
/// while it is held through the first panics.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "holds the queue lock already")]
fn one_lock_behind_two_adapters_is_one_identity() {
    let first = TrackedQueueLock::new(Arc::new(ProcessWide));
    let second = TrackedQueueLock::new(Arc::new(ProcessWide));
    let _held = QueueLockGuard::new(&first, QUEUE);
    let _again = QueueLockGuard::new(&second, QUEUE);
}
