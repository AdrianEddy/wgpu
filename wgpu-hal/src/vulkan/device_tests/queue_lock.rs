//! The lock that wgpu-hal hands out excludes its commands on the queue, and nothing else.

use core::time::Duration;
use std::{thread, time::Instant};

use crate::{Device as _, Queue as _, Surface as _};

/// How long a call that must stay blocked is given to finish anyway.
const BLOCKED_FOR: Duration = Duration::from_millis(300);

/// How long a call that must not wait for the lock is given to finish.
const GOES_THROUGH_WITHIN: Duration = Duration::from_secs(10);

/// Runs `call` on another thread while this one holds `queue`'s lock: `call` must not finish
/// until the lock is released, then must.
fn blocked_by_the_lock<R: Send>(queue: &super::super::Queue, call: impl FnOnce() -> R + Send) -> R {
    let guard = queue
        .lock_queue()
        .expect("a queue created without flags takes a lock");
    thread::scope(|scope| {
        let call = scope.spawn(call);
        thread::sleep(BLOCKED_FOR);
        assert!(
            !call.is_finished(),
            "the call went through with the lock held"
        );
        drop(guard);
        call.join().unwrap()
    })
}

/// Runs `call` on another thread while this one holds `queue`'s lock: `call` must finish all the
/// same, within [`GOES_THROUGH_WITHIN`]. The lock is released either way, so that a call that
/// waits for it fails the test instead of hanging it.
fn not_blocked_by_the_lock<R: Send>(
    queue: &super::super::Queue,
    call: impl FnOnce() -> R + Send,
) -> R {
    let guard = queue
        .lock_queue()
        .expect("a queue created without flags takes a lock");
    thread::scope(|scope| {
        let call = scope.spawn(call);
        let started = Instant::now();
        while !call.is_finished() && started.elapsed() < GOES_THROUGH_WITHIN {
            thread::sleep(Duration::from_millis(10));
        }
        let finished = call.is_finished();
        drop(guard);
        let result = call.join().unwrap();
        assert!(finished, "the call waited for the lock");
        result
    })
}

/// Submitting and presenting wait for the lock that the queue hands out, on each adapter that
/// supports a headless surface. So does tearing a swapchain down where it waits for the queue to
/// become idle; where it waits on its presents' fences, it does not.
#[test]
fn queue_commands_wait_for_the_lock_handed_out_and_present_fence_waits_do_not() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, surface)) = super::headless_instance(wgt::InstanceFlags::empty()) else {
        return;
    };
    for (exposed, caps) in super::adapters(&instance, &surface) {
        let (device, queue) = super::open(&exposed.adapter);
        std::println!("{:?}: {}", exposed.info.name, super::teardown_path(&device));
        let config = super::config(&caps);
        // SAFETY: The device's own fence.
        let fence = unsafe { device.create_fence() }.unwrap();
        // SAFETY: Each call meets wgpu-hal's requirements: every submission completes before the
        // surface is unconfigured; the threads call the queue one at a time.
        unsafe {
            surface.configure(&device, &config).unwrap();
            super::present_frame(&device, &queue, &surface, &fence, 1);
            blocked_by_the_lock(&queue, || queue.submit(&[], &[], (&fence, 2)).unwrap());
            assert!(device.wait(&fence, 2, None).unwrap());
            let acquired = surface
                .acquire_texture(Some(Duration::from_secs(1)), &fence)
                .unwrap();
            super::submit_for_present(&device, &queue, &acquired.texture, &fence, 3);
            blocked_by_the_lock(&queue, || {
                queue.present(&surface, acquired.texture, &[]).unwrap()
            });
            if device.presents_signal_fences() {
                not_blocked_by_the_lock(&queue, || surface.unconfigure(&device));
            } else {
                blocked_by_the_lock(&queue, || surface.unconfigure(&device));
            }
            device.destroy_fence(fence);
        }
    }
}
