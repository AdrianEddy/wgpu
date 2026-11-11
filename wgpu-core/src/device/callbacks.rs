//! Delivery of the application's callbacks.
//!
//! The application's callbacks may call wgpu-core again, and wgpu-core's locks are not
//! reentrant, so wgpu-core calls them only while it holds no lock. Code that has callbacks to
//! call, wherever it runs and whatever it holds, only queues them on the instance
//! ([`PendingCallbacks::defer`]). Each entry point that delivers callbacks opens a
//! [`CallbackScope`] before it takes any lock, and when the outermost scope on the thread closes,
//! [`PendingCallbacks::deliver`], the one function that calls the application's callbacks, calls
//! the queued ones.
//!
//! The callbacks are queued in batches, one per operation that produced them, and a batch is
//! called on one thread, in order, so that the order that a batch promises holds: mapping
//! callbacks before the work-done callbacks of the same submission. A delivery nested in a
//! callback, by a wgpu call that the callback makes, calls the rest of the running batch first,
//! so that batches are called in the order they were queued. A callback that panics leaves the
//! rest of its batch at the front of the queue, for the next delivery.

use alloc::{collections::VecDeque, sync::Arc};
use core::{mem, ops::Deref};

use wgpu_sync::atomic::{AtomicBool, Ordering};

use super::{
    queue::SubmittedWorkDoneClosure, BufferMapPendingClosure, DeviceLostInvocation, UserClosures,
};
use crate::ray_tracing::BlasCompactReadyPendingClosure;

/// A callback of the application, with what to call it with.
pub(crate) enum PendingCallback {
    Mapping(BufferMapPendingClosure),
    BlasCompactReady(BlasCompactReadyPendingClosure),
    WorkDone(SubmittedWorkDoneClosure),
    DeviceLost(DeviceLostInvocation),
}

impl PendingCallback {
    fn call(self) {
        match self {
            Self::Mapping((mut callback, status)) => {
                if let Some(callback) = callback.take() {
                    callback(status);
                }
            }
            Self::BlasCompactReady((mut callback, status)) => {
                if let Some(callback) = callback.take() {
                    callback(status);
                }
            }
            Self::WorkDone(closure) => closure(),
            Self::DeviceLost(invocation) => invocation.call(),
        }
    }
}

/// Callbacks to call on one thread, in order.
type Batch = VecDeque<PendingCallback>;

impl UserClosures {
    /// The closures, in the order to call them.
    ///
    /// Mapping callbacks come before work-done callbacks: the spec requires every mapping callback
    /// registered before an `onSubmittedWorkDone` callback to be called before it.
    fn into_batch(self) -> Batch {
        let Self {
            mappings,
            blas_compact_ready,
            submissions,
            device_lost_invocations,
        } = self;
        mappings
            .into_iter()
            .map(PendingCallback::Mapping)
            .chain(
                blas_compact_ready
                    .into_iter()
                    .map(PendingCallback::BlasCompactReady),
            )
            .chain(submissions.into_iter().map(PendingCallback::WorkDone))
            .chain(
                device_lost_invocations
                    .into_iter()
                    .map(PendingCallback::DeviceLost),
            )
            .collect()
    }
}

#[cfg(feature = "std")]
std::thread_local! {
    /// How many [`CallbackScope`]s and [`InternalRef`]s are open on this thread.
    static SCOPE_DEPTH: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };

    /// The rests of the batches that this thread is delivering, each while one of its callbacks
    /// runs, with the address of their instance's [`PendingCallbacks`]: a delivery nested in that
    /// callback calls the rest first. Other threads never see them, so a batch is still called on
    /// one thread.
    static PARKED: core::cell::RefCell<alloc::vec::Vec<(usize, Batch)>> =
        const { core::cell::RefCell::new(alloc::vec::Vec::new()) };
}

