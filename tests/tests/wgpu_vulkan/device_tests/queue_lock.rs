//! The lock that wgpu-hal takes around each command on its queue, and hands out.

use std::{sync::mpsc, thread, time::Duration};

use ash::vk;
use wgpu_hal::{
    api::Vulkan,
    vulkan::{call_log, QueueLockGuard},
};

/// How long a submission that must wait for the lock is given to go through anyway.
const BLOCKED_FOR: Duration = Duration::from_millis(500);
/// How long a submission that must not wait is given to go through: generous, for a loaded
/// machine.
const FINISH_WITHIN: Duration = Duration::from_secs(10);

/// While the application holds the lock that wgpu-hal hands out (`Queue::queue_lock`), another
/// thread's submission waits for it, and goes through once it is released.
#[test]
fn a_submission_waits_for_the_lock_handed_out() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    for adapter in super::adapters(&instance) {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let lock = {
            // SAFETY: Only the lock is taken.
            let hal = unsafe { queue.as_hal::<Vulkan>() }.unwrap();
            hal.queue_lock()
                .expect("a queue created without flags takes a lock")
        };
        let guard = QueueLockGuard::new(&*lock, super::QUEUE);
        let (submitted, submission) = mpsc::channel();
        let submitter = thread::spawn({
            let queue = queue.clone();
            move || {
                queue.submit([]);
                submitted.send(()).unwrap();
            }
        });
        assert!(
            submission.recv_timeout(BLOCKED_FOR).is_err(),
            "a submission went through while the lock was held"
        );
        drop(guard);
        submission
            .recv_timeout(FINISH_WITHIN)
            .expect("the submission did not go through once the lock was released");
        submitter.join().unwrap();
        drop((queue, device));
    }
}

/// A raw submitter that takes the lock wgpu-hal hands out around each `vkQueueSubmit`, as FFmpeg's
/// `lock_queue` does, and wgpu never use the queue at once: on another thread, a bounded number of
/// raw submissions race wgpu's. The wrappers count the queue commands in progress at once, and the
/// validation layer's thread-safety checker, where it is installed, reports a concurrent access.
#[test]
fn a_raw_submitter_and_wgpu_never_use_the_queue_at_once() {
    const ROUNDS: usize = 200;
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, messenger)) =
        super::instance(wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG)
    else {
        return;
    };
    for adapter in super::adapters(&instance) {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let (raw_device, raw_queue, lock) = {
            // SAFETY: The raw submitter takes the queue lock around its queue commands.
            let hal = unsafe { queue.as_hal::<Vulkan>() }.unwrap();
            let lock = hal
                .queue_lock()
                .expect("a queue created without flags takes a lock");
            (hal.raw_device().clone(), hal.as_raw(), lock)
        };
        call_log::take();
        thread::scope(|scope| {
            scope.spawn(|| {
                // SAFETY: A fence of the device, waited for before it is reset and destroyed.
                let fence =
                    unsafe { raw_device.create_fence(&vk::FenceCreateInfo::default(), None) }
                        .unwrap();
                for _ in 0..ROUNDS {
                    {
                        let _guard = QueueLockGuard::new(&*lock, super::QUEUE);
                        // SAFETY: The queue lock is held; an empty submission signals the fence.
                        unsafe { raw_device.queue_submit(raw_queue, &[], fence) }.unwrap();
                    }
                    // SAFETY: The fence was submitted above.
                    unsafe {
                        raw_device
                            .wait_for_fences(&[fence], true, u64::MAX)
                            .unwrap();
                        raw_device.reset_fences(&[fence]).unwrap();
                    }
                }
                // SAFETY: Its last submission is complete.
                unsafe { raw_device.destroy_fence(fence, None) };
            });
            for round in 0..ROUNDS {
                queue.submit([]);
                if round % 16 == 0 {
                    device.poll(wgpu::PollType::Poll).unwrap();
                }
            }
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        drop((queue, device));
        let recorded = call_log::take();
        let submissions = recorded
            .calls
            .iter()
            .filter(|call| call.name == "vkQueueSubmit")
            .count();
        assert!(submissions >= 2 * ROUNDS, "{submissions} submissions");
        assert_eq!(recorded.most_queue_commands_at_once, 1);
        if let Some(messenger) = &messenger {
            assert_eq!(messenger.take(), [] as [String; 0]);
        }
    }
}
