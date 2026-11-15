//! A lock that the application gives the device: wgpu-hal takes it around each command on the
//! queue, and around nothing else.

use std::sync::{Arc, Condvar, Mutex};

use ash::vk;
use wgpu_hal::{
    api::Vulkan,
    vulkan::{call_log, DeviceQueue, QueueLock, QueueLockScope},
};

/// A lock that excludes like a mutex, for every scope, tells the wrappers which thread holds it,
/// and records the scope of each acquisition. A binary semaphore: it keeps no guard state.
#[derive(Default)]
pub struct InstrumentedLock {
    locked: Mutex<bool>,
    unlocked: Condvar,
    pub scopes: Mutex<Vec<QueueLockScope>>,
}

// SAFETY: Every scope sets `locked`, which `unlock` clears, under the mutex. Neither unwinds.
unsafe impl QueueLock for InstrumentedLock {
    fn lock(&self, scope: QueueLockScope) {
        let mut locked = self.locked.lock().unwrap();
        while *locked {
            locked = self.unlocked.wait(locked).unwrap();
        }
        *locked = true;
        call_log::set_held(true);
        self.scopes.lock().unwrap().push(scope);
    }

    unsafe fn unlock(&self, _scope: QueueLockScope) {
        call_log::set_held(false);
        *self.locked.lock().unwrap() = false;
        self.unlocked.notify_one();
    }
}

/// Opens a device on `adapter` as an application would, with `vkCreateDevice` and
/// `device_from_raw`, on queue 0 of family 0, giving it `lock`.
pub fn open(adapter: &wgpu::Adapter, lock: Arc<dyn QueueLock>) -> (wgpu::Device, wgpu::Queue) {
    let features = wgpu::Features::empty();
    let open = {
        // SAFETY: Used to create a device, as `device_from_raw` requires.
        let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
        let extensions = hal.required_device_extensions(features);
        let mut physical_features = hal.physical_device_features(&extensions, features);
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(0)
            .queue_priorities(&priorities)];
        let names = extensions
            .iter()
            .map(|name| name.as_ptr())
            .collect::<Vec<_>>();
        let info = physical_features.add_to_device_create(
            vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&names),
        );
        // SAFETY: A valid create info, with wgpu-hal's required extensions and features.
        let raw = unsafe {
            hal.shared_instance().raw_instance().create_device(
                hal.raw_physical_device(),
                &info,
                None,
            )
        }
        .expect("vkCreateDevice failed");
        // SAFETY: Created above as `device_from_raw` requires; wgpu-hal destroys it.
        unsafe {
            hal.device_from_raw(
                raw,
                None,
                &extensions,
                &physical_features,
                features,
                &adapter.limits(),
                &wgpu::MemoryHints::default(),
                DeviceQueue::new(0, 0).lock(lock),
            )
        }
        .expect("device_from_raw failed")
    };
    // SAFETY: Opened from this adapter.
    unsafe { adapter.create_device_from_hal(open, &wgpu::DeviceDescriptor::default()) }
        .expect("create_device_from_hal failed")
}

/// Uses the device as an application would, so that wgpu makes each kind of queue call it makes
/// without a surface: a submission, a mapping, a wait for submitted work, a command buffer, and
/// dropping the queue right after a submission, then the device.
fn exercise(device: wgpu::Device, queue: wgpu::Queue) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("written then read"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[1; 256]);
    queue.submit([]);
    queue.write_buffer(&buffer, 0, &[2; 256]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert_eq!(buffer.slice(..).get_mapped_range().unwrap()[0], 2);
    buffer.unmap();
    let source = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("copied"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&source, 0, &[3; 256]);
    queue.on_submitted_work_done(|| ());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_buffer_to_buffer(&source, 0, &buffer, 0, 256);
    queue.submit([encoder.finish()]);
    // Dropping the queue right after that submission waits on the fence for it, then for the
    // queue to become idle: nothing else waits for the submission in between.
    drop((buffer, source));
    drop(queue);
    drop(device);
}

/// Checks the calls of a device given an instrumented lock.
fn check_locked(calls: &[call_log::Call]) {
    let queue_calls = calls
        .iter()
        .filter(|call| call.kind == call_log::Kind::Queue)
        .collect::<Vec<_>>();
    for name in ["vkQueueSubmit", "vkQueueWaitIdle"] {
        assert!(
            queue_calls.iter().any(|call| call.name == name),
            "{name} was never called: {calls:#?}"
        );
    }
    let unlocked = queue_calls
        .iter()
        .filter(|call| !call.locked)
        .collect::<Vec<_>>();
    assert!(
        unlocked.is_empty(),
        "queue commands without the lock: {unlocked:#?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.kind == call_log::Kind::DeviceWaitIdle),
        "vkDeviceWaitIdle was called: {calls:#?}"
    );
    let locked_waits = calls
        .iter()
        .filter(|call| call.kind == call_log::Kind::FenceWait && call.locked)
        .collect::<Vec<_>>();
    assert!(
        locked_waits.is_empty(),
        "fence waits with the lock held: {locked_waits:#?}"
    );
    // Dropping the queue waits on the fence after its last submission, and only then for idleness.
    let idle = calls
        .iter()
        .rposition(|call| call.name == "vkQueueWaitIdle")
        .unwrap();
    let last_submission = calls[..idle]
        .iter()
        .rposition(|call| call.name == "vkQueueSubmit")
        .unwrap();
    assert!(
        calls[last_submission..idle]
            .iter()
            .any(|call| call.kind == call_log::Kind::FenceWait),
        "no fence wait between the last submission and the idle wait: {calls:#?}"
    );
    // The device is destroyed last, with no queue command in progress.
    let destroyed = calls.last().unwrap();
    assert_eq!(destroyed.kind, call_log::Kind::DestroyDevice, "{calls:#?}");
    assert_eq!(destroyed.in_flight, 0);
}