/// The callbacks of an instance's devices, waiting to be delivered.
#[derive(Default)]
pub(crate) struct PendingCallbacks {
    /// Unranked, like `ErrorSink`'s mutex: it is only held to queue or take a batch, never while
    /// calling or dropping anything of the application's, so it can be locked under any lock.
    batches: wgpu_sync::Mutex<VecDeque<Batch>>,
    /// Whether `batches` may hold a batch: set by every push, and cleared by the pop that empties
    /// it, both with `batches` locked. A delivery that finds it clear takes no lock, so that when
    /// there is nothing to deliver, the common case, the threads that deliver write to no shared
    /// memory.
    queued: AtomicBool,
    /// How many [`CallbackScope`]s and [`InternalRef`]s are open. Without `std`, an instance is
    /// used by one thread.
    #[cfg(not(feature = "std"))]
    depth: core::cell::Cell<usize>,
    /// The rests of the batches being delivered, as `PARKED` holds them with `std`.
    #[cfg(not(feature = "std"))]
    parked: core::cell::RefCell<alloc::vec::Vec<Batch>>,
}

impl PendingCallbacks {
    /// Queues `closures`, to be called in their order by the next delivery.
    pub(crate) fn defer(&self, closures: UserClosures) {
        let batch = closures.into_batch();
        if !batch.is_empty() {
            self.push(batch, Placement::Back);
        }
    }

    /// Queues one callback, to be called by the next delivery.
    pub(crate) fn defer_one(&self, callback: PendingCallback) {
        self.push(VecDeque::from([callback]), Placement::Back);
    }

    fn push(&self, batch: Batch, placement: Placement) {
        let mut batches = self.batches.lock();
        match placement {
            Placement::Front => batches.push_front(batch),
            Placement::Back => batches.push_back(batch),
        }
        self.queued.store(true, Ordering::Release);
    }

    /// The next batch to call: the rest of a batch that this thread is delivering, if a delivery
    /// is nested in one of its callbacks, or else the first batch queued.
    fn next_batch(&self) -> Option<Batch> {
        if let Some(rest) = self.unpark() {
            return Some(rest);
        }
        if !self.queued.load(Ordering::Acquire) {
            return None;
        }
        let mut batches = self.batches.lock();
        let batch = batches.pop_front();
        if batches.is_empty() {
            self.queued.store(false, Ordering::Release);
        }
        batch
    }

    /// Sets `rest`, the rest of a batch that this thread is delivering, aside while one of its
    /// callbacks runs. Gives it back if it cannot.
    fn park(&self, rest: Batch) -> Result<(), Batch> {
        #[cfg(feature = "std")]
        {
            let key = core::ptr::from_ref(self) as usize;
            let mut rest = Some(rest);
            let _ = PARKED.try_with(|parked| {
                if let Some(rest) = rest.take() {
                    parked.borrow_mut().push((key, rest));
                }
            });
            match rest {
                None => Ok(()),
                Some(rest) => Err(rest),
            }
        }
        #[cfg(not(feature = "std"))]
        {
            self.parked.borrow_mut().push(rest);
            Ok(())
        }
    }

    /// Takes the rest of a batch of this instance that this thread set aside last, if any.
    fn unpark(&self) -> Option<Batch> {
        #[cfg(feature = "std")]
        {
            let key = core::ptr::from_ref(self) as usize;
            PARKED
                .try_with(|parked| {
                    let mut parked = parked.borrow_mut();
                    let index = parked.iter().rposition(|&(parker, _)| parker == key)?;
                    Some(parked.remove(index).1)
                })
                .ok()
                .flatten()
        }
        #[cfg(not(feature = "std"))]
        self.parked.borrow_mut().pop()
    }

    /// Calls the queued callbacks, a batch at a time. The only function that calls the
    /// application's callbacks: [`CallbackScope`] calls it when the outermost scope on the thread
    /// closes, so this thread holds no lock.
    ///
    /// Nothing is delivered while the thread unwinds; the callbacks wait for the next delivery.
    fn deliver(&self) {
        #[cfg(feature = "std")]
        if std::thread::panicking() {
            return;
        }
        while let Some(batch) = self.next_batch() {
            let mut running = RunningBatch {
                callbacks: self,
                batch,
                parked: false,
            };
            while let Some(callback) = running.batch.pop_front() {
                crate::lock::assert_no_locks_held();
                running.park();
                callback.call();
                running.unpark();
            }
        }
    }

