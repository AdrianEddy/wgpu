use alloc::{sync::Arc, vec::Vec};
use core::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use std::{panic, sync::mpsc, thread};

use super::{DeviceQueueLock, QueueLock, QueueLockGuard};

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
        let _guard = QueueLockGuard::new(&*lock);
        sender.send(()).unwrap();
    });
    receiver
}

#[test]
fn the_lock_excludes_other_threads_until_it_is_released() {
    let lock = Arc::new(DeviceQueueLock::new());
    lock.lock();
    let locked = lock_on_another_thread(&lock);
    assert!(locked.recv_timeout(BLOCKED_FOR).is_err());
    // SAFETY: This thread took the lock above.
    unsafe { lock.unlock() };
    locked.recv_timeout(FINISH_WITHIN).unwrap();
}

#[test]
fn a_guard_releases_the_lock_when_it_drops() {
    let lock = Arc::new(DeviceQueueLock::new());
    let guard = QueueLockGuard::new(&*lock);
    let locked = lock_on_another_thread(&lock);
    assert!(locked.recv_timeout(BLOCKED_FOR).is_err());
    drop(guard);
    locked.recv_timeout(FINISH_WITHIN).unwrap();
}

#[test]
fn a_guard_releases_the_lock_when_its_thread_unwinds() {
    let lock = Arc::new(DeviceQueueLock::new());
    let unwound = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let _guard = QueueLockGuard::new(&*lock);
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
                    let _guard = QueueLockGuard::new(&*lock);
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