/// Every queue command that wgpu makes, for submissions, mapping, polls and dropping the queue,
/// holds the lock that the application gave the device, with the scope of the device's queue.
/// None waits for the whole device, and no fence wait holds the lock.
#[test]
fn every_queue_command_holds_the_lock_the_application_gave() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    for flags in super::FLAGS {
        let Some((instance, _messenger)) = super::instance(flags) else {
            return;
        };
        for adapter in super::adapters(&instance) {
            let lock = Arc::new(InstrumentedLock::default());
            let (device, queue) = open(&adapter, lock.clone());
            call_log::take();
            exercise(device, queue);
            check_locked(&call_log::take().calls);
            let scopes = lock.scopes.lock().unwrap();
            assert!(!scopes.is_empty());
            assert!(
                scopes.iter().all(|&scope| scope == super::QUEUE),
                "{scopes:?}"
            );
        }
    }
}

/// A lock given with an internally synchronized queue is taken all the same.
#[test]
fn a_lock_given_with_an_internally_synchronized_queue_is_taken() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    for adapter in super::adapters(&instance) {
        if !super::internal::supports_internally_synchronized_queues(&adapter) {
            continue;
        }
        let lock = Arc::new(InstrumentedLock::default());
        let open = super::internal::open_with(
            &adapter,
            DeviceQueue::new(0, 0)
                .create_flags(super::internal::INTERNALLY_SYNCHRONIZED)
                .lock(lock.clone()),
            true,
        )
        .unwrap();
        assert!(open.queue.queue_lock().is_some());
        // SAFETY: Opened from this adapter.
        let (device, queue) =
            unsafe { adapter.create_device_from_hal(open, &wgpu::DeviceDescriptor::default()) }
                .unwrap();
        call_log::take();
        exercise(device, queue);
        check_locked(&call_log::take().calls);
    }
}

/// Runs `call` holding the queue lock that wgpu-hal hands out for `queue`, and returns its panic
/// message, or `None` if it did not panic.
#[cfg(debug_assertions)]
fn panic_holding_the_lock(queue: &wgpu::Queue, call: impl FnOnce()) -> Option<String> {
    let lock = {
        // SAFETY: Only the lock is taken.
        let hal = unsafe { queue.as_hal::<Vulkan>() }.unwrap();
        hal.queue_lock()
            .expect("a queue created without flags takes a lock")
    };
    let guard = wgpu_hal::vulkan::QueueLockGuard::new(&*lock, super::QUEUE);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call));
    drop(guard);
    result.err().map(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    })
}

/// With lock validation, a holder of the queue lock that calls into wgpu panics at the first lock
/// of wgpu-core's it takes, instead of deadlocking against a submission that holds that lock and
/// waits for the queue lock.
#[cfg(all(wgpu_validate_locks_debug, debug_assertions))]
#[test]
fn a_holder_that_takes_a_lock_of_wgpu_core_panics() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    for adapter in super::adapters(&instance) {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let message = panic_holding_the_lock(&queue, || queue.write_buffer(&buffer, 0, &[1; 4]));
        assert!(
            message
                .as_deref()
                .is_some_and(|message| message.contains("holds a Vulkan queue lock")),
            "{message:?}"
        );
    }
}

/// A holder of the queue lock that stages a semaphore panics, in debug builds, instead of
/// deadlocking against a submission that holds the queue's semaphore lists and waits for the lock.
#[cfg(debug_assertions)]
#[test]
fn a_holder_that_stages_a_semaphore_panics() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    for adapter in super::adapters(&instance) {
        let (_device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        let message = panic_holding_the_lock(&queue, || {
            // SAFETY: Only a semaphore that is never used is staged, and taken back.
            let hal = unsafe { queue.as_hal::<Vulkan>() }.unwrap();
            hal.add_signal_semaphore(vk::Semaphore::null(), None);
        });
        assert!(
            message
                .as_deref()
                .is_some_and(|message| message.contains("holds the queue lock")),
            "{message:?}"
        );
    }
}