    /// Runs `f`, application code that wgpu-core calls directly, as if no [`CallbackScope`] were
    /// open, so that the wgpu calls it makes deliver callbacks as they would anywhere else. The
    /// uncaptured error handler is called this way: called inside an entry point's scope, a
    /// handler that waited for a callback would wait forever.
    ///
    /// Call it with no lock held: the callbacks that `f`'s calls deliver run with what the thread
    /// holds.
    pub(crate) fn outside_scopes<R>(&self, f: impl FnOnce() -> R) -> R {
        /// Puts the depth back when `f` returns or unwinds.
        struct Restore<'a> {
            #[cfg_attr(feature = "std", expect(dead_code))]
            callbacks: &'a PendingCallbacks,
            depth: usize,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                #[cfg(feature = "std")]
                let _ = SCOPE_DEPTH.try_with(|depth| depth.set(self.depth));
                #[cfg(not(feature = "std"))]
                self.callbacks.depth.set(self.depth);
            }
        }
        #[cfg(feature = "std")]
        let depth = SCOPE_DEPTH.try_with(|depth| depth.replace(0)).unwrap_or(0);
        #[cfg(not(feature = "std"))]
        let depth = self.depth.replace(0);
        let _restore = Restore {
            callbacks: self,
            depth,
        };
        f()
    }

    fn enter_scope(&self) {
        #[cfg(feature = "std")]
        let _ = SCOPE_DEPTH.try_with(|depth| depth.set(depth.get() + 1));
        #[cfg(not(feature = "std"))]
        self.depth.set(self.depth.get() + 1);
    }

    /// Returns whether the scope that closed was the outermost one.
    fn exit_scope(&self) -> bool {
        #[cfg(feature = "std")]
        {
            SCOPE_DEPTH
                .try_with(|depth| {
                    depth.set(depth.get() - 1);
                    depth.get() == 0
                })
                .unwrap_or(false)
        }
        #[cfg(not(feature = "std"))]
        {
            self.depth.set(self.depth.get() - 1);
            self.depth.get() == 0
        }
    }
}

impl Drop for PendingCallbacks {
    fn drop(&mut self) {
        // The instance is gone, and the callbacks still queued are dropped uncalled, with what
        // they captured, which belongs to the application.
        if !self.batches.get_mut().is_empty() {
            crate::lock::assert_no_locks_held();
        }
    }
}

/// Where [`PendingCallbacks::push`] queues a batch.
enum Placement {
    Front,
    Back,
}

/// A batch being called. While one of its callbacks runs, the rest of the batch is parked (see
/// [`PendingCallbacks::park`]). If a callback panics, dropping this puts the rest of the batch
/// back at the front of the queue.
struct RunningBatch<'a> {
    callbacks: &'a PendingCallbacks,
    batch: Batch,
    /// Whether the rest of the batch is parked.
    parked: bool,
}

impl RunningBatch<'_> {
    /// Parks the rest of the batch, before one of its callbacks runs.
    fn park(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        match self.callbacks.park(mem::take(&mut self.batch)) {
            Ok(()) => self.parked = true,
            Err(rest) => self.batch = rest,
        }
    }

    /// Takes back what a delivery nested in the callback left of the rest of the batch.
    fn unpark(&mut self) {
        if mem::take(&mut self.parked) {
            if let Some(rest) = self.callbacks.unpark() {
                self.batch = rest;
            }
        }
    }
}

impl Drop for RunningBatch<'_> {
    fn drop(&mut self) {
        self.unpark();
        if !self.batch.is_empty() {
            let rest = mem::take(&mut self.batch);
            self.callbacks.push(rest, Placement::Front);
        }
    }
}

/// Delivers the queued callbacks when it closes, if it is the outermost scope open on the thread.
///
/// Open one at the start of each entry point that delivers callbacks, before taking any lock, and
/// keep it until the entry point returns.
#[must_use]
pub(crate) struct CallbackScope<'a> {
    callbacks: &'a PendingCallbacks,
}

impl<'a> CallbackScope<'a> {
    pub(crate) fn new(callbacks: &'a PendingCallbacks) -> Self {
        callbacks.enter_scope();
        Self { callbacks }
    }
}

impl Drop for CallbackScope<'_> {
    fn drop(&mut self) {
        if self.callbacks.exit_scope() {
            self.callbacks.deliver();
        }
    }
}

/// A reference to `T` that wgpu-core holds for itself, in a scope that delivers nothing.
///
/// If the application drops its own references meanwhile, dropping this drops `T` where wgpu-core
/// may hold locks, so `T`'s drop must not deliver the callbacks it queues: they wait for the next
/// delivery.
pub(crate) struct InternalRef<'a, T> {
    value: Option<Arc<T>>,
    callbacks: &'a PendingCallbacks,
}

impl<'a, T> InternalRef<'a, T> {
    pub(crate) fn new(value: Arc<T>, callbacks: &'a PendingCallbacks) -> Self {
        callbacks.enter_scope();
        Self {
            value: Some(value),
            callbacks,
        }
    }
}

impl<T> Deref for InternalRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.value
            .as_ref()
            .expect("an internal reference is held until it is dropped")
    }
}

impl<T> Drop for InternalRef<'_, T> {
    fn drop(&mut self) {
        // Drop the reference inside the scope, then close the scope without delivering.
        drop(self.value.take());
        let _ = self.callbacks.exit_scope();
    }
}

#[cfg(test)]
mod tests {
    use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};
    use core::panic::AssertUnwindSafe;
    use std::{panic::catch_unwind, sync::Mutex};

    use super::{CallbackScope, InternalRef, PendingCallback, PendingCallbacks, Placement};

    type Calls = Arc<Mutex<Vec<u32>>>;

    /// A callback that records `number` in `calls`.
    fn record(calls: &Calls, number: u32) -> PendingCallback {
        let calls = Arc::clone(calls);
        PendingCallback::WorkDone(Box::new(move || calls.lock().unwrap().push(number)))
    }

    fn queue(callbacks: &PendingCallbacks, batch: impl IntoIterator<Item = PendingCallback>) {
        callbacks.push(batch.into_iter().collect::<VecDeque<_>>(), Placement::Back);
    }

    #[test]
    fn only_the_outermost_scope_delivers() {
        let callbacks = PendingCallbacks::default();
        let calls = Calls::default();
        let outer = CallbackScope::new(&callbacks);
        let inner = CallbackScope::new(&callbacks);
        queue(&callbacks, [record(&calls, 1)]);
        drop(inner);
        assert_eq!(*calls.lock().unwrap(), []);
        drop(outer);
        assert_eq!(*calls.lock().unwrap(), [1]);
    }

    #[test]
    fn an_internal_reference_delivers_nothing() {
        let callbacks = PendingCallbacks::default();
        let calls = Calls::default();
        let reference = InternalRef::new(Arc::new(()), &callbacks);
        queue(&callbacks, [record(&calls, 1)]);
        drop(reference);
        assert_eq!(*calls.lock().unwrap(), []);
        drop(CallbackScope::new(&callbacks));
        assert_eq!(*calls.lock().unwrap(), [1]);
    }

    #[test]
    fn nothing_is_delivered_while_the_thread_unwinds() {
        let callbacks = PendingCallbacks::default();
        let calls = Calls::default();
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _scope = CallbackScope::new(&callbacks);
            queue(&callbacks, [record(&calls, 1)]);
            panic!("an entry point panics");
        }));
        assert!(unwound.is_err());
        assert_eq!(*calls.lock().unwrap(), []);
        drop(CallbackScope::new(&callbacks));
        assert_eq!(*calls.lock().unwrap(), [1]);
    }

    /// A delivery nested in a callback calls the rest of the running batch before later batches.
    #[test]
    fn a_nested_delivery_calls_the_rest_of_the_running_batch_first() {
        let callbacks = Arc::new(PendingCallbacks::default());
        let calls = Calls::default();
        let nested = {
            let (callbacks, calls) = (Arc::clone(&callbacks), Arc::clone(&calls));
            PendingCallback::WorkDone(Box::new(move || {
                calls.lock().unwrap().push(1);
                queue(&callbacks, [record(&calls, 4)]);
                drop(CallbackScope::new(&callbacks));
                calls.lock().unwrap().push(5);
            }))
        };
        queue(&callbacks, [nested, record(&calls, 2), record(&calls, 3)]);
        drop(CallbackScope::new(&callbacks));
        assert_eq!(*calls.lock().unwrap(), [1, 2, 3, 4, 5]);
    }

    /// A nested delivery calls the rest of the running batch even when nothing else is queued,
    /// although the queue then looks empty without its lock.
    #[test]
    fn a_nested_delivery_with_nothing_queued_calls_the_rest_of_the_running_batch() {
        let callbacks = Arc::new(PendingCallbacks::default());
        let calls = Calls::default();
        let nested = {
            let (callbacks, calls) = (Arc::clone(&callbacks), Arc::clone(&calls));
            PendingCallback::WorkDone(Box::new(move || {
                calls.lock().unwrap().push(1);
                drop(CallbackScope::new(&callbacks));
                calls.lock().unwrap().push(3);
            }))
        };
        queue(&callbacks, [nested, record(&calls, 2)]);
        drop(CallbackScope::new(&callbacks));
        assert_eq!(*calls.lock().unwrap(), [1, 2, 3]);
    }

    /// A callback that panics in a nested delivery leaves the rest in order.
    #[test]
    fn a_panic_in_a_nested_delivery_keeps_the_order() {
        let callbacks = Arc::new(PendingCallbacks::default());
        let calls = Calls::default();
        let nested = {
            let (callbacks, calls) = (Arc::clone(&callbacks), Arc::clone(&calls));
            PendingCallback::WorkDone(Box::new(move || {
                calls.lock().unwrap().push(1);
                let panics = PendingCallback::WorkDone(Box::new(|| panic!("a callback panics")));
                queue(&callbacks, [panics, record(&calls, 4)]);
                drop(CallbackScope::new(&callbacks));
                calls.lock().unwrap().push(99);
            }))
        };
        queue(&callbacks, [nested, record(&calls, 2), record(&calls, 3)]);
        queue(&callbacks, [record(&calls, 5)]);
        let delivered = catch_unwind(AssertUnwindSafe(|| {
            drop(CallbackScope::new(&callbacks));
        }));
        // [5] was queued before the nested callback queued [panics, 4].
        assert!(delivered.is_err());
        assert_eq!(*calls.lock().unwrap(), [1, 2, 3, 5]);
        drop(CallbackScope::new(&callbacks));
        assert_eq!(*calls.lock().unwrap(), [1, 2, 3, 5, 4]);
    }

    /// Threads that queue and deliver at once call every callback once.
    ///
    /// Only with `send_sync`: without it the queued callbacks need not be `Send`, and without
    /// `std` an instance is used by one thread, so `PendingCallbacks` is not `Sync`.
    #[cfg(send_sync)]
    #[test]
    fn concurrent_deliveries_call_every_callback_once() {
        const THREADS: u32 = 4;
        const BATCHES: u32 = 10_000;
        let callbacks = Arc::new(PendingCallbacks::default());
        let calls = Calls::default();
        let threads = (0..THREADS)
            .map(|thread| {
                let (callbacks, calls) = (Arc::clone(&callbacks), Arc::clone(&calls));
                std::thread::spawn(move || {
                    for batch in 0..BATCHES {
                        let scope = CallbackScope::new(&callbacks);
                        callbacks.defer_one(record(&calls, thread * BATCHES + batch));
                        drop(scope);
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        drop(CallbackScope::new(&callbacks));
        let mut calls = calls.lock().unwrap().clone();
        calls.sort_unstable();
        assert_eq!(calls, (0..THREADS * BATCHES).collect::<Vec<_>>());
    }

    #[test]
    fn a_panicking_callback_leaves_the_rest_of_its_batch_first() {
        let callbacks = PendingCallbacks::default();
        let calls = Calls::default();
        let panics = PendingCallback::WorkDone(Box::new(|| panic!("a callback panics")));
        queue(&callbacks, [panics, record(&calls, 1), record(&calls, 2)]);
        queue(&callbacks, [record(&calls, 3)]);

        let delivered = catch_unwind(AssertUnwindSafe(|| {
            drop(CallbackScope::new(&callbacks));
        }));
        assert!(delivered.is_err());
        assert_eq!(*calls.lock().unwrap(), []);
        drop(CallbackScope::new(&callbacks));
        assert_eq!(*calls.lock().unwrap(), [1, 2, 3]);
    }
}
